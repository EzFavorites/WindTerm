//! 自动补全引擎（I-auto-completion / AC-01~04）。
//!
//! # 边界（ADR-0014）
//!
//! 纯语义单元：零 UI、零 IO、零线程、零墙钟。历史条目带的时间戳由宿主传入，
//! 近因计算只做算术，不读系统时钟。文件 IO 与屏幕坐标都不在这里。
//!
//! # 为什么只写「后缀」
//!
//! shell 自己的行编辑状态（readline 的光标位置、undo 栈、历史取回）不出现在
//! 输入字节里。要安全地补一个词，唯一不依赖这些隐藏状态的做法是**只发送当前
//! token 尚未输入的那一段**：已输入 `git st`、候选 `git status` → 只写 `atus`。
//! 绝不发退格去「替换当前词」——那会在用户按过 `←` 之后删掉别人的内容。
//!
//! # 候选源与设置页的对应
//!
//! [`Settings`] 的六个布尔分别对应视觉基线 Auto Completion 页的六个勾，出厂值
//! 与截图一致（全未勾）。数值三项也照截图：1 characters / 7 rows / Nowhere。
//! 每个开关都真实过滤一类候选，没有「画成能点却无效」的占位字段。

use std::cmp::Ordering;
use std::collections::{HashMap, HashSet};

use serde::{Deserialize, Serialize};

use crate::completion_words::{spec, PLAIN_COMMANDS, SPECS};

/// 候选来自哪一类词表。与设置页的开关一一对应（子命令归入 Command Name）。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Source {
    /// Command Name 勾：命令名 + 子命令。
    Command,
    /// Command Option 勾：以 `-` 开头的选项。
    Option,
    /// Command Parameter 勾：命令的常见参数值。
    Parameter,
    /// History Commands 勾：命令历史整行。
    History,
    /// Quick Commands 勾：QuickBar 快速命令。
    Quick,
}

/// 一个补全候选。
#[derive(Clone, Debug, PartialEq)]
pub struct Candidate {
    /// 弹层里显示的完整文本，如 `git status`。
    pub display: String,
    /// 真正写进 PTY 的后缀，如 `atus`。
    pub insert: String,
    /// 来源，决定它受哪个设置开关控制。
    pub source: Source,
    /// 排序分数，越大越靠前。
    pub score: f64,
}

/// `Preselect the best match` 下拉的三个选项（照视觉基线）。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum Preselect {
    /// 视觉基线出厂态：不预选任何候选。
    #[default]
    Nowhere,
    /// 预选列表第一项（按当前排序）。
    First,
    /// 预选分数最高项。列表已按分数排好时与 `First` 相同，但语义独立，
    /// 调用方即使传入未排序列表也得到同一个「最佳」。
    Best,
}

/// 补全偏好。**六个勾出厂全开**（与参考设置页出厂态逐项一致）、1 字符触发、
/// 7 行、不预选。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    /// Enable auto completion。
    pub enabled: bool,
    /// Command Name。
    pub commands: bool,
    /// Command Option。
    pub options: bool,
    /// Command Parameter。
    pub parameters: bool,
    /// History Commands。
    pub history: bool,
    /// Quick Commands。
    pub quick: bool,
    /// Start completion after entering（字符数）。
    pub min_chars: usize,
    /// Completion list height（行数）。
    pub list_height: usize,
    /// Preselect the best match。
    pub preselect: Preselect,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            enabled: true,
            commands: true,
            options: true,
            parameters: true,
            history: true,
            quick: true,
            min_chars: 1,
            list_height: 7,
            preselect: Preselect::Nowhere,
        }
    }
}

/// 一次补全调用的全部输入。所有借用都只读，函数不持有状态。
pub struct Input<'a> {
    /// 当前输入行（`InputLine::current()`）。
    pub line: &'a str,
    /// 历史命令 `(整行, 纪元秒)`；时间由宿主注入，本层不读钟。
    pub history: &'a [(&'a str, u64)],
    /// QuickBar 的快速命令文本（实际会写进 PTY 的那串）。
    pub quick: &'a [&'a str],
    /// 补全偏好。
    pub settings: &'a Settings,
    /// 当前纪元秒，仅用于近因衰减。
    pub now: u64,
}

/// 算出按分数降序排列的候选，最多 `settings.list_height` 条。
///
/// 总开关关闭、或当前 token 短于 `min_chars` 时返回空表——调用方据此关闭弹层。
pub fn complete(input: Input<'_>) -> Vec<Candidate> {
    let s = input.settings;
    if !s.enabled {
        return Vec::new();
    }
    let (token, words) = token_and_words(input.line);
    if token.chars().count() < s.min_chars {
        return Vec::new();
    }

    let mut out = Vec::new();
    push_static(&mut out, token, &words, s);
    push_history(&mut out, input.line, input.history, s, input.now);
    push_quick(&mut out, input.line, input.quick, s);

    dedup_and_sort(&mut out);
    out.truncate(s.list_height.max(1));
    out
}

/// 按 `Preselect` 策略给出预选下标。`Nowhere` 表示不高亮任何项。
pub fn preselect_index(candidates: &[Candidate], policy: Preselect) -> Option<usize> {
    if candidates.is_empty() {
        return None;
    }
    match policy {
        Preselect::Nowhere => None,
        Preselect::First => Some(0),
        Preselect::Best => candidates
            .iter()
            .enumerate()
            // 分数相同取靠前的那一项，保证结果确定（max_by 默认取末位）。
            .max_by(|(ia, a), (ib, b)| {
                cmp_score(a.score, b.score).then_with(|| ib.cmp(ia))
            })
            .map(|(i, _)| i),
    }
}

/// 把一行切成 `(当前 token, 之前的词)`。行尾是空白时当前 token 为空。
fn token_and_words(line: &str) -> (&str, Vec<&str>) {
    let trimmed = line.trim_end_matches(char::is_whitespace);
    let mut words: Vec<&str> = trimmed.split_whitespace().collect();
    let token = if trimmed.is_empty() || trimmed.len() != line.len() {
        ""
    } else {
        words.pop().unwrap_or("")
    };
    (token, words)
}

/// 静态词库候选。第一个词给命令名；其后按 `-` 开头与否给选项 / 子命令 / 参数。
fn push_static(out: &mut Vec<Candidate>, token: &str, words: &[&str], s: &Settings) {
    if words.is_empty() {
        if s.commands {
            for spec in SPECS {
                push_word(out, token, spec.name, Source::Command);
            }
            for name in PLAIN_COMMANDS {
                push_word(out, token, name, Source::Command);
            }
        }
        return;
    }

    let head = words[0];
    if token.starts_with('-') {
        if s.options {
            if let Some(spec) = spec(head) {
                for opt in spec.options {
                    push_word(out, token, opt, Source::Option);
                }
            }
        }
        return;
    }

    if s.commands {
        if let Some(spec) = spec(head) {
            for sub in spec.subcommands {
                push_word(out, token, sub, Source::Command);
            }
        }
    }
    if s.parameters {
        if let Some(spec) = spec(head) {
            for param in spec.parameters {
                push_word(out, token, param, Source::Parameter);
            }
        }
    }
}

/// 一个静态词：必须以 token 为前缀，且不能已经完整输入。
fn push_word(out: &mut Vec<Candidate>, token: &str, word: &str, source: Source) {
    if !word.starts_with(token) || word.len() == token.len() {
        return;
    }
    out.push(Candidate {
        display: word.to_string(),
        insert: word[token.len()..].to_string(),
        source,
        score: match source {
            Source::Command => 50.0,
            Source::Option => 42.0,
            Source::Parameter => 36.0,
            Source::History | Source::Quick => 0.0,
        },
    });
}

/// 历史候选：整行必须以当前行为前缀；同命令按频次与近因加权。
fn push_history(
    out: &mut Vec<Candidate>,
    line: &str,
    history: &[(&str, u64)],
    s: &Settings,
    now: u64,
) {
    if !s.history {
        return;
    }
    let mut agg: HashMap<&str, (u32, u64)> = HashMap::new();
    for &(cmd, ts) in history {
        if !cmd.starts_with(line) || cmd.len() == line.len() {
            continue;
        }
        let entry = agg.entry(cmd).or_insert((0, 0));
        entry.0 += 1;
        entry.1 = entry.1.max(ts);
    }
    for (cmd, (freq, latest)) in agg {
        let age = now.saturating_sub(latest);
        out.push(Candidate {
            display: cmd.to_string(),
            insert: cmd[line.len()..].to_string(),
            source: Source::History,
            score: 60.0 + (freq as f64).ln_1p() * 8.0 + 20.0 / (1.0 + age as f64 / 86_400.0),
        });
    }
}

/// QuickBar 候选：与历史同一套「整行前缀 + 只补后缀」规则。
fn push_quick(out: &mut Vec<Candidate>, line: &str, quick: &[&str], s: &Settings) {
    if !s.quick {
        return;
    }
    for &cmd in quick {
        if !cmd.starts_with(line) || cmd.len() == line.len() {
            continue;
        }
        out.push(Candidate {
            display: cmd.to_string(),
            insert: cmd[line.len()..].to_string(),
            source: Source::Quick,
            score: 40.0,
        });
    }
}

/// 按 insert 去重（补进 PTY 的后缀相同就是同一个建议），再按分数降序、
/// 同分按文本升序，保证输出确定。历史里的 `git status` 与词库里的 `status`
/// 都补 `atus`，留分数高的那条，正是「按历史智能推荐」想要的效果。
fn dedup_and_sort(out: &mut Vec<Candidate>) {
    // 先排序再去重：同后缀时留下的是分数最高的那条，而不是先被塞进表的那条。
    out.sort_by(|a, b| {
        cmp_score(a.score, b.score).then_with(|| a.display.cmp(&b.display))
    });
    let mut seen = HashSet::new();
    out.retain(|c| seen.insert(c.insert.clone()));
}

/// `f64` 全序比较；NaN 不去扰动排序。
fn cmp_score(a: f64, b: f64) -> Ordering {
    b.partial_cmp(&a).unwrap_or(Ordering::Equal)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn settings() -> Settings {
        Settings {
            enabled: true,
            commands: true,
            options: true,
            parameters: true,
            history: true,
            quick: true,
            ..Settings::default()
        }
    }

    fn run<'a>(line: &'a str, s: &'a Settings) -> Vec<Candidate> {
        complete(Input {
            line,
            history: &[],
            quick: &[],
            settings: s,
            now: 1_000_000,
        })
    }

    #[test]
    fn factory_defaults_match_reference_screenshot() {
        let s = Settings::default();
        assert!(s.enabled && s.commands && s.options && s.parameters);
        assert!(s.history && s.quick);
        assert_eq!(s.min_chars, 1);
        assert_eq!(s.list_height, 7);
        assert_eq!(s.preselect, Preselect::Nowhere);
    }

    #[test]
    fn disabled_returns_no_candidates() {
        let mut s = Settings::default();
        s.enabled = false;
        assert!(run("git st", &s).is_empty());
    }

    #[test]
    fn insert_is_only_the_missing_suffix() {
        let s = settings();
        let hits = run("git st", &s);
        let status = hits.iter().find(|c| c.display == "status").expect("status 应在候选里");
        assert_eq!(status.insert, "atus", "绝不能提示整词或发退格");
    }

    #[test]
    fn command_names_only_when_switch_on() {
        let mut s = settings();
        s.commands = false;
        assert!(run("gi", &s).is_empty(), "总开关开了但命令勾没开时不给词库候选");
        s.commands = true;
        assert!(!run("gi", &s).is_empty());
    }

    #[test]
    fn subcommands_belong_to_command_name_switch() {
        let mut s = settings();
        assert!(run("git st", &s).iter().any(|c| c.display == "status"));
        s.commands = false;
        assert!(run("git st", &s).is_empty(), "子命令归 Command Name 开关");
    }

    #[test]
    fn options_and_parameters_have_independent_switches() {
        let s = settings();
        assert!(run("git --", &s).iter().any(|c| c.display == "--all"));
        assert!(run("git or", &s).iter().any(|c| c.display == "origin"));

        let mut no_opt = s.clone();
        no_opt.options = false;
        assert!(run("git --", &no_opt).is_empty());
        let mut no_param = s;
        no_param.parameters = false;
        assert!(run("git or", &no_param).is_empty());
    }

    #[test]
    fn utf8_token_is_char_counted_and_sliced_safely() {
        assert_eq!(token_and_words("echo 你好"), ("你好", vec!["echo"]));
        let s = settings();
        let hits = run("echo 你好", &s);
        assert!(hits.iter().all(|c| c.insert.is_char_boundary(0)));
    }

    #[test]
    fn min_chars_gates_the_popup() {
        let mut s = settings();
        s.min_chars = 3;
        assert!(run("git st", &s).is_empty());
        assert!(!run("git sta", &s).is_empty());
    }

    #[test]
    fn trailing_space_means_no_token() {
        assert_eq!(token_and_words("git "), ("", vec!["git"]));
        assert_eq!(token_and_words("git st"), ("st", vec!["git"]));
        assert_eq!(token_and_words(""), ("", vec![]));
    }

    #[test]
    fn history_requires_whole_line_prefix_and_ranks_by_frequency() {
        let s = settings();
        let h = [
            ("git status", 1_000_000),
            ("git status", 999_900),
            ("git push", 500_000),
            ("ls -la", 1_000_000),
        ];
        let hits = complete(Input {
            line: "git st",
            history: &h,
            quick: &[],
            settings: &s,
            now: 1_000_100,
        });
        // 词库还会给 status/stash 两条；同 insert 的 "status" 被历史那条
        // （分数更高）去重吸收，剩下的两条是「历史 git status」+「词库 stash」。
        assert_eq!(hits.len(), 2);
        assert_eq!(hits[0].display, "git status");
        assert_eq!(hits[0].insert, "atus");
        assert_eq!(hits[0].source, Source::History);
    }

    #[test]
    fn history_switch_off_removes_history_candidates() {
        let mut s = settings();
        let h = [("git status", 1_000_000)];
        s.history = false;
        let hits = complete(Input {
            line: "git st",
            history: &h,
            quick: &[],
            settings: &s,
            now: 1_000_000,
        });
        assert!(hits.iter().all(|c| c.source != Source::History));
    }

    #[test]
    fn quick_commands_are_inserted_as_suffix() {
        let mut s = settings();
        s.commands = false; // 隔离词库源，只留 QuickBar
        let quick = ["git fetch --all", "git status --short"];
        let hits = complete(Input {
            line: "git f",
            history: &[],
            quick: &quick,
            settings: &s,
            now: 0,
        });
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].insert, "etch --all");
        assert_eq!(hits[0].source, Source::Quick);
    }

    #[test]
    fn candidates_are_deduped_by_insert() {
        let s = settings();
        let quick = ["git status"];
        let hits = complete(Input {
            line: "git st",
            history: &[("git status", 1)],
            quick: &quick,
            settings: &s,
            now: 1,
        });
        assert_eq!(hits.iter().filter(|c| c.insert == "atus").count(), 1);
    }

    #[test]
    fn list_height_truncates() {
        let mut s = settings();
        let all = run("git s", &s).len();
        assert!(all > 2, "前提：候选数要超过上限，实测 {all}");
        s.list_height = 2;
        assert_eq!(run("git s", &s).len(), 2);
    }

    #[test]
    fn preselect_policies() {
        let s = settings();
        let hits = run("git st", &s);
        assert_eq!(preselect_index(&hits, Preselect::Nowhere), None);
        assert_eq!(preselect_index(&hits, Preselect::First), Some(0));
        assert_eq!(preselect_index(&hits, Preselect::Best), Some(0));
        assert_eq!(preselect_index(&[], Preselect::First), None);
    }

    #[test]
    fn complete_word_is_not_offered_again() {
        let s = settings();
        let hits = run("git", &s);
        assert!(hits.iter().all(|c| c.display != "git"), "已完整输入就不再提示自己");
    }
}

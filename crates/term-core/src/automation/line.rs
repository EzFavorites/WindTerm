//! 行级清理：**结果后处理，不影响前级**（2026-11-19 用户裁定）。
//!
//! 取数动词的执行顺序固定为：
//!
//! ```text
//! ① 在原始清洗文本上匹配（与 KeepAll 逐字节同一条路）
//! ② 消费到命中结尾 / 排水点（与 KeepAll 完全一致）
//! ③ 对「本次返回文本」按策略清理行 → 返回
//! ```
//!
//! 所以这里**没有状态、没有视图、没有偏移映射**——只是一个纯函数。
//! 旧的 `filter_view` / `Seg` / `raw_offset_of` / `mid_line` 整套机制都不需要：
//! grep 不碰游标，"被丢的行卡住游标"的缺陷按构造消失。

use crate::automation::pattern::Pattern;

/// 行级清理策略。`KeepAll` 是缺省，零成本直通。
#[derive(Clone, Debug, Default)]
pub enum LinePolicy {
    #[default]
    KeepAll,
    /// 留匹配行（`grep`）。
    KeepIf(Pattern),
    /// 丢匹配行（`grep -v`）。
    DropIf(Pattern),
}

impl LinePolicy {
    /// 对**行内容**（已去掉 `\r\n`）判定是否保留。
    pub fn admits(&self, content: &str) -> bool {
        match self {
            Self::KeepAll => true,
            // grep 问的是"这行有没有命中"，含 `^` / 空模式这类零宽命中。
            // 不能用 `find`（它要"结束后于 after_end"，零宽永远判不出）。
            Self::KeepIf(p) => p.matches(content),
            Self::DropIf(p) => !p.matches(content),
        }
    }
}

/// 对一次返回文本按行清理。纯函数，可对着任意夹具单测。
///
/// 最后一段若不以 `\n` 结尾则**透传、不判定**——它可能是还没写完的半行
/// （`noise` + `2\n` = `noise2\n`），提前判会把没写完的行误丢。
pub fn apply_lines(text: &str, policy: &LinePolicy) -> String {
    if matches!(policy, LinePolicy::KeepAll) {
        return text.to_owned();
    }
    let mut out = String::with_capacity(text.len());
    let mut lines = text.split_inclusive('\n').peekable();
    while let Some(line) = lines.next() {
        if !line.ends_with('\n') && lines.peek().is_none() {
            out.push_str(line);
            continue;
        }
        let content = line.strip_suffix('\n').unwrap_or(line);
        let content = content.strip_suffix('\r').unwrap_or(content);
        if policy.admits(content) {
            out.push_str(line);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::automation::pattern::{Pattern, Syntax};

    fn keep(p: &str) -> LinePolicy {
        LinePolicy::KeepIf(Pattern::compile(p, Syntax::Regex, true).unwrap())
    }
    fn drop(p: &str) -> LinePolicy {
        LinePolicy::DropIf(Pattern::compile(p, Syntax::Regex, true).unwrap())
    }

    #[test]
    fn keep_all_returns_the_text_unchanged() {
        let text = "a\nnoise\nb";
        assert_eq!(apply_lines(text, &LinePolicy::KeepAll), text);
    }

    #[test]
    fn grep_keeps_only_matching_lines() {
        assert_eq!(apply_lines("keep1\ndrop\nkeep2\n", &keep("keep")), "keep1\nkeep2\n");
    }

    #[test]
    fn grep_v_drops_matching_lines() {
        assert_eq!(
            apply_lines("noise\nkeep\nnoise2\n", &drop("^noise")),
            "keep\n"
        );
    }

    #[test]
    fn whole_line_matching_strips_crlf_like_grep() {
        let p = keep("^noise$");
        assert_eq!(apply_lines("noise\nnoise\r\nnoise x\n", &p), "noise\nnoise\r\n");
    }

    #[test]
    fn the_last_incomplete_line_is_passed_through_unjudged() {
        // "noise" 是没写完的半行：不能按整行判掉。
        assert_eq!(apply_lines("noise\nnoise", &drop("^noise")), "noise");
        // 补全后它成为完整行，下一次调用就会被判掉。
        assert_eq!(apply_lines("noise2\n", &drop("^noise")), "");
    }

    #[test]
    fn empty_text_stays_empty() {
        assert_eq!(apply_lines("", &drop(".*")), "");
    }

    #[test]
    fn extreme_empty_pattern_keeps_every_line() {
        assert_eq!(apply_lines("a\nb\nc\n", &keep("")), "a\nb\nc\n");
    }

    #[test]
    fn extreme_anchor_matches_every_line() {
        assert_eq!(apply_lines("x\ny\n", &keep("^")), "x\ny\n");
    }

    #[test]
    fn extreme_grep_v_dot_star_drops_every_complete_line() {
        assert_eq!(apply_lines("a\nb\nc\n", &drop(".*")), "");
    }

    #[test]
    fn cleaning_does_not_touch_the_input_string() {
        let text = String::from("noise\nkeep\n");
        let out = apply_lines(&text, &drop("^noise"));
        assert_eq!(text, "noise\nkeep\n");
        assert_eq!(out, "keep\n");
    }
}

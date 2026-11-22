//! Trigger：**事件 → 匹配 → 动作** 的求值内核（roadmap 的语义层）。
//!
//! 企业场景的典型用法：登录后自动 `cd /app`、看到 `password:` 自动填、看到
//! `panic` 高亮告警。这一层只回答「命中没有、命中了要做什么」，**不执行任何东西**：
//! `SendText` / `Beep` / `RunCommand` 都只是 `Hit.actions` 里的数据，与
//! `Effect` 同一条纪律。
//!
//! 三个设计约束：
//!
//! - **零墙钟**：`now` 由宿主注入（ADR-0014），冷却与空闲判定都用它；
//! - **有界内存**：输出是流式分块来的，每条规则只保留上一轮末尾
//!   `pattern` 字符数 − 1 个字符，绝不把整段历史留住；
//! - **运行期不 panic**：畸形模式在装载时由 [`validate`] 拒绝，即使有人绕过它
//!   直接喂规则，匹配失败也只是「这一轮没命中」。
//!
//! 已知边界：带量词的模式（`panic.*`）如果一次匹配横跨的字符数超过
//! `pattern` 自身长度，跨 chunk 的**后半段**可能接不上——保内存有界优先，
//! 这类模式通常在第一个 chunk 里就已经命中。

use std::collections::{HashMap, HashSet};

use crate::trigger_regex;

/// 数据层（TriggerOn / Rule / TriggerAction / Hit）下放到 `term-core`（见
/// `term_core::trigger`）：规则要落盘进 `Prefs`（`term-profile` 拥有），而
/// `term-profile` 按 ADR-0014 不得依赖 `term-app`（应用层）。求值内核只消费这些
/// 类型，故在这里原样再导出——`term_app::trigger::Rule` 等调用路径全部不变，
/// `term-profile` 则直接吃 `term_core::trigger::Rule`，分层不被破坏。
pub use term_core::trigger::{Hit, Rule, TriggerAction, TriggerOn};

/// 带游标的匹配器：跨 chunk 的尾巴、冷却表、一次性事件表都在这里。
///
/// 与会话同生命周期：会话结束调 [`Matcher::reset`]，下一次会话从干净状态开始。
#[derive(Debug, Default)]
pub struct Matcher {
    /// 每条规则保留的「未闭合尾巴」（字符，不是字节）。
    tails: HashMap<String, Vec<char>>,
    /// 上次触发时刻，只在 `cooldown_secs > 0` 时参与判定。
    last_fired: HashMap<String, u64>,
    /// 最近一次输出的注入时刻（`Idle` 用）。
    last_output_at: u64,
    connect_fired: HashSet<String>,
    disconnect_fired: HashSet<String>,
    idle_fired: HashSet<String>,
    before_connect_fired: HashSet<String>,
    before_disconnect_fired: HashSet<String>,
    /// 等提示符 / 换行 / Send Later 的命中。
    queue: crate::trigger_queue::TriggerQueue,
}

impl Matcher {
    pub fn new() -> Self {
        Self::default()
    }

    /// 把一批命中按 Send Condition / Send Later 排队；立即放行的原样返回。
    pub fn schedule(&mut self, hits: Vec<Hit>, rules: &[Rule], now: u64) -> Vec<Hit> {
        self.queue.schedule(hits, rules, now)
    }

    /// 提示符到达：放行等 Prompt 的命中，顺带收下已到期的 Send Later。
    pub fn on_prompt(&mut self, now: u64) -> Vec<Hit> {
        let mut out = self.queue.on_event(term_core::trigger_schedule::Gate::Prompt);
        out.extend(self.queue.due(now));
        out
    }

    /// 换行到达：放行等 LineBreak 的命中，顺带收下已到期的 Send Later。
    pub fn on_line_break(&mut self, now: u64) -> Vec<Hit> {
        let mut out = self.queue.on_event(term_core::trigger_schedule::Gate::LineBreak);
        out.extend(self.queue.due(now));
        out
    }

    /// 仅检查 Send Later 是否到期（宿主空闲时也要调用，不能只靠输出事件）。
    pub fn due(&mut self, now: u64) -> Vec<Hit> {
        self.queue.due(now)
    }

    pub fn pending_count(&self) -> usize {
        self.queue.len()
    }

    /// 喂一段输出；返回本轮命中的规则（按 `rules` 顺序，保证稳定）。
    ///
    /// 每条规则每轮最多一条 `Hit`：同一 chunk 里出现多次也只报最早那次——
    /// 动作本身是幂等的意图，重复报只会让 `SendLine` 发两遍。
    pub fn feed(&mut self, rules: &[Rule], chunk: &str, now: u64) -> Vec<Hit> {
        self.feed_for(rules, chunk, now, None)
    }

    /// 带标签上下文的 feed：规则 `Sessions` 页的目标表在这里生效。
    pub fn feed_for(
        &mut self,
        rules: &[Rule],
        chunk: &str,
        now: u64,
        target: Option<term_core::trigger_target::Target<'_>>,
    ) -> Vec<Hit> {
        if chunk.is_empty() {
            return Vec::new();
        }
        self.last_output_at = now;
        self.idle_fired.clear();
        let mut hits = Vec::new();
        for rule in rules {
            if !rule.enabled || rule.on != TriggerOn::Output || rule.pattern.is_empty() {
                continue;
            }
            if !term_core::trigger_target::rule_matches(rule, target) {
                continue;
            }
            let mut window = self.tails.remove(&rule.id).unwrap_or_default();
            let scanned = window.len();
            window.extend(chunk.chars());
            if let Some((start, end)) = find_in(rule, &window, scanned) {
                if self.allow(rule, now) {
                    self.last_fired.insert(rule.id.clone(), now);
                    hits.push(Hit::new(rule, window[start..end].iter().collect()));
                }
            }
            let keep = rule.pattern.chars().count().saturating_sub(1);
            let from = window.len().saturating_sub(keep);
            self.tails.insert(rule.id.clone(), window[from..].to_vec());
        }
        // 规则被删掉后，尾巴也别留着（否则长时间运行会攒出孤儿表项）。
        self.tails
            .retain(|id, _| rules.iter().any(|rule| rule.id == *id));
        hits
    }

    /// 会话级事件（`Connect` / `Disconnect` / `Idle`）。
    ///
    /// `Idle` 只在「距 `last_output_at` 至少 `idle_secs` 秒」且本段空闲还没报过时
    /// 命中；下次 `feed` 一有输出就重新计。
    pub fn event(&mut self, rules: &[Rule], on: TriggerOn, now: u64) -> Vec<Hit> {
        self.event_for(rules, on, now, None)
    }

    /// 带标签上下文的事件求值；目标过滤与 `feed_for` 同一套判据。
    pub fn event_for(
        &mut self,
        rules: &[Rule],
        on: TriggerOn,
        now: u64,
        target: Option<term_core::trigger_target::Target<'_>>,
    ) -> Vec<Hit> {
        let mut hits = Vec::new();
        for rule in rules {
            if !rule.enabled || rule.on != on {
                continue;
            }
            if !term_core::trigger_target::rule_matches(rule, target) {
                continue;
            }
            if on == TriggerOn::Idle && now.saturating_sub(self.last_output_at) < rule.idle_secs {
                continue;
            }
            if !self.allow(rule, now) {
                continue;
            }
            let once = match on {
                TriggerOn::Connect => &mut self.connect_fired,
                TriggerOn::Disconnect => &mut self.disconnect_fired,
                TriggerOn::Idle => &mut self.idle_fired,
                TriggerOn::BeforeConnect => &mut self.before_connect_fired,
                TriggerOn::BeforeDisconnect => &mut self.before_disconnect_fired,
                // `Output` 走 `feed`，事件通道不重复实现。
                TriggerOn::Output => continue,
            };
            if !once.insert(rule.id.clone()) {
                continue;
            }
            self.last_fired.insert(rule.id.clone(), now);
            hits.push(Hit::new(rule, String::new()));
        }
        hits
    }

    /// 会话结束：清掉尾巴、冷却表与一次性事件表。
    pub fn reset(&mut self) {
        self.tails.clear();
        self.last_fired.clear();
        self.last_output_at = 0;
        self.connect_fired.clear();
        self.disconnect_fired.clear();
        self.idle_fired.clear();
        self.before_connect_fired.clear();
        self.before_disconnect_fired.clear();
    }

    fn allow(&self, rule: &Rule, now: u64) -> bool {
        if rule.cooldown_secs == 0 {
            return true;
        }
        match self.last_fired.get(&rule.id) {
            // 窗口边界算「可以触发」：恰好隔满 cooldown 秒就是合法的最小间隔。
            // `now < last` 说明宿主时钟被重置过，别把规则永久卡死，直接放行。
            Some(&last) => now < last || now - last >= rule.cooldown_secs,
            None => true,
        }
    }
}

/// 装载期校验：`literal = false` 的规则必须先过这一关。
///
/// `literal = true` 的模式永远合法（没有元字符），返回 `Ok(())`。
pub fn validate(pattern: &str) -> Result<(), String> {
    trigger_regex::parse(pattern).map(|_| ())
}

fn find_in(rule: &Rule, window: &[char], scanned: usize) -> Option<(usize, usize)> {
    if rule.literal {
        let needle: Vec<char> = rule.pattern.chars().collect();
        find_literal(window, &needle, rule.case_sensitive, scanned)
    } else {
        let pieces = trigger_regex::parse(&rule.pattern).ok()?;
        trigger_regex::search(
            &pieces,
            window,
            rule.case_sensitive,
            scanned,
            rule.pattern.chars().count(),
        )
    }
}

/// 纯文本包含：返回最早一个「结束位置在旧前缀之后」的出现。
fn find_literal(
    window: &[char],
    needle: &[char],
    case_sensitive: bool,
    scanned: usize,
) -> Option<(usize, usize)> {
    if needle.is_empty() || needle.len() > window.len() {
        return None;
    }
    for (start, slice) in window.windows(needle.len()).enumerate() {
        let end = start + needle.len();
        let hit = slice
            .iter()
            .zip(needle)
            .all(|(a, b)| trigger_regex::char_eq(*a, *b, case_sensitive));
        if hit && end > scanned {
            return Some((start, end));
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rule(id: &str, on: TriggerOn, pattern: &str) -> Rule {
        Rule {
            id: id.to_string(),
            // 测试夹具不关心显示名——空串正是「旧配置读进来」的形态。
            name: String::new(),
            description: String::new(),
            // kind / config 不参与求值，取默认值即可。
            kind: term_core::trigger::TriggerKind::default(),
            config: term_core::trigger::RuleConfig::default(),
            on,
            pattern: pattern.to_string(),
            literal: true,
            case_sensitive: true,
            cooldown_secs: 0,
            idle_secs: 0,
            actions: vec![TriggerAction::Highlight],
            enabled: true,
        }
    }

    fn output(id: &str, pattern: &str) -> Rule {
        rule(id, TriggerOn::Output, pattern)
    }

    #[test]
    fn a_match_may_span_two_chunks() {
        let rules = vec![output("login", "password:")];
        let mut m = Matcher::new();
        assert!(m.feed(&rules, "login: pass", 1).is_empty());
        let hits = m.feed(&rules, "word: ", 2);
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].rule_id, "login");
        assert_eq!(hits[0].matched, "password:");
    }

    #[test]
    fn a_regex_match_may_span_two_chunks() {
        let mut r = output("p", "pass.*:");
        r.literal = false;
        let rules = vec![r];
        let mut m = Matcher::new();
        assert!(m.feed(&rules, "login: pass", 1).is_empty());
        let hits = m.feed(&rules, "word:", 2);
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].matched, "password:");
    }

    #[test]
    fn already_scanned_text_does_not_fire_twice() {
        let rules = vec![output("p", "panic")];
        let mut m = Matcher::new();
        assert_eq!(m.feed(&rules, "panic", 1).len(), 1);
        // 尾巴里还留着 "anic"，但没有新的 "panic"。
        assert!(m.feed(&rules, "!", 2).is_empty());
    }

    #[test]
    fn no_cooldown_fires_on_every_chunk() {
        let rules = vec![output("p", "hi")];
        let mut m = Matcher::new();
        assert_eq!(m.feed(&rules, "hi", 1).len(), 1);
        assert_eq!(m.feed(&rules, "hi", 2).len(), 1);
        assert_eq!(m.feed(&rules, "hi", 3).len(), 1);
    }

    #[test]
    fn cooldown_suppresses_until_the_boundary() {
        let mut r = output("p", "hi");
        r.cooldown_secs = 10;
        let rules = vec![r];
        let mut m = Matcher::new();
        assert_eq!(m.feed(&rules, "hi", 100).len(), 1);
        assert!(m.feed(&rules, "hi", 105).is_empty(), "窗口内应被冷却");
        assert!(m.feed(&rules, "hi", 109).is_empty());
        assert_eq!(m.feed(&rules, "hi", 110).len(), 1, "恰好隔满 10 秒应触发");
        assert!(m.feed(&rules, "hi", 111).is_empty(), "新的冷却窗口重新开始");
    }

    #[test]
    fn zero_cooldown_never_suppresses() {
        let mut r = output("p", "x");
        r.cooldown_secs = 0;
        let rules = vec![r];
        let mut m = Matcher::new();
        assert_eq!(m.feed(&rules, "x", 7).len(), 1);
        assert_eq!(
            m.feed(&rules, "x", 7).len(),
            1,
            "同一时刻也不该被 0 冷却拦住"
        );
    }

    #[test]
    fn case_sensitivity_is_honoured() {
        let sensitive = vec![output("s", "ERROR")];
        let mut m = Matcher::new();
        assert!(m.feed(&sensitive, "error", 1).is_empty());
        assert_eq!(m.feed(&sensitive, "ERROR", 2).len(), 1);

        let mut loose = output("l", "ERROR");
        loose.case_sensitive = false;
        let rules = vec![loose];
        let mut m = Matcher::new();
        assert_eq!(m.feed(&rules, "an error happened", 1).len(), 1);
    }

    #[test]
    fn regex_case_folding_is_honoured() {
        let mut r = output("p", "panic");
        r.literal = false;
        r.case_sensitive = false;
        let rules = vec![r];
        let mut m = Matcher::new();
        assert_eq!(m.feed(&rules, "PANIC: core dumped", 1).len(), 1);
    }

    #[test]
    fn idle_fires_once_per_idle_period() {
        let mut r = rule("idle", TriggerOn::Idle, "");
        r.idle_secs = 30;
        let rules = vec![r];
        let mut m = Matcher::new();
        m.feed(&rules, "banner", 100);
        assert!(
            m.event(&rules, TriggerOn::Idle, 129).is_empty(),
            "未满 30 秒"
        );
        assert_eq!(m.event(&rules, TriggerOn::Idle, 130).len(), 1);
        assert!(
            m.event(&rules, TriggerOn::Idle, 400).is_empty(),
            "同一段空闲只报一次"
        );
        // 有新输出后进入下一段空闲。
        m.feed(&rules, "more", 410);
        assert!(m.event(&rules, TriggerOn::Idle, 420).is_empty());
        assert_eq!(m.event(&rules, TriggerOn::Idle, 440).len(), 1);
    }

    #[test]
    fn connect_and_disconnect_fire_once_per_session() {
        let rules = vec![
            rule("up", TriggerOn::Connect, ""),
            rule("down", TriggerOn::Disconnect, ""),
        ];
        let mut m = Matcher::new();
        assert_eq!(m.event(&rules, TriggerOn::Connect, 1).len(), 1);
        assert!(m.event(&rules, TriggerOn::Connect, 2).is_empty());
        assert_eq!(m.event(&rules, TriggerOn::Disconnect, 3).len(), 1);
        assert!(m.event(&rules, TriggerOn::Disconnect, 4).is_empty());
        m.reset();
        assert_eq!(
            m.event(&rules, TriggerOn::Connect, 5).len(),
            1,
            "新会话重新计"
        );
        assert_eq!(m.event(&rules, TriggerOn::Disconnect, 6).len(), 1);
    }

    #[test]
    fn disabled_rules_never_fire() {
        let mut r = output("p", "x");
        r.enabled = false;
        let rules = vec![r];
        let mut m = Matcher::new();
        assert!(m.feed(&rules, "x", 1).is_empty());
        assert!(m.event(&rules, TriggerOn::Idle, 100).is_empty());
    }

    #[test]
    fn malformed_patterns_are_rejected_by_validate() {
        for bad in ["", "a(", "a|b", "[oops", "*x", "a{2}"] {
            assert!(validate(bad).is_err(), "{bad:?} 应被拒");
        }
        for good in ["abc", "^ok$", "[0-9]+\\.[0-9]+"] {
            assert!(validate(good).is_ok(), "{good:?} 应可编译");
        }
    }

    #[test]
    fn malformed_pattern_never_panics_at_runtime() {
        let mut r = output("bad", "(");
        r.literal = false;
        let rules = vec![r];
        let mut m = Matcher::new();
        assert!(m.feed(&rules, "whatever (", 1).is_empty());
    }

    #[test]
    fn catastrophic_backtracking_is_abandoned_quickly() {
        let mut r = output("evil", "a*a*a*a*a*a*b");
        r.literal = false;
        let rules = vec![r];
        let text = format!("{}c", "a".repeat(120));
        let mut m = Matcher::new();
        let started = std::time::Instant::now();
        assert!(m.feed(&rules, &text, 1).is_empty());
        assert!(
            started.elapsed().as_millis() < 500,
            "步数上限应让回溯快速放弃，实际 {:?}",
            started.elapsed()
        );
    }

    #[test]
    fn literal_mode_treats_metacharacters_as_plain_text() {
        let rules = vec![output("dot", "a.b")];
        let mut m = Matcher::new();
        assert!(
            m.feed(&rules, "axb", 1).is_empty(),
            "字面量模式不该把 . 当通配"
        );
        assert_eq!(m.feed(&rules, "a.b", 2).len(), 1);
    }

    #[test]
    fn regex_supports_anchors_and_classes() {
        let mut r = output("err", "^ERR[0-9]+:");
        r.literal = false;
        let rules = vec![r];
        let mut m = Matcher::new();
        assert_eq!(m.feed(&rules, "ERR42: bad", 1).len(), 1);
        let mut m = Matcher::new();
        assert!(
            m.feed(&rules, "ok ERR42:", 1).is_empty(),
            "^ 锚在本轮窗口开头"
        );
    }

    #[test]
    fn hits_follow_rule_order_and_carry_actions() {
        let mut first = output("first", "boom");
        first.actions = vec![TriggerAction::SendLine("cd /app".to_string())];
        let second = output("second", "oom");
        let rules = vec![first, second];
        let mut m = Matcher::new();
        let hits = m.feed(&rules, "boom", 1);
        assert_eq!(hits.len(), 2);
        assert_eq!(hits[0].rule_id, "first");
        assert_eq!(hits[1].rule_id, "second");
        assert_eq!(
            hits[0].actions,
            vec![TriggerAction::SendLine("cd /app".into())]
        );
    }

    #[test]
    fn reset_clears_cooldown_and_tails() {
        let mut r = output("p", "hi");
        r.cooldown_secs = 60;
        let rules = vec![r];
        let mut m = Matcher::new();
        assert_eq!(m.feed(&rules, "hi", 10).len(), 1);
        assert!(m.feed(&rules, "hi", 11).is_empty());
        m.reset();
        assert_eq!(
            m.feed(&rules, "hi", 12).len(),
            1,
            "reset 后冷却与尾巴都应清空"
        );
    }

    #[test]
    fn unicode_is_counted_in_chars_not_bytes() {
        let rules = vec![output("cn", "密码")];
        let mut m = Matcher::new();
        assert!(m.feed(&rules, "输入", 1).is_empty());
        assert_eq!(m.feed(&rules, "密码", 2).len(), 1);
    }

    #[test]
    fn output_and_events_do_not_cross_trigger() {
        let rules = vec![
            output("o", "x"),
            rule("c", TriggerOn::Connect, ""),
            rule("i", TriggerOn::Idle, ""),
        ];
        let mut m = Matcher::new();
        let hits = m.event(&rules, TriggerOn::Connect, 1);
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].rule_id, "c");
        // `feed` 不该触发 Connect/Idle；`event(Output)` 是空操作。
        let mut m = Matcher::new();
        assert_eq!(m.feed(&rules, "x", 1).len(), 1);
        assert!(m.event(&rules, TriggerOn::Output, 2).is_empty());
    }
}

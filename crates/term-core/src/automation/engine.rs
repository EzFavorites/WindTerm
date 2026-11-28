//! 规则求值与节流。
//!
//! 本模块是纯计算：`Signal` 与注入的 `Ticks` 进来，`Effect` 出去。它不读时钟、
//! 不做 IO、不执行动作——真正的副作用只发生在 apps 层的 dispatcher。
//!
//! 执行顺序固定为：`enabled` → `Filter` → `When` → `Guard` → 记账 → `Effect`。
//! 四道判定都是"要不要触发"，没有一道会改信号或缓冲。

use std::collections::HashMap;

use crate::automation::pattern::PatternError;
use crate::automation::rule::{Effect, OnceScope, Rule, RuleId};
use crate::automation::signal::{Signal, Target, Ticks, Verdict};

/// 一条"编译好"的规则。
///
/// `Rule::when` 里的 `Pattern` 在构造 / serde 反序列化时就已编译，所以这里
/// **不再持有第二份 `CompiledWhen`**——双份表示必然漂移，也违背"单一真相"。
/// 保留这个包装类型是为了给 `rules` 一个不随规则结构变化而变的名字。
#[derive(Clone, Debug)]
pub struct CompiledRule {
    pub rule: Rule,
}

/// 每条规则的运行态。整个求值器只有这一个 map 需要 `reset`。
#[derive(Clone, Debug, Default)]
pub struct RuleState {
    /// 上次触发的时刻，供 `cooldown` 判定。
    last_fired: Option<Ticks>,
    /// `OnceScope::Session` / `Connection` 的一次性标记。
    fired_once: bool,
}

/// 纯求值器：输入信号与目标，输出动作意图。
#[derive(Clone, Debug, Default)]
pub struct Evaluator {
    rules: Vec<CompiledRule>,
    state: HashMap<RuleId, RuleState>,
}

impl Evaluator {
    pub fn new() -> Self {
        Self::default()
    }

    /// 换一组规则。`Pattern` 已在 `Rule` 内编译好，不会产生新的编译错误；
    /// 返回 `Vec<PatternError>` 是为保持"编辑器标红"的接口形状稳定，当前恒为空。
    ///
    /// 不在新规则集里的旧状态会被清掉，仍存在的规则状态保留（微调规则不丢
    /// 触发历史）。要清全部运行态请显式调 [`Self::reset`]。
    pub fn reload(&mut self, rules: &[Rule]) -> Vec<PatternError> {
        self.rules = rules
            .iter()
            .cloned()
            .map(|rule| CompiledRule { rule })
            .collect();
        self.state
            .retain(|id, _| rules.iter().any(|r| &r.id == id));
        Vec::new()
    }

    pub fn rules(&self) -> &[CompiledRule] {
        &self.rules
    }

    pub fn state_len(&self) -> usize {
        self.state.len()
    }

    /// 对一条信号求值。
    ///
    /// 契约：每条规则一次最多触发一轮；一轮里每个 `action` 产一条 `Effect`，
    /// 同一轮不会因为模式在文本里出现多次而重复报（"只报最早那次"）。
    pub fn eval(&mut self, signal: &Signal, target: &Target) -> Vec<Effect> {
        // 连接事件先重置 Connection 一次性标记，让连接事件本身也能触发
        // once=Connection 的规则（顺序颠倒会让它永远被上一连接挡住）。
        if matches!(
            signal,
            Signal::Connected { .. } | Signal::BeforeConnect { .. }
        ) {
            for cr in &self.rules {
                if cr.rule.guard.once == OnceScope::Connection {
                    if let Some(st) = self.state.get_mut(&cr.rule.id) {
                        st.fired_once = false;
                    }
                }
            }
        }

        let now = signal.at();
        let mut out = Vec::new();
        // 拆字段借，避免"遍历规则"与"改状态"争用整个 `self`。
        let Self { rules, state } = self;
        for cr in rules.iter() {
            let r = &cr.rule;
            // ① 启用 + ② 目标过滤
            if !r.enabled || !r.filter.admits(target) {
                continue;
            }
            // ③ 关心哪种信号
            let span = match r.when.evaluate(signal) {
                Verdict::Yes(span) => span,
                Verdict::No => continue,
            };
            // ④ 节流：once 与 cooldown
            let st = state.entry(r.id.clone()).or_default();
            if r.guard.once != OnceScope::Never && st.fired_once {
                continue;
            }
            if let (Some(window), Some(now)) = (r.guard.cooldown, now) {
                if let Some(prev) = st.last_fired {
                    if now.elapsed_since(prev) < window {
                        continue;
                    }
                }
            }
            // ⑤ 记账：只在真的触发时推进
            if let Some(now) = now {
                st.last_fired = Some(now);
            }
            if r.guard.once != OnceScope::Never {
                st.fired_once = true;
            }
            // ⑥ 产出意图。span 对所有 action 相同（同一次命中）。
            for action in &r.actions {
                out.push(Effect {
                    rule: r.id.clone(),
                    action: action.clone(),
                    span,
                });
            }
        }
        out
    }

    /// 只清运行态，规则保留。
    pub fn reset(&mut self) {
        self.state.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::automation::pattern::{Pattern, Span, Syntax};
    use crate::automation::rule::{Action, Filter, Guard};
    use crate::automation::signal::{CloseReason, When};
    use std::time::Duration;

    fn pattern(s: &str) -> Pattern {
        Pattern::compile(s, Syntax::Literal, true).unwrap()
    }

    fn action() -> Action {
        Action::SessionInvocation {
            method: "write".into(),
            params: serde_json::json!({ "session": "s", "text": "x" }),
        }
    }

    fn rule(id: &str, when: When) -> Rule {
        Rule {
            id: id.into(),
            name: String::new(),
            enabled: true,
            when,
            filter: Filter::default(),
            guard: Guard::default(),
            actions: vec![action()],
        }
    }

    fn output(text: &str, ms: u64) -> Signal<'_> {
        Signal::Output {
            stream: "s",
            text,
            at: Ticks::from_millis(ms),
        }
    }

    fn connected(ms: u64) -> Signal<'static> {
        Signal::Connected {
            stream: "s",
            at: Ticks::from_millis(ms),
        }
    }

    #[test]
    fn reload_loads_rules_without_new_compile_errors() {
        let mut ev = Evaluator::new();
        let errs = ev.reload(&[rule("r1", When::Output(pattern("error")))]);
        assert!(errs.is_empty());
        assert_eq!(ev.rules().len(), 1);
        assert_eq!(ev.state_len(), 0, "加载规则不该预先造运行态");
    }

    #[test]
    fn disabled_rule_and_filtered_target_are_skipped_before_matching() {
        let mut ev = Evaluator::new();
        let mut r = rule("off", When::Output(pattern("x")));
        r.enabled = false;
        let mut other = rule("other", When::Output(pattern("x")));
        other.filter.sessions = vec!["s2".into()];
        ev.reload(&[r, other]);
        let t = Target {
            system: "linux".into(),
            session: "s1".into(),
        };
        assert!(ev.eval(&output("x", 0), &t).is_empty());
    }

    #[test]
    fn a_hit_produces_one_effect_per_action_with_the_same_span() {
        let mut r = rule("r1", When::Output(pattern("error")));
        r.actions.push(action());
        let mut ev = Evaluator::new();
        ev.reload(&[r]);
        let effects = ev.eval(&output("kernel error", 0), &Target::default());
        assert_eq!(effects.len(), 2, "一轮里每个 action 一条");
        for e in &effects {
            assert_eq!(e.rule, "r1");
            assert_eq!(e.span, Some(Span { start: 7, end: 12 }));
        }
    }

    #[test]
    fn when_mismatch_produces_nothing() {
        let mut ev = Evaluator::new();
        ev.reload(&[rule("r1", When::Output(pattern("error")))]);
        assert!(ev.eval(&output("all good", 0), &Target::default()).is_empty());
        // 文本规则也不该被连接事件触发。
        assert!(ev.eval(&connected(0), &Target::default()).is_empty());
    }

    #[test]
    fn cooldown_rejects_too_soon_and_accepts_exactly_at_the_boundary() {
        let mut r = rule("r1", When::Output(pattern("x")));
        r.guard.cooldown = Some(Duration::from_millis(100));
        let mut ev = Evaluator::new();
        ev.reload(&[r]);
        let t = Target::default();
        assert_eq!(ev.eval(&output("x", 0), &t).len(), 1, "首次直接触发");
        assert!(ev.eval(&output("x", 50), &t).is_empty(), "冷却中");
        assert_eq!(ev.eval(&output("x", 100), &t).len(), 1, "恰好隔满算合法");
    }

    #[test]
    fn once_session_fires_once_until_reset() {
        let mut r = rule("r1", When::Connected);
        r.guard.once = OnceScope::Session;
        let mut ev = Evaluator::new();
        ev.reload(&[r]);
        let t = Target::default();
        assert_eq!(ev.eval(&connected(0), &t).len(), 1);
        assert!(ev.eval(&connected(9999), &t).is_empty());
        ev.reset();
        assert_eq!(ev.eval(&connected(10_000), &t).len(), 1, "reset 后恢复");
    }

    #[test]
    fn once_connection_resets_on_connect_and_disconnect_event() {
        let mut r = rule("r1", When::Output(pattern("go")));
        r.guard.once = OnceScope::Connection;
        let mut ev = Evaluator::new();
        ev.reload(&[r]);
        let t = Target::default();
        assert_eq!(ev.eval(&output("go", 0), &t).len(), 1);
        assert!(ev.eval(&output("go", 10), &t).is_empty(), "本连接内只一次");
        // Connected 不匹配 Output，但必须先重置 Connection 标记。
        assert!(ev.eval(&connected(20), &t).is_empty());
        assert_eq!(ev.eval(&output("go", 30), &t).len(), 1, "重连后可再触发");
        // Disconnected 不重置；只有连接事件重置。
        let gone = Signal::Disconnected {
            stream: "s",
            at: Ticks::from_millis(40),
            reason: CloseReason::Normal,
        };
        assert!(ev.eval(&gone, &t).is_empty());
        assert!(ev.eval(&output("go", 50), &t).is_empty());
    }

    #[test]
    fn reload_prunes_state_of_rules_that_disappeared() {
        let mut ev = Evaluator::new();
        ev.reload(&[rule("keep", When::Connected), rule("drop", When::Connected)]);
        let t = Target::default();
        ev.eval(&connected(0), &t);
        assert_eq!(ev.state_len(), 2);
        ev.reload(&[rule("keep", When::Connected)]);
        assert_eq!(ev.state_len(), 1, "消失的规则状态被清掉");
    }

    #[test]
    fn a_single_signal_never_repeats_the_same_rule_for_multiple_hits() {
        // 规则里一个 action：即便文本里 "x" 出现三次，也只产一条 Effect。
        let mut ev = Evaluator::new();
        ev.reload(&[rule("r1", When::Output(pattern("x")))]);
        let effects = ev.eval(&output("xxx", 0), &Target::default());
        assert_eq!(effects.len(), 1);
    }
}

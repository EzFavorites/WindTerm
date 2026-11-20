//! 触发器双轨（R 规格 M3）：旧 `term_app::trigger::Matcher` 与新
//! `term_core::automation::Evaluator` 并行求值，默认走旧轨产出副作用。
//!
//! - **影子轨**（默认）：新内核只 `eval`，产出 `Effect` 仅用于比对 / 计数，
//!   **绝不 dispatch**——所以 98 张截图与旧行为逐字节一致。
//! - **新轨**（`WINDTERM_AUTOMATION_V2` 在场）：Effect 经 [`dispatcher`] 落到
//!   真实副作用；关掉环境变量立刻回到旧轨，可回退。
//!
//! 这里不读墙钟：`Ticks` / `Duration` 由 [`sources`] 从宿主注入的时间算出。
//! 本模块是组装层，纯函数与映射分别在 `sources` / `dispatcher`，可单独单测。

pub mod dispatcher;
pub mod sources;

use std::time::Duration;

use term_core::automation::{Effect, Evaluator, Rule, RuleId, Signal, Target, Ticks};

/// 新轨开关。默认（环境变量不在场）= 影子轨，零副作用。
pub fn v2_enabled() -> bool {
    std::env::var_os("WINDTERM_AUTOMATION_V2").is_some()
}

/// 一次影子比对的结果：新轨命中了哪些规则、旧轨命中了哪些。
#[derive(Clone, Debug, Default)]
pub struct ShadowReport {
    /// 新内核这次信号产出的 Effect 规则 id（去重、按产出顺序）。
    pub v2_rules: Vec<RuleId>,
    /// 供对账：同一信号下旧轨命中的规则 id（由调用方填入）。
    pub v1_rules: Vec<RuleId>,
}

impl ShadowReport {
    /// 两轨是否一致（只看规则 id 集合，不看动作负载——M3 只验「命中集合」）。
    pub fn agrees(&self) -> bool {
        let mut a = self.v2_rules.clone();
        let mut b = self.v1_rules.clone();
        a.sort();
        b.sort();
        a == b
    }
}

/// 每 tab 一份的双轨状态。与会话同生命周期，重连时 [`Self::reset`]。
#[derive(Clone, Debug, Default)]
pub struct Shadow {
    eval: Evaluator,
    /// 上次装入的规则指纹，变化时才重新编译规则。
    signature: u64,
    /// 上次输出注入的时刻（毫秒），用于算 `Signal::Idle.quiet`。
    last_output: Option<Ticks>,
    /// 影子比对累计（仅诊断，不进持久化）。
    disagreements: usize,
}

impl Shadow {
    pub fn new() -> Self {
        Self {
            signature: u64::MAX,
            ..Self::default()
        }
    }

    pub fn signature(&self) -> u64 {
        self.signature
    }

    pub fn set_signature(&mut self, signature: u64) {
        self.signature = signature;
    }

    /// 换一批规则（来自旧规则转换）。编译错误在此处被丢弃（旧 Rule 已由
    /// `term_app::trigger::validate` 校验过，新 Pattern 编译失败只可能是
    /// 简化正则与标准正则的语法差异——留空规则，不让它影响旧轨）。
    pub fn reload(&mut self, rules: &[Rule]) {
        let _errors = self.eval.reload(rules);
    }

    /// 喂一条信号，返回新轨产出的 Effect。默认路径调用方**只比对不执行**。
    pub fn eval(&mut self, signal: &Signal<'_>, target: &Target) -> Vec<Effect> {
        self.eval.eval(signal, target)
    }

    /// 记一次输出时刻（毫秒），供下一次空闲判定。
    pub fn note_output(&mut self, at: Ticks) {
        self.last_output = Some(at);
    }

    pub fn last_output(&self) -> Option<Ticks> {
        self.last_output
    }

    pub fn note_disagreement(&mut self) {
        self.disagreements += 1;
    }

    pub fn disagreements(&self) -> usize {
        self.disagreements
    }

    /// 会话结束：清求值器运行态与空闲时钟（一次性标记按 [`term_core::automation::OnceScope`]
    /// 语义重置）。规则本身不在运行态里，`reload` 时重新给。
    pub fn reset(&mut self) {
        self.eval.reset();
        self.last_output = None;
    }
}

/// 把一次信号的 Effect 映射成规则 id（影子比对用）。
pub fn effect_rule_ids(effects: &[Effect]) -> Vec<RuleId> {
    let mut ids: Vec<RuleId> = Vec::new();
    for e in effects {
        if !ids.contains(&e.rule) {
            ids.push(e.rule.clone());
        }
    }
    ids
}

/// 会话目标：名字跨进程稳定（ADR-0024）。影子比对只用会话名，不用句柄。
pub fn target_for(session: &str) -> Target {
    Target {
        system: String::new(),
        session: session.to_owned(),
    }
}

/// 毫秒时钟（与旧轨的秒时钟分开，避免单位混用）。
pub fn now_ms() -> Ticks {
    let millis = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0);
    Ticks::from_millis(millis)
}

/// 空闲时长：从上次输出到现在的毫秒差。没记过输出时给一个足够大的值
/// （旧轨 `last_output_at` 初值 0 同样意味着「一开始就算空闲」）。
pub fn quiet_since(last: Option<Ticks>, now: Ticks) -> Duration {
    match last {
        Some(t) => now.elapsed_since(t),
        None => Duration::from_secs(u64::MAX / 1000),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use term_core::automation::rule::{Action, AlertKind, AppAction};
    use term_core::automation::RuleId;

    #[test]
    fn shadow_report_agrees_on_the_same_rule_set_regardless_of_order() {
        let a = ShadowReport {
            v2_rules: vec!["r1".into(), "r2".into()],
            v1_rules: vec!["r2".into(), "r1".into()],
        };
        assert!(a.agrees());
        let b = ShadowReport {
            v2_rules: vec!["r1".into()],
            v1_rules: vec!["r2".into()],
        };
        assert!(!b.agrees());
    }

    #[test]
    fn effect_rule_ids_deduplicates_in_production_order() {
        let ids: Vec<RuleId> = vec!["r2".into(), "r1".into(), "r2".into()];
        let effects: Vec<Effect> = ids
            .into_iter()
            .map(|rule| Effect {
                rule,
                action: Action::App(AppAction::Alert(AlertKind::Beep)),
                span: None,
            })
            .collect();
        assert_eq!(effect_rule_ids(&effects), vec!["r2", "r1"]);
    }

    #[test]
    fn quiet_since_handles_never_had_output() {
        assert_eq!(
            quiet_since(None, Ticks::from_millis(42)),
            Duration::from_secs(u64::MAX / 1000)
        );
        assert_eq!(
            quiet_since(Some(Ticks::from_millis(10)), Ticks::from_millis(42)),
            Duration::from_millis(32)
        );
    }

    #[test]
    fn now_ms_is_monotonic_enough_for_a_shadow_report() {
        let a = now_ms();
        let b = now_ms();
        assert!(b >= a);
    }
}

//! 触发器 Send text 的时序后端：发送条件与 Send Later。
//!
//! 这里只算「这次命中什么时候可以发」，不碰时钟/线程/IO。求值内核把命中
//! 连同 `Wait` 排队，宿主在对应事件到来时再问一次 `ready` 即可。
//! 兼容旧规则：`SendCondition::AfterMatchAndPrompt` 是界面默认，但老规则没有
//! `config` 键时 `send.text` 为空、没有可发内容，所以不会造成「默认全部延后」。

use std::time::Duration;

use super::trigger::{Rule, SendCondition};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Gate {
    /// 匹配即发。
    Match,
    /// 等下一个命令提示符。
    Prompt,
    /// 等下一个换行。
    LineBreak,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Wait {
    Ready(Gate),
    Later(Duration),
}

/// 解析设置页里的 `Send Later` 原始文本（`1.0` = 1 秒）。
/// 空串 / 非数字 / 负数 / 非有限值都当「不延迟」，不 panic。
pub fn parse_later_secs(raw: &str) -> Option<Duration> {
    let value = raw.trim().parse::<f64>().ok()?;
    if !value.is_finite() || value <= 0.0 {
        return None;
    }
    Some(Duration::from_secs_f64(value))
}

/// 一条规则的命中该怎么排：`Send Later` 优先于发送条件。
pub fn wait_for(rule: &Rule) -> Wait {
    if let Some(delay) = parse_later_secs(&rule.config.send.later_secs) {
        return Wait::Later(delay);
    }
    let gate = match rule.config.send.condition {
        SendCondition::AfterMatchAndPrompt => Gate::Prompt,
        SendCondition::AfterMatchAndLineBreak => Gate::LineBreak,
        SendCondition::AfterMatch => Gate::Match,
    };
    Wait::Ready(gate)
}

/// 到点判定：`event` 与 `gate` 相等时放行。调用方负责传入对应事件。
pub fn ready(wait: &Wait, event: Gate) -> bool {
    match wait {
        Wait::Ready(gate) => *gate == event || *gate == Gate::Match,
        // 定时等待由调用方用时钟判定，事件不放行。
        Wait::Later(_) => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn later_seconds_parse_without_panicking() {
        assert_eq!(parse_later_secs("1.0"), Some(Duration::from_millis(1000)));
        assert_eq!(parse_later_secs(" 0.25 "), Some(Duration::from_millis(250)));
        assert_eq!(parse_later_secs("0"), None);
        assert_eq!(parse_later_secs("-1"), None);
        assert_eq!(parse_later_secs("abc"), None);
    }

    #[test]
    fn a_rule_waits_for_its_configured_event() {
        let mut rule = Rule::default();
        rule.config.send.later_secs = "0".into();
        rule.config.send.condition = SendCondition::AfterMatchAndPrompt;
        assert_eq!(wait_for(&rule), Wait::Ready(Gate::Prompt));
        rule.config.send.condition = SendCondition::AfterMatchAndLineBreak;
        assert_eq!(wait_for(&rule), Wait::Ready(Gate::LineBreak));
        rule.config.send.condition = SendCondition::AfterMatch;
        assert_eq!(wait_for(&rule), Wait::Ready(Gate::Match));
        rule.config.send.condition = SendCondition::AfterMatchAndPrompt;
        rule.config.send.later_secs = "2".into();
        assert_eq!(wait_for(&rule), Wait::Later(Duration::from_secs(2)));
    }

    #[test]
    fn the_default_rule_lets_the_send_condition_decide() {
        // 默认 Send Later = 0（不延迟），三种条件出厂即生效；
        // 只有显式填正数才覆盖条件。
        let mut rule = Rule::default();
        rule.config.send.condition = SendCondition::AfterMatchAndPrompt;
        assert_eq!(wait_for(&rule), Wait::Ready(Gate::Prompt));
        rule.config.send.condition = SendCondition::AfterMatchAndLineBreak;
        assert_eq!(wait_for(&rule), Wait::Ready(Gate::LineBreak));
        rule.config.send.condition = SendCondition::AfterMatch;
        assert_eq!(wait_for(&rule), Wait::Ready(Gate::Match));
    }

    #[test]
    fn a_gate_is_released_by_its_own_event_and_match_is_never_blocked() {
        assert!(ready(&Wait::Ready(Gate::Prompt), Gate::Prompt));
        assert!(!ready(&Wait::Ready(Gate::Prompt), Gate::LineBreak));
        assert!(ready(&Wait::Ready(Gate::Match), Gate::LineBreak));
        assert!(!ready(&Wait::Later(Duration::from_secs(1)), Gate::Prompt));
    }
}

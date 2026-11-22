//! 触发器 `Sessions` 页的目标过滤判据（纯函数）。
//!
//! 空列表 = 不过滤（兼容旧规则）。给了系统/会话名时两者是「或」：
//! 命中任意一个就算这条规则作用于当前标签，这正是编辑页两列打勾的语义。

use super::trigger::{Rule, TargetConfig};

/// 当前标签的身份，供规则过滤。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Target<'a> {
    pub system: &'a str,
    pub session: &'a str,
}

pub fn matches(config: &TargetConfig, target: Target<'_>) -> bool {
    if config.systems.is_empty() && config.sessions.is_empty() {
        return true;
    }
    config
        .systems
        .iter()
        .any(|s| s == target.system)
        || config
            .sessions
            .iter()
            .any(|s| s == target.session)
}

/// 规则是否作用于这次事件。`None` = 没有上下文（脚本/桥接），按不过滤处理。
pub fn rule_matches(rule: &Rule, target: Option<Target<'_>>) -> bool {
    match target {
        Some(target) => matches(&rule.config.targets, target),
        None => true,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config(systems: &[&str], sessions: &[&str]) -> TargetConfig {
        TargetConfig {
            systems: systems.iter().map(|s| s.to_string()).collect(),
            sessions: sessions.iter().map(|s| s.to_string()).collect(),
        }
    }

    #[test]
    fn an_empty_target_list_matches_everything() {
        assert!(matches(&config(&[], &[]), Target { system: "linux", session: "prod" }));
    }

    #[test]
    fn system_and_session_lists_are_alternatives() {
        let cfg = config(&["linux"], &["prod"]);
        assert!(matches(&cfg, Target { system: "linux", session: "dev" }));
        assert!(matches(&cfg, Target { system: "windows", session: "prod" }));
        assert!(!matches(&cfg, Target { system: "windows", session: "dev" }));
    }

    #[test]
    fn a_missing_context_does_not_silently_drop_rules() {
        let mut rule = Rule::default();
        rule.config.targets = config(&["linux"], &[]);
        assert!(rule_matches(&rule, None));
        assert!(rule_matches(&rule, Some(Target { system: "linux", session: "x" })));
        assert!(!rule_matches(&rule, Some(Target { system: "macos", session: "x" })));
    }
}

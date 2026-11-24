//! 触发器命中模板展开。放在独立文件是为了让 `trigger.rs` 守住行数门禁。

use super::trigger::{Hit, Rule, SendMode, TriggerAction};

/// 把 `${match}` 换成这次命中的文本；其余动作原样返回。
pub(crate) fn expand_actions(rule: &Rule, matched: &str) -> Vec<TriggerAction> {
    let expand = |text: &str| text.replace("${match}", matched);
    let script = rule.config.send.mode == SendMode::Script;
    let text = |s: &str| {
        let expanded = expand(s);
        if script {
            super::trigger_script::eval(&expanded)
        } else {
            expanded
        }
    };
    rule.actions
        .iter()
        .map(|action| match action {
            TriggerAction::SendText(s) => TriggerAction::SendText(text(s)),
            TriggerAction::SendLine(s) => TriggerAction::SendLine(text(s)),
            TriggerAction::RunCommand(s) => TriggerAction::RunCommand(expand(s)),
            TriggerAction::LocalSendLine(s) => TriggerAction::LocalSendLine(text(s)),
            // URL 不是脚本文本：只替换 `${match}`，不进 Script 求值器。
            TriggerAction::OpenLink(s) => TriggerAction::OpenLink(expand(s)),
            other => other.clone(),
        })
        .collect()
}

/// 命中构造共用的展开入口。
pub(crate) fn hit_with(rule: &Rule, matched: String) -> Hit {
    let actions = expand_actions(rule, &matched);
    Hit {
        rule_id: rule.id.clone(),
        actions,
        matched,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn script_mode_returns_a_literal_value_after_expansion() {
        let mut rule: Rule = serde_json::from_str(
            r#"{"id":"r","name":"n","description":"","on":"Output","pattern":"x",
                "literal":true,"case_sensitive":true,"cooldown_secs":0,"idle_secs":0,
                "actions":[],"enabled":true}"#,
        )
        .unwrap();
        rule.config.send.mode = SendMode::Script;
        rule.actions = vec![TriggerAction::SendText(r#"return "got ${match}""#.into())];
        let hit = hit_with(&rule, "42".into());
        assert_eq!(
            hit.actions,
            vec![TriggerAction::SendText("got 42".into())]
        );
    }

    #[test]
    fn a_hit_expands_the_match_placeholder_in_every_text_action() {
        let mut rule: Rule = serde_json::from_str(
            r#"{"id":"r1","name":"legacy","description":"","on":"Output","pattern":"error",
                "literal":true,"case_sensitive":true,"cooldown_secs":0,"idle_secs":0,
                "actions":[],"enabled":true}"#,
        )
        .unwrap();
        rule.actions = vec![
            TriggerAction::SendText("got ${match}".into()),
            TriggerAction::SendLine("echo ${match}".into()),
            TriggerAction::RunCommand("log ${match}".into()),
            TriggerAction::Beep,
        ];
        let hit = hit_with(&rule, "ERR-42".into());
        assert_eq!(hit.matched, "ERR-42");
        assert_eq!(
            hit.actions,
            vec![
                TriggerAction::SendText("got ERR-42".into()),
                TriggerAction::SendLine("echo ERR-42".into()),
                TriggerAction::RunCommand("log ERR-42".into()),
                TriggerAction::Beep,
            ]
        );
        // 原规则不能被这次展开改写，下一次命中仍按模板来。
        assert!(matches!(&rule.actions[0], TriggerAction::SendText(s) if s.contains("${match}")));
    }

    #[test]
    fn open_link_action_survives_a_serde_roundtrip() {
        let action = TriggerAction::OpenLink("https://e.test/${match}".into());
        let json = serde_json::to_string(&action).unwrap();
        assert_eq!(json, r#"{"OpenLink":"https://e.test/${match}"}"#);
        assert_eq!(serde_json::from_str::<TriggerAction>(&json).unwrap(), action);
        // 新增变体不能让旧配置里的动作读不出来。
        let old: TriggerAction = serde_json::from_str(r#"{"SendText":"go"}"#).unwrap();
        assert_eq!(old, TriggerAction::SendText("go".into()));
    }

    #[test]
    fn open_link_expands_the_match_placeholder_only() {
        let mut rule = Rule::default();
        rule.config.send.mode = SendMode::Script;
        rule.actions = vec![TriggerAction::OpenLink("https://e.test/${match}".into())];
        let hit = hit_with(&rule, "42".into());
        // URL 不像 SendText 那样进脚本求值器：${match} 只是文本替换。
        assert_eq!(
            hit.actions,
            vec![TriggerAction::OpenLink("https://e.test/42".into())]
        );
    }
}

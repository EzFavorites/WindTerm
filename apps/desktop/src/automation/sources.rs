//! 信号源：把宿主事实转成新内核的 `Signal` / `Rule`。
//!
//! 纯转换，无副作用、不读时钟：旧 `term_app::trigger::Rule` → 新
//! `term_core::automation::Rule`，连接/断开/输出/空闲边沿 → `Signal`。
//! 两边都可对着纯数据单测——这正是旧代码把边沿埋在 `poll` 里做不到的。

use std::time::Duration;

use term_core::automation::rule::{AlertKind, OnceScope};
use term_core::automation::{
    Action, AppAction, Filter, Guard, Invocation, Pattern, Rule, Signal, Syntax, Target, Ticks,
    When,
};
use term_core::trigger::{Rule as LegacyRule, TriggerAction, TriggerOn};

/// 旧规则 → 新规则。`id` 原样保留，影子比对靠它对齐两轨。
///
/// - `on` 六通道一一映射到 `When`（不再有不可达通道）。
/// - `literal` / `case_sensitive` 映射到 `Pattern` 的 `Syntax` / 大小写开关。
/// - `cooldown_secs` → `Guard.cooldown`；一次性语义默认 `Never`（旧规则没有
///   Session/Connection 的显式区分，迁移时才按 kind 细化）。
/// - `actions` 原样映射：`SendText/SendLine` 走 `write` 动词（SendLine 在文本
///   末尾拼 CR），`Beep` → `AppAction::Alert`，`RunCommand` → `Dispatch`。
///   `Highlight` 无渲染基底，映射成空动作（保留规则但不产生效果）。
pub fn convert_rule(old: &LegacyRule) -> Option<Rule> {
    let when = match old.on {
        TriggerOn::Output => {
            if old.pattern.is_empty() {
                return None;
            }
            let syntax = if old.literal { Syntax::Literal } else { Syntax::Regex };
            let p = Pattern::compile(&old.pattern, syntax, old.case_sensitive).ok()?;
            When::Output(p)
        }
        TriggerOn::Connect => When::Connected,
        TriggerOn::Disconnect => When::Disconnected,
        TriggerOn::BeforeConnect => When::BeforeConnect,
        TriggerOn::BeforeDisconnect => When::BeforeDisconnect,
        TriggerOn::Idle => When::Idle {
            after: Duration::from_secs(old.idle_secs),
        },
    };
    let guard = Guard {
        cooldown: if old.cooldown_secs == 0 {
            None
        } else {
            Some(Duration::from_secs(old.cooldown_secs))
        },
        once: OnceScope::Never,
    };
    let actions = old
        .actions
        .iter()
        .filter_map(convert_action)
        .collect::<Vec<_>>();
    Some(Rule {
        id: old.id.clone(),
        name: old.name.clone(),
        enabled: old.enabled,
        when,
        filter: Filter::default(),
        guard,
        actions,
    })
}

/// 批量转换。失败的单条被跳过（影子轨不因一条坏规则拖垮旧轨）。
pub fn convert_rules(legacy: &[LegacyRule]) -> Vec<Rule> {
    legacy.iter().filter_map(convert_rule).collect()
}

/// 规则清单指纹：仅用于判断「要不要重新编译新规则」，不参与匹配。
/// 覆盖影响转换结果的全部字段；用 FNV 风格哈希，够检测变化即可，不要求
/// 密码学强度。空表给固定值 0（首帧也会 reload 一次，代价可忽略）。
pub fn rules_fingerprint(legacy: &[LegacyRule]) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for r in legacy {
        for part in [
            r.id.as_str(),
            r.name.as_str(),
            r.pattern.as_str(),
            match r.on {
                TriggerOn::Output => "O",
                TriggerOn::Connect => "C",
                TriggerOn::Disconnect => "D",
                TriggerOn::Idle => "I",
                TriggerOn::BeforeConnect => "BC",
                TriggerOn::BeforeDisconnect => "BD",
            },
        ] {
            for b in part.bytes().chain(std::iter::once(0)) {
                h ^= b as u64;
                h = h.wrapping_mul(0x0100_0000_01b3);
            }
        }
        for v in [
            r.enabled as u64,
            r.literal as u64,
            r.case_sensitive as u64,
            r.cooldown_secs,
            r.idle_secs,
            r.actions.len() as u64,
        ] {
            h ^= v.wrapping_add(0x9e37_79b9_7f4a_7c15);
            h = h.wrapping_mul(0x0100_0000_01b3);
        }
    }
    h
}

fn convert_action(old: &TriggerAction) -> Option<Action> {
    match old {
        TriggerAction::SendText(s) => Some(Action::Session(Invocation::new(
            "write",
            serde_json::json!({ "text": s }),
        ))),
        TriggerAction::SendLine(s) => {
            let mut line = s.clone();
            line.push('\r');
            Some(Action::Session(Invocation::new(
                "write",
                serde_json::json!({ "text": line }),
            )))
        }
        TriggerAction::Beep => Some(Action::App(AppAction::Alert(AlertKind::Beep))),
        TriggerAction::RunCommand(id) => Some(Action::App(AppAction::Dispatch(id.clone()))),
        // 无渲染基底：保留规则命中，但不产出副作用（与旧轨一致）。
        TriggerAction::Highlight => None,
    }
}

/// 一条输出信号。`stream` 用会话名，供 `Filter` 按需过滤（当前为空）。
pub fn output_signal<'a>(session: &'a str, text: &'a str, at: Ticks) -> Signal<'a> {
    Signal::Output {
        stream: session,
        text,
        at,
    }
}

pub fn connected_signal(session: &str, at: Ticks) -> Signal<'_> {
    Signal::Connected { stream: session, at }
}

pub fn disconnected_signal(session: &str, at: Ticks) -> Signal<'_> {
    Signal::Disconnected {
        stream: session,
        at,
        reason: term_core::automation::CloseReason::Normal,
    }
}

pub fn idle_signal(session: &str, quiet: Duration, at: Ticks) -> Signal<'_> {
    Signal::Idle {
        stream: session,
        quiet,
        at,
    }
}

pub fn target(session: &str) -> Target {
    Target {
        system: String::new(),
        session: session.to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use term_core::trigger::RuleConfig;

    fn legacy(on: TriggerOn, pattern: &str, actions: Vec<TriggerAction>) -> LegacyRule {
        LegacyRule {
            id: "r1".into(),
            name: "test".into(),
            description: String::new(),
            kind: term_core::trigger::TriggerKind::default(),
            config: RuleConfig::default(),
            on,
            pattern: pattern.into(),
            literal: true,
            case_sensitive: true,
            cooldown_secs: 0,
            idle_secs: 5,
            actions,
            enabled: true,
        }
    }

    #[test]
    fn output_rule_uses_literal_pattern_by_default() {
        let r = convert_rule(&legacy(TriggerOn::Output, "a.b", vec![])).unwrap();
        assert_eq!(r.id, "r1");
        match r.when {
            When::Output(p) => {
                assert_eq!(p.syntax(), Syntax::Literal);
                assert!(p.matches("xa.by"));
                assert!(!p.matches("xaby"));
            }
            _ => panic!("输出规则应有 Output 条件"),
        }
    }

    #[test]
    fn regex_and_case_insensitive_flags_are_honoured() {
        let mut old = legacy(TriggerOn::Output, "^ERR\\d+", vec![]);
        old.literal = false;
        old.case_sensitive = false;
        let r = convert_rule(&old).unwrap();
        match r.when {
            When::Output(p) => {
                assert_eq!(p.syntax(), Syntax::Regex);
                assert!(!p.case_sensitive());
                assert!(p.matches("err42: x"));
            }
            _ => panic!(),
        }
    }

    #[test]
    fn all_six_channels_map_without_a_dead_one() {
        for (on, expected) in [
            (TriggerOn::Connect, "Connected"),
            (TriggerOn::Disconnect, "Disconnected"),
            (TriggerOn::BeforeConnect, "BeforeConnect"),
            (TriggerOn::BeforeDisconnect, "BeforeDisconnect"),
            (TriggerOn::Idle, "Idle"),
        ] {
            let r = convert_rule(&legacy(on, "", vec![])).unwrap();
            let got = match r.when {
                When::Connected => "Connected",
                When::Disconnected => "Disconnected",
                When::BeforeConnect => "BeforeConnect",
                When::BeforeDisconnect => "BeforeDisconnect",
                When::Idle { .. } => "Idle",
                When::Output(_) => "Output",
            };
            assert_eq!(got, expected, "{on:?} 应映射到 {expected}");
        }
    }

    #[test]
    fn idle_rule_carries_the_idle_seconds() {
        let r = convert_rule(&legacy(TriggerOn::Idle, "", vec![])).unwrap();
        match r.when {
            When::Idle { after } => assert_eq!(after, Duration::from_secs(5)),
            _ => panic!(),
        }
    }

    #[test]
    fn cooldown_maps_to_a_duration_and_zero_stays_none() {
        let mut old = legacy(TriggerOn::Output, "x", vec![]);
        assert!(convert_rule(&old).unwrap().guard.cooldown.is_none());
        old.cooldown_secs = 7;
        assert_eq!(
            convert_rule(&old).unwrap().guard.cooldown,
            Some(Duration::from_secs(7))
        );
    }

    #[test]
    fn actions_map_to_write_alert_and_dispatch() {
        let old = legacy(
            TriggerOn::Output,
            "x",
            vec![
                TriggerAction::SendText("hi".into()),
                TriggerAction::SendLine("go".into()),
                TriggerAction::Beep,
                TriggerAction::RunCommand("File.Save".into()),
                TriggerAction::Highlight,
            ],
        );
        let r = convert_rule(&old).unwrap();
        assert_eq!(r.actions.len(), 4, "Highlight 无渲染基底，不产动作");
        match &r.actions[0] {
            Action::Session(inv) => {
                assert_eq!(inv.method, "write");
                assert_eq!(inv.params["text"], "hi");
            }
            _ => panic!(),
        }
        match &r.actions[1] {
            Action::Session(inv) => assert_eq!(inv.params["text"], "go\r"),
            _ => panic!(),
        }
        match &r.actions[2] {
            Action::App(AppAction::Alert(AlertKind::Beep)) => {}
            _ => panic!(),
        }
        match &r.actions[3] {
            Action::App(AppAction::Dispatch(id)) => assert_eq!(id, "File.Save"),
            _ => panic!(),
        }
    }

    #[test]
    fn an_empty_output_pattern_is_skipped_not_turned_into_a_match_all_rule() {
        assert!(convert_rule(&legacy(TriggerOn::Output, "", vec![])).is_none());
    }

    #[test]
    fn signals_carry_the_session_as_stream() {
        let at = Ticks::from_millis(9);
        assert_eq!(output_signal("s1", "x", at).stream(), "s1");
        assert_eq!(connected_signal("s2", at).stream(), "s2");
        assert_eq!(disconnected_signal("s3", at).stream(), "s3");
        assert_eq!(
            idle_signal("s4", Duration::from_secs(1), at).stream(),
            "s4"
        );
    }
}

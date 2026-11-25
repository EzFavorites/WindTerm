//! 规则与意图。全是纯数据 + serde，无副作用。
//!
//! 两类动作按**是否落在会话面**分开：`Action::Session` 直接引用
//! `surface::METHODS` 的动词（`Invocation`），`Action::App` 才是应用层的
//! 标记 / 菜单 / 跑脚本等。这样"能做什么"只有一份清单，新增会话动作不动内核。
//!
//! `Rule` 里**没有 `kind` 字段**：兼容终端 11 种类型是 UI 预设，经工厂展开成
//! 这里的正交字段。改过结构的规则不强行归类（`kind_of` 返回 `None`）。

use std::fmt;
use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::automation::pattern::Span;
use crate::automation::signal::Target;

/// 规则标识。`String` 而不是 newtype：与 `prefs.json` / 旧 `Rule.id` 同形，
/// 迁移时不需要改键名。
pub type RuleId = String;

/// "只触发一次"的作用域。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum OnceScope {
    #[default]
    /// 每次满足条件都触发（仍受 cooldown 约束）。
    Never,
    /// 本会话内只一次。
    Session,
    /// 每次连接只一次（重连重置）。
    Connection,
}

/// 节流：冷却窗口 + 一次性作用域。
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Guard {
    #[serde(default)]
    pub cooldown: Option<Duration>,
    #[serde(default)]
    pub once: OnceScope,
}

/// 规则作用的目标。空表 = 不限。
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Filter {
    #[serde(default)]
    pub systems: Vec<String>,
    #[serde(default)]
    pub sessions: Vec<String>,
}

impl Filter {
    /// 两个维度都是"空 = 不限，非空 = 命中任意一个"。
    pub fn admits(&self, target: &Target) -> bool {
        (self.systems.is_empty() || self.systems.iter().any(|s| s == &target.system))
            && (self.sessions.is_empty() || self.sessions.iter().any(|s| s == &target.session))
    }
}

/// 一次会话面调用。`method` 必须是 `surface::METHODS` 里的名字，
/// dispatcher 调之前用同一份 schema 校验参数。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Invocation {
    pub method: String,
    pub params: serde_json::Value,
}

impl Invocation {
    pub fn new(method: impl Into<String>, params: serde_json::Value) -> Self {
        Self {
            method: method.into(),
            params,
        }
    }
}

/// 行标记（颜色名沿用视觉基线的写法，UI 侧解析）。
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct MarkSpec {
    #[serde(default)]
    pub text_color: String,
    #[serde(default)]
    pub background: String,
    #[serde(default)]
    pub line_style: String,
    #[serde(default)]
    pub line_color: String,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct LinkSpec {
    #[serde(default)]
    pub url: String,
    #[serde(default)]
    pub text: String,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct MenuItemSpec {
    #[serde(default)]
    pub label: String,
    #[serde(default)]
    pub separator: bool,
    #[serde(default)]
    pub action: Option<Invocation>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct MenuSpec {
    #[serde(default)]
    pub items: Vec<MenuItemSpec>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TextSpec {
    #[serde(default)]
    pub text: String,
    /// `true` = 文本要当 Rhai 片段求值。
    #[serde(default)]
    pub is_script: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum AlertKind {
    /// 终端 BEL（0x07）。
    Beep,
    /// 桌面通知。
    SystemNotice,
}

/// 一次脚本运行的授权声明。**不复用 `Capabilities`**——那个类型没有 serde，
/// 且它是"一次运行的实际授权"，这里是"规则里存的授权请求"，两者在
/// dispatcher 里显式映射，避免把持久化格式绑到运行期类型。
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScriptGrant {
    #[serde(default)]
    pub fs_roots: Vec<String>,
    #[serde(default)]
    pub fs_write: bool,
    #[serde(default)]
    pub process: bool,
    #[serde(default)]
    pub clipboard: bool,
    #[serde(default)]
    pub ui: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScriptSpec {
    pub path: String,
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(default)]
    pub grant: ScriptGrant,
}

/// 应用面动作：没有对应的会话动词。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum AppAction {
    Mark(MarkSpec),
    Link(LinkSpec),
    Menu(MenuSpec),
    Tooltip(TextSpec),
    Alert(AlertKind),
    /// 菜单命令 ID，走既有命令通道。
    Dispatch(String),
    /// 后台线程跑一段 Rhai（触发器 → 脚本的唯一连接方式）。
    RunScript(ScriptSpec),
}

/// 一条规则命中后要做的事。只声明意图，不执行。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum Action {
    /// 会话面：引用 `surface::METHODS` 的动词。
    Session(Invocation),
    /// 应用层：标记 / 菜单 / 通知 / 跑脚本。
    App(AppAction),
}

/// 规则校验失败：动作里有内核 / 分发器执行不了的项。
///
/// 为什么要校验而不是「命中后 NoEffect」：后者让用户在规则管理器里配好一条
/// 规则、等了半天才在日志里看见「没效果」。装载期就拒绝，错误停在写规则的那一刻。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RuleFault(pub String);

impl fmt::Display for RuleFault {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for RuleFault {}

/// 有分发实现的会话动词。**规则校验与 dispatcher 共用这一份清单**，免得
/// 「校验放行」与「命中后才有 NoEffect」两边各写一张表然后漂移。
pub const DISPATCHABLE_METHODS: &[&str] = &["write", "keys", "notify"];

pub fn dispatchable_method(method: &str) -> bool {
    DISPATCHABLE_METHODS.contains(&method)
}

impl Action {
    /// 装载期校验：动词既要在能力面登记，也要有分发实现；AppAction 同理。
    pub fn validate(&self) -> Result<(), RuleFault> {
        match self {
            Action::Session(inv) => {
                if crate::surface::find(&inv.method).is_none() {
                    return Err(RuleFault(format!(
                        "会话动词 {:?} 不在能力面上",
                        inv.method
                    )));
                }
                if !dispatchable_method(&inv.method) {
                    return Err(RuleFault(format!(
                        "会话动词 {:?} 已登记但还没有分发实现，规则不能装载",
                        inv.method
                    )));
                }
                Ok(())
            }
            Action::App(app) => app.validate(),
        }
    }
}

impl AppAction {
    pub fn validate(&self) -> Result<(), RuleFault> {
        match self {
            AppAction::Alert(AlertKind::Beep)
            | AppAction::Dispatch(_)
            | AppAction::RunScript(_) => Ok(()),
            AppAction::Alert(AlertKind::SystemNotice) => Err(RuleFault(
                "桌面通知还没有分发实现，规则不能装载".to_string(),
            )),
            AppAction::Mark(_) | AppAction::Link(_) | AppAction::Menu(_) | AppAction::Tooltip(_) => {
                Err(RuleFault(
                    "Mark / Link / Menu / Tooltip 的渲染基底还没落地，规则不能装载".to_string(),
                ))
            }
        }
    }
}

/// 校验整批规则。返回第一条不合格规则的 `(规则 id, 原因)`。
pub fn validate_rules(rules: &[Rule]) -> Result<(), (RuleId, RuleFault)> {
    for rule in rules {
        rule.validate()
            .map_err(|fault| (rule.id.clone(), fault))?;
    }
    Ok(())
}

/// 一条规则。字段正交，UI 的 11 种类型由预设工厂展开成它。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Rule {
    pub id: RuleId,
    #[serde(default)]
    pub name: String,
    #[serde(default = "default_enabled")]
    pub enabled: bool,
    pub when: crate::automation::signal::When,
    #[serde(default)]
    pub filter: Filter,
    #[serde(default)]
    pub guard: Guard,
    #[serde(default)]
    pub actions: Vec<Action>,
}

impl Rule {
    /// 规则装载期校验：任一动作不可执行就整条拒绝，绝不让它命中后变 NoEffect。
    pub fn validate(&self) -> Result<(), RuleFault> {
        for action in &self.actions {
            action.validate()?;
        }
        Ok(())
    }
}

fn default_enabled() -> bool {
    true
}

/// 一次命中产出的意图。`span` 只在文本类命中时有值。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Effect {
    pub rule: RuleId,
    pub action: Action,
    #[serde(default)]
    pub span: Option<Span>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::automation::signal::When;
    use crate::automation::pattern::{Pattern, Syntax};

    fn rule() -> Rule {
        Rule {
            id: "r1".into(),
            name: String::new(),
            enabled: true,
            when: When::Output(Pattern::compile("error", Syntax::Literal, true).unwrap()),
            filter: Filter::default(),
            guard: Guard::default(),
            actions: vec![Action::Session(Invocation::new(
                "write",
                serde_json::json!({ "session": "s1", "data": "hi" }),
            ))],
        }
    }

    #[test]
    fn empty_filter_admits_everything() {
        let t = Target { system: "linux".into(), session: "s1".into() };
        assert!(Filter::default().admits(&t));
    }

    #[test]
    fn filter_checks_both_dimensions_and_uses_any_semantics() {
        let t = Target { system: "linux".into(), session: "s1".into() };
        let mut f = Filter {
            systems: vec!["macos".into(), "linux".into()],
            sessions: vec!["s2".into()],
        };
        assert!(!f.admits(&t));
        f.sessions.push("s1".into());
        assert!(f.admits(&t));
    }

    #[test]
    fn a_rule_without_actions_is_valid_but_does_nothing() {
        let mut r = rule();
        r.actions.clear();
        assert!(r.actions.is_empty());
    }

    #[test]
    fn rule_roundtrips_through_json() {
        let r = rule();
        let json = serde_json::to_string(&r).unwrap();
        let back: Rule = serde_json::from_str(&json).unwrap();
        assert_eq!(back, r);
    }

    #[test]
    fn legacy_defaults_are_honest() {
        assert_eq!(Guard::default().once, OnceScope::Never);
        assert!(Guard::default().cooldown.is_none());
        let json = r#"{
            "id":"r2","when":"Connected","actions":[]
        }"#;
        let r: Rule = serde_json::from_str(json).unwrap();
        assert!(r.enabled);
        assert_eq!(r.name, "");
        assert!(r.filter.systems.is_empty());
    }

    #[test]
    fn invocation_references_a_session_verb_not_a_new_action_vocabulary() {
        let inv = Invocation::new("log_start", serde_json::json!({"session": "s1", "dir": "/tmp"}));
        assert_eq!(inv.method, "log_start");
    }

    #[test]
    fn validation_admits_only_methods_that_the_dispatcher_can_execute() {
        for method in ["write", "keys", "notify"] {
            let action = Action::Session(Invocation::new(method, serde_json::json!({})));
            assert!(action.validate().is_ok(), "{method} 应可装载");
        }
        // 登记在能力面但没有分发实现 → 装载期就拒，不等命中变 NoEffect。
        let action = Action::Session(Invocation::new("log_start", serde_json::json!({})));
        assert!(action.validate().is_err());
        let unknown = Action::Session(Invocation::new("not_a_verb", serde_json::json!({})));
        assert!(unknown.validate().is_err());
    }

    #[test]
    fn validation_rejects_unimplemented_app_actions() {
        assert!(AppAction::Alert(AlertKind::Beep).validate().is_ok());
        assert!(AppAction::Alert(AlertKind::SystemNotice).validate().is_err());
        assert!(AppAction::Mark(MarkSpec::default()).validate().is_err());
        assert!(AppAction::Link(LinkSpec::default()).validate().is_err());
        assert!(AppAction::Menu(MenuSpec::default()).validate().is_err());
        assert!(AppAction::Tooltip(TextSpec::default()).validate().is_err());
    }

    #[test]
    fn validate_rules_points_at_the_bad_rule() {
        let mut bad = rule();
        bad.actions.push(Action::App(AppAction::Mark(MarkSpec::default())));
        let (id, fault) = validate_rules(&[rule(), bad]).expect_err("应拒");
        assert_eq!(id, "r1");
        assert!(fault.to_string().contains("Mark"));
    }
}

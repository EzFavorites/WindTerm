//! 规则与意图。全是纯数据 + serde，无副作用。
//!
//! 两类动作按**是否落在会话面**分开：`Action::Session` 直接引用
//! `surface::METHODS` 的动词（`Invocation`），`Action::App` 才是应用层的
//! 标记 / 菜单 / 跑脚本等。这样"能做什么"只有一份清单，新增会话动作不动内核。
//!
//! `Rule` 里**没有 `kind` 字段**：兼容终端 11 种类型是 UI 预设，经工厂展开成
//! 这里的正交字段。改过结构的规则不强行归类（`kind_of` 返回 `None`）。

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
}

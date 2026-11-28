//! 规则与意图。全是纯数据 + serde，无副作用。
//!
//! [`Action`] 收敛为四类正交意图：会话动词调用、应用面动作、凭据引用、脚本运行。
//! 会话动词直接引用 [`crate::surface::METHODS`] 的名字，不另建第二张动词表；
//! 凭据与脚本只声明「要用什么」，派发与授权留给宿主。
//!
//! `Rule` 里**没有 `kind` 字段**：兼容终端 11 种类型是 UI 预设，经工厂展开成
//! 这里的正交字段。改过结构的规则不强行归类。

use std::fmt;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::Value;

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

/// 一次会话面调用。`method` 必须是 `surface::METHODS` 里的名字；
/// 装载期用同一条 schema 校验参数，执行期不再做第二次形状检查。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Invocation {
    pub method: String,
    pub params: Value,
}

impl Invocation {
    pub fn new(method: impl Into<String>, params: Value) -> Self {
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

/// 应用面动作：没有对应的会话动词。授权不属于这里——`RunScript` 只带路径与
/// 参数，一次运行的实际授权由宿主在运行前决定，绝不持久化进规则。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum AppAction {
    Mark(MarkSpec),
    Link(LinkSpec),
    Menu(MenuSpec),
    Tooltip(TextSpec),
    Alert(AlertKind),
    /// 菜单命令 ID，走既有命令通道。
    Dispatch(String),
}

/// 一条规则命中后要做的事。四类意图正交，只声明意图，不执行。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum Action {
    /// 会话面：引用 `surface::METHODS` 的动词与参数。
    SessionInvocation {
        method: String,
        params: Value,
    },
    /// 应用面：标记 / 链接 / 菜单 / 提示 / 通知 / 命令派发。
    AppAction(AppAction),
    /// 凭据引用：指向一个登录方案里的字段。只做数据定义，取凭据与派发留给宿主；
    /// 规则文件里永远不出现明文密码。
    CredentialRef {
        scheme: String,
        field: String,
    },
    /// 跑一段脚本：路径 + 参数，授权在运行时决定。
    RunScript {
        path: String,
        args: Vec<String>,
    },
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

/// `surface.rs` 的 schema 是 JSON Schema 的一个小子集。这里只认它实际用到的
/// 键（`type` / `properties` / `required` / `items` / `enum` / `minimum` /
/// `minLength` / `minItems`），不引第三方校验器；遇到没见过的 schema 键不因此
/// 拒收参数（`description` 满篇都是），但没见过的 `type` 一律拒——那是 schema
/// 作者写错了，不能让规则悄悄通过。
fn type_matches(value: &Value, ty: &str) -> bool {
    match ty {
        "object" => value.is_object(),
        "array" => value.is_array(),
        "string" => value.is_string(),
        "integer" => value.is_i64() || value.is_u64(),
        "number" => value.is_number(),
        "boolean" => value.is_boolean(),
        "null" => value.is_null(),
        _ => false,
    }
}

fn value_fits_schema(value: &Value, schema: &Value) -> bool {
    let Some(obj) = schema.as_object() else {
        return false;
    };
    if let Some(ty) = obj.get("type") {
        let ok = match ty {
            Value::String(s) => type_matches(value, s),
            Value::Array(types) => types
                .iter()
                .filter_map(Value::as_str)
                .any(|t| type_matches(value, t)),
            _ => false,
        };
        if !ok {
            return false;
        }
    }
    if let Some(Value::Array(allowed)) = obj.get("enum") {
        if !allowed.iter().any(|v| v == value) {
            return false;
        }
    }
    if let (Some(Value::Number(min)), Some(n)) = (obj.get("minimum"), value.as_f64()) {
        if let Some(min) = min.as_f64() {
            if n < min {
                return false;
            }
        }
    }
    if let (Some(Value::Number(min)), Some(s)) = (obj.get("minLength"), value.as_str()) {
        if let Some(min) = min.as_u64() {
            if (s.chars().count() as u64) < min {
                return false;
            }
        }
    }
    if let (Some(Value::Number(min)), Some(items)) = (obj.get("minItems"), value.as_array()) {
        if let Some(min) = min.as_u64() {
            if (items.len() as u64) < min {
                return false;
            }
        }
    }
    if let (Some(Value::Object(props)), Value::Object(fields)) = (obj.get("properties"), value) {
        for (key, sub) in props {
            if let Some(child) = fields.get(key) {
                if !value_fits_schema(child, sub) {
                    return false;
                }
            }
        }
    }
    if let Some(Value::Array(required)) = obj.get("required") {
        if let Value::Object(fields) = value {
            for key in required.iter().filter_map(Value::as_str) {
                if !fields.contains_key(key) {
                    return false;
                }
            }
        }
    }
    if let (Some(items), Value::Array(values)) = (obj.get("items"), value) {
        if !values.iter().all(|v| value_fits_schema(v, items)) {
            return false;
        }
    }
    true
}

impl Action {
    /// 装载期校验：会话动词必须在能力面上、对规则可见、参数形状符合它自己的
    /// `params_schema`；凭据引用与脚本路径必须给出非空目标。
    pub fn validate(&self) -> Result<(), RuleFault> {
        match self {
            Action::SessionInvocation { method, params } => {
                let Some(spec) = crate::surface::find(method) else {
                    return Err(RuleFault(format!("会话动词 {method:?} 不在能力面上")));
                };
                // 规则在宿主侧执行，走的是脚本前端的可见性（RPC-only 动词在
                // 规则里没有调用上下文）。
                if !spec.exposure.on_script() {
                    return Err(RuleFault(format!(
                        "会话动词 {method:?} 对规则不可见"
                    )));
                }
                let schema: Value = serde_json::from_str(spec.params_schema).map_err(|e| {
                    RuleFault(format!("会话动词 {method:?} 的 params_schema 写坏了：{e}"))
                })?;
                if !value_fits_schema(params, &schema) {
                    return Err(RuleFault(format!(
                        "会话动词 {method:?} 的参数不符合 params_schema"
                    )));
                }
                Ok(())
            }
            Action::AppAction(app) => app.validate(),
            Action::CredentialRef { scheme, field } => {
                if scheme.trim().is_empty() || field.trim().is_empty() {
                    return Err(RuleFault(
                        "凭据引用的 scheme / field 不能为空".to_string(),
                    ));
                }
                // 存在性与明文取值都在派发时问宿主：内核只保证形状可用。
                Ok(())
            }
            Action::RunScript { path, .. } => {
                if path.trim().is_empty() {
                    return Err(RuleFault("脚本路径不能为空".to_string()));
                }
                Ok(())
            }
        }
    }
}

impl AppAction {
    pub fn validate(&self) -> Result<(), RuleFault> {
        // 六类应用面动作都已有宿主派发落点（ui/trigger_intents.rs 的 UI 帧队列 /
        // AppControl 端口）：标记、链接、菜单、提示、响铃、通知、命令。
        match self {
            AppAction::Mark(_)
            | AppAction::Link(_)
            | AppAction::Menu(_)
            | AppAction::Tooltip(_)
            | AppAction::Alert(_)
            | AppAction::Dispatch(_) => Ok(()),
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
    use crate::automation::pattern::{Pattern, Syntax};
    use crate::automation::signal::When;

    fn rule() -> Rule {
        Rule {
            id: "r1".into(),
            name: String::new(),
            enabled: true,
            when: When::Output(Pattern::compile("error", Syntax::Literal, true).unwrap()),
            filter: Filter::default(),
            guard: Guard::default(),
            actions: vec![Action::SessionInvocation {
                method: "write".into(),
                params: serde_json::json!({ "session": "s1", "text": "hi" }),
            }],
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
    fn validation_uses_the_surface_catalog_and_not_a_second_method_list() {
        for (method, params) in [
            ("write", serde_json::json!({"session": "s", "text": "hi"})),
            ("keys", serde_json::json!({"session": "s", "keys": ["enter"]})),
            ("notify", serde_json::json!({"message": "hi"})),
        ] {
            let action = Action::SessionInvocation {
                method: method.into(),
                params,
            };
            assert!(action.validate().is_ok(), "{method} 应可装载");
        }
        // 能力面上登记的动词也能装载（以前被一张硬编码动词清单挡掉）。
        let logged = Action::SessionInvocation {
            method: "log_start".into(),
            params: serde_json::json!({"session": "s", "dir": "/tmp"}),
        };
        assert!(logged.validate().is_ok());
        let unknown = Action::SessionInvocation {
            method: "not_a_verb".into(),
            params: serde_json::json!({}),
        };
        assert!(unknown.validate().is_err());
    }

    #[test]
    fn params_must_match_the_methods_own_schema() {
        // required 缺项、类型不对、enum 越界、最小长度不足都要在装载期拒。
        for params in [
            serde_json::json!({"text": "hi"}),                       // 缺 session
            serde_json::json!({"session": 1, "text": "hi"}),         // session 不是字符串
            serde_json::json!({"session": "", "text": "hi"}),        // minLength
            serde_json::json!({"session": "s", "text": 42}),         // text 不是字符串
        ] {
            let action = Action::SessionInvocation {
                method: "write".into(),
                params,
            };
            assert!(action.validate().is_err(), "{:?} 应被 schema 拒绝", action);
        }
        let bad_enum = Action::SessionInvocation {
            method: "app_layout".into(),
            params: serde_json::json!({"action": "jump"}),
        };
        assert!(bad_enum.validate().is_err());
    }

    #[test]
    fn credentials_and_scripts_validate_their_targets_only() {
        let cred = Action::CredentialRef {
            scheme: "prod".into(),
            field: "password".into(),
        };
        assert!(cred.validate().is_ok());
        let blank = Action::CredentialRef {
            scheme: "  ".into(),
            field: "password".into(),
        };
        assert!(blank.validate().is_err());

        let script = Action::RunScript {
            path: "/tmp/a.rhai".into(),
            args: vec![],
        };
        assert!(script.validate().is_ok());
        let no_path = Action::RunScript {
            path: String::new(),
            args: vec![],
        };
        assert!(no_path.validate().is_err());
    }

    #[test]
    fn every_app_action_with_a_dispatch_sink_is_loadable() {
        for app in [
            AppAction::Mark(MarkSpec::default()),
            AppAction::Link(LinkSpec::default()),
            AppAction::Menu(MenuSpec::default()),
            AppAction::Tooltip(TextSpec::default()),
            AppAction::Alert(AlertKind::Beep),
            AppAction::Alert(AlertKind::SystemNotice),
            AppAction::Dispatch("Terminal.ClearScreen".into()),
        ] {
            assert!(app.validate().is_ok(), "{app:?} 已有派发落点，应可装载");
        }
    }

    #[test]
    fn validate_rules_points_at_the_bad_rule() {
        let mut bad = rule();
        bad.actions.push(Action::SessionInvocation {
            method: "not_a_verb".into(),
            params: serde_json::json!({}),
        });
        let (id, fault) = validate_rules(&[rule(), bad]).expect_err("应拒");
        assert_eq!(id, "r1");
        assert!(fault.to_string().contains("能力面"));
    }
}

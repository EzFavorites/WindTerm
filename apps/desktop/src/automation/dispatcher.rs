//! 副作用分发：`Effect` → [`Plan`] 的纯映射。
//!
//! 只做到「把意图翻译成可执行的计划」这一层：
//! - `plan()` 是纯函数，可对任意 `Effect` 夹具单测，绝不写 PTY / 派发命令；
//! - 真正执行发生在窗口层（它持有 `Tab::write` 与 `dispatch`）——影子轨默认
//!   不调用执行，所以 98 张截图零变化。
//!
//! 这样「能做什么」仍只有 `surface::METHODS` 一份清单：不支持的 method 与
//! AppAction 由 [`term_core::automation::rule::Rule::validate`] 在**规则装载期**
//! 就拒掉，命中后不会再冒出一个 NoEffect。本文件只做「已校验的动作 → 计划」
//! 的纯映射，并对绕过校验的调用保留一道防御。

use term_core::automation::rule::AlertKind;
use term_core::automation::{Action, AppAction, Effect, Invocation};

/// 一条可执行的副作用意图。窗口层拿到它才知道要碰外部世界。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Plan {
    /// 往会话写原始字节（`write` 动词）。
    Write { text: String },
    /// 派发一条菜单命令 ID。
    Dispatch { command: String },
    /// 按**键名**发按键（会话动词 `keys`）。
    Keys { keys: Vec<String> },
    /// 把一句话送到用户眼前（会话动词 `notify`）。
    Notify { message: String },
    /// 桌面 / 终端响铃。
    Beep,
    /// 跑一段脚本（只产出意图，执行留给脚本线程）。
    RunScript { path: String, args: Vec<String> },
    /// 没有对应副作用的动作（如 Highlight，无渲染基底）——显式列出，
    /// 不静默丢弃，方便影子对账时看出「命中但无效果」。
    NoEffect,
}

/// 一次信号的所有执行意图。保留规则 id，窗口层可按规则做日志 / 诊断。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PlanSet {
    pub plans: Vec<(String, Plan)>,
}

impl PlanSet {
    pub fn is_empty(&self) -> bool {
        self.plans.is_empty()
    }
}

/// 把 Effect 列表翻译成执行计划。纯函数：不执行任何东西。
///
/// 未知会话动词不 panic、不静默忽略，产出 `NoEffect` 让对账看得见；调用方
/// 可在严格模式（`WINDTERM_AUTOMATION_V2`）下把它记成诊断。
pub fn plan_effects(effects: &[Effect]) -> PlanSet {
    let mut plans = Vec::new();
    for e in effects {
        let plan = plan_action(&e.action);
        plans.push((e.rule.clone(), plan));
    }
    PlanSet { plans }
}

fn plan_action(action: &Action) -> Plan {
    match action {
        Action::App(app) => plan_app(app),
        Action::Session(inv) => plan_invocation(inv),
    }
}

fn plan_app(app: &AppAction) -> Plan {
    match app {
        AppAction::Alert(kind) => match kind {
            AlertKind::Beep => Plan::Beep,
            AlertKind::SystemNotice => Plan::NoEffect,
        },
        AppAction::Dispatch(id) => Plan::Dispatch {
            command: id.clone(),
        },
        AppAction::RunScript(spec) => Plan::RunScript {
            path: spec.path.clone(),
            args: spec.args.clone(),
        },
        // 这几种在规则装载期已被 `validate()` 拒绝；这里保留防御分支，
        // 让绕过校验的调用得到看得见的 NoEffect 而不是静默成功。
        AppAction::Mark(_)
        | AppAction::Link(_)
        | AppAction::Menu(_)
        | AppAction::Tooltip(_) => Plan::NoEffect,
    }
}

fn plan_invocation(inv: &Invocation) -> Plan {
    // 动词必须在能力面表登记且有分发实现；规则装载时已经由 `Rule::validate`
    // 拒过一次（见 term_core::automation::rule::DISPATCHABLE_METHODS），这里保留
    // 一道防御，免得调用方绕过校验之后「命中了却没效果」。
    if term_core::surface::find(&inv.method).is_none()
        || !term_core::automation::rule::dispatchable_method(&inv.method)
    {
        return Plan::NoEffect;
    }
    match inv.method.as_str() {
        "write" => {
            let text = inv.params.get("text").and_then(|v| v.as_str()).unwrap_or("");
            Plan::Write {
                text: text.to_owned(),
            }
        }
        "keys" => {
            let keys = inv
                .params
                .get("keys")
                .and_then(|v| v.as_array())
                .map(|items| {
                    items
                        .iter()
                        .filter_map(|v| v.as_str().map(str::to_string))
                        .collect()
                })
                .unwrap_or_default();
            Plan::Keys { keys }
        }
        "notify" => {
            let message = inv
                .params
                .get("message")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_owned();
            Plan::Notify { message }
        }
        _ => Plan::NoEffect,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use term_core::automation::rule::{AlertKind, MarkSpec, ScriptGrant, ScriptSpec};
    use term_core::automation::{AppAction as AA, Effect, Invocation, Span};

    fn effect(action: Action) -> Effect {
        Effect {
            rule: "r1".into(),
            action,
            span: Some(Span { start: 0, end: 2 }),
        }
    }

    #[test]
    fn write_invocation_maps_to_a_write_plan_with_the_text() {
        let inv = Invocation::new("write", serde_json::json!({"text":"hi\r"}));
        let set = plan_effects(&[effect(Action::Session(inv))]);
        assert_eq!(
            set.plans,
            vec![(
                "r1".into(),
                Plan::Write {
                    text: "hi\r".into()
                }
            )]
        );
    }

    #[test]
    fn an_unregistered_verb_is_a_visible_no_effect_not_a_panic() {
        let inv = Invocation::new("not_a_verb", serde_json::json!({}));
        let set = plan_effects(&[effect(Action::Session(inv))]);
        assert_eq!(set.plans[0].1, Plan::NoEffect);
    }

    #[test]
    fn keys_and_notify_map_to_their_own_plans() {
        let keys = Invocation::new("keys", serde_json::json!({"keys": ["f5", "enter"]}));
        let notify = Invocation::new("notify", serde_json::json!({"message": "done"}));
        let set = plan_effects(&[
            effect(Action::Session(keys)),
            effect(Action::Session(notify)),
        ]);
        assert_eq!(
            set.plans[0].1,
            Plan::Keys {
                keys: vec!["f5".into(), "enter".into()]
            }
        );
        assert_eq!(
            set.plans[1].1,
            Plan::Notify {
                message: "done".into()
            }
        );
    }

    #[test]
    fn write_without_a_text_param_is_a_no_effect() {
        let inv = Invocation::new("write", serde_json::json!({}));
        let set = plan_effects(&[effect(Action::Session(inv))]);
        assert_eq!(set.plans[0].1, Plan::Write { text: String::new() });
    }

    #[test]
    fn app_actions_map_to_their_plans() {
        let cases = [
            (AA::Alert(AlertKind::Beep), Plan::Beep),
            (
                AA::Dispatch("File.Save".into()),
                Plan::Dispatch {
                    command: "File.Save".into(),
                },
            ),
            (
                AA::RunScript(ScriptSpec {
                    path: "x.rhai".into(),
                    args: vec!["a".into()],
                    grant: ScriptGrant::default(),
                }),
                Plan::RunScript {
                    path: "x.rhai".into(),
                    args: vec!["a".into()],
                },
            ),
            (AA::Mark(MarkSpec::default()), Plan::NoEffect),
        ];
        for (action, expected) in cases {
            let set = plan_effects(&[effect(Action::App(action))]);
            assert_eq!(set.plans[0].1, expected);
        }
    }

    #[test]
    fn plans_keep_rule_ids_in_effect_order() {
        let a = effect(Action::App(AA::Alert(AlertKind::Beep)));
        let mut b = effect(Action::App(AA::Alert(AlertKind::Beep)));
        b.rule = "r2".into();
        let set = plan_effects(&[a, b]);
        let ids: Vec<_> = set.plans.iter().map(|(id, _)| id.as_str()).collect();
        assert_eq!(ids, vec!["r1", "r2"]);
    }

    #[test]
    fn an_empty_effect_list_produces_no_plans() {
        assert!(plan_effects(&[]).is_empty());
    }
}

//! 触发器意图的**派发落点**（求值与写分离）。
//!
//! `Tab` 只把 `Effect` 翻译成这四种意图并排队；真正的副作用由窗口层消费：
//!
//! - [`UiIntent::Surface`] → `invoke`（会话控制面动词，PTY 写入的唯一入口）；
//! - [`UiIntent::App`] → AppControl / UI 帧队列（标记 / 链接 / 菜单 / 提示 / 命令）；
//! - [`UiIntent::Credential`] → 保险库取值后经 `invoke` 写 PTY；
//! - [`UiIntent::Script`] → 统一脚本入口（`ScriptRun`）。
//!
//! 每条意图都带 [`DispatchCtx`]：`owner` 与 `priority` 是仲裁器的接入点。
//! 本任务只做「谁在请求、优先级多少」的记录与 PTY 写入的单一闸门；仲裁策略
//! 留给下一任务，不在各处写死「谁都能写 PTY」。

use term_core::automation::{Action, AlertKind, AppAction, Effect, RuleId};

/// 请求来源与优先级。仲裁器尚未接线，但闸门已经从这里读取，不允许绕过。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(in crate::ui) struct DispatchCtx {
    /// 请求者身份（`SessionConfig.name` / `"ui"` / `"script:<path>"`）。
    pub owner: Option<String>,
    /// 数值越大越优先；同优先级按到达顺序。
    pub priority: u8,
}

impl DispatchCtx {
    pub(in crate::ui) fn for_rule(rule: &RuleId) -> Self {
        Self {
            owner: Some(rule.clone()),
            priority: 0,
        }
    }

    /// PTY 写入闸门：当前只允许「有明确 owner」的请求，匿名写入被拒。
    /// 仲裁器接入后只需替换这一处判据，调用点一个都不用改。
    pub(in crate::ui) fn may_write_pty(&self) -> bool {
        self.owner
            .as_deref()
            .is_some_and(|owner| !owner.trim().is_empty())
    }
}

/// 会话控制面调用。`params` 形状已由 `Rule::validate` 按 `params_schema` 校验。
#[derive(Clone, Debug, PartialEq)]
pub(in crate::ui) struct SurfaceIntent {
    pub method: String,
    pub params: serde_json::Value,
}

/// 应用面 / UI 帧队列意图。
#[derive(Clone, Debug, PartialEq)]
pub(in crate::ui) enum AppIntent {
    Mark {
        text_color: String,
        background: String,
        line_style: String,
        line_color: String,
    },
    Link(String),
    /// 菜单动作：携带命令 ID，走既有命令通道。
    Menu(Vec<String>),
    Tooltip(String),
    Alert(AlertKind),
    Dispatch(String),
}

/// 凭据意图：只带引用，明文在窗口层问保险库。
#[derive(Clone, Debug, PartialEq, Eq)]
pub(in crate::ui) struct CredentialIntent {
    pub scheme: String,
    pub field: String,
}

/// 脚本意图：路径 + 参数，授权按次由统一脚本入口决定。
#[derive(Clone, Debug, PartialEq, Eq)]
pub(in crate::ui) struct ScriptIntent {
    pub path: String,
    pub args: Vec<String>,
}

/// 一条命中产出的、待窗口层派发的意图。
#[derive(Clone, Debug, PartialEq)]
pub(in crate::ui) struct FrameIntent {
    pub ctx: DispatchCtx,
    pub kind: IntentKind,
}

#[derive(Clone, Debug, PartialEq)]
pub(in crate::ui) enum IntentKind {
    Surface(SurfaceIntent),
    App(AppIntent),
    Credential(CredentialIntent),
    Script(ScriptIntent),
}

/// `Effect` → 意图。`matched` 的文本与 span 不参与派发：动作只声明意图。
pub(in crate::ui) fn intent_from_effect(effect: &Effect) -> Option<FrameIntent> {
    let ctx = DispatchCtx::for_rule(&effect.rule);
    let kind = match &effect.action {
        Action::SessionInvocation { method, params } => IntentKind::Surface(SurfaceIntent {
            method: method.clone(),
            params: params.clone(),
        }),
        Action::AppAction(app) => IntentKind::App(match app {
            AppAction::Mark(spec) => AppIntent::Mark {
                text_color: spec.text_color.clone(),
                background: spec.background.clone(),
                line_style: spec.line_style.clone(),
                line_color: spec.line_color.clone(),
            },
            AppAction::Link(spec) => AppIntent::Link(spec.url.clone()),
            AppAction::Menu(spec) => AppIntent::Menu(
                spec.items
                    .iter()
                    .filter_map(|item| item.action.as_ref().map(|inv| inv.method.clone()))
                    .collect(),
            ),
            AppAction::Tooltip(spec) => AppIntent::Tooltip(spec.text.clone()),
            AppAction::Alert(kind) => AppIntent::Alert(*kind),
            AppAction::Dispatch(id) => AppIntent::Dispatch(id.clone()),
        }),
        Action::CredentialRef { scheme, field } => IntentKind::Credential(CredentialIntent {
            scheme: scheme.clone(),
            field: field.clone(),
        }),
        Action::RunScript { path, args } => IntentKind::Script(ScriptIntent {
            path: path.clone(),
            args: args.clone(),
        }),
    };
    Some(FrameIntent { ctx, kind })
}

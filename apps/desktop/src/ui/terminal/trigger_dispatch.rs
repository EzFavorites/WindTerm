//! 触发器意图的**派发落点**（求值与写分离）。
//!
//! `Tab` 只把 `Effect` 翻译成这四种意图并排队；真正的副作用由窗口层消费：
//!
//! - [`IntentKind::Surface`] → `invoke`（会话控制面动词，PTY 写入的唯一入口）；
//! - [`IntentKind::App`] → AppControl / UI 帧队列（标记 / 链接 / 菜单 / 提示 / 命令）；
//! - [`IntentKind::Credential`] → 保险库取值后经 `invoke` 写 PTY；
//! - [`IntentKind::Script`] → 统一脚本入口（`ScriptRun`）。
//!
//! 写意图可带内核算出的 `Release` 门控（`Prompt` / `LineBreak` / `Later`），
//! 由帧末派发层排队到正确时机；其余三类不排队。
//!
//! 每条意图都带 [`DispatchCtx`]：`owner` 与 `priority` 是仲裁器的接入点。
//! 本任务只做「谁在请求、优先级多少」的记录与 PTY 写入的单一闸门；仲裁策略
//! 留给下一任务，不在各处写死「谁都能写 PTY」。

use std::time::Duration;

use term_core::automation::{Action, AlertKind, AppAction, Effect, Release, RuleId, Ticks};

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
///
/// `release` / `since` 只管会话写意图（见 [`FrameIntent::ready`]）：
/// 事件类与 UI 类动作不排队，命中当帧就走 `trigger_intents`。
#[derive(Clone, Debug, PartialEq)]
pub(in crate::ui) struct FrameIntent {
    pub ctx: DispatchCtx,
    pub kind: IntentKind,
    pub release: Release,
    /// 命中时刻（毫秒刻度），`Release::Later` 从这一刻起算。
    pub since: Ticks,
}

impl FrameIntent {
    /// 只有会话写动词受发送门控；Mark / Alert / Dispatch / Script 立即发生。
    pub(in crate::ui) fn is_write(&self) -> bool {
        matches!(&self.kind, IntentKind::Surface(call) if call.method == "write")
    }

    /// 本帧是否该落地。等待提示符 / 换行只看这一帧的输出；
    /// `Later` 只看注入的时钟（与求值器同一时基，不读墙钟）。
    pub(in crate::ui) fn ready(&self, now: u64, output: Option<&str>) -> bool {
        if !self.is_write() {
            return true;
        }
        match self.release {
            Release::Immediate => true,
            Release::Prompt => output.is_some_and(term_app::trigger_builtins::looks_like_prompt),
            Release::LineBreak => output.is_some_and(|text| text.contains('\n')),
            Release::Later(delay) => {
                // `now` 是秒（与求值器同源），`since` 是 Ticks（毫秒）。
                let elapsed_ms = Ticks::from_secs(now).0.saturating_sub(self.since.0);
                Duration::from_millis(elapsed_ms) >= delay
            }
        }
    }
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
    Some(FrameIntent {
        ctx,
        kind,
        release: effect.release,
        since: Ticks::ZERO,
    })
}

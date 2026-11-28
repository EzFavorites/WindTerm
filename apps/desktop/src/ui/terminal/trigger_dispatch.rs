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
//! 每条意图都带 [`DispatchCtx`]：`source` / `priority` / `owner` 与仲裁许可
//! 一并带走。落地前必须用 [`DispatchCtx::may_write_pty`] 复核许可——用户按键
//! 或独占抢占发生在排队之后时，旧许可已经失效。

use std::time::Duration;

use serde_json::{json, Value};
use term_core::automation::{
    Action, AlertKind, AppAction, CredentialField, Effect, Invocation, Priority, Release, RuleId,
    Ticks,
};
use term_core::session_runtime::SessionId;

use term_core::automation::{Arbitrator, Grant};

/// 谁、以什么优先级、在哪个会话上请求写入。`owner` 是请求时刻的会话 owner。
#[derive(Clone, Debug, PartialEq, Eq)]
pub(in crate::ui) struct DispatchCtx {
    pub session: SessionId,
    /// 请求者身份（规则 id / `api:write` / `keyboard`）。
    pub source: String,
    pub priority: Priority,
    /// 请求时刻的 owner（可能不是自己）。仅用于提示，不用于判据。
    pub owner: Option<String>,
    /// 仲裁许可。没有许可的意图根本不产生 PTY 写。
    pub grant: Option<Grant>,
}

impl DispatchCtx {
    pub(in crate::ui) fn for_rule(
        session: SessionId,
        rule: &RuleId,
        owner: Option<String>,
        grant: Option<Grant>,
    ) -> Self {
        Self {
            session,
            source: rule.clone(),
            priority: Priority::P3Rule,
            owner,
            grant,
        }
    }

    /// PTY 写入闸门：许可必须在落地时依然有效（按键 / 独占抢占会让它失效）。
    pub(in crate::ui) fn may_write_pty(&self, arbiter: &Arbitrator) -> bool {
        self.grant
            .is_some_and(|grant| arbiter.is_valid(self.session, &grant))
    }

    /// 同帧排序键：数值越大越优先，同优先级按到达序号（不交错）。
    pub(in crate::ui) fn seq(&self) -> u64 {
        self.grant.map_or(u64::MAX, |grant| grant.seq)
    }
}

/// 会话控制面调用。`params` 形状已由 `Rule::validate` 按 `params_schema` 校验。
#[derive(Clone, Debug, PartialEq)]
pub(in crate::ui) struct SurfaceIntent {
    pub method: String,
    pub params: Value,
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
    /// 菜单动作：**完整 Invocation**（method + params）一起交给命令通道。
    Menu(Vec<Invocation>),
    Tooltip(String),
    Alert(AlertKind),
    Dispatch(String),
}

/// 命令通道的一格：菜单 ID 可以没有参数，会话动词必须把 params 带来。
#[derive(Clone, Debug, PartialEq)]
pub(in crate::ui) struct CommandCall {
    pub method: String,
    pub params: Value,
}

impl CommandCall {
    pub(in crate::ui) fn plain(id: impl Into<String>) -> Self {
        Self {
            method: id.into(),
            params: json!({}),
        }
    }

    pub(in crate::ui) fn from_invocation(inv: Invocation) -> Self {
        Self {
            method: inv.method,
            params: inv.params,
        }
    }
}

impl From<String> for CommandCall {
    fn from(id: String) -> Self {
        Self::plain(id)
    }
}

/// 凭据意图：只带引用与**类型化字段**，明文在窗口层问保险库。
#[derive(Clone, Debug, PartialEq, Eq)]
pub(in crate::ui) struct CredentialIntent {
    pub scheme: String,
    pub field: CredentialField,
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
    /// 只有会话写动词受发送门控与仲裁排序；所有其它动作立即发生。
    pub(in crate::ui) fn is_write(&self) -> bool {
        matches!(&self.kind, IntentKind::Surface(call) if call.method == "write" || call.method == "keys")
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
pub(in crate::ui) fn intent_from_effect(effect: &Effect, ctx: DispatchCtx) -> Option<FrameIntent> {
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
            AppAction::Menu(spec) => {
                AppIntent::Menu(spec.items.iter().filter_map(|item| item.action.clone()).collect())
            }
            AppAction::Tooltip(spec) => AppIntent::Tooltip(spec.text.clone()),
            AppAction::Alert(kind) => AppIntent::Alert(*kind),
            AppAction::Dispatch(id) => AppIntent::Dispatch(id.clone()),
        }),
        Action::CredentialRef { scheme, field } => IntentKind::Credential(CredentialIntent {
            scheme: scheme.clone(),
            field: *field,
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

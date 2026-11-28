//! 帧末派发 `FrameIntent`：求值与写分离后的**唯一副作用落点**。
//!
//! 四类意图各有明确去处：
//!
//! - `Surface` → `Surface::invoke` / `invoke_granted`：动词只在桥接的
//!   `DeskSurface` 实现一次；PTY 写入前再过仲裁器复核许可；
//! - `App` → AppControl / UI 帧队列：标记、链接、菜单、提示、通知、命令；
//! - `Credential` → 按类型化字段从保险库取值，转成一次带仲裁许可的 write；
//! - `Script` → `ScriptRun` 统一入口（按次授权由 `RunConfig` 决定）。
//!
//! # 发送门控与仲裁
//!
//! 内核把「何时放行」随 `Effect.release` 交出来。只有会话写意图受门控：
//! `Prompt` / `LineBreak` 等本帧输出事件，`Later` 等注入时钟，到点才落地。
//! 落地的唯一漏斗是 `PTY`；同帧多条写按 `(priority, seq)` 排序后逐条落地，
//! 绝不拼接文本。被压制的写**不产生**（连 `pool.send` 都不调），只留痕。

#[cfg(test)]
#[path = "trigger_arbiter_tests.rs"]
mod trigger_arbiter_tests;

use serde_json::Value;
use term_core::automation::{AlertKind, CredentialField, Release, Ticks};
use term_core::surface_error::{ErrorKind, SurfaceError};

use super::*;
use crate::bridge::surface::{arbiter_session, shared_arbiter, shared_arbiter_arc, DeskSurface};
use crate::ui::terminal::session::{decode_send_text, note_escape_error, open_link_with};
use crate::ui::terminal::tab_slot::{tab_of, TabSlot};
use crate::ui::terminal::trigger_dispatch::{
    AppIntent, CommandCall, CredentialIntent, DispatchCtx, FrameIntent, IntentKind, ScriptIntent,
    SurfaceIntent,
};

/// 用户按键是 P0 抢占信号：释放 owner，让此前排队的写许可全部失效。
pub(in crate::ui) fn user_key_preempt(tab: &Tab) {
    let session = tab.session.unwrap_or(tab.identity);
    let key = arbiter_session(&tab.pool, session);
    let mut arbiter = shared_arbiter().lock().unwrap_or_else(|e| e.into_inner());
    arbiter.user_key_pressed(key);
}

/// 会话相位切换（连接期只许 P1 写；Live 放开 P3/P2）。
pub(in crate::ui) fn set_phase(tab: &Tab, phase: term_core::automation::Phase) {
    let session = tab.session.unwrap_or(tab.identity);
    let key = arbiter_session(&tab.pool, session);
    let mut arbiter = shared_arbiter().lock().unwrap_or_else(|e| e.into_inner());
    arbiter.set_phase(key, phase);
}

/// 把一条 surface 意图落到会话上。PTY 写入只有这一条路，先过仲裁闸门，
/// 动词实现来自 `DeskSurface`（与 API 同一份），不在这里手写 write/keys 分派。
fn invoke_surface(
    tab: &mut Tab,
    ctx: &DispatchCtx,
    call: &SurfaceIntent,
    rule: &str,
    note: &mut Option<String>,
) {
    let Some(grant) = ctx.grant else {
        if note.is_none() {
            *note = Some(format!("触发器 {rule} 的写入被仲裁拒绝（无许可）"));
        }
        return;
    };
    {
        let arbiter = shared_arbiter().lock().unwrap_or_else(|e| e.into_inner());
        if !ctx.may_write_pty(&arbiter) {
            if note.is_none() {
                *note = Some(format!(
                    "触发器 {rule} 的写入被用户按键 / 独占调用抢占（已压制并留痕）"
                ));
            }
            return;
        }
    }
    // 注意：上面只是早退的提示，不是判据。真正的 is_valid + pool.send 在
    // `DeskSurface::send_granted` 的同一把 arbiter 锁里复核；这里若再拿一次
    // 锁判完就写，仍会留下一个能被抢占插进来的 TOCTOU 窗口（P1-2）。

    let mut params = call.params.clone();
    let session = params
        .get("session")
        .and_then(Value::as_str)
        .unwrap_or("current")
        .to_owned();
    if session == "current" || !params.get("session").is_some_and(Value::is_string) {
        params["session"] = Value::String(tab.config.name.clone());
    }
    let text = params.get("text").and_then(Value::as_str).unwrap_or("").to_owned();

    // 两个 UI 专有的落点：本地发送没有远端会话；notify 是 HumanChannel，
    // 不属于 DeskSurface 的远程能力面。除此之外全部交给唯一的动词实现。
    if call.method == "write" && session == "local" {
        let mut owned = text.clone();
        owned.push('\r');
        match term_core::send_escapes::decode(&owned) {
            Ok(bytes) => tab
                .pending_local_sends
                .push(String::from_utf8_lossy(&bytes).into_owned()),
            Err(err) => note_escape_error(note, rule, "Local send line", &err),
        }
        return;
    }
    if call.method == "notify" {
        if note.is_none() {
            *note = Some(text);
        }
        return;
    }

    let surface = DeskSurface::with_arbiter(tab.pool.clone(), shared_arbiter_arc());
    let result = if call.method == "write" {
        match decode_send_text(&text) {
            Ok(bytes) => {
                params["text"] = Value::String(String::from_utf8_lossy(&bytes).into_owned());
                surface.invoke_granted("write", &params, &grant)
            }
            Err(err) => {
                note_escape_error(note, rule, "Send text", &err);
                return;
            }
        }
    } else {
        surface.invoke_granted(&call.method, &params, &grant)
    };
    if let Err(err) = result {
        let SurfaceError { kind, message, .. } = err;
        if note.is_none() {
            // Busy 是可重试的时序冲突，Suppressed 是本次没产生写，与
            // 「锁屏 / 保险库未解锁」的 Locked 不是一回事，文案要说清。
            *note = Some(match kind {
                ErrorKind::Busy | ErrorKind::Suppressed => {
                    format!("触发器 {rule} 的写入未落地：{message}（稍后重试）")
                }
                _ => format!("触发器 {rule} 的 {kind:?} 未落地：{message}"),
            });
        }
    }
}

/// 应用面意图在本标签上落地。只需要 tab，所以帧泵与帧末走的是同一条路径。
fn dispatch_app(
    tab: &mut Tab,
    app: AppIntent,
    run_commands: &mut Vec<CommandCall>,
    note: &mut Option<String>,
) {
    match app {
        AppIntent::Mark { .. } => {
            if let Some((row, _)) = tab.view.cursor {
                let row = row as i64;
                if !tab.trigger_highlight_rows.contains(&row) {
                    tab.trigger_highlight_rows.push(row);
                    if tab.trigger_highlight_rows.len() > 64 {
                        tab.trigger_highlight_rows.remove(0);
                    }
                }
            }
        }
        AppIntent::Link(url) => open_link_with(&url, crate::sysopen::open),
        // 完整 Invocation（method + params）交给命令通道，不再只取 method。
        AppIntent::Menu(invocations) => run_commands.extend(
            invocations.into_iter().map(CommandCall::from_invocation),
        ),
        AppIntent::Dispatch(id) => run_commands.push(CommandCall::plain(id)),
        AppIntent::Tooltip(text) => {
            if note.is_none() {
                *note = Some(text);
            }
        }
        AppIntent::Alert(AlertKind::Beep) => {
            tab.write(&[0x07]);
        }
        AppIntent::Alert(AlertKind::SystemNotice) => {
            if note.is_none() {
                *note = Some("触发器通知".to_owned());
            }
        }
    }
}

/// 同帧写意图按 `(priority, seq)` 排序：高优先级先落地，同级按到达顺序，
/// 两条文本绝不交错。
fn order_writes(intents: &mut Vec<FrameIntent>) {
    intents.sort_by(|a, b| {
        let pa = a.ctx.priority.rank();
        let pb = b.ctx.priority.rank();
        pb.cmp(&pa).then(a.ctx.seq().cmp(&b.ctx.seq()))
    });
}

/// 推进一条标签上的发送门控：本帧输出放行 Prompt / LineBreak，注入时钟放行
/// Later，未到期的写意图原样留队。
pub(in crate::ui) fn drain_tab(
    tab: &mut Tab,
    now: u64,
    output: Option<&str>,
    run_commands: &mut Vec<CommandCall>,
    note: &mut Option<String>,
) {
    let mut pending = Vec::new();
    let mut ready = Vec::new();
    for intent in std::mem::take(&mut tab.pending_intents) {
        if intent.ready(now, output) {
            ready.push(intent);
        } else {
            pending.push(intent);
        }
    }
    // 同一帧的写按 (priority, seq) 落地：帧泵内与帧末走同一个排序，
    // 否则先到的低优先级规则会在窗口级排序之前就把字节送进 PTY。
    order_writes(&mut ready);
    for FrameIntent { ctx, kind, release, since } in ready {
        match kind {
            IntentKind::Surface(call) => {
                let rule = ctx.source.clone();
                invoke_surface(tab, &ctx, &call, &rule, note);
            }
            IntentKind::App(app) => dispatch_app(tab, app, run_commands, note),
            // 凭据 / 脚本要窗口上下文（保险库 / 会话池 / 运行窗口），留给窗口级 drain。
            other => pending.push(FrameIntent { ctx, kind: other, release, since }),
        }
    }
    tab.pending_intents = pending;
}

/// 按枚举字段取值。取不到 / 为空一律报错，**绝不回落**到 KBI 第一项。
pub(in crate::ui) fn credential_value(
    win: &TerminalWindow,
    cred: &CredentialIntent,
) -> Result<String, String> {
    match cred.field {
        CredentialField::Username => win
            .onekeys
            .iter()
            .find(|key| key.name == cred.scheme)
            .map(|key| key.username.clone())
            .filter(|name| !name.is_empty())
            .ok_or_else(|| format!("凭据方案 {:?} 没有用户名", cred.scheme)),
        CredentialField::Password => {
            let secret = win.onekey_password(&cred.scheme);
            if secret.is_empty() {
                Err(format!("凭据方案 {:?} 的密码为空或未解锁", cred.scheme))
            } else {
                Ok(secret)
            }
        }
        CredentialField::KeyboardInteractive(index) => {
            let answers = win
                .vault
                .as_ref()
                .and_then(|v| v.find(&term_core::onekey::ki_vault_id(&cred.scheme)))
                .and_then(|entry| entry.secret())
                .map(|secret| term_core::onekey::decode_ki_answers(secret));
            match answers
                .as_ref()
                .and_then(|answers| answers.get(index))
                .filter(|answer| !answer.is_empty())
            {
                Some(answer) => Ok(answer.clone()),
                None => Err(format!(
                    "凭据方案 {:?} 的第 {index} 个键盘交互答案不存在或为空",
                    cred.scheme
                )),
            }
        }
    }
}

/// 凭据意图 → 带仲裁许可的 write。明文只在窗口层出现，之后不再跨层传递。
fn resolve_credential(
    win: &mut TerminalWindow,
    ctx: &mut DispatchCtx,
    cred: &CredentialIntent,
    note: &mut Option<String>,
) -> Option<SurfaceIntent> {
    let secret = match credential_value(win, cred) {
        Ok(value) => value,
        Err(reason) => {
            if note.is_none() {
                *note = Some(reason);
            }
            return None;
        }
    };
    let session = win
        .tabs
        .get(win.active_slot())
        .map(|tab| tab.session.unwrap_or(tab.identity));
    let Some(id) = session else {
        if note.is_none() {
            *note = Some("凭据写入需要一个活会话".to_owned());
        }
        return None;
    };
    let key = arbiter_session(&win.pool, id);
    let grant = {
        let mut arbiter = shared_arbiter().lock().unwrap_or_else(|e| e.into_inner());
        arbiter.request(key, &ctx.source, ctx.priority, false)
    };
    let grant = match grant {
        Ok(grant) => grant,
        Err(denied) => {
            if note.is_none() {
                *note = Some(denied.as_message());
            }
            return None;
        }
    };
    ctx.grant = Some(grant);
    Some(SurfaceIntent {
        method: "write".into(),
        params: serde_json::json!({ "session": "current", "text": secret }),
    })
}

/// 把一次命中的四类意图派发出去。`note` 是本帧局部缓冲，调用方帧末交回窗口。
pub(in crate::ui) fn drain(
    win: &mut TerminalWindow,
    run_commands: &mut Vec<CommandCall>,
    note: &mut Option<String>,
) {
    // Run Command 的回执每帧收一次：UI 线程只做 try_recv / kill，真正的
    // 子进程等待在后台线程。成功输出写回活动会话，失败 / 超时升为提示。
    for done in crate::ui::terminal::trigger_dispatch::poll_commands() {
        if let Some(text) = done.note() {
            if note.is_none() {
                *note = Some(text);
            }
        } else {
            let stdout = done.stdout();
            if !stdout.is_empty() {
                let active = win.active_slot();
                if let Some(tab) = win.tabs.get_mut(active) {
                    tab.write(stdout);
                }
            }
        }
    }

    let now = crate::ui::terminal::session::now_secs();
    // 帧泵末尾已经做过一次；这里再走同一条路径，保证「帧末才入队」的
    // 交互命中（IP 悬停 / 粘贴）也按同样规则落地，且不会重复发已取走的项。
    for tab in win.tabs.iter_mut() {
        drain_tab(tab, now, None, run_commands, note);
    }
    for tab in win.shell_tabs.iter_mut() {
        drain_tab(tab, now, None, run_commands, note);
    }

    let mut batches: Vec<(TabSlot, Vec<FrameIntent>)> = Vec::new();
    for tab in win.tabs.iter_mut() {
        let intents = std::mem::take(&mut tab.pending_intents);
        let (deferred, for_window): (Vec<_>, Vec<_>) =
            intents.into_iter().partition(FrameIntent::is_write);
        // 未到期的写意图留在自己的标签上，等下一次帧泵 / 帧末再试。
        tab.pending_intents = deferred;
        batches.push((TabSlot::Main(tab.identity), for_window));
    }
    for tab in win.shell_tabs.iter_mut() {
        let intents = std::mem::take(&mut tab.pending_intents);
        let (deferred, for_window): (Vec<_>, Vec<_>) =
            intents.into_iter().partition(FrameIntent::is_write);
        tab.pending_intents = deferred;
        batches.push((TabSlot::Shell(tab.identity), for_window));
    }

    // 凭据先转成 write 意图：明文只在窗口层从保险库读一次，之后不再跨层传递。
    for (_, batch) in &mut batches {
        for intent in batch.iter_mut() {
            let IntentKind::Credential(cred) = &intent.kind else {
                continue;
            };
            let cred = cred.clone();
            if let Some(call) = resolve_credential(win, &mut intent.ctx, &cred, note) {
                intent.kind = IntentKind::Surface(call);
                // 明文已经到窗口层，不再受规则里的发送门控。
                intent.release = Release::Immediate;
                intent.since = Ticks::ZERO;
            } else {
                intent.kind = IntentKind::App(AppIntent::Tooltip(String::new()));
            }
        }
    }

    let mut scripts: Vec<(TabSlot, ScriptIntent, DispatchCtx)> = Vec::new();
    for (slot, mut batch) in batches {
        // 同帧写按优先级排序后一次落地，不同文本的 SendText 绝不拼接。
        order_writes(&mut batch);
        for FrameIntent { ctx, kind, .. } in batch {
            match kind {
                IntentKind::Surface(call) => {
                    let rule = ctx.source.clone();
                    if let Some(tab) = tab_of(&mut win.tabs, &mut win.shell_tabs, slot) {
                        invoke_surface(tab, &ctx, &call, &rule, note);
                    }
                }
                IntentKind::App(app) => {
                    if let Some(tab) = tab_of(&mut win.tabs, &mut win.shell_tabs, slot) {
                        dispatch_app(tab, app, run_commands, note);
                    }
                }
                IntentKind::Script(script) => scripts.push((slot, script, ctx)),
                IntentKind::Credential(_) => unreachable!("凭据已在上面转成 write 意图"),
            }
        }
    }

    // 脚本走与运行窗口 / 停靠面板同一个 `ScriptRun` 入口，授权按次决定。
    for (slot, script, ctx) in scripts {
        let Some(tab) = tab_of(&mut win.tabs, &mut win.shell_tabs, slot) else {
            continue;
        };
        let (session, read_only) = (tab.session, tab.local_mode);
        let _ = tab;
        let Some(id) = session else {
            if note.is_none() {
                *note = Some("触发器脚本需要一个活会话".to_owned());
            }
            continue;
        };
        if read_only {
            if note.is_none() {
                *note = Some("只读查看时不执行触发器脚本".to_owned());
            }
            continue;
        }
        let who = ctx.source;
        let options = win.script_run_options(&who);
        match crate::script_run::read_source(&script.path)
            .map_err(|e| e)
            .and_then(|source| crate::script_run::ScriptRun::start(
                win.pool.clone(),
                id,
                source,
                options,
            ))
        {
            Ok(run) => {
                // 句柄要留在窗口上：提前 drop 会把刚起来的脚本收掉。
                win.trigger_script_runs.push(run);
                win.ctx.request_repaint();
            }
            Err(message) => {
                if note.is_none() {
                    *note = Some(format!("触发器脚本 {}: {message}", script.path));
                }
            }
        }
    }
}

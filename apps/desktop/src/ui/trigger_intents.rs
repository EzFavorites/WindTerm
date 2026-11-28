//! 帧末派发 `FrameIntent`：求值与写分离后的**唯一副作用落点**。
//!
//! 四类意图各有明确去处，仲裁器从 `DispatchCtx` 的 owner / priority 接入：
//!
//! - `Surface` → `invoke`：只经 `Tab::write` 写 PTY，闸门检查 owner；
//! - `App` → AppControl / UI 帧队列：标记、链接、菜单、提示、通知、命令；
//! - `Credential` → 保险库取值后转成一次 `write` 意图；
//! - `Script` → `ScriptRun` 统一入口（按次授权由 `RunConfig` 决定）。
//!
//! # 发送门控
//!
//! 内核把「何时放行」随 `Effect.release` 交出来。只有会话写意图受门控：
//! `Prompt` / `LineBreak` 等本帧输出事件，`Later` 等注入时钟，到点才到这里
//! 的 `invoke`。Mark / Alert / Dispatch 等一律当帧发生，绝不排进队列。

use serde_json::Value;
use term_core::automation::{AlertKind, Release, Ticks};

use super::*;
use crate::ui::terminal::session::{decode_send_text, note_escape_error, open_link_with};
use crate::ui::terminal::tab_slot::{tab_of, TabSlot};
use crate::ui::terminal::trigger_dispatch::{
    AppIntent, DispatchCtx, FrameIntent, IntentKind, ScriptIntent, SurfaceIntent,
};

/// 把一条 surface 意图落到会话上。PTY 写入只有这一条路，且先过 owner 闸门。
fn invoke_surface(
    tab: &mut Tab,
    ctx: &DispatchCtx,
    call: &SurfaceIntent,
    rule: &str,
    note: &mut Option<String>,
) {
    if !ctx.may_write_pty() {
        if note.is_none() {
            *note = Some(format!("触发器 {rule} 的写入被闸门拒绝（owner 缺失）"));
        }
        return;
    }
    let session = call.params.get("session").and_then(Value::as_str).unwrap_or("current");
    let text = call.params.get("text").and_then(Value::as_str).unwrap_or("");
    match call.method.as_str() {
        "write" => {
            if session == "local" {
                let mut owned = text.to_owned();
                owned.push('\r');
                match term_core::send_escapes::decode(&owned) {
                    Ok(bytes) => tab
                        .pending_local_sends
                        .push(String::from_utf8_lossy(&bytes).into_owned()),
                    Err(err) => note_escape_error(note, rule, "Local send line", &err),
                }
                return;
            }
            match decode_send_text(text) {
                Ok(bytes) => {
                    tab.write(&bytes);
                }
                Err(err) => note_escape_error(note, rule, "Send text", &err),
            }
        }
        "keys" => {
            let keys = call
                .params
                .get("keys")
                .and_then(Value::as_array)
                .map(|a| a.iter().filter_map(Value::as_str).collect::<Vec<_>>())
                .unwrap_or_default();
            let mut bytes = Vec::new();
            for key in keys {
                match term_core::keys::key_bytes(key, false) {
                    Some(seq) => bytes.extend_from_slice(seq.as_bytes()),
                    None => {
                        if note.is_none() {
                            *note = Some(format!("触发器 {rule} 的按键 {key:?} 无法识别"));
                        }
                        return;
                    }
                }
            }
            tab.write(&bytes);
        }
        "notify" => {
            // HumanChannel：状态行提示，不写 PTY。
            if note.is_none() {
                *note = Some(text.to_owned());
            }
        }
        _ => {
            // 其它会话动词由桥 / 脚本前端的 dispatcher 提供；触发器宿主没有
            // 对应通道时明确上报，绝不假装执行成功。
            if note.is_none() {
                *note = Some(format!("触发器 {rule} 的会话动词 {:?} 没有宿主派发实现", call.method));
            }
        }
    }
}

/// 应用面意图在本标签上落地。只需要 tab，所以帧泵与帧末走的是同一条路径。
fn dispatch_app(
    tab: &mut Tab,
    app: AppIntent,
    run_commands: &mut Vec<String>,
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
        AppIntent::Menu(ids) => run_commands.extend(ids),
        AppIntent::Dispatch(id) => run_commands.push(id),
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

/// 推进一条标签上的发送门控：本帧输出放行 Prompt / LineBreak，注入时钟放行
/// Later，未到期的写意图原样留队。转义解码失败在这里写进本帧 `note`，
/// 由调用方 `commit_note` 抬到前景条——不写 stderr 冒充 UI 提示。
pub(in crate::ui) fn drain_tab(
    tab: &mut Tab,
    now: u64,
    output: Option<&str>,
    run_commands: &mut Vec<String>,
    note: &mut Option<String>,
) {
    let mut pending = Vec::new();
    for FrameIntent { ctx, kind, release, since } in std::mem::take(&mut tab.pending_intents) {
        let intent = FrameIntent { ctx, kind, release, since };
        if !intent.ready(now, output) {
            pending.push(intent);
            continue;
        }
        let FrameIntent { ctx, kind, release, since } = intent;
        match kind {
            IntentKind::Surface(call) => {
                let rule = ctx.owner.clone().unwrap_or_default();
                invoke_surface(tab, &ctx, &call, &rule, note);
            }
            IntentKind::App(app) => dispatch_app(tab, app, run_commands, note),
            // 凭据 / 脚本要窗口上下文（保险库 / 会话池 / 运行窗口），留给窗口级 drain。
            other => pending.push(FrameIntent { ctx, kind: other, release, since }),
        }
    }
    tab.pending_intents = pending;
}

/// 把一次命中的四类意图派发出去。`note` 是本帧局部缓冲，调用方帧末交回窗口。
pub(in crate::ui) fn drain(
    win: &mut TerminalWindow,
    run_commands: &mut Vec<String>,
    note: &mut Option<String>,
) {
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

    // 凭据先转为 write 意图：明文只在窗口层从保险库读一次，之后不再跨层传递。
    for (_, batch) in &mut batches {
        for intent in batch.iter_mut() {
            let IntentKind::Credential(cred) = &intent.kind else {
                continue;
            };
            let secret = if cred.field == "password" {
                win.onekey_password(&cred.scheme)
            } else {
                win.vault
                    .as_ref()
                    .and_then(|v| v.find(&term_core::onekey::ki_vault_id(&cred.scheme)))
                    .and_then(|e| e.secret())
                    .and_then(|s| term_core::onekey::decode_ki_answers(s).first().cloned())
                    .unwrap_or_default()
            };
            intent.kind = IntentKind::Surface(SurfaceIntent {
                method: "write".into(),
                params: serde_json::json!({ "session": "current", "text": secret }),
            });
            // 明文已经到窗口层，不再受规则里的发送门控。
            intent.release = Release::Immediate;
            intent.since = Ticks::ZERO;
        }
    }

    let mut scripts: Vec<(TabSlot, ScriptIntent, DispatchCtx)> = Vec::new();
    for (slot, batch) in batches {
        for FrameIntent { ctx, kind, .. } in batch {
            match kind {
                IntentKind::Surface(call) => {
                    let rule = ctx.owner.clone().unwrap_or_default();
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
        let who = ctx.owner.unwrap_or_else(|| script.path.clone());
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

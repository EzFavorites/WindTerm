//! 帧末派发 `FrameIntent`：求值与写分离后的**唯一副作用落点**。
//!
//! 四类意图各有明确去处，仲裁器从 `DispatchCtx` 的 owner / priority 接入：
//!
//! - `Surface` → `invoke`：只经 `Tab::write` 写 PTY，闸门检查 owner；
//! - `App` → AppControl / UI 帧队列：标记、链接、菜单、提示、通知、命令；
//! - `Credential` → 保险库取值后转成一次 `write` 意图；
//! - `Script` → `ScriptRun` 统一入口（按次授权由 `RunConfig` 决定）。

use serde_json::Value;
use term_core::automation::AlertKind;

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

/// 把一次命中的四类意图派发出去。`note` 是本帧局部缓冲，调用方帧末交回窗口。
pub(in crate::ui) fn drain(
    win: &mut TerminalWindow,
    run_commands: &mut Vec<String>,
    note: &mut Option<String>,
) {
    let mut batches: Vec<(TabSlot, Vec<FrameIntent>)> = Vec::new();
    for tab in win.tabs.iter_mut() {
        batches.push((TabSlot::Main(tab.identity), std::mem::take(&mut tab.pending_intents)));
    }
    for tab in win.shell_tabs.iter_mut() {
        batches.push((TabSlot::Shell(tab.identity), std::mem::take(&mut tab.pending_intents)));
    }
    if batches.is_empty() {
        return;
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
        }
    }

    let mut scripts: Vec<(TabSlot, ScriptIntent, DispatchCtx)> = Vec::new();
    let mut delayed_note: Option<String> = None;
    for (slot, batch) in batches {
        for FrameIntent { ctx, kind } in batch {
            match kind {
                IntentKind::Surface(call) => {
                    let rule = ctx.owner.clone().unwrap_or_default();
                    if let Some(tab) = tab_of(&mut win.tabs, &mut win.shell_tabs, slot) {
                        invoke_surface(tab, &ctx, &call, &rule, note);
                    }
                }
                IntentKind::App(app) => match app {
                    AppIntent::Mark { .. } => {
                        if let Some(tab) = tab_of(&mut win.tabs, &mut win.shell_tabs, slot) {
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
                    }
                    AppIntent::Link(url) => open_link_with(&url, crate::sysopen::open),
                    AppIntent::Menu(ids) => run_commands.extend(ids),
                    AppIntent::Dispatch(id) => run_commands.push(id),
                    AppIntent::Tooltip(text) => {
                        if delayed_note.is_none() {
                            delayed_note = Some(text);
                        }
                    }
                    AppIntent::Alert(AlertKind::Beep) => {
                        if let Some(tab) = tab_of(&mut win.tabs, &mut win.shell_tabs, slot) {
                            tab.write(&[0x07]);
                        }
                    }
                    AppIntent::Alert(AlertKind::SystemNotice) => {
                        if delayed_note.is_none() {
                            delayed_note = Some("触发器通知".to_owned());
                        }
                    }
                },
                IntentKind::Script(script) => scripts.push((slot, script, ctx)),
                IntentKind::Credential(_) => unreachable!("凭据已在上面转成 write 意图"),
            }
        }
    }
    if note.is_none() {
        *note = delayed_note;
    }

    // 脚本走与运行窗口 / 停靠面板同一个 `ScriptRun` 入口，授权按次决定。
    for (slot, script, ctx) in scripts {
        let Some(tab) = tab_of(&mut win.tabs, &mut win.shell_tabs, slot) else {
            continue;
        };
        let (session, read_only) = (tab.session, tab.local_mode);
        drop(tab);
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

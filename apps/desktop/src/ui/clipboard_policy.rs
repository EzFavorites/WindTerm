//! `OSC 52` 的宿主侧策略：Ask / Allow / Forbid。
//!
//! 剪贴板只归 UI 线程（egui 的 Context），监管者线程只把请求搬过来。
//! 三档都在这里落地：Forbid 直接丢，Allow 立即执行，Ask 弹一次确认框，
//! 用户放行后写剪贴板或把文本回送 VT 编码成应答。

use eframe::egui;
use term_core::session_runtime::{ClipboardRequest, SessionId};

use crate::ui::*;

/// 请求队列与待确认项的类型别名；`ui/mod.rs` 的字段可以同行声明。
pub(in crate::ui) type ClipboardInbox =
    Vec<(term_core::session_runtime::SessionId, ClipboardRequest)>;
pub(in crate::ui) type ClipboardAsk =
    Option<(term_core::session_runtime::SessionId, ClipboardRequest)>;

/// 三档策略。越界值按 Ask 处理——宁可多问一次，也不替用户偷偷放行。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Decision {
    Ask,
    Allow,
    Forbid,
}

fn decision(policy: usize) -> Decision {
    match policy {
        1 => Decision::Allow,
        2 => Decision::Forbid,
        _ => Decision::Ask,
    }
}

fn policy_of(win: &TerminalWindow, id: SessionId, req: &ClipboardRequest) -> usize {
    let Some(tab) = win.tabs.iter().find(|t| t.session == Some(id)) else {
        return 2; // 会话已经没了：当 Forbid 处理，绝不给一个不存在的标签弹窗
    };
    match req {
        ClipboardRequest::Write(_) => tab.config.clipboard_write_policy,
        ClipboardRequest::Read => tab.config.clipboard_read_policy,
    }
}

fn apply(ctx: &egui::Context, win: &TerminalWindow, id: SessionId, req: ClipboardRequest) {
    match req {
        ClipboardRequest::Write(text) => ctx.copy_text(text),
        ClipboardRequest::Read => {
            let text = ctx.output(|o| o.copied_text.clone());
            win.pool.answer_clipboard(id, text);
        }
    }
}

/// 每帧一次：搬请求、放行 Allow、丢掉 Forbid、把 Ask 排到队首等确认。
pub(in crate::ui) fn poll(ctx: &egui::Context, win: &mut TerminalWindow) {
    let mut incoming = Vec::new();
    for tab in &win.tabs {
        if let Some(id) = tab.session {
            incoming.extend(
                win.pool
                    .take_clipboard_requests(id)
                    .into_iter()
                    .map(|req| (id, req)),
            );
        }
    }
    win.clipboard_requests.extend(incoming);

    // 队首是 Ask 时停在这里等用户；后面的请求保持顺序，不插队。
    loop {
        let Some((id, req)) = win.clipboard_requests.first().cloned() else {
            break;
        };
        match decision(policy_of(win, id, &req)) {
            Decision::Forbid => {
                win.clipboard_requests.remove(0);
            }
            Decision::Allow => {
                win.clipboard_requests.remove(0);
                apply(ctx, win, id, req);
            }
            Decision::Ask => break,
        }
    }

    if win.clipboard_ask.is_none() && !win.clipboard_requests.is_empty() {
        win.clipboard_ask = Some(win.clipboard_requests.remove(0));
    }
    if let Some((id, req)) = win.clipboard_ask.clone() {
        let mut allow = false;
        let mut deny = false;
        egui::Window::new("Clipboard access")
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, egui::Vec2::ZERO)
            .show(ctx, |ui| {
                match &req {
                    ClipboardRequest::Write(text) => {
                        ui.label("The remote asked to replace your clipboard with:");
                        ui.label(egui::RichText::new(text.chars().take(200).collect::<String>()).monospace());
                    }
                    ClipboardRequest::Read => {
                        ui.label("The remote asked to read your clipboard.");
                    }
                }
                ui.horizontal(|ui| {
                    allow = ui.button("Allow").clicked();
                    deny = ui.button("Deny").clicked();
                });
            });
        if allow {
            apply(ctx, win, id, req);
        }
        if allow || deny {
            win.clipboard_ask = None;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_out_of_range_policy_asks_instead_of_guessing() {
        assert_eq!(decision(0), Decision::Ask);
        assert_eq!(decision(1), Decision::Allow);
        assert_eq!(decision(2), Decision::Forbid);
        assert_eq!(decision(9), Decision::Ask);
    }
}

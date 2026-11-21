//! 把文件拖到终端上 = 打开 XMODEM 发送弹层并填好路径。
//!
//! 协议取活动会话的 `modem_protocol`（Modem 页那一行），多文件按 ZMODEM
//! 既有语法用 `;` 连接。真正的文件 IO 仍由 `XferHandle::start` 做，这里只
//! 把「拖进来的本地路径」变成弹层里的一段文本。

use eframe::egui;
use term_app::action::{XferCmd, XferProtocol};

use crate::ui::*;

pub(in crate::ui) fn poll(ctx: &egui::Context, win: &mut TerminalWindow) {
    if win.view.lock.locked {
        return;
    }
    let paths: Vec<String> = ctx.input(|i| {
        i.raw.dropped_files
            .iter()
            .filter_map(|f| f.path.clone())
            .map(|p| p.to_string_lossy().into_owned())
            .collect()
    });
    if paths.is_empty() {
        return;
    }
    let protocol = win
        .tabs
        .get(win.active_slot())
        .map(|t| t.config.modem_protocol)
        .unwrap_or(2);
    let protocol = match protocol {
        0 => XferProtocol::Xmodem,
        1 => XferProtocol::Ymodem,
        _ => XferProtocol::Zmodem,
    };
    win.dispatch_action(Action::Xfer(XferCmd::Send(protocol)));
    if let Some(prompt) = win.xfer_prompt.as_mut() {
        prompt.path = paths.join(";");
    }
    ctx.request_repaint();
}

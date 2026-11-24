//! 把文件拖到终端上 = 打开 XMODEM 发送弹层并填好路径。
//!
//! 协议取活动会话的 `modem_protocol`（Modem 页那一行），多文件按 ZMODEM
//! 既有语法用 `;` 连接。真正的文件 IO 仍由 `XferHandle::start` 做，这里只
//! 把「拖进来的本地路径」变成弹层里的一段文本。

use eframe::egui;
use std::path::Path;
use term_app::action::{XferCmd, XferProtocol};

use crate::ui::*;
use crate::ui::panels::filer::xfer::XferPick;

/// 一条本地路径 → 上传意图。目录必须带 `is_dir=true`，否则会走单文件
/// 通道去 `File::open` 一个目录（EISDIR），官网 page/21 的「上传多个文件夹」
/// 就根本不成立。`is_dir` 由调用方按路径判定，这里只做可单测的纯映射。
fn upload_pick_for(local: &str, is_dir: bool) -> Option<XferPick> {
    let name = Path::new(local).file_name()?.to_string_lossy().into_owned();
    Some(XferPick::Upload {
        local: local.to_string(),
        name,
        size: 0,
        is_dir,
    })
}

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
    // 落在文件面板上 = SFTP 上传（取真实本地路径，进传输队列）；
    // 落在终端上才走 Modem 发送弹层。矩形取上一帧，足以判落点。
    let over_filer = ctx.input(|i| {
        i.pointer
            .hover_pos()
            .is_some_and(|p| win.filer_rect.is_some_and(|r| r.contains(p)))
    });
    if over_filer && !win.filer_remote_dir().is_empty() {
        for local in &paths {
            let is_dir = Path::new(local).is_dir();
            if let Some(pick) = upload_pick_for(local, is_dir) {
                win.start_filer_transfer(pick);
            }
        }
        ctx.request_repaint();
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dropped_directories_and_files_get_distinct_upload_intents() {
        let dir = upload_pick_for("/tmp/project", true).expect("目录应有上传意图");
        let file = upload_pick_for("/tmp/notes.txt", false).expect("文件应有上传意图");
        match (dir, file) {
            (XferPick::Upload { local, name, is_dir, .. }, XferPick::Upload { name: file_name, is_dir: file_is_dir, .. }) => {
                assert_eq!(local, "/tmp/project");
                assert_eq!(name, "project");
                assert!(is_dir, "拖入的目录必须走树上传");
                assert_eq!(file_name, "notes.txt");
                assert!(!file_is_dir, "普通文件仍走单文件上传");
            }
            _ => panic!("上传意图类型不对"),
        }
    }

    #[test]
    fn a_path_without_a_file_name_is_skipped() {
        assert!(upload_pick_for("/", false).is_none());
    }
}

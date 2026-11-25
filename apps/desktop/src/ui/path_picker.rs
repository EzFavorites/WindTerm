//! 文件/目录选择入口：一律唤起系统对话框。
//!
//! 过去这里自己用 `term_io::local_dir::list` 画了一个「假文件管理器」——目录树、
//! 选中框、OK/Cancel 一应俱全。它既跟系统对话框长得不像，又多一份路径校验逻辑。
//! 现在 `open_path_picker` 只决定「选文件还是选目录、标题是什么」，`draw` 只负责
//! 取回结果并写回控件；真正的交互交给平台的 NSOpenPanel / 公共对话框 / portal。

use eframe::egui;

use crate::native_picker::{NativePick, NativePicker};
use crate::ui::*;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::ui) enum PathTarget {
    BellSound,
    ModemUploadDir,
    ModemDownloadDir,
    /// 首次启动「Choose Profiles Directory」的指定目录。
    ProfileDir,
    /// Authentication 页的身份文件；`usize` 是平台页签下标，写回对应那一格。
    AuthIdentity(usize),
    /// 脚本注册表里选择脚本文件。
    ScriptPath,
}

pub(in crate::ui) struct PathPicker {
    target: PathTarget,
    native: NativePicker,
}

impl TerminalWindow {
    /// 从控件当前值打开系统对话框：已填的路径直接进入它的父目录。
    pub(in crate::ui) fn open_path_picker(&mut self, target: PathTarget, current: &str) {
        let (kind, title) = match target {
            PathTarget::BellSound => (NativePick::File, "Choose sound file"),
            PathTarget::AuthIdentity(_) => (NativePick::File, "Choose identity file"),
            PathTarget::ScriptPath => (NativePick::File, "Choose script file"),
            PathTarget::ModemUploadDir
            | PathTarget::ModemDownloadDir
            | PathTarget::ProfileDir => (NativePick::Folder, "Choose directory"),
        };
        self.path_picker = Some(PathPicker {
            target,
            native: NativePicker::new(kind, title, current),
        });
    }
}

fn apply(win: &mut TerminalWindow, target: PathTarget, path: String) {
    match target {
        PathTarget::BellSound => win.new_session.bell_sound = path,
        PathTarget::ModemUploadDir => win.new_session.modem_upload_dir = path,
        PathTarget::ModemDownloadDir => win.new_session.modem_download_dir = path,
        PathTarget::ProfileDir => win.profile_choice.custom_dir = path,
        PathTarget::AuthIdentity(tab) => win.new_session.auth_identity[tab] = path,
        PathTarget::ScriptPath => {
            if let Some(edit) = win.script_manager.as_mut() {
                edit.set_path(path);
            }
        }
    }
}

/// 每帧一次：对话框还开着就请求重画；关闭后取回路径写回控件（取消则什么都不做）。
pub(in crate::ui) fn draw(ctx: &egui::Context, win: &mut TerminalWindow) {
    let result = match win.path_picker.as_ref() {
        Some(picker) => picker.native.poll(),
        None => return,
    };
    let Some(result) = result else {
        ctx.request_repaint();
        return;
    };
    // 先把 target 拿出来再放掉借用的 picker，随后才可能借用整个 win 写回。
    let target = match win.path_picker.take() {
        Some(picker) => picker.target,
        None => return,
    };
    if let Some(path) = result {
        apply(win, target, path.to_string_lossy().into_owned());
    }
}

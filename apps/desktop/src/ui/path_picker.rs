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
    /// 脚本面板的自由路径行；`usize` 是**打开对话框那一刻**的页签下标。
    ///
    /// 不能写回 `current_mut()`：系统对话框开着时用户可以切页，那时当前页签
    /// 已经不是当初点 Browse 的那一个，写错页签等于把路径塞进别人的会话。
    /// 兼容终端没有这条自由输入行，这里是我们自己的便利入口。
    ScriptPanelPath(usize),
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
            PathTarget::ScriptPath | PathTarget::ScriptPanelPath(_) => {
                (NativePick::File, "Choose script file")
            }
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
        PathTarget::ScriptPanelPath(i) => {
            // 写**捕获到的那一页**，不是 `current_mut()`：对话框开着时用户可能
            // 已经切了页签。越界时安静丢弃（页签在这期间被关掉了）。
            if let Some(tab) = win.script_panel.tabs.get_mut(i) {
                tab.state.path = path;
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

#[cfg(test)]
mod tests {
    use super::*;

    /// 系统对话框开着时切页，结果仍要写回**当初点 Browse 的那一页**。
    /// 写 `current_mut()` 的实现会把路径塞进第二页，第一页留空——这条直接钉死。
    #[test]
    fn the_panel_picker_writes_only_the_captured_tab() {
        let ctx = egui::Context::default();
        let pool = std::sync::Arc::new(term_session::SessionPool::new());
        let mut win = crate::ui::lifecycle_test_support::window_with(pool, &ctx, Vec::new());
        win.script_panel.add(); // 两页：0 与 1
        win.script_panel.select(1); // 用户在对话框开着时切到了第 1 页

        apply(
            &mut win,
            PathTarget::ScriptPanelPath(0),
            "/tmp/captured.rhai".to_string(),
        );

        assert_eq!(win.script_panel.tabs[0].state.path, "/tmp/captured.rhai");
        assert_eq!(win.script_panel.tabs[1].state.path, "");
        // 越界的捕获下标（期间那一页被关掉了）安静丢弃，不 panic、不误写别的页。
        apply(
            &mut win,
            PathTarget::ScriptPanelPath(9),
            "/tmp/gone.rhai".to_string(),
        );
        assert_eq!(win.script_panel.tabs[1].state.path, "");
    }
}

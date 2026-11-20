//! About 对话框。
//!
//! 内容故意极简：它只回答「这是什么程序、用什么做的」。
//! 版本号不写死成会漂的字符串——`env!("CARGO_PKG_VERSION")` 由 Cargo 注入。
//!
//! 窗口外框走 `harness::StableWindow`：它是第一个「能单独出图」的组件
//! （`--demo comp:dialog_about`），所以也顺便当出图链路的样板——固定矩形、
//! 右上角关闭、内容与外壳分开。

use crate::ui::kit::window::{DialogId, DialogWindow};
use crate::ui::*;

impl TerminalWindow {
    pub(in crate::ui) fn draw_about_dialog(&mut self, ctx: &egui::Context) {
        let title = self.tr("About WindTerm");
        let version = self.tr("WindTerm");
        let line1 = self.tr("A Rust & egui terminal emulator.");
        let line2 = self.tr("Supports Local Shell, SSH, Telnet, TCP and Serial via portable-pty.");
        let mut ok = false;
        let open = DialogWindow::new(DialogId::About, title).show(ctx, |ui| {
            ui.heading(version);
            ui.label(line1);
            ui.label(line2);
            ui.separator();
            if ui.button("OK").clicked() {
                ok = true;
            }
        });
        // 「OK」与右上角 × 走同一条关闭路径。
        //
        // 这里直接写 `view.show_about` 是既有的一处例外（About 没有
        // 自己的 `Action`）。把它改成 `Action::Window(CloseAbout)` 时一并收掉；
        // 现在挪出闭包只是为了和 × 合并成一条路径，不新增例外。
        if ok || !open {
            self.view.show_about = false;
        }
    }
}

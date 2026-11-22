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
use term_app::shell::ShellAction;

impl TerminalWindow {
    pub(in crate::ui) fn draw_about_dialog(&mut self, ctx: &egui::Context) {
        let title = self.tr("About WindTerm");
        let version = self.tr("WindTerm");
        let line1 = self.tr("A Rust & egui terminal emulator.");
        let line2 = self.tr("Supports Local Shell, SSH, Telnet, TCP and Serial via portable-pty.");
        let mut ok = false;
        let mut open_home = false;
        // Esc / Enter 与 OK、右上角 × 同路：About 没有可丢失的草稿。
        let keys = DialogId::About.keys().poll(ctx);
        ok |= keys.cancel || keys.confirm;
        let open = DialogWindow::new(DialogId::About, title).show_with_rect(ctx, |ui, win| {
            ui.heading(version);
            ui.label(line1);
            ui.label(line2);
            ui.separator();
            // 项目主页：与状态栏同一条 Help 动作（kit::effects::HELP_URL）。
            let home = egui::RichText::new("github.com/EzFavorites/WindTerm")
                .color(egui::Color32::from_rgb(0x4A, 0x8F, 0xE8))
                .underline();
            if ui
                .add(egui::Label::new(home).sense(egui::Sense::click()))
                .on_hover_text(self.tr("Open project home"))
                .clicked()
            {
                open_home = true;
            }
            // OK 与 Preferences 的 Close 同规格、同边距：所有弹窗页脚只有一套。
            let spec = crate::ui::kit::spec::FooterButton::DialogOk.spec();
            let footer = egui::Rect::from_min_max(
                egui::pos2(win.right() - 9.0 - spec.size.x, win.bottom() - 9.0 - spec.size.y),
                egui::pos2(win.right() - 9.0, win.bottom() - 9.0),
            );
            ok |= super::dialog_chrome::place(ui, footer, |ui| {
                crate::ui::kit::footer::widget_button(ui, "OK", spec, true)
            })
            .clicked();
        });
        // 「OK」与右上角 × 走同一条关闭路径。
        //
        // 这里直接写 `view.show_about` 是既有的一处例外（About 没有
        // 自己的 `Action`）。把它改成 `Action::Window(CloseAbout)` 时一并收掉；
        // 现在挪出闭包只是为了和 × 合并成一条路径，不新增例外。
        if open_home {
            self.dispatch_action(Action::Shell(ShellAction::Help));
        }
        if ok || !open {
            self.view.show_about = false;
        }
    }
}

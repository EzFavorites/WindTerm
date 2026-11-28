//! 脚本面板的「本次运行授权」弹出面板（由主操作条的 ⚙ 展开）。
//!
//! 只有需要时才画，不常驻占结果区；录制入口不在这里——它属于 `Tool ▸ Script`
//! 菜单，面板只负责「跑哪个脚本 / 跑在哪 / 用哪些能力」。
//!
//! 单独成文件是为了让 `script_panel.rs` 守住 500 代码行门禁。

use crate::ui::TerminalWindow;
use eframe::egui;

/// 画授权面板，返回它占掉的高度。
pub(in crate::ui) fn draw(win: &mut TerminalWindow, ui: &mut egui::Ui, body: egui::Rect) -> f32 {
    let mut c = ui.new_child(egui::UiBuilder::new().max_rect(body));
    let t_permissions = win.tr("Permissions (this run only)");
    let t_programs = win.tr("Run programs");
    let t_clipboard = win.tr("Read clipboard");
    let t_window = win.tr("Control window");
    let t_dir = win.tr("folder for file access");
    let t_writable = win.tr("Writable");
    let t_subdirs = win.tr("Sub-folders are included; `..` is rejected before comparing.");
    c.collapsing(t_permissions, |ui| {
        ui.checkbox(&mut win.run_config.grants.process, t_programs);
        ui.checkbox(&mut win.run_config.grants.clipboard, t_clipboard);
        ui.checkbox(&mut win.run_config.grants.ui, t_window);
        ui.horizontal(|ui| {
            ui.add(
                egui::TextEdit::singleline(&mut win.run_config.grants.dir)
                    .hint_text(t_dir)
                    .desired_width(180.0),
            );
            ui.checkbox(&mut win.run_config.grants.write, t_writable);
        });
        if !win.run_config.grants.dir.trim().is_empty() {
            ui.weak(t_subdirs);
        }
    });
    c.min_rect().height() + 2.0
}

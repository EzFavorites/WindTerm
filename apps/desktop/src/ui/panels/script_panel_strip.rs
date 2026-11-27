//! 脚本面板结果区顶部的「录制 + 本次运行授权」条。
//!
//! 单独成文件是为了让 `script_panel.rs` 守住 500 代码行门禁：这一条带十几行
//! 取词 + 三处勾选，留在主文件里既挤占篇幅，又和结果区的状态排版混在一起。
//! 它只画控件、产出高度，不持有跨帧状态——状态全在 `ScriptPanelState` 上。

use crate::ui::TerminalWindow;
use eframe::egui;

/// 画录制按钮与单次授权折叠面板，返回它占掉的高度。
pub(in crate::ui) fn draw(win: &mut TerminalWindow, ui: &mut egui::Ui, body: egui::Rect) -> f32 {
    let mut c = ui.new_child(egui::UiBuilder::new().max_rect(body));
    c.horizontal_wrapped(|ui| {
        // 文案先取成 `&'static str`：下面对 `grants` 是可变借用，不能在
        // 同一个实参列表里再借 `win` 取词（E0502）。
        let t_nothing = win.tr("Nothing recorded (no keystrokes captured)");
        let t_permissions = win.tr("Permissions (this run only)");
        let t_programs = win.tr("Run programs");
        let t_clipboard = win.tr("Read clipboard");
        let t_window = win.tr("Control window");
        let t_dir = win.tr("folder for file access");
        let t_writable = win.tr("Writable");
        let t_subdirs = win.tr("Sub-folders are included; `..` is rejected before comparing.");
        let recording = win.is_recording();
        let label = if recording {
            win.tr("Stop Recording")
        } else {
            win.tr("Record")
        };
        if ui.button(label).clicked() {
            match win.toggle_recording() {
                Ok(Some(path)) => {
                    win.script_panel.current_mut().path = path.to_string_lossy().into_owned();
                    win.script_panel.current_mut().outcome = None;
                    win.script_panel.current_mut().error = None;
                }
                Ok(None) => {
                    win.script_panel.current_mut().error = if win.is_recording() {
                        None
                    } else {
                        Some(t_nothing.to_string())
                    };
                }
                Err(message) => win.script_panel.current_mut().error = Some(message),
            }
        }
        // 勾选写进窗口上的共享 RunConfig（内存态）：运行对话框 / 编辑器下一次
        // Run 立刻拿到同一组授权。兼容终端没有「按次授权」这个概念，这是本项目
        // 自己加的安全开关；面板只负责画，不再独占一份别人读不到的状态。
        ui.collapsing(t_permissions, |ui| {
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
    });
    c.min_rect().height() + 2.0
}

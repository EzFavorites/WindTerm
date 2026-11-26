//! Shell ▾ Process / Environment 两页的共用画法（金标准 `04-new-session/shell/02..03`）。
//!
//! 两页逐字一致，放在独立文件只是为了保住 `session_pages.rs` 的 500 行上限。

use super::session_form_body::{heading, FORM_W, ROW_H};
use super::dialog_chrome::{framed_button, tiny_button};
use crate::ui::kit::form::{field_box, field_label, spin_field_delta};
use crate::ui::*;

/// 13/14 `Shell ▾ Process / Environment`：金标准 `04-new-session/shell/02..03`。
///
/// 两页共用同一组字段（参考图两张图逐字一致），分别是启动参数 / 工作目录 /
/// 启动后命令与延迟 / locale / 环境变量表。字段都落在 `SessionConfig` 上，
/// 保存后可在会话库里往返。
pub(in crate::ui) fn shell_process_page(win: &mut TerminalWindow, ui: &mut egui::Ui) {
    let ns = &mut win.new_session;
    heading(ui, "Arguments:");
    field_box(ui, |ui| {
        ui.add(
            egui::TextEdit::singleline(&mut ns.shell_arguments)
                .hint_text("-1 -1")
                .desired_width(ui.available_width())
                .frame(false),
        );
    });

    heading(ui, "Working directory:");
    field_box(ui, |ui| {
        ui.add(
            egui::TextEdit::singleline(&mut ns.working_dir)
                .hint_text("${HomeDir}")
                .desired_width(ui.available_width())
                .frame(false),
        );
    });

    heading(ui, "Execute command after start:");
    ui.horizontal(|ui| {
        ui.allocate_ui_with_layout(
            egui::vec2(FORM_W * 0.62, ROW_H),
            egui::Layout::top_down(egui::Align::Min),
            |ui| {
                ui.set_width(FORM_W * 0.62);
                field_box(ui, |ui| {
                    ui.add(
                        egui::TextEdit::singleline(&mut ns.shell_exec_command)
                            .desired_width(ui.available_width())
                            .frame(false),
                    );
                });
            },
        );
        ui.add_space(8.0);
        ui.allocate_ui_with_layout(
            egui::vec2(FORM_W * 0.38 - 8.0, ROW_H),
            egui::Layout::top_down(egui::Align::Min),
            |ui| {
                ui.set_width(FORM_W * 0.38 - 8.0);
                let (_, stepped) = spin_field_delta(ui, |ui| {
                    ui.add(
                        egui::DragValue::new(&mut ns.shell_exec_delay)
                            .range(0..=3600)
                            .custom_formatter(|n, _| format!("Delay: {n}s")),
                    );
                });
                if let Some(d) = stepped {
                    ns.shell_exec_delay =
                        (ns.shell_exec_delay as i64 + d as i64).clamp(0, 3600) as u32;
                }
            },
        );
    });

    heading(ui, "Locale environment variables:");
    egui::ComboBox::from_id_salt("shell_locale")
        .selected_text(ns.shell_locale.clone())
        .width(FORM_W)
        .wrap_mode(egui::TextWrapMode::Truncate)
        .show_ui(ui, |ui| {
            for locale in ["Automatic (zh_CN.UTF-8)", "C", "POSIX", "en_US.UTF-8"] {
                if ui.selectable_label(ns.shell_locale == locale, locale).clicked() {
                    ns.shell_locale = locale.to_string();
                }
            }
        });

    heading(ui, "Environment variables:");
    let row_h = 22.0;
    let col = FORM_W / 2.0;
    egui::Frame::none()
        .stroke(egui::Stroke::new(1.0, crate::theme::chrome(0x4A4A4A)))
        .show(ui, |ui| {
            ui.set_width(FORM_W);
            ui.set_min_height(220.0);
            ui.horizontal(|ui| {
                field_label(ui, "Variable", col);
                field_label(ui, "Value", col);
            });
            ui.separator();
            for (name, value) in ns.shell_env_vars.iter_mut() {
                ui.horizontal(|ui| {
                    ui.add_sized(
                        egui::vec2(col, row_h),
                        egui::TextEdit::singleline(name).frame(false),
                    );
                    ui.add_sized(
                        egui::vec2(col, row_h),
                        egui::TextEdit::singleline(value).frame(false),
                    );
                });
            }
            ui.with_layout(egui::Layout::bottom_up(egui::Align::LEFT), |ui| {
                ui.horizontal(|ui| {
                    if tiny_button(ui, "+").clicked() {
                        ns.shell_env_vars.push((String::new(), String::new()));
                    }
                    if tiny_button(ui, "-").clicked() {
                        ns.shell_env_vars.pop();
                    }
                    ui.add_space((ui.available_width() - 76.0).max(0.0));
                    framed_button(ui, "Modify", false, false);
                });
            });
        });
}


//! 新建方案模态框（原版图 `07-dialogs/onekeys-new-dialog.png`）。
//! 独立成文件只为保住 `settings_live_onekeys.rs` 的代码行上限。

use super::dialog_chrome::{framed_button, place, rgb};
use super::settings_live_onekeys::{OneKeysEdit, DIALOG_H, DIALOG_W};
use crate::ui::*;
use term_core::onekey::OneKey;

/// 新建方案模态框（原版 `07-dialogs/onekeys-new-dialog.png`）：遮罩 + 居中
/// 小窗 + 「Enter a name for the new OneKey.」+ Name 输入框（蓝色聚焦框）+
/// Cancel / Create。几何是原图物理像素 ÷2。
pub(in crate::ui) fn new_scheme_dialog(ui: &mut egui::Ui, origin: egui::Pos2, edit: &mut OneKeysEdit) {
    let dialog = egui::Rect::from_min_size(origin, egui::vec2(DIALOG_W, DIALOG_H));
    ui.painter().rect_filled(
        dialog,
        egui::Rounding::ZERO,
        egui::Color32::from_black_alpha(128),
    );
    ui.allocate_rect(dialog, egui::Sense::click());

    let r = egui::Rect::from_center_size(dialog.center(), egui::vec2(448.0, 98.5));
    let p = ui.painter_at(r);
    p.rect_filled(r, egui::Rounding::same(4.0), rgb(0x1C1C1C));
    p.rect_stroke(
        r,
        egui::Rounding::same(4.0),
        egui::Stroke::new(0.8, rgb(0x333333)),
    );
    let font = egui::TextStyle::Button.resolve(ui.style());
    p.text(
        r.min + egui::vec2(12.0, 10.0),
        egui::Align2::LEFT_TOP,
        "Enter a name for the new OneKey.",
        font.clone(),
        rgb(0xBBBBBB),
    );
    p.text(
        r.min + egui::vec2(12.0, 41.0),
        egui::Align2::LEFT_TOP,
        "Name:",
        font.clone(),
        rgb(0xBBBBBB),
    );
    let fr = egui::Rect::from_min_size(
        r.min + egui::vec2(53.0, 34.0),
        egui::vec2(383.5, 23.5),
    );
    p.rect_filled(fr, egui::Rounding::ZERO, rgb(0x1C1C1C));
    p.rect_stroke(fr, egui::Rounding::ZERO, egui::Stroke::new(1.0, rgb(0x007CD2)));
    let mut create = false;
    let mut cancel = false;
    let mut buf = std::mem::take(&mut edit.new_scheme_buf);
    place(ui, fr.shrink2(egui::vec2(4.0, 2.0)), |ui| {
        ui.add(
            egui::TextEdit::singleline(&mut buf)
                .font(font.clone())
                .frame(false)
                .desired_width(f32::INFINITY),
        );
    });
    place(
        ui,
        egui::Rect::from_min_size(
            r.min + egui::vec2(340.0, 66.0),
            egui::vec2(48.0, 22.0),
        ),
        |ui| {
            if framed_button(ui, "Cancel", true, false).clicked() {
                cancel = true;
            }
        },
    );
    place(
        ui,
        egui::Rect::from_min_size(
            r.min + egui::vec2(392.0, 66.0),
            egui::vec2(44.0, 22.0),
        ),
        |ui| {
            let enabled = !buf.trim().is_empty();
            if framed_button(ui, "Create", enabled, false).clicked() {
                create = true;
            }
        },
    );
    edit.new_scheme_buf = buf;
    if cancel {
        edit.new_scheme_modal = false;
    }
    if create {
        let name = edit.new_scheme_buf.trim().to_string();
        if !name.is_empty() {
            if let Some(draft) = edit.draft.as_mut() {
                if !draft.iter().any(|k| k.name == name) {
                    draft.push(OneKey {
                        name,
                        ..OneKey::default()
                    });
                    edit.selected = Some(draft.len() - 1);
                }
            }
        }
        edit.new_scheme_modal = false;
    }
}

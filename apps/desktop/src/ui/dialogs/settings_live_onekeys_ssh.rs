//! OneKeys 对话框的 `SSH` 页（从 `settings_live_onekeys` 拆出，满足 4j 单文件上限）。
//!
//! 右栏各元素的 y 均为视觉基线（1408×1314 @2×）物理像素 ÷2 的实测值，
//! 相对右栏内容原点；绘制走 `place` 绝对定位，不用 flow。

use super::dialog_chrome::{fill_field, labeled_field, place, rgb, table_frame, tiny_button, toggle, FIELD_HH};
use super::settings_live_onekeys::{OneKeysEdit, FIELD_W};
use crate::ui::*;
use term_core::onekey::{ChallengeResponse, OneKey, SYSTEM_NAMES};

// ── 右栏各元素的相对 y（相对主体顶，实测） ──
const Y_USER_LABEL: f32 = 2.0;
const Y_USER_FIELD: f32 = 21.0;
const Y_PASS_LABEL: f32 = 54.0;
const Y_PASS_FIELD: f32 = 73.0;
const Y_KEY_LABEL: f32 = 105.0;
const Y_KEY_TABLE: f32 = 125.0;
const Y_KEY_TABLE_H: f32 = 194.0;
const Y_KI_LABEL: f32 = 347.0;
const Y_KI_TABLE: f32 = 367.0;
const Y_KI_TABLE_H: f32 = 193.0;

/// 表格右下角那枚 `Modify`：纯文字 + 下划线，点击切到编辑态。
fn modify_link(ui: &mut egui::Ui, rect: egui::Rect, label: &str) -> bool {
    let resp = ui.allocate_rect(rect, egui::Sense::click());
    let c = rgb(0xAAAAAA);
    let mut job = egui::text::LayoutJob::default();
    job.append(
        label,
        0.0,
        egui::TextFormat {
            underline: egui::Stroke::new(1.0, c),
            ..egui::TextFormat::simple(egui::TextStyle::Button.resolve(ui.style()), c)
        },
    );
    let galley = ui.fonts(|f| f.layout_job(job));
    ui.painter().galley(
        egui::pos2(rect.right() - galley.size().x, rect.center().y - galley.size().y / 2.0),
        galley,
        c,
    );
    resp.clicked()
}

/// Password 行输入框右端的眼睛（视觉基线是线描小图标，不用 emoji 字形）。
fn eye_icon(ui: &mut egui::Ui, center: egui::Pos2, visible: bool) -> bool {
    let rect = egui::Rect::from_center_size(center, egui::vec2(16.0, 14.0));
    let resp = ui.allocate_rect(rect, egui::Sense::click());
    let p = ui.painter_at(rect);
    let c = rgb(0xBBBBBB);
    p.circle_stroke(center, 5.0, egui::Stroke::new(1.0, c));
    p.circle_filled(center, if visible { 2.6 } else { 1.8 }, c);
    resp.clicked()
}

/// Public Key 表里的系统图标：几何字形，避免 emoji 彩色渲染。
fn os_glyph(p: &egui::Painter, center: egui::Pos2, idx: usize) {
    let glyph = ["◆", "●", "▦"].get(idx).copied().unwrap_or("◆");
    p.text(
        center,
        egui::Align2::CENTER_CENTER,
        glyph,
        crate::ui::type_scale::micro(),
        rgb(0xBBBBBB),
    );
}

/// 只读格子：视觉基线默认态连底色都不带（`#1C1C1C` 直接透出），只有文字；
/// `#333` 底是 `Modify` 之后才出现的编辑态，别在这里提前铺。
fn readonly_cell(ui: &mut egui::Ui, rect: egui::Rect, text: &str) {
    ui.painter().text(
        rect.min + egui::vec2(3.0, 2.0),
        egui::Align2::LEFT_TOP,
        text,
        egui::TextStyle::Button.resolve(ui.style()),
        rgb(0xC8C8C8),
    );
}

/// 表格单元格里的输入框：`#333` 底 + 内缩 1 点，与 `dark_field` 同观感但更薄。
fn dark_cell(ui: &mut egui::Ui, rect: egui::Rect, value: &mut String) {
    fill_field(ui, rect, 0x333333);
    place(ui, rect.shrink2(egui::vec2(3.0, 1.0)), |ui| {
        ui.add(
            egui::TextEdit::singleline(value)
                .desired_width(f32::INFINITY)
                .frame(false),
        );
    });
}

/// 单行标签（`Public Key:` / `Keyboard Interactive:` 这类）。
fn label_at(ui: &mut egui::Ui, at: egui::Pos2, text: &str) {
    place(ui, egui::Rect::from_min_size(at, egui::vec2(FIELD_W, 12.0)), |ui| {
        ui.label(egui::RichText::new(text).color(rgb(0xC8C8C8)));
    });
}

/// SSH 标签：Username / Password / Public Key 表 / Keyboard Interactive 表。
pub(in crate::ui) fn ssh_editor(
    win: &mut TerminalWindow,
    ui: &mut egui::Ui,
    key: &mut OneKey,
    edit: &mut OneKeysEdit,
) {
    // 必须先取原点：`labeled_field` 内部用 `place`，出来时光标已经推进到
    // 输入框下方。以前在这里重取 `ui.cursor().min`，后面所有表格都下移
    // 45pt（实测视觉基线表格顶 y=318，实现 y=413）。
    let o = ui.cursor().min;
    labeled_field(ui, FIELD_W, "Username:", Y_USER_LABEL, Y_USER_FIELD, &mut key.username);

    label_at(ui, o + egui::vec2(0.0, Y_PASS_LABEL), "Password:");
    place(
        ui,
        egui::Rect::from_min_size(
            o + egui::vec2(FIELD_W - 32.0, Y_PASS_LABEL - 4.0),
            egui::vec2(32.0, 16.0),
        ),
        |ui| {
            // 与 Triggers 的 `Rules:` 开关**同一个控件**（`dialog_chrome::toggle`）：
            // 两页的开关形状/尺寸/开闭色必须一致，不再一个用 checkbox、一个手绘。
            toggle(ui, egui::vec2(32.0, 16.0), &mut edit.show_password, true)
                .on_hover_text("Reveal password");
        },
    );
    let field = egui::Rect::from_min_size(
        o + egui::vec2(0.0, Y_PASS_FIELD),
        egui::vec2(FIELD_W, FIELD_HH),
    );
    let mut password = win.onekey_password(&key.name);
    fill_field(ui, field, 0x333333);
    let changed = place(ui, field.shrink2(egui::vec2(3.0, 2.0)), |ui| {
        ui.add(
            egui::TextEdit::singleline(&mut password)
                .password(!edit.show_password)
                .desired_width(f32::INFINITY)
                .frame(false),
        )
        .changed()
    });
    if changed {
        // 按**方案名**记草稿，不按下标：删/复制一个方案之后下标会整体错位，
        // 之前的 `(usize, String)` 会把密码写到隔壁方案去。
        win.onekeys_password_draft = Some((key.name.clone(), password));
    }
    if eye_icon(ui, o + egui::vec2(FIELD_W - 14.0, Y_PASS_FIELD + FIELD_HH / 2.0), edit.show_password) {
        edit.show_password = !edit.show_password;
    }

    label_at(ui, o + egui::vec2(0.0, Y_KEY_LABEL), "Public Key:");
    let key_tbl = egui::Rect::from_min_size(
        o + egui::vec2(0.0, Y_KEY_TABLE),
        egui::vec2(FIELD_W, Y_KEY_TABLE_H),
    );
    let rows = key.identity_files.len().min(SYSTEM_NAMES.len());
    let key_editing = edit.key_editing;
    table_frame(ui, key_tbl, rows, "System", "Identity File", |ui, row_rect, r| {
        if r >= rows {
            return;
        }
        os_glyph(ui.painter(), row_rect.min + egui::vec2(10.0, row_rect.height() / 2.0), r);
        place(
            ui,
            egui::Rect::from_min_size(row_rect.min + egui::vec2(20.0, 0.0), row_rect.size()),
            |ui| {
                ui.label(egui::RichText::new(SYSTEM_NAMES[r]).color(rgb(0xC8C8C8)));
            },
        );
        let cell = egui::Rect::from_min_size(
            row_rect.min + egui::vec2(FIELD_W / 2.0, 1.0),
            egui::vec2(FIELD_W / 2.0 - 4.0, row_rect.height() - 2.0),
        );
        if key_editing {
            dark_cell(ui, cell, &mut key.identity_files[r]);
        } else {
            readonly_cell(ui, cell, &key.identity_files[r]);
        }
    });
    if modify_link(
        ui,
        egui::Rect::from_min_size(
            o + egui::vec2(FIELD_W - 44.0, Y_KEY_TABLE + Y_KEY_TABLE_H + 3.0),
            egui::vec2(44.0, 14.0),
        ),
        "Modify",
    ) {
        edit.key_editing = !edit.key_editing;
    }

    // `Keyboard Interactive:` 标签 + 行增删。这张表**可编辑**：连接时学到的
    // 挑战/应答会落在这里，也允许手动预置（2026-11-17 之前它永远空着，
    // 且没有任何添加入口——一整块死区）。
    label_at(ui, o + egui::vec2(0.0, Y_KI_LABEL), "Keyboard Interactive:");
    // 视觉基线默认态没有 `+ / -`，只有表下方的 `Modify`；进入编辑态才露出按钮。
    let mut add = false;
    let mut del = false;
    if edit.ki_editing {
        add = place(
            ui,
            egui::Rect::from_min_size(o + egui::vec2(FIELD_W - 45.0, Y_KI_LABEL - 2.0), egui::vec2(19.0, 16.0)),
            |ui| tiny_button(ui, "+").clicked(),
        );
        del = place(
            ui,
            egui::Rect::from_min_size(o + egui::vec2(FIELD_W - 22.0, Y_KI_LABEL - 2.0), egui::vec2(19.0, 16.0)),
            |ui| tiny_button(ui, "-").clicked(),
        );
    }
    if add {
        key.keyboard_interactive.push(ChallengeResponse {
            challenge: String::new(),
            response: String::new(),
        });
    } else if del {
        key.keyboard_interactive.pop();
    }

    let ki_tbl = egui::Rect::from_min_size(
        o + egui::vec2(0.0, Y_KI_TABLE),
        egui::vec2(FIELD_W, Y_KI_TABLE_H),
    );
    let ki_n = key.keyboard_interactive.len();
    let ki_editing = edit.ki_editing;
    table_frame(ui, ki_tbl, ki_n, "Challenge", "Response", |ui, row_rect, r| {
        if r >= ki_n {
            return;
        }
        let half = FIELD_W / 2.0 - 2.0;
        let l = egui::Rect::from_min_size(row_rect.min, egui::vec2(half, row_rect.height()));
        if ki_editing {
            dark_cell(ui, l, &mut key.keyboard_interactive[r].challenge);
        } else {
            readonly_cell(ui, l, &key.keyboard_interactive[r].challenge);
        }
        let rr = egui::Rect::from_min_size(
            row_rect.min + egui::vec2(FIELD_W / 2.0, 0.0),
            egui::vec2(half, row_rect.height()),
        );
        if ki_editing {
            dark_cell(ui, rr, &mut key.keyboard_interactive[r].response);
        } else {
            readonly_cell(ui, rr, &key.keyboard_interactive[r].response);
        }
    });
    if modify_link(
        ui,
        egui::Rect::from_min_size(
            o + egui::vec2(FIELD_W - 44.0, Y_KI_TABLE + Y_KI_TABLE_H + 3.0),
            egui::vec2(44.0, 14.0),
        ),
        "Modify",
    ) {
        edit.ki_editing = !edit.ki_editing;
    }
}

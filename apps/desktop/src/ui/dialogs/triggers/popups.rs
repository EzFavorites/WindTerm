//! 两个模态弹窗：新建触发器（`03` / `09`）与 Highlight 设置（`11`）。
//!
//! 两者都盖一层半透明遮罩，弹窗本体居中。新建弹窗的 `Type` 下拉展开后是
//! 11 项带图标的列表（视觉基线 `08` / `09`）。

use super::fields::{footer_button, mono, place, rgb};
use super::edit::color_label;
use super::list::icon;
use super::{Actions, Open, View};
use crate::ui::view::trigger_spec as spec;
use crate::ui::view::trigger_spec::color as c;
use crate::ui::*;
use term_core::trigger::TriggerKind;

/// 画当前打开的弹窗。
pub(in crate::ui) fn show(
    win: &mut TerminalWindow,
    ui: &mut egui::Ui,
    o: egui::Pos2,
    view: &mut View,
    act: &mut Actions,
) {
    if view.new_dialog {
        new_trigger(ui, o, view, act);
    }
    if view.hl_dialog {
        highlight(win, ui, o, view);
    }
    if view.ps_dialog {
        super::ps_dialog::play_sound(win, ui, o, view);
    }
}


/// 遮罩：把背后的主对话框压暗并吃掉点击。
pub(super) fn scrim(ui: &mut egui::Ui, r: egui::Rect) -> egui::Rect {
    ui.painter()
        .rect_filled(r, egui::Rounding::ZERO, egui::Color32::from_black_alpha(140));
    ui.allocate_rect(r, egui::Sense::click());
    r
}

/// 主对话框矩形（遮罩默认覆盖范围）。
pub(super) fn parent_rect(o: egui::Pos2) -> egui::Rect {
    egui::Rect::from_min_size(o, egui::vec2(spec::DIALOG_W, spec::DIALOG_H))
}

pub(super) fn dialog_rect(o: egui::Pos2, w: f32, h: f32) -> egui::Rect {
    egui::Rect::from_center_size(
        o + egui::vec2(spec::DIALOG_W / 2.0, spec::DIALOG_H / 2.0),
        egui::vec2(w, h),
    )
}

/// 新建触发器弹窗。
fn new_trigger(ui: &mut egui::Ui, o: egui::Pos2, view: &mut View, act: &mut Actions) {
    scrim(ui, parent_rect(o));
    let w = spec::new_trigger::W;
    let h = spec::new_trigger::H;
    let r = dialog_rect(o, w, h);
    let p = ui.painter().clone();
    p.rect_filled(r, egui::Rounding::same(3.0), rgb(c::BG));
    p.rect_stroke(
        r,
        egui::Rounding::same(3.0),
        egui::Stroke::new(0.8, rgb(0x333333)),
    );

    p.text(
        r.min + egui::vec2(12.0, 14.0),
        egui::Align2::LEFT_TOP,
        "Enter a name for the new trigger.",
        mono(crate::ui::type_scale::SIZE_SMALL),
        rgb(c::TEXT_2),
    );

    let fx = spec::new_trigger::FIELD_X;
    let fw = spec::new_trigger::FIELD_W;
    let fh = spec::new_trigger::FIELD_H;
    let rows = spec::new_trigger::ROW_Y;
    for (k, name) in ["Name:", "Description:", "Type:"].iter().enumerate() {
        p.text(
            egui::pos2(r.left() + 12.0, r.top() + rows[k] + (fh - 11.0) / 2.0),
            egui::Align2::LEFT_TOP,
            *name,
            mono(crate::ui::type_scale::SIZE_SMALL),
            rgb(c::TEXT_2),
        );
    }
    let fr = |k: usize| {
        egui::Rect::from_min_size(
            egui::pos2(r.left() + fx, r.top() + rows[k]),
            egui::vec2(fw, fh),
        )
    };
    field_at(ui, fr(0), &mut view.new_name, "");
    field_at(ui, fr(1), &mut view.new_desc, "");
    // Type：显示当前类型名 + 箭头。
    let tr = fr(2);
    p.rect_filled(tr, egui::Rounding::ZERO, rgb(c::BG));
    p.rect_stroke(
        tr,
        egui::Rounding::ZERO,
        egui::Stroke::new(
            1.0,
            rgb(if view.type_menu { c::BLUE_FOCUS } else { c::FIELD_LINE }),
        ),
    );
    let kind = TriggerKind::ALL[view.new_kind.min(10)];
    p.text(
        tr.left_center() + egui::vec2(6.0, 0.0),
        egui::Align2::LEFT_CENTER,
        kind.label(),
        mono(crate::ui::type_scale::SIZE_SMALL),
        rgb(c::TEXT_2),
    );
    p.add(egui::Shape::closed_line(
        vec![
            egui::pos2(tr.right() - 12.0, tr.center().y - 2.0),
            egui::pos2(tr.right() - 4.0, tr.center().y - 2.0),
            egui::pos2(tr.right() - 8.0, tr.center().y + 2.0),
        ],
        egui::Stroke::new(1.0, rgb(c::TEXT_3)),
    ));
    if ui.allocate_rect(tr, egui::Sense::click()).clicked() {
        view.type_menu = !view.type_menu;
    }

    // 页脚按钮。
    let by = r.bottom() - 30.0;
    if footer_button(
        ui,
        egui::Rect::from_min_size(egui::pos2(r.right() - 190.0, by), egui::vec2(80.0, 22.0)),
        "Cancel",
        false,
    ) {
        view.new_dialog = false;
        view.type_menu = false;
        return;
    }
    if footer_button(
        ui,
        egui::Rect::from_min_size(egui::pos2(r.right() - 100.0, by), egui::vec2(80.0, 22.0)),
        "Ok",
        true,
    ) {
        let name = if view.new_name.trim().is_empty() {
            "new trigger".to_owned()
        } else {
            view.new_name.trim().to_owned()
        };
        act.create = Some((name, view.new_desc.trim().to_owned(), view.new_kind.min(10)));
        view.new_dialog = false;
        view.type_menu = false;
        return;
    }

    // Type 展开列表（浮在最上层）。
    if view.type_menu {
        type_menu(ui, tr, view);
    }
}

/// `Type` 展开后的 11 项列表。
///
/// 视觉基线 `03` / `09` 里它**盖住** Type 输入框本身（首行与框同高），而不是落在
/// 框下方——所以 y 取 `anchor.top()`。
fn type_menu(ui: &mut egui::Ui, anchor: egui::Rect, view: &mut View) {
    let ctx = ui.ctx().clone();
    let item_h = spec::new_trigger::ITEM_H;
    let h = TriggerKind::ALL.len() as f32 * item_h + 2.0;
    let screen = ctx.screen_rect();
    let mut y = anchor.top();
    if y + h > screen.bottom() - 6.0 {
        y = (anchor.bottom() - h).max(screen.top() + 6.0);
    }
    let r = egui::Rect::from_min_size(egui::pos2(anchor.left(), y), egui::vec2(anchor.width(), h));
    egui::Area::new(egui::Id::new("trig_type_menu"))
        .order(egui::Order::Foreground)
        .fixed_pos(r.min)
        .fade_in(false)
        .show(&ctx, |ui| {
            ui.set_min_size(r.size());
            let p = ui.painter().clone();
            p.rect_filled(r, egui::Rounding::ZERO, rgb(c::BG));
            p.rect_stroke(r, egui::Rounding::ZERO, egui::Stroke::new(1.0, rgb(c::FIELD_LINE)));
            for (i, kind) in TriggerKind::ALL.iter().enumerate() {
                let row = egui::Rect::from_min_size(
                    r.min + egui::vec2(1.0, 1.0 + i as f32 * item_h),
                    egui::vec2(r.width() - 2.0, item_h),
                );
                let resp = ui.allocate_rect(row, egui::Sense::click());
                if view.new_kind == i || resp.hovered() {
                    p.rect_filled(row, egui::Rounding::ZERO, rgb(c::SEL));
                }
                icon(ui, &p, row, *kind);
                p.text(
                    egui::pos2(row.left() + 26.0, row.center().y),
                    egui::Align2::LEFT_CENTER,
                    kind.label(),
                    mono(crate::ui::type_scale::SIZE_SMALL),
                    rgb(c::TEXT_2),
                );
                if resp.clicked() {
                    view.new_kind = i;
                    view.type_menu = false;
                }
            }
        });
    // 点空白处收起。
    if ctx.input(|i| i.pointer.any_click()) {
        let inside = ctx
            .pointer_interact_pos()
            .is_some_and(|pos| r.contains(pos) || anchor.contains(pos));
        if !inside {
            view.type_menu = false;
        }
    }
}

/// 弹窗内的单行输入框。
fn field_at(ui: &mut egui::Ui, r: egui::Rect, value: &mut String, hint: &str) {
    let p = ui.painter().clone();
    p.rect_filled(r, egui::Rounding::ZERO, rgb(c::BG));
    p.rect_stroke(r, egui::Rounding::ZERO, egui::Stroke::new(1.0, rgb(c::FIELD_LINE)));
    place(ui, r.shrink2(egui::vec2(4.0, 2.0)), |ui| {
        ui.add(
            egui::TextEdit::singleline(value)
                .font(mono(crate::ui::type_scale::SIZE_SMALL))
                .frame(false)
                .hint_text(egui::RichText::new(hint).color(rgb(c::TEXT_4)))
                .desired_width(f32::INFINITY),
        );
    });
}

/// Highlight 设置弹窗（`11-highlight-settings-dialog.png`，2 px/点）。
///
/// 结构：`Match` 组（左标签 Pattern/Description + 右侧输入框）→ `Style` 组
/// （四行左标签 + 右下拉）→ `Preview` → 页脚。`Pattern` 框内的 flag bar 贴在
/// 框下沿内侧。
///
/// 尺寸 397×563 与父窗口 562 只差 1 点，所以仍然居中对齐父对话框
/// （[`dialog_rect`]），不再走屏幕居中的老路。
fn highlight(win: &mut TerminalWindow, ui: &mut egui::Ui, o: egui::Pos2, view: &mut View) {
    use spec::highlight as hl;

    let r = dialog_rect(o, hl::W, hl::H);
    scrim(ui, parent_rect(o));
    let p = ui.painter().clone();
    p.rect_filled(r, egui::Rounding::same(3.0), rgb(c::BG));
    p.rect_stroke(
        r,
        egui::Rounding::same(3.0),
        egui::Stroke::new(1.0, rgb(0x2e2e2e)),
    );

    group_title(&p, r, "Match", hl::MATCH_TITLE);
    group_title(&p, r, "Style", hl::STYLE_TITLE);
    group_box(&p, r, hl::G1);
    group_box(&p, r, hl::G2);

    // ── Match 组：Pattern（含 flag bar）+ Description ──
    let pat = at(r, hl::PATTERN);
    super::fields::label_ul(
        ui,
        r.left() + hl::PATTERN_LABEL.0,
        r.top() + hl::PATTERN_LABEL.1,
        "Pattern:",
        'P',
    );
    p.rect_filled(pat, egui::Rounding::ZERO, rgb(c::BG));
    // 原版 `11` 的 Pattern 框是聚焦蓝边；夹具用环境变量钉住这一态。
    let pat_focus = std::env::var_os("WINDTERM_TRIG_FOCUS_MATCH").is_some();
    p.rect_stroke(
        pat,
        egui::Rounding::ZERO,
        egui::Stroke::new(1.0, rgb(if pat_focus { c::BLUE_TRACK } else { 0x444444 })),
    );

    let flag = egui::Rect::from_min_size(
        egui::pos2(pat.left(), pat.bottom() - hl::FLAG_H),
        egui::vec2(pat.width(), hl::FLAG_H),
    );
    let sel = view.sel.filter(|i| *i < win.triggers.len());
    // `OK` 的可用性：视觉基线 `11` 里未改动时是暗字禁用的，改动后才亮起来。
    // 基线在「进弹窗的第一帧」取，之后每帧比对指纹。
    let dirty = match sel {
        Some(i) => {
            let cur = hl_fingerprint(&win.triggers[i]);
            match view.hl_base {
                None => {
                    view.hl_base = Some(cur);
                    false
                }
                Some(base) => base != cur,
            }
        }
        None => false,
    };
    if let Some(i) = sel {
        // 编辑区只到 flag bar 上沿：否则换行文字会压到 `.* Aa W` 上。
        let area = egui::Rect::from_min_max(
            pat.min + egui::vec2(4.0, 3.0),
            egui::pos2(pat.right() - 4.0, flag.top() - 2.0),
        );
        let rule = &mut win.triggers[i];
        super::fields::place(ui, area, |ui| {
            ui.add(
                egui::TextEdit::multiline(&mut rule.pattern)
                    .font(super::fields::mono(super::fields::FS))
                    .text_color(rgb(c::TEXT_2))
                    .frame(false)
                    .desired_width(f32::INFINITY),
            );
        });
    }
    if let Some(i) = sel {
        let rule = &mut win.triggers[i];
        super::fields::flag_bar(
            ui,
            flag,
            &mut rule.config.flags,
            &mut rule.literal,
            &mut rule.case_sensitive,
        );
    }

    let desc = at(r, hl::DESC);
    super::fields::label_ul(
        ui,
        r.left() + hl::DESC_LABEL.0,
        r.top() + hl::DESC_LABEL.1,
        "Description:",
        'D',
    );
    p.rect_filled(desc, egui::Rounding::ZERO, rgb(c::BG));
    p.rect_stroke(desc, egui::Rounding::ZERO, egui::Stroke::new(1.0, rgb(0x444444)));
    if let Some(i) = sel {
        super::fields::place(ui, desc.shrink2(egui::vec2(5.0, 3.0)), |ui| {
            ui.add(
                egui::TextEdit::singleline(&mut win.triggers[i].description)
                    .font(super::fields::mono(super::fields::FS))
                    .text_color(rgb(c::TEXT_2))
                    .frame(false)
                    .desired_width(f32::INFINITY),
            );
        });
    }

    // ── Style 组：四行 ──
    let opens = [Open::TextColor, Open::BgColor, Open::LineStyle, Open::LineColor];
    let line_none = sel
        .map(|i| win.triggers[i].config.mark.line_style == term_core::trigger::LineStyle::None)
        .unwrap_or(true);
    for (k, top) in hl::ROWS.iter().enumerate() {
        let fr = egui::Rect::from_min_size(
            r.min + egui::vec2(hl::ROW_FIELD_X, *top),
            egui::vec2(hl::ROW_FIELD_W, hl::ROW_H),
        );
        super::fields::label_ul(
            ui,
            r.left() + hl::LABEL_X,
            fr.center().y,
            hl::LABELS[k],
            hl::LABELS[k].chars().next().unwrap_or(' '),
        );
        let (text, swatch) = match sel {
            Some(i) => {
                let m = &win.triggers[i].config.mark;
                match k {
                    0 => (color_label(&m.text_color), Some(color_of(&m.text_color))),
                    1 => (color_label(&m.background_color), Some(color_of(&m.background_color))),
                    2 => (m.line_style.label().to_owned(), None),
                    _ => (color_label(&m.line_color), Some(color_of(&m.line_color))),
                }
            }
            None => ("Empty".to_owned(), Some(None)),
        };
        // 视觉基线：`Line Style = None` 时 `Line Color` 整行压暗。
        let enabled = k != 3 || !line_none;
        if super::fields::color_combo(
            ui,
            fr,
            &text,
            swatch,
            view.open == Some(opens[k]),
            enabled,
        )
        .clicked()
        {
            view.toggle_at(opens[k], fr);
        }
    }

    // Preview 块。
    let pv = at(r, hl::PREVIEW);
    p.rect_stroke(pv, egui::Rounding::ZERO, egui::Stroke::new(1.0, rgb(0x444444)));
    p.text(
        pv.center(),
        egui::Align2::CENTER_CENTER,
        "Preview",
        super::fields::mono(crate::ui::type_scale::SIZE_EMPHASIS),
        rgb(c::TEXT_2),
    );

    // 页脚按钮。视觉基线里 OK 在「未改动」时是灰面暗字；改动后转常态。
    let by = r.min.y + hl::BTN_Y;
    let size = egui::vec2(hl::BTN_W, hl::BTN_H);
    if super::fields::footer_button_as(
        ui,
        egui::Rect::from_min_size(egui::pos2(r.min.x + hl::BTN_CANCEL_X, by), size),
        "Cancel",
        super::fields::Btn::Secondary,
    ) {
        view.hl_dialog = false;
        view.hl_base = None;
        view.dismiss();
    }
    let ok_kind = if dirty {
        super::fields::Btn::Secondary
    } else {
        super::fields::Btn::Muted
    };
    if super::fields::footer_button_as(
        ui,
        egui::Rect::from_min_size(egui::pos2(r.min.x + hl::BTN_OK_X, by), size),
        "OK",
        ok_kind,
    ) && dirty
    {
        view.hl_dialog = false;
        view.hl_base = None;
        view.dismiss();
    }
}

/// 弹窗里可改字段的指纹（`OK` 可用性的判据）。
///
/// 覆盖 `Pattern` / `Description` / flag bar 三个开关 / 四行 Style 的颜色与线型。
fn hl_fingerprint(r: &term_app::trigger::Rule) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    r.pattern.hash(&mut h);
    r.description.hash(&mut h);
    r.literal.hash(&mut h);
    r.case_sensitive.hash(&mut h);
    r.config.flags.whole_word.hash(&mut h);
    format!("{:?}", r.config.mark).hash(&mut h);
    h.finish()
}

/// 弹窗坐标（`r.min` 为原点）取矩形。
pub(super) fn at(r: egui::Rect, q: (f32, f32, f32, f32)) -> egui::Rect {
    egui::Rect::from_min_size(r.min + egui::vec2(q.0, q.1), egui::vec2(q.2, q.3))
}

/// 分组标题（`Match` / `Style`）：视觉基线里**不带**下划线，只是普通灰字。
pub(super) fn group_title(p: &egui::Painter, r: egui::Rect, text: &str, pos: (f32, f32)) {
    p.text(
        r.min + egui::vec2(pos.0, pos.1),
        egui::Align2::LEFT_TOP,
        text,
        super::fields::mono(super::fields::FS),
        rgb(c::TEXT_2),
    );
}

/// 分组框：`#2e2e2e` 描边 + 3 点圆角（视觉基线角是圆的，不是直角）。
fn group_box(p: &egui::Painter, r: egui::Rect, g: (f32, f32)) {
    let gr = egui::Rect::from_min_size(
        r.min + egui::vec2(spec::highlight::GROUP_X, g.0),
        egui::vec2(spec::highlight::GROUP_W, g.1),
    );
    p.rect_stroke(
        gr,
        egui::Rounding::same(3.0),
        egui::Stroke::new(1.0, rgb(0x2e2e2e)),
    );
}

/// 颜色选择对应的实际色值（`Empty` 无块）。
use super::style::color_of;


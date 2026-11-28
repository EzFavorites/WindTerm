//! `Show tooltip` 页（视觉基线 `30` / `31`）：Match + `Tooltip:` 框 + Mark 两行。
//!
//! 与 `Send text` 页的区别：中间那块不再是「发送内容 + 发送条件 + 延迟」，而是
//! 一段**提示内容**（可含 `${match}` 与有限 HTML），下面仍保留 `Text`/`Script`
//! 与 `Captures`，因为提示文本也支持脚本求值。页脚主按钮是 `Next`。

use super::fields::{label_ul, mono, multiline, rgb, text_w};
use super::View;
use crate::ui::view::trigger_pages_spec as ps;
use crate::ui::view::trigger_spec as spec;
use crate::ui::view::trigger_spec::color as c;
use crate::ui::*;
use crate::ui::dialogs::triggers::model::SendMode;

fn origin(o: egui::Pos2) -> egui::Pos2 {
    o + egui::vec2(spec::BODY.0, spec::BODY.1)
}

/// 画整页。
pub(in crate::ui) fn page(
    ui: &mut egui::Ui,
    o: egui::Pos2,
    view: &mut View,
    rule: &mut crate::ui::dialogs::triggers::model::Rule,
) {
    use ps::tooltip as tt;
    let base = origin(o);
    super::edit::match_box(ui, base, rule, tt::MATCH_H, true);

    label_ul(ui, base.x, base.y + tt::LABEL_Y + super::fields::FS / 2.0, "Tooltip:", 'T');

    let box_r = egui::Rect::from_min_size(
        base + egui::vec2(tt::BOX.0, tt::BOX.1),
        egui::vec2(tt::BOX.2, tt::BOX.3),
    );
    let hint = if rule.config.tooltip.mode == SendMode::Script {
        tt::SCRIPT_HINT.to_owned()
    } else {
        format!("{}\n{}", tt::HINT[0], tt::HINT[1])
    };
    multiline(ui, box_r, &mut rule.config.tooltip.text, &hint, false);

    let p = ui.painter().clone();
    let tabs = egui::Rect::from_min_size(
        base + egui::vec2(0.0, tt::MODE_TABS_TOP),
        egui::vec2(spec::BODY.2, tt::MODE_TABS_H),
    );
    p.line_segment(
        [tabs.left_bottom(), tabs.right_bottom()],
        egui::Stroke::new(0.75, rgb(c::FIELD_LINE)),
    );
    let mut x = tabs.left();
    for (i, name) in tt::MODES.iter().enumerate() {
        let w = text_w(name) + 16.0;
        let r = egui::Rect::from_min_size(egui::pos2(x, tabs.top()), egui::vec2(w, tabs.height()));
        let active = (rule.config.tooltip.mode == SendMode::Script) == (i == 1);
        p.rect_filled(r, egui::Rounding::ZERO, rgb(if active { c::TAB } else { c::BG }));
        p.rect_stroke(r, egui::Rounding::ZERO, egui::Stroke::new(0.75, rgb(c::LINE)));
        super::fields::mnemonic_at(ui, &p, r, name, name.chars().next().unwrap_or(' '));
        if ui.allocate_rect(r, egui::Sense::click()).clicked() {
            rule.config.tooltip.mode = if i == 1 { SendMode::Script } else { SendMode::Text };
        }
        x += w;
    }

    // 右端 `Captures`：Text 模式是纯标签，Script 模式是蓝底取值（视觉基线 `30`/`31`）。
    let cap = egui::Rect::from_min_size(
        egui::pos2(tabs.right() - tt::CAPTURES_W, tabs.top()),
        egui::vec2(tt::CAPTURES_W, tabs.height()),
    );
    if rule.config.tooltip.mode == SendMode::Script {
        p.rect_filled(cap, egui::Rounding::ZERO, rgb(c::BLUE_TRACK));
        p.text(
            cap.left_center() + egui::vec2(6.0, 0.0),
            egui::Align2::LEFT_CENTER,
            "captures.match",
            mono(super::fields::FS),
            rgb(c::TEXT),
        );
    } else {
        p.text(
            cap.right_center(),
            egui::Align2::RIGHT_CENTER,
            "Captures",
            mono(super::fields::FS),
            rgb(c::TEXT_2),
        );
    }
    if ui.allocate_rect(cap, egui::Sense::click()).clicked() {
        view.toggle_at(super::Open::Captures, cap);
    }

    let style = rule.config.mark.mark_style.label().to_owned();
    super::edit::mark_row(
        ui,
        base,
        super::edit::MarkRow {
            top: tt::MARK_STYLE_TOP,
            name: "Mark Style:",
            value: &style,
            swatch: None,
            open: super::Open::LineStyle,
        },
        view,
    );
    let color = super::edit::color_label(&rule.config.mark.mark_color);
    let swatch = super::style::color_of(&rule.config.mark.mark_color);
    super::edit::mark_row(
        ui,
        base,
        super::edit::MarkRow {
            top: tt::MARK_COLOR_TOP,
            name: "Mark Color:",
            value: &color,
            swatch: Some(swatch),
            open: super::Open::MarkColor,
        },
        view,
    );
    super::sync_actions(rule);
}

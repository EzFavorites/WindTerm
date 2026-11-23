//! 底部两条工具条与页脚 `Cancel` / `Ok`，以及 `Rename` 内联输入行。
//!
//! 从 `mod.rs` 拆出（单文件 500 行上限）。

use super::fields::{self, mono, place, rgb, text_w};
use super::list::display_name;
use super::{Actions, View};
use crate::ui::view::trigger_spec as spec;
use crate::ui::view::trigger_spec::color as c;
use crate::ui::*;
use term_core::trigger::TriggerKind;

/// 两条底部工具条。
pub(super) fn strips(
    win: &TerminalWindow,
    ui: &mut egui::Ui,
    o: egui::Pos2,
    view: &mut View,
    act: &mut Actions,
) {
    let p = ui.painter().clone();
    let (lx, _, lw, _) = spec::LEFT;
    let (bx, _, bw, _) = spec::BODY;
    let y = spec::STRIP_Y;
    let h = spec::STRIP_H;
    // 右栏工具条只属于两张「规则表」页（视觉基线 `01` / `28`）：Highlight Text 与
    // Play sound；`10` / `15` / `23` / `26` / `30` 那些页面里同一带被 Mark 行或
    // 多行框占着，多画一条会压字。
    let rules_page = view.tab == 0
        && matches!(view.kind, TriggerKind::HighlightText | TriggerKind::PlaySound);
    let bars: &[(f32, f32)] = if rules_page {
        &[(lx, lw), (bx, bw)]
    } else {
        &[(lx, lw)]
    };
    for (x, w) in bars {
        let r = egui::Rect::from_min_size(o + egui::vec2(*x, y), egui::vec2(*w, h));
        p.line_segment(
            [r.left_top(), r.right_top()],
            egui::Stroke::new(0.6, rgb(c::LINE)),
        );
    }
    // 左条：`+ -` 在左，`Duplicate` `Rename` 在右。
    let left = egui::Rect::from_min_size(o + egui::vec2(lx, y), egui::vec2(lw, h));
    let mut x = left.left() + 6.0;
    for (label, hit) in [("+", 0u8), ("-", 1)] {
        let r = egui::Rect::from_min_size(egui::pos2(x, left.top()), egui::vec2(16.0, h));
        if ui.allocate_rect(r, egui::Sense::click()).clicked() {
            match hit {
                0 => {
                    view.new_dialog = true;
                    view.new_name.clear();
                    view.new_desc.clear();
                    view.new_kind = TriggerKind::ALL
                        .iter()
                        .position(|k| *k == TriggerKind::HighlightText)
                        .unwrap_or(0);
                }
                _ => act.del = true,
            }
        }
        p.text(
            egui::pos2(r.left() + 5.0, r.center().y),
            egui::Align2::LEFT_CENTER,
            label,
            mono(crate::ui::type_scale::SIZE_BODY),
            rgb(c::TEXT_2),
        );
        x += 22.0;
    }
    {
        let enabled = view.sel.is_some();
        let mut rx = left.right() - 6.0;
        for (label, act_kind) in [("Rename", 1u8), ("Duplicate", 0)] {
            let wpx = text_w(label);
            rx -= wpx;
            let r = egui::Rect::from_min_size(egui::pos2(rx, left.top()), egui::vec2(wpx, h));
            let resp = ui.allocate_rect(r, egui::Sense::click());
            p.text(
                egui::pos2(r.left(), r.center().y),
                egui::Align2::LEFT_CENTER,
                label,
                mono(crate::ui::type_scale::SIZE_SMALL),
                rgb(if enabled { c::TEXT_2 } else { c::TEXT_4 }),
            );
            if resp.clicked() && enabled {
                if act_kind == 1 {
                    view.renaming = true;
                    view.rename_buf = view
                        .sel
                        .and_then(|i| win.triggers.get(i))
                        .map(|r| r.name.clone())
                        .unwrap_or_default();
                } else {
                    act.duplicate = true;
                }
            }
            rx -= 14.0;
        }
    }
    if !rules_page {
        return;
    }
    // 右条：`+ - ▲ ▼` 在左，`Modify` `Remove All` 在右。
    let right = egui::Rect::from_min_size(o + egui::vec2(bx, y), egui::vec2(bw, h));
    for (i, label) in ["+", "-", "\u{25b2}", "\u{25bc}"].iter().enumerate() {
        let r = egui::Rect::from_min_size(
            egui::pos2(right.left() + 6.0 + i as f32 * 22.0, right.top()),
            egui::vec2(16.0, h),
        );
        let resp = ui.allocate_rect(r, egui::Sense::click());
        let dim = i >= 2;
        p.text(
            egui::pos2(r.left() + 4.0, r.center().y),
            egui::Align2::LEFT_CENTER,
            *label,
            mono(crate::ui::type_scale::SIZE_SMALL),
            rgb(if dim { c::TEXT_4 } else { c::TEXT_2 }),
        );
        if resp.clicked() && !dim {
            if i == 0 {
                act.add = true;
            } else {
                act.del = true;
            }
        }
    }
    {
        let mut rx = right.right() - 6.0;
        for (label, kind) in [("Remove All", 1u8), ("Modify", 0)] {
            let wpx = text_w(label);
            rx -= wpx;
            let r = egui::Rect::from_min_size(egui::pos2(rx, right.top()), egui::vec2(wpx, h));
            let resp = ui.allocate_rect(r, egui::Sense::click());
            p.text(
                egui::pos2(r.left(), r.center().y),
                egui::Align2::LEFT_CENTER,
                label,
                mono(crate::ui::type_scale::SIZE_SMALL),
                rgb(c::TEXT_2),
            );
            if resp.clicked() {
                if kind == 1 {
                    act.remove_all = true;
                } else {
                    act.modify = true;
                }
            }
            rx -= 14.0;
        }
    }
}

/// 页脚 `Cancel` / 主按钮。
///
/// 主按钮的文案随类型变：`Custom menu` / `Run command` / `Play sound` /
/// `Show tooltip` 四页在视觉基线里都是 `Next`（`23` / `26` / `28` / `30`），
/// 其余仍是 `Ok`。
pub(super) fn footer(ui: &mut egui::Ui, o: egui::Pos2, view: &View, act: &mut Actions) {
    let primary = match view.kind {
        TriggerKind::CustomMenu
        | TriggerKind::RunCommand
        | TriggerKind::PlaySound
        | TriggerKind::ShowTooltip => "Next",
        _ => "Ok",
    };
    // 2026-11-23 实机补截的 04/10/13/15/16-page 是「向导」上下文（模态框 +
    // 主按钮 Next），与 Preferences 独立页（Ok）是同一界面的两种宿主。
    // 夹具用 WINDTERM_TRIG_NEXT=1 复现向导态，不改生产路径、不改冻结基线。
    let primary = if cfg!(debug_assertions)
        && std::env::var("WINDTERM_TRIG_NEXT").as_deref() == Ok("1")
    {
        "Next"
    } else {
        primary
    };
    for (label, (x, w), is_primary) in [
        ("Cancel", spec::BTN_CANCEL, false),
        (primary, spec::BTN_OK, true),
    ] {
        let r = egui::Rect::from_min_size(
            o + egui::vec2(x, spec::BTN_Y),
            egui::vec2(w, spec::BTN_H),
        );
        if fields::footer_button(ui, r, label, is_primary) {
            if is_primary {
                act.save = true;
            }
            act.close = true;
        }
    }
}

/// `Rename` 的内联输入行。
pub(super) fn rename_row(win: &mut TerminalWindow, ui: &mut egui::Ui, o: egui::Pos2, view: &mut View) {
    let Some(i) = view.sel.filter(|i| *i < win.triggers.len()) else {
        view.renaming = false;
        return;
    };
    let r = egui::Rect::from_min_size(
        o + egui::vec2(spec::BODY.0 + 100.0, spec::LABEL_Y - 2.0),
        egui::vec2(280.0, 18.0),
    );
    if view.rename_buf.is_empty() {
        view.rename_buf = display_name(&win.triggers[i], i);
    }
    let mut done = false;
    place(ui, r, |ui| {
        ui.horizontal(|ui| {
            ui.label(egui::RichText::new("Name:").size(crate::ui::type_scale::SIZE_SMALL).color(rgb(c::TEXT_2)));
            ui.add(
                egui::TextEdit::singleline(&mut view.rename_buf)
                    .desired_width(180.0)
                    .font(mono(crate::ui::type_scale::SIZE_SMALL)),
            );
            if ui.small_button("OK").clicked() {
                done = true;
            }
        });
    });
    if done {
        if !view.rename_buf.trim().is_empty() {
            win.triggers[i].name = view.rename_buf.trim().to_owned();
        }
        view.renaming = false;
    }
}

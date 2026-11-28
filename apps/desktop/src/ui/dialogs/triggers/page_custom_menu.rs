//! `Custom menu` 页（视觉基线 `23` / `25`）：Match + `Actions` 表 + 两行 Mark。
//!
//! # 与 Custom link 的区别
//!
//! Custom link 的下半页是「Action 下拉 + 分 OS 三字段」；Custom menu 换成一张
//! **两列表**（`Label` | `Action`），表下一条工具条负责增删与排序，表外仍然是
//! `Mark Style` / `Mark Color`。页脚主按钮是 `Next` 而不是 `Ok`。
//!
//! # 表内编辑的边界
//!
//! 视觉基线只给了**空表**（`No action available`），没有给「一行数据长什么样」。
//! 所以本实现严格只做图里有的东西：`+` 弹出 `Add Action` / `Add Separator`
//! （图 `25`），行的选中/删除/上下移动按工具条按钮的语义接上；`Action` 列的
//! 取值切换暂用「点击循环四项」，因为兼容终端的下拉形态没有视觉基线——补图后应换成
//! 下拉，`README` 的待办里记了这一条。

use super::fields::{mono, rgb, text_w};
use super::{Open, View};
use crate::ui::view::trigger_pages_spec as ps;
use crate::ui::view::trigger_spec as spec;
use crate::ui::view::trigger_spec::color as c;
use crate::ui::*;
use crate::ui::dialogs::triggers::model::{ActionKind, MenuItem};

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
    let base = origin(o);
    super::edit::match_box(ui, base, rule, ps::custom_menu::MATCH_H, true);

    let p = ui.painter().clone();
    p.text(
        base + egui::vec2(0.0, ps::custom_menu::ACTIONS_LABEL_Y),
        egui::Align2::LEFT_TOP,
        "Actions:",
        mono(super::fields::FS),
        rgb(c::TEXT_2),
    );

    table(ui, base, view, rule);
    tools(ui, base, view, rule);
    mark_rows(ui, base, view, rule);
    super::sync_actions(rule);
}

/// `Label` / `Action` 两列表（含表头与空态）。
fn table(
    ui: &mut egui::Ui,
    base: egui::Pos2,
    view: &mut View,
    rule: &mut crate::ui::dialogs::triggers::model::Rule,
) {
    use ps::custom_menu as cm;
    let p = ui.painter().clone();
    let r = egui::Rect::from_min_size(
        base + egui::vec2(0.0, cm::TABLE_TOP),
        egui::vec2(spec::BODY.2, cm::TABLE_BOTTOM - cm::TABLE_TOP),
    );
    p.rect_stroke(r, egui::Rounding::ZERO, egui::Stroke::new(0.6, rgb(c::FIELD_LINE)));

    let head = egui::Rect::from_min_size(r.min, egui::vec2(r.width(), cm::HEAD_H));
    p.rect_filled(head, egui::Rounding::ZERO, rgb(0x222222));
    p.line_segment(
        [head.left_bottom(), head.right_bottom()],
        egui::Stroke::new(0.6, rgb(0x3a3a3a)),
    );
    // 两列分界（表头内一条竖线，表体不画——视觉基线 `23` 表体是空的）。
    p.line_segment(
        [
            egui::pos2(head.left() + cm::COL_SPLIT, head.top()),
            egui::pos2(head.left() + cm::COL_SPLIT, head.bottom()),
        ],
        egui::Stroke::new(0.6, rgb(0x3a3a3a)),
    );
    for (i, name) in cm::HEADERS.iter().enumerate() {
        let x = if i == 0 { 4.0 } else { cm::COL_SPLIT + 4.0 };
        p.text(
            head.min + egui::vec2(x, 4.0),
            egui::Align2::LEFT_TOP,
            *name,
            mono(super::fields::FS),
            rgb(c::TEXT_2),
        );
    }

    let body = egui::Rect::from_min_max(
        egui::pos2(r.left() + 1.0, head.bottom() + 1.0),
        egui::pos2(r.right() - 1.0, r.bottom() - 1.0),
    );
    if rule.config.menu.items.is_empty() {
        p.text(
            body.center(),
            egui::Align2::CENTER_CENTER,
            cm::EMPTY,
            mono(super::fields::FS),
            rgb(0x555555),
        );
        return;
    }

    let row_h = 20.0;
    let mut top = body.top();
    let count = rule.config.menu.items.len();
    let mut clicked_row = None;
    let mut double_row = None;
    for i in 0..count {
        if top + row_h > body.bottom() {
            break;
        }
        let row = egui::Rect::from_min_size(egui::pos2(body.left(), top), egui::vec2(body.width(), row_h));
        let resp = ui.allocate_rect(row, egui::Sense::click());
        if view.menu_sel == Some(i) {
            p.rect_filled(row, egui::Rounding::ZERO, rgb(c::TAB));
        }
        let item = &rule.config.menu.items[i];
        if item.separator {
            // `Add Separator` 建出来的行：只在 `Label` 列画一条横线。
            p.line_segment(
                [
                    egui::pos2(row.left() + 8.0, row.center().y),
                    egui::pos2(row.left() + cm::COL_SPLIT - 8.0, row.center().y),
                ],
                egui::Stroke::new(1.0, rgb(0x4a4a4a)),
            );
        } else {
            p.text(
                egui::pos2(row.left() + 4.0, row.center().y),
                egui::Align2::LEFT_CENTER,
                &item.label,
                mono(super::fields::FS),
                rgb(c::TEXT_2),
            );
            p.text(
                egui::pos2(row.left() + cm::COL_SPLIT + 4.0, row.center().y),
                egui::Align2::LEFT_CENTER,
                item.action.label(),
                mono(super::fields::FS),
                rgb(c::TEXT_2),
            );
        }
        if resp.clicked() {
            clicked_row = Some(i);
        }
        if resp.double_clicked() {
            double_row = Some(i);
        }
        top += row_h;
    }
    if let Some(i) = clicked_row {
        view.menu_sel = Some(i);
    }
    // `Action` 列的取值切换：兼容终端是下拉，视觉基线里没有它的展开态，先按双击循环。
    if let Some(i) = double_row.filter(|i| *i < count) {
        let item = &mut rule.config.menu.items[i];
        if !item.separator {
            let at = ActionKind::ALL.iter().position(|k| *k == item.action).unwrap_or(0);
            item.action = ActionKind::ALL[(at + 1) % ActionKind::ALL.len()];
        }
    }
}

/// 表下工具条：`+ - ▲ ▼` 在左，`Modify` / `Remove All` 在右。
fn tools(
    ui: &mut egui::Ui,
    base: egui::Pos2,
    view: &mut View,
    rule: &mut crate::ui::dialogs::triggers::model::Rule,
) {
    use ps::custom_menu as cm;
    let p = ui.painter().clone();
    let y = ps::custom_menu::TABLE_BOTTOM;
    let left = egui::Rect::from_min_size(
        base + egui::vec2(0.0, y),
        egui::vec2(spec::BODY.2, cm::TOOLS_H),
    );
    let mut x = left.left() + 5.0;
    for (i, label) in cm::TOOLS_LEFT.iter().enumerate() {
        let r = egui::Rect::from_min_size(egui::pos2(x, left.top()), egui::vec2(16.0, cm::TOOLS_H));
        let resp = ui.allocate_rect(r, egui::Sense::click());
        let dim = i >= 2 && view.menu_sel.is_none();
        p.text(
            egui::pos2(r.left() + 4.0, r.center().y),
            egui::Align2::LEFT_CENTER,
            *label,
            mono(super::fields::FS),
            rgb(if dim { c::TEXT_4 } else { c::TEXT_2 }),
        );
        if !resp.clicked() {
            x += 22.0;
            continue;
        }
        match i {
            0 => {
                view.toggle_at(Open::AddMenu, r);
            }
            1 => {
                if let Some(k) = view.menu_sel.filter(|k| *k < rule.config.menu.items.len()) {
                    rule.config.menu.items.remove(k);
                    view.menu_sel = None;
                }
            }
            2 => move_row(rule, view, -1),
            _ => move_row(rule, view, 1),
        }
        x += 22.0;
    }

    let mut rx = left.right() - 6.0;
    // 从右往左摆，所以**倒序**迭代：视觉基线 `23` 里最右是 `Remove All`，
    // 它左边才是 `Modify`。正序会画成 `Remove All Modify`（与图相反）。
    for name in cm::TOOLS_RIGHT.iter().rev() {
        let w = text_w(name);
        rx -= w;
        let r = egui::Rect::from_min_size(egui::pos2(rx, left.top()), egui::vec2(w, cm::TOOLS_H));
        let resp = ui.allocate_rect(r, egui::Sense::click());
        p.text(
            egui::pos2(r.left(), r.center().y),
            egui::Align2::LEFT_CENTER,
            name,
            mono(super::fields::FS),
            rgb(c::TEXT_2),
        );
        if resp.clicked() {
            if *name == "Remove All" {
                rule.config.menu.items.clear();
                view.menu_sel = None;
            } else if let Some(k) = view.menu_sel {
                // `Modify` 在视觉基线里只对「有选中行」有意义；本页没有独立弹窗，
                // 选中即已在表内可改（见模块头），这里只确保选中态生效。
                view.menu_sel = Some(k.min(rule.config.menu.items.len().saturating_sub(1)));
            }
        }
        rx -= 14.0;
    }
}

fn move_row(rule: &mut crate::ui::dialogs::triggers::model::Rule, view: &mut View, delta: isize) {
    let Some(i) = view.menu_sel else { return };
    let n = rule.config.menu.items.len();
    let j = i as isize + delta;
    if i >= n || j < 0 || j >= n as isize {
        return;
    }
    rule.config.menu.items.swap(i, j as usize);
    view.menu_sel = Some(j as usize);
}

/// `Mark Style` / `Mark Color` 两行。
fn mark_rows(
    ui: &mut egui::Ui,
    base: egui::Pos2,
    view: &mut View,
    rule: &mut crate::ui::dialogs::triggers::model::Rule,
) {
    let style = rule.config.mark.mark_style.label().to_owned();
    super::edit::mark_row(
        ui,
        base,
        super::edit::MarkRow {
            top: ps::custom_menu::MARK_STYLE_TOP,
            name: "Mark Style:",
            value: &style,
            swatch: None,
            open: Open::LineStyle,
        },
        view,
    );
    let color = super::edit::color_label(&rule.config.mark.mark_color);
    let swatch = super::style::color_of(&rule.config.mark.mark_color);
    super::edit::mark_row(
        ui,
        base,
        super::edit::MarkRow {
            top: ps::custom_menu::MARK_COLOR_TOP,
            name: "Mark Color:",
            value: &color,
            swatch: Some(swatch),
            open: Open::MarkColor,
        },
        view,
    );
}

/// `Add Action` / `Add Separator` 的落地（由 `dropdowns::apply` 调用）。
pub(in crate::ui) fn add_item(rule: &mut crate::ui::dialogs::triggers::model::Rule, separator: bool) {
    rule.config.menu.items.push(if separator {
        MenuItem { separator: true, ..MenuItem::default() }
    } else {
        MenuItem {
            label: format!("Action {}", rule.config.menu.items.len() + 1),
            action: ActionKind::RunCommand,
            separator: false,
        }
    });
}

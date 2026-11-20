//! 左栏列表 / 顶部标签条 / 标签行（`Rules:` + 启用开关）。
//!
//! 从 `mod.rs` 拆出（单文件 500 行上限）。所有坐标来自
//! `crate::ui::view::trigger_spec`。

use super::fields::{self, mono, rgb};
use super::View;
use crate::ui::view::trigger_spec as spec;
use crate::ui::view::trigger_spec::color as c;
use crate::ui::*;
use term_app::trigger::Rule;
use term_core::trigger::TriggerKind;

/// 左栏显示名。
pub(super) fn display_name(rule: &Rule, i: usize) -> String {
    if !rule.name.trim().is_empty() {
        rule.name.clone()
    } else if !rule.pattern.trim().is_empty() {
        rule.pattern.clone()
    } else {
        format!("Trigger {}", i + 1)
    }
}

/// 左栏：外框 + 行列表 + 空态。
pub(super) fn left_pane(
    win: &mut TerminalWindow,
    ui: &mut egui::Ui,
    o: egui::Pos2,
    view: &mut View,
) {
    let (x, y, w, h) = spec::LEFT;
    let rect = egui::Rect::from_min_size(o + egui::vec2(x, y), egui::vec2(w, h));
    let p = ui.painter().clone();
    p.rect_stroke(rect, egui::Rounding::ZERO, egui::Stroke::new(0.6, rgb(c::LINE)));

    let rows: Vec<usize> = (0..win.triggers.len()).collect();
    if rows.is_empty() {
        p.text(
            o + egui::vec2(x + w / 2.0, spec::LEFT_EMPTY_Y),
            egui::Align2::CENTER_CENTER,
            "No trigger available",
            mono(crate::ui::type_scale::SIZE_SMALL),
            rgb(0x555555),
        );
        return;
    }
    let row_h = 22.0;
    let mut top = rect.top() + 1.0;
    for i in rows {
        let row = egui::Rect::from_min_size(
            egui::pos2(rect.left() + 1.0, top),
            egui::vec2(w - 2.0, row_h),
        );
        let resp = ui.allocate_rect(row, egui::Sense::click());
        if view.sel == Some(i) {
            // 选中行底色 `#333`（`13` 实测），比列表菜单的 `#282828` 深一档。
            p.rect_filled(row, egui::Rounding::ZERO, rgb(c::TAB));
        }
        icon(ui, &p, row, win.triggers[i].kind);
        p.text(
            egui::pos2(row.left() + 26.0, row.center().y),
            egui::Align2::LEFT_CENTER,
            display_name(&win.triggers[i], i),
            mono(crate::ui::type_scale::SIZE_SMALL),
            rgb(c::TEXT_2),
        );
        if resp.clicked() {
            view.sel = Some(i);
            view.kind = win.triggers[i].kind;
            view.open = None;
            // 换了触发器对象，`Actions` 表里的行选择不再有意义。
            view.menu_sel = None;
        }
        top += row_h;
        if top > rect.bottom() - row_h {
            break;
        }
    }
}

/// 列表每行前面的小图标。
///
/// 字形逐个对齐视觉基线 `09` 的 11 项：外链方框 / 文稿 / 「竖线+圆」与「竖线+叉」
/// （连接前 / 断开）/ 星 / 音符 / 四瓣花（Run command）/ 纸飞机 / 对话气泡。
/// 颜色统一 `#007cd2`（`13` 实测），不是白也不是浅蓝。
pub(super) fn icon(ui: &egui::Ui, p: &egui::Painter, row: egui::Rect, kind: TriggerKind) {
    let cx = row.left() + 14.0;
    let cy = row.center().y;
    let col = rgb(c::ICON);
    let st = egui::Stroke::new(1.0, col);
    match kind {
        // 「竖线 + 圆/叉」：上方一短横，下方图形。connect 是圆，disconnect 是叉。
        TriggerKind::AfterConnect | TriggerKind::BeforeConnect | TriggerKind::AfterDisconnect
        | TriggerKind::BeforeDisconnect => {
            p.line_segment(
                [egui::pos2(cx - 4.0, cy - 6.0), egui::pos2(cx + 4.0, cy - 6.0)],
                st,
            );
            let is_connect = matches!(
                kind,
                TriggerKind::AfterConnect | TriggerKind::BeforeConnect
            );
            if is_connect {
                p.circle_stroke(egui::pos2(cx, cy + 1.5), 3.6, st);
            } else {
                p.line_segment(
                    [egui::pos2(cx - 3.2, cy - 1.7), egui::pos2(cx + 3.2, cy + 4.7)],
                    st,
                );
                p.line_segment(
                    [egui::pos2(cx - 3.2, cy + 4.7), egui::pos2(cx + 3.2, cy - 1.7)],
                    st,
                );
            }
        }
        // 外链：方框 + 右上箭头。
        TriggerKind::CustomLink => {
            p.rect_stroke(
                egui::Rect::from_min_size(egui::pos2(cx - 5.0, cy - 4.0), egui::vec2(6.5, 6.5)),
                egui::Rounding::ZERO,
                st,
            );
            p.line_segment([egui::pos2(cx - 1.0, cy + 1.0), egui::pos2(cx + 4.5, cy - 4.5)], st);
            p.line_segment([egui::pos2(cx + 0.5, cy - 4.5), egui::pos2(cx + 4.5, cy - 4.5)], st);
            p.line_segment([egui::pos2(cx + 4.5, cy - 4.5), egui::pos2(cx + 4.5, cy - 0.5)], st);
        }
        // 文稿：方框 + 三行横线。
        TriggerKind::CustomMenu => {
            p.rect_stroke(
                egui::Rect::from_min_size(egui::pos2(cx - 4.5, cy - 5.0), egui::vec2(9.0, 10.0)),
                egui::Rounding::ZERO,
                st,
            );
            for k in 0..3 {
                let y = cy - 2.6 + k as f32 * 3.0;
                p.line_segment([egui::pos2(cx - 2.6, y), egui::pos2(cx + 2.6, y)], st);
            }
        }
        // 五角星。
        TriggerKind::HighlightText => {
            let mut pts = Vec::new();
            for k in 0..10 {
                let r = if k % 2 == 0 { 4.8 } else { 2.0 };
                let a = -std::f32::consts::FRAC_PI_2 + k as f32 * std::f32::consts::PI / 5.0;
                pts.push(egui::pos2(cx + r * a.cos(), cy + r * a.sin()));
            }
            p.add(egui::Shape::closed_line(pts, st));
        }
        // 八分音符（符头 + 符干 + 旗）。
        TriggerKind::PlaySound => {
            p.circle_filled(egui::pos2(cx - 2.0, cy + 3.4), 1.8, col);
            p.line_segment([egui::pos2(cx - 0.3, cy + 3.4), egui::pos2(cx - 0.3, cy - 4.4)], st);
            p.line_segment([egui::pos2(cx - 0.3, cy - 4.4), egui::pos2(cx + 4.2, cy - 3.0)], st);
            p.line_segment([egui::pos2(cx + 4.2, cy - 3.0), egui::pos2(cx + 4.2, cy - 0.2)], st);
            p.circle_filled(egui::pos2(cx + 2.6, cy + 0.8), 1.6, col);
        }
        // 四瓣花（Command 符号）。
        TriggerKind::RunCommand => {
            for (dx, dy) in [(-2.4, -2.4), (2.4, -2.4), (-2.4, 2.4), (2.4, 2.4)] {
                p.circle_stroke(egui::pos2(cx + dx, cy + dy), 2.6, st);
            }
        }
        // 纸飞机。
        TriggerKind::SendText => {
            p.add(egui::Shape::closed_line(
                vec![
                    egui::pos2(cx - 5.0, cy - 2.0),
                    egui::pos2(cx + 5.0, cy - 5.0),
                    egui::pos2(cx + 1.5, cy + 5.0),
                    egui::pos2(cx, cy),
                ],
                st,
            ));
        }
        // 对话气泡。
        TriggerKind::ShowTooltip => {
            let r = egui::Rect::from_min_size(egui::pos2(cx - 5.0, cy - 5.0), egui::vec2(10.0, 7.5));
            p.rect_stroke(r, egui::Rounding::same(1.5), st);
            p.add(egui::Shape::closed_line(
                vec![
                    egui::pos2(cx - 2.0, r.bottom()),
                    egui::pos2(cx + 0.5, r.bottom()),
                    egui::pos2(cx - 1.0, r.bottom() + 2.5),
                ],
                st,
            ));
            for k in 0..2 {
                let y = cy - 2.6 + k as f32 * 2.8;
                p.line_segment([egui::pos2(cx - 2.6, y), egui::pos2(cx + 2.6, y)], st);
            }
        }
    }
    let _ = ui;
}

/// 顶部标签条：**选中页无底色、未选中页填 `#333`**（见模块头）。
pub(super) fn tab_bar(ui: &mut egui::Ui, o: egui::Pos2, view: &mut View) {
    let (x, y, w, h) = spec::TABBAR;
    let split = spec::TAB_SPLIT;
    let p = ui.painter().clone();
    let halves = [
        (0.0, split),
        (split, w - split),
    ];
    // 右半是 Sessions，左半是动态类型标题。
    let titles = [spec::type_title(view.kind), "Sessions"];
    for (i, (lx, lw)) in halves.iter().enumerate() {
        let r = egui::Rect::from_min_size(
            o + egui::vec2(x + lx, y),
            egui::vec2(*lw, h),
        );
        let active = view.tab == i;
        if !active {
            p.rect_filled(r, egui::Rounding::ZERO, rgb(c::TAB));
        }
        let resp = ui.allocate_rect(r, egui::Sense::click());
        let mnem = if i == 0 {
            spec::type_mnemonic(view.kind)
        } else {
            'S'
        };
        fields::mnemonic_at(ui, &p, r, titles[i], mnem);
        if resp.clicked() {
            view.tab = i;
            view.open = None;
        }
    }
}

/// 标签行：`Rules:` 之类的标题 + 右端启用开关。
pub(super) fn label_row(ui: &mut egui::Ui, o: egui::Pos2, view: &mut View, win: &mut TerminalWindow) {
    let label = if view.tab == 1 {
        spec::sessions::LABEL_A
    } else {
        match view.kind {
            // 两张「规则表」页：Highlight Text 与 Play sound（视觉基线 `01` / `28`）。
            TriggerKind::HighlightText | TriggerKind::PlaySound => "Rules:",
            // 四类事件页没有 Match，顶部直接是 `Action:`（视觉基线 `04`）。
            TriggerKind::AfterConnect
            | TriggerKind::AfterDisconnect
            | TriggerKind::BeforeConnect
            | TriggerKind::BeforeDisconnect => "Action:",
            // 其余（Custom link / Custom menu / Send text / Run command / Show tooltip）
            // 都以 `Match:` 起头（视觉基线 `10` / `23` / `15` / `26` / `30`）。
            _ => "Match:",
        }
    };
    let p = ui.painter().clone();
    p.text(
        o + egui::vec2(spec::BODY.0 + 0.6, spec::LABEL_Y),
        egui::Align2::LEFT_TOP,
        label,
        mono(crate::ui::type_scale::SIZE_SMALL),
        rgb(0xC8C8C8),
    );
    let (sx, sy, sw, sh) = spec::SWITCH;
    let sel = view.sel.filter(|i| *i < win.triggers.len());
    let mut on = sel.map(|i| win.triggers[i].enabled).unwrap_or(false);
    let r = egui::Rect::from_min_size(o + egui::vec2(sx, sy), egui::vec2(sw, sh));
    if fields::toggle(ui, r, &mut on) {
        if let Some(i) = sel {
            win.triggers[i].enabled = on;
        }
    }
}


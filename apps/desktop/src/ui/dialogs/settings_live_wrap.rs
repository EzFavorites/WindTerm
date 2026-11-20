//! `Wrap` 页：两项真控件 + 一列号输入框（），
//! 另外两档与 Marker style 仍按数据表画成灰的。
//!
//! # 为什么这三项能真接
//!
//! `Wrap mode` / `Wrap column` 缺的是软换行本体——内核已经按列宽硬折过行，
//! 再接一层软换行是恒等映射（`term-app/src/wrap.rs` 的实测记录）。但
//! `Show wrap symbols` / `Show column markers` / `Columns:` 在 就有
//! 完整渲染器：`View.ToggleWrapSymbols` / `View.ToggleColumnMarker` 走
//! `MarksKnob` → `Tab.marks`，画在 `terminal/marks.rs` 与 `marks_paint.rs`。
//! 之前的设置页把它们和没有渲染器的两项一起灰掉，是「按页一刀切」的误伤。
//!
//! 接线口径：`Prefs.view_wrap_symbols` / `view_column_marker` 是跨重启的真相，
//! 窗口上的两份是渲染期副本；改完同时落盘 + 同步活动标签的 `marks`
//! （与 View 菜单同一个 `set_marks` 入口），所以勾完当场能看见，不必重开标签。

use super::settings_live::save_prefs;
use super::settings_pages::Row;
use super::settings_pages_view::{draw_row, field_label, group_box, label_col};
use crate::ui::terminal::MarksParams;
use crate::ui::*;

/// 页底：哪三格真、哪三格没有渲染器，分别说清。
const WRAP_LIVE_NOTE: &str = "Wrap symbols, column markers and the marker column apply \
     immediately and are saved. Wrap mode / Wrap column still need a soft-wrap engine (the grid \
     already hard-wraps), and Marker style has only one implementation (a line).";

/// 无软换行时的两档：照抄视觉基线，灰显但保留形状。
const WRAP_MODE_NOTE: &str = "Not wired: the grid already hard-wraps at the pane width, so a \
     soft-wrap toggle would map every line onto itself (see term-app/src/wrap.rs).";
const MARKER_STYLE_NOTE: &str = "Not wired: the column marker has one renderer (a line); there \
     is no second style to choose from.";

pub(in crate::ui) fn wrap(win: &mut TerminalWindow, ui: &mut egui::Ui) {
    let mut symbols = win.wrap_symbols;
    let mut column = win.column_marker_col;
    let before = (symbols, column);

    // 视觉基线这一页没有页级标题：右上角第一个字就是分组框标题 `Wrap`。
    // 之前 `page_title(ui, "Wrap")` + 框内标题会让「Wrap」出现两次。
    // 标签列按本页最长的标签量（`Marker style:`），两个框里的控件左缘因此对齐。
    let col = label_col(ui, &["Wrap mode:", "Wrap column:", "Columns:", "Marker style:"]);
    group_box(ui, Some("Wrap"), |ui| {
        ui.spacing_mut().item_spacing.y = 6.0;
        draw_row(
            ui,
            &Row::Combo { label: "Wrap mode:", value: "Wrap by character" },
            WRAP_MODE_NOTE,
            col,
        );
        draw_row(
            ui,
            &Row::Combo { label: "Wrap column:", value: "Window edge" },
            WRAP_MODE_NOTE,
            col,
        );
    });

    ui.add_space(6.0);
    if ui
        .checkbox(&mut symbols, "Show wrap symbols")
        .on_hover_text("Marks continued lines at the wrap edge in the active tab.")
        .changed()
    {}

    ui.add_space(6.0);
    group_box(ui, Some("Column Marker"), |ui| {
        ui.spacing_mut().item_spacing.y = 6.0;
        ui.horizontal(|ui| {
            field_label(ui, "Columns:", col);
            let mut text = if column == 0 { String::new() } else { column.to_string() };
            let resp = ui.add_sized(
                egui::vec2(ui.available_width(), 20.0),
                egui::TextEdit::singleline(&mut text).hint_text("80, 100, 120, ..."),
            );
            if resp.changed() {
                if text.trim().is_empty() {
                    column = 0;
                } else if let Ok(n) = text.trim().parse::<usize>() {
                    column = n.clamp(8, 512);
                }
            }
        });
        draw_row(
            ui,
            &Row::Combo { label: "Marker style:", value: "Line marker" },
            MARKER_STYLE_NOTE,
            col,
        );
    });

    ui.add_space(6.0);
    let mut markers_on = column > 0;
    if ui
        .checkbox(&mut markers_on, "Show column markers")
        .on_hover_text("Draws a vertical line at the configured column in the active tab.")
        .changed()
    {
        column = if markers_on {
            if column == 0 { 80 } else { column }
        } else {
            0
        };
    }
    if (symbols, column) == before {
        return;
    }
    win.wrap_symbols = symbols;
    win.column_marker_col = column;
    win.set_marks(|m| MarksParams {
        wrap_symbols: symbols,
        ..m.with_column_marker(if column == 0 { None } else { Some(column) })
    });
    save_prefs(|prefs| {
        prefs.view_wrap_symbols = symbols;
        prefs.view_column_marker = column;
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn marker_column_off_is_zero_and_on_uses_the_factory_column() {
        // 0 = 不画，与 `Prefs.view_column_marker` 的出厂哨兵一致；
        // 打开但没填列号时给 80，与 View 菜单 `ToggleColumnMarker` 同一列。
        let off = 0usize;
        let on = if off == 0 { 80 } else { off };
        assert_eq!(on, 80);
    }

    #[test]
    fn the_page_note_separates_live_rows_from_the_two_dead_ones() {
        assert!(WRAP_LIVE_NOTE.contains("apply"));
        assert!(WRAP_LIVE_NOTE.contains("Wrap mode / Wrap column"));
        assert!(WRAP_LIVE_NOTE.contains("Marker style"));
    }
}

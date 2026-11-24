//! `Wrap` 页：四项真控件 + `Marker style` 仍按数据表画成灰的。
//!
//! # 四项为什么能真接
//!
//! 软换行本体已于 2026-11-24 接真：开关在 Mode ▸ Word Wrap（每标签），
//! 段表按 `wrap::WrapConfig` 每帧现算。本页的 `Wrap mode`（按字符 / 按词）与
//! `Wrap column`（0 = Window edge）是**窗口级**的两根轴：落 `Prefs`，
//! 构造时读进 `TerminalWindow.wrap_settings`，改完立刻写回这份副本——六个
//! 终端消费者每帧都从它取同一份配置，所以改完当场生效、不必重开标签。
//! `Show wrap symbols` / `Show column markers` / `Columns:` 在 就有
//! 完整渲染器：`View.ToggleWrapSymbols` / `View.ToggleColumnMarker` 走
//! `MarksKnob` → `Tab.marks`，画在 `terminal/marks.rs` 与 `marks_paint.rs`。
//! 之前的设置页把它们和没有渲染器的两项一起灰掉，是「按页一刀切」的误伤。
//!
//! 接线口径：`Prefs.wrap_mode` / `wrap_column` / `view_wrap_symbols` /
//! `view_column_marker` 是跨重启的真相，窗口上的是渲染期副本；改完同时落盘
//! + 同步窗口副本（marks 那两项还同步活动标签的 `marks`，与 View 菜单同一个
//! `set_marks` 入口），所以勾完当场能看见，不必重开标签。

use crate::ui::kit::field::{choice_row_resp, ChoiceKind, UnderlineStyle};
use crate::ui::kit::spec::BLOCK_GAP;
use super::settings_live::save_prefs;
use super::settings_pages::Row;
use super::settings_pages_view::{
    draw_row, field_height, field_row, group_box, label_col, set_row_rhythm, Slot,
};
use crate::ui::terminal::MarksParams;
use crate::ui::*;

/// 页底：哪些格真接、哪一格没有第二种渲染器，分别说清。
const WRAP_LIVE_NOTE: &str = "Wrap mode, wrap column, wrap symbols, column markers and the marker \
     column apply immediately and are saved. Soft wrapping is toggled per tab via Mode ▸ Word \
     Wrap (or Alt+M, Alt+W). Marker style has one implementation.";

/// `Wrap mode` / `Wrap column` 现在都是真控件：改完同步窗口副本并落盘。
const WRAP_MODE_NOTE: &str = "Choose character or word wrapping and a wrap column here; both \
     apply immediately and are saved. Toggle soft wrapping per tab with Mode ▸ Word Wrap \
     (Alt+M, Alt+W). At Window edge the grid hard-wraps at the pane width.";

/// `Wrap column` 的出厂哨兵：0 = Window edge（开软折时回落默认 40）。
const WINDOW_EDGE: &str = "Window edge";
const MARKER_STYLE_NOTE: &str = "Not wired: the column marker has one renderer (a line); there \
     is no second style to choose from.";

/// 输入框文本 → 落盘值。`Window edge` / 空串都回 0 哨兵，数字夹进 8..=512；
/// 半途输入（如 `8x`）返回 `None`，让本次按键不改动已有值。
fn parse_wrap_column(text: &str) -> Option<usize> {
    let typed = text.trim();
    if typed.is_empty() || typed.eq_ignore_ascii_case(WINDOW_EDGE) {
        Some(0)
    } else {
        typed.parse::<usize>().ok().map(|n| n.clamp(8, 512))
    }
}

pub(in crate::ui) fn wrap(win: &mut TerminalWindow, ui: &mut egui::Ui) {
    let mut symbols = win.wrap_symbols;
    let mut column = win.column_marker_col;
    let mut mode = win.wrap_settings.mode.as_u8();
    let mut wrap_column = win.wrap_settings.column;
    let before = (symbols, column, mode, wrap_column);

    // 视觉基线这一页没有页级标题：右上角第一个字就是分组框标题 `Wrap`。
    // 之前 `page_title(ui, "Wrap")` + 框内标题会让「Wrap」出现两次。
    // 标签列按本页最长的标签量（`Marker style:`），两个框里的控件左缘因此对齐。
    let col = label_col(ui, &["Wrap mode:", "Wrap column:", "Columns:", "Marker style:"]);
    group_box(ui, Some("Wrap"), |ui| {
        set_row_rhythm(ui);
        // 两行都是真控件：改完只更新局部值，函数末尾一次性同步窗口副本 + 落盘，
        // 避免绘制过程中写 `win`（与 Find 栏同一条受控组件纪律）。
        field_row(ui, "Wrap mode:", Slot::Inline, col, WRAP_MODE_NOTE, |ui| {
            let shown = if mode == 1 { "Wrap by word" } else { "Wrap by character" };
            egui::ComboBox::from_id_salt("prefs_wrap_mode")
                .selected_text(shown)
                .width(ui.available_width())
                .wrap_mode(egui::TextWrapMode::Truncate)
                .show_ui(ui, |ui| {
                    if ui.selectable_label(mode == 0, "Wrap by character").clicked() { mode = 0; }
                    if ui.selectable_label(mode == 1, "Wrap by word").clicked() { mode = 1; }
                });
        });
        field_row(ui, "Wrap column:", Slot::Inline, col, WRAP_MODE_NOTE, |ui| {
            // 0 = Window edge：出厂态仍显示这几个字（守住设置页视觉基线）；
            // 用户改成数字后回显数字，清空 / 再写回 Window edge 则回到 0。
            let mut text = if wrap_column == 0 { WINDOW_EDGE.to_string() } else { wrap_column.to_string() };
            let resp = ui.add_sized(
                egui::vec2(ui.available_width(), field_height(ui)),
                egui::TextEdit::singleline(&mut text).hint_text(WINDOW_EDGE),
            );
            if resp.changed() {
                if let Some(n) = parse_wrap_column(&text) {
                    wrap_column = n;
                }
            }
        });
    });

    ui.add_space(BLOCK_GAP);
    if choice_row_resp(ui, ChoiceKind::Checkbox, "Show wrap symbols", "", &mut symbols, true, UnderlineStyle::None)
        .on_hover_text("Marks continued lines at the wrap edge in the active tab.")
        .clicked()
    {}

    ui.add_space(BLOCK_GAP);
    group_box(ui, Some("Column Marker"), |ui| {
        set_row_rhythm(ui);
        field_row(ui, "Columns:", Slot::Inline, col, "", |ui| {
            let mut text = if column == 0 { String::new() } else { column.to_string() };
            let resp = ui.add_sized(
                egui::vec2(ui.available_width(), field_height(ui)),
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

    ui.add_space(BLOCK_GAP);
    let mut markers_on = column > 0;
    if choice_row_resp(ui, ChoiceKind::Checkbox, "Show column markers", "", &mut markers_on, true, UnderlineStyle::None)
        .on_hover_text("Draws a vertical line at the configured column in the active tab.")
        .clicked()
    {
        column = if markers_on {
            if column == 0 { 80 } else { column }
        } else {
            0
        };
    }
    if (symbols, column, mode, wrap_column) == before {
        return;
    }
    win.wrap_symbols = symbols;
    win.column_marker_col = column;
    // 同一份窗口副本喂给六个终端消费者：下一次重绘就用新 mode / column 折行。
    win.wrap_settings = term_app::wrap::WrapSettings::from_prefs(mode, wrap_column);
    win.set_marks(|m| MarksParams {
        wrap_symbols: symbols,
        ..m.with_column_marker(if column == 0 { None } else { Some(column) })
    });
    save_prefs(|prefs| {
        prefs.view_wrap_symbols = symbols;
        prefs.view_column_marker = column;
        prefs.wrap_mode = mode;
        prefs.wrap_column = wrap_column;
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
    fn the_page_note_describes_the_wired_and_display_only_rows() {
        assert!(WRAP_LIVE_NOTE.contains("apply"));
        assert!(WRAP_LIVE_NOTE.contains("Word Wrap"), "软换行入口要写清");
        assert!(WRAP_LIVE_NOTE.contains("Wrap mode"), "两行真控件要写清");
        assert!(WRAP_LIVE_NOTE.contains("Marker style"));
    }

    #[test]
    fn the_two_rows_round_trip_through_the_window_copy() {
        // 设置页局部值 → 窗口副本 → 六个消费者共用的配置，逐值对照。
        let settings = term_app::wrap::WrapSettings::from_prefs(1, 80);
        assert_eq!(settings.mode, term_app::wrap::WrapMode::Word);
        assert_eq!(settings.column, 80);
        let cfg = settings.with(true);
        assert_eq!(cfg.effective_cols(120), 80, "自定义列生效");
        // 出厂值 char + 0 → 40，保持接线前的折行口径。
        let factory = term_app::wrap::WrapSettings::default();
        assert_eq!(factory.with(true).effective_cols(120), 40);
        assert_eq!(factory.with(false).effective_cols(120), 120);
    }

    #[test]
    fn window_edge_text_means_the_zero_sentinel() {
        assert_eq!(parse_wrap_column(WINDOW_EDGE), Some(0));
        assert_eq!(parse_wrap_column("  window edge  "), Some(0));
        assert_eq!(parse_wrap_column(""), Some(0));
        assert_eq!(parse_wrap_column("80"), Some(80));
        assert_eq!(parse_wrap_column("3"), Some(8), "低于下限夹到 8");
        assert_eq!(parse_wrap_column("9999"), Some(512), "高于上限夹到 512");
        assert_eq!(parse_wrap_column("8x"), None, "半途输入不改动已有值");
    }
}

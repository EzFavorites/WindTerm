//! `Tabs` 页：八行全是**真控件**。
//!
//! # 哪三行是真的
//!
//! - `Activate the tab after closing:` → `Prefs.tab_activate_after_close`，
//!   读者是 `shell::tab_strip::close_tab`（标签 ×、菜单 Close、确认弹窗 Yes
//!   三条路径都汇到那一个方法）。
//! - `Double click:` → `Prefs.tab_double_click`，读者是 `shell::tab_bar`
//!   的双击判定。**只有两档**：参考页的 `Rename tab` 在本仓库没有入口
//!   （改名在 `Properties` 表单里，改的是会话名字，不是标签），点下去没有反应
//!   的档位不如不放。
//! - `Tab text elide mode:` → `Prefs.tab_elide`，读者是 `shell::tab_bar` 画
//!   标题那一处（按字符截断，不按字节）。
//!
//! 缩略图用事实窗口的 cell 画小色块图（`ui/tab_thumbnail.rs`），
//! 占标签宽度的百分比可调，0 = 关闭。
//!
//! 2026-11-21 起又接真五行：悬停激活、编号、缩略图、中键、右键。
//! - `Activate the tab after hovering:` → `Prefs.tab_hover_activate` / 延时毫秒，
//!   判据在 `ui/tab_hover.rs`（停够时间才切，点击 / 拖动不抢）。
//! - `Show the tab number on tabs:` → `Prefs.tab_numbers`（不显示 / 前九个 /
//!   全部）。编号拼进标题再走同一条 `layout_no_wrap`，宽度自然变宽，
//!   读者是 `shell::tab_bar::tab_number_prefix`（全窗与分屏两条标签条共用）。
//! - `Middle click:` → `Prefs.tab_middle_click`（没有动作 / 关闭标签）。
//! - `Right click:` → `Prefs.tab_right_click`（标签菜单 / 关闭标签 / 没有动作）。
//!   右键菜单现在也是一档可选项，而不是无条件弹出。
//!
//! # 铁律
//!
//! 灰行永远是空框 / 空下拉（`settings_pages_view::draw_row`）：勾与选中态传达
//! 「这项已生效」，而它们零读者。

use super::settings_live::save_prefs;
use super::settings_pages_view::{field_row, group_box, label_col, set_row_rhythm, Slot};
use crate::ui::shell::ELIDE_MODES;
use crate::ui::*;
use term_runtime::prefs::{
    TAB_ACTIVATE_MODES, TAB_DOUBLE_CLICK_ACTIONS, TAB_MIDDLE_CLICK_ACTIONS,
    TAB_NUMBER_MODES, TAB_RIGHT_CLICK_ACTIONS,
};

/// 「关闭后激活」三档的文案，下标与 `Prefs.tab_activate_after_close` 一一对应。
///
/// 措辞对齐参考页（`Activate the previously selected tab` 是它的原文），
/// 0 档说的是本实现接线之前的行为：关掉当前标签后原地留下，也就是「下一个」。
const ACTIVATE_MODES: [&str; TAB_ACTIVATE_MODES] = [
    "Activate the next tab",
    "Activate the previous tab",
    "Activate the previously selected tab",
];

/// 双击两档。参考页还把 `Close tab` 当出厂态，这里是 `No action`——接线之前
/// 双击标签就是一次普通单击（只激活），默认值必须保持现状。
const DOUBLE_CLICK_ACTIONS: [&str; TAB_DOUBLE_CLICK_ACTIONS] = ["No action", "Close tab"];

/// 编号三档。0 档是接线前的行为（一条编号都不画），所以默认不改外观。
const NUMBER_MODES: [&str; TAB_NUMBER_MODES] = ["No numbers", "First nine tabs", "All tabs"];

/// 中键两档。默认「没有动作」= 接线前的行为。
const MIDDLE_ACTIONS: [&str; TAB_MIDDLE_CLICK_ACTIONS] = ["No action", "Close tab"];

/// 右键三档。默认「标签菜单」= 接线前的行为（`resp.context_menu` 无条件注册）。
const RIGHT_ACTIONS: [&str; TAB_RIGHT_CLICK_ACTIONS] = ["Tab context menu", "Close tab", "No action"];

/// 悬停激活的四档：0 = 关闭，其余是毫秒数。
const HOVER_DELAYS: [u32; 4] = [0, 200, 500, 1000];
const HOVER_MODES: [&str; 4] = ["Off", "0.2 seconds", "0.5 seconds", "1 second"];

fn hover_index(enabled: bool, delay_ms: u32) -> usize {
    if !enabled {
        return 0;
    }
    HOVER_DELAYS
        .iter()
        .position(|d| *d == delay_ms)
        .unwrap_or(1)
}

/// 缩略图大小档位：0 = 关闭，其余是相对标签宽度的百分比。
const THUMBNAILS: [usize; 4] = [0, 20, 40, 60];
const THUMBNAIL_MODES: [&str; 4] = ["Off", "20 %", "40 %", "60 %"];

fn thumbnail_index(pct: usize) -> usize {
    THUMBNAILS.iter().position(|v| *v == pct).unwrap_or(0)
}

/// 页底说明：所有行都当场生效。
const TABS_NOTE: &str = "Activation after closing, hover activation, double-click, text elision, \
     tab numbers, thumbnails and the middle / right click actions all apply immediately.";

/// 一行**真下拉**：与灰显行共用 `field_row`（标签、字段框、行距都是同一份），
/// 区别只有「控件能不能点」。
///
/// 排布由 `slot` 决定：标签太长时独占一行（`Slot::Above`，否则
/// `Activate the previously selected tab` 在两百出头的宽度里会被截断），
/// 短标签与控件同一行（`Slot::Inline`）。这是兼容终端的两种排布，不是两套风格。
fn live_combo(
    ui: &mut egui::Ui,
    id: &str,
    label: &str,
    slot: Slot,
    col: f32,
    items: &[&str],
    selected: &mut usize,
) -> bool {
    let mut changed = false;
    field_row(ui, label, slot, col, "", |ui| {
        egui::ComboBox::from_id_salt(id)
            .selected_text(items[*selected])
            .width(ui.available_width())
            // `width` 是下限；不锁 wrap 模式时长文案会把固定宽度的对话框顶宽，
            // 页脚 `Close` 就被裁成只剩一个字母（Tabs 页实测）。
            .wrap_mode(egui::TextWrapMode::Truncate)
            .show_ui(ui, |ui| {
                for (i, name) in items.iter().enumerate() {
                    if ui.selectable_label(i == *selected, *name).clicked() {
                        *selected = i;
                        changed = true;
                    }
                }
            });
    });
    changed
}

/// `Tabs` 页。三档改完立刻写进窗口上的副本（本帧剩下的绘制按新值走）并落盘。
pub(in crate::ui) fn tabs(win: &mut TerminalWindow, ui: &mut egui::Ui) {
    let mut activate = win.tab_activate_after_close;
    let mut hover = hover_index(win.tab_hover_activate, win.tab_hover_delay_ms);
    let mut thumbnail = thumbnail_index(win.tab_thumbnail_pct);
    let mut double = win.tab_double_click;
    let mut elide = win.tab_elide;
    let mut numbers = win.tab_numbers;
    let mut middle = win.tab_middle_click;
    let mut right = win.tab_right_click;
    let mut changed = false;

    ui.add_space(6.0);
    // 视觉基线 `06-tabs.png`：内容区第一个字是组标题 `Tab Features`，没有页级灰标题。
    group_box(ui, Some("Tab Features"), |ui| {
        // 行距只有一处出处；原先这里把它压成 0 再手补 4，是「两套节奏」的来源。
        set_row_rhythm(ui);
        changed |= live_combo(
            ui,
            "prefs_tab_activate",
            "Activate the tab after closing:",
            Slot::Above,
            0.0,
            &ACTIVATE_MODES,
            &mut activate,
        );
        // 灰显行直接复用数据页那一套 `Row`：同一页里两种行只差「能不能点」。
        changed |= live_combo(
            ui,
            "prefs_tab_hover",
            "Activate the tab after hovering:",
            Slot::Above,
            0.0,
            &HOVER_MODES,
            &mut hover,
        );
        changed |= live_combo(
            ui,
            "prefs_tab_numbers",
            "Show the tab number on tabs:",
            Slot::Above,
            0.0,
            &NUMBER_MODES,
            &mut numbers,
        );
        let col = label_col(ui, &["Tab text elide mode:", "Tab thumbnail radio:"]);
        changed |= live_combo(
            ui,
            "prefs_tab_elide",
            "Tab text elide mode:",
            Slot::Inline,
            col,
            &ELIDE_MODES,
            &mut elide,
        );
        changed |= live_combo(
            ui,
            "prefs_tab_thumbnail",
            "Tab thumbnail radio:",
            Slot::Inline,
            col,
            &THUMBNAIL_MODES,
            &mut thumbnail,
        );
    });

    ui.add_space(2.0);
    let mouse_col = label_col(ui, &["Double click:", "Middle click:", "Right click:"]);
    group_box(ui, Some("Mouse Actions"), |ui| {
        set_row_rhythm(ui);
        changed |= live_combo(
            ui,
            "prefs_tab_double_click",
            "Double click:",
            Slot::Inline,
            mouse_col,
            &DOUBLE_CLICK_ACTIONS,
            &mut double,
        );
        changed |= live_combo(
            ui,
            "prefs_tab_middle_click",
            "Middle click:",
            Slot::Inline,
            mouse_col,
            &MIDDLE_ACTIONS,
            &mut middle,
        );
        changed |= live_combo(
            ui,
            "prefs_tab_right_click",
            "Right click:",
            Slot::Inline,
            mouse_col,
            &RIGHT_ACTIONS,
            &mut right,
        );
    });
    // 视觉基线没有页脚说明；两行灰显的理由仍在各自 hover 上。

    if changed {
        // 先改内存里的副本（标签条 / 关闭逻辑读到的是同一份），再 load-modify-save。
        // 只写这六个字段：语言 / 字号 / 光标样式 / 启动策略原样保留。
        win.tab_activate_after_close = activate;
        win.tab_hover_activate = hover != 0;
        win.tab_hover_delay_ms = HOVER_DELAYS[hover];
        if !win.tab_hover_activate {
            win.tab_hover = None;
        }
        win.tab_thumbnail_pct = THUMBNAILS[thumbnail];
        win.tab_double_click = double;
        win.tab_elide = elide;
        win.tab_numbers = numbers;
        win.tab_middle_click = middle;
        win.tab_right_click = right;
        save_prefs(|prefs| {
            prefs.tab_activate_after_close = activate;
            prefs.tab_hover_activate = win.tab_hover_activate;
            prefs.tab_hover_delay_ms = win.tab_hover_delay_ms;
            prefs.tab_thumbnail_pct = win.tab_thumbnail_pct;
            prefs.tab_double_click = double;
            prefs.tab_elide = elide;
            prefs.tab_numbers = numbers;
            prefs.tab_middle_click = middle;
            prefs.tab_right_click = right;
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use term_runtime::prefs::TAB_ELIDE_MODES;

    #[test]
    fn the_label_tables_match_the_values_prefs_accepts() {
        // 下拉里有几档，`Prefs` 就必须认几档——多一档会被夹回 0（点了没反应），
        // 少一档则是「存得进去、页面显示不出来」。
        assert_eq!(ACTIVATE_MODES.len(), TAB_ACTIVATE_MODES);
        assert_eq!(DOUBLE_CLICK_ACTIONS.len(), TAB_DOUBLE_CLICK_ACTIONS);
        assert_eq!(ELIDE_MODES.len(), TAB_ELIDE_MODES);
        assert_eq!(NUMBER_MODES.len(), TAB_NUMBER_MODES);
        assert_eq!(MIDDLE_ACTIONS.len(), TAB_MIDDLE_CLICK_ACTIONS);
        assert_eq!(RIGHT_ACTIONS.len(), TAB_RIGHT_CLICK_ACTIONS);
        // 0 档都必须是「现状」的措辞：默认值不会被用户选过，但会显示出来。
        assert_eq!(DOUBLE_CLICK_ACTIONS[0], "No action");
        assert_eq!(ELIDE_MODES[0], "None");
        assert_eq!(ACTIVATE_MODES[0], "Activate the next tab");
        assert_eq!(NUMBER_MODES[0], "No numbers");
        assert_eq!(MIDDLE_ACTIONS[0], "No action");
        assert_eq!(RIGHT_ACTIONS[0], "Tab context menu");
    }

    #[test]
    fn the_page_note_claims_every_row_is_live() {
        assert!(TABS_NOTE.contains("immediately"));
        assert!(!TABS_NOTE.contains("subsystem"), "缩略图已接真");
        assert!(!TABS_NOTE.contains("width algorithm"), "编号已接真");
        assert!(!TABS_NOTE.contains("action set"), "中键/右键已接真");
        assert!(!TABS_NOTE.contains("delay timer"), "悬停激活已接真");
    }

    #[test]
    fn the_thumbnail_options_map_to_percentages() {
        assert_eq!(thumbnail_index(0), 0);
        assert_eq!(thumbnail_index(20), 1);
        assert_eq!(thumbnail_index(40), 2);
        assert_eq!(thumbnail_index(60), 3);
        assert_eq!(thumbnail_index(999), 0);
    }

    #[test]
    fn the_hover_options_map_to_the_stored_delay() {
        assert_eq!(hover_index(false, 200), 0);
        assert_eq!(hover_index(true, 200), 1);
        assert_eq!(hover_index(true, 500), 2);
        assert_eq!(hover_index(true, 1000), 3);
        // 手改了非档位值：显示 0.2 秒，不 panic。
        assert_eq!(hover_index(true, 999), 1);
    }
}

//! `Tabs` 页：三行**真控件** + 五行灰显（部分真，与 `settings_live_text` 同一套写法）。
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
//! 其余五行各自缺的是**不同的东西**，所以逐行写清（悬停 + 页底同一份理由），
//! 而不是共用一句「尚未实现」：
//! - 悬停激活：缺「悬停延时切换」的定时器。
//! - 标签编号：要给编号预留宽度——那是改宽度算法，不只是改文字。
//! - 缩略图：全新子系统（没有预览面，也没有任何东西画它）。
//! - 中键 / 右键动作：动作集里没有别的选项，右键现在就是标签右键菜单。
//!
//! # 铁律
//!
//! 灰行永远是空框 / 空下拉（`settings_pages_view::draw_row`）：勾与选中态传达
//! 「这项已生效」，而它们零读者。

use super::settings_live::save_prefs;
use super::settings_pages_view::{field_box, field_label, group_box, label_col};
use crate::ui::shell::ELIDE_MODES;
use crate::ui::*;
use term_runtime::prefs::{TAB_ACTIVATE_MODES, TAB_DOUBLE_CLICK_ACTIONS};

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

/// 悬停激活缺的是一段定时器。
const HOVER_NOTE: &str = "Not wired: switching on hover needs a per-tab hover timer (the strip \
     has no delay clock), and a strip that switches while the pointer merely passes over it \
     steals focus from whatever you are typing in.";

/// 编号不是画个字那么简单：每条标签都要为它预留宽度。
const NUMBER_NOTE: &str = "Not wired: drawing the tab number reserves width in every tab, so it is \
     a change to the tab width algorithm, not a text change.";
const THUMBNAIL_NOTE: &str = "Not wired: thumbnails are a new subsystem — the strip has no \
     per-tab preview surface and nothing renders one.";

/// 中键：标签条只注册了单击与双击。
const MIDDLE_NOTE: &str = "Not wired: the strip only registers click and double click, and there \
     is no second action set (rename / close / new tab) for the middle button to pick from.";
/// 右键：现在就是标签右键菜单，改成「动作」要另做一套无菜单的落地。
const RIGHT_NOTE: &str = "Right click already opens the tab context menu; making it configurable \
     needs a second action set and a menu-less variant.";

/// 页底说明：哪三行当场生效、其余五行各缺什么。
const TABS_NOTE: &str = "Activation after closing, double-click and text elision apply \
     immediately. Hover activation needs a delay timer, tab numbers would change the width \
     algorithm, thumbnails are a new subsystem, and middle / right click have no second action \
     set — see each row's tooltip.";

/// 一行真下拉：**标签在控件上方**，控件占满整行。
///
/// 视觉基线  的前三行是这个排布（标签太长，挤在一行里下拉只剩
/// 两百出头，`Activate the previously selected tab` 会被截断）。第 4/5 行与
/// `Mouse Actions` 那三行的标签短，视觉基线是**同一行**「标签 + 控件」——
/// 由 [`inline_combo`] / [`dead_inline`] 画。
fn live_combo(
    ui: &mut egui::Ui,
    id: &str,
    label: &str,
    items: &[&str],
    selected: &mut usize,
) -> bool {
    let mut changed = false;
    ui.label(label);
    egui::ComboBox::from_id_salt(id)
        .selected_text(items[*selected])
        .width(ui.available_width)
        // `width` 是下限；不锁 wrap 模式时长文案会把固定宽度的对话框顶宽，
        // 页脚 `Close` 就被裁成只剩一个字母（Tabs 页实测）。
        .wrap_mode(egui::TextWrapMode::Truncate)
        .show_ui(ui, |ui| {
            for (i, name) in items.iter.enumerate {
                if ui.selectable_label(i == *selected, *name).clicked {
                    *selected = i;
                    changed = true;
                }
            }
        });
    changed
}

/// 一行「左对齐标签 + 通栏下拉」（视觉基线`Tab text elide mode:` /
/// `Mouse Actions` 三行的排布）。
fn inline_combo(
    ui: &mut egui::Ui,
    id: &str,
    label: &str,
    col: f32,
    items: &[&str],
    selected: &mut usize,
) -> bool {
    let mut changed = false;
    ui.horizontal(|ui| {
        field_label(ui, label, col);
        egui::ComboBox::from_id_salt(id)
            .selected_text(items[*selected])
            .width(ui.available_width)
            .wrap_mode(egui::TextWrapMode::Truncate)
            .show_ui(ui, |ui| {
                for (i, name) in items.iter.enumerate {
                    if ui.selectable_label(i == *selected, *name).clicked {
                        *selected = i;
                        changed = true;
                    }
                }
            });
    });
    changed
}

/// 灰显的堆叠下拉：标签完整、控件占满整行、说明理由挂在 hover 上。
fn dead_combo(ui: &mut egui::Ui, label: &str, value: &str, note: &str) {
    ui.label(label);
    let mut shown = value.to_string;
    ui.add_enabled(
        false,
        egui::TextEdit::singleline(&mut shown).desired_width(ui.available_width),
    )
    .on_disabled_hover_text(note);
}

/// 灰显的**同一行**「标签 + 控件」：视觉基线`Middle click:` / `Right click:`
/// 与 `Tab thumbnail radio:` 都是这个排布。
fn dead_inline(ui: &mut egui::Ui, label: &str, value: &str, col: f32, note: &str) {
    ui.horizontal(|ui| {
        field_label(ui, label, col);
        let mut shown = value.to_string;
        field_box(ui, |ui| {
            ui.add_enabled(
                false,
                egui::TextEdit::singleline(&mut shown).desired_width(ui.available_width),
            )
            .on_disabled_hover_text(note);
        });
    });
}

/// `Tabs` 页。三档改完立刻写进窗口上的副本（本帧剩下的绘制按新值走）并落盘。
pub(in crate::ui) fn tabs(win: &mut TerminalWindow, ui: &mut egui::Ui) {
    let mut activate = win.tab_activate_after_close;
    let mut double = win.tab_double_click;
    let mut elide = win.tab_elide;
    let mut changed = false;

    ui.add_space(6.0);
    // 视觉基线：内容区第一个字是组标题 `Tab Features`，没有页级灰标题。
    group_box(ui, Some("Tab Features"), |ui| {
        // 堆叠排布一行的高度翻倍；行距必须收紧，否则八行把固定高度的对话框
        // 顶大，页脚 Close 会压在最后一行上（实测）。
        ui.spacing_mut.item_spacing.y = 0.0;
        changed |= live_combo(
            ui,
            "prefs_tab_activate",
            "Activate the tab after closing:",
            &ACTIVATE_MODES,
            &mut activate,
        );
        dead_combo(
            ui,
            "Activate the tab after hovering:",
            "0.2 seconds",
            HOVER_NOTE,
        );
        dead_combo(
            ui,
            "Show the tab number on tabs:",
            "First nine tabs",
            NUMBER_NOTE,
        );
        // 后两行的标签短，视觉基线是同一行「标签 + 控件」。堆叠行收紧到 0 之后
        // 这里要补 4 点，否则上一行的下拉底边贴着这一行的标签。
        ui.add_space(4.0);
        let col = label_col(ui, &["Tab text elide mode:", "Tab thumbnail radio:"]);
        changed |= inline_combo(
            ui,
            "prefs_tab_elide",
            "Tab text elide mode:",
            col,
            &ELIDE_MODES,
            &mut elide,
        );
        dead_inline(ui, "Tab thumbnail radio:", "40 %", col, THUMBNAIL_NOTE);
    });

    ui.add_space(2.0);
    let mouse_col = label_col(ui, &["Double click:", "Middle click:", "Right click:"]);
    group_box(ui, Some("Mouse Actions"), |ui| {
        ui.spacing_mut.item_spacing.y = 4.0;
        changed |= inline_combo(
            ui,
            "prefs_tab_double_click",
            "Double click:",
            mouse_col,
            &DOUBLE_CLICK_ACTIONS,
            &mut double,
        );
        dead_inline(ui, "Middle click:", "Rename tab", mouse_col, MIDDLE_NOTE);
        dead_inline(ui, "Right click:", "No action", mouse_col, RIGHT_NOTE);
    });
    // 视觉基线没有页脚说明；五行灰显的理由仍在各自 hover 上。

    if changed {
        // 先改内存里的副本（标签条 / 关闭逻辑读到的是同一份），再 load-modify-save。
        // 只写这三个字段：语言 / 字号 / 光标样式 / 启动策略原样保留。
        win.tab_activate_after_close = activate;
        win.tab_double_click = double;
        win.tab_elide = elide;
        save_prefs(|prefs| {
            prefs.tab_activate_after_close = activate;
            prefs.tab_double_click = double;
            prefs.tab_elide = elide;
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use term_runtime::prefs::TAB_ELIDE_MODES;

    #[test]
    fn the_label_tables_match_the_values_prefs_accepts {
        // 下拉里有几档，`Prefs` 就必须认几档——多一档会被夹回 0（点了没反应），
        // 少一档则是「存得进去、页面显示不出来」。
        assert_eq!(ACTIVATE_MODES.len, TAB_ACTIVATE_MODES);
        assert_eq!(DOUBLE_CLICK_ACTIONS.len, TAB_DOUBLE_CLICK_ACTIONS);
        assert_eq!(ELIDE_MODES.len, TAB_ELIDE_MODES);
        // 0 档都必须是「现状」的措辞：默认值不会被用户选过，但会显示出来。
        assert_eq!(DOUBLE_CLICK_ACTIONS[0], "No action");
        assert_eq!(ELIDE_MODES[0], "None");
        assert_eq!(ACTIVATE_MODES[0], "Activate the next tab");
    }

    #[test]
    fn the_page_note_names_every_dead_row_and_why {
        // 五行灰显缺的是五种不同的东西，页底要能逐个对上；
        // 一句「尚未实现」对下一个人没有信息量。
        for needle in ["delay timer", "width algorithm", "subsystem", "action set"] {
            assert!(TABS_NOTE.contains(needle), "页底缺少「{needle}」这一条理由");
        }
        // 哪几行当场生效也要写清，否则用户不知道点了有没有用。
        assert!(TABS_NOTE.contains("immediately"));
        // 中键 / 右键各有各的理由，不能共用一句。
        assert!(MIDDLE_NOTE.contains("click and double click"));
        assert!(RIGHT_NOTE.contains("context menu"));
    }
}

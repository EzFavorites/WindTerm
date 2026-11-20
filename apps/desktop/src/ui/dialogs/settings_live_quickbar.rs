//! `Quick Bar` 页：三项**真控件**（2026-11-19 从灰显占位接成真）。
//!
//! 这一页原先是「数据表 + 三个 `DisabledCheck`」，理由是「Show multiple groups
//! 已有写入者，再接一遍就成了第二份真相」。那个判断漏了一件事：写者本来就有
//! 两个——底栏齿轮里的配置弹窗（`dialogs::quick_commands`）已经把这三个开关
//! 落进 `prefs.json`；设置页读的也是同一份 `Prefs`，写回仍是同一个
//! `settings_live::save_prefs` 入口。真正的「第二份真相」是窗口上的运行期副本
//! `quick_bar_display`，所以接线时必须**同时**更新它，否则勾会变、底栏不变。
//!
//! 三名控件的落点都是现成的：
//! - `Show group commands in order` → `!QuickBarDisplay.alphabetical`
//!   （`quick_command_bar::group_views` 按它排序）；
//! - `Show group title` → `quick_command_bar::draw_group` / `quick_layout`；
//! - `Show multiple groups` → `quick_command_bar` 的组展开分支。
//!
//! 三个字段在 `Prefs` 里都有名字，落盘和齿轮弹窗共用同一条 load-modify-save。

use super::settings_live::save_prefs;
use super::settings_live_text::page_title;
use super::settings_pages_view::group_box;
use crate::ui::*;

/// `Quick Bar` 页。复制出显示开关的副本，画完再一次性同步内存与磁盘。
pub(in crate::ui) fn quickbar(win: &mut TerminalWindow, ui: &mut egui::Ui) {
    let mut display = win.quick_bar_display;
    let before = display;
    let mut in_order = !display.alphabetical;

    page_title(ui, "Quick Bar");
    group_box(ui, |ui| {
        ui.spacing_mut().item_spacing.y = 6.0;
        ui.label(egui::RichText::new("Features").strong());
        ui.add_space(11.0);
        if ui
            .checkbox(&mut in_order, "Show group commands in order")
            .on_hover_text("Off shows the commands in the bar sorted alphabetically.")
            .changed()
        {
            display.alphabetical = !in_order;
        }
        if ui
            .checkbox(&mut display.show_group_title, "Show group title")
            .on_hover_text("Shows the group name above its commands in the bottom bar.")
            .changed()
        {}
        if ui
            .checkbox(&mut display.show_multiple_groups, "Show multiple groups")
            .on_hover_text("Off collapses the bottom bar to its first group.")
            .changed()
        {}
    });
    if display == before {
        return;
    }
    win.quick_bar_display = display;
    save_prefs(|prefs| {
        prefs.quickbar_show_multiple_groups = display.show_multiple_groups;
        prefs.quickbar_show_group_title = display.show_group_title;
        prefs.quickbar_alphabetical = display.alphabetical;
    });
}

#[cfg(test)]
mod tests {
    // 走真实 crate 路径，不经 term_runtime 门面（verify 4s 基线只许减不许增）。
    use term_profile::prefs::Prefs;

    #[test]
    fn the_three_checkboxes_match_the_factory_preferences() {
        let d = Prefs::default().quick_bar_display();
        // 出厂：多组 + 组标题 + 配置顺序（不按字母序）。
        assert!(d.show_multiple_groups && d.show_group_title);
        assert!(!d.alphabetical, "in_order 复选框出厂必须是勾上的");
    }
}

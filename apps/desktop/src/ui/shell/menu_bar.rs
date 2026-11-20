//! 菜单**顶栏**：十个菜单按钮 + 顶栏专用外观。
//!
//! 2026-11-05 从 `ui/mod.rs` 拆出；2026-11-08 又把「弹层里一行怎么画」
//! 拆到 `menu_popup.rs`——勾选栏让这个文件涨到 613 行，超了 450 行。
//! 现在它只剩两件事：`draw_menu`（摆按钮）与 `flat_menu_style`（顶栏无框）。

use crate::ui::*;
use super::menu_items::draw_menu_items;
use super::menu_style::{col, MENU_HIGHLIGHT};

impl TerminalWindow {
    /// 画顶级菜单。**不再套 `egui::menu::bar`**：它内部用的是 `ui.horizontal`
    /// （顶部对齐），菜单文字因此在 21px 的条里整体偏上 2.75px。改成由调用点
    /// 用 `horizontal_centered` 提供居中的布局，这里只按顺序摆菜单按钮。
    pub(in crate::ui) fn draw_menu(&mut self, ui: &mut egui::Ui) {
        flat_menu_style(ui.style_mut());
        for menu in menus() {
            let title = self.tr(menu.label);
            ui.menu_button(title, |ui| {
                draw_menu_items(self, ui, &menu.items);
            });
        }
        super::menu_tools::draw(self, ui);
    }
}

/// 菜单栏专用外观：**平的**——无底色、无描边，连展开态也不高亮。
///
/// # 为什么需要这个函数
///
/// `ui.menu_button` 内部是一个 `Button`（`frame` 默认为真），会读主题里的
/// `widgets.inactive`：底色 `#262626`、描边 `#444444`。于是十个菜单项各自
/// 变成一格带边框的方块，整条菜单栏看起来像一排按钮。用户原话：
/// 「（菜单栏）没有框和背景，实现的是框起来的」。
///
/// 对照视觉基线逐像素确认过：`Goto` **展开时**文字周围同样没有任何高亮块，
/// 底色始终是 `#1C1C1C`，所以 `open` 也要清掉——只清 `inactive` 会让展开的
/// 那一项单独冒出个方块，比常态有框更显眼。
///
/// 处置照抄 egui 自己的 `menu::bar` / `set_menu_style`（同一套三行），再加
/// 两条：`open` 一并清掉、`item_spacing` 收到 [`MENU_ITEM_GAP`]（字距的
/// 依据见那个常量的表）。
///
/// 悬停保留主题的 `#2A2A2A`：静态基线里不出现，但「鼠标在哪一项上」需要
/// 反馈。同理描边在悬停/按下态也置空——否则鼠标一过又变回方块。
fn flat_menu_style(style: &mut egui::Style) {
    use egui::Stroke;

    // 零内边距 + 14 px 间距 = 视觉基线实测的 31.5 px 墨迹间隙。
    style.spacing.button_padding.x = 0.0;
    style.spacing.item_spacing.x = super::MENU_ITEM_GAP;

    let w = &mut style.visuals.widgets;
    w.inactive.weak_bg_fill = egui::Color32::TRANSPARENT;
    w.inactive.bg_stroke = Stroke::NONE;
    // 2026-11-08 新参考：顶级菜单**展开时**是蓝块（旧结论「无高亮」作废）；
    // 悬停仍保持 #2A2A2A（新截图没拍到悬停态，维持旧观察）。
    w.open.weak_bg_fill = col(MENU_HIGHLIGHT);
    w.open.bg_stroke = Stroke::NONE;
    w.hovered.bg_stroke = Stroke::NONE;
    w.active.bg_stroke = Stroke::NONE;
}


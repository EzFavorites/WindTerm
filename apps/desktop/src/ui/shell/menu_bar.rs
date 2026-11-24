//! 菜单**顶栏**：十个菜单按钮 + 顶栏专用外观。
//!
//! 2026-11-05 从 `ui/mod.rs` 拆出；2026-11-08 又把「弹层里一行怎么画」
//! 拆到 `menu_popup.rs`——勾选栏让这个文件涨到 613 行，超了 450 行。
//! 现在它只剩两件事：`draw_menu`（摆按钮）与 `flat_menu_style`（顶栏无框）。

use crate::ui::*;
use super::menu_items::draw_menu_items;
use super::menu_style::{col, MENU_HIGHLIGHT};

/// 此刻有没有顶级菜单弹层开着。
///
/// 自动隐藏的菜单条不能在用户正看菜单时收起来：egui 的弹层是独立 `Area`，
/// 但按钮一旦不画，弹层的锚点就丢了。这里按菜单标题反查 `Memory` 里的
/// popup id（与 `ui.menu_button` 用的是同一套 id），让宿主在展开期间保持高度。
pub(in crate::ui) fn any_menu_open(ctx: &egui::Context, win: &TerminalWindow) -> bool {
    menus().iter().any(|menu| {
        let title = win.tr(menu.label);
        ctx.memory(|mem| mem.is_popup_open(egui::Id::new(title)))
    })
}

/// 自动隐藏口径下这一帧菜单条该占多高。
///
/// 指针靠近顶端 8pt 内、按住 Alt、或有菜单弹层打开时展开（并保持 1 秒，
/// 避免细带边缘抖动）；否则收成 4pt 细带，指针落在带上就会再次展开。
pub(in crate::ui) fn menu_bar_height(ctx: &egui::Context, win: &mut TerminalWindow) -> f32 {
    if !win.auto_hide_menu_bar {
        return super::MENU_BAR_H;
    }
    let now = ctx.input(|i| i.time);
    let alt = ctx.input(|i| i.modifiers.alt);
    let near_top = ctx.input(|i| {
        i.pointer
            .hover_pos()
            .is_some_and(|p| p.y <= super::MENU_BAR_H + 8.0)
    });
    let open = any_menu_open(ctx, win);
    if near_top || alt || open {
        win.menu_bar_peek_until = now + 1.0;
    }
    height_for(win.menu_bar_peek_until, now, open)
}

/// 纯高度判据：展开到时刻之前（或有菜单弹层）用全高，否则细带。
fn height_for(peek_until: f64, now: f64, open: bool) -> f32 {
    if open || now < peek_until {
        super::MENU_BAR_H
    } else {
        4.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_collapsed_menu_bar_stays_tall_until_the_peek_window_ends() {
        assert_eq!(height_for(10.0, 9.9, false), crate::ui::shell::MENU_BAR_H);
        assert_eq!(height_for(10.0, 10.0, false), 4.0);
        // 菜单开着时与 peek 时钟无关：不能在用户看菜单时收起来。
        assert_eq!(height_for(0.0, 99.0, true), crate::ui::shell::MENU_BAR_H);
    }
}

impl TerminalWindow {
    /// 画顶级菜单。**不再套 `egui::menu::bar`**：它内部用的是 `ui.horizontal`
    /// （顶部对齐），菜单文字因此在 21px 的条里整体偏上 2.75px。改成由调用点
    /// 用 `horizontal_centered` 提供居中的布局，这里只按顺序摆菜单按钮。
    pub(in crate::ui) fn draw_menu(&mut self, ui: &mut egui::Ui) {
        // 只在本条菜单的绘制窗口里改全局 widget 样式：原版顶级按钮 hover 是
        // 与展开态同一块蓝（不是主题灰），画完必须还原，免得整窗其它按钮跟着变蓝。
        let prev_hover = ui.style().visuals.widgets.hovered.weak_bg_fill;
        flat_menu_style(ui.style_mut());
        for menu in menus() {
            let title = self.tr(menu.label);
            ui.menu_button(title, |ui| {
                draw_menu_items(self, ui, &menu.items);
            });
        }
        ui.style_mut().visuals.widgets.hovered.weak_bg_fill = prev_hover;
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
    // 原版顶级按钮 hover 与展开同色（0x007CD2），不是主题灰 #2A2A2A。
    w.hovered.weak_bg_fill = col(MENU_HIGHLIGHT);
    w.hovered.bg_stroke = Stroke::NONE;
    w.active.bg_stroke = Stroke::NONE;
}


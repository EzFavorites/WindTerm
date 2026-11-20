//! 模态对话框的灰色遮罩：盖住主窗口、让对话框从背景里浮出来。
//!
//! # 为什么单独一层
//!
//! 对话框 themselves 走 `DialogWindow`（`Order::Middle`），主窗口的菜单栏 /
//! 面板 / 终端都在 `Order::Background`。缺的是中间那一层：只把对话框画上去，
//! 它和底下的主窗口同色同亮，看起来像「嵌在主窗口里」而不是一个模态窗口。
//!
//! 遮罩用 `Order::PanelResizeLine`——egui 的顺序是
//! Background → PanelResizeLine → Middle → Foreground，正好「在所有面板之上、
//! 在所有对话框之下」。既不用改每个对话框的绘制，也不用担心调用先后：
//! 层序由 `Order` 决定，不由 `draw()` 里的语句顺序决定。
//!
//! 同时分配一个整屏 `Sense::click` 命中区：模态开着时点主窗口不该穿透过去
//! （例如点着 Preferences 时误触终端或菜单）。对话框在自己更高的层里，
//! 所以点击照常先落到对话框上。

use eframe::egui;

/// 遮罩的黑度。约 44%：主窗口仍可辨认（看得出自己盖住了什么），
/// 但对比度足够让对话框的边框与内容立刻分层。
const DIM_ALPHA: u8 = 112;

/// 铺一层整屏灰罩。只在确实有模态开着时调用：空闲路径连 Area 都不创建。
pub(in crate::ui) fn draw_backdrop(ctx: &egui::Context) {
    egui::Area::new(egui::Id::new("modal_backdrop"))
        // 屏幕坐标即 Area 本地坐标，整块从 (0,0) 铺到 screen_rect。
        .fixed_pos(egui::pos2(0.0, 0.0))
        .movable(false)
        // 夹具把时间钉在 0；默认淡入会让遮罩永远停在几乎全透明的第一帧。
        .fade_in(false)
        .order(egui::Order::PanelResizeLine)
        .show(ctx, |ui| {
            let screen = ctx.screen_rect();
            // 吃掉穿透点击；返回值不关心，交互本身才是目的。
            let _ = ui.allocate_rect(screen, egui::Sense::click());
            ui.painter().rect_filled(
                screen,
                egui::Rounding::ZERO,
                egui::Color32::from_black_alpha(DIM_ALPHA),
            );
        });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dim_is_visible_but_not_opaque() {
        // 全黑就成了「另一个窗口」，全透明又回到「融为一体」的毛病。
        assert!(DIM_ALPHA > 64 && DIM_ALPHA < 160);
    }
}

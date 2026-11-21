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
//! 层序由 `Order` 决定，不由 `draw ` 里的语句顺序决定。
//!
//! 同时分配一个整屏 `Sense::click` 命中区：模态开着时点主窗口不该穿透过去
//! （例如点着 Preferences 时误触终端或菜单）。对话框在自己更高的层里，
//! 所以点击照常先落到对话框上。

use eframe::egui;

/// 遮罩的黑度。
///
/// # 为什么是 69% 而不是「大约一半」
///
/// 这里踩过一个反复被用户投诉的坑：遮罩一直在画，但**看不出来**。
///
/// 本仓是深色主题，主窗口底是 `#1C1C1C`（28），弹窗底也是 `#1C1C1C`。黑色遮罩
/// 只能**把 28 压低**，压多少由 alpha 决定：44%（112）压到 `#101010`（16），
/// 与弹窗的 28 只差 12 级灰——两块深色之间的 12 级差在显示器上几乎不可辨，
/// 用户的原话是「两个一个颜色，区分不出来」。
///
/// 深色主题里「压暗」这个方向本身没有余量（下界是 0），所以要么把主体压到明显
/// 低于弹窗，要么换方向去抬弹窗。这里两件一起做：
///
/// - 遮罩压到 69%（176）：`#1C1C1C` → `#090909`（9），与弹窗的 28 差 3 倍，
///   视觉上是一道明确的台阶；
/// - 弹窗自己加一条比本体亮的 1pt 描边（见 `kit::window::dialog_frame`）——
///   深色里「边界」靠描边比靠投影可靠，投影（黑）投在更黑的主体上等于没有。
const DIM_ALPHA: u8 = 176;

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

    /// 遮罩必须把主体压到**弹窗底色的一半以下**，否则又回到「两个一个颜色」。
    ///
    /// 阈值按**比例**定，不按色阶差：深色主题里 28→16 只差 12 级，看着一模一样；
    /// 而 28→9 差 19 级，视觉上却是三倍亮度差。绝对差值在暗端没有意义。
    #[test]
    fn dim_makes_a_visible_step_below_the_dialog_surface() {
        let surface = 0x1C1C1C_u32 as f32; // CHROME_SCALE.dialog，与弹窗底同一个值
        let dimmed = surface * (1.0 - DIM_ALPHA as f32 / 255.0);
        assert!(
            dimmed <= surface * 0.4,
            "遮罩只把主体压到 {dimmed:.1}（弹窗底 {surface:.0}），两块仍在同一档灰上"
        );
    }

    /// 但也不能压成纯黑：那样主窗口就「没了」，用户看不出自己盖住的是什么。
    #[test]
    fn dim_is_visible_but_not_opaque() {
        let surface = 0x1C1C1C_u32 as f32;
        let dimmed = surface * (1.0 - DIM_ALPHA as f32 / 255.0);
        assert!(dimmed > 4.0, "遮罩压到 {dimmed:.1}，主窗口等于被抹掉了");
        assert!(DIM_ALPHA < 220, "遮罩 alpha={DIM_ALPHA} 已经接近不透明");
    }
}

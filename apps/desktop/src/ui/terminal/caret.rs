//! 光标形状的绘制（从 `pane.rs` 拆出，4j：单文件 500 代码行上限）。
//! 只吃几何与颜色，不碰任何状态；公开路径仍是 `terminal::draw_caret`。

use eframe::egui::{self, Rect, Vec2};

/// 按光标形状绘制光标。`fg` 取调色板前景色。
///
/// 四种形状对应 `grid::CursorShape`：
/// - `Block`：实心填充整格（块光标）。
/// - `HollowBlock`：描边空心方块（旧实现的唯一行为）。
/// - `Underline`：格底一条横杠。
/// - `Beam`：格左一条竖杠（「Line / I 形」光标）。
///
/// 闪烁不在这里处理——由调用方按墙钟相位决定本帧是否调用本函数。

///
/// 可见性放到 `pub(in crate::ui)`，`settings_live_caret` 的预览方块直接
/// 调用它——光标长什么样只有这一份实现，设置页的预览与终端里画的光标不会走样。
///
/// 2026-11-19：`width_px` 是 `Underline` / `Beam` 的**绝对像素宽度**，来自
/// `SessionConfig.caret_width`（视觉基线 `1.5 pixels`）。此前是 `cell_h * 0.14` /
/// `cell_w * 0.12` 的比例常数——随字号缩放的「相对粗细」。改成像素后同一档宽度
/// 在 12pt 与 20pt 下视觉一致；夹进 `[0.5, 格宽/格高]` 防止写满整格。
/// `Block` / `HollowBlock` 是整格填充，不使用这个参数。
pub(in crate::ui) fn draw_caret(
    painter: &egui::Painter,
    cell: Rect,
    shape: term_core::grid::CursorShape,
    fg: egui::Color32,
    cell_w: f32,
    cell_h: f32,
    width_px: f32,
) {
    match shape {
        term_core::grid::CursorShape::Block => {
            painter.rect_filled(cell, 0.0, fg);
        }
        term_core::grid::CursorShape::HollowBlock => {
            painter.rect_stroke(cell, 0.0, egui::Stroke::new(1.0, fg));
        }
        term_core::grid::CursorShape::Underline => {
            let h = width_px.clamp(0.5, cell_h * 0.9);
            let bar = Rect::from_min_size(
                egui::pos2(cell.min.x, cell.max.y - h),
                Vec2::new(cell.width(), h),
            );
            painter.rect_filled(bar, 0.0, fg);
        }
        term_core::grid::CursorShape::Beam => {
            let w = width_px.clamp(0.5, cell_w * 0.9);
            let bar = Rect::from_min_size(
                egui::pos2(cell.min.x, cell.min.y),
                Vec2::new(w, cell.height()),
            );
            painter.rect_filled(bar, 0.0, fg);
        }
    }
}

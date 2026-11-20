//! 全软件唯一的 `×` / `⚙` 画法，以及「右端那一对图标」的统一落位。
//!
//! # 为什么要有这个文件
//!
//! 2026-11-17 之前，仓里躺着**五份** `×` 与**三份** `⚙`：
//!
//! | 位置 | 画法 | 墨迹 |
//! |---|---|---|
//! | `shell::tab_bar` | `line_segment`，半长 3.5 / 线宽 1.2 | 8.2 见方 |
//! | `layout::dock_container` | 排 `×` 字形，13pt，按 `mesh_bounds` 反推落点 | ≈7 |
//! | `shell::quick_command_bar` | 排 `×` / `⚙` 字形，12pt × 1.15 | 7 / 10.5 |
//! | `panels::session_icons` | `line_segment`，半长由调用方传 / 线宽 1.4 | 随调用方 |
//! | `layout::dock_bottom_menu` | `BTN_ICON * 0.35` / 圆 `BTN_ICON * 0.30` | 随常量 |
//!
//! 后果是同一条窗口右下角量出三种尺寸：Find 栏的 `×` 6.5、快捷栏的 `×` 7.0
//! 且**整体偏左 6.5 点**、面板标题带的 `×` 又是排出来的字形；`⚙` 在快捷栏
//! 10.5、在面板标题带 9.0。用户 2026-11-17 的要求原话是「齿轮这个图标，x 这个
//! 图标，到全软件统一……右下角这里，最好做到纵向对齐，在各种栏目里面，上下
//! 间距调整到合适」。
//!
//! # 目标尺寸（视觉基线 @2× 截图 ÷2 = 逻辑点）
//!
//! ```text
//! ×   墨迹 6.5 × 6.5       = 对角线 5.5 + 线宽 1.0（线宽在两端各外扩 0.5）
//! ⚙   墨迹 10.0 × 10.0     = 齿顶圆 + 线宽；中心是**空的**（环，不是实心圆）
//! ⚙ 墨迹右沿 → × 墨迹左沿 = 10.5
//! × 墨迹右沿 → 栏右端     = 7.5
//! ```
//!
//! 三处独立量测同向：
//!
//!   列 2510..2529、`×` 墨迹列 2552..2564；
//! * 同一张图的 Find 行：`×` 墨迹列 2552..2564 —— 与快捷栏**同一个 x**，
//!   「各栏目纵向对齐」这件事视觉基线自己就钉死了；
//!   363..382、`×` 列 402..413。
//!
//! # 纪律：只画形状，不排字
//!
//! `⚙` / `×` 在夹具字体（egui 自带的 Ubuntu-Light / NotoEmoji）里有没有字形
//! 不可控，缺字会画成豆腐块，逐字节基线就会在不同机器上给出不同的图。这一条与
//! `panels/session_icons.rs` 的模块头同源：**能画就不排字**。

use eframe::egui;

/// 两颗图标共用的线宽。视觉基线里它们的笔画粗细一致（1 逻辑点）。
pub(in crate::ui) const ICON_STROKE: f32 = 1.0;

/// `×` 的**视觉边长**（@2× 实测 13 物理像素 = 6.5 逻辑点）。
///
/// 两条对角线按 `CROSS_INK - ICON_STROKE` 画：1 点线宽在两端各外扩 0.5，
/// 加回去才是屏幕上量到的那个方块。写 6.5 却按 6.5 画对角线，量出来会是 7.5。
pub(in crate::ui) const CROSS_INK: f32 = 6.5;

/// `⚙` 的**视觉外径**（含描边外扩）。
pub(in crate::ui) const GEAR_INK: f32 = 10.0;

/// `⚙` 齿顶圆半径：外径一半减去半个线宽。
pub(in crate::ui) const GEAR_TIP: f32 = GEAR_INK / 2.0 - ICON_STROKE / 2.0;

/// `⚙` 环的半径。视觉基线中心那个洞直径 4.5（@2× 实测 9 物理像素），
/// 所以洞半径 2.25，加上半个线宽就是环的半径。
pub(in crate::ui) const GEAR_RING: f32 = 2.75;

/// 齿根半径：**落在环的笔画正中间**。
///
/// 这一位 2026-11-17 实测踩过：先取 2.35（比洞半径 2.25 只外一点），看似「探进
/// 环里」，实际线段端点是平头、线宽在两端各外扩 0.5，齿根那半截就**戳进洞里**
/// 0.4——八颗齿一起把洞从 4.5 吃到 4.0，齿轮看着比视觉基线「实」。改成环半径之后
/// 齿的内端完全藏在环的笔画里，洞就是量到的那个 4.5。
pub(in crate::ui) const GEAR_ROOT: f32 = GEAR_RING;

/// `⚙` 墨迹右沿 → `×` 墨迹左沿。
pub(in crate::ui) const ICON_GAP: f32 = 10.5;

/// `×` 墨迹右沿 → 栏右端。
pub(in crate::ui) const ICON_R_PAD: f32 = 7.5;

/// 每颗图标的命中框边长。比墨迹大一圈，6.5 点见方的热区太难点。
pub(in crate::ui) const HIT: f32 = 15.0;

/// 「`⚙` + `×`」这一对占的**排版宽度**。
///
/// 从 `⚙` 墨迹左沿再往左留 2 点余量算到栏右端：34.5 − 6.5 + 2 = 30。
/// 给流式布局（快捷栏的 `right_to_left`）预留位用——那一路不按这个宽度画，
/// 只靠它把游标推到图标左边，免得后面的「溢出 ▾ / 竖线」压到图标上。
pub(in crate::ui) const PAIR_W: f32 = 30.0;

/// `×`：两条 45° 斜线。
pub(in crate::ui) fn paint_cross(p: &egui::Painter, center: egui::Pos2, color: egui::Color32) {
    let d = (CROSS_INK - ICON_STROKE) / 2.0;
    let st = egui::Stroke::new(ICON_STROKE, color);
    for (a, b) in [
        (center + egui::vec2(-d, -d), center + egui::vec2(d, d)),
        (center + egui::vec2(d, -d), center + egui::vec2(-d, d)),
    ] {
        p.line_segment([a, b], st);
    }
}

/// `⚙`：一个环 + 八颗辐射短齿（中心留空）。
pub(in crate::ui) fn paint_gear(p: &egui::Painter, center: egui::Pos2, color: egui::Color32) {
    let st = egui::Stroke::new(ICON_STROKE, color);
    p.circle_stroke(center, GEAR_RING, st);
    for i in 0..8 {
        let a = std::f32::consts::TAU * i as f32 / 8.0;
        let (s, c) = a.sin_cos();
        p.line_segment(
            [
                egui::pos2(center.x + c * GEAR_ROOT, center.y + s * GEAR_ROOT),
                egui::pos2(center.x + c * GEAR_TIP, center.y + s * GEAR_TIP),
            ],
            st,
        );
    }
}

/// 给定栏右沿与栏内中线，返回「`⚙` 中心 / `×` 中心」。
///
/// `right` 是**栏的右沿**（不是内容区右沿）。走 `screen_rect()` 的地方拿到的
/// 就是窗口右端，视觉基线的 7.5 也是从窗口右端量的；侧栏标题带传自己那条带的
/// 右沿。两种调用得到的是同一个「离右沿 7.5」。
pub(in crate::ui) fn pair_centers(right: f32, cy: f32) -> (egui::Pos2, egui::Pos2) {
    let cross_cx = right - ICON_R_PAD - CROSS_INK / 2.0;
    let gear_cx = cross_cx - CROSS_INK / 2.0 - ICON_GAP - GEAR_INK / 2.0;
    (egui::pos2(gear_cx, cy), egui::pos2(cross_cx, cy))
}

/// 与 [`pair_centers`] 同源的两块命中框（`⚙` 在前）。
pub(in crate::ui) fn pair_rects(right: f32, cy: f32) -> (egui::Rect, egui::Rect) {
    let (g, c) = pair_centers(right, cy);
    (
        egui::Rect::from_center_size(g, egui::Vec2::splat(HIT)),
        egui::Rect::from_center_size(c, egui::Vec2::splat(HIT)),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 视觉基线那几个数是死约定。任何一格改宽，这条先红——不然「统一」会悄悄
    /// 退化成「今天统一下午再说」。
    #[test]
    fn the_pair_lands_on_the_reference_offsets() {
        let right = 1000.0;
        let (gear, cross) = pair_centers(right, 0.0);
        // `×` 墨迹右沿距栏右端 7.5。
        assert_eq!(cross.x + CROSS_INK / 2.0, right - ICON_R_PAD);
        // `⚙` 墨迹右沿 → `×` 墨迹左沿 10.5。
        assert_eq!(
            cross.x - CROSS_INK / 2.0 - (gear.x + GEAR_INK / 2.0),
            ICON_GAP
        );
        // 视觉边长就是视觉基线量到的方块；对角线 = 边长 − 线宽。
        assert_eq!(CROSS_INK, 6.5);
        assert_eq!(GEAR_INK, 10.0);
        assert_eq!(2.0 * (CROSS_INK - ICON_STROKE) / 2.0 + ICON_STROKE, CROSS_INK);
        // 齿顶 + 半个线宽 = 视觉外径，不然 `⚙` 会比 `×` 看着胖一圈。
        assert_eq!(GEAR_TIP + ICON_STROKE / 2.0, GEAR_INK / 2.0);
        // 命中框不能互相压住，否则点 `×` 会被 `⚙` 抢走。
        let (gr, cr) = pair_rects(right, 0.0);
        assert!(!gr.intersects(cr));
        // 排版预留宽度要盖得住这一对的墨迹（27 点），否则快捷栏的溢出按钮
        // 会压在齿轮左齿上。
        let pair_ink = (right - ICON_R_PAD) - (gear.x - GEAR_INK / 2.0);
        assert_eq!(pair_ink, 27.0);
        assert!(PAIR_W > pair_ink);
    }
}

//! 工具行与工具栏面板上的小图形：抓手、窗口三态、下拉箭头、面包屑箭头、(i)。
//!
//! # 为什么全部是绘图原语
//!
//! 视觉基线里这些是 1 px 描边的几何图形，字体里没有对应字形；用 emoji 或 `⚙`
//! 之类还要赌字体覆盖，而且渲染出来大小随字体走，对不齐基线。与
//! `session_icons.rs` / `menu_tools.rs` 同一原则：**能画就不排字**。
//!
//! # 窗口三态的几何（逐像素量自视觉基线，2×→逻辑点）
//!
//! ```text
//! 盒子      10.5 × 12.5    不是正方形：右上角挂符号，整坨墨迹因此偏高偏宽
//! 上边      在 72% 处收笔   右上角整个让给符号
//! 右/左/下  通高 / 通高 / 通宽
//! 三角切角  0.8            这个尺寸上斜切与圆角分不出来，一条折线就够
//! 符号圆心  正压在盒子右上角，臂长 ±2.5
//! ```
//!
//! 关键一条：**右边是通高的**。第一版把右边也截了一段（以为要给符号让位），
//! 结果三个图标都像「撕了一角的方框」；视觉基线里符号是压在边上，不是把边断开。

use crate::ui::*;
use term_core::theme::CHROME_SCALE;

use super::tool_bar::PANEL_DX;

/// 抓手比图标暗一档：视觉基线采样 `#6C6C6C`，而图标是 `#A1A1A1`。
/// 抓手是「装饰」不是「按钮」，同色会让它看起来可以点。
const GRIP_INK: u8 = 0x6C;

/// 右上角符号的臂长（视觉基线 ±2.5）。
pub(super) const BADGE_ARM: f32 = 2.5;

// ── 面板段内部：相对面板左缘 ──
const INFO_CX: f32 = 15.25;
const INFO_SIZE: f32 = 15.5;
const SEP_X: f32 = 32.5;
const SEP_H: f32 = 12.0;
const CRUMB_X0: f32 = 45.5;
const ARROW_W: f32 = 4.0;
const ARROW_H: f32 = 4.0;
/// 箭头 → 文字的间隙（视觉基线 15.5）。
const ARROW_GAP: f32 = 15.5;
/// 文字 → 下一个箭头的间隙（视觉基线 14.5）。
const CRUMB_GAP: f32 = 14.5;

/// 把图标盒摆进点按区：**墨迹**（盒子 + 右上角溢出的符号）居中，盒子本身因此
/// 往左下各让半个溢出量。
pub(super) fn icon_in(slot: egui::Rect) -> egui::Rect {
    const W: f32 = 10.5;
    const H: f32 = 12.5;
    let ink = egui::vec2(W + BADGE_ARM, H + BADGE_ARM);
    egui::Rect::from_min_size(
        egui::pos2(
            slot.center().x - ink.x / 2.0,
            slot.center().y - ink.y / 2.0 + BADGE_ARM,
        ),
        egui::vec2(W, H),
    )
}

/// 拖动抓手：2 列 × 5 行小方块（视觉基线 2×2 点、横向 1 点间隙、纵向 1.75）。
pub(super) fn paint_grip(p: &egui::Painter, r: egui::Rect) {
    let c = egui::Color32::from_rgb(GRIP_INK, GRIP_INK, GRIP_INK);
    const DOT: f32 = 2.0;
    const GAP_X: f32 = 1.0;
    const GAP_Y: f32 = 1.75;
    for col in 0..2 {
        for row in 0..5 {
            let x = r.left() + col as f32 * (DOT + GAP_X);
            let y = r.top() + row as f32 * (DOT + GAP_Y);
            p.rect_filled(
                egui::Rect::from_min_size(egui::pos2(x, y), egui::vec2(DOT, DOT)),
                0.0,
                c,
            );
        }
    }
}

/// 带缺口的窗口轮廓（见模块头的几何表）。
fn window_path(p: &egui::Painter, r: egui::Rect, c: egui::Color32) {
    const K: f32 = 0.8;
    let (l, t, rt, b) = (r.left(), r.top(), r.right(), r.bottom());
    p.add(egui::Shape::line(
        vec![
            egui::pos2(rt, t + K),
            egui::pos2(rt, b - K),
            egui::pos2(rt - K, b),
            egui::pos2(l + K, b),
            egui::pos2(l, b - K),
            egui::pos2(l, t + K),
            egui::pos2(l + K, t),
            egui::pos2(l + 0.72 * r.width(), t),
        ],
        egui::Stroke::new(1.0, c),
    ));
}

/// 符号圆心：正压在窗口轮廓的右上角。
fn badge_center(r: egui::Rect) -> egui::Pos2 {
    egui::pos2(r.right(), r.top())
}

/// 窗口 + 加号：新会话。
pub(super) fn paint_window_plus(p: &egui::Painter, r: egui::Rect, c: egui::Color32) {
    window_path(p, r, c);
    let st = egui::Stroke::new(1.0, c);
    let k = badge_center(r);
    let a = BADGE_ARM;
    p.line_segment([egui::pos2(k.x - a, k.y), egui::pos2(k.x + a, k.y)], st);
    p.line_segment([egui::pos2(k.x, k.y - a), egui::pos2(k.x, k.y + a)], st);
}

/// 窗口 + 叉：断开。
pub(super) fn paint_window_cross(p: &egui::Painter, r: egui::Rect, c: egui::Color32) {
    window_path(p, r, c);
    let st = egui::Stroke::new(1.0, c);
    let k = badge_center(r);
    let a = BADGE_ARM - 0.5;
    p.line_segment(
        [egui::pos2(k.x - a, k.y - a), egui::pos2(k.x + a, k.y + a)],
        st,
    );
    p.line_segment(
        [egui::pos2(k.x - a, k.y + a), egui::pos2(k.x + a, k.y - a)],
        st,
    );
}

/// 窗口 + 圆圈：连接。
pub(super) fn paint_window_ring(p: &egui::Painter, r: egui::Rect, c: egui::Color32) {
    window_path(p, r, c);
    p.circle_stroke(badge_center(r), BADGE_ARM, egui::Stroke::new(1.0, c));
}

/// 下拉三角（实心，朝下）。
pub(super) fn paint_caret(p: &egui::Painter, r: egui::Rect, c: egui::Color32) {
    p.add(egui::Shape::convex_polygon(
        vec![
            egui::pos2(r.left(), r.top()),
            egui::pos2(r.right(), r.top()),
            egui::pos2(r.center().x, r.bottom()),
        ],
        c,
        egui::Stroke::NONE,
    ));
}

/// 面包屑之间的三角（实心，朝右）。
pub(super) fn paint_crumb_arrow(p: &egui::Painter, r: egui::Rect, c: egui::Color32) {
    p.add(egui::Shape::convex_polygon(
        vec![
            egui::pos2(r.left(), r.top()),
            egui::pos2(r.right(), r.center().y),
            egui::pos2(r.left(), r.bottom()),
        ],
        c,
        egui::Stroke::NONE,
    ));
}

/// 圆圈 + 小写 i。
pub(super) fn paint_info(p: &egui::Painter, r: egui::Rect, c: egui::Color32) {
    p.circle_stroke(r.center(), 0.46 * r.width(), egui::Stroke::new(1.0, c));
    let cx = r.center().x;
    p.rect_filled(
        egui::Rect::from_center_size(
            egui::pos2(cx, r.center().y - 0.20 * r.height()),
            egui::vec2(1.4, 1.4),
        ),
        0.0,
        c,
    );
    p.line_segment(
        [
            egui::pos2(cx, r.center().y - 0.02 * r.height()),
            egui::pos2(cx, r.center().y + 0.24 * r.height()),
        ],
        egui::Stroke::new(1.4, c),
    );
}

// ── 底色 ──────────────────────────────────────────────────────────────────

/// 整条的底：左段（抓手 + 按钮组）读 `dialog`，读起来就像「按钮坐在窗格上」；
/// 右段由 [`paint_panel`] 铺 `tab`。两段只是同一块不同深浅，不是两个控件。
pub(super) fn paint_strip(p: &egui::Painter, rect: egui::Rect) {
    let split = rect.left() + PANEL_DX;
    p.rect_filled(
        egui::Rect::from_min_max(rect.min, egui::pos2(split, rect.bottom())),
        0.0,
        crate::theme::chrome(CHROME_SCALE.dialog),
    );
}

/// 面板段：`#333333` 底 + (i) + 竖线 + 面包屑。
pub(super) fn paint_panel(p: &egui::Painter, rect: egui::Rect, crumbs: &[String], font: &egui::FontId) {
    p.rect_filled(rect, 0.0, crate::theme::chrome(CHROME_SCALE.tab));
    let icon_c = chrome_icon();
    let text_c = crate::theme::chrome(CHROME_SCALE.text);

    let info = egui::Rect::from_center_size(
        egui::pos2(rect.left() + INFO_CX, rect.center().y),
        egui::vec2(INFO_SIZE, INFO_SIZE),
    );
    paint_info(p, info, icon_c);

    let sx = rect.left() + SEP_X;
    p.line_segment(
        [
            egui::pos2(sx, rect.center().y - SEP_H / 2.0),
            egui::pos2(sx, rect.center().y + SEP_H / 2.0),
        ],
        egui::Stroke::new(1.0, crate::theme::chrome(CHROME_SCALE.border)),
    );

    // 面包屑：`▸ 分组 ▸ 会话`。箭头**每段都有**（含第一段），视觉基线是这么画的。
    let mut x = rect.left() + CRUMB_X0;
    for part in crumbs {
        paint_crumb_arrow(
            p,
            egui::Rect::from_center_size(
                egui::pos2(x + ARROW_W / 2.0, rect.center().y + 1.0),
                egui::vec2(ARROW_W, ARROW_H),
            ),
            text_c,
        );
        x += ARROW_W + ARROW_GAP;
        let g = p.layout_no_wrap(part.clone(), font.clone(), text_c);
        p.galley(
            egui::pos2(x, rect.center().y - g.size().y / 2.0),
            g.clone(),
            text_c,
        );
        x += g.size().x + CRUMB_GAP;
    }
}

pub(super) fn chrome_icon() -> egui::Color32 {
    crate::theme::chrome(CHROME_SCALE.icon)
}

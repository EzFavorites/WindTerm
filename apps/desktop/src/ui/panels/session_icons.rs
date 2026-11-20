//! 会话树的低层图标原语：协议终端窗 / 文件夹 / 折叠箭头。
//!
//! R9：从 `sidebar_view.rs` 拆出（2026-11-07，越过 450 行上限）。这里只放
//! **画形状**的纯函数，布局与交互仍在上层。全部用绘图原语，不依赖字体字形。
//!
//! # 为什么连齿轮都手画
//!
//! 视觉基线标题带右边是「齿轮 + 叉」，齿轮是 8 齿的环形。用 `⚙` 字符的话，夹具
//! 字体（egui 自带的 Ubuntu-Light / NotoEmoji）里有没有这个字形不可控，缺字
//! 会画成豆腐块，逐字节基线就会在不同机器上给出不同的图。所以这里和 P9 的
//! `quick_mark` 同一原则：**能画就不排字**。
//!
//! # 齿轮与 `×` 已搬走（2026-11-17）
//!
//! 原来这两个函数住在这个文件里，但同一个窗口里还有另外四份 `×` 与两份 `⚙`
//! （标签条 / 快捷栏 / 底带 / Find 栏），各自量出来差半个点到一点。现在它们
//! 统一到 `crate::ui::shell::glyph`，本文件只留协议图标 / 文件夹 / 折叠箭头。

use crate::ui::*;
use term_core::theme::SESSION_TREE;

/// 分组文件夹的宽度（实测 2× 图上 28px，换算成逻辑点）。
pub(super) const FOLDER_W: f32 = 14.0;
/// 分组文件夹的高度（实测 2× 图上 24px）。
pub(super) const FOLDER_H: f32 = 12.0;

/// 协议 → 图标底色（`SESSION_TREE` 调色板）。
pub(super) fn protocol_color(protocol: Protocol) -> u32 {
    match protocol {
        Protocol::Ssh => SESSION_TREE.ssh,
        Protocol::Telnet => SESSION_TREE.telnet,
        Protocol::Tcp => SESSION_TREE.tcp,
        Protocol::Serial => SESSION_TREE.serial,
        Protocol::Shell => SESSION_TREE.shell,
    }
}

/// 彩色终端窗图标：圆角方底 + 顶部暗标题条 + 白色提示符 `>_`（全用几何画）。
pub(super) fn paint_protocol_icon(
    p: &egui::Painter,
    center: egui::Pos2,
    r: f32,
    protocol: Protocol,
) {
    let color = crate::theme::chrome(protocol_color(protocol));
    let rect = egui::Rect::from_center_size(center, egui::vec2(2.0 * r, 2.0 * r));
    p.rect_filled(rect, 2.5, color);
    p.rect_filled(
        egui::Rect::from_min_size(
            egui::pos2(rect.left(), rect.top()),
            egui::vec2(rect.width(), 0.36 * r),
        ),
        2.5,
        color.gamma_multiply(0.6),
    );
    let white = egui::Color32::from_rgb(0xF0, 0xF0, 0xF0);
    let st = egui::Stroke::new(1.1, white);
    let cx = center.x - 0.22 * r;
    let cy = center.y + 0.18 * r;
    // `>`
    p.line_segment(
        [
            egui::pos2(cx - 0.28 * r, cy - 0.30 * r),
            egui::pos2(cx + 0.12 * r, cy),
        ],
        st,
    );
    p.line_segment(
        [
            egui::pos2(cx + 0.12 * r, cy),
            egui::pos2(cx - 0.28 * r, cy + 0.30 * r),
        ],
        st,
    );
    // `_`
    p.line_segment(
        [
            egui::pos2(cx + 0.34 * r, cy + 0.30 * r),
            egui::pos2(cx + 0.80 * r, cy + 0.30 * r),
        ],
        st,
    );
}

/// 青色渐变文件夹：顶部一条深蓝背板 + 主体自上而下由亮青渐变到青蓝。
///
/// 实测剖面（2× 图，x=50 竖切）：`y+1..y+5` 恒为 `#00AAE1`（背板），
/// `y+6` 起从 `#61D7FB` 一路压到 `#00B6E7`（主体），最底两行再暗一点（描边感）。
/// egui 的 `rect_filled` 只有纯色填充，所以主体用 8 条等高色带近似渐变——
/// 12px 高的图标上肉眼分不出是连续的，但像素基线是确定的。
pub(super) fn paint_folder(p: &egui::Painter, center: egui::Pos2, w: f32, h: f32) {
    let left = center.x - w * 0.5;
    let top = center.y - h * 0.5;
    // 背板占 0.21h（12px 高的图标上约 2.5px），主体从**背板下沿**起画。
    // 这两个数不能重叠：主体的亮青一旦压到背板上，背板就整块看不见了，
    // 文件夹会退化成「一块蓝方块」——初版就是这个错。
    let flap_h = 0.21 * h;
    p.rect_filled(
        egui::Rect::from_min_size(egui::pos2(left, top), egui::vec2(w, flap_h)),
        1.0,
        crate::theme::chrome(SESSION_TREE.folder_flap),
    );
    const BANDS: usize = 6;
    let body_h = h - flap_h;
    let band_h = body_h / BANDS as f32;
    let light = crate::theme::chrome(SESSION_TREE.folder_light);
    let deep = crate::theme::chrome(SESSION_TREE.folder_deep);
    for i in 0..BANDS {
        let t = i as f32 / (BANDS - 1) as f32;
        p.rect_filled(
            egui::Rect::from_min_size(
                egui::pos2(left, top + flap_h + band_h * i as f32),
                egui::vec2(w, band_h + 0.5),
            ),
            if i + 1 == BANDS { 1.4 } else { 0.0 },
            lerp_color(light, deep, t),
        );
    }
}

/// 两个颜色按 `t`（0..=1）线性插值。只给渐变用，不走 gamma 校正——
/// 8 条色带每个只占 1.2px，误差在抗锯齿噪声以内。
fn lerp_color(a: egui::Color32, b: egui::Color32, t: f32) -> egui::Color32 {
    let f = |x: u8, y: u8| (x as f32 + (y as f32 - x as f32) * t).round() as u8;
    egui::Color32::from_rgb(f(a.r(), b.r()), f(a.g(), b.g()), f(a.b(), b.b()))
}

/// 折叠箭头：展开向下、收起向右。**实心三角**（原图是填充三角，不是折线）。
pub(super) fn paint_chevron(
    p: &egui::Painter,
    center: egui::Pos2,
    open: bool,
    color: egui::Color32,
) {
    let half_h = 3.75;
    let w = 3.5;
    let pts = if open {
        vec![
            egui::pos2(center.x - half_h, center.y - w * 0.5),
            egui::pos2(center.x + half_h, center.y - w * 0.5),
            egui::pos2(center.x, center.y + w * 0.5),
        ]
    } else {
        vec![
            egui::pos2(center.x - w * 0.5, center.y - half_h),
            egui::pos2(center.x + w * 0.5, center.y),
            egui::pos2(center.x - w * 0.5, center.y + half_h),
        ]
    };
    p.add(egui::Shape::convex_polygon(pts, color, egui::Stroke::NONE));
}

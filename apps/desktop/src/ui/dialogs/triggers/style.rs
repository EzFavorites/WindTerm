//! 样式样例绘制：规则表 `Style` 列的预览、线型下拉里的示范线。
//!
//! 视觉基线 `01` / `13` 的规则表是**空表**，没有任何数据行，所以「Style 列的行内容
//! 长什么样」没有像素依据。这里按列的语义画「本色块 + 一条该线型的样例线」，三个
//! 颜色维度（text / background / line）与 `line_style` 都能一眼看出来，改动后也能
//! 在表里立刻看见效果——兼容终端从 `Modify` 进 [`super::popups`] 改的就是这三项。

use super::fields::{mono, rgb};
use crate::ui::view::trigger_spec::color as c;
use term_core::trigger::{ColorChoice, LineStyle, MarkConfig};

/// `ColorChoice` → RGB（`Empty` = 不设色，返回 `None`）。
pub(super) fn color_of(choice: &ColorChoice) -> Option<u32> {
    match choice {
        ColorChoice::Empty => None,
        ColorChoice::Ansi(name) => c::ANSI_LABELS
            .iter()
            .position(|l| l == name)
            .map(|i| c::ANSI[i]),
        ColorChoice::Custom(hex) => u32::from_str_radix(hex.trim_start_matches('#'), 16).ok(),
    }
}

/// 规则表 `Style` 列的单元格：底色块（背景色）+ `Aa`（文字色）+ 线型样例。
pub(super) fn cell_sample(p: &egui::Painter, rect: egui::Rect, mark: &MarkConfig) {
    let fg = color_of(&mark.text_color).map(rgb).unwrap_or(rgb(c::TEXT_2));
    let chip = egui::Rect::from_min_size(
        egui::pos2(rect.left() + 4.0, rect.center().y - 7.0),
        egui::vec2(20.0, 14.0),
    );
    match color_of(&mark.background_color) {
        Some(hex) => {
            p.rect_filled(chip, egui::Rounding::ZERO, rgb(hex));
        }
        None => {
            p.rect_stroke(
                chip,
                egui::Rounding::ZERO,
                egui::Stroke::new(0.5, rgb(0x4a4a4a)),
            );
        }
    }
    p.text(chip.center(), egui::Align2::CENTER_CENTER, "Aa", mono(crate::ui::type_scale::SIZE_MICRO), fg);
    // 线型样例：色块右侧到单元格右缘。
    let x0 = chip.right() + 6.0;
    let x1 = rect.right() - 6.0;
    if x1 - x0 > 8.0 {
        let col = color_of(&mark.line_color).map(rgb).unwrap_or(fg);
        line(p, x0, x1, rect.center().y, mark.line_style, col);
    }
}

/// 在 `y` 高度画一条 `x0..x1` 的 `style` 样例线。
pub(super) fn line(
    p: &egui::Painter,
    x0: f32,
    x1: f32,
    y: f32,
    style: LineStyle,
    col: egui::Color32,
) {
    // 虚线段 = (长度, 是否落笔)。
    const DOT: &[(f32, bool)] = &[(1.0, true), (3.0, false)];
    const DASH: &[(f32, bool)] = &[(6.0, true), (3.0, false)];
    const DASH_DOT: &[(f32, bool)] = &[(6.0, true), (3.0, false), (1.0, true), (3.0, false)];
    const DASH_DOT_DOT: &[(f32, bool)] = &[
        (6.0, true),
        (3.0, false),
        (1.0, true),
        (3.0, false),
        (1.0, true),
        (3.0, false),
    ];
    match style {
        LineStyle::None => {}
        LineStyle::Solid | LineStyle::Strike => seg(p, x0, x1, y, 1.0, col),
        LineStyle::Double => {
            seg(p, x0, x1, y - 1.5, 1.0, col);
            seg(p, x0, x1, y + 1.5, 1.0, col);
        }
        LineStyle::Thick => seg(p, x0, x1, y, 2.6, col),
        LineStyle::Wavy => {
            // 三角波，每 4 点换向，幅度 1.5。
            let mut pts = Vec::new();
            let mut x = x0;
            let mut up = true;
            while x <= x1 {
                pts.push(egui::pos2(x, if up { y - 1.5 } else { y + 1.5 }));
                x += 4.0;
                up = !up;
            }
            p.add(egui::Shape::line(pts, egui::Stroke::new(1.0, col)));
        }
        LineStyle::Dot => patterned(p, x0, x1, y, 1.6, col, DOT),
        LineStyle::Dash => patterned(p, x0, x1, y, 1.0, col, DASH),
        LineStyle::DashDot => patterned(p, x0, x1, y, 1.0, col, DASH_DOT),
        LineStyle::DashDotDot => patterned(p, x0, x1, y, 1.0, col, DASH_DOT_DOT),
        LineStyle::Box | LineStyle::RoundBox => {
            let r = frame(x0, x1, y);
            let round = if style == LineStyle::RoundBox {
                egui::Rounding::same(3.0)
            } else {
                egui::Rounding::ZERO
            };
            p.rect_stroke(r, round, egui::Stroke::new(1.0, col));
        }
        LineStyle::DashedBox => frame_edges(p, x0, x1, y, 1.0, col, DASH),
        LineStyle::DottedBox => frame_edges(p, x0, x1, y, 1.6, col, DOT),
    }
}

fn seg(p: &egui::Painter, x0: f32, x1: f32, y: f32, w: f32, col: egui::Color32) {
    p.line_segment(
        [egui::pos2(x0, y), egui::pos2(x1, y)],
        egui::Stroke::new(w, col),
    );
}

/// 沿 `x0..x1` 按 `pat` 落笔。
fn patterned(
    p: &egui::Painter,
    x0: f32,
    x1: f32,
    y: f32,
    w: f32,
    col: egui::Color32,
    pat: &[(f32, bool)],
) {
    let mut x = x0;
    let mut i = 0;
    while x < x1 {
        let (len, on) = pat[i % pat.len()];
        let b = (x + len).min(x1);
        if on {
            seg(p, x, b, y, w, col);
        }
        x = b;
        i += 1;
    }
}

/// 方框样例的矩形（高 10 点）。
fn frame(x0: f32, x1: f32, y: f32) -> egui::Rect {
    egui::Rect::from_min_max(egui::pos2(x0, y - 5.0), egui::pos2(x1, y + 5.0))
}

/// 四条边都按虚线 / 点线画（egui 的 stroke 不支持 dash）。
fn frame_edges(
    p: &egui::Painter,
    x0: f32,
    x1: f32,
    y: f32,
    w: f32,
    col: egui::Color32,
    pat: &[(f32, bool)],
) {
    let r = frame(x0, x1, y);
    patterned(p, r.left(), r.right(), r.top(), w, col, pat);
    patterned(p, r.left(), r.right(), r.bottom(), w, col, pat);
    // 竖边只有 10 点高，按同一图案铺下来即可。
    for x in [r.left(), r.right()] {
        let mut yy = r.top();
        let mut i = 0;
        while yy < r.bottom() {
            let (len, on) = pat[i % pat.len()];
            let b = (yy + len).min(r.bottom());
            if on {
                p.line_segment(
                    [egui::pos2(x, yy), egui::pos2(x, b)],
                    egui::Stroke::new(w, col),
                );
            }
            yy = b;
            i += 1;
        }
    }
}

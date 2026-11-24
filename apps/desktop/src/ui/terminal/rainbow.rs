//! 彩虹括号：按嵌套深度给 `()[]{}` 的字自身上前景色。
//!
//! 与 `highlights::paint_row_floor` 的「字下淡色底」是两回事：这里改的是
//! `Cell.fg`，于是 `view::build_spans` 会按新颜色切段、`painter.text` 把括号
//! 字形本身画成彩色。配对高亮（`current_pair`）那层底可以并存，互不替代。
//!
//! 深度来自纯函数 `term_app::selection::bracket_depths`（可见窗口一屏口径）；
//! 本文件只做「深度 → 颜色」这一步，纯数据、不依赖 egui，可单测。

use term_core::grid::Colour;

/// 深度 → ANSI 色号。取 1..=6 的明亮色循环（跳过 0=黑与 7=灰），
/// 8 个深度一循环；第 8 层回到第 0 层的颜色，但相邻层仍不同色。
const DEPTH_COLORS: [u8; 6] = [1, 2, 3, 4, 5, 6];

/// 把 `(绝对行, 显示列, 深度)` 的括号表转成可直接改 cell fg 的查表键。
///
/// `usize::MAX` 深度是「不着色」哨兵（孤立 / 交叉不平衡括号），直接跳过。
pub(in crate::ui) fn depth_color(depth: usize) -> Colour {
    if depth == usize::MAX {
        Colour::Default
    } else {
        Colour::Indexed(DEPTH_COLORS[depth % DEPTH_COLORS.len()])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn depth_maps_to_distinct_adjacent_colors_and_skips_sentinel() {
        assert_eq!(depth_color(0), Colour::Indexed(1));
        assert_eq!(depth_color(1), Colour::Indexed(2));
        assert_ne!(depth_color(0), depth_color(1), "相邻层必须异色");
        assert_eq!(depth_color(6), depth_color(0), "8 层后循环");
        assert_eq!(depth_color(usize::MAX), Colour::Default, "不平衡括号不上色");
    }
}

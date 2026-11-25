//! 额外高亮的地板绘制：触发器命中、当前折叠块、光标词、括号对。
//!
//! 都在字形之下、ANSI 背景之前画；只铺淡色，不改字形、不进选区/复制。
//! 放在独立文件是为了让 `view.rs` 守住单文件行数门禁。

use eframe::egui::{Color32, Painter, Pos2, Rect, Vec2};
use term_core::grid::ScreenImage;

/// 一个视口行号 → 它在本帧事实窗口里的**绝对行**。
///
/// `session.rs` 记触发器高亮用的是 `view.range.start + row`，绘制侧必须用同一把
/// 尺子（过去只传 `line.row`，回看时窗口整体平移，两边口径分叉 → 高亮消失）。
pub(super) fn absolute_row(row: i64, range_start: i64) -> i64 {
    row + range_start
}

#[allow(clippy::too_many_arguments)]
pub(in crate::ui) fn paint_row_floor(
    painter: &Painter,
    image: &ScreenImage,
    origin: Pos2,
    y: f32,
    cell: Vec2,
    highlight_rows: &[i64],
    fold_rows: &[i64],
    word_cells: &[(i64, usize)],
    pair_cells: &[(i64, usize)],
    row: i64,
    seg_start: usize,
    seg_width: usize,
) {
    let soft_tint = |r: u8, g: u8, b: u8| Color32::from_rgba_unmultiplied(r, g, b, 36);
    // 软折时底色只铺本段宽度（不越到右侧留白）；关软折时 seg_width == image.cols。
    let w = (seg_width.min(image.cols.saturating_sub(seg_start)) as f32) * cell.x;
    let seg_origin_x = origin.x + (seg_start as f32) * cell.x;

    let seg_origin = Pos2::new(seg_origin_x, origin.y);
    // 触发器高亮表存的是绝对行：绘制时把 line.row 换成同一口径。
    if highlight_rows.contains(&absolute_row(row, image.range.start)) {
        painter.rect_filled(Rect::from_min_size(seg_origin, Vec2::new(w, cell.y)), 0.0, soft_tint(0x2E, 0x7C, 0xD2));
    }
    if fold_rows.contains(&row) {
        painter.rect_filled(Rect::from_min_size(seg_origin, Vec2::new(w, cell.y)), 0.0, soft_tint(0xD2, 0x8B, 0x00));
    }
    for (cell_row, col) in word_cells.iter().chain(pair_cells.iter()) {
        if *cell_row != row || *col < seg_start || *col >= seg_start + seg_width {
            continue;
        }
        painter.rect_filled(
            Rect::from_min_size(
                Pos2::new(origin.x + ((*col - seg_start) as f32) * cell.x, y),
                Vec2::new(cell.x, cell.y),
            ),
            0.0,
            soft_tint(0x8D, 0x4E, 0xCE),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn trigger_highlight_rows_and_paint_share_one_absolute_row_scale() {
        // 贴底时 range.start == 0，原来的写法与现在逐值相同（基线零变化）。
        assert_eq!(absolute_row(7, 0), 7);
        // 回看时窗口整体平移：记录侧 `view.range.start + row` 与绘制侧
        // `line.row + image.range.start` 必须是同一个函数，否则高亮消失。
        for (row, start) in [(3, -5), (-2, -8), (11, 0)] {
            let stored_in_session = start + row; // session.rs 的 self.view.range.start + row
            assert_eq!(absolute_row(row, start), stored_in_session);
        }
    }
}

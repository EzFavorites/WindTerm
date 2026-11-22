//! 额外高亮的地板绘制：触发器命中、当前折叠块、光标词、括号对。
//!
//! 都在字形之下、ANSI 背景之前画；只铺淡色，不改字形、不进选区/复制。
//! 放在独立文件是为了让 `view.rs` 守住单文件行数门禁。

use eframe::egui::{Color32, Painter, Pos2, Rect, Vec2};
use term_core::grid::ScreenImage;

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
) {
    let soft_tint = |r: u8, g: u8, b: u8| Color32::from_rgba_unmultiplied(r, g, b, 36);
    let w = (image.cols as f32) * cell.x;

    if highlight_rows.contains(&row) {
        painter.rect_filled(Rect::from_min_size(origin, Vec2::new(w, cell.y)), 0.0, soft_tint(0x2E, 0x7C, 0xD2));
    }
    if fold_rows.contains(&row) {
        painter.rect_filled(Rect::from_min_size(origin, Vec2::new(w, cell.y)), 0.0, soft_tint(0xD2, 0x8B, 0x00));
    }
    for (cell_row, col) in word_cells.iter().chain(pair_cells.iter()) {
        if *cell_row != row || *col >= image.cols {
            continue;
        }
        painter.rect_filled(
            Rect::from_min_size(
                Pos2::new(origin.x + *col as f32 * cell.x, y),
                Vec2::new(cell.x, cell.y),
            ),
            0.0,
            soft_tint(0x8D, 0x4E, 0xCE),
        );
    }
}

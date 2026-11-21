//! 标签缩略图：把一条标签当前屏的字形盒缩成一张小色块图。
//!
//! 这不是屏幕截图（那条路要抓渲染后的像素，夹具里拿不到），而是直接用
//! 事实窗口里每个 cell 的前景色画点：非空白格子一个小方块，空白格子不画。
//! 它足够回答「这条标签现在大概是什么内容」，而且完全在投影层、零 IO。

use eframe::egui;
use term_core::grid::{Cell, Colour, ScreenImage};
use term_core::theme::Palette;

fn colour(c: Colour, palette: &Palette) -> egui::Color32 {
    match c {
        Colour::Default => crate::theme::to_color32(palette.default_fg),
        Colour::Indexed(i) => crate::theme::to_color32(palette.ansi[(i as usize) & 15]),
        Colour::Rgb(r, g, b) => egui::Color32::from_rgb(r, g, b),
    }
}

fn visible(cell: &Cell) -> bool {
    !cell.ch.is_whitespace() && cell.ch != '\0'
}

/// 在标签下方画一张缩略图。`pct = 0` 表示关闭。
pub(in crate::ui) fn paint(
    ui: &egui::Ui,
    tab_rect: egui::Rect,
    image: &ScreenImage,
    palette: &Palette,
    pct: usize,
) {
    if pct == 0 || image.cols == 0 || image.lines.is_empty() {
        return;
    }
    let width = (tab_rect.width() * pct as f32 / 100.0).max(48.0);
    let cell = (width / image.cols as f32).max(1.0);
    let height = (cell * image.lines.len() as f32).min(160.0);
    let pos = egui::pos2(tab_rect.left(), tab_rect.bottom() + 2.0);
    let rect = egui::Rect::from_min_size(pos, egui::vec2(width, height));
    let area = egui::Area::new(egui::Id::new("tab_thumbnail"))
        .order(egui::Order::Tooltip)
        .fixed_pos(rect.min)
        .show(ui.ctx(), |ui| {
            let painter = ui.painter();
            painter.rect_filled(rect, 0.0, crate::theme::to_color32(palette.default_bg));
            for (row, line) in image.lines.iter().enumerate() {
                for (col, cell_ref) in line.cells.iter().enumerate() {
                    if !visible(cell_ref) {
                        continue;
                    }
                    let px = egui::Rect::from_min_size(
                        egui::pos2(rect.left() + col as f32 * cell, rect.top() + row as f32 * cell),
                        egui::vec2(cell.max(1.0), cell.max(1.0)),
                    );
                    painter.rect_filled(px, 0.0, colour(cell_ref.fg, palette));
                }
            }
        });
    let _ = area;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn whitespace_is_not_a_visible_mark() {
        assert!(!visible(&Cell::default()));
        assert!(visible(&Cell { ch: 'x', ..Cell::default() }));
    }
}

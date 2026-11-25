//! 叠层标记（空白符号 / 调试 pip / 列参考线）的**段内夹取**。
//!
//! 软折打开后一条逻辑行占多段：`marks_paint` 与 `debug_paint` 若仍按 `lines`
//! 下标算 y、按整行宽度画，标记会落到别的段上或越出该段区间。这里提供唯一的
//! 「内容列 → 段内相对列」判据，两个绘制模块共用，与正文 / 查找高亮同源。

use term_app::wrap::VisualRow;

/// 内容列 `col` 落在段 `seg` 内时换成段内相对列，否则 `None`（这一格不画）。
pub(in crate::ui) fn mark_col_in_segment(seg: &VisualRow, col: usize) -> Option<usize> {
    let end = seg.start_col.saturating_add(seg.width);
    if col >= seg.start_col && col < end {
        Some(col - seg.start_col)
    } else {
        None
    }
}

/// 本段占的 cell 区间（与正文 `from..to` 同一口径，右界夹到行宽）。
pub(in crate::ui) fn segment_cell_range(seg: &VisualRow, line_cols: usize) -> (usize, usize) {
    let to = seg.start_col.saturating_add(seg.width).min(line_cols);
    (seg.start_col.min(to), to)
}

#[cfg(test)]
mod tests {
    use super::*;
    use term_core::grid::{Cell, LineSlice, ScreenImage};

    #[test]
    fn a_mark_outside_a_segment_is_not_drawn_into_it() {
        let first = VisualRow { line: 0, start_col: 0, width: 10, ..VisualRow::default() };
        let second = VisualRow { line: 0, start_col: 10, width: 6, ..VisualRow::default() };
        assert_eq!(mark_col_in_segment(&first, 3), Some(3));
        assert_eq!(mark_col_in_segment(&first, 12), None, "第二段的列不许画进第一段");
        assert_eq!(mark_col_in_segment(&second, 12), Some(2));
        assert_eq!(mark_col_in_segment(&second, 3), None);
    }

    #[test]
    fn with_word_wrap_on_marks_stay_inside_the_segment_that_owns_them() {
        let mut image = ScreenImage::blank(30, 1, 0..1);
        image.lines[0] = LineSlice {
            row: 0,
            cells: (0..30).map(|i| Cell { ch: if i < 24 { 'x' } else { ' ' }, ..Cell::default() }).collect(),
        };
        let cfg = term_app::wrap::WrapConfig::new(
            true,
            term_app::wrap::WrapSettings { column: 10, ..term_app::wrap::WrapSettings::default() },
        );
        let table = term_app::wrap::segments_for_wrap(&image, cfg);
        assert!(table.rows.len() >= 3, "24 格按 10 列折至少三段");
        let (line_cols, mut hits) = (image.lines[0].cells.len(), 0usize);
        for (idx, seg) in table.rows.iter().enumerate() {
            let (from, to) = segment_cell_range(seg, line_cols);
            assert!(from <= to && to <= line_cols, "段 {idx} 越出行宽：{from}..{to}");
            for col in from..to {
                if let Some(rel) = mark_col_in_segment(seg, col) {
                    assert!(rel < seg.width.min(line_cols.saturating_sub(seg.start_col)));
                    hits += 1;
                }
            }
        }
        assert!(hits >= 24, "每一个有内容的格子都必须落在唯一一段里：{hits}");
    }
}

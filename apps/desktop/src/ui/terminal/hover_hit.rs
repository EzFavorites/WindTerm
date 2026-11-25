//! 悬停命中：把「文本里的字节 / 字符下标」换成**屏幕 cell 列**。
//!
//! `trigger_builtins::detect` 返回字节偏移，`trigger::match_in_line` 返回字符下标，
//! 而指针的 `col` 是 cell 列。中文 / 全角字形一个字占两格，三者直接混用会让前缀
//! 带中文的行上「IP 明明看得见、悬停却弹不出来」。唯一的换算表是 `glyph_columns`：
//! 第 i 个字形的起始 cell 下标，与查找 / 软折 / 正文渲染同一口径。

use term_core::grid::{glyph_columns, Cell};

/// 第 `col` 这个字形占几格（宽字符占字形格 + 占位格）。
fn glyph_width(cells: &[Cell], col: usize) -> usize {
    usize::from(cells.get(col).is_some_and(|cell| cell.wide)) + 1
}

/// 字符区间 `[from, to)` → cell 列区间 `[start, end)`。
///
/// `to == 字形数` 时用最后一个字形的宽度收尾（宽字符要 +2，不能只 +1）；
/// 落在表里没有的位置返回 `None`，宁可不弹也不弹到别的 token 上。
pub(in crate::ui) fn char_range_to_cols(
    cells: &[Cell],
    from_char: usize,
    to_char: usize,
) -> Option<(usize, usize)> {
    if to_char < from_char {
        return None;
    }
    let table = glyph_columns(cells);
    let start = *table.get(from_char)?;
    let end = match table.get(to_char) {
        Some(&col) => col,
        None if to_char == table.len() => {
            let last = *table.last()?;
            last + glyph_width(cells, last)
        }
        None => return None,
    };
    Some((start, end.max(start + 1)))
}

/// 字节区间 `[from, to)` → cell 列区间。调用方先确认边界落在 char 边界上。
pub(in crate::ui) fn byte_range_to_cols(
    text: &str,
    cells: &[Cell],
    from_byte: usize,
    to_byte: usize,
) -> Option<(usize, usize)> {
    if from_byte > to_byte
        || to_byte > text.len()
        || !text.is_char_boundary(from_byte)
        || !text.is_char_boundary(to_byte)
    {
        return None;
    }
    char_range_to_cols(
        cells,
        text[..from_byte].chars().count(),
        text[..to_byte].chars().count(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cells(text: &str) -> Vec<Cell> {
        let mut out = Vec::new();
        for ch in text.chars() {
            let wide = ('\u{4e00}'..='\u{9fff}').contains(&ch);
            out.push(Cell { ch, wide, ..Cell::default() });
            if wide {
                out.push(Cell::default());
            }
        }
        out
    }

    #[test]
    fn a_chinese_prefix_shifts_the_ip_to_its_real_cell_column() {
        let text = "中文 IP 10.0.0.9 tail";
        let row = cells(text);
        let found = term_app::trigger_builtins::detect(text)
            .into_iter()
            .find(|f| f.text == "10.0.0.9")
            .expect("夹具里必须有这个 IP");
        // 字节偏移 10、字符下标 6，但中文各占两格 → 屏幕列从 8 起。
        assert_eq!(found.start, 10);
        let (start, end) = byte_range_to_cols(text, &row, found.start, found.start + found.text.len())
            .expect("换算必须成功");
        // 8 个字符的 IPv4 在屏幕上占 8 格：8..16。
        assert_eq!((start, end), (8, 16));
        // 指针落在这八列里应命中，落在中国字上不命中。
        for col in 8..16 {
            assert!((start..end).contains(&col));
        }
        assert!(!(start..end).contains(&0));
    }

    #[test]
    fn a_chinese_prefix_shifts_a_hex_token_too() {
        let text = "前缀 0xFF rest";
        let row = cells(text);
        let found = term_app::trigger_builtins::detect(text)
            .into_iter()
            .find(|f| f.text == "0xFF")
            .expect("夹具里必须有这个 hex");
        let (start, end) = byte_range_to_cols(text, &row, found.start, found.start + found.text.len())
            .expect("换算必须成功");
        // 两个中文 = 4 格 + 空格 = 1 格 → 0xFF 在 5..9。
        assert_eq!((start, end), (5, 9));
    }

    #[test]
    fn a_wide_glyph_at_the_end_ends_two_columns_later() {
        let row = cells("AB中");
        assert_eq!(char_range_to_cols(&row, 2, 3), Some((2, 4)));
        assert_eq!(char_range_to_cols(&row, 0, 1), Some((0, 1)));
        assert_eq!(char_range_to_cols(&row, 3, 4), None);
    }

    #[test]
    fn an_off_boundary_byte_offset_is_rejected_instead_of_panicking() {
        let text = "中文 IP 10.0.0.9";
        let row = cells(text);
        assert_eq!(byte_range_to_cols(text, &row, 1, 4), None);
        assert_eq!(byte_range_to_cols(text, &row, 0, text.len() + 1), None);
    }
}

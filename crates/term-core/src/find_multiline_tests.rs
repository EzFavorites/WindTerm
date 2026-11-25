//! `find_matches_regex_multiline_in` 的坐标单测。
//!
//! 从 `find.rs` 拆出：`find.rs` 已有近 500 代码行，不能再往里堆。
//! 这里只测「含端点的 regex 偏移 → 半开的 Match 列区间」这一条边界，
//!  cases 覆盖单行多字符、宽字符末字、跨行首列结束、显式 `\n` 与字面版逐值比对。

use super::{find_matches_in, find_matches_regex_multiline_in, Match};
use crate::grid::{Cell, LineSlice, ScreenImage};

fn window(lines: &[&str], cols: usize) -> ScreenImage {
    let rows = lines.len();
    let lines: Vec<LineSlice> = lines
        .iter()
        .enumerate()
        .map(|(i, text)| {
            let mut cells = vec![Cell::default(); cols];
            let mut at = 0;
            for ch in text.chars() {
                let wide = crate::width::is_wide(ch);
                let width = if wide { 2 } else { 1 };
                if at + width > cols {
                    break;
                }
                cells[at] = Cell {
                    ch,
                    wide,
                    ..Cell::default()
                };
                at += width;
            }
            LineSlice {
                row: i as i64,
                cells,
            }
        })
        .collect();
    ScreenImage {
        cols,
        version: 1,
        range: 0..rows as i64,
        lines,
        cursor: None,
        history: 0,
    }
}

#[test]
fn single_line_multichar_regex_marks_every_character() {
    // 旧实现把 end_char 当半开端点：`[ab]` 在 "abc" 上只画出 'a' 一处，
    // 且那一处还落在 'b' 的列上（少一个匹配、错一列）。
    let img = window(&["abc"], 8);
    let hits = find_matches_regex_multiline_in(&img, "[ab]", true).unwrap();
    assert_eq!(hits, vec![Match::new(0, 0, 1), Match::new(0, 1, 2)]);
}

#[test]
fn multiline_regex_includes_a_trailing_wide_glyph() {
    let img = window(&["你好world"], 12);
    let hits = find_matches_regex_multiline_in(&img, "你好", true).unwrap();
    assert_eq!(hits, vec![Match::new(0, 0, 4)], "汉字各占两格，末字不能丢");
    let last = find_matches_regex_multiline_in(&img, "好", true).unwrap();
    assert_eq!(last, vec![Match::new(0, 2, 4)]);
    let whole = find_matches_regex_multiline_in(&img, "你好world", true).unwrap();
    assert_eq!(whole, vec![Match::new(0, 0, 9)]);
}

#[test]
fn multiline_regex_ending_on_next_lines_first_column_keeps_that_cell() {
    // `c\nd` 的末字是下一行第 0 列：+1 之前 `to == from`，整段直接消失。
    let img = window(&["abc", "def"], 8);
    let hits = find_matches_regex_multiline_in(&img, "c\\nd", true).unwrap();
    assert_eq!(hits, vec![Match::new(0, 2, 3), Match::new(1, 0, 1)]);
}

#[test]
fn explicit_newline_pattern_highlights_whole_lines_but_not_the_newline() {
    let img = window(&["abc", "def"], 8);
    let both = find_matches_regex_multiline_in(&img, "abc\\ndef", true).unwrap();
    assert_eq!(both, vec![Match::new(0, 0, 3), Match::new(1, 0, 3)]);
    // 命中以换行收尾：换行字节映射到 chars.len()+1，钳回行宽 → 整行高亮。
    let with_tail = find_matches_regex_multiline_in(&img, "c\\n", true).unwrap();
    assert_eq!(with_tail, vec![Match::new(0, 2, 3)]);
    // 只匹配换行本身：不产生任何格上的高亮。
    assert!(find_matches_regex_multiline_in(&img, "\\n", true)
        .unwrap()
        .is_empty());
}

#[test]
fn multiline_regex_agrees_with_the_literal_engine_value_by_value() {
    let img = window(&["你好world", "abWORLD"], 12);
    for pattern in ["a", "ab", "你好", "world", "好w"] {
        let regex = find_matches_regex_multiline_in(&img, pattern, true).unwrap();
        let literal = find_matches_in(&img, pattern, true, false);
        assert_eq!(regex, literal, "pattern={pattern:?}");
    }
    // 忽略大小写走与单行版同一个 builder 开关，结果也逐值相等。
    let regex = find_matches_regex_multiline_in(&img, "world", false).unwrap();
    assert_eq!(regex, find_matches_in(&img, "world", false, false));
    assert_eq!(regex, vec![Match::new(0, 4, 9), Match::new(1, 2, 7)]);
}

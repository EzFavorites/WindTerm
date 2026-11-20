//! B1 纯核心：在事实窗口里查找文本，零 UI 依赖，可单测。
//!
//! 在事实窗口上做增量子串查找，并提供匹配导航（上/下一个、计数）。
//!
//! F2：扫描层不再绑定 `GridSnapshot`。同一套引擎既扫当前屏（`find_matches`），
//! 也扫回看窗口（`find_matches_in`）——查找的输入从「整块快照」变成「你看得见的
//! 那一段事实」，两个入口只是喂给引擎的行序列不同。
//!
//! 行号统一是**内容绝对行号**：`0` = 当前屏第一行，负数 = 回看缓冲里的历史行。
//! 这与 `LineSlice.row`、`Selection` 的 `Pos.row` 是同一坐标系，投影拿到
//! `Match.row` 就能直接换算成「屏上第几行」，不需要再补一层偏移。

use crate::grid::{Cell, GridSnapshot, ScreenImage};

/// 一次匹配：内容绝对行号（负数 = 历史行）、起始列、结束列（不含）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Match {
    pub row: i64,
    pub col_start: usize,
    pub col_end: usize,
}

impl Match {
    pub fn new(row: i64, col_start: usize, col_end: usize) -> Self {
        Self {
            row,
            col_start,
            col_end,
        }
    }
}

/// 在当前屏（快照）中查找所有匹配项。
///
/// - `query` 为空返回空 Vec。
/// - `case_sensitive` 控制是否区分大小写。
/// - 逐行扫描；列号是单元格下标（宽字符的占位格也算一格），可直接用于画高亮。
/// - 返回按行号、列号升序排列的所有匹配；行号为 `0..rows`。
pub fn find_matches(snap: &GridSnapshot, query: &str, case_sensitive: bool) -> Vec<Match> {
    let query: Vec<char> = query.chars().collect();
    if query.is_empty() || snap.rows == 0 || snap.cols == 0 {
        return Vec::new();
    }

    let mut matches = Vec::new();
    for row in 0..snap.rows {
        if let Some(cells) = snap.row_cells(row) {
            // 快照入口是旧路径（只有测试与历史调用走它），不带整词：
            // 要整词的调用方走 `find_matches_in`。
            scan_line(row as i64, cells, &query, case_sensitive, false, &mut matches);
        }
    }
    matches
}

/// 在事实窗口（当前屏或回看缓冲）中查找所有匹配项。
///
/// 只扫窗口覆盖到的行——用户看到什么就搜什么。`Match.row` 直接是
/// `LineSlice.row`，回看时是负数。
///
/// `whole_word`（`W` 那颗按钮）要求命中的**两端**都落在词的边界上：
/// 前一个字不是词字符（行首也算），后一个字也不是词字符（行尾也算）。
/// 词字符按「字母 / 数字 / 下划线」判——这三样之外都当分隔符，
/// 所以 `foo-bar` 里搜 `foo` 在整词模式下仍然命中。
pub fn find_matches_in(
    image: &ScreenImage,
    query: &str,
    case_sensitive: bool,
    whole_word: bool,
) -> Vec<Match> {
    let query: Vec<char> = query.chars().collect();
    if query.is_empty() || image.cols == 0 {
        return Vec::new();
    }

    let mut matches = Vec::new();
    for line in &image.lines {
        scan_line(
            line.row,
            &line.cells,
            &query,
            case_sensitive,
            whole_word,
            &mut matches,
        );
    }
    matches
}

/// 在事实窗口中查找下一个匹配（相对于游标位置向后）。
///
/// 返回 (match_index_in_results, Match) 或 None。行号是内容绝对行号，
/// 所以 `cursor_row` 在回看时同样是负数。
///
/// 注意：投影层「下一个 / 上一个」用的是匹配集上的环形索引
/// （ADR-0015），不走这里——两者坐标系相同，但导航语义不同。
pub fn find_next(
    image: &ScreenImage,
    query: &str,
    case_sensitive: bool,
    whole_word: bool,
    cursor_row: i64,
    cursor_col: usize,
) -> Option<(usize, Match)> {
    let all = find_matches_in(image, query, case_sensitive, whole_word);
    if all.is_empty() {
        return None;
    }
    for (idx, m) in all.iter().enumerate() {
        if m.row > cursor_row || (m.row == cursor_row && m.col_start > cursor_col) {
            return Some((idx, *m));
        }
    }
    // 循环到第一个
    Some((0, all[0]))
}

/// 在事实窗口中查找上一个匹配（相对于游标位置向前）。
pub fn find_previous(
    image: &ScreenImage,
    query: &str,
    case_sensitive: bool,
    whole_word: bool,
    cursor_row: i64,
    cursor_col: usize,
) -> Option<(usize, Match)> {
    let all = find_matches_in(image, query, case_sensitive, whole_word);
    if all.is_empty() {
        return None;
    }
    for (idx, m) in all.iter().enumerate().rev() {
        if m.row < cursor_row || (m.row == cursor_row && m.col_end <= cursor_col) {
            return Some((idx, *m));
        }
    }
    // 循环到最后一个
    let last = all.len() - 1;
    Some((last, all[last]))
}

/// 单行扫描：快照与事实窗口共用的唯一实现。
///
/// `row` 是这一行的内容绝对行号，`cells` 是它的完整单元格。
///
/// 扫描单位是**字形**，不是格：宽字符占两格（起始格带字、第二格是占位空格），
/// 按格扫会让「你好」在中隔一个空格，`查找 "你好"` 永远命中不了。这里先把格
/// 折叠成 `(列号, 字符)` 序列，匹配在字符序列上做，命中后再用列号还原成
/// 「屏上从第几格到第几格」——高亮框与文本因此不会各说一套。
///
/// 行尾空白不参与匹配（终端一行的右半部分是填充）。
fn scan_line(
    row: i64,
    cells: &[Cell],
    query: &[char],
    case_sensitive: bool,
    whole_word: bool,
    out: &mut Vec<Match>,
) {
    // 字形序列：`cols[i]` 是第 i 个字形的起始列，`chars[i]` 是它的字符。
    let mut cols: Vec<usize> = Vec::with_capacity(cells.len());
    let mut chars: Vec<char> = Vec::with_capacity(cells.len());
    for (col, cell) in cells.iter().enumerate() {
        if col > 0 && cells[col - 1].wide {
            continue; // 宽字符的延续格：没有字形
        }
        cols.push(col);
        chars.push(cell.ch);
    }
    while matches!(chars.last(), Some(c) if c.is_whitespace()) {
        chars.pop();
        cols.pop();
    }
    if chars.len() < query.len() {
        return;
    }

    let mut at = 0;
    while at + query.len() <= chars.len() {
        let hit = (0..query.len()).all(|i| same_char(query[i], chars[at + i], case_sensitive))
            && (!whole_word || on_word_boundaries(&chars, at, query.len()));
        if hit {
            let col_start = cols[at];
            let last = at + query.len() - 1;
            let col_end = cols[last] + if cells[cols[last]].wide { 2 } else { 1 };
            out.push(Match::new(row, col_start, col_end));
            at += query.len(); // 允许重叠匹配时可改为 at += 1
        } else {
            at += 1;
        }
    }
}

/// `at..at+len` 这一段是否两端都贴着词边界（行首 / 行尾也算边界）。
///
/// 只看**紧邻**的两个字符：`foo` 在 `foobar` 里不算整词（后面是 `b`），
/// 在 `foo_bar` 里也不算（`_` 是词字符），在 `foo.bar` 里算。
fn on_word_boundaries(chars: &[char], at: usize, len: usize) -> bool {
    let before_ok = at == 0 || !is_word_char(chars[at - 1]);
    let after_ok = at + len >= chars.len() || !is_word_char(chars[at + len]);
    before_ok && after_ok
}

/// 词字符：字母 / 数字 / 下划线。其余（空格、标点、制表）都当分隔符。
fn is_word_char(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

fn same_char(a: char, b: char, case_sensitive: bool) -> bool {
    if case_sensitive {
        a == b
    } else {
        a.eq_ignore_ascii_case(&b)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::grid::{Attrs, Cell, Colour, GridSnapshot, LineSlice, ScreenImage};

    fn make_snap(text: &str) -> GridSnapshot {
        let lines: Vec<&str> = text.lines().collect();
        let rows = lines.len();
        let cols = lines.iter().map(|l| l.chars().count()).max().unwrap_or(0);
        let mut cells = vec![Cell::default(); rows * cols];
        for (r, line) in lines.iter().enumerate() {
            for (c, ch) in line.chars().enumerate() {
                cells[r * cols + c] = Cell {
                    ch,
                    fg: Colour::Default,
                    bg: Colour::Default,
                    attrs: Attrs::EMPTY,
                    wide: false,
                };
            }
        }
        GridSnapshot {
            cols,
            rows,
            cells,
            cursor: (0, 0),
            cursor_visible: true,
            title: String::new(),
            version: 1,
            bytes_received: 0,
            application_cursor_keys: false,
            bracketed_paste: false,
            mouse_reporting: false,
            mouse_mode: crate::mouse_report::MouseMode::Off,
            mouse_encoding: crate::mouse_report::MouseEncoding::X10,
            alt: false,
            history: 0,
            blocks: Vec::new(),
            row_times: Vec::new(),
            wraps: Vec::new(),
        }
    }

    /// 贴底窗口：与 `find_matches` 的输入完全等价。
    fn window(text: &str) -> ScreenImage {
        make_snap(text).to_screen_image()
    }

    /// 带历史的窗口：行号从 `start_row` 起，负数即回看行。
    ///
    /// 宽字符按内核的写法铺格子：字形占一格（`wide = true`），
    /// 后面跟一格无字形的占位——否则网格列号会算错。
    fn window_from(lines: &[&str], cols: usize, start_row: i64) -> ScreenImage {
        let lines: Vec<LineSlice> = lines
            .iter()
            .enumerate()
            .map(|(i, text)| {
                let mut cells = vec![Cell::default(); cols];
                let mut at = 0;
                for ch in text.chars() {
                    let wide = crate::width::is_wide(ch);
                    if at + if wide { 2 } else { 1 } > cols {
                        break;
                    }
                    cells[at] = Cell {
                        ch,
                        wide,
                        ..Cell::default()
                    };
                    at += if wide { 2 } else { 1 };
                }
                LineSlice {
                    row: start_row + i as i64,
                    cells,
                }
            })
            .collect();
        ScreenImage {
            cols,
            version: 1,
            range: start_row..start_row + lines.len() as i64,
            lines,
            cursor: None,
            history: start_row.unsigned_abs() as usize,
        }
    }

    #[test]
    fn find_matches_across_wide_glyphs_uses_glyph_columns() {
        // 网格：你(0, 两格) 好(2, 两格) world(4..9)。
        let img = window_from(&["你好world"], 12, 0);
        assert_eq!(
            find_matches_in(&img, "好", true, false),
            vec![Match::new(0, 2, 4)],
            "一个汉字占两格，高亮要盖住两格"
        );
        assert_eq!(find_matches_in(&img, "你好", true, false), vec![Match::new(0, 0, 4)]);
        assert_eq!(find_matches_in(&img, "world", true, false), vec![Match::new(0, 4, 9)]);
        // 搜索串本身也按字形比较：中间不能多出一个空格。
        assert!(find_matches_in(&img, "你 好", true, false).is_empty());
    }

    #[test]
    fn find_matches_empty_query() {
        let snap = make_snap("hello world");
        assert!(find_matches(&snap, "", true).is_empty());
        assert!(find_matches(&snap, "", false).is_empty());
    }

    #[test]
    fn find_matches_case_sensitive() {
        let snap = make_snap("Hello World\nhello world");
        let m = find_matches(&snap, "Hello", true);
        assert_eq!(m.len(), 1);
        assert_eq!(m[0], Match::new(0, 0, 5));
        let m2 = find_matches(&snap, "hello", true);
        assert_eq!(m2.len(), 1);
        assert_eq!(m2[0], Match::new(1, 0, 5));
    }

    #[test]
    fn find_matches_case_insensitive() {
        let snap = make_snap("Hello World\nhello world");
        let m = find_matches(&snap, "hello", false);
        assert_eq!(m.len(), 2);
        assert_eq!(m[0], Match::new(0, 0, 5));
        assert_eq!(m[1], Match::new(1, 0, 5));
    }

    #[test]
    fn find_matches_multiple_per_line() {
        let snap = make_snap("foo bar foo baz");
        let m = find_matches(&snap, "foo", false);
        assert_eq!(m.len(), 2);
        assert_eq!(m[0].col_start, 0);
        assert_eq!(m[1].col_start, 8);
    }

    #[test]
    fn find_next_wraps() {
        let win = window("one\ntwo\nthree");
        let m = find_matches_in(&win, "o", false, false);
        assert_eq!(m.len(), 2); // one, two (three has no 'o')
        let next = find_next(&win, "o", false, false, 0, 0);
        assert!(next.is_some());
        let (idx, match_) = next.unwrap();
        assert_eq!(idx, 1); // 跳过当前位置的匹配，找到下一个（row=1）
        assert_eq!(match_.row, 1);
    }

    #[test]
    fn find_previous_wraps() {
        let win = window("one\ntwo\nthree");
        let m = find_matches_in(&win, "o", false, false);
        assert_eq!(m.len(), 2); // one, two
        let prev = find_previous(&win, "o", false, false, 2, 5);
        assert!(prev.is_some());
        let (idx, match_) = prev.unwrap();
        assert_eq!(idx, 1); // 最后一个匹配是 row=1 (two)
        assert_eq!(match_.row, 1);
    }

    #[test]
    fn find_matches_multiline() {
        let snap = make_snap("line1\nline2\nline3");
        let m = find_matches(&snap, "line", false);
        assert_eq!(m.len(), 3);
        assert_eq!(m[0].row, 0);
        assert_eq!(m[1].row, 1);
        assert_eq!(m[2].row, 2);
    }

    #[test]
    fn find_next_from_middle() {
        let win = window("a b c d e");
        let m = find_matches_in(&win, " ", false, false);
        assert_eq!(m.len(), 4); // 4 spaces at col 1, 3, 5, 7
        let next = find_next(&win, " ", false, false, 0, 4);
        assert!(next.is_some());
        let (idx, match_) = next.unwrap();
        assert_eq!(idx, 2); // 第 3 个空格（索引 2）
        assert_eq!(match_.col_start, 5);
    }

    // ── F2：同一套引擎，两种窗口 ──────────────────────────────

    #[test]
    fn window_engine_agrees_with_the_snapshot_engine() {
        // 贴底时事实窗口就是从快照派生的，两者结果必须逐项相同——
        // 否则「贴底/回看」会得到两套搜索结果。
        let snap = make_snap("alpha beta\nGamma alpha\nalpha");
        let win = snap.to_screen_image();
        for case_sensitive in [false, true] {
            for query in ["alpha", "Alpha", "a", "", "zzz"] {
                assert_eq!(
                    find_matches(&snap, query, case_sensitive),
                    find_matches_in(&win, query, case_sensitive, false),
                    "query={query:?} case_sensitive={case_sensitive}"
                );
            }
        }
    }

    #[test]
    fn history_rows_keep_their_negative_numbers() {
        // 回看窗口的第一行是历史行（-2），匹配必须报负数行号，
        // 投影才能直接换算成「屏上第几行」。
        let win = window_from(&["needle one", "no match", "needle two"], 16, -2);
        let m = find_matches_in(&win, "needle", false, false);
        assert_eq!(m.len(), 2);
        assert_eq!(m[0].row, -2);
        assert_eq!(m[1].row, 0);
        assert_eq!((m[0].col_start, m[0].col_end), (0, 6));
    }

    #[test]
    fn engine_scans_only_the_rows_the_window_covers() {
        // 窗口之外的行不参与：这是「看到什么搜什么」的直接体现。
        let win = window_from(&["needle here"], 16, -1);
        assert_eq!(find_matches_in(&win, "needle", false, false).len(), 1);
        assert!(find_matches_in(&win, "absent", false, false).is_empty());
        // 换成只覆盖下一行的窗口，同一段文本就搜不到了。
        let other = window_from(&["nothing"], 16, 0);
        assert!(find_matches_in(&other, "needle", false, false).is_empty());
    }

    #[test]
    fn wide_characters_report_grid_columns() {
        // '你' '好' 各占两格，所以 'w' 在网格第 4 列而不是第 2 列。
        let win = window_from(&["你好world"], 12, 0);
        let m = find_matches_in(&win, "world", false, false);
        assert_eq!(m.len(), 1);
        assert_eq!((m[0].col_start, m[0].col_end), (4, 9));
    }

    #[test]
    fn trailing_padding_is_not_searchable() {
        // 行尾空白是填充，不是内容：搜空格不应命中一行的右半边。
        let win = window_from(&["ab"], 8, 0);
        assert!(find_matches_in(&win, " ", false, false).is_empty());
        assert!(find_matches_in(&win, "  ", false, false).is_empty());
    }

    #[test]
    fn query_longer_than_the_line_matches_nothing() {
        let win = window_from(&["ab", ""], 8, 0);
        assert!(find_matches_in(&win, "abc", false, false).is_empty());
    }

    /// 2026-11-17 Find 栏那颗 `W`（Match Whole Word）：命中两端必须贴着词边界。
    ///
    /// 判据用「词字符 = 字母 / 数字 / 下划线」这一条：`foobar` / `foo_bar` 不算，
    /// `foo.bar` / `(foo)` / 行首行尾都算。四个反例一个正例，钉住的是边界规则
    /// 本身——这条规则错了的症状是「搜 foo 却高亮了 foo_bar 里那三个字母」。
    #[test]
    fn whole_word_only_matches_on_word_boundaries() {
        let win = window_from(&["foo foobar foo_bar foo.bar (foo)"], 40, 0);
        let hits = find_matches_in(&win, "foo", false, true);
        // 命中：行首 `foo`(0..3)、`foo.bar` 的 `foo`(19..22)、`(foo)` 的 `foo`(28..31)。
        assert_eq!(
            hits.iter()
                .map(|m| (m.col_start, m.col_end))
                .collect::<Vec<_>>(),
            vec![(0, 3), (19, 22), (28, 31)],
            "整词模式只认两端是边界的那些"
        );
        // 关掉整词，同一个查询命中全部五处——这说明差异确实来自这条规则，
        // 不是坐标算错。
        assert_eq!(find_matches_in(&win, "foo", false, false).len(), 5);
    }

    /// 整词的大小写规则与普通查找共用一套：`Foo` 在区分大小写时只命中 `Foo`。
    #[test]
    fn whole_word_respects_case_sensitivity() {
        let win = window_from(&["Foo foo"], 12, 0);
        assert_eq!(find_matches_in(&win, "foo", true, true), vec![Match::new(0, 4, 7)]);
        assert_eq!(find_matches_in(&win, "foo", false, true).len(), 2);
    }

    #[test]
    fn next_and_previous_work_on_history_rows() {
        let win = window_from(&["x", "hit", "x", "hit"], 8, -2);
        // 游标在窗口倒数第二行（0），下一个匹配应落在第 1 行。
        let next = find_next(&win, "hit", false, false, 0, 0);
        assert_eq!(next.map(|(i, m)| (i, m.row)), Some((1, 1)));
        // 从同一个位置往回找，应回到 -1 行那一处。
        let prev = find_previous(&win, "hit", false, false, 0, 0);
        assert_eq!(prev.map(|(i, m)| (i, m.row)), Some((0, -1)));
    }
}

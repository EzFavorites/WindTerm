// 本模块从 find.rs 以 #[path] 挂出，`super` 是 crate 根；显式指回 find。
use crate::find::*;
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
fn regex_matches_use_the_same_cell_coordinates_as_literal_matches() {
    let img = window_from(&["你好world", "abc123"], 12, 0);
    // 字面与正则对同一模式给出完全相同的列区间。
    let literal = find_matches_in(&img, "world", false, false);
    let regex = find_matches_regex_in(&img, "w.rld", false).expect("合法正则");
    assert_eq!(regex, literal);
    // 宽字符：汉字占两格，正则的列映射必须和字面版一样按格算，
    // 不能按字节把「你」算成一格。
    let han = find_matches_regex_in(&img, "好", false).expect("合法正则");
    assert_eq!(han, vec![Match::new(0, 2, 4)]);
}

#[test]
fn regex_matches_honour_case_and_report_syntax_errors() {
    let img = window_from(&["Hello hello"], 12, 0);
    let ci = find_matches_regex_in(&img, "hello", false).expect("合法正则");
    assert_eq!(ci.len(), 2, "忽略大小写时两处都命中");
    let cs = find_matches_regex_in(&img, "hello", true).expect("合法正则");
    assert_eq!(cs.len(), 1, "区分大小写时只命中小写那处");
    // 语法错必须返回 Err，让 UI 能提示「表达式写错了」而不是显示 0 个匹配。
    assert!(find_matches_regex_in(&img, "[", false).is_err());
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

// ── 同一套引擎，两种窗口 ──────────────────────────────

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

// ── 字面 / 正则：同一套大小写折叠口径 ────────────────

/// 审查发现的真实分叉：`Ä` / `ä` 原先字面版用 `eq_ignore_ascii_case` 漏掉，
/// 正则版用 `case_insensitive` 命中。现在两种模式必须逐项相等。
#[test]
fn literal_and_regex_agree_on_non_ascii_case_folding() {
    let win = window_from(&["Ä ä Ähnlich"], 16, 0);
    let q = "ä";
    let literal = find_matches_in(&win, q, false, false);
    let regex = find_matches_regex_in(&win, &regex::escape(q), false).expect("合法正则");
    assert_eq!(literal, regex, "忽略大小写时两条引擎必须给出同一组列区间");
    assert_eq!(literal.len(), 3, "两处 Ä 与一处 ä 都应命中");
    assert_eq!(
        literal.iter().map(|m| (m.col_start, m.col_end)).collect::<Vec<_>>(),
        vec![(0, 1), (2, 3), (4, 5)]
    );
    // 区分大小写时两边都只认小写那处，且仍然逐项相等。
    let literal_cs = find_matches_in(&win, q, true, false);
    let regex_cs = find_matches_regex_in(&win, &regex::escape(q), true).expect("合法正则");
    assert_eq!(literal_cs, regex_cs);
    assert_eq!(literal_cs, vec![Match::new(0, 2, 3)]);
    // 快照入口（find_matches）走同一个 scan_line，口径也不能分叉。
    let snap = make_snap("Ä ä");
    assert_eq!(find_matches(&snap, q, false), vec![Match::new(0, 0, 1), Match::new(0, 2, 3)]);
    assert_eq!(find_matches(&snap, q, true), vec![Match::new(0, 2, 3)]);
}

/// 同一段文本上分别跑字面与正则，逐项断言两条引擎口径一致。
fn assert_literal_equals_regex(text: &str, query: &str, case_sensitive: bool, expected: usize) {
    let win = window_from(&[text], text.chars().count().max(8), 0);
    let literal = find_matches_in(&win, query, case_sensitive, false);
    let pattern = regex::escape(query);
    let regex = find_matches_regex_in(&win, &pattern, case_sensitive).expect("转义后的字面量必然合法");
    assert_eq!(literal, regex, "text={text:?} query={query:?} cs={case_sensitive}");
    assert_eq!(literal.len(), expected, "text={text:?} query={query:?} cs={case_sensitive}");
}

/// 第三轮回归：手写 lower/upper 双向等价对 28 对字符过度匹配，其中 `I`/`ı`、
/// `i`/`ı`、`U+A7CE`/`U+A7CF` 都会被判成同义（Python/Rust 的 `to_uppercase`
/// 把 `ı` 映成 `I`、把 `A7CE` 与 `A7CF` 互映），但 `regex` 的 Simple_Case_Fold
/// 并不折叠它们。字面模式必须和正则一样**不命中**，两边逐项相等。
#[test]
fn literal_and_regex_agree_on_overfolded_unicode_pairs() {
    for (a, b) in [
        ('I', '\u{0131}'),
        ('i', '\u{0131}'),
        ('\u{A7CE}', '\u{A7CF}'),
        ('\u{A7CF}', '\u{A7CE}'),
        ('İ', 'i'),
    ] {
        for case_sensitive in [false, true] {
            let text = format!("{a}{b}");
            let q = a.to_string();
            assert_literal_equals_regex(&text, &q, case_sensitive, 1);
        }
        // 反向查询同理：两个字形互为查询时，两边都只认自己。
        let text = format!("{a}{b}");
        let q = b.to_string();
        assert_literal_equals_regex(&text, &q, false, 1);
    }
    // 对照：字符串里真的含两份相同字形时，两边都命中两次。
    assert_literal_equals_regex("ıı", "ı", false, 2);
    assert_literal_equals_regex("꟏꟏", "\u{A7CF}", false, 2);
}

/// 正常折叠类仍要在两种模式下都命中，避免「为了修过度匹配把真等价也砍掉」。
/// 这些对子正是第二轮 `same_char` 想覆盖的范围，现在由 regex 引擎统一给出。
#[test]
fn literal_and_regex_agree_on_real_unicode_fold_classes() {
    for (a, b) in [
        ('Ä', 'ä'),
        ('É', 'é'),
        ('Σ', 'σ'),
        ('σ', 'ς'),
        ('K', '\u{212A}'), // K / KELVIN SIGN
        ('ß', '\u{1E9E}'), // ß / LATIN CAPITAL SHARP S
        ('\u{03D1}', '\u{03F4}'), // 希腊 θ 的两个符号字形
    ] {
        let text = format!("{a}{b}");
        let q = a.to_string();
        assert_literal_equals_regex(&text, &q, false, 2);
        assert_literal_equals_regex(&text, &q, true, 1);
    }
    // 快照入口也走同一个引擎，不能因为输入形状不同又分叉。
    let snap = make_snap("İ ı i");
    assert_eq!(find_matches(&snap, "ı", false).len(), 1);
    assert_eq!(find_matches(&snap, "I", false).len(), 1);
}

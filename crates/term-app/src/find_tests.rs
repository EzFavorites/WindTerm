//! `find.rs` 的单测模块（由 `#[path]` 挂回，主文件留给生产逻辑）。

use super::*;
use term_core::grid::{Attrs, Cell, Colour, GridSnapshot, LineSlice, ScreenImage};

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
        mouse_mode: term_core::mouse_report::MouseMode::Off,
        mouse_encoding: term_core::mouse_report::MouseEncoding::X10,
        alt: false,
        history: 0,
        blocks: Vec::new(),
        row_times: Vec::new(),
        wraps: Vec::new(),
    }
}

/// 贴底窗口（行号 `0..rows`）。
fn window(text: &str) -> ScreenImage {
    make_snap(text).to_screen_image()
}

/// 回看窗口：行号可为负，用来验证「搜的是看得见的那一段」。
fn window_from(lines: &[&str], cols: usize, start_row: i64) -> ScreenImage {
    let lines: Vec<LineSlice> = lines
        .iter()
        .enumerate()
        .map(|(i, text)| {
            let mut cells = vec![Cell::default(); cols];
            for (c, ch) in text.chars().take(cols).enumerate() {
                cells[c].ch = ch;
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

fn open_state() -> FindState {
    let mut s = FindState::default();
    reduce(&mut s, FindAction::Open, None);
    s
}

fn counted(state: &FindState, win: &ScreenImage) -> usize {
    matches(state, win).len()
}

#[test]
fn regex_toggle_changes_the_match_set_and_open_resets_it() {
    let win = window_from(&["cat cot cut"], 12, 0);
    let mut s = open_state();
    s.query = "c.t".into();
    // 字面模式：`c.t` 不含通配符，搜不到任何东西。
    assert_eq!(counted(&s, &win), 0, "字面模式不该把 . 当通配符");
    // 正则模式：一个模式命中三个词。
    reduce(&mut s, FindAction::ToggleRegex, None);
    assert!(s.regex);
    assert_eq!(counted(&s, &win), 3, "正则模式 c.t 命中 cat/cot/cut");
    // 语法错返回空集合而非 panic。
    s.query = "[".into();
    assert_eq!(counted(&s, &win), 0, "非法正则不该 panic，计为 0 个匹配");
    // Open 必须把 regex 复位，否则新一次查找会带着上一次的口径。
    reduce(&mut s, FindAction::Open, None);
    assert!(!s.regex, "Open 应复位正则开关");
}

// ── 缺陷修复：栏关着时 Find Next / Previous 要有可见效果 ─────────

#[test]
fn find_next_with_the_bar_closed_opens_it_and_keeps_the_query() {
    // 出厂 `FindState::default()` 里 `visible == false`（查找栏不绘制）。
    // 修复前 `Next` 只挪 `current_match`，屏幕上看不见任何变化；
    // 修复后必须先把栏打开，同时**保留已有查询**（Next ≠ 重新搜索）。
    let mut s = FindState {
        visible: false,
        query: "abc".into(),
        current_match: 3,
        ..FindState::default()
    };
    reduce(&mut s, FindAction::Next, None);
    assert!(s.visible, "栏关着时 Find Next 必须把查找栏打开");
    assert_eq!(s.query, "abc", "Find Next 不得清空查询词");
}

#[test]
fn find_previous_with_the_bar_closed_opens_it_too() {
    let mut s = FindState {
        visible: false,
        query: "abc".into(),
        current_match: 3,
        ..FindState::default()
    };
    reduce(&mut s, FindAction::Previous, None);
    assert!(s.visible, "栏关着时 Find Previous 必须把查找栏打开");
    assert_eq!(s.query, "abc", "Find Previous 不得清空查询词");
}

#[test]
fn parse_maps_the_three_menu_ids() {
    assert_eq!(parse("Text.Find"), Some(FindAction::Open));
    assert_eq!(parse("Text.FindNext"), Some(FindAction::Next));
    assert_eq!(parse("Text.FindPrevious"), Some(FindAction::Previous));
    assert_eq!(
        parse("Text.FindNextSelection"),
        Some(FindAction::SelectionNext)
    );
    assert_eq!(
        parse("Text.FindPreviousSelection"),
        Some(FindAction::SelectionPrev)
    );
    assert_eq!(parse("Text.Copy"), None);
}

#[test]
fn selection_find_opens_the_bar_and_resets() {
    // SelectionNext / SelectionPrev 与 Open 同款复位：打开栏、清空旧查询、
    // 回到第一个匹配。真正的「用选区当查询词」在投影层做（reducer 没有选区）。
    let win = window("test");
    let mut s = open_state();
    reduce(&mut s, FindAction::SetQuery("stale".into()), Some(&win));
    reduce(&mut s, FindAction::SelectionNext, Some(&win));
    assert!(s.visible);
    assert!(s.query.is_empty());
    assert_eq!(s.current_match, 0);
    reduce(&mut s, FindAction::SelectionPrev, Some(&win));
    assert!(s.visible);
    assert!(s.query.is_empty());
}

#[test]
fn open_resets_every_field() {
    let mut s = FindState {
        query: "stale".into(),
        case_sensitive: true,
        current_match: 3,
        ..FindState::default()
    };
    reduce(&mut s, FindAction::Open, None);
    assert!(s.visible);
    assert!(s.query.is_empty());
    assert!(!s.case_sensitive);
    assert_eq!(s.current_match, 0);
    let win = window("test");
    assert_eq!(counted(&s, &win), 0);
}

#[test]
fn set_query_counts_matches_from_the_window() {
    let win = window("test line\ntest again\nno match");
    let mut s = open_state();
    reduce(&mut s, FindAction::SetQuery("test".into()), Some(&win));
    assert_eq!(counted(&s, &win), 2);
    assert_eq!(s.current_match, 0);
}

#[test]
fn toggle_case_recounts() {
    let win = window("Test\ntest");
    let mut s = open_state();
    reduce(&mut s, FindAction::SetQuery("test".into()), Some(&win));
    assert_eq!(counted(&s, &win), 2);
    reduce(&mut s, FindAction::ToggleCase, Some(&win));
    assert_eq!(counted(&s, &win), 1);
    reduce(&mut s, FindAction::ToggleCase, Some(&win));
    assert_eq!(counted(&s, &win), 2);
}

#[test]
fn next_wraps_around_the_match_set() {
    let win = window("test\ntest\ntest");
    let mut s = open_state();
    reduce(&mut s, FindAction::SetQuery("test".into()), Some(&win));
    assert_eq!(s.current_match, 0);
    reduce(&mut s, FindAction::Next, Some(&win));
    assert_eq!(s.current_match, 1);
    reduce(&mut s, FindAction::Next, Some(&win));
    assert_eq!(s.current_match, 2);
    reduce(&mut s, FindAction::Next, Some(&win));
    assert_eq!(s.current_match, 0, "越过最后一个应回到第一个");
}

#[test]
fn previous_wraps_backwards() {
    let win = window("test\ntest\ntest");
    let mut s = open_state();
    reduce(&mut s, FindAction::SetQuery("test".into()), Some(&win));
    reduce(&mut s, FindAction::Previous, Some(&win));
    assert_eq!(s.current_match, 2, "从第一个往回应到最后一个");
}

#[test]
fn next_without_matches_is_a_stable_noop() {
    let win = window("nothing here");
    let mut s = open_state();
    reduce(&mut s, FindAction::SetQuery("zzz".into()), Some(&win));
    reduce(&mut s, FindAction::Next, Some(&win));
    assert_eq!((s.current_match, counted(&s, &win)), (0, 0));
    reduce(&mut s, FindAction::Next, Some(&win));
    assert_eq!((s.current_match, counted(&s, &win)), (0, 0));
}

#[test]
fn hidden_bar_never_counts() {
    let win = window("test\ntest");
    let mut s = FindState::default();
    reduce(&mut s, FindAction::SetQuery("test".into()), Some(&win));
    assert_eq!(counted(&s, &win), 0, "不可见时不得统计，避免隐藏状态下算匹配");
}

#[test]
fn reduce_is_deterministic_for_the_same_inputs() {
    let win = window("test\ntest");
    let run = || {
        let mut s = open_state();
        reduce(&mut s, FindAction::SetQuery("test".into()), Some(&win));
        reduce(&mut s, FindAction::Next, Some(&win));
        s
    };
    assert_eq!(run(), run());
}

#[test]
fn close_clears_everything() {
    let win = window("test\ntest");
    let mut s = open_state();
    reduce(&mut s, FindAction::SetQuery("test".into()), Some(&win));
    reduce(&mut s, FindAction::Close, Some(&win));
    assert!(!s.visible);
    assert!(s.query.is_empty());
    assert_eq!(s.current_match, 0);
}

// ── F2：查找扩到回看缓冲 ────────────────────────────────

#[test]
fn matches_are_counted_in_the_visible_window_including_history() {
    // 窗口覆盖 -3..1，其中两行历史、一行当前屏命中。
    let win = window_from(&["hit a", "miss", "hit b", "hit c"], 12, -3);
    let mut s = open_state();
    reduce(&mut s, FindAction::SetQuery("hit".into()), Some(&win));
    assert_eq!(counted(&s, &win), 3);
    let rows: Vec<i64> = matches(&s, &win).iter().map(|m| m.row).collect();
    assert_eq!(rows, vec![-3, -1, 0], "行号必须是内容绝对行号（含负数）");
}

#[test]
fn scrolling_changes_the_count_without_any_action() {
    // 这是 F2 把计数删出状态的理由：窗口换了，计数就该跟着换。
    let mut s = open_state();
    reduce(&mut s, FindAction::SetQuery("hit".into()), None);
    let top = window_from(&["hit", "hit"], 8, -6);
    let bottom = window_from(&["nothing", "nothing"], 8, 0);
    assert_eq!(counted(&s, &top), 2);
    assert_eq!(counted(&s, &bottom), 0);
    assert!(current_index(&s, 0).is_none());
}

#[test]
fn current_index_never_points_outside_the_set() {
    // 在 5 个匹配里选了第 3 个，滚回只有 1 个匹配的窗口时不该越界。
    let mut s = open_state();
    reduce(&mut s, FindAction::SetQuery("hit".into()), None);
    s.current_match = 3;
    assert_eq!(current_index(&s, 5), Some(3));
    assert_eq!(current_index(&s, 2), Some(1));
    assert_eq!(current_index(&s, 1), Some(0));
}

/// 2026-11-17 `W`（Match Whole Word）进状态：开关本身是纯状态，
/// 但它换的是**匹配口径**——换口径必须回到第一个匹配，否则「第 3 个」
/// 那个索引会指向新集合里完全不同的地方。
#[test]
fn toggling_whole_word_resets_the_current_match() {
    let win = window("foo foobar foo");
    let mut s = open_state();
    reduce(&mut s, FindAction::SetQuery("foo".into()), Some(&win));
    assert_eq!(matches(&s, &win).len(), 3, "关掉整词时三处都算");
    reduce(&mut s, FindAction::Next, Some(&win));
    reduce(&mut s, FindAction::Next, Some(&win));
    assert_eq!(s.current_match, 2);

    reduce(&mut s, FindAction::ToggleWholeWord, Some(&win));
    assert!(s.whole_word);
    assert_eq!(s.current_match, 0, "换口径要回到第一个");
    assert_eq!(matches(&s, &win).len(), 2, "整词下 `foobar` 里那处不算");

    reduce(&mut s, FindAction::ToggleWholeWord, Some(&win));
    assert!(!s.whole_word);
}

/// `Open` 是「新开一次查找」：三个开关一起回出厂，与 `query` 一起清。
#[test]
fn opening_the_bar_clears_every_option() {
    let mut s = open_state();
    s.case_sensitive = true;
    s.whole_word = true;
    reduce(&mut s, FindAction::Open, None);
    assert!(!s.case_sensitive && !s.whole_word);
    assert!(s.query.is_empty());
    assert_eq!(s.current_match, 0);
}

#[test]
fn set_query_goes_back_to_the_first_match() {
    let win = window("hit\nhit\nhit");
    let mut s = open_state();
    reduce(&mut s, FindAction::SetQuery("hit".into()), Some(&win));
    reduce(&mut s, FindAction::Next, Some(&win));
    reduce(&mut s, FindAction::Next, Some(&win));
    assert_eq!(s.current_match, 2);
    reduce(&mut s, FindAction::SetQuery("hi".into()), Some(&win));
    assert_eq!(s.current_match, 0, "新查询应从第一个匹配开始");
}

// ── 字面正则缓存：命中 / 失效键 ───────────────────────────

#[test]
fn literal_regex_is_compiled_once_per_query_and_case() {
    // 唯一的查询串：并发跑全量单测时也不与其他用例的缓存条目相撞。
    let query = "__r4_cache_hit_词__";
    let win = window("Hello hello WORLD");
    let mut s = open_state();
    s.query = query.into();
    let before = literal_compiles_for(query, false);
    let first = matches(&s, &win);
    assert!(first.is_empty(), "这个查询串不该命中任何行");
    assert_eq!(literal_compiles_for(query, false), before + 1, "首次查找编译一次");

    let again = matches(&s, &win);
    assert_eq!(again, first);
    assert_eq!(
        literal_compiles_for(query, false),
        before + 1,
        "同 (query, case) 必须命中缓存，不重编译"
    );

    // 换窗口不换键：缓存的是编译结果，不是匹配结果。
    let other = window_from(&["__r4_cache_hit_词__ miss"], 24, -5);
    assert_eq!(matches(&s, &other).len(), 1);
    assert_eq!(literal_compiles_for(query, false), before + 1);

    // 大小写是键的一部分：切过去要另编译一次，切回来仍命中旧条目。
    let cs_before = literal_compiles_for(query, true);
    s.case_sensitive = true;
    let _ = matches(&s, &win);
    assert_eq!(literal_compiles_for(query, true), cs_before + 1);
    s.case_sensitive = false;
    let _ = matches(&s, &win);
    assert_eq!(literal_compiles_for(query, false), before + 1, "切回旧口径仍是缓存命中");
    assert_eq!(literal_compiles_for(query, true), cs_before + 1);

    // 换查询失效到新键。
    let next = "__r4_cache_hit_next__";
    let next_before = literal_compiles_for(next, false);
    s.query = next.into();
    let _ = matches(&s, &win);
    assert_eq!(literal_compiles_for(next, false), next_before + 1);
    assert_eq!(literal_compiles_for(query, false), before + 1, "旧键不受影响");
}

#[test]
fn cached_literal_matches_agree_with_the_core_engine() {
    // 缓存只是复用了 Regex 对象，口径（含宽字符列、标点转义、大小写折叠）
    // 必须与 term-core 的 find_matches_in 逐项一致。
    let win = window_from(&["你好world", "a.b a+b", "Hello HELLO"], 16, -2);
    for query in ["你好", "a.b", "a+b", "hello", "[", " "] {
        for case_sensitive in [false, true] {
            for whole_word in [false, true] {
                assert_eq!(
                    find_matches_literal_cached(&win, query, case_sensitive, whole_word),
                    term_core::find::find_matches_in(&win, query, case_sensitive, whole_word),
                    "query={query:?} case={case_sensitive} whole={whole_word}"
                );
            }
        }
    }
}

#[test]
fn whole_word_filter_scales_with_hits_not_lines_times_hits() {
    // 400 行回看、每行 2 处命中（共 800 处，只有末词算整词）。旧实现逐命中
    // `lines.iter().find` 是 O(行数×命中数)；这里钉住索引化后的正确结果。
    let data: Vec<String> = (0..400).map(|i| format!("r{i} foobar foo")).collect();
    let refs: Vec<&str> = data.iter().map(String::as_str).collect();
    let win = window_from(&refs, 32, -400);
    let mut s = open_state();
    s.query = "foo".into();
    s.whole_word = true;
    let hits = matches(&s, &win);
    assert_eq!(hits.len(), 400, "每行只有末尾那个 foo 是整词");
    assert_eq!(hits.first().map(|m| m.row), Some(-400));
    assert_eq!(hits.last().map(|m| m.row), Some(-1));
    assert!(hits.iter().all(|m| m.col_start >= 10), "foobar 里的前缀命中必须被过滤");

    // 结果列表同样按 row 索引取摘要，不能把 800 处命中缩成 400 行后错配。
    let rows = result_rows(&s, &win).rows;
    assert_eq!(rows.len(), 400);
    assert!(rows.iter().all(|r| r.summary.contains("foobar foo")));
    assert_eq!(rows.first().map(|r| r.row), Some(-400));
}

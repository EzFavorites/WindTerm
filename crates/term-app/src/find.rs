//! Find 能力：会话内查找的局部动作与局部 reducer。
//!
//! 之前 Find 的状态是「投影层直接写」的：`ui/find_bar.rs` 里的
//! `update_matches(&mut FindState, …)` 在绘制过程中把 `total_matches` /
//! `current_match` 写进 `ViewState`，等于投影层成了第二个写入者（违反单向依赖）。
//!
//! 把计算搬进了 reducer，但保留了「计数」这个缓存槽。
//! F2 发现缓存槽必然过期：查找的输入从「整块快照」变成「当前事实窗口」，
//! 而滚动会换窗口却不产生任何 `Action`，于是「3 / 7」会在回看后停在旧数字上。
//! 补法不是「滚动时也写一次」，而是**把计数从状态里删掉**——它是
//! `(query, 大小写, 窗口)` 的纯函数值，边界在 `matches()`，没有任何缓存。
//!
//! 状态里因此只剩三件事：开没开、搜什么、区分大小写；外加一个
//! `current_match`（用户的「第几个」选择，按总数取模使用）。

use crate::state::FindState;
use term_core::find::{find_matches_in, find_matches_regex_in, Match};
use term_core::grid::ScreenImage;

pub use crate::vocab::FindAction;


/// 窗口内的全部匹配：`(query, 大小写, 窗口)` 的纯函数值。
///
/// 「窗口」是用户此刻看得见的那一屏（含回看部分），所以贴在底部时
/// 就是当前屏，滚进历史时就是历史——同一份代码，不需要分支。
///
/// 栏没打开时恒为空：隐藏状态下谁也别想拿到计数，
/// 否则「不可见但仍在统计」迟早会被人当成状态用。
pub fn matches(state: &FindState, window: &ScreenImage) -> Vec<Match> {
    if !state.visible || state.query.is_empty() {
        return Vec::new();
    }
    if state.regex {
        // 表达式写错时返回空集合（显示 0/N），而不是退回字面匹配——那会让
        // 用户以为 `[` 真的搜到了东西。UI 另有语法校验提示（见 find_bar）。
        find_matches_regex_in(window, &state.query, state.case_sensitive).unwrap_or_default()
    } else {
        find_matches_in(window, &state.query, state.case_sensitive, state.whole_word)
    }
}

/// 当前匹配在匹配集合里的下标；没有匹配时为 `None`。
///
/// 对总数取模：窗口一变总数就变，取模保证 `current_match` 永远不会指到
/// 集合外面，也不需要在换窗口时「帮忙修正」它。
pub fn current_index(state: &FindState, total: usize) -> Option<usize> {
    if total == 0 {
        None
    } else {
        Some(state.current_match % total)
    }
}

/// 纯函数：只改 `FindState`，匹配集合由「窗口 + query」确定性推导。
///
/// `Next` / `Previous` 在匹配集合上循环，而不是从光标位置重新搜索——
/// 前者与「N / M」计数天然一致，也不会因为光标停在匹配内部而原地打转。
/// 命令 ID → 本能力的动作。未接线的 ID 返回 `None`。
///
/// 2026-11-12：实现只有一份，在 [`crate::parse`]（入口层）。这里保留同名入口，
/// 一是因为「每个能力自带字符串入口」是这个 crate 对外的约定，二是因为
/// `apps/desktop` 有直接调 `term_app::log::parse` 的地方——把路径改掉没有架构价值。
pub fn parse(id: &str) -> Option<FindAction> {
    crate::parse::find_parse(id)
}

pub fn reduce(state: &mut FindState, action: FindAction, window: Option<&ScreenImage>) {
    match action {
        FindAction::Open => {
            state.visible = true;
            state.query.clear();
            state.case_sensitive = false;
            state.whole_word = false;
            state.regex = false;
            state.current_match = 0;
        }
        FindAction::SetQuery(q) => {
            state.query = q;
            // 换了查询就回到第一个匹配——否则「第 9 个」这种残留索引
            // 会让人以为新查询是从中间开始搜的。
            state.current_match = 0;
        }
        FindAction::ToggleCase => {
            state.case_sensitive = !state.case_sensitive;
            state.current_match = 0;
        }
        // 换匹配口径 = 换了一个匹配集合，与改大小写同理要回到第一个。
        FindAction::ToggleWholeWord => {
            state.whole_word = !state.whole_word;
            state.current_match = 0;
        }
        // 正则与整词同理：换匹配口径就换了一个匹配集合，回到第一个。
        FindAction::ToggleRegex => {
            state.regex = !state.regex;
            state.current_match = 0;
        }
        // 栏关着时按 Find Next，之前只挪了 `current_match`：没人看得见
        // （绘制条件是 `find.visible`），而且 `Open` 会把 `current_match` 归零，
        // 等于什么都没留下。兄弟动作 `SelectionNext`/`SelectionPrev` 早就先开栏，
        // 这里对齐它们；但**不清 `query`**——「在现有查询上再走一步」是 Next 的语义，
        // 清了就变成「重新查一次空串」。
        FindAction::Next => {
            state.visible = true;
            step(state, window, 1)
        }
        // 同上：开栏 + 保留查询，只是在匹配集合上反向走一步。
        FindAction::Previous => {
            state.visible = true;
            step(state, window, -1)
        }
        FindAction::Close => {
            state.visible = false;
            state.query.clear();
            state.current_match = 0;
        }
        // 以选区为查询词的前置复位——打开栏、清空旧查询、回到第一个匹配。
        // 「填选区 + 跳一步」交给投影层（`SelectionNext`/`SelectionPrev` 在 action.rs
        // 路由成 `Effect::FindSelection(±1)`）。
        FindAction::SelectionNext | FindAction::SelectionPrev => {
            state.visible = true;
            state.query.clear();
            state.current_match = 0;
        }
    }
}

fn step(state: &mut FindState, window: Option<&ScreenImage>, delta: i64) {
    let total = window.map(|w| matches(state, w).len()).unwrap_or(0);
    state.current_match = match current_index(state, total) {
        Some(cur) => (cur as i64 + delta).rem_euclid(total as i64) as usize,
        None => 0,
    };
}

#[cfg(test)]
mod tests {
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
}

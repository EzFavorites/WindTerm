//! Find 栏的纯逻辑测试：受控输入、视觉几何、以及三项新控件的纯层语义。
//! 由 `find_bar.rs` 的 `#[cfg(test)] #[path = …] mod tests;` 引入。

use super::*;
use term_app::find::{matches, result_rows, ui_command, FindUiCommand};
use term_app::state::{FindDirection, FindState};
use term_core::grid::{Cell, LineSlice, ScreenImage};

fn window(lines: &[(&str, i64)]) -> ScreenImage {
    let rows: Vec<LineSlice> = lines
        .iter()
        .map(|(text, row)| {
            let mut cells = vec![Cell::default(); 16];
            for (i, ch) in text.chars().enumerate() {
                cells[i].ch = ch;
            }
            LineSlice {
                row: *row,
                cells,
            }
        })
        .collect();
    let first = lines.first().map(|l| l.1).unwrap_or(0);
    let last = lines.last().map(|l| l.1).unwrap_or(0) + 1;
    ScreenImage {
        cols: 16,
        version: 1,
        range: first..last,
        lines: rows,
        cursor: None,
        history: lines
            .iter()
            .map(|l| l.1.unsigned_abs() as usize)
            .max()
            .unwrap_or(0),
    }
}

fn open_state() -> FindState {
    let mut s = FindState::default();
    term_app::find::reduce(&mut s, FindAction::Open, None);
    s
}

#[test]
fn editing_dispatches_set_query_and_nothing_else() {
    assert_eq!(
        edit_action("abc"),
        Action::Find(FindAction::SetQuery("abc".to_string()))
    );
    assert_eq!(
        edit_action(""),
        Action::Find(FindAction::SetQuery(String::new()))
    );
}

#[test]
fn sync_input_leaves_the_buffer_alone_when_query_is_unchanged() {
    let mut input = String::from("ab");
    assert!(!sync_input(&mut input, "ab"));
    assert_eq!(input, "ab");
}

#[test]
fn sync_input_picks_up_an_external_change() {
    let mut input = String::from("stale");
    assert!(sync_input(&mut input, ""));
    assert_eq!(input, "");
    assert!(sync_input(&mut input, "line"));
    assert_eq!(input, "line");
}

/// 视觉基线的横坐标是死的：任何一格宽度改了，都必须同步改基线。
#[test]
fn the_icon_strip_and_the_three_buttons_add_up_to_the_reference() {
    let icons = ICON_W * 5.0 + CYCLE_GAP + CYCLE_W + ICON_R_PAD;
    assert_eq!(icons, 189.5);
    let right = CLOSE_R_PAD
        + CLOSE_SIZE
        + CLOSE_GAP
        + ALL_W
        + BEFORE_ALL_GAP
        + NAV_W
        + NAV_GAP
        + NAV_W
        + FIELD_TO_NAV;
    assert_eq!(right, 285.0);
    assert_eq!(CLOSE_SIZE - 1.0, 5.5);
}

// ── Find All / ⇄▾ / ≡：三项可见控件的行为证据 ─────────────

#[test]
fn result_rows_come_from_matches_with_absolute_rows_and_summaries() {
    let win = window(&[("hit a", -2), ("miss", -1), ("hit b", 0)]);
    let mut s = open_state();
    term_app::find::reduce(&mut s, FindAction::SetQuery("hit".into()), Some(&win));
    let all = result_rows(&s, &win);
    assert_eq!(all.query, "hit");
    assert_eq!(all.rows.len(), matches(&s, &win).len());
    assert_eq!(
        all.rows
            .iter()
            .map(|r| (r.row, r.summary.as_str()))
            .collect::<Vec<_>>(),
        vec![(-2, "hit a"), (0, "hit b")]
    );
    s.visible = false;
    assert!(result_rows(&s, &win).rows.is_empty(), "不可见时不出列表");
}

#[test]
fn direction_cycles_and_swaps_the_navigation_actions() {
    let mut s = open_state();
    assert_eq!(s.direction, FindDirection::Forward);
    assert_eq!(nav_action(&s, true), FindAction::Next);
    term_app::find::reduce(&mut s, ui_command(FindUiCommand::CycleDirection), None);
    assert_eq!(s.direction, FindDirection::Backward);
    assert_eq!(nav_action(&s, true), FindAction::Previous);
    assert_eq!(nav_action(&s, false), FindAction::Next);
    term_app::find::reduce(&mut s, ui_command(FindUiCommand::CycleDirection), None);
    assert_eq!(s.direction, FindDirection::All);
    assert!(s.find_all_open, "全范围要展开结果列表");
    assert_eq!(nav_action(&s, true), FindAction::Next);
    term_app::find::reduce(&mut s, ui_command(FindUiCommand::CycleDirection), None);
    assert_eq!(s.direction, FindDirection::Forward);
    assert!(!s.find_all_open);
    // 开关不改查询，也不重置当前匹配（用 reducer 走到第 3 项，不直接写状态）。
    let win = window(&[("hit", 0), ("hit", 1), ("hit", 2)]);
    term_app::find::reduce(&mut s, FindAction::SetQuery("hit".into()), Some(&win));
    term_app::find::reduce(&mut s, FindAction::Next, Some(&win));
    term_app::find::reduce(&mut s, FindAction::Next, Some(&win));
    let before = (s.query.clone(), s.current_match);
    assert_eq!(before.1, 2);
    term_app::find::reduce(&mut s, ui_command(FindUiCommand::ToggleFindAll), None);
    assert_eq!((s.query.clone(), s.current_match), before);
}

#[test]
fn multiline_flag_changes_the_regex_match_set() {
    let win = window(&[("foo", 0), ("bar", 1)]);
    let mut s = open_state();
    term_app::find::reduce(&mut s, FindAction::SetQuery("foo.bar".into()), Some(&win));
    term_app::find::reduce(&mut s, FindAction::ToggleRegex, Some(&win));
    assert!(matches(&s, &win).is_empty(), "默认 `.` 不跨行");
    term_app::find::reduce(&mut s, ui_command(FindUiCommand::ToggleMultiLine), None);
    assert!(s.multi_line);
    let found = matches(&s, &win);
    assert_eq!(found.len(), 2, "跨行命中按覆盖的行拆成两段");
    assert_eq!((found[0].row, found[1].row), (0, 1));
    term_app::find::reduce(&mut s, FindAction::SetQuery("^bar$".into()), Some(&win));
    assert_eq!(matches(&s, &win).len(), 1, "多行模式下 ^/$ 按行锚定");
    term_app::find::reduce(&mut s, FindAction::Open, None);
    assert!(!s.multi_line, "新一次查找回到默认口径");
}

#[test]
fn jump_plan_picks_the_shorter_side_of_the_ring() {
    let s = open_state();
    assert_eq!(jump_plan(&s, 5, 3), Some((FindAction::Previous, 2)));
    assert_eq!(jump_plan(&s, 5, 1), Some((FindAction::Next, 1)));
    assert_eq!(jump_plan(&s, 0, 0), None);
}

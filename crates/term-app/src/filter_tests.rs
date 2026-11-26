//! `filter.rs` 的纯层单测（独立文件，守住 500 行门禁）。

use super::*;
use crate::state::FilterState;
use term_core::grid::{Cell, LineSlice, ScreenImage};

fn window(texts: &[&str]) -> ScreenImage {
    let start = -10;
    let lines = texts
        .iter()
        .enumerate()
        .map(|(i, text)| LineSlice {
            row: start + i as i64,
            cells: text
                .chars()
                .map(|ch| Cell {
                    ch,
                    ..Cell::default()
                })
                .collect(),
        })
        .collect();
    ScreenImage {
        cols: 80,
        version: 1,
        range: start..start + texts.len() as i64,
        lines,
        cursor: None,
        history: 10,
    }
}

fn state(query: &str, mode: FilterMode) -> FilterState {
    FilterState {
        visible: true,
        query: query.to_string(),
        mode,
        ..FilterState::default()
    }
}

#[test]
fn closed_or_empty_query_never_filters() {
    let w = window(&["keep", "drop", "keep"]);
    let mut s = state("keep", FilterMode::KeepMatching);
    s.visible = false;
    assert!(hidden_rows(&s, &w).is_empty());
    s.visible = true;
    s.query.clear();
    assert!(hidden_rows(&s, &w).is_empty());
}

#[test]
fn grep_keeps_only_matching_rows() {
    let w = window(&["ERR boot", "ok", "ERR disk"]);
    let hidden = hidden_rows(&state("ERR", FilterMode::KeepMatching), &w);
    assert_eq!(hidden.len(), 1);
    assert!(hidden.contains(&-9));
}

#[test]
fn grep_v_drops_matching_rows() {
    let w = window(&["noise 1", "data", "noise 2"]);
    let hidden = hidden_rows(&state("^noise", FilterMode::DropMatching), &w);
    assert_eq!(hidden.len(), 2);
    assert!(hidden.contains(&-10));
    assert!(hidden.contains(&-8));
}

#[test]
fn literal_mode_treats_metacharacters_as_text() {
    let w = window(&["a.b", "axb"]);
    let hidden = hidden_rows(&state("a.b", FilterMode::KeepMatching), &w);
    assert!(hidden.contains(&-9), "正则元字符在字面模式下不生效");
}

#[test]
fn regex_mode_and_case_toggle_use_the_shared_compiler() {
    let w = window(&["a.b", "axb"]);
    let hidden = hidden_rows(
        &FilterState {
            regex: true,
            ..state("a.b", FilterMode::KeepMatching)
        },
        &w,
    );
    assert!(hidden.contains(&-9));

    let w2 = window(&["ERR", "err"]);
    let hidden2 = hidden_rows(&state("ERR", FilterMode::KeepMatching), &w2);
    assert_eq!(hidden2.len(), 1, "默认大小写敏感");
    let hidden3 = hidden_rows(
        &FilterState {
            case_sensitive: false,
            ..state("ERR", FilterMode::KeepMatching)
        },
        &w2,
    );
    assert!(hidden3.is_empty());
}

#[test]
fn invalid_regex_is_an_error_not_a_silent_fallback() {
    let s = FilterState {
        regex: true,
        ..state("[", FilterMode::KeepMatching)
    };
    assert!(compile(&s).is_err());
    assert!(hidden_rows(&s, &window(&["a"])).is_empty());
}

#[test]
fn counts_report_visible_and_hidden() {
    let w = window(&["ERR a", "b", "ERR c", "d"]);
    assert_eq!(counts(&state("ERR", FilterMode::KeepMatching), &w), (2, 2));
    assert_eq!(counts(&state("ERR", FilterMode::DropMatching), &w), (2, 2));
}

#[test]
fn reduce_resets_on_open_and_closes_cleanly() {
    let mut s = state("old", FilterMode::DropMatching);
    s.regex = true;
    reduce(&mut s, FilterAction::Open);
    assert!(s.visible);
    assert!(s.query.is_empty());
    assert_eq!(s.mode, FilterMode::KeepMatching);
    assert!(!s.regex);
    reduce(&mut s, FilterAction::Close);
    assert!(!s.visible);
}

#[test]
fn ui_commands_toggle_flags_without_touching_the_query() {
    let mut s = state("ERR", FilterMode::KeepMatching);
    reduce(&mut s, ui_command(FilterUiCommand::ToggleMode));
    assert_eq!(s.mode, FilterMode::DropMatching);
    assert_eq!(s.query, "ERR");
    reduce(&mut s, ui_command(FilterUiCommand::ToggleCase));
    assert!(s.case_sensitive);
    reduce(&mut s, ui_command(FilterUiCommand::ToggleRegex));
    assert!(s.regex);
    assert_eq!(s.query, "ERR");
}

#[test]
fn toggle_reuses_the_query() {
    let mut s = state("ERR", FilterMode::KeepMatching);
    reduce(&mut s, FilterAction::Toggle);
    assert!(!s.visible);
    reduce(&mut s, FilterAction::Toggle);
    assert_eq!(s.query, "ERR");
}

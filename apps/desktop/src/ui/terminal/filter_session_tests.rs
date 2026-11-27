//! 行过滤器「每条会话独立」的判据：`Tab.filter` / `Tab.filter_input` 归属各自 tab，
//! `Effect::Filter` / `apply_filter_on` 对指定会话落地，**不碰 `ViewState`**，
//! 两个 tab（分屏两个 pane）互不串味。
//!
//! 与 `marks_menu_tests.rs` 同款：真窗口 + `lifecycle_test_support`，走 `apply_filter_on`
//! 那一个 `filter::reduce` 落地点，不直接改字段、不绕过动作。

use crate::ui::lifecycle_test_support::{fake_tab, window_with, CloseCount};
use crate::ui::TerminalWindow;
use term_app::state::{FilterMode, FilterState};
use term_app::vocab::FilterAction;

fn two_tab_window(ctx: &egui::Context) -> TerminalWindow {
    let pool = std::sync::Arc::new(term_session::SessionPool::new());
    let closes = CloseCount::default();
    let first = fake_tab(&pool, ctx, 0, &closes);
    let second = fake_tab(&pool, ctx, 1, &closes);
    window_with(pool, ctx, vec![first, second])
}

/// 两个 tab 的过滤互不串味：对 tab 0 落地的 `Action::Filter` 只改 tab 0，
/// tab 1 的 `filter` / `filter_input` 保持出厂态；也不写 `ViewState`。
#[test]
fn two_tabs_filter_independently_without_touching_view_state() {
    let ctx = egui::Context::default();
    let mut win = two_tab_window(&ctx);
    let id_a = win.tabs[0].identity;
    let id_b = win.tabs[1].identity;

    let view_before = win.view.clone();

    // 对 tab 0 依次：打开 → 填查询 → grep -v → 大小写 → 正则。
    win.apply_filter_on(id_a, FilterAction::Open);
    win.apply_filter_on(id_a, FilterAction::SetQuery("ERR".to_string()));
    win.apply_filter_on(id_a, FilterAction::SetMode(FilterMode::DropMatching));
    win.apply_filter_on(id_a, FilterAction::ToggleCase);
    win.apply_filter_on(id_a, FilterAction::ToggleRegex);

    let a = &win.tabs[0].filter;
    assert!(a.visible, "tab 0 过滤栏打开");
    assert_eq!(a.query, "ERR");
    assert_eq!(a.mode, FilterMode::DropMatching);
    assert!(a.case_sensitive, "大小写切换只影响 tab 0");
    assert!(a.regex, "正则切换只影响 tab 0");
    assert_eq!(win.tabs[0].filter_input, "ERR", "输入缓冲归 tab 0");

    // tab 1 完全没动。
    assert_eq!(
        win.tabs[1].filter,
        FilterState::default(),
        "tab 1 不受 tab 0 过滤影响"
    );
    assert_eq!(win.tabs[1].filter_input, "", "tab 1 输入缓冲不串味");
    assert_ne!(id_a, id_b);

    // `Action::Filter` 不写 `ViewState`。
    assert_eq!(win.view, view_before, "过滤不写 ViewState");

    // 对 tab 1 落地独立的过滤，互不覆盖。
    win.apply_filter_on(id_b, FilterAction::Open);
    win.apply_filter_on(id_b, FilterAction::SetQuery("noise".to_string()));
    assert_eq!(win.tabs[0].filter.query, "ERR", "tab 0 查询不被 tab 1 覆盖");
    assert_eq!(win.tabs[1].filter.query, "noise", "tab 1 有自己的查询");
    assert_eq!(
        win.tabs[1].filter.mode,
        FilterMode::KeepMatching,
        "tab 1 默认 grep"
    );
    assert_eq!(
        win.tabs[0].filter.mode,
        FilterMode::DropMatching,
        "tab 0 仍是 grep -v"
    );
}

/// 反转开关默认未激活；点击那一格派发的 UI 命令把**本会话**切到 DropMatching。
#[test]
fn invert_starts_off_and_click_command_drops_matches() {
    let ctx = egui::Context::default();
    let mut win = two_tab_window(&ctx);
    let id = win.tabs[0].identity;
    assert_eq!(
        win.tabs[0].filter.mode,
        FilterMode::KeepMatching,
        "Invert 默认未激活"
    );

    win.apply_filter_on(id, FilterAction::Open);
    win.apply_filter_on(
        id,
        term_app::filter::ui_command(term_app::filter::FilterUiCommand::ToggleMode),
    );
    assert_eq!(
        win.tabs[0].filter.mode,
        FilterMode::DropMatching,
        "点击后为 DropMatching（grep -v）"
    );
    // 分屏另一片不受影响。
    assert_eq!(win.tabs[1].filter.mode, FilterMode::KeepMatching);
}

/// `apply_filter`（`Effect::Filter` 落地）作用于**活动 tab**，不是全局。
#[test]
fn effect_filter_applies_to_active_tab_only() {
    let ctx = egui::Context::default();
    let mut win = two_tab_window(&ctx);
    let id_a = win.tabs[0].identity;
    win.active = Some(id_a);
    // 走 `Action::Filter` → `Effect::Filter` → `apply_filter`，不写 `ViewState`。
    win.dispatch_action(term_app::action::Action::Filter(FilterAction::Open));
    win.dispatch_action(term_app::action::Action::Filter(FilterAction::SetQuery(
        "x".to_string(),
    )));
    assert!(win.tabs[0].filter.visible && win.tabs[0].filter.query == "x");
    assert_eq!(win.tabs[1].filter, FilterState::default());
}

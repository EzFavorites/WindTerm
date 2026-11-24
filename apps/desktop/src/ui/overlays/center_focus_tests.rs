//! 专注模式页的单测：按钮派发的 Action 与回显的当前态。
//!
//! 这些断言走的是绘制用的同一份纯行表（`window_mode_rows` / `chrome_rows`），
//! 不是测试里另写一份期望动作——否则页面换成别的命令，测试仍会绿。

use super::*;
use term_app::shell::ShellAction;

fn all_off() -> FocusFlags {
    FocusFlags {
        focus_mode: false,
        full_screen: false,
        always_on_top: false,
        show_status_bar: false,
        show_quick_bar: false,
        show_toolbar: false,
        show_sidebar: false,
    }
}

fn sh(action: ShellAction) -> Action {
    Action::Shell(action)
}

#[test]
fn window_mode_buttons_dispatch_existing_shell_actions() {
    let actions: Vec<_> = window_mode_rows(all_off())
        .into_iter()
        .map(|row| row.action)
        .collect();
    assert_eq!(
        actions,
        vec![
            sh(ShellAction::ToggleFocusMode),
            sh(ShellAction::ToggleFullScreen),
            sh(ShellAction::ToggleAlwaysOnTop),
        ]
    );
}

#[test]
fn hidden_part_buttons_dispatch_existing_shell_actions() {
    let actions: Vec<_> = chrome_rows(all_off())
        .into_iter()
        .map(|row| row.action)
        .collect();
    assert_eq!(
        actions,
        vec![
            sh(ShellAction::ToggleStatusBar),
            sh(ShellAction::ToggleQuickBar),
            sh(ShellAction::ToggleSidebar),
            sh(ShellAction::ToggleToolbar),
        ]
    );
}

#[test]
fn rows_echo_the_current_flags_not_a_fixed_checkmark() {
    let f = FocusFlags {
        focus_mode: true,
        full_screen: true,
        always_on_top: false,
        show_status_bar: false,
        show_quick_bar: true,
        show_toolbar: false,
        show_sidebar: true,
    };
    let modes: Vec<_> = window_mode_rows(f).into_iter().map(|r| r.on).collect();
    assert_eq!(modes, vec![true, true, false]);
    let parts: Vec<_> = chrome_rows(f).into_iter().map(|r| r.on).collect();
    assert_eq!(parts, vec![false, true, true, false]);
}

#[test]
fn focus_flags_are_read_from_the_view_snapshot() {
    let sessions: Vec<SessionConfig> = vec![];
    let groups: Vec<String> = vec![];
    let tabs: Vec<Tab> = vec![];
    let tools: Vec<ToolButton> = vec![];
    let view = View {
        sessions: &sessions,
        groups: &groups,
        tabs: &tabs,
        tools: &tools,
        selection_text: "",
        theme_name: "test",
        scheme_idx: 0,
        focus_mode: true,
        full_screen: false,
        always_on_top: true,
        show_status_bar: false,
        show_quick_bar: true,
        show_toolbar: false,
        show_sidebar: true,
        default_term: 0,
        default_font_size: 12.0,
        active_tab: 0,
    };
    assert_eq!(
        FocusFlags::from(&view),
        FocusFlags {
            focus_mode: true,
            full_screen: false,
            always_on_top: true,
            show_status_bar: false,
            show_quick_bar: true,
            show_toolbar: false,
            show_sidebar: true,
        }
    );
}

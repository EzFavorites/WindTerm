//! `confirm.rs` 的单测（`#[path]` 挂在 `confirm` 模块下，私有项直接可测）。

use super::*;

#[test]
fn the_two_kinds_map_to_their_reference_texts() {
    // 视觉基线原文就是 key：关窗口两行、关会话一行。谁改了文案，
    // 这里立刻显形（中文对照在 i18n.rs 的表里）。
    let (w1, w2) = ConfirmClose::Window.message_lines();
    assert_eq!(w1, "One or more sessions are opening,");
    assert_eq!(w2, Some("Do you want to close the current window?"));
    let (s1, s2) = ConfirmClose::Tab(3).message_lines();
    assert_eq!(s1, "Do you want to close the current session?");
    assert_eq!(s2, None);
    let (a1, a2) = ConfirmClose::AllTabs.message_lines();
    assert_eq!(a1, s1);
    assert_eq!(a2, None);
}

#[test]
fn the_dialog_is_centered_in_the_golden_window() {
    // 视觉基线主窗口 1280×760：关窗口那张（176 高）居中落点必须是
    // (440, 292)。夹具与交互共用这个矩形，位置一漂两张基线一起红。
    let r = win_rect(ConfirmClose::Window);
    assert_eq!(r.min, egui::pos2(440.0, 292.0));
    assert_eq!(r.max, egui::pos2(840.0, 468.0));
    // 单行那张矮 14，居中后上下各让 7。
    let s = win_rect(ConfirmClose::Tab(0));
    assert_eq!(s.min.y, 299.0);
    assert_eq!(s.height(), 162.0);
}

#[test]
fn default_confirm_state_is_quiet_and_unsuppressed() {
    // 出厂态：没有弹窗、不抑制任何一路。两个抑制位从 prefs 读回，
    // 默认值必须与 Prefs::default() 对得上。
    let s = ConfirmState::default();
    assert_eq!(s.pending, None);
    assert!(!s.check);
    assert!(!s.close_confirmed);
    assert!(!s.suppress_window);
    assert!(!s.suppress_session);
    // 结构体默认的 `false` 会让「忘了从 prefs 读回」悄悄变成「关多标签不问」，
    // 所以 `Default` 手写成与出厂 prefs 一致的 `true`。
    assert!(s.multi_tabs);
    let p = term_runtime::prefs::Prefs::default();
    let from_prefs = ConfirmState::from_prefs(&p);
    assert_eq!(from_prefs.pending, None);
    assert!(!from_prefs.suppress_window && !from_prefs.suppress_session);
    assert!(from_prefs.multi_tabs, "出厂就问「关多标签」，与 prefs 默认一致");
}

#[test]
fn closing_one_tab_always_asks_unless_the_user_suppressed_it() {
    // 只剩一条标签时**不看** `multi`：那个字段说的是 multiple，拿它去管最后
    // 一条等于偷偷减掉现有的确认网。
    assert!(should_confirm_close(1, false, false));
    assert!(should_confirm_close(1, false, true));
    assert!(!should_confirm_close(1, true, false));
    assert!(!should_confirm_close(1, true, true));
}

#[test]
fn the_multi_tab_switch_only_governs_closes_that_are_really_multiple() {
    // 两条以上：`multi = true` 才问。
    assert!(should_confirm_close(3, false, true));
    assert!(!should_confirm_close(3, false, false));
    // 「不再提示」优先于开关本身——用户亲手勾的那一位是最后一道闸。
    assert!(!should_confirm_close(3, true, true));
    assert!(!should_confirm_close(3, true, false));
    // 边界就是 1/2：`> 1` 不是 `>= 1`。
    assert!(should_confirm_close(2, false, true));
    assert!(!should_confirm_close(2, false, false));
}

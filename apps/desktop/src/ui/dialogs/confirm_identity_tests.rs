//! 关闭确认的**恒在身份**判据（由 `#[path]` 挂回 `confirm.rs`）。
//!
//! 审查发现：缓存标签下标后，脚本在弹窗挂着时开一条新标签，Yes 关掉的是
//! 另一条会话。这里用两个真标签 + 中间插一条，验「按身份反查」确实关对人。

use super::*;
use crate::ui::lifecycle_test_support::{fake_tab, window_with, CloseCount};

#[test]
fn a_pending_tab_close_survives_an_insert_in_front() {
    let ctx = egui::Context::default();
    let pool = std::sync::Arc::new(term_session::SessionPool::new());
    let closes = CloseCount::default();
    let first = fake_tab(&pool, &ctx, 0, &closes);
    let second = fake_tab(&pool, &ctx, 1, &closes);
    let second_id = second.identity;
    let mut win = window_with(pool.clone(), &ctx, vec![first, second]);

    // 用户点第二条标签的 ×：存的是身份，不是下标 1。
    win.confirm.pending = tab_at(&win.tabs, 1);
    assert_eq!(win.confirm.pending, Some(ConfirmClose::Tab(second_id)));

    // 弹窗还挂着，脚本在前面插了一条标签——现在第二条是别人的了。
    let inserted = fake_tab(&pool, &ctx, 2, &closes);
    win.tabs.insert(0, inserted);
    assert_ne!(win.tabs[1].identity, second_id, "下标 1 已经换了人");

    // Yes：按身份反查，关掉的仍是用户当时点的那一条。
    win.confirm_resolve(true);
    assert!(
        !win.tabs.iter().any(|t| t.identity == second_id),
        "身份对应的标签没被关掉"
    );
    assert_eq!(win.tabs.len(), 2, "插进来的那条和原来的第一条都还在");
}

#[test]
fn a_vanished_identity_resolves_to_no_close() {
    let ctx = egui::Context::default();
    let pool = std::sync::Arc::new(term_session::SessionPool::new());
    let closes = CloseCount::default();
    let tab = fake_tab(&pool, &ctx, 0, &closes);
    let mut win = window_with(pool, &ctx, vec![tab]);

    // 身份已被别处关掉：Yes 不能按过期下标误伤另一条。
    win.confirm.pending = Some(ConfirmClose::Tab(u64::MAX));
    win.confirm_resolve(true);
    assert_eq!(win.tabs.len(), 1);
}

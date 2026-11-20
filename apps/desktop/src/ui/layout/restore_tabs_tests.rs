//! `replace_tabs` 的会话生命周期回归单测（第二刀之二）。
//!
//! 缺陷形态：`restore_layout` 用 `self.tabs = tabs;` 整份替换标签数组。构造窗口时
//! （`ui/construct.rs:41-48`）已经起了一条活本地 shell，直接赋值把它连同它的 PTY、
//! 监管者线程一起丢掉而不 close。现在先 `Tab::close()` 收口旧标签，再整份替换。
//!
//! 这里**不调** `restore_layout()` / `layout_state::load()`：那会读写真实 profile
//! （`WINDTERM_PROFILE_DIR` 是进程级全局），会污染并行跑的其它测试。本文件直接测
//! 替换这一步本身。证伪方式见 `/tmp/falsify-p01b.log`。

use super::*;
use crate::ui::lifecycle_test_support::{
    assert_gone, eventually, fake_tab, real_shell_tab, window_with, CloseCount,
};

/// 旧的两条标签（一条真本地 shell + 一条假通道）在整份替换时必须先被关掉，
/// 新标签照常进数组、会话照常在册。
#[test]
fn replacing_the_tab_list_closes_every_old_session_first() {
    let pool = std::sync::Arc::new(term_session::SessionPool::new());
    let ctx = egui::Context::default();
    let closes = CloseCount::default();

    let old_real = real_shell_tab(&pool, &ctx, 0);
    let old_fake = fake_tab(&pool, &ctx, 1, &closes);
    let real_id = old_real.session.expect("真本地 shell 会话应当起来了");
    let fake_id = old_fake.session.expect("注入的假通道必须给出会话 id");
    assert!(pool.state(real_id).is_some(), "替换前旧真会话必须在册");
    assert!(pool.send(fake_id, b"x"), "替换前旧假会话必须在册");

    let mut win = window_with(pool.clone(), &ctx, vec![old_real, old_fake]);

    let new_tab = real_shell_tab(&pool, &ctx, 2);
    let new_id = new_tab.session.expect("新真本地 shell 会话应当起来了");
    win.replace_tabs(vec![new_tab]);

    // 两条旧会话都必须从控制面除名（旧实现这里两条都还在册）。
    assert_gone(&pool, real_id, "replace_tabs 旧真会话");
    assert_gone(&pool, fake_id, "replace_tabs 旧假会话");
    eventually("旧假通道 close() 被调用一次", || closes.get() == 1);
    // 新标签在数组里、会话仍在册。
    assert_eq!(win.tabs.len(), 1);
    assert_eq!(win.tabs[0].session, Some(new_id));
    assert!(pool.state(new_id).is_some(), "新标签的会话必须还在册");
    assert!(pool.send(new_id, b"x"), "新标签的会话仍应可写");

    win.tabs[0].close();
}

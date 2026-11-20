//! `delete_session_config` 的会话生命周期回归单测（第二刀之一）。
//!
//! 缺陷形态：删除会话配置时旧代码只做 `self.tabs.retain(|t| t.config.name != name)`，
//! 直接把标签从 `Vec` 里丢掉。`Tab` 只持一个 `SessionId`（Copy），丢掉它什么都不
//! 会发生——`SessionPool` 的 Entry 不会被 remove，PTY、监管者线程、tee 槽位一直留着。
//! 现在必须先 `Tab::close()`（先摘 tee 再 `pool.close`）再 `retain`。
//!
//! 夹具来自 `crate::ui::lifecycle_test_support`（与 `tab_strip` / `restore` 的回归
//! 单测共用同一份）。证伪方式见 `/tmp/falsify-p01b.log`：把 `close()` 循环还原成
//! 旧代码后本文件必挂。

use super::*;
use crate::ui::lifecycle_test_support::{
    assert_gone, eventually, fake_tab, real_shell_tab, window_with, CloseCount,
};

/// 两条同名标签（同一会话开两个分屏）+ 一条异名标签：删配置只收口同名那两条，
/// 异名那条的会话与标签都必须原样留着。
#[test]
fn deleting_a_session_closes_every_tab_of_that_name_and_leaves_others() {
    let pool = std::sync::Arc::new(term_session::SessionPool::new());
    let ctx = egui::Context::default();
    let first = CloseCount::default();
    let second = CloseCount::default();

    let mut a = fake_tab(&pool, &ctx, 0, &first);
    a.config.name = "shared".to_string();
    let mut b = fake_tab(&pool, &ctx, 1, &second);
    b.config.name = "shared".to_string();
    let mut other = real_shell_tab(&pool, &ctx, 2);
    other.config.name = "other".to_string();

    let a_id = a.session.expect("注入的假通道必须给出会话 id");
    let b_id = b.session.expect("第二条注入通道必须给出会话 id");
    let other_id = other.session.expect("真本地 shell 会话应当起来了");
    assert!(pool.send(a_id, b"x"), "删除前 a 必须在册");
    assert!(pool.send(b_id, b"x"), "删除前 b 必须在册");
    assert!(pool.state(other_id).is_some(), "删除前异名会话必须在册");

    let mut win = window_with(pool.clone(), &ctx, vec![a, b, other]);
    let mut shared_cfg = SessionConfig::new(Protocol::Shell);
    shared_cfg.name = "shared".to_string();
    let mut other_cfg = SessionConfig::new(Protocol::Shell);
    other_cfg.name = "other".to_string();
    win.sessions = vec![shared_cfg, other_cfg];

    win.delete_session_config(0);

    // 两条同名标签的会话都被收口（旧实现这里两条都还在册）。
    assert_gone(&pool, a_id, "删除同名标签 a");
    assert_gone(&pool, b_id, "删除同名标签 b");
    eventually("第一条假通道 close() 被调用一次", || {
        first.get() == 1
    });
    eventually("第二条假通道 close() 被调用一次", || {
        second.get() == 1
    });
    // 异名标签与它的配置原样留着。
    assert!(pool.state(other_id).is_some(), "异名会话不该被关");
    assert!(pool.send(other_id, b"x"), "异名会话仍应可写");
    assert_eq!(win.tabs.len(), 1, "只该剩异名那一条");
    assert_eq!(win.tabs[0].config.name, "other");
    assert_eq!(win.sessions.len(), 1);
    assert_eq!(win.sessions[0].name, "other");

    win.tabs[0].close();
}

/// 越界下标：旧实现 `unwrap_or_default()` 会得到空名字，再用它 `retain` 会把所有
/// 空名标签一并丢掉。新实现 `sessions.get(i)` 取不到就返回，什么都不做。
#[test]
fn an_out_of_range_delete_does_not_drop_unnamed_tabs() {
    let pool = std::sync::Arc::new(term_session::SessionPool::new());
    let ctx = egui::Context::default();
    let closes = CloseCount::default();
    let mut tab = fake_tab(&pool, &ctx, 0, &closes);
    tab.config.name = String::new();
    let id = tab.session.expect("注入的假通道必须给出会话 id");
    assert!(pool.send(id, b"x"));

    let mut win = window_with(pool.clone(), &ctx, vec![tab]);
    win.sessions = Vec::new();

    win.delete_session_config(0);

    assert_eq!(win.tabs.len(), 1, "越界删除不该丢标签");
    assert!(pool.state(id).is_some(), "越界删除不该关会话");
    assert_eq!(closes.get(), 0, "越界删除不该关通道");

    win.tabs[0].close();
    eventually("收尾关闭", || closes.get() == 1);
}

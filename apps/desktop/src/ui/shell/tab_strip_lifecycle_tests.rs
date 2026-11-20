//! `close_tab` / `close_all_tabs` 的会话生命周期回归单测（第一刀）。
//!
//! 缺陷形态：`close_tab` 曾经只把 `Tab` 从 `Vec` 里 `remove` 掉，`close_all_tabs`
//! 只 `clear()`。会话句柄在 [`term_session::SessionPool`] 里，丢掉一个
//! `SessionId` 什么都不会发生——孤儿 PTY / SSH、没摘的 tee、没 `finish()` 的日志
//! 一起留下。这两处现在必须走 [`Tab::close`]（先 `detach_tap` 再 `pool.close`）。
//!
//! 夹具（完整 `TerminalWindow` / 真本地 shell / 可观察假通道 / `assert_gone` /
//! `eventually`）在 [`crate::ui::lifecycle_test_support`]，与 `sidebar_view_tests`、
//! `restore_tabs_tests` 共用同一份，避免三份复制各自漂移。
//!
//! # 断言为什么不是恒真的
//!
//! 旧实现下这些测试**都会挂**：`pool.send(old_id, ..)` 仍为 `true`、
//! `pool.screen(old_id)` 仍是 `Some`、假通道的 `close()` 一次都不会被调用、
//! tee 探针仍连着（`try_recv` 给 `Empty` 而不是 `Disconnected`）。
//!
//! `close_tab` 内部把 `Tab` 取出所有权、`close()` 之后就丢掉了，测试拿不到那个
//! `Tab` 再读字段，所以 `RawLog.tap.is_none()` / `session.is_none()` 放在
//! [`tab_close_detaches_the_hex_tap_takes_the_session_and_closes_the_channel`]
//! 里对 `Tab::close()` 直接断言；`close_tab` / `close_all_tabs` 两条则用控制面上
//! 可观察的效果证伪旧实现。

use super::*;
use crate::ui::lifecycle_test_support::{
    assert_gone, eventually, fake_tab, real_shell_tab, window_with, CloseCount,
};

/// `Tab::close` 本身的三个可观察事实：摘 tee、take session、关掉池里的会话。
/// 这条同时覆盖「hex tap 已摘」——`close_tab` 里那个 `Tab` 被取走所有权后
/// 立即丢弃，字段只有在直接持有 `Tab` 时读得到。
#[test]
fn tab_close_detaches_the_hex_tap_takes_the_session_and_closes_the_channel() {
    let pool = std::sync::Arc::new(term_session::SessionPool::new());
    let ctx = egui::Context::default();
    let closes = CloseCount::default();
    let mut tab = fake_tab(&pool, &ctx, 0, &closes);

    let id = tab.session.expect("注入的假通道必须给出会话 id");
    assert!(tab.raw.tap.is_some(), "建标签时无条件挂 tee");
    assert!(pool.send(id, b"x"), "关闭前会话必须在册且可写");
    assert_eq!(closes.get(), 0, "关闭前通道不该被关过");

    tab.close();

    assert!(tab.raw.tap.is_none(), "close 必须摘掉 hex tee");
    assert!(tab.session.is_none(), "close 必须 take 掉 session");
    assert_gone(&pool, id, "Tab::close");
    eventually("假通道 close() 被调用一次", || closes.get() == 1);
}

/// 真本地 shell：`close_tab(0)` 之后旧会话从 `SessionPool` 消失，另一条不受影响，
/// 激活下标 / 布局 / 颜色表 / 焦点历史这些既有行为逐项不变。
#[test]
fn close_tab_removes_the_live_local_shell_session_from_the_pool() {
    let pool = std::sync::Arc::new(term_session::SessionPool::new());
    let ctx = egui::Context::default();
    let tabs = vec![real_shell_tab(&pool, &ctx, 0), real_shell_tab(&pool, &ctx, 1)];
    let old_id = tabs[0].session.expect("真本地 shell 会话应当起来了");
    let keep_id = tabs[1].session.expect("第二条真本地 shell 会话应当起来了");
    let closed_tab_id = tabs[0].id;
    let keep_tab_id = tabs[1].id;
    assert!(pool.screen(old_id).is_some(), "关闭前会话必须在册");
    assert!(tabs[0].raw.tap.is_some(), "建标签时无条件挂 tee");

    let mut win = window_with(pool.clone(), &ctx, tabs);
    win.tab_colors = vec![Some(2), Some(5)];
    win.tab_focus = vec![closed_tab_id, keep_tab_id];

    win.close_tab(0);

    assert_gone(&pool, old_id, "close_tab");
    // 只关一条：另一条仍在册、tee 仍挂着。
    assert!(pool.send(keep_id, b"x"), "另一条会话不该被连带关掉");
    assert!(win.tabs[0].raw.tap.is_some(), "剩下的标签 tee 不该被摘");
    // 既有行为逐项不变。
    assert_eq!(win.tabs.len(), 1);
    assert_eq!(win.active_slot(), 0, "关掉第 0 条后落在第 0 条");
    assert!(win.layout.is_single(), "布局仍是单窗格");
    assert_eq!(win.tab_colors, vec![Some(5)], "颜色表跟着下标左移");
    assert_eq!(win.tab_focus, vec![keep_tab_id], "焦点历史摘掉被关的 id");

    win.tabs[0].close();
}

/// 退出路径用的 `close_all_sessions()`：会话全收口，但**不**补 fallback。
///
/// `close_all_tabs()` 末尾那句 `ensure_tab()` 是给「Close All Views」用的——界面上
/// 要留一个能继续敲的窗格。退出时补一条本地 shell 等于「刚 fork 一个进程就为它
/// 送终」，所以 `main.rs::on_exit` 走这一条。两条路径的差别就是这一个断言：
/// `close_all_tabs` 之后 `tabs.len() == 1`（见上一条），这里必须是 0。
///
/// `on_exit` 本体没法在单测里构造（要 `eframe::CreationContext` + `Frame`），
/// 所以把它委托的这一步单独钉住。
#[test]
fn close_all_sessions_closes_every_session_and_leaves_no_fallback() {
    let pool = std::sync::Arc::new(term_session::SessionPool::new());
    let ctx = egui::Context::default();
    let tabs: Vec<Tab> = (0..2).map(|i| real_shell_tab(&pool, &ctx, i)).collect();
    let old_ids: Vec<term_core::session_runtime::SessionId> = tabs
        .iter()
        .map(|t| t.session.expect("真本地 shell 会话应当起来了"))
        .collect();

    let mut win = window_with(pool.clone(), &ctx, tabs);
    win.close_all_sessions();

    for id in &old_ids {
        assert_gone(&pool, *id, "close_all_sessions");
    }
    assert!(win.tabs.is_empty(), "退出路径不该补 fallback 标签");
    assert_eq!(win.active_slot(), 0);
    assert!(win.layout.is_single(), "布局仍回到单窗格");
    assert!(win.tab_colors.is_empty());
}

/// 真本地 shell × 3：`close_all_tabs()` 之后三条旧会话全部从池中除名，
/// 且 `ensure_tab()` 补出的 fallback 本地 shell 是**可用**的（在册、可写、挂了 tee）。
#[test]
fn close_all_tabs_removes_every_live_session_and_ensure_tab_stays_usable() {
    let pool = std::sync::Arc::new(term_session::SessionPool::new());
    let ctx = egui::Context::default();
    let tabs: Vec<Tab> = (0..3).map(|i| real_shell_tab(&pool, &ctx, i)).collect();
    let old_ids: Vec<term_core::session_runtime::SessionId> = tabs
        .iter()
        .map(|t| t.session.expect("真本地 shell 会话应当起来了"))
        .collect();
    for id in &old_ids {
        assert!(pool.screen(*id).is_some(), "关闭前会话必须在册");
    }

    let mut win = window_with(pool.clone(), &ctx, tabs);
    win.close_all_tabs();

    for id in &old_ids {
        assert_gone(&pool, *id, "close_all_tabs");
    }
    // fallback：ensure_tab 补出的是一条**新**本地 shell，不是旧会话的残留。
    assert_eq!(win.tabs.len(), 1, "关空后必须补一条");
    assert_eq!(win.active_slot(), 0);
    assert!(win.layout.is_single(), "关空后布局回到单窗格");
    assert!(win.tab_colors.is_empty());
    assert_eq!(win.tabs[0].config.protocol, Protocol::Shell);
    let fallback = win.tabs[0].session.expect("fallback 本地 shell 应当起来了");
    assert!(!old_ids.contains(&fallback), "fallback 必须是新会话，不是旧 id");
    assert!(pool.send(fallback, b"x"), "fallback 会话必须在册且可写");
    assert!(pool.screen(fallback).is_some(), "fallback 必须给得出屏幕事实");
    assert!(win.tabs[0].raw.tap.is_some(), "fallback 也挂上了 tee");

    win.tabs[0].close();
}

/// 假通道版 `close_tab`：除池之外，还要能证伪「没摘 tee」。
///
/// 把监管者手上的 tee 生产端换成测试自己的 sender。旧实现不摘 tee、也不关会话，
/// 这条探针会一直连着（`try_recv` 给 `Empty`）；新实现 `DetachTap` 之后生产端被
/// 丢掉，`try_recv` 给 `Disconnected`。这是可证伪的，不是恒真布尔。
#[test]
fn close_tab_detaches_the_tee_and_closes_the_injected_channel() {
    let pool = std::sync::Arc::new(term_session::SessionPool::new());
    let ctx = egui::Context::default();
    let first = CloseCount::default();
    let second = CloseCount::default();
    let tabs = vec![
        fake_tab(&pool, &ctx, 0, &first),
        fake_tab(&pool, &ctx, 1, &second),
    ];
    let old_id = tabs[0].session.expect("注入的假通道必须给出会话 id");
    let keep_id = tabs[1].session.expect("第二条注入通道必须给出会话 id");
    assert!(pool.send(old_id, b"x"));
    assert!(tabs[0].raw.tap.is_some());

    let (probe_tx, probe_rx) = std::sync::mpsc::channel();
    assert!(pool.attach_tap(old_id, probe_tx), "会话在册才能挂探针");
    assert!(
        matches!(probe_rx.try_recv(), Err(std::sync::mpsc::TryRecvError::Empty)),
        "探针此刻连着、且没有字节"
    );

    let mut win = window_with(pool.clone(), &ctx, tabs);
    win.close_tab(0);

    assert_gone(&pool, old_id, "close_tab（假通道）");
    eventually("第一条假通道 close() 被调用一次", || first.get() == 1);
    eventually(
        "tee 生产端被摘（探针断开）",
        || matches!(probe_rx.try_recv(), Err(std::sync::mpsc::TryRecvError::Disconnected)),
    );
    // 只关一条：另一条通道没被关、tee 还挂着、会话还在册。
    assert_eq!(second.get(), 0, "另一条通道不该被连带关掉");
    assert!(pool.send(keep_id, b"x"));
    assert!(win.tabs[0].raw.tap.is_some());
    win.tabs[0].close();
    eventually("收尾：第二条假通道也关掉", || second.get() == 1);
}

/// 假通道版 `close_all_tabs`：三条 tee 全摘、三条通道全关，fallback 仍可用。
#[test]
fn close_all_tabs_detaches_every_tee_and_closes_every_injected_channel() {
    let pool = std::sync::Arc::new(term_session::SessionPool::new());
    let ctx = egui::Context::default();
    let counters: Vec<CloseCount> = (0..3).map(|_| CloseCount::default()).collect();
    let tabs: Vec<Tab> = counters
        .iter()
        .enumerate()
        .map(|(i, c)| fake_tab(&pool, &ctx, i, c))
        .collect();
    let ids: Vec<term_core::session_runtime::SessionId> = tabs
        .iter()
        .map(|t| t.session.expect("注入的假通道必须给出会话 id"))
        .collect();

    let mut probes = Vec::new();
    for id in &ids {
        let (probe_tx, probe_rx) = std::sync::mpsc::channel();
        assert!(pool.attach_tap(*id, probe_tx), "会话在册才能挂探针");
        probes.push(probe_rx);
        assert!(pool.send(*id, b"x"));
    }

    let mut win = window_with(pool.clone(), &ctx, tabs);
    win.close_all_tabs();

    for id in &ids {
        assert_gone(&pool, *id, "close_all_tabs（假通道）");
    }
    for (i, counter) in counters.iter().enumerate() {
        let label = format!("第 {i} 条假通道 close() 被调用一次");
        eventually(&label, || counter.get() == 1);
    }
    for (i, probe) in probes.iter().enumerate() {
        let label = format!("第 {i} 条 tee 生产端被摘（探针断开）");
        eventually(&label, || {
            matches!(probe.try_recv(), Err(std::sync::mpsc::TryRecvError::Disconnected))
        });
    }
    // fallback 本地 shell 仍然可用，没有因为收口被弄坏。
    assert_eq!(win.tabs.len(), 1);
    assert_eq!(win.active_slot(), 0);
    assert!(win.layout.is_single());
    assert_eq!(win.tabs[0].config.protocol, Protocol::Shell);
    let fallback = win.tabs[0].session.expect("fallback 本地 shell 应当起来了");
    assert!(pool.send(fallback, b"x"));
    assert!(pool.screen(fallback).is_some());
    win.tabs[0].close();
}

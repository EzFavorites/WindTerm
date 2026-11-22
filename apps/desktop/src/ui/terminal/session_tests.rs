//! ADR-0025 阶段 5 的**对账**判据：被外部关掉的会话，标签必须自己认出来。
//!
//! # 这一刀修的是什么
//!
//! `SessionPool::close` 是 `remove`——把条目从登记表里摘掉（不是打一个「已关闭」
//! 标记）。而 `Tab.session` 是 `Option<SessionId>`，只在本界面自己的关标签 / 重连
//! 路径上被改写。于是「别人关掉了这条会话」本界面**完全不知道**：
//! `is_live()` 一直为真，标签永远显示活着、画面冻结在最后一帧、敲键盘全落空
//! （`write` 对不在册的 id 返回 false）。
//!
//! 这不是理论风险：脚本与桥接的 `session_close` 关得掉用户正看着的那条会话。
//! 阶段 5 撤掉 `PooledSessions::list_only` 之后，界面里的脚本还能自己开会话、再关掉它，
//! 于是「开 → 投影成标签 → 关 → 僵尸」是一条完整可走的路径。
//!
//! # 为什么判据是 `poll` 的返回值而不是「画出来几块像素」
//!
//! 对账发生在事件泵里（`Tab::poll` 已经在碰池子），它决定的是**状态**：
//! 会话还在不在、要不要重绘。像素层的后果（标题带上 `(disconnected)`）由既有的
//! `!is_live()` 分支承担，那条分支有自己的测试。这里只钉住「认出来了没有」。

use crate::ui::lifecycle_test_support::{fake_tab, CloseCount};

/// 外部关掉之后：标签转断开、id 不再留着、且**要求重绘一次**。
#[test]
fn an_externally_closed_session_turns_the_tab_into_a_disconnected_one() {
    let ctx = egui::Context::default();
    let pool = std::sync::Arc::new(term_session::SessionPool::new());
    let closes = CloseCount::default();
    let mut tab = fake_tab(&pool, &ctx, 0, &closes);
    let id = tab.session().expect("夹具应当绑上一条会话");

    // 脚本 / 桥接的 `session_close` 走的就是这条路：从登记表里摘掉。
    assert!(pool.close(id), "关一条在册的会话必须成功");

    let mut commands = Vec::new();
    let redraw = tab.poll(&[], &mut commands, false, &[]);

    assert!(redraw, "会话没了是**新事实**，必须要求重绘一次");
    assert!(!tab.is_live(), "标签要转成断开形态，不能永远显示活着");
    assert_eq!(
        tab.session(),
        None,
        "id 已不在册，留着它只会让每一帧都空转一次查表"
    );
    assert!(
        commands.is_empty(),
        "对账是纯状态修正，不该顺带派发命令"
    );
}

/// 反面：会话还在册时对账不该误伤——否则每帧都会把活标签打成断开。
#[test]
fn a_session_that_is_still_in_the_pool_is_left_alone() {
    let ctx = egui::Context::default();
    let pool = std::sync::Arc::new(term_session::SessionPool::new());
    let closes = CloseCount::default();
    let mut tab = fake_tab(&pool, &ctx, 0, &closes);
    let id = tab.session().expect("夹具应当绑上一条会话");

    // 连跑几帧：第一帧可能真的带回新事实，之后没有——两种都不该改 `session`。
    for _ in 0..3 {
        tab.poll(&[], &mut Vec::new(), false, &[]);
        assert_eq!(tab.session(), Some(id), "在册的会话不许被对账改成断开");
        assert!(tab.is_live());
    }

    pool.close(id);
}

/// 自动日志（`Log types`）唯一认的时点：「刚刚连上」那一刻的**边沿**。
///
/// 为什么必须钉住它：拿 `log_status().phase == Off` 当代替判据也能「看起来能用」，
/// 但用户手动 `Stop` 之后那一条每帧都是 `Off`，下一帧就会被自动重开——
/// 界面与自己打架，用户还以为 Stop 坏了。边沿只来一次，这条测试就是它的锚。
#[test]
fn the_connect_edge_is_reported_once_per_connection_and_consumed() {
    let ctx = egui::Context::default();
    let pool = std::sync::Arc::new(term_session::SessionPool::new());
    let closes = CloseCount::default();
    let mut tab = fake_tab(&pool, &ctx, 0, &closes);
    let id = tab.session().expect("夹具应当绑上一条会话");

    tab.poll(&[], &mut Vec::new(), false, &[]);
    assert!(tab.take_just_connected(), "首帧是「没连过 → 连着」，必须报边沿");

    // 同一次连接里的后续帧不再报：自动日志因此只开一份。
    for _ in 0..3 {
        tab.poll(&[], &mut Vec::new(), false, &[]);
        assert!(!tab.take_just_connected(), "同一次连接只配一个边沿");
    }

    // 断开那一帧不是「连上」边沿（那是 Disconnect 触发器的活）。
    pool.close(id);
    tab.poll(&[], &mut Vec::new(), false, &[]);
    assert!(!tab.is_live());
    assert!(!tab.take_just_connected(), "断开不该被认成连上");
}

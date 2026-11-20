//! `Drop for Tab` 的会话收口回归单测（P0-1c 的 E）。
//!
//! 缺陷形态：`Tab` 只持一个 `SessionId`（`Copy`），丢掉它什么都不会发生——
//! `SessionPool` 的 Entry 不 remove，PTY、监管者线程、tee 槽位、正在写的日志
//! 文件一起留下。四条显式关闭路径（`close_tab` / `close_all_tabs` /
//! `delete_session_config` / `replace_tabs`）已经各自改成先 `close()` 再丢，
//! `Drop` 是第五层网：负责「以后谁加了第五条路径、忘了 `close()`」。
//!
//! 夹具（真本地 shell / 可观察假通道 / `assert_gone` / `eventually`）在
//! [`crate::ui::lifecycle_test_support`]，与另外三个测试文件共用同一份。
//!
//! # 断言为什么不是恒真的
//!
//! 逐条做过变异（team lead 复核，2026-11-11）：
//!
//! | 变异 | 挂掉的用例 |
//! |---|---|
//! | 清空 `Drop::drop` 体（保留空 impl） | `dropping_a_tab_closes_its_live_local_shell_session`、`dropping_a_tab_closes_the_injected_channel_too` |
//! | 只摘 `if std::thread::panicking() { return; }`（保留 `close()`） | `a_panic_in_flight_skips_the_close_so_unwinding_can_finish` |
//! | `close()` 里的 `session.take()` 改成不 take | `an_explicit_close_followed_by_drop_leaves_other_sessions_alone` |
//!
//! 前两行不是「四条全挂」，别照那句写文档：清空函数体时 panic 那条**会过**
//! （它断言的就是「panic 期间别关」，空实现正好满足），幂等那条也会过
//! （`SessionPool::close` 对不在册的 id 静默 no-op）。第三行的 `session.is_none()`
//! 断言是复核时补的——原来只量通道计数，而计数在「不 take」时仍然是 1，恒真。

use super::*;
use crate::ui::lifecycle_test_support::{
    assert_gone, eventually, fake_tab, real_shell_tab, CloseCount,
};

/// 真本地 shell：`drop(tab)` 之后会话必须从 `SessionPool` 里除名。
///
/// 这一条同时覆盖「tee 已摘」：`detach_tap` 在 `pool.close` 之前跑，若顺序反了
/// （先 `close` 再 `detach`），`detach_tap` 会因会话不在册而返回 `false`，
/// 监管者手上的 sink 要等它自己发现通道断了才清——`Drop` 里做不到等待。
#[test]
fn dropping_a_tab_closes_its_live_local_shell_session() {
    let pool = std::sync::Arc::new(term_session::SessionPool::new());
    let ctx = egui::Context::default();
    let tab = real_shell_tab(&pool, &ctx, 0);
    let id = tab.session.expect("真本地 shell 会话应当起来了");
    assert!(pool.send(id, b"x"), "drop 前会话必须在册且可写");
    assert!(tab.raw.tap.is_some(), "建标签时无条件挂 tee");

    drop(tab);

    assert_gone(&pool, id, "Drop for Tab（真本地 shell）");
}

/// 假通道：`drop(tab)` 之后监管者必须真的调用了通道的 `close()`。
///
/// 与上一条的区别：那一条证明「池里的条目没了」，这一条证明「控制面真的往下走了
/// 一步」——`pool.close` 若只是把 Entry 从表里删掉而不通知监管者，PTY 就成了孤儿，
/// 而 `send` 照样返回 `false`，上一条看不出来。
#[test]
fn dropping_a_tab_closes_the_injected_channel_too() {
    let pool = std::sync::Arc::new(term_session::SessionPool::new());
    let ctx = egui::Context::default();
    let closes = CloseCount::default();
    let tab = fake_tab(&pool, &ctx, 0, &closes);
    let id = tab.session.expect("注入的假通道必须给出会话 id");
    assert!(pool.send(id, b"x"), "drop 前会话必须在册");
    assert_eq!(closes.get(), 0, "drop 前通道不该被关过");

    drop(tab);

    assert_gone(&pool, id, "Drop for Tab（假通道）");
    eventually("假通道 close() 被调用一次", || closes.get() == 1);
}

/// 显式 `close()` 之后再 `drop`：不会牵连**别的**会话，通道也只关一次。
///
/// 这条守的是幂等：「显式 close + Drop」双保险的前提是 `close()` 可以调两次。
/// 断言取的是可观察后果——同一个池里的旁观会话必须还在册、可写；通道计数停在 1。
/// 若 `close()` 变成非幂等（例如把 `session.take()` 改成不 take），第二次调用会
/// 再发一次 `Command::Close`，计数变 2。
#[test]
fn an_explicit_close_followed_by_drop_leaves_other_sessions_alone() {
    let pool = std::sync::Arc::new(term_session::SessionPool::new());
    let ctx = egui::Context::default();
    let closes = CloseCount::default();
    let mut closing = fake_tab(&pool, &ctx, 0, &closes);
    let closing_id = closing.session.expect("注入的假通道必须给出会话 id");

    // 旁观者：同一个池里的另一条真会话，Drop 绝不该碰它。
    let bystander = real_shell_tab(&pool, &ctx, 1);
    let bystander_id = bystander.session.expect("真本地 shell 会话应当起来了");

    closing.close();
    assert_gone(&pool, closing_id, "显式 close");
    eventually("通道 close() 被调用一次", || closes.get() == 1);
    // 幂等的**机制**断言：`close()` 必须把 id 置空。只靠「通道计数仍是 1」看不出
    // 这件事——`SessionPool::close` 对不在册的 id 是静默 no-op，所以把 `take()`
    // 改成不 take 之后计数照样是 1，断言全绿而幂等已经没了（实测过）。
    assert!(
        closing.session.is_none(),
        "close 之后 session 必须被 take 走，否则第二次 close 会重复下发"
    );

    drop(closing);

    assert!(
        pool.send(bystander_id, b"x"),
        "Drop 不该关掉同一个池里的其它会话"
    );
    assert_eq!(closes.get(), 1, "通道只该被关一次，Drop 不该再关一次");
    drop(bystander);
}

/// panic 展开期间 `Drop` 必须**放弃**收口，否则会二次 panic → `abort`。
///
/// 做法：把标签移进一个子线程、在子线程里 panic。展开时 `_tab` 的 `Drop` 跑在
/// `std::thread::panicking() == true` 的上下文里，于是直接返回、不碰控制面。
/// `join()` 拿到 `Err` 之后，本线程再断言「那条会话**仍然**在册」——
/// 这是可证伪的：把 `if std::thread::panicking() { return; }` 去掉，
/// `pool.close(id)` 就会返回 `false`（Drop 已经把它关掉了），本测试立刻挂。
///
/// 为什么用子线程而不是 `catch_unwind`：`Tab` 里带 `Cell`，`catch_unwind` 要
/// `AssertUnwindSafe` 才编得过，而 `AssertUnwindSafe` 只是「把编译器按住」，
/// 断言的可信度不如「panic 真的发生在另一个线程里、由 `join` 交回 `Err`」。
#[test]
fn a_panic_in_flight_skips_the_close_so_unwinding_can_finish() {
    let pool = std::sync::Arc::new(term_session::SessionPool::new());
    let ctx = egui::Context::default();
    let tab = real_shell_tab(&pool, &ctx, 0);
    let id = tab.session.expect("真本地 shell 会话应当起来了");
    assert!(pool.send(id, b"x"), "panic 前会话必须在册");

    let child_pool = pool.clone();
    let joined = std::thread::spawn(move || {
        let _tab = tab;
        // 展开期间 `_tab` 被 drop；`Drop::drop` 看到 `panicking() == true` 直接返回。
        panic!("模拟 Drop 期间的 panic 展开");
    })
    .join();

    assert!(joined.is_err(), "子线程应当 panic，join 必须交回 Err");
    assert!(
        child_pool.send(id, b"x"),
        "panic 展开期间的 Drop 必须放弃收口：会话应当仍在册"
    );
    assert!(
        child_pool.close(id),
        "会话仍在册（若已被 Drop 关掉，这里会返回 false）"
    );
}

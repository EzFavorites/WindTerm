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
use std::sync::{Arc, Mutex};
use term_core::session::SessionConfig;

/// 把触发器写回会话的字节记下来，用来判定 Send Later 到底发没发。
#[derive(Clone, Default)]
struct Writes(Arc<Mutex<Vec<u8>>>);

struct WriteChannel {
    writes: Writes,
    finished: bool,
}

impl term_core::channel::TermChannel for WriteChannel {
    fn drain(&mut self) -> Vec<u8> {
        Vec::new()
    }
    fn write(&mut self, bytes: &[u8]) {
        self.writes.0.lock().expect("写入记录锁").extend_from_slice(bytes);
    }
    fn resize(&mut self, _cols: usize, _rows: usize) {}
    fn poll(&mut self) -> Option<String> {
        None
    }
    fn finished(&self) -> bool {
        self.finished
    }
    fn close(&mut self) {
        self.finished = true;
    }
    fn description(&self) -> &str {
        "write recorder"
    }
}

/// 触发器 Send text 里字面 Windows 路径必须往返一致；显式转义仍然生效。
#[test]
fn send_text_round_trips_windows_paths_and_explicit_escapes() {
    assert_eq!(
        super::session::decode_send_text(r"C:\Windows\system32\drivers\etc\hosts").expect("字面路径"),
        br"C:\Windows\system32\drivers\etc\hosts",
        "未知转义不能吞掉反斜杠"
    );
    // 末尾孤立反斜杠也要发出去，不能让整条消息静默消失。
    assert_eq!(super::session::decode_send_text("tail\\").expect("字面反斜杠"), br"tail\");
    // 明确写了表内转义时仍按原版口径解码（Send text 的关键用途）。
    assert_eq!(super::session::decode_send_text(r"\x03").expect("\\x03"), b"\x03");
    assert_eq!(super::session::decode_send_text(r"\003").expect("\\003"), b"\x03");
    assert_eq!(super::session::decode_send_text(r"\q").expect("未知转义按字面"), br"\q");
}

/// 显式转义写错必须报错，绝不能静默回退成原样字节（那会把 `\x0` 当成文本发出）。
#[test]
fn bad_explicit_escape_is_an_error_not_a_silent_fallback() {
    let err = super::session::decode_send_text(r"\x0").expect_err("写错的转义必须拒绝");
    assert_eq!(err.at, 0);
    assert_eq!(err.reason, "\\x 需要两位十六进制");
    // 非 ASCII 的 \c / \m 同样被 send_escapes 拦下，不写出垃圾字节。
    assert!(super::session::decode_send_text(r"\m你").is_err());
}

/// 与 Sender 一致：显式转义写错不静默 no-op，错误经 `poll_with_note` 上报到
/// 窗口状态行提示槽，且一个字节都不写进 PTY。
#[test]
fn a_bad_trigger_escape_reaches_the_note_slot_and_writes_nothing() {
    let ctx = egui::Context::default();
    let pool = Arc::new(term_session::SessionPool::new());
    let writes = Writes::default();
    let ch = WriteChannel {
        writes: writes.clone(),
        finished: false,
    };
    let mut tab = crate::ui::terminal::make_tab_with_channel(
        &pool,
        term_core::session::SessionConfig::new(term_core::session::Protocol::Shell),
        Box::new(ch),
        egui::Id::new(("bad-escape", 0)),
        &ctx,
    );
    let (tx, rx) = std::sync::mpsc::channel();
    tab.raw.attach(crate::hex_tap::HexTap::new(rx));

    let mut rule = term_core::trigger::Rule::default();
    rule.id = "bad-escape".into();
    rule.enabled = true;
    rule.on = term_core::trigger::TriggerOn::Output;
    rule.pattern = "wake".into();
    rule.literal = true;
    rule.config.send.condition = term_core::trigger::SendCondition::AfterMatch;
    rule.config.send.later_secs = "0".into();
    rule.actions = vec![term_core::trigger::TriggerAction::SendText(r"\x0".into())];
    let rules = [rule];

    super::session::set_test_now(Some(10));
    tx.send(b"wake".to_vec()).expect("送一次远端输出");
    let mut note = None;
    tab.poll_with_note(&rules, &mut Vec::new(), false, &[], &std::collections::HashMap::new(), &mut note);
    let note = note.expect("转义写错必须上报，不许静默吞掉");
    assert!(note.contains("触发器 bad-escape 的 Send text 转义写错"), "note={note:?}");
    assert!(note.contains("\\x 需要两位十六进制"), "note={note:?}");
    // 解码失败的那一路没有任何 pool.send，等监管者一轮也不该冒出字节。
    std::thread::sleep(std::time::Duration::from_millis(100));
    assert!(writes.0.lock().expect("读写入记录").is_empty(), "写错的转义一个字节都不能发出去");
    super::session::set_test_now(None);
}

/// Send Later 的定时发送不能靠远端继续输出来驱动：命中之后远端安静 2s，
/// 到期那一帧必须由 `poll` 的空闲段补一次 `due` 才发得出去。
#[test]
fn send_later_fires_after_the_remote_goes_quiet_for_two_seconds() {
    let ctx = egui::Context::default();
    let pool = Arc::new(term_session::SessionPool::new());
    let writes = Writes::default();
    let ch = WriteChannel {
        writes: writes.clone(),
        finished: false,
    };
    let mut tab = crate::ui::terminal::make_tab_with_channel(
        &pool,
        term_core::session::SessionConfig::new(term_core::session::Protocol::Shell),
        Box::new(ch),
        egui::Id::new(("send-later-quiet", 0)),
        &ctx,
    );
    // 输出从假 tap 进本帧，不依赖监管者线程时序。
    let (tx, rx) = std::sync::mpsc::channel();
    tab.raw.attach(crate::hex_tap::HexTap::new(rx));

    let mut rule = term_core::trigger::Rule::default();
    rule.id = "later".into();
    rule.enabled = true;
    rule.on = term_core::trigger::TriggerOn::Output;
    rule.pattern = "wake".into();
    rule.literal = true;
    rule.config.send.later_secs = "2".into();
    rule.actions = vec![term_core::trigger::TriggerAction::SendText("LATER".into())];
    let rules = [rule];
    let autofill = std::collections::HashMap::new();

    super::session::set_test_now(Some(100));
    tx.send(b"wake".to_vec()).expect("送一次远端输出");
    tab.poll(&rules, &mut Vec::new(), false, &[], &autofill);
    assert!(writes.0.lock().expect("读写入记录").is_empty(), "命中只排队，不能立刻发");
    assert_eq!(tab.triggers.pending_count(), 1, "Send Later 已入队");

    // 远端安静下来：101s 还没到期，102s 到期。两帧 tap 都是空的，
    // 输出分支不会再跑——只有空闲段补调 due 才会出现这次发送。
    super::session::set_test_now(Some(101));
    tab.poll(&rules, &mut Vec::new(), false, &[], &autofill);
    assert!(writes.0.lock().expect("读写入记录").is_empty(), "还没到 2s 不该发");

    super::session::set_test_now(Some(102));
    tab.poll(&rules, &mut Vec::new(), false, &[], &autofill);
    // `pool.send` 把写命令交给监管者线程，落盘到假通道是异步的。
    crate::ui::lifecycle_test_support::eventually("Send Later 写出", || {
        writes.0.lock().map(|w| w.as_slice() == b"LATER").unwrap_or(false)
    });
    assert_eq!(tab.triggers.pending_count(), 0, "到期项应被取走，不能重复发");

    // 同帧不重复：后续继续静默也不该再写一遍。
    for t in 103..107 {
        super::session::set_test_now(Some(t));
        tab.poll(&rules, &mut Vec::new(), false, &[], &autofill);
    }
    assert_eq!(writes.0.lock().expect("读写入记录").as_slice(), b"LATER");
    super::session::set_test_now(None);
}

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
    let redraw = tab.poll(&[], &mut commands, false, &[], &std::collections::HashMap::new());

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
        tab.poll(&[], &mut Vec::new(), false, &[], &std::collections::HashMap::new());
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

    tab.poll(&[], &mut Vec::new(), false, &[], &std::collections::HashMap::new());
    assert!(tab.take_just_connected(), "首帧是「没连过 → 连着」，必须报边沿");

    // 同一次连接里的后续帧不再报：自动日志因此只开一份。
    for _ in 0..3 {
        tab.poll(&[], &mut Vec::new(), false, &[], &std::collections::HashMap::new());
        assert!(!tab.take_just_connected(), "同一次连接只配一个边沿");
    }

    // 断开那一帧不是「连上」边沿（那是 Disconnect 触发器的活）。
    pool.close(id);
    tab.poll(&[], &mut Vec::new(), false, &[], &std::collections::HashMap::new());
    assert!(!tab.is_live());
    assert!(!tab.take_just_connected(), "断开不该被认成连上");
}

/// 造一条带假 tap 的标签，方便把「本帧输出」直接塞进 `poll`。
fn tab_with_tap(
    id: &str,
) -> (
    term_core::session_runtime::SessionId,
    crate::ui::Tab,
    crate::hex_tap::TapSender,
) {
    let ctx = egui::Context::default();
    let pool = Arc::new(term_session::SessionPool::new());
    let ch = WriteChannel {
        writes: Writes::default(),
        finished: false,
    };
    let mut tab = crate::ui::terminal::make_tab_with_channel(
        &pool,
        term_core::session::SessionConfig::new(term_core::session::Protocol::Shell),
        Box::new(ch),
        egui::Id::new(id),
        &ctx,
    );
    let (tx, rx) = std::sync::mpsc::channel();
    tab.raw.attach(crate::hex_tap::HexTap::new(rx));
    (
        tab.session().expect("夹具带一条会话"),
        tab,
        crate::hex_tap::TapSender::new(tx),
    )
}

fn later_rule(id: &str, secs: &str) -> term_core::trigger::Rule {
    let mut rule = term_core::trigger::Rule::default();
    rule.id = id.into();
    rule.enabled = true;
    rule.on = term_core::trigger::TriggerOn::Output;
    rule.pattern = "wake".into();
    rule.literal = true;
    rule.config.send.later_secs = secs.into();
    rule.actions = vec![term_core::trigger::TriggerAction::SendText("LATER".into())];
    rule
}

/// `due` 的补调条件不能挂在 `live &&` 上：会话没了之后同样没有输出事件，
/// Send Later 就永远卡在队列里（静默期 / 断连后都不触发）。
#[test]
fn a_due_send_later_is_checked_even_after_the_session_is_gone() {
    let (_sid, mut tab, tx) = tab_with_tap("due-after-disconnect");
    let rules = [later_rule("later-gone", "2")];
    let autofill = std::collections::HashMap::new();

    super::session::set_test_now(Some(100));
    // 直接把标签转成断开态，`was_live` 也清掉，不触发断连边沿的丢弃逻辑。
    tab.session = None;
    tab.was_live = false;
    tx.send(b"wake".to_vec()).expect("送一次输出");
    tab.poll(&rules, &mut Vec::new(), false, &[], &autofill);
    assert_eq!(tab.triggers.pending_count(), 1, "命中先排队");

    super::session::set_test_now(Some(102));
    tab.poll(&rules, &mut Vec::new(), false, &[], &autofill);
    assert_eq!(
        tab.triggers.pending_count(),
        0,
        "静默期（且会话已断）也必须补调 due，不能只靠输出事件"
    );
    super::session::set_test_now(None);
}

/// 断连边沿只丢**已到期**却发不出去的项并提示：会话已经不在，这条发送
/// 既送不出去，也不能留到下一条会话上诈尸。未到期的计划不在此列。
#[test]
fn a_disconnect_edge_discards_due_pending_sends_and_reports_it() {
    let (_sid, mut tab, tx) = tab_with_tap("discard-on-disconnect");
    let rules = [later_rule("later-discard", "1")];
    let autofill = std::collections::HashMap::new();

    super::session::set_test_now(Some(100));
    tx.send(b"wake".to_vec()).expect("送一次输出");
    tab.poll(&rules, &mut Vec::new(), false, &[], &autofill);
    assert_eq!(tab.triggers.pending_count(), 1);

    // 模拟显式断开：`was_live` 还留着 true，下一帧就是断连边沿。
    tab.session = None;
    let mut note = None;
    // 1s 后确实到期了，才该被丢；用 due(u64::MAX) 时未到期也会被抽干。
    super::session::set_test_now(Some(102));
    tab.poll_with_note(&rules, &mut Vec::new(), false, &[], &autofill, &mut note);

    assert_eq!(tab.triggers.pending_count(), 0, "到期项必须取走，不能留给下一条会话");
    let note = note.expect("丢弃必须上屏，不能静默");
    assert!(note.contains("丢弃"), "note={note:?}");
    assert!(note.contains("未送达"), "note={note:?}");
    super::session::set_test_now(None);
}

/// 反面：没有自动重连的边沿也不许抽干**未到期**的 Send Later。
/// 用户手动重连后它仍有机会落地；上一版 due(u64::MAX) 在这里会把未来项吃掉。
#[test]
fn a_not_yet_due_send_later_is_kept_on_a_non_auto_reconnect_disconnect() {
    let (_sid, mut tab, tx) = tab_with_tap("keep-undue-on-disconnect");
    let rules = [later_rule("later-keep", "5")];
    let autofill = std::collections::HashMap::new();

    super::session::set_test_now(Some(100));
    tx.send(b"wake".to_vec()).expect("送一次输出");
    tab.poll(&rules, &mut Vec::new(), false, &[], &autofill);
    assert_eq!(tab.triggers.pending_count(), 1);

    tab.session = None;
    let mut note = None;
    super::session::set_test_now(Some(101));
    tab.poll_with_note(&rules, &mut Vec::new(), false, &[], &autofill, &mut note);
    assert_eq!(
        tab.triggers.pending_count(),
        1,
        "未到期的项不属于「已到期但无会话可发」，不能抽干"
    );
    assert!(note.is_none(), "没丢东西就不该报丢弃：{note:?}");
    super::session::set_test_now(None);
}

/// 自动重连的非 shell 会话：1 秒重连之后，命中时的 Send Later 仍要到期落地。
/// 这是「断连边沿无条件抽干队列」那个 bug 的端到端判据——只看队列长度不够，
/// 必须真的在新会话的通道上看到写出的字节。
#[test]
fn a_send_later_survives_a_one_second_auto_reconnect_and_fires() {
    let ctx = egui::Context::default();
    let pool = Arc::new(term_session::SessionPool::new());
    let writes = Writes::default();
    let mk_ch = || WriteChannel {
        writes: writes.clone(),
        finished: false,
    };
    let mut tab = crate::ui::terminal::make_tab_with_channel(
        &pool,
        SessionConfig::new(term_core::session::Protocol::Ssh),
        Box::new(mk_ch()),
        egui::Id::new("later-across-reconnect"),
        &ctx,
    );
    let old_sid = tab.session().expect("夹具带一条会话");
    let (tx, rx) = std::sync::mpsc::channel();
    tab.raw.attach(crate::hex_tap::HexTap::new(rx));
    let rules = [later_rule("later-reconnect", "1")];
    let autofill = std::collections::HashMap::new();

    super::session::set_test_now(Some(100));
    tx.send(b"wake".to_vec()).expect("送一次输出");
    tab.poll(&rules, &mut Vec::new(), true, &[], &autofill);
    assert_eq!(tab.triggers.pending_count(), 1, "命中先排队");

    // 断开边沿：自动重连 + 非 shell → 队列原样留住，且按 1s 退避排重连。
    // 断开必须发生在到期之前，否则静默期的 due 会先把它发出去（那时没有会话）。
    tab.session = None;
    let mut note = None;
    tab.poll_with_note(&rules, &mut Vec::new(), true, &[], &autofill, &mut note);
    assert_eq!(
        tab.triggers.pending_count(),
        1,
        "短暂重连必须留住未到期的 Send Later"
    );
    assert!(note.is_none(), "没丢弃就不该报丢弃：{note:?}");
    assert_eq!(tab.auto_reconnect_at, Some(101), "首次退避是 1 秒");

    // 1 秒后重连：同槽位换一条新会话，假通道仍记到同一份 Writes。
    let new_sid = tab.pool.open_with_channel(&tab.config, 80, 24, Box::new(mk_ch()));
    tab.session = Some(new_sid);
    assert_ne!(new_sid, old_sid, "重连换成新会话 id");

    super::session::set_test_now(Some(101));
    tab.poll(&rules, &mut Vec::new(), true, &[], &autofill);
    crate::ui::lifecycle_test_support::eventually("重连后的 Send Later 写出", || {
        writes
            .0
            .lock()
            .map(|w| w.as_slice() == b"LATER")
            .unwrap_or(false)
    });
    assert_eq!(tab.triggers.pending_count(), 0, "到期项必须被取走，不能重复发");
    super::session::set_test_now(None);
}

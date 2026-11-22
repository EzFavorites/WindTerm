//! B2：**控制行经 tap 字节流自动 feed** 的回归判据。
//!
//! 为什么单独一个文件：`bin_spec.rs` 的代码行是 489 / 500，
//! 塞不下新测试；而这里验的是「字节 → 跨块攒行 → 本 tab 的 ControlMode」
//! 这条链，与 hex 排版无关，本来也该分开。
//!
//! 三条可证伪的判据：
//! 1. 控制行被任意切成两块时，只有整行到达才置 `active`（半行留在缓冲）；
//! 2. 普通 shell 输出全量喂进去永远 idle（后台标签也安全）；
//! 3. `Tab::poll` 真的把 tap 字节喂进**本 tab** 的解析器，而不是窗口单例。
//!    把 `poll` 里的 `tmux_consume_bytes` 删掉，第 3 条必红。

use std::sync::mpsc::channel;

use crate::hex_tap::HexTap;
use crate::ui::Tab;

use super::bin_spec::RawLog;
use super::HexParams;

/// 一条**没有会话**的标签：不碰真 PTY，也不受后台线程影响。
/// `session = None` 让 `poll()` 搬完字节就返回，正好只验 tap → tmux 那一段。
fn bare_tab() -> Tab {
    Tab {
        pool: std::sync::Arc::new(term_session::SessionPool::new()),
        session: None,
        // ADR-0025 阶段 2：身份走铸造源，每条夹具互异——`active_slot()` 按身份反查，
        // 两条标签同为 1 会撞号（都解析到 0 号），「切到第 2 条」就验不出来了。
        identity: super::setup::mint_tab_identity(),
        config: term_core::session::SessionConfig::new(term_core::session::Protocol::Shell),
        snapshot: term_core::grid::GridSnapshot::empty(80, 24),
        screen: None,
        view: term_core::grid::ScreenImage::blank(80, 0, 0..24),
        viewport: term_app::viewport::Viewport::new(),
        selections: term_app::selection::Selections::default(),
        last_request: None,
        preview_screen: None,
        preview_last_request: None,
        id: egui::Id::new("tmux_feed_test"),
        auth: None,
        input_line: term_core::input_line::InputLine::new(),
        last_input_at: std::cell::Cell::new(None),
        mode: term_runtime::prefs::DisplayMode::Text,
        hex: HexParams::default(),
        // 空白/符号可视化六项：这条夹具只验 tmux 控制行，六项取出厂值（全关）。
        marks: crate::ui::terminal::MarksParams::default(),
        trigger_highlight_rows: Vec::new(),
        // 调试覆盖层：这条夹具只验 tmux 控制行，覆盖层取出厂值（关）。
        debug: false,
        // 只读查看：这条夹具只想验 tmux 控制行，取出厂值（可输入）。
        local_mode: false,
        raw: RawLog::new(),
        tmux: term_app::tmux::ControlMode::default(),
        tmux_pending: Vec::new(),
        triggers: term_app::trigger::Matcher::new(),
        shadow: crate::automation::Shadow::new(),
        was_live: false,
            just_connected: false,
            auto_reconnect_at: None,
            auto_reconnect_attempts: 0,
            auto_reconnect_blocked: false,
        // 折叠与输入行表出厂为空（空集合 = 渲染一屏不变，夹具不受影响）。
        folds: term_app::folds::Folds::new(),
        input_rows: std::cell::RefCell::new(Vec::new()),
        restore_scroll_top: None,
        restore_history_len: None,
    }
}

/// 第 1 条：控制行跨任意 chunk，整行到达才置位；空行跳过、`\r` 剥掉。
#[test]
fn a_control_line_split_across_chunks_activates_the_tab() {
    let mut tab = bare_tab();
    tab.tmux_consume_bytes(b"%beg");
    assert!(!tab.tmux.active, "半行不该喂进解析器");
    assert_eq!(tab.tmux_pending, b"%beg".to_vec(), "半行要留到下一块");

    tab.tmux_consume_bytes(b"in 1700000000 42 0\r\n\n");
    assert!(tab.tmux.active, "整行到达后 %begin 才把 active 置位");
    assert!(tab.tmux.in_notification);
    assert!(tab.tmux_pending.is_empty(), "整行与随后的空行都要消费干净");
}

/// 第 2 条：普通会话字节全量喂进去永远 idle——这是「后台标签也喂」的安全前提。
#[test]
fn plain_shell_output_never_activates_a_background_tab() {
    let mut tab = bare_tab();
    tab.tmux_consume_bytes(b"bash-3.2$ echo hi\r\nls -la\r\n");
    assert!(!tab.tmux.active, "普通行是 Passthrough，不许置 active");
    assert!(tab.tmux_pending.is_empty());
}

/// 第 2 条的补角：`%exit` 能把一个已 active 的标签降回 idle（降级路径仍通）。
#[test]
fn a_real_exit_deactivates_the_tab() {
    let mut tab = bare_tab();
    tab.tmux_consume_bytes(b"%begin 1 2 0\n");
    assert!(tab.tmux.active);
    tab.tmux_consume_bytes(b"%exit detached\n");
    assert!(!tab.tmux.active);
    assert!(tab.tmux.exited);
}

/// 第 3 条：`poll` 把 tap 字节搬进环**并且**喂进本 tab 的 ControlMode。
/// 这条同时钉住 `drain_take`：环要照旧被填，字节也要交回调用方。
#[test]
fn poll_drains_the_tap_and_automatically_feeds_control_mode() {
    let mut tab = bare_tab();
    let (tx, rx) = channel();
    tab.raw.attach(HexTap::new(rx));
    tx.send(b"%beg".to_vec()).expect("送第一块");
    tx.send(b"in 1700000000 42 0\r\n".to_vec())
        .expect("送第二块");

    assert!(tab.poll(&[], &mut Vec::new(), false), "有新字节就该请求重绘");
    assert!(tab.tmux.in_notification);
    assert!(tab.tmux.panes.is_empty());
    assert!(tab.tmux_pending.is_empty());
    assert_eq!(
        tab.raw.ring.take(0, 24),
        b"%begin 1700000000 42 0\r\n".to_vec(),
        "环里仍要留下原始字节（hex 视图不受影响）"
    );
    // 手动喂与自动喂是两条独立来源：这里再喂一条只进面板日志的测试行，
    // 不会把 tap 那条重复喂一遍（pending 已空，环不会多出字节）。
    assert!(!tab.poll(&[], &mut Vec::new(), false), "tap 已空，第二帧不该再报新事实");
    assert_eq!(tab.raw.ring.take(0, 24).len(), 24, "环没有被重复追加");
}

/// `drain_take` 与 `drain` 同源：空 tap 返回零分配的空 Vec，挂上后两块一起收。
#[test]
fn drain_take_hands_back_every_byte_and_still_fills_the_ring() {
    let mut log = RawLog::new();
    assert!(log.drain_take().is_empty(), "没挂 tap 时返回空 Vec");

    let (tx, rx) = channel();
    log.attach(HexTap::new(rx));
    tx.send(b"hello".to_vec()).expect("送第一块");
    tx.send(b" world".to_vec()).expect("送第二块");

    assert_eq!(log.drain_take(), b"hello world".to_vec());
    assert_eq!(log.ring.take(0, 11), b"hello world".to_vec(), "环照旧被填");
    assert!(log.drain_take().is_empty(), "搬完就空了");
}

/// B2 的核心动机：状态机是**每 tab** 的，不是窗口单例。
/// 一个窗口里 idle 标签与 tmux 标签 coexistence，`active_tab` 越界降级成 None。
#[test]
fn the_guard_reads_the_active_tab_not_a_window_singleton() {
    let ctx = egui::Context::default();
    let pool = std::sync::Arc::new(term_session::SessionPool::new());
    let mut hot = bare_tab();
    hot.tmux.feed("%begin 1 2 0");
    assert!(hot.tmux.active, "夹具前提：第二条是 tmux 会话");
    let mut win = crate::ui::lifecycle_test_support::window_with(pool, &ctx, vec![bare_tab(), hot]);

    assert!(
        !win.active_tab_tmux().unwrap().active,
        "第 0 条仍是普通会话，不该被第 1 条串成 active"
    );
    win.set_active_slot(1);
    assert!(win.active_tab_tmux().unwrap().active, "切到 tmux 标签才 active");
    win.set_active_slot(99);
    assert!(win.active_tab_tmux().is_none(), "越界降级成 None，不 panic");

    // 越界时 send / feed 都不许 panic：硬闸一剑不发，日志仍能记一条解析结果。
    win.tmux_send("split-window -h");
    win.tmux_feed("%begin 1 2 0");
    assert_eq!(win.tmux_log.len(), 1);
}

/// 硬闸「未 attach 一剑不发」的可钉住判据：`Tab::write` 无论有没有送到都
/// 会记 `last_input_at`，所以它正好是「有没有尝试写 PTY」的观测孔。
#[test]
fn an_idle_tab_refuses_reverse_control_but_an_active_one_writes() {
    let ctx = egui::Context::default();
    let pool = std::sync::Arc::new(term_session::SessionPool::new());
    let mut win = crate::ui::lifecycle_test_support::window_with(pool, &ctx, vec![bare_tab()]);

    win.tmux_send("split-window -h");
    assert!(
        win.tabs[0].last_input_at.get().is_none(),
        "idle 标签不该尝试写 PTY（一剑不发）"
    );

    win.tabs[0].tmux.feed("%begin 1 2 0");
    win.tmux_send("split-window -h");
    assert!(
        win.tabs[0].last_input_at.get().is_some(),
        "active 标签才允许把控制行写出去"
    );
}

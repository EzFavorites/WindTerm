//! 斑马线（`Tab.hex.zebra`）的作用域回归：设置页改默认值**只影响新标签**。
//!
//! # 这一刀修的是什么
//!
//! 2026-11-11 之前，`Alternate column background color` 那一格写的是窗口上的
//! `TerminalWindow::alternate_column_bg`，而 `pane.rs` 每帧直接读它喂
//! `bin::paint`——所以勾一下当场改掉**所有已开标签**的画法。同一个设置页上
//! 另外四个 hex 参数（`Prefs.hex_*`）只写 prefs、只影响**新**标签。五个同类
//! 参数两套语义。
//!
//! 收进 `HexParams` 之后五个参数一个语义：设置页改的是「默认参数」，
//! 已开标签各带自己的一份 `Tab.hex`。
//!
//! # 为什么这两条测试要真的画一帧
//!
//! 「读写路径对不对」用 `assert_eq!(hex.zebra, …)` 就能测，但那测不出
//! `pane.rs` 到底把**哪一份**交给了渲染器——旧实现里 `Tab.hex` 可以完全正确，
//! 屏幕照样跟着窗口字段变。所以这里的判据是**屏幕上真的画了几块底色**：
//! 走一遍 `draw_terminal_area_for`，再数这一帧的 rect 形状里有几块的填充色
//! 等于 `bin::zebra_tint(前景色)`。
//!
//! 数出来的数不是恒真的（证伪实录见交付说明）：
//!
//! - 把 `pane.rs` 那一格改回读窗口字段 → 第 1 条红（窗口上是 `false`，
//!   已开标签的底色会**当场消失**，`after != before`）；
//! - 把 `setup::hex_defaults` 里的斑马线写死成 `true`（不读 prefs）→
//!   第 2 条红（`prefs.alternate_column_bg = false` 时新标签仍画出底色）。

use crate::ui::lifecycle_test_support::window_with;
use crate::ui::*;

use super::bin;
use super::setup::hex_defaults;
use super::HexParams;
use term_runtime::prefs::Prefs;

/// 一帧的宽度：够宽，16 列 16 进制（`60` 字符 × 4px = 240px）不会被 `fitted` 夹。
const FRAME: egui::Vec2 = egui::vec2(800.0, 600.0);

/// 一条**没有会话**的 hex 标签，字节环里预置 4 行满 16 列的数据。
///
/// 「没有会话」是有意的：这几条测的是**渲染参数的来源**，不是 PTY。没有会话
/// 时 `Tab::resize` / `request_range` 都是空操作，一帧画下来不受后台线程影响，
/// 数出来的底色块数才可复现。
fn hex_tab(pool: &std::sync::Arc<term_session::SessionPool>, hex: HexParams) -> Tab {
    let mut snapshot = GridSnapshot::empty(80, 24);
    snapshot.bracketed_paste = false;
    let mut tab = Tab {
        pool: pool.clone(),
        session: None,
        identity: 1,
        config: SessionConfig::new(Protocol::Shell),
        snapshot,
        screen: None,
        view: ScreenImage::blank(80, 0, 0..24),
        viewport: Viewport::new(),
        selections: Selections::default(),
        last_request: None,
        preview_screen: None,
        preview_last_request: None,
        auth: None,
        input_line: term_core::input_line::InputLine::new(),
        id: egui::Id::new("zebra-scope-test"),
        last_input_at: std::cell::Cell::new(None),
        // 必须是 Bin：这一帧要走 `bin::paint` 那条分支。
        mode: term_runtime::prefs::DisplayMode::Bin,
        hex,
        // 空白/符号可视化六项：这条夹具只验斑马线，六项取出厂值（全关）。
        marks: crate::ui::terminal::MarksParams::default(),
        trigger_highlight_rows: Vec::new(),
        onekey_expects_fired: std::collections::HashSet::new(),
        modem: crate::ui::modem_auto::TabState::default(),
        // 调试覆盖层：这条夹具要逐像素数斑马线底色块，覆盖层必须关。
        debug: false,
        debug_log_active: false,
        vim: term_app::vim::VimState::default(),
        // 只读查看：这条夹具只验斑马线，取出厂值（可输入、无冻结锚点）。
        local_mode: false,
        word_wrap: false,
        raw: crate::ui::terminal::bin_spec::RawLog::new(),
        // B2：控制模式状态机归本 tab，跨块行缓冲从空开始。
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
        filter: term_app::state::FilterState::default(),
        filter_input: String::new(),
        filter_history: Vec::new(),
        filter_diag: None,
        input_rows: std::cell::RefCell::new(Vec::new()),
        command_texts: std::cell::RefCell::new(Vec::new()),
        pending_local_sends: Vec::new(),
        // N：恢复运行态字段（新建标签出厂 None）。
        restore_scroll_top: None,
        restore_history_len: None,
    };
    tab.raw.ring.push(&[0x41u8; 64]);
    tab
}

/// 跑一帧 `draw_terminal_area_for`，数这一帧里**几块底色**是斑马线的颜色。
///
/// 判据是填充色**逐位相等**：`bin::paint` 用同一个 [`bin::zebra_tint`] 算出来，
/// 所以「有几块」不是靠位置或顺序猜的，而是那层底色本身的定义。
///
/// 面板自己的背景、终端区的底色都是别的颜色（`default_bg`），不会混进来。
fn stripes_in_one_frame(ctx: &egui::Context, win: &mut TerminalWindow) -> usize {
    let tint = bin::zebra_tint(to_color32(win.palette.default_fg));
    let input = egui::RawInput {
        screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, FRAME)),
        ..Default::default()
    };
    // ADR-0025 阶段 3：绘制按身份；这条夹具只有一条标签，取它的身份。
    let tab_id = win.tabs.first().map(|t| t.identity).unwrap_or(0);
    let out = ctx.run(input, |ctx| {
        egui::CentralPanel::default().show(ctx, |ui| {
            win.draw_terminal_area_for(ui, tab_id);
        });
    });
    out.shapes
        .iter()
        .filter(|clipped| match &clipped.shape {
            egui::Shape::Rect(rect) => rect.fill == tint,
            _ => false,
        })
        .count()
}

/// **第 1 条**：设置页改斑马线的默认值，**已开标签一个字都不变**。
///
/// 旧实现下这条必挂：`pane.rs` 读的是窗口字段，把默认值改成 `false` 之后
/// 已开标签的底色当场消失（`after = 0 != before`）。
#[test]
fn changing_the_zebra_default_does_not_repaint_an_already_open_tab() {
    let ctx = egui::Context::default();
    let pool = std::sync::Arc::new(term_session::SessionPool::new());
    // 已开标签带着自己那份参数（出厂 `zebra = true`）。
    let tab = hex_tab(&pool, HexParams::default());
    let mut win = window_with(pool, &ctx, vec![tab]);
    assert!(win.tabs[0].hex.zebra, "夹具前提：已开标签的斑马线是开着的");

    let before = stripes_in_one_frame(&ctx, &mut win);
    assert!(
        before > 0,
        "已开标签带着 `zebra = true`，这一帧却一块底色都没有：渲染读的不是标签上的\
         参数（旧实现读的是窗口级 `alternate_column_bg`）"
    );

    // 「设置页」改默认值：这一格只写窗口上那份默认参数副本（以及落盘），
    // 它手上没有标签（`settings_live_binary::binary` 的签名里就没有）。
    win.hex_defaults.zebra = false;

    let after = stripes_in_one_frame(&ctx, &mut win);
    assert!(
        win.tabs[0].hex.zebra,
        "已开标签的 `Tab.hex.zebra` 被默认值改掉了——两条路径串线了"
    );
    assert_eq!(
        after, before,
        "已开标签的画法跟着默认值变了：`pane.rs` 又把窗口上的默认参数当渲染参数读了"
    );
}

/// **第 2 条**：新标签的斑马线确实**按 `Prefs` 取初值**（不是恒为出厂值）。
///
/// 旧实现/写死实现下这条必挂：`hex_defaults` 里若不读
/// `prefs.alternate_column_bg`，`false` 的 prefs 会得到 `zebra = true`，
/// 于是「关掉默认值」的新标签照样画出底色。
#[test]
fn a_new_tab_takes_the_zebra_default_from_prefs() {
    let ctx = egui::Context::default();

    // 一、prefs 把斑马线关掉：新标签的渲染参数里它就是关的，屏幕上一块底色都不该有。
    let off = Prefs {
        alternate_column_bg: false,
        hex_columns: 20,
        ..Prefs::default()
    };
    let hex_off = hex_defaults(&off);
    assert_eq!(
        (hex_off.zebra, hex_off.columns),
        (false, 20),
        "新标签的初值必须整份来自 prefs（斑马线与列数一起），不是只有排版数"
    );
    let pool = std::sync::Arc::new(term_session::SessionPool::new());
    let mut win = window_with(pool.clone(), &ctx, vec![hex_tab(&pool, hex_off)]);
    assert_eq!(
        win.tabs[0].hex.zebra, off.alternate_column_bg,
        "接线断了：新标签的斑马线没跟着 prefs 走"
    );
    assert_eq!(
        stripes_in_one_frame(&ctx, &mut win),
        0,
        "prefs 关掉斑马线，新标签就该一块底色都没有"
    );

    // 二、反过来：勾上就必须画出来。两半合起来才说明它**跟着 prefs 变**，
    // 而不是「恒为 false」或「恒为出厂 true」。
    let on = Prefs {
        alternate_column_bg: true,
        hex_columns: 20,
        ..Prefs::default()
    };
    let hex_on = hex_defaults(&on);
    assert!(hex_on.zebra, "勾上时新标签的斑马线必须是开的");
    let pool = std::sync::Arc::new(term_session::SessionPool::new());
    let mut win = window_with(pool.clone(), &ctx, vec![hex_tab(&pool, hex_on)]);
    assert!(
        stripes_in_one_frame(&ctx, &mut win) > 0,
        "prefs 勾上斑马线，新标签却一块底色都没画"
    );
}

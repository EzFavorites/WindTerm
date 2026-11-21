//! 标签生命周期回归单测的共享夹具（`#[cfg(test)]`，不进生产构建）。
//!
//! 第一刀（`tab_strip.rs` 的两条关闭入口）与第二刀（`sidebar_view.rs` 删配置、
//! `restore.rs` 整份替换）测的是同一件事：**把 `Tab` 移出窗口数组前必须先经
//! `Tab::close()` 走控制面**。三处测试都要一个完整 `TerminalWindow`、一条真本地
//! shell、一条可观察「通道被关」的假通道，所以夹具只放这一份，由
//! `crate::ui::lifecycle_test_support` 统一导出，避免三份复制各自漂移。
//!
//! # 为什么手写整份结构体字面量
//!
//! `TerminalWindow::new` 需要 `eframe::CreationContext`（0.29.1 里字段是
//! `pub(crate)`、无公开构造器），测试侧面构造不出来。按 `terminal/paste.rs` 的
//! 先例，在 `ui` 的后代模块里手写结构体字面量——后代模块能看见 `TerminalWindow`
//! 的私有字段。
//!
//! # 为什么这些断言不是恒真的
//!
//! 旧实现下 `pool.send(old_id, ..)` 仍为 `true`、`pool.screen(old_id)` 仍是
//! `Some`、假通道的 `close()` 一次都不会被调用。证伪方式：把被测处的 `close()`
//! 循环还原成旧代码，测试必挂（见 `/tmp/falsify-p01b.log`）。

use crate::ui::*;
// 造窗夹具要建「窗格身份 + group」配对。
use crate::ui::layout::{PaneGroup, PaneId};
use crate::ui::layout::mint_pane_identity;
use std::collections::HashMap;

use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

/// 把「监管者真的调用了假通道的 `close()`」记下来的计数器。
///
/// 与 `term-session` 的 `StubChannel` / `term-runtime` 的 `ScriptedChannel` 同一条
/// 注入范式：`SessionPool::open_with_channel` → `spawn_with_channel`。
#[derive(Clone, Default)]
pub(in crate::ui) struct CloseCount(std::sync::Arc<AtomicUsize>);

impl CloseCount {
    pub(in crate::ui) fn bump(&self) {
        self.0.fetch_add(1, Ordering::SeqCst);
    }

    pub(in crate::ui) fn get(&self) -> usize {
        self.0.load(Ordering::SeqCst)
    }
}

/// 不碰 PTY / SSH 的假通道。`drain` 恒空，所以 tee 探针不会被字节填满；
/// `close()` 只记一次数，让「会话被关掉」成为测试能读到的客观事实。
struct RecordingChannel {
    closes: CloseCount,
    finished: bool,
}

impl term_runtime::channel::TermChannel for RecordingChannel {
    fn drain(&mut self) -> Vec<u8> {
        Vec::new()
    }

    fn write(&mut self, _bytes: &[u8]) {}

    fn resize(&mut self, _cols: usize, _rows: usize) {}

    fn poll(&mut self) -> Option<String> {
        None
    }

    fn finished(&self) -> bool {
        self.finished
    }

    fn close(&mut self) {
        self.finished = true;
        self.closes.bump();
    }

    fn description(&self) -> &str {
        "recording channel"
    }
}

/// 起一条真本地 shell 标签：`make_tab` → `SessionPool::open` → 真 PTY。
///
/// 真 PTY 在当前环境起不来时 `session` 会是 `None`，调用方用 `expect` 直接把测试
/// 判失败，而不是退化成一个「怎么改都通过」的假断言。
pub(in crate::ui) fn real_shell_tab(
    pool: &std::sync::Arc<term_session::SessionPool>,
    ctx: &egui::Context,
    i: usize,
) -> Tab {
    crate::ui::terminal::make_tab(
        pool,
        SessionConfig::new(Protocol::Shell),
        egui::Id::new(("lifecycle_real", i)),
        ctx,
    )
}

/// 起一条注入假通道的标签，通道的 `close()` 记到 `closes` 上。
pub(in crate::ui) fn fake_tab(
    pool: &std::sync::Arc<term_session::SessionPool>,
    ctx: &egui::Context,
    i: usize,
    closes: &CloseCount,
) -> Tab {
    let ch = Box::new(RecordingChannel {
        closes: closes.clone(),
        finished: false,
    }) as Box<dyn term_runtime::channel::TermChannel>;
    crate::ui::terminal::make_tab_with_channel(
        pool,
        SessionConfig::new(Protocol::Shell),
        ch,
        egui::Id::new(("lifecycle_fake", i)),
        ctx,
    )
}

/// 监管者线程的 `DetachTap` / `Close` 是异步入队的，给它一个有上限的等待窗口。
/// 超时就是断言失败，不会无限挂住。
pub(in crate::ui) fn eventually(what: &str, mut done: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        if done() {
            return;
        }
        assert!(Instant::now() < deadline, "等不到：{what}");
        std::thread::sleep(Duration::from_millis(5));
    }
}

/// 会话从控制面除名的三条独立查询，逐条都该是诚实的不在册。
pub(in crate::ui) fn assert_gone(
    pool: &term_session::SessionPool,
    id: term_core::session_runtime::SessionId,
    label: &str,
) {
    assert!(!pool.send(id, b"x"), "{label}：send 必须返回 false");
    assert!(
        pool.screen(id).is_none(),
        "{label}：screen 必须是 None"
    );
    assert!(pool.state(id).is_none(), "{label}：state 必须是 None");
    assert!(
        !pool.list().iter().any(|(listed, _, _)| *listed == id),
        "{label}：list 里不该再有这个 id"
    );
}

/// 手写一份最小但**字段齐全**的 `TerminalWindow`。
///
/// 只填会影响标签增删路径的字段（tabs / pool / ctx / layout / tab_colors /
/// tab_focus / active_tab / caret_style / sessions），其余一律 `Default` 或空集合
/// ——这些字段在这些路径上没有任何读者。
///
/// `terminal/zebra_scope_tests` 也借它跑一帧 hex 绘制：那条路径要的 `palette`
/// （这里是真的 `load_palette()`）、`fonts`（`Default` 的 0 尺寸会被 `pane.rs`
/// 的 `.max(4.0)` / `.max(8.0)` 兜住）与 `hex_defaults`（`Default`）都够用。
pub(in crate::ui) fn window_with(
    pool: std::sync::Arc<term_session::SessionPool>,
    ctx: &egui::Context,
    tabs: Vec<Tab>,
) -> TerminalWindow {
    // 从入参 tabs 反推恒在身份顺序与活动身份。
    let open_order = tabs.iter().map(|t| t.identity).collect();
    let active = tabs.first().map(|t| t.identity);
    // 初始单窗格 = 一个 PaneId + 一片只含当前会话的 group。
    let init_pane = mint_pane_identity();
    let mut pane_groups: HashMap<PaneId, PaneGroup> = HashMap::new();
    if let Some(s) = active {
        pane_groups.insert(init_pane, PaneGroup { order: vec![s], active: s });
    }
    TerminalWindow {
        tabs,
        open_order,
        active,
        layout: Layout::single(init_pane),
        pane_groups,
        palette: term_runtime::theme_load::load_palette(),
        fonts: Default::default(),
        view: Default::default(),
        // 测试夹具用出厂词表，不碰真实 profile 目录里的 quickbar.config。
        quick_bar: term_runtime::quickbar::QuickBarConfig::default(),
        quick_bar_display: term_runtime::quickbar::QuickBarDisplay::default(),
        wrap_symbols: false,
        column_marker_col: 0,
        completion: term_core::completion::Settings::default(),
        completion_state: Default::default(),
        groups: Vec::new(),
        sessions: Vec::new(),
        selected_session: 0,
        session_filter: String::new(),
        panel_filters: Default::default(),
        shell_tabs: Vec::new(),
        shell_active: None,
        shell_filter: String::new(),
        shell_list_open: false,
        closed_sessions: Vec::new(),
        tools: Vec::new(),
        new_session: NewSessionData::from_config(&SessionConfig::new(Protocol::Shell)),
        session_dialog: SessionDialogKind::New { group: None },
        default_session: SessionConfig::new(Protocol::Ssh),
        settings: SettingsData {
            term: String::new(),
            scrollback: String::new(),
            font: String::new(),
            font_size: 13.0,
            groups_open: [true; 3],
        },
        caret_style: 0,
        caret_period: term_core::config::DEFAULT_CARET_PERIOD,
        caret_width: term_core::config::DEFAULT_CARET_WIDTH,
        alt_row_bg: false,
        hide_cursor_typing: false,
        // 出厂 `true`：查找命中高亮本来就无条件在画，夹具照出厂态摆。
        highlight_search: true,
        hex_defaults: Default::default(),
        focus_new_tab: true,
        tab_activate_after_close: 0,
        tab_double_click: 0,
        tab_elide: 0,
        tab_numbers: 0,
        tab_middle_click: 0,
        tab_right_click: 0,
        show_address_bar: true,
        auto_hide_menu_bar: false,
        menu_bar_peek_until: 0.0,
        lock_timeout_secs: 0,
        last_lock_activity: 0.0,
        auto_reconnect: false,
        clipboard_requests: Vec::new(),
        clipboard_ask: None,
        triggers: Vec::new(),
        onekeys: Vec::new(),
        // 2026-11-17 Tunnels：测试窗口不配隧道、对话框关着、总线开关按出厂开着。
        tunnel_editor: None,
        tunnel_profiles: Vec::new(),
        tunnels_enabled: true,
        onekeys_edit: None,
        onekeys_password_draft: None,
        // 测试窗口从「没话说、没崩过」开始。
        notice: None,
        frame_panics: 0,
        tab_focus: Vec::new(),
        settings_page: Default::default(),
        term_view: Default::default(),
        // 底带 Shell 面板那一份（主区与面板各养一份字形缓存，见 `ui/mod.rs`）。
        term_view_shell: Default::default(),
        selection_text: String::new(),
        selection_text_shell: String::new(),
        command_history: Default::default(),
        ctx: ctx.clone(),
        // 周期留存测试字段（0 = 从没留存过）。
        last_periodic_save: 0.0,
        // 结构变化即时落盘标志（测试窗口默认 false = 本帧无待存结构变化）。
        pending_save_layout: false,
        // 拖拽态（测试窗口默认无拖拽、本帧未松手）。
        drag_session: None,
        drag_released_this_frame: false,
        restore_focus: false,
        restore_focus_shell: false,
        find_input: String::new(),
        palette_input: String::new(),
        goto_input: String::new(),
        center_input: String::new(),
        lock_password: String::new(),
        shortcut_overrides: Vec::new(),
        shortcut_capture: None,
        keyseq: term_app::shortcuts::KeySeqState::new(),
        seq_table: crate::ui::dialogs::shortcuts::runtime_sequence_table(),
        tab_colors: Vec::new(),
        tunnel_input: String::new(),
        tunnel_error: None,
        tmux_log: Vec::new(),
        tmux_input: String::new(),
        tmux_blocks: Vec::new(),
        pending_select: None,
        connector: None,
        ssh_password: String::new(),
        save_password: false,
        tunnels: Vec::new(),
        pool,
        vault: None,
        vault_form: Default::default(),
        log_note: None,
        log_settings: Default::default(),
        filer: Default::default(),
        script: None,
        script_panel: crate::ui::panels::ScriptPanelState::default(),
        script_opened: Default::default(),
        script_activated: Default::default(),
        script_asks: Default::default(),
        script_picks: Default::default(),
        script_selection: Default::default(),
        script_active: Default::default(),
        script_ask: None,
        script_runs: Default::default(),
        logon_fired: Default::default(),
        logon_runs: Vec::new(),
        script_recent: Vec::new(),
        recorder: Default::default(),
        confirm: Default::default(),
        paste_confirm_main: false,
        paste_confirm_alt: false,
        // Terminal ▸ Mouse Actions：夹具走出厂态（右键出菜单、选中不自动复制），
        // 与 `Prefs::default()` 逐项对齐；两份「上一次写出去的选区」从空开始。
        right_click: 0,
        copy_on_select: false,
        mouse_events: term_app::mouse::EventMask::ALL,
        copied_on_select: None,
        copied_on_select_shell: None,
        pending_paste: None,
        xfer: None,
        xfer_prompt: None,
        demo: None,
        demo_preview_hover_y: None,
        comp_demo: None,
        dock: Default::default(),
        app_queue: Default::default(),
        app_rev: 0,
        prefs_rev: 0,
        dock_drag: None,
        band_tabs: [None, None, None],
        float_focus_tries: Vec::new(),
        bottom_maximized: false,
        bottom_split: None,
        pending_dock_cmds: Vec::new(),
        bottom_height: 0.0,
        sender: Default::default(),
        toolbar_pos: None,
        dock_rects: Vec::new(),
        startup: Default::default(),
        pending_activation: Vec::new(),
        applied_window: Default::default(),
        // 夹具窗口一律出厂不透明（这些生命周期用例不碰窗口透明度）。
        window_opacity: Default::default(),
        // 地址栏默认关闭；这些生命周期用例不碰它。
        address: Default::default(),
        // 首次启动引导默认关着；生命周期用例不碰它（探测要读家目录，
        // 用例里也不该依赖那台机器上有没有 `~/.ssh/config`）。
        onboarding: term_app::onboarding::OnboardingState::new(),
        // 预览窗默认关着；这些生命周期用例不碰它（打开要读真实文件，
        // 用例里不该依赖那台机器上有哪个文件）。
        preview: Default::default(),
        // 脚本编辑器默认没开；生命周期用例不碰它。
        script_editor: None,
    }
}

#[cfg(test)]
mod phase0_identity_tests {
    use super::*;

    /// 验收：窗口态 `open_order` / `active` 从 `tabs` 反推且一致。
    #[test]
    fn open_order_tracks_tab_identities_and_active_points_first() {
        let pool = std::sync::Arc::new(term_session::SessionPool::new());
        let ctx = egui::Context::default();
        let t0 = real_shell_tab(&pool, &ctx, 0);
        let t1 = real_shell_tab(&pool, &ctx, 1);
        let t2 = real_shell_tab(&pool, &ctx, 2);
        let ids = vec![t0.identity, t1.identity, t2.identity];

        let win = window_with(pool, &ctx, vec![t0, t1, t2]);

        assert_eq!(win.open_order.len(), ids.len(), "open_order 必须与 tabs 等长");
        assert_eq!(win.open_order, ids, "open_order 必须与 tabs 同序");
        assert_eq!(win.active, Some(ids[0]), "active 必须指向首 tab 的 identity");
        // 铸造源递增：三条标签身份互不相同。
        assert!(ids[0] != ids[1] && ids[1] != ids[2], "恒在身份必须互异");
    }
}

#[cfg(test)]
mod a2_save_layout_tests {
    use super::*;

    /// 造一个双窗格窗口：两片真本地 shell 叶子并排，活动身份落在第一片。
    fn two_pane_window() -> TerminalWindow {
        let pool = std::sync::Arc::new(term_session::SessionPool::new());
        let ctx = egui::Context::default();
        let t0 = real_shell_tab(&pool, &ctx, 0);
        let t1 = real_shell_tab(&pool, &ctx, 1);
        let id0 = t0.identity;
        let id1 = t1.identity;
        let mut win = window_with(pool, &ctx, vec![t0, t1]);
        // 覆盖 `window_with` 默认的单叶布局，摆成左右各一片；每片配一个 PaneId + group。
        let p0 = mint_pane_identity();
        let p1 = mint_pane_identity();
        let mut groups = HashMap::new();
        groups.insert(p0, PaneGroup { order: vec![id0], active: id0 });
        groups.insert(p1, PaneGroup { order: vec![id1], active: id1 });
        win.pane_groups = groups;
        win.layout = Layout::Split {
            horizontal: true,
            ratio: 500,
            first: Box::new(Layout::Leaf(p0)),
            second: Box::new(Layout::Leaf(p1)),
        };
        win.active = Some(id0);
        win
    }

    /// 关掉当前窗格（剪叶，结构变化）后必须置「待落盘」标志，由帧末 flush
    /// 派发 `Action::SaveLayout`——旧实现靠 30s 周期 / 退出兜底，结构变化不即时落盘。
    #[test]
    fn closing_an_active_group_flags_pending_save_layout() {
        let mut win = two_pane_window();
        assert!(!win.pending_save_layout, "起始不应有待存标志");
        win.close_active_group();
        assert!(
            win.pending_save_layout,
            "关窗格（结构变化）后必须置待落盘标志（A2 即时落盘）"
        );
        assert!(win.layout.is_single(), "关掉 active 窗格后树应收成单叶");
    }

    /// 把当前会话并入下一窗格（合并，结构变化）后同样置「待落盘」标志。
    #[test]
    fn merging_into_next_group_flags_pending_save_layout() {
        let mut win = two_pane_window();
        assert!(!win.pending_save_layout);
        win.move_active_to_next_group();
        assert!(
            win.pending_save_layout,
            "合并进下一窗格（结构变化）后必须置待落盘标志（A2 即时落盘）"
        );
    }
}

#[cfg(test)]
mod a3_drag_swap_tests {
    use super::*;

    /// 与上面同款的造窗助手：两片真本地 shell 叶子并排，活动身份落在第一片。
    fn two_pane_window() -> TerminalWindow {
        let pool = std::sync::Arc::new(term_session::SessionPool::new());
        let ctx = egui::Context::default();
        let t0 = real_shell_tab(&pool, &ctx, 0);
        let t1 = real_shell_tab(&pool, &ctx, 1);
        let id0 = t0.identity;
        let id1 = t1.identity;
        let mut win = window_with(pool, &ctx, vec![t0, t1]);
        // 两片窗格各自一个 `PaneId` + 一个只含本会话的 group。先清掉
        // `window_with` 留下的单窗格 init_pane 条目，避免 stale group 污染 `tabs()`。
        let p0 = mint_pane_identity();
        let p1 = mint_pane_identity();
        win.pane_groups.clear();
        win.pane_groups.insert(p0, PaneGroup { order: vec![id0], active: id0 });
        win.pane_groups.insert(p1, PaneGroup { order: vec![id1], active: id1 });
        win.layout = Layout::Split {
            horizontal: true,
            ratio: 500,
            first: Box::new(Layout::Leaf(p0)),
            second: Box::new(Layout::Leaf(p1)),
        };
        win.active = Some(id0);
        win
    }

    /// 把会话 A 拖到显示会话 B 的窗格上，两格内容交换，且置「待落盘」标志
    /// （即时落盘链路复用）。
    #[test]
    fn dropping_a_tab_on_another_pane_swaps_the_two_panes() {
        let mut win = two_pane_window();
        let before = win.layout.tabs(&win.pane_groups);
        let id0 = before[0];
        let id1 = before[1];
        let p1 = win.pane_of_session(id1).expect("id1 所在的窗格必须存在");
        assert!(!win.pending_save_layout, "起始不应有待存标志");
        // 模拟「从标签栏拖起 id0、松手落在显示 id1 的窗格」——`draw_node` 命中落点后调用。
        win.drag_session = Some(id0);
        assert!(win.drop_drag_on_pane(p1), "落点命中应返回 true");
        // 两片窗格内容互换：原来 [id0, id1] 现在 [id1, id0]。
        let after = win.layout.tabs(&win.pane_groups);
        assert_eq!(after, vec![id1, id0], "两格内容交换");
        assert!(win.drag_session.is_none(), "落点命中后拖拽态清空");
        assert!(
            win.pending_save_layout,
            "结构变了，置待落盘标志（A2 即时落盘链路复用）"
        );
        // 身份集合守恒、无重复。
        let set: std::collections::HashSet<_> = after.into_iter().collect();
        assert_eq!(set.len(), 2, "身份集合不变、不重复");
    }

    /// 拖回自己那片（同身份）是空操作，不算命中、不触发落盘、不清拖拽态
    /// （拖拽态交给帧末 flush，与「落空」同口径）。
    #[test]
    fn dropping_a_tab_on_its_own_pane_is_a_no_op() {
        let mut win = two_pane_window();
        let id0 = win.layout.tabs(&win.pane_groups)[0];
        let p0 = win.pane_of_session(id0).expect("id0 所在的窗格必须存在");
        win.drag_session = Some(id0);
        assert!(!win.drop_drag_on_pane(p0), "同身份落点不算命中");
        assert_eq!(win.layout.tabs(&win.pane_groups)[0], id0, "树原样");
        assert!(
            win.drag_session.is_some(),
            "拖回自己不清拖拽态（留给帧末 flush）"
        );
        assert!(!win.pending_save_layout, "空操作不应触发落盘");
    }
}

/// 把一次用例包在「一次性 profile 目录」里，退出时还原环境变量。
///
/// 应用面（`layout.json`）与窗口透明度（`prefs.json`）两组用例都会写盘，必须
/// 各自沙箱到临时目录。**进程级环境变量只能有一把锁**，所以锁放在这里共用：
/// 各模块各持一把的话，两组用例会互相看见对方的目录（这正是本夹具取代
/// `window_opacity_tests` 里那份私有副本的原因——原来那个锁只锁得住自己）。
pub(in crate::ui) fn with_temp_profile<T>(check: impl FnOnce() -> T) -> T {
    static PROFILE_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
    static SEQ: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    let _guard = PROFILE_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let dir = std::env::temp_dir().join(format!(
        "windterm-test-profile-{}-{}",
        std::process::id(),
        SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    ));
    let _ = std::fs::remove_dir_all(&dir);
    let _ = std::fs::create_dir_all(&dir);
    let previous = std::env::var("WINDTERM_PROFILE_DIR").ok();
    std::env::set_var("WINDTERM_PROFILE_DIR", &dir);
    let result = check();
    match previous {
        Some(value) => std::env::set_var("WINDTERM_PROFILE_DIR", value),
        None => std::env::remove_var("WINDTERM_PROFILE_DIR"),
    }
    let _ = std::fs::remove_dir_all(&dir);
    result
}

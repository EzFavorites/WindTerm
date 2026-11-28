//! 窗口的初始状态（ 从 `ui/mod.rs` 拆出，4j：那里的行数上限）。
//!
//! 这里只回答一个问题：**一个刚启动的窗口长什么样**。出厂默认会话、
//! 工具按钮、表单初值、从会话库读回来的会话树——都在这一个函数里。
//! 拆出来还有个好处：`mod.rs` 只留「每帧做什么」，不用在两百行字段初值里翻。

use super::*;
// 目录树里的两个新模块 ：`use super::*` 看不到私有 `mod`，要显式引入。
use crate::ui::{comp, harness};
use crate::ui::startup_defaults::startup_defaults;
use term_profile::prefs::Prefs;
use crate::ui::layout::{PaneGroup, PaneId};
use crate::ui::layout::mint_pane_identity;

/// prefs 里的全局代理 → 控制面能认的类型；0 档 / 缺主机 / 缺端口都当没配。
fn global_proxy_from_prefs(prefs: &Prefs) -> Option<term_core::session::GlobalProxy> {
    use term_core::session::{GlobalProxy, ProxyType};
    let kind = match prefs.global_proxy_type {
        1 => ProxyType::Http,
        2 => ProxyType::Socks5,
        _ => return None,
    };
    if prefs.global_proxy_host.trim().is_empty() || prefs.global_proxy_port == 0 {
        return None;
    }
    Some(GlobalProxy {
        kind,
        host: prefs.global_proxy_host.clone(),
        port: prefs.global_proxy_port,
    })
}

impl TerminalWindow {
    pub fn new(cc: &eframe::CreationContext<'_>) -> Self {
        // 控制面先建：下面每一条 tab 都要经它开会话（ADR-0019 档 1.1）。
        // 前台 GUI 独持会话池；`api` / `mcp` 只是它的控制端点客户端（ADR-0030）。
        let pool = std::sync::Arc::new(term_session::SessionPool::new());
        // Before* 事件先入队，设置页规则在下一帧的 draw 里求值——构造期还没有规则表。
        let lifecycle_events = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let sink_events = Arc::clone(&lifecycle_events);
        pool.set_event_sink(Some(Arc::new(move |on| {
            if let Ok(mut q) = sink_events.lock() {
                q.push(on);
            }
        })));
        // 会话从 SQLite 会话库读（`persist::load`）。库不存在 / 读不出来
        // 才是首次启动：探测本机可用 shell，按原版 `Shell sessions` 分组造
        // 一条一条本地 Shell 会话。已有会话库时 `startup_defaults` 会原样
        // 交回，探测闭包连调用都不会发生（用户数据绝不被动）。
        // 登录名同样由宿主注入：`term-core` 不读 `$USER`，读环境落在 `term-io`。
        let (groups, sessions) = startup_defaults(
            term_runtime::persist::load(),
            term_io::host_shells::detect_host_shells,
            term_io::host::login,
        );

        // 语言从偏好库读（读不到 / 标签不认就回默认英文）。这是**初值**，不是
        // 第二写入者：之后的每一次切换都走 `ShellAction::SetLanguage` → reduce。
        // 同一份 prefs 还带着两个「不再提示」抑制位（关闭确认弹窗用）。
        let prefs = term_runtime::prefs::load().unwrap_or_default();
        // 调色板按偏好加载：菜单选过的终端 scheme 优先，否则整窗主题
        // （老 prefs 没 scheme_name / theme_name 时回落 dige-black）。
        let palette = effects::theme::startup_palette(&prefs.scheme_name, &prefs.theme_name);
        pool.set_global_proxy(global_proxy_from_prefs(&prefs));
        // OneKey 方案表必须在**第一条会话之前**就位：控制面只在自己开会话时
        // 读它（`SessionPool::resolve_onekey`），而系统 `ssh` 那条路径不经过
        // 任何界面代码。晚一步推，下面 `make_tab` 起的那条 tab 就已经按
        // 未覆盖的配置起来了。
        pool.set_onekeys(prefs.onekeys.clone());
        let lang = std::env::var("WINDTERM_LANG")
            .ok()
            .and_then(|tag| term_app::i18n::Lang::from_tag(&tag))
            .or_else(|| term_app::i18n::Lang::from_tag(&prefs.language))
            .unwrap_or_default();

        let mut tabs = Vec::new();
        // Start with a live local shell tab on the first session.
        if let Some(shell_cfg) = sessions.first().cloned() {
            tabs.push(make_tab(
                &pool,
                shell_cfg.clone(),
                egui::Id::new("tab_local_shell"),
                &cc.egui_ctx,
            ));
        }

        // 「上一次激活」的历史从当前标签开始：没有它时那一档只能退到前一条。
        // 先算好——`tabs` 在下面的结构体字面量里被移走了。
        let tab_focus = tabs.first().map(|t| vec![t.id]).unwrap_or_default();
        // ADR-0025 阶段 0：恒在身份顺序与活动身份，从刚建好的 tabs 反推。
        let open_order = tabs.iter().map(|t| t.identity).collect();
        let active = tabs.first().map(|t| t.identity);

        let (shortcut_map, seq_table) = crate::ui::dialogs::shortcuts::build_runtime(&prefs);
        // ④-0：初始单窗格 = 一个 PaneId + 一片只含当前会话的 group。
        let init_pane = mint_pane_identity();
        let mut pane_groups: std::collections::HashMap<PaneId, PaneGroup> = Default::default();
        if let Some(s) = active {
            pane_groups.insert(init_pane, PaneGroup { order: vec![s], active: s });
        }

        // 主题自带字体只在开关打开且主题真的声明了它时才覆盖用户选择；
        // 缺任一项就回落 prefs，老主题不会突然换字体。
        let (font_families, font_weight) = if prefs.prefer_theme_fonts {
            let (family, weight) = palette.theme_fonts();
            (
                family.map(|f| [f, String::new(), String::new(), String::new()])
                    .unwrap_or_else(|| prefs.font_family_value()),
                weight.unwrap_or_else(|| prefs.font_weight_value()),
            )
        } else {
            (prefs.font_family_value(), prefs.font_weight_value())
        };

        Self {
            pool,
            tabs,
            open_order,
            active,
            layout: Layout::single(init_pane),
            pane_groups,
            palette,
            // 字号在 `install` 之前就位：`load_with` 只给出厂 13pt，用户存过的
            // `prefs.font_size`（0 哨兵 → 出厂值）在这里换上去，第一次
            // `install` 就按它装字形、按它量 cell——不会先画一帧 13pt 再跳。
            // 字族 / 字重同理：四槽与目标字重一并交给 `load_with`，也是为了让
            // 第一帧就是用户存的那套字（`load_with` 内部按需扫一次字体目录，
            // 四槽全空时一次 IO 都不做）。
            fonts: {
                let mut fonts = TerminalFonts::load_with(font_families, font_weight);
                fonts.set_size(prefs.font_size_value());
                fonts
            },
            groups,
            sessions,
            selected_session: 0,
            session_filter: String::new(),
            panel_filters: crate::ui::layout::PanelFilters::default(),
            shell_tabs: Vec::new(), pending_ip_hit: None,
            shell_active: None,
            shell_filter: String::new(),
            shell_list_open: false,
            closed_sessions: Vec::new(),
            tools: default_tools(),
            new_session: NewSessionData {
                protocol: Protocol::Shell,
                name: "New Session".to_string(),
                host: String::new(),
                port: Protocol::Shell.default_port(),
                username: String::new(),
                password: String::new(),
                auth: AuthMethod::Password,
                // 新会话勾选集按金标准 `14-ssh-authentication.png`：ssh-agent 空，
                // 其余四种都勾（首选 password，其余进回退链）。
                auth_chain: vec![AuthMethod::PublicKey, AuthMethod::KeyboardInteractive, AuthMethod::GssapiWithMic],
                group: term_core::config::DEFAULT_GROUP.to_string(),
                keep_alive: term_core::config::KEEP_ALIVE,
                connection_timeout: term_core::config::CONNECTION_TIMEOUT,
                tcp_keep_alive: term_core::config::TCP_KEEP_ALIVE,
                tcp_low_delay: term_core::config::TCP_LOW_DELAY,
                // 新会话默认用内置客户端（与 `SessionConfig::new` 一致）。
                builtin_ssh: true,
                // 新建会话的光标样式初值取 `Text ▸ Caret` 的偏好。
                caret_style: prefs.caret_style_index(),
                term: term_core::config::TERM_TYPES
                    .iter()
                    .position(|t| *t == term_core::config::DEFAULT_TERM)
                    .unwrap_or(0),
                description: String::new(),
                charset: term_core::config::CHARSET.to_string(),
                system: "linux".to_string(),
                // 自动登录是 OneKey 的内置语义，选中方案即生效（默认 None）。
                onekey: "None".to_string(),
                sftp: true,
                sysmon: false,
                shell_command: String::new(),
                shell_arguments: "-1 -1".to_string(),
                shell_exec_command: String::new(),
                shell_exec_delay: 1,
                shell_locale: "Automatic (zh_CN.UTF-8)".to_string(),
                shell_env_vars: Vec::new(),
                selected_page: 0,
                forwards: String::new(),
                forward_error: None,
                proxy_jump: String::new(),
                auth_identity: Default::default(), auth_exec_command: Default::default(), auth_exec_delay: [1, 1, 1],
                auth_tab: term_core::session::auth_platform(), auth_vault_open: false, auth_vault_note: None, verify_host_key: true,
                rekey_time_minutes: 60, rekey_data_mbytes: 1000,
                agent_forward: false,
                x11_forward: false,
                x11_screen: 0,
                proxy_type: 0,
                proxy_host: String::new(),
                proxy_port: 1080,
                proxy_command: String::new(),
                working_dir: "${HomeDir}".to_string(),
                // 空串 = 未设置：页面显示出厂串，读侧回落到同一串。
                word_separators: String::new(),
                word_separators_alt: String::new(),
                bracket_pairs: String::new(),
                sftp_preserve_mtime: true,
                sftp_preserve_dir_mtime: false,
                sftp_fix_invalid_path: true,
                clipboard_read_policy: 0,
                clipboard_write_policy: 0,
                modem_protocol: 2,
                modem_upload_dir: String::new(),
                modem_download_dir: String::new(),
                modem_conflict_policy: 0,
                modem_open_after: false,
                modem_use_default: false,
                modem_commands: ["rx".into(), "rb".into(), "rz".into()],
                modem_large_packets: [false, true, false],
                dns_over_proxy: true,
                initial_auto_wrap: true,
                initial_cursor_keys_application: false,
                backspace_sends: 0,
                delete_sends: 0,
                alt_modifier_sends: 0,
                meta_modifier_sends: 1,
                bell_style: 1,
                bell_sound: String::new(),
                bell_dedup_secs: 2,
                logon_script: String::new(),
                cols: term_core::config::DEFAULT_COLS,
                rows: term_core::config::DEFAULT_ROWS,
                scrollback: term_core::config::DEFAULT_SCROLLBACK,
                wheel_lines: term_core::config::MOUSE_WHEEL_SCROLL_LINES,
                baud_rate: u32::from(term_core::config::SERIAL_BAUD),
                data_bits: term_core::config::SERIAL_DATA_BITS,
                stop_bits: term_core::config::SERIAL_STOP_BITS,
                parity: term_core::config::SERIAL_PARITY.to_string(),
                flow_control: term_core::config::SERIAL_FLOW_CONTROL.to_string(), reconnect: Default::default(),
            },
            session_dialog: SessionDialogKind::New { group: None },
            // Default Session Settings 的落盘模板：用户没保存过时回落出厂值。
            default_session: prefs.default_session.clone().unwrap_or_else(|| SessionConfig::new(Protocol::Ssh)),
            // 快捷命令表出厂四组十五项；文件坏 / 不存在都回默认，不拦启动。
            quick_bar: term_runtime::quickbar::load(),
            // 三个显示开关从 prefs 读初值（兼容终端 user.config 的位置）；
            // 命令配置弹窗 Ok 时再同步这一份渲染副本。
            quick_bar_display: prefs.quick_bar_display(),
            settings: SettingsData {
                term: "xterm-256color".to_string(),
                scrollback: "999999".to_string(),
                font: "Menlo".to_string(),
                // 命令中心拿它当「默认字号」显示；启动时就是生效值，
                // 免得用户在设置里改成 18 之后这行还写着 13。
                font_size: prefs.font_size_value(),
                groups_open: [true; 3],
            },
            // 光标样式偏好（0 哨兵 = 老文件没这项 → 出厂 Block）。
            caret_style: prefs.caret_style_index(),
            // ：光标周期 / 宽度，同一份 prefs（构造时读一次，设置页同步）。
            caret_period: prefs.caret_period_value(),
            caret_width: prefs.caret_width_value(),
            // Text 页的三行与已移除 Behaviour 页的三开关：同一份 prefs（构造时读一次；
            // 三页移除后这些值只能由 prefs 文件配置，窗口副本仍供原有读者使用）。
            // 出厂值都等于「现在就是这样」。
            alt_row_bg: prefs.text_alternate_row_bg,
            hide_cursor_typing: prefs.text_hide_cursor_while_typing,
            // Highlight 页第 4 行：出厂 `true`（这个效果本来就无条件在画，
            // 见 `Prefs::text_highlight_search` 的注释）。
            highlight_search: prefs.text_highlight_search,
            // Wrap 页的两项真控件：同一份 prefs，同一写法（构造时读一次，
            // 设置页改完写回并同步窗口副本 + 活动标签的 marks）。
            wrap_symbols: prefs.view_wrap_symbols,
            column_marker_col: prefs.view_column_marker,
            wrap_settings: term_app::wrap::WrapSettings::from_prefs(prefs.wrap_mode, prefs.wrap_column),
            // I-AC：补全偏好一次读入运行期副本，与 wrap 两项同一写法。
            completion: prefs.completion.clone(),
            completion_state: Default::default(),
            // hex 的**默认参数**（设置 ▸ Binary 页那四个数 + 一格斑马线）：
            // 0 哨兵与越界值在 `from_prefs` 一处夹掉。建标签时用它做初值，
            // `View ▸ Hex *` 改的是标签上那份副本、不落盘。
            //
            // 斑马线也在这里：它是同一个设置页上的第五个同类参数，**没有**理由
            // 单独留在窗口上（留在窗口上就意味着勾一下当场改掉所有已开标签）。
            hex_defaults: crate::ui::terminal::HexParams::from_prefs(
                prefs.hex_base, prefs.hex_columns, prefs.hex_group, prefs.hex_division, prefs.alternate_column_bg,
            ),
            focus_new_tab: prefs.focus_new_tab,
            // Tabs 页三项：同一份 prefs，同一写法（构造时读一次，设置页改完
            // 写回并同步窗口上的副本）。出厂档全部等于「现在就是这样」。
            tab_activate_after_close: prefs.tab_activate_after_close_kind(),
            tab_double_click: prefs.tab_double_click_action(),
            tab_elide: prefs.tab_elide_mode(),
            tab_numbers: prefs.tab_number_mode(),
            tab_middle_click: prefs.tab_middle_click_action(),
            tab_right_click: prefs.tab_right_click_action(),
            show_address_bar: prefs.show_address_bar,
            auto_hide_menu_bar: prefs.auto_hide_menu_bar,
            menu_bar_peek_until: 0.0,
            lock_timeout_secs: prefs.lock_timeout_secs,
            last_lock_activity: 0.0,
            reconnect: prefs.effective_reconnect().into(),
            prefer_theme_fonts: prefs.prefer_theme_fonts,
            tab_hover_activate: prefs.tab_hover_activate,
            tab_hover_delay_ms: prefs.tab_hover_delay_ms,
            tab_hover: None,
            global_proxy_type: prefs.global_proxy_type,
            global_proxy_host: prefs.global_proxy_host.clone(),
            global_proxy_port: prefs.global_proxy_port,
            tab_thumbnail_pct: prefs.tab_thumbnail_pct,
            filer_rect: None, path_picker: None, upload_picker: None,
            clipboard_requests: Vec::new(),
            clipboard_ask: None,
            // 触发器规则从同一份 prefs 读（构造时一次，之后由设置页同步）。
            // 空数组 = 出厂态，求值器收不到任何规则、整条链路静默短路。
            triggers: prefs
                .triggers
                .iter()
                .map(crate::ui::dialogs::triggers::model::ViewRule::from_core)
                .collect(),
            // OneKey 登录方案：构造时从同一份 prefs 读一次，OneKeys 编辑页负责同步。
            onekeys: prefs.onekeys.clone(),
            // Tunnels 设置：与上面两条同一条纪律——构造时读一次，
            // 对话框 `Ok` 时写回 prefs 并 `reload_tunnels ` 同步回来。
            tunnel_editor: None,
            tunnel_profiles: prefs.tunnels.clone(),
            tunnels_enabled: prefs.tunnels_enabled,
            // 激活历史从当前标签开始：没有它时「上一次激活」只能退到前一条。
            tab_focus,
            // `show_toolbar` 出厂 `true`（在此之前工具行无条件绘制）；它不落盘，
            // 重启回默认——与菜单栏 / 状态栏 / 快捷栏三个开关同一条边界。
            show_sync_channels: false,
            view: ViewState {
                lang,
                // D6：「要不要打开首次启动引导」的初值来自**这一处已经读好的**
                // `prefs`（本函数开头那次 `load `），不另读一遍——同一个文件在
                // 同一毫秒里读两次只会得到两个可能不同的答案，而它们没有理由不同。
                // 真正的 `open_onboarding `（探测 + 置位）在 `init ` 里，
                // 那里还要过一道夹具闸门。
                show_onboarding: !prefs.onboarded,
                ..ViewState::default()
            },
            term_view: crate::ui::terminal::LineCache::default(),
            term_view_shell: crate::ui::terminal::LineCache::default(),
            selection_text: String::new(),
            selection_text_shell: String::new(),
            // W5-I：命令行历史容量 / TTL 取自同一份 prefs（0 哨兵回落 30 天 / 10000 条）。
            command_history: prefs.command_history_store(),
            ctx: cc.egui_ctx.clone(),
            // N：首次周期留存立即触发（0 表示「从没留存过」）。
            last_periodic_save: 0.0,
            // 结构变化即时落盘标志（默认 false = 本帧无待存结构变化）。
            pending_save_layout: false,
            // A3：拖拽态（默认无拖拽、本帧未松手）。
            drag_session: None,
            drag_released_this_frame: false,
            restore_focus: false,
            restore_focus_shell: false,
            pending_select: None,
            find_input: String::new(),
            palette_input: String::new(),
            goto_input: String::new(),
            center_input: String::new(),
            lock_password: String::new(), show_lock_password: false,
            shortcuts: shortcut_map,
            shortcut_capture: None,
            shortcut_new_id: String::new(),
            keyseq: term_app::shortcuts::KeySeqState::new(),
            seq_table,
            tab_colors: Vec::new(),
            tmux_log: Vec::new(),
            tunnel_input: String::new(),
            tunnel_error: None,
            tmux_input: String::new(), tmux_send: String::new(), tmux_pane_name: String::new(), tmux_session_name: String::new(),
            tmux_blocks: Vec::new(),
            connector: None,
            ssh_password: String::new(),
            save_password: false,
            tunnels: Vec::new(),
            vault: None,
            vault_form: crate::ui::dialogs::vault::VaultForm::default(),
            log_note: None,
            // 同一份 prefs 也带着 Logging 配置页的五个字段（老文件缺项回落视觉基线默认值）。
            log_settings: dialogs::LogSettingsData::from_prefs(Some(&prefs)),
            // Filer 的隐藏项开关初值取同一份 prefs（与 `caret_style` 同一条
            // 写法：prefs 只在构造时读一次，之后由设置页写回并同步这份副本）。
            filer: {
                let mut filer = crate::ui::panels::Filer::default();
                filer.show_hidden = prefs.filer_show_hidden;
                filer
            },
            script: None, script_manager: None,
            // 会话树右键的 Logon Script 编辑框：出厂关着。
            logon_script_editor: None,
            // 脚本停靠面板：常驻状态（路径 / 在跑的线程 / 结果 / 错误）。
            script_panel: crate::ui::panels::ScriptPanel::default(),
            // 三处 Run 共用的一次性授权 / 预算档：出厂全关，不读盘、不落盘。
            run_config: Default::default(),
            // 脚本新开会话的交接队列（ADR-0025 阶段 5）：出厂空队列，脚本线程往里
            // 推一条，`absorb_script_opened` 下一帧就把它变成标签页。
            script_opened: Default::default(),
            script_activated: Default::default(),
            script_asks: Default::default(),
            script_picks: Default::default(),
            script_selection: Default::default(),
            script_active: Default::default(),
            script_ask: None,
            script_runs: Default::default(),
            logon_fired: Default::default(),
            auth_exec_pending: Default::default(),
            auth_exec_fired: Default::default(),
            logon_runs: Vec::new(),
            trigger_script_runs: Vec::new(),
            script_recent: prefs.recent_scripts.clone(),
            recorder: Default::default(),
            // 关闭确认弹窗：两个「不再提示」抑制位从 prefs 读回，其余默认。
            confirm: dialogs::confirm::ConfirmState::from_prefs(&prefs),
            // Terminal 页的两个粘贴确认开关：出厂 false = 粘贴立刻写进 PTY，
            // 与 `focus_new_tab` 一样只在构造时读一次，设置页负责同步。
            paste_confirm_main: prefs.paste_confirm_main,
            paste_confirm_alt: prefs.paste_confirm_alt,
            // Terminal ▸ Mouse Actions：右镜档位与选中即复制，同样只在构造时读一次。
            right_click: prefs.right_click_kind(),
            copy_on_select: prefs.copy_on_select,
            mouse_events: term_app::mouse::EventMask {
                move_event: prefs.mouse_move_event,
                wheel_event: prefs.mouse_wheel_event, click_event: prefs.mouse_click_event,
                middle_event: prefs.mouse_middle_event, double_event: prefs.mouse_double_event,
                right_event: prefs.mouse_right_event,
            },
            copied_on_select: None,
            copied_on_select_shell: None,
            pending_paste: None,
            // ：文件传输（句柄 + 路径弹层），两者都从「没有」开始。
            xfer: None,
            xfer_prompt: None,
            demo: None,
            demo_preview_hover_y: None,
            comp_demo: None,
            // 启动会话策略从同一份 prefs 读。老文件缺键 → 空串 →
            // parse_tag 回落 LastSessions（视觉基线出厂态），不需要 migration。
            startup: term_app::startup::StartupPolicy::from_tags(
                &prefs.startup_mode,
                &prefs.startup_sessions,
            ),
            // 恢复流程在 init  里登记待连标签；构造时还没有恢复任何东西。
            pending_activation: Vec::new(),
            dock: DockLayout::new(),
            // 应用控制面：脚本请求队列与两个版本号（Q2 刀1）。
            app_queue: Default::default(),
            app_rev: 0,
            prefs_rev: 0,
            dock_drag: None,
            band_tabs: [None, None, None],
            float_focus_tries: Vec::new(),
            // 截图夹具：`WINDTERM_DOCK_MAX=1` 让底带首帧就处于展开态，供
            // `--screenshot` 出图核对「向上展开覆盖到菜单栏」。默认不设，
            // 42 张视觉基线一个像素都不变（与 `WINDTERM_GEAR_DEMO` 同理）。
            bottom_maximized: std::env::var("WINDTERM_DOCK_MAX").is_ok(),
            bottom_split: None,
            pending_dock_cmds: Vec::new(),
            bottom_height: crate::ui::layout::dock_bottom::DEFAULT_H,
            sender: crate::ui::panels::SenderPanel::seeded(
                // 截图夹具：`WINDTERM_SENDER_PAGES=10` 让 Sender 首帧就有 10 页，
                // 供 `--screenshot` 核对底部那条数字 tab 条（视觉基线 `0..9`）。
                // 默认不设 = 一页（条不画），生产路径零变化。
                std::env::var("WINDTERM_SENDER_PAGES")
                    .ok()
                    .and_then(|v| v.parse().ok())
                    .unwrap_or(1),
            ),
            settings_page: crate::ui::dialogs::SettingsPage::default(),
            onekeys_edit: None,
            onekeys_password_draft: None,
            onekeys_ki_draft: None,
            tab_list_filter: String::new(),
            // L1/L3：出厂没有话说、也没有崩过。两条都只由 `announce_recovery`
            // 与 `main.rs` 的 `catch_unwind` 写入。
            notice: None,
            lifecycle_events,
            frame_panics: 0,
            dock_rects: Vec::new(),
            toolbar_pos: None,
            // 运行期副本从盘上接过来。`transparent` 的「已应用值」初始化成
            // 同一个值——`main.rs` 建窗时用的就是 `with_transparent(它)`，
            // 所以第一帧不该再发一次重复的视口命令。alpha 那一侧分两种：
            //   · 存过半透明 → 留 None，第一帧由 `sync_window_opacity` 把
            //     `egui::Visuals` 的背景 alpha 装上（构造时还没有 ctx）；
            //   · 出厂不透明 → 直接记 1000，第一帧整段跳过，一个像素都不碰。
            applied_window: {
                let initial = WindowOpacity::from_prefs(&prefs);
                let opaque = initial.alpha() >= term_runtime::prefs::WINDOW_OPACITY_MAX;
                AppliedWindow {
                    full_screen: None,
                    always_on_top: None,
                    transparent: Some(initial.transparent),
                    alpha_permille: if opaque { Some(1000) } else { None },
                }
            },
            window_opacity: WindowOpacity::from_prefs(&prefs),
            // 地址栏浮层出厂关闭（没有任何锚点、没有待显示的报错）。
            // `recent` 也留空——它每次打开时从 `prefs.json` 读一次。
            address: crate::ui::address::AddressBar::new(),
            // D6：引导状态机出厂「关着、停在 Welcome、零探测结果」。
            // 探测要读文件（IO），所以不在这里做——由 `init ` 的
            // `open_onboarding ` 一次性完成，且只在真的要显示时才做。
            onboarding: term_app::onboarding::OnboardingState::new(),
            profile_choice: overlays::ProfileChoiceState::default(),
            // D6：预览窗出厂「关着、没有内容」。不在构造时读任何文件——真读文件
            // 是 `open_preview_local` 的事，而那是用户点了才发生的。
            preview: crate::ui::overlays::PreviewState::default(),
            // 脚本编辑器出厂「没开过」。不开窗口不读任何文件。
            script_editor: None,
        }
    }

    pub fn init(&mut self, cc: &eframe::CreationContext<'_>) {
        self.fonts.install(&cc.egui_ctx);
        // V1-1：还原上次的标签 / 分屏结构（不还原进程态）。
        // 截图基线用一次性 profile 目录，那里没有 layout.json，不会触发。
        // `Application.NewWindow` 拉起的第二实例**不还原**：layout.json 是单份
        // 全局状态，照常恢复会长成旧窗口的副本。此时 `new ` 已建好的那一条
        // live 本地 shell 标签就是干净的出厂窗口，不用另造默认态。
        if !harness::secondary_window() {
            self.restore_layout();
            // L3（O 设计稿 §3e）：上次没走正常退出就说一声。与 `restore_layout`
            // 分开放，是因为「恢复出几条标签」和「上次崩没崩」是两件事——会话库
            // 被清空、Startup 落在 No session 时前者啥也不恢复，后者照样成立。
            // 第二实例（`Application.NewWindow`）不碰：它会消费掉那条事实。
            self.announce_recovery();
        }
        // K3-b 出图入口：`WINDTERM_FLOAT_BAND=right` 让整条带首帧就浮起来。
        // 判据只有环境变量、没有任何默认值——生产路径不设它就零变化；这里不能
        // 挂在 `fixture_mode ` 上，因为浮窗现在是**系统原生窗口**，看它必须起
        // 真实进程（`--demo dock` 不带 `--screenshot`），那种进程不是夹具态。
        // 放在 `restore_layout ` 之后：否则会被恢复出来的布局覆盖。
        if let Ok(name) = std::env::var("WINDTERM_FLOAT_BAND") {
            if let Some(zone) = DockZone::parse(&name) {
                // `init ` 里 Context 还没跑过第一帧，不能问 `available_rect `
                // （egui 会 panic）。截图窗口固定 1280×760，直接给逻辑坐标。
                // 底带用整宽，与真实 Float 命令的推宽一致（Sender 控件不能被裁）。
                let rect = if zone == DockZone::Bottom {
                    egui::Rect::from_min_size(egui::pos2(12.0, 140.0), egui::vec2(1256.0, 360.0))
                } else {
                    egui::Rect::from_min_size(egui::pos2(680.0, 240.0), egui::vec2(440.0, 460.0))
                };
                self.dock.band_float(zone, rect);
            }
        }
        // D6：首次启动引导。判据是 `Prefs::onboarded`（`new ` 里从同一份
        // prefs 取到的初值），**不是**「布局文件不存在」——后者把「删过布局」
        // 误当成「第一次用」，也会让「从没改过设置但用了半年」的用户被重新引导。
        //
        // 夹具（`--screenshot` / `--comp`）一律不出引导：42 张基线都是在
        // 「不是第一次用」的机器上冻结的，让一次性的引导出现在截图里，
        // 等于给每一张图都加一层不该有的窗口。
        if std::env::var_os("WINDTERM_SHOW_PROFILE_CHOICE").is_some() {
            // 出图 / 人工调试用的显式开关：生产路径无人设置，零影响。
            self.view.show_onboarding = false;
            self.view.show_profile_choice = true;
        } else if harness::fixture_mode() {
            self.view.show_onboarding = false;
            self.view.show_profile_choice = false;
        } else if !term_paths::has_profile_dir_choice() {
            // 第一次启动先选 profile 目录；确认后再开 Welcome 引导。
            self.view.show_profile_choice = true;
        } else if self.view.show_onboarding {
            self.open_onboarding();
        }
    }

    /// 视觉基线夹具入口（`--demo scroll`）。只在截图流程里用。
    pub fn set_demo(&mut self, spec: crate::demo::DemoSpec) {
        self.demo = Some(spec);
        // 弹窗夹具在首帧之前就打开对话框——终端区只负责把静态画面铺好。
        if spec.open_new_session {
            self.view.show_new_session = true;
            harness::quiet_diagnostics(&self.ctx);
        }
    }

    /// 把文件面板钉在一组固定条目上（截图流程用）。
    ///
    /// 真实家目录每台机器都不同，逐字节基线会永远红。注意 `screenshot-round64.png`
    /// 那张「贴底状态」**不带 `--demo`**，所以注入不能只挂在 `set_demo` 上——
    /// `main.rs` 见到 `--screenshot` 就会调一次。
    pub fn use_demo_files(&mut self) {
        self.filer = crate::ui::panels::Filer::with_demo_rows(crate::demo::demo_files());
    }

    /// 组件夹具入口（`--demo comp:<name>`）：整窗只画这一个组件。
    ///
    /// 返回 `false` 表示名字不认识——调用方负责报错退出，**不静默出一张空图**。
    /// 返回布尔而不是 `CompId`：`CompId` 属于 `ui` 内部，命令行不需要认识它。
    pub fn set_comp_demo(&mut self, name: &str) -> bool {
        let Some(comp) = comp::CompId::parse(name) else {
            return false;
        };
        self.comp_demo = Some(comp);
        // 组件出图同样要安静：热身与真画共用 widget id。
        harness::quiet_diagnostics(&self.ctx);
        true
    }

    /// 从 `prefs.json` 重新读入触发器规则，同步到运行期缓存。
    ///
    /// 设置页「保存」后调用：把编辑器里那份（已写回 prefs）重新读回来，保证
    /// 内存副本与磁盘一致。`load ` 失败回落空规则集（等于「没有规则」），不崩。
    pub(in crate::ui) fn reload_triggers(&mut self) {
        self.triggers = term_runtime::prefs::load()
            .map(|p| {
                p.triggers
                    .iter()
                    .map(crate::ui::dialogs::triggers::model::ViewRule::from_core)
                    .collect()
            })
            .unwrap_or_default();
    }

    /// ：从 `prefs.json` 重新读入命名隧道与总线开关。
    ///
    /// 与 [`reload_triggers`](Self::reload_triggers) 同形；对话框 `Ok` 之后调用。
    /// `load ` 失败回落空列表（等于「一条隧道都没配」），不崩。
    /// 设置对话框按 `Ok` 之后把草稿装回窗口，并按新配置重起转发。
    ///
    /// **不重读盘**：`save_prefs` 刚写进去的就是这一份，再读一次只会多一条
    /// 门面引用（4s 只许减不许增）和一次多余 IO——而且读写之间如果有人手改了
    /// `prefs.json`，读回来的会是「盘上的」，与用户刚点的 `Ok` 不一致。
    pub(in crate::ui) fn apply_tunnels(&mut self, profiles: Vec<term_core::tunnel::TunnelProfile>) {
        self.tunnel_profiles = profiles;
        let enabled = self.tunnels_enabled;
        self.set_tunnels_enabled(enabled);
    }
}

/// 工具按钮的出厂列表（图标 + 提示 + 动作）。
fn default_tools() -> Vec<ToolButton> {
    vec![
        ToolButton {
            icon: "⬜",
            tooltip: "New Session",
            action: ToolAction::NewSession,
        },
        ToolButton {
            icon: "🔌",
            tooltip: "Connect",
            action: ToolAction::Connect,
        },
        ToolButton {
            icon: "🚫",
            tooltip: "Disconnect",
            action: ToolAction::Disconnect,
        },
        ToolButton {
            icon: "🔄",
            tooltip: "Refresh",
            action: ToolAction::Refresh,
        },
        ToolButton {
            icon: "⚙",
            tooltip: "Settings",
            action: ToolAction::Settings,
        },
        ToolButton {
            icon: "⌨",
            tooltip: "Terminal",
            action: ToolAction::Terminal,
        },
        ToolButton {
            icon: "📁",
            tooltip: "SFTP",
            action: ToolAction::Sftp,
        },
        ToolButton {
            icon: "📡",
            tooltip: "Serial",
            action: ToolAction::Serial,
        },
    ]
}

impl TerminalWindow {
    /// **新开**一条会话的出厂配置。
    ///
    /// 与 `SessionConfig::new` 的唯一区别是补上窗口级偏好——目前只有光标样式。
    /// 只在「凭空造一条会话」的地方用（标签栏兜底标签、分屏新开本地 shell）；
    /// 从会话库打开的那条**不能**走这里：它带着自己的 `caret_style`，改偏好
    /// 不该回头改用户的会话属性。
    ///
    /// P1-5：本地 Shell 的用户名在这里注入（`term_io::host::login`）。
    /// core 不读 `$USER`——同一个入参必须得到同一条会话配置（R0）。
    pub(in crate::ui) fn fresh_session(&self, protocol: Protocol) -> SessionConfig {
        let mut cfg = if protocol == Protocol::Shell {
            SessionConfig::new_local_shell(&term_io::host::login())
        } else {
            SessionConfig::new(protocol)
        };
        cfg.caret_style = self.caret_style;
        // ：光标周期 / 宽度随样式一起进新会话（同一份窗口副本）。
        cfg.caret_period = self.caret_period;
        cfg.caret_width = self.caret_width;
        cfg
    }

    /// D4：用磁盘上读回来的历史替换启动时那份空历史。
    ///
    /// 为什么由宿主（`main.rs`）调而不是在这里自己读文件：文件 IO 在
    /// `term-runtime::persist`，窗口层不认识路径；窗口只领一份已经
    /// `reconfigure` 过的纯数据（容量 / TTL 由 `prefs` 决定）。
    pub fn adopt_command_history(
        &mut self,
        history: term_core::command_history::CommandHistory,
    ) {
        self.command_history = history;
    }

    /// D4：退出时给宿主读的只读视图（宿主负责落盘）。
    pub fn command_history(&self) -> &term_core::command_history::CommandHistory {
        &self.command_history
    }
}

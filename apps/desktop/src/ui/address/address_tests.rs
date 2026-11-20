//! 地址栏投影层的单测：`plan.rs` 的纯函数逐条钉住。
//!
//! 为什么这些判定必须有测试：它们都是「看起来显然」的映射，错了的症状却离
//! 现场很远——名字少了 `:port` 要到两台同名机器并排才看得出；`builtin_ssh`
//! 忘了打开的表现是「地址栏连不上而会话树连得上」；激活意图映射错一格就是
//! 「点 A 连到 B」。这些都不该靠跑起一个窗口去发现。
//!
//! 夹具只造地址相关的字段，其余交给 `SessionConfig::new` 的出厂值。

use super::plan::{self, config_for, config_target, candidates, name_hint, Activation};
use term_app::address::{self, AddressKind, Candidate, Target};
use term_core::session::{Protocol, SessionConfig};

/// 造一个目标（协议 / 用户 / 主机 / 端口）。
fn target(protocol: Protocol, username: &str, host: &str, port: u16) -> Target {
    Target {
        protocol,
        username: username.to_string(),
        host: host.to_string(),
        port,
    }
}

/// 造一条会话配置。
fn session(protocol: Protocol, name: &str, host: &str, port: u16) -> SessionConfig {
    let mut config = SessionConfig::new(protocol);
    config.name = name.to_string();
    config.host = host.to_string();
    config.port = port;
    config
}

/// 一条候选（只填测试关心的那几项）。
fn candidate(kind: AddressKind, label: &str, target_text: &str, index: Option<usize>) -> Candidate {
    Candidate {
        kind,
        label: label.to_string(),
        target: target_text.to_string(),
        hint: String::new(),
        index,
    }
}

#[test]
fn config_for_carries_every_address_field_and_uses_the_builtin_client() {
    let parsed = target(Protocol::Ssh, "u", "h", 22);
    let config = config_for(&parsed, "");
    assert_eq!(config.protocol, Protocol::Ssh);
    assert_eq!(config.username, "u");
    assert_eq!(config.host, "h");
    assert_eq!(config.port, 22);
    // 地址栏的目标没有对应的会话条目，认证只能当场问——所以走进程内客户端。
    assert!(
        config.builtin_ssh,
        "输入即连接必须走内置 SSH 客户端，否则会退化成裸 ssh 提示符"
    );
    // 默认端口不写进名字：`ssh://h:22` 与 `ssh://h` 是同一台机器。
    assert_eq!(config.name, "u@h");
}

#[test]
fn config_for_appends_a_non_default_port_to_the_name() {
    let config = config_for(&target(Protocol::Ssh, "u", "h", 2222), "");
    assert_eq!(config.name, "u@h:2222", "非默认端口必须写出来，否则两台机器同名");
    // 显式给的提示优先于推导（调用方可以自己起名）。
    assert_eq!(config_for(&target(Protocol::Ssh, "u", "h", 2222), "prod").name, "prod");
}

#[test]
fn config_for_without_a_user_names_the_target_by_host_only() {
    let config = config_for(&target(Protocol::Ssh, "", "10.0.0.5", 22), "");
    assert_eq!(config.name, "10.0.0.5");
    // 用户名是空串，不是「当前登录用户」——「我连了谁」不该由本机身份回答。
    assert_eq!(config.username, "");
    // 只有空白的提示等于没给。
    assert_eq!(config_for(&target(Protocol::Ssh, "", "h", 22), "   ").name, "h");
}

#[test]
fn config_for_is_pure_and_touches_no_session_store() {
    // 纯函数：同样的入参必须得到同样的结果，且**不**产生任何「入库」的痕迹。
    // 这里用「连造两次、逐字段相等」钉住无副作用的那一半——真正的不入库由
    // 调用点保证（`connect_target` 只 `tabs.push`，不碰 `self.sessions`）。
    let parsed = target(Protocol::Ssh, "root", "example.com", 2200);
    let first = config_for(&parsed, "");
    let second = config_for(&parsed, "");
    assert_eq!(first.name, second.name);
    assert_eq!(first.host, second.host);
    assert_eq!(first.port, second.port);
    assert_eq!(first.username, second.username);
    // 名字里带的是这个目标本身，不是某条既有会话的名字。
    assert_eq!(first.name, "root@example.com:2200");
}

#[test]
fn candidates_lists_opened_then_sessions_then_recent() {
    let opened = vec![("web".to_string(), "ssh://10.0.0.5:22".to_string())];
    let sessions = vec![session(Protocol::Ssh, "db", "10.0.0.7", 2222)];
    let recent = vec!["ssh://1.2.3.4:22".to_string()];
    let rows = candidates(&opened, &sessions, &recent, "", 40);
    let kinds: Vec<AddressKind> = rows.iter().map(|row| row.kind).collect();
    assert_eq!(
        kinds,
        vec![AddressKind::Opened, AddressKind::Session, AddressKind::Host],
        "空过滤 = 打开即览全部候选，顺序是 已打开 → 会话 → 最近使用"
    );
    assert_eq!(rows[0].label, "web");
    assert_eq!(rows[1].label, "db");
    // 最近使用那段是裸地址串：label 与 target 都填它（画行时不会重复写两遍）。
    assert_eq!(rows[2].label, "ssh://1.2.3.4:22");
    assert_eq!(rows[2].target, "ssh://1.2.3.4:22");
    assert_eq!(rows[2].index, None);
}

#[test]
fn candidates_filters_by_label_or_host() {
    let opened = vec![
        ("web".to_string(), "ssh://10.0.0.5:22".to_string()),
        ("local".to_string(), "shell:///bin/bash".to_string()),
    ];
    let sessions = vec![session(Protocol::Ssh, "prod-db", "10.0.0.7", 22)];
    let recent = vec!["ssh://prod.example.com:22".to_string()];
    // 按标签命中（大小写不敏感）。
    let by_name = candidates(&opened, &sessions, &recent, "WEB", 40);
    assert_eq!(by_name.len(), 1);
    assert_eq!(by_name[0].label, "web");
    // 按主机命中：会话的 host 与最近使用的地址串都算。
    let by_host = candidates(&opened, &sessions, &recent, "prod", 40);
    assert_eq!(
        by_host.iter().map(|row| row.label.as_str()).collect::<Vec<_>>(),
        vec!["prod-db", "ssh://prod.example.com:22"]
    );
    // 什么都不命中就是空（画出来是 No matches）。
    assert!(candidates(&opened, &sessions, &recent, "zzz", 40).is_empty());
}

#[test]
fn candidates_respect_the_cap() {
    let sessions: Vec<SessionConfig> = (0..30)
        .map(|i| session(Protocol::Ssh, &format!("host{i}"), &format!("10.0.0.{i}"), 22))
        .collect();
    assert_eq!(candidates(&[], &sessions, &[], "", 3).len(), 3);
    // `cap == 0` 是空集，不是「不限量」——上限就是上限。
    assert!(candidates(&[], &sessions, &[], "", 0).is_empty());
}

#[test]
fn candidates_drop_the_duplicate_of_a_tab_that_is_also_a_configured_session() {
    // 同一个会话既开着、又在会话库里：只留 `Opened` 那行（切过去永远更便宜）。
    let config = session(Protocol::Ssh, "web", "10.0.0.5", 22);
    let target_text = config_target(&config);
    let opened = vec![("web".to_string(), target_text.clone())];
    let rows = candidates(&opened, &[config], &[], "", 40);
    assert_eq!(rows.len(), 1, "两行长得一模一样会让用户以为手抖点重了");
    assert_eq!(rows[0].kind, AddressKind::Opened);
    // 最近使用里同一条目标串也不该再出现一次。
    let rows = candidates(&opened, &[], std::slice::from_ref(&target_text), "", 40);
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].kind, AddressKind::Opened);
}

#[test]
fn activation_maps_each_kind_to_its_own_landing() {
    let switch = candidate(AddressKind::Opened, "web", "ssh://h:22", Some(3));
    assert_eq!(plan::activation_for(&switch), Some(Activation::SwitchTab(3)));
    let connect = candidate(AddressKind::Session, "db", "ssh://h:2222", Some(1));
    assert_eq!(
        plan::activation_for(&connect),
        Some(Activation::ConnectSession(1))
    );
    let quick = candidate(AddressKind::Host, "ssh://u@h:2200", "ssh://u@h:2200", None);
    assert_eq!(
        plan::activation_for(&quick),
        Some(Activation::QuickConnect(target(Protocol::Ssh, "u", "h", 2200)))
    );
}

#[test]
fn a_broken_recent_target_activates_nothing() {
    // 手改过的 `prefs.json` 里可能有根本解析不了的一条。`None` 是正常返回值：
    // 拿它去连会是「点了没反应」，拿它去索引会话库才是真事故。
    let broken = candidate(AddressKind::Host, "not a host:abc", "not a host:abc", None);
    assert_eq!(plan::activation_for(&broken), None);
    // 下标记为缺失的 `Opened` / `Session` 行同样什么都不做。
    assert_eq!(
        plan::activation_for(&candidate(AddressKind::Session, "x", "ssh://h:22", None)),
        None
    );
}

#[test]
fn the_two_hint_texts_are_both_non_empty() {
    // 视觉基线那一列说的是「按了会发生什么」，所以两种 kind 的提示都必须有内容
    // ——空串会让右端那一列凭空少一块，与视觉基线的行形态不一致。
    let opened_rows = candidates(
        &[("web".to_string(), "ssh://h:22".to_string())],
        &[],
        &[],
        "",
        40,
    );
    let configured_rows = candidates(&[], &[session(Protocol::Ssh, "db", "h", 22)], &[], "", 40);
    assert_eq!(opened_rows[0].hint, "switch to tab");
    assert_eq!(configured_rows[0].hint, "connect");
    for hint in [&opened_rows[0].hint, &configured_rows[0].hint] {
        assert!(!plan::hint_text(term_app::i18n::Lang::En, hint).is_empty());
    }
    // 中文界面下这两条要真的翻了（不然就是地址栏里露英文）。
    assert_eq!(
        plan::hint_text(term_app::i18n::Lang::Zh, "switch to tab"),
        "切换到标签页"
    );
    assert_eq!(plan::hint_text(term_app::i18n::Lang::Zh, "connect"), "连接");
}

#[test]
fn problem_text_keeps_the_offending_fragment() {
    // 报错要能指出是哪一段错：`host:abc` 里的 `abc` 必须原样出现在文案里。
    let bad_port = address::AddressProblem::BadPort("abc".to_string());
    let text = plan::problem_text(term_app::i18n::Lang::En, &bad_port);
    assert!(text.contains("abc"), "报错原文丢了：{text}");
    assert!(!text.contains("{raw}"), "占位符没被替换：{text}");
    let bad_user = address::AddressProblem::BadUser("a b".to_string());
    let text = plan::problem_text(term_app::i18n::Lang::En, &bad_user);
    assert!(text.contains("a b"));
    // 另外两条没有占位符，中英都得有话说。
    for problem in [address::AddressProblem::Empty, address::AddressProblem::NoHost] {
        assert!(!plan::problem_text(term_app::i18n::Lang::En, &problem).is_empty());
        assert!(!plan::problem_text(term_app::i18n::Lang::Zh, &problem).is_empty());
    }
}

#[test]
fn name_hint_follows_the_same_three_rules_as_config_for() {
    // `name_hint` 与 `config_for` 的推导必须一致，否则调用处传进去的提示会把
    // 名字覆盖成另一套规则。
    for parsed in [
        target(Protocol::Ssh, "u", "h", 22),
        target(Protocol::Ssh, "u", "h", 2222),
        target(Protocol::Ssh, "", "h", 22),
        target(Protocol::Telnet, "", "h", 23),
    ] {
        assert_eq!(name_hint(&parsed), config_for(&parsed, "").name);
        // 显式传 `name_hint` 与让它自己推导，也要是同一个名字。
        assert_eq!(
            config_for(&parsed, &name_hint(&parsed)).name,
            config_for(&parsed, "").name
        );
    }
    // Telnet 的默认端口是 23，不写进名字；22 对它来说是**非**默认端口。
    assert_eq!(name_hint(&target(Protocol::Telnet, "", "h", 23)), "h");
    assert_eq!(name_hint(&target(Protocol::Telnet, "", "h", 22)), "h:22");
}

#[test]
fn a_local_shell_tab_shows_its_command_instead_of_an_empty_host() {
    // `Opened` 段的目标串规则必须与 `term_app::address` 里那条会话串逐字相同
    // ——不同则去重失效（两行长得一样却一个切标签一个重连）。
    let mut config = session(Protocol::Shell, "bash", "", 0);
    config.shell_command = "/bin/bash".to_string();
    config.username = "alex".to_string();
    assert_eq!(config_target(&config), "shell:///bin/bash");
    // 远端会话的端口一律写全（右列是给人核对「是不是那台机器」的）。
    assert_eq!(
        config_target(&session(Protocol::Ssh, "db", "10.0.0.7", 2222)),
        "ssh://10.0.0.7:2222"
    );
}

// ── 绘制路径的冒烟测试 ────────────────────────────────────────────────────
//
// 上面那些钉的是**判定**；这两条钉的是「这段绘制代码真的能跑完一帧」。
// 地址栏是唯一一个把 `TextEdit` 抢焦点 + `Frame` + `Area` + 菜单按钮叠在一起
// 的浮层，纯逻辑测试一条都覆盖不到它——而它一旦在运行期炸，症状是整窗崩掉。

/// 跑 `frames` 帧 `draw_address_bar`，返回最后一帧的形状数。
///
/// 第一帧是 `Area` 的 sizing pass（形状全被换成 `Noop`），所以至少要跑两帧
/// 才量得到真内容（与 `harness.rs` 模块文档第 1 条同一个坑）。
fn frames_of_address_bar(win: &mut crate::ui::TerminalWindow, frames: usize) -> usize {
    let ctx = win.ctx.clone();
    let mut shapes = 0;
    for _ in 0..frames {
        let input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(1280.0, 800.0),
            )),
            ..Default::default()
        };
        let out = ctx.run(input, |ctx| win.draw_address_bar(ctx));
        shapes = out.shapes.len();
    }
    shapes
}

#[test]
fn a_closed_address_bar_paints_nothing() {
    // 默认关闭时**一个形状都不许画**：五张整窗视觉基线靠这条保持不动。
    let ctx = egui::Context::default();
    let pool = std::sync::Arc::new(term_session::SessionPool::new());
    let mut win = crate::ui::lifecycle_test_support::window_with(pool, &ctx, Vec::new());
    assert_eq!(frames_of_address_bar(&mut win, 2), 0);
}

#[test]
fn an_open_address_bar_paints_input_and_rows() {
    let ctx = egui::Context::default();
    let pool = std::sync::Arc::new(term_session::SessionPool::new());
    let mut win = crate::ui::lifecycle_test_support::window_with(pool, &ctx, Vec::new());
    win.sessions.push(session(Protocol::Ssh, "db", "10.0.0.7", 2222));
    // 直接用 `AddressState` 播种，而不是 `open_address_bar`：后者要读一次
    // `prefs.json`，测试不该依赖这台机器上的真实偏好（同
    // `lifecycle_test_support` 里「不碰真实 profile 目录」那条口径）。
    win.address.state.open_with("");
    win.address.anchor = Some(egui::Rect::from_min_size(
        egui::pos2(20.0, 120.0),
        egui::vec2(520.0, 26.0),
    ));
    win.address.recent = vec!["ssh://1.2.3.4:22".to_string()];
    assert!(
        frames_of_address_bar(&mut win, 2) > 0,
        "地址栏开着，两帧之后一个形状都没画——输入框与候选行都没出来"
    );
}

// ── 工具栏接线：点一下开地址栏，拖一下照旧拖整条 ─────────────────────────
//
// 这两条守的是同一个改动的两半：面板段的热区从 `Sense::drag()` 换成
// `Sense::click_and_drag()`。只测「点得开」会漏掉「拖不动了」——那是把一个
// 已交付的交互弄坏的典型形状，而屏幕上看不出区别（要上手拖才知道）。

/// 跑一帧工具行，喂一组指针事件。
fn frame_of_tool_row(
    ctx: &egui::Context,
    win: &mut crate::ui::TerminalWindow,
    events: Vec<egui::Event>,
) {
    let input = egui::RawInput {
        screen_rect: Some(egui::Rect::from_min_size(
            egui::Pos2::ZERO,
            egui::vec2(1280.0, 800.0),
        )),
        events,
        ..Default::default()
    };
    // `FullOutput` 是 `#[must_use]`，这里只关心它引起的副作用（窗口字段被改）。
    let _ = ctx.run(input, |ctx| {
        egui::CentralPanel::default().show(ctx, |ui| win.draw_pane_tools(ui));
    });
}

/// 在地址栏浮层上按一下再松开（两帧）。
///
/// 两帧是 egui 的点击语义：`clicked()` 只在「按下又松开且没移动」时为真，
/// 而按下与松开必须落在两个 pass 里。
fn click_at(ctx: &egui::Context, win: &mut crate::ui::TerminalWindow, pos: egui::Pos2) {
    for pressed in [true, false] {
        let input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(1280.0, 800.0),
            )),
            events: vec![egui::Event::PointerButton {
                pos,
                button: egui::PointerButton::Primary,
                pressed,
                modifiers: egui::Modifiers::default(),
            }],
            ..Default::default()
        };
        let _ = ctx.run(input, |ctx| win.draw_address_bar(ctx));
    }
}

/// 先把工具行「热身」一帧（`Area` 的首帧只量尺寸，交互不生效），再喂事件。
fn warm_tool_row(ctx: &egui::Context, win: &mut crate::ui::TerminalWindow) {
    frame_of_tool_row(ctx, win, Vec::new());
    frame_of_tool_row(ctx, win, Vec::new());
}

/// 按下 / 移动 / 松开三种指针事件。
fn pointer(pos: egui::Pos2, pressed: Option<bool>) -> egui::Event {
    match pressed {
        Some(pressed) => egui::Event::PointerButton {
            pos,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: egui::Modifiers::default(),
        },
        None => egui::Event::PointerMoved(pos),
    }
}

/// 面板段里一个「既不在抓手、也不在右端三颗菜单上」的点。
fn panel_point() -> egui::Pos2 {
    egui::pos2(420.0, 13.0)
}

#[test]
fn clicking_the_panel_segment_opens_the_address_bar_anchored_on_it() {
    let ctx = egui::Context::default();
    let pool = std::sync::Arc::new(term_session::SessionPool::new());
    let mut win = crate::ui::lifecycle_test_support::window_with(pool, &ctx, Vec::new());
    assert!(!win.address.state.open, "夹具前提：地址栏出厂是关的");
    warm_tool_row(&ctx, &mut win);
    let spot = panel_point();
    frame_of_tool_row(&ctx, &mut win, vec![pointer(spot, Some(true))]);
    frame_of_tool_row(&ctx, &mut win, vec![pointer(spot, Some(false))]);
    assert!(
        win.address.state.open,
        "点面板段没有打开地址栏——`click_and_drag` 的点击那一半没接上"
    );
    // 锚点必须是那一块面板段，不是窗口左上角：地址栏该贴在用户点的地方下面。
    let anchor = win.address.anchor.expect("点开时应当带上锚点矩形");
    assert!(
        anchor.top() > 0.0 || anchor.left() > 0.0,
        "锚点看起来是零矩形：{anchor:?}"
    );
}

#[test]
fn dragging_the_panel_segment_still_moves_the_whole_row() {
    let ctx = egui::Context::default();
    let pool = std::sync::Arc::new(term_session::SessionPool::new());
    let mut win = crate::ui::lifecycle_test_support::window_with(pool, &ctx, Vec::new());
    assert!(win.toolbar_pos.is_none(), "夹具前提：工具条是停靠的");
    warm_tool_row(&ctx, &mut win);
    let start = panel_point();
    frame_of_tool_row(&ctx, &mut win, vec![pointer(start, Some(true))]);
    // 移动超过 egui 的拖动阈值（约 6 点），于是这一帧是「拖」而不是「点」。
    frame_of_tool_row(&ctx, &mut win, vec![pointer(start + egui::vec2(120.0, 60.0), None)]);
    assert!(
        win.toolbar_pos.is_some(),
        "在面板段上起手拖动没有移动整条工具栏——`click_and_drag` 把拖动吃掉了"
    );
    assert!(
        !win.address.state.open,
        "拖动不该顺手把地址栏打开（那是点击的语义）"
    );
}

#[test]
fn the_arrow_menu_opens_and_its_first_row_opens_the_new_session_dialog() {
    // `▾` 菜单：它必须**真的能点开、点了有反应**。
    // 这条不钉像素坐标，而是在「输入框右端」那条带里找一个能点开的点：
    // 字体 / 行高变了也不该让这条测试变得「红得对不上原因」。
    //
    // 夹具故意**零会话、零最近使用**：候选表为空（下拉只有 No matches），
    // 于是任何落到下拉区域的误点都不会触发连接或写 prefs。
    let ctx = egui::Context::default();
    let pool = std::sync::Arc::new(term_session::SessionPool::new());
    let mut win = crate::ui::lifecycle_test_support::window_with(pool, &ctx, Vec::new());
    win.address.state.open_with("");
    win.address.anchor = Some(egui::Rect::from_min_size(
        egui::pos2(20.0, 120.0),
        egui::vec2(520.0, 26.0),
    ));
    let settle = |win: &mut crate::ui::TerminalWindow| -> usize { frames_of_address_bar(win, 2) };
    settle(&mut win);
    let base_layers = ctx.memory(|m| m.areas().visible_layer_ids().len());

    let mut opened = None;
    'find_arrow: for yi in 0..8 {
        for xi in 0..24 {
            let pos = egui::pos2(500.0 + xi as f32 * 5.0, 150.0 + yi as f32 * 3.0);
            click_at(&ctx, &mut win, pos);
            if ctx.memory(|m| m.areas().visible_layer_ids().len()) > base_layers {
                opened = Some(pos);
                break 'find_arrow;
            }
        }
    }
    let arrow = opened.expect("输入框右端那颗 ▾ 点不开任何菜单");
    // 菜单开着的时候，第一行 `New Session` 应当打开新建会话对话框。
    let mut fired = false;
    for dy in 0..12 {
        let pos = egui::pos2(arrow.x, arrow.y + 18.0 + dy as f32 * 2.0);
        click_at(&ctx, &mut win, pos);
        if win.view.show_new_session {
            fired = true;
            break;
        }
        // 点空地会把菜单关掉：重新点开再继续找下一行。
        if ctx.memory(|m| m.areas().visible_layer_ids().len()) == base_layers {
            click_at(&ctx, &mut win, arrow);
        }
    }
    assert!(
        fired,
        "▾ 菜单打开了，但第一行 `New Session` 点了没反应（空转开关）"
    );
}

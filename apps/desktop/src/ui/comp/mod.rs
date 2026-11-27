//! 组件注册表 —— 一个组件一张出图，一个名字一次调用。
//!
//! # 契约（自上而下强制）
//!
//! 每个组件 = 一个私有状态 struct + 一个 `show()`（纯绘制）+ 一个 `Props`
//! 结构（每帧只读输入）。三条硬规矩：
//!
//! 1. **Props 进、Action 出**：`show()` 不返回 `&mut` 任何东西，写入一律
//!    打包成 `Vec<Action>` 回给调用方，走同一条 `reduce()`。
//! 2. **不挂 `&mut TerminalWindow`**：那会把「画一个组件」变成「持有整个世界」，
//!    于是没法脱离整窗渲染——这正是组件化的出发点（方案文档 §1.2）。
//! 3. **组件私有状态不回流**：滚动位置、hover、输入缓冲可以留在组件里
//!    （沿用 `find_input` 的先例），领域状态只能由 `reduce` 写。
//!
//! # 现在的状态
//!
//! 先只登记 `dialog_new_session`：它是弹窗皮肤与 `sizing_pass` 补丁的主战场，
//! 先把「能单独出图」这条链打通。此后按同一形状迁入其余弹窗——**外框一律走
//! `harness::StableWindow`**，没有它，弹窗在截图第一帧是空的（sizing pass 的
//! 形状全是 `Noop`），冻结出来的基线就是一张「空窗口」。
//! 未迁移的组件**也**要能登记——所以 [`CompId`] 是唯一的名单，脚本按它遍历，
//! 漏登记等于漏一张基线。

/// 可独立出图的组件。名字即 `--demo comp:<name>` 的参数，也是基线文件名。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::ui) enum CompId {
    /// About 对话框：最小的一张弹窗，先拿它把出图链路跑通。
    About,
    /// Preferences（字体 / 尺寸 / 滚动缓冲）。
    Preferences,
    /// 键盘绑定表。
    Shortcuts,
    /// 独立的「快捷命令」配置弹窗（命令表来自 quickbar.json）。
    QuickCommands,
    /// 快捷命令栏左侧按钮点开的 `Config` 面板（组勾选 + 命令树预览）。
    QuickConfig,
    /// `05-context-menus`：终端 / 会话面板条目的右键菜单。
    TerminalContext,
    /// `06-button-popups/04-quick-popup.png`：另一份视图型上下文菜单。
    ViewContext,
    /// `04-color-schemes/01-scheme-list-popup.png`：shell 类型下拉（7 项）。
    ShellListPopup,
    /// `06-button-popups/06-more-enter.png`：Outline 面板 ⋮ 菜单。
    OutlineMoreMenu,
    /// `06-button-popups/11-address-bar-down.png`：地址栏下拉。
    AddressBarDropdown,
    /// Session Settings ▸ Logging 配置页（2026-11-08 视觉基线，Shell / Logging 态）。
    SessionLog,
    /// 内置 SSH 客户端的三个状态：连接中 / 要密码 / 失败。
    ///
    /// 这三个窗口平时**没法手动复现**（要真去连一台机器），而它们又最容易在
    /// 换皮肤时被改坏——出图是唯一能低成本盯住它们的办法。主机 key 按 owner
    /// 裁定（与对齐目标 / 参考版本一致）内部自动接受，没有对应的确认窗口。
    SshConnecting,
    SshPassword,
    SshFailed,
    /// 文件面板的写操作弹层（新建目录 / 改名 / 权限 / 删除）。
    ///
    /// 单独登记的理由：它只能从**文件列表的右键菜单**打开，而右键菜单进不了
    /// 截图（截图是一帧定格）。不单独出图，这个窗口就永远没人看过。
    FilerPrompt,
    /// 「运行脚本」窗口。同样只能从右键菜单打开，理由同上。
    ScriptRun,
    /// 脚本编辑器窗口。
    ///
    /// 它比运行窗口还深一层：入口挂在运行窗口的 `Edit` 按钮上，而运行窗口本身
    /// 就进不了截图。不在这里登记，这个窗口的像素直到发布都没人核对过。
    ScriptEditor,
    /// 三个键盘浮层 + 两个工具面板。
    ///
    /// 它们和对话框的区别是「定宽不定高」：内容多高窗口就多高，
    /// 所以走 `harness::AnchoredWindow` 而不是 `StableWindow`。
    Palette,
    Goto,
    Center,
    Tunnels,
    /// 2026-11-17：Tunnels **设置对话框**（与上面那块浮窗面板不是一回事）。
    TunnelSettings,
    Tmux,
    /// **浮动态**的面板。停靠态的侧栏在整窗图里已经有了，但「浮起来之后
    /// 长什么样」整窗图看不出来（那时它盖在终端上，背景全是字）。
    ///
    /// 浮动窗的矩形来自 `DockLayout`，不是这里临时摆的——出图走的就是真实路径。
    FloatingSidebar,
    /// 2026-11-07：**停靠态**侧栏（会话树）。整窗图里它被终端内容包围，
    /// 单独出图才能逐字核对标题带 / 筛选 / 分组头 / 彩色协议图标 / 选中行。
    SessionTree,
    /// New Session 的**单页**：`--demo comp:dialog_new_session_<页名>`。
    ///
    /// 为什么要拆到页：所有页共用同一套窗口外壳，外壳对了不等于每页都对。
    /// 一页一张图之后，「这一页少了哪个字段 / 顺序不对」是逐字节问题，
    /// 不再需要在整屏截图里靠肉眼找。此后视觉基线 SSH 树 13 个叶子各占一页，
    /// 加上其余协议的专属页，共 `session_nav::PAGE_COUNT` 页。
    NewSessionPage(u8),
    /// Preferences 的**单页**：`--demo comp:dialog_settings_<页名>`。
    ///
    /// 理由同 New Session：整张 `dialog_settings` 只拍得到默认页（2026-11-15
    /// 起是 Application），其余 23 页改了什么基线完全看不见。
    /// 下标是 `SettingsPage::ALL` 的**组件夹具页序**（含 5 个不在左栏显示的
    /// 管理页，见 `settings.rs::NAV_TOP` 注释），共 24 页。
    SettingsPage(u8),
    /// 窗口底部两条整宽横栏。整窗图里它们被终端内容包围，
    /// 单独出图才能逐字核对 4 组 15 项与状态栏右区 5 段。
    QuickCommandBar,
    StatusBar,
    /// 2026-11-08：关闭确认弹窗两张（关闭软件 / 关闭会话）。平时没法稳定
    /// 复现（得真的去点关闭），出图是唯一低成本盯住它们的办法。复选框
    /// 按视觉基线画成勾上。
    ConfirmCloseWindow,
    ConfirmCloseSession,
    /// 2026-11-08：Session 菜单展开态。平时截图拍不到（要点开菜单），
    /// 而标签文案 / 快捷键列 / 访问字母下划线都是这次对齐的对象——
    /// 出一张图才能逐字节盯住。
    MenuSession,
    /// Log 子菜单展开态：同时摆出启用 / 禁用 / 选中三态（2026-11-08 视觉基线）。
    MenuLog,
    /// Mode 菜单展开态（2026-11-08）。
    ///
    /// 勾选栏只在菜单真的展开时才存在，整窗图拍不到；而「框多大、离左边
    /// 多远、无框行的文字要不要跟着右移」全是像素问题。出图走的是生产同一个
    /// `draw_menu_items_static` → 同一套行绘制，所以它守的是真路径。
    MenuMode,
    /// 2026-11-08（晚）：其余 7 个顶层菜单的展开态。菜单栏共 10 个菜单，此前
    /// 只出了 Session / Log（子菜单）/ Mode 三张；Edit / Search / Selection /
    /// Goto / View / Window / Help 仅有 `menu.rs` 的逻辑对齐，缺逐字节视觉基线。
    /// 这 7 张补上后，菜单栏 10 个菜单全部有独立出图，可逐张比对视觉基线。
    MenuEdit,
    /// `01-menu-bar/tool/01-tool-menu.png`：五项 + Transfer Binary 子菜单。
    MenuTool,
    MenuSearch,
    MenuSelection,
    MenuGoto,
    MenuView,
    MenuWindow,
    MenuHelp,
    /// 参考图的子菜单展开态（`menu_submenu_*`），路径表在 `submenu_fixtures`。
    MenuSubmenu(u8),
}

// 名字表（`ALL` / `name` / `has_baseline` / `page` / `parse`）在 `ids.rs`。
// 它是一张长得像数据的表，与「注册表 + 出图入口」混在一个文件里会让代码行（不含
// 注释）越线；拆出去只挪位置，`CompId` 仍然只活在 `ui` 内部。
mod ids;
/// 触发器编辑器组件夹具的样例规则（`mod.rs` 行数上限）。
mod trig_sample;
mod quick_fixtures;
mod context_fixtures;
mod reference_popup;
mod fixture_data;
mod menu_fixtures;
mod submenu_fixtures;
/// OneKeys 逐张出图夹具（`mod.rs` 行数上限）。
mod onekeys_fixtures;
/// 脚本两窗（运行窗口 / 编辑器）的夹具。与 `reference_popup` 同一条理由：
/// 本文件贴近 500 行上限，再添一个 arm 就越线。
mod script_fixtures;

/// 页组件的最大合法下标。页表是唯一来源，不在这里再写一个 9 / 16。
fn last_page() -> usize {
    dialogs::session_nav::PAGE_COUNT - 1
}

/// 给命令行报错用的名字清单（`--demo comp:<name>` 的合法取值）。
///
/// `pub(crate)`：`apps/desktop` 的命令行入口也要用它拼报错信息，
/// 但组件本体（`CompId`）仍然只活在 `ui` 内部。
pub(crate) fn comp_names() -> Vec<&'static str> {
    CompId::ALL.iter().map(|c| c.name()).collect()
}

/// 组件名 + 是否已冻结基线。`--list-components` 用，给比对脚本取名单。
///
/// 脚本不该自己维护一份组件清单：漏一个就等于少一张基线，而且没人会发现。
pub(crate) fn comp_list() -> Vec<(&'static str, bool)> {
    CompId::ALL
        .iter()
        .map(|c| (c.name(), c.has_baseline()))
        .collect()
}

use crate::ui::dialogs::ssh_connect::{Connector, Step};
use crate::ui::harness;
use crate::ui::*;

impl TerminalWindow {
    /// 只画一个组件的出图路径。
    ///
    /// 与 `draw()` 共用同一套样式与字体（否则图上的字不是屏幕上那几个字），
    /// 但**不画任何窗口框架**：没有菜单栏、没有状态栏、没有终端网格、也没有
    /// 分屏树。画布是纯色，组件之外的一切都是噪音。
    ///
    /// 组件本体此前仍挂在 `TerminalWindow` 上（`draw_new_session_dialog`
    /// 读的是 `self.new_session`）——这一层拆开放到以后处理，本阶段先把
    /// 「能单独出图、能逐字节比对」这条链打通。
    pub(in crate::ui) fn draw_component_demo(&mut self, ctx: &egui::Context, comp: comp::CompId) {
        apply_ui_style(ctx, &self.palette);
        // 与 `ui/mod.rs::update` 同一条判据：换了定义的那一帧不量 cell 盒。
        let defs_changed = self.fonts.install(ctx);
        self.fonts.measure_once(ctx, defs_changed);
        harness::quiet_diagnostics(ctx);
        harness::freeze_clock(ctx);

        harness::show_canvas(ctx, |_ui| {});
        // 视觉基线夹具（7 弹层 + 2 菜单展开态）走独立文件，避免超限。
        if comp.is_reference_popup() || matches!(comp, comp::CompId::MenuSession | comp::CompId::MenuLog) {
            if matches!(comp, comp::CompId::MenuSession | comp::CompId::MenuLog) {
                menu_fixtures::draw(self, ctx, comp);
            } else {
                reference_popup::draw(self, ctx, comp);
            }
            return;
        }
        match comp {
            comp::CompId::About => {
                self.view.show_about = true;
                self.draw_about_dialog(ctx);
            }
            comp::CompId::Preferences => {
                self.view.show_settings = true;
                // 视觉基线 `01-application.png` 的两个确认开关都是**未勾选**。
                // 夹具把状态钉死，避免用户本地 prefs 让基线随机翻转。
                self.confirm.suppress_session = true;
                self.confirm.suppress_window = true;
                self.draw_settings_dialog(ctx);
            }
            comp::CompId::SettingsPage(n) => {
                // 只改「左栏停在哪一页」——绘制仍是 `draw_settings_dialog` 那份，
                // 与整张弹窗基线同一路径。越界（命令行写不出这种名字）落到默认页。
                self.settings_page = dialogs::SettingsPage::from_index(n as usize).unwrap_or_default();
                // 与 `Preferences` 分支同理：视觉基线两个确认框都是未勾选。
                self.confirm.suppress_session = true;
                self.confirm.suppress_window = true;
                // 2026-11-15：Triggers / OneKeys 已从 Preferences 内嵌页改成
                // **独立对话框**（兼容终端形态）。夹具跟着走真实路径，否则这张基线
                // 冻结的是一个用户点菜单永远看不到的形态。
                match self.settings_page {
                    dialogs::SettingsPage::Triggers => {
                        self.view.show_triggers = true;
                        // 组件基线只截空态是测不出布局的：11 种类型里真正有
                        // 编辑面板的页面（Action / Custom link / Send text）
                        // 一条规则都没有就永远是空白。这里塞一份样例规则，
                        // 用 `WINDTERM_TRIG_SAMPLE` 指定要展示的类型名
                        // （缺省 Highlight text），便于逐页截图核对。
                        // `WINDTERM_TRIG_EMPTY=1`：跳过样例，走真正的空触发器态，
                        // 供视觉基线 `01`/`02`（空列表）做整图差分。
                        if self.triggers.is_empty()
                            && std::env::var_os("WINDTERM_TRIG_EMPTY").is_none() {
                            self.triggers = trig_sample::sample_triggers();
                        }
                        self.draw_triggers_dialog(ctx);
                    }
                    dialogs::SettingsPage::OneKeys => {
                        if harness::fixture_mode() {
                            onekeys_fixtures::apply(self);
                        }
                        self.view.show_onekeys = true;
                        self.draw_onekeys_dialog(ctx);
                    }
                    _ => {
                        self.view.show_settings = true;
                        self.draw_settings_dialog(ctx);
                    }
                }
            }
            comp::CompId::Shortcuts => {
                self.view.show_shortcuts = true;
                self.draw_shortcuts(ctx);
            }
            comp::CompId::SessionLog => {
                // 上游没有独立 Log 面板：它就是 New Session ▸ SSH ▸ Terminal ▸
                // Logging 子页（`02-session-dialog/ssh/12-ssh-logging.png`）。
                // 这里定格 Shell 协议 + Logging 页 + 出厂日志配置。
                self.log_settings = dialogs::LogSettingsData::default(); self.view.show_log_settings = false;
                self.new_session.protocol = term_core::session::Protocol::Shell; self.new_session.selected_page = 2; self.view.show_new_session = true;
                self.session_dialog = dialogs::SessionDialogKind::Properties { index: 0 };
                self.draw_new_session_dialog(ctx);
            }
            comp::CompId::SshConnecting
            | comp::CompId::SshPassword
            | comp::CompId::SshFailed => {
                // 合成一台不会变的「假主机」：出图要的是窗口长什么样，
                // 不是某台机器的真实地址。
                let mut config = SessionConfig::new(Protocol::Ssh);
                config.name = "prod-web".to_string();
                config.host = "10.0.0.7".to_string();
                config.username = "deploy".to_string();
                self.connector = Some(match comp {
                    comp::CompId::SshConnecting => Connector::new(config, Step::Connecting),
                    comp::CompId::SshPassword => Connector::waiting_password_with(
                        config,
                        "这个会话没有保存密码（我们也不存明文）".to_string(),
                    ),
                    _ => Connector::failed(
                        config,
                        "Permission denied (publickey,password).".to_string(),
                    ),
                });
                self.draw_ssh_dialogs(ctx);
            }
            comp::CompId::FilerPrompt => {
                // 夹具要的是「这个窗口长什么样」，所以直接立一个待确认的操作，
                // 不走右键那条路（右键菜单进不了截图）。
                self.begin_filer_op(panels::filer::ops::FilerOp::NewFolder {
                    dir: "/srv/www".to_string(),
                    name: "logs".to_string(),
                });
                self.draw_filer_prompt(ctx);
            }
            // 脚本两窗（运行窗口 / 编辑器）：状态摆法在 `script_fixtures.rs`，
            // 这里只留一个转发——两个 arm 的注释与摆位代码会顶破行数上限。
            comp::CompId::ScriptRun | comp::CompId::ScriptEditor => {
                self.draw_script_fixture(ctx, comp);
            }
            comp::CompId::NewSessionPage(n) => {
                // 页夹具只改「表单停在哪一页」——`selected_page` 是受控输入缓冲
                // （沿用 `find_input` 的先例），不是领域状态，夹具写它不越界。
                //
                // 二级导航随协议变，所以还要把协议设成**真的包含这一页**
                // 的那个（例如 Ciphers 只属于 SSH、Serial Port 只属于串口）。
                // 否则单页出图里左侧树没有高亮项，看不出「这一页归哪个协议」。
                let page = (n as usize).min(last_page()); let mut protocol = dialogs::session_nav::protocol_for_page(page);
                // 04-new-session 与 default-session 的 Session 页都是 SSH；protocol_for_page(0)
                // 按首项给 Shell，夹具按视觉基线覆盖。
                if page == 0 { protocol = term_core::session::Protocol::Ssh; }
                // 夹具专用：Session 页按协议有一行不同的首字段（Tcp=Host+Data、
                // Serial=设备下拉）。出图时可用 WINDTERM_SESSION_PROTOCOL 覆盖，
                // 生产路径不读这个变量。
                if page == 0 && harness::fixture_mode() {
                    if let Ok(name) = std::env::var("WINDTERM_SESSION_PROTOCOL") {
                        protocol = match name.as_str() {
                            "shell" => Protocol::Shell,
                            "telnet" => Protocol::Telnet,
                            "tcp" => Protocol::Tcp,
                            "serial" => Protocol::Serial,
                            _ => Protocol::Ssh,
                        };
                    }
                }
                self.view.show_new_session = true; self.new_session.protocol = protocol;
                self.new_session.port = protocol.default_port(); self.new_session.selected_page = page;
                // 比对夹具：同一张表单按 New / Default / Properties 出图；只在组件夹具下读。
                if harness::fixture_mode() {
                    let mode = std::env::var("WINDTERM_SESSION_DIALOG").unwrap_or_default();
                    self.session_dialog = if mode == "default" { dialogs::SessionDialogKind::Default }
                        else if mode == "properties" { dialogs::SessionDialogKind::Properties { index: 0 } }
                        else { self.new_session.name.clear(); self.new_session.group.clear(); dialogs::SessionDialogKind::New { group: None } };
                }
                self.draw_new_session_dialog(ctx);
            }
            comp::CompId::Palette => {
                self.view.command_palette.visible = true;
                self.draw_command_palette(ctx);
            }
            comp::CompId::Goto => {
                self.view.goto.visible = true;
                self.draw_goto(ctx);
            }
            comp::CompId::Center => {
                self.view.center.visible = true;
                self.draw_center(ctx);
            }
            comp::CompId::FloatingSidebar => {
                // 浮动态：把侧栏从停靠带里摘出来。位置写死在这里是为了让这张图
                // 可复现；真实路径里它来自拖动松开的位置（`float_rect`）。
                self.dock.float(
                    term_app::dock::PanelId::Sidebar,
                    egui::Rect::from_min_size(egui::pos2(270.0, 110.0), egui::vec2(260.0, 360.0)),
                );
                self.draw_floating_panels(ctx);
            }
            comp::CompId::SessionTree => {
                crate::ui::comp::fixture_data::seed_session_tree(self);
                // 停靠态侧栏：无标题栏、无淡入的固定尺寸 Window。用 Window 而不是
                // Area，是因为 Area 在夹具时钟（t=0）下停在淡入第一帧，整块内容
                // 只有 ~30% 不透明度；`fade_in(false)` 是 harness 处理同一问题的既有做法。
                egui::Window::new("")
                    .id(egui::Id::new("component_session_tree"))
                    .title_bar(false)
                    .collapsible(false)
                    .resizable(false)
                    .fade_in(false)
                    .fixed_rect(egui::Rect::from_min_size(
                        egui::pos2(8.0, 8.0),
                        egui::vec2(268.0, 600.0),
                    ))
                    .frame(
                        egui::Frame::none()
                            .fill(crate::theme::chrome(term_core::theme::CHROME_SCALE.nav)),
                    )
                    .show(ctx, |ui| {
                        ui.set_width(268.0);
                        ui.set_height(600.0);
                        ui.add_space(21.0);
                        self.draw_panel_body(term_app::dock::PanelId::Sidebar, ui);
                        crate::ui::comp::fixture_data::draw_session_tree_title(ui);
                    });
            }
            comp::CompId::Tunnels => {
                // 金标准 `06-button-popups/09-tunnels-button-popup.png`：这不是
                // 转发管理面板，而是工具条 Tunnels 按钮的两行下拉。
                self.tunnels_enabled = true;
                crate::ui::shell::draw_tunnels_popup_demo(self, ctx);
            }
            comp::CompId::TunnelSettings => {
                // 金标准 `07-dialogs/tunnel-settings-empty.png`：左列表空、
                // 右栏是未选状态下的出厂占位（Dynamic port allocation / 0）。
                if self.tunnel_editor.is_none() {
                    self.tunnel_editor =
                        Some(crate::ui::dialogs::tunnel_settings::Editor::new(Vec::new()));
                }
                let names = vec!["demo".to_string()];
                ctx.data_mut(|d| d.insert_temp(egui::Id::new("tunnel_session_names"), names));
                let _ = crate::ui::dialogs::tunnel_settings::draw(self, ctx);
            }
            comp::CompId::Tmux => {
                // 与真实入口同源：命令 ID → `Action::parse` → reducer 翻 show_tmux。
                // 夹具每帧都跑，所以只在没开时派发一次，避免逐帧取反把面板关掉。
                if !self.view.show_tmux {
                    self.dispatch(term_app::menu::context_ids::TOGGLE_PANE_TMUX);
                }
                self.draw_tmux_panel(ctx);
            }
            comp::CompId::QuickCommandBar => {
                // 纯函数组件：没有 TerminalWindow 依赖，只要一个 ui 和一个动作袋。
                // 夹具里不会有点击，actions 恒为空，出图因此完全确定。
                let mut actions = Vec::new();
                harness::show_bar_canvas(ctx, crate::ui::shell::QUICK_BAR_H, |ui| {
                    // 右端图标的锚点是**画布**的右沿，不是窗口右沿：这个 Frame
                    // 左右各内缩 8 点，拿 `screen_rect()` 会把 `×` 甩到画布框外。
                    let right_edge = ui.max_rect().right();
                    crate::ui::shell::draw_quick_command_bar(
                        ui,
                        &[true; term_app::state::QUICK_GROUP_COUNT],
                        // 组件基线固定英文 + 出厂词表，出图完全确定。
                        &term_runtime::quickbar::QuickBarConfig::default(),
                        // 出厂显示开关（多组 + 组标题 + 不字母序），守住现有基线。
                        &term_runtime::quickbar::QuickBarDisplay::default(),
                        &mut actions,
                        // 组件基线是英文的：夹具固定英文，与整窗基线同一条规矩。
                        term_app::i18n::Lang::En,
                        right_edge,
                    );
                });
            }
            comp::CompId::StatusBar => {
                // 状态栏读的是只读快照与编译期常量；夹具时钟走固定串，
                // 所以「本地 / 远端模式、Ln Ch、os、时间、WindTerm」逐字节可复现。
                harness::show_bar_canvas(ctx, crate::ui::shell::STATUS_BAR_H, |ui| {
                    self.draw_status_bar(ui);
                });
            }
            comp::CompId::ConfirmCloseWindow | comp::CompId::ConfirmCloseSession => {
                // 复选框按参考图 `07-dialogs/confirm-*.png` 画成未勾选（默认态）；
                // pending 直接摆好，弹窗绘制与交互路径同一份代码。
                self.confirm.pending = Some(match comp {
                    comp::CompId::ConfirmCloseSession => {
                        // 夹具没有真标签，用哨兵身份：绘制只关心文案与尺寸。
                        crate::ui::dialogs::confirm::ConfirmClose::Tab(u64::MAX)
                    }
                    _ => crate::ui::dialogs::confirm::ConfirmClose::Window,
                });
                self.confirm.check = false;
                self.draw_confirm_close_dialog(ctx);
            }
            comp::CompId::MenuMode => {
                // Mode 菜单：4 个勾选行（Remote Mode / Word Wrap 打勾，
                // Virtual Space / Multiple Caret 空框）+ 其余普通行。
                //
                // Word Wrap 是 `CheckState::Live`，勾读活动标签的 `Tab.word_wrap`。
                // 参考 `mode/01-mode-menu.png` 那一格是打勾的运行态；夹具只把
                // 活动标签的种子设成 true 来复现那一帧，不改出厂默认（那会影响
                // 真实用户行为，属 C1 设置口径）。
                let seed_slot = self.active_slot();
                if let Some(tab) = self.tabs.get_mut(seed_slot) { tab.word_wrap = true; }
                //
                // 宽度按内容自适应（`menu_popup_width`：最宽行 × 等宽字宽 +
                // 勾选栏），不再写死 262——视觉基线里 `mode/01-mode-menu.png`
                // 的 262 只是那张图的最宽行，换字体 / 换文案就失效。
                let menu = term_app::menu::menus()
                    .into_iter()
                    .find(|m| m.label == "Mode")
                    .expect("Mode 菜单必须存在");
                harness::show_canvas(ctx, |_ui| {});
                egui::Area::new(egui::Id::new("comp_menu_mode"))
                    .fixed_pos(egui::pos2(4.0, 26.0))
                    .fade_in(false)
                    .order(egui::Order::Foreground)
                    .show(ctx, |ui| {
                        ui.set_width(shell::menu_popup_width(self, &menu.items, ui));
                        egui::Frame::none()
                            .fill(shell::popup_bg())
                            .inner_margin(egui::Margin::symmetric(4.0, 6.0))
                            .rounding(4.0)
                            .show(ui, |ui| {
                                let _ =
                                    shell::draw_menu_items_static(self, ui, &menu.items, &[]);
                            });
                    });
            }
            comp::CompId::MenuEdit
            | comp::CompId::MenuTool
            | comp::CompId::MenuSearch
            | comp::CompId::MenuSelection
            | comp::CompId::MenuGoto
            | comp::CompId::MenuView
            | comp::CompId::MenuWindow
            | comp::CompId::MenuHelp => {
                let label = match comp {
                    comp::CompId::MenuEdit => "Edit",
                    comp::CompId::MenuTool => "Tool",
                    comp::CompId::MenuSearch => "Search",
                    comp::CompId::MenuSelection => "Selection",
                    comp::CompId::MenuGoto => "Goto",
                    comp::CompId::MenuView => "View",
                    comp::CompId::MenuWindow => "Window",
                    comp::CompId::MenuHelp => "Help",
                    _ => unreachable!(),
                };
                self.draw_top_menu_demo(ctx, label);
            }
            comp::CompId::MenuSubmenu(i) => submenu_fixtures::draw(self, ctx, i),
            _ => unreachable!("视觉基线夹具已提前返回"),
        }
    }

    /// 顶层菜单展开态出图：取 `menus()` 里对应 label 的菜单，走与生产同一条
    /// `draw_menu_items_static`（只不派发、不弹层）。弹层宽度按内容自适应
    /// （`menu_popup_width`：最宽行字符格数 × 等宽字宽 + 勾选栏），消除写死
    /// 宽度留下的右侧大段空白。出图即真实渲染路径，守的是菜单本身长相。
    fn draw_top_menu_demo(&mut self, ctx: &egui::Context, label: &str) {
        let menu = term_app::menu::menus()
            .into_iter()
            .find(|m| m.label == label)
            .unwrap_or_else(|| panic!("菜单 {label} 必须存在于 menus()"));
        harness::show_canvas(ctx, |_ui| {});
        egui::Area::new(egui::Id::new(format!("comp_top_menu_{label}")))
            .fixed_pos(egui::pos2(4.0, 26.0))
            .fade_in(false)
            .order(egui::Order::Foreground)
            .show(ctx, |ui| {
                // 弹层宽度按内容自适应（最宽行的字符格数 × 等宽字宽 + 勾选栏），
                // 消除「右侧大段空白」：真实 ui.menu_button 由 egui 自动收缩到
                // 内容宽，这里手动算一遍等价行为，让组件图与真实弹层同宽。
                ui.set_width(shell::menu_popup_width(self, &menu.items, ui));
                egui::Frame::none()
                    .fill(shell::popup_bg())
                    .inner_margin(egui::Margin::symmetric(4.0, 6.0))
                    .rounding(4.0)
                    .show(ui, |ui| {
                        let _ = shell::draw_menu_items_static(self, ui, &menu.items, &[]);
                    });
            });
    }

}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_registered_component_round_trips_through_its_name() {
        // 名字是脚本的唯一入口：拼错一个字母就等于那张基线没人跑。
        for comp in CompId::ALL {
            assert_eq!(CompId::parse(comp.name()), Some(*comp));
        }
    }

    #[test]
    fn every_content_page_has_exactly_one_registered_component() {
        // 页表扩到 17 页后，注册名单漏一页就等于那一页没有视觉基线。
        let pages: Vec<u8> = CompId::ALL
            .iter()
            .filter_map(|c| c.page())
            .collect();
        assert_eq!(pages.len(), dialogs::session_nav::PAGE_COUNT);
        for n in 0..dialogs::session_nav::PAGE_COUNT {
            assert_eq!(
                pages.iter().filter(|p| **p == n as u8).count(),
                1,
                "页 {n} 的组件登记数不是 1"
            );
        }
    }

    #[test]
    fn every_settings_page_has_exactly_one_registered_component() {
        // 与 New Session 同一条契约：一页一张图。Preferences 21 页
        // （上游 19 + Triggers / OneKeys），漏一页就没人发现内容改了。
        let pages: Vec<u8> = CompId::ALL
            .iter()
            .filter_map(|c| match c {
                CompId::SettingsPage(n) => Some(*n),
                _ => None,
            })
            .collect();
        assert_eq!(pages.len(), dialogs::SettingsPage::ALL.len());
        assert_eq!(pages.len(), 21);
        for n in 0..21u8 {
            assert_eq!(
                pages.iter().filter(|p| **p == n).count(),
                1,
                "Preferences 第 {n} 页的组件登记数不是 1"
            );
        }
    }

    #[test]
    fn settings_page_baselines_follow_the_page_keys() {
        // 期望值 = `SettingsPage::key()` 的 slug，按组件夹具页序
        // （TOP 8 项 → Terminal / Text / Widgets 三个「组头 + 子页」；
        // 其中 2 项新增管理页不再画进左栏，但基线槽位不动）。
        // 这条同时钉住三件事：名字、页序（下标 ↔ 页）、`parse` 的反查。
        let slugs = [
            "application",
            "appearance",
            "proxy",
            "search",
            "security",
            "tabs",
            "triggers",
            "onekeys",
            "terminal",
            "auto_completion",
            "command_history",
            "mouse_tracking",
            "text",
            "binary",
            "caret",
            "font",
            "highlight",
            "wrap",
            "widgets",
            "filer",
            "quick_bar",
        ];
        for (i, slug) in slugs.iter().enumerate() {
            let want = format!("dialog_settings_{slug}");
            let comp = CompId::SettingsPage(i as u8);
            assert_eq!(comp.name(), want, "第 {i} 页的名字");
            assert_eq!(CompId::parse(&want), Some(comp), "{want} 应能反查");
        }
        // 只有前缀 / 未知页名都不算合法组件。
        assert_eq!(CompId::parse("dialog_settings_"), None);
        assert_eq!(CompId::parse("dialog_settings_not_a_page"), None);
    }

    #[test]
    fn registered_components_declare_their_baseline_state() {
        // 未冻结基线的组件不进 `--components`（否则「修好它」会被判成回归）；
        // 但至少得有一个受门禁保护——全改成 false 等于把门禁悄悄关掉。
        assert!(CompId::ALL.iter().any(|c| c.has_baseline()));
        assert!(CompId::About.has_baseline());
        assert!(CompId::NewSessionPage(0).has_baseline());
        // Preferences 的每一页都出图并参与逐字节比对（OneKeys 已按参考图
        // Sessions 图重冻，`dialog_settings_onekeys.png` 进了 git）。
        assert!(CompId::SettingsPage(0).has_baseline());
        assert!(CompId::SettingsPage(21).has_baseline());
        let onekeys = dialogs::SettingsPage::ALL
            .iter()
            .position(|p| *p == dialogs::SettingsPage::OneKeys)
            .expect("OneKeys 必须在 Preferences 页表里");
        assert!(CompId::SettingsPage(onekeys as u8).has_baseline());
        // 两条底部横栏各自受一张逐字节基线保护。
        assert!(CompId::QuickCommandBar.has_baseline());
        assert!(CompId::StatusBar.has_baseline());
    }

    #[test]
    fn unknown_names_are_rejected() {
        // 只认清单里的名字——`comp:` 后面写什么都当合法，是「静默出了张空图」的源头。
        assert_eq!(
            CompId::parse("dialog_new_session_session"),
            Some(CompId::NewSessionPage(0))
        );
        assert_eq!(
            CompId::parse("dialog_new_session_proxy"),
            Some(CompId::NewSessionPage(5))
        );
        assert_eq!(CompId::parse("dialog_about"), Some(CompId::About));
        assert_eq!(CompId::parse("dialog"), None);
        // 旧名（整张弹窗、以及先前的自造页名）都不再是合法组件。
        assert_eq!(CompId::parse("dialog_new_session"), None);
        assert_eq!(CompId::parse("dialog_new_session_general"), None);
        assert_eq!(CompId::parse(""), None);
        assert_eq!(CompId::parse("Dialog_New_Session"), None);
    }

    #[test]
    fn names_are_unique_and_file_safe() {
        // 名字会直接变成 `tests/baseline/components/<name>.png`：只许小写与下划线。
        let mut seen = Vec::new();
        for comp in CompId::ALL {
            let name = comp.name();
            assert!(!seen.contains(&name), "组件名重复：{name}");
            seen.push(name);
            assert!(
                name.chars()
                    .all(|c| c.is_ascii_lowercase() || c == '_' || c.is_ascii_digit()),
                "组件名只该有小写字母 / 数字 / 下划线：{name}"
            );
        }
    }
}


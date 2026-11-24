//! WindTerm UI — 投影层。按组件分组的模块树，本文件只做装配。
//!
//! S2 之后每个 tab 只持有 `SessionId` 与一份只读 `GridSnapshot`；会话句柄在控制面上
//! （ADR-0019 档 1.1：`Arc<SessionPool>` 才是所有者，tab 不握线程 / 队列 / 邮箱）。
//! PTY、子进程、VT 网格、字节摄入全部在监管者线程里；
//! 本文件只做三件事：收快照 / 画 / 发 Action（B2、B3 边界）。
//!
//! # 模块树（按组件分类）
//! ```text
//! ui/  mod.rs 装配 / tab 一个 tab 的全部状态 / construct 构造 / effects 副作用
//!      harness+comp 组件夹具
//!      shell/ 固定框架（不参与浮动）  terminal/ 终端窗格
//!      panels/ 可停靠面板            dialogs/ 模态对话框
//!      overlays/ 临时覆盖层          layout/ 分屏与停靠
//! ```
//! **划分原则**：固定框架 / 可停靠 / 临时覆盖 / 模态，四类生命周期不同，
//! 混在一起就是 此前那个 1,586 行的 `mod.rs`。

mod clipboard_policy; mod comp;
mod construct;
mod dialogs; mod dispatch; mod lifecycle_events;
mod effects; mod harness;
pub(crate) mod kit;
pub(crate) use kit::type_scale;
mod layout;
mod lock_timeout;
mod modem_drop;
mod reconnect;
mod overlays;
mod panels;
mod path_picker;
mod shell;
mod tab;
mod tab_hover; mod tab_thumbnail;
mod terminal;
mod tr;
// ADR-0021 R2：纯计算 / 几何规格层（零 egui）。Trigger 对话框是第一个真实迁入的面板。
mod view;
// 窗口透明度的运行期副本与渲染落地（egui 0.29 没有数值 opacity 命令，
// 数值效果由 `egui::Visuals` 背景 alpha + 终端区底色合成）。
mod window_opacity;
pub(in crate::ui) use window_opacity::WindowOpacity;
// 地址栏浮层（连接目标输入框）。它既不是「固定框架」也不是「临时覆盖层
// 的那四个」——它锚在工具栏面板段上、有自己的输入焦点与候选表，所以自成一组。
mod address;
mod app_state;
// L1 / L3（O 设计稿）：界面内部错误与「恢复自最近留存点」的同一行提示条。
// 自成一组：它不是固定框架的一部分（不占布局），也不是那四个临时覆盖层之一
// （它没有开关，只在真有话说时存在）。
mod notice;

// 标签生命周期回归单测的共享夹具（只在测试构建里编译）。三处测试文件
// （shell/tab_strip、panels/sidebar_view、layout/restore）都要同一套
// 「完整 TerminalWindow + 真本地 shell + 可观察假通道」，放一处避免复制漂移。
#[cfg(test)]
pub(in crate::ui) mod lifecycle_test_support;

/// `Effect` 落地里**带命令 ID 入口**的那几条判据（`dispatch("…")`）。
///
/// 单独成文件是被 4e 逼的： 判据扫 `ui/` 下一处 ID 字面量都不许有，只放过由
/// `#[cfg(test)] #[path = …] mod …;` 两行声明拉进来的纯测试模块——`effects.rs`
/// 里内联的 `mod tests` 不在白名单里（判据回到声明处，不认文件名后缀，否则随便
/// 起个 `_tests.rs` 就能藏命令 ID）。而这条判据又**必须**走字符串入口才成立。
#[cfg(test)]
#[path = "effects_menu_tests.rs"]
mod effects_menu_tests;

///  窗口透明度里**带命令 ID 入口**的端到端判据（`dispatch("…")`）。
///
/// 与 `effects_menu_tests` 同一个理由：4e 扫 `ui/` 下一处 ID 字面量都不许有，
/// 只放过由 `#[cfg(test)] #[path = …] mod …;` 两行声明拉进来的纯测试模块。
/// 这里的判据又必须走字符串入口，才能证明「菜单点下去 → Prefs 真的落盘」。
#[cfg(test)]
#[path = "window_opacity_tests.rs"]
mod window_opacity_tests;

/// Q2 刀1 验收③：应用状态采集层（`app_state.rs`）的判据——`panels` 恒十条、
/// `zone/order/floating` 与 `DockLayout::placement` 逐条一致、可见性来自
/// `ViewState`。同样走两行声明拉进来的纯测试模块（与上两条同一套组织）。
#[cfg(test)]
#[path = "app_state_tests.rs"]
mod app_state_tests;

use std::collections::HashMap; use std::sync::Arc;

use dialogs::SettingsData; use dialogs::{unique_name, Connector, NewSessionData, SessionDialogKind};
use layout::{DockLayout, Layout, PaneGroup, PaneId}; use shell::{ToolAction, ToolButton};

use terminal::make_tab;

/// `--demo comp:<name>` 的合法取值，供命令行报错用（名单只有 `comp` 一份）。
pub(crate) use comp::comp_names; pub(crate) use comp::comp_list;

use crate::fonts::TerminalFonts; use crate::theme::{apply_ui_style, to_color32};
pub(in crate::ui) use tr::t; use crate::ui::shell::draw_find_bar; use eframe::egui; use term_app::state::ViewState; use egui::emath::Align2; use egui::{DragValue, Rect, Sense, TextEdit, Vec2};
use term_app::action::{reduce, Action}; use term_app::capability;
use term_app::dock::{DockZone, PanelId}; use term_app::find::{self, current_index}; use term_app::log::LogAction;
use term_app::menu::{menus, BarGlyph, DockItem, Item, BAR_TOOLS, DOCK_ITEMS}; use term_app::selection::{line_span_at, word_at_with, Cmd, Op, Pos, Selection, Selections}; use term_app::viewport::Viewport; use term_core::grid::{Rgb, GridSnapshot, ScreenImage};
use term_core::session::{AuthMethod, Protocol, SessionConfig}; use term_core::session_runtime::SessionId;
use term_core::theme::Palette; use term_core::vt::{arrow_bytes_for, paste_bytes_for, Arrow};

/// Cell padding around the grid, in points.
const GRID_PAD: f32 = 4.0;

/// The main terminal window.
///
/// S3：原来 50 个字段里的 14 个纯死状态位（只写不读、照抄菜单配置）已删除，
/// 其余视图开关集中到 `ViewState`，由 `reduce ` 这个纯函数唯一写入。
pub struct TerminalWindow {
    /// One entry per open tab.
    tabs: Vec<Tab>,
    /// R14-d：分屏布局树。单叶子时走原来的渲染路径（视觉基线不变）。
    layout: Layout,
    /// ④-0：每片窗格（一个 `PaneId`）对应的会话集合与当前显示的那一条。
    ///
    /// `Layout` 树只管「窗格怎么切分」，会话归属在这里——`draw_node` 渲染前经
    /// `pane_groups[pane].active` 解析出 `SessionId` 再交给终端绘制，
    /// 窗格里切标签只改 `active`，不动布局树（详见 `docs/adr-0026-per-pane-tab-groups.md`）。
    /// 与 `groups: Vec<String>`（会话分组名）无关：那个字段早已被占用，故另起一名。
    pane_groups: HashMap<PaneId, PaneGroup>,
    /// The terminal's palette.
    palette: Palette,
    /// Font faces and the measured cell box.
    fonts: TerminalFonts,
    /// B3：所有视图开关集中在这里，只能经 `reduce ` 修改。
    /// 状态栏 SYNC 徽标点开的通道分配面板。投影态，不进 ViewState。
    view: ViewState, show_sync_channels: bool,
    /// Session groups (folders) in the tree, in creation order.
    groups: Vec<String>,
    /// List of session profiles (the tree's entries).
    sessions: Vec<SessionConfig>,
    /// Currently selected session index in `sessions`.
    selected_session: usize,
    /// Filter text for the session tree.
    session_filter: String,
    /// Explorer / Outline 的 Filter 缓冲（跨帧控件态，与 `find_input` 同类）。
    panel_filters: layout::PanelFilters,
    /// 底带「Shell」面板里的会话：若干条**特殊的标签页**，默认打开系统本地 shell。
    ///
    /// 渲染与交互与主区标签页走**完全同一份**代码（`terminal/pane.rs::draw_tab`，
    /// 槽位是 `TabSlot::Shell(index)`），只是它们**不在** `tabs` 里——`active_tab`
    /// 是下标型（分屏树、标签条、十几处赋值点都按下标说话），把一个不是主区标签页
    /// 的东西混进同一个 `Vec`，症状是「点 A 标签跳到 B」。面板第一次要画时惰性开
    /// 一条（`terminal/tab_slot.rs::ensure_shell_tabs`），`+` 再加。
    shell_tabs: Vec<Tab>,
    /// 面板内当前激活那条的**恒在身份**（`None` = 尚未选 / 空面板）。
    ///
    /// ADR-0025 阶段 3：与主区 `active` 一样改用身份而非下标——关掉活动标签**之前**
    /// 的其它条目不再让活动位跟着挪（身份稳定）；面板的选中态也不进主区世界。
    /// 取「第几条」统一走 `shell_active_index `（身份反查 + 退化 0）。
    shell_active: Option<SessionId>,
    /// 面板标签列表（`≡` 下拉）里的过滤词。
    ///
    /// 控件状态，与 `find_input` / `panel_filters` 同一类：不进 `ViewState`。
    shell_filter: String,
    /// `≡` 下拉开着没有。与 `dock_drag` 同一层——**投影态**（处理矩形与指针），
    /// 不进 `ViewState`：那里只装领域语义，装不下「此刻有没有一个浮层展开」。
    shell_list_open: bool,
    /// Sessions closed this run, for "Reopen Closed".
    closed_sessions: Vec<String>,
    /// 就地改名那一套（`renaming` 缓冲）已删除——改名现在在
    /// `Properties` 表单里（见 `dialogs`）。两条改名路并存只会让人猜用哪条。
    /// Tool buttons.
    tools: Vec<ToolButton>,
    /// New session dialog form data.
    new_session: NewSessionData,
    /// 对话框正在服务哪个调用方（`New` / `Edit { index }` / `Default`）。
    /// 取代原先的 `editing_session: Option<(usize, String)>` —— 标题 / 按钮 /
    /// 保存语义全由 `session_dialog` 这一个枚举分派，不再散落 `if editing`。
    session_dialog: dialogs::SessionDialogKind,
    /// 默认会话模板（新增会话的默认配置）。`Default` 模式读写它；目前为内存态，
    /// 落盘是后续步骤（需要接入 prefs 持久化，且会让「新建会话」的初始表单随之变化，
    /// 需重冻会话弹窗组件基线，故单列）。
    default_session: SessionConfig,
    /// Settings dialog form data.
    settings: SettingsData,
    /// 快捷命令表（`quickbar.config` 的内存副本）。构造时读一次，设置页改完由
    /// 那一页写回磁盘；绘制层每帧只读这份副本，不碰盘。与 `prefs` 的
    /// 渲染期副本同一条规矩：拖动 / 输入过程中的值和盘上的值不能各说一套。
    quick_bar: term_runtime::quickbar::QuickBarConfig,
    /// 快捷命令栏三个显示开关的渲染期副本（`show_multiple_groups` /
    /// `show_group_title` / `alphabetical`），来自 `prefs.json`（兼容终端
    /// `user.config` 的位置）。它们不随命令表落 `quickbar.config`，所以单独存
    /// 一份窗口副本，构造时从 prefs 读一次、命令配置弹窗 Ok 时同步。底栏每帧
    /// 读这份副本，不碰盘。
    quick_bar_display: term_runtime::quickbar::QuickBarDisplay,
    /// `Text ▸ Caret` 选的光标样式（`CARET_STYLES` 下标，0 = Block）。
    ///
    /// 存活在窗口上而不是每帧读 prefs：它是「新会话的初值」，`ensure_tab` /
    /// 分屏新开本地 shell 时直接取，不为了一个下标去读磁盘。落盘仍走
    /// `prefs.json`（`Prefs::caret_style`），这里是内存里的同一份值。
    /// **已存在的会话不受影响**：它们各自带着 `SessionConfig.caret_style`。
    caret_style: usize,
    /// `Text ▸ Caret ▸ Period`（`prefs.caret_period`，秒）。
    ///
    /// 与 `caret_style` 同一写法：构造时读一次 prefs，设置页改完写回并同步这份
    /// 副本；新建会话时拷进 `SessionConfig.caret_period`。已存在的会话不受影响。
    caret_period: f32,
    /// `Text ▸ Caret ▸ Width`（`prefs.caret_width`，像素）。
    ///
    /// `Underline` / `Beam` 用；与 `caret_period` 同一条生命周期。
    caret_width: f32,
    /// `Text ▸ Alternate line background color`（`prefs.text_alternate_row_bg`）。
    ///
    /// 与 `caret_style` 同一写法：构造时读一次 prefs，设置页改完写回 prefs 并同步
    /// 这份副本。不能每帧读 `prefs.json`——`paint_rows` 每一行都要问它一次。
    alt_row_bg: bool,
    /// `Text ▸ Hide cursor when typing`（`prefs.text_hide_cursor_while_typing`）。
    /// 判据本身（最近一次输入的时刻）记在 `Tab` 上，这里只是「这个开关开没开」。
    hide_cursor_typing: bool,
    /// `Preferences ▸ Highlight ▸ Highlight the incremental search`
    /// （`prefs.text_highlight_search`，出厂 `true`）。
    ///
    /// 读者是 `terminal/pane_overlays.rs` 的查找命中那一段：关掉时一个字节都不画。
    /// 与 `alt_row_bg` / `hide_cursor_typing` 同一写法（构造时读一次 prefs，
    /// 设置页改完写回 prefs 并同步这份副本）——不能每帧读盘，这段在每次
    /// 重绘都要问一次。**窗口级而不是 `Tab` 级**：它不管标签是哪条，
    /// 与 `Tab.marks.caret_line`（每条标签各一份）是两回事。
    highlight_search: bool,
    /// `Wrap ▸ Show wrap symbols` / `Show column markers` 的运行期副本
    /// （`prefs.view_wrap_symbols` / `view_column_marker`）。构造时读一次，
    /// Wrap 页改完写回 prefs 并同步这份副本；读者是 `Tab.marks` 的两段绘制。
    /// 与 `caret_style` 同一写法：不每帧读盘，否则拖动列号时会跳。
    wrap_symbols: bool,
    /// 0 = 不画参考列；>0 = 参考列号。
    column_marker_col: usize,
    /// I-AC：补全偏好的运行期副本。设置页改完立刻写回这里供键盘漏斗每帧
    /// 读取，落盘仍走 `settings_live::save_prefs` 那一个入口。
    /// 与 `caret_style` / `wrap_symbols` 同一写法：不每帧读盘。
    completion: term_core::completion::Settings,
    /// I-AC：当前候选弹层的状态（含 owner 标签 id）。切换标签自动失效。
    completion_state: terminal::completion_popup::CompletionState,
    /// hex 的**默认参数**（设置 ▸ Binary 页那四个数 + 一格斑马线，
    /// `Prefs.hex_*` 与 `Prefs.alternate_column_bg` 的渲染期副本）。
    ///
    /// 两层模型里的「默认参数」层：建标签时用它做初值（`terminal/setup.rs`），
    /// 设置页读写它并落盘。**`View ▸ Hex *` 不碰它**——那条路改的是
    /// `Tab.hex`（当前会话）且不落盘。
    ///
    /// 斑马线在这份参数里，不在窗口上单独留一个 bool：同一页那五个数是同一类
    /// 东西，语义必须一致（只影响新标签）。单独留一份渲染期副本的下场是
    /// 「勾一下当场改掉所有已开标签」，而同页四个数字框不会。
    ///
    /// 为什么在窗口上留一份而不是每帧读盘：设置页的 `DragValue` 每帧都要一个
    /// 数字，读 `prefs.json` 会多出一份可能落后的真相（拖动时盘上还是旧值，
    /// 喂回去就跳回原位）。同 `settings_live_font` 的字号。
    hex_defaults: terminal::HexParams,
    /// `prefs.focus_new_tab`（`prefs.focus_new_tab`，出厂 `true`）。
    ///
    /// 读者是新建标签的三个落地点（分屏 / 会话树连接 / 复制标签）：关掉时新标签
    /// 照常建出来，但不动 `active_tab`。
    focus_new_tab: bool,
    /// `Tabs ▸ Activate the tab after closing:`（`prefs.tab_activate_after_close`）。
    /// 0 = 下一个（接线前的行为）/ 1 = 上一个 / 2 = 上一次激活的那条。
    /// 读者是 `shell::tab_strip::close_tab`——关标签只有那一条路径。
    tab_activate_after_close: usize,
    /// `Tabs ▸ Double click:`（0 = 没有动作 / 1 = 关闭标签）。读者是标签条。
    tab_double_click: usize,
    /// `Tabs ▸ Tab text elide mode:`（0 = 不省略 / 1 左 / 2 右 / 3 中）。
    /// 只影响画出来的标题文本，宽度仍由同一个公式从（截断后的）文本量出来。
    tab_elide: usize,
    /// `Tabs ▸ Show the tab number on tabs:`（0 = 不显示 / 1 = 前九个 / 2 = 全部）；
    /// `Middle click:`（0 = 没有动作 / 1 = 关闭标签）；
    /// `Right click:`（0 = 标签菜单 / 1 = 关闭标签 / 2 = 没有动作）。
    /// prefs.show_address_bar（`prefs.show_address_bar`，出厂 `true`）：
    /// 读者是 `shell/tool_bar.rs`；构造时读一次，设置页负责同步。
    tab_numbers: usize, tab_middle_click: usize, tab_right_click: usize, show_address_bar: bool,
    /// prefs.auto_hide_menu_bar（`prefs.auto_hide_menu_bar`）。
    /// 展开状态是投影态：指针靠近顶端 / 按住 Alt / 有菜单弹层时展开，
    /// 否则收成细带。不落盘。
    /// `ui.input.time` 口径的「保持展开到」时刻，避免指针在细带附近抖动。
    auto_hide_menu_bar: bool, menu_bar_peek_until: f64,
    /// `Preferences ▸ Security ▸ Lock screen timeout`（秒）与上一次真实输入时刻。
    lock_timeout_secs: u32, last_lock_activity: f64,
    /// prefs.auto_reconnect（出厂关）。
    auto_reconnect: bool, prefer_theme_fonts: bool,
    /// 悬停激活：当前悬停的会话与起算时刻。
    tab_hover_activate: bool, tab_hover_delay_ms: u32, tab_hover: Option<(term_core::session_runtime::SessionId, f64)>,
    /// Preferences ▸ Proxy：全局默认代理的运行期副本。
    global_proxy_type: usize, global_proxy_host: String, global_proxy_port: u16,
    tab_thumbnail_pct: usize,
    /// 上一帧文件面板的矩形：拖放时判断落在 Modem 还是 SFTP 上传。
    filer_rect: Option<egui::Rect>, path_picker: Option<path_picker::PathPicker>,
    /// `OSC 52` 请求队列与正在等用户确认的那一条。
    clipboard_requests: clipboard_policy::ClipboardInbox, clipboard_ask: clipboard_policy::ClipboardAsk,
    /// 触发器规则的运行期副本（来自 `Prefs.triggers`）。每帧 `poll` 时传给各
    /// tab 的求值器；设置页改完写回 `prefs` 并 `reload_triggers` 同步到这里。
    /// 与 `caret_style` / `alt_row_bg` 同一写法：构造时读一次，设置页负责同步。
    triggers: Vec<term_app::trigger::Rule>,
    /// OneKey 登录方案的运行期副本（来自 `Prefs.onekeys`）。OneKeys 编辑页
    /// 读写它，保存时写回 `prefs`。
    ///
    /// **连接时读的不是它**，是控制面里那一份（`SessionPool::onekey_for` /
    /// `resolve_onekey`）——用户要的是「同一个方案，终端与 SFTP、界面与脚本
    /// 看到的是同一份」，那只能由控制面持有。这份是**编辑用的正本**：
    /// 构造时推一份过去，每次落盘（`save_onekeys`）再推一次。所以任何改动
    /// 方案的路径都必须以 `save_onekeys ` 结尾，否则已开的窗口还会按旧方案连。
    onekeys: Vec<term_core::onekey::OneKey>,
    /// Tunnels 设置对话框的编辑草稿（`Some` = 对话框开着）。
    ///
    /// 与 `show_triggers` 那类「ViewState 里的 bool」不同：这里还带着一份
    /// 未提交的草稿，放 `ViewState` 会把领域数据塞进纯视图状态，所以用
    /// `Option<Editor>`——`None` 就是关着。
    tunnel_editor: Option<dialogs::tunnel_settings::Editor>,
    /// Tunnels 设置对话框的命名隧道（运行期副本，来自 `Prefs.tunnels`）。
    /// 对话框编辑它、`Ok` 时写回 `prefs`；`set_tunnels_enabled` 按它起连接。
    tunnel_profiles: Vec<term_core::tunnel::TunnelProfile>,
    /// 隧道总线开关（菜单栏 `Tunnels` 下拉里那个蓝色开关）。
    /// 关掉 = 全停，配置不丢。它**落盘**（`Prefs.tunnels_enabled`）：
    /// 关机前关掉的人，下次开机不该被自动起的隧道吓一跳。
    tunnels_enabled: bool,
    /// 标签激活历史（最近激活的在末尾），存的是**标签 id 而不是下标**：
    /// 关一条 / 复制一条都会让下标整体错位，id 不会。`Tabs ▸ 上一次激活`
    /// 那一档靠它；每帧由 `poll_tab_focus` 采集，不在 18 处赋值点各加一行。
    tab_focus: Vec<egui::Id>,
    /// Preferences 对话框当前选中的分类（投影态：几何/选中，不是领域语义）。
    settings_page: dialogs::SettingsPage,
    /// OneKeys 页的局部编辑态（选中下标 + 分类）。页面级控件态，不进 ViewState。
    onekeys_edit: Option<crate::ui::dialogs::settings_live_onekeys::OneKeysEdit>,
    /// OneKeys 页里用户刚敲的密码草稿 `(方案名, 明文)`。只在按 Save 并解锁
    /// 时写入 vault 之后清空——绝不进 prefs.json（凭据纪律）。
    ///
    /// ：键从**下标**改成**方案名**。下标在删/复制方案之后会整体错位，
    /// 于是"密码存到隔壁方案上"——静默且很难发现。
    onekeys_password_draft: Option<(String, String)>,
    /// OneKey 键盘交互应答草稿（方案名 → 按提示顺序的答案）。
    /// Account 页 Password 组的应答草稿（方案名 → 密码），保存时写 vault。
    onekeys_ki_draft: Option<(String, Vec<String>)>, onekeys_autofill_draft: Option<(String, String)>, tab_list_filter: String,
    /// L1/L3（O 设计稿）：一行提示条——界面内部错误 / 恢复自最近留存点。
    /// `None` = 没有话说。它是**浮层**，不进任何面板的布局计算。
    notice: Option<notice::Notice>,
    /// 控制面 Before* 事件队列；每帧由触发器求值层取走。
    lifecycle_events: Arc<std::sync::Mutex<Vec<term_core::trigger::TriggerOn>>>,
    /// L1：连续「帧内 panic」次数。成功画完一帧就清零（见 `note_frame_ok`）。
    frame_panics: u32,
    /// 逐行 span 缓存，未变化的行零构建成本。
    term_view: terminal::LineCache,
    /// 底带 Shell 面板自己的逐行 span 缓存（主区那份是 `term_view`）。
    ///
    /// `LineCache` 的键是「行号 + 内容 hash」（`terminal/view.rs`）。两块区域共用
    /// 一份时，交替写同一下标会**每帧把对方判成失效**：画得没错，但缓存等于没养
    /// （每行每帧重新 `build_spans`）。所以按槽位分两份。
    term_view_shell: terminal::LineCache,
    /// F1：复制用的是选区文本，按帧从事实窗口推导，Effect 只负责「写剪贴板」。
    ///
    /// 这里存的是**主区**那份。读它的是 `effects`（复制 / 搜索 / 发送选区）、
    /// 工作台与右键菜单——那些动作都作用在活动标签上，所以读主区那份是对的。
    selection_text: String,
    /// 底带 Shell 面板的剪贴板派生值（主区那份是 `selection_text`）。
    ///
    /// 行为一致优先：面板里也必须能 `Cmd+C`（用户裁定面板与标签页同一套交互），
    /// 而窗口级只有一份字段时两块区域会互相覆盖「现在复制什么」。所以分两份，
    /// `Cmd+C` 按正在画的那块区域（`TabSlot`）决定读哪一份。
    selection_text_shell: String,
    /// W5-I：窗口级命令行历史。各 tab 的回车行（`InputLine::feed` 吐出）汇总到这里，
    /// 容量 / TTL 取自 `prefs`（`command_history_store`），纯内存。
    command_history: term_core::command_history::CommandHistory,
    /// F1：egui 上下文（写剪贴板需要在非 `draw` 参数里拿到 ctx）。
    ctx: egui::Context,
    /// N：上一次周期留存的时刻（egui `i.time` 秒）。用于 30s 防抖。
    /// `0` = 本次运行还没留存过。夹具模式 `i.time` 被钉在 0，永不触发。
    last_periodic_save: f64,
    /// 结构变化（分屏 / 关窗格 / 合并 / 内置 SSH 分屏落地 / sash 松手）需要在
    /// 本帧末即时落盘，复活 `Action::SaveLayout` 死路径——不再等 30s 周期或退出兜底。
    /// 各变更点在 `draw_node` / 命令处理函数里只置这个标志，`draw ` 末尾统一
    /// `dispatch_action(Action::SaveLayout)` 一次，避免渲染期 / 嵌套 dispatch 重入。
    pending_save_layout: bool,
    /// A3：当前正被从标签栏拖拽的会话身份（`None` = 没在拖）。`draw_node` 的每片窗格
    /// 读它判断「拖到我没」；松手落在某片窗格上（`drop_drag_on_pane`）即交换两格内容并置 `None`。
    /// 拖拽全程由标签的 `drag_started/dragged` 每帧续上，松手那帧由 `drag_released_this_frame` 兜清空。
    drag_session: Option<SessionId>,
    /// A3：本帧松手结束拖拽但没落在任何窗格上（落空 / 拖回自己那片）。`draw ` 末尾据此清掉 `drag_session`，
    /// 避免拖拽态卡死在下帧。命中落点时 `drop_drag_on_pane` 已把它置回 `false`，这里便不再清。
    drag_released_this_frame: bool,
    /// F1-b：菜单「粘贴」把焦点从终端带走了，本帧末要还回去。
    restore_focus: bool,
    /// 底带 Shell 面板版的 `restore_focus`（两份各记一份）。
    ///
    /// 混成一份的下场：在面板里中键粘贴一次，主区那一帧就把旗子取走并抢走焦点，
    /// 下一次敲键盘进的是主区而不是面板——而粘贴的文字属于面板。
    restore_focus_shell: bool,
    /// R4：Find 输入框的编辑缓冲（控件状态，不是领域状态）。
    ///
    /// 为什么不能每帧用临时 `String` 喂 `TextEdit`：输入法的预编辑文本是写进
    /// 缓冲的，每帧重建会逐帧重复拼接。所以缓冲必须跨帧存活——但它只属于投影，
    /// `ViewState` 仍然只由 `reduce` 写。
    find_input: String,
    /// R14-c：命令面板输入框的编辑缓冲（与 `find_input` 同一类）。
    palette_input: String,
    /// R14-d：Goto 行号输入框的编辑缓冲（与 `find_input` 同一类）。
    goto_input: String,
    /// R14-f：工作台输入框的编辑缓冲（与 `find_input` 同一类）。
    center_input: String,
    /// V1-3：锁屏主密码的跨帧缓冲（受控输入，不进 ViewState）。
    lock_password: String,
    /// V1-2：快捷键映射层（出厂表 + 用户覆盖 + 平台展示）与重绑捕获态。
    /// 捕获态记的是**命令 ID**，不是表下标——表增删项不会把用户绑定挪到别的命令。
    shortcuts: term_app::shortcut_map::KeyBindingMap,
    shortcut_capture: Option<String>,
    /// 两段式键（`Alt+W, Alt+H`）的等待态与解析好的运行期表。
    ///
    /// 都活在窗口上而不是 `ViewState`：前缀是**跨帧的控件状态**，不是业务事实。
    /// 放进 `ViewState` 会逼出一个假读者（`verify.sh` 4h 只认真读者），
    /// 而且 reducer 测试里整份 `ViewState` 会被 `assert_eq!(state, before)`，
    /// 塞一个时间戳进去等于承认 reducer 碰墙钟。
    ///
    /// `seq_table` 由映射层（`shortcut_map::KeyBindingMap`）派生并按「已接线」
    /// 过滤（见 `dialogs::shortcuts::sequence_table`）：没接线的键序不进
    /// 运行期表，否则它的第一段会白吞按键。
    keyseq: term_app::shortcuts::KeySeqState,
    seq_table: Vec<(Vec<term_app::shortcuts::Chord>, &'static str)>,
    /// V1-2：标签 12 色（下标 → 调色板索引）。
    tab_colors: Vec<Option<usize>>,
    /// R16：隧道面板里「新增规则」的输入缓冲与解析错误（与 `find_input` 同类）。
    tunnel_input: String,
    tunnel_error: Option<String>,
    /// V1-4：tmux 面板日志（投影层 IO 缓冲）。
    ///
    /// B2：控制模式状态机已下移到每个 `Tab.tmux`——它只能喂本会话的字节流，
    /// 窗口级单例会让「A 标签在跑 tmux，B 标签却判定 active」串线。
    /// 这里只留面板自己的日志 / 输入 / 命令块索引。
    tmux_log: Vec<String>,
    tmux_input: String, tmux_send: String, tmux_pane_name: String,
    tmux_blocks: Vec<term_app::blocks::Block>,
    /// R11：菜单触发的选区操作，等本帧算出事实窗口后再落地。
    ///
    /// 为什么不立刻执行：落点要用「此刻画在屏上的那一屏」，而 `dispatch` 里
    /// 手上只有上一帧的窗口。攒一帧不会让人察觉到延迟（点击本身就发生在本帧绘制前）。
    ///
    /// 槽位里放的是 `Cmd`——位移（`Move`）与集合命令（`Set`）共用它，
    /// 所以「锁 / 拆分 / 反选」与「选到下一行」走的是同一条落地路径，
    /// 不会因为多开一个字段而出现两条先后顺序不明的待办。
    pending_select: Option<Cmd>,
    /// 进行中的原生 SSH 连接（`None` = 没有）。
    /// 它不在 `ViewState` 里：`ViewState` 是**领域状态**（只能由 `reduce` 写），
    /// 而这个东西是「一次副作用正在发生」，带有 `Receiver`、线程与凭据——
    /// 放进 reducer 会让它变成不可单测的 IO 状态机。它是投影层的 IO 缓冲，
    /// 与 `find_input` 同一类。
    connector: Option<Connector>,
    /// 密码输入框的跨帧缓冲（同 `find_input`：受控输入，不回流成领域状态）。
    /// 存在的理由很实际：每帧新建 `String` 会让输入法的预编辑文本逐帧重复拼接。
    ssh_password: String,
    /// 密码对话框里的「存入保险库」勾选（一次性意图，提交后清掉）。
    save_password: bool,
    /// 在跑的隧道。每条会话最多一条，句柄与隧道本体都在 `term-tunnel` 里。
    /// 与 `connector` 同一类：这是**副作用的持有者**，不是领域状态，所以不进
    /// `ViewState`（那会让 reducer 持有一个线程句柄）。
    tunnels: Vec<panels::RunningTunnel>,
    /// 会话控制面（ADR-0019 档 1.1）：本窗口所有会话都登记在这里。不做全局单例，
    /// 因为 `desk serve` 与 GUI 是两个进程、两个池（ADR-0022 §决策 7）。
    pool: Arc<term_session::SessionPool>,
    /// 已解锁的保险库（`None` = 还没解锁 / 还没建）。
    /// ADR-0017 §未决问题 5：解锁一次、本次运行可用，所以实例放在 `apps/*` 里。
    /// 它只持有密钥与条目（纯内存），文件读写走 `term-runtime::persist`。
    vault: Option<term_vault::Vault>,
    /// Authentication 页里的解锁区（主密码输入与错误提示）。
    vault_form: dialogs::vault::VaultForm,
    /// R14-a：日志动作的一次性提示（与状态栏 `log_label ` 分工），失败也显示在界面上。
    log_note: Option<String>,
    /// Session Settings ▸ Logging 配置页的跨帧控件态（数据在 dialogs::log_settings_spec，只画在 log_settings）。
    log_settings: dialogs::LogSettingsData,
    /// R13-f：右侧文件面板（远端目录浏览）。
    /// 与 `tunnels` 同一类：持有线程与队列（`SftpHandle`），不是领域状态；
    /// 领域状态只有 `view.show_filer` 那个 bool。
    filer: panels::Filer,
    /// 「运行脚本」窗口（`None` = 关着）。与 `connector` 同类：带线程句柄，
    /// 所以不进 `ViewState`。
    script: Option<terminal::ScriptDialog>,
    script_manager: Option<crate::ui::dialogs::script_manager::ScriptManagerEdit>,
    /// 「脚本」停靠面板（齿轮菜单勾选打开的那块）。与 `script` 共用执行线程
    /// （`script_run::ScriptRun`），但它是**常驻**的：选脚本、点运行、看日志都在
    /// 面板里，不另开窗口。`ViewState` 只管「显不显示」（`show_pane_script`）。
    script_panel: panels::ScriptPanel,
    /// ADR-0025 阶段 5：脚本新开的会话从**这里**回到窗口。
    ///
    /// 脚本线程没法直接往 `tabs` 里塞东西（那是 UI 线程的 `Vec<Tab>`），所以它
    /// 只把「我开了哪条会话、用什么配置开的」推进这个共享队列，窗口每帧 drain
    /// 一次、逐条变成真标签页。队列挂在窗口而不是两个对话框上：运行窗口关掉
    /// 之后，脚本线程可能还在跑并接着开会话，那些会话同样得出现。
    ///
    /// 空队列时零成本（一次 `lock` + `take`），不进 `ViewState`——它装的是
    /// 跨线程交接，不是领域状态。
    script_opened: crate::script_host::OpenedQueue,
    /// 脚本 `session_activate` 的落点：脚本线程推进来，窗口每帧取走并切标签。
    script_activated: crate::script_host::ActivatedQueue,
    /// 脚本提问的队列（`ask` / `confirm` / `message` / `choose`）。
    ///
    /// 与 `script_opened` 同一种跨线程交接：脚本线程等在回信通道上，窗口每帧
    /// 取一个画成模态框。**同时只画一个**——脚本是顺序执行的，答案也该按序给。
    script_asks: crate::script_human::AskQueue,
    /// 脚本文件框的队列（`pick_file`）。用户打的路径**当场**进授权信封。
    script_picks: std::sync::Arc<std::sync::Mutex<Vec<crate::script_human::PendingPick>>>,
    /// 界面每帧抄进来的选区文本（`crt.GetSelection` 与 `selection ` 的事实源）。
    script_selection: crate::script_clip::SelectionSlot,
    /// 界面当前激活的会话（脚本 `session_active ` 的事实源）。
    script_active: std::sync::Arc<std::sync::Mutex<Option<SessionId>>>,
    /// 正在等答复的那个模态框（`None` = 没有脚本在问人）。
    script_ask: Option<overlays::AskModal>,
    /// 同会话互斥登记簿（ADR-0027 §S3）：三条运行入口共用**同一本**。
    ///
    /// 挂在窗口而不是各个对话框上：运行窗口、停靠面板、编辑器是三个独立的
    /// `Option<ScriptRun>`，各持一本的话「在面板里跑的时候，运行窗口那边还以为
    /// 会话是空的」，两条脚本就会对着同一个 PTY 交错读写。
    script_runs: std::sync::Arc<crate::script_run::ScriptRunRegistry>,
    /// 哪些会话的登录脚本已经跑过（ADR-0027 §S6）。
    ///
    /// 按 `SessionId` 而不是标签下标记：重排 / 分屏会让下标漂，而「这条会话的
    /// 登录脚本跑过了」是个**一次性**事实，认错对象就会跑两遍（登录脚本常常
    /// 是 `sudo` 或改配置，跑两遍比不跑更糟）。
    /// 同两组登记簿：`logon_fired` 是登录脚本；`auth_exec_*` 是 Authentication
    /// 页的认证后命令（待发时刻 + 一次性标记）。都按 `SessionId` 而非标签下标。
    logon_fired: std::collections::HashSet<SessionId>, auth_exec_pending: std::collections::HashMap<SessionId, std::time::Instant>, auth_exec_fired: std::collections::HashSet<SessionId>,
    /// 正在跑的登录脚本。**句柄要留着**：丢了就等于让它在后台对着 PTY 继续
    /// 说话，而界面上没有任何「它还在跑」的痕迹，同会话互斥也会提前放行。
    logon_runs: Vec<crate::script_run::ScriptRun>,
    /// 最近跑过的脚本（MRU 前 10 条）的**渲染期副本**。
    ///
    /// 构造时从 `prefs.recent_scripts` 抄一次，之后每次起跑再落盘（load-modify-save，
    /// 与 `address::save_recent_prefs` 同一套）。不每帧读盘：面板每帧都要拿它
    /// 拼下拉，一次 `read_to_string` + 反序列化放在帧里就是白烧 CPU。
    script_recent: Vec<String>,
    /// 录制器（ADR-0027 §S6）。`None` = 没在录。
    ///
    /// 包了一层 `Arc<Mutex<…>>` 而不是裸 `Option`：键盘漏斗在 `pane.rs` 里，
    /// 那里已经借走了 `tab`（`&mut self.tabs[i]`），再调 `&mut self` 的方法会
    /// 撞借用。克隆一个 `Arc` 出来就能在同一个作用域里记账。
    recorder: std::sync::Arc<std::sync::Mutex<Option<crate::script_recorder::Recorder>>>,
    /// 关闭确认弹窗（关闭软件 / 关闭会话）：待执行的关闭 + 两个抑制位。
    /// 与 `connector` 同类——带待执行副作用的投影态，见 `dialogs::confirm`。
    confirm: dialogs::confirm::ConfirmState,
    /// `Terminal ▸ Show paste dialog on the main screen`（`prefs.paste_confirm_main`）。
    ///
    /// 与 `focus_new_tab` 同一写法：构造时读一次 prefs，设置页改完写回 prefs 并
    /// 同步这份副本。判据是 `dialogs::confirm_paste::paste_needs_confirm`，读者
    /// 只有 `pane.rs` 的粘贴汇聚点。
    paste_confirm_main: bool,
    /// `Terminal ▸ Show paste dialog on the alternative screen`（`prefs.paste_confirm_alt`）。
    /// 备用屏标志取 `GridSnapshot.alt`，与主屏分开是两件不同的事。
    paste_confirm_alt: bool,
    /// `Terminal ▸ Mouse Actions ▸ Right click:` 的档位下标
    /// （`prefs.right_click_action`；语义见 `term_app::mouse::RightClickAction`）。
    ///
    /// 存下标而不是枚举：prefs 存的就是下标，中间再包一层枚举等于多一处
    /// 「下标 ↔ 枚举」的转换可以写错。用的时候一律 `from_index` 翻。
    right_click: usize,
    /// `Terminal ▸ Mouse Actions ▸ When I select text, copy it to clipboard`
    /// （`prefs.copy_on_select`）。
    ///
    /// 读者只有 `pane.rs` 里「选区文本每帧刷新」那一处，判据是
    /// `term_app::mouse::should_copy_on_select`（开关 + 非空 + 指针没按着 + 是新选区）。
    /// `Preferences ▸ Mouse Tracking` 的四类事件掩码（出厂全开）：
    /// 读者只有 `pane.rs` 的指针报告那一段，它不给事件编码，只决定某类事件
    /// 要不要交给 `term_core::mouse_report::encode_frame`。
    copy_on_select: bool, mouse_events: term_app::mouse::EventMask,
    /// 上一次由「选中即复制」写出去的文本，按槽位两份。
    ///
    /// 为什么要记：同一段选区每一帧都满足「非空、指针没按着」，
    /// 不记上一份就会每帧写一次剪贴板（并持续惊动系统剪贴板监听）。
    /// 清空时机只有一个：选区变空——那时下一段选区必须重新写。
    copied_on_select: Option<String>, copied_on_select_shell: Option<String>,
    /// 待确认的一次粘贴（`None` = 没有弹窗）。
    ///
    /// 与 `confirm` 同类：**一次待执行的副作用**，不进 `ViewState`——那里装的是
    /// 领域语义，装不下一段剪贴板文本与一个标签下标（reducer 不该认识剪贴板）。
    pending_paste: Option<dialogs::confirm_paste::PendingPaste>,
    /// ：传输的两块投影态——在跑的句柄与路径输入弹层（各自 `None`
    /// 即「没有」）。**句柄绝不能每帧重建**：`XferHandle` 的 `Drop` 会取消传输。
    /// 两者与 `connector` / `script` 同类——持有线程句柄的投影态，不进 `ViewState`。
    xfer: Option<terminal::xfer::XferJob>,
    xfer_prompt: Option<dialogs::xfer::XferPrompt>,
    /// 视觉基线夹具：设置后第一帧生效，用完即清空。
    demo: Option<crate::demo::DemoSpec>,
    /// P4 §7.3：hover 预览夹具冻定的轨道归一化位置（绝对 y）。仅 `--demo preview`
    /// 时被 set，生产路径恒 `None`。与 `demo` 同生命周期——第一帧生效、跨帧持续，
    /// 让预览框在每一帧都点亮（否则 `demo.take ` 之后只剩首帧有框，基线截图截到
    /// 的是末帧，框就消失了）。框内容（`preview_screen`）已挂在 `Tab` 上持久，
    /// 这里只持久「框位」。
    demo_preview_hover_y: Option<f32>,
    /// 组件夹具 ：设置后整窗只画这一个组件，画布是纯色。
    /// 与 `demo` 分开是因为两者回答的不是同一个问题：`demo` 问「整窗长什么样」，
    /// 这个问「这个组件长什么样」。混在一个字段里，组件出图就得先把终端铺好
    /// ——那正是「组件离不开整窗」的老毛病。
    comp_demo: Option<comp::CompId>,
    /// P4：停靠位与浮动位。与 `layout`（分屏树）同类——**投影层的视图结构**，
    /// 不进 `ViewState`：它要处理矩形与指针，放进去就等于让 reducer 认识 `egui`。
    dock: DockLayout,
    /// 脚本侧应用控制面的请求队列（`script_app::AppQueue`）。
    ///
    /// 与 `script_asks` / `script_picks` 同一层：脚本线程塞请求、界面每帧取一条。
    /// **读也走这里**——读共享槽会拿到上一帧的快照，脚本写完立刻读就会误判。
    app_queue: crate::script_app::AppQueue,
    /// 界面状态版本：只有**真的改了界面**的写入才 +1（同值不推进）。
    app_rev: u64,
    /// 偏好版本：偏好写入成功后 +1，与 `app_rev` 互相独立。
    prefs_rev: u64,
    /// P4：正在拖的面板与按下点。松手时要靠这两样区分「点了一下」与「拖了一下」。
    dock_drag: Option<(PanelId, egui::Pos2)>,
    /// 三条带各自当前选中的标签（`None` = 用第一颗）。按 `DockZone::ALL` 的下标索引。
    ///
    /// K3-b 从 `bottom_tab` 扩成整组：浮动带窗内也要用横向 tab 切成员，选择态
    /// 必须是「每带一份」，否则底带和浮起来的右带会抢同一个光标变量。
    /// 与 `dock` 同层：标签的选中态是**视图结构**，不是领域语义——`ViewState`
    /// 里没有「底带选中的是 Sender 还是 Shell」这种东西，重开窗口就该回到第一颗。
    band_tabs: [Option<PanelId>; 3],
    /// 已经替哪些浮窗视口要过前台、各要了几帧（上限见 `layout::dock_native`）。
    ///
    /// 原生浮窗刚建出来时可能**埋在主窗口后面**（实测 macOS）：创建它的那一刻
    /// 应用不在前台，它就一直排在后头，用户点了 Float 却看不到东西。这里只数
    /// 「打了几枪」，够数就停——不能一直要，否则用户点回主窗口会被反复抢焦点。
    /// 视口不浮了记录一起删（`draw_native_floats` 末尾），重新浮起来还能再要。
    float_focus_tries: Vec<(egui::ViewportId, u8)>,
    /// 底带"向上展开占满"（填满可用高度）。投影态，不进 `ViewState`。
    /// 注意与旧名 `bottom_collapsed`（折成标签条）相反：兼容终端点 `^` 是向上展开
    /// 占满全屏，不是收起——用户明确纠正过这个方向。
    bottom_maximized: bool,
    /// 底带分组：`Some(k)` = 前 k 个成员在左组（`Split` 的产物）。
    bottom_split: Option<usize>,
    /// 齿轮菜单攒下、还没落地的停靠命令。
    ///
    /// 左右带在自己的闭包里就地落地（画完这条带之后），但 Sidebar 走的是
    /// `draw_panel_body` 这条不认带的自绘通道，拿不到那条带的缓冲，只好挂到
    /// 这里，由 `flush_pending_dock_cmds ` 在三条带都画完之后统一落地。
    pending_dock_cmds: Vec<(crate::ui::layout::dock_bottom::Cmd, term_app::dock::DockZone)>,
    /// 底带高度。`egui` 的 `TopBottomPanel` 也记一份，这里是折起再展开时的回位值。
    bottom_height: f32,
    /// Sender 面板的控件状态（文本缓冲 / 编码 / 次数 / 间隔 / 目标 / 定时器）。
    ///
    /// 与 `find_input` 同一类：**控件状态**，不是领域语义。发送本身走的是
    /// `SupervisorHandle::write`，没有第二条写 PTY 的路（这条判据在  上）。
    sender: panels::SenderPanel,
    /// P9-f：工具栏面板被拖走后的左上角（`None` = 停靠在工具行里）。
    ///
    /// 与 `dock_drag` 同一层：这是**投影态**（处理矩形与指针），不进 `ViewState`
    /// ——`ViewState` 只装领域语义，装不下一个浮点坐标。它与 `find_input` 同理，
    /// 只在本进程里活着，不落盘。
    toolbar_pos: Option<egui::Pos2>,
    /// P4：本帧各停靠面板的**实际矩形**（落点要看面板真正画在哪，不能估算）。
    ///
    /// 每帧重建：面板宽度可拖、带内位次可换，上一帧的矩形这一帧就可能是错的。
    dock_rects: Vec<(PanelId, DockZone, egui::Rect)>,
    /// 启动会话策略（三档 + Session 1–4），初值来自 `prefs.json`。
    /// 不进 `ViewState`：它是**启动时刻**的配置，不是一个可以按了就翻转的
    /// 视图开关；设置页改完直接写盘，下一次启动由 `restore_layout` 读。
    startup: term_app::startup::StartupPolicy,
    /// 本次启动恢复出来的、还没尝试过连接的标签 id。
    ///
    /// 官方行为（`restore-sessions/index.md:27`）：会话恢复成断开态，
    /// **视图被激活时才连**。这里只登记 id，不登记槽位——重连是
    /// `PendingOpen::Replace`，槽位不变但 tab 会被整个换掉，而 id 会保留。
    /// ADR-0025 阶段 0：窗口打开标签的**恒在身份顺序**（与 `tabs` 等长、同序）。
    /// 身份键是 `Tab.identity`（`SessionId`），不随存活态 / 下标漂移；本阶段只在
    /// `construct::new ` 与 `restore_layout` 处初始化，后续标签增删 / 重排再同步。
    open_order: Vec<SessionId>,
    /// ADR-0025 阶段 0：当前活动标签的恒在身份（`None` = 无标签）。
    /// 取代「`active_tab` 下标即身份」的隐含假设；重排 / 关闭后活动身份不变。
    active: Option<SessionId>,
    /// 等待（重）连接的标签恒在身份：`restore_layout` 对内置 SSH 建出
    /// `session: None` 的标签后登记，连上后由 `poll_activation_connect` 取走。
    /// 连过一次（成功或失败）就摘牌，避免来回切标签无限弹连接窗口。
    pending_activation: Vec<SessionId>,
    /// `full_screen` / `always_on_top` 的**已应用值**。
    ///
    /// 这两个不是视图状态，而是「上一次发给窗口系统的值」。`ViewState` 说想要什么，
    /// 这里记已经是什么——每帧比较，只有变了才发 `ViewportCommand`。
    /// 无条件发会让窗口系统每帧收到一次同样的请求（部分平台会抖）。
    applied_window: AppliedWindow,
    /// 窗口透明度的运行期副本。初值来自 `prefs.json`，菜单三档与设置页
    /// 的开关 / 滑杆都改它并同时落盘；渲染每帧只读这一份，不碰盘。
    /// 不进 `ViewState`：它是**整窗 chrome 的视觉参数**，不是领域语义，
    /// 纯层 reducer 既不认识盘也不该持有 f32（见 `ui/window_opacity.rs`）。
    window_opacity: WindowOpacity,
    /// 地址栏浮层（锚在工具栏面板段上的连接目标输入框）。
    ///
    /// 不进 `ViewState`：它带一个 `egui::Rect` 锚点、一份 `prefs` 渲染期副本
    /// （`recent`）与一条待显示的报错文案——`ViewState` 只装领域语义，装不下
    /// 矩形与文案（同 `find_input` / `palette_input` 的分界）。
    address: address::AddressBar,
    /// D6：首次启动引导的状态（页号 / 探测到的主机 / 勾选位 / 提示语）。
    ///
    /// 同样不进 `ViewState`：它带着一串 `DetectedHost`（会话名 + 目标 + 来源
    /// 路径），`ViewState` 只装领域语义与开关（与 `address` / `find_input` 的
    /// 分界一致）。**可见性**那一个 bool 仍在 `ViewState::show_onboarding`——
    /// 两个字段恒定同步，改写点只有 `open_onboarding` / `finish_onboarding` 两处。
    onboarding: term_app::onboarding::OnboardingState,
    /// 首次启动的目录选择：选中档位 + 指定目录缓冲。可见性在
    /// `ViewState::show_profile_choice`，两者只在打开 / 确认两处一起改。
    profile_choice: overlays::ProfileChoiceState,
    /// D6：预览窗（Filer 双击 / 右键「预览」打开的文件）。
    ///
    /// 不进 `ViewState`：它带着一张 `TextureHandle`（显存句柄）与一段已解码的
    /// 正文——`ViewState` 只装领域语义与开关，装不下显存。**可见性**那一个 bool
    /// 在 `ViewState::show_preview`，两边只在 `open_preview_*` / `close_preview`
    /// 三处一起改（与 `onboarding` 同一条边界）。
    preview: overlays::PreviewState,
    /// D8-3：脚本编辑器。`None` = 没开过；`Some` 里带着可编辑缓冲与运行句柄。
    /// 与 `script`（只读路径的运行窗口）分开：那个按路径跑一次，这个能改能存。
    script_editor: Option<overlays::ScriptEditorState>,
}

/// 上次发给窗口系统的窗口属性。
#[derive(Clone, Copy, Default)]
struct AppliedWindow {
    full_screen: Option<bool>,
    always_on_top: Option<bool>,
    /// 上一次发给窗口系统的 `Transparent(bool)`。构造时初始化成盘上
    /// 那个值（= 建窗时 `with_transparent` 用的值），所以出厂态第一帧
    /// 一个视口命令都不发，现状 100% 不变。
    transparent: Option<bool>,
    /// 上一次装到 `egui::Visuals` 背景上的 alpha 千分比（1000 = 1.0）。
    /// 用整数记账，免得 f32 的末位抖动被当成「每帧都变了」。
    alpha_permille: Option<u32>,
}

// Tab 只是数据（字段多、没有绘制逻辑），搬到 `tab.rs` 让本文件留在 4o 的 500 行内。
pub use tab::Tab;

// --- Menu tree ---
// 菜单树本身是纯数据，已迁到 `app/menu.rs`（R3 / ）。本文件只剩绘制：
// 点击把命令 ID 交给 `dispatch`，未接线的项由 `capability` 判定并置灰。

impl TerminalWindow {
    /// ADR-0025 阶段 1：往主区 `tabs` 追加一条标签，同步 `open_order`，返回其下标。
    /// 所有「打开新标签」的入口收口到这里，避免散落各文件各自写一遍登记。
    pub(in crate::ui) fn add_tab(&mut self, tab: Tab) -> usize {
        let idx = self.tabs.len();
        self.tabs.push(tab);
        self.sync_open_order();
        idx
    }

    /// ADR-0025 阶段 1：在 `tabs` 增删之后，从 `tabs` 重算 `open_order`（与 `tabs`
    /// 等长、同序），让窗口恒在身份顺序始终反映真实标签集合。集中在这一处，
    /// 漏一次增量维护就错位。`active` 的同步属阶段 2，本阶段不动。
    fn sync_open_order(&mut self) {
        self.open_order = self.tabs.iter().map(|t| t.identity).collect();
    }

    /// ADR-0025 阶段 2：当前活动标签的**槽位下标**，由 `active`（身份）反查。
    /// 返回值供尚未身份化的消费者（分屏树 / 面板 / 绘制）沿用，直到阶段 3 把布局
    /// 也改成按身份寻址。身份不存在时退化成 0，与旧 `active_tab` 出厂值一致；
    /// 真正越界由 `.get ` 收口，不再有「下标指空」的隐患。
    pub(in crate::ui) fn active_slot(&self) -> usize {
        match self.active {
            Some(id) => self.tabs.iter().position(|t| t.identity == id).unwrap_or(0),
            None => 0,
        }
    }

    /// ADR-0025 阶段 2：`active`（身份）的写入收口——把槽位下标翻译成身份登记。
    /// 下标越界 / 空窗口时登记 `None`，等价「没有活动标签」，比旧 `active_tab`
    /// 存一个 stale 下标更安全。
    pub(in crate::ui) fn set_active_slot(&mut self, idx: usize) {
        self.active = self.tabs.get(idx).map(|t| t.identity);
        // ④-0：单窗格时，那唯一一片窗格的 group 也要跟着 active 走，
        // 这样 split 时 `pane_of_session(self.active)` 才找得到锚点。
        self.sync_single_pane_to_active();
    }

    /// ④-0：单窗格时把那唯一一片窗格的 group 指到当前 `active` 会话。
    ///
    /// 分屏态不碰 group——每个窗格各显各的会话，切全局 active 不改叶子内容
    /// （与旧行为一致：`draw_node` 按 `pane_groups[pane].active` 渲染，而切全局
    /// active 不会重排分屏树）。这只在「单窗格」下有意义：单窗格的渲染走
    /// `draw_terminal_area`（看 `active`），分屏时看 `pane_groups[pane].active`，
    /// 两者在单窗格下必须一致，否则从单窗格分屏会锚到错误的会话。
    pub(in crate::ui) fn sync_single_pane_to_active(&mut self) {
        if !self.layout.is_single() {
            return;
        }
        let Some(pane) = self.layout.leaf_panes().into_iter().next() else {
            return;
        };
        let Some(sess) = self.active else { return };
        self.pane_groups
            .insert(pane, PaneGroup { order: vec![sess], active: sess });
    }

    /// 此刻有没有**模态对话框**立着。灰罩与弹窗必须同一口径：
    /// 少算一处，那个弹窗还是融在主窗口里；多算一处，主窗口会莫名变暗。
    ///
    /// 命令面板 / Goto / 工作台 / 预览窗是有意**不**算的：它们是浮层而非模态，
    /// 主窗口在它们开着时仍可点（工作台里就摆着「切标签 / 关标签」的按钮）。
    fn modal_dialog_open(&self) -> bool {
        let v = &self.view;
        // 新建会话默认在**系统独立窗口**里画（用户两次确认）：它自带
        // 窗管外框，主体不需要被压暗——遮罩是「应用内弹层」的语义。只有在
        // 应用内回退路径上（截图夹具 / 后端没有多视口）它才退化成普通模态弹窗，
        // 那时照旧加罩，否则弹窗和终端糊成一片。
        //
        // 会话属性 / 默认模板始终是应用内弹层，一律加罩。
        let new_session_in_app = match self.session_dialog {
            dialogs::SessionDialogKind::New { .. } => harness::fixture_mode(),
            _ => true,
        } && v.show_new_session;
        new_session_in_app
            || v.show_settings
            || v.show_about
            || v.show_log_settings
            || v.show_triggers
            || v.show_onekeys
            || v.show_shortcuts
            || v.show_quick_commands
            || v.show_onboarding
            || self.confirm.pending.is_some()
            || self.pending_paste.is_some()
            || self.connector.is_some()
            || self.tunnel_editor.is_some()
            || self.xfer_prompt.is_some()
            || self.script.is_some()
            || self.script_ask.is_some()
            // 判 `open` 而不是只判 `is_some `：万一将来又有哪条路径留下
            // 「关着但还在」的状态，灰罩也不该因此永久盖住窗口。
            || self.script_editor.as_ref().is_some_and(|s| s.open)
            || self.filer.has_modal()
    }

    pub fn draw(&mut self, ctx: &egui::Context) {
        // L1/L3：提示条**第一个画**。`main.rs` 用 `catch_unwind` 包着整个 `draw`，
        // panic 之后本帧剩下的一半不会执行——画在最后就等于「崩了永远看不到为什么」。
        // 它是前景浮层，先画也压在上面。
        self.draw_notice(ctx);
        // L1 的故障注入（`WINDTERM_PANIC_FRAME=<n>`，只在 debug 构建里）：这里
        // 是「提示条已画出、后面半帧还没画」的位置，panic 的形状与真实崩溃一致。
        harness::maybe_panic_on_frame();
        // 组件夹具——整窗只画一个组件，其余一律不画。放在最前面，
        // 是为了让出图路径与交互路径彻底分开：组件图里出现会话树或状态栏，
        // 就说明「独立渲染」还没做到。
        // 夹具模式先把时间钉死：spinner 与光标闪烁都按 `i.time` 走，
        // 不冻住的话同一份代码两次出图的字节不同（逐字节基线永远红）。
        harness::freeze_clock(ctx);
        // 截图运行不再激活窗口（`ActivationPolicy::Prohibited`，见 `main.rs`），
        // egui 会把「未聚焦」画出来。夹具里钉成聚焦态，基线才是用户所见。
        harness::pretend_focused(ctx);
        if let Some(comp) = self.comp_demo {
            self.draw_component_demo(ctx, comp);
            return;
        }
        // 关闭确认弹窗：键盘模态 + 系统关闭请求的拦截，必须赶在所有事件
        // 消费者之前（见 `dialogs::confirm`）。
        self.confirm_pre_update(ctx);
        // 粘贴确认弹窗的键盘模态：同样要赶在终端吃键之前（弹窗开着时 Esc /
        // Enter 归它，其余键与文字一律吃掉）。
        self.paste_pre_update(ctx);
        self.handle_global_shortcuts(ctx);
        // 空闲超时先判：它派发的 Lock 在下面这一帧就生效，不多留一帧空窗。
        self.poll_lock_timeout(ctx);
        // V1-3：锁屏时只画遮罩，其余面板一律不画——否则「锁」只是装饰。
        if self.view.lock.locked {
            self.draw_lock(ctx);
            return;
        }
        apply_ui_style(ctx, &self.palette);
        // `install` 返回 `true` = 这一帧才把新字形定义交给 egui，它下一帧才
        // 生效（`Context::set_fonts` 只挂 `new_font_definitions`）。此时量 cell
        // 盒量到的是**旧字体**的宽度，所以跳过这一帧，下一帧再量。
        let defs_changed = self.fonts.install(ctx);
        self.fonts.measure_once(ctx, defs_changed);

        // 把「模式」落到窗口系统上。这是 `full_screen` / `always_on_top`
        // 从「一个没人读的 bool」变成真实效果的地方——之前点了没反应，
        // 因为它们只被 `reduce` 翻转，从来没有第二个人看。
        self.sync_window_modes(ctx);

        // 空闲 0 CPU —— 仅在新快照/状态到达时请求重绘，
        // 不再因为「进程还活着」就每帧无条件重绘。
        // 恢复出的断开态标签在**被激活时**才连（官方 restore-sessions
        // 文档）。放在每帧统一钩子里，各 `set_active_slot` 写入点一个都不用改；
        // 必须在 poll_connector 之前，让本帧就能推进刚发起的连接。
        self.poll_activation_connect();
        // N：30s 周期留存（路径 B）——崩溃 / 杀进程时至少能恢复最近一次方位。
        // 放在每帧钩子里：事件驱动的 `save_layout` 调用点覆盖「结构变化」，
        // 30s 防抖兜「安静运行 / 大量输出」那部分。
        self.poll_periodic_save(ctx);
        // Tabs ▸「上一次激活」那一档的历史。与上面同一个理由：每帧钩子比给
        // 各 `set_active_slot(…)` 写入点可靠——漏一处就少记一次激活。
        self.poll_tab_focus();
        self.poll_connector();
        self.poll_tunnels();
        self.poll_filer();
        // ：传输线程不会通知 egui，跑着的时候靠这里按 120ms 要重绘
        // （结束后就不再要，交回  的空闲 0 CPU 约定）。
        self.request_xfer_repaint(ctx);

        let mut changed = false;
        // 规则来自窗口级缓存（构造时从 prefs 读、设置页改完同步）；`run_commands`
        // 收集本轮命中里 `RunCommand` 要派发的命令 ID，循环结束后再由窗口级分发器
        // 统一处理（tab 层没有 `Action` 分发器）。按下标遍历而非 `&mut self.tabs`
        // 迭代器，以便同时持有一份 `&self.triggers` 不可变借用，互不打架。
        let rules = &self.triggers;
        // Account OneKey 的密码应答在保险库里，Tab 不认识保险库；这里派生一份
        // 「方案名 → 密码」表给本帧所有标签共用（锁着时是空表，静默跳过）。
        let n = self.tabs.len(); let autofill = self.onekey_autofill_map();
        let mut run_commands: Vec<String> = Vec::new();
        for i in 0..n {
            // 泵在 tab 里：它会经控制面把本帧事件并进快照，顺带刷新状态与
            // 日志态（那两样是**事件**报上来的，不是每帧去问会话线程）。
            if self.tabs[i].poll(rules, &mut run_commands, self.auto_reconnect, &self.onekeys, &autofill) {
                changed = true;
            }
        }
        // 底带 Shell 面板里的每一条都是**一条特殊会话**，泵得和标签页一样，
        // 一条都不能漏。少了哪一条，它的字节环就没人搬（tap 是无界 channel），
        // 而面板这边还照旧画着上一帧——长输出等于一条静默的内存泄漏。
        for tab in &mut self.shell_tabs {
            if tab.poll(rules, &mut run_commands, false, &[], &autofill) {
                changed = true;
            }
        }
        lifecycle_events::poll(self, crate::now_secs(), &mut run_commands);
        self.poll_auto_reconnect(); clipboard_policy::poll(ctx, self); modem_drop::poll(ctx, self); path_picker::draw(ctx, self);
        // `Log types` 的触发面：会话刚连上时按设置自动开一份日志。
        // 放在泵之后：边沿是 `poll` 置的，且此时 `SessionPool` 里那条会话
        // 已经真的在册（否则 `start_log` 的命令会发给一条不存在的会话）。
        self.poll_auto_log();
        for id in &run_commands {
            self.dispatch(id);
        }
        if changed {
            ctx.request_repaint();
        }

        // 每个面板都受自己的开关控制。专注模式压掉除菜单栏以外的一切——
        // **菜单栏必须留着**，否则用户点进专注模式后没有退出的路。
        let focus = self.view.focus_mode;
        // P4：落点边带按「整窗可用区」算，必须在任何面板切分之前取。
        let bounds = ctx.available_rect();
        self.dock_rects.clear();

        let menu_bar_h = shell::menu_bar_height(ctx, self);
        if self.view.show_menu_bar {
            egui::TopBottomPanel::top("menu_bar")
                .exact_height(menu_bar_h)
                .frame(shell::menu_bar_frame())
                .show(ctx, |ui| {
                    if menu_bar_h >= shell::MENU_BAR_H {
                        ui.horizontal_centered(|ui| self.draw_menu(ui));
                    }
                });
        }

        // P9：两条底部横栏相邻、整窗宽。先声明的贴窗口最底边，所以状态栏
        // 在前、快捷命令栏在它上方（视觉基线 G7/G8）。
        if self.view.show_status_bar && !focus {
            // 查找命中时整条状态栏换蓝底（视觉基线的联动），底色必须在这一层换。
            let accent = self.find_hit().is_some();
            egui::TopBottomPanel::bottom("status_bar")
                .exact_height(shell::STATUS_BAR_H)
                .frame(shell::status_bar_frame(accent))
                .show(ctx, |ui| {
                    self.draw_status_bar(ui);
                });
        }
        if self.view.show_quick_bar && !focus {
            let mut actions = Vec::new();
                egui::TopBottomPanel::bottom("quick_command_bar")
                    .exact_height(shell::QUICK_BAR_H)
                    .show(ctx, |ui| {
                        shell::draw_quick_command_bar(
                            ui,
                            &self.view.quick_groups_visible,
                            &self.quick_bar,
                            &self.quick_bar_display,
                            &mut actions,
                            self.lang(),
                            // 快捷栏是整窗宽的 `TopBottomPanel`：栏右沿 = 窗口右沿。
                            // 右端 `⚙ ×` 靠这个数往回退 7.5（视觉基线实测）。
                            ui.ctx().screen_rect().right(),
                        );
                    });
            for action in actions {
                self.dispatch_action(action);
            }
        }

        // ：Find 栏是**全局栏**，不是终端区的一部分。
        //
        // 原先它声明在 `CentralPanel` 之后——那时中央区已经把剩余空间吃光了，
        // 后建的 panel 分不到高度，结果是「夹具里 find 打开着，屏幕上却没有这条栏」。
        // 视觉基线给的位置很明确：Find 栏在**快捷命令栏之上**、整窗宽，与状态栏
        // 隔着快捷栏相望；搜索命中时被染色的是状态栏（`status_bar.rs` 读同一份
        // `view.find` 现算计数）。所以它必须和另外两条底栏一起、在任何中央面板
        // 之前声明。
        if self.view.find.visible && !focus {
            // 只读窗口：匹配数由 `app::find::matches` 从「query + 当前事实窗口」
            // 确定性算出，投影层既不计算也不存它。
            let total = match self.tabs.get(self.active_slot()) {
                Some(tab) => find::matches(&self.view.find, &tab.view).len(),
                None => 0,
            };

            // 先收集栏内动作，面板画完再统一 reduce，避免借用冲突。
            let mut find_actions: Vec<Action> = Vec::new();
            // R4：状态只读、缓冲可变——两者是不同字段，借用互不干扰。
            let find_state = &self.view.find;
            let input = &mut self.find_input;
            egui::TopBottomPanel::bottom("find_bar")
                .exact_height(shell::FIND_BAR_H)
                .frame(shell::find_bar_frame())
                .show_separator_line(false)
                .show(ctx, |ui| {
                    let mut dispatch = |action: Action| find_actions.push(action);
                    draw_find_bar(ui, find_state, input, total, self.view.lang, &mut dispatch);
                });
            for action in find_actions {
                // 窗口借用只活到 reduce 返回；副作用执行期间不再持有它。
                let effects = {
                    let window = self.tabs.get(self.active_slot()).map(|t| &t.view);
                    reduce(&mut self.view, action, window)
                };
                for effect in effects {
                    self.apply_effect(effect);
                }
            }
        }

        // 底带展开态的覆盖矩形：此刻只声明了菜单栏 / 状态栏 / 快捷栏，剩下的
        // 整块就是「上沿到菜单栏、下沿到状态栏」的区域。必须在底带与其余面板
        // **之前**取，否则会被它们切走。展开态由帧末的浮层盖住这一块。
        let bottom_overlay_rect = ctx.available_rect();

        // 新版截图：底带在左右带**下方**、横跨整窗宽度。egui 先声明的 panel
        // 占外圈，顺序必须是 状态栏 → 快捷栏 → 底带 → 左右竖带 → 标签条 →
        // 窗格工具行 → Sysmon 细带 → 中央区；底带挪到竖带之后就只切走中间一条。
        if !focus {
            self.draw_dock_zone(ctx, DockZone::Bottom);
        }

        // P4：**发射点固定，成员与位次是数据**。两条竖带都必须在标签条之前
        // 声明，竖带才是整高的、标签条只横跨终端区（视觉基线里会话树与右侧
        // Session 的标题都和标签条同一行）。修正：右带原先排在
        // 标签条之后，于是它矮了 56 点、标签条一路铺到右带上方，`A ＋ ≡`
        // 三颗也就永远对不上视觉基线里的位置。
        if !focus {
            self.draw_dock_zone(ctx, DockZone::Left);
            self.draw_dock_zone(ctx, DockZone::Right);
        }

        // T3 / P5：标签条与窗格工具行是两条独立顶栏（picture-1/2 的结构）。
        // 图标是窗格 chrome，总在；`show_quick_bar` 只管底部那条快捷命令栏。
        //
        // ④-1：这三条**全窗**顶/底栏只在单窗格时画。分屏时每片 pane 自己画
        // 一套（标签条 / 地址栏 / Sysmon，见 `layout/pane_chrome.rs`），否则
        // 会出现「全窗一条 + 每格一条」的重复 chrome。单窗格路径一个像素不动，
        // 12 张整窗视觉基线因此不受影响。
        if !focus && self.layout.is_single() {
            egui::TopBottomPanel::top("tabs_bar")
                .exact_height(shell::TAB_BAR_H)
                .frame(shell::bar_frame())
                // 视觉基线里标签条与工具行之间**没有**分隔线：两侧同色，只有标签
                // 块的明暗差。egui 默认会在条内最后 1 px 画一条 `#444444`，
                // 在两张深底之间就是一条凭空多出来的亮线。
                .show_separator_line(false)
                .show(ctx, |ui| {
                    ui.horizontal_centered(|ui| {
                        self.draw_tabs(ui);
                    });
                });
            // show_toolbar（原 Toolbar / Menu 页已移除）：工具行在此之前无条件绘制，现在受
            // `view.show_toolbar` 管。专注模式仍然连标签条一起压掉（上游那个 if
            // 已经管住了），所以这里不再重复判 `focus`。
            if self.view.show_toolbar {
                egui::TopBottomPanel::top("pane_tools")
                    .exact_height(shell::PANE_TOOLS_H)
                    .frame(shell::tool_row_frame())
                    .show_separator_line(false)
                    .show(ctx, |ui| {
                        self.draw_pane_tools(ui);
                    });
            }
        }

        // 右带已在两条竖带那里一起声明（见上）。

        // 视觉基线终端区下沿的 Sysmon 细带：底带已在左右带之前声明（见上），
        // 查找栏在 CentralPanel 之后声明，是覆盖而非切分。
        // ④-1：分屏时 Sysmon 由每片 pane 自己画（见 `pane_chrome`），这条全窗
        // 底带只在单窗格时声明。
        if !focus && self.layout.is_single() {
            egui::TopBottomPanel::bottom("sysmon_bar")
                .exact_height(shell::SYSMON_BAR_H)
                .frame(shell::sysmon_bar_frame())
                .show_separator_line(false)
                .show(ctx, |ui| self.draw_sysmon_bar(ui, None));
        }

        // P4：拖拽中的落点提示（只在按住时画）+ 松手判定。两件都必须在三条带
        // 都画完之后做——它们要用本帧各面板的真实矩形。
        self.paint_drop_hint(ctx, bounds);
        self.flush_pending_dock_cmds();
        self.finish_dock_drag(ctx, bounds);

        egui::CentralPanel::default().show(ctx, |ui| {
            // R14-d：只有真的分了屏才进递归切分；单窗格保持原路径。
            if self.layout.is_single() {
                self.draw_terminal_area(ui);
            } else {
                self.draw_split_layout(ui);
            }
        });

        // 底带展开态覆盖层：画在中央区**之后**，盖住上面的所有面板；放在对话框
        // 之前，让真正的模态（关闭确认 / 粘贴确认 / 地址栏）仍压在它上面。
        // 收回态整段不画，零像素变化。见 `layout/dock_bottom.rs`。
        if !focus {
            self.draw_bottom_dock_overlay(ctx, bottom_overlay_rect);
        }

        // ：传输进度行。与 `find_bar` 同一条位置逻辑——声明在
        // `CentralPanel` **之后**，覆盖终端底部一行而非切走它。**没有句柄时
        // 整段不执行**（连 panel 都不创建），空闲布局零变化。
        self.draw_xfer_status(ctx);

        // P4：浮动面板画在中央区之后、对话框之前——它们盖在终端上，但不能
        // 盖住对话框（对话框是模态的，浮在它上面就没法操作了）。
        if !focus {
            self.draw_floating_panels(ctx);
        }

        // 模态灰罩：盖住上面所有面板，对话框在更高的层里浮着。
        // `Order::PanelResizeLine` 保证它在 Background（面板）与 Middle（对话框）
        // 之间，与本句出现在哪个调用位置无关；放在对话框段之前只为可读性。
        if self.modal_dialog_open() {
            overlays::draw_backdrop(ctx);
        }

        if self.view.show_new_session {
            self.draw_new_session_dialog(ctx);
        }
        if self.view.show_settings {
            self.draw_settings_dialog(ctx);
        }
        // ：`Terminal.ManageTriggers` / `ManageOneKeys` 的落点是
        // **独立对话框**（兼容终端形态），不再借 Preferences 翻页。
        if self.view.show_triggers { self.draw_triggers_dialog(ctx); }
        if self.view.show_onekeys { self.draw_onekeys_dialog(ctx); }
        self.draw_script_manager(ctx);
        if self.view.show_about {
            self.draw_about_dialog(ctx);
        }
        // ：Tunnels 设置（`Terminal.TunnelSettings` / 菜单栏 Tunnels 下拉）。
        if self.tunnel_editor.is_some() {
            let names: Vec<String> = self.tabs.iter().map(|t| t.config.name.clone()).collect();
            ctx.data_mut(|d| d.insert_temp(egui::Id::new("tunnel_session_names"), names));
            let _ = crate::ui::dialogs::tunnel_settings::draw(self, ctx);
        }
        // 首次启动的 profile 目录选择：先定存储位置，再走引导。
        // 夹具模式两处都不开，视觉基线不受影响。
        if self.view.show_profile_choice { overlays::draw_profile_choice(ctx, self); }
        // D6：首次启动引导（Welcome → 探测到的 SSH 主机）。默认关闭时一行都不画，
        // 所以五张视觉基线不受影响；打开与否的判定在 `construct.rs::init`。
        if self.view.show_onboarding { self.draw_onboarding(ctx); }
        // D6：预览窗（Filer 双击 / 右键「预览」）。默认关闭时一行都不画，
        // 所以七张视觉基线与组件图不受影响。放在对话框这一段，是因为它与
        // 其余窗口同级：浮在终端之上，但被模态（关闭确认 / 粘贴确认）压住。
        if self.view.show_preview {
            self.draw_preview(ctx);
        }
        if self.view.show_log_settings {
            self.draw_log_settings_dialog(ctx);
        }
        self.draw_ssh_dialogs(ctx);
        // ：文件面板的写操作弹层（新建 / 改名 / 删除 / 权限）。
        // 默认 `None` 时一行都不画。
        self.draw_filer_prompt(ctx);
        // XFER-06：目录同步的三个窗（扫描中 / 审阅 / 执行）。同样默认不画。
        // 放在写操作弹层之后：两者不会同时立起来（同步期间菜单的写操作是灰的）。
        self.draw_sync_prompt(ctx);
        // 「运行脚本」窗口。同样默认关闭。
        // 先收脚本新开的会话（ADR-0025 阶段 5）：它不看窗口开没开，只看向队列里
        // 有没有东西——脚本线程可能在窗口关掉之后接着开会话。
        self.absorb_script_opened();
        // 脚本能看见的界面事实：选区与活动会话每帧抄一次（派生值，不是状态）。
        self.sync_script_slots();
        // 应用控制面：每帧最多处理一条请求（置位是 exclusive 的）。
        self.drain_app_requests();
        // 脚本的 `session_activate`：把那条会话的标签切到前台。
        self.drain_script_activated();
        // 登录脚本：标签带着活会话出现之后跑一次（§S6）。放在 `poll_script` 之前，
        // 这样登录脚本的第一条提示能在同一帧进状态栏。
        // 认证后命令与登录脚本同一帧调度：前者写进会话，后者起脚本线程。
        self.fire_logon_scripts(); self.fire_auth_commands();
        // 录制器：每帧看一眼屏幕是否安静（录制中才做事）。
        self.tick_recorder();
        self.poll_script();
        // 脚本提问的模态框：取一个、画一个、回一个。放在对话框这一段，与其余
        // 模态同级；`draw_address_bar` 那类「必须盖在最上」的浮层不受影响。
        self.poll_script_ask(ctx);
        self.draw_script_dialog(ctx);
        // 「脚本」停靠面板：每帧收一次运行结果（无论面板可见与否）。
        self.poll_script_panel();
        // D8-3：脚本编辑器（从「运行脚本」窗口的 Edit 进入）。默认没有状态，
        // 一行都不画。
        self.draw_script_editor(ctx);
        // ：传输的路径输入弹层（发要文件、收要目录）。默认关闭。
        self.draw_xfer_prompt(ctx);
        // ：关闭确认弹窗（关闭软件 / 关闭会话）与其他对话框同级。
        self.draw_confirm_close_dialog(ctx);
        // ：粘贴确认弹窗（`Terminal ▸ Show paste dialog on ...`）。
        // 与关闭确认同级：默认没有待确认的粘贴时一行都不画。
        self.draw_paste_dialog(ctx);
        // R14-c / R14-d / R14-f：三个浮层都放在最后画；默认关闭时一行都不画。
        self.draw_command_palette(ctx);
        self.draw_goto(ctx);
        self.draw_center(ctx);
        self.draw_shortcuts(ctx);
        // 独立的快捷命令配置弹窗（入口在底栏左侧配置菜单里）。
        self.draw_quick_commands(ctx);
        self.draw_tmux_panel(ctx);
        self.draw_tunnel_manager(ctx);
        // 地址栏浮层画在**最后**——它是直接输入焦点，必须盖在终端、面板
        // 与其余浮层之上。危险操作的模态（关闭确认 / 粘贴确认）虽在这之前
        // 声明，但 `draw_address_bar` 自己会在它们挂着时整段不画：那样才同时
        // 成立「输得进去」与「不盖住模态」，而不是靠这一个调用点的位置去赌。
        self.draw_address_bar(ctx);

        // 结构变化即时落盘——复活 `Action::SaveLayout` 死路径。各结构变更处
        // （分屏 / 关窗格 / 合并 / 内置 SSH 分屏落地 / sash 拖动）只置
        // `pending_save_layout`，这里在帧末统一派发一次，避免渲染期 / 嵌套
        // dispatch 重入。30s 周期（`poll_periodic_save`）与退出兜底保持不变。
        if self.pending_save_layout {
            self.pending_save_layout = false;
            self.dispatch_action(Action::SaveLayout);
        }

        // A3：本帧松手结束拖拽但没落在任何窗格（`drag_released_this_frame` 为真、
        // `drop_drag_on_pane` 没把它清掉）→ 拖拽态作废，免得卡死在下帧。命中落点的
        // 情况 `drop_drag_on_pane` 已经把 `drag_session` 置 `None` 且本旗复位，这里不动。
        if self.drag_released_this_frame {
            self.drag_released_this_frame = false;
            self.drag_session = None;
        }
    }

}

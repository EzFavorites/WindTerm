//! 词表层：`term-app` 的**纯类型闭合**。
//!
//! 这里只有枚举与它们的不变量，零函数、零 IO、零 `crate::` 依赖（只依赖
//! `term-core` 的坐标类型）。它是整个 crate 的最低层，所有能力模块、`state`、
//! `action`、`effect`、`parse`、`reduce` 都在它上面。
//!
//! 为什么这些类型必须集中在这里，而不是各留各的能力模块里：
//!
//! 因为 `Action` 与 `Effect` 要**包装**每一个能力的动作类型
//! （`Action::Shell(ShellAction)`、`Effect::Layout(LayoutAction)`…），
//! 而每个能力的 reducer 又必须**接收** `ViewState` 并**返回** `Effect`。
//! 只要那些类型留在能力模块里，就必然出现
//! `action → shell → effect → action` 这样的环——2026-11-12 实测正是
//! 13 个模块的强连通分量。把词表压到最低层之后，
//! `vocab → (无) → action → effect → 能力 → parse → reduce` 是一条单向链。
//!
//! 路径不受影响：原模块用 `pub use crate::vocab::X;` 原样再导出，
//! 于是 `term_app::shell::ShellAction`、`term_app::dock::PanelId` 这些
//! 既有写法一处都不用改（`apps/desktop` 有 59 个文件在用）。

pub mod tmux_palette;

/// `Script.RunFile:<路径>` 的 ID 前缀（不是完整 ID）。路径是任意串，不进枚举；
/// 放在词表层是因为入口层 `parse` 只许依赖本模块，而它必须认这个前缀。
pub const RUN_FILE_PREFIX: &str = "Script.RunFile:";

/// 一个能力就是一个可独立演进的纵切面。
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum FeatureId {
    Shell,
    Find,
    Clipboard,
    Selection,
    /// 会话日志（开始 / 停止 / 暂停 / 恢复 / 打开 / 设置）。
    Log,
    /// 命令面板（注册表 / 过滤 / 选中执行）。
    Palette,
    /// 跳到回看绝对行 / 已打开的会话。
    Goto,
    /// v0.1 工作台（15 条命令的页面状态机）。
    Center,
    /// 锁屏（复用 Vault 主密码）。
    Lock,
    /// 停靠 / 浮动。状态在投影层的 `DockLayout`，不在 `ViewState`。
    Layout,
    /// 2026-11-07：终端 margin 边栏的五列开关。状态就在 `ViewState.margin`，
    /// 绘制在 `ui/terminal/margin.rs`——每列都对应真的像素。
    Margin,
    /// 2026-11-09：文件传输（XMODEM / YMODEM 收发 + 取消）。
    ///
    /// 与 `Shell` 分开：它不改任何视图开关，也不发「一次性的窗口意图」——
    /// 它启动的是**一个有生命周期的后台传输**，进度要持续画在状态行上。
    /// 状态本身不在 `ViewState`（里面有一个线程句柄），这一层只出口令。
    Xfer,
    /// 标签右键菜单的 10 个动作。它们不在菜单栏里，
    /// 但是真实可点入口；`capability` 把 `tab::SHADOW_ENTRIES` 并进能力面，
    /// 于是「已接线」不再少算这一块。
    Tab,
    /// 显示模式：文本网格 ↔ 十六进制（`Mode ▸ Hex Edit`）。
    ///
    /// 与 `Shell` 分开的理由：它不改任何窗口 chrome，也不是「一次性窗口意图」，
    /// 而是**换了一种看字节的方式**——模式本身是每条会话的属性，落地在投影层
    /// 的 `Tab.mode`，**不落盘**（用户定死：「View 可以切当前会话的显示方式，
    /// 但是不会保存」）。单独一格，能力面才数得清这件事。
    Bin,
    /// 2026-11-13：空白与符号可视化（`View ▸ Symbols` / `View ▸ White Spaces` /
    /// `Text ▸ Highlight Caret Line`，10 条命令）。
    ///
    /// 与 `Bin` 分开的理由与它和 `Shell` 分开的理由同构：这十条改的是**栅格上
    /// 多画了什么**（空格点、行尾 `$`、参考列、折行勾、光标行底色），既不换
    /// 「怎么看字节」，也不动任何窗口 chrome，更不发一次性窗口意图。
    /// 落地在投影层的 `Tab.marks`（每标签一份、**不落盘**），渲染在
    /// `ui/terminal/marks_paint.rs`。归 `Shell` 会让这十条散进「窗口 chrome」
    /// 那一大堆里，能力面上就看不出「栅格可视化」这一块自己的进度。
    Marks,
}

impl FeatureId {
    pub const ALL: [FeatureId; 15] = [
        FeatureId::Shell,
        FeatureId::Find,
        FeatureId::Clipboard,
        FeatureId::Selection,
        FeatureId::Log,
        FeatureId::Palette,
        FeatureId::Goto,
        FeatureId::Center,
        FeatureId::Lock,
        FeatureId::Layout,
        FeatureId::Margin,
        FeatureId::Xfer,
        FeatureId::Tab,
        FeatureId::Bin,
        FeatureId::Marks,
    ];

    pub const fn name(self) -> &'static str {
        match self {
            FeatureId::Shell => "shell",
            FeatureId::Find => "find",
            FeatureId::Clipboard => "clipboard",
            FeatureId::Selection => "selection",
            FeatureId::Log => "log",
            FeatureId::Palette => "palette",
            FeatureId::Goto => "goto",
            FeatureId::Center => "center",
            FeatureId::Lock => "lock",
            FeatureId::Layout => "layout",
            FeatureId::Margin => "margin",
            FeatureId::Xfer => "xfer",
            FeatureId::Tab => "tab",
            FeatureId::Bin => "bin",
            FeatureId::Marks => "marks",
        }
    }
}

/// 传输协议。**故意不复用** `term-xfer::Protocol`：`term-app` 是投影层，
/// 让 UI 依赖协议实现会把两层焊死；映射在 desk 侧做一处。
///
/// 与菜单的关系：`Xmodem` / `Ymodem` / `Zmodem` 各有一对收发命令 ID；
/// `Xmodem1k` 是块长变体，菜单里没有独立命令，留给「1K 作为 XMODEM 的一个
/// 选项」时用——不给它编一个不存在的命令 ID。
///
/// ZMODEM 与另外三种不是同一个协议家族（内核在 `term-zmodem`，时序 / 帧格式 /
/// 错误恢复都不同），但对投影层而言它只是「又一个协议名」——分派发生在驱动层，
/// 本层不认识线上字节。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum XferProtocol {
    /// 128 字节块。
    Xmodem,
    /// 1024 字节块（尾块降到 128）。
    Xmodem1k,
    /// 头块带文件名与长度的批量传输。
    Ymodem,
    /// 可变长帧 + 帧内转义 + 按偏移重传。
    Zmodem,
}

/// 传输命令：发一份 / 收一份 / 取消当前那次。
///
/// 不带文件名与目标路径：那是**用户选择的结果**，由投影层先弹选择框再决定；
/// 纯层只声明「要开始一次哪种方向的传输」。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum XferCmd {
    Send(XferProtocol),
    Receive(XferProtocol),
    /// 取消正在跑的那次。没有在跑时是安静的空操作（不报错）。
    Cancel,
}

/// 外壳能力可执行的动作。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ShellAction {
    NewSession,
    OpenSettings,
    ShowAbout,
    DisconnectSession,
    ReconnectSession,
    CloseActiveView,
    ToggleMenuBar,
    ToggleStatusBar,
    ToggleQuickBar,
    /// show_toolbar（原 Toolbar / Menu 页已移除）：标签条下面那条窗格工具行的显示开关。
    ///
    /// 唯一入口是设置页那个勾选（没有对应的菜单项：视觉基线的工具行自带齿轮，
    /// 不为它再开一条同义菜单路径）。只翻 bool，画不画由 `ui/mod.rs` 决定。
    ToggleToolbar,
    /// 2026-11-07：左侧会话树（Session Manager）的显示开关。
    ///
    /// 侧栏标题栏的 × 走这一条。它必须是**开关**而不是「只关」：关掉之后
    /// 还要能再打开，回程是菜单 `Window › Panes › Toggle Sidebar`。
    ToggleSidebar,
    /// 右侧文件面板（空 = 关）。
    TogglePaneFiler,
    /// 六个工具面板开关。与 Filer 同款：只翻 bool，真正的内容由
    /// `ui/panes.rs` 每帧从事实状态现算；reducer 不碰任何面板数据。
    TogglePaneExplorer,
    TogglePaneOutline,
    TogglePaneSender,
    TogglePaneSession,
    TogglePaneShell,
    /// 2026-11-17：脚本面板的专用菜单项（`Window ▸ Panes ▸ Toggle Pane Script`）。
    TogglePaneScript,
    TogglePaneTransfer,
    /// tmux 控制模式面板的显示开关（`ViewState.show_tmux`）。
    ///
    /// 这块面板不是可停靠的 dock 面板（它是 `AnchoredWindow` 浮层），
    /// 所以不进 `PanelId`，但**必须有真实产品入口**：此前只有截图
    /// 夹具会把它设成 true，产品里点了任何地方都打不开。
    /// 现在 `Window › Panes › Tmux Control Mode` 走这一条，
    /// `ui/panels/tmux.rs::draw_tmux_panel` 每帧读开关并真的画出来。
    TogglePaneTmux,
    /// 2026-11-07：按身份翻**一块**面板的显示开关。
    ///
    /// 底带标签条的 `×`（关掉整条带）与齿轮菜单的勾选都走这一条：它们手上有
    /// 的是 `PanelId`，而上面那七条是「一条命令绑死一块面板」。与其在投影层
    /// 再抄一份 `PanelId → ShellAction` 的表，不如让纯层自己按 id 取字段——
    /// 归属只有一份（`crate::shell::panel_visible` / `set_panel_visible`，
    /// 起连绘制侧也调它，不再各列一份）。
    TogglePanel(PanelId),
    /// 2026-11-19：按身份**置位**一块面板的显示开关。
    ///
    /// 与 `TogglePanel` 的唯一区别是「置位 vs 取反」：脚本与 API 侧要的是
    /// 「把 filer 打开」，而取反语义要求调用方先读现状，两次往返之间界面可能
    /// 已经变了（TOCTOU），重试也不安全（重发一次会再翻回去）。
    ///
    /// 两条动作共用 `set_panel_visible` 那一张表，`toggle()` 只是它的
    /// 「先读再置反」包装——不新增第二条写入路径。
    SetPanel(PanelId, bool),
    /// Sysmon 读数条的**监控源**开关（本机 ↔ 远端）。
    ///
    /// 与面板开关同属窗口 chrome，但**没有命令 ID 字符串**：它是细带上那枚
    /// R 方块的内部动作，不进菜单 / 脚本 API / capability 库存，因此不增减
    /// 任何冻结的 ID 计数。状态写在 `ViewState::sysmon_remote`，唯一的写点在
    /// `shell::reduce`。
    ToggleSysmonRemote,
    ToggleFocusMode,
    ToggleFullScreen,
    ToggleAlwaysOnTop,
    /// 2026-11-07：**整带**开关（菜单栏右侧布局按钮的两项勾选）。
    ///
    /// 与上面七个单项开关的分工：单项翻一格，这一条翻一整带。判定是
    /// 「带里还有任何一块亮着就全关，否则全开」——同一条动作既当收起也当
    /// 展开，不需要额外记住关之前的组合（关掉必须有回程）。
    ///
    /// 只翻 bool，不碰面板数据，也不碰布局：带里各面板落在哪由
    /// `PanelId::default_zone()` 决定（`dock.rs`），本动作只管可见性。
    ToggleDocks(DockZone),
    /// 循环切到下一个 / 上一个标签（相对位移，到头回绕）。
    ///
    /// 真正的下标在投影层（`tabs` 长度、`active_tab`）算——reducer 拿不到标签列表，
    /// 也不该拿（那是 `apps/*` 的视图状态）。本动作只带「+1 / −1」的意图。
    ActivateNextView,
    ActivatePreviousView,
    /// 把活动标签的会话名复制到系统剪贴板（纯会话属性，不需要选区）。
    ///
    /// 文本在投影层取（`tab.config.name`）——reducer 没有那条会话，也不持有窗口。
    CopySessionName,
    /// 把活动标签的连接串复制到系统剪贴板。
    ///
    /// 形如 `ssh://user@host:port`（Shell 本地会话退化为纯名字）。同样只在投影层拼，
    /// 因为协议 / 主机 / 端口 / 用户名都由那条会话的配置决定。
    CopySessionUrl,
    /// 打开帮助（项目文档 / 上游仓库）。URL 是静态常量，reduce 直接带出去。
    Help,
    /// `View ▸ Syntax Schemes ▸ {cmd,…}`：切换终端的 16 色 ANSI 方案。
    ///
    /// **名字**是权威值，映射表只有 `term_core::theme::builtin_scheme` 一处。
    /// 与整窗主题（`theme_name`）不是同一件事：前者是终端格子的 16 色，后者
    /// 是窗口 chrome，两份真相不能互相顶替。reducer 只带名字，落地与写回
    /// 偏好都在投影层。
    SetScheme(&'static str),
    /// 用当前选区文本发起一次网络搜索。
    ///
    /// 查询词在投影层取（`selection_text`）——reducer 没有选区，选区是每帧从事实窗口
    /// 推导的派生值。动作本身只声明意图，应用层负责 URL 编码与拉起系统浏览器。
    SearchOnline,
    /// `Search ▸ Search Online ▸ {Google,…}`：六个引擎各一条命令，按**引擎名**
    /// 分派（名字来自 `term_profile::search::SEARCH_ENGINES`，这一层不认识 URL）。
    ///
    /// 与不带参数的 [`ShellAction::SearchOnline`] 同形：查询词仍是投影层的
    /// 选区（无选区退回输入缓冲），reducer 只吐一个带名字的意图。引擎名做成
    /// `&'static str` 而不是复制第六张 URL 表——设置页与菜单必须读同一份模板。
    SearchOnlineWith(&'static str),
    /// 终端字号缩放。+1 放大 / −1 缩小 / 0 复位。
    ///
    /// 目标字号在投影层算（当前字号 ±1，带上下限），因为它活在 `apps/desktop` 的字体
    /// 度量里，不属于「领域状态」——缩放是视觉调整，不进 `ViewState`。
    ZoomIn,
    ZoomOut,
    ZoomReset,
    /// E 类三条——应用级退出与标签页语义的关闭；另加一条新进程窗口。
    ///
    /// 与既有的 `CloseActiveView`（只断开、标签还在）不同：`Close` 真的把标签
    /// 从列表里摘掉，`CloseAllViews` 摘掉全部。标签列表活在投影层，所以这里
    /// 只带意图，一个字段都不写。
    /// 再起一个**独立应用实例**（新进程）。与 `tmux::Command::NewWindow` 不是一回事：
    /// 那条是 tmux 控制面的窗格命令，这条是桌面窗口。纯层只声明意图，不碰进程。
    NewWindow,
    Exit,
    Close,
    CloseAllViews,
    /// 分屏六条。方向（左右 / 上下）与「新建本地 shell / 复制当前会话」
    /// 两组语义在这里只作为意图；布局树与标签列表都活在投影层。
    SplitViewHorizontally,
    SplitViewVertically,
    SplitGroupHorizontally,
    SplitGroupVertically,
    CloseActiveGroup,
    MoveGroupToGroup,
    /// 多标签同步输入（C 类里唯一不需要新协议的一项）。
    /// 只翻 `ViewState` 的开关；真正的扇出在投影层的按键处理里。
    SyncInput,
    /// 选中最近一条命令（`Text.SelectCommand`）。块边界来自 OSC 133，
    /// 纯层只声明意图——选区落在投影层，因为它要事实窗口的行。
    SelectCommand,
    /// 隧道管理（`Terminal.ManageTunnels`）。打开面板；面板自己轮询。
    ManageTunnels,
    /// 2026-11-17：`Terminal.TunnelSettings` —— 打开 **Tunnels 设置对话框**。
    ///
    /// 与 `ManageTunnels` 是两件事：那条开的是「在跑的隧道」面板（看状态），
    /// 这条开的是配置编辑器（改名、增删、改目标）。同一个名字在兼容终端里也是
    /// 两个入口：`Tool ▸ Tunnels` 进设置，菜单栏那颗按钮的下拉进这里。
    ///
    /// reducer 只抛 `Effect::OpenTunnelSettings`：草稿与校验都在投影层。
    TunnelSettings,
    /// 2026-11-17：隧道**总线开关**（菜单栏 `Tunnels` 下拉第一行那颗胶囊）。
    ///
    /// 关掉 = 现在起着的全停、配置一条不丢；打开 = 按各自的 `Status` 重起。
    /// 开关值落盘（`Prefs.tunnels_enabled`），所以 reducer 只翻内存那份，
    /// 落地与写盘都交给 `Effect::ToggleTunnels`。
    ToggleTunnels,
    /// `Terminal.ManageTriggers`：打开设置对话框并**预选 Triggers 页**。
    ///
    /// 与 `OpenSettings` 的唯一区别就是落点：那条只把对话框打开（停在默认页），
    /// 这条要直接翻到规则编辑器。reducer 能做的一半是 `show_settings = true`
    /// （那活确实在 `ViewState` 里），翻页做不了——当前页是投影层的
    /// `TerminalWindow::settings_page`（`dialogs::SettingsPage`），纯层不认识
    /// 那页叫什么。所以后半截以 `Effect::FocusTriggersPage` 带出去。
    ManageTriggers,
    /// `Terminal.ManageOneKeys`：打开设置对话框并**预选 OneKeys 页**。
    ///
    /// 与 `ManageTriggers` 同形：reducer 做 `show_settings = true`（`ViewState`
    /// 里的字段），翻页以 `Effect::FocusOneKeysPage` 带出去——纯层不认识
    /// `SettingsPage` 的页面叫什么。OneKeys 页是凭据管理器
    /// （SSH / Account / Sessions 三标签），编辑结果落 `prefs` 并由
    /// `auth_chain` 在连接时读取。
    ManageOneKeys,
    /// `Terminal.ClearRecentUrls`：清空「最近使用」的目标串（地址栏那串候选）。
    ///
    /// **reducer 什么也不写**：那份列表活在 `Prefs.recent_targets`，缓存副本在
    /// 投影层的 `TerminalWindow::address.recent`。纯层既没有盘也没有缓存，
    /// 只把意图带出去；清空 + 落盘在 `ui/address/mod.rs::clear_recent_targets`
    /// 一处（地址栏右键的「Clear Recent」走的也是它，不新开第二条清理路径）。
    ClearRecentUrls,
    /// 显示模式切换：文本网格 ↔ 十六进制（`Text.ToggleHexEdit`）。
    ///
    /// **只声明意图，一个字段都不写**：模式是每条会话的属性，活在投影层的
    /// `Tab.mode` 里，而 reducer 手上只有全局 `ViewState`。真正的落地也在投影层
    /// （`ui/effects.rs::toggle_hex_mode`）。**不落盘**：用户定死「View 可以切
    /// 当前会话的显示方式，但是不会保存」，所以没有「上次那个模式」。
    ToggleHexEdit,
    /// 调试覆盖层开关（`Mode ▸ Debug Mode`）：把每格的 `Attrs` 八位、`fg`/`bg`
    /// 色号种类、`Cell.wide` 标成像素。
    ///
    /// 与 [`ShellAction::ToggleHexEdit`] 完全同一套两层模型：**只声明意图，一个
    /// 字段都不写**——它翻的是投影层每条会话的 `Tab.debug`，reducer 手上只有全局
    /// `ViewState`。落地在 `ui/effects.rs::toggle_debug_mode`，像素在
    /// `ui/terminal/debug_paint.rs`。**不落盘**：调试覆盖是「我此刻在查一个问题」
    /// 的临时状态，重启后自动开着会被当成渲染坏了。
    ToggleDebugMode,
    /// 只读查看开关（`Window ▸ Local Mode`，2026-11-19）。
    ///
    /// 与 [`ShellAction::ToggleDebugMode`] 同一套两层模型：**只声明意图，一个字段
    /// 都不写**——它翻的是投影层每条会话的 `Tab.local_mode`，reducer 手上只有
    /// 全局 `ViewState`。落地在 `ui/effects.rs::toggle_local_mode`。
    ///
    /// 只读的四条判据（不写 PTY / 不自动滚动 / 触发器不执行 / 脚本不执行）都在
    /// 投影层，`Tab::local_mode` 的文档是唯一事实来源；本枚举只负责「用户点了」。
    /// **不落盘**：只读是临时查看态，重启后一律可输入。
    ToggleLocalMode,
    /// `Mode ▸ Remote Mode`：把活动标签从只读查看态切回可输入的远端态。
    ///
    /// 与 `ToggleLocalMode` 是同一份 `Tab.local_mode` 的两个方向——这个变体
    /// 只把那一位清成 false（已经是远端态时是空操作），不落盘。它是菜单里
    /// `CheckState::Live` 的勾：勾上表示「当前是远端态」，由投影层现读。
    SetRemoteMode,
    /// `Text.ToggleWordWrap`（`Mode ▸ Word Wrap`）：把活动标签的逻辑行按 wrap
    /// column 软折成多段。与 `ToggleHexEdit` 同一套两层模型——只声明意图，
    /// 生效值活在投影层 `Tab.word_wrap`，reducer 不碰；不落盘。
    ToggleWordWrap,
    /// `View ▸ Hex Base`：一个字节用几进制写（2 / 7 / 8 / 10 / 16）。
    ///
    /// 与 `ToggleHexEdit` 同一类：reducer 只说「用户点了哪一档」，**不碰**
    /// 生效值——那份值活在投影层（`TerminalWindow::hex`）并落 `prefs.hex_*`。
    /// `ViewState` 里放排版参数会立刻变成一个「reducer 写、渲染读、设置页也写」
    /// 的三方共享变量，而它本来只有一个写入口（菜单）。
    HexBase(u32),
    /// `View ▸ Hex Columns`：每行几个字节（8 / 16 / 24 / 32）。
    HexColumns(usize),
    /// `View ▸ Hex Group Chars`：每几个字节连写一组（1 / 2 / 4 / 8）。
    HexGroupChars(usize),
    /// 2026-11-07：切换界面语言（首选项 ▸ 外观 ▸ 语言）。
    ///
    /// 配置里存 `application.language` 并**要求重启**；
    /// 这里立即生效——`ViewState` 是每帧都读的，没有缓存下来的文案。
    /// 落盘由 `term-runtime::prefs` 负责，reducer 只改内存状态。
    SetLanguage(crate::i18n::Lang),
    /// 空白 / 符号可视化的开关（`View ▸ Symbols` / `View ▸ White Spaces` /
    /// `Text ▸ Highlight Caret Line`，共 10 条命令）。
    ///
    /// 与 [`ShellAction::HexBase`] 同一类：reducer **只转述**用户点了哪一个旋钮，
    /// 不碰生效值。生效值活在投影层的 `Tab.marks`（`MarksParams`），
    /// **不落盘**——用户定死「View 可以切当前会话的显示方式，但是不会保存」。
    /// 落盘的是另一层：设置页写的 `Prefs.view_*` 是「默认参数」，只影响新标签。
    ///
    /// 为什么不给「切」与「设成某一档」各开一个变体：五档空白模式是**互斥单选**，
    /// 四个 bool 开关是**翻转**，两件事由 [`MarksKnob`] 自己分流；reducer 这层
    /// 一个字节的状态都不用读，所以一条 `SetMarks` 就够（与 `SetHex` 同款）。
    Marks(MarksKnob),
    /// `Application.SetWindow{Opaque,Translucent,Transparent}`：三档预设共用一条
    /// 带参数的动作（与 [`ShellAction::HexBase`] / [`ShellAction::Marks`] 同款）。
    ///
    /// reducer 只转述「用户点了哪一档」，**一个 `ViewState` 字段都不写**：生效值
    /// 活在 `Prefs.window_transparent` / `Prefs.window_opacity` 与投影层的运行期
    /// 副本里，落盘也在投影层——纯层既不认识盘，也不该为了一条视觉偏好新造一个
    /// 全局 bool（那会让「菜单写、渲染读、设置页也写」变成三方共享变量）。
    SetWindowOpacity(WindowOpacityPreset),
    /// `Application.SetWindowOpacity`：打开设置对话框并**预选能调透明度的那一页**。
    ///
    /// 与 [`ShellAction::ManageTriggers`] / [`ShellAction::ManageOneKeys`] 同形：
    /// reducer 做 `show_settings = true`（`ViewState` 里的字段），翻页做不了——
    /// 「当前页」是投影层的 `TerminalWindow::settings_page`，纯层不认识
    /// `SettingsPage`。后半截以 `Effect::FocusAppearancePage` 带出去，用户到
    /// `Appearance` 页用开关 + 滑杆自己调数值（语义假设，可在 GUI 验收后调整）。
    ManageWindowOpacity,
}

/// 窗口透明度的三档预设（`View ▸ Window Opacity` 子菜单的前三条命令）。
///
/// # 档位语义（产品假设，可在 GUI 验收后调整）
///
/// | 命令 | `window_transparent` | `window_opacity` |
/// |---|---|---|
/// | `Application.SetWindowOpaque` | `false` | `1.0` |
/// | `Application.SetWindowTranslucent` | `true` | `0.6` |
/// | `Application.SetWindowTransparent` | `true` | `0.3` |
///
/// 第四条 `Application.SetWindowOpacity` **不设值**：它打开设置并翻到
/// `Appearance` 页，让用户用滑杆自己调（见 [`ShellAction::ManageWindowOpacity`]）。
///
/// 0.3 是渲染侧下限（`prefs.rs:356` 注释写明 `[0.3, 1.0]`），再低整窗不可读；
/// 数值档位放在这里而不是让 `Effect` 带一个 `f32`：`Effect` 要 `Eq`（穷举 match
/// 与能力台账断言都靠它），而 `f32` 不实现 `Eq`。所以用**千分比** `u32` 过纯层，
/// 投影层一处除以 1000.0 写进 `Prefs.window_opacity`，不会有第二张映射表。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WindowOpacityPreset {
    /// 完全不透明：出厂态，所有既有视觉基线一个像素都不许变。
    Opaque,
    /// 半透明：整窗 60% 不透明。
    Translucent,
    /// 全透明（可读性下限）：整窗 30% 不透明。
    Transparent,
}

impl WindowOpacityPreset {
    /// 是否让 OS 窗口背景透明。启动时的 `NativeOptions.viewport.with_transparent`
    /// 与运行期的 `ViewportCommand::Transparent` 共用这一位。
    pub const fn transparent(self) -> bool {
        match self {
            WindowOpacityPreset::Opaque => false,
            WindowOpacityPreset::Translucent | WindowOpacityPreset::Transparent => true,
        }
    }

    /// 不透明度的千分比（1000 = 1.0，300 = 0.3）。
    pub const fn permille(self) -> u32 {
        match self {
            WindowOpacityPreset::Opaque => 1000,
            WindowOpacityPreset::Translucent => 600,
            WindowOpacityPreset::Transparent => 300,
        }
    }

    /// 落盘用的 `f32`。只在投影层落地时调用一次。
    pub fn opacity(self) -> f32 {
        self.permille() as f32 / 1000.0
    }
}

/// hex 渲染器要改哪一个旋钮，改到哪一档。
///
/// **不带「当前值」**：reducer 不知道现在是多少（那是投影层手上的
/// `TerminalWindow::hex` 与盘上的 `prefs.hex_*`），它只转述用户点了哪一档。
/// 夹取（非法档位 / 越界 → 回落出厂）也在投影层做，因为合法档位表是渲染侧的
/// 概念，纯层不该认识「7 进制是不是有效」。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HexKnob {
    Base(u32),
    Columns(usize),
    Group(usize),
}

/// 栅格可视化要改哪一个旋钮。
///
/// 与 [`HexKnob`] 同构：**不带「当前值」**——reducer 不知道现在开没开（那是
/// 投影层手上 `Tab.marks` 与盘上 `Prefs.view_*`），它只转述用户点了哪一个。
///
/// 四个 bool 开关与一个五档互斥模式共用一条 `Effect::SetMarks`：落地动作在投影
/// 层分流（翻转 / 设档），纯层不需要知道「Column Marker 是开关而 Show All 是
/// 单选」。`Whitespace(u8)` 里的编号就是 `Prefs.view_whitespace` 与
/// `WhitespaceMode::ALL` 的顺序（0 = Hide All … 4 = In Line End），
/// 所以「ID → 编号」在 `parse` 里就写死，没有第二张映射表。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MarksKnob {
    /// `View ▸ Toggle Line Endings`：逻辑行行尾的 `$`。
    LineEndings,
    /// `View ▸ Toggle Column Marker`：第 N 列那条贯穿竖线（开 = 出厂列号 80）。
    ColumnMarker,
    /// `View ▸ Toggle Indent Guides`：行首缩进参考线。
    IndentGuides,
    /// `View ▸ Toggle Wrap Symbols`：自动换行续行的折行勾。
    WrapSymbols,
    /// `Text ▸ Highlight Caret Line`：光标行那层淡底。
    CaretLine,
    /// `View ▸ White Spaces` 的五档之一（0 = Hide All … 4 = In Line End）。
    Whitespace(u8),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FindAction {
    Open,
    SetQuery(String),
    ToggleCase,
    /// `W`（Match Whole Word）：命中两端必须落在词边界上。
    ToggleWholeWord,
    /// `.*`（Regular Expression）：查询串按正则解释，而非字面子串。
    ToggleRegex,
    Next,
    Previous,
    /// 以「当前选区文本」为查询词，再跳到下一个 / 上一个匹配。
    ///
    /// 这里只负责「把查找栏打开、清空旧查询、回到第一个」——与 `Open` 同款复位；
    /// 真正的「用选区当查询词 + 跳一步」在投影层落地（查询词来自每帧推导的
    /// `selection_text`，reducer 没有选区，所以无法在这里填）。
    SelectionNext,
    SelectionPrev,
    Close,
}

/// 终端行过滤器的两档语义，对齐 `grep` / `grep -v`。
///
/// 住在 `vocab` 而不是 `state`：`FilterAction::SetMode` 直接携带它，而 `vocab`
/// 是分层最底层（词汇 → 副作用 → 状态 → …），`vocab` 不许反向依赖 `state`。
/// `state.rs` 用 `pub use crate::vocab::FilterMode` 原样再导出，既有引用点不改名。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum FilterMode {
    /// 只保留命中行（`grep`）。
    #[default]
    KeepMatching,
    /// 丢掉命中行（`grep -v`）。
    DropMatching,
}

/// 终端行过滤器的动作。没有命令 ID——入口是底部细带上的 `Filter` 按钮，
/// 与 `ToggleSysmonRemote` 同样是投影层的强类型意图，不进菜单树。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FilterAction {
    /// 打开过滤栏并复位。
    Open,
    /// 细带上的按钮：开 / 关翻转。
    Toggle,
    SetQuery(String),
    /// 把当前输入缓冲提交为生效查询，并记入**本会话**的过滤历史。
    ///
    /// 历史属于 UI 层的 `Tab`（不进 `ViewState`、不落盘），所以 reducer 不写它；
    /// 投影层 `apply_filter_on` 在收到这条动作时读 `Tab.filter_input` 落地。
    /// Enter / 下拉选中 / 关闭过滤栏才发这条，逐字符的 `SetQuery` 不发。
    Commit,
    /// `grep` / `grep -v` 两档。
    SetMode(FilterMode),
    ToggleCase,
    /// `W` 全词开关：命中两端必须是词边界。
    ToggleWholeWord,
    ToggleRegex,
    Close,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ClipboardAction {
    Copy,
    Paste,
    /// 把当前选区文本直接写给 PTY（等同于「复制选区 + 粘贴」一步完成）。
    PasteSelection,
}

/// 日志能力可执行的动作。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LogAction {
    Start,
    Stop,
    Pause,
    Resume,
    /// 用系统默认程序打开日志文件（还没有日志时由投影层给出理由）。
    OpenFile,
    /// 打开日志目录。
    OpenFolder,
    /// 日志设置对话框（状态、路径、按钮）。
    OpenSettings,
}

/// 面板范围。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PaletteMode {
    /// 只列已接线命令。
    Command,
    /// 列全部菜单项（含未接线的占位）。
    Anything,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PaletteAction {
    Open(PaletteMode),
    SetQuery(String),
    /// 选中项上下移动（+1 / −1，到头回绕）。
    Move(i32),
    /// 鼠标点中第几条（投影层不写状态，索引经动作交给 reducer）。
    Select(usize),
    Confirm,
    Close,
}

/// 两种跳转模式。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GotoMode {
    /// 输入回看绝对行号。
    Line,
    /// 在已打开的标签里选一个。
    Sessions,
    /// 在 OSC 133 命令块组成的「符号表」里选一条命令跳转。
    Symbol,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum GotoAction {
    Open(GotoMode),
    /// 行模式里 `+N` 的相对基准（当前光标的内容行号），由投影层注入。
    SetBase(i64),
    SetInput(String),
    /// `total` 是投影层此刻可见的条目数（只读派生值），用来做选中回绕。
    Move(i32, usize),
    Select(usize),
    /// 符号模式下选中某一绝对历史行，Confirm 时跳过去。
    SelectSymbol(i64),
    Confirm,
    Close,
}

/// 一条命令 = 一个页面。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CenterPage {
    OpenSession,
    Save,
    SaveSelectionAs,
    SessionSettings,
    DefaultSettings,
    Profile,
    ToolbarList,
    ToolbarCustomize,
    /// **命令行**页——把一行字当 shell 命令发给当前终端。
    ///
    /// 原名 `GotoAddressBar`，与参考夹具里那个「连接目标输入框」的地址栏撞了名：
    /// 命令 `Window.GotoAddressBar` 点开的是这一页，而用户以为它要打开地址栏。
    /// 把两件事分开——这一页改叫 `Command Line`（命令 ID 一起改成
    /// `Window.GotoCommandLine`），而 `Window.GotoAddressBar` 落到
    /// [`Action::AddressBar`]，由投影层直接开地址栏。
    GotoCommandLine,
    CompleteHistory,
    SchemeList,
    WindowList,
    WindowManager,
    FocusModeSettings,
    HighlightText,
}

impl CenterPage {
    pub const ALL: [CenterPage; 15] = [
        CenterPage::OpenSession,
        CenterPage::Save,
        CenterPage::SaveSelectionAs,
        CenterPage::SessionSettings,
        CenterPage::DefaultSettings,
        CenterPage::Profile,
        CenterPage::ToolbarList,
        CenterPage::ToolbarCustomize,
        CenterPage::GotoCommandLine,
        CenterPage::CompleteHistory,
        CenterPage::SchemeList,
        CenterPage::WindowList,
        CenterPage::WindowManager,
        CenterPage::FocusModeSettings,
        CenterPage::HighlightText,
    ];

    pub const fn title(self) -> &'static str {
        match self {
            CenterPage::OpenSession => "Open Session",
            CenterPage::Save => "Save Session",
            CenterPage::SaveSelectionAs => "Save Selection As",
            CenterPage::SessionSettings => "Session Settings",
            CenterPage::DefaultSettings => "Default Session Settings",
            CenterPage::Profile => "Profile",
            CenterPage::ToolbarList => "Toolbar List",
            CenterPage::ToolbarCustomize => "Customize Toolbar",
            // 这一页发的是 shell 命令，不是连接地址——标题必须说清是哪一件。
            CenterPage::GotoCommandLine => "Command Line",
            CenterPage::CompleteHistory => "Complete History",
            CenterPage::SchemeList => "Scheme List",
            CenterPage::WindowList => "Window List",
            CenterPage::WindowManager => "Window Manager",
            CenterPage::FocusModeSettings => "Focus Mode Settings",
            CenterPage::HighlightText => "Highlight Text",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CenterAction {
    Open(CenterPage),
    Close,
    SetInput(String),
    Select(usize),
    /// 记录一条发过的命令。
    Record(String),
    ToggleTool(usize),
    AddHighlight(String),
    RemoveHighlight(usize),
    SetNote(String),
    /// 进入「确认删除选中会话」两步确认态，避免误删。
    ConfirmDeleteSession,
    /// 取消删除确认，回到普通列表态。
    CancelDeleteSession,
    /// 会话管理器「移动到分组」下拉框的当前选择（绘制时提交回 reducer）。
    SetMoveGroup(String),
    /// 工作台配色下标。过去投影层直接写 `ViewState.center.scheme_idx`，
    /// 绕过 reducer 的第二份真相；现在和别的字段一样走 dispatch。
    SetScheme(usize),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LockAction {
    Lock,
    Unlock,
    Failed,
    ClearError,
    Cancel,
}

/// margin 的五列。
///
/// `Fold` 是「折叠标记列画不画」这条**列开关**，与「折哪个块」不是一件事——
/// 后者归 [`FoldCmd`]（见它的文档）。目前没有任何命令 ID 映射到
/// `MarginPart::Fold`（`Text.MarginFold` 走的是 [`FoldCmd::Toggle`]），
/// 所以这一列恒为关、`margin::reduce` 收到它时什么也不做：状态位必须与
/// 「画得出来」对齐，先给它一个能翻的开关就是假反馈。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MarginPart {
    Blank,
    Number,
    Fold,
    Symbol,
    Timestamp,
}

/// 一条 margin 命令：翻某一列。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MarginAction {
    Toggle(MarginPart),
}

/// 折叠族九条命令（`View ▸ Folds` 六条 + `Text ▸ Margin Fold` +
/// `Goto ▸ Fold ▸ Previous / Next Fold`），2026-11-14 一刀接通。
///
/// # 为什么给它们一个**独立**子枚举，而不是塞进各自的邻居
///
/// 三个 ID 前缀在词表里各有归宿（`View.*` 里 hex 与栅格可视化都在
/// [`ShellAction`]，`Text.Margin*` 在 [`MarginAction`]，`Text.Goto*` 映射到
/// [`Op`]），但这九条动作改的是**同一份状态**：投影层的 `Tab.folds`
/// （这个标签已经折了哪些块）。摊进邻居会立刻撞上两处语义冲突：
///
/// - **[`MarginAction`] 不行**：`Text.MarginBlank / Number / Symbol / Timestamp`
///   翻的是 `ViewState.margin` 里的**列开关**（那一列画不画）。Margin Fold 翻的
///   是**内容**——把光标所在的块折起来。两者状态不同、读者不同、效果不同；
///   共用一个变体等于逼 `margin::reduce` 去改一个它访问不到的字段。
///   [`MarginPart::Fold`] 因此也不能复用：它说的是「折叠标记列画不画」，
///   是一条纯粹的 margin 列开关，与「折哪个块」无关。
/// - [`Op`] 不行：`Op` 的定义是「**挪活动选区的 head**」，
///   `selection::apply` 的实现前提就是「只动选区，不动视口」。而折叠跳转恰恰
///   相反——终端里没有可移动的光标（光标属于 PTY），跳转只能移动**视口**。
///   硬塞进 `Op` 会逼着那个纯位移函数开始改视口，把两件事焊死。
///
/// 九条一个枚举还让「接线」在能力面上是一整块：`parse` 九条、`Effect::Fold`
/// 一条、落地一处（`apps/desktop/src/ui/effects.rs::apply_fold`）。不存在
/// 「六条接了、三条漏了」的中间态。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FoldCmd {
    /// `View ▸ Folds ▸ Fold`：把光标所在的块折起来。
    ///
    /// 没有块（光标在最后一条输入之后 / 第一条输入之上）或那个块已经在集合里
    /// 时是**空操作**，并且会留一句话——静默无反应会让人以为菜单坏了。
    Fold,
    /// `View ▸ Folds ▸ Unfold`：把光标所在的块解开。光标不在任何折叠里时无操作。
    Unfold,
    /// `View ▸ Folds ▸ Fold Inside`：把**当前视口内**的所有块折起来。
    ///
    /// # 「Inside」为什么是「视口内」而不是「再折一层」
    ///
    /// 本仓的块由**输入行**算出（`folds::blocks_from_inputs`），相邻两个输入
    /// `a < b` 给出闭区间 `[a, b - 1]`，于是所有块**平坦不嵌套**：区间两两不相交，
    /// 不可能出现「块里有块」。所以编辑器里那个「Fold Inside = 把当前块内部
    /// 再折一层」在这里没有对应物——本版根本没有「当前块」这个概念（终端不做
    /// 语法分析，不知道光标在哪个函数里）。
    ///
    /// 唯一说得通、且用户能看见的口径就是「限定在当前视口内」：等于对屏幕上
    /// 看得见的那几个块逐个执行 [`FoldCmd::Fold`]，看不见的块不动。写清楚这条
    /// 取舍比假装支持嵌套诚实——假装的做法是留一个永远算不出结果的
    /// `parent_block()`，然后所有人以为它有实现。
    FoldInside,
    /// `View ▸ Folds ▸ Unfold Inside`：解掉**当前视口内**的所有块（口径同上）。
    UnfoldInside,
    /// `View ▸ Folds ▸ Fold All`：所有块都折起来（不管在不在视口内）。
    FoldAll,
    /// `View ▸ Folds ▸ Unfold All`：清空折叠集合。
    UnfoldAll,
    /// `Text ▸ Margin Fold`：在光标行**切换**——落在已折的块里就解，否则折。
    ///
    /// 与 [`FoldCmd::Fold`] / [`FoldCmd::Unfold`] 的关系：那两条是**单向**的
    /// （「折」永远折、「解」永远解），这一条是**单向开关**（同一个入口一去一回）。
    /// 视觉基线里 `Margin Fold` 在 margin 那组命令里、带一个可勾的语义，所以它必须
    /// 有回程；而 Folds 子菜单已经把两个方向各给了一条，再让它们变成开关就会
    /// 出现「点两次 Fold 又展开了」这种反直觉的行为。
    Toggle,
    /// `Goto ▸ Fold ▸ Previous Fold`：视口跳到上一个折叠头。
    ///
    /// 「跳转」= 移动**视口**（终端没有可移动的光标），而且**不环绕**：
    /// 到顶就是到顶，回绕会让用户跳到自己看不见的地方。
    Previous,
    /// `Goto ▸ Fold ▸ Next Fold`：视口跳到下一个折叠头，同样不环绕。
    Next,
    /// 鼠标点折叠列上的某个头：切换**那一个**块（不是光标所在块）。
    /// 折叠头行号由投影层从点击的那一行算出，领域层只按这个绝对行号找块。
    ToggleAt(term_core::coords::HistoryRow),
}

/// 标签右键菜单的一个动作。10 个变体与旧 `Work` 一一对应。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TabAction {
    /// 激活目标标签（命令 ID 路径 = 当前标签，等于无操作）。
    Activate { target: Option<usize> },
    /// 关掉目标标签，走关闭确认与「最近关闭」队列。
    Close { target: Option<usize> },
    /// 只留目标标签，其余按从后往前的顺序关掉。
    CloseOthers { target: Option<usize> },
    /// 关掉全部标签（关空后按既有约定补一个本地 shell）。
    CloseAll,
    /// 重开最近关闭的那一条。
    Reopen,
    /// 复制目标标签的会话配置，按内置 SSH / 系统 ssh 分派连接。
    Duplicate { target: Option<usize> },
    /// 二分窗格；`horizontal = true` 是左右分屏（沿用旧 `Work::Split(bool)`）。
    Split { horizontal: bool },
    /// 把活动会话名写进剪贴板。
    CopyName,
    /// 把活动会话的连接串写进剪贴板。
    CopyUrl,
    /// 给目标标签上色；`slot = None` 是恢复默认。
    Color {
        target: Option<usize>,
        slot: Option<usize>,
    },
    // ── 终端内容右键的四条真实命令（2026-11-25 统一入口）──
    //
    // 这四条原先在投影层的私有 `Work` 枚举里直接调 `tab.clear()` /
    // `copy_command()`，绕开了 `Action::parse`：于是它们既没有命令 ID，也进不了
    // `capability` 台账，脚本 / 快捷键 / 命令面板都碰不到。提升到这里后，右键
    // 只发 ID，`dispatch` 走与其它命令完全相同的 parse → reduce → Effect 通道。
    // 四条都作用在**活动标签**，所以沿用 `Action::Tab` / `Effect::Tab` 这一对
    // 现成的纯层通道，不另造第二套 Effect。
    /// 复制 OSC 133 标记的当前整条命令（不是选区）到剪贴板。
    CopyCommand,
    /// 只清当前屏幕，历史保留。
    ClearScreen,
    /// 只丢历史（回看），屏幕留着。
    ClearScrollback,
    /// 清屏 + 清历史。
    ClearAll,
}

/// 一个可停靠面板的身份。
///
/// `name()` 是**唯一**的字符串身份：既是 `egui` 的 panel id（换掉它，`egui` 记的
/// 面板宽度就丢了，视觉基线会跟着动），也是 `layout.json` 里的键。两处用同一份
/// 名字，是为了让「布局文件里那个 panel」能一眼对上是屏幕上的哪块。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum PanelId {
    Sidebar,
    Filer,
    Explorer,
    Outline,
    Sender,
    Session,
    Shell,
    Transfer,
    /// 终端底部 `Sysmon` 读数条的展开态开关位。
    ///
    /// WindTerm 的 `Sysmon` 不是可停靠面板，而是终端区底部细带上的
    /// 无边框按钮 ↔ 原地 T/C/M 读数切换。这个枚举值**不再是停靠面板**：
    /// 它已从 `ALL` 移除（不进任何带、不进齿轮菜单、不画面板体），只作为
    /// `TogglePanel` 的状态键延续下来，供那条细带读同一位 `ViewState`。
    Sysmon,
    /// 「脚本」停靠面板：从脚本库挑一个 Rhai 脚本，在**当前主区活动标签页的会话**
    /// 上跑，结果日志就画在面板里。默认关着——和其它工具面板一样由齿轮菜单勾选打开。
    ///
    /// 复用 `apps/desktop/src/script_run.rs` 的执行线程（`ScriptRun`）；本面板只是把它
    /// 从「一次性的运行窗口」（`terminal/script.rs` 的 `ScriptDialog`）挪成「常驻面板」。
    Script,
}

impl PanelId {
    /// 全部可停靠面板，按**默认停靠顺序**排列（`DockLayout::new` 直接照它建）。
    ///
    /// 按视觉基线重排：左带 Explorer → Filer（上下堆叠），
    /// 右带 Sidebar(Session) → Outline（上下堆叠），底带 Sender → Shell。
    /// `DockLayout::new()` 是「遍历 ALL + 按 `default_zone` 归带」，所以
    /// **ALL 的先后就是带内从上到下 / 从左到右的位次**，不是随手排的清单。
    ///
    /// `Sysmon` 已移出（它不是停靠面板，见枚举值上的说明）。
    pub const ALL: [PanelId; 9] = [
        PanelId::Explorer,
        PanelId::Filer,
        PanelId::Sidebar,
        PanelId::Outline,
        PanelId::Sender,
        PanelId::Session,
        PanelId::Shell,
        PanelId::Transfer,
        PanelId::Script,
    ];

    pub const fn name(self) -> &'static str {
        match self {
            PanelId::Sidebar => "sidebar",
            PanelId::Filer => "filer",
            PanelId::Explorer => "pane_explorer",
            PanelId::Outline => "pane_outline",
            PanelId::Sender => "pane_sender",
            PanelId::Session => "pane_session",
            PanelId::Shell => "pane_shell",
            PanelId::Transfer => "pane_transfer",
            PanelId::Sysmon => "pane_sysmon",
            PanelId::Script => "pane_script",
        }
    }

    /// 浮动窗的标题栏文字。停靠时侧栏与文件面板不画标题（它们自带工具行），
    /// 只有六个工具面板用这个文字当 `heading`。
    pub const fn title(self) -> &'static str {
        match self {
            // 单数：3.x 会话管理面板的标题就是 `Session`（不是 `Sessions`）。
            PanelId::Sidebar => "Session",
            PanelId::Filer => "Filer",
            PanelId::Explorer => "Explorer",
            PanelId::Outline => "Outline",
            PanelId::Sender => "Sender",
            PanelId::Session => "Session",
            PanelId::Shell => "Shell",
            PanelId::Transfer => "Transfer",
            PanelId::Sysmon => "Sysmon",
            PanelId::Script => "Script",
        }
    }

    /// 七个工具面板与两个「自带工具行」的面板（侧栏 / 文件）。
    /// Sysmon 不是工具面板（见枚举值说明）。
    ///
    /// 这个区分只有一个用途：停靠容器要不要替它画 `heading`。侧栏第一行是
    /// 「＋ New Session」按钮、文件面板第一行是地址栏，再生硬加个标题就是
    /// 两行标题。
    pub const fn is_tool_pane(self) -> bool {
        !matches!(self, PanelId::Sidebar | PanelId::Filer)
    }

    /// 停靠容器要不要替它包一层 `ScrollArea`。
    ///
    /// 与 [`Self::is_tool_pane`] 差一个 `Shell`：那块面板装的是**终端**（与主区
    /// 标签页同一份渲染），两件事都不允许外面再包一层滚动——
    ///
    /// 1. 滚轮归终端自己的回看（`terminal/pane.rs` 的 `viewport.scroll_up/down`），
    ///    容器先吃一遍，回看就再也滚不动；
    /// 2. `ScrollArea` 把内容方向摊成无穷高，`draw_tab` 拿到的
    ///    `available_rect_before_wrap()` 会是 `f32::INFINITY`，PTY 的行数根本
    ///    算不出来（行数钳到 `MAX_ROWS`，屏幕上看不出，翻页全错）。
    ///
    /// 单独一个方法而不是在两处各写一遍 `is_tool_pane() && != Shell`：
    /// 判据散成三份（两个绘制点 + 一条门禁）时，漏改一处不会有任何东西报警，
    /// 症状只是「Shell 面板某天突然滚不动了」。`panels/mod.rs` 的
    /// `every_panel_owns_its_scroll` 拿它做异或判据。
    pub const fn scrolls_in_dock(self) -> bool {
        self.is_tool_pane() && !matches!(self, PanelId::Shell)
    }

    /// 默认落在哪个带（新版截图，2026-11-08）。
    ///
    /// - 左带 = `Explorer` + `Filer`：上半是资源管理器，下半是文件列表。
    /// - 右带 = `Sidebar`（会话树，标题就叫 `Session`）+ `Outline`，其余工具面板
    ///   也归右带但默认关着。
    /// - 底带 = `Sender` / `Shell`：一条带 + 标签条，不是两块上下叠着的面板。
    ///
    /// 这张表同时是 `shell.rs::zone_panels` 的归属来源，所以「整带一起收起」
    /// 这类动作不用另立一张表。改这里等于改 `ToggleDocks` 的作用范围。
    pub const fn default_zone(self) -> DockZone {
        match self {
            PanelId::Explorer | PanelId::Filer => DockZone::Left,
            PanelId::Sender | PanelId::Shell | PanelId::Script => DockZone::Bottom,
            _ => DockZone::Right,
        }
    }

    pub fn parse(name: &str) -> Option<PanelId> {
        PanelId::ALL.into_iter().find(|p| p.name() == name)
    }
}

/// 停靠带。**只有三条**，与 `draw()` 里固定的三个发射点一一对应。
///
/// 没有 `Top`：顶上是菜单栏与标签条两条固定框架，允许把面板塞到它们中间只会
/// 让「标签条横跨到哪」变成不可解释的问题。少一条带比多一条说不清的规则好。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum DockZone {
    Left,
    Right,
    Bottom,
}

impl DockZone {
    pub const ALL: [DockZone; 3] = [DockZone::Left, DockZone::Right, DockZone::Bottom];

    pub const fn name(self) -> &'static str {
        match self {
            DockZone::Left => "left",
            DockZone::Right => "right",
            DockZone::Bottom => "bottom",
        }
    }

    pub fn parse(name: &str) -> Option<DockZone> {
        DockZone::ALL.into_iter().find(|z| z.name() == name)
    }
}

/// 布局动作（新子枚举）。
///
/// 六个动作都**不碰** `ViewState`：布局不是领域状态，是投影层的视图结构。
/// `reduce()` 原样把它们包进 `Effect` 抛回去，落地的活由投影层干——它才认识
/// 矩形与 `egui` 的指针位置（例如「浮到哪」取决于松手时的鼠标）。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LayoutAction {
    /// 停到某带的第 `index` 位（拖拽落点算出来的）。
    Dock {
        panel: PanelId,
        zone: DockZone,
        index: usize,
    },
    /// 单块面板变成浮动窗。**不携带坐标**：坐标来自松手时的指针，属于投影层。
    Float(PanelId),
    /// 整条带一起浮动，窗内用横向 tab 切成员（兼容终端「浮动窗口」的形态）。
    FloatBand(DockZone),
    /// 收回它的默认带（浮动窗标题栏的 ×，以及布局文件里读不出矩形时的退路）。
    Redock(PanelId),
    /// 整条带收回原位次：浮动带窗口原生标题栏的 ×。
    RedockBand(DockZone),
    /// 全部回到默认布局。浮动窗拖丢之后唯一的确定性出口。
    Reset,
}

/// 选区操作：50 条菜单命令与 Shift+方向键共用的**同一组意图**。
///
/// 为什么是枚举而不是「一条命令一个函数」：17 条里 6 组是同一种移动的不同方向，
/// 键盘还要复用其中一部分。枚举是唯一的词汇表，`parse` 只做 ID → 意图的映射，
/// 真正的位移规则只有 `apply` 一处——菜单点「下一行」和按 Shift+↓ 必须得到同一个结果。
///
/// 命名按**阅读顺序**（prev/next），不按锚点在左还是在右：头部向左移动永远叫
/// `prev`，这样 Shift+← 连按两次不会因为跨过锚点而突然变成向右。
///
/// 2026-11-13 一刀加了 26 个变体（17 → 43），把菜单上最后一批置灰的光标 / 选区
/// 命令接上。它们分七族，每族的**边界判定**都落在纯层
/// （`selection/geometry.rs` / `selection/words.rs`）：`apply` 里只有「取边界 →
/// 造 head」这一句，不在 match 里现算。分族的理由是这七族各自要回答一个不同的
/// 问题（矩形语义 / 子词切分 / 半页步长 / 段落 / 命令块 / 缩进 / 括号配对），
/// 而同一族里的方向差异只是 `prev` / `next` 一个符号。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Op {
    All,
    Word,
    Line,
    ToDocumentStart,
    ToDocumentEnd,
    ToLineHome,
    /// Vim `^`：跳到当前行第一个非空白字符。终端里没有 Vim 的光标对象，
    /// 这条仍按选区头的绝对行取字形，保证与 `Home` / `End` 同一坐标系。
    ToLineFirstNonBlank,
    ToLineEnd,
    CharPrev,
    CharNext,
    LinePrev,
    LineNext,
    PagePrev,
    PageNext,
    WordStartPrev,
    WordStartNext,
    WordEndPrev,
    WordEndNext,
    // ── Select Column To：10 条，矩形（Block）语义 ──
    //
    // 与上面同名的 `CharPrev` / `LineNext` / `PagePrev` 不是同一件事：那几条按
    // **线性**语义移动（Char 会跨行回绕、选区形状沿用旧的），这 10 条按**按列**
    // 语义移动，并且选区形状**无论首次与否都落成 `Block`**。两条口径不能混：
    // 「Select To Next Line」是流选（中间整行吞掉），「Select Column To Next Line」
    // 是只加一行、列不动。
    ColumnCharPrev,
    ColumnCharNext,
    ColumnLinePrev,
    ColumnLineNext,
    ColumnPagePrev,
    ColumnPageNext,
    ColumnParagraphPrev,
    ColumnParagraphNext,
    ColumnLineHome,
    ColumnLineEnd,
    // ── Word Part：子词（驼峰 / 下划线 / 字母数字交界）──
    WordPartPrev,
    WordPartNext,
    // ── Stuttered Page：半页步长 ──
    StutteredPagePrev,
    StutteredPageNext,
    // ── Paragraph：段 = 连续非空行 ──
    ParagraphPrev,
    ParagraphNext,
    // ── Command：OSC 133 命令块的 prompt_end ──
    CommandPrev,
    CommandNext,
    // ── Scope：终端里没有嵌套作用域，退化成一整条命令区段 ──
    ScopePrev,
    ScopeNext,
    // ── Indentation：行首空白宽度变化处 ──
    IndentationPrev,
    IndentationNext,
    // ── Pair：`()[]{}` 深度配对 ──
    PairStart,
    PairEnd,
    PairPrev,
    PairNext,
    // ── M1（2026-11-27）：按语义单位整段选中（Select 5）──
    //
    // 上面那 43 条都是「挪 head 到某个边界」，这五条是「选中 head 所在的
    // 整个语义单位」——与 `Word` / `Line` 同款：锚点取单位首、头部取单位尾。
    // 它们的边界判定同样落在纯层（`selection/units.rs`），不带方向。
    Paragraph,
    Pair,
    Scope,
    Indentation,
    Fold,
    // ── M1：Select To 的 Fold 两条（其余单位复用上面已有的边界 Op）──
    //
    // 折叠块不是网格字形，而是「两次用户输入之间」的区间，边界要折叠引擎
    // 的输入行表才算得出来；所以另立两个变体，经带上下文的入口落到纯函数。
    FoldPrev,
    FoldNext,
}

/// 9 条集合命令（菜单 Selection / Goto 里那批「多选区」项）。
///
/// 与 `Op` 分开的理由：`Op` 是**一段区间**的位移规则，这 9 条改的是**集合**
/// 本身（活动下标 / 成员 / 锁 / 形状 / 端点）。混进 `Op` 会让 `apply` 这个
/// 纯位移函数开始管集合，Shift+方向键也会跟着走进它用不上的分支。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SetOp {
    ActivateNext,
    ActivatePrevious,
    Lock,
    Drop,
    Cancel,
    Split,
    Invert,
    SwitchShape,
    Exchange,
}

/// 派发给投影层的一条选区命令。
///
/// 两档合一：`Move` 挪头部（17 条选择命令 + Goto 的 4 条光标移动 + Shift+方向键），
/// `Set` 改集合（上面 9 条）。投影层只认这一个类型，所以「待办」只有一个槽位，
/// 不存在两条命令抢同一个槽位的问题。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Cmd {
    Move(Op),
    Set(SetOp),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Action {
    Shell(ShellAction),
    Find(FindAction),
    /// 行过滤器意图；状态在每条会话的 `Tab.filter`，reducer 只产 `Effect::Filter`。
    Filter(FilterAction),
    Clipboard(ClipboardAction),
    /// 选区移动。真正的位移在投影层拿到事实窗口后由 `selection::apply` 算，
    /// 本层只把命令翻译成意图——因为它拿不到「现在画在屏上的那一屏」。
    Select(Op),
    /// 多选区集合命令（激活下一份 / 锁 / 丢 / 取消 / 拆分 / 反选 / 换形状 /
    /// 换端点）。与 `Select` 分开是因为它改的是**集合**而不是一段区间的端点；
    /// 但两者最终都落到投影层的同一个待办槽位（`selection::Cmd`）。
    SelectSet(SetOp),
    /// 会话日志。同样只带意图：文件名与目录由写入端与投影层决定。
    Log(LogAction),
    /// 命令面板。打开 / 输入 / 移动选中 / 确认 / 关闭都是纯状态动作。
    Palette(PaletteAction),
    /// Goto 面板。行号解析与会话选中都在纯层，投影层只落地跳转。
    Goto(GotoAction),
    /// 工作台。状态只由 `center::reduce` 写，副作用复用投影层既有方法。
    Center(CenterAction),
    /// 锁屏。密码校验在投影层，状态只由 `lock::reduce` 写。
    Lock(LockAction),
    /// 停靠 / 浮动。布局树不在 `ViewState` 里（与分屏树同类，是投影层的
    /// 视图结构），所以这一支的 reducer 只把意图包成 `Effect` 抛回去。
    Layout(LayoutAction),
    /// 2026-11-07：margin 边栏的列开关。纯状态动作，不产 `Effect`。
    Margin(MarginAction),
    /// 折叠族九条（`View ▸ Folds` 六条 + `Text ▸ Margin Fold` +
    /// `Goto ▸ Fold` 两条）。
    ///
    /// 与 [`Action::Margin`] 相反：这一支**不写任何 `ViewState` 字段**，只吐
    /// `Effect::Fold`。折叠集合是**每条会话**的属性（`apps/desktop` 的 `Tab.folds`），
    /// 与 `mode` / `hex` / `marks` 同一条理由：同一个窗口里两个标签可以一个收着
    /// 输出、一个摊开看。而 reducer 手上只有全局 `ViewState`，所以它连「现在折了
    /// 哪些块」都读不到——决策所需的输入（输入行、光标行、视口区间）全在投影层。
    Fold(FoldCmd),
    /// 2026-11-09：文件传输。与 `Margin` 相反——它**只**产 `Effect`，
    /// 一行 `ViewState` 都不写：传输句柄与进度都在投影层。
    Xfer(XferCmd),
    // ── 投影层直接构造的强类型意图（不经命令 ID 派发，无字符串旁路）──
    ImportSessions(&'static str),
    SaveLayout,
    OpenShortcuts,
    /// 打开独立的「快捷命令」配置弹窗。命令表来自 `quickbar.json`，不放在
    /// Preferences 的 Quick Bar 页里（那一页按视觉基线只有三个 Features 开关）。
    OpenQuickCommands,
    TmuxCommand(String),
    TmuxFeed(String),
    /// 底部快捷命令栏。参数是**送进输入行的文本**，不是命令 ID，
    /// 也不带回车——点一下就执行 `reboot` 是危险的（见 `ui/quick_command_bar.rs`）。
    /// 命令表现在来自 `quickbar.json`（用户可改），所以是 owned `String`。
    QuickCommand(String),
    /// 左侧配置菜单里切换某个快捷命令组的可见性。index 是
    /// `quickbar.json` 组表的下标，与 `ViewState.quick_groups_visible` 一一对应；
    /// reducer 只翻这一位 `bool`，不产任何 Effect（纯 UI 状态）。
    QuickGroupToggle(usize),
    /// 标签右键菜单的 10 个动作。reducer 只把它包成
    /// `Effect::Tab`，一行 `ViewState` 都不写——标签列表、
    /// 「最近关闭」队列与颜色表都在投影层。
    Tab(TabAction),
    /// 打开地址栏（连接目标输入框），`Window.GotoAddressBar` 的命令面。
    ///
    /// # 为什么是顶层变体，而不是又一个 `CenterPage`
    ///
    /// 因为它**不是一页工作台**：地址栏是贴在工具行上的输入框，它的开关、
    /// 候选与最近使用都在 [`crate::address`] 与投影层，与中心面板的页面状态机
    /// 无关。做成 `CenterPage` 就等于把「输入框」画成「一页带标题的对话框」，
    /// 那正是要解掉的语义冲突。
    ///
    /// `reduce()` 对它**不产生 `Effect`**，也**不写任何 `ViewState` 字段**
    /// （地址栏状态由投影层持有；本层既不该编造一个它会忘了维护的 bool，
    /// 也不该为一条纯 UI 意图凭空造一个副作用）。消费点是另一半——
    /// `apps/desktop` 的 `dispatch_action` 认这个变体，就地开地址栏。
    AddressBar,
    /// 运行一个已登记脚本（映射键 / 按钮栏 / 命令面板的 `Script.RunFile:<path>`）。
    ///
    /// 路径是任意串，做成枚举变体不会让 `Action` 认识文件系统——它只携带文本，
    /// 真正读文件、起线程的是投影层。reducer 对它不写状态、不产 Effect，
    /// 消费点在 `apps/desktop` 的 `dispatch_action`（与 `AddressBar` 同一形态）。
    RunScript(String),
}

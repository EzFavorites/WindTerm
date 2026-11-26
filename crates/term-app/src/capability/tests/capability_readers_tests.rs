//! 「已接线命令必须有真实读者」的穷举清单。从 `capability.rs` 拆出来：
//! 那张表本身就是能力面的显式账本，放在 4j 的代码行大户里迟早超标。

use super::*;
use std::collections::BTreeSet;

#[test]
fn every_implemented_command_has_a_reader() {
    // 教训：命令「能被 parse」不等于「点了有效果」。
    // 这条测试列出**允许**出现在已接线集合里的命令；新增一条必须同时回答
    // 「谁读它」。目前分三类：
    //   1) 打开对话框 / 触发副作用（NewSession / OpenSettings / About / 断开 / 重连 / 关闭）；
    //   2) 直接决定绘制分支（MenuBar / StatusBar / QuickBar / FocusMode）；
    //   3) 每帧同步到窗口系统（FullScreen / AlwaysOnTop）。
    let expected: BTreeSet<&str> = [
        "Terminal.NewSession",
        "Window.OpenSettings",
        "Help.About",
        "Terminal.DisconnectSession",
        "Terminal.ReconnectSession",
        "Window.CloseActiveView",
        "Window.ToggleMenuBar",
        "Window.ToggleStatusBar",
        "Window.ToggleQuickBar",
        // 2026-11-07：margin 边栏四列。读者是 `ui/terminal/margin.rs`——
        // 每一列都真的改边栏像素（宽度、行号、符号、时间戳），
        // 不是「翻了个没人看的 bool」。`Text.MarginFold` **不在**这里：
        // 它翻的是「折哪个块」而不是「画不画那一列」，2026-11-14 起归
        // 下面折叠族那一段（`Action::Fold(FoldCmd::Toggle)`）。
        "Text.MarginBlank",
        "Text.MarginNumber",
        "Text.MarginSymbol",
        "Text.MarginTimestamp",
        // 2026-11-18：第五列。读者与上面四列同一处（`margin.rs`），
        // 但它读的是**折叠集合**：`is_fold_head(tab.folds, abs)` 决定在折叠头
        // 那一行画 `▸`。与 `Text.MarginFold` 的差别只有一格语义——
        // 那条折块，这条画「哪里折着」。
        "Text.MarginFoldColumn",
        // 2026-11-07：会话树开关。读者是 `crate::shell::panel_visible`
        // （`PanelId::Sidebar` 的 `show_sidebar` 分支，`ui` 直接调它）；
        // `ui/panels/sidebar_view.rs` 标题栏的 × 是它的写入点，
        // 菜单 Window › Panes 是回程。
        // 2026-11-07：菜单栏右侧布局按钮的勾选项（整带开关）。
        // 读者仍是 `crate::shell::panel_visible`——`ToggleDocks` 只是
        // 一次写一组 `show_*`，没有第二条可见性通路。
        // 2026-11-17 补底带：归属表 `PanelId::default_zone()` 里
        // Bottom 一直是 Sender / Shell / Script 三块，缺的只是菜单入口。
        // 2026-11-17：Tunnels 两条。读者分别是
        // `ui/effects.rs` 的 `Effect::ToggleTunnels` → `set_tunnels_enabled`
        // （真停/起连接）与 `Effect::OpenTunnelSettings` → `tunnel_editor`
        // （真对话框）。都有屏幕上的后果，不是空转。
        "Terminal.TunnelSettings",
        "Window.ToggleFocusMode",
        "Window.ToggleFullScreen",
        // 2026-11-25：布局下拉的整带开关三条 + Tunnels 下拉的总线开关。
        // 读者分别是 `ui/layout/dock_paint.rs::apply_layout_action`
        // （整带收回/展开）与 `ui/effects.rs::set_tunnels_enabled`（真停/起转发）。
        // 此前它们没进过 inventory，所以虽然一直能点，能力台账少算四条。
        "Window.ToggleLeftDocks",
        "Window.ToggleRightDocks",
        "Window.ToggleBottomDocks",
        "Window.ToggleTunnels",
        "Window.ToggleAlwaysOnTop",
        // 读者是 `ui/filer.rs` 的面板分支——开关一开就画，
        // 并在第一次显示时为当前标签开一条 SFTP 会话。
        "Window.TogglePaneFiler",
        // 7 条日志命令。读者是 `ui/logger.rs`——开始/停止/暂停/恢复
        // 直接落在 `SupervisorHandle` 上，打开文件/目录走系统默认程序。
        "Terminal.StartLogger",
        "Terminal.StopLogger",
        "Terminal.PauseLogger",
        "Terminal.ResumeLogger",
        "Terminal.OpenLogFile",
        "Terminal.OpenLogFolder",
        "Terminal.OpenLogSettings",
        // 12 条 A 类便宜货。`ui/mod.rs` 的 `apply_effect` 装配分派，
        // 落地在 `ui/effects.rs`——切标签改 `active_tab`、复制取 `tab.config`、
        // 帮助 / 在线搜索用 `sysopen`、字号调 `fonts.zoom`；从选区查找把
        // `SetQuery` + `Next/Previous` 派发回 reducer（投影层只给派生值，
        // 不写 `FindState`）；粘贴选区经 `paste_bytes_for` 写给 PTY。
        "Window.ActivateNextView",
        "Window.ActivatePreviousView",
        "Terminal.CopySessionName",
        "Terminal.CopySessionUrl",
        "Help.Help",
        // `Text.SearchOnline` / `Terminal.SchemeList` / 两条 Toolbar 命令
        // 2026-11-26 从菜单里退出：原版这三处是运行期展开的引擎 / scheme /
        // 空工具栏列表，本版对应入口尚未接线，按 ADR-0015 置灰，
        // 因此不再属于「已接线且可见」集合。
        "Text.ZoomIn",
        "Text.ZoomOut",
        "Text.ResetZoom",
        "Text.FindNextSelection",
        "Text.FindPreviousSelection",
        // 粘贴选区，与 `Text.Paste` 同一读者（`Effect::PasteSelection`）。
        "Terminal.PasteSelectedText",
        // 2 条命令面板命令。读者是 `ui/palette.rs`：面板只在
        // `ViewState.command_palette.visible` 时画，条目取自菜单树，确认后
        // 由顶层 `reduce` 把选中命令递归派发回同一条纯函数路径。
        "Window.ShowPaletteCommand",
        "Window.ShowPaletteAnything",
        // 3 条 E 类。读者是 `ui/effects.rs`——Exit 发
        // `ViewportCommand::Close`；Close / CloseAllViews 经 `ui/tabs.rs`
        // 的 `close_tab` / `close_all_tabs`，与标签栏 × 同一条路径。
        "Application.Exit",
        "Window.Close",
        "Window.CloseAllViews",
        // Goto 三条。读者是 `ui/goto.rs` 与 `ui/effects.rs`——
        // 行号经 `Viewport::jump_to_row` 落到活动标签的视口，会话索引
        // 落到 `active_tab`；符号模式把 OSC 133 命令块的头行当符号跳转。
        // 解析与失败提示都在 `term-app/goto.rs`。
        "Window.GotoLine",
        "Window.GotoSession",
        "Window.GotoSymbol",
        // 分屏六条。读者是 `ui/layout.rs` 的布局树与递归渲染——
        // SplitView 新建本地 shell、SplitGroup 复制当前会话配置，
        // 方向决定左右 / 上下；CloseActiveGroup 收起当前窗格，
        // MoveGroupToGroup 把当前会话并入下一个窗格。
        "Window.SplitViewHorizontally",
        "Window.SplitViewVertically",
        "Window.SplitGroupHorizontally",
        "Window.SplitGroupVertically",
        "Window.CloseActiveGroup",
        "Window.MoveGroupToGroup",
        // 六个工具面板。读者是 `ui/panes.rs`——每个开关开着就画一个
        // 有真实内容的面板（会话树 / 当前屏行摘要 / 预设命令 / 会话列表 /
        // 会话状态 / 在跑的隧道），不是空转 bool。
        "Window.TogglePaneExplorer",
        "Window.TogglePaneOutline",
        "Window.TogglePaneSender",
        "Window.TogglePaneSession",
        "Window.TogglePaneShell",
        "Window.TogglePaneScript",
        "Window.TogglePaneTransfer",
        // v0.1 工作台 15 条。读者是 `ui/center.rs` 的对应页面——
        // 每条命令打开一个有真实内容/动作的页面，不是同义开关；
        // `Text.HighlightText` 的读者还包括 `ui/terminal.rs` 的常驻高亮渲染。
        "Terminal.OpenSession",
        "Terminal.Save",
        "Terminal.SaveSelectionAs",
        "Terminal.OpenSessionSettings",
        "Terminal.OpenDefaultSessionSettings",
        "Application.OpenProfileDialog",
        // 这两条此前是同一条命令。现在 `GotoAddressBar` 是地址栏
        // （连接目标输入框：解析 / 候选 / 最近使用在 `term_app::address`，
        // 浮层与 `dispatch_action` 在投影层），`GotoCommandLine` 是那个
        // 「把一行当 shell 命令发出去」的工作台页。两条都算已接线：
        // 各有各的读者，不是同一个效果的别名。
        "Window.GotoAddressBar",
        "Window.GotoCommandLine",
        "Terminal.CompleteHistory",
        "Window.WindowList",
        "Window.WindowManager",
        "Window.OpenFocusModeSettings",
        "Text.HighlightText",
        // 锁屏。读者是 `ui/lock.rs` 的全屏遮罩 + `term-vault` 的主密码校验。
        "Window.LockScreen",
        // 重置布局。读者是 `ui/layout/dock_paint.rs::apply_layout_action`
        // ——把拖过 / 浮过的面板全部收回默认停靠位（`DockLayout::reset`）。
        "Window.ResetLayout",
        // 三条。读者分别是——同步输入：`ui/terminal.rs` 的按键扇出
        // 与状态栏 SYNC 徽标；选中命令：`ui/effects.rs` 按内核 OSC 133
        // 的行号取 `B..C` 那段；隧道管理：`ui/tunnel_manager.rs` 的列表 +
        // 增删规则（规则语法仍只有 `tunnels::parse_rule` 一处）。
        "Terminal.SyncInput",
        "Text.SelectCommand",
        "Terminal.ManageTunnels",
        // 2026-11-13：设置页与「最近使用」的两条直达。
        //
        // `ManageTriggers` 一分为二：`show_settings` 由 reducer 置位（与
        // `Window.OpenSettings` 同一句），落到屏幕上是
        // `ui/effects.rs::Effect::FocusTriggersPage` → 把
        // `TerminalWindow::settings_page` 翻到 `SettingsPage::Triggers`，
        // 那一页的真实现是 `ui/dialogs/settings_live_triggers.rs`。
        // `ClearRecentUrls` 的读者是 `ui/address/mod.rs::clear_recent_targets`
        // ——清空 `address.recent` 并 load-modify-save 写回
        // `Prefs.recent_targets`，地址栏右键「Clear Recent」走的是同一个函数。
        "Terminal.ManageTriggers",
        // `ManageOneKeys` 与 `ManageTriggers` 同形：reducer 置 `show_settings`
        // + `Effect::FocusOneKeysPage` → 设置页翻到 `SettingsPage::OneKeys`，
        // 真实现是 `ui/dialogs/settings_live_onekeys.rs`。
        "Terminal.ManageOneKeys",
        "Terminal.ClearRecentUrls",
        "Text.Find",
        "Text.FindNext",
        "Text.FindPrevious",
        "Text.Copy",
        "Text.Paste",
        // 17 条选择命令。读者是 `ui/selection` 的落点（`Effect::Select`
        // → `selection::apply`），鼠标拖动、Shift+方向键、菜单点选共用它。
        "Text.SelectAll",
        "Text.SelectWord",
        "Text.SelectLine",
        "Text.SelectToDocumentStart",
        "Text.SelectToDocumentEnd",
        "Text.SelectToLineHome",
        "Text.SelectToLineEnd",
        "Text.SelectToPreviousChar",
        "Text.SelectToNextChar",
        "Text.SelectToPreviousLine",
        "Text.SelectToNextLine",
        "Text.SelectToPreviousPage",
        "Text.SelectToNextPage",
        "Text.SelectToPreviousWordStart",
        "Text.SelectToNextWordStart",
        "Text.SelectToPreviousWordEnd",
        "Text.SelectToNextWordEnd",
        // 2026-11-13：26 条光标 / 选区命令。读者与上面 17 条**同一个**
        // （`Effect::Select(Cmd::Move)` → `selection::apply_with_blocks`），
        // 但这次不是纯映射——26 条各有自己的位移规则，边界判定在
        // `selection/geometry.rs` / `selection/words.rs`，逐条单测在
        // `selection.rs`（`every_cursor_command_parses_to_its_own_op` 钉 ID，
        // 其后每族一条落点断言）。`Command` / `Scope` 两条需要 OSC 133 块表，
        // 空表时前者不动、后者退化到窗口首末行。
        "Text.SelectColumnToPreviousChar",
        "Text.SelectColumnToNextChar",
        "Text.SelectColumnToPreviousLine",
        "Text.SelectColumnToNextLine",
        "Text.SelectColumnToPreviousPage",
        "Text.SelectColumnToNextPage",
        "Text.SelectColumnToPreviousParagraph",
        "Text.SelectColumnToNextParagraph",
        "Text.SelectColumnToLineHome",
        "Text.SelectColumnToLineEnd",
        "Text.GotoPreviousWordPart",
        "Text.GotoNextWordPart",
        "Text.GotoPreviousStutteredPage",
        "Text.GotoNextStutteredPage",
        "Text.GotoPreviousParagraph",
        "Text.GotoNextParagraph",
        "Text.GotoPreviousCommand",
        "Text.GotoNextCommand",
        "Text.GotoPreviousScope",
        "Text.GotoNextScope",
        "Text.GotoPreviousIndentation",
        "Text.GotoNextIndentation",
        "Text.GotoPairStart",
        "Text.GotoPairEnd",
        "Text.GotoPreviousPair",
        "Text.GotoNextPair",
        // 9 条集合命令。读者是 `ui/terminal/pane.rs` 的落点——它们改的是
        // 标签上的 `Selections`（活动下标 / 成员 / 锁 / 形状 / 端点），
        // 纯函数在 `term-app/selection.rs::apply_cmd`；非活动选区在
        // `ui/terminal/view.rs` 里淡一档画出来，所以「切到哪一份」看得见。
        "Text.ActivateNextSelection",
        "Text.ActivatePreviousSelection",
        "Text.LockSelection",
        "Text.DropSelection",
        "Text.CancelSelection",
        "Text.SplitSelection",
        "Text.InvertSelection",
        "Text.SwitchStreamAndColumn",
        "Text.ExchangeCaretAndAnchor",
        // Goto 的 4 条光标移动。读者与 17 条选择命令同一条路径
        // （`Effect::Select(Cmd::Move)` → `selection::apply`），
        // 语义是「只挪活动选区的 head」，不是另开一套光标。
        "Text.CharLeft",
        "Text.CharRight",
        "Text.LineUp",
        "Text.LineDown",
        // Goto 另外 10 条。读者与上面四条**同一个**：`parse` 只把它们映射到
        // 既有的 `Op`（词首 / 词尾 / 翻页 / 行首行尾 / 文首文末），落地仍是
        // `selection::apply` → `Effect::Select(Cmd::Move)`。没有新增位移规则，
        // 也没有第二套光标——`selection.rs::goto_commands_land_where_the_menu_says`
        // 逐条验落点（行尾列号 / 词首列号 / 翻页夹取 / 文末行号）。
        "Text.GotoPreviousWord",
        "Text.GotoPreviousWordEnd",
        "Text.GotoNextWord",
        "Text.GotoNextWordEnd",
        "Text.GotoPreviousPage",
        "Text.GotoNextPage",
        "Text.GotoLineHome",
        "Text.GotoLineEnd",
        "Text.GotoDocumentStart",
        "Text.GotoDocumentEnd",
        // 2026-11-09：Transfer Binary 的 5 条。读者分两段——口令由
        // `apps/desktop/src/xfer_run.rs` 的驱动线程消费（它才持有会话通道），
        // 进度与结果由 `apps/desktop/src/ui/terminal/xfer.rs::draw` 画成终端底部
        // 的一行状态（在跑 / 跑完没值 / 失败 / 空闲四种说法互不混淆）。
        "Terminal.SendXModem",
        "Terminal.ReceiveXModem",
        "Terminal.SendYModem",
        "Terminal.ReceiveYModem",
        "Terminal.CancelModem",
        // 同日：ZMODEM 两条，读者是同一条驱动线程的另一台状态机
        // （`term-zmodem` 的 `Sender` / `Receiver`，经 `xfer_run.rs` 的
        // `ZTxEngine` / `ZRxEngine` 两个壳接进同一套 `Engine` 循环）。
        // 「点了有效果」的证据在 `apps/desktop/tests/xfer_zmodem.rs`：
        // 真 PTY 里起真 `sz`/`rz`，落盘内容与源文件逐字节相同。
        "Terminal.SendZModem",
        "Terminal.ReceiveZModem",
        // 标签右键 10 个影子入口。读者是
        // `ui/effects.rs::apply_tab_action`——它把意图落到
        // `close_tab_with_history` / `reopen_closed` / `duplicate_tab` /
        // 分屏 / 复制 / 上色这些**既有**方法上，不是空转开关。
        // `ui/shell/tab_menu.rs` 不再直接改 `TerminalWindow`，
        // 只发 `Action::Tab(...)`，所以「真实入口」与这张表同源。
        "Terminal.TabActivate",
        "Terminal.TabClose",
        "Terminal.TabCloseOthers",
        "Terminal.TabCloseAll",
        "Terminal.TabReopenClosed",
        "Terminal.TabDuplicate",
        "Terminal.TabSplit",
        "Terminal.TabCopyName",
        "Terminal.TabCopyUrl",
        "Terminal.TabColor",
        // 终端内容右键四条。读者是 `apps/desktop/src/ui/effects.rs` 的
        // `apply_tab_action`（copy_command / tab.clear* 三个既有方法），
        // 右键清单经 `dispatch(id)` 走与菜单完全相同的 Action 通道。
        "Terminal.CopyCommand",
        "Terminal.ClearScreen",
        "Terminal.ClearScrollback",
        "Terminal.ClearAll",
        // tmux 浮层开关。此前它只有 parse 臂、没有入口表，是能力台账查
        // 不到的孤儿命令；现在和其余命令一样在 inventory 里，读者仍是
        // `ui/panels/tmux.rs`。
        "Window.TogglePaneTmux",
        // tmux 控制模式浮层。读者是
        // `ui/panels/tmux.rs::draw_tmux_panel`（`ui/mod.rs` 每帧调用），
        // 开关由 `ShellAction::TogglePaneTmux` 翻 `ViewState.show_tmux`。
        // 2026-11-10：显示模式切换。读者是投影层的 `Tab.mode`——本层只吐
        // `Effect::ToggleHex`，真正的落地（翻 `Tab.mode`、不落盘）在
        // `apps/desktop`。归 `FeatureId::Bin`：它换的是「怎么看字节」，
        // 不是窗口 chrome。
        "Text.ToggleHexEdit",
        // 2026-11-24：软换行。读者是投影层 `Tab.word_wrap`——段表每帧
        // 按这一位现算（`wrap::segments_for`），点了真的多出可视段。
        "Text.ToggleWordWrap",
        // 2026-11-14：调试覆盖层。读者是 `apps/desktop/src/ui/terminal/
        // pane_overlays.rs` 的 `if !bin && tab.debug` —— 它去调
        // `ui/terminal/debug_paint.rs::paint`，点了屏幕上的像素**真的变**：
        // 每格的 `Attrs` 八位变成 pip 架与两条贯穿线、`fg`/`bg` 的
        // `Indexed`/`Rgb` 变成 1 / 3 个小方块、`Cell.wide` 变成右缘竖线。
        // 归 `FeatureId::Shell`（走 `Action::Shell(_)` 的兜底归属）：
        // 命令 ID 本身就是 `Window.*` 前缀，与 `Window.ToggleFocusMode` 一族。
        "Window.ToggleDebugMode",
        // 2026-11-19：只读查看（`Window ▸ Local Mode`）。**四位读者**全在
        // 投影层，每一位都是一条独立判据：`Tab::write` / `write_paste`
        // （不写 PTY）、`Tab::poll`（钉住视口 + 跳过触发器求值）、
        // `ui/terminal/pane.rs`（不发鼠标上报 / 不弹补全 / 不落粘贴）、
        // `ui/terminal/view.rs::paint_cursor`（不画光标）。归
        // `FeatureId::Shell`（`Window.*` 前缀 + `Action::Shell(_)` 兜底）。
        "Window.SetLocalMode",
        "Window.SetRemoteMode",
        // 2026-11-15：`Application.NewWindow`。读者是
        // `apps/desktop/src/new_window.rs` 的 `SpawnPlan::launch`——它拉起一个
        // 完整的**新进程**（窗口态独立、不恢复布局），「效果」落在窗口
        // 系统而不是像素上，与 `Effect::Exit` 同一类。归 `FeatureId::Shell`，
        // 走 `Action::Shell(_)` 的兜底归属，不新增 `FeatureId`。
        "Application.NewWindow",
        // 2026-11-11：hex 三个参数子菜单的 13 条档位命令。读者全是
        // `ui/effects.rs::set_tab_hex`——它只改**活动标签**的 `Tab.hex`
        // 一个旋钮、整份过一遍 `HexParams::from_prefs` 夹一次（不落盘；
        // 落盘的那一层是设置 ▸ Binary 页的 `Prefs.hex_*`）；
        // `bin.rs`（绘制）与 hex 视图表头读同一份值。设置页 Binary 页那四行
        // 是**另一层**（默认参数），不读 `Tab.hex`。
        // 归 `FeatureId::Bin` 的理由与上面那条相同：换的是怎么看字节。
        "View.HexBase2",
        "View.HexBase7",
        "View.HexBase8",
        "View.HexBase10",
        "View.HexBase16",
        "View.HexColumns8",
        "View.HexColumns16",
        "View.HexColumns24",
        "View.HexColumns32",
        "View.HexGroupChars1",
        "View.HexGroupChars2",
        "View.HexGroupChars4",
        "View.HexGroupChars8",
        // 2026-11-13：栅格可视化 10 条。读者是投影层的 `effect::SetMarks` →
        // `ui/effects.rs::set_marks_knob` → `TerminalWindow::set_marks_knob`
        // （四个翻转 + 五档设档，改**活动标签**的 `Tab.marks`，不落盘）。
        // 「点了屏幕上哪一像素会变」逐个写在这里，不是「翻了个没人看的 bool」：
        //   · LineEndings → 逻辑行末尾多一个 `$` 字形
        //   · ColumnMarker → 第 N 列一条贯穿竖线
        //   · IndentGuides → 行首每 4 列一条 1px 竖线
        //   · WrapSymbols → 自动换行续行右下角一个 L 形勾
        //   · Whitespace(*) → 空格小方点 / Tab 箭头（五档范围不同）
        //   · CaretLine → 光标行一层 7% 淡底（压在字形之下）
        // 归 `FeatureId::Marks`：既不换「怎么看字节」，也不动窗口 chrome。
        "View.ToggleLineEndings",
        "View.ToggleColumnMarker",
        "View.ToggleIndentGuides",
        "View.ToggleWrapSymbols",
        "View.WhiteSpacesHideAll",
        "View.WhiteSpacesShowAll",
        "View.WhiteSpacesShowInSelection",
        "View.WhiteSpacesShowInLineStart",
        "View.WhiteSpacesShowInLineEnd",
        "Text.HighlightCaretLine",
        // 2026-11-13：窗口透明度四条。读者分两处——
        //   · 三档预设 `SetWindow{Opaque,Translucent,Transparent}`：
        //     `ui/effects.rs` 落地到 `TerminalWindow::set_window_opacity`
        //     （改运行期副本 + `save_prefs` 写回 `Prefs.window_transparent`
        //     / `window_opacity`）；`main.rs::update` 每帧按钳制后的值把
        //     alpha 装到窗口表面色与终端区底色上，并在开关
        //     变化时只发一次 `ViewportCommand::Transparent`。
        //   · 第四条 `SetWindowOpacity`：`show_settings` 由 reducer 置位，
        //     `Effect::FocusAppearancePage` 把设置页翻到 `Appearance`，
        //     那一页的开关 + 滑杆是真控件（`ui/dialogs/settings_live.rs`）。
        // 归 `FeatureId::Shell`：它改的是整窗 chrome，不是栅格上的像素。
        // 语义假设（可在 GUI 验收后调整）见 `vocab.rs::WindowOpacityPreset`。
        "Application.SetWindowOpaque",
        "Application.SetWindowTranslucent",
        "Application.SetWindowTransparent",
        "Application.SetWindowOpacity",
        // 2026-11-14：折叠族九条。读者全部是 `ui/effects.rs::apply_fold`——
        // 它把 `FoldCmd` 交给纯函数 `decide_fold`，再把结果写回
        // `Tab.folds` / `Tab.viewport` / `last_request`。
        // 「点了屏幕上哪一行会变」逐条写在这里（渲染在
        // `terminal/setup.rs::folded_view`，它在 `folds` 非空时把被折的行丢出窗口）：
        //   · Fold / `MarginFold`(Toggle) → 光标所在的块收起，屏幕上少掉
        //     `b - a - 1` 行输出，折叠头（那条命令行）留着
        //   · Unfold → 那一块摊开，行数长回来
        //   · FoldInside / UnfoldInside → 当前视口内所有块一起折 / 解
        //     （本仓的块平坦不嵌套，见 `vocab::FoldCmd::FoldInside` 的取舍）
        //   · FoldAll / UnfoldAll → 全部块 / 整个集合
        //   · GotoPreviousFold / GotoNextFold → 视口跳到上一个 / 下一个折叠头
        // 归 `FeatureId::Margin`：折叠就是 margin 折叠标记列的内容。
        "View.Fold",
        "View.Unfold",
        "View.FoldInside",
        "View.UnfoldInside",
        "View.FoldAll",
        "View.UnfoldAll",
        "Text.MarginFold",
        "Text.GotoPreviousFold",
        "Text.GotoNextFold",
        // 2026-11-27 M1：34 条从 Planned 转 Implemented，每条的「点了哪里会变」
        // 逐组写在这里（总数仍是「菜单里点得到」的一张表）：
        //   · Search Online 6：`Effect::SearchOnlineWith` → `ui/effects/search.rs`
        //     取选区 / 输入缓冲，按引擎表拼 URL，走 `open_with::open_path`
        //     拉起系统默认浏览器——窗口外可见的后果。
        //   · Syntax Schemes 7：`Effect::SetScheme` → `ui/effects/theme.rs` 用
        //     `builtin_scheme(name)` 换运行中调色板并 load-modify-save 写回
        //     `Prefs.scheme_name`；下一次绘制的终端 16 色当场变化，重启仍在。
        //   · Select 5：`selection::units::select_unit` / `fold_unit` 取 head
        //     所在语义单位（折叠块用输入行表算），写回活动选区——屏幕上那段
        //     文字的底色真的变了；与 Shift+方向键同一条纯函数路径。
        //   · Select To 16：`Effect::Select(Cmd::Move)` → `selection::apply_*`
        //     （Fold 两条经带上下文的入口），只挪 head；14 条复用 Goto 已经
        //     验过的边界 Op，不存在第二套位移规则。
        "Text.SearchOnlineGoogle",
        "Text.SearchOnlineBing",
        "Text.SearchOnlineGithub",
        "Text.SearchOnlineStackoverflow",
        "Text.SearchOnlineWikipedia",
        "Text.SearchOnlineDuckDuckGo",
        "Terminal.SchemeCmd",
        "Terminal.SchemeDumb",
        "Terminal.SchemeLinux",
        "Terminal.SchemeMacos",
        "Terminal.SchemeNone",
        "Terminal.SchemePowershell",
        "Terminal.SchemeRouteros",
        "Text.SelectParagraph",
        "Text.SelectPair",
        "Text.SelectFold",
        "Text.SelectScope",
        "Text.SelectIndentation",
        "Text.SelectToPreviousWordPart",
        "Text.SelectToNextWordPart",
        "Text.SelectToPreviousStutteredPage",
        "Text.SelectToNextStutteredPage",
        "Text.SelectToPreviousPair",
        "Text.SelectToNextPair",
        "Text.SelectToPreviousParagraph",
        "Text.SelectToNextParagraph",
        "Text.SelectToPreviousFold",
        "Text.SelectToNextFold",
        "Text.SelectToPreviousScope",
        "Text.SelectToNextScope",
        "Text.SelectToPreviousIndentation",
        "Text.SelectToNextIndentation",
        "Text.SelectToPairStart",
        "Text.SelectToPairEnd",
    ]
    .into_iter()
    .collect();
    let actual: BTreeSet<&str> = inventory()
        .iter()
        .filter(|c| c.is_implemented())
        .map(|c| c.id)
        .collect();
    assert_eq!(actual, expected, "已接线的命令集合变化需显式确认");
}

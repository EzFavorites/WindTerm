//! 副作用词表：`reduce()` 不能碰 PTY / 窗口 / 文件系统，所以副作用以值返回。
//!
//! 这些值由投影层（`apps/desktop/src/ui/effects.rs`）消费，是**纯层与执行层之间
//! 唯一的通道**。放在独立文件而不是 `action.rs` 里的理由是可判定的：
//! `Effect` 要包装各能力的动作类型（`Effect::Layout(LayoutAction)`…），
//! 而各能力的 reducer 又要返回 `Effect`。两者同处一个模块，就必然出现
//! `clipboard → action → clipboard` 这样的双向边——2026-11-12 实测这正是
//! 13 个模块强连通分量的一半来源（7 条回边里有 6 条是 `use crate::action::Effect`）。
//!
//! 依赖方向定死为 `effect → vocab`，能力模块只许向它**单向**取类型。

use crate::vocab::{
    Cmd, FoldCmd, HexKnob, LayoutAction, LogAction, MarksKnob, TabAction, XferCmd,
};

/// `reduce()` 不能碰 PTY / 窗口 / 文件系统，所以副作用以值返回。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Effect {
    Disconnect,
    Reconnect,
    OpenNewSession,
    OpenSettings,
    ShowAbout,
    /// 把当前选区文本写入系统剪贴板（文本由投影从事实窗口提取）。
    CopySelection,
    /// 在选区头部执行一次移动/选中。
    ///
    /// 这里只传意图，不传结果：位置取决于「此刻画在屏上的事实窗口」，
    /// 而 reducer 拿到的 `window` 只用于查找。投影层拿 `Op` 调
    /// `selection::apply(op, &tab.view, tab.selection)` 得到新选区——
    /// 菜单点「下一行」与 Shift+↓ 走的是同一条纯函数。
    ///
    /// `Move` 与 `Set`（集合命令）共用这一个变体——投影层的待办只有一个
    /// 槽位，两种命令不会互相顶掉，也不会各走一条会漂的路径。
    Select(Cmd),
    /// 读系统剪贴板并粘贴进终端。
    ///
    /// reducer 只声明意图，**不携带内容**：本层是纯函数，拿不到也没必要拿剪贴板；
    /// 文本由投影向框架索取（`ViewportCommand::RequestPaste`），再交给
    /// `term_core::vt::paste_bytes_for` 按终端模式编码。
    Paste,
    /// 会话日志的开始 / 停止 / 暂停 / 恢复 / 打开文件 / 打开目录。
    ///
    /// 不携带路径：文件名的最终形态（会话名 + 时间戳 + 重名序号）由写入端
    /// 决定——只有它知道同一秒里是不是已经有同名文件。界面从
    /// `supervisor::LogStatus` 拿回报的路径。
    Log(LogAction),
    /// 循环切到相对位置的标签（+1 / −1，到头回绕）。
    ///
    /// 真正改 `active_tab` 的是投影层——它才有标签列表；这里只带位移意图。
    ActivateTab(i32),
    /// 把活动会话的名字写入系统剪贴板。
    CopySessionName,
    /// 把活动会话的连接串（`ssh://user@host:port`）写入系统剪贴板。
    CopySessionUrl,
    /// 打开帮助（静态 URL，由投影层用系统默认程序拉起）。
    OpenHelp,
    /// 用当前选区文本发起一次网络搜索。查询词由投影层取并 URL 编码。
    SearchOnline,
    /// 终端字号缩放。+1 放大 / −1 缩小 / 0 复位；目标字号由投影层算。
    Zoom(i8),
    /// 以当前选区文本为查询词，跳到下一个（+1）/ 上一个（−1）匹配。
    ///
    /// 查询词来自每帧推导的 `selection_text`，reducer 没有选区，所以这一条
    /// 必须落到投影层——`Action::Find(SelectionNext/Prev)` 在此被翻译成它。
    FindSelection(i8),
    /// 把当前选区文本写给 PTY（等同于「复制选区 + 粘贴」一步完成）。
    PasteSelection,
    /// 再起一个**独立应用实例**（新进程）。
    ///
    /// 不携带目标状态、不碰进程：拉起进程是投影层 OS 集成的事
    /// （`apps/desktop/src/new_window.rs` 的 `SpawnPlan::launch`），纯层只吐意图。
    /// 新窗口不恢复布局（`ui/construct.rs::init` 按角色变量跳过），所以出厂就是
    /// 「一个干净的本地 shell 标签」；`prefs.json` / SQLite 会话库仍与应用级共享。
    NewWindow,
    /// 请求窗口系统关闭本窗口（应用级退出）。
    Exit,
    /// 关掉当前标签页（标签列表由投影层持有）。
    CloseActiveTab,
    /// 关掉全部标签页（关空后按既有约定补一个本地 shell）。
    CloseAllTabs,
    /// 把某个回看绝对行号放到视口顶部（数值由 reducer 从状态里解析）。
    GotoLine(i64),
    /// 切到第几个已打开的标签（索引由投影层落地时钳制）。
    GotoSession(usize),
    /// 左右（true）/ 上下（false）二分当前窗格，新窗格开一条本地 shell。
    SplitView(bool),
    /// 同上，但新窗格复制当前会话配置（同协议 / 主机，名字加序号）。
    SplitGroup(bool),
    /// 关掉当前窗格，兄弟顶上来。
    CloseActiveGroup,
    /// 把当前会话并入下一个窗格，当前窗格收起。
    MoveGroupToGroup,
    /// 导入外部会话（字符串是来源名，由投影层读文件后交给纯解析器）。
    ImportSessions(&'static str),
    /// 把当前标签 / 分屏结构写进布局库（退出或结构变化时）。
    SaveLayout,
    /// 打开快捷键绑定表。
    OpenShortcuts,
    /// 把一条 tmux 反向控制命令写给当前 PTY。
    TmuxCommand(String),
    /// 喂一段控制模式输出给解析器（面板调试 / 降级路径）。
    TmuxFeed(String),
    /// 选中最近一条命令（`B..C` 那段）。块边界来自内核的 OSC 133 记录，
    /// 选区要事实窗口的行，所以只在投影层落地。
    SelectCommand,
    /// 停靠 / 浮动。布局树活在投影层，这一条只是把意图送到它手里。
    Layout(LayoutAction),
    /// 把命令文本写进活动会话的输入行。文本由 reducer 从 `Action` 带出，
    /// 但**何时写、写不写得进去**由投影层决定（它才持有 PTY 句柄）。
    /// 与 `PasteSelection` 同一条落地路径，且不追加换行。
    SendCommand(String),
    /// 2026-11-09：文件传输（发 / 收 / 取消）。
    ///
    /// 读者是 `apps/desktop/src/ui/effects.rs` 的 `Effect::Xfer` 两个 arm
    /// （`XferCmd::Cancel` 特例 + 通用分支）。原文写「这一变体目前没有消费者」
    /// 是 2026-11-09 的状态，2026-11-13 架构审查按实测订正。
    /// 驱动仍在 `apps/desktop/src/xfer_run.rs`，进度状态行在
    /// `apps/desktop/src/ui/terminal/xfer.rs`，两边的连线由投影层主线程做一处胶水
    /// （它才拿得到 PTY 与会话句柄）。reducer 只把口令原样抛上去——协议选择与
    /// 文件选择都不在这一层。
    Xfer(XferCmd),
    /// 标签右键的一个动作。读者是 `ui/effects.rs::apply_tab_action`——
    /// 它把意图落到 `close_tab_with_history` / `reopen_closed` / 分屏等
    /// **既有**方法上，不新开第二条路径（点了有效果）。
    Tab(TabAction),
    /// 显示模式：文本网格 ↔ 十六进制（`Mode ▸ Hex Edit`）。
    ///
    /// **不携带目标模式**：reducer 不知道当前是哪一种——模式是**每条会话**的
    /// 属性，活在投影层的 `Tab.mode`，而 reducer 手上只有全局 `ViewState`。
    /// 所以这里表达的是「切一下」。落地只有一件事：投影层翻 `Tab.mode`。
    /// **不落盘**：模式不保存，重开标签一律回到文本。
    ToggleHex,
    /// 调试覆盖层：`Mode ▸ Debug Mode`，把 `Cell` 的属性位 / 色号种类 / 宽字符
    /// 标成像素。
    ///
    /// **不携带目标状态**，理由与 [`Effect::ToggleHex`] 一字不差：开关是**每条
    /// 会话**的属性，活在投影层的 `Tab.debug`，而 reducer 手上只有全局
    /// `ViewState`。所以这里表达的是「切一下」，落地只有一件事：投影层翻
    /// `Tab.debug`。**不落盘**：调试覆盖是临时状态，不该有「上次那个状态」。
    ///
    /// 读者是 `apps/desktop/src/ui/effects.rs::toggle_debug_mode`；像素在
    /// `ui/terminal/debug_paint.rs`，调用点是 `ui/terminal/pane_overlays.rs`
    /// 的第三与第四段之间（`if !bin && tab.debug`）。
    ToggleDebug,
    /// 只读查看：`Window ▸ Local Mode`（2026-11-19）。
    ///
    /// **不携带目标状态**，理由与 [`Effect::ToggleDebug`] 一字不差：开关是**每条
    /// 会话**的属性，活在投影层的 `Tab.local_mode`，而 reducer 手上只有全局
    /// `ViewState`。落地是 `apps/desktop/src/ui/effects.rs::toggle_local_mode`：
    /// 翻一位，并在打开时取消本窗口正在跑的脚本。**不落盘**。
    ///
    /// 四位读者都在投影层：`Tab::write` / `write_paste`（不发字节）、
    /// `Tab::poll`（钉住视口 + 跳过触发器）、`ui/terminal/pane.rs`（不发鼠标上报、
    /// 不弹补全、不落粘贴）、`ui/terminal/view.rs::paint_cursor`（不画光标）。
    ToggleLocalMode,
    /// 2026-11-17：隧道总线开关切一下（菜单栏 `Tunnels` 下拉）。
    ///
    /// 落地是 `TerminalWindow::set_tunnels_enabled`：关 → 停掉全部在跑的隧道，
    /// 配置不动；开 → 按各条的 `Status` 重起。同时写回 `Prefs.tunnels_enabled`
    /// ——关机前关掉的人，下次开机不该被自动起的隧道吓一跳。
    ToggleTunnels,
    /// 2026-11-17：打开 Tunnels 设置对话框（草稿全在投影层，reducer 不参与）。
    OpenTunnelSettings,
    /// hex 渲染器的三个旋钮（`View ▸ Hex Base / Hex Columns / Hex Group Chars`）。
    ///
    /// 与 [`Effect::ToggleHex`] 同一类：reducer 只转述「点了哪一档」，不碰生效值。
    /// 那份值活在投影层（`Tab.hex`），**不落盘**；合法区间由
    /// `HexParams::from_prefs` 一处夹掉。落盘的是另一层：设置 ▸ Binary 页写的
    /// `Prefs.hex_*`（默认参数，只对新会话生效）。
    /// 也是渲染侧的概念，所以「这个数合不合法」由投影层的 `HexParams::from_prefs`
    /// 一处夹掉，纯层不认识进制。
    SetHex(HexKnob),
    /// 栅格可视化的一条开关（`View ▸ Symbols` / `View ▸ White Spaces` /
    /// `Text ▸ Highlight Caret Line`）。
    ///
    /// 与 [`Effect::SetHex`] 同一类：reducer 只转述「点了哪一个旋钮」，不碰生效值。
    /// 生效值活在投影层（`Tab.marks`），**不落盘**；四个 bool 是翻转、五档空白
    /// 模式是设档，分流在 `ui/effects.rs::set_marks_knob` 一处。
    /// 落在屏幕上的东西在 `ui/terminal/marks_paint.rs`（`$` / 参考线 / 折行勾 /
    /// 空格点 / Tab 箭头）与 `ui/terminal/view.rs`（光标行底色，它压在字形之下）。
    SetMarks(MarksKnob),
    /// 打开「默认会话设置」对话框（`Session ▸ Preferences ▸ Default Session
    /// Settings…`）。读者是 `apps/desktop/src/ui/effects.rs::apply_effect`——它调
    /// `TerminalWindow::open_default_session_settings()`，把默认模板填进那个
    /// 与 New/Edit 共用的会话弹窗，保存时只写回默认模板、不连接。
    OpenDefaultSessionSettings,
    /// 打开「当前会话的属性」对话框（`Session ▸ Preferences ▸ Session
    /// Settings…`）。读者是 `apps/desktop/src/ui/effects.rs::apply_effect`——它调
    /// `TerminalWindow::open_session_properties(self.selected_session)`，把当前
    /// 选中会话填进与 New/Edit 共用的会话弹窗（标题 `Session Properties`）。
    /// 与侧边栏右键 `Properties` 走同一条落地路径，避免菜单与右键打开两个
    /// 不同的「配置会话」界面（此前菜单走的是中心面板，是另一个 UI 表面）。
    OpenActiveSessionSettings,
    /// `Terminal.ClearRecentUrls`：清空地址栏的「最近使用」并落盘。
    ///
    /// 读者是 `apps/desktop/src/ui/address/mod.rs::clear_recent_targets`——同一个
    /// 函数也是地址栏右键「Clear Recent」的落点，不新开第二条清理路径。
    ClearRecentUrls,
    /// `Application.SetWindow{Opaque,Translucent,Transparent}`：三档预设共用这一条
    /// **带参数**的效应，而不是三个几乎一样的变体。
    ///
    /// `opacity_permille` 是千分比（1000 = 1.0 / 600 = 0.6 / 300 = 0.3）。用整数
    /// 不是为了精确，而是因为 `Effect` 要 `Eq`（穷举 match 与能力台账断言靠它），
    /// `f32` 不实现 `Eq`；换算到 `f32` 在投影层落地时做一次。
    ///
    /// 读者是 `apps/desktop/src/ui/effects.rs` → `TerminalWindow::set_window_opacity`：
    /// 它同时改**运行期副本**和 `Prefs`（load-modify-save，重启仍生效）。视口命令
    /// 里没有数值 opacity 这一条，所以数值效果由 `ui/window_opacity.rs` 每帧按
    /// 钳制后的值装到窗口表面色的背景 alpha 上；这里只声明「要改成哪一档」，
    /// 不碰任何渲染框架的类型。
    ///
    /// 档位语义（可在 GUI 验收后调整）与 `window_transparent` 的对应关系写在
    /// `vocab.rs::WindowOpacityPreset` 的文档里，不在这里复制第二份。
    SetWindowOpacity {
        transparent: bool,
        opacity_permille: u32,
    },
    /// `Application.SetWindowOpacity`：设置对话框已由 reducer 置成打开状态，这一条
    /// 只负责**把左栏翻到 `SettingsPage::Appearance`**——那页的 `Application`
    /// 分组框里有「Window opacity」开关 + 滑杆（真控件，改完落盘）。
    ///
    /// 与 `Effect::OpenSettings` 的分工：那条的语义是「打开设置」，落点是默认页；
    /// 这条的语义是「带着一个落点打开」。纯层不认识 `SettingsPage` 的页面叫什么，
    /// 所以不能携带页码，只能是一条点名的效应。
    FocusAppearancePage,
    /// 折叠族九条（`View ▸ Folds` 六条 + `Text ▸ Margin Fold` +
    /// `Goto ▸ Fold` 两条）。
    ///
    /// # 为什么只带「折哪一类」而不带行号
    ///
    /// 决策要的三样输入**全在投影层**：输入行（`Tab::fold_inputs`，优先 OSC 133、
    /// 退回 `Tab::input_rows`）、光标行（快照的 `history + cursor`）、视口区间
    /// （`Tab::view.view_range()`）。纯层一样都拿不到，也不该拿——`ViewState` 里
    /// 放一份「当前光标行」就是第二份真相，而光标属于 PTY。
    ///
    /// 所以这一变体与 [`Effect::Select`] / [`Effect::Xfer`] 同款：只说「哪一类
    /// 折叠动作」，具体折哪几个块、跳到哪一行，由
    /// `apps/desktop/src/ui/effects.rs::apply_fold` 拿投影层的输入算。
    /// 算的那一段是**纯函数**（`effects::decide_fold`），所以「点了之后折没折对」
    /// 可以在没有窗口、没有会话的测试里逐条验。
    Fold(FoldCmd),
}

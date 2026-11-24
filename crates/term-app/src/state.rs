//! 视图状态集中化。`reduce()` 是唯一写入者。
//!
//! 这个类型的价值在于「少」：**每个字段都必须有一个读者**。删掉了 14 个照抄
//! 菜单配置、从未参与绘制的死状态位；又做了第二遍审计——把 19 个 bool 字段
//! 逐个 grep 一遍，发现 13 个 `ui/` 一次都没读过，即「菜单是亮的，点了没反应」。
//! 那一批现已全部处置：能实现真实效果的就实现，实现不了的回退 Planned + 置灰，
//! 对应的状态字段一并删除。
//!
//! 所以本文件的规矩是：**新增字段前先确认哪里会读它**。没有读者的字段不是「预留」，
//! 是第二步的假接口。

use crate::center::CenterState;
use crate::goto::GotoState;
use crate::palette::PaletteState;

// 2026-11-12：`LockState` / `MarginState` 的定义从 `lock.rs` / `margin.rs` 挪到这里。
// 原因是它们既是 `ViewState` 的字段，又被各自模块的 reducer 读（`&mut ViewState`）——
// 定义留在能力模块里就必然出现 `state ↔ lock` 这样的双向边。两个结构都是纯数据，
// 没有任何 `crate::` 依赖，压到聚合根里不损失内聚。原路径由 `pub use` 保住。

/// 锁屏状态。放在聚合根里，理由见上方注记。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct LockState {
    pub locked: bool,
    pub error: bool,
}

/// 终端左边栏五列的开关（留白 / 行号 / 符号 / 时间戳 / 折叠）。
/// margin 各列的开关。默认：留白 / 行号 / 符号 / 时间戳开，折叠关。
///
/// 2026-11-23 起时间戳默认开：用户反馈「默认不显示时间」，而宿主在
/// `term-io::supervisor` 里每批输出前都会 `stamp_row(now_secs())`，数据一直有，
/// 只是原先默认把这一列关着。它占 10 格（`[HH:MM:SS]` 正好 10 个字符），
/// 不想要的用户可以用 `Text.MarginTimestamp` 关掉。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MarginState {
    pub blank: bool,
    pub number: bool,
    pub fold: bool,
    pub symbol: bool,
    pub timestamp: bool,
}

/// Find 相关状态（查找栏的可见性与查询串）。
///
/// F2：这里**没有**匹配总数。匹配是 `(query, 大小写, 事实窗口)` 的纯函数值，
/// 不是需要维护的状态——把它存下来就一定会过期（滚动换了窗口但没人通知），
/// 于是迟早要有第二个写入者去「顺手修正」，就是这么破的。
/// 计数改由 `app::find::matches` 每帧从窗口算出，状态只留用户的选择。
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct FindState {
    pub visible: bool,
    pub query: String,
    pub case_sensitive: bool,
    /// `W`（Match Whole Word）：命中两端必须是词边界。
    ///
    /// 与 `case_sensitive` 同一条纪律——**只由 `reduce` 写**，投影层只读；
    /// 匹配集合在 `term_core::find::find_matches_in` 里算，这里不存结果。
    pub whole_word: bool,
    /// `.*`（Regular Expression）：查询串按正则解释。与大小写 / 整词同一纪律，
    /// 只由 `reduce` 写；匹配集合在 `find::matches` 里按这一位选字面 / 正则引擎。
    pub regex: bool,
    /// 当前匹配在窗口内匹配集合中的索引（对总数取模，窗口变了也不会越界）。
    pub current_match: usize,
}

/// 所有视图开关。纯数据，不含任何 UI 框架类型，可单测。
///
/// 字段集与「读者」一一对应（读者在 `ui/`，括号里注明）：
/// - `show_*` 五个：面板绘制前后的条件（`ui/mod.rs` 的 panel 分支）；
///   `show_toolbar` 是最后一个加入的（show_toolbar（原 Toolbar / Menu 页已移除））；
/// - `full_screen` / `always_on_top`：每帧与「已应用值」比较后发 `ViewportCommand`；
/// - `focus_mode`：压掉 sidebar / 状态栏 / 快捷栏 / 标签栏（菜单栏保留，作为退出路径）。
///
/// `focus_mode` 之所以不压菜单栏：**必须有退出的路**，否则用户点进专注模式就出不来。
// 快捷命令栏的分组数。必须与 `apps/desktop/src/ui/shell/quick_command_bar.rs::GROUPS`
// 长度一致（`quick_command_bar.rs` 的测试会断言）；改任意两边都要同步。
pub const QUICK_GROUP_COUNT: usize = 4;



#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ViewState {
    // ── 窗口 chrome ──
    pub show_sidebar: bool,
    pub show_menu_bar: bool,
    /// show_toolbar（原 Toolbar / Menu 页已移除）：标签条下面那条「窗格工具行」要不要画。
    ///
    /// 在此之前它**无条件绘制**（只有专注模式会压掉）。读者是 `ui/mod.rs`
    /// 声明 `pane_tools` 那条 `TopBottomPanel` 的那一行。
    ///
    /// 它**不落盘**：`layout_state.json` 只存标签 / 分屏 / 停靠位，从不存
    /// 菜单栏 / 状态栏 / 快捷栏这三个 `show_*`，这里跟着它们——重启回默认
    /// （显示）。要让它跨重启记住，得先给这三条一起做持久化，那是另一件事。
    pub show_toolbar: bool,
    pub show_status_bar: bool,
    pub show_quick_bar: bool,
    /// 四个快捷命令组的可见性开关，由底部栏左侧的配置菜单切换。
    ///
    /// 长度必须与 `apps/desktop/src/ui/shell/quick_command_bar.rs::GROUPS` 一致；
    /// 改任意一边都要同步更新 `QUICK_GROUP_COUNT`，`quick_command_bar.rs` 的测试
    /// 会断言两者相等。
    pub quick_groups_visible: [bool; QUICK_GROUP_COUNT],
    /// 文件面板（远端目录浏览）。新版截图里它在**左带下半**，默认开。
    ///
    /// 曾经默认关的理由是「会额外开一条 SSH 连接」；2026-11-08 起按视觉基线
    /// 铺满布局，这个开销是用户要的默认画面。没有可用凭据的会话仍只显示
    /// 一行提示、不发起连接（`filer.rs::ensure_filer_session`）。
    pub show_filer: bool,

    // ── 六个工具面板 + 2026-11-08 的 Sysmon ──
    // 每个开关都有 `ui/panels/` 的读者：开着就画一个有真实内容的面板，
    // 不是「点了只翻一个没人看的 bool」。2026-11-08 起 Explorer /
    // Outline / Sender / Shell 默认开（见 Default 里的分带说明）。
    pub show_pane_explorer: bool,
    pub show_pane_outline: bool,
    pub show_pane_sender: bool,
    pub show_pane_session: bool,
    pub show_pane_shell: bool,
    pub show_pane_transfer: bool,
    /// 终端底部 `Sysmon` 细带的展开位（`false` = 无边框 `Sysmon` 按钮，
    /// `true` = 原地显示 T/C/M 读数条）。默认关着。
    ///
    /// Sysmon 已不是停靠面板，此位只由底部那条细带的按钮翻
    /// （点按钮 ↔ 点读数条），夹具 `--demo sysmon` 也派发同一 `TogglePanel`。
    pub show_pane_sysmon: bool,
    /// Sysmon 的**远端采样开关**：开时读数条读 `sysmon_remote` 的第二 exec 通道
    /// 快照（活动标签是内置 SSH 才有数据），关时只读宿主本机。
    ///
    /// 它不进命令 ID 表（没有菜单 / 脚本字符串入口）：翻转只有界面上那一枚 R 方块，
    /// 走内部的 `ShellAction::ToggleSysmonRemote`。与 `show_pane_sysmon`（细带展开位）
    /// 分开——收起细带不该顺手关掉用户选的监控源。
    pub sysmon_remote: bool,
    /// 「脚本」停靠面板：在**当前活动标签页的会话**上跑脚本库的 Rhai 脚本。
    /// 默认关着，靠齿轮菜单勾选打开。面板体在
    /// `apps/desktop/src/ui/panels/script_panel.rs`，执行线程复用 `script_run`。
    pub show_pane_script: bool,

    // ── 模式 ──
    pub full_screen: bool,
    pub always_on_top: bool,
    pub focus_mode: bool,

    // ── 对话框 ──
    pub show_new_session: bool,
    pub show_settings: bool,
    pub show_about: bool,
    /// 日志设置 / 状态对话框。
    ///
    /// 它不只是「设置」——还显示当前会话正在往哪个文件写、暂停了没有。
    /// 日志是后台行为，没有面板就没人知道它到底有没有在记。
    pub show_log_settings: bool,
    /// 2026-11-15：`Terminal.ManageTriggers` 的落点从「Preferences 里翻到
    /// Triggers 页」改为**独立对话框**——兼容终端是带左列表、`+ - Duplicate Rename`
    /// 与 `Cancel / Ok` 的自有窗口，不是设置树里的一页（视觉基线
    /// `Highlight Text / Sessions` 双标签那份）。
    pub show_triggers: bool,
    /// 2026-11-15：同上，`Terminal.ManageOneKeys` 打开独立的 OneKeys 对话框
    /// （`SSH / Account / Sessions` 三标签 + 左列表 + `Cancel / Ok`）。
    pub show_onekeys: bool,

    /// 首次启动引导（Welcome → 探测到的 SSH 主机 → Done）。
    ///
    /// 读者是 `ui/mod.rs` 末尾那段对话框分发（`if view.show_onboarding`）与
    /// `ui/overlays/onboarding.rs`。出厂 `false`：**不是**「第一次用」，而是
    /// 「这一帧不画」——「要不要打开」的判断在 `construct.rs` 的 `init` 里，
    /// 依据是 `Prefs::onboarded`（判据必须是偏好文件里的显式位，不是「布局文件
    /// 存不存在」：后者把「删过布局」误当成「第一次用」）。
    ///
    /// 它与 `term_app::onboarding::OnboardingState::open` 恒等，两边在投影层的
    /// `open_onboarding` / `finish_onboarding` 里一起改——没有第二个写入者。
    pub show_onboarding: bool,

    /// 首次启动的「Choose Profiles Directory」对话框。出厂 `false`；打开判据在
    /// `construct.rs::init`：没有选择标记、且不在夹具模式时置位。确认后写
    /// profile 目录标记并关窗，再继续 `show_onboarding`。
    pub show_profile_choice: bool,

    /// 预览窗（Filer 双击 / 右键「预览」打开的文件）。
    ///
    /// 读者是 `ui/mod.rs` 末尾那段窗口分发（`if view.show_preview`）与
    /// `ui/overlays/preview.rs` 的 `draw_preview` / `close_preview`。
    ///
    /// 出厂 `false`：一次启动没有任何文件在预览。它与
    /// `overlays::PreviewState::open` 恒等，两边只在 `open_preview_local` /
    /// `open_preview_remote` / `close_preview` 三处一起改——没有第二个写入者
    /// （与 `show_onboarding` 同一条边界）。
    pub show_preview: bool,

    // ── Find (Search) ──
    pub find: FindState,

    /// 命令面板（开没开 / 范围 / 查询 / 选中第几个）。
    ///
    /// 读者是 `ui/palette.rs`：面板只在 `visible` 时画，条目与过滤结果由
    /// `app::palette` 从菜单树 + 查询词现算，投影层一个字段都不写回来。
    pub command_palette: PaletteState,

    /// Goto 面板（开没开 / 模式 / 输入 / 选中 / 输入是否非法）。
    ///
    /// 读者是 `ui/goto.rs`：面板只在 `visible` 时画，`error` 直接显示成提示，
    /// 输入经 `SetInput` 派发；投影层一个字段都不写回来。
    pub goto: GotoState,

    /// v0.1 工作台（开没开 / 哪个页面 / 输入 / 选中 / 历史 / 高亮 /
    /// 工具栏隐藏位 / 一次性提示）。读者是 `ui/center.rs` 与 `ui/terminal.rs`
    /// （常驻高亮）——投影层只读，一个字段都不写回来。
    pub center: CenterState,

    /// 锁屏（复用 Vault 主密码）。读者是 `ui/lock.rs` 的全屏遮罩。
    pub lock: LockState,
    /// 快捷键绑定表窗口。读者是 `ui/shortcuts.rs`。
    pub show_shortcuts: bool,
    /// 2026-11-11：独立的「快捷命令」配置弹窗。读者是
    /// `ui/dialogs/quick_commands.rs`；命令表数据来自 `quickbar.json`。
    pub show_quick_commands: bool,
    /// tmux 控制模式面板。读者是 `ui/tmux_panel.rs`。
    pub show_tmux: bool,
    /// 同步输入开关。开着时键盘输入写给**所有**活着的标签，
    /// 读者是 `ui/terminal.rs` 的按键处理与状态栏的 SYNC 徽标。
    pub sync_input: bool,
    /// 4 条同步输入通道：每条是一组成员身份。空表 = 该通道无人；
    /// `sync_input` 仍表示「是否开着」，两者由 `sync_channels` 的辅助函数同步。
    pub sync_channels: [Vec<term_core::session_runtime::SessionId>; 4],
    /// 隧道管理面板。读者是 `ui/tunnel_manager.rs`。
    pub show_tunnels: bool,

    /// 2026-11-07：界面语言。读者是每一处 `i18n::t(...)`（菜单 / 面板标题 /
    /// 状态栏 / 发送器工具行 / 首选项 / 关于）。
    ///
    /// 它是**领域状态**而不是几何：语言要跨面板一致、要能被 reducer 唯一写入，
    /// 还要落盘（`term-runtime::prefs`）。与 `toolbar_pos` 那种投影态不同。
    pub lang: crate::i18n::Lang,

    /// 2026-11-07：终端左侧 margin 的五列开关。读者是
    /// `ui/terminal/margin.rs`——每一列都对应一段真的绘制，`Fold` 连 `parse`
    /// 都不认（见 `app::margin`），所以它永远画不出置灰以外的样子。
    pub margin: MarginState,
}

impl Default for ViewState {
    fn default() -> Self {
        Self {
            show_sidebar: true,
            show_menu_bar: true,
            show_toolbar: true,
            show_status_bar: true,
            show_quick_bar: true,
            quick_groups_visible: [true; QUICK_GROUP_COUNT],
            // 2026-11-08：新版截图是「全套面板铺满」的默认布局——左带
            // Explorer + Filer、右带会话树 + Outline、底带 Sender + Shell
            // 全部亮着。下面这些连同 show_sidebar 就是启动画面。
            // Session / Transfer / Sysmon 视觉基线里没有内容区，仍默认关。
            show_filer: true,
            show_pane_explorer: true,
            show_pane_outline: true,
            show_pane_sender: true,
            show_pane_session: false,
            show_pane_shell: true,
            show_pane_transfer: false,
            show_pane_sysmon: false,
            sysmon_remote: false,
            show_pane_script: false,
            full_screen: false,
            always_on_top: false,
            focus_mode: false,
            show_new_session: false,
            show_settings: false,
            show_about: false,
            show_log_settings: false,
            show_triggers: false,
            show_onekeys: false,
            // 出厂不画引导。打开它是 `construct.rs::init` 的活（依据
            // `Prefs::onboarded`），`Default` 只回答「这一帧画不画」。
            show_onboarding: false,
            show_profile_choice: false,
            // 预览窗同样出厂不画——它是浮层，启动时不该盖住终端。
            show_preview: false,
            find: FindState::default(),
            command_palette: PaletteState::default(),
            goto: GotoState::default(),
            center: CenterState::default(),
            lock: LockState::default(),
            show_shortcuts: false,
            show_quick_commands: false,
            show_tmux: false,
            sync_input: false,
            sync_channels: Default::default(),
            show_tunnels: false,
            lang: crate::i18n::Lang::default(),
            margin: MarginState::default(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_show_all_chrome_and_no_mode() {
        let s = ViewState::default();
        assert!(s.show_sidebar && s.show_menu_bar && s.show_status_bar && s.show_quick_bar);
        // 工具行出厂是画的（在此之前它无条件绘制，默认值必须保持这个画面）。
        assert!(s.show_toolbar);
        // 三种「模式」默认关闭：它们会改变窗口外观，启动时不该有任何意外效果。
        assert!(!s.full_screen && !s.always_on_top && !s.focus_mode);
        assert!(!s.show_new_session && !s.show_settings && !s.show_about);
        // 日志默认关：它往磁盘写东西，不该因为「有这个功能」就自己开始记。
        assert!(!s.show_log_settings);
        // 命令面板默认关：它是浮层，启动时不该盖住终端。
        assert!(!s.command_palette.visible);
        // Goto 面板同理。
        assert!(!s.goto.visible && !s.goto.error);
        // 工作台同理，且出厂不带任何高亮词 / 隐藏按钮。
        assert!(!s.center.visible && s.center.highlights.is_empty());
        // v0.1 出口：锁屏 / 快捷键表 / tmux 面板默认都不占屏。
        assert!(!s.lock.locked && !s.show_shortcuts && !s.show_tmux);
        // 两个一次性 / 临时窗口出厂都不占屏（引导由 init 按 prefs 打开，
        // 预览窗只能由用户对某一行动手打开）。
        assert!(!s.show_onboarding && !s.show_preview && !s.show_profile_choice);
        // 同步输入默认关：它会让一次按键作用到所有会话，不能默认打开。
        assert!(!s.sync_input && !s.show_tunnels);
        // 新版截图默认铺满：左 Explorer/Filer、右会话树/Outline、底 Sender/Shell。
        // 另两个工具面板（Session 列表 / Transfer）视觉基线里没有，仍默认关。
        assert!(s.show_sidebar && s.show_filer);
        assert!(s.show_pane_explorer
            && s.show_pane_outline
            && s.show_pane_sender
            && s.show_pane_shell);
        assert!(!s.show_pane_session && !s.show_pane_transfer && !s.show_pane_sysmon);
        // 出厂读本机，不主动开第二条连接（远端 exec 要等用户明确点 R）。
        assert!(!s.sysmon_remote);
    }
}

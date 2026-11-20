//! 菜单树：纯配置数据，不含任何 UI 类型（I5 / I4）。
//!
//! R3 之前这棵 10 个顶级菜单的树长在 `ui/mod.rs` 里，和绘制逻辑
//! 混在一起，`&'static str` 命令 ID 直接写死在 UI 代码中。现在它只是数据：
//! 某项「有没有实现」不在这里决定，由 `capability` 查 `Action::parse` 得出，
//! UI 只负责把未实现的项画成置灰。
//!
//! **R10 起是 114 项**（原 220）。**R10 删掉的 106 条是原程序文本编辑器语义**，但**删除结论已于 2026-11-10 作废**：
//! 这 106 条**是正常需求**，不是噪音（Hex 视图、折叠、边距/符号列/行尾/缩进引导、空白符显示、
//! 编辑器式选择与移动、多选操作）——删菜单不可逆，会破坏视觉基线结构对齐（P1 视觉基线判据）。
//! 正确做法是**保留结构 + 逐波接线**。W2（2026-11-09）已补回 68 条（`menu_submenus.rs`），
//! 2026-11-13 又从中接走 44 条（Hex 13 + Symbols 5 + White Spaces 5 + Goto 28 + Select Column To 10），
//! 剩 62 条仍置灰，逐条处置见 `docs/design/12-capability-surface-and-menus.md`。菜单不是越全越好，长期置灰会把信噪比做没，
//! 但让置灰项看得见「为什么置灰」+ 逐波接线比删菜单好。
//!
//! **W2 订正（按 §4-D1 裁决，不是推翻 R10）**：父级子菜单的**结构**是
//! independent 可观测的对齐对象，而「点开只有一条横线」比缺一项更坏。
//! 所以 18 个空子菜单由 `menu_submenus.rs` 按 P1 视觉基线逐行补齐，
//! 补进来的 68 条一律 `Planned`（置灰 + 理由 + 入册），
//! **一条都没假装接线**。R10 删的是「长期无意义的灰项」，
//! W2 补的是「用户看得见的结构」——两件事分开记账，
//! 接线的进度看 `capability::counts()`，结构对齐的进度看子菜单判据。
//! `menu_check_tests.rs` 与 `menu_submenus.rs` 的断言一起守住这条分界。
//!
//! # 文件布局
//!
//! `menu.rs` 是**模块根**（不是 `menu/mod.rs`）：`scripts/verify.sh` 第 0 项的
//! 门禁路径与第 4e 项的数据表白名单都按 `crates/term-app/src/menu.rs` 这个
//! **路径**写死，改成 `menu/mod.rs` 会让门禁整片判红。子模块放在同名目录里：
//!
//! - `menu/tree.rs`：10 棵顶级菜单的数据表（`menus()`，最大的一块）
//! - `menu/shortcuts.rs`：出厂绑定表 / 两段式键序表 / 菜单右侧的快捷键列
//! - `menu/bars.rs`：菜单栏右侧工具条、标签条右端、布局下拉的条目
//! - 本文件：`Menu` / `Item` 数据模型、`access_char_pos`，以及把上面三项
//!   按原路径再导出（`pub use`），外部读者一行都不用改。

mod bars;
#[path = "menu_submenus.rs"]
mod submenus;
mod shortcuts;
mod tree;

pub use bars::{
    BarGlyph, BarTool, DockItem, TabTool, TunnelItem, BAR_TOOLS, DOCK_ITEMS, GOTO_ITEMS,
    TAB_NEW_TAB_ID, TAB_TOOLS, TUNNEL_ITEMS, ZOOM_ITEMS,
};
pub use shortcuts::{
    shortcut_for, shortcut_text, SeqShortcut, Shortcut, DEFAULT_SHORTCUTS, DEFAULT_SHORTCUT_SEQS,
};
pub use tree::menus;

/// 右键菜单复用的命令 ID。UI 侧行表（`terminal/context_rows.rs`）按 label
/// 画手绘行，判态（`capability::is_implemented`）与派发（`Work::Command`）
/// 都需要命令 ID——但 I5 不许命令 ID 字面量出现在 ui/ 里。这里把「右键那几行
/// 对应的 ID」集中成数据层常量，ui 只引用名字。`menu_check_tests.rs` 钉住
/// 每个常量都与菜单树里同 label 项的 id 一致，杜绝两处漂移。
pub mod context_ids {
    pub const HIGHLIGHT_TEXT: &str = "Text.HighlightText";
    pub const SELECT_COMMAND: &str = "Text.SelectCommand";
    pub const SAVE_SESSION: &str = "Terminal.Save";
    pub const LINE_ENDINGS: &str = "View.ToggleLineEndings";
    pub const WRAP_SYMBOLS: &str = "View.ToggleWrapSymbols";
    pub const SEARCH_ONLINE: &str = "Text.SearchOnline";
}

/// 一个顶级菜单。
#[derive(Clone)]
pub struct Menu {
    pub label: &'static str,
    pub items: Vec<Item>,
}

/// 一条勾选行的勾从哪来。
///
/// 只有两种可能，而且**可判定地**对应「这条命令接没接线」：
///
/// - [`CheckState::Fixed`]：编译期常量。只能用在**没有运行期状态可读**的行上
///   （本版剩下 Mode 的四条「模式」行），它们也都没接线、点不动；
/// - [`CheckState::Live`]：投影层每帧按当前会话现读。只能用在**已接线**的行上
///   （十行栅格可视化），否则屏幕上会出现一个点了不动、勾也不动的框。
///
/// 判据在 `menu_check_tests.rs::a_wired_command_must_read_live_state`，
/// 它把两个方向都钉死——这不是风格问题：勾错方向的唯一症状是「屏幕撒谎」，
/// 而那正是 S4 要消灭的东西。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CheckState {
    /// 写死的事实（`true` = 框里画 ✓）。
    Fixed(bool),
    /// 由投影层按当前会话现读（菜单树只说「这里有个勾选框」）。
    Live,
}

/// 菜单项：命令、可勾选项、分隔线、子菜单。
#[derive(Clone)]
pub enum Item {
    /// `id` 是命令 ID，与 `Action::parse` 的入口一致；未实现时由 UI 置灰。
    Action {
        label: &'static str,
        id: &'static str,
    },
    /// 左边带一个方框的一行（`checked` 决定框里有没有 ✓）。
    ///
    /// # `checked` 有两种：写死的事实 / 让投影层现读
    ///
    /// 菜单树是纯数据（`menu.rs` 不认识 `ViewState`，也不认识任何窗口），所以
    /// 「用户此刻开没开」在**这一层**永远表达不了。第一版因此只允许写常量，
    /// 于是四条「模式」行（Remote Mode / Word Wrap / Virtual Space /
    /// Multiple Caret）写死不动的勾——它们都没有运行期状态可读：
    ///
    /// | 行 | 勾 | 这句话为什么为真 |
    /// |---|---|---|
    /// | Remote Mode | ✓ | 本版没有「本地 / 远端模式」这个**开关**：协议是每个会话自己的属性（`Protocol::Shell` 起本地 shell，`Ssh` 连远端、`Serial` 开串口，见 `ui/construct.rs` 的出厂会话），不是整机的一个模式，所以没有「切过去 / 切回来」的状态可翻 |
    /// | Word Wrap | ✓ | 终端是定宽栅格，换行是栅格本身，没有「不换行」形态 |
    /// | Virtual Space | ☐ | 编辑器语义，`R10` 已从本程序删除 |
    /// | Multiple Caret | ☐ | 同上 |
    ///
    /// 2026-11-13 起多了一种 [`CheckState::Live`]：**这一行是个勾选框，
    /// 勾不勾由投影层按当前会话现读**。十行栅格可视化（`View ▸ Symbols`
    /// 四项 / `View ▸ White Spaces ▸ Hide All` / `Text ▸ Highlight Caret Line`）
    /// 走这一种——它们真的能切，而一个不跟着状态走的勾正是 S4
    /// （「有开关、没反馈」）。
    ///
    /// 谁来读：`apps/desktop/src/ui/shell/menu_items.rs::live_check`，走
    /// `Action::parse(id)` → `ShellAction::Marks(knob)` →
    /// `TerminalWindow::marks_knob_on(knob)` → 活动标签的 `Tab.marks`。
    /// 两条护栏互锁（见 `menu_check_tests.rs::a_wired_command_must_read_live_state`）：
    /// `Fixed` 必须**没有**接线，`Live` 必须**已经**接线——写反任一方向，
    /// 屏幕上的勾都会撒谎。
    Check {
        label: &'static str,
        id: &'static str,
        checked: CheckState,
    },
    Separator,
    Submenu {
        label: &'static str,
        items: Vec<Item>,
    },
}

/// 菜单标签里要加下划线的访问字母的**字符下标**（从 0 数）。视觉基线里
/// 多数项划首字母，但有五个例外（`New Window` 划 W、`Open Recent` 划
/// Recent 的 R……）；中文标签一律划首字。返回 `None` = 不划。
pub fn access_char_pos(label_en: &str) -> Option<usize> {
    const EXCEPTIONS: &[(&str, &str)] = &[
        ("New Window", "Window"),
        ("Open Recent", "Recent"),
        ("Save Selection As", "Selection"),
        ("Close All Views", "All"),
        ("Close Window", "Window"),
        // Log 子菜单（2026-11-08 视觉基线）：`Open L̲og File` 划 Log 的 L、
        // `Open Log F̲older` 划 Folder 的 F，避开重复的 O。
        ("Open Log File", "Log"),
        ("Open Log Folder", "Folder"),
        // Mode 菜单（2026-11-08 视觉基线）：同一菜单里 `F̲ocus Mode` 先占了 F，
        // 紧跟的 `Focus Mode S̲ettings...` 于是划 Settings 的 S——视觉基线就是这样。
        ("Focus Mode Settings", "Settings"),
        // Tool ▸ Transfer Binary（2026-11-17 视觉基线）：六行的下划线全钉在传输
        // 协议的字母上（`S̲end X̲Modem` → 划 X），不是首字母 `S` / `R`。
        // 六行同处一个子菜单，首字母全是 S、R 两个重复字母，划首位等于没划。
        ("Send XModem", "X"),
        ("Send YModem", "Y"),
        ("Send ZModem", "Z"),
        ("Receive XModem", "X"),
        ("Receive YModem", "Y"),
        ("Receive ZModem", "Z"),
    ];
    for (prefix, word) in EXCEPTIONS {
        if label_en.starts_with(prefix) {
            return label_en.find(word);
        }
    }
    // 默认：首字符。
    label_en.char_indices().next().map(|(i, _)| i)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// R5：工具行 `>` 菜单、`＋` 派发命令、以及 `View ▸ Zoom` 的三条缩放命令
    /// 都必须是已接线。少一条就变成「点了没反应」的假菜单项。
    /// 2026-11-15：标签条右端的字母已确认是 **group 名字**（不是字体缩放按钮），
    /// 不再弹 Zoom 菜单；这三条 Zoom 命令仍由 `View ▸ Zoom` 独立入口消费，
    /// 所以这里继续守它们的接线状态。
    #[test]
    fn every_tab_and_tool_bar_menu_command_is_wired() {
        let ids = ZOOM_ITEMS
            .iter()
            .chain(GOTO_ITEMS.iter())
            .map(|(_, id)| *id)
            .chain(std::iter::once(TAB_NEW_TAB_ID));
        for id in ids {
            assert!(
                crate::capability::is_implemented(id),
                "{id} 没有接线，画出来就是假按钮"
            );
        }
        assert_eq!(TAB_TOOLS, &[TabTool::GroupName, TabTool::NewTab, TabTool::TabList]);
    }

    /// 2026-11-17 用户点名：「右上角布局按钮展开内容不对，目前 list 只有 2 个」。
    ///
    /// 判据要钉在**带的数量**上，而不是「下拉里有几行」：`DockZone::ALL` 是三条，
    /// 下拉就必须逐条给出口，少一条的症状是「某条带永远收不起来」，而菜单仍然
    /// 画得出来、编译也不报。第二段守的是另一半——这三条都得真接线，
    /// 否则点了只是关掉弹层。
    #[test]
    fn the_layout_dropdown_offers_every_dock_zone_and_all_are_wired() {
        for zone in crate::dock::DockZone::ALL {
            assert!(
                DOCK_ITEMS.iter().any(|i| i.zone == zone),
                "`{}` 带在布局下拉里没有入口",
                zone.name()
            );
        }
        assert_eq!(DOCK_ITEMS.len(), crate::dock::DockZone::ALL.len(), "一个带只该有一条命令");
        for item in DOCK_ITEMS {
            assert!(
                crate::capability::is_implemented(item.id),
                "{} 没接线，点下去只会关掉弹层",
                item.id
            );
        }
    }

    /// 2026-11-17 Tunnels 视觉基线：配置对话框在三个地方露出——菜单栏 `Tunnels`
    /// 下拉第二行、`Tool ▸ Tunnel Settings`、以及 `Window ▸ Panes` 里的总线开关。
    /// 这里守的是前两个的**位置关系**：兼容终端 `Tool ▸ Tunnels` 打开的就是编辑器，
    /// 本仓那条先占了「在跑的面板」，所以设置单独给一行，且必须紧跟在它后面——
    /// 夹进别的项会让「Tunnels 之后就是设置」这条肌肉记忆失效。
    #[test]
    fn tunnel_settings_sits_right_after_tunnels_in_the_tool_menu() {
        fn label_of(item: &Item) -> &'static str {
            match item {
                Item::Action { label, .. }
                | Item::Check { label, .. }
                | Item::Submenu { label, .. } => label,
                Item::Separator => "",
            }
        }
        let tool = menus()
            .into_iter()
            .find(|m| m.label == "Tool")
            .expect("Tool 菜单在树里");
        let at = tool
            .items
            .iter()
            .position(|i| label_of(i) == "Tunnels")
            .expect("Tool 逐项里要有 Tunnels");
        assert_eq!(
            label_of(&tool.items[at + 1]),
            "Tunnel Settings",
            "设置对话框要紧跟 Tunnels，中间不夹别的项"
        );
        for id in ["Terminal.ManageTunnels", "Terminal.TunnelSettings"] {
            assert!(
                crate::capability::is_implemented(id),
                "{id} 没接线，点下去只会关掉弹层"
            );
        }
    }

    /// 2026-11-08 视觉基线：Session 菜单的快捷键列与访问字母下划线。
    /// 快捷键按 id 查（改标签文案不动快捷键）；下划线五个例外词 +
    /// 默认首字母，谁动了这里立刻显形。
    #[test]
    fn session_menu_shortcuts_and_access_letters_match_the_reference() {
        assert_eq!(shortcut_text("Terminal.NewSession"), "Meta+N");
        assert_eq!(shortcut_text("Application.NewWindow"), "Meta+Shift+N");
        assert_eq!(shortcut_text("Terminal.ReconnectSession"), "Return");
        assert_eq!(shortcut_text("Window.CloseActiveView"), "Meta+Shift+W");
        assert_eq!(shortcut_text("Window.Close"), "Meta+Shift+Q");
        assert_eq!(shortcut_text("Application.Exit"), "Alt+F4");
        // 没抄进视觉基线的项一律空串（不占位）。
        assert_eq!(shortcut_text("Text.Copy"), "");
        // 下划线：默认首字母，例外词取词首。
        assert_eq!(access_char_pos("Save Session"), Some(0));
        assert_eq!(access_char_pos("New Window"), Some(4));
        assert_eq!(access_char_pos("Open Recent"), Some(5));
        assert_eq!(access_char_pos("Save Selection As"), Some(5));
        assert_eq!(access_char_pos("Close All Views"), Some(6));
        assert_eq!(access_char_pos("Close Window"), Some(6));
        // Log 子菜单的两个例外：避开重复的 O。
        assert_eq!(access_char_pos("Open Log File"), Some(5));
        assert_eq!(access_char_pos("Open Log Folder"), Some(9));
    }

    /// 2026-11-08 视觉基线：Log 子菜单是四组，文案比旧版短；命令 ID 一个都不动
    /// （capability 台账、派发、命令中心路径都靠它）。
    #[test]
    fn log_submenu_matches_the_reference_four_groups() {
        let session = &menus()[0];
        let log = session
            .items
            .iter()
            .find_map(|it| match it {
                Item::Submenu { label, items } if *label == "Log" => Some(items),
                _ => None,
            })
            .expect("Session 菜单里要有 Log 子菜单");
        let shape: Vec<&str> = log
            .iter()
            .map(|it| match it {
                Item::Action { label, .. } | Item::Check { label, .. } => *label,
                Item::Separator => "|",
                Item::Submenu { .. } => "<submenu>",
            })
            .collect();
        assert_eq!(
            shape,
            vec![
                "Start",
                "|",
                "Stop",
                "Pause",
                "Resume",
                "|",
                "Open Log File",
                "Open Log Folder",
                "|",
                "Settings",
            ]
        );
        let ids: Vec<&str> = log
            .iter()
            .filter_map(|it| match it {
                Item::Action { id, .. } => Some(*id),
                _ => None,
            })
            .collect();
        assert_eq!(
            ids,
            vec![
                "Terminal.StartLogger",
                "Terminal.StopLogger",
                "Terminal.PauseLogger",
                "Terminal.ResumeLogger",
                "Terminal.OpenLogFile",
                "Terminal.OpenLogFolder",
                "Terminal.OpenLogSettings",
            ]
        );
    }
}

#[cfg(test)]
#[path = "menu_check_tests.rs"]
mod check_tests;

#[cfg(test)]
#[path = "menu_context_ids_tests.rs"]
mod context_ids_tests;

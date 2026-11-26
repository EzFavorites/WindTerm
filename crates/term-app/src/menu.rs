//! 菜单树：纯配置数据，不含任何 UI 类型。
//!
//! 之前这棵 10 个顶级菜单的树长在 `ui/mod.rs` 里，和绘制逻辑
//! 混在一起，`&'static str` 命令 ID 直接写死在 UI 代码中。现在它只是数据：
//! 某项「有没有实现」不在这里决定，由 `capability` 查 `Action::parse` 得出，
//! UI 只负责把未实现的项画成置灰。
//!
//! 菜单曾按「编辑器语义是噪音」删过一批，这个结论后来作废：那些项是正常需求
//! （Hex 视图、折叠、边距与符号列、空白符显示、编辑器式选择与移动、多选操作）。
//! 删菜单不可逆，还会破坏视觉基线的结构对齐，所以定下的做法是**保留结构 + 逐波接线**：
//! 先按视觉基线把空子菜单补齐并登记为置灰，再逐步把其中真正能做的接成真开关。
//! 置灰项是结构的一部分，逐条处置见 `docs/design/12-capability-surface-and-menus.md`。
//! 菜单不是越全越好，但让置灰项看得见「为什么置灰」比删菜单好。
//! 已接线数与置灰数不在这里写死，统一由能力清单的 `inventory()` 现算；写死的数字会随
//! 每次接线立刻过期，能力面文档与断言才是唯一口径。
//!
//! **订正（按既有裁决，不是推翻之前的删除）**：父级子菜单的**结构**是
//! independent 可观测的对齐对象，而「点开只有一条横线」比缺一项更坏。
//! 所以那些空子菜单由 `menu_submenus.rs` 按视觉基线逐行补齐，
//! 补进来的条目一律 `Planned`（置灰 + 理由 + 入册），
//! **一条都没假装接线**。删的是「长期无意义的灰项」，
//! 补的是「用户看得见的结构」——两件事分开记账，
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
    context_seq_for, margin_seq, shortcut_for, shortcut_text, SeqShortcut, Shortcut,
    CONTEXT_SEQ_ROWS, DEFAULT_SHORTCUTS, DEFAULT_SHORTCUT_SEQS,
};
pub use tree::menus;

/// 右键菜单复用的命令 ID。UI 侧行表（`terminal/context_rows.rs`）按 label
/// 画手绘行，判态（`capability::is_implemented`）与派发（`Work::Command`）
/// 都需要命令 ID——但命令 ID 字面量不许出现在 ui/ 里。这里把「右键那几行
/// 对应的 ID」集中成数据层常量，ui 只引用名字。`menu_check_tests.rs` 钉住
/// 每个常量都与菜单树里同 label 项的 id 一致，杜绝两处漂移。
pub mod context_ids {
    pub const HIGHLIGHT_TEXT: &str = "Text.HighlightText";
    pub const SELECT_COMMAND: &str = "Text.SelectCommand";
    pub const SAVE_SESSION: &str = "Terminal.Save";
    pub const LINE_ENDINGS: &str = "View.ToggleLineEndings";
    pub const WRAP_SYMBOLS: &str = "View.ToggleWrapSymbols";
    /// ⋮ 弹层的 `Word Wrap` 行：2026-11-24 已接线（`Tab.word_wrap` 的软折开关）。
    /// UI 侧不许写 ID 字面量（4e），所以在这里给出唯一的数据层入口。
    pub const TEXT_TOGGLE_WORD_WRAP: &str = "Text.ToggleWordWrap";
    pub const SEARCH_ONLINE: &str = "Text.SearchOnline";

    /// 终端**内容**右键的四条真实命令。它们不在菜单栏里，但和标签右键的
    /// 影子入口一样是真实可点入口：这里给出唯一的 ID 数据表，UI 只引用名字
    /// （`scripts/verify.sh` 4e 不允许 `ui/` 出现 ID 字面量）。
    ///
    /// 同时被 `capability::inventory` 与 `palette::entries` 消费——两处因此
    /// 不可能只改一处。文案用于面板分组显示，与右键清单同源。
    pub const COPY_COMMAND: &str = "Terminal.CopyCommand";
    pub const CLEAR_SCREEN: &str = "Terminal.ClearScreen";
    pub const CLEAR_SCROLLBACK: &str = "Terminal.ClearScrollback";
    pub const CLEAR_ALL: &str = "Terminal.ClearAll";
    pub const TERMINAL_CONTEXT_COMMANDS: &[(&str, &str)] = &[
        ("Copy Command", COPY_COMMAND),
        ("Clear Screen", CLEAR_SCREEN),
        ("Clear Scrollback", CLEAR_SCROLLBACK),
        ("Clear All", CLEAR_ALL),
    ];

    /// tmux 浮层开关。原先只有 `parse` 臂、没有菜单 / 影子入口，是一条孤儿
    /// 命令（能力台账查不到它）。这里给它一个数据层身份，`comp` 夹具入口与
    /// 自省清单都引用同一个常量。
    pub const TOGGLE_PANE_TMUX: &str = "Window.TogglePaneTmux";

    /// 脚本运行命令 ID 的前缀，定义在词表层（`parse` 只许依赖 `vocab`）。
    pub const RUN_FILE_PREFIX: &str = crate::vocab::RUN_FILE_PREFIX;
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
///   （hex 三组互斥档位 + 十行栅格可视化），否则屏幕上会出现一个点了不动、
///   勾也不动的框。
///
/// 判据在 `menu_check_tests.rs::a_wired_command_must_read_live_state`，
/// 它把两个方向都钉死——这不是风格问题：勾错方向的唯一症状是「屏幕撒谎」，
/// 而那正是要消灭的东西。
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
    /// 「用户此刻开没开」在**这一层**永远表达不了，常量勾只能写死。第一版的四条
    /// 「模式」行已有两条改成 [`CheckState::Live`]：勾由投影层按当前会话现读，
    /// 不再是写死不动的常量。只剩两条编辑器语义仍是 `Fixed`：
    ///
    /// | 行 | 勾 | 现状 |
    /// |---|---|---|
    /// | Remote Mode | Live | 2026-11-23 已接线：`Window.SetRemoteMode` 退出只读，勾读 `Tab.local_mode`（`!local_mode_on()`） |
    /// | Word Wrap | Live | 2026-11-24 已接线：`Text.ToggleWordWrap` 翻 `Tab.word_wrap`，段表按位软折 |
    /// | Virtual Space | Fixed(false) | 终端行编辑行为，尚未实现（待办）；空框按视觉基线写死 |
    /// | Multiple Caret | Fixed(true) | 同上；勾按视觉基线写死，不代表命令可用 |
    ///
    /// 另有一种 [`CheckState::Live`]：**这一行是个勾选框，
    /// 勾不勾由投影层按当前会话现读**。hex 三张子菜单的 13 档互斥单选
    /// （`View ▸ Hex Base / Hex Columns / Hex Group Chars`）与十行栅格可视化
    /// （`View ▸ Symbols` 四项 / `View ▸ White Spaces ▸ Hide All` /
    /// `Text ▸ Highlight Caret Line`）走这一种——它们真的能切，而一个不跟着
    /// 状态走的勾正是问题所在（「有开关、没反馈」）。
    ///
    /// 谁来读：`apps/desktop/src/ui/shell/menu_items.rs::live_check`，走
    /// `Action::parse(id)` → `ShellAction::Marks(knob)` /
    /// `ShellAction::HexBase|HexColumns|HexGroupChars(value)` →
    /// `TerminalWindow::marks_knob_on(knob)` / `active_hex()` →
    /// 活动标签的 `Tab.marks` / `Tab.hex`。
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
        // 上游 `menu.config` 写的是 `Open Rece&nt`——划的是 n，不是 Recent 的 R。
        ("Open Recent", "nt"),
        // 以下五条同样按上游 `menu.config` 的 `&` 位置取词，避免同级的
        // Select / Syntax / Window 重复占首字母。
        ("Select To", "To"),
        ("Select Column To", "Column"),
        ("Syntax Schemes", "yntax"),
        ("Window Opacity", "Opacity"),
        ("Set Tab Color", "Color"),
        ("Save Selection As", "Selection"),
        ("Close All Views", "All"),
        ("Close Window", "Window"),
        // Log 子菜单（视觉基线）：`Open L̲og File` 划 Log 的 L、
        // `Open Log F̲older` 划 Folder 的 F，避开重复的 O。
        ("Open Log File", "Log"),
        ("Open Log Folder", "Folder"),
        // Mode 菜单（视觉基线）：同一菜单里 `F̲ocus Mode` 先占了 F，
        // 紧跟的 `Focus Mode S̲ettings...` 于是划 Settings 的 S——视觉基线就是这样。
        ("Focus Mode Settings", "Settings"),
        // View ▸ Hex（参考图 `01-view-menu.png`）：三行都以 `Hex ` 开头，
        // 下划线钉在第二个词的首字母上（`Hex B̲ase` / `Hex C̲olumns` /
        // `Hex G̲roup Chars`），不是每行都重复的 `H`。
        ("Hex Base", "Base"),
        ("Hex Columns", "Columns"),
        ("Hex Group Chars", "Group"),
        // Tool ▸ Transfer Binary（视觉基线）：六行的下划线全钉在传输
        // 协议的字母上（`S̲end X̲Modem` → 划 X），不是首字母 `S` / `R`。
        // 六行同处一个子菜单，首字母全是 S、R 两个重复字母，划首位等于没划。
        // Edit（`01-edit-menu.png`）：`Paste Selected T̲ext` 划第二个词，
        // `Copy Session N̲ame` / `Copy Session U̲rl` 划末词，不是首字母 C。
        ("Paste Selected Text", "Selected"),
        ("Copy Session Name", "Name"),
        ("Copy Session Url", "Url"),
        // Search（`01-search-menu.png`）：四条 Find 行划「Next / Previous」，
        // 不是每行重复的 F。
        ("Goto Address Bar", "Address"),
        ("Goto Session", "Session"),
        ("Goto Position", "Position"),
        ("Goto Symbol", "Symbol"),
        ("Find Next", "Next"),
        ("Find Previous", "Previous"),
        ("Find Next Selection", "Next"),
        ("Find Previous Selection", "Previous"),
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

/// 同一个字符能不能当访问键：空白和标点划了没有意义，也不会被键盘命中。
fn usable_access_char(c: char) -> bool {
    c.is_alphanumeric()
}

/// 给**同一级**菜单逐行分配访问字母的下划线位置。
///
/// `access_char_pos` 只认单条英文标签，不知道旁边那行占了哪个字母，于是
/// `Select*` / `Margin*` / `Show*` 这些同族行全划首字，按 Alt+S 根本分不清。
/// 这里在上游优先位置（`keys`）之上加一道同级去重：先用上游的词位，撞了就在
/// 本行**实际显示的文字**（`shown`，中文时与英文不同）里往后找没被占的字符，
/// 再从头兜一圈；整行都撞才留空。返回顺序与 `shown` 一一对应。
pub fn access_positions_unique(shown: &[&str], keys: &[&str]) -> Vec<Option<usize>> {
    let preferred: Vec<Option<usize>> = keys
        .iter()
        .enumerate()
        .map(|(i, key)| {
            if shown.get(i).copied() == Some(*key) {
                access_char_pos(key)
            } else {
                shown
                    .get(i)
                    .and_then(|text| {
                        text.char_indices()
                            .find(|(_, c)| usable_access_char(*c))
                            .map(|(pos, _)| pos)
                    })
            }
        })
        .collect();
    access_positions_from(shown, &preferred)
}

/// 与 [`access_positions_unique`] 同一套去重，但**优先位置由调用方给**：
/// 右键菜单每行自带视觉基线的 `acc`，不能让 `access_char_pos` 把它改掉。
pub fn access_positions_from(shown: &[&str], preferred: &[Option<usize>]) -> Vec<Option<usize>> {
    let mut used = std::collections::HashSet::new();
    shown
        .iter()
        .enumerate()
        .map(|(i, text)| {
            let preferred = preferred.get(i).copied().flatten();
            let mut candidates = Vec::new();
            if let Some(pos) = preferred {
                // 外部调用方可能把英文标签的字节下标配到中文显示文本上；
                // 不是字符边界就直接丢掉，不能让它进 `text[pos..]` 造成 panic。
                if text.is_char_boundary(pos) {
                    candidates.push(pos);
                }
            }
            candidates.extend(
                text.char_indices()
                    .filter(|(pos, c)| usable_access_char(*c) && Some(*pos) != preferred)
                    .map(|(pos, _)| pos),
            );
            let chosen = candidates.into_iter().find(|pos| {
                text[*pos..]
                    .chars()
                    .next()
                    .is_some_and(|c| !used.contains(&c.to_ascii_lowercase()))
            });
            if let Some(pos) = chosen {
                if let Some(c) = text[pos..].chars().next() {
                    used.insert(c.to_ascii_lowercase());
                }
            }
            chosen
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 工具行 `>` 菜单、`＋` 派发命令、以及 `View ▸ Zoom` 的三条缩放命令
    /// 都必须是已接线。少一条就变成「点了没反应」的假菜单项。
    /// 标签条右端的字母已确认是 **group 名字**（不是字体缩放按钮），
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

    /// 用户反馈[「"]右上角布局按钮展开内容不对，目前 list 只有 2 个」。
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

    /// Tunnels 视觉基线：配置对话框在三个地方露出——菜单栏 `Tunnels`
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
            .expect("Tool 逐项里要有 Tunnels（翻译表原文不带省略号）");
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

    /// 视觉基线：Session 菜单的快捷键列与访问字母下划线。
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
        // 金标准 `01-edit-menu.png` 给 Copy 标了 Shift+Del（由映射层派生）。
        assert_eq!(shortcut_text("Text.Copy"), "Shift+Del");
        // 下划线：默认首字母，例外词取词首。
        assert_eq!(access_char_pos("Save Session"), Some(0));
        assert_eq!(access_char_pos("New Window"), Some(4));
        assert_eq!(access_char_pos("Open Recent"), Some(9)); // 上游写法 Open Rece&nt
        assert_eq!(access_char_pos("Save Selection As"), Some(5));
        assert_eq!(access_char_pos("Close All Views"), Some(6));
        assert_eq!(access_char_pos("Close Window"), Some(6));
        // Log 子菜单的两个例外：避开重复的 O。
        assert_eq!(access_char_pos("Open Log File"), Some(5));
        assert_eq!(access_char_pos("Open Log Folder"), Some(9));
        // View ▸ Hex 三行：避开重复的 H，划第二个词的首字母。
        assert_eq!(access_char_pos("Hex Base"), Some(4));
        assert_eq!(access_char_pos("Hex Columns"), Some(4));
        assert_eq!(access_char_pos("Hex Group Chars"), Some(4));
    }

    /// 视觉基线：Log 子菜单是四组，文案比旧版短；命令 ID 一个都不动
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

    /// 上游 `menu.config` 里显式写 `&` 的几处：下划线位置必须与它一致。
    #[test]
    fn original_ampersand_positions_are_preserved() {
        assert_eq!(access_char_pos("Open Recent"), Some(9));
        assert_eq!(access_char_pos("Select To"), Some(7));
        assert_eq!(access_char_pos("Select Column To"), Some(7));
        assert_eq!(access_char_pos("Syntax Schemes"), Some(1));
        assert_eq!(access_char_pos("Window Opacity"), Some(7));
        assert_eq!(access_char_pos("Set Tab Color"), Some(8));
    }

    /// 同一级不能有两个访问键：此前 `Select*` / `Margin*` / `Show*` 等族全划首字。
    #[test]
    fn no_repeated_access_key_in_any_menu_level() {
        fn label(item: &Item) -> Option<&'static str> {
            match item {
                Item::Action { label, .. } | Item::Check { label, .. }
                | Item::Submenu { label, .. } => Some(label),
                Item::Separator => None,
            }
        }
        fn walk(items: &[Item], path: &str) {
            let rows: Vec<&'static str> = items.iter().filter_map(label).collect();
            let positions = access_positions_unique(&rows, &rows);
            let mut seen = std::collections::HashSet::new();
            for (row, pos) in rows.iter().zip(positions) {
                let Some(pos) = pos else { continue };
                let c = row[pos..].chars().next().expect("位置来自本行");
                assert!(
                    seen.insert(c.to_ascii_lowercase()),
                    "{path} ▸ {row} 的访问键 {c} 与同级重复"
                );
            }
            for item in items {
                if let Item::Submenu { label, items } = item {
                    walk(items, &format!("{path} ▸ {label}"));
                }
            }
        }
        let tree = menus();
        let top: Vec<&'static str> = tree.iter().map(|m| m.label).collect();
        let positions = access_positions_unique(&top, &top);
        let mut seen = std::collections::HashSet::new();
        for (menu, pos) in top.iter().zip(positions) {
            if let Some(pos) = pos {
                let c = menu[pos..].chars().next().expect("位置来自本行");
                assert!(seen.insert(c.to_ascii_lowercase()), "{menu} 的顶级访问键重复");
            }
        }
        for menu in &tree {
            walk(&menu.items, menu.label);
        }
    }

    /// 中文按**显示的字**去重，不再一律划首字。
    #[test]
    fn translated_rows_get_distinct_access_keys() {
        let keys = ["Select", "Select To", "Select Column To"];
        let shown = ["选择", "选择到", "选择列到"];
        let positions = access_positions_unique(&shown, &keys);
        let chars: Vec<char> = positions
            .iter()
            .zip(shown)
            .filter_map(|(pos, text)| pos.map(|p| text[p..].chars().next().expect("位置有效")))
            .collect();
        assert_eq!(chars.len(), 3);
        let unique: std::collections::HashSet<char> = chars.into_iter().collect();
        assert_eq!(unique.len(), 3, "中文同级也要不重复");
    }
}

#[cfg(test)]
#[path = "menu_check_tests.rs"]
mod check_tests;

#[cfg(test)]
#[path = "menu_check_shortcut_tests.rs"]
mod check_shortcut_tests;

#[cfg(test)]
#[path = "menu_context_ids_tests.rs"]
mod context_ids_tests;

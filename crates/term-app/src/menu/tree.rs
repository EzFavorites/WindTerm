//! 菜单树本体：10 棵顶级菜单的数据表。
//!
//! 从 `menu.rs` 搬出来的最大一段（约 860 行），只有 `menus()` 一个函数。
//! 18 个空子菜单由 `menu_submenus.rs::fill` 在出口前补齐结构（W2），
//! 所以这里写的 `vec![]` / `vec![Item::Separator]` 是**结构占位标记**，不是空菜单。

use super::submenus;
use super::{CheckState, Item, Menu};

/// 10 棵顶级菜单。
///
/// W2：数据表里 18 个子菜单仍写作 `vec![]` / `vec![Item::Separator]`
/// 作为「结构占位」标记，出口前统一由 `submenus::fill` 换成真条目。
/// 这样父级位次与子项明细各在一处，逐张比对时不用在 1000 行表里找子项。
pub fn menus() -> Vec<Menu> {
    let mut tree = vec![
        Menu {
            label: "Session",
            items: vec![
                Item::Action {
                    label: "New Session",
                    id: "Terminal.NewSession",
                },
                Item::Action {
                    label: "New Window",
                    id: "Application.NewWindow",
                },
                Item::Separator,
                Item::Action {
                    label: "Disconnect Session",
                    id: "Terminal.DisconnectSession",
                },
                Item::Action {
                    label: "Reconnect Session",
                    id: "Terminal.ReconnectSession",
                },
                // 视觉基线 `01-session-menu.png`：Disconnect / Reconnect /
                // Open Session... / Open Recent 是同一组，中间不插分隔线。
                Item::Action {
                    label: "Open Session",
                    id: "Terminal.OpenSession",
                },
                Item::Submenu {
                    label: "Open Recent",
                    items: vec![],
                },
                Item::Separator,
                Item::Action {
                    label: "Save Session",
                    id: "Terminal.Save",
                },
                Item::Action {
                    label: "Save Selection As",
                    id: "Terminal.SaveSelectionAs",
                },
                Item::Separator,
                Item::Submenu {
                    // 2026-11-08 视觉基线：四组 [Start...] [Stop/Pause/Resume]
                    // [Open Log File/Folder] [Settings...]，命令 ID 一律不动。
                    label: "Log",
                    items: vec![
                        Item::Action {
                            label: "Start",
                            id: "Terminal.StartLogger",
                        },
                        Item::Separator,
                        Item::Action {
                            label: "Stop",
                            id: "Terminal.StopLogger",
                        },
                        Item::Action {
                            label: "Pause",
                            id: "Terminal.PauseLogger",
                        },
                        Item::Action {
                            label: "Resume",
                            id: "Terminal.ResumeLogger",
                        },
                        Item::Separator,
                        Item::Action {
                            label: "Open Log File",
                            id: "Terminal.OpenLogFile",
                        },
                        Item::Action {
                            label: "Open Log Folder",
                            id: "Terminal.OpenLogFolder",
                        },
                        Item::Separator,
                        Item::Action {
                            label: "Settings",
                            id: "Terminal.OpenLogSettings",
                        },
                    ],
                },
                Item::Separator,
                Item::Submenu {
                    label: "Preferences",
                    items: vec![
                        Item::Action {
                            label: "Settings",
                            id: "Window.OpenSettings",
                        },
                        Item::Action {
                            label: "Session Settings",
                            id: "Terminal.OpenSessionSettings",
                        },
                        Item::Action {
                            label: "Default Session Settings",
                            id: "Terminal.OpenDefaultSessionSettings",
                        },
                        Item::Separator,
                        Item::Action {
                            label: "Profiles Directory",
                            id: "Application.OpenProfileDialog",
                        },
                    ],
                },
                Item::Separator,
                Item::Action {
                    label: "Close View",
                    id: "Window.CloseActiveView",
                },
                Item::Action {
                    label: "Close All Views",
                    id: "Window.CloseAllViews",
                },
                Item::Action {
                    label: "Close Window",
                    id: "Window.Close",
                },
                Item::Separator,
                Item::Action {
                    label: "Exit",
                    id: "Application.Exit",
                },
            ],
        },
        Menu {
            label: "Edit",
            items: vec![
                Item::Action {
                    label: "Copy",
                    id: "Text.Copy",
                },
                Item::Action {
                    label: "Paste",
                    id: "Text.Paste",
                },
                Item::Action {
                    label: "Paste Selected Text",
                    id: "Terminal.PasteSelectedText",
                },
                Item::Separator,
                Item::Action {
                    label: "Copy Session Name",
                    id: "Terminal.CopySessionName",
                },
                Item::Action {
                    label: "Copy Session Url",
                    id: "Terminal.CopySessionUrl",
                },
                Item::Separator,
                Item::Submenu {
                    label: "IntelliSense",
                    items: vec![Item::Action {
                        label: "Complete History",
                        id: "Terminal.CompleteHistory",
                    }],
                },
            ],
        },
        Menu {
            label: "Search",
            items: vec![
                // 视觉基线 `01-search-menu.png` 上这两条是带省略号的，但真源头不
                // 是截图：`windterm_en_zh.tsv`（1430 条官方英中对照）两侧一条
                // 省略号都没有。省略号是当初照英文截图抄进菜单、又顺手抄进译文
                // 表时加上的，2026-11-14 一并去掉（见 `i18n/zh.rs` 顶部注记）。
                Item::Action {
                    label: "Find...",
                    id: "Text.Find",
                },
                Item::Submenu {
                    label: "Search Online",
                    items: vec![Item::Action {
                        label: "Search Online",
                        id: "Text.SearchOnline",
                    }],
                },
                Item::Separator,
                Item::Action {
                    label: "Find Next",
                    id: "Text.FindNext",
                },
                Item::Action {
                    label: "Find Previous",
                    id: "Text.FindPrevious",
                },
                Item::Action {
                    label: "Find Next Selection",
                    id: "Text.FindNextSelection",
                },
                Item::Action {
                    label: "Find Previous Selection",
                    id: "Text.FindPreviousSelection",
                },
                Item::Separator,
                Item::Action {
                    label: "Highlight Text...",
                    id: "Text.HighlightText",
                },
            ],
        },
        Menu {
            label: "Selection",
            items: vec![
                Item::Submenu {
                    label: "Select",
                    items: vec![
                        Item::Action {
                            label: "Select All",
                            id: "Text.SelectAll",
                        },
                        Item::Separator,
                        Item::Action {
                            label: "Select Word",
                            id: "Text.SelectWord",
                        },
                        Item::Action {
                            label: "Select Line",
                            id: "Text.SelectLine",
                        },
                        Item::Separator,
                        Item::Action {
                            label: "Select Command",
                            id: "Text.SelectCommand",
                        },
                    ],
                },
                Item::Submenu {
                    label: "Select To",
                    items: vec![
                        Item::Action {
                            label: "Select To Previous Char",
                            id: "Text.SelectToPreviousChar",
                        },
                        Item::Action {
                            label: "Select To Previous Word Start",
                            id: "Text.SelectToPreviousWordStart",
                        },
                        Item::Action {
                            label: "Select To Previous Word End",
                            id: "Text.SelectToPreviousWordEnd",
                        },
                        Item::Action {
                            label: "Select To Previous Line",
                            id: "Text.SelectToPreviousLine",
                        },
                        Item::Action {
                            label: "Select To Previous Page",
                            id: "Text.SelectToPreviousPage",
                        },
                        Item::Separator,
                        Item::Action {
                            label: "Select To Next Char",
                            id: "Text.SelectToNextChar",
                        },
                        Item::Action {
                            label: "Select To Next Word Start",
                            id: "Text.SelectToNextWordStart",
                        },
                        Item::Action {
                            label: "Select To Next Word End",
                            id: "Text.SelectToNextWordEnd",
                        },
                        Item::Action {
                            label: "Select To Next Line",
                            id: "Text.SelectToNextLine",
                        },
                        Item::Action {
                            label: "Select To Next Page",
                            id: "Text.SelectToNextPage",
                        },
                        Item::Separator,
                        Item::Action {
                            label: "Select To Line Home",
                            id: "Text.SelectToLineHome",
                        },
                        Item::Action {
                            label: "Select To Document Start",
                            id: "Text.SelectToDocumentStart",
                        },
                        Item::Separator,
                        Item::Action {
                            label: "Select To Line End",
                            id: "Text.SelectToLineEnd",
                        },
                        Item::Action {
                            label: "Select To Document End",
                            id: "Text.SelectToDocumentEnd",
                        },
                    ],
                },
                Item::Submenu {
                    label: "Select Column To",
                    items: vec![Item::Separator],
                },
                // 视觉基线 `01-selection-menu.png`：三个子菜单之后是九条平铺项，
                // 分四组（激活 / 锁·丢·取消 / 拆分·反转 / 流与列互换·交换）。
                // W2 起这九条**已接线**（不再是灰显占位）：集合模型在
                // `selection.rs` 的 `Selections`，落地在 `apply_cmd`。
                // 语义按终端侧定，不照抄原程序的文本编辑器模型——见
                // `docs/design/03-selection-find-scrollback.md` 的选区语义定义。
                Item::Separator,
                Item::Action {
                    label: "Activate Next Selection",
                    id: "Text.ActivateNextSelection",
                },
                Item::Action {
                    label: "Activate Previous Selection",
                    id: "Text.ActivatePreviousSelection",
                },
                Item::Separator,
                Item::Action {
                    label: "Lock Selection",
                    id: "Text.LockSelection",
                },
                Item::Action {
                    label: "Drop Selection",
                    id: "Text.DropSelection",
                },
                Item::Action {
                    label: "Cancel Selection",
                    id: "Text.CancelSelection",
                },
                Item::Separator,
                Item::Action {
                    label: "Split Selection",
                    id: "Text.SplitSelection",
                },
                Item::Action {
                    label: "Invert Selection",
                    id: "Text.InvertSelection",
                },
                Item::Separator,
                Item::Action {
                    label: "Switch Stream And Column",
                    id: "Text.SwitchStreamAndColumn",
                },
                Item::Action {
                    label: "Exchange Caret And Anchor",
                    id: "Text.ExchangeCaretAndAnchor",
                },
            ],
        },
        Menu {
            label: "Goto",
            items: vec![
                // D3：两件事此前共用一条命令 ID。`Goto Address Bar` 现在真的是
                // 地址栏（连接目标输入框），`Command Line` 才是那个「把一行字
                // 当 shell 命令发出去」的工作台页——两行并排，名字各自说清自己。
                Item::Action {
                    label: "Goto Address Bar",
                    id: "Window.GotoAddressBar",
                },
                Item::Action {
                    label: "Command Line",
                    id: "Window.GotoCommandLine",
                },
                Item::Separator,
                Item::Action {
                    label: "Goto Session...",
                    id: "Window.GotoSession",
                },
                Item::Action {
                    label: "Goto Position...",
                    id: "Window.GotoLine",
                },
                Item::Action {
                    label: "Goto Symbol...",
                    id: "Window.GotoSymbol",
                },
                Item::Separator,
                // 视觉基线：Goto Symbol 之后是四条光标移动（Char Left/Right、
                // Line Up/Down），再进入 Word… 子菜单组。
                Item::Action {
                    label: "Char Left",
                    id: "Text.CharLeft",
                },
                Item::Action {
                    label: "Char Right",
                    id: "Text.CharRight",
                },
                Item::Action {
                    label: "Line Up",
                    id: "Text.LineUp",
                },
                Item::Action {
                    label: "Line Down",
                    id: "Text.LineDown",
                },
                Item::Separator,
                Item::Submenu {
                    label: "Word",
                    items: vec![Item::Separator],
                },
                Item::Submenu {
                    label: "Page",
                    items: vec![Item::Separator],
                },
                // 视觉基线 `01-goto-menu.png`：Word / Page / Paragraph / Command /
                // Fold / Scope / Indentation 是连续七个子菜单，然后才是 Pair。
                // 子项命令尚未接线，先补出父级结构与分组，避免「少了一半入口」。
                Item::Submenu {
                    label: "Paragraph",
                    items: vec![Item::Separator],
                },
                Item::Submenu {
                    label: "Command",
                    items: vec![Item::Separator],
                },
                Item::Submenu {
                    label: "Fold",
                    items: vec![Item::Separator],
                },
                Item::Submenu {
                    label: "Scope",
                    items: vec![Item::Separator],
                },
                Item::Submenu {
                    label: "Indentation",
                    items: vec![Item::Separator],
                },
                Item::Submenu {
                    label: "Pair",
                    items: vec![Item::Separator],
                },
                Item::Submenu {
                    label: "Line",
                    items: vec![Item::Separator],
                },
                Item::Submenu {
                    label: "Document",
                    items: vec![Item::Separator],
                },
            ],
        },
        Menu {
            label: "View",
            items: vec![
                Item::Action {
                    label: "Anything Palette...",
                    id: "Window.ShowPaletteAnything",
                },
                Item::Action {
                    label: "Command Palette...",
                    id: "Window.ShowPaletteCommand",
                },
                Item::Separator,
                // 视觉基线 `01-view-menu.png`：三个 Hex 子菜单（Base / Columns /
                // Group Chars）在 palette 之后、Bars 之前。子项命令未接线，
                // 先补父级结构，避免「少了一半入口」。
                Item::Submenu {
                    label: "Hex Base",
                    items: vec![Item::Separator],
                },
                Item::Submenu {
                    label: "Hex Columns",
                    items: vec![Item::Separator],
                },
                Item::Submenu {
                    label: "Hex Group Chars",
                    items: vec![Item::Separator],
                },
                Item::Separator,
                Item::Submenu {
                    label: "Bars",
                    items: vec![
                        Item::Action {
                            label: "Toggle Menu Bar",
                            id: "Window.ToggleMenuBar",
                        },
                        Item::Action {
                            label: "Toggle Status Bar",
                            id: "Window.ToggleStatusBar",
                        },
                        Item::Submenu {
                            label: "Toolbar",
                            items: vec![
                                Item::Action {
                                    label: "Toolbar List",
                                    id: "Window.ToolbarList",
                                },
                                Item::Separator,
                                Item::Action {
                                    label: "Toolbar Customize",
                                    id: "Window.ToolbarCustomize",
                                },
                            ],
                        },
                        Item::Separator,
                        Item::Action {
                            label: "Toggle Quick Bar",
                            id: "Window.ToggleQuickBar",
                        },
                    ],
                },
                Item::Submenu {
                    label: "Panes",
                    items: vec![
                        // 2026-11-07：整带开关。菜单栏右侧那个布局按钮的下拉
                        // 就是这三条——同一个命令两个入口，不是两套实现。
                        // 2026-11-17 补底带：原先只有左/右，底带（Sender /
                        // Shell / Script）没有整带收起的入口。
                        Item::Action {
                            label: "Toggle Left Docks",
                            id: "Window.ToggleLeftDocks",
                        },
                        Item::Action {
                            label: "Toggle Right Docks",
                            id: "Window.ToggleRightDocks",
                        },
                        Item::Action {
                            label: "Toggle Bottom Docks",
                            id: "Window.ToggleBottomDocks",
                        },
                        // 2026-11-17：隧道总线开关的第二个入口（菜单栏 Tunnels
                        // 下拉是第一个）。关掉 = 全停、配置不丢。
                        Item::Action {
                            label: "Toggle Tunnels",
                            id: "Window.ToggleTunnels",
                        },
                        Item::Separator,
                        Item::Action {
                            label: "Toggle Sidebar",
                            id: "Window.ToggleSidebar",
                        },
                        Item::Action {
                            label: "Toggle Pane Explorer",
                            id: "Window.TogglePaneExplorer",
                        },
                        Item::Action {
                            label: "Toggle Pane Filer",
                            id: "Window.TogglePaneFiler",
                        },
                        Item::Action {
                            label: "Toggle Pane Outline",
                            id: "Window.TogglePaneOutline",
                        },
                        Item::Action {
                            label: "Toggle Pane Sender",
                            id: "Window.TogglePaneSender",
                        },
                        Item::Action {
                            label: "Toggle Pane Session",
                            id: "Window.TogglePaneSession",
                        },
                        Item::Action {
                            label: "Toggle Pane Shell",
                            id: "Window.TogglePaneShell",
                        },
                        // 2026-11-17：脚本停靠面板早就在 `PanelId::ALL` 里（齿轮菜单
                        // 遍历那张表，所以齿轮菜单里一直有 Script 行），但**公共的
                        // `Window ▸ Panes` 里漏了它**——从菜单栏打不开。补上这一条，
                        // 两个入口指向同一个 `TogglePanel(PanelId::Script)`。
                        Item::Action {
                            label: "Toggle Pane Script",
                            id: "Window.TogglePaneScript",
                        },
                        Item::Action {
                            label: "Toggle Pane Transfer",
                            id: "Window.TogglePaneTransfer",
                        },
                        // tmux 控制模式浮层的真实入口。
                        // `01-window-menu.png` 没有展开 Panes 子菜单，
                        // 这一组本来就是我们自己按 dock 面板命名的；
                        // 加这一条不破坏视觉基线的可见结构，却让 `show_tmux`
                        // 从「只有截图夹具设过」变成产品里点得到。
                        Item::Action {
                            label: "Tmux Control Mode",
                            id: "Window.TogglePaneTmux",
                        },
                    ],
                },
                Item::Separator,
                // 2026-11-23 原版 `01-view-menu.png`：Panes 与 Syntax Schemes
                // 之间有一条分隔线（旧图判成连排，已纠正）。
                Item::Submenu {
                    label: "Syntax Schemes",
                    items: vec![Item::Action {
                        label: "Scheme List",
                        id: "Terminal.SchemeList",
                    }],
                },
                Item::Separator,
                // 视觉基线 `01-view-menu.png`：这一条带框且**独占一行**，
                // 是全菜单唯一的勾选项。
                //
                // 2026-11-13：它不再是灰的。`Text.HighlightCaretLine` 现在是真的
                // 开关（`ShellAction::Marks(MarksKnob::CaretLine)` → 翻活动标签的
                // `Tab.marks.caret_line`，像素在 `ui/terminal/view.rs` 铺在字形
                // 之下）。所以勾改成 `Live`：勾不勾读当前标签，而**不是**抄截图
                // 里那一刻的状态——抄常量就变成「永远空框」（之前那版）或
                // 「永远打勾」，两种都是假反馈。
                Item::Check {
                    label: "Highlight Caret Line",
                    id: "Text.HighlightCaretLine",
                    checked: CheckState::Live,
                },
                Item::Separator,
                // 2026-11-07：margin 边栏。之前把「边距」当成编辑器语义删掉过，
                // 现在它有真实实现（行号 / 符号 / 时间戳三列都从内核快照算），
                // 所以按那个判据重新加回来：**菜单项跟着能力走**。
                //
                // 2026-11-18：`Fold` 这一项不再是缺口——
                // · `Margin Fold`（`Text.MarginFold`）折的是光标所在的块（折叠族第 9 条）；
                // · `Margin Fold Column`（`Text.MarginFoldColumn`）才是「画不画折叠
                //   标记列」，画的是折叠头，行号取绝对历史行（与 `folded_view` 同源）。
                // 两个 ID 只差一个词，所以各自一行、各自解释，不做合并。
                Item::Submenu {
                    label: "Margins",
                    items: vec![
                        Item::Action {
                            label: "Margin Blank",
                            id: "Text.MarginBlank",
                        },
                        Item::Action {
                            label: "Margin Number",
                            id: "Text.MarginNumber",
                        },
                        Item::Action {
                            label: "Margin Fold",
                            id: "Text.MarginFold",
                        },
                        Item::Action {
                            label: "Margin Fold Column",
                            id: "Text.MarginFoldColumn",
                        },
                        Item::Action {
                            label: "Margin Symbol",
                            id: "Text.MarginSymbol",
                        },
                        Item::Action {
                            label: "Margin Timestamp",
                            id: "Text.MarginTimestamp",
                        },
                    ],
                },
                Item::Submenu {
                    label: "Symbols",
                    items: vec![Item::Separator],
                },
                // 视觉基线：Margins / Symbols / White Spaces / Folds / Zoom /
                // Window Opacity 连续排布，中间不插线。
                Item::Submenu {
                    label: "White Spaces",
                    items: vec![Item::Separator],
                },
                Item::Separator,
                Item::Submenu {
                    label: "Folds",
                    items: vec![Item::Separator],
                },
                Item::Separator,
                Item::Submenu {
                    label: "Zoom",
                    items: vec![
                        Item::Action {
                            label: "Zoom In",
                            id: "Text.ZoomIn",
                        },
                        Item::Action {
                            label: "Zoom Out",
                            id: "Text.ZoomOut",
                        },
                        Item::Separator,
                        Item::Action {
                            label: "Reset Zoom",
                            id: "Text.ResetZoom",
                        },
                    ],
                },
                Item::Submenu {
                    label: "Window Opacity",
                    items: vec![
                        Item::Action {
                            label: "Set Window Transparent",
                            id: "Application.SetWindowTransparent",
                        },
                        Item::Action {
                            label: "Set Window Translucent",
                            id: "Application.SetWindowTranslucent",
                        },
                        Item::Action {
                            label: "Set Window Opaque",
                            id: "Application.SetWindowOpaque",
                        },
                        Item::Separator,
                        Item::Action {
                            label: "Set Window Opacity",
                            id: "Application.SetWindowOpacity",
                        },
                    ],
                },
            ],
        },
        Menu {
            label: "Mode",
            // 2026-11-08 按 `01-menu-bar/mode/01-mode-menu.png` 逐字重排：
            // 文案去掉了旧版的 `Set` / `Toggle` 前缀（视觉基线里没有这些动词），
            // `Remote Mode` / `Word Wrap` / `Virtual Space` / `Multiple Caret`
            // 四条是**可勾选**行——见 `Item::Check` 的口径表。
            //
            // 这四条的 `checked` 都是 `CheckState::Fixed`：勾或空框都是**照视觉基线
            // 画死的**，读的不是运行期状态（图里 `Remote Mode` / `Word Wrap` 勾着，
            // `Virtual Space` / `Multiple Caret` 空着）。所以「行是灰的（点不动）
            // + 框是勾着的」并不自相矛盾——视觉基线里这几行本来就这么画，而它们
            // 对应的动作还没接线，点不动。
            // 判据在 `crates/term-app/src/menu_check_tests.rs` 的
            // `a_wired_command_must_read_live_state`：双向断言
            // `Fixed ⇒ !capability::is_implemented(id)` 与
            // `Live ⇒ capability::is_implemented(id)`。要改这里的 `Fixed`，得先
            // 让那条命令真的接线，否则测试会红。
            items: vec![
                Item::Check {
                    label: "Remote Mode",
                    id: "Window.SetRemoteMode",
                    checked: CheckState::Live,
                },
                Item::Action {
                    label: "Local Mode",
                    id: "Window.SetLocalMode",
                },
                Item::Action {
                    label: "Hex Edit",
                    id: "Text.ToggleHexEdit",
                },
                Item::Check {
                    label: "Word Wrap",
                    id: "Text.ToggleWordWrap",
                    checked: CheckState::Live,
                },
                Item::Check {
                    label: "Virtual Space",
                    id: "Text.ToggleVirtualSpace",
                    checked: CheckState::Fixed(false),
                },
                Item::Check {
                    label: "Multiple Caret",
                    id: "Text.ToggleMultipleCaret",
                    checked: CheckState::Fixed(true),
                },
                Item::Action {
                    label: "Focus Mode",
                    id: "Window.ToggleFocusMode",
                },
                Item::Action {
                    label: "Focus Mode Settings...",
                    id: "Window.OpenFocusModeSettings",
                },
                Item::Action {
                    label: "Full Screen",
                    id: "Window.ToggleFullScreen",
                },
                Item::Action {
                    label: "Always On Top",
                    id: "Window.ToggleAlwaysOnTop",
                },
                Item::Action {
                    label: "Lock Screen",
                    id: "Window.LockScreen",
                },
                Item::Action {
                    label: "Debug Mode",
                    id: "Window.ToggleDebugMode",
                },
            ],
        },
        Menu {
            label: "Tool",
            items: vec![
                // 视觉基线 `01-tool-menu.png`：前四项带省略号，无分隔线。
                // `Tunnel Settings` 是本实现多出的一行（隧道配置编辑器入口），
                // 保留它是因为 `Terminal.ManageTunnels` 面板没有别的菜单入口。
                Item::Action {
                    label: "Sync Input...",
                    id: "Terminal.SyncInput",
                },
                Item::Action {
                    label: "OneKeys...",
                    id: "Terminal.ManageOneKeys",
                },
                Item::Action {
                    label: "Triggers...",
                    id: "Terminal.ManageTriggers",
                },
                Item::Action {
                    label: "Tunnels...",
                    id: "Terminal.ManageTunnels",
                },
                Item::Action {
                    label: "Tunnel Settings",
                    id: "Terminal.TunnelSettings",
                },
                Item::Submenu {
                    label: "Transfer Binary",
                    items: vec![
                        Item::Action {
                            label: "Send XModem",
                            id: "Terminal.SendXModem",
                        },
                        Item::Action {
                            label: "Send YModem",
                            id: "Terminal.SendYModem",
                        },
                        Item::Action {
                            label: "Send ZModem",
                            id: "Terminal.SendZModem",
                        },
                        Item::Separator,
                        Item::Action {
                            label: "Receive XModem",
                            id: "Terminal.ReceiveXModem",
                        },
                        Item::Action {
                            label: "Receive YModem",
                            id: "Terminal.ReceiveYModem",
                        },
                        Item::Action {
                            label: "Receive ZModem",
                            id: "Terminal.ReceiveZModem",
                        },
                        Item::Separator,
                        Item::Action {
                            label: "Cancel Modem",
                            id: "Terminal.CancelModem",
                        },
                    ],
                },
            ],
        },
        Menu {
            label: "Window",
            items: vec![
                Item::Action {
                    label: "Split View Horizontally",
                    id: "Window.SplitViewHorizontally",
                },
                Item::Action {
                    label: "Split View Vertically",
                    id: "Window.SplitViewVertically",
                },
                Item::Action {
                    label: "Split Group Horizontally",
                    id: "Window.SplitGroupHorizontally",
                },
                Item::Action {
                    label: "Split Group Vertically",
                    id: "Window.SplitGroupVertically",
                },
                Item::Submenu {
                    label: "Merge To Group",
                    items: vec![Item::Action {
                        label: "Move Group To Group",
                        id: "Window.MoveGroupToGroup",
                    }],
                },
                Item::Action {
                    label: "Close Group",
                    id: "Window.CloseActiveGroup",
                },
                Item::Separator,
                Item::Action {
                    label: "Activate Next View",
                    id: "Window.ActivateNextView",
                },
                Item::Action {
                    label: "Activate Previous View",
                    id: "Window.ActivatePreviousView",
                },
                Item::Separator,
                Item::Action {
                    label: "Windows Manager...",
                    id: "Window.WindowManager",
                },
                Item::Separator,
                Item::Action {
                    label: "Window List",
                    id: "Window.WindowList",
                },
                Item::Separator,
                // 把浮动 / 移位过的面板全部收回默认停靠位。拖丢之后的确定性出口。
                Item::Action {
                    label: "Reset Layout",
                    id: "Window.ResetLayout",
                },
            ],
        },
        Menu {
            label: "Help",
            items: vec![
                Item::Action {
                    label: "Help",
                    id: "Help.Help",
                },
                Item::Action {
                    label: "About",
                    id: "Help.About",
                },
            ],
        },
    ];
    submenus::fill(&mut tree);
    tree
}

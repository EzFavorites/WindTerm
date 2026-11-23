//! 入口层：命令 ID → `Action`。字符串**只在这里**被解释一次。
//!
//! 这一层只依赖 `vocab`——不认识 `ViewState`，不认识任何 reducer，也不认识
//! `capability`。这条约束是「已接线 / 未接线」判定能成立的前提：
//! `capability::is_implemented(id)` 就是 `Action::parse(id).is_some()`，
//! 如果入口层反过来依赖 `capability`（或依赖读 `capability` 的 `palette`），
//! 判定就会成环——2026-11-12 之前正是 `action → palette → capability → action`。
//!
//! 未接线的命令返回 `None`，而不是「什么也不做」的占位变体：
//! 这样「已实现」与「占位」在类型层面就分开了，菜单据此置灰（见 `capability`）。

use crate::vocab::*;

pub(crate) fn shell_parse(id: &str) -> Option<ShellAction> {
    let action = match id {
        "Terminal.NewSession" => ShellAction::NewSession,
        "Window.OpenSettings" => ShellAction::OpenSettings,
        "Help.About" => ShellAction::ShowAbout,
        "Terminal.DisconnectSession" => ShellAction::DisconnectSession,
        "Terminal.ReconnectSession" => ShellAction::ReconnectSession,
        "Window.CloseActiveView" => ShellAction::CloseActiveView,
        "Window.ToggleMenuBar" => ShellAction::ToggleMenuBar,
        "Window.ToggleStatusBar" => ShellAction::ToggleStatusBar,
        "Window.ToggleQuickBar" => ShellAction::ToggleQuickBar,
        "Window.ToggleSidebar" => ShellAction::ToggleSidebar,
        // 整带开关：三条带各一条。2026-11-17 补上底带——原先注释写着「底带由
        // 它自己的标签条 × 与齿轮菜单管」，但那是**面板级**的关法；整带一起
        // 收起（Sender + Shell + Script）在屏幕上根本没有入口，用户点名要。
        "Window.ToggleLeftDocks" => ShellAction::ToggleDocks(DockZone::Left),
        "Window.ToggleRightDocks" => ShellAction::ToggleDocks(DockZone::Right),
        "Window.ToggleBottomDocks" => ShellAction::ToggleDocks(DockZone::Bottom),
        "Window.TogglePaneFiler" => ShellAction::TogglePaneFiler,
        "Window.TogglePaneExplorer" => ShellAction::TogglePaneExplorer,
        "Window.TogglePaneOutline" => ShellAction::TogglePaneOutline,
        "Window.TogglePaneSender" => ShellAction::TogglePaneSender,
        "Window.TogglePaneSession" => ShellAction::TogglePaneSession,
        "Window.TogglePaneShell" => ShellAction::TogglePaneShell,
        "Window.TogglePaneScript" => ShellAction::TogglePaneScript,
        "Window.TogglePaneTransfer" => ShellAction::TogglePaneTransfer,
        // tmux 浮层的菜单入口（不是 dock 面板，所以没有 PanelId）。
        "Window.TogglePaneTmux" => ShellAction::TogglePaneTmux,
        "Window.ToggleFocusMode" => ShellAction::ToggleFocusMode,
        "Window.ToggleFullScreen" => ShellAction::ToggleFullScreen,
        "Window.ToggleAlwaysOnTop" => ShellAction::ToggleAlwaysOnTop,
        // 调试覆盖层：`Mode ▸ Debug Mode`。它是本仓最后一条「点了屏幕上真的
        // 多出东西」的占位命令——读者是 `ui/terminal/debug_paint.rs`。
        "Window.ToggleDebugMode" => ShellAction::ToggleDebugMode,
        // 2026-11-19：只读查看（`Window ▸ Local Mode`）。此前它在菜单里是
        // `Item::Action` 且解析成 `None` → 一直画成灰的。接线后菜单行改成
        // `Item::Check { Live }`，勾读活动标签的 `Tab.local_mode`。
        "Window.SetLocalMode" => ShellAction::ToggleLocalMode,
        "Window.SetRemoteMode" => ShellAction::SetRemoteMode,
        "Window.ActivateNextView" => ShellAction::ActivateNextView,
        "Window.ActivatePreviousView" => ShellAction::ActivatePreviousView,
        "Terminal.CopySessionName" => ShellAction::CopySessionName,
        "Terminal.CopySessionUrl" => ShellAction::CopySessionUrl,
        "Help.Help" => ShellAction::Help,
        "Text.SearchOnline" => ShellAction::SearchOnline,
        "Text.ZoomIn" => ShellAction::ZoomIn,
        "Text.ZoomOut" => ShellAction::ZoomOut,
        "Text.ResetZoom" => ShellAction::ZoomReset,
        // 新窗口 = 再起一个完整应用实例（新进程），落地在投影层的 OS 集成。
        "Application.NewWindow" => ShellAction::NewWindow,
        "Application.Exit" => ShellAction::Exit,
        "Window.Close" => ShellAction::Close,
        "Window.CloseAllViews" => ShellAction::CloseAllViews,
        "Window.SplitViewHorizontally" => ShellAction::SplitViewHorizontally,
        "Window.SplitViewVertically" => ShellAction::SplitViewVertically,
        "Window.SplitGroupHorizontally" => ShellAction::SplitGroupHorizontally,
        "Window.SplitGroupVertically" => ShellAction::SplitGroupVertically,
        "Window.CloseActiveGroup" => ShellAction::CloseActiveGroup,
        "Window.MoveGroupToGroup" => ShellAction::MoveGroupToGroup,
        "Terminal.SyncInput" => ShellAction::SyncInput,
        "Text.SelectCommand" => ShellAction::SelectCommand,
        "Terminal.ManageTunnels" => ShellAction::ManageTunnels,
        "Terminal.TunnelSettings" => ShellAction::TunnelSettings,
        "Window.ToggleTunnels" => ShellAction::ToggleTunnels,
        // 设置页里的「直达」入口。它们与 `Window.OpenSettings` 是**不同**
        // 的命令，不是同义路径：那一条停在默认页，这几条各自预选一页。
        "Terminal.ManageTriggers" => ShellAction::ManageTriggers,
        "Terminal.ManageOneKeys" => ShellAction::ManageOneKeys,
        "Terminal.ClearRecentUrls" => ShellAction::ClearRecentUrls,
        // 显示模式：菜单 `Mode ▸ Hex Edit`。之前它在这张表外面（解析成 None
        // → 画成灰的），因为「点了没有效果」；现在意图有人接了，不再置灰。
        "Text.ToggleHexEdit" => ShellAction::ToggleHexEdit,
        // `View ▸ Hex Base / Hex Columns / Hex Group Chars`：13 条档位命令。
        // ID 里的数字就是那一档的值，没有第二张映射表——菜单标签改了值，
        // 命令 ID 必然跟着改，两者不可能悄悄错位。
        "View.HexBase2" => ShellAction::HexBase(2),
        "View.HexBase7" => ShellAction::HexBase(7),
        "View.HexBase8" => ShellAction::HexBase(8),
        "View.HexBase10" => ShellAction::HexBase(10),
        "View.HexBase16" => ShellAction::HexBase(16),
        "View.HexColumns8" => ShellAction::HexColumns(8),
        "View.HexColumns16" => ShellAction::HexColumns(16),
        "View.HexColumns24" => ShellAction::HexColumns(24),
        "View.HexColumns32" => ShellAction::HexColumns(32),
        "View.HexGroupChars1" => ShellAction::HexGroupChars(1),
        "View.HexGroupChars2" => ShellAction::HexGroupChars(2),
        "View.HexGroupChars4" => ShellAction::HexGroupChars(4),
        "View.HexGroupChars8" => ShellAction::HexGroupChars(8),
        // `View ▸ Symbols` / `View ▸ White Spaces` / `Text ▸ Highlight Caret Line`：
        // 10 条栅格可视化命令。与上面 hex 那批同一个形状——ID 就是那一档的名字，
        // 不存在第二张「ID → 编号」的映射表。
        //
        // 五档空白模式是**互斥单选**，编号与 `WhitespaceMode::ALL` 一致
        // （0 Hide All / 1 Show All / 2 In Selection / 3 In Line Start /
        // 4 In Line End）。编号对齐由 `parse` 的 `the_ten_marks_commands_...`
        // 与 `marks_tests.rs` 的菜单路径用例两头钉住：这里写错一位，
        // 用户点 `Show All` 会看到 `In Selection` 的画法。
        "View.ToggleLineEndings" => ShellAction::Marks(MarksKnob::LineEndings),
        "View.ToggleColumnMarker" => ShellAction::Marks(MarksKnob::ColumnMarker),
        "View.ToggleIndentGuides" => ShellAction::Marks(MarksKnob::IndentGuides),
        "View.ToggleWrapSymbols" => ShellAction::Marks(MarksKnob::WrapSymbols),
        "View.WhiteSpacesHideAll" => ShellAction::Marks(MarksKnob::Whitespace(0)),
        "View.WhiteSpacesShowAll" => ShellAction::Marks(MarksKnob::Whitespace(1)),
        "View.WhiteSpacesShowInSelection" => ShellAction::Marks(MarksKnob::Whitespace(2)),
        "View.WhiteSpacesShowInLineStart" => ShellAction::Marks(MarksKnob::Whitespace(3)),
        "View.WhiteSpacesShowInLineEnd" => ShellAction::Marks(MarksKnob::Whitespace(4)),
        "Text.HighlightCaretLine" => ShellAction::Marks(MarksKnob::CaretLine),
        // `View ▸ Window Opacity`：三档预设 + 一条「打开设置自己调」。
        // 前三条直接写在 `menu/tree.rs` 里（不进 `menu_submenus.rs` 的 68 条
        // TABLE），所以这里的 ID 是它们唯一的字符串入口；档位值由
        // `WindowOpacityPreset` 自带，没有第二张「ID → 数值」表。
        "Application.SetWindowOpaque" => {
            ShellAction::SetWindowOpacity(WindowOpacityPreset::Opaque)
        }
        "Application.SetWindowTranslucent" => {
            ShellAction::SetWindowOpacity(WindowOpacityPreset::Translucent)
        }
        "Application.SetWindowTransparent" => {
            ShellAction::SetWindowOpacity(WindowOpacityPreset::Transparent)
        }
        "Application.SetWindowOpacity" => ShellAction::ManageWindowOpacity,
        _ => return None,
    };
    Some(action)
}

pub(crate) fn find_parse(id: &str) -> Option<FindAction> {
    let action = match id {
        "Text.Find" => FindAction::Open,
        "Text.FindNext" => FindAction::Next,
        "Text.FindPrevious" => FindAction::Previous,
        "Text.FindNextSelection" => FindAction::SelectionNext,
        "Text.FindPreviousSelection" => FindAction::SelectionPrev,
        _ => return None,
    };
    Some(action)
}

pub(crate) fn clipboard_parse(id: &str) -> Option<ClipboardAction> {
    match id {
        "Text.Copy" => Some(ClipboardAction::Copy),
        "Text.Paste" => Some(ClipboardAction::Paste),
        "Terminal.PasteSelectedText" => Some(ClipboardAction::PasteSelection),
        _ => None,
    }
}

pub(crate) fn log_parse(id: &str) -> Option<LogAction> {
    let action = match id {
        "Terminal.StartLogger" => LogAction::Start,
        "Terminal.StopLogger" => LogAction::Stop,
        "Terminal.PauseLogger" => LogAction::Pause,
        "Terminal.ResumeLogger" => LogAction::Resume,
        "Terminal.OpenLogFile" => LogAction::OpenFile,
        "Terminal.OpenLogFolder" => LogAction::OpenFolder,
        "Terminal.OpenLogSettings" => LogAction::OpenSettings,
        _ => return None,
    };
    Some(action)
}

pub(crate) fn goto_parse(id: &str) -> Option<GotoAction> {
    let action = match id {
        "Window.GotoLine" => GotoAction::Open(GotoMode::Line),
        "Window.GotoSession" => GotoAction::Open(GotoMode::Sessions),
        "Window.GotoSymbol" => GotoAction::Open(GotoMode::Symbol),
        _ => return None,
    };
    Some(action)
}

pub(crate) fn center_parse(id: &str) -> Option<CenterAction> {
    let action = match id {
        "Terminal.OpenSession" => CenterAction::Open(CenterPage::OpenSession),
        "Terminal.Save" => CenterAction::Open(CenterPage::Save),
        "Terminal.SaveSelectionAs" => CenterAction::Open(CenterPage::SaveSelectionAs),
        "Terminal.OpenSessionSettings" => CenterAction::Open(CenterPage::SessionSettings),
        "Terminal.OpenDefaultSessionSettings" => CenterAction::Open(CenterPage::DefaultSettings),
        "Application.OpenProfileDialog" => CenterAction::Open(CenterPage::Profile),
        "Window.ToolbarList" => CenterAction::Open(CenterPage::ToolbarList),
        "Window.ToolbarCustomize" => CenterAction::Open(CenterPage::ToolbarCustomize),
        // 这一条曾指向「地址栏」页，现在指向**命令行**页——它做的本来就是
        // 「把一行字当 shell 命令发出去」。地址栏（连接目标输入框）另有命令 ID。
        "Window.GotoCommandLine" => CenterAction::Open(CenterPage::GotoCommandLine),
        "Terminal.CompleteHistory" => CenterAction::Open(CenterPage::CompleteHistory),
        "Terminal.SchemeList" => CenterAction::Open(CenterPage::SchemeList),
        "Window.WindowList" => CenterAction::Open(CenterPage::WindowList),
        "Window.WindowManager" => CenterAction::Open(CenterPage::WindowManager),
        "Window.OpenFocusModeSettings" => CenterAction::Open(CenterPage::FocusModeSettings),
        "Text.HighlightText" => CenterAction::Open(CenterPage::HighlightText),
        _ => return None,
    };
    Some(action)
}

pub(crate) fn lock_parse(id: &str) -> Option<LockAction> {
    match id {
        "Window.LockScreen" => Some(LockAction::Lock),
        _ => None,
    }
}

pub(crate) fn margin_parse(id: &str) -> Option<MarginAction> {
    let part = match id {
        "Text.MarginBlank" => MarginPart::Blank,
        "Text.MarginNumber" => MarginPart::Number,
        // `Text.MarginFoldColumn`：画不画**折叠标记列**（2026-11-18 接线）。
        "Text.MarginFoldColumn" => MarginPart::Fold,
        "Text.MarginSymbol" => MarginPart::Symbol,
        "Text.MarginTimestamp" => MarginPart::Timestamp,
        // `Text.MarginFold` **不在这里**：它翻的是「折哪个块」而不是「画不画那一列」，
        // 见 `fold_parse` 与 `vocab::FoldCmd`。两个 ID 只差 `Column` 一个词，
        // 混进同一条解析就等于把「折哪个块」写成「画不画那一列」。
        _ => return None,
    };
    Some(MarginAction::Toggle(part))
}

/// 折叠族九条：`View ▸ Folds` 六条 + `Text ▸ Margin Fold` +
/// `Goto ▸ Fold ▸ Previous / Next Fold`。
///
/// 九条 ID 只在 `"ID" => …` 分支里出现（判据按这个形状判），
/// 语义与取舍写在 [`FoldCmd`] 各变体的文档里，这里只管 ID → 意图。
///
/// 九条**同一张表**，不是按菜单归属分三处写：它们的落地是同一条
/// `Effect::Fold` → `ui/effects.rs::apply_fold`，拆开只会让「九条都接了吗」
/// 变成三处各自回答的问题。
pub(crate) fn fold_parse(id: &str) -> Option<FoldCmd> {
    let cmd = match id {
        "View.Fold" => FoldCmd::Fold,
        "View.Unfold" => FoldCmd::Unfold,
        "View.FoldInside" => FoldCmd::FoldInside,
        "View.UnfoldInside" => FoldCmd::UnfoldInside,
        "View.FoldAll" => FoldCmd::FoldAll,
        "View.UnfoldAll" => FoldCmd::UnfoldAll,
        "Text.MarginFold" => FoldCmd::Toggle,
        "Text.GotoPreviousFold" => FoldCmd::Previous,
        "Text.GotoNextFold" => FoldCmd::Next,
        _ => return None,
    };
    Some(cmd)
}

pub(crate) fn tab_parse(id: &str) -> Option<TabAction> {
    Some(match id {
        "Terminal.TabActivate" => TabAction::Activate { target: None },
        "Terminal.TabClose" => TabAction::Close { target: None },
        "Terminal.TabCloseOthers" => TabAction::CloseOthers { target: None },
        "Terminal.TabCloseAll" => TabAction::CloseAll,
        "Terminal.TabReopenClosed" => TabAction::Reopen,
        "Terminal.TabDuplicate" => TabAction::Duplicate { target: None },
        "Terminal.TabSplit" => TabAction::Split { horizontal: true },
        "Terminal.TabCopyName" => TabAction::CopyName,
        "Terminal.TabCopyUrl" => TabAction::CopyUrl,
        "Terminal.TabColor" => TabAction::Color {
            target: None,
            slot: None,
        },
        _ => return None,
    })
}

pub(crate) fn dock_parse(id: &str) -> Option<LayoutAction> {
    match id {
        "Window.ResetLayout" => Some(LayoutAction::Reset),
        _ => None,
    }
}

pub(crate) fn selection_parse(id: &str) -> Option<Op> {
    let op = match id {
        "Text.SelectAll" => Op::All,
        "Text.SelectWord" => Op::Word,
        "Text.SelectLine" => Op::Line,
        "Text.SelectToDocumentStart" => Op::ToDocumentStart,
        "Text.SelectToDocumentEnd" => Op::ToDocumentEnd,
        "Text.SelectToLineHome" => Op::ToLineHome,
        "Text.SelectToLineEnd" => Op::ToLineEnd,
        "Text.SelectToPreviousChar" => Op::CharPrev,
        "Text.SelectToNextChar" => Op::CharNext,
        "Text.SelectToPreviousLine" => Op::LinePrev,
        "Text.SelectToNextLine" => Op::LineNext,
        "Text.SelectToPreviousPage" => Op::PagePrev,
        "Text.SelectToNextPage" => Op::PageNext,
        "Text.SelectToPreviousWordStart" => Op::WordStartPrev,
        "Text.SelectToNextWordStart" => Op::WordStartNext,
        "Text.SelectToPreviousWordEnd" => Op::WordEndPrev,
        "Text.SelectToNextWordEnd" => Op::WordEndNext,
        // Goto 区里那四条光标移动：语义就是「挪活动选区的 head」，与 Shift+方向键
        // 完全同一条位移规则，所以复用同一批 `Op`，不另立一套。
        "Text.CharLeft" => Op::CharPrev,
        "Text.CharRight" => Op::CharNext,
        "Text.LineUp" => Op::LinePrev,
        "Text.LineDown" => Op::LineNext,
        // Goto 另外 10 条。同样是「挪 head」，同样没有新位移规则——词首 / 词尾 /
        // 翻页 / 行首行尾 / 文首文末在 `apply` 里早就各有一个分支（`Text.SelectTo*`
        // 一直在用），这一刀补的只是 ID → `Op` 这一层映射。
        //
        // 为什么不给它们另立「光标」：终端里没有编辑器那套独立光标，只有「一组选区
        // + 一个活动下标」（`action.rs::goto_cursor_commands_move_the_head_…` 钉住了
        // 这个取舍）。另立一个光标就是第二份真相，光标与选区迟早各走各的。
        //
        // 同组里 `Word Part` / `Stuttered Page` / `Paragraph` / `Command` / `Scope` /
        // `Pair` 当时也留灰，共同原因是「终端里的词的一部分 / 一次停顿 / 段落边界
        // 是什么」还没有答案。2026-11-13 全部回答了，逐条落点见下面的分族注释与
        // `selection.rs::every_cursor_command_parses_to_its_own_op`。
        // 唯一**不归这里**的邻居是 `Goto ▸ Fold` 两条：它们移动视口而不是选区，
        // 走 `fold_parse`。
        "Text.GotoPreviousWord" => Op::WordStartPrev,
        "Text.GotoPreviousWordEnd" => Op::WordEndPrev,
        "Text.GotoNextWord" => Op::WordStartNext,
        "Text.GotoNextWordEnd" => Op::WordEndNext,
        "Text.GotoPreviousPage" => Op::PagePrev,
        "Text.GotoNextPage" => Op::PageNext,
        "Text.GotoLineHome" => Op::ToLineHome,
        "Text.GotoLineEnd" => Op::ToLineEnd,
        "Text.GotoDocumentStart" => Op::ToDocumentStart,
        "Text.GotoDocumentEnd" => Op::ToDocumentEnd,
        // 2026-11-13：又接通 26 条（词段 / 半页 / 段落 / 命令 / 作用域 / 缩进 /
        // 括号对 + 10 条按列选）。这 26 条与那 10 条不同 —— 是纯映射
        // （复用既有的 17 条 `Op`），这 26 条**各有自己的位移规则**，所以每条都有
        // 独立变体、边界判定落在纯层（`geometry.rs` / `words.rs`）。
        // 逐条语义见 `vocab.rs::Op` 的分族注释与 `selection/ops.rs::apply`。
        //
        // `Text.GotoPreviousFold` / `Text.GotoNextFold` 曾经留在这里当注释的尾巴
        // （「折叠引擎还没建」）。2026-11-14 它们和 `View ▸ Folds` 六条一起接线，
        // 但**没有**映射到 `Op`：`Op` 的定义是「挪活动选区的 head」，而折叠跳转
        // 移动的是视口。见 `fold_parse` 与 `vocab::FoldCmd`。
        //
        // Select Column To（10 条，矩形语义）
        "Text.SelectColumnToPreviousChar" => Op::ColumnCharPrev,
        "Text.SelectColumnToNextChar" => Op::ColumnCharNext,
        "Text.SelectColumnToPreviousLine" => Op::ColumnLinePrev,
        "Text.SelectColumnToNextLine" => Op::ColumnLineNext,
        "Text.SelectColumnToPreviousPage" => Op::ColumnPagePrev,
        "Text.SelectColumnToNextPage" => Op::ColumnPageNext,
        "Text.SelectColumnToPreviousParagraph" => Op::ColumnParagraphPrev,
        "Text.SelectColumnToNextParagraph" => Op::ColumnParagraphNext,
        "Text.SelectColumnToLineHome" => Op::ColumnLineHome,
        "Text.SelectColumnToLineEnd" => Op::ColumnLineEnd,
        // Goto ▸ Word（词段 / 子词）
        "Text.GotoPreviousWordPart" => Op::WordPartPrev,
        "Text.GotoNextWordPart" => Op::WordPartNext,
        // Goto ▸ Page（半页）
        "Text.GotoPreviousStutteredPage" => Op::StutteredPagePrev,
        "Text.GotoNextStutteredPage" => Op::StutteredPageNext,
        // Goto ▸ Paragraph
        "Text.GotoPreviousParagraph" => Op::ParagraphPrev,
        "Text.GotoNextParagraph" => Op::ParagraphNext,
        // Goto ▸ Command / Scope（OSC 133 块表；无块表时前者不动、后者退化到窗口首末行）
        "Text.GotoPreviousCommand" => Op::CommandPrev,
        "Text.GotoNextCommand" => Op::CommandNext,
        "Text.GotoPreviousScope" => Op::ScopePrev,
        "Text.GotoNextScope" => Op::ScopeNext,
        // Goto ▸ Indentation
        "Text.GotoPreviousIndentation" => Op::IndentationPrev,
        "Text.GotoNextIndentation" => Op::IndentationNext,
        // Goto ▸ Pair
        "Text.GotoPairStart" => Op::PairStart,
        "Text.GotoPairEnd" => Op::PairEnd,
        "Text.GotoPreviousPair" => Op::PairPrev,
        "Text.GotoNextPair" => Op::PairNext,
        _ => return None,
    };
    Some(op)
}

// 2026-11-12：`palette::parse` 的两个分支搬到这里。
// 它留在 `palette.rs` 时形成 `action → palette → capability → action` 的三角环：
// 面板要按「是否已接线」过滤条目（那是 `capability` 的职责），而「是否接线」
// 又只能在入口层回答。把入口集中到这一层，环就断了。
pub(crate) fn palette_parse(id: &str) -> Option<PaletteAction> {
    let action = match id {
        "Window.ShowPaletteCommand" => PaletteAction::Open(PaletteMode::Command),
        "Window.ShowPaletteAnything" => PaletteAction::Open(PaletteMode::Anything),
        _ => return None,
    };
    Some(action)
}

pub(crate) fn selection_set_parse(id: &str) -> Option<SetOp> {
    let op = match id {
        "Text.ActivateNextSelection" => SetOp::ActivateNext,
        "Text.ActivatePreviousSelection" => SetOp::ActivatePrevious,
        "Text.LockSelection" => SetOp::Lock,
        "Text.DropSelection" => SetOp::Drop,
        "Text.CancelSelection" => SetOp::Cancel,
        "Text.SplitSelection" => SetOp::Split,
        "Text.InvertSelection" => SetOp::Invert,
        "Text.SwitchStreamAndColumn" => SetOp::SwitchShape,
        "Text.ExchangeCaretAndAnchor" => SetOp::Exchange,
        _ => return None,
    };
    Some(op)
}

/// 传输命令的字符串入口。返回 `None` 的命令在菜单里置灰。
///
/// **五条（含 ZMODEM 两条）都通了**：`Terminal.SendZModem` / `ReceiveZModem`
/// 曾在本层故意返回 `None`（当时 `term-zmodem` 还没有真实读者），现在内核
/// 与真实 `sz` / `rz` 双向互通过了，读者是 `apps/desktop/src/xfer_run.rs` 的
/// 驱动线程——它按 [`XferProtocol::Zmodem`] 建 ZMODEM 引擎。
/// 「能 parse」与「点了有效果」之间那道关由端到端测试守（真 PTY + 真 `sz`），
/// 不在本层。
pub(crate) fn parse_xfer(id: &str) -> Option<XferCmd> {
    Some(match id {
        "Terminal.SendXModem" => XferCmd::Send(XferProtocol::Xmodem),
        "Terminal.ReceiveXModem" => XferCmd::Receive(XferProtocol::Xmodem),
        "Terminal.SendYModem" => XferCmd::Send(XferProtocol::Ymodem),
        "Terminal.ReceiveYModem" => XferCmd::Receive(XferProtocol::Ymodem),
        "Terminal.SendZModem" => XferCmd::Send(XferProtocol::Zmodem),
        "Terminal.ReceiveZModem" => XferCmd::Receive(XferProtocol::Zmodem),
        "Terminal.CancelModem" => XferCmd::Cancel,
        _ => return None,
    })
}

/// 地址栏。`Action::AddressBar` 没有子枚举（见它自己的注释），所以单独
/// 一条 `parse`，形态与本文件其余能力一致——**命令 ID 只允许出现在
/// `"ID" => …` 这种分支里**（`scripts/verify.sh` 4e 的判据按这个形状判）。
pub(crate) fn address_parse(id: &str) -> Option<Action> {
    Some(match id {
        "Window.GotoAddressBar" => Action::AddressBar,
        _ => return None,
    })
}

impl Action {
    /// 命令 ID → 能力动作的唯一入口。未接线的命令返回 `None`。
    pub fn parse(id: &str) -> Option<Action> {
        // 地址栏是本层唯一**没有子枚举**的动作——它没有 reducer，也不产
        // `Effect`（见 `Action::AddressBar` 的注释），所以走自己那个
        // [`address_parse`]。放在链首：它的 ID 与 `center_parse` 里那批
        // `Window.*` 挨着，万一以后有人把那条命令加进中心页表，这里先命中，
        // 语义不会被悄悄改回「打开一页工作台」。
        if let Some(action) = address_parse(id) {
            return Some(action);
        }
        if let Some(a) = shell_parse(id) {
            return Some(Action::Shell(a));
        }
        if let Some(a) = find_parse(id) {
            return Some(Action::Find(a));
        }
        if let Some(a) = clipboard_parse(id) {
            return Some(Action::Clipboard(a));
        }
        if let Some(op) = selection_parse(id) {
            return Some(Action::Select(op));
        }
        if let Some(op) = selection_set_parse(id) {
            return Some(Action::SelectSet(op));
        }
        if let Some(a) = log_parse(id) {
            return Some(Action::Log(a));
        }
        if let Some(a) = palette_parse(id) {
            return Some(Action::Palette(a));
        }
        if let Some(a) = goto_parse(id) {
            return Some(Action::Goto(a));
        }
        if let Some(a) = center_parse(id) {
            return Some(Action::Center(a));
        }
        if let Some(a) = lock_parse(id) {
            return Some(Action::Lock(a));
        }
        if let Some(a) = margin_parse(id) {
            return Some(Action::Margin(a));
        }
        // 2026-11-14：折叠族九条。排在 `margin_parse` **之后**只是读起来顺，
        // 两张表没有交集（`Text.MarginFold` 只在这一张里）；真正的约束是它必须
        // 排在 `selection_parse` 之后——否则将来谁手滑把 `Text.GotoPreviousFold`
        // 写进 `Op` 表，两条路径会抢同一个 ID 而没人发现。
        if let Some(cmd) = fold_parse(id) {
            return Some(Action::Fold(cmd));
        }
        // 2026-11-09：文件传输。X/Y/ZMODEM 七条命令全部有动作，
        // 区别只在驱动层建哪台状态机（见 `parse_xfer` 的注释）。
        if let Some(cmd) = parse_xfer(id) {
            return Some(Action::Xfer(cmd));
        }
        // 标签右键的 10 个影子入口。与菜单命令走同一个 `parse`，
        // `capability` 不需要维护第二份判定。
        if let Some(a) = tab_parse(id) {
            return Some(Action::Tab(a));
        }
        // 布局动作排在最后——它只有一条命令 ID（重置布局），
        // 其余三个动作由拖拽产生，没有字符串入口。
        if let Some(a) = dock_parse(id) {
            return Some(Action::Layout(a));
        }
        // Tmux 控制命令（命令面板的 `Tmux:*` 条目）。无目标命令才进全局面板；
        // 带窗格号的由面板按解析出的集合校验后再发，避免误注入。
        if let Some(line) = tmux_palette::line_for(id) {
            return Some(Action::TmuxCommand(line.to_string()));
        }
        None
    }

    pub fn feature(&self) -> FeatureId {
        match self {
            // 归属看的是**意图**，不是「哪个模块的 parse 认了这条 ID」。
            // 显示模式由 `shell::parse` 收进来（它是最靠近菜单表的那一层），
            // 但它要换的是看字节的方式，与窗口 chrome 无关，所以归 `Bin`。
            Action::Shell(ShellAction::ToggleHexEdit)
            | Action::Shell(ShellAction::HexBase(_))
            | Action::Shell(ShellAction::HexColumns(_))
            | Action::Shell(ShellAction::HexGroupChars(_)) => FeatureId::Bin,
            // 栅格可视化十条：多画了什么，既不是窗口 chrome 也不是「怎么看字节」。
            // **必须排在 `Action::Shell(_)` 之前**——那个 arm 是 Shell 能力的
            // 兜底归属，写在它后面这一条永远轮不到（Rust 的 match 自上而下）。
            Action::Shell(ShellAction::Marks(_)) => FeatureId::Marks,
            Action::Shell(_) => FeatureId::Shell,
            Action::Find(_) => FeatureId::Find,
            Action::Clipboard(_) => FeatureId::Clipboard,
            Action::Select(_) => FeatureId::Selection,
            Action::SelectSet(_) => FeatureId::Selection,
            Action::Log(_) => FeatureId::Log,
            Action::Palette(_) => FeatureId::Palette,
            Action::Goto(_) => FeatureId::Goto,
            // 地址栏归 `Goto`。它和 `Goto Line` / `Goto Session` 是同一类东西
            // ——「跳到某处」的浮层入口（一个跳到回看行，一个跳到标签，一个跳到
            // 一台主机）。归 `Shell` 会让这组入口在能力面上散成两处。
            Action::AddressBar => FeatureId::Goto,
            Action::Center(_) => FeatureId::Center,
            Action::Lock(_) => FeatureId::Lock,
            Action::Layout(_) => FeatureId::Layout,
            Action::Margin(_) => FeatureId::Margin,
            // 折叠族九条归 `Margin`，理由不是「ID 里带 Margin」（九个里只有一个带），
            // 而是**折叠就是 margin 那一列的内容**：`MarginPart::Fold` 说的
            // 「折叠标记列」画的是哪一行被折了，与这九条改的 `Tab.folds` 是同一份
            // 真相的两个读者。拆成新 feature 会让「折叠」在能力面上变成两块
            // （一条命令要同时在两处报进度），而拆到 `Selection` 更错——这九条
            // 一次都不碰选区（见 `FoldCmd` 的文档）。
            Action::Fold(_) => FeatureId::Margin,
            Action::Xfer(_) => FeatureId::Xfer,
            Action::Tab(_) => FeatureId::Tab,
            Action::ImportSessions(_)
            | Action::SaveLayout
            | Action::OpenShortcuts
            | Action::OpenQuickCommands
            | Action::TmuxCommand(_)
            | Action::TmuxFeed(_)
            | Action::QuickCommand(_)
            | Action::QuickGroupToggle(_) => FeatureId::Shell,
        }
    }
}

#[cfg(test)]
#[path = "local_mode_tests.rs"]
mod local_mode_tests;

#[cfg(test)]
mod tests {
    use super::*;

    /// 10 条栅格可视化命令的**入口表**：ID → 旋钮 → 归属能力。
    ///
    /// 这一条与 `apps/desktop/src/ui/terminal/marks_menu_tests.rs` 是一对：
    /// 这里钉「ID 有没有被认出来、认出的是哪一个旋钮」，那边钉「认出来之后
    /// 屏幕上真的变了没有」。缺了任何一半，「点了没反应」都能溜过去——
    /// 只钉前者会得到「命令能点、但写错了旋钮」；只钉后者会得到
    /// 「某个 ID 根本不在这张表里，测试用的却是私有方法」。
    ///
    /// 五档空白模式的编号是这张表里唯一容易错位的一处（`Whitespace(0..=4)`
    /// 必须与 `WhitespaceMode::ALL` 同序），所以逐条写死，不用循环生成：
    /// 循环会跟着实现一起歪，写死的期望值不会。
    #[test]
    fn the_ten_marks_commands_parse_to_their_own_knob_and_feature() {
        let table: [(&str, MarksKnob); 10] = [
            ("View.ToggleLineEndings", MarksKnob::LineEndings),
            ("View.ToggleColumnMarker", MarksKnob::ColumnMarker),
            ("View.ToggleIndentGuides", MarksKnob::IndentGuides),
            ("View.ToggleWrapSymbols", MarksKnob::WrapSymbols),
            ("View.WhiteSpacesHideAll", MarksKnob::Whitespace(0)),
            ("View.WhiteSpacesShowAll", MarksKnob::Whitespace(1)),
            ("View.WhiteSpacesShowInSelection", MarksKnob::Whitespace(2)),
            ("View.WhiteSpacesShowInLineStart", MarksKnob::Whitespace(3)),
            ("View.WhiteSpacesShowInLineEnd", MarksKnob::Whitespace(4)),
            ("Text.HighlightCaretLine", MarksKnob::CaretLine),
        ];
        for (id, knob) in table {
            let action = Action::parse(id)
                .unwrap_or_else(|| panic!("{id} 解析不出动作——菜单里它就是灰的"));
            assert_eq!(
                action,
                Action::Shell(ShellAction::Marks(knob)),
                "{id} 认成了别的动作"
            );
            // 归属必须是 `Marks` 而不是被 `Action::Shell(_)` 那条兜底吞掉：
            // 后者会让这十条散进「窗口 chrome」那一大堆里，能力面数不清。
            assert_eq!(action.feature(), FeatureId::Marks, "{id} 归属错");
            assert!(
                crate::capability::is_implemented(id),
                "{id} 必须在已接线集合里"
            );
        }
        // 五档编号与 `Prefs.view_whitespace` 同序：写错一位的表现是
        // 「点 Show All，画出 In Selection 的样子」，肉眼极难发现。
        assert_eq!(table[4].1, MarksKnob::Whitespace(0), "Hide All 是 0");
        assert_eq!(table[8].1, MarksKnob::Whitespace(4), "In Line End 是 4");
    }

    /// 十条之外没有第二条路径：`View.WhiteSpaces`（不带档位后缀）不是一个命令。
    ///
    /// 菜单里那五个 ID 各带一档，给一个「没有档位的父级 ID」补 arm 只会得到
    /// 「点父级等于点某一档」的隐性默认值——正是 `View ▸ Hex Base` 当初拒掉的形状。
    #[test]
    fn there_is_no_parent_id_for_the_white_spaces_group() {
        for id in [
            "View.WhiteSpaces",
            "View.Symbols",
            "View.ToggleWhiteSpaces",
            "Text.HighlightCaretLineOn",
        ] {
            assert_eq!(Action::parse(id), None, "{id} 不该是一条命令");
        }
    }

    /// 折叠族九条：ID → 意图，**逐条**写死。
    ///
    /// 不用循环：这九条的期望值一共九个不同变体，写成表也就省几行，而漏一条时
    /// 循环版的反例只有 `"{id}"`，还得回去数第几条。判据钉的是「哪一条 ID 认成
    /// 哪一个动作」——`View.FoldAll` 与 `View.Fold` 只差三个字母，混淆一次就是
    /// 「点了 Fold All 只折了一个块」这种半对的行为。
    #[test]
    fn the_nine_fold_commands_parse_to_their_own_variant() {
        assert_eq!(Action::parse("View.Fold"), Some(Action::Fold(FoldCmd::Fold)));
        assert_eq!(
            Action::parse("View.Unfold"),
            Some(Action::Fold(FoldCmd::Unfold))
        );
        assert_eq!(
            Action::parse("View.FoldInside"),
            Some(Action::Fold(FoldCmd::FoldInside))
        );
        assert_eq!(
            Action::parse("View.UnfoldInside"),
            Some(Action::Fold(FoldCmd::UnfoldInside))
        );
        assert_eq!(
            Action::parse("View.FoldAll"),
            Some(Action::Fold(FoldCmd::FoldAll))
        );
        assert_eq!(
            Action::parse("View.UnfoldAll"),
            Some(Action::Fold(FoldCmd::UnfoldAll))
        );
        assert_eq!(
            Action::parse("Text.MarginFold"),
            Some(Action::Fold(FoldCmd::Toggle))
        );
        assert_eq!(
            Action::parse("Text.GotoPreviousFold"),
            Some(Action::Fold(FoldCmd::Previous))
        );
        assert_eq!(
            Action::parse("Text.GotoNextFold"),
            Some(Action::Fold(FoldCmd::Next))
        );
        // 九条都归 `Margin`（折叠就是 margin 折叠标记列的内容，见 `feature()`）。
        assert_eq!(
            Action::parse("View.Fold").map(|a| a.feature()),
            Some(FeatureId::Margin)
        );
        // 反向：折叠子菜单以外没有第二条「没有档位的父级 ID」。
        for id in ["View.Folds", "View.FoldInsideAll", "Text.Fold"] {
            assert_eq!(Action::parse(id), None, "{id} 不该是一条命令");
        }
    }
}

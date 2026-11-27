//! 菜单命令与应用面动词的**分类表**（取代「不暴露清单」）。
//!
//! 每个能点的入口都要回答一个问题：它为什么不在脚本 API 里？
//!
//! - [`ApiClass::Verb`]：有动词表达同一意图（`Window.TogglePaneFiler` →
//!   `app_panel_set`）。菜单侧保留 `Toggle*` 的手感，脚本侧必须用**幂等置位**——
//!   `toggle` 不幂等，重试会翻回去，所以能力面上没有 toggle 动词。
//! - [`ApiClass::Equivalent`]：由既有动词**组合**覆盖（`Text.Copy` → `clipboard_set`）。
//! - [`ApiClass::Interactive`]：必须等人在界面上继续（打开对话框、拖选区、等外部输入）。
//! - [`ApiClass::Data`]：纯展示，没有可读回的状态（About / Help）。
//!
//! 这张表是**穷举**的：`inventory()` 多一条没分类就红，表里多一条查无此项也红
//! （断言 1 / 2）。于是「不暴露多少条」从一份手写清单变成一个可算的余数
//! （断言 5）——余数变了，表格一定跟着变了。
//!
//! # 为什么比对 `find` 而不是 `names()`
//!
//! `names()` 是**远程**投影（只有 17 条）。分类表描述的是「脚本能用的动作」，
//! 而 `app_panel_set` / `clipboard_set` 这类 `Exposure::Script` 的动词都不在它里面；
//! 拿 `names()` 对账会让整张表必红。断言 3 用 `surface::find` + `on_script()`。

/// 一条菜单命令与脚本动作的关系。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ApiClass {
    /// 有同名意图的脚本动词（幂等置位语义）。
    Verb(&'static str),
    /// 由既有动词组合覆盖，名字取其中最核心的那个。
    Equivalent(&'static str),
    /// 只能在界面上由人完成。
    Interactive,
    /// 纯展示，无可读回状态。
    Data,
}

/// 全表。按 id 排序（断言用 `inventory()` 反查，顺序不影响判定，只是好读）。
///
/// 三处**已知的口径选择**，改前先读：
///
/// 1. `Window.Toggle*Docks` 归 `Equivalent("app_panel_set")`——整条带是「带里几块
///    一起置位」，脚本侧用多次 `app_panel_set` 表达，不需要新动词。
/// 2. `Window.TogglePaneTmux` 归 `Interactive`：tmux 面板不在 `PanelId::ALL` 里
///    （`shell.rs` 的已知偏差），`app_panel_set` 不认识它。
/// 3. `Application.SetWindowOpacity*` 归 `Interactive`：本版没有窗口写动词
///    （的「不做」清单）。
static CLASSES: &[(&str, ApiClass)] = &[
    ("Application.Exit", ApiClass::Verb("app_quit")),
    ("Application.NewWindow", ApiClass::Interactive),
    ("Application.OpenProfileDialog", ApiClass::Interactive),
    ("Application.SetWindowOpacity", ApiClass::Interactive),
    ("Application.SetWindowOpaque", ApiClass::Interactive),
    ("Application.SetWindowTranslucent", ApiClass::Interactive),
    ("Application.SetWindowTransparent", ApiClass::Interactive),
    ("Help.About", ApiClass::Data),
    ("Help.Help", ApiClass::Data),
    ("Terminal.CancelModem", ApiClass::Interactive),
    // 终端内容右键四条：脚本侧有通用派发动词 `app_command`，可以直接用命令
    // ID 表达同一意图，不需要为每条再单列一个动词。
    ("Terminal.ClearAll", ApiClass::Verb("app_command")),
    ("Terminal.ClearRecentUrls", ApiClass::Interactive),
    ("Terminal.ClearScreen", ApiClass::Verb("app_command")),
    ("Terminal.ClearScrollback", ApiClass::Verb("app_command")),
    ("Terminal.CompleteHistory", ApiClass::Interactive),
    ("Terminal.CopyCommand", ApiClass::Verb("app_command")),
    ("Terminal.CopySessionName", ApiClass::Equivalent("clipboard_set")),
    ("Terminal.CopySessionUrl", ApiClass::Equivalent("clipboard_set")),
    ("Terminal.DisconnectSession", ApiClass::Equivalent("session_close")),
    ("Terminal.ManageOneKeys", ApiClass::Interactive),
    ("Terminal.ManageScripts", ApiClass::Interactive),
    ("Terminal.ManageTriggers", ApiClass::Interactive),
    ("Terminal.ManageTunnels", ApiClass::Interactive),
    ("Terminal.NewScript", ApiClass::Interactive),
    ("Terminal.NewSession", ApiClass::Equivalent("session_open")),
    ("Terminal.OpenDefaultSessionSettings", ApiClass::Interactive),
    ("Terminal.OpenLogFile", ApiClass::Interactive),
    ("Terminal.OpenLogFolder", ApiClass::Interactive),
    ("Terminal.OpenLogSettings", ApiClass::Interactive),
    ("Terminal.OpenSession", ApiClass::Interactive),
    ("Terminal.OpenSessionSettings", ApiClass::Interactive),
    ("Terminal.PasteSelectedText", ApiClass::Equivalent("write")),
    ("Terminal.PauseLogger", ApiClass::Equivalent("log_stop")),
    ("Terminal.ReceiveXModem", ApiClass::Interactive),
    ("Terminal.ReceiveYModem", ApiClass::Interactive),
    ("Terminal.ReceiveZModem", ApiClass::Interactive),
    ("Terminal.ReconnectSession", ApiClass::Equivalent("session_open")),
    ("Terminal.RecordScript", ApiClass::Interactive),
    ("Terminal.ResumeLogger", ApiClass::Equivalent("log_start")),
    ("Terminal.RunScript", ApiClass::Interactive),
    ("Terminal.Save", ApiClass::Interactive),
    ("Terminal.SaveSelectionAs", ApiClass::Equivalent("fs_write")),
    ("Terminal.SchemeCmd", ApiClass::Interactive),
    ("Terminal.SchemeDumb", ApiClass::Interactive),
    ("Terminal.SchemeLinux", ApiClass::Interactive),
    ("Terminal.SchemeMacos", ApiClass::Interactive),
    ("Terminal.SchemeNone", ApiClass::Interactive),
    ("Terminal.SchemePowershell", ApiClass::Interactive),
    ("Terminal.SchemeRouteros", ApiClass::Interactive),
    ("Terminal.SendXModem", ApiClass::Interactive),
    ("Terminal.SendYModem", ApiClass::Interactive),
    ("Terminal.SendZModem", ApiClass::Interactive),
    ("Terminal.StartLogger", ApiClass::Equivalent("log_start")),
    ("Terminal.StopLogger", ApiClass::Equivalent("log_stop")),
    ("Terminal.StopScript", ApiClass::Interactive),
    ("Terminal.SyncInput", ApiClass::Interactive),
    ("Terminal.TabActivate", ApiClass::Interactive),
    ("Terminal.TabClose", ApiClass::Equivalent("session_close")),
    ("Terminal.TabCloseAll", ApiClass::Equivalent("session_close")),
    ("Terminal.TabCloseOthers", ApiClass::Equivalent("session_close")),
    ("Terminal.TabColor", ApiClass::Interactive),
    ("Terminal.TabCopyName", ApiClass::Equivalent("clipboard_set")),
    ("Terminal.TabCopyUrl", ApiClass::Equivalent("clipboard_set")),
    ("Terminal.TabDuplicate", ApiClass::Interactive),
    ("Terminal.TabReopenClosed", ApiClass::Interactive),
    ("Terminal.TabSplit", ApiClass::Interactive),
    ("Terminal.TunnelSettings", ApiClass::Interactive),
    ("Text.ActivateNextSelection", ApiClass::Interactive),
    ("Text.ActivatePreviousSelection", ApiClass::Interactive),
    ("Text.CancelSelection", ApiClass::Interactive),
    ("Text.CharLeft", ApiClass::Interactive),
    ("Text.CharRight", ApiClass::Interactive),
    ("Text.Copy", ApiClass::Equivalent("clipboard_set")),
    ("Text.DropSelection", ApiClass::Interactive),
    ("Text.ExchangeCaretAndAnchor", ApiClass::Interactive),
    ("Text.Find", ApiClass::Interactive),
    ("Text.FindNext", ApiClass::Interactive),
    ("Text.FindNextSelection", ApiClass::Interactive),
    ("Text.FindPrevious", ApiClass::Interactive),
    ("Text.FindPreviousSelection", ApiClass::Interactive),
    ("Text.GotoDocumentEnd", ApiClass::Interactive),
    ("Text.GotoDocumentStart", ApiClass::Interactive),
    ("Text.GotoLineEnd", ApiClass::Interactive),
    ("Text.GotoLineHome", ApiClass::Interactive),
    ("Text.GotoNextCommand", ApiClass::Interactive),
    ("Text.GotoNextFold", ApiClass::Interactive),
    ("Text.GotoNextIndentation", ApiClass::Interactive),
    ("Text.GotoNextPage", ApiClass::Interactive),
    ("Text.GotoNextPair", ApiClass::Interactive),
    ("Text.GotoNextParagraph", ApiClass::Interactive),
    ("Text.GotoNextScope", ApiClass::Interactive),
    ("Text.GotoNextStutteredPage", ApiClass::Interactive),
    ("Text.GotoNextWord", ApiClass::Interactive),
    ("Text.GotoNextWordEnd", ApiClass::Interactive),
    ("Text.GotoNextWordPart", ApiClass::Interactive),
    ("Text.GotoPairEnd", ApiClass::Interactive),
    ("Text.GotoPairStart", ApiClass::Interactive),
    ("Text.GotoPreviousCommand", ApiClass::Interactive),
    ("Text.GotoPreviousFold", ApiClass::Interactive),
    ("Text.GotoPreviousIndentation", ApiClass::Interactive),
    ("Text.GotoPreviousPage", ApiClass::Interactive),
    ("Text.GotoPreviousPair", ApiClass::Interactive),
    ("Text.GotoPreviousParagraph", ApiClass::Interactive),
    ("Text.GotoPreviousScope", ApiClass::Interactive),
    ("Text.GotoPreviousStutteredPage", ApiClass::Interactive),
    ("Text.GotoPreviousWord", ApiClass::Interactive),
    ("Text.GotoPreviousWordEnd", ApiClass::Interactive),
    ("Text.GotoPreviousWordPart", ApiClass::Interactive),
    ("Text.HighlightCaretLine", ApiClass::Interactive),
    ("Text.HighlightText", ApiClass::Interactive),
    ("Text.InvertSelection", ApiClass::Interactive),
    ("Text.LineDown", ApiClass::Interactive),
    ("Text.LineUp", ApiClass::Interactive),
    ("Text.LockSelection", ApiClass::Interactive),
    ("Text.MarginBlank", ApiClass::Interactive),
    ("Text.MarginFold", ApiClass::Interactive),
    ("Text.MarginFoldColumn", ApiClass::Interactive),
    ("Text.MarginNumber", ApiClass::Interactive),
    ("Text.MarginSymbol", ApiClass::Interactive),
    ("Text.MarginTimestamp", ApiClass::Interactive),
    ("Text.Paste", ApiClass::Equivalent("clipboard_get")),
    ("Text.ResetZoom", ApiClass::Interactive),
    ("Text.SearchOnlineBing", ApiClass::Interactive),
    ("Text.SearchOnlineDuckDuckGo", ApiClass::Interactive),
    ("Text.SearchOnlineGithub", ApiClass::Interactive),
    ("Text.SearchOnlineGoogle", ApiClass::Interactive),
    ("Text.SearchOnlineStackoverflow", ApiClass::Interactive),
    ("Text.SearchOnlineWikipedia", ApiClass::Interactive),
    ("Text.SelectAll", ApiClass::Interactive),
    ("Text.SelectColumnToLineEnd", ApiClass::Interactive),
    ("Text.SelectColumnToLineHome", ApiClass::Interactive),
    ("Text.SelectColumnToNextChar", ApiClass::Interactive),
    ("Text.SelectColumnToNextLine", ApiClass::Interactive),
    ("Text.SelectColumnToNextPage", ApiClass::Interactive),
    ("Text.SelectColumnToNextParagraph", ApiClass::Interactive),
    ("Text.SelectColumnToPreviousChar", ApiClass::Interactive),
    ("Text.SelectColumnToPreviousLine", ApiClass::Interactive),
    ("Text.SelectColumnToPreviousPage", ApiClass::Interactive),
    ("Text.SelectColumnToPreviousParagraph", ApiClass::Interactive),
    ("Text.SelectCommand", ApiClass::Interactive),
    ("Text.SelectFold", ApiClass::Interactive),
    ("Text.SelectIndentation", ApiClass::Interactive),
    ("Text.SelectLine", ApiClass::Interactive),
    ("Text.SelectPair", ApiClass::Interactive),
    ("Text.SelectParagraph", ApiClass::Interactive),
    ("Text.SelectScope", ApiClass::Interactive),
    ("Text.SelectToDocumentEnd", ApiClass::Interactive),
    ("Text.SelectToDocumentStart", ApiClass::Interactive),
    ("Text.SelectToLineEnd", ApiClass::Interactive),
    ("Text.SelectToLineHome", ApiClass::Interactive),
    ("Text.SelectToNextChar", ApiClass::Interactive),
    ("Text.SelectToNextFold", ApiClass::Interactive),
    ("Text.SelectToNextIndentation", ApiClass::Interactive),
    ("Text.SelectToNextLine", ApiClass::Interactive),
    ("Text.SelectToNextPage", ApiClass::Interactive),
    ("Text.SelectToNextPair", ApiClass::Interactive),
    ("Text.SelectToNextParagraph", ApiClass::Interactive),
    ("Text.SelectToNextScope", ApiClass::Interactive),
    ("Text.SelectToNextStutteredPage", ApiClass::Interactive),
    ("Text.SelectToNextWordEnd", ApiClass::Interactive),
    ("Text.SelectToNextWordPart", ApiClass::Interactive),
    ("Text.SelectToNextWordStart", ApiClass::Interactive),
    ("Text.SelectToPairEnd", ApiClass::Interactive),
    ("Text.SelectToPairStart", ApiClass::Interactive),
    ("Text.SelectToPreviousChar", ApiClass::Interactive),
    ("Text.SelectToPreviousFold", ApiClass::Interactive),
    ("Text.SelectToPreviousIndentation", ApiClass::Interactive),
    ("Text.SelectToPreviousLine", ApiClass::Interactive),
    ("Text.SelectToPreviousPage", ApiClass::Interactive),
    ("Text.SelectToPreviousPair", ApiClass::Interactive),
    ("Text.SelectToPreviousParagraph", ApiClass::Interactive),
    ("Text.SelectToPreviousScope", ApiClass::Interactive),
    ("Text.SelectToPreviousStutteredPage", ApiClass::Interactive),
    ("Text.SelectToPreviousWordEnd", ApiClass::Interactive),
    ("Text.SelectToPreviousWordPart", ApiClass::Interactive),
    ("Text.SelectToPreviousWordStart", ApiClass::Interactive),
    ("Text.SelectWord", ApiClass::Interactive),
    ("Text.SplitSelection", ApiClass::Interactive),
    ("Text.SwitchStreamAndColumn", ApiClass::Interactive),
    ("Text.ToggleHexEdit", ApiClass::Interactive),
    ("Text.ToggleMultipleCaret", ApiClass::Interactive),
    ("Text.ToggleVirtualSpace", ApiClass::Interactive),
    ("Text.ToggleWordWrap", ApiClass::Interactive),
    ("Text.ZoomIn", ApiClass::Interactive),
    ("Text.ZoomOut", ApiClass::Interactive),
    // View 域全是「界面怎么显示」的开关（Hex 进制 / 列宽 / 分组、行尾与折叠、
    // 空白可见性）。surface 里没有对应的脚本动词，`app_state` 也不回读这些显示
    // 选项——所以按「得由人在界面上切」归 Interactive，与表里其它 Toggle* 一致。
    ("View.Fold", ApiClass::Interactive),
    ("View.FoldAll", ApiClass::Interactive),
    ("View.FoldInside", ApiClass::Interactive),
    ("View.HexBase10", ApiClass::Interactive),
    ("View.HexBase16", ApiClass::Interactive),
    ("View.HexBase2", ApiClass::Interactive),
    ("View.HexBase7", ApiClass::Interactive),
    ("View.HexBase8", ApiClass::Interactive),
    ("View.HexColumns16", ApiClass::Interactive),
    ("View.HexColumns24", ApiClass::Interactive),
    ("View.HexColumns32", ApiClass::Interactive),
    ("View.HexColumns8", ApiClass::Interactive),
    ("View.HexGroupChars1", ApiClass::Interactive),
    ("View.HexGroupChars2", ApiClass::Interactive),
    ("View.HexGroupChars4", ApiClass::Interactive),
    ("View.HexGroupChars8", ApiClass::Interactive),
    ("View.ToggleColumnMarker", ApiClass::Interactive),
    ("View.ToggleIndentGuides", ApiClass::Interactive),
    ("View.ToggleLineEndings", ApiClass::Interactive),
    ("View.ToggleWrapSymbols", ApiClass::Interactive),
    ("View.Unfold", ApiClass::Interactive),
    ("View.UnfoldAll", ApiClass::Interactive),
    ("View.UnfoldInside", ApiClass::Interactive),
    ("View.WhiteSpacesHideAll", ApiClass::Interactive),
    ("View.WhiteSpacesShowAll", ApiClass::Interactive),
    ("View.WhiteSpacesShowInLineEnd", ApiClass::Interactive),
    ("View.WhiteSpacesShowInLineStart", ApiClass::Interactive),
    ("View.WhiteSpacesShowInSelection", ApiClass::Interactive),
    ("Window.ActivateNextView", ApiClass::Interactive),
    ("Window.ActivatePreviousView", ApiClass::Interactive),
    ("Window.Close", ApiClass::Interactive),
    ("Window.CloseActiveGroup", ApiClass::Interactive),
    ("Window.CloseActiveView", ApiClass::Interactive),
    ("Window.CloseAllViews", ApiClass::Interactive),
    ("Window.GotoAddressBar", ApiClass::Interactive),
    ("Window.GotoCommandLine", ApiClass::Interactive),
    ("Window.GotoLine", ApiClass::Interactive),
    ("Window.GotoSession", ApiClass::Interactive),
    ("Window.GotoSymbol", ApiClass::Interactive),
    ("Window.LockScreen", ApiClass::Interactive),
    ("Window.MoveGroupToGroup", ApiClass::Interactive),
    ("Window.OpenFocusModeSettings", ApiClass::Interactive),
    ("Window.OpenSettings", ApiClass::Interactive),
    ("Window.ResetLayout", ApiClass::Verb("app_layout")),
    ("Window.SetLocalMode", ApiClass::Interactive),
    ("Window.SetRemoteMode", ApiClass::Interactive),
    ("Window.ShowPaletteAnything", ApiClass::Interactive),
    ("Window.ShowPaletteCommand", ApiClass::Interactive),
    ("Window.SplitGroupHorizontally", ApiClass::Interactive),
    ("Window.SplitGroupVertically", ApiClass::Interactive),
    ("Window.SplitViewHorizontally", ApiClass::Interactive),
    ("Window.SplitViewVertically", ApiClass::Interactive),
    ("Window.ToggleAlwaysOnTop", ApiClass::Interactive),
    ("Window.ToggleBottomDocks", ApiClass::Equivalent("app_panel_set")),
    ("Window.ToggleDebugMode", ApiClass::Interactive),
    ("Window.ToggleFocusMode", ApiClass::Interactive),
    ("Window.ToggleFullScreen", ApiClass::Interactive),
    ("Window.ToggleLeftDocks", ApiClass::Equivalent("app_panel_set")),
    ("Window.ToggleMenuBar", ApiClass::Interactive),
    ("Window.ToggleRightDocks", ApiClass::Equivalent("app_panel_set")),
    ("Window.TogglePaneExplorer", ApiClass::Verb("app_panel_set")),
    ("Window.TogglePaneFiler", ApiClass::Verb("app_panel_set")),
    ("Window.TogglePaneOutline", ApiClass::Verb("app_panel_set")),
    ("Window.TogglePaneSender", ApiClass::Verb("app_panel_set")),
    ("Window.TogglePaneSession", ApiClass::Verb("app_panel_set")),
    ("Window.TogglePaneShell", ApiClass::Verb("app_panel_set")),
    ("Window.TogglePaneScript", ApiClass::Verb("app_panel_set")),
    // tmux 浮层已入册（此前只有 parse 臂的孤儿命令）：`app_command` 能派发它。
    ("Window.TogglePaneTmux", ApiClass::Verb("app_command")),
    ("Window.TogglePaneTransfer", ApiClass::Verb("app_panel_set")),
    ("Window.ToggleQuickBar", ApiClass::Interactive),
    ("Window.ToggleStatusBar", ApiClass::Interactive),
    // 总线开关既不是面板可见性也不是窗口动词：转发线程真的停/起，脚本侧
    // 没有对应动词，归 Interactive（与 TogglePaneTmux 同一口径）。
    ("Window.ToggleTunnels", ApiClass::Interactive),
    ("Window.ToolbarCustomizePlaceholder", ApiClass::Interactive),
    ("Window.ToolbarListEmpty", ApiClass::Interactive),
    ("Window.WindowList", ApiClass::Interactive),
    ("Window.WindowManager", ApiClass::Interactive),
];

/// 按 id 查分类。`None` = 这条命令没被分类（断言 1 会红）。
pub fn class_of(id: &str) -> Option<ApiClass> {
    CLASSES
        .iter()
        .find(|(name, _)| *name == id)
        .map(|(_, class)| *class)
}

/// 余数：`inventory()` 里 `Interactive` 与 `Data` 各多少条。
///
/// 按**入口条数**算（不是按 id 去重）：同一个 id 出现在菜单与标签右键两处时
/// 是两个可点入口，余数也该是 2。
pub fn remainder() -> (usize, usize) {
    let mut interactive = 0;
    let mut data = 0;
    for capability in crate::capability::inventory() {
        match class_of(capability.id) {
            Some(ApiClass::Interactive) => interactive += 1,
            Some(ApiClass::Data) => data += 1,
            // `Verb` / `Equivalent` / `None` 都不进余数：前两者有动词兜着，
            // 后者由断言 1 单独报。
            _ => {}
        }
    }
    (interactive, data)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;
    use term_core::surface::{self, MethodKind};

    /// 写类动词清单：**每一条都必须被至少一条记录引用**（断言 4）。
    ///
    /// 为什么单独列一份而不是从 `surface` 里筛 `app_*` + `Write`：这条断言要拦的是
    /// 「造出一个没人能触发的写动词」，判据必须**点名**（`surface` 里筛出来的东西
    /// 天然满足自己）。`WRITE_VERBS` 漏登记一条不会静默通过——断言 4 会先查它在
    /// 能力面上存在且是 `Write`，再查有记录认领它。
    ///
    /// 只在本模块内声明：它和 `surface` import 都只服务断言，放顶层会让生产构建
    /// 报 `dead_code`（`-D warnings` 下是硬错误）。
    const WRITE_VERBS: [&str; 3] = ["app_panel_set", "app_layout", "app_command"];

    /// 断言 1：`inventory()` 每一条都有分类。
    #[test]
    fn every_menu_entry_has_a_class() {
        let missing: Vec<&str> = crate::capability::inventory()
            .iter()
            .filter(|c| class_of(c.id).is_none())
            .map(|c| c.id)
            .collect();
        assert!(
            missing.is_empty(),
            "这些入口没有分类（新加菜单项忘了归类？）：{missing:?}"
        );
    }

    /// 断言 2：分类表里每一条都对应真实入口（删了菜单项忘删分类）。
    #[test]
    fn every_classified_id_is_a_real_menu_entry() {
        let real: BTreeSet<&str> = crate::capability::inventory()
            .iter()
            .map(|c| c.id)
            .collect();
        let stale: Vec<&str> = CLASSES
            .iter()
            .filter(|(id, _)| !real.contains(id))
            .map(|(id, _)| *id)
            .collect();
        assert!(
            stale.is_empty(),
            "分类表里有查无此入口的 id（菜单项删了/改名了？）：{stale:?}"
        );
    }

    /// 断言 3：`Verb` / `Equivalent` 指向的动词真实存在，且**脚本看得见**。
    #[test]
    fn every_referenced_verb_exists_on_the_script_surface() {
        for (id, class) in CLASSES {
            let name = match class {
                ApiClass::Verb(name) | ApiClass::Equivalent(name) => *name,
                ApiClass::Interactive | ApiClass::Data => continue,
            };
            let spec = surface::find(name).unwrap_or_else(|| {
                panic!("{id} 指向的动词 {name:?} 不在能力面上（拼错 / 被删？）")
            });
            assert!(
                spec.exposure.on_script(),
                "{id} 指向的 {name:?} 是 {:?}，脚本看不见——分类表描述的是脚本动作",
                spec.exposure
            );
        }
    }

    /// 断言 4：每个写类动词都被至少一条菜单记录认领。
    #[test]
    fn every_write_verb_is_reachable_from_a_menu_entry() {
        for name in WRITE_VERBS {
            let spec = surface::find(name)
                .unwrap_or_else(|| panic!("写动词清单里的 {name:?} 不在能力面上"));
            assert_eq!(
                spec.kind,
                MethodKind::Write,
                "{name:?} 在清单里，但能力面上不是 Write"
            );
            assert!(spec.exposure.on_script(), "{name:?} 脚本看不见");
            let claimed: Vec<&str> = CLASSES
                .iter()
                .filter(|(_, class)| *class == ApiClass::Verb(name))
                .map(|(id, _)| *id)
                .collect();
            assert!(
                !claimed.is_empty(),
                "写动词 {name:?} 没有任何菜单记录引用它——造了个没人能触发的动词"
            );
        }
    }

    /// 断言 5：余数随 `inventory()` 重算并打印（不写死「不暴露 44 条」）。
    #[test]
    fn the_unexposed_remainder_is_computed_not_hardcoded() {
        let total = crate::capability::inventory().len();
        let (interactive, data) = remainder();
        let with_verb = crate::capability::inventory()
            .iter()
            .filter(|c| {
                matches!(
                    class_of(c.id),
                    Some(ApiClass::Verb(_)) | Some(ApiClass::Equivalent(_))
                )
            })
            .count();
        let unclassified = crate::capability::inventory()
            .iter()
            .filter(|c| class_of(c.id).is_none())
            .count();
        println!(
            "菜单入口 {total} 条：有动词 {with_verb} / Interactive {interactive} / \
             Data {data} / 未分类 {unclassified}；分类记录 {} 条",
            CLASSES.len()
        );
        assert_eq!(
            with_verb + interactive + data + unclassified,
            total,
            "四栏之和必须等于入口总数——不等说明某一栏的统计口径漏了分支"
        );
        assert_eq!(unclassified, 0, "未分类的入口由断言 1 单独报，这里只保证算术闭合");
    }
}

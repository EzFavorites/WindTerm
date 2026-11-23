//! 兼容门面：`Action` / `Effect` / `reduce` 的对外路径。
//!
//! 2026-11-12 之前，这个文件同时装着**类型词表**（`Action` / `Effect` /
//! 各能力的子枚举）、**字符串入口**（13 段 `if let` 链）和**分发**（`reduce`）。
//! 三件事挤在一起，于是 `action` 同时向 13 个能力模块射出依赖，而其中 7 个
//! 又反向 `use crate::action::Effect`——实测是一个 13 节点的强连通分量，
//! `action` 与 `state` 各 7 条入边，谁也拆不动。
//!
//! 现在三层分开了：
//!
//! - 类型 → [`crate::vocab`]（纯数据，零 `crate::` 依赖）
//! - 副作用 → [`crate::effect`]（只依赖 vocab）
//! - 入口 → [`crate::parse`]（只依赖 vocab）
//! - 分发 → [`crate::reduce`]（唯一知道全部能力的地方，栈顶）
//!
//! 本文件只剩再导出，为的是不改 `apps/desktop` 里 17 处 `term_app::action::…`
//! 的写法——那些改动没有一行有架构价值，只会让 UI 侧重新校对截图。
//!
//! 测试留在本文件：它们断言的是「入口 + 分发」的联合行为，正好跨这三层。

pub use crate::effect::Effect;
pub use crate::reduce::reduce;
pub use crate::vocab::{Action, FeatureId, XferCmd, XferProtocol};

#[cfg(test)]
mod tests {
    use super::*;
    // 2026-11-12：`ViewState` / `Cmd` 的定义已下沉（`state` / `vocab`），
    // 生产路径不再需要它们，但测试要构造状态与断言副作用，所以只在测试里引入。
    use crate::state::ViewState;
    // 子枚举已下沉到 `vocab`，门面只再导出 `Action` 本身——测试要构造
    // `ShellAction::…` / `FindAction::…`，所以整层词表都引入。
    use crate::vocab::*;
    // 测试里按能力分派时用的别名（生产路径已不再需要它们）。
    use crate::goto as goto_feature;
    use crate::palette as palette_feature;
    use crate::tab as tab_feature;
    use crate::dock::DockZone;

    /// 把外壳动作包成顶层 `Action`，测试里少写一层括号。
    fn sh(action: ShellAction) -> Action {
        Action::Shell(action)
    }

    fn red(action: Action) -> (ViewState, Vec<Effect>) {
        let mut state = ViewState::default();
        let effects = reduce(&mut state, action, None);
        (state, effects)
    }

    // ── 命令 ID → 枚举 ──
    #[test]
    fn parse_new_session() {
        assert_eq!(
            Action::parse("Terminal.NewSession"),
            Some(sh(ShellAction::NewSession))
        );
    }
    #[test]
    fn parse_settings() {
        assert_eq!(
            Action::parse("Window.OpenSettings"),
            Some(sh(ShellAction::OpenSettings))
        );
    }
    #[test]
    fn parse_about() {
        assert_eq!(
            Action::parse("Help.About"),
            Some(sh(ShellAction::ShowAbout))
        );
    }
    #[test]
    fn parse_disconnect() {
        assert_eq!(
            Action::parse("Terminal.DisconnectSession"),
            Some(sh(ShellAction::DisconnectSession))
        );
    }
    #[test]
    fn parse_reconnect() {
        assert_eq!(
            Action::parse("Terminal.ReconnectSession"),
            Some(sh(ShellAction::ReconnectSession))
        );
    }
    #[test]
    fn parse_close_active_view() {
        assert_eq!(
            Action::parse("Window.CloseActiveView"),
            Some(sh(ShellAction::CloseActiveView))
        );
    }
    #[test]
    fn parse_window_toggles() {
        let ids = [
            ("Window.ToggleMenuBar", ShellAction::ToggleMenuBar),
            ("Window.ToggleStatusBar", ShellAction::ToggleStatusBar),
            ("Window.ToggleQuickBar", ShellAction::ToggleQuickBar),
            ("Window.ToggleSidebar", ShellAction::ToggleSidebar),
            ("Window.ToggleFocusMode", ShellAction::ToggleFocusMode),
            ("Window.ToggleFullScreen", ShellAction::ToggleFullScreen),
            ("Window.ToggleAlwaysOnTop", ShellAction::ToggleAlwaysOnTop),
        ];
        for (id, want) in ids {
            assert_eq!(Action::parse(id), Some(Action::Shell(want)), "{id}");
        }
    }

    #[test]
    fn commands_without_a_visible_effect_stay_planned() {
        // 这些开关一度被算作「已接线」，但 `ui/` 里没有任何代码读它们——
        // 点了只是翻转一个没人看的 bool。现在回退成占位：`parse` 返回 `None`，
        // 菜单据此置灰。**不要再为了凑数字把它们接回去**，除非先有真实效果。
        for id in [
            // `Window.LockScreen` 已从这条名单移出——它现在有真实读者
            // （`ui/lock.rs` 的遮罩 + Vault 主密码校验），不是空转开关。
            // `Text.ToggleHexEdit` 也已移出：它现在会翻每条会话的显示模式
            // （`Effect::ToggleHex` → 投影层的 `Tab.mode`，不落盘），
            // 不再是一个没人看的 bool。
            //
            // `Window.ToggleDebugMode` 也已移出：它翻 `Tab.debug`，读者是
            // `ui/terminal/pane_overlays.rs` 的 `if !bin && tab.debug` →
            // `debug_paint::paint`，屏幕上真的多出属性位 / 色号 / 宽字符标记。
            "Text.ToggleWordWrap",
            "Text.ToggleVirtualSpace",
            "Text.ToggleMultipleCaret",
        ] {
            assert_eq!(Action::parse(id), None, "{id} 不应再算已接线");
        }
    }

    /// `Text.ToggleHexEdit` 从「点了没效果」变成真开关。
    ///
    /// 三段一起钉：ID 能解析成外壳动作、reducer 只吐意图、**`ViewState` 一位不动**。
    /// 最后一段是这条测试的重点——模式是**每条会话**的属性，一旦有人把它写成
    /// `ViewState` 里的全局 bool，「A 标签在 hex、B 标签在文本」就不可能表达了。
    #[test]
    fn toggle_hex_edit_is_a_real_switch_that_only_declares_intent() {
        assert_eq!(
            Action::parse("Text.ToggleHexEdit"),
            Some(sh(ShellAction::ToggleHexEdit))
        );

        let mut state = ViewState::default();
        let before = state.clone();
        let effects = reduce(&mut state, sh(ShellAction::ToggleHexEdit), None);
        assert_eq!(effects, vec![Effect::ToggleHex]);
        assert_eq!(state, before, "显示模式不该落在 reducer 里");
    }

    /// `Application.NewWindow`：入口能解析、reducer 只吐意图、状态一位不动。
    ///
    /// 新窗口是**另一个进程**，本进程没有任何字段该变；把「第几个窗口」写进
    /// `ViewState` 会立刻变成一份假的全局状态。
    #[test]
    fn new_window_is_a_real_switch_that_only_declares_intent() {
        assert_eq!(
            Action::parse("Application.NewWindow"),
            Some(sh(ShellAction::NewWindow))
        );
        let mut state = ViewState::default();
        let before = state.clone();
        let effects = reduce(&mut state, sh(ShellAction::NewWindow), None);
        assert_eq!(effects, vec![Effect::NewWindow]);
        assert_eq!(state, before, "拉起新进程不该写任何领域状态");
    }

    /// 归属：这条命令归 `Bin`，尽管它是由 `shell::parse` 收进来的。
    /// 归属看意图（换一种看字节的方式），不看哪个模块先认领了 ID。
    #[test]
    fn toggle_hex_edit_belongs_to_the_bin_feature() {
        assert_eq!(
            Action::parse("Text.ToggleHexEdit").unwrap().feature(),
            FeatureId::Bin
        );
    }
    #[test]
    fn parse_unknown_id_is_none() {
        assert_eq!(Action::parse("Terminal.SomeFutureFeature"), None);
        assert_eq!(Action::parse(""), None);
    }

    // ── 能力归属 ──
    #[test]
    fn parsed_actions_report_their_feature() {
        assert_eq!(
            Action::parse("Text.Find").unwrap().feature(),
            FeatureId::Find
        );
        assert_eq!(
            Action::parse("Window.ToggleStatusBar").unwrap().feature(),
            FeatureId::Shell
        );
        assert_eq!(
            Action::parse("Text.Copy").unwrap().feature(),
            FeatureId::Clipboard
        );
    }

    // ── 对话框与副作用 ──
    #[test]
    fn new_session_opens_dialog() {
        let (s, e) = red(sh(ShellAction::NewSession));
        assert!(s.show_new_session);
        assert_eq!(e, vec![Effect::OpenNewSession]);
    }
    #[test]
    fn settings_opens_dialog() {
        let (s, e) = red(sh(ShellAction::OpenSettings));
        assert!(s.show_settings);
        assert_eq!(e, vec![Effect::OpenSettings]);
    }
    #[test]
    fn about_opens_dialog() {
        let (s, e) = red(sh(ShellAction::ShowAbout));
        assert!(s.show_about);
        assert_eq!(e, vec![Effect::ShowAbout]);
    }
    #[test]
    fn disconnect_emits_effect_without_touching_view() {
        let (s, e) = red(sh(ShellAction::DisconnectSession));
        assert_eq!(e, vec![Effect::Disconnect]);
        assert_eq!(s, ViewState::default());
    }
    #[test]
    fn close_active_view_also_disconnects() {
        assert_eq!(
            red(sh(ShellAction::CloseActiveView)).1,
            vec![Effect::Disconnect]
        );
    }
    #[test]
    fn reconnect_emits_effect() {
        assert_eq!(
            red(sh(ShellAction::ReconnectSession)).1,
            vec![Effect::Reconnect]
        );
    }

    // ── 每个 toggle ──
    #[test]
    fn toggle_menu_bar() {
        assert!(!red(sh(ShellAction::ToggleMenuBar)).0.show_menu_bar);
    }
    #[test]
    fn toggle_status_bar() {
        assert!(!red(sh(ShellAction::ToggleStatusBar)).0.show_status_bar);
    }
    #[test]
    fn toggle_quick_bar() {
        assert!(!red(sh(ShellAction::ToggleQuickBar)).0.show_quick_bar);
    }
    /// 工具行在此之前无条件绘制，这个位只由设置页翻；翻完 `ui/mod.rs` 那条
    /// `pane_tools` 面板就不再声明（专注模式仍然无条件压掉它）。
    #[test]
    fn toggle_toolbar() {
        assert!(!red(sh(ShellAction::ToggleToolbar)).0.show_toolbar);
    }
    #[test]
    fn toggle_focus_mode() {
        assert!(red(sh(ShellAction::ToggleFocusMode)).0.focus_mode);
    }
    #[test]
    fn toggle_full_screen() {
        assert!(red(sh(ShellAction::ToggleFullScreen)).0.full_screen);
    }
    #[test]
    fn toggle_always_on_top() {
        assert!(red(sh(ShellAction::ToggleAlwaysOnTop)).0.always_on_top);
    }
    /// 整带开关：新版默认布局左带是 Explorer + Filer，翻一次应当一起收起。
    #[test]
    fn toggle_left_docks_hides_explorer_and_filer() {
        let (s, _) = red(sh(ShellAction::ToggleDocks(DockZone::Left)));
        assert!(!s.show_pane_explorer && !s.show_filer);
        assert!(s.show_sidebar, "会话树现在归右带，不该被左带开关波及");
    }
    /// 整带开关是**一条动作两个方向**：带里有任何一块亮着就全关，全灭就全开。
    ///
    /// 这里把三块右带面板点亮再连翻两次：第一次全灭，第二次全亮。
    /// 第二条断言是重点——「关掉之后回不去」是明令禁止的。
    #[test]
    fn toggle_right_docks_closes_then_restores_the_whole_band() {
        let mut state = ViewState {
            show_pane_outline: true,
            show_pane_session: true,
            show_pane_transfer: true,
            ..ViewState::default()
        };

        reduce(&mut state, sh(ShellAction::ToggleDocks(DockZone::Right)), None);
        assert!(!state.show_pane_outline && !state.show_pane_session && !state.show_pane_transfer);

        reduce(&mut state, sh(ShellAction::ToggleDocks(DockZone::Right)), None);
        assert!(state.show_pane_outline && state.show_pane_session && state.show_pane_transfer);
    }
    #[test]
    fn every_toggle_is_involutive() {
        let toggles = [
            ShellAction::ToggleMenuBar,
            ShellAction::ToggleStatusBar,
            ShellAction::ToggleQuickBar,
            ShellAction::ToggleSidebar,
            ShellAction::ToggleFocusMode,
            ShellAction::ToggleFullScreen,
            ShellAction::ToggleAlwaysOnTop,
        ];
        for a in toggles {
            let mut s = ViewState::default();
            let before = s.clone();
            reduce(&mut s, sh(a.clone()), None);
            reduce(&mut s, sh(a), None);
            assert_eq!(s, before);
        }
    }

    #[test]
    fn copy_is_routed_to_the_clipboard_feature() {
        assert_eq!(
            Action::parse("Text.Copy"),
            Some(Action::Clipboard(ClipboardAction::Copy))
        );
        let mut state = ViewState::default();
        let effects = reduce(&mut state, Action::Clipboard(ClipboardAction::Copy), None);
        assert_eq!(effects, vec![Effect::CopySelection]);
    }

    // ── 选区命令 ──
    #[test]
    fn selection_commands_route_to_the_selection_feature() {
        assert_eq!(
            Action::parse("Text.SelectAll"),
            Some(Action::Select(Op::All))
        );
        assert_eq!(
            Action::parse("Text.SelectToNextLine"),
            Some(Action::Select(Op::LineNext))
        );
        assert_eq!(
            Action::parse("Text.SelectToLineEnd").unwrap().feature(),
            FeatureId::Selection
        );
    }

    #[test]
    fn selection_actions_emit_their_op_without_touching_the_view() {
        let mut state = ViewState::default();
        let before = state.clone();
        let effects = reduce(&mut state, Action::Select(Op::CharNext), None);
        assert_eq!(effects, vec![Effect::Select(Cmd::Move(Op::CharNext))]);
        assert_eq!(state, before, "位移在投影层算，reducer 不写视图状态");
    }

    // ── 9 条集合命令 ──
    #[test]
    fn set_commands_route_to_the_selection_feature() {
        let ids = [
            ("Text.ActivateNextSelection", SetOp::ActivateNext),
            ("Text.ActivatePreviousSelection", SetOp::ActivatePrevious),
            ("Text.LockSelection", SetOp::Lock),
            ("Text.DropSelection", SetOp::Drop),
            ("Text.CancelSelection", SetOp::Cancel),
            ("Text.SplitSelection", SetOp::Split),
            ("Text.InvertSelection", SetOp::Invert),
            ("Text.SwitchStreamAndColumn", SetOp::SwitchShape),
            ("Text.ExchangeCaretAndAnchor", SetOp::Exchange),
        ];
        for (id, want) in ids {
            let action = Action::parse(id).unwrap_or_else(|| panic!("{id} 必须能解析"));
            assert_eq!(action, Action::SelectSet(want), "{id}");
            assert_eq!(action.feature(), FeatureId::Selection, "{id}");
            // 集合命令同样不写 `ViewState`：集合活在标签上（投影层）。
            let before = ViewState::default();
            let mut state = before.clone();
            let effects = reduce(&mut state, action, None);
            assert_eq!(effects, vec![Effect::Select(Cmd::Set(want))], "{id}");
            assert_eq!(state, before, "{id} 不该写视图状态");
        }
        // 位移命令不能被集合表接走，反之亦然。
        assert_eq!(Action::parse("Text.SelectToNextLine"), Some(Action::Select(Op::LineNext)));
    }

    // ── Goto 的 4 条光标移动 ──
    #[test]
    fn goto_cursor_commands_move_the_head_through_the_same_op_path() {
        let ids = [
            ("Text.CharLeft", Op::CharPrev),
            ("Text.CharRight", Op::CharNext),
            ("Text.LineUp", Op::LinePrev),
            ("Text.LineDown", Op::LineNext),
        ];
        for (id, want) in ids {
            assert_eq!(
                Action::parse(id),
                Some(Action::Select(want)),
                "{id} 走的是与 Shift+方向键同一条位移"
            );
        }
    }

    // ── 2026-11-13：26 条光标 / 选区命令 ──
    #[test]
    fn cursor_and_column_commands_route_through_the_same_selection_effect() {
        // 与上面 4 条 Goto 光标命令**同一条**路径：`Action::Select(op)` →
        // `Effect::Select(Cmd::Move(op))`。这一刀没给它们另开门（没有第二套光标、
        // 没有第二个待办槽位），所以这里断言的是「路由没分叉」而不是新机制。
        let ids = [
            ("Text.SelectColumnToPreviousChar", Op::ColumnCharPrev),
            ("Text.SelectColumnToNextChar", Op::ColumnCharNext),
            ("Text.SelectColumnToPreviousLine", Op::ColumnLinePrev),
            ("Text.SelectColumnToNextLine", Op::ColumnLineNext),
            ("Text.SelectColumnToPreviousPage", Op::ColumnPagePrev),
            ("Text.SelectColumnToNextPage", Op::ColumnPageNext),
            ("Text.SelectColumnToPreviousParagraph", Op::ColumnParagraphPrev),
            ("Text.SelectColumnToNextParagraph", Op::ColumnParagraphNext),
            ("Text.SelectColumnToLineHome", Op::ColumnLineHome),
            ("Text.SelectColumnToLineEnd", Op::ColumnLineEnd),
            ("Text.GotoPreviousWordPart", Op::WordPartPrev),
            ("Text.GotoNextWordPart", Op::WordPartNext),
            ("Text.GotoPreviousStutteredPage", Op::StutteredPagePrev),
            ("Text.GotoNextStutteredPage", Op::StutteredPageNext),
            ("Text.GotoPreviousParagraph", Op::ParagraphPrev),
            ("Text.GotoNextParagraph", Op::ParagraphNext),
            ("Text.GotoPreviousCommand", Op::CommandPrev),
            ("Text.GotoNextCommand", Op::CommandNext),
            ("Text.GotoPreviousScope", Op::ScopePrev),
            ("Text.GotoNextScope", Op::ScopeNext),
            ("Text.GotoPreviousIndentation", Op::IndentationPrev),
            ("Text.GotoNextIndentation", Op::IndentationNext),
            ("Text.GotoPairStart", Op::PairStart),
            ("Text.GotoPairEnd", Op::PairEnd),
            ("Text.GotoPreviousPair", Op::PairPrev),
            ("Text.GotoNextPair", Op::PairNext),
        ];
        let before = ViewState::default();
        for (id, want) in ids {
            let action = Action::parse(id).unwrap_or_else(|| panic!("{id} 必须能解析"));
            assert_eq!(action, Action::Select(want), "{id}");
            assert_eq!(action.feature(), FeatureId::Selection, "{id}");
            let mut state = before.clone();
            let effects = reduce(&mut state, action, None);
            assert_eq!(effects, vec![Effect::Select(Cmd::Move(want))], "{id}");
            assert_eq!(state, before, "{id} 不该写视图状态（4h：ViewState 不加字段）");
        }
        // `Goto ▸ Fold` 两条**不在**上面这张 `Op` 表里，而且这一刀之后它们是
        // 已接线的——所以这里显式钉一句「它们走的是折叠，不是选区」。
        // 判据不是「解析得出东西」（那早就对了），而是**解析成哪一支**：
        // `Op` 的前提是「只挪选区不动视口」，折叠跳转动的是视口。
        assert_eq!(
            Action::parse("Text.GotoPreviousFold"),
            Some(Action::Fold(FoldCmd::Previous))
        );
        assert_eq!(
            Action::parse("Text.GotoNextFold"),
            Some(Action::Fold(FoldCmd::Next))
        );
        assert_eq!(
            Action::parse("Text.GotoPreviousFold").map(|a| a.feature()),
            Some(FeatureId::Margin),
            "折叠跳转不是选区能力"
        );
    }

    // ── 折叠族九条：Action → Effect ──

    /// 九条折叠命令**每一条**都要产出它自己那一个 `Effect::Fold(..)`。
    ///
    /// 逐条写死，不用循环：这条断言守的是「ID 认成哪一个变体」，而九个变体里
    /// `Fold` / `FoldAll` / `FoldInside` 三个只差后缀——循环版拿实现当期望值时
    /// 一条都不会红，正好把这类错位放过去。
    ///
    /// 同时钉住「纯层一行 `ViewState` 都不写」：折叠集合是每条会话的属性
    /// （`Tab.folds`），`ViewState` 里多一个字段就是第二份真相。
    #[test]
    fn the_nine_fold_commands_each_emit_their_own_effect() {
        let before = ViewState::default();
        let cases = |id: &str, want: FoldCmd| {
            let action = Action::parse(id).unwrap_or_else(|| panic!("{id} 必须能解析"));
            assert_eq!(action, Action::Fold(want), "{id}");
            let mut state = before.clone();
            let effects = reduce(&mut state, action, None);
            assert_eq!(effects, vec![Effect::Fold(want)], "{id}");
            assert_eq!(
                state, before,
                "{id} 不该写视图状态（折叠集合归投影层的 Tab）"
            );
        };
        cases("View.Fold", FoldCmd::Fold);
        cases("View.Unfold", FoldCmd::Unfold);
        cases("View.FoldInside", FoldCmd::FoldInside);
        cases("View.UnfoldInside", FoldCmd::UnfoldInside);
        cases("View.FoldAll", FoldCmd::FoldAll);
        cases("View.UnfoldAll", FoldCmd::UnfoldAll);
        cases("Text.MarginFold", FoldCmd::Toggle);
        cases("Text.GotoPreviousFold", FoldCmd::Previous);
        cases("Text.GotoNextFold", FoldCmd::Next);
    }

    // ── Find 经由顶层路由 ──
    #[test]
    fn find_actions_emit_no_effects() {
        let mut state = ViewState::default();
        let e = reduce(&mut state, Action::Find(FindAction::Open), None);
        assert!(e.is_empty(), "Find 的状态变化不再需要 UI 侧补副作用");
        assert!(state.find.visible);
    }

    // ── 窗口 / 会话 / 缩放动作的解析 ──
    #[test]
    fn window_and_session_actions_route_to_shell() {
        let ids = [
            ("Window.ActivateNextView", ShellAction::ActivateNextView),
            (
                "Window.ActivatePreviousView",
                ShellAction::ActivatePreviousView,
            ),
            ("Terminal.CopySessionName", ShellAction::CopySessionName),
            ("Terminal.CopySessionUrl", ShellAction::CopySessionUrl),
            ("Help.Help", ShellAction::Help),
            ("Text.SearchOnline", ShellAction::SearchOnline),
            ("Text.ZoomIn", ShellAction::ZoomIn),
            ("Text.ZoomOut", ShellAction::ZoomOut),
            ("Text.ResetZoom", ShellAction::ZoomReset),
        ];
        for (id, want) in ids {
            assert_eq!(Action::parse(id), Some(Action::Shell(want)), "{id}");
        }
    }

    #[test]
    fn shell_view_actions_carry_only_an_intent() {
        // 这些动作不写 ViewState：标签列表 / 会话配置 / 选区 / 字号都活在投影层。
        let no_state = [
            ShellAction::ActivateNextView,
            ShellAction::ActivatePreviousView,
            ShellAction::CopySessionName,
            ShellAction::CopySessionUrl,
            ShellAction::Help,
            ShellAction::SearchOnline,
        ];
        for a in no_state {
            let before = ViewState::default();
            let mut s = before.clone();
            let effects = reduce(&mut s, Action::Shell(a.clone()), None);
            assert_eq!(s, before, "{a:?} 不写视图状态");
            assert!(!effects.is_empty(), "{a:?} 必须声明一个副作用");
        }
        // 缩放同理，但副作用是 Zoom；state 仍不动。
        for a in [
            ShellAction::ZoomIn,
            ShellAction::ZoomOut,
            ShellAction::ZoomReset,
        ] {
            let before = ViewState::default();
            let mut s = before.clone();
            let effects = reduce(&mut s, Action::Shell(a.clone()), None);
            assert_eq!(s, before, "{a:?} 不写视图状态");
            assert_eq!(effects.len(), 1);
        }
        assert_eq!(
            reduce(
                &mut ViewState::default(),
                Action::Shell(ShellAction::ActivateNextView),
                None
            ),
            vec![Effect::ActivateTab(1)]
        );
        assert_eq!(
            reduce(
                &mut ViewState::default(),
                Action::Shell(ShellAction::ZoomReset),
                None
            ),
            vec![Effect::Zoom(0)]
        );
    }

    #[test]
    fn find_from_selection_routes_to_find_selection_effect() {
        // SelectionNext/Prev 经过顶层路由被翻译成带方向的 FindSelection 副作用，
        // 因为查询词（选区文本）只在投影层才有。
        assert_eq!(
            reduce(
                &mut ViewState::default(),
                Action::Find(FindAction::SelectionNext),
                None
            ),
            vec![Effect::FindSelection(1)]
        );
        assert_eq!(
            reduce(
                &mut ViewState::default(),
                Action::Find(FindAction::SelectionPrev),
                None
            ),
            vec![Effect::FindSelection(-1)]
        );
        // 普通 Next/Prev 仍是零副作用（查询由查找栏输入）。
        assert!(reduce(
            &mut ViewState::default(),
            Action::Find(FindAction::Next),
            None
        )
        .is_empty());
    }

    // ── 命令面板 ──
    #[test]
    fn palette_commands_route_to_the_palette_feature() {
        assert_eq!(
            Action::parse("Window.ShowPaletteCommand"),
            Some(Action::Palette(PaletteAction::Open(
                palette_feature::PaletteMode::Command
            )))
        );
        assert_eq!(
            Action::parse("Window.ShowPaletteAnything"),
            Some(Action::Palette(PaletteAction::Open(
                palette_feature::PaletteMode::Anything
            )))
        );
        assert_eq!(
            Action::parse("Window.ShowPaletteCommand")
                .unwrap()
                .feature(),
            FeatureId::Palette
        );
    }

    #[test]
    fn opening_the_palette_only_writes_palette_state() {
        let mut s = ViewState::default();
        let before = s.clone();
        let effects = reduce(
            &mut s,
            Action::Palette(PaletteAction::Open(palette_feature::PaletteMode::Anything)),
            None,
        );
        assert!(effects.is_empty(), "打开面板不产生副作用");
        assert!(s.command_palette.visible);
        s.command_palette = before.command_palette.clone();
        assert_eq!(s, before, "除 command_palette 外一行都不许动");
    }

    #[test]
    fn confirm_executes_the_selected_command_through_the_same_reducer() {
        let mut s = ViewState::default();
        reduce(
            &mut s,
            Action::Palette(PaletteAction::Open(palette_feature::PaletteMode::Command)),
            None,
        );
        reduce(
            &mut s,
            Action::Palette(PaletteAction::SetQuery("ZoomIn".into())),
            None,
        );
        let effects = reduce(&mut s, Action::Palette(PaletteAction::Confirm), None);
        assert_eq!(effects, vec![Effect::Zoom(1)]);
        assert!(!s.command_palette.visible, "执行完面板要关掉");
    }

    #[test]
    fn confirming_a_planned_item_is_a_no_op() {
        let mut s = ViewState::default();
        reduce(
            &mut s,
            Action::Palette(PaletteAction::Open(palette_feature::PaletteMode::Anything)),
            None,
        );
        let idx = palette_feature::entries(palette_feature::PaletteMode::Anything)
            .iter()
            .position(|e| e.id == "Text.ToggleWordWrap")
            .unwrap();
        s.command_palette.selected = idx;
        let effects = reduce(&mut s, Action::Palette(PaletteAction::Confirm), None);
        assert!(effects.is_empty());
        assert!(!s.command_palette.visible);
    }

    #[test]
    fn exit_and_close_commands_carry_window_intents_only() {
        let ids = [
            ("Application.Exit", ShellAction::Exit),
            ("Window.Close", ShellAction::Close),
            ("Window.CloseAllViews", ShellAction::CloseAllViews),
        ];
        for (id, want) in ids {
            assert_eq!(Action::parse(id), Some(Action::Shell(want.clone())), "{id}");
        }
        assert_eq!(
            reduce(&mut ViewState::default(), sh(ShellAction::Exit), None),
            vec![Effect::Exit]
        );
        assert_eq!(
            reduce(&mut ViewState::default(), sh(ShellAction::Close), None),
            vec![Effect::CloseActiveTab]
        );
        assert_eq!(
            reduce(
                &mut ViewState::default(),
                sh(ShellAction::CloseAllViews),
                None
            ),
            vec![Effect::CloseAllTabs]
        );
        // 标签列表 / 窗口系统都活在投影层，reducer 一个字段都不写。
        let before = ViewState::default();
        let mut s = before.clone();
        reduce(&mut s, sh(ShellAction::CloseAllViews), None);
        assert_eq!(s, before);
    }

    // ── Goto 两条 ──
    #[test]
    fn goto_commands_route_to_the_goto_feature() {
        assert_eq!(
            Action::parse("Window.GotoLine"),
            Some(Action::Goto(GotoAction::Open(goto_feature::GotoMode::Line)))
        );
        assert_eq!(
            Action::parse("Window.GotoSession"),
            Some(Action::Goto(GotoAction::Open(
                goto_feature::GotoMode::Sessions
            )))
        );
        assert_eq!(
            Action::parse("Window.GotoLine").unwrap().feature(),
            FeatureId::Goto
        );
    }

    #[test]
    fn opening_goto_writes_only_goto_state() {
        let mut s = ViewState::default();
        let before = s.clone();
        let effects = reduce(
            &mut s,
            Action::Goto(GotoAction::Open(goto_feature::GotoMode::Sessions)),
            None,
        );
        assert!(effects.is_empty());
        assert!(s.goto.visible);
        s.goto = before.goto.clone();
        assert_eq!(s, before);
    }

    #[test]
    fn confirming_a_goto_line_emits_the_parsed_row() {
        let mut s = ViewState::default();
        reduce(
            &mut s,
            Action::Goto(GotoAction::Open(goto_feature::GotoMode::Line)),
            None,
        );
        reduce(
            &mut s,
            Action::Goto(GotoAction::SetInput("-30".into())),
            None,
        );
        assert_eq!(
            reduce(&mut s, Action::Goto(GotoAction::Confirm), None),
            vec![Effect::GotoLine(-30)]
        );
        assert!(!s.goto.visible);
    }

    #[test]
    fn an_invalid_goto_line_emits_nothing_and_stays_open() {
        let mut s = ViewState::default();
        reduce(
            &mut s,
            Action::Goto(GotoAction::Open(goto_feature::GotoMode::Line)),
            None,
        );
        reduce(
            &mut s,
            Action::Goto(GotoAction::SetInput("abc".into())),
            None,
        );
        assert!(reduce(&mut s, Action::Goto(GotoAction::Confirm), None).is_empty());
        assert!(s.goto.visible && s.goto.error);
    }

    #[test]
    fn confirming_a_goto_session_emits_the_selected_index() {
        let mut s = ViewState::default();
        reduce(
            &mut s,
            Action::Goto(GotoAction::Open(goto_feature::GotoMode::Sessions)),
            None,
        );
        reduce(&mut s, Action::Goto(GotoAction::Move(2, 3)), None);
        assert_eq!(
            reduce(&mut s, Action::Goto(GotoAction::Confirm), None),
            vec![Effect::GotoSession(2)]
        );
    }

    #[test]
    fn sync_input_tunnels_toggle_and_select_command_only_carries_an_intent() {
        let mut s = ViewState::default();
        assert!(!s.sync_input && !s.show_tunnels);

        // 同步输入：翻一次开、再翻一次关（对合），且不产生副作用。
        assert!(reduce(&mut s, Action::parse("Terminal.SyncInput").unwrap(), None).is_empty());
        assert!(s.sync_input);
        reduce(&mut s, Action::parse("Terminal.SyncInput").unwrap(), None);
        assert!(!s.sync_input, "再点一次要回到原状");

        // 隧道管理：面板开关同样对合。
        reduce(
            &mut s,
            Action::parse("Terminal.ManageTunnels").unwrap(),
            None,
        );
        assert!(s.show_tunnels);
        reduce(
            &mut s,
            Action::parse("Terminal.ManageTunnels").unwrap(),
            None,
        );
        assert!(!s.show_tunnels);

        // 选中命令：块边界与选区都在投影层，纯层只带意图。
        let before = s.clone();
        assert_eq!(
            reduce(&mut s, Action::parse("Text.SelectCommand").unwrap(), None),
            vec![Effect::SelectCommand]
        );
        assert_eq!(s, before, "reducer 不该写视图状态");
    }

    /// 两条「直达」命令：`ManageTriggers` / `ManageOneKeys` 打开各自的
    /// **独立对话框**；`ClearRecentUrls` 一个字段都不写。
    ///
    /// 2026-11-15 改口径：这两条原先置 `show_settings` 再发「翻到某页」的效应，
    /// 弹出来的是 Preferences（而且那两页已按用户裁定移出左树，翻不到）。兼容终端
    /// 是自有窗口，所以现在只置 `show_triggers` / `show_onekeys`、不再发效应。
    #[test]
    fn the_two_direct_commands_carry_their_intent_and_only_triggers_touches_state() {
        let mut s = ViewState::default();
        assert!(!s.show_settings, "出厂不弹设置");
        assert!(!s.show_triggers && !s.show_onekeys, "出厂不弹这两个对话框");

        // ManageTriggers：打开 Triggers 对话框，不借道 Preferences。
        assert_eq!(
            reduce(
                &mut s,
                Action::parse("Terminal.ManageTriggers").unwrap(),
                None
            ),
            vec![]
        );
        assert!(s.show_triggers, "点了必须弹出独立对话框");
        assert!(!s.show_settings, "不该顺带把 Preferences 也打开");
        // ManageOneKeys：同形。
        assert_eq!(
            reduce(
                &mut s,
                Action::parse("Terminal.ManageOneKeys").unwrap(),
                None
            ),
            vec![]
        );
        assert!(s.show_onekeys, "点了必须弹出独立对话框");
        assert!(!s.show_settings, "不该顺带把 Preferences 也打开");

        // ClearRecentUrls：那份列表在 `Prefs.recent_targets`，纯层没有可写的字段。
        let before = s.clone();
        assert_eq!(
            reduce(
                &mut s,
                Action::parse("Terminal.ClearRecentUrls").unwrap(),
                None
            ),
            vec![Effect::ClearRecentUrls]
        );
        assert_eq!(s, before, "reducer 不该写视图状态");
    }

    /// 四档语义：**三档预设各自的 transparent / opacity 值**，第四条翻页 + 开对话框。
    ///
    /// 判据刻意分成三层：`WindowOpacityPreset` 自带数值（全仓唯一一份档位表）、
    /// `parse` 只把 ID 映射到那一档、`reduce` 只把它翻译成一条带参数的 `Effect`。
    /// 任何一层偷偷改了档位，这里红。三档都**不写 `ViewState`**：值是 `Prefs`
    /// 与投影层的运行期副本，纯层不认识盘，也不该造一个「菜单写、渲染读、
    /// 设置页也写」的三方共享 bool。
    ///
    /// `Opaque` 那一行同时钉住「现状不变」：它回落 `transparent=false` +
    /// `opacity=1.0`，也就是出厂态（渲染侧据此一个像素都不改，见
    /// `apps/desktop/src/ui/window_opacity_tests.rs`）。
    #[test]
    fn the_four_window_opacity_commands_carry_their_own_semantics() {
        use crate::vocab::WindowOpacityPreset as P;

        let presets: [(&str, P, bool, u32, f32); 3] = [
            ("Application.SetWindowOpaque", P::Opaque, false, 1000, 1.0),
            (
                "Application.SetWindowTranslucent",
                P::Translucent,
                true,
                600,
                0.6,
            ),
            (
                "Application.SetWindowTransparent",
                P::Transparent,
                true,
                300,
                0.3,
            ),
        ];
        for (id, preset, transparent, permille, opacity) in presets {
            assert_eq!(preset.transparent(), transparent, "{id} transparent");
            assert_eq!(preset.permille(), permille, "{id} permille");
            assert!(
                (preset.opacity() - opacity).abs() < 1e-6,
                "{id} opacity：{} 应等于 {opacity}",
                preset.opacity()
            );

            let action = Action::parse(id).unwrap();
            assert_eq!(
                action,
                sh(ShellAction::SetWindowOpacity(preset)),
                "{id} parse"
            );
            let mut s = ViewState::default();
            let before = s.clone();
            assert_eq!(
                reduce(&mut s, action, None),
                vec![Effect::SetWindowOpacity {
                    transparent,
                    opacity_permille: permille,
                }],
                "{id} reduce"
            );
            assert_eq!(s, before, "{id} 不该写 ViewState：透明度在 Prefs 与投影层");
        }

        // 第四条不设值：开设置 + 翻到能调透明度的那一页，让用户用滑杆调。
        let mut s = ViewState::default();
        assert!(!s.show_settings, "出厂不弹设置");
        assert_eq!(
            reduce(
                &mut s,
                Action::parse("Application.SetWindowOpacity").unwrap(),
                None
            ),
            vec![Effect::FocusAppearancePage]
        );
        assert!(s.show_settings, "只发翻页效应不开对话框 = 点了没反应");
    }

    #[test]
    fn split_commands_carry_direction_and_scope_without_touching_state() {
        let cases = [
            ("Window.SplitViewHorizontally", Effect::SplitView(true)),
            ("Window.SplitViewVertically", Effect::SplitView(false)),
            ("Window.SplitGroupHorizontally", Effect::SplitGroup(true)),
            ("Window.SplitGroupVertically", Effect::SplitGroup(false)),
            ("Window.CloseActiveGroup", Effect::CloseActiveGroup),
            ("Window.MoveGroupToGroup", Effect::MoveGroupToGroup),
        ];
        for (id, want) in cases {
            let before = ViewState::default();
            let mut s = before.clone();
            let effects = reduce(&mut s, Action::parse(id).unwrap(), None);
            assert_eq!(effects, vec![want], "{id}");
            assert_eq!(s, before, "{id} 不该写视图状态：布局树在投影层");
        }
    }

    // ── 六个工具面板开关 ──
    fn pane_flags(s: &ViewState) -> [bool; 7] {
        [
            s.show_pane_explorer,
            s.show_pane_outline,
            s.show_pane_sender,
            s.show_pane_session,
            s.show_pane_shell,
            s.show_pane_script,
            s.show_pane_transfer,
        ]
    }

    #[test]
    fn pane_toggles_flip_exactly_their_own_flag_and_are_involutive() {
        let ids = [
            "Window.TogglePaneExplorer",
            "Window.TogglePaneOutline",
            "Window.TogglePaneSender",
            "Window.TogglePaneSession",
            "Window.TogglePaneShell",
            "Window.TogglePaneScript",
            "Window.TogglePaneTransfer",
        ];
        for (idx, id) in ids.iter().enumerate() {
            let before = ViewState::default();
            let mut s = before.clone();
            assert_eq!(
                Action::parse(id).unwrap().feature(),
                FeatureId::Shell,
                "{id}"
            );
            assert!(
                reduce(&mut s, Action::parse(id).unwrap(), None).is_empty(),
                "{id} 不该有副作用"
            );
            // 默认布局已经铺满，四块工具面板本来就是亮的——所以不能断言
            // 「翻完恰好一块亮」，只能逐位比较：只有第 idx 位取了反。
            let before_flags = pane_flags(&before);
            let flags = pane_flags(&s);
            for (i, on) in flags.iter().enumerate() {
                let want = if i == idx { !before_flags[i] } else { before_flags[i] };
                assert_eq!(*on, want, "{id} 只翻自己的那个字段（位 {i}）");
            }
            reduce(&mut s, Action::parse(id).unwrap(), None);
            assert_eq!(s, before, "{id} 两次应回到原状");
        }
    }

    #[test]
    fn center_commands_open_distinct_pages_and_touch_only_center_state() {
        // 注：两条 Session Settings 命令**都**不在此列——它们不打开中心面板，
        // 而是各自发出 Effect 改开会话属性 / 默认会话设置对话框（见下面两条
        // 测试）。2026-11-11 三对话框语义修复后 `Terminal.OpenSessionSettings`
        // 也改走这条路，旧清单漏改，于是一直红在「不该有副作用」。
        // 其余 13 条命令各自打开一个不同的中心页。
        let ids = [
            "Terminal.OpenSession",
            "Terminal.Save",
            "Terminal.SaveSelectionAs",
            "Application.OpenProfileDialog",
            "Window.ToolbarList",
            "Window.ToolbarCustomize",
            "Window.GotoCommandLine",
            "Terminal.CompleteHistory",
            "Terminal.SchemeList",
            "Window.WindowList",
            "Window.WindowManager",
            "Window.OpenFocusModeSettings",
            "Text.HighlightText",
        ];
        let mut pages = Vec::new();
        for id in ids {
            let action = Action::parse(id).unwrap();
            assert_eq!(action.feature(), FeatureId::Center, "{id}");
            let before = ViewState::default();
            let mut s = before.clone();
            assert!(reduce(&mut s, action, None).is_empty(), "{id} 不该有副作用");
            assert!(s.center.visible, "{id}");
            if !pages.contains(&s.center.page) {
                pages.push(s.center.page);
            }
            s.center = before.center.clone();
            assert_eq!(s, before, "{id} 只改 center 状态");
        }
        assert_eq!(pages.len(), 13, "13 条命令必须对应 13 个不同页面");
    }

    /// `Terminal.OpenDefaultSessionSettings` 走 `reduce` 时被拦下，改发
    /// `Effect::OpenDefaultSessionSettings`，由投影层打开「默认会话设置」对话框。
    /// 它不写任何 `ViewState`（既不翻中心面板，也不改会话列表）。
    #[test]
    fn open_default_session_settings_emits_dialog_effect_without_view_state() {
        let action = Action::parse("Terminal.OpenDefaultSessionSettings").unwrap();
        assert_eq!(action.feature(), FeatureId::Center);
        let before = ViewState::default();
        let mut s = before.clone();
        let effects = reduce(&mut s, action, None);
        assert_eq!(
            effects,
            vec![Effect::OpenDefaultSessionSettings],
            "必须只发打开默认会话设置对话框这一个副作用"
        );
        assert_eq!(s, before, "不该改任何视图状态");
    }

    /// 与上一条对称：`Session Settings…` 也改开会话属性对话框，不是中心面板
    /// 的占位页。三对话框语义修复（2026-11-11）新增的效果要在这里钉住。
    #[test]
    fn open_active_session_settings_emits_dialog_effect_without_view_state() {
        let action = Action::parse("Terminal.OpenSessionSettings").unwrap();
        assert_eq!(action.feature(), FeatureId::Center);
        let before = ViewState::default();
        let mut s = before.clone();
        let effects = reduce(&mut s, action, None);
        assert_eq!(
            effects,
            vec![Effect::OpenActiveSessionSettings],
            "必须只发打开会话属性对话框这一个副作用"
        );
        assert_eq!(s, before, "不该改任何视图状态");
    }

    // ── 快捷命令栏 ──
    #[test]
    fn a_quick_command_emits_send_command_and_leaves_view_state_alone() {
        let (state, effects) = red(Action::QuickCommand("tcpdump ".to_string()));
        assert_eq!(state, ViewState::default(), "送文本不写任何视图状态");
        assert_eq!(effects, vec![Effect::SendCommand("tcpdump ".to_string())]);
    }

    #[test]
    fn a_quick_command_never_appends_a_newline() {
        // WindTerm 的快捷栏是「送文本到输入行」，不是「执行」。`reboot`
        // 这类命令点一下就跑是危险的，所以纯层也不许替投影层补回车。
        let (_, effects) = red(Action::QuickCommand("reboot ".to_string()));
        let Effect::SendCommand(text) = &effects[0] else {
            panic!("快捷命令必须产出 SendCommand");
        };
        assert!(!text.contains('\r') && !text.contains('\n'));
        assert!(text.ends_with(' '), "尾部留空格，用户接着敲参数");
    }

    #[test]
    fn a_quick_command_belongs_to_the_shell_feature() {
        assert_eq!(
            Action::QuickCommand("ls ".to_string()).feature(),
            FeatureId::Shell
        );
    }

    // ── 2026-11-09：Transfer Binary 子菜单 ──
    #[test]
    fn transfer_commands_route_to_the_xfer_feature() {
        assert_eq!(
            Action::parse("Terminal.SendXModem"),
            Some(Action::Xfer(XferCmd::Send(XferProtocol::Xmodem)))
        );
        assert_eq!(
            Action::parse("Terminal.ReceiveXModem"),
            Some(Action::Xfer(XferCmd::Receive(XferProtocol::Xmodem)))
        );
        assert_eq!(
            Action::parse("Terminal.SendYModem"),
            Some(Action::Xfer(XferCmd::Send(XferProtocol::Ymodem)))
        );
        assert_eq!(
            Action::parse("Terminal.ReceiveYModem"),
            Some(Action::Xfer(XferCmd::Receive(XferProtocol::Ymodem)))
        );
        assert_eq!(
            Action::parse("Terminal.CancelModem"),
            Some(Action::Xfer(XferCmd::Cancel))
        );
        for id in [
            "Terminal.SendXModem",
            "Terminal.ReceiveXModem",
            "Terminal.SendYModem",
            "Terminal.ReceiveYModem",
            "Terminal.CancelModem",
        ] {
            assert_eq!(
                Action::parse(id).unwrap().feature(),
                FeatureId::Xfer,
                "{id} 必须归到传输能力；归到 Shell 会让能力面少一块"
            );
        }
    }

    #[test]
    fn zmodem_transfer_commands_parse_to_a_transfer_intent() {
        // 这两条曾经是**故意**返回 `None` 的（当时 `term-zmodem` 还没落地，
        // 接上只会「菜单变亮、点了不动」。内核与真实 `sz`/`rz` 双向互通
        // 之后它们必须正向落位——**反向断言改成正向断言，不是删掉**：
        // 这条测试守的是「菜单亮着 = 有动作」，翻回去就是又一条违规。
        assert_eq!(
            Action::parse("Terminal.SendZModem"),
            Some(Action::Xfer(XferCmd::Send(XferProtocol::Zmodem)))
        );
        assert_eq!(
            Action::parse("Terminal.ReceiveZModem"),
            Some(Action::Xfer(XferCmd::Receive(XferProtocol::Zmodem)))
        );
        // 归属也必须落在传输能力下：归到 Shell 会让能力面少一块。
        for id in ["Terminal.SendZModem", "Terminal.ReceiveZModem"] {
            assert_eq!(Action::parse(id).unwrap().feature(), FeatureId::Xfer, "{id}");
        }
    }

    #[test]
    fn a_transfer_command_only_carries_an_intent() {
        let before = ViewState::default();
        let mut state = before.clone();
        let effects = reduce(
            &mut state,
            Action::Xfer(XferCmd::Send(XferProtocol::Ymodem)),
            None,
        );
        assert_eq!(effects, vec![Effect::Xfer(XferCmd::Send(XferProtocol::Ymodem))]);
        assert_eq!(state, before, "传输句柄与进度在投影层，reducer 不写视图状态");
    }

    // ── 标签右键 10 个影子入口 ──
    #[test]
    fn every_tab_context_command_parses_and_belongs_to_the_tab_feature() {
        for entry in tab_feature::SHADOW_ENTRIES {
            let action = Action::parse(entry.id)
                .unwrap_or_else(|| panic!("{} 没有接线，右键菜单就是假入口", entry.id));
            assert_eq!(
                action.feature(),
                FeatureId::Tab,
                "{} 归错能力，能力面会少一块",
                entry.id
            );
            assert!(matches!(action, Action::Tab(_)), "{}", entry.id);
        }
    }

    #[test]
    fn a_tab_action_only_carries_an_intent() {
        let before = ViewState::default();
        let mut state = before.clone();
        let action = Action::Tab(TabAction::CloseOthers { target: Some(3) });
        let effects = reduce(&mut state, action.clone(), None);
        assert_eq!(effects, vec![Effect::Tab(TabAction::CloseOthers { target: Some(3) })]);
        assert_eq!(state, before, "标签列表在投影层，reducer 不写视图状态");
        // 强类型路径的 target 必须原样带过去，不能被 reducer 悄悄换成活动标签。
        assert_eq!(action, Action::Tab(TabAction::CloseOthers { target: Some(3) }));
    }
}

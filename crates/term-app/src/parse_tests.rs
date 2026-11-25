//! 入口层的接线单测。从 `parse.rs` 拆出来：那里是 4j 的代码行大户，测试塞在
//! 一起会把「入口层只认 ID，不认识任何能力」这条架构判据挤过 500 行上限。

use super::*;

/// 10 条栅格可视化命令的**入口表**：ID → 旋钮 → 归属能力。
///
/// 这一条与 `apps/desktop/src/ui/terminal/marks_menu_tests.rs` 是一对：
/// 这里钉「ID 有没有被认出来、认出的是哪一个旋钮」，那边钉「认出来之后
/// 屏幕上真的变了没有」。缺了任何一半，「点了没反应」都能溜过去——
/// 只钉前者会得到「命令能点、但写错了旋钮」；只钉后者会得到
/// 「某个 ID 根本不在这张表里，测试用的却是私有方法」。
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
        assert_eq!(action.feature(), FeatureId::Marks, "{id} 归属错");
        assert!(
            crate::capability::is_implemented(id),
            "{id} 必须在已接线集合里"
        );
    }
    assert_eq!(table[4].1, MarksKnob::Whitespace(0), "Hide All 是 0");
    assert_eq!(table[8].1, MarksKnob::Whitespace(4), "In Line End 是 4");
}

/// 十条之外没有第二条路径：`View.WhiteSpaces`（不带档位后缀）不是一个命令。
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
    assert_eq!(
        Action::parse("View.Fold").map(|a| a.feature()),
        Some(FeatureId::Margin)
    );
    for id in ["View.Folds", "View.FoldInsideAll", "Text.Fold"] {
        assert_eq!(Action::parse(id), None, "{id} 不该是一条命令");
    }
}

/// 终端右键四条真实命令 + tmux 浮层开关：ID → 强类型动作，逐条写死。
///
/// 这五条此前不在 `parse` 里（前四条走投影层私有 `Work`，tmux 只有 parse
/// 臂却没有入口表），所以「菜单能点」与「能力台账能查到」是两件事。现在
/// 统一从这一层进，测试同时钉住 parse、台账与能力归属三处。
#[test]
fn terminal_context_commands_parse_to_their_own_action() {
    use crate::vocab::TabAction;
    let table: [(&str, TabAction); 4] = [
        ("Terminal.CopyCommand", TabAction::CopyCommand),
        ("Terminal.ClearScreen", TabAction::ClearScreen),
        ("Terminal.ClearScrollback", TabAction::ClearScrollback),
        ("Terminal.ClearAll", TabAction::ClearAll),
    ];
    for (id, expected) in table {
        assert_eq!(Action::parse(id), Some(Action::Tab(expected)), "{id}");
        assert!(crate::capability::is_implemented(id), "{id} 必须在台账里");
        assert_eq!(
            describe_feature(id),
            Some(FeatureId::Tab),
            "{id} 沿用 Action::Tab 的能力归属"
        );
    }
    assert_eq!(
        Action::parse("Window.TogglePaneTmux"),
        Some(Action::Shell(ShellAction::TogglePaneTmux))
    );
    assert!(crate::capability::is_implemented("Window.TogglePaneTmux"));
}

/// 已登记脚本的映射键走同一个 `Action::parse`：路径从载荷里原样带出来。
#[test]
fn script_run_file_ids_parse_to_the_run_script_action() {
    assert_eq!(
        Action::parse("Script.RunFile:/tmp/a.rhai"),
        Some(Action::RunScript("/tmp/a.rhai".to_string()))
    );
    assert_eq!(Action::parse("Script.RunFile:"), None);
    assert_eq!(Action::parse("Script.RunFile:   "), None);
}

fn describe_feature(id: &str) -> Option<FeatureId> {
    Action::parse(id).map(|a| a.feature())
}

//! 上下文菜单行表的单测（已接线＝有效果 / 视觉基线结构 / Log 子菜单同源 / 粘贴选中语义）。
//!
//! 单独一个文件只为一件事：行表加接线后本文件逼近门禁 4j 的 500 代码行上限，
//! 测试塞在里面会把「数据 + 绘制」挤出去。与 `tab_strip_lifecycle_tests.rs`
//! 同一处理（`#[cfg(test)]` + `#[path]` 两行声明）。

use super::*;

#[test]
fn terminal_menu_keeps_the_reference_row_order_and_groups() {
    let labels: Vec<&str> = TERMINAL_ROWS.iter().filter(|r| !r.label.is_empty()).map(|r| r.label).collect();
    assert_eq!(labels.first(), Some(&"Copy"));
    assert_eq!(labels.last(), Some(&"Reset"));
    assert_eq!(labels.len(), 18, "视觉基线 18 个文字行");
    assert_eq!(TERMINAL_ROWS.len(), 28, "18 行 + 1 圆点行 + 8 分隔线 + 空占位");
    assert_eq!(TERMINAL_ROWS.iter().filter(|r| r.separator).count(), 9, "9 条分隔线");
    assert_eq!(TERMINAL_ROWS.iter().filter(|r| r.swatches).count(), 1, "一排七色圆点");
    assert_eq!(TERMINAL_ROWS.iter().filter(|r| r.submenu).count(), 4, "4 个子菜单箭头");
    // 快捷键列逐字对齐视觉基线，不许悄悄改键位。
    let find = TERMINAL_ROWS.iter().find(|r| r.label == "Find").unwrap();
    assert_eq!(find.shortcut, "Meta+F");
    let goto = TERMINAL_ROWS.iter().find(|r| r.label == "Goto Previous Command").unwrap();
    assert_eq!(goto.shortcut, "Meta+Shift+Left");
}

#[test]
fn view_menu_keeps_three_checkboxes_and_the_split_rows() {
    assert_eq!(VIEW_ROWS.iter().filter(|r| r.check.is_some()).count(), 3);
    assert!(VIEW_ROWS.iter().any(|r| r.label == "Word Wrap" && r.check == Some(true)));
    // 2026-11-17：Split View 两行与 Full Screen 曾经是灰的（当时没接线），
    // 现在都派到既有命令上（`TabAction::Split` / `ShellAction::ToggleFullScreen`），
    // 所以断言反过来：**必须有 work**，否则等于又退回了「点了没反应」。
    assert!(VIEW_ROWS
        .iter()
        .any(|r| r.label == "Split View Horizontally" && r.work.is_some()));
    assert!(VIEW_ROWS
        .iter()
        .any(|r| r.label == "Split View Vertically" && r.work.is_some()));
    assert!(VIEW_ROWS
        .iter()
        .any(|r| r.label == "Full Screen" && r.work.is_some()));
    // 横向与纵向是两个不同的动作，不能都推给同一支。
    let h = VIEW_ROWS.iter().find(|r| r.label == "Split View Horizontally").unwrap().work;
    let v = VIEW_ROWS.iter().find(|r| r.label == "Split View Vertically").unwrap().work;
    assert!(matches!(h, Some(Work::Split(true))));
    assert!(matches!(v, Some(Work::Split(false))));
}

#[test]
fn rows_without_an_action_are_always_drawn_disabled() {
    for r in TERMINAL_ROWS.iter().chain(VIEW_ROWS.iter()) {
        if r.label.is_empty() {
            continue;
        }
        let state = row_state(r, None);
        // 两种合法的「可点」：派一个 `Work`，或者展开一棵真子菜单
        // （`children` 指向菜单树里那一棵）。二者都没有就是假开关。
        if r.work.is_none() && r.children.is_none() {
            assert_eq!(state, MenuRowState::Disabled, "{} 没有真动作却可点", r.label);
        } else {
            assert_eq!(state, MenuRowState::Enabled, "{} 有动作却灰显", r.label);
        }
    }
}

/// 用户 2026-11-17：「log 和 session 菜单里面的 log 展开一致」。
///
/// 判据不是「有没有七条命令」，而是**两处读的是同一棵树**：右键里的
/// `Log ▸` 报 `children = Some("Log")`，展开时 `tree_submenu_items("Log")`
/// 从 `term_app::menu::menus()` 取到 Session 菜单下那一份，于是分隔线分组、
/// 命令 ID 与可用性天然一致——抄一份常量也能看起来一样，但那种一致是会
/// 随时间漂走的。
#[test]
fn the_log_submenu_expands_the_same_tree_as_the_session_menu() {
    let log = TERMINAL_ROWS
        .iter()
        .find(|r| r.label == "Log")
        .expect("内容菜单里要有 Log 子菜单");
    assert_eq!(log.children, Some("Log"), "Log 必须是可展开的真子菜单");
    assert!(log.submenu, "真子菜单也要画展开箭头");
    assert_eq!(row_state(log, None), MenuRowState::Enabled, "有内容却灰着");

    let items = tree_submenu_items("Log");
    let labels: Vec<&str> = items
        .iter()
        .map(|it| match it {
            term_app::menu::Item::Action { label, .. }
            | term_app::menu::Item::Check { label, .. } => *label,
            term_app::menu::Item::Separator => "|",
            term_app::menu::Item::Submenu { .. } => "<submenu>",
        })
        .collect();
    assert_eq!(
        labels,
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
        ],
        "与 Session ▸ Log 的分组必须是同一份（这条与 menu.rs 的 \
         log_submenu_matches_the_reference_four_groups 互锁）"
    );
    // 与菜单栏同一份可用性判据：能力台账说接了，展开出来才不是一排灰字。
    for it in &items {
        if let term_app::menu::Item::Action { id, .. } = it {
            assert!(
                term_app::capability::is_implemented(id),
                "{id} 在 Log 子菜单里但没有接线，展开就是灰字"
            );
        }
    }
}

/// `Transfer Binary` 也一样：展开的就是「Tool ▸ Transfer Binary」那一棵。
///
/// 视觉基线（2026-11-17 用户提供）逐行是：Send X/Y/Z 三条一组 → 分隔线 →
/// Receive X/Y/Z 三条一组 → 分隔线 → Cancel Modem（带 `Meta+C`）。
/// 分组错了不会崩、也不会少一条命令，只是「两个入口展开不一样」——
/// 那正是用户点名要修的那类缺陷，所以钉在行序上。
#[test]
fn the_transfer_binary_submenu_expands_the_same_tree_as_the_tool_menu() {
    let row = TERMINAL_ROWS
        .iter()
        .find(|r| r.label == "Transfer Binary")
        .expect("内容菜单里要有 Transfer Binary 子菜单");
    assert_eq!(row.children, Some("Transfer Binary"), "必须读菜单树那一棵");
    assert!(row.submenu, "真子菜单也要画展开箭头");
    assert_eq!(row_state(row, None), MenuRowState::Enabled, "有内容却灰着");

    let items = tree_submenu_items("Transfer Binary");
    let labels: Vec<&str> = items
        .iter()
        .map(|it| match it {
            term_app::menu::Item::Action { label, .. }
            | term_app::menu::Item::Check { label, .. } => *label,
            term_app::menu::Item::Separator => "|",
            term_app::menu::Item::Submenu { .. } => "<submenu>",
        })
        .collect();
    assert_eq!(
        labels,
        vec![
            "Send XModem",
            "Send YModem",
            "Send ZModem",
            "|",
            "Receive XModem",
            "Receive YModem",
            "Receive ZModem",
            "|",
            "Cancel Modem",
        ],
        "与 Tool ▸ Transfer Binary 的分组必须是同一份"
    );
    for it in &items {
        if let term_app::menu::Item::Action { id, .. } = it {
            assert!(
                term_app::capability::is_implemented(id),
                "{id} 在 Transfer Binary 子菜单里但没有接线，展开就是灰字"
            );
        }
    }
}

/// 「粘贴选中」与「粘贴」是两件事：前者读**屏幕选区**，后者读剪贴板。
/// 两行都接到同一个 `Work` 就等于把剪贴板内容又粘了一遍。
#[test]
fn paste_selected_text_is_not_the_same_action_as_paste() {
    let work = |rows: &[Row], label: &str| {
        rows.iter().find(|r| r.label == label).unwrap().work
    };
    assert!(matches!(work(TERMINAL_ROWS, "Paste"), Some(Work::Paste)));
    assert!(matches!(
        work(TERMINAL_ROWS, "Paste Selected Text"),
        Some(Work::PasteSelection)
    ));
    assert!(matches!(
        work(VIEW_ROWS, "Paste Selected Text"),
        Some(Work::PasteSelection)
    ));
}

/// 菜单树 → 手绘行的搬运：命令 ID 必须同时进 `gate`（判态）与 `Work::Command`
/// （派发）。只进一个的话，要么灰错（判不了态）要么点了没反应（派不出去）。
#[test]
fn tree_rows_carry_both_the_gate_and_the_dispatch_id() {
    let rows = tree_rows(&tree_submenu_items("Log"));
    assert_eq!(rows.len(), 10, "7 条命令 + 3 条分隔线");
    for r in rows.iter().filter(|r| !r.separator) {
        match (r.gate, r.work) {
            (Some(gate), Some(Work::Command(id))) => assert_eq!(gate, id),
            other => panic!("{} 的 gate/work 不成对：{:?}", r.label, other),
        }
        assert!(!r.submenu, "{} 是命令行，不该有展开箭头", r.label);
        // 快捷键与访问字母取自菜单树同一份函数（Log 七条都没有快捷键）。
        assert_eq!(r.shortcut, term_app::menu::shortcut_text(r.gate.unwrap()));
    }
    assert_eq!(rows.iter().filter(|r| r.separator).count(), 3);
}

/// 三种「清」是三条不同的动作，不是同一个动作的三个名字。
///
/// 这条同时守住内核链路：`ClearScrollback` 只丢历史、`ClearScreen` 清屏、
/// `ClearAll` 两者都清——三个 `Work` 变体各自落到 `Terminal` 上三个不同的
/// 方法（`clear_scrollback` / `clear` / `clear_screen_and_scrollback`）。
#[test]
fn the_three_clear_commands_are_three_distinct_actions() {
    let work = |label: &str| TERMINAL_ROWS.iter().find(|r| r.label == label).unwrap().work;
    assert!(matches!(work("Clear Scrollback"), Some(Work::ClearScrollback)));
    assert!(matches!(work("Clear Screen"), Some(Work::ClearScreen)));
    assert!(matches!(work("Clear All"), Some(Work::ClearAll)));
    // 上一条 / 下一条命令是**方向相反**的两条，不能都推给同一支。
    assert!(matches!(
        work("Goto Previous Command"),
        Some(Work::GotoCommand { next: false })
    ));
    assert!(matches!(
        work("Goto Next Command"),
        Some(Work::GotoCommand { next: true })
    ));
    assert!(matches!(work("Session Triggers"), Some(Work::ManageTriggers)));
}

#[test]
fn view_menu_structure_matches_the_reference() {
    let labels: Vec<&str> = VIEW_ROWS.iter().filter(|r| !r.label.is_empty()).map(|r| r.label).collect();
    assert_eq!(labels.len(), 12, "局部清晰图 12 个文字行（含 Paste Selected Text）");
    assert_eq!(VIEW_ROWS.len(), 16, "12 行 + 4 条分隔线");
    assert_eq!(VIEW_ROWS.iter().filter(|r| r.separator).count(), 4);
    assert_eq!(labels.first(), Some(&"Copy"));
    assert_eq!(labels.last(), Some(&"Full Screen"));
}

/// 2026-11-19：台账说已接线、却被手绘 disabled 挡住的「假灰」行，逐条钉住
/// gate/work 成对。它们底层全通（HighlightText / SelectCommand / Save /
/// ToggleLineEndings / ToggleWrapSymbols 全 Implemented），只差这一行接线。
#[test]
fn previously_fake_grey_rows_now_carry_a_real_command() {
    let paired = |rows: &[Row], label: &str| -> bool {
        rows.iter()
            .find(|r| r.label == label)
            .is_some_and(|r| matches!((r.gate, r.work), (Some(g), Some(Work::Command(id))) if g == id))
    };
    for (rows, label) in [
        (TERMINAL_ROWS, "Highlight Text"),
        (TERMINAL_ROWS, "Select Command"),
        (VIEW_ROWS, "Save Session"),
        (VIEW_ROWS, "Line Endings"),
        (VIEW_ROWS, "Wrap Symbols"),
    ] {
        assert!(paired(rows, label), "{label} 底层已接线，右键不能再是假灰");
    }
    // 与菜单栏同源：这些 ID 必须真的在能力台账里是 Implemented。
    for id in [
        "Text.HighlightText",
        "Text.SelectCommand",
        "Terminal.Save",
        "View.ToggleLineEndings",
        "View.ToggleWrapSymbols",
    ] {
        assert!(
            term_app::capability::is_implemented(id),
            "{id} 台账未接线，右键却接了真"
        );
    }
}

/// 2026-11-19：`Copy Command` 从真占位接成真动作（复用 OSC 133 命令边界）。
/// 它没有独立命令 ID，所以走 `Work::CopyCommand` 而不是 `Work::Command`。
#[test]
fn copy_command_is_now_a_real_action() {
    let r = TERMINAL_ROWS.iter().find(|x| x.label == "Copy Command").unwrap();
    assert!(
        matches!(r.work, Some(Work::CopyCommand)),
        "Copy Command 已接线，必须能派发（2026-11-19）"
    );
    assert_eq!(row_state(r, None), MenuRowState::Enabled);
}

/// 2026-11-19：`Search Online ▸` 与 `Log ▸` / `Transfer Binary ▸` 一样读菜单树。
#[test]
fn search_online_submenu_expands_the_same_tree_as_the_menu_bar() {
    let row = TERMINAL_ROWS
        .iter()
        .find(|r| r.label == "Search Online")
        .expect("内容菜单里要有 Search Online 子菜单");
    assert_eq!(row.children, Some("Search Online"), "必须读菜单树那一棵");
    assert_eq!(row_state(row, None), MenuRowState::Enabled, "有真子树却灰着");
    let items = tree_submenu_items("Search Online");
    assert_eq!(items.len(), 1, "Search Online 子树只有一条命令");
    assert!(term_app::capability::is_implemented("Text.SearchOnline"));
}

/// 2026-11-19：margin 栏的 Fold 列有折叠头绘制读者（`terminal/margin.rs`），
/// 从「未接线」的假灰改成与上下四列同一个 `Work::Margin`。
#[test]
fn margin_fold_column_is_wired_like_the_other_columns() {
    // margin_rows 要的是 `MarginState`；用 Default 构造一个（全关）。
    let rows = margin_rows(&term_app::margin::MarginState::default(), term_app::i18n::Lang::En);
    let fold = rows.iter().find(|r| r.label == "Margin Fold").unwrap();
    assert!(
        matches!(fold.work, Some(Work::Margin(term_app::vocab::MarginPart::Fold))),
        "Fold 列必须与其它四列一样是可点的 margin 开关"
    );
}

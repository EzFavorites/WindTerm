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
    // 列里存语义写法 `Cmd+`（平台主修饰键），展示时才折成 `Meta+` / `Ctrl+`。
    assert_eq!(find.shortcut, "Cmd+F");
    let goto = TERMINAL_ROWS.iter().find(|r| r.label == "Goto Previous Command").unwrap();
    assert_eq!(goto.shortcut, "Cmd+Shift+Left");
}

/// 右键菜单同一级不许出现重复访问键。子菜单行（`children`）也是这一级的一行，
/// 必须和其余行一起参与去重——历史上它曾绕过 `accs`、按原始基线另取位置。
#[test]
fn context_rows_have_unique_access_keys_per_level() {
    for rows in [TERMINAL_ROWS, VIEW_ROWS] {
        let shown: Vec<&'static str> = rows.iter().map(|r| t(Lang::En, r.label)).collect();
        let preferred: Vec<Option<usize>> = rows
            .iter()
            .zip(&shown)
            .map(|(r, text)| {
                if r.separator || r.swatches {
                    None
                } else if *text == r.label {
                    r.acc
                } else {
                    Some(0)
                }
            })
            .collect();
        let positions = term_app::menu::access_positions_from(&shown, &preferred);
        let mut seen = std::collections::HashSet::new();
        for (idx, (r, pos)) in rows.iter().zip(positions).enumerate() {
            if r.separator || r.swatches {
                continue;
            }
            let Some(pos) = pos else { continue };
            let ch = shown[idx][pos..].chars().next().expect("位置来自本行");
            assert!(
                seen.insert(ch.to_ascii_lowercase()),
                "{} 的访问键 {ch} 与同级重复",
                r.label
            );
        }
    }
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

/// 2026-11-26：Word Wrap 这条 ⋮ 行此前是裸 `check(...)` ——有勾、没 `work` /
/// `gate`，于是既点了没反应，悬停又说 `Not wired`，而命令其实 2026-11-24 就接了。
/// 这条钉住它和另外两行都按台账启用，且勾选走 live 而非夹具出厂字面量。
#[test]
fn view_check_rows_are_wired_and_read_live_state() {
    let ctx = egui::Context::default();
    let pool = std::sync::Arc::new(term_session::SessionPool::new());
    let win = crate::ui::lifecycle_test_support::window_with(pool, &ctx, Vec::new());
    let checked: Vec<&Row> = VIEW_ROWS.iter().filter(|r| r.check.is_some()).collect();
    assert_eq!(checked.len(), 3);
    for r in checked {
        let id = r.check_id.unwrap_or_else(|| panic!("{} 的勾没挂 live ID", r.label));
        assert_eq!(r.gate, Some(id), "{} 的判态与勾选必须是同一条命令", r.label);
        assert!(
            matches!(r.work, Some(Work::Command(w)) if w == id),
            "{} 的派发与勾选必须是同一条命令",
            r.label
        );
        // 夹具字面量只描述基线那一瞬，真值必须来自 live（此处是无标签出厂态）。
        assert!(!live_check(&win, id), "{id} 出厂态应是空框");
        let (state, why) = crate::ui::shell::action_state(&win, id);
        assert_eq!(state, MenuRowState::Enabled);
        assert_ne!(why, "Not wired (placeholder)", "{id} 已接线，不能再这么骗人");
    }
    // 夹具里 Word Wrap 写的是 true，live 读到 false——这个差就是像素变化的来源之一。
    let wrap = VIEW_ROWS.iter().find(|r| r.label == "Word Wrap").unwrap();
    assert_eq!(wrap.check, Some(true));
    assert!(!live_check(&win, wrap.check_id.unwrap()));
}

/// 2026-11-26：两条两段快捷键是**夹具字面量偏差**，不是绑定表事实。
///
/// 全局绑定表只有 `Text.ToggleWordWrap`（Word Wrap 那行）和 Alt+T 段的 Margin
/// 系列，没有 `View.ToggleLineEndings` / `View.ToggleWrapSymbols`。这里照视觉
/// 基线抄的字面量靠 `shortcut_on_platform` 的 `display(id)` 为空这条路显示。
/// 锁死这一点，免得有人以为它们与绑定表同源、或者为了「对齐」去改全局绑定表。
#[test]
fn the_two_two_stroke_view_shortcuts_are_fixture_literals() {
    use term_app::shortcut_map::{KeyBindingMap, Platform};
    let map = KeyBindingMap::new(&[], Platform::Macos);
    for (label, literal) in [
        ("Line Endings", "Alt+T, Alt+E"),
        ("Wrap Symbols", "Alt+T, Alt+W"),
    ] {
        let r = VIEW_ROWS.iter().find(|r| r.label == label).unwrap();
        assert_eq!(r.shortcut, literal, "{label} 必须照基线逐字抄");
        let id = r.gate.unwrap();
        // C2 按原版 1:1 补了绑定表条目，显示现在从映射层派生；
        // 夹具字面量与绑定表一致，不再有"表里没有、只能抄字面量"的偏差。
        assert_eq!(map.display(id), literal, "{id} 的绑定表条目应与夹具逐字一致");
        assert_eq!(
            shortcut_on_platform(r, &map),
            literal,
            "{label} 的显示应来自绑定表"
        );
        assert!(matches!(r.work, Some(Work::Command(w)) if w == id));
    }
    // 对照组：Word Wrap 走的是绑定表真值，不许被这条测试误伤成字面量偏差。
    let wrap = VIEW_ROWS.iter().find(|r| r.label == "Word Wrap").unwrap();
    assert_eq!(map.display(wrap.gate.unwrap()), "Alt+M, Alt+W");
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
            "Cancel",
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

/// 三种「清」是三条不同的命令 ID，不是同一个命令的三个名字。
///
/// 2026-11-25 起它们不再是私有 `Work` 变体，而是数据层的三个 ID，通过
/// `Work::Command(id)` → `dispatch(id)` → `Action::parse` 落到投影层三个
/// 不同的方法（`clear_scrollback` / `clear` / `clear_all`）。两处判态同源。
#[test]
fn the_three_clear_commands_are_three_distinct_actions() {
    let work = |label: &str| TERMINAL_ROWS.iter().find(|r| r.label == label).unwrap().work;
    let id = |label: &str| match work(label) {
        Some(Work::Command(id)) => id,
        other => panic!("{label} 必须走命令 ID 通道：{other:?}"),
    };
    let ids = [
        id("Clear Scrollback"),
        id("Clear Screen"),
        id("Clear All"),
    ];
    assert_eq!(ids[0], term_app::menu::context_ids::CLEAR_SCROLLBACK);
    assert_eq!(ids[1], term_app::menu::context_ids::CLEAR_SCREEN);
    assert_eq!(ids[2], term_app::menu::context_ids::CLEAR_ALL);
    assert_eq!(ids[0], "Terminal.ClearScrollback");
    assert_eq!(ids[1], "Terminal.ClearScreen");
    assert_eq!(ids[2], "Terminal.ClearAll");
    for id in ids {
        assert!(term_app::capability::is_implemented(id), "{id} 台账未接线");
    }
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
    assert_eq!(labels.len(), 13, "12 行 + C2 补回的 Copy Command（含 Paste Selected Text）");
    assert_eq!(VIEW_ROWS.len(), 17, "13 行 + 4 条分隔线");
    assert_eq!(VIEW_ROWS.iter().filter(|r| r.separator).count(), 4);
    assert_eq!(labels.first(), Some(&"Copy"));
    assert_eq!(labels[1], "Copy Command", "紧跟 Copy 的同一类复制命令");
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

/// `Copy Command` 现在是数据层命令 ID，与菜单项走同一条 `Work::Command` 派发。
#[test]
fn copy_command_is_now_a_real_action() {
    let r = TERMINAL_ROWS.iter().find(|x| x.label == "Copy Command").unwrap();
    assert!(
        matches!(r.work, Some(Work::Command(term_app::menu::context_ids::COPY_COMMAND))),
        "Copy Command 必须走命令 ID 通道"
    );
    assert_eq!(r.gate, Some(term_app::menu::context_ids::COPY_COMMAND));
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
    // 原版这一层是运行期展开的六个搜索引擎，不再是一条泛化的 Search Online。
    assert_eq!(items.len(), 6, "Search Online 子树是六个引擎");
    let labels: Vec<&str> = items
        .iter()
        .map(|it| match it {
            term_app::menu::Item::Action { label, .. }
            | term_app::menu::Item::Check { label, .. } => *label,
            term_app::menu::Item::Submenu { label, .. } => *label,
            term_app::menu::Item::Separator => "",
        })
        .collect();
    assert_eq!(
        labels,
        ["Google", "Bing", "Github", "Stackoverflow", "Wikipedia", "DuckDuckGo"]
    );
}

/// 2026-11-19：margin 栏的 Fold 列有折叠头绘制读者（`terminal/margin.rs`），
/// 从「未接线」的假灰改成与上下四列同一个 `Work::Margin`。
#[test]
fn margin_fold_column_is_wired_like_the_other_columns() {
    // margin_rows 要的是 `MarginState`；用 Default 构造一个（出厂档）。
    let rows = margin_rows(&term_app::margin::MarginState::default(), term_app::i18n::Lang::En);
    let fold = rows.iter().find(|r| r.label == "Margin Fold").unwrap();
    assert!(
        matches!(fold.work, Some(Work::Margin(term_app::vocab::MarginPart::Fold))),
        "Fold 列必须与其它四列一样是可点的 margin 开关"
    );
}

#[test]
fn right_click_shortcuts_follow_the_real_platform() {
    use term_app::shortcut_map::{KeyBindingMap, Platform};
    let find = TERMINAL_ROWS.iter().find(|r| r.label == "Find").unwrap();
    let clear = TERMINAL_ROWS.iter().find(|r| r.label == "Clear Screen").unwrap();
    let save = VIEW_ROWS.iter().find(|r| r.label == "Save Session").unwrap();
    let wrap = VIEW_ROWS.iter().find(|r| r.label == "Word Wrap").unwrap();
    let mac = KeyBindingMap::new(&[], Platform::Macos);
    let win = KeyBindingMap::new(&[], Platform::Windows);

    // macOS 就是视觉基线的那份 Meta+ 写法，重冻不需要。
    assert_eq!(shortcut_on_platform(find, &mac), "Meta+F");
    assert_eq!(shortcut_on_platform(clear, &mac), "Meta+L");
    assert_eq!(shortcut_on_platform(save, &mac), "Meta+S");

    // 真平台是 Win/Linux 时，右键列与菜单栏一样写 Ctrl；没有命令 ID 的历史行
    // （Clear Screen）也必须跟着主修饰键走，不能只修有 ID 的那几行。
    assert_eq!(shortcut_on_platform(find, &win), "Ctrl+F");
    assert_eq!(shortcut_on_platform(clear, &win), "Ctrl+L");
    assert_eq!(shortcut_on_platform(save, &win), "Ctrl+S");
    // 字面 Alt 两段键不是主修饰键，任何平台都照基线原样。
    assert_eq!(shortcut_on_platform(wrap, &win), "Alt+M, Alt+W");
}

/// 用户重绑后右键列必须跟菜单栏同一条映射：有 gate 的行问 `map.display`，
/// 不再拿基线里的出厂键硬画。否则「右键写 Meta+S、菜单写 Ctrl+Shift+S」
/// 这种两处不一致会让用户以为重绑没生效。
#[test]
fn a_user_rebind_shows_in_both_the_menu_and_the_right_click_column() {
    use term_app::shortcut_map::{KeyBindingMap, Platform};
    let save = VIEW_ROWS.iter().find(|r| r.label == "Save Session").unwrap();
    let user = vec![(
        term_app::menu::context_ids::SAVE_SESSION.to_string(),
        "Cmd+Shift+S".to_string(),
    )];
    for platform in [Platform::Macos, Platform::Windows] {
        let map = KeyBindingMap::new(&user, platform);
        let menu = map.display(term_app::menu::context_ids::SAVE_SESSION);
        assert_eq!(menu, display_chord("Cmd+Shift+S", platform));
        assert_eq!(
            shortcut_on_platform(save, &map),
            menu,
            "右键列与菜单栏必须同源：{platform:?}"
        );
    }
    // 非 gate 行仍按平台折出厂基线（`Cmd+` → `Ctrl+`），不会被重绑误伤。
    let find = TERMINAL_ROWS.iter().find(|r| r.label == "Find").unwrap();
    let win = KeyBindingMap::new(&user, Platform::Windows);
    assert_eq!(shortcut_on_platform(find, &win), "Ctrl+F");
}

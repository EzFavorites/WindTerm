//! **菜单路径**的判据：十条栅格可视化命令从「置灰」变成「点了屏幕上真的变」。
//!
//! # 这一刀要证明的三件事
//!
//! 1. `Action::parse(id)` 认得这条 ID（不是灰的）；
//! 2. `reduce` 吐出的 `Effect::SetMarks(knob)` 落到**活动标签**的 `Tab.marks`
//!    ——而且只动它自己那一项（点 `Line Endings` 顺手把 `Indent Guides` 打开，
//!    就是「改一个旋钮动了另一个」）；
//! 3. 菜单那一侧用来画勾的 [`TerminalWindow::marks_knob_on`] 与
//!    `Tab.marks` 说的是同一句话（勾与状态错开一档 = 屏幕撒谎）。
//!
//! 判据全程走 `dispatch(id)` ——**字符串入口**，不是直接调 `toggle_*`。
//! 这一点是刻意的：直接调私有方法只能证明「方法本身对」，证明不了
//! 「菜单点下去能走到这个方法」。上一版 `Text ▸ Word Wrap` 整套接线做完、
//! 点了屏幕逐字节不变，缺的正是这条端到端判据。
//!
//! 纯函数判据在 `marks_tests.rs`（`push_row_marks` 该吐哪些 `Mark`），
//! 真跑一帧数图元的判据也在那里。这个文件只管**入口 → 状态**这一段。
//!
//! # 为什么单独成文件
//!
//! `marks_tests.rs` 已到 476 代码行（4j 上限 500），再塞一条会顶破；
//! 而这条用例需要真窗口（`lifecycle_test_support`），与那边的纯函数夹具
//! 不是一类东西。

use crate::ui::lifecycle_test_support::{fake_tab, window_with, CloseCount};
use crate::ui::terminal::{MarksParams, WhitespaceMode};
use crate::ui::TerminalWindow;

use super::marks::COLUMN_MARKER_DEFAULT;
use term_app::shell::MarksKnob;

/// 出厂 prefs 的 marks 快照（原版高亮项默认勾选），用于替代旧的"全关"期望。
fn factory_marks() -> MarksParams {
    super::setup::marks_defaults(&term_runtime::prefs::Prefs::default())
}

/// 两条标签的窗口：第 0 条是活动标签，第 1 条用来证明「只改活动那一条」。
fn two_tab_window(ctx: &egui::Context) -> TerminalWindow {
    let pool = std::sync::Arc::new(term_session::SessionPool::new());
    let closes = CloseCount::default();
    let first = fake_tab(&pool, ctx, 0, &closes);
    let second = fake_tab(&pool, ctx, 1, &closes);
    window_with(pool, ctx, vec![first, second])
}

/// 两份参数里有几项不同。
///
/// 用来钉「点一下只动一个旋钮」。写成数数而不是逐字段 `assert_eq!`：六个字段
/// 逐个比要写 12 行，而且**新增第七项时不会有人记得回来补**——这条判据只在
/// 「多动了什么」时才该红，所以让它对字段个数免疫。
fn changed_fields(before: MarksParams, after: MarksParams) -> usize {
    usize::from(before.line_endings != after.line_endings)
        + usize::from(before.column_marker != after.column_marker)
        + usize::from(before.indent_guides != after.indent_guides)
        + usize::from(before.wrap_symbols != after.wrap_symbols)
        + usize::from(before.whitespace != after.whitespace)
        + usize::from(before.caret_line != after.caret_line)
}

/// 四个翻转开关 + 光标行：点一次开、再点一次关，每次只动自己那一项。
///
/// 十条里这五条是 `Toggle*` 语义（同一句话的反面），五档空白模式是另算的单选
/// ——两种语义分开测，免得一条循环把「翻转」与「设档」混成一个判据。
#[test]
fn each_toggle_reaches_the_active_tab_and_flips_only_its_own_field() {
    let ctx = egui::Context::default();
    let mut win = two_tab_window(&ctx);
    // 本用例逐项验证"点一次翻一层"，先把出厂勾选态归零。
    win.tabs[0].marks = MarksParams::default();
    assert_eq!(win.active_marks(), MarksParams::default(), "测试起点全关");
    let other_tab_before = win.tabs[1].marks;

    let toggles: [(&str, MarksKnob); 5] = [
        ("View.ToggleLineEndings", MarksKnob::LineEndings),
        ("View.ToggleIndentGuides", MarksKnob::IndentGuides),
        ("View.ToggleWrapSymbols", MarksKnob::WrapSymbols),
        ("Text.HighlightCaretLine", MarksKnob::CaretLine),
        ("View.ToggleColumnMarker", MarksKnob::ColumnMarker),
    ];

    for (id, knob) in toggles {
        let before = win.active_marks();
        assert!(!win.marks_knob_on(knob), "{id}：点之前菜单的勾不该亮着");

        win.dispatch(id);

        let after = win.active_marks();
        assert_ne!(before, after, "{id} 点下去 `Tab.marks` 一个字段都没变");
        assert_eq!(
            changed_fields(before, after),
            1,
            "{id} 只该动一个旋钮，实际动了 {} 项",
            changed_fields(before, after)
        );
        assert!(
            win.marks_knob_on(knob),
            "{id} 点完状态开了但 `marks_knob_on` 说没开——菜单的勾会画成空框"
        );

        // 再点一次回到原样：勾上 / 勾掉是同一句话的反面。
        win.dispatch(id);
        assert_eq!(win.active_marks(), before, "{id} 点两次必须回到原样");
        assert!(!win.marks_knob_on(knob), "{id} 关掉之后勾必须跟着灭");
    }

    // 参考列是唯一带「值」的开关：开的时候落在出厂列号，不是别处。
    win.dispatch("View.ToggleColumnMarker");
    assert_eq!(
        win.active_marks().column_marker,
        Some(COLUMN_MARKER_DEFAULT),
        "参考列打开时必须落在出厂列号（第 80 列），否则屏幕上那条线会跑到别处"
    );
    win.dispatch("View.ToggleColumnMarker");

    assert_eq!(win.active_marks(), MarksParams::default(), "收尾要回到全关");
    assert_eq!(
        win.tabs[1].marks, other_tab_before,
        "第二个标签的六项一个字都不许动——`View` 改的是当前会话"
    );
}

/// 五档空白模式是**互斥单选**：点哪一档就只是那一档，且只影响活动标签。
///
/// 判据分三层：档位对不对（`marks.whitespace`）、`marks_knob_on` 是否只对
/// 当前那档说「开」、以及「再点另一档不会把上一档留下」——互斥这件事只有第三
/// 层能抓住：如果实现里改成五个 bool，前两层都可能照样绿。
#[test]
fn the_five_whitespace_modes_are_mutually_exclusive_and_land_on_the_active_tab() {
    let ctx = egui::Context::default();
    let mut win = two_tab_window(&ctx);
    let other_tab_before = win.tabs[1].marks;

    // 与 `parse.rs` 的编号表同序：0 Hide All / 1 Show All / 2 In Selection /
    // 3 In Line Start / 4 In Line End。
    let rows: [(&str, WhitespaceMode); 5] = [
        ("View.WhiteSpacesHideAll", WhitespaceMode::HideAll),
        ("View.WhiteSpacesShowAll", WhitespaceMode::ShowAll),
        ("View.WhiteSpacesShowInSelection", WhitespaceMode::InSelection),
        ("View.WhiteSpacesShowInLineStart", WhitespaceMode::InLineStart),
        ("View.WhiteSpacesShowInLineEnd", WhitespaceMode::InLineEnd),
    ];

    // 从 InSelection 出发逐档点一遍（跳过第三行自己），最后回到 Hide All：
    // 起点不是 Hide All 才能看出「换档」与「本来就这一档」的区别。
    win.dispatch("View.WhiteSpacesShowInSelection");
    assert_eq!(win.active_marks().whitespace, WhitespaceMode::InSelection);

    for (id, mode) in rows {
        win.dispatch(id);
        let after = win.active_marks();
        assert_eq!(after.whitespace, mode, "{id} 落到了别的档位");

        for (probe_id, probe) in rows {
            // 五档里只有当前这一档的勾该亮着——「互斥」在屏幕上的样子。
            let want = probe == mode;
            assert_eq!(
                win.marks_knob_on(MarksKnob::Whitespace(probe.index() as u8)),
                want,
                "{id} 生效后，{probe_id} 的勾是 {}，应为 {want}",
                !want
            );
        }
    }

    // Hide All 就是出厂态：收尾点回它，`MarksParams` 必须与默认值逐字段相等
    // ——五档切换不会顺手把别的开关带开或带关。
    win.dispatch("View.WhiteSpacesHideAll");
    assert_eq!(win.active_marks(), factory_marks());
    assert_eq!(win.tabs[1].marks, other_tab_before, "另一个标签不受影响");
}

/// 没有活动标签时点这些命令**什么也不该发生**（不 panic、不改任何东西）。
///
/// 菜单能在「一个标签都没有」的窗口里点开（关掉全部标签后菜单栏还在），
/// 那时 `tabs.get_mut(..)` 返回 `None`。这条钉的是「静默」而不是「崩」——
/// `set_marks` 的早退分支就是为它写的，但没有判据的话，谁把它改成
/// `expect` 都不会有人发现。
#[test]
fn the_marks_commands_are_silent_without_an_active_tab() {
    let ctx = egui::Context::default();
    let pool = std::sync::Arc::new(term_session::SessionPool::new());
    let mut win = window_with(pool, &ctx, Vec::new());
    assert_eq!(win.active_marks(), factory_marks(), "没标签时给出厂值");

    for id in [
        "View.ToggleLineEndings",
        "View.ToggleColumnMarker",
        "View.ToggleIndentGuides",
        "View.ToggleWrapSymbols",
        "Text.HighlightCaretLine",
        "View.WhiteSpacesShowAll",
    ] {
        win.dispatch(id);
        assert_eq!(
            win.active_marks(),
            factory_marks(),
            "{id} 在没有标签时改了东西"
        );
    }
    assert!(win.tabs.is_empty(), "不该顺手造出一条标签");
}

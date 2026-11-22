//! 折叠族九条命令的**决策**断言（`decide_fold` 的判据）。
//!
//! # 为什么这一组非有不可
//!
//! 九条命令的全部判断都压在 `decide_fold` 一个函数上：折哪个块、解哪个块、
//! 视口里算不算、往哪一行跳、什么时候该说一句话。它没有 `Tab`、没有 PTY、
//! 没有 `egui`，所以**每一条分支都能在这里逐条钉住**——反过来，若只靠真机点
//! 菜单，那七条分支里至少「最后一条输入之后不折」「已经折过的块再折一次不是
//! 错误」「视口没有下一个折叠头时不要假装跳过去」这三条会漏掉：它们在界面上
//! 的表现恰好是「按了没反应」，与「命令坏了」长得一模一样。
//!
//! 期望值一律写死成字面量而不是让被测函数自己算（那样断言恒真），并在注释里
//! 标出这个数字是怎么来的（输入行 → 块区间 → 视口交集）。

use super::decide_fold;
use term_app::folds::{block_at, blocks_from_inputs, FoldRegion, Folds};
use term_app::vocab::FoldCmd;
use term_core::coords::{HistoryRow, ViewRange, ViewRow};

/// 非负整数 → `HistoryRow`。测试里行号写错就该立刻炸，所以这里用 `expect`。
fn row(n: i64) -> HistoryRow {
    HistoryRow::new(n).expect("测试用的行号必须非负")
}

/// 输入行表。`inputs(&[0, 5])` 表示用户在第 0 行和第 5 行各敲过一次回车。
fn inputs(rows: &[i64]) -> Vec<HistoryRow> {
    rows.iter().copied().map(row).collect()
}

/// 视口区间 `[start, start + rows)`（视口行）。
fn view(start: i64, rows: i64) -> ViewRange {
    ViewRange::new(ViewRow::new(start), ViewRow::new(start + rows))
}

/// 把折叠集合摊成 `(start, end)` 元组，便于整段 `assert_eq!`。
fn spans(folds: &Folds) -> Vec<(i64, i64)> {
    folds
        .regions()
        .iter()
        .map(|r| (r.start().get(), r.end().get()))
        .collect()
}

/// 造一个已经折好像素 `spans` 里那些块的集合。
fn folded(regions: &[(i64, i64)]) -> Folds {
    let mut folds = Folds::new();
    for &(start, end) in regions {
        let region = FoldRegion::new(row(start), row(end)).expect("测试区间必须正序");
        folds.add(region).expect("测试区间必须互不重叠");
    }
    folds
}

#[test]
fn clicking_a_fold_head_toggles_exactly_that_block() {
    // 两个块：[0,4] 与 [10,19]。鼠标点第二个头（行 10）只该动第二个块。
    let ins = inputs(&[0, 5, 10, 20]);
    let empty = Folds::new();
    let d = decide_fold(FoldCmd::ToggleAt(row(10)), &ins, None, &empty, view(0, 24), 24);
    assert_eq!(spans(&d.folds), vec![(10, 19)], "点第二个头只折第二个块");
    // 再点一次同一个头 → 解开，第一个块不受影响。
    let d2 = decide_fold(
        FoldCmd::ToggleAt(row(10)),
        &ins,
        None,
        &d.folds,
        view(0, 24),
        24,
    );
    assert!(d2.folds.is_empty(), "同一个头一去一回");
    // 点不是头的位置 → 空操作且给一句话，不误折别的块。
    let d3 = decide_fold(FoldCmd::ToggleAt(row(21)), &ins, None, &empty, view(0, 24), 24);
    assert!(d3.folds.is_empty());
    assert!(d3.note.is_some(), "点在空白处要说清为什么没反应");
}

// ── 前置：块怎么算出来 ──────────────────────────────────────────────────
// 这一条是下面所有期望值的来源。它挂了，下面十几条会跟着一起红，那说明是
// 领域层变了而不是决策层变了。

#[test]
fn the_inputs_in_this_file_produce_the_blocks_the_tests_assume() {
    // [0,5] → 块 [0,4]：命令行 0 是折叠头，1..=4 是它的输出。
    assert_eq!(
        blocks_from_inputs(&inputs(&[0, 5]))
            .iter()
            .map(|b| (b.start().get(), b.end().get()))
            .collect::<Vec<_>>(),
        vec![(0, 4)]
    );
    // [0,1] 相邻 → 中间没有输出 → 不产生块。
    assert!(blocks_from_inputs(&inputs(&[0, 1])).is_empty());
    // 三条输入 → 两个块。
    assert_eq!(
        blocks_from_inputs(&inputs(&[0, 5, 10]))
            .iter()
            .map(|b| (b.start().get(), b.end().get()))
            .collect::<Vec<_>>(),
        vec![(0, 4), (5, 9)]
    );
}

// ── Fold：折光标所在的块 ────────────────────────────────────────────────

#[test]
fn fold_folds_the_block_under_the_cursor() {
    let decision = decide_fold(
        FoldCmd::Fold,
        &inputs(&[0, 5]),
        Some(row(3)),
        &Folds::new(),
        view(0, 24),
        0,
    );
    assert_eq!(spans(&decision.folds), vec![(0, 4)]);
    assert_eq!(decision.jump, None, "Fold 不移动视口");
    assert_eq!(decision.note, None, "折成功了就不该有提示");
}

#[test]
fn fold_also_hits_when_the_cursor_sits_on_the_command_line_itself() {
    // 命令刚回车完，光标就停在提示符后面（第 0 行）。这时按 Fold 必须折到
    // 这条命令的输出——否则用户最自然的操作会静默失败。
    let decision = decide_fold(
        FoldCmd::Fold,
        &inputs(&[0, 5]),
        Some(row(0)),
        &Folds::new(),
        view(0, 24),
        0,
    );
    assert_eq!(spans(&decision.folds), vec![(0, 4)]);
    assert_eq!(decision.note, None);
}

#[test]
fn fold_without_a_block_is_a_no_op_that_says_so() {
    // 两条输入紧挨着（中间没有输出）→ 没有块可折。
    let decision = decide_fold(
        FoldCmd::Fold,
        &inputs(&[0, 1]),
        Some(row(0)),
        &Folds::new(),
        view(0, 24),
        0,
    );
    assert!(decision.folds.is_empty(), "没有块就不许凭空造一个");
    assert_eq!(
        decision.note,
        Some("光标行不在任何折叠块里"),
        "点了没反应与命令坏了长得一样，必须说一句"
    );
}

#[test]
fn folding_an_already_folded_block_is_a_no_op_not_an_error() {
    // `Folds::add` 对任何相交都返回 `OverlappingFoldRegions`，而这里唯一可能的
    // 相交就是「这个块已经在集合里」——那是合法空操作。若把它当错误上报，第二次
    // 点 Fold 就会弹一句莫名其妙的错误。
    let before = folded(&[(0, 4)]);
    let decision = decide_fold(
        FoldCmd::Fold,
        &inputs(&[0, 5]),
        Some(row(3)),
        &before,
        view(0, 24),
        0,
    );
    assert_eq!(spans(&decision.folds), vec![(0, 4)], "集合原封不动");
    assert_eq!(decision.note, Some("这个块已经折起来了"));
}

// ── Unfold ─────────────────────────────────────────────────────────────

#[test]
fn unfold_opens_the_block_under_the_cursor() {
    // 光标落在块里的**任何一个位置**都要能解开：中间行 2、折叠头 0、尾行 4。
    // 头行这一格尤其不能漏——`Folds::containing` 用闭区间，头行算在块内；若谁把
    // 判据改成开区间，折叠一旦创建就再也打不开，等于自锁。
    for cursor in [2, 0, 4] {
        let decision = decide_fold(
            FoldCmd::Unfold,
            &inputs(&[0, 5]),
            Some(row(cursor)),
            &folded(&[(0, 4)]),
            view(0, 24),
            0,
        );
        assert!(decision.folds.is_empty(), "光标在 {cursor} 必须能解开");
        assert_eq!(decision.note, None, "光标在 {cursor}：解开了就不该有提示");
    }
}

#[test]
fn unfold_on_flat_ground_says_there_was_nothing_folded() {
    let decision = decide_fold(
        FoldCmd::Unfold,
        &inputs(&[0, 5]),
        Some(row(2)),
        &Folds::new(),
        view(0, 24),
        0,
    );
    assert!(decision.folds.is_empty());
    assert_eq!(decision.note, Some("光标行上没有折起来的块"));
}

#[test]
fn unfold_without_a_cursor_row_does_nothing_and_says_so() {
    let before = folded(&[(0, 4)]);
    let decision = decide_fold(
        FoldCmd::Unfold,
        &inputs(&[0, 5]),
        None,
        &before,
        view(0, 24),
        0,
    );
    assert_eq!(spans(&decision.folds), vec![(0, 4)], "拿不到光标就什么都不改");
    assert_eq!(decision.note, Some("光标行不在任何折叠块里"));
}

// ── 最后一条输入之后不折 ────────────────────────────────────────────────

#[test]
fn fold_never_touches_the_output_after_the_last_input() {
    // 第 5 行是最后一条输入，它后面那段是**正在产生**的输出——没有下一条输入
    // 来划出尾行，所以不属于任何块。这条是用户口径的边界，不能靠「大概」。
    let decision = decide_fold(
        FoldCmd::Fold,
        &inputs(&[0, 5]),
        Some(row(5)),
        &Folds::new(),
        view(0, 24),
        0,
    );
    assert!(decision.folds.is_empty());
    assert_eq!(decision.note, Some("光标行不在任何折叠块里"));
}

// ── FoldInside / UnfoldInside：只看视口 ────────────────────────────────

#[test]
fn fold_inside_takes_only_the_blocks_the_viewport_covers() {
    // 三条输入 → 块 [0,4] / [5,9] / [10,14]；视口 0..10 覆盖前两块。
    let decision = decide_fold(
        FoldCmd::FoldInside,
        &inputs(&[0, 5, 10, 15]),
        None,
        &Folds::new(),
        view(0, 10),
        0,
    );
    assert_eq!(
        spans(&decision.folds),
        vec![(0, 4), (5, 9)],
        "第三块起点 10 已经出了视口尾行 9"
    );
}

#[test]
fn fold_inside_says_so_when_the_viewport_holds_no_block() {
    // 视口落在最后一条输入之后：那段不折，所以视口里一个块也没有。
    let decision = decide_fold(
        FoldCmd::FoldInside,
        &inputs(&[0, 5]),
        None,
        &Folds::new(),
        view(6, 10),
        0,
    );
    assert!(decision.folds.is_empty());
    assert_eq!(decision.note, Some("视口里没有可折的块"));
}

#[test]
fn unfold_inside_clips_to_the_viewport() {
    // 已折两块，视口只有 5 行（历史行 0..=4）→ 只解掉 [0,4]。
    //
    // 视口必须收到**碰不到第二块**为止：`[5,9]` 的首行 5 一旦落进视口尾行之内
    // 就算交集，所以这里刻意用 0..5 而不是 0..10（上一版断言写错就错在这：
    // 0..10 的尾行是 9，第二块整块都在里面，本来就该一起解掉）。
    let decision = decide_fold(
        FoldCmd::UnfoldInside,
        &inputs(&[0, 5, 10, 15]),
        None,
        &folded(&[(0, 4), (5, 9)]),
        view(0, 5),
        0,
    );
    assert_eq!(spans(&decision.folds), vec![(5, 9)], "视口外的不许顺手解掉");
}

#[test]
fn inside_commands_convert_view_rows_into_history_rows() {
    // 这一条守的是**唯一**会让 Inside 折错块的地方：视口行与历史行差一个
    // `history`。`history = 10`、视口 `0..14`（贴底那一屏）对应的历史行是
    // `10..=23`，只有块 [10, 19] 落在那里面。
    //
    // 若实现把视口行当历史行用，区间会变成 `0..=13`，于是 [0,4] 与 [5,9] 都
    // 被误判为「在视口内」——折掉三块，用户看到的两块根本不在屏幕上。
    let decision = decide_fold(
        FoldCmd::FoldInside,
        &inputs(&[0, 5, 10, 20]),
        None,
        &Folds::new(),
        view(0, 14),
        10,
    );
    assert_eq!(
        spans(&decision.folds),
        vec![(10, 19)],
        "只允许折落在历史行 10..=23 里的那一块"
    );
}

// ── FoldAll / UnfoldAll ────────────────────────────────────────────────

#[test]
fn fold_all_ignores_the_viewport() {
    // 同一个很窄的视口，FoldAll 仍然把三块全折起来——「All」就是不筛。
    let decision = decide_fold(
        FoldCmd::FoldAll,
        &inputs(&[0, 5, 10, 15]),
        None,
        &Folds::new(),
        view(0, 1),
        0,
    );
    assert_eq!(spans(&decision.folds), vec![(0, 4), (5, 9), (10, 14)]);
}

#[test]
fn fold_all_again_says_everything_is_already_folded() {
    // 第二次点 Fold All：集合一个都没变。这句提示必须说——否则看起来完全像是
    // 命令坏了。
    let before = folded(&[(0, 4), (5, 9), (10, 14)]);
    let decision = decide_fold(
        FoldCmd::FoldAll,
        &inputs(&[0, 5, 10, 15]),
        None,
        &before,
        view(0, 24),
        0,
    );
    assert_eq!(spans(&decision.folds), vec![(0, 4), (5, 9), (10, 14)]);
    assert_eq!(decision.note, Some("所有块都已经折起来了"));
}

#[test]
fn fold_all_without_any_block_says_there_is_nothing_to_fold() {
    let decision = decide_fold(
        FoldCmd::FoldAll,
        &inputs(&[0, 1]),
        None,
        &Folds::new(),
        view(0, 24),
        0,
    );
    assert!(decision.folds.is_empty());
    assert_eq!(decision.note, Some("没有可折的块"));
}

#[test]
fn unfold_all_clears_everything_without_a_word() {
    // 「全部展开」不需要提示：用户看到的本来就是「折着的全摊开了」。
    let decision = decide_fold(
        FoldCmd::UnfoldAll,
        &inputs(&[0, 5, 10, 15]),
        None,
        &folded(&[(0, 4), (5, 9)]),
        view(0, 24),
        0,
    );
    assert!(decision.folds.is_empty());
    assert_eq!(decision.note, None);
}

// ── MarginFold：同一个入口一去一回 ─────────────────────────────────────

#[test]
fn margin_fold_toggles_the_block_under_the_cursor() {
    // 第一次：还没折 → 折起来。
    let first = decide_fold(
        FoldCmd::Toggle,
        &inputs(&[0, 5]),
        Some(row(2)),
        &Folds::new(),
        view(0, 24),
        0,
    );
    assert_eq!(spans(&first.folds), vec![(0, 4)]);
    // 第二次：已经折了 → 展开。用户连点两下回到原样。
    let second = decide_fold(
        FoldCmd::Toggle,
        &inputs(&[0, 5]),
        Some(row(2)),
        &first.folds,
        view(0, 24),
        0,
    );
    assert!(second.folds.is_empty());
    assert_eq!(second.note, None);
}

#[test]
fn margin_fold_on_the_command_line_toggles_the_block_it_heads() {
    // 光标停在命令行（折叠头）上：`Folds::containing` 认它，所以第一下是**展开**
    // 而不是再折一次——否则最自然的位置按下去反而什么都不发生。
    let decision = decide_fold(
        FoldCmd::Toggle,
        &inputs(&[0, 5]),
        Some(row(0)),
        &folded(&[(0, 4)]),
        view(0, 24),
        0,
    );
    assert!(decision.folds.is_empty());
}

// ── Goto：只挪视口，不改集合 ───────────────────────────────────────────

#[test]
fn goto_next_jumps_to_the_next_header_and_leaves_the_set_alone() {
    // 折叠块 [0,4] 与 [5,9]；视口首行在历史行 0 → 下一个折叠头是 5。
    let before = folded(&[(0, 4), (5, 9)]);
    let decision = decide_fold(
        FoldCmd::Next,
        &inputs(&[0, 5, 10]),
        Some(row(0)),
        &before,
        view(0, 24),
        0,
    );
    assert_eq!(decision.jump, Some(5), "跳转目标是视口行 5");
    assert_eq!(
        spans(&decision.folds),
        vec![(0, 4), (5, 9)],
        "跳转不许改折叠集合"
    );
    assert_eq!(decision.note, None);
}

#[test]
fn goto_next_at_the_last_header_stops_instead_of_wrapping() {
    // 视口首行已经在最后一个折叠头上 → 没有下一个。**不许环绕到第一个**：
    // 那会让「下一个」在末尾突然把视图拉回顶部，用户以为按错了键。
    let before = folded(&[(0, 4), (5, 9)]);
    let decision = decide_fold(
        FoldCmd::Next,
        &inputs(&[0, 5, 10]),
        Some(row(0)),
        &before,
        view(5, 24),
        0,
    );
    assert_eq!(decision.jump, None);
    assert_eq!(decision.note, Some("这个方向没有折叠头了"));
}

#[test]
fn goto_previous_before_the_first_header_does_not_wrap_around() {
    let before = folded(&[(0, 4)]);
    let decision = decide_fold(
        FoldCmd::Previous,
        &inputs(&[0, 5]),
        Some(row(0)),
        &before,
        view(0, 24),
        0,
    );
    assert_eq!(decision.jump, None);
    assert_eq!(decision.note, Some("这个方向没有折叠头了"));
}

#[test]
fn goto_with_an_empty_set_says_there_is_no_fold_at_all() {
    // 与上一条的区别只在措辞：一块都没折时说的是「没有折叠块」，而不是
    // 「到头了」——后者会让人以为屏幕上明明有折叠却跳不过去。
    let decision = decide_fold(
        FoldCmd::Next,
        &inputs(&[0, 5]),
        Some(row(0)),
        &Folds::new(),
        view(0, 24),
        0,
    );
    assert_eq!(decision.jump, None);
    assert_eq!(decision.note, Some("没有折叠块"));
}

#[test]
fn goto_reports_the_jump_in_view_rows_not_history_rows() {
    // `history = 10`、折叠头在历史行 12 → 视口行 2。若把历史行直接交给
    // `jump_to_row`，视图会跳到别处（多滚 10 行）。
    let before = folded(&[(12, 16)]);
    let decision = decide_fold(
        FoldCmd::Next,
        &inputs(&[0, 12, 20]),
        Some(row(12)),
        &before,
        view(0, 24),
        10,
    );
    assert_eq!(decision.jump, Some(2), "12 - history(10) = 2");
}

// ── 退化输入：一个字节都不许 panic ──────────────────────────────────────

#[test]
fn every_command_survives_an_empty_input_list() {
    // 用户刚开标签、还没敲过回车。九条命令都得能安全落地——菜单在没有输入的
    // 窗口里照样点得到。
    let commands = [
        FoldCmd::Fold,
        FoldCmd::Unfold,
        FoldCmd::FoldInside,
        FoldCmd::UnfoldInside,
        FoldCmd::FoldAll,
        FoldCmd::UnfoldAll,
        FoldCmd::Toggle,
        FoldCmd::Previous,
        FoldCmd::Next,
    ];
    for cmd in commands {
        let decision = decide_fold(cmd, &[], Some(row(0)), &Folds::new(), view(0, 24), 0);
        assert!(
            decision.folds.is_empty(),
            "{cmd:?} 在空输入上不许凭空造块"
        );
        assert_eq!(decision.jump, None, "{cmd:?} 在空输入上不该跳");
    }
}

#[test]
fn fold_family_survives_an_unknown_cursor_row() {
    // 备用屏 / resize 中间态里快照可能给不出光标行。三条**依赖光标**的命令都
    // 必须原样返回集合（不 panic、不改状态），并且留一句话——「拿不到光标」与
    // 「命令坏了」在屏幕上长得一样，静默就是让用户以为功能没做。
    //
    // Inside / All 四条**故意不在**这一组：它们的判据是视口与输入行，与光标
    // 无关，把它们也写成「不许改集合」等于把「没光标」错当成「什么都不该发生」。
    let before = folded(&[(0, 4)]);
    for cmd in [FoldCmd::Fold, FoldCmd::Unfold, FoldCmd::Toggle] {
        let decision = decide_fold(cmd, &inputs(&[0, 5]), None, &before, view(0, 24), 0);
        assert_eq!(
            spans(&decision.folds),
            vec![(0, 4)],
            "{cmd:?} 拿不到光标时不许改集合"
        );
        assert!(decision.note.is_some(), "{cmd:?} 拿不到光标时要说一句");
    }
}

#[test]
fn fold_all_on_an_empty_screen_view_does_not_panic() {
    // 视口高度为 0（窗格被拖到极小 / 首帧还没拿到尺寸）：Inside 两条要给出
    // 「视口里没有可见的行」，而不是在空区间上做减法。
    for cmd in [FoldCmd::FoldInside, FoldCmd::UnfoldInside] {
        let decision = decide_fold(
            cmd,
            &inputs(&[0, 5]),
            Some(row(2)),
            &Folds::new(),
            view(0, 0),
            0,
        );
        assert!(decision.folds.is_empty());
        assert_eq!(decision.note, Some("视口里没有可见的行"), "{cmd:?}");
    }
}

#[test]
fn inside_before_the_first_history_row_says_nothing_is_visible() {
    // 视口整段落在历史第一条**之前**（内核刚清屏、历史被裁短的那一帧）：
    // `view_to_history` 两端都返回 `None`，「视口里看得见的行」还不存在。
    //
    // 换算：视口行 -110..-100 加回 `history = 100` → 历史行 -10..-1，全在 0 之前。
    // 若实现把负的历史行当成合法行（`HistoryRow` 本身不允许负值，这里先于构造
    // 就断了），会得到一个与任何块都无交集的区间——结果同样是 0 段；所以这条
    // 只钉行为（不折、要说话），不钉内部走了哪个分支。
    let decision = decide_fold(
        FoldCmd::FoldInside,
        &inputs(&[0, 5]),
        None,
        &Folds::new(),
        view(-110, 10),
        100,
    );
    assert!(decision.folds.is_empty());
    assert_eq!(decision.note, Some("视口里没有可见的行"));
}

// ── 交叉：决策与领域层的 `block_at` 必须同源 ───────────────────────────

#[test]
fn fold_uses_the_same_block_at_as_the_domain_layer() {
    // 若有人在这里另算一份边界（例如把块尾写成 `b` 而不是 `b - 1`），
    // 这条会红：期望值直接来自领域层的 `block_at`。
    let list = inputs(&[0, 5]);
    let expected = block_at(&list, row(3)).expect("第 3 行在块里");
    let decision = decide_fold(
        FoldCmd::Fold,
        &list,
        Some(row(3)),
        &Folds::new(),
        view(0, 24),
        0,
    );
    assert_eq!(
        spans(&decision.folds),
        vec![(expected.start().get(), expected.end().get())]
    );
}

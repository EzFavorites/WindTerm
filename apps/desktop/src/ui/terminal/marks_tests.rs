//! [`super`]（`marks.rs`）的判据：五项可视化各画在哪一格、夹值、以及「点了
//! 屏幕到底哪一像素会变」。
//!
//! 单独一个文件只是因为 `marks.rs` 的文档与实现已经占满一屏——判据与实现在同一个
//! `#[path]` 模块里，`use super::*` 照样拿得到私有项。

use super::*;

/// 12 列的夹具行：`text` 之后的格子是默认空白（`' '`）。
fn row(text: &str, cols: usize) -> Vec<Cell> {
    let mut cells: Vec<Cell> = text.chars().map(|ch| Cell { ch, ..Cell::default() }).collect();
    cells.resize(cols, Cell::default());
    cells
}

fn plain(cells: &[Cell]) -> RowLine<'_> {
    RowLine {
        cells,
        continuation: false,
        next_continuation: false,
        selected: &[],
    }
}

fn marks_of(params: MarksParams, line: &RowLine<'_>) -> Vec<Mark> {
    let mut out = Vec::new();
    push_row_marks(&params, line, &mut out);
    out
}

fn dots(params: MarksParams, text: &str) -> Vec<usize> {
    let cells = row(text, 12);
    marks_of(params, &plain(&cells))
        .into_iter()
        .filter_map(|m| match m {
            Mark::SpaceDot { col } => Some(col),
            _ => None,
        })
        .collect()
}

fn whitespace(mode: WhitespaceMode) -> MarksParams {
    MarksParams {
        whitespace: mode,
        ..MarksParams::default()
    }
}

/// 出厂六项全关 = 屏幕上**一个像素都不变**。这是五张视觉基线的前提。
#[test]
fn the_default_params_draw_nothing_at_all() {
    let p = MarksParams::default();
    assert!(!p.any_enabled(), "出厂态不该有任何一项开着");
    assert!(!p.paints_row_marks());
    assert_eq!(p.column_marker, None);
    assert_eq!(p.whitespace, WhitespaceMode::HideAll);
    let cells = row("    a b    ", 12);
    assert!(marks_of(p, &plain(&cells)).is_empty(), "全关时一行也不该有标记");
}

/// 五档 whitespace 的第一档：不画。
#[test]
fn hide_all_draws_no_whitespace_even_on_a_row_full_of_blanks() {
    assert!(dots(whitespace(WhitespaceMode::HideAll), "    a  b   ").is_empty());
}

/// 第二档「全画」：整行**所有**空白格都点，包含行首、词间与行尾。
///
/// 行尾那几个点是**有意的**：终端网格右侧的补白与程序写的尾部空格在 `Cell` 上
/// 逐字节相同（见模块头的解释），不点它这一档就永远画不出东西。
#[test]
fn show_all_dots_every_blank_cell_in_the_row() {
    // "ab cd" → 0:a 1:b 2:空 3:c 4:d，5..12 是补白。
    assert_eq!(
        dots(whitespace(WhitespaceMode::ShowAll), "ab cd"),
        vec![2, 5, 6, 7, 8, 9, 10, 11]
    );
}

/// 第三档「只画选区内的」：区间是 `[from, to)`，落在区间外的空白一个都不点。
#[test]
fn show_in_selection_dots_only_the_columns_inside_the_selection() {
    let cells = row("a b c d", 12);
    let p = whitespace(WhitespaceMode::InSelection);
    // 选 {1..=2} 这一段：只有第 1 列的空格在里面。
    let line = RowLine {
        cells: &cells,
        continuation: false,
        next_continuation: false,
        selected: &[(1, 2)],
    };
    assert_eq!(marks_of(p, &line), vec![Mark::SpaceDot { col: 1 }]);
    // 一批选区（两段）：两段里的空白都点，集合是并集。
    let line = RowLine {
        cells: &cells,
        continuation: false,
        next_continuation: false,
        selected: &[(1, 2), (5, 6)],
    };
    assert_eq!(
        marks_of(p, &line),
        vec![Mark::SpaceDot { col: 1 }, Mark::SpaceDot { col: 5 }]
    );
    // 没有选区时这一档什么都不画——「选区内的空白」在没有选区时是空集。
    assert!(marks_of(p, &plain(&cells)).is_empty());
}

/// 第四档「只画行首连续空白」：词间的那个空格不点。
#[test]
fn show_at_line_start_dots_only_the_leading_run() {
    // "    a b" → 行首 4 格空白，第 5 列（a 之后）也是空格。
    assert_eq!(
        dots(whitespace(WhitespaceMode::InLineStart), "    a b"),
        vec![0, 1, 2, 3]
    );
    // 没有缩进的行一个点也没有。
    assert!(dots(whitespace(WhitespaceMode::InLineStart), "abc").is_empty());
}

/// 第五档「只画行尾连续空白」：内容末尾之后那一段，行首与词间不点。
#[test]
fn show_at_line_end_dots_only_the_trailing_run() {
    // "ab   " → 内容到第 2 列结束，从第 2 列起都是行尾空白（含右侧补白）。
    let got = dots(whitespace(WhitespaceMode::InLineEnd), "ab");
    assert_eq!(got, (2..12).collect::<Vec<_>>());
    // 行首缩进**不**算行尾空白。
    assert!(!got.contains(&0));
}

/// Tab 与空格是两种标记：Tab 画箭头（绘制层用三条线段，不排字形）。
#[test]
fn a_tab_cell_becomes_an_arrow_while_a_space_becomes_a_dot() {
    // 0:Tab 1:space 2:'a'
    let cells = row("\t a", 12);
    let got = marks_of(whitespace(WhitespaceMode::InLineStart), &plain(&cells));
    assert_eq!(
        got,
        vec![Mark::TabArrow { col: 0 }, Mark::SpaceDot { col: 1 }],
        "Tab 必须是 TabArrow——它的画法是线段，不依赖字体有没有 → 字形"
    );
}

/// 缩进参考线：每 4 列一档，只画到缩进宽度为止。
#[test]
fn indent_guides_step_by_four_columns_and_stop_at_the_indent() {
    let guides = |text: &str| {
        let cells = row(text, 16);
        let p = MarksParams {
            indent_guides: true,
            ..MarksParams::default()
        };
        marks_of(p, &plain(&cells))
            .into_iter()
            .filter_map(|m| match m {
                Mark::IndentGuide { col } => Some(col),
                _ => None,
            })
            .collect::<Vec<_>>()
    };
    // 缩进 8 格 → 第 4、8 列各一条。
    assert_eq!(guides("        ab"), vec![4, 8]);
    // 缩进 6 格 → 只有第 4 列那条（第 8 列已经越过内容起点）。
    assert_eq!(guides("      ab"), vec![4]);
    // 缩进 3 格（不满一档）→ 一条也没有。
    assert!(guides("   ab").is_empty());
    // 没有缩进 → 一条也没有。
    assert!(guides("ab").is_empty());
    // 缩进 12 格 → 三条。
    assert_eq!(guides("            ab"), vec![4, 8, 12]);
}

/// 行尾标记只画在**逻辑行**上：续行与「被硬折的行」都不画。
#[test]
fn the_line_ending_skips_continuation_rows_and_hard_wrapped_rows() {
    let cells = row("hello", 12);
    let endings = |line: &RowLine<'_>| {
        let p = MarksParams {
            line_endings: true,
            ..MarksParams::default()
        };
        marks_of(p, line)
            .into_iter()
            .filter(|m| matches!(m, Mark::LineEnding { .. }))
            .collect::<Vec<_>>()
    };
    // 逻辑行结尾：`$` 落在最后一个字符之后那一格。
    assert_eq!(endings(&plain(&cells)), vec![Mark::LineEnding { col: 5 }]);
    // 本行是续行（上一行折下来的后半截）：不画。
    let cont = RowLine {
        continuation: true,
        ..plain(&cells)
    };
    assert!(endings(&cont).is_empty(), "续行是半行，不该有句号");
    // 本行是逻辑行开头，但下一行是续行（本行被硬折了）：也不画。
    let hard = RowLine {
        next_continuation: true,
        ..plain(&cells)
    };
    assert!(endings(&hard).is_empty(), "半个句子的中间不画句号");
}

/// 空行也有行尾标记，落在第 0 列——那正是空行的「行尾」。
#[test]
fn an_empty_row_puts_the_line_ending_at_column_zero() {
    let cells = row("", 12);
    let p = MarksParams {
        line_endings: true,
        ..MarksParams::default()
    };
    assert_eq!(marks_of(p, &plain(&cells)), vec![Mark::LineEnding { col: 0 }]);
}

/// 写满一整行的逻辑行：`$` 夹在最后一列，不会画到网格外面去。
#[test]
fn a_full_row_keeps_the_line_ending_inside_the_grid() {
    let cells = row("abcdefghijkl", 12);
    let p = MarksParams {
        line_endings: true,
        ..MarksParams::default()
    };
    assert_eq!(marks_of(p, &plain(&cells)), vec![Mark::LineEnding { col: 11 }]);
}

/// 折行标记只画在续行上，位置是这一行的最后一列。
#[test]
fn the_wrap_symbol_only_marks_continuation_rows() {
    let cells = row("tail", 12);
    let p = MarksParams {
        wrap_symbols: true,
        ..MarksParams::default()
    };
    let cont = RowLine {
        continuation: true,
        ..plain(&cells)
    };
    assert_eq!(marks_of(p, &cont), vec![Mark::WrapSymbol { col: 11 }]);
    // 逻辑行开头没有折行标记——它没有「上一行的后半截」这回事。
    assert!(marks_of(p, &plain(&cells)).is_empty());
}

/// 宽字符的后半格是**字形**不是空白：给它画点会把「中」戳一个洞。
#[test]
fn a_wide_glyph_continuation_cell_is_not_whitespace() {
    let mut cells = row("", 12);
    cells[0] = Cell {
        ch: '中',
        wide: true,
        ..Cell::default()
    };
    // cells[1] 是内核写的占位（空格）。它必须被跳过：第 0、1 两格都不该出现点。
    let got = marks_of(whitespace(WhitespaceMode::ShowAll), &plain(&cells));
    assert_eq!(
        got,
        (2..12).map(|col| Mark::SpaceDot { col }).collect::<Vec<_>>(),
        "字形占的两格都不画空白符号"
    );
    // `content_end` 把宽字符算两格：`$` 落在整个字形之后。
    assert_eq!(content_end(&cells), 2);
    assert_eq!(leading_blanks(&cells), 0);
}

/// 列标记的夹值：`0` = 关，其余夹进 `8..=512`。
#[test]
fn the_column_marker_is_clamped_into_eight_to_512() {
    let from = |n: usize| MarksParams::from_prefs(false, n, false, false, 0, false, false, false, false, false).column_marker;
    assert_eq!(from(0), None, "0 是「老文件没这项」，也是「关」");
    assert_eq!(from(80), Some(80));
    assert_eq!(from(3), Some(8), "比 8 还窄的参考线没有意义");
    assert_eq!(from(9999), Some(512), "再右就夹到上界，而不是退回 80");
    assert_eq!(from(512), Some(512));
    assert_eq!(from(8), Some(8));
    // 菜单那条路（不落盘）走同一个夹值函数。
    let on = MarksParams::default().with_column_marker(Some(1));
    assert_eq!(on.column_marker, Some(8));
    assert_eq!(on.with_column_marker(None).column_marker, None);
}

/// 网格比 N 窄时列标记不画（那条线会落在网格外面，看不出是什么）。
#[test]
fn the_column_marker_hides_itself_when_the_grid_is_narrower_than_n() {
    let p = MarksParams::default().with_column_marker(Some(80));
    assert_eq!(p.column_marker_col(120), Some(80));
    assert_eq!(p.column_marker_col(80), Some(80), "正好 80 列时线在最右边");
    assert_eq!(p.column_marker_col(40), None, "40 列的窗口里没有第 80 列");
    assert_eq!(p.column_marker_col(0), None);
}

/// 光标行底色：只在不回看时命中光标那一行，且行号夹进本屏。
#[test]
fn the_caret_band_only_lands_on_the_cursor_row_while_pinned_to_the_bottom() {
    assert_eq!(caret_band_row(true, true, 5, 24), Some(5));
    assert_eq!(caret_band_row(false, true, 5, 24), None, "开关关着");
    assert_eq!(caret_band_row(true, false, 5, 24), None, "回看时光标不在这一屏上");
    assert_eq!(caret_band_row(true, true, 99, 24), Some(23), "夹进本屏行数");
    assert_eq!(caret_band_row(true, true, 0, 0), None, "一屏 0 行时没有行可画");
}

/// `wraps` 是升序的绝对行号表，二分查就够。
#[test]
fn is_wrapped_finds_exactly_the_recorded_rows() {
    let wraps = [3i64, 4, 9];
    assert!(is_wrapped(&wraps, 3));
    assert!(is_wrapped(&wraps, 4));
    assert!(is_wrapped(&wraps, 9));
    assert!(!is_wrapped(&wraps, 5));
    assert!(!is_wrapped(&wraps, 2));
    assert!(!is_wrapped(&[], 0), "备用屏没有折行记录");
}

/// 五档的落盘编号必须可逆，越界回落第一档。
#[test]
fn every_whitespace_mode_round_trips_through_its_index() {
    for (i, mode) in WhitespaceMode::ALL.iter().enumerate() {
        assert_eq!(mode.index(), i, "{mode:?}");
        assert_eq!(WhitespaceMode::from_index(i), *mode);
    }
    assert_eq!(WhitespaceMode::from_index(9), WhitespaceMode::HideAll);
    assert_eq!(WhitespaceMode::ALL.len(), 5, "五档互斥，一档不多一档不少");
}

/// 空白符号在枚举里的次序：先空白（列升序），再缩进线，再行尾，最后折行。
///
/// 次序不是审美问题：绘制层按这个次序追加 draw call，行尾 `$` 必须压在
/// 空白符号之后才不会被同一格的点盖住。
#[test]
fn the_marks_of_a_row_come_out_in_a_fixed_order() {
    // "    a b" → 缩进 4 格、内容到第 6 列结束、第 5 列是词间空格。
    let cells = row("    a b", 16);
    let p = MarksParams {
        whitespace: WhitespaceMode::ShowAll,
        indent_guides: true,
        line_endings: true,
        wrap_symbols: true,
        ..MarksParams::default()
    };
    let cont = RowLine {
        continuation: true,
        ..plain(&cells)
    };
    let got = marks_of(p, &cont);
    assert_eq!(got[0], Mark::SpaceDot { col: 0 });
    assert_eq!(got[1], Mark::SpaceDot { col: 1 });
    assert_eq!(got[2], Mark::SpaceDot { col: 2 });
    assert_eq!(got[3], Mark::SpaceDot { col: 3 });
    assert_eq!(got[4], Mark::SpaceDot { col: 5 }, "词间那一个空格");
    // 缩进线在所有空白符号**之后**画：它比点更淡，压在点下面就看不清了。
    let guide_at = got
        .iter()
        .position(|m| matches!(m, Mark::IndentGuide { .. }))
        .expect("缩进 4 格该有一条参考线");
    let last_dot = got
        .iter()
        .rposition(|m| matches!(m, Mark::SpaceDot { .. }))
        .expect("ShowAll 该有点");
    assert!(guide_at > last_dot, "{got:?}");
    assert_eq!(got[guide_at], Mark::IndentGuide { col: 4 });
    // 续行不画行尾 `$`，折行标记在最后。
    assert_eq!(got.last(), Some(&Mark::WrapSymbol { col: 15 }));
    assert!(!got.iter().any(|m| matches!(m, Mark::LineEnding { .. })));
    // 空白符号永远是列升序。
    let cols: Vec<usize> = got
        .iter()
        .filter_map(|m| match m {
            Mark::SpaceDot { col } => Some(*col),
            _ => None,
        })
        .collect();
    assert!(cols.windows(2).all(|w| w[0] < w[1]), "{cols:?}");
}

/// **反假开关**：六项各自单独打开，都必须产出至少一个可见标记。
///
/// 这条是本轮的核心判据。上一版 `Text ▸ Word Wrap` 整套接线做完、点了屏幕
/// 逐字节不变，被主动回退；这里用一个纯函数把「每一档都有东西可画」钉死，
/// 谁把它改成恒空，这条先红。
#[test]
fn every_one_of_the_six_switches_produces_a_visible_mark_on_its_own() {
    let cells = row("    a b", 16);
    let cont = RowLine {
        continuation: true,
        ..plain(&cells)
    };
    // 行尾标记要落在**非续行**上，所以两行都准备一份。
    for (name, params) in [
        (
            "line_endings",
            MarksParams {
                line_endings: true,
                ..MarksParams::default()
            },
        ),
        (
            "indent_guides",
            MarksParams {
                indent_guides: true,
                ..MarksParams::default()
            },
        ),
        (
            "wrap_symbols",
            MarksParams {
                wrap_symbols: true,
                ..MarksParams::default()
            },
        ),
        ("whitespace", whitespace(WhitespaceMode::ShowAll)),
    ] {
        let drawn = !marks_of(params, &plain(&cells)).is_empty()
            || !marks_of(params, &cont).is_empty();
        assert!(drawn, "{name} 打开了却一个标记都画不出来");
        assert!(params.any_enabled());
    }
    // 列标记：整屏一条竖线，判据是「有列号可画」。
    let column = MarksParams {
        column_marker: Some(COLUMN_MARKER_DEFAULT),
        ..MarksParams::default()
    };
    assert!(column.any_enabled());
    assert_eq!(column.column_marker_col(100), Some(COLUMN_MARKER_DEFAULT));
    // 光标行底色：命中光标那一行。
    let caret = MarksParams {
        caret_line: true,
        ..MarksParams::default()
    };
    assert!(caret.any_enabled());
    assert!(caret_band_row(caret.caret_line, true, 2, 24).is_some());
}

/// 行首缩进与内容末尾是两个独立的量：前者数前导空白，后者找最后一个非空白。
#[test]
fn the_indent_and_the_content_end_are_counted_independently() {
    let cells = row("    ab   ", 16);
    assert_eq!(leading_blanks(&cells), 4);
    assert_eq!(content_end(&cells), 6);
    // 整行空白：缩进 = 整行，内容末尾 = 0。
    let blank = row("", 16);
    assert_eq!(leading_blanks(&blank), 16);
    assert_eq!(content_end(&blank), 0);
    // Tab 也算缩进（它在网格里占一格）。
    let tabbed = row("\tab", 16);
    assert_eq!(leading_blanks(&tabbed), 1);
}

// ── 真的画一帧：屏幕上的形状数就是「点了哪一像素会变」 ────────────────────

/// 一帧里画出来的三类图元各有几个。
///
/// 判据按**图元种类**分：空格点与光标行底色是 `Rect`、参考线与折行勾是 `Path`
/// （`Shape::line` 的产物）、行尾 `$` 是 `Text`。这样「开了这一档到底多了什么」
/// 不是靠坐标猜的，而是数得出来的。
#[derive(Debug, Default, PartialEq, Eq)]
struct Drawn {
    rects: usize,
    paths: usize,
    texts: usize,
}

/// 带内容的 80×24 夹具网格：每行 `"    hello   world"`。
///
/// 行首 4 格缩进、词间 3 个空格、内容到第 17 列结束——三种空白（行首 / 词间 /
/// 行尾）各有一段，五档模式才能互相区分开。
///
/// # 为什么过一遍真内核，而不是手填格子
///
/// 两个理由，后者是硬的：
///
/// 1. 手填的格子只有 `ch` 是对的，宽度 / 颜色 / 属性全是默认值——画出来的
///    与真会话不是一回事，夹具越省事，判据离屏幕越远；
/// 2. **4f**：`ui/` 不得直接索引 `GridSnapshot.cells`（渲染只有一个入口：
///    事实窗口），而那条判据**不排除测试文件**——`marks_tests.rs` 照样扫。
///    手填就是绕过内核给自己造事实，门禁拦得对。
///
/// 交错喂的写法是刻意对齐原来那份手填数据的：**23 个换行 + 最后一行不带换行**。
/// 24 个 `\r\n` 会让第 24 次换行把整屏滚上去一格（第 0 行内容掉进回滚区，
/// 底行留空），那时「24 行每行都是那串文本」就不成立了，而 `+18` / `+24` /
/// `24 × 70` 这些计数全要跟着变。`grid_fixture_lays_out_the_text_it_claims`
/// 反过来验这一点。
///
/// 三项内核不会替我们摆的「测量参数」：
/// - `wraps`：自动换行的续行表由调用方给（夹具要造的是「第 1/3/5 行是续行」
///   这种**几何**事实，纯层不生产它）；
/// - `cursor`：钉在第 3 行第 1 列，与内核真实光标位置无关——夹具要的是
///   「光标那一行」这件事，位置本身是任意选定的；
/// - `cursor_visible = false`：一帧里多画一个光标块会污染 `Drawn::rects` 的
///   计数，那些断言就会莫名其妙差 1。
fn grid_fixture(wraps: Vec<i64>) -> term_core::grid::GridSnapshot {
    const LINE: &str = "    hello   world";
    let mut term = term_core::vt::Terminal::new(80, 24, 0);
    for _ in 0..23 {
        term.process(format!("{LINE}\r\n").as_bytes());
    }
    // 第 24 行不带换行：否则底行被滚动空出一格，少一行内容。
    term.process(LINE.as_bytes());
    let mut snap = term.snapshot(0);
    snap.wraps = wraps;
    snap.cursor = (2, 0);
    snap.cursor_visible = false;
    snap
}

/// 夹具自检：**真的**是 24 行、每行都是那串文本。
///
/// 这条是上一条的准入条件。夹具从「手填格子」换成「喂真内核」之后，行数、
/// 行尾空白、内容末尾全由内核决定——`23 个换行 + 最后一次不带换行` 这个
/// 写法一旦被谁改成 24 个 `\r\n`，底行会被滚动空出一格，`content_end` 立刻
/// 掉到 0。那时 `$ 的数量 = +18`、`空白点 = 24 × 70` 两个断言都会以「差 18 /
/// 差 70」的形式红在**别处**，而根因在这个夹具里——所以这里先用
/// `leading_blanks` / `content_end` 这两个纯判据把它自己验一遍。
///
/// 读格子走 `row_cells(row)`（快照的公开访问器），不走 `GridSnapshot.cells`：
/// 那正是 4f 拦住的东西，测试里也不给自己开后门。
#[test]
fn grid_fixture_lays_out_the_text_it_claims() {
    let snap = grid_fixture(Vec::new());
    assert_eq!((snap.cols, snap.rows), (80, 24));
    for r in 0..24usize {
        let cells = snap.row_cells(r).expect("24 行都该取得到切片");
        assert_eq!(leading_blanks(cells), 4, "第 {r} 行缩进不是 4 格");
        assert_eq!(content_end(cells), 17, "第 {r} 行内容末尾不在第 17 列");
    }
    assert!(snap.wraps.is_empty(), "折行表由调用方给，内核不该凭空造");
    assert!(!snap.cursor_visible, "夹具关掉光标，否则 rects 计数会多一块");
}

/// 跑一帧 `draw_terminal_area_for`，数出这一帧的图元。
fn drawn(ctx: &egui::Context, win: &mut crate::ui::TerminalWindow) -> Drawn {
    let input = egui::RawInput {
        screen_rect: Some(egui::Rect::from_min_size(
            egui::Pos2::ZERO,
            egui::vec2(800.0, 600.0),
        )),
        ..Default::default()
    };
    // ADR-0025 阶段 3：绘制按身份；这条夹具只有一条标签，取它的身份。
    let tab_id = win.tabs.first().map(|t| t.identity).unwrap_or(0);
    let out = ctx.run(input, |ctx| {
        egui::CentralPanel::default().show(ctx, |ui| {
            win.draw_terminal_area_for(ui, tab_id);
        });
    });
    let mut drawn = Drawn::default();
    for clipped in &out.shapes {
        match &clipped.shape {
            egui::Shape::Rect(_) => drawn.rects += 1,
            egui::Shape::Path(_) => drawn.paths += 1,
            egui::Shape::Text(_) => drawn.texts += 1,
            _ => {}
        }
    }
    drawn
}

/// **六项开关各画在哪一像素**：真的跑一帧，数图元。
///
/// 这条是本轮的核心判据，理由见 `wrap.rs` 模块头记的那次回退：`Text ▸ Word Wrap`
/// 整套接线做完、夹具字节流一个字不改，两张截图**逐字节相同**。纯函数单测
/// 挡不住那种事——判据必须是「这一帧里真的多出了形状」。
#[test]
fn each_switch_adds_shapes_to_the_frame_it_belongs_to() {
    use crate::ui::lifecycle_test_support::{fake_tab, window_with, CloseCount};

    let ctx = egui::Context::default();
    let pool = std::sync::Arc::new(term_session::SessionPool::new());
    let closes = CloseCount::default();
    let mut tab = fake_tab(&pool, &ctx, 0, &closes);
    // 1、3、5 行是自动换行的续行（`wraps` 里的绝对行号）。
    tab.snapshot = grid_fixture(vec![1, 3, 5]);
    let mut win = window_with(pool, &ctx, vec![tab]);
    // 本用例逐项测「每打开一项多哪些图元」，先把出厂的高亮默认归零，
    // 否则基线已经带着高亮图元，后面的增量断言全部错位。
    win.tabs[0].marks = MarksParams::default();

    // 六项全关：这一帧是基线。
    let base = drawn(&ctx, &mut win);
    assert_eq!(base.paths, 0, "基线里不该有任何线段");

    // ① 行尾 `$`：每行一个 `Text`，但**续行与被硬折的行不算**。
    //
    // 夹具的 `wraps = [1,3,5]` 是刻意造的：第 1/3/5 行是续行，于是第 0/2/4 行
    // 是「写满了折下去」的硬折行。两种都不是逻辑行结尾，24 行里只有 18 行该有 `$`。
    win.toggle_line_endings();
    let with_ends = drawn(&ctx, &mut win);
    assert_eq!(
        with_ends.texts,
        base.texts + 18,
        "24 行减去 3 个续行、3 个硬折行"
    );
    win.toggle_line_endings();

    // ② 缩进参考线：缩进 4 格 → 每行第 4 列一条，24 条线段。
    win.toggle_indent_guides();
    let with_guides = drawn(&ctx, &mut win);
    assert_eq!(with_guides.paths, base.paths + 24, "每行一条缩进参考线");
    win.toggle_indent_guides();

    // ③ 折行标记：只有 3 个续行有，每个两笔。
    win.toggle_wrap_symbols();
    let with_wraps = drawn(&ctx, &mut win);
    assert_eq!(with_wraps.paths, base.paths + 6, "3 个续行 × L 形勾两笔");
    win.toggle_wrap_symbols();

    // ④ 参考列：整屏一条竖线（第 80 列的右边界）。
    win.set_column_marker(Some(80));
    let with_column = drawn(&ctx, &mut win);
    assert_eq!(with_column.paths, base.paths + 1, "贯穿全屏的一条竖线");
    win.set_column_marker(None);

    // ⑤ 光标行底色：多**一块**填充（光标在第 2 行）。
    win.toggle_caret_line();
    let with_caret = drawn(&ctx, &mut win);
    assert_eq!(with_caret.rects, base.rects + 1, "光标那一行铺一层底");
    win.toggle_caret_line();

    // ⑥ 空白符号：整行所有空白格各一个方点。
    //
    // `"    hello   world"` = 4 格行首 + 3 格词间 + 第 17 列之后的 63 格补白
    // = 70 个点，24 行共 1680。
    win.set_whitespace_mode(WhitespaceMode::ShowAll);
    let with_dots = drawn(&ctx, &mut win);
    assert_eq!(with_dots.rects, base.rects + 24 * 70, "每行 70 个空白格");
    win.set_whitespace_mode(WhitespaceMode::HideAll);

    // 全部关回来之后逐项与基线相同：这些绘制没有留下任何残留。
    assert_eq!(drawn(&ctx, &mut win), base, "关掉之后必须回到逐项相同的画面");
}

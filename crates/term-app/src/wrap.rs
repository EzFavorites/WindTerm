//! 软换行的行映射——纯函数，零 UI 依赖。
//!
//! 终端本身是**硬栅格**：一行内容只占一行屏幕，`GridSnapshot.wraps` 只是
//! 「哪些屏幕行是自动换行的续行」的标记（边栏画 `~` 用），不是渲染折行。
//! `Text.ToggleWordWrap` 要打开的是另一件事：把一个**逻辑行**按视口列宽切成
//! 若干**屏幕行**。这里只交付这张映射表（第几行、从第几列开始、占多宽），
//! 不碰像素：每列的显示宽度由调用方折算成 `1` / `2` 传进来——唯一一处替调用方
//! 折算是 [`glyph_widths`]，它只读 `Cell` 的 `wide` 位，仍然不知道颜色与字形。
//!
//! 两条硬约束：
//!
//! - **宽字符不许劈成两半**。折行点正好落在宽字符中间时，整个字推到下一段，
//!   本段末尾留一格空白——每段最多浪费 1 列（见
//!   `waste_is_at_most_one_column_per_segment` 的断言）。
//! - **空行也占一行屏幕**。`split_row(0, cols)` 产出一段 `(0, 0)`，
//!   不能凭空消失，否则屏幕行号、光标位置与滚动条全对不上。
//!
//! # 为什么 `Text.ToggleWordWrap` 还没有接线（2026-11-09 实测）
//!
//! 一次完整的接线做过又**主动回退**了：状态位、`reduce`、渲染分支、逐段绘制、
//! 甚至一个 `--demo wrap` 夹具全都写完并通过了测试，然后把夹具里那条命令去掉、
//! 字节流一个字不改重跑——**两张整窗截图逐字节相同**。
//!
//! 原因不是渲染没接上，而是坐标系还没换：内核在**同一个列宽**上已经硬折过行了
//! （`GridSnapshot.wraps` 就是那些续行），所以一条逻辑行的显示宽度永远
//! ≤ 视口列宽，`wrap_lines` 对真实内容就是**恒等映射**。接上去等于一个点了没
//! 反应的开关，而判据是「已接线 = 点了有效果」，不是「能被 `parse`」。
//!
//! 要让它有可见效果，先得让「逻辑行」跨过硬折行的边界，两条路都要动坐标系：
//!
//! 1. **reflow**：读 `wraps` 把续行拼回一条逻辑行，再按当前列宽重折。这条路原先
//!    以为只要「折行宽度 ≠ 内核当初折行的宽度」就有料可用，2026-11-14 逐行复核
//!    内核（`crates/term-core/src/vt.rs`）之后发现**原料本身已经不在了**：
//!
//!    - `resize`（`vt.rs:385-415`）是左上角锚定：开一张 `blank_grid(cols, rows)`
//!      新网，逐行按 `cols.min(self.cols).min(row.len())` 拷过去。**变窄时可见行
//!      的右半截当场丢弃**，既不重排，也不搬 scrollback。
//!    - `slice_range`（`vt.rs:476-483`）逐行只发 `0..self.cols` 格，超出的格子
//!      `row_cells.get(c)` 拿到 `None` → 补默认格，**投影处按当前列宽再截一刀**。
//!      所以**变宽方向也拼不回去**：`resize` 只是把右边补成空白格，投影又只读
//!      当前列宽，那条逻辑行的后半截没有任何一处存着。
//!    - 唯一还留着全宽的是 `scrollback` 队列：`scroll_region_up`
//!      （`vt.rs:621-635`）`push_back` 的是 `self.grid.first().cloned()`，即**滚出
//!      那一刻**的整行（当时 cols 宽）。也就是说**已滚出屏幕的历史行数据还在**，
//!      但它只被上一条的投影读着——数据在网里、出不了 `slice_range` 那道闸。
//!
//!    合起来：变窄方向数据已亡，变宽方向只剩 padding，只有已滚出的历史行还留着
//!    全宽却读不出来。要交付一致的软换行，reflow 必须先落进**内核**：`slice_range`
//!    不再按 `self.cols` 截、`resize` 真的收起 / 展开重排，并且 `cmd_marks` /
//!    `row_times` / `wrap_rows` 三张**绝对行号表**跟着重映射（重排会让行数变，而
//!    这三张表目前只靠「scrollback 裁剪时整体 -1」维持，兜不住行数增减）。
//! 2. **折行宽度可配**：按比网格更窄的宽度折，给软换行一个自己的度量。
//!
//! 两条都要求选区 / 查找高亮 / 回看 / margin 边栏从「一行 = 一行屏幕」改成
//! 「一行逻辑行 = n 段」——四处一起动才不自相矛盾。所以这不是「渲染没做完」，
//! 是**下一轮要整体迁移坐标系**；迁移完成之前，这一格保持置灰。
//!
//! 2026-11-14 判定：第 1 条要触到，必须先做**内核 reflow**（上面三处 + 三张行号
//! 表的重映射），那是独立一轮的活，**本轮搁置**。搁置只表示「这一轮不做」，不
//! 表示上面那段坐标系迁移（选区 / 查找 / 回看 / margin 四处一起改）的必要性被
//! 推翻——那四条仍然是接线的前提，只是现在连原料都还没有。

/// 一段屏幕行：来自哪个逻辑行、从该行第几列开始、占多少显示列。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct VisualRow {
    /// 逻辑行下标（`wrap_lines` 输入里的位置）。
    pub line: usize,
    /// 该逻辑行内的起始**显示列**。
    ///
    /// 注意不是 cell 下标：宽字符占两列，所以它可能跳过某个 cell 编号。
    pub start_col: usize,
    /// 这一段占的显示列数（宽字符的后半格算在内）。
    pub width: usize,
    /// 是否是同一逻辑行的后续段（首段为 `false`）。
    pub continued: bool,
}

/// 把一行按显示宽度切成若干段。返回每段的 `(起始显示列, 宽度)`。
///
/// 不知道宽字符边界——每一列都按 1 计。需要宽字符感知时用 [`wrap_lines`]，
/// 它拿到的是逐格宽度。空行（`display_width == 0`）返回 `[(0, 0)]`：
/// 空行照样占一行屏幕。`cols == 0` 时返回空 Vec（视口没有列宽就没有行可言）。
pub fn split_row(display_width: usize, cols: usize) -> Vec<(usize, usize)> {
    pack(std::iter::repeat_n(1u8, display_width), cols)
}

/// 整屏映射：`row_widths` 每一项是「这一行的每列宽度」（1 或 2）。
///
/// 逻辑行按顺序展开成屏幕行；`line` 是输入下标，`start_col` 是该行内的
/// 显示列偏移（不是段内偏移），`continued` 标出续段。没有任何逻辑行被吞掉：
/// 只要 `cols > 0`，每个输入（含空行）至少产出一段。
pub fn wrap_lines(row_widths: &[Vec<u8>], cols: usize) -> Vec<VisualRow> {
    let mut rows = Vec::new();
    for (line, widths) in row_widths.iter().enumerate() {
        let segments = pack(widths.iter().copied(), cols);
        for (i, &(start_col, width)) in segments.iter().enumerate() {
            rows.push(VisualRow {
                line,
                start_col,
                width,
                continued: i > 0,
            });
        }
    }
    rows
}

/// 一行 `Cell` 的**按字形**显示宽度表，正是 [`wrap_lines`] 要的那份输入。
///
/// # 为什么不能逐 cell 推一项
///
/// `LineSlice.cells` 是**按显示列**一格一项：一个宽字符占**两项**（`wide == true`
/// 的字形格，紧跟其后一格占位），而 [`wrap_lines`] 吃的是**按字形**一项的宽度
/// 表——宽字形写 `2`、其余写 `1`。两者差着那些占位格。写成
/// `cells.map(|c| if c.wide { 2 } else { 1 })` 会把每个宽字符算成「字形 2 + 占位
/// 1 = 3 列」，折行点整体右移，屏幕上宽字符越多偏得越远（而屏幕上看起来只是
/// 「折行位置怪怪的」，很难反推到这一行）。
///
/// 占位格的判据照 `apps/desktop/src/ui/terminal/view.rs::build_spans` 的
/// `skip_placeholder` 抄：遇到 `wide` 的格子就吃掉后面那一项。不假定占位格的
/// `ch` 是什么——内核现在填空格，但「占位格是空格」不是接口的一部分，
/// 按字符判断会在某次内核改动后静默失效。
///
/// 空行（`cells` 为空）返回空 `Vec`，然后 [`wrap_lines`] 仍会为它产出
/// `(0, 0)` 一段——空行占一行屏幕这条约束不归这里管。
pub fn glyph_widths(cells: &[term_core::grid::Cell]) -> Vec<u8> {
    // 上限：一个字形最多两个 cell，所以 `cells.len()` 是安全上界。
    let mut widths: Vec<u8> = Vec::with_capacity(cells.len());
    let mut skip_placeholder = false;
    for cell in cells {
        if skip_placeholder {
            skip_placeholder = false;
            continue;
        }
        if cell.wide {
            widths.push(2);
            skip_placeholder = true;
        } else {
            widths.push(1);
        }
    }
    widths
}

/// 贪心装箱：一格一格地往当前段里塞，塞不下就收段。
///
/// 收段条件是「这一段已经有东西，且再塞就超过 `cols`」——`used > 0` 这一半
/// 保证恰好装满时不会折出一个空段。宽字符塞不下时整格推到下一段，本段就
/// 空出一列（1/2 宽的格子最多空一列，不可能空两列）。
///
/// 一格本身就比视口还宽（`cols == 1` 遇到宽字符）是防御分支：单独成段、
/// 宁可溢出也不能不前进，否则循环永远推不动。
///
/// 零宽格子（如果调用方塞了 `0`）不占列，只推进 `cursor`；如果整行都是零宽，
/// 结尾仍产出 `(0, 0)` 一段，保证「每行至少一段」。
fn pack(widths: impl Iterator<Item = u8>, cols: usize) -> Vec<(usize, usize)> {
    if cols == 0 {
        return Vec::new();
    }
    let mut segments: Vec<(usize, usize)> = Vec::new();
    let mut start = 0usize;
    let mut cursor = 0usize;
    let mut used = 0usize;
    for width in widths {
        let width = width as usize;
        if used > 0 && used + width > cols {
            segments.push((start, used));
            start = cursor;
            used = 0;
        }
        if used == 0 && width > cols {
            // 比视口还宽的格子：独占一段（并保证 cursor 前进）。
            segments.push((cursor, width));
            cursor += width;
            start = cursor;
            continue;
        }
        cursor += width;
        used += width;
    }
    // 收尾。刚推过「超宽格」的那一段之后 used 还是 0，别再补一个空段。
    if used > 0 || segments.is_empty() {
        segments.push((start, used));
    }
    segments
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 把字符串折成逐格宽度：ASCII 记 1，其余（CJK / 全角）记 2。
    fn widths(s: &str) -> Vec<u8> {
        s.chars().map(|c| if c.is_ascii() { 1 } else { 2 }).collect()
    }

    /// 按 `line` 分组，方便逐逻辑行断言。
    fn per_line(rows: &[VisualRow]) -> Vec<Vec<VisualRow>> {
        let mut out: Vec<Vec<VisualRow>> = Vec::new();
        for row in rows {
            if out.len() <= row.line {
                out.resize(row.line + 1, Vec::new());
            }
            out[row.line].push(*row);
        }
        out
    }

    fn segments(rows: &[VisualRow]) -> Vec<(usize, usize)> {
        rows.iter().map(|r| (r.start_col, r.width)).collect()
    }

    // ---- glyph_widths：显示列 → 字形 ----

    /// 窄字符 cell。
    fn narrow(ch: char) -> term_core::grid::Cell {
        term_core::grid::Cell { ch, ..term_core::grid::Cell::default() }
    }

    /// 宽字符**字形** cell（它后面还要跟一格占位，别忘了一起给）。
    fn wide(ch: char) -> term_core::grid::Cell {
        term_core::grid::Cell { ch, wide: true, ..term_core::grid::Cell::default() }
    }

    #[test]
    fn glyph_widths_is_one_column_per_ascii_cell() {
        let cells: Vec<_> = "abc".chars().map(narrow).collect();
        assert_eq!(glyph_widths(&cells), vec![1, 1, 1]);
        // 空行：空表，交给 `wrap_lines` 产 `(0, 0)`，不在这里伪造一项。
        assert!(glyph_widths(&[]).is_empty());
    }

    #[test]
    fn glyph_widths_skips_the_placeholder_cell_after_a_wide_glyph() {
        // 4 个 cell、3 个字形：`a` + 宽字符(字形格) + 占位格 + `b`。
        let cells = vec![narrow('a'), wide('中'), narrow(' '), narrow('b')];
        assert_eq!(
            glyph_widths(&cells),
            vec![1, 2, 1],
            "占位格不许再算一列（逐 cell 一项会得到 [1,2,1,1]）"
        );
    }

    #[test]
    fn glyph_widths_counts_padding_blanks_as_columns() {
        // 行尾补齐的空格是**真的格子**，每一格占 1 列——不能当尾随空白裁掉，
        // 否则段宽对不上网格，背景块与选区会短一截。
        let cells: Vec<_> = std::iter::repeat_n(' ', 5).map(narrow).collect();
        assert_eq!(glyph_widths(&cells), vec![1, 1, 1, 1, 1]);
    }

    #[test]
    fn glyph_widths_feeds_wrap_lines_as_one_segment_when_it_fits() {
        // 走完「cells → glyph_widths → wrap_lines」这条真实数据流：
        // `a` + 宽字符 + 占位 + `b` 的**显示宽**恰好 4 列，cols = 4 下 1 段。
        let cells = vec![narrow('a'), wide('中'), narrow(' '), narrow('b')];
        let rows = wrap_lines(&[glyph_widths(&cells)], 4);
        assert_eq!(segments(&rows), vec![(0, 4)], "按字形折，恰好装满不折");
        assert_eq!(rows.len(), 1);
    }

    // ---- split_row：纯宽度切分 ----

    #[test]
    fn split_row_keeps_a_narrow_row_in_one_segment() {
        // 边界 1：窄行。
        assert_eq!(split_row(5, 10), vec![(0, 5)]);
        assert_eq!(split_row(1, 10), vec![(0, 1)]);
    }

    #[test]
    fn split_row_exactly_fits_without_an_empty_tail() {
        // 边界 2：恰好等于 cols —— 一段，不许折出 width 0 的尾巴。
        assert_eq!(split_row(10, 10), vec![(0, 10)]);
        assert_eq!(split_row(1, 1), vec![(0, 1)]);
    }

    #[test]
    fn split_row_overflows_by_one_column() {
        // 边界 3：cols + 1。
        assert_eq!(split_row(11, 10), vec![(0, 10), (10, 1)]);
    }

    #[test]
    fn split_row_chunks_evenly_into_column_width_pieces() {
        assert_eq!(split_row(25, 10), vec![(0, 10), (10, 10), (20, 5)]);
        assert_eq!(split_row(30, 10), vec![(0, 10), (10, 10), (20, 10)]);
    }

    #[test]
    fn split_row_of_an_empty_line_still_yields_one_segment() {
        // 边界 5：空行占一行屏幕，不能消失。
        assert_eq!(split_row(0, 10), vec![(0, 0)]);
    }

    #[test]
    fn split_row_is_empty_when_the_viewport_has_no_columns() {
        // 边界 6：cols == 0 —— 空 Vec，不 panic、不死循环。
        assert!(split_row(5, 0).is_empty());
        assert!(split_row(0, 0).is_empty());
    }

    // ---- wrap_lines：整屏映射 ----

    #[test]
    fn wrap_lines_keeps_each_short_line_on_one_visual_row() {
        let rows = wrap_lines(&[widths("abc"), widths("de")], 10);
        assert_eq!(
            rows,
            vec![
                VisualRow { line: 0, start_col: 0, width: 3, continued: false },
                VisualRow { line: 1, start_col: 0, width: 2, continued: false },
            ]
        );
    }

    #[test]
    fn wrap_lines_marks_continuations_and_keeps_absolute_columns() {
        let rows = wrap_lines(&[widths("0123456789012345678901234")], 10);
        assert_eq!(segments(&rows), vec![(0, 10), (10, 10), (20, 5)]);
        assert_eq!(
            rows.iter().map(|r| r.continued).collect::<Vec<_>>(),
            vec![false, true, true]
        );
        assert!(rows.iter().all(|r| r.line == 0));
    }

    #[test]
    fn wrap_lines_resets_continuation_at_each_logical_line() {
        let rows = wrap_lines(&[widths("abcdefghijkl"), widths("xyz")], 5);
        assert_eq!(
            rows.iter().map(|r| (r.line, r.continued)).collect::<Vec<_>>(),
            vec![(0, false), (0, true), (0, true), (1, false)]
        );
        assert_eq!(segments(&rows), vec![(0, 5), (5, 5), (10, 2), (0, 3)]);
    }

    #[test]
    fn wrap_lines_keeps_an_empty_line_visible() {
        // 边界 5：空行在整屏映射里也要占一段 width 0。
        let rows = wrap_lines(&[widths(""), widths("a")], 10);
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0], VisualRow { line: 0, start_col: 0, width: 0, continued: false });
        assert_eq!(rows[1], VisualRow { line: 1, start_col: 0, width: 1, continued: false });
    }

    #[test]
    fn wrap_lines_returns_nothing_for_zero_columns() {
        // 边界 6：视口没有列宽时整屏映射为空（连空行都不产出）。
        assert!(wrap_lines(&[widths("abc"), widths(""), widths("de")], 0).is_empty());
    }

    #[test]
    fn wrap_lines_of_no_lines_is_empty() {
        assert!(wrap_lines(&[], 10).is_empty());
    }

    // ---- 宽字符：不许劈成两半 ----

    #[test]
    fn wide_chars_pack_onto_a_full_row_when_they_fit() {
        let rows = wrap_lines(&[widths("ab汉")], 4);
        assert_eq!(segments(&rows), vec![(0, 4)]);
        assert_eq!(rows.len(), 1, "恰好奇偶对齐时不许折");
    }

    #[test]
    fn wide_char_at_the_last_free_column_is_pushed_down() {
        // 边界 4：只剩 1 列，宽字符要 2 列 —— 整字推下去，本段留白 1 列。
        let rows = wrap_lines(&[widths("abc汉")], 4);
        assert_eq!(segments(&rows), vec![(0, 3), (3, 2)]);
        assert_eq!(
            rows.iter().map(|r| r.continued).collect::<Vec<_>>(),
            vec![false, true]
        );
    }

    #[test]
    fn wide_char_never_gets_split_across_two_segments() {
        // 边界 4 的主诉：窄行 1 列 + 宽字符，若按「宽度硬切」就会把汉劈成两半。
        let rows = wrap_lines(&[widths("a汉")], 2);
        assert_eq!(segments(&rows), vec![(0, 1), (1, 2)]);
        assert_eq!(rows[0].width, 1, "前一段末尾空出一列，而不是塞半个字");
        assert_eq!(rows[1].width, 2, "宽字符整格落在下一段");
    }

    #[test]
    fn a_cell_wider_than_the_whole_viewport_gets_its_own_segment() {
        // 防御分支：cols == 1 遇到宽字符，宁可溢出也不能不前进（否则死循环）。
        let rows = wrap_lines(&[vec![2u8]], 1);
        assert_eq!(segments(&rows), vec![(0, 2)]);
        let rows = wrap_lines(&[vec![2u8, 2u8]], 1);
        assert_eq!(segments(&rows), vec![(0, 2), (2, 2)]);
    }

    // ---- 不变量 ----

    #[test]
    fn segments_never_lose_a_column() {
        // 边界 7：总宽守恒 —— 同一逻辑行所有段的 width 之和 == 该行总宽
        // （宽字符的「浪费」是排版空位，不是丢格：段的 width 仍按原宽记）。
        let cases: Vec<(Vec<Vec<u8>>, usize)> = vec![
            (vec![widths("abc"), widths(""), widths("0123456789")], 4),
            (vec![widths("a汉b汉汉c")], 3),
            (vec![widths("汉汉汉")], 2),
            (vec![vec![], vec![]], 7),
            (vec![widths("...."), widths("汉")], 1),
        ];
        for (lines, cols) in cases {
            for (i, group) in per_line(&wrap_lines(&lines, cols)).iter().enumerate() {
                let total: usize = lines[i].iter().map(|&w| w as usize).sum();
                let got: usize = group.iter().map(|r| r.width).sum();
                assert_eq!(got, total, "逻辑行 {i}（cols = {cols}）丢了列");
            }
        }
    }

    #[test]
    fn waste_is_at_most_one_column_per_segment() {
        // 边界 7 的另一半：只有宽字符会留下空位，而且每段最多留 1 列。
        let cases: Vec<(Vec<Vec<u8>>, usize)> = vec![
            (vec![widths("abcdefgh"), widths("汉汉汉")], 4),
            (vec![widths("a汉b")], 3),
            (vec![widths("abc")], 10),
            (vec![widths("a汉b汉c汉")], 5),
        ];
        for (lines, cols) in cases {
            for group in per_line(&wrap_lines(&lines, cols)) {
                for (i, row) in group.iter().enumerate() {
                    assert!(row.width <= cols, "段宽 {} 超出视口 {}", row.width, cols);
                    if i + 1 < group.len() {
                        let wasted = cols - row.width;
                        assert!(wasted <= 1, "非末段浪费了 {wasted} 列（cols = {cols}）");
                    }
                }
            }
        }
    }

    #[test]
    fn every_line_produces_at_least_one_visual_row() {
        let lines = vec![widths(""), widths("a"), widths("0123456789ABCDEF")];
        let rows = wrap_lines(&lines, 5);
        let groups = per_line(&rows);
        assert_eq!(groups.len(), lines.len());
        assert!(groups.iter().all(|g| !g.is_empty()));
        // 行下标必须按顺序出现，不能跳号。
        assert!(rows.windows(2).all(|w| w[0].line <= w[1].line));
    }

    #[test]
    fn segments_advance_without_overlapping_or_leaving_a_gap() {
        for (lines, cols) in [
            (vec![widths("0123456789abcdef")], 6usize),
            (vec![widths("a汉b汉c汉d")], 4usize),
        ] {
            for group in per_line(&wrap_lines(&lines, cols)) {
                for pair in group.windows(2) {
                    let (a, b) = (pair[0], pair[1]);
                    assert!(b.start_col >= a.start_col + a.width, "段之间重叠了");
                    assert!(
                        b.start_col <= a.start_col + a.width + 1,
                        "段之间空出了不止 1 列"
                    );
                }
            }
        }
    }
}

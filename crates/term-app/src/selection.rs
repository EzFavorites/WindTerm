//! F1：选区——纯函数，零 UI 依赖。
//!
//! 选区锚在**内容行号**上（与 `ScreenImage.range` 同一坐标系，负数即历史行），
//! 而不是像素或屏幕行，所以滚动不会让选区跟着漂。
//!
//! 选区有**两种形状**（F4）：
//!
//! - `Shape::Linear`：从 anchor 读到 head 的一条连续文本，跨行时中间行整行选中；
//! - `Shape::Block`：矩形块（Alt+拖动）。每一行只取 `[min_col, max_col)`，
//!   不因为跨行就把中间行整行吞掉——这正是「复制表格某一列」需要的语义。
//!
//! 提取文本时落在当前窗口之外的行不参与——用户看到什么就复制什么，
//! 不做隐藏补全。
//!
//! 起这里也是**选区能力的入口**：17 条「选择」菜单命令与 Shift+方向键共用
//! 一份位移规则（见 `Op` / `apply`）。菜单与键盘走同一条纯函数路径，所以
//! 「点菜单下一行」和「按 Shift+↓」不可能出现两种行为。
//!
//! # 文件布局
//!
//! 本文件是**模块根**（路径仍是 `selection.rs`，不改 `mod.rs`：门禁按路径写死）。
//! 三个数据模型留在这里，其余按关注点分到同名目录：
//!
//! - `selection/geometry.rs`：内容行号 / 窗口边界 / 列号的换算，以及 2026-11-13
//!   接线的七族边界判定（段落 / 半页 / 缩进 / 括号配对 / 命令块行号）；
//! - `selection/words.rs`：格 → 字形 → 词边界（双击选词、三击选行、按词移动、
//!   词段切分）；
//! - `selection/ops.rs`：43 条选择命令的位移规则、整屏全选、多选区集合与命令分派。
//!
//! 对外 API 与拆分前一致：`Pos` / `Shape` / `Selection` / `Selections` /
//! `Op` / `Cmd` / `SetOp` / `word_at` / `line_span_at` / `apply` / `apply_cmd` /
//! `parse` / `parse_set` 都还能从 `term_app::selection::…` 取到；
//! `apply_with_blocks` / `apply_cmd_with_blocks` 是「带 OSC 133 块表」的入口。

use term_core::grid::ScreenImage;

mod geometry;
mod ops;
mod words;

// 对外 API 一个都没变：数据模型在本文件，命令落地在 `ops`，
// 选词在 `words`，行列换算在 `geometry`。
pub use crate::vocab::Cmd;
pub use crate::vocab::Op;
pub use crate::vocab::SetOp;
pub use ops::{
    apply, apply_cmd, apply_cmd_with_blocks, apply_cmd_with_blocks_seps, apply_with_blocks,
    apply_with_blocks_seps, parse, parse_set, Selections,
};
pub use words::{line_span_at, separators_or_default, word_at, word_at_with, DEFAULT_SEPARATORS};
pub use geometry::{pair_end, pair_start};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Pos {
    pub row: i64,
    pub col: usize,
}

impl Pos {
    pub const fn new(row: i64, col: usize) -> Self {
        Self { row, col }
    }
}

/// 选区形状。形状属于选区本身，不是拖动时的临时参数——
/// shift-click 扩选要继续沿用原来的形状（矩形扩出来还是矩形）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Shape {
    /// 线性：一段连续文本（普通拖动、双击选词、三击选行）。
    Linear,
    /// 矩形：按列切（Alt+拖动）。
    Block,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Selection {
    anchor: Pos,
    head: Pos,
    shape: Shape,
}

impl Selection {
    /// 按下鼠标的位置。此时是塌缩选区（anchor == head）。
    pub const fn new(anchor: Pos) -> Self {
        Self {
            anchor,
            head: anchor,
            shape: Shape::Linear,
        }
    }

    /// 直接构造一段已经确定的区间（F3：双击选词 / 三击选行）。
    pub const fn span(anchor: Pos, head: Pos) -> Self {
        Self {
            anchor,
            head,
            shape: Shape::Linear,
        }
    }

    /// 把当前选区改成矩形（F4：Alt+拖动按下时决定，之后不变）。
    pub fn block(mut self) -> Self {
        self.shape = Shape::Block;
        self
    }

    /// 形状在 Linear ↔ Block 之间翻转（菜单「Switch Stream And Column」）。
    ///
    /// 端点一个都不动：流选与列选是**同一对端点的两种读法**，不是重新选一次。
    /// 所以反向拖出来的选区翻转后仍然是反向的（`normalized` / `block_bounds`
    /// 各自会把它读对）。
    pub fn toggle_shape(mut self) -> Self {
        self.shape = match self.shape {
            Shape::Linear => Shape::Block,
            Shape::Block => Shape::Linear,
        };
        self
    }

    /// anchor 与 head 互换（菜单「Exchange Caret And Anchor」）。
    pub fn swap_ends(mut self) -> Self {
        std::mem::swap(&mut self.anchor, &mut self.head);
        self
    }

    pub fn shape(&self) -> Shape {
        self.shape
    }

    pub fn drag_to(&mut self, pos: Pos) {
        self.head = pos;
    }

    pub fn anchor(&self) -> Pos {
        self.anchor
    }

    pub fn head(&self) -> Pos {
        self.head
    }

    /// 按读取顺序返回 (start, end)。
    pub fn normalized(&self) -> (Pos, Pos) {
        if (self.anchor.row, self.anchor.col) <= (self.head.row, self.head.col) {
            (self.anchor, self.head)
        } else {
            (self.head, self.anchor)
        }
    }

    /// 一次点击（没有拖动）不构成选区。
    ///
    /// 矩形的宽度为 0 时同样没有内容（只有行没有列），也当作塌缩。
    pub fn is_collapsed(&self) -> bool {
        if self.anchor == self.head {
            return true;
        }
        match self.shape {
            Shape::Linear => false,
            Shape::Block => {
                let (_, _, from, to) = self.block_bounds();
                from == to
            }
        }
    }

    /// 矩形的行列边界 `(首行, 末行, 首列, 末列)`。
    ///
    /// 注意不能用 `normalized()`：它按 (行, 列) 排序，反向拖出一块矩形时
    /// 会把列序也一起翻过来，取到的就是个空区间。列要**各自**取 min/max。
    fn block_bounds(&self) -> (i64, i64, usize, usize) {
        (
            self.anchor.row.min(self.head.row),
            self.anchor.row.max(self.head.row),
            self.anchor.col.min(self.head.col),
            self.anchor.col.max(self.head.col),
        )
    }

    /// 某一行被选中的列区间 `[start, end)`；该行不在选区内返回 `None`。
    pub fn columns_on(&self, row: i64) -> Option<(usize, usize)> {
        if self.shape == Shape::Block {
            let (first, last, from, to) = self.block_bounds();
            if row < first || row > last {
                return None;
            }
            return Some((from, to));
        }
        let (start, end) = self.normalized();
        if row < start.row || row > end.row {
            return None;
        }
        let from = if row == start.row { start.col } else { 0 };
        let mut to = if row == end.row { end.col } else { usize::MAX };
        if to < from {
            // 反向拖动时 end.col 可能小于 from（同一行内），按空区间处理。
            to = from;
        }
        Some((from, to))
    }

    pub fn contains(&self, row: i64, col: usize) -> bool {
        matches!(self.columns_on(row), Some((from, to)) if col >= from && col < to)
    }

    /// 提取选区文本。窗口外的行不参与；每行去掉尾部空格。
    ///
    /// `wraps` 是内核记录的自动换行续行表（历史绝对行号、升序，来自
    /// `GridSnapshot::wraps`）：**非首段、且这一行不是续行，才插 `\n`。**
    ///
    /// 这条判据是必须的：内核的自动换行是**真的换了一行**（`vt::mark_wrap` 把新
    /// 行号 push 进 `wraps`），所以 20 列宽打 25 个字符时屏幕上是两行。若按屏幕行
    /// 无脑 `join("\n")`，第 20 个字符处就会多出一个程序从没打印过的换行符，
    /// 粘回 PTY 时一条命令会被拆成两条执行。
    ///
    /// 累加 `String` 而不是攒 `Vec<String>` 再 `join`：要不要插分隔符取决于
    /// **相邻两行**的关系（上一行是不是续行），`join` 表达不了这件事。
    pub fn text(&self, image: &ScreenImage, wraps: &[i64]) -> String {
        if self.is_collapsed() {
            return String::new();
        }
        let mut out = String::new();
        let mut first = true;
        for line in &image.lines {
            let Some((from, to)) = self.columns_on(line.row) else {
                // 整行不在选区内：连着它的换行也不该出现，否则复制结果里会多空行。
                continue;
            };
            // 绝对行号 = 回看行数 + 视口行号，与 `ui/terminal/marks_paint.rs::abs_row`
            // 同一套换算；`line.row` 是视口行，`image.history` 是 `scrollback.len()`。
            if !first && !term_core::grid::is_continued(wraps, image.history as i64 + line.row) {
                out.push('\n');
            }
            first = false;
            let from = from.min(line.cells.len());
            let to = to.min(line.cells.len());
            if to <= from {
                // 选中了却没有列（`to <= from`）：这一行贡献一个空段。分隔符已在上面
                // 按续行判据插过，这里什么都不追——`"a\n\nb"` 这种既有结果不变。
                continue;
            }
            // 取文本的唯一口径在 term-core：跳过宽字符的延续格，否则
            // 「你好」会被复制成「你 好」。
            out.push_str(&term_core::grid::cells_text_range(&line.cells, from, to));
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parse::{selection_parse as parse, selection_set_parse as parse_set};
    use term_core::grid::{Attrs, Cell, Colour, GridSnapshot, LineSlice};

    fn cells(text: &str, cols: usize) -> Vec<Cell> {
        let mut v: Vec<Cell> = text
            .chars()
            .map(|ch| Cell {
                ch,
                fg: Colour::Default,
                bg: Colour::Default,
                attrs: Attrs::EMPTY,
                wide: false,
            })
            .collect();
        v.resize(cols, Cell::default());
        v
    }

    fn image(lines: &[&str], cols: usize, first_row: i64) -> ScreenImage {
        let rows: Vec<LineSlice> = lines
            .iter()
            .enumerate()
            .map(|(i, text)| LineSlice {
                row: first_row + i as i64,
                cells: cells(text, cols),
            })
            .collect();
        ScreenImage {
            cols,
            version: 1,
            range: first_row..first_row + lines.len() as i64,
            lines: rows,
            cursor: None,
            history: 0,
        }
    }

    #[test]
    fn a_click_without_drag_selects_nothing() {
        let sel = Selection::new(Pos::new(0, 5));
        assert!(sel.is_collapsed());
        let img = image(&["hello world"], 11, 0);
        assert_eq!(sel.text(&img, &[]), "");
    }

    #[test]
    fn single_row_selection_takes_the_half_open_range() {
        let mut sel = Selection::new(Pos::new(0, 6));
        sel.drag_to(Pos::new(0, 11));
        let img = image(&["hello world"], 11, 0);
        assert_eq!(sel.text(&img, &[]), "world");
    }

    #[test]
    fn multi_row_selection_joins_with_newlines() {
        let mut sel = Selection::new(Pos::new(0, 6));
        sel.drag_to(Pos::new(1, 5));
        let img = image(&["hello world", "hello again", "ignored"], 11, 0);
        assert_eq!(sel.text(&img, &[]), "world\nhello");
    }

    #[test]
    fn reversed_drag_is_normalized_to_reading_order() {
        let mut forward = Selection::new(Pos::new(0, 2));
        forward.drag_to(Pos::new(1, 4));
        let mut backward = Selection::new(Pos::new(1, 4));
        backward.drag_to(Pos::new(0, 2));
        let img = image(&["abcdef", "ghijkl"], 6, 0);
        assert_eq!(forward.text(&img, &[]), backward.text(&img, &[]));
        assert_eq!(forward.text(&img, &[]), "cdef\nghij");
    }

    #[test]
    fn trailing_blanks_are_trimmed_per_row() {
        let mut sel = Selection::new(Pos::new(0, 0));
        sel.drag_to(Pos::new(1, 8));
        let img = image(&["ab      ", "cd      "], 8, 0);
        assert_eq!(sel.text(&img, &[]), "ab\ncd");
    }

    // ------------------------------- 硬折行：屏幕上的换行不是程序打的换行
    //
    // 内核的自动换行是**真的换了一行**（`vt::mark_wrap`），所以 20 列宽打 25 个
    // 字符，屏幕上是两行、`wraps == [1]`。复制时若按屏幕行无脑插 `\n`，第 20 个
    // 字符处就会多出一个程序从没打印过的换行符——粘回 PTY 时一条命令会被拆成两条。

    #[test]
    fn copying_across_a_hard_wrap_does_not_add_a_newline() {
        // 20 列、第 0 行正好写满、第 1 行是续行（`wraps` 记的是历史绝对行号 1）。
        let full = "a".repeat(20);
        let img = image(&[full.as_str(), "aaaaa"], 20, 0);
        assert_eq!(img.history, 0, "视口行号与绝对行号在这一屏上重合");
        let mut sel = Selection::new(Pos::new(0, 0));
        sel.drag_to(Pos::new(1, 5));
        let result = sel.text(&img, &[1]);
        assert_eq!(result, "a".repeat(25), "折行处不得出现分隔符");
        assert!(!result.contains('\n'), "程序没打印过换行：{result:?}");
    }

    #[test]
    fn real_line_breaks_still_become_newlines() {
        // 防「把所有行都当续行」的过度修复：`wraps` 空 = 这两行是两个真行。
        let img = image(&["aaa", "bbb"], 3, 0);
        let mut sel = Selection::new(Pos::new(0, 0));
        sel.drag_to(Pos::new(1, 3));
        assert_eq!(sel.text(&img, &[]), "aaa\nbbb");
    }

    #[test]
    fn a_wrap_across_three_rows_stays_one_line() {
        // 45 个字符挤进 20 列：三次写入、两次折行，复制出来仍是一条 45 字符的串。
        let full = "a".repeat(20);
        let img = image(&[full.as_str(), full.as_str(), "aaaaa"], 20, 0);
        let mut sel = Selection::new(Pos::new(0, 0));
        sel.drag_to(Pos::new(2, 5));
        let result = sel.text(&img, &[1, 2]);
        assert_eq!(result, "a".repeat(45));
        assert!(!result.contains('\n'), "两端折行都不许留换行：{result:?}");
    }

    #[test]
    fn leading_spaces_on_a_continuation_row_are_kept() {
        // 续行里的前导空格是程序真的打印过的字节，不能被「拼接」这一步吞掉。
        let full = "a".repeat(20);
        let img = image(&[full.as_str(), "   abc"], 20, 0);
        let mut sel = Selection::new(Pos::new(0, 0));
        sel.drag_to(Pos::new(1, 6));
        assert_eq!(sel.text(&img, &[1]), format!("{full}   abc"));
    }

    #[test]
    fn wrap_lookup_uses_the_absolute_row_not_the_viewport_row() {
        // `wraps` 存的是历史绝对行号，`LineSlice::row` 是视口行：回看 3 行之后，
        // 视口的第 1 行在绝对坐标里是第 4 行。查错坐标系就会凭空多一个换行符。
        let mut img = image(&["b", "aaaaa"], 5, 0);
        img.history = 3;
        let mut sel = Selection::new(Pos::new(0, 0));
        sel.drag_to(Pos::new(1, 5));
        assert_eq!(sel.text(&img, &[4]), "baaaaa", "绝对行号 4 = 视口行 1");
        // 反向对照：拿视口行号当绝对行号来查，就会把续行当成真行。
        assert_eq!(sel.text(&img, &[1]), "b\naaaaa");
    }

    #[test]
    fn rows_outside_the_current_window_are_ignored() {
        // 选区跨 3 行，但窗口只有 2 行：只复制看得见的部分。
        let mut sel = Selection::new(Pos::new(-2, 0));
        sel.drag_to(Pos::new(1, 3));
        let img = image(&["one", "two"], 3, -1);
        assert_eq!(sel.text(&img, &[]), "one\ntwo");
    }

    #[test]
    fn columns_on_only_covers_the_selected_rows() {
        let mut sel = Selection::new(Pos::new(0, 3));
        sel.drag_to(Pos::new(2, 4));
        assert_eq!(sel.columns_on(-1), None);
        assert_eq!(sel.columns_on(0), Some((3, usize::MAX)));
        assert_eq!(sel.columns_on(1), Some((0, usize::MAX)));
        assert_eq!(sel.columns_on(2), Some((0, 4)));
        assert_eq!(sel.columns_on(3), None);
        assert!(sel.contains(1, 999));
        assert!(!sel.contains(2, 4));
    }

    #[test]
    fn wide_glyph_placeholders_are_preserved() {
        // 真实网格布局：中(0) 占位(1) 文(2) 占位(3) a(4) b(5)。
        let img = row_image(&["中文ab"], 6);
        let mut sel = Selection::new(Pos::new(0, 0));
        sel.drag_to(Pos::new(0, 6));
        assert_eq!(sel.text(&img, &[]), "中文ab", "延续格不得变成空格");
    }

    #[test]
    fn snapshot_image_is_selectable_like_any_other_window() {
        let mut snap = GridSnapshot::empty(6, 1);
        for (i, ch) in "abcdef".chars().enumerate() {
            snap.cells[i].ch = ch;
        }
        let img = snap.to_screen_image();
        let mut sel = Selection::new(Pos::new(0, 1));
        sel.drag_to(Pos::new(0, 4));
        assert_eq!(sel.text(&img, &[]), "bcd");
    }

    // ------------------------------------------------------ F3：双击 / 三击

    /// 与真实网格一致的行：宽字符占两格，第二格是延续格（`ch` 为空格、`wide=false`）。
    fn wide_row(text: &str, cols: usize) -> Vec<Cell> {
        let mut row: Vec<Cell> = Vec::new();
        for ch in text.chars() {
            let wide = term_core::width::is_wide(ch);
            row.push(Cell {
                ch,
                fg: Colour::Default,
                bg: Colour::Default,
                attrs: Attrs::EMPTY,
                wide,
            });
            if wide {
                row.push(Cell::default());
            }
        }
        row.resize(cols, Cell::default());
        row
    }

    fn row_image(lines: &[&str], cols: usize) -> ScreenImage {
        let rows: Vec<LineSlice> = lines
            .iter()
            .enumerate()
            .map(|(i, text)| LineSlice {
                row: i as i64,
                cells: wide_row(text, cols),
            })
            .collect();
        ScreenImage {
            cols,
            version: 1,
            range: 0..lines.len() as i64,
            lines: rows,
            cursor: None,
            history: 0,
        }
    }

    #[test]
    fn double_click_takes_the_token_under_the_cursor() {
        let img = image(&["hello world 42"], 14, 0);
        for col in [6, 8, 10] {
            let sel = word_at(&img, Pos::new(0, col)).expect("空白之外都该有词");
            assert_eq!(sel.text(&img, &[]), "world", "第 {col} 格应落在 world 上");
        }
        let sel = word_at(&img, Pos::new(0, 12)).unwrap();
        assert_eq!(sel.text(&img, &[]), "42");
    }

    #[test]
    fn double_click_on_whitespace_selects_nothing() {
        // 11 格刚好装下 "hello world"，第 11 格起是行尾填充。
        let img = image(&["hello world"], 14, 0);
        assert_eq!(word_at(&img, Pos::new(0, 5)), None, "词间空格");
        assert_eq!(word_at(&img, Pos::new(0, 11)), None, "行尾填充空格");
    }

    #[test]
    fn double_click_groups_runs_of_punctuation() {
        // 同类连写的标点整段选中：`---` 是三格，而不是一格。
        // （`--` 之所以同类，是因为 `-` 不在出厂分隔符串里；见下一条测试。）
        let img = image(&["a --- b"], 7, 0);
        let sel = word_at(&img, Pos::new(0, 3)).unwrap();
        assert_eq!(sel.text(&img, &[]), "---");
        // 相邻但不同类则断开：`a.b` 里的点是单独一段。
        let img = image(&["a.b"], 3, 0);
        assert_eq!(word_at(&img, Pos::new(0, 1)).unwrap().text(&img, &[]), ".");
    }

    #[test]
    fn a_factory_separator_ends_a_punctuation_run() {
        // `>` 在出厂串里，所以 `-->` 在 `>` 处断开——这正是「分隔符」的含义。
        // 旧语义（所有标点按同类连写）在设置可配之后不再自洽：同一串必须
        // 同时决定双击与按词移动，不能对某些标点网开一面。
        let img = image(&["a --> b"], 7, 0);
        assert_eq!(
            word_at(&img, Pos::new(0, 2)).unwrap().text(&img, &[]),
            "--"
        );
        assert_eq!(
            word_at(&img, Pos::new(0, 4)).unwrap().text(&img, &[]),
            ">"
        );
    }

    #[test]
    fn unlisted_ascii_punctuation_does_not_break_a_word() {
        // 出厂分隔符串里没有 `-`：`foo-bar` 因此是一个词。这正是
        // 「Word Separators」的直接含义——没列出来的字符不切断词。
        let img = image(&["foo-bar"], 10, 0);
        assert_eq!(
            word_at(&img, Pos::new(0, 0)).unwrap().text(&img, &[]),
            "foo-bar"
        );
    }

    #[test]
    fn a_configured_separator_really_breaks_the_word() {
        let img = image(&["foo-bar"], 10, 0);
        // 填了 `-` 之后，两侧各自成词；点在分隔符上仍选中它本身。
        assert_eq!(
            word_at_with(&img, Pos::new(0, 0), "-").unwrap().text(&img, &[]),
            "foo"
        );
        assert_eq!(
            word_at_with(&img, Pos::new(0, 4), "-").unwrap().text(&img, &[]),
            "bar"
        );
        assert_eq!(
            word_at_with(&img, Pos::new(0, 3), "-").unwrap().text(&img, &[]),
            "-"
        );
    }

    #[test]
    fn separators_also_drive_word_movement() {
        // 同一份串必须同时作用于双击与 Shift+Alt+方向键，否则「改了设置只有
        // 一个入口变」——那正是这次接线要消灭的分裂。
        let img = image(&["foo-bar"], 10, 0);
        let start = Some(Selection::new(Pos::new(0, 0)));
        let default_end = apply_with_blocks_seps(
            Op::WordEndNext,
            &img,
            start,
            &[],
            DEFAULT_SEPARATORS,
        )
        .unwrap();
        assert_eq!(default_end.head(), Pos::new(0, 7), "默认串下整段是一个词");
        let custom_end =
            apply_with_blocks_seps(Op::WordEndNext, &img, start, &[], "-").unwrap();
        assert_eq!(custom_end.head(), Pos::new(0, 3), "填了 `-` 之后停在 foo 末尾");
    }

    #[test]
    fn an_empty_separator_string_falls_back_to_the_factory_set() {
        // 页面在字段为空时显示出厂串；读侧必须与页面显示的是同一串。
        assert_eq!(separators_or_default(""), DEFAULT_SEPARATORS);
        assert_eq!(separators_or_default("abc"), "abc");
    }

    #[test]
    fn double_click_uses_glyph_boundaries_not_cells() {
        // 「你好，世界」：你好(4 格) ，(2 格) 世界(4 格)。
        let img = row_image(&["你好，世界"], 10);
        let sel = word_at(&img, Pos::new(0, 0)).expect("汉字是词");
        assert_eq!(sel.text(&img, &[]), "你好");
        // 点在延续格（第 1 格）上，仍属于「你」这个字形。
        let sel = word_at(&img, Pos::new(0, 1)).expect("延续格归前一个字形");
        assert_eq!(sel.text(&img, &[]), "你好");
        // 全角标点自成一段。
        let sel = word_at(&img, Pos::new(0, 4)).expect("全角逗号");
        assert_eq!(sel.text(&img, &[]), "，");
        let sel = word_at(&img, Pos::new(0, 6)).expect("第二个词");
        assert_eq!(sel.text(&img, &[]), "世界");
    }

    #[test]
    fn triple_click_takes_the_line_without_trailing_blanks() {
        let img = image(&["echo hello    ", "   "], 14, 0);
        let sel = line_span_at(&img, 0).expect("非空行应该可选");
        assert_eq!(sel.text(&img, &[]), "echo hello");
        assert_eq!(line_span_at(&img, 1), None, "空白行没有可复制的内容");
        assert_eq!(line_span_at(&img, 9), None, "窗口外的行不存在");
    }

    // ------------------------------------------------ F4：矩形（列选）

    #[test]
    fn block_selection_takes_the_same_columns_on_every_row() {
        let mut sel = Selection::new(Pos::new(0, 2)).block();
        sel.drag_to(Pos::new(2, 6));
        assert_eq!(sel.shape(), Shape::Block);
        // 中间行不再「整行吞掉」，这是与线性选区的根本区别。
        assert_eq!(sel.columns_on(0), Some((2, 6)));
        assert_eq!(sel.columns_on(1), Some((2, 6)));
        assert_eq!(sel.columns_on(2), Some((2, 6)));
        assert_eq!(sel.columns_on(3), None);
        assert_eq!(sel.columns_on(-1), None);
        assert!(sel.contains(1, 2) && sel.contains(1, 5));
        assert!(!sel.contains(1, 6), "右边界不含");
        assert!(!sel.contains(1, 1));
    }

    #[test]
    fn block_text_is_a_column_slice_per_row() {
        let img = image(&["abcdef", "ghijkl", "mnopqr"], 6, 0);
        let mut sel = Selection::new(Pos::new(0, 1)).block();
        sel.drag_to(Pos::new(2, 4));
        assert_eq!(sel.text(&img, &[]), "bcd\nhij\nnop", "第 1..4 列");
    }

    #[test]
    fn reversed_block_drag_keeps_columns_ordered() {
        // 从右下拖到左上：行、列各自取 min/max，不能像线性选区那样整体翻转。
        let img = image(&["abcdef", "ghijkl", "mnopqr"], 6, 0);
        let mut sel = Selection::new(Pos::new(2, 4)).block();
        sel.drag_to(Pos::new(0, 1));
        assert_eq!(sel.columns_on(0), Some((1, 4)));
        assert_eq!(sel.text(&img, &[]), "bcd\nhij\nnop", "第 1..4 列");
    }

    #[test]
    fn zero_width_block_counts_as_collapsed() {
        // 只拉了行没拉列：矩形没有内容，不该在复制时吐出一串空行。
        let mut sel = Selection::new(Pos::new(0, 3)).block();
        sel.drag_to(Pos::new(2, 3));
        assert!(sel.is_collapsed());
        let img = image(&["abcdef", "ghijkl", "mnopqr"], 6, 0);
        assert_eq!(sel.text(&img, &[]), "");
    }

    #[test]
    fn block_selection_skips_rows_outside_the_window() {
        // 选 4 行，但窗口只有 2 行（-1..1）：只复制看得见的那两行。
        let img = image(&["abcdef", "ghijkl"], 6, -1);
        let mut sel = Selection::new(Pos::new(-2, 1)).block();
        sel.drag_to(Pos::new(1, 4));
        assert_eq!(sel.text(&img, &[]), "bcd\nhij");
    }

    #[test]
    fn linear_selection_is_still_the_default_shape() {
        let sel = Selection::new(Pos::new(0, 0));
        assert_eq!(sel.shape(), Shape::Linear);
        assert_eq!(Selection::span(Pos::new(0, 0), Pos::new(0, 2)).shape(), Shape::Linear);
    }

    #[test]
    fn word_and_line_helpers_work_on_history_rows_too() {
        // 行号是内容行号：负数代表历史行，取行用的是事实窗口而不是快照下标。
        let img = image(&["first line", "second line"], 11, -2);
        let sel = word_at(&img, Pos::new(-2, 0)).expect("历史行也能选词");
        assert_eq!(sel.text(&img, &[]), "first");
        let sel = line_span_at(&img, -1).expect("历史行也能整行选");
        assert_eq!(sel.text(&img, &[]), "second line");
    }

    // ------------------------------------- 键盘 / 菜单的选区命令

    /// 带光标的窗口：没有现成选区时，扩选从光标处起步。
    fn image_with_cursor(lines: &[&str], cols: usize, cursor: (i64, usize)) -> ScreenImage {
        let mut img = image(lines, cols, 0);
        img.cursor = Some(cursor);
        img
    }

    #[test]
    fn every_selection_command_parses_and_nothing_else_does() {
        let ids = [
            ("Text.SelectAll", Op::All),
            ("Text.SelectWord", Op::Word),
            ("Text.SelectLine", Op::Line),
            ("Text.SelectToDocumentStart", Op::ToDocumentStart),
            ("Text.SelectToDocumentEnd", Op::ToDocumentEnd),
            ("Text.SelectToLineHome", Op::ToLineHome),
            ("Text.SelectToLineEnd", Op::ToLineEnd),
            ("Text.SelectToPreviousChar", Op::CharPrev),
            ("Text.SelectToNextChar", Op::CharNext),
            ("Text.SelectToPreviousLine", Op::LinePrev),
            ("Text.SelectToNextLine", Op::LineNext),
            ("Text.SelectToPreviousPage", Op::PagePrev),
            ("Text.SelectToNextPage", Op::PageNext),
            ("Text.SelectToPreviousWordStart", Op::WordStartPrev),
            ("Text.SelectToNextWordStart", Op::WordStartNext),
            ("Text.SelectToPreviousWordEnd", Op::WordEndPrev),
            ("Text.SelectToNextWordEnd", Op::WordEndNext),
        ];
        for (id, want) in ids {
            assert_eq!(parse(id), Some(want), "{id}");
        }
        // 仍然是占位的邻居不能被顺手接走。
        assert_eq!(parse("Text.SelectParagraph"), None);
        assert_eq!(parse("Text.SelectCommand"), None, "要 shell 集成标记");
        // 集合命令走 `parse_set`，不在位移表里——两边都认同一条 ID 就会出现
        // 「有的入口按位移解释、有的按集合解释」的第二口径。
        assert_eq!(parse("Text.InvertSelection"), None);
        assert_eq!(parse("Text.SplitSelection"), None);
    }

    #[test]
    fn every_set_command_parses_into_its_own_intent() {
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
            assert_eq!(parse_set(id), Some(want), "{id}");
        }
        // 位移表里那 21 条一个都不该被集合表认领。
        assert_eq!(parse_set("Text.SelectAll"), None);
        assert_eq!(parse_set("Text.CharLeft"), None);
        assert_eq!(parse_set("Text.SelectToNextLine"), None);
    }

    /// 14 条 Goto 命令逐条钉住 ID → `Op` 的映射。
    ///
    /// 写一张表而不是 14 个函数：这一层只做「字符串 → 既有意图」的翻译，
    /// 逐条抄一遍会让「接到相邻的 Op 上」（Word / WordEnd、Page 上下、
    /// Line 的 Home / End 最容易串）看起来像通了一样。
    #[test]
    fn every_goto_command_parses_into_an_existing_op() {
        let ids = [
            ("Text.CharLeft", Op::CharPrev),
            ("Text.CharRight", Op::CharNext),
            ("Text.LineUp", Op::LinePrev),
            ("Text.LineDown", Op::LineNext),
            // 补的 10 条
            ("Text.GotoPreviousWord", Op::WordStartPrev),
            ("Text.GotoPreviousWordEnd", Op::WordEndPrev),
            ("Text.GotoNextWord", Op::WordStartNext),
            ("Text.GotoNextWordEnd", Op::WordEndNext),
            ("Text.GotoPreviousPage", Op::PagePrev),
            ("Text.GotoNextPage", Op::PageNext),
            ("Text.GotoLineHome", Op::ToLineHome),
            ("Text.GotoLineEnd", Op::ToLineEnd),
            ("Text.GotoDocumentStart", Op::ToDocumentStart),
            ("Text.GotoDocumentEnd", Op::ToDocumentEnd),
        ];
        for (id, want) in ids {
            assert_eq!(parse(id), Some(want), "{id}");
        }
        // 同组邻居里 `Fold` 两条不在这张表上，而且**不该**在这张表上：
        // 折叠跳转移动的是视口，`Op` 的前提是「只挪选区、不动视口」。
        // 它们 2026-11-14 起走 `Action::Fold`（见 `parse::fold_parse`），
        // 所以在 `selection::parse` 这一侧仍然必须是 `None`——这条断言守的是
        // 「别顺手把它们接进 `Op`」，不是「它们还没接线」。
        for id in ["Text.GotoPreviousFold", "Text.GotoNextFold"] {
            assert_eq!(parse(id), None, "{id} 不许进选区这一支");
        }
        // 2026-11-13：其余 24 条（词段 / 半页 / 段落 / 命令 / 作用域 / 缩进 / 括号对
        // + 10 条按列选）全部接线，逐条钉在 `every_cursor_command_parses_to_its_own_op`。
        assert_eq!(parse("Text.GotoPreviousWordPart"), Some(Op::WordPartPrev));
    }

    /// 映射对不等于落点对：4 条最容易「看起来对」的命令各验一次真实位移。
    ///
    /// 断言的全是**活动选区的 head**，锚点一次都不许动——这与
    /// `Text.CharLeft/Right`、`Text.LineUp/Down` 是同一条规则（见
    /// `action.rs::goto_cursor_commands_move_the_head_through_the_same_op_path`），
    /// 终端里没有第二套光标。
    #[test]
    fn goto_commands_land_where_the_menu_says() {
        // 三行真实栅格，光标停在第一行 "two" 的中间（第 5 列）。
        let img = image_with_cursor(&["one two three", "alpha beta", "gamma delta"], 13, (0, 5));

        // Line › End Of Line：head 落到最后一个非空白字符之后，锚点仍是光标。
        let sel = apply(Op::ToLineEnd, &img, None).unwrap();
        assert_eq!(sel.head(), Pos::new(0, 13));
        assert_eq!(sel.anchor(), Pos::new(0, 5), "锚点不得跟着跑");

        // Word › Previous Word：词中间往回 → 本词词首（"two" 的第 4 列）。
        let sel = apply(Op::WordStartPrev, &img, None).unwrap();
        assert_eq!(sel.head(), Pos::new(0, 4));

        // Page › Previous Page：窗口只有 3 行，上翻一屏钳到窗口首行，
        // 不能算出负行号（屏外行复制不出来，见 `apply` 的文档）。
        let sel = apply(Op::PagePrev, &img, None).unwrap();
        assert_eq!(sel.head(), Pos::new(0, 5));

        // Document › End Of Document：最后一行最后一个非空白字符之后，
        // 且从光标到那里确实选出一段可复制的文本。
        let sel = apply(Op::ToDocumentEnd, &img, None).unwrap();
        assert_eq!(sel.head(), Pos::new(2, 11));
        assert_eq!(sel.anchor(), Pos::new(0, 5));
        assert_eq!(sel.text(&img, &[]), "wo three\nalpha beta\ngamma delta");
    }

    #[test]
    fn select_all_takes_the_window_and_skips_blank_filler_rows() {
        // 屏幕下方往往是空白填充：选进去只会在复制结果末尾留一串空行。
        let img = image(&["one", "two", "   ", ""], 5, 0);
        let sel = apply(Op::All, &img, None).unwrap();
        assert_eq!(sel.anchor(), Pos::new(0, 0));
        assert_eq!(sel.head(), Pos::new(1, 3));
        assert_eq!(sel.text(&img, &[]), "one\ntwo");
    }

    #[test]
    fn select_all_on_an_empty_screen_selects_nothing() {
        let img = image(&["   ", ""], 5, 0);
        assert_eq!(apply(Op::All, &img, None), None);
    }

    #[test]
    fn extending_moves_the_head_and_leaves_the_anchor_alone() {
        let img = image(&["abcdef"], 6, 0);
        let start = Some(Selection::span(Pos::new(0, 1), Pos::new(0, 2)));
        let sel = apply(Op::CharNext, &img, start).unwrap();
        assert_eq!(sel.anchor(), Pos::new(0, 1), "锚点不得跟着跑");
        assert_eq!(sel.head(), Pos::new(0, 3));
        // 反向：头部往回走，锚点仍然不动——不按「谁在前」重排。
        let sel = apply(Op::CharPrev, &img, Some(sel)).unwrap();
        assert_eq!(sel.anchor(), Pos::new(0, 1));
        assert_eq!(sel.head(), Pos::new(0, 2));
    }

    #[test]
    fn extending_from_nothing_starts_at_the_cursor() {
        let img = image_with_cursor(&["abcdef", "ghijkl"], 6, (1, 2));
        let sel = apply(Op::CharNext, &img, None).unwrap();
        assert_eq!(sel.anchor(), Pos::new(1, 2), "光标是默认锚点");
        assert_eq!(sel.head(), Pos::new(1, 3));
    }

    #[test]
    fn char_moves_cross_lines_and_skip_continuation_cells() {
        // 宽字符占两格：按一次右键应该跨过整个字形，而不是停在延续格上。
        let img = row_image(&["中文ab"], 6);
        let mut at = Pos::new(0, 0);
        let mut cols = Vec::new();
        for _ in 0..4 {
            let sel = apply(Op::CharNext, &img, Some(Selection::new(at))).unwrap();
            at = sel.head();
            cols.push(at.col);
        }
        assert_eq!(cols, vec![2, 4, 5, 6], "0→2→4→6，宽字符整格跳过");
        // 反向同理；行首继续往左落到上一行行尾。
        let img = image(&["abc", "def"], 6, 0);
        let sel = apply(Op::CharPrev, &img, Some(Selection::new(Pos::new(1, 0)))).unwrap();
        assert_eq!(sel.head(), Pos::new(0, 3));
        let sel = apply(Op::CharNext, &img, Some(Selection::new(Pos::new(0, 3)))).unwrap();
        assert_eq!(sel.head(), Pos::new(1, 0), "行尾继续往右落到下一行行首");
    }

    #[test]
    fn line_moves_keep_the_column_and_clamp_to_the_window() {
        let img = image(&["abcdef", "gh", "ijklmn"], 6, 0);
        let sel = apply(Op::LineNext, &img, Some(Selection::new(Pos::new(0, 4)))).unwrap();
        assert_eq!(sel.head(), Pos::new(1, 4), "下一行保持同一列，不夹回行尾");
        let sel = apply(Op::LinePrev, &img, Some(Selection::new(Pos::new(0, 4)))).unwrap();
        assert_eq!(sel.head(), Pos::new(0, 4), "已经在首行就不再动");
    }

    #[test]
    fn page_moves_are_clamped_to_the_visible_window() {
        let img = image(&["a", "b", "c", "d"], 4, 0);
        let sel = apply(Op::PageNext, &img, Some(Selection::new(Pos::new(0, 0)))).unwrap();
        assert_eq!(sel.head(), Pos::new(3, 0), "一屏 4 行，从首行直接到底");
        let sel = apply(Op::PagePrev, &img, Some(Selection::new(Pos::new(3, 0)))).unwrap();
        assert_eq!(sel.head(), Pos::new(0, 0));
    }

    #[test]
    fn line_home_and_end_use_content_not_screen_width() {
        let img = image(&["abcdef      ", "  "], 12, 0);
        let sel = apply(Op::ToLineEnd, &img, Some(Selection::new(Pos::new(0, 2)))).unwrap();
        assert_eq!(sel.head(), Pos::new(0, 6), "行尾是最后一个非空白字符");
        let sel = apply(Op::ToLineHome, &img, Some(sel)).unwrap();
        assert_eq!(sel.head(), Pos::new(0, 0));
        let sel = apply(Op::ToLineEnd, &img, Some(Selection::new(Pos::new(1, 0)))).unwrap();
        assert_eq!(sel.head(), Pos::new(1, 0), "整行空白时行尾就是行首");
    }

    #[test]
    fn document_start_and_end_follow_the_window() {
        let img = image(&["one", "two", "   "], 5, -2);
        let sel = apply(Op::ToDocumentStart, &img, Some(Selection::new(Pos::new(-1, 2)))).unwrap();
        assert_eq!(sel.head(), Pos::new(-2, 0));
        let sel = apply(Op::ToDocumentEnd, &img, Some(sel)).unwrap();
        assert_eq!(sel.head(), Pos::new(-1, 3), "末尾是最后一个有内容的行");
    }

    #[test]
    fn word_moves_land_on_word_boundaries() {
        let img = image(&["hello world 42"], 14, 0);
        let at = |sel: Selection| sel.head();
        let step = |op: Op, from: Pos| at(apply(op, &img, Some(Selection::new(from))).unwrap());

        // 词中间 / 词首 / 空白三种起点都要正常。
        assert_eq!(step(Op::WordStartNext, Pos::new(0, 2)), Pos::new(0, 6));
        assert_eq!(step(Op::WordStartNext, Pos::new(0, 0)), Pos::new(0, 6));
        assert_eq!(step(Op::WordStartNext, Pos::new(0, 5)), Pos::new(0, 6));
        assert_eq!(step(Op::WordStartNext, Pos::new(0, 6)), Pos::new(0, 12));
        assert_eq!(step(Op::WordStartPrev, Pos::new(0, 8)), Pos::new(0, 6));
        assert_eq!(step(Op::WordStartPrev, Pos::new(0, 6)), Pos::new(0, 0));
        assert_eq!(step(Op::WordEndNext, Pos::new(0, 2)), Pos::new(0, 5));
        assert_eq!(step(Op::WordEndNext, Pos::new(0, 5)), Pos::new(0, 11));
        assert_eq!(step(Op::WordEndPrev, Pos::new(0, 8)), Pos::new(0, 5));
        assert_eq!(step(Op::WordEndPrev, Pos::new(0, 12)), Pos::new(0, 11));
    }

    #[test]
    fn word_moves_cross_lines_like_char_moves() {
        let img = image(&["ab cd", "ef"], 6, 0);
        let step = |op: Op, from: Pos| {
            apply(op, &img, Some(Selection::new(from))).unwrap().head()
        };
        assert_eq!(step(Op::WordStartNext, Pos::new(0, 3)), Pos::new(1, 0), "本行没有下一个词就落到下一行首个词");
        assert_eq!(step(Op::WordStartNext, Pos::new(0, 4)), Pos::new(1, 0));
        assert_eq!(step(Op::WordEndPrev, Pos::new(1, 0)), Pos::new(0, 5));
        assert_eq!(step(Op::WordStartPrev, Pos::new(1, 0)), Pos::new(0, 3));
    }

    #[test]
    fn select_word_and_line_on_whitespace_keep_the_old_selection() {
        // 命中空白时「什么都不变」比「把选区清掉」好：连按不会突然失去高亮。
        let img = image(&["ab   ", "cd"], 6, 0);
        let before = Selection::span(Pos::new(0, 0), Pos::new(0, 2));
        assert_eq!(apply(Op::Word, &img, Some(before)), Some(before));
        // 整行空白时选行同理（line_span_at 返回 None）。
        let blank = image(&["   ", "cd"], 4, 0);
        assert_eq!(apply(Op::Line, &blank, Some(before)), Some(before));
    }

    #[test]
    fn select_word_and_line_reuse_the_double_click_helpers() {
        let img = image(&["hello world"], 11, 0);
        let sel = apply(Op::Word, &img, Some(Selection::new(Pos::new(0, 7)))).unwrap();
        assert_eq!(sel.text(&img, &[]), "world");
        let sel = apply(Op::Line, &img, Some(Selection::new(Pos::new(0, 7)))).unwrap();
        assert_eq!(sel.text(&img, &[]), "hello world");
    }

    // ------------------------------------------------- 多选区（集合）

    /// 两行文本的集合夹具：一份跨两行的选区已经放进集合里。
    fn two_row_set() -> Selections {
        let sel = Selection::span(Pos::new(0, 1), Pos::new(1, 4));
        Selections::from_active(Some(sel))
    }

    #[test]
    fn a_new_selection_replaces_the_whole_set() {
        // 拖一次 = 「我要这一段」，不是往旧集合里再加一段。多份选区只由
        // split / invert 产生，用户不可能因为点了一下就多出几段。
        let mut set = Selections::new();
        assert!(set.is_empty() && set.active().is_none() && set.active_index().is_none());
        set.set_active(Some(Selection::new(Pos::new(0, 1))));
        assert_eq!(set.len(), 1);
        set.set_active(Some(Selection::new(Pos::new(2, 3))));
        assert_eq!(set.len(), 1, "第二次拖不该留下第一段");
        assert_eq!(set.active().unwrap().head(), Pos::new(2, 3));
        set.set_active(None);
        assert!(set.is_empty(), "单击（None）应清空集合");
    }

    #[test]
    fn activate_next_and_previous_switch_the_active_one_and_wrap() {
        let img = image(&["abcd", "efgh"], 4, 0);
        let mut set = two_row_set();
        set.split(&img);
        assert_eq!(set.len(), 2, "跨两行的选区拆成两份");
        assert_eq!(set.active_index(), Some(0), "第一份落在原活动位置");
        set.activate_next();
        assert_eq!(set.active_index(), Some(1));
        set.activate_next();
        assert_eq!(set.active_index(), Some(0), "到头要回绕");
        set.activate_previous();
        assert_eq!(set.active_index(), Some(1), "反着也要回绕");
        // 只有一份时是空操作——但活动下标仍然存在（不是 None）。
        let mut one = Selections::from_active(Some(Selection::new(Pos::new(0, 0))));
        one.activate_next();
        assert_eq!(one.active_index(), Some(0));
        // 空集合上切活动是空操作，不能 panic、也不能凭空造出下标。
        let mut empty = Selections::new();
        empty.activate_next();
        empty.activate_previous();
        assert_eq!(empty.active_index(), None);
    }

    #[test]
    fn lock_blocks_drag_and_keyboard_extension_but_not_the_set_commands() {
        let img = image(&["abcdef", "ghijkl"], 6, 0);
        let mut set = two_row_set();
        let before = set.active().unwrap();
        set.toggle_lock();
        assert!(set.is_locked());

        // 拖动 / shift-click：不改。
        set.drag_active_to(Pos::new(1, 5));
        assert_eq!(set.active().unwrap(), before, "锁住后拖动不得改选区");
        // 键盘 / Goto 的光标移动：不改（走的是同一个 Cmd::Move）。
        apply_cmd(Cmd::Move(Op::CharNext), &img, &mut set);
        assert_eq!(set.active().unwrap(), before, "锁住后扩选不得改选区");
        // 新拖一次也不能把它顶掉。
        set.set_active(Some(Selection::new(Pos::new(0, 3))));
        assert_eq!(set.active().unwrap(), before, "锁住后新拖不得顶掉它");

        // 集合命令照旧生效：锁是防手滑，不是把命令也关掉。
        set.exchange_ends();
        assert_eq!(set.active().unwrap().anchor(), Pos::new(1, 4));
        set.toggle_lock();
        assert!(!set.is_locked(), "再点一次要解锁（没有单独的 Unlock 命令）");
        set.drag_active_to(Pos::new(1, 6));
        assert_eq!(set.active().unwrap().head(), Pos::new(1, 6), "解锁后恢复正常");
    }

    #[test]
    fn locking_an_empty_set_does_nothing() {
        // 空集合上点「Lock Selection」不能把锁位点亮：否则下一份选区
        // 一出生就是锁死的，用户会以为拖动坏了。
        let mut set = Selections::new();
        set.toggle_lock();
        assert!(!set.is_locked());
        set.set_active(Some(Selection::new(Pos::new(0, 0))));
        set.drag_active_to(Pos::new(0, 3));
        assert_eq!(set.active().unwrap().head(), Pos::new(0, 3), "新选区必须能拖");
    }

    #[test]
    fn drop_removes_only_the_active_selection() {
        let img = image(&["abcd", "efgh", "ijkl"], 4, 0);
        let mut set = Selections::from_active(Some(Selection::span(
            Pos::new(0, 0),
            Pos::new(2, 4),
        )));
        set.split(&img);
        assert_eq!(set.len(), 3);
        set.activate_next();
        set.drop_active();
        assert_eq!(set.len(), 2, "只掉活动那一份");
        assert_eq!(set.active_index(), Some(1), "活动位落到后一份上");
        // 删掉末尾那份时活动位要退一格，不能指到集合之外。
        set.drop_active();
        assert_eq!(set.len(), 1);
        assert_eq!(set.active_index(), Some(0), "活动位退到还剩下的那一份上");
        assert_eq!(
            set.active().unwrap().anchor(),
            Pos::new(0, 0),
            "留下的是第一行那一份"
        );
        // 删掉最后一份：集合变空、锁位复位，且不再有活动下标。
        set.toggle_lock();
        set.drop_active();
        assert!(set.is_empty());
        assert_eq!(set.active_index(), None);
        assert!(!set.is_locked());
        set.drop_active();
        assert!(set.is_empty(), "空集合上再删也不能 panic");
    }

    #[test]
    fn cancel_clears_everything_and_releases_the_lock() {
        let img = image(&["abcd", "efgh"], 4, 0);
        let mut set = two_row_set();
        set.split(&img);
        set.toggle_lock();
        apply_cmd(Cmd::Set(SetOp::Cancel), &img, &mut set);
        assert!(set.is_empty(), "Cancel 清的是整个集合，不是活动那一份");
        assert!(!set.is_locked(), "没有选区可锁了，锁位必须复位");
        assert_eq!(set.active_index(), None);
    }

    #[test]
    fn split_gives_one_selection_per_row_and_keeps_the_first_active() {
        let img = image(&["abcdef", "ghijkl", "mnopqr"], 6, 0);
        let mut set = Selections::from_active(Some(Selection::span(
            Pos::new(0, 1),
            Pos::new(2, 4),
        )));
        set.split(&img);
        assert_eq!(set.len(), 3, "三行 → 三份");
        assert_eq!(set.active_index(), Some(0));
        // 首行：锚点到头部的原样区间；中间行整行；末行到头部列。
        assert_eq!(set.items()[0], Selection::span(Pos::new(0, 1), Pos::new(0, 6)));
        assert_eq!(set.items()[1], Selection::span(Pos::new(1, 0), Pos::new(1, 6)));
        assert_eq!(set.items()[2], Selection::span(Pos::new(2, 0), Pos::new(2, 4)));
        // 每份都是单行，且复制出来就是这三段。
        for sel in set.items() {
            assert_eq!(sel.anchor().row, sel.head().row);
        }
        assert_eq!(set.text(&img, &[]), "bcdef\nghijkl\nmnop");
    }

    #[test]
    fn split_of_a_block_keeps_the_same_columns_on_every_row() {
        let img = image(&["abcdef", "ghijkl", "mnopqr"], 6, 0);
        let mut set = Selections::from_active(Some(
            Selection::span(Pos::new(0, 1), Pos::new(2, 4)).block(),
        ));
        set.split(&img);
        assert_eq!(set.len(), 3);
        for sel in set.items() {
            assert_eq!(sel.columns_on(sel.anchor().row), Some((1, 4)));
            assert_eq!(sel.shape(), Shape::Linear, "单行上矩形与线性是同一个区间");
        }
        assert_eq!(set.text(&img, &[]), "bcd\nhij\nnop");
    }

    #[test]
    fn invert_of_an_empty_set_is_the_whole_window() {
        let img = image(&["abcde", "fghij", "klmno"], 5, 0);
        let mut set = Selections::new();
        set.invert(&img);
        assert_eq!(set.len(), 1, "连续几行同一段列区间要合成一块矩形");
        let sel = set.active().unwrap();
        assert_eq!(sel.shape(), Shape::Block);
        assert_eq!(sel.anchor(), Pos::new(0, 0));
        assert_eq!(sel.head(), Pos::new(2, 5));
        assert_eq!(set.text(&img, &[]), "abcde\nfghij\nklmno");
    }

    #[test]
    fn invert_subtracts_the_existing_selection() {
        let img = image(&["abcde", "abcde"], 5, 0);
        // 已经选上第 2 列（两行都是同一个矩形块）。
        let mut set = Selections::from_active(Some(
            Selection::span(Pos::new(0, 2), Pos::new(1, 3)).block(),
        ));
        set.invert(&img);
        // 左块 [0,2) 与右块 [3,5) 各跨两行，旧的选区被挖掉。
        assert_eq!(set.len(), 2, "左块 + 右块：{:?}", set.items());
        assert_eq!(set.text(&img, &[]), "ab\nab\nde\nde");
        for sel in set.items() {
            assert!(!sel.contains(0, 2), "被挖掉的列不能再被选中");
            assert!(!sel.contains(1, 2));
        }
    }

    #[test]
    fn invert_of_a_full_window_selection_is_empty() {
        let img = image(&["abc", "def"], 3, 0);
        let mut set = Selections::from_active(Some(
            Selection::span(Pos::new(0, 0), Pos::new(1, 3)).block(),
        ));
        set.invert(&img);
        assert!(set.is_empty(), "整块都已经选上了，反选之后什么都不剩");
        assert_eq!(set.active_index(), None);
    }

    #[test]
    fn switch_stream_and_column_toggles_the_shape_without_moving_the_ends() {
        let img = image(&["abcdef", "ghijkl"], 6, 0);
        let mut set = Selections::from_active(Some(Selection::span(
            Pos::new(0, 1),
            Pos::new(1, 4),
        )));
        assert_eq!(set.active().unwrap().text(&img, &[]), "bcdef\nghij");
        apply_cmd(Cmd::Set(SetOp::SwitchShape), &img, &mut set);
        let sel = set.active().unwrap();
        assert_eq!(sel.shape(), Shape::Block);
        assert_eq!(sel.anchor(), Pos::new(0, 1), "换形状不换端点");
        assert_eq!(sel.head(), Pos::new(1, 4));
        assert_eq!(set.text(&img, &[]), "bcd\nhij", "列选每行只取 [1, 4)");
        apply_cmd(Cmd::Set(SetOp::SwitchShape), &img, &mut set);
        assert_eq!(set.active().unwrap().shape(), Shape::Linear, "再切一次回到流选");
        assert_eq!(set.text(&img, &[]), "bcdef\nghij");
    }

    #[test]
    fn exchange_swaps_anchor_and_head() {
        let img = image(&["abcdef"], 6, 0);
        let mut set = Selections::from_active(Some(Selection::span(
            Pos::new(0, 1),
            Pos::new(0, 4),
        )));
        apply_cmd(Cmd::Set(SetOp::Exchange), &img, &mut set);
        let sel = set.active().unwrap();
        assert_eq!(sel.anchor(), Pos::new(0, 4));
        assert_eq!(sel.head(), Pos::new(0, 1));
        // 取到的文本不变（`normalized` 按阅读顺序），变的只是「哪一头是锚点」。
        assert_eq!(set.text(&img, &[]), "bcd");
        // 空集合上换端点不能 panic，也不能凭空造出选区。
        let mut empty = Selections::new();
        empty.exchange_ends();
        assert!(empty.is_empty());
    }

    #[test]
    fn cursor_moves_shift_the_head_of_the_active_selection_only() {
        let img = image(&["abcdef", "ghijkl"], 6, 0);
        let mut set = Selections::from_active(Some(Selection::span(
            Pos::new(0, 1),
            Pos::new(0, 3),
        )));
        // Goto › Char Right / Line Down：anchor 不动，head 走。
        apply_cmd(Cmd::Move(Op::CharNext), &img, &mut set);
        assert_eq!(set.active().unwrap().head(), Pos::new(0, 4));
        assert_eq!(set.active().unwrap().anchor(), Pos::new(0, 1));
        apply_cmd(Cmd::Move(Op::LineNext), &img, &mut set);
        assert_eq!(set.active().unwrap().head(), Pos::new(1, 4));
        apply_cmd(Cmd::Move(Op::CharPrev), &img, &mut set);
        assert_eq!(set.active().unwrap().head(), Pos::new(1, 3));
        assert_eq!(set.active().unwrap().anchor(), Pos::new(0, 1), "锚点一次都没动");
        // 没有选区时与 Shift+方向键一样，从终端光标起一段。
        let img = image_with_cursor(&["abcdef"], 6, (0, 2));
        let mut empty = Selections::new();
        apply_cmd(Cmd::Move(Op::CharNext), &img, &mut empty);
        assert_eq!(empty.len(), 1);
        assert_eq!(empty.active().unwrap().anchor(), Pos::new(0, 2));
        assert_eq!(empty.active().unwrap().head(), Pos::new(0, 3));
    }

    #[test]
    fn a_move_that_yields_nothing_leaves_no_selection() {
        // 空屏上的「全选」算不出区间：集合必须真的变空，不能留一个假选区。
        let img = image(&["   "], 3, 0);
        let mut set = Selections::from_active(Some(Selection::new(Pos::new(0, 0))));
        apply_cmd(Cmd::Move(Op::All), &img, &mut set);
        assert!(set.is_empty());
    }

    #[test]
    fn the_whole_set_copies_all_of_its_pieces() {
        let img = image(&["abc", "def", "ghi"], 3, 0);
        let mut set = Selections::from_active(Some(Selection::span(
            Pos::new(0, 0),
            Pos::new(2, 2),
        )));
        assert_eq!(set.text(&img, &[]), "abc\ndef\ngh", "只有一份时与 Selection::text 一致");
        set.split(&img);
        assert_eq!(set.len(), 3);
        assert_eq!(set.active().unwrap().text(&img, &[]), "abc", "活动那份只有一行");
        assert_eq!(
            set.text(&img, &[]),
            "abc\ndef\ngh",
            "复制读的是整批高亮，不是只读活动那一份"
        );
    }

    // ------------------------- 2026-11-13：26 条光标 / 选区命令（逐条钉语义）
    //
    // 判定「接线」有三条，缺一条都算假接线：`parse` 能解析、`apply` 有真实落点、
    // 落点被单测钉住。下面每条测试都断言**活动选区的 head**（与既有的
    // `goto_commands_land_where_the_menu_says` 同一口径），锚点另有总闸
    // `every_cursor_command_leaves_the_anchor_alone`。

    use term_core::osc133::CommandBlock;

    /// 一条 OSC 133 命令块（只有这两端参与寻位）。
    fn cmd_block(prompt_start: i64, prompt_end: Option<i64>) -> CommandBlock<i64> {
        CommandBlock {
            prompt_start,
            prompt_end,
            command_start: prompt_end.unwrap_or(prompt_start),
            end: None,
            exit: None,
        }
    }

    /// 移动一步，只要 head（测试里最常写的形状）。
    fn step(op: Op, img: &ScreenImage, from: Pos) -> Pos {
        apply(op, img, Some(Selection::new(from)))
            .expect("位移一定会产出选区")
            .head()
    }

    #[test]
    fn every_cursor_command_parses_to_its_own_op() {
        // 一张表钉住 26 条 ID → `Op`。写表而不是 26 个函数：这一层只做
        // 「字符串 → 意图」的翻译，但**每一行都必须是它自己的变体**——
        // 接到邻居上（Char/Line/Page 之间、Prev/Next 之间）正是最容易看起来
        // 通了的地方，同一条 ID 出现两次也会在这里被抓出来。
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
        for (id, want) in ids {
            assert_eq!(parse(id), Some(want), "{id}");
        }
        // 26 条各占一个变体：表里不许有两条 ID 指向同一个 `Op`。
        for (i, (id, op)) in ids.iter().enumerate() {
            for (other_id, other_op) in &ids[i + 1..] {
                assert_ne!(op, other_op, "{id} 与 {other_id} 指向同一个 Op，是假接线");
            }
        }
    }

    // ── Select Column To：10 条 ──

    #[test]
    fn select_column_to_commands_always_land_in_a_block_shape() {
        let img = image(&["abcdef", "ghijkl", "mnopqr"], 6, 0);
        let ops = [
            Op::ColumnCharPrev,
            Op::ColumnCharNext,
            Op::ColumnLinePrev,
            Op::ColumnLineNext,
            Op::ColumnPagePrev,
            Op::ColumnPageNext,
            Op::ColumnParagraphPrev,
            Op::ColumnParagraphNext,
            Op::ColumnLineHome,
            Op::ColumnLineEnd,
        ];
        let start = Some(Selection::span(Pos::new(0, 2), Pos::new(0, 3)));
        for op in ops {
            let sel = apply(op, &img, start).unwrap_or_else(|| panic!("{op:?} 必须产出选区"));
            assert_eq!(sel.shape(), Shape::Block, "{op:?} 必须落成矩形");
            assert_eq!(sel.anchor(), Pos::new(0, 2), "{op:?} 锚点不得跟着跑");
        }
        // 首次（前面没有任何选区）也是矩形：命令名字就叫「按列选」。
        let sel = apply(Op::ColumnLineNext, &img, None).unwrap();
        assert_eq!(sel.shape(), Shape::Block, "首按就是矩形");
        assert_eq!(sel.anchor(), Pos::new(0, 0), "没有选区时从窗口首行起步");
        assert_eq!(sel.head(), Pos::new(1, 0));
    }

    #[test]
    fn select_column_to_char_moves_stop_at_the_line_edges() {
        let img = image(&["abc", "defg"], 6, 0);
        // 行尾之后不再往右回绕——那是线性语义干的事。
        assert_eq!(
            step(Op::ColumnCharNext, &img, Pos::new(0, 2)),
            Pos::new(0, 3)
        );
        assert_eq!(
            step(Op::ColumnCharNext, &img, Pos::new(0, 3)),
            Pos::new(0, 3),
            "行尾（最后一个非空白之后）停住"
        );
        assert_eq!(
            step(Op::ColumnCharPrev, &img, Pos::new(1, 0)),
            Pos::new(1, 0),
            "行首列 0 停住"
        );
        assert_eq!(
            step(Op::ColumnCharPrev, &img, Pos::new(1, 2)),
            Pos::new(1, 1)
        );
        // 对照：线性 `CharNext` 照旧跨行——两条命令不是同一件事。
        assert_eq!(step(Op::CharNext, &img, Pos::new(0, 3)), Pos::new(1, 0));
    }

    #[test]
    fn select_column_to_line_page_and_paragraph_keep_the_column() {
        let img = image(&["abcdef", "ghijkl", "", "mnopqr"], 6, 0);
        // 行 / 页：列原样带走（一屏 4 行，页步长 4，钳到窗口末行）。
        assert_eq!(step(Op::ColumnLineNext, &img, Pos::new(0, 3)), Pos::new(1, 3));
        assert_eq!(step(Op::ColumnLinePrev, &img, Pos::new(1, 3)), Pos::new(0, 3));
        assert_eq!(step(Op::ColumnPageNext, &img, Pos::new(0, 3)), Pos::new(3, 3));
        assert_eq!(step(Op::ColumnPagePrev, &img, Pos::new(3, 3)), Pos::new(0, 3));
        // 段：向下到下一段首行（跳过中间空行），向上到上一段末行。
        assert_eq!(
            step(Op::ColumnParagraphNext, &img, Pos::new(0, 3)),
            Pos::new(3, 3)
        );
        assert_eq!(
            step(Op::ColumnParagraphPrev, &img, Pos::new(3, 3)),
            Pos::new(1, 3)
        );
        // 列号夹进屏幕宽度：屏幕外那一列回不来。
        assert_eq!(
            step(Op::ColumnLineNext, &img, Pos::new(0, 99)),
            Pos::new(1, 6)
        );
    }

    #[test]
    fn select_column_to_home_and_end_use_the_content_bounds() {
        let img = image(&["  ab  ", "      "], 6, 0);
        assert_eq!(step(Op::ColumnLineHome, &img, Pos::new(0, 3)), Pos::new(0, 0));
        assert_eq!(
            step(Op::ColumnLineEnd, &img, Pos::new(0, 0)),
            Pos::new(0, 4),
            "行尾是最后一个非空白之后"
        );
        assert_eq!(
            step(Op::ColumnLineEnd, &img, Pos::new(1, 0)),
            Pos::new(1, 0),
            "空行的行尾就是行首"
        );
    }

    // ── Word Part：2 条 ──

    #[test]
    fn word_part_commands_land_on_sub_word_boundaries() {
        // `fooBar_baz2` → foo / Bar / baz / 2。
        let img = image(&["fooBar_baz2"], 11, 0);
        assert_eq!(step(Op::WordPartNext, &img, Pos::new(0, 1)), Pos::new(0, 3));
        assert_eq!(step(Op::WordPartNext, &img, Pos::new(0, 3)), Pos::new(0, 7));
        assert_eq!(step(Op::WordPartNext, &img, Pos::new(0, 7)), Pos::new(0, 10));
        assert_eq!(step(Op::WordPartPrev, &img, Pos::new(0, 4)), Pos::new(0, 3));
        assert_eq!(step(Op::WordPartPrev, &img, Pos::new(0, 3)), Pos::new(0, 0));
        assert_eq!(step(Op::WordPartPrev, &img, Pos::new(0, 10)), Pos::new(0, 7));
        // 与「按词移动」形成对照：整条 `fooBar_baz2` 是一个 `Class::Word`。
        assert_eq!(step(Op::WordStartNext, &img, Pos::new(0, 1)), Pos::new(0, 11));
    }

    // ── Stuttered Page：2 条 ──

    #[test]
    fn stuttered_page_commands_move_half_a_screen() {
        let img = image(&["a", "b", "c", "d", "e", "f"], 3, 0);
        // 6 行窗口 → 半页 3 行；列不变。
        assert_eq!(
            step(Op::StutteredPageNext, &img, Pos::new(0, 1)),
            Pos::new(3, 1)
        );
        assert_eq!(
            step(Op::StutteredPagePrev, &img, Pos::new(5, 1)),
            Pos::new(2, 1)
        );
        // 与整页对照：整页是 6 行，一步到底。
        assert_eq!(step(Op::PageNext, &img, Pos::new(0, 1)), Pos::new(5, 1));
        // 钳在窗口内：从首行往回不动。
        assert_eq!(
            step(Op::StutteredPagePrev, &img, Pos::new(0, 1)),
            Pos::new(0, 1)
        );
    }

    // ── Paragraph：2 条 ──

    #[test]
    fn paragraph_commands_land_on_the_paragraph_edges() {
        // 两段：0-1 与 3-4，第 2 行是空行分隔。
        let img = image(&["a", "b", "", "c", "d", ""], 3, 0);
        assert_eq!(
            step(Op::ParagraphNext, &img, Pos::new(0, 0)),
            Pos::new(3, 0),
            "段内 → 下一段首行（跳过空行）"
        );
        assert_eq!(step(Op::ParagraphNext, &img, Pos::new(1, 0)), Pos::new(3, 0));
        assert_eq!(
            step(Op::ParagraphPrev, &img, Pos::new(4, 0)),
            Pos::new(3, 0),
            "段内 → 本段段首"
        );
        assert_eq!(
            step(Op::ParagraphPrev, &img, Pos::new(3, 0)),
            Pos::new(1, 0),
            "已在段首 → 上一段末行"
        );
        // 没有下一段 / 上一段就不动（不是跳到窗口另一头）。
        assert_eq!(step(Op::ParagraphNext, &img, Pos::new(5, 0)), Pos::new(5, 0));
        assert_eq!(step(Op::ParagraphPrev, &img, Pos::new(0, 0)), Pos::new(0, 0));
    }

    // ── Command / Scope：4 条（要 OSC 133 块表）──

    #[test]
    fn command_commands_land_on_the_prompt_end_of_each_block() {
        let img = image(&["$ ls", "out", "$ pwd", "files"], 8, 0);
        // 两条命令：prompt_end 落在第 0 / 第 2 行。
        let blocks = [cmd_block(0, Some(0)), cmd_block(2, Some(2))];
        let at = |op: Op, from: Pos| {
            apply_with_blocks(op, &img, Some(Selection::new(from)), &blocks)
                .unwrap()
                .head()
        };
        assert_eq!(at(Op::CommandNext, Pos::new(1, 0)), Pos::new(2, 0));
        assert_eq!(at(Op::CommandPrev, Pos::new(3, 0)), Pos::new(2, 0));
        assert_eq!(at(Op::CommandPrev, Pos::new(2, 0)), Pos::new(0, 0));
        // 到头了就不动。
        assert_eq!(at(Op::CommandNext, Pos::new(3, 0)), Pos::new(3, 0));
        assert_eq!(at(Op::CommandPrev, Pos::new(0, 0)), Pos::new(0, 0));
    }

    #[test]
    fn command_commands_do_nothing_without_a_block_table() {
        // 没有块表时**不动**：跳到第 0 行会把「数据不在」伪装成「命令生效了」。
        let img = image(&["$ ls", "out", "$ pwd", "files"], 8, 0);
        assert_eq!(step(Op::CommandPrev, &img, Pos::new(3, 0)), Pos::new(3, 0));
        assert_eq!(step(Op::CommandNext, &img, Pos::new(1, 0)), Pos::new(1, 0));
        // 只有一条命令时也不许原地打转成「跳了两行」。
        let one = [cmd_block(2, Some(2))];
        let sel = apply_with_blocks(Op::CommandPrev, &img, Some(Selection::new(Pos::new(3, 0))), &one)
            .unwrap();
        assert_eq!(sel.head(), Pos::new(2, 0));
    }

    #[test]
    fn scope_commands_land_on_the_block_start_and_fall_back_to_the_window() {
        let img = image(&["$ ls", "out", "$ pwd", "files"], 8, 0);
        let blocks = [cmd_block(0, Some(0)), cmd_block(2, Some(2))];
        let at = |op: Op, from: Pos| {
            apply_with_blocks(op, &img, Some(Selection::new(from)), &blocks)
                .unwrap()
                .head()
        };
        // 作用域 = 一整条命令区段，落到 `prompt_start`。
        assert_eq!(at(Op::ScopeNext, Pos::new(1, 0)), Pos::new(2, 0));
        assert_eq!(at(Op::ScopePrev, Pos::new(2, 0)), Pos::new(0, 0));
        // 没有块表时退化为可见窗口首 / 末行（作用域覆盖全屏）。
        assert_eq!(step(Op::ScopePrev, &img, Pos::new(2, 0)), Pos::new(0, 0));
        assert_eq!(step(Op::ScopeNext, &img, Pos::new(1, 0)), Pos::new(3, 0));
    }

    // ── Indentation：2 条 ──

    #[test]
    fn indentation_commands_land_on_the_next_different_indent() {
        let img = image(&["  a", "  b", "    c", "      d", ""], 12, 0);
        assert_eq!(
            step(Op::IndentationNext, &img, Pos::new(0, 0)),
            Pos::new(2, 4),
            "跳过缩进相同的一行，落到第一个非空白列"
        );
        assert_eq!(step(Op::IndentationNext, &img, Pos::new(2, 4)), Pos::new(3, 6));
        assert_eq!(step(Op::IndentationPrev, &img, Pos::new(2, 4)), Pos::new(1, 2));
        // 一路都没有不同的缩进 → 不动。
        let flat = image(&["    a", "    b", "    c"], 8, 0);
        assert_eq!(step(Op::IndentationNext, &flat, Pos::new(0, 4)), Pos::new(0, 4));
        assert_eq!(step(Op::IndentationPrev, &flat, Pos::new(2, 4)), Pos::new(2, 4));
    }

    // ── Pair：4 条 ──

    #[test]
    fn pair_commands_jump_between_brackets() {
        let img = image(&["foo(bar)baz"], 12, 0);
        assert_eq!(step(Op::PairStart, &img, Pos::new(0, 5)), Pos::new(0, 3));
        assert_eq!(step(Op::PairEnd, &img, Pos::new(0, 5)), Pos::new(0, 7));
        // Previous / Next 先移到上 / 下一个括号字形，再跳到它的配对端。
        assert_eq!(step(Op::PairPrev, &img, Pos::new(0, 6)), Pos::new(0, 7));
        assert_eq!(step(Op::PairNext, &img, Pos::new(0, 6)), Pos::new(0, 3));
        // 跨行配对：可见窗口是一个字符流。
        let multi = image(&["fn f(", "  a)"], 8, 0);
        assert_eq!(step(Op::PairEnd, &multi, Pos::new(0, 2)), Pos::new(1, 3));
        assert_eq!(step(Op::PairStart, &multi, Pos::new(1, 3)), Pos::new(0, 4));
        // 不平衡就不动。
        let broken = image(&["a(b"], 4, 0);
        assert_eq!(step(Op::PairEnd, &broken, Pos::new(0, 1)), Pos::new(0, 1));
        assert_eq!(step(Op::PairStart, &broken, Pos::new(0, 1)), Pos::new(0, 1));
    }

    #[test]
    fn every_cursor_command_leaves_the_anchor_alone() {
        // 26 条的共同契约：只挪 head。这条总闸拦住「顺手把选区重造一遍」的实现。
        let img = image(&["foo(bar)", "  baz2", "", "  qux", "end"], 12, 0);
        let blocks = [cmd_block(0, Some(0)), cmd_block(3, Some(3))];
        let start = Selection::span(Pos::new(0, 2), Pos::new(0, 4));
        for op in [
            Op::ColumnCharPrev,
            Op::ColumnCharNext,
            Op::ColumnLinePrev,
            Op::ColumnLineNext,
            Op::ColumnPagePrev,
            Op::ColumnPageNext,
            Op::ColumnParagraphPrev,
            Op::ColumnParagraphNext,
            Op::ColumnLineHome,
            Op::ColumnLineEnd,
            Op::WordPartPrev,
            Op::WordPartNext,
            Op::StutteredPagePrev,
            Op::StutteredPageNext,
            Op::ParagraphPrev,
            Op::ParagraphNext,
            Op::CommandPrev,
            Op::CommandNext,
            Op::ScopePrev,
            Op::ScopeNext,
            Op::IndentationPrev,
            Op::IndentationNext,
            Op::PairStart,
            Op::PairEnd,
            Op::PairPrev,
            Op::PairNext,
        ] {
            let sel = apply_with_blocks(op, &img, Some(start), &blocks)
                .unwrap_or_else(|| panic!("{op:?} 必须产出选区"));
            assert_eq!(sel.anchor(), start.anchor(), "{op:?} 锚点不得跟着跑");
        }
    }

    #[test]
    fn cursor_commands_go_through_apply_cmd_with_a_block_table() {
        // 集合层的入口也要能拿到块表，否则菜单点「下一命令」永远不动。
        let img = image(&["$ ls", "out", "$ pwd", "files"], 8, 0);
        let blocks = [cmd_block(0, Some(0)), cmd_block(2, Some(2))];
        let mut set = Selections::from_active(Some(Selection::new(Pos::new(1, 0))));
        apply_cmd_with_blocks(Cmd::Move(Op::CommandNext), &img, &blocks, &mut set);
        assert_eq!(set.active().unwrap().head(), Pos::new(2, 0));
        assert_eq!(set.active().unwrap().anchor(), Pos::new(1, 0));
        // 不带块表的入口照旧（且按规格不动）。
        apply_cmd(Cmd::Move(Op::CommandNext), &img, &mut set);
        assert_eq!(set.active().unwrap().head(), Pos::new(2, 0), "没有块表就不动");
        // 锁定时两条入口都不改选区。
        set.toggle_lock();
        apply_cmd_with_blocks(Cmd::Move(Op::CommandPrev), &img, &blocks, &mut set);
        assert_eq!(set.active().unwrap().head(), Pos::new(2, 0), "锁住后连块表也救不了");
    }
}



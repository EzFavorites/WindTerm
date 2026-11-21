//! 选区几何：内容行号 ↔ 窗口边界 ↔ 列号的全部换算。
//!
//! 从 `selection.rs` 搬出来的一段。这里只回答「这一行有多长」「下一格在哪」
//! 「跨行落到哪一列」，不认识选区的形状、也不认识字符分类的语义——除了
//! 「词的落点」那三个 picker（`first_word_*` / `last_word_col`），它们读的是
//! `words` 里的字形分类，形状由调用方传进来。
//!
//! 窗口边界一律走 `window_start` / `window_end` 两个入口，不直接摸
//! `image.range` 字段：裸字段仍然存在（架构蓝图 §8 冻结金样本数据源），但读它
//! 只发生在 core 的类型化入口内部。

use term_core::coords::{history_to_view, HistoryRow};
use term_core::grid::{Cell, ScreenImage};
use term_core::osc133::CommandBlock;

use super::words::{glyphs, run_bounds, Class};
use super::Pos;

/// 没有选区时的起点：终端光标 → 否则窗口首行行首。
pub(super) fn caret(image: &ScreenImage) -> Option<Pos> {
    image
        .cursor
        .map(|(row, col)| Pos::new(row, col))
        .or_else(|| image.lines.first().map(|l| Pos::new(l.row, 0)))
}

pub(super) fn page(image: &ScreenImage) -> i64 {
    image.lines.len().max(1) as i64
}

/// 窗口首行（内容绝对行）。
///
/// 这份文件里所有「窗口边界」读取都走这里，不再各自写 `image.range.start`：
/// 裸字段仍然存在（架构蓝图 §8 冻结金样本数据源），但读它只发生在 core 的
/// 类型化入口内部，业务代码不再直接摸字段。
pub(super) fn window_start(image: &ScreenImage) -> i64 {
    image.view_range().start().get()
}

/// 窗口尾行（半开，内容绝对行）。
fn window_end(image: &ScreenImage) -> i64 {
    image.view_range().end().get()
}

pub(super) fn last_row(image: &ScreenImage) -> i64 {
    window_end(image).max(window_start(image) + 1) - 1
}

/// 行号钳进窗口；列原样保留（「到下一行」要保持同一列，否则连着按会退到行首）。
pub(super) fn clamp_row(image: &ScreenImage, row: i64, col: usize) -> Pos {
    Pos::new(
        row.clamp(window_start(image), last_row(image)),
        col,
    )
}

/// 最后一个非空白字形之后的列号；整行空白为 0。
pub(super) fn line_end(image: &ScreenImage, row: i64) -> usize {
    match image.line_at(row) {
        Some(line) => content_len(&line.cells),
        None => 0,
    }
}

pub(super) fn content_len(cells: &[Cell]) -> usize {
    let gs = glyphs(cells);
    gs.iter()
        .rposition(|g| !g.ch.is_whitespace())
        .map_or(0, |i| gs[i].to)
}

pub(super) fn document_end(image: &ScreenImage) -> Pos {
    match image
        .lines
        .iter()
        .rev()
        .find(|l| content_len(&l.cells) > 0)
    {
        Some(line) => Pos::new(line.row, content_len(&line.cells)),
        None => Pos::new(window_start(image), 0),
    }
}

/// 下一个字形（宽字符整格跳过）；行尾则落到下一行行首。
pub(super) fn char_next(image: &ScreenImage, at: Pos) -> Pos {
    let Some(line) = image.line_at(at.row) else {
        return at;
    };
    let gs = glyphs(&line.cells);
    if at.col < content_len(&line.cells) {
        if let Some(g) = gs.iter().find(|g| g.to > at.col) {
            return Pos::new(at.row, g.to);
        }
    }
    cross_next(image, at.row)
}

/// 上一个字形；行首则落到上一行行尾。
pub(super) fn char_prev(image: &ScreenImage, at: Pos) -> Pos {
    if at.col == 0 {
        return cross_prev(image, at.row);
    }
    let Some(line) = image.line_at(at.row) else {
        return at;
    };
    let gs = glyphs(&line.cells);
    match gs.iter().rev().find(|g| g.from < at.col) {
        Some(g) => Pos::new(at.row, g.from),
        None => cross_prev(image, at.row),
    }
}

fn cross_next(image: &ScreenImage, row: i64) -> Pos {
    if row >= last_row(image) {
        Pos::new(row, line_end(image, row))
    } else {
        Pos::new(row + 1, 0)
    }
}

/// 该行第一个非空白字形的列号；整行空白为 0。
pub(super) fn first_word_col(image: &ScreenImage, row: i64, separators: &str) -> usize {
    match image.line_at(row) {
        Some(line) => {
            let gs = glyphs(&line.cells);
            gs.iter()
                .find(|g| Class::of(g.ch, separators) != Class::Space)
                .map_or(0, |g| g.from)
        }
        None => 0,
    }
}

/// 该行第一个词的词尾列号；整行空白为 0。
pub(super) fn first_word_end(image: &ScreenImage, row: i64, separators: &str) -> usize {
    match image.line_at(row) {
        Some(line) => {
            let gs = glyphs(&line.cells);
            match gs.iter().position(|g| Class::of(g.ch, separators) != Class::Space) {
                Some(i) => gs[run_bounds(&gs, i, separators).1].to,
                None => 0,
            }
        }
        None => 0,
    }
}

/// 该行最后一个词的词首列号；整行空白为 0。
pub(super) fn last_word_col(image: &ScreenImage, row: i64, separators: &str) -> usize {
    match image.line_at(row) {
        Some(line) => {
            let gs = glyphs(&line.cells);
            match gs.iter().rposition(|g| Class::of(g.ch, separators) != Class::Space) {
                Some(i) => gs[run_bounds(&gs, i, separators).0].from,
                None => 0,
            }
        }
        None => 0,
    }
}

pub(super) fn cross_prev(image: &ScreenImage, row: i64) -> Pos {
    if row <= window_start(image) {
        Pos::new(row, 0)
    } else {
        Pos::new(row - 1, line_end(image, row - 1))
    }
}

// ------------------------------------- 2026-11-13：26 条光标 / 选区命令的边界判定
//
// 下面这批函数是 `Select Column To` / `Word Part`（在 `words.rs`）/
// `Stuttered Page` / `Paragraph` / `Command` / `Scope` / `Indentation` / `Pair`
// 的**唯一**边界来源。`selection/ops.rs::apply` 里只有「取边界 → 造 head」一句，
// 没有一处现算——否则那 26 条又会在 match 里各自长出一套数学，改一处漏一处。
//
// 不变的契约：所有落点都**钳在可见窗口内**（窗口外的行复制不出来，
// 见 `apply` 的文档），所以每个「找一行」的函数都以 `window_start` /
// `last_row` 为边界，找不到就返回 `None` 让调用方「不动」。

/// 列号钳进 `0..=cols`（矩形语义里「列不变」要有个上界，否则列号会跑到屏幕外）。
pub(super) fn clamp_col(image: &ScreenImage, col: usize) -> usize {
    col.min(image.cols)
}

/// 同一行内的下一个字形（**不跨行**）；已经在行尾最后一个非空白之后就不动。
///
/// 与 [`char_next`] 的唯一区别就在这句「不跨行」：矩形选区往右扩一格时，
/// 行尾继续往右应当是停在行尾，而不是绕到下一行的行首——那会把选区从一块
/// 矩形拉成一条斜的阶梯。
pub(super) fn char_next_in_row(image: &ScreenImage, at: Pos) -> Pos {
    let Some(line) = image.line_at(at.row) else {
        return at;
    };
    let end = content_len(&line.cells);
    if at.col < end {
        let gs = glyphs(&line.cells);
        if let Some(g) = gs.iter().find(|g| g.to > at.col) {
            return Pos::new(at.row, g.to.min(end));
        }
    }
    Pos::new(at.row, end)
}

/// 同一行内的上一个字形（**不跨行**）；行首列 0 就停。
pub(super) fn char_prev_in_row(image: &ScreenImage, at: Pos) -> Pos {
    if at.col == 0 {
        return Pos::new(at.row, 0);
    }
    let Some(line) = image.line_at(at.row) else {
        return at;
    };
    let gs = glyphs(&line.cells);
    match gs.iter().rev().find(|g| g.from < at.col) {
        Some(g) => Pos::new(at.row, g.from),
        None => Pos::new(at.row, 0),
    }
}

/// 这一行有没有内容。空行的唯一判据是 `content_len == 0`（整行空白也算空行）。
pub(super) fn row_is_blank(image: &ScreenImage, row: i64) -> bool {
    line_end(image, row) == 0
}

/// 段落 = 连续的**非空**行。返回 `row` 所在段的首行；`row` 本身是空行则 `None`。
///
/// 「只在可见窗口内找」：段首最多退到窗口首行。终端里没有语义上的段落标记，
/// 空行是唯一的分隔符——这与用户眼里的一模一样。
pub(super) fn paragraph_start(image: &ScreenImage, row: i64) -> Option<i64> {
    if row_is_blank(image, row) {
        return None;
    }
    let mut r = row;
    while r > window_start(image) && !row_is_blank(image, r - 1) {
        r -= 1;
    }
    Some(r)
}

/// `row` 所在段的末行；`row` 是空行则 `None`。
pub(super) fn paragraph_end(image: &ScreenImage, row: i64) -> Option<i64> {
    if row_is_blank(image, row) {
        return None;
    }
    let last = last_row(image);
    let mut r = row;
    while r < last && !row_is_blank(image, r + 1) {
        r += 1;
    }
    Some(r)
}

/// `Goto Previous Paragraph` 的落点行。
///
/// 段内（非段首）→ 本段段首；已经在段首、或站在空行上 → 上一段的末行
/// （中间的空行整片跳过）。窗口内找不到上一段就 `None`（不动）。
pub(super) fn paragraph_prev(image: &ScreenImage, row: i64) -> Option<i64> {
    if let Some(start) = paragraph_start(image, row) {
        if start < row {
            return Some(start);
        }
    }
    let mut r = row - 1;
    while r >= window_start(image) && row_is_blank(image, r) {
        r -= 1;
    }
    if r < window_start(image) {
        None
    } else {
        Some(r)
    }
}

/// `Goto Next Paragraph` 的落点行：下一段的**首**行（跳过中间空行）。
///
/// 从段内出发先跳到本段之后——否则「下一段」会落在自己这一段的第二行上，
/// 连按几次都还在原地打转。
pub(super) fn paragraph_next(image: &ScreenImage, row: i64) -> Option<i64> {
    let from = paragraph_end(image, row).unwrap_or(row);
    let last = last_row(image);
    let mut r = from + 1;
    while r <= last && row_is_blank(image, r) {
        r += 1;
    }
    if r > last {
        None
    } else {
        Some(r)
    }
}

/// `\t` 的默认停位（终端把 tab 展开成到下一个 8 的整数倍的空白）。
pub(super) const TABSTOP: usize = 8;

/// 行首空白的**显示宽度**：空格算一格、宽空白算两格、`\t` 跳到下一个停位。
///
/// 为什么不是「数一数前导空格」：粘贴进来的文本里可能真的留着 `\t`
/// （见 `vt.rs::paste_drops_control_characters_but_keeps_tabs`），
/// 那时一个制表符可能是 1 到 8 格的视觉缩进，数格子会把两级缩进算成同一级。
pub(super) fn indentation_width(image: &ScreenImage, row: i64) -> usize {
    // 空行没有缩进可讲：网格里「一道空白行」与「屏幕下方的填充」长得一模一样，
    // 数它们的空格只会数到屏幕宽度。
    if row_is_blank(image, row) {
        return 0;
    }
    let Some(line) = image.line_at(row) else {
        return 0;
    };
    let mut width = 0usize;
    for g in glyphs(&line.cells) {
        if g.ch == '\t' {
            width += TABSTOP - (width % TABSTOP);
        } else if g.ch.is_whitespace() {
            width += g.to - g.from;
        } else {
            break;
        }
    }
    width
}

/// `Goto Previous / Next Indentation` 的落点：沿 `step`（-1 / +1）方向找第一行
/// 「非空且行首空白宽度与当前行不同」的行，落在它的第一个非空白列上。
///
/// 找不到就 `None`（不动）。用「宽度不同」而不是「更深 / 更浅」：两个方向共用
/// 同一条判据，缩进相当的相邻行自然被跳过——那正是「跳到下一个缩进级别」的意思。
pub(super) fn indentation_target(
    image: &ScreenImage,
    row: i64,
    step: i64,
    separators: &str,
) -> Option<Pos> {
    if step == 0 {
        return None;
    }
    let width = indentation_width(image, row);
    let mut r = row + step;
    while r >= window_start(image) && r <= last_row(image) {
        if !row_is_blank(image, r) && indentation_width(image, r) != width {
            return Some(Pos::new(r, first_word_col(image, r, separators)));
        }
        r += step;
    }
    None
}

/// `Stuttered Page`：半页步长，至少一行（一行高的窗口里「半页」不能是 0，
/// 那会让命令变成空操作）。
pub(super) fn stuttered_page(image: &ScreenImage) -> i64 {
    (page(image) / 2).max(1)
}

// ------------------------------------------------------------- 括号对（`Pair`）

/// 开括号 → 配对的闭括号。
fn closing_of(ch: char) -> Option<char> {
    match ch {
        '(' => Some(')'),
        '[' => Some(']'),
        '{' => Some('}'),
        _ => None,
    }
}

/// 闭括号 → 配对的开括号。
fn opening_of(ch: char) -> Option<char> {
    match ch {
        ')' => Some('('),
        ']' => Some('['),
        '}' => Some('{'),
        _ => None,
    }
}

fn is_bracket(ch: char) -> bool {
    closing_of(ch).is_some() || opening_of(ch).is_some()
}

/// 可见窗口里的括号字形，按阅读顺序（**跨行连续**，把整屏当一个字符流）。
///
/// 「跨行连续」是这套配对的关键：一段 `(` 写在上一行、`)` 写在下一行，
/// 在编辑器里仍然是一对；按行各自配对的话它们都是孤零零的半个。
fn brackets(image: &ScreenImage) -> Vec<(Pos, char)> {
    let mut out: Vec<(Pos, char)> = Vec::new();
    for line in &image.lines {
        for g in glyphs(&line.cells) {
            if is_bracket(g.ch) {
                out.push((Pos::new(line.row, g.from), g.ch));
            }
        }
    }
    out
}

/// 括号流里每个括号的配对端下标表；没配上的是 `None`。
///
/// 一趟**栈扫描**：开括号压栈、闭括号出栈，出栈时种类必须对得上。
/// 这样 `( [ ) ]` 这种交叉嵌套两边都配不上（栈顶是 `[`，来的是 `)`），
/// 于是整条命令「不动」；而屏幕上别处多出来的一个孤立 `]` 只是自己配不上，
/// 不会把别人的括号对一起判死。
fn pair_partners(stream: &[(Pos, char)]) -> Vec<Option<usize>> {
    let mut partner: Vec<Option<usize>> = vec![None; stream.len()];
    let mut open: Vec<usize> = Vec::new();
    for (i, (_, ch)) in stream.iter().enumerate() {
        if closing_of(*ch).is_some() {
            open.push(i);
        } else if let Some(top) = open.pop() {
            if closing_of(stream[top].1) == Some(*ch) {
                partner[top] = Some(i);
                partner[i] = Some(top);
            }
        }
    }
    partner
}

/// 括号流里第 `idx` 个括号的配对端下标；不平衡（种类对不上）则 `None`。
fn match_index(stream: &[(Pos, char)], idx: usize) -> Option<usize> {
    pair_partners(stream).get(idx).copied().flatten()
}

/// 距 `at` 最近的括号下标：先看 `at` 所在（或右侧第一个），右侧没有就退到最后一个。
fn nearest_bracket(stream: &[(Pos, char)], at: Pos) -> Option<usize> {
    match stream
        .iter()
        .position(|(p, _)| (p.row, p.col) >= (at.row, at.col))
    {
        Some(i) => Some(i),
        None => stream.len().checked_sub(1),
    }
}

/// `Start Of Pair`：落到所在（或最近）括号那一对的**开**端。
///
/// 光标停在 `foo(bar|)` 的中间时，`(…)` 这一对由最近的括号（`(`）确定，
/// 落点是 `(` 本身——这就是「跳到这一对的开头」。
pub(super) fn pair_start(image: &ScreenImage, at: Pos) -> Option<Pos> {
    let stream = brackets(image);
    let idx = nearest_bracket(&stream, at)?;
    let other = match_index(&stream, idx)?;
    let open = idx.min(other);
    Some(stream[open].0)
}

/// `End Of Pair`：落到同一对的**闭**端。
pub(super) fn pair_end(image: &ScreenImage, at: Pos) -> Option<Pos> {
    let stream = brackets(image);
    let idx = nearest_bracket(&stream, at)?;
    let other = match_index(&stream, idx)?;
    let close = idx.max(other);
    Some(stream[close].0)
}

/// `Previous Pair`：先退到上一个括号字形，再跳到它的配对端。
pub(super) fn pair_prev(image: &ScreenImage, at: Pos) -> Option<Pos> {
    let stream = brackets(image);
    let idx = stream
        .iter()
        .rposition(|(p, _)| (p.row, p.col) < (at.row, at.col))?;
    let other = match_index(&stream, idx)?;
    Some(stream[other].0)
}

/// `Next Pair`：先前进到下一个括号字形，再跳到它的配对端。
pub(super) fn pair_next(image: &ScreenImage, at: Pos) -> Option<Pos> {
    let stream = brackets(image);
    let idx = stream
        .iter()
        .position(|(p, _)| (p.row, p.col) > (at.row, at.col))?;
    let other = match_index(&stream, idx)?;
    Some(stream[other].0)
}

// ------------------------------------------------- 命令块（`Command` / `Scope`）

/// 块里的绝对历史行 → 视口行；落在窗口外（或被裁剪过）返回 `None`。
///
/// 换算只走 `term_core::coords` 这一个出口（`blocks.rs::typed_command_rows`
/// 用的是同一条），这里不再自己写 `row - history`。
fn block_view_row(image: &ScreenImage, absolute: i64) -> Option<i64> {
    let row = history_to_view(HistoryRow::new(absolute)?, image.history as u64).get();
    if row < window_start(image) || row > last_row(image) {
        return None;
    }
    Some(row)
}

/// 命令块里「用户开始打命令」的那一行（`prompt_end`）的视口行，升序去重。
///
/// 没有 `B` 标记的块不算——宁可什么都不做，也不把提示符当成命令位置。
pub(super) fn command_rows(image: &ScreenImage, blocks: &[CommandBlock<i64>]) -> Vec<i64> {
    let mut rows: Vec<i64> = blocks
        .iter()
        .filter_map(|b| b.prompt_end)
        .filter_map(|abs| block_view_row(image, abs))
        .collect();
    rows.sort_unstable();
    rows.dedup();
    rows
}

/// 命令区段的起点（`prompt_start`）的视口行，升序去重。
pub(super) fn scope_rows(image: &ScreenImage, blocks: &[CommandBlock<i64>]) -> Vec<i64> {
    let mut rows: Vec<i64> = blocks
        .iter()
        .filter_map(|b| block_view_row(image, b.prompt_start))
        .collect();
    rows.sort_unstable();
    rows.dedup();
    rows
}

/// 严格小于 `at` 的最近一行；没有则 `None`。
pub(super) fn nearest_above(rows: &[i64], at: i64) -> Option<i64> {
    rows.iter().copied().filter(|r| *r < at).max()
}

/// 严格大于 `at` 的最近一行；没有则 `None`。
pub(super) fn nearest_below(rows: &[i64], at: i64) -> Option<i64> {
    rows.iter().copied().filter(|r| *r > at).min()
}


#[cfg(test)]
mod tests {
    use super::*;
    use crate::selection::DEFAULT_SEPARATORS;
    use term_core::grid::{Attrs, Colour, LineSlice};

    /// 一行 `text` 补齐到 `cols` 格（尾部是空白填充）。
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

    /// 贴底窗口（视口行 `0..lines.len()`），没有历史、没有光标。
    fn image(lines: &[&str], cols: usize) -> ScreenImage {
        image_with_history(lines, cols, 0)
    }

    fn image_with_history(lines: &[&str], cols: usize, history: usize) -> ScreenImage {
        let rows: Vec<LineSlice> = lines
            .iter()
            .enumerate()
            .map(|(i, text)| LineSlice {
                row: i as i64,
                cells: cells(text, cols),
            })
            .collect();
        ScreenImage {
            cols,
            version: 1,
            range: 0..lines.len() as i64,
            lines: rows,
            cursor: None,
            history,
        }
    }

    fn block(prompt_start: i64, prompt_end: Option<i64>) -> CommandBlock<i64> {
        CommandBlock {
            prompt_start,
            prompt_end,
            command_start: prompt_end.unwrap_or(prompt_start),
            end: None,
            exit: None,
        }
    }

    #[test]
    fn column_helpers_never_leave_the_row() {
        let img = image(&["abcdef   ", "gh"], 9);
        // 右移：行尾是最后一个非空白之后，继续按不动。
        assert_eq!(
            char_next_in_row(&img, Pos::new(0, 4)),
            Pos::new(0, 5),
            "正常右移一格"
        );
        assert_eq!(
            char_next_in_row(&img, Pos::new(0, 6)),
            Pos::new(0, 6),
            "已在行尾（最后一个非空白之后）就不再动，不绕到下一行"
        );
        // 左移：行首停住。
        assert_eq!(
            char_prev_in_row(&img, Pos::new(0, 3)),
            Pos::new(0, 2)
        );
        assert_eq!(
            char_prev_in_row(&img, Pos::new(1, 0)),
            Pos::new(1, 0),
            "行首列 0 就停，不绕到上一行行尾"
        );
        assert_eq!(clamp_col(&img, 99), 9, "列号夹进屏幕宽度");
    }

    #[test]
    fn row_is_blank_counts_whitespace_only_lines_as_blank() {
        let img = image(&["abc", "", "   ", "\t"], 5);
        assert!(!row_is_blank(&img, 0));
        assert!(row_is_blank(&img, 1));
        assert!(row_is_blank(&img, 2), "整行空白也是空行");
        assert!(row_is_blank(&img, 3), "只有一个制表符同样是空行");
        assert!(row_is_blank(&img, 9), "窗口外的行按空行处理，不 panic");
    }

    #[test]
    fn a_paragraph_is_a_run_of_non_blank_rows() {
        let img = image(&["a", "b", "", "c", "d", "e", ""], 3);
        assert_eq!(paragraph_start(&img, 1), Some(0));
        assert_eq!(paragraph_end(&img, 1), Some(1));
        assert_eq!(paragraph_start(&img, 4), Some(3));
        assert_eq!(paragraph_end(&img, 4), Some(5));
        assert_eq!(paragraph_start(&img, 2), None, "空行不属于任何段");
        assert_eq!(paragraph_end(&img, 2), None);
    }

    #[test]
    fn paragraph_prev_stops_at_the_current_start_then_jumps_back() {
        let img = image(&["a", "b", "", "c", "d", ""], 3);
        // 段内 → 本段段首。
        assert_eq!(paragraph_prev(&img, 4), Some(3));
        // 已在段首 → 上一段末行（中间空行整片跳过）。
        assert_eq!(paragraph_prev(&img, 3), Some(1));
        // 站在空行上 → 上一段末行。
        assert_eq!(paragraph_prev(&img, 5), Some(4));
        // 窗口首行就是段首 → 没有上一段，不动。
        assert_eq!(paragraph_prev(&img, 0), None);
    }

    #[test]
    fn paragraph_next_lands_on_the_next_start_and_skips_blank_separators() {
        let img = image(&["a", "b", "", "", "c", "d"], 3);
        // 段内 → 下一段首行，而不是自己这一段的第二行。
        assert_eq!(paragraph_next(&img, 0), Some(4));
        assert_eq!(paragraph_next(&img, 1), Some(4));
        // 空行上 → 下一段首行。
        assert_eq!(paragraph_next(&img, 2), Some(4));
        // 最后一段里 → 没有下一段，不动。
        assert_eq!(paragraph_next(&img, 5), None);
    }

    #[test]
    fn indentation_width_expands_tabs_to_the_next_stop() {
        let img = image(&["    a", "  \tb", "c", "\td", "", "  "], 12);
        assert_eq!(indentation_width(&img, 0), 4);
        // 两格之后遇到 `\t`：跳到第 8 格，不是第 3 格。
        assert_eq!(indentation_width(&img, 1), TABSTOP);
        assert_eq!(indentation_width(&img, 2), 0);
        assert_eq!(indentation_width(&img, 3), TABSTOP);
        assert_eq!(indentation_width(&img, 4), 0, "空行没有缩进");
        assert_eq!(
            indentation_width(&img, 5),
            0,
            "整行空白也是空行：屏幕下方的填充不该被当成缩进"
        );
    }

    #[test]
    fn indentation_target_only_stops_on_a_different_leading_width() {
        let img = image(&["  a", "  b", "    c", "      d", ""], 12);
        // 当前缩进 2：向下第一个「缩进不同」的非空行是第 2 行的 4 格。
        assert_eq!(
            indentation_target(&img, 0, 1, DEFAULT_SEPARATORS),
            Some(Pos::new(2, 4))
        );
        assert_eq!(
            indentation_target(&img, 2, 1, DEFAULT_SEPARATORS),
            Some(Pos::new(3, 6)),
            "再深一级"
        );
        // 向上：从 4 格回到 2 格的第一行。
        assert_eq!(
            indentation_target(&img, 2, -1, DEFAULT_SEPARATORS),
            Some(Pos::new(1, 2))
        );
        // 一路到底都没有不同的缩进 → 不动。
        assert_eq!(
            indentation_target(&img, 1, 1, DEFAULT_SEPARATORS),
            Some(Pos::new(2, 4))
        );
        let same = image(&["    a", "    b", "    c"], 8);
        assert_eq!(indentation_target(&same, 0, 1, DEFAULT_SEPARATORS), None);
        assert_eq!(indentation_target(&same, 2, -1, DEFAULT_SEPARATORS), None);
        assert_eq!(
            indentation_target(&same, 1, 0, DEFAULT_SEPARATORS),
            None,
            "step=0 不动"
        );
    }

    #[test]
    fn stuttered_page_is_half_a_screen_and_never_zero() {
        let one = image(&["a"], 3);
        assert_eq!(stuttered_page(&one), 1, "一行高的窗口里半页仍是 1 行");
        let two = image(&["a", "b"], 3);
        assert_eq!(stuttered_page(&two), 1);
        let nine = image(&["a"; 9], 3);
        assert_eq!(stuttered_page(&nine), 4, "9 / 2 = 4（向下取整）");
    }

    #[test]
    fn a_pair_is_matched_across_lines() {
        // 整屏当一个字符流：开括号在上一行、闭括号在下一行，仍然是一对。
        let img = image(&["fn f(", "  a)", ")"], 8);
        let stream = brackets(&img);
        assert_eq!(stream.len(), 3);
        assert_eq!(match_index(&stream, 0), Some(1), "第 0 个括号的配对端");
        assert_eq!(match_index(&stream, 2), None, "多出来的闭括号没有配对");
    }

    #[test]
    fn nesting_is_counted_and_unbalanced_brackets_have_no_pair() {
        // 括号流：`(` `(` `)` `)` `[` `]`。
        let img = image(&["(a(b)c) [d]"], 16);
        let stream = brackets(&img);
        assert_eq!(match_index(&stream, 0), Some(3), "外层括号配最外层那一半");
        assert_eq!(match_index(&stream, 1), Some(2), "内层括号配内层");
        assert_eq!(match_index(&stream, 4), Some(5), "方括号自成一对");
        // 交叉嵌套：`( [ ) ]` 里 `(` 找不到自己的 `)`。
        let crossed = image(&["( [ ) ]"], 8);
        let stream = brackets(&crossed);
        assert_eq!(match_index(&stream, 0), None, "不平衡就不动");
        assert_eq!(match_index(&stream, 1), None);
    }

    #[test]
    fn pair_start_and_end_pick_the_two_ends_of_the_same_pair() {
        let img = image(&["foo(bar)baz"], 12);
        // 光标在括号中间：Start 到 `(`，End 到 `)`。
        assert_eq!(pair_start(&img, Pos::new(0, 5)), Some(Pos::new(0, 3)));
        assert_eq!(pair_end(&img, Pos::new(0, 5)), Some(Pos::new(0, 7)));
        // 光标在闭括号上：同一对的两个端点仍然对得上。
        assert_eq!(pair_start(&img, Pos::new(0, 7)), Some(Pos::new(0, 3)));
        assert_eq!(pair_end(&img, Pos::new(0, 7)), Some(Pos::new(0, 7)));
        // 光标在括号右边（右侧没有括号了）：退到最后一个括号，仍能定位这一对。
        assert_eq!(pair_start(&img, Pos::new(0, 10)), Some(Pos::new(0, 3)));
        // 屏上一个括号都没有 → 不动。
        let plain = image(&["no brackets here"], 16);
        assert_eq!(pair_start(&plain, Pos::new(0, 3)), None);
        assert_eq!(pair_end(&plain, Pos::new(0, 3)), None);
    }

    #[test]
    fn previous_and_next_pair_step_onto_a_bracket_first() {
        // 括号流：`(`(1) `)`(3) `[`(5) `]`(7)。
        let img = image(&["a(b)c[d]e"], 10);
        // 光标在 `[` 上：Next Pair 先移到 `]`（下一个括号字形），再跳到它的配对端 `[`。
        assert_eq!(pair_next(&img, Pos::new(0, 5)), Some(Pos::new(0, 5)));
        // 光标在 `d` 上（`[` 之后）：Next 落到 `]` 的配对端 `[`。
        assert_eq!(pair_next(&img, Pos::new(0, 6)), Some(Pos::new(0, 5)));
        // 光标在 `d` 上：Previous 先退到 `[`，再跳到配对端 `]`。
        assert_eq!(pair_prev(&img, Pos::new(0, 6)), Some(Pos::new(0, 7)));
        // 光标在最左边：没有上一个括号，不动。
        assert_eq!(pair_prev(&img, Pos::new(0, 0)), None);
        // 光标在最右边：没有下一个括号，不动。
        assert_eq!(pair_next(&img, Pos::new(0, 9)), None);
    }

    #[test]
    fn command_rows_convert_absolute_rows_and_drop_the_ones_off_screen() {
        // 窗口 3 行、可回看 3 行：视口行 0 == 绝对行 3。
        let img = image_with_history(&["$ ls", "out", "$ pwd"], 8, 3);
        let blocks = [
            block(0, Some(0)),  // 绝对行 0 → 视口行 -3，屏外
            block(3, Some(3)),  // → 视口行 0
            block(5, None),     // 没有 B 标记，不算
        ];
        assert_eq!(command_rows(&img, &blocks), vec![0], "只留窗口内且带 B 的块");
        assert_eq!(scope_rows(&img, &blocks), vec![0, 2], "prompt_start 全部计入");
        // 没有块表：两列都空，调用方据此「不动 / 退化到窗口首末行」。
        assert!(command_rows(&img, &[]).is_empty());
        assert!(scope_rows(&img, &[]).is_empty());
    }

    #[test]
    fn nearest_above_and_below_are_strict_and_ordered() {
        let rows = [0, 4, 9];
        assert_eq!(nearest_above(&rows, 4), Some(0), "同行的不算「上一个」");
        assert_eq!(nearest_above(&rows, 0), None);
        assert_eq!(nearest_below(&rows, 4), Some(9), "同行的不算「下一个」");
        assert_eq!(nearest_below(&rows, 9), None);
        assert_eq!(nearest_above(&rows, 20), Some(9));
        assert_eq!(nearest_below(&rows, -1), Some(0));
    }
}

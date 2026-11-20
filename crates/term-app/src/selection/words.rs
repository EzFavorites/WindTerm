//! 选词：把网格的「格」折叠回字形，按字符分类切出词边界。
//!
//! 从 `selection.rs` 搬出来的一段。两级入口：`word_at`（双击选词）与
//! `line_span_at`（三击选行）；`word_start_*` / `word_end_*` 是 Shift+方向键走
//! 的「按词移动」，与 `Op::WordStartNext` 一族一一对应。
//!
//! 网格是**按格**存的，宽字符占两格（第二格是延续格）。这一层把格折叠回字形，
//! 选词只跟字形打交道——否则「你好」会被切成「汉字 + 空格」两个东西。

use term_core::grid::{Cell, ScreenImage};

use super::geometry::{
    cross_prev, first_word_col, first_word_end, last_row, last_word_col, line_end, window_start,
};
use super::{Pos, Selection};

// ---------------------------------------------------------------- 双击 / 三击

/// 一个字形占的格区间 `[from, to)`。
///
/// 为什么需要它：网格是**按格**存的，宽字符占两格（第二格是延续格，`ch` 是空格）。
/// 选词若按格扫描，会把「一个汉字」当成「汉字 + 空格」两个东西，边界全错。
/// 这一层把格折叠回字形，选词只跟字形打交道。
pub(super) struct Glyph {
    pub(super) from: usize,
    pub(super) to: usize,
    pub(super) ch: char,
}

pub(super) fn glyphs(cells: &[Cell]) -> Vec<Glyph> {
    let mut out = Vec::with_capacity(cells.len());
    let mut col = 0;
    while col < cells.len() {
        let cell = cells[col];
        let to = if cell.wide {
            (col + 2).min(cells.len())
        } else {
            col + 1
        };
        out.push(Glyph {
            from: col,
            to,
            ch: cell.ch,
        });
        col = to;
    }
    out
}

/// 选词用的字符分类。
///
/// - `Word`：字母、数字、下划线。CJK 汉字也是「字母」，所以「你好，世界」双击得到「你好」。
/// - `Punct`：其余可见字符，按**同类连写**成组——`-->`、`::`、`...` 都能整段选中。
/// - `Space`：空白，双击不产生选区（选中一段空白没有意义，也无法从复制结果里看出来）。
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Class {
    Word,
    Punct,
    Space,
}

impl Class {
    pub(super) fn of(ch: char) -> Class {
        if ch.is_whitespace() {
            Class::Space
        } else if ch.is_alphanumeric() || ch == '_' {
            Class::Word
        } else {
            Class::Punct
        }
    }
}

/// 双击：把光标下的那个「词」变成选区。点在空白上返回 `None`。
///
/// 只读一个事实窗口——回看里的行同样是内容行号，所以双击历史行与双击当前屏
/// 走的是同一条路径。
pub fn word_at(image: &ScreenImage, at: Pos) -> Option<Selection> {
    let cells = &image.line_at(at.row)?.cells;
    let glyphs = glyphs(cells);
    let idx = glyphs
        .iter()
        .position(|g| g.from <= at.col && at.col < g.to)?;
    let class = Class::of(glyphs[idx].ch);
    if class == Class::Space {
        return None;
    }
    let mut start = idx;
    while start > 0 && Class::of(glyphs[start - 1].ch) == class {
        start -= 1;
    }
    let mut end = idx;
    while end + 1 < glyphs.len() && Class::of(glyphs[end + 1].ch) == class {
        end += 1;
    }
    Some(Selection::span(
        Pos::new(at.row, glyphs[start].from),
        Pos::new(at.row, glyphs[end].to),
    ))
}

/// 三击：整行，到最后一个非空白字符为止；空行返回 `None`。
///
/// 行尾不补空格：复制出来的文本应该和肉眼看到的一样，而不是带上半个屏幕的空白。
pub fn line_span_at(image: &ScreenImage, row: i64) -> Option<Selection> {
    let cells = &image.line_at(row)?.cells;
    let glyphs = glyphs(cells);
    let last = glyphs.iter().rposition(|g| !g.ch.is_whitespace())?;
    Some(Selection::span(
        Pos::new(row, 0),
        Pos::new(row, glyphs[last].to),
    ))
}

/// 跨行时的落点由调用方给（词的移动落到词边界，不是行边界）。
fn word_cross_next(image: &ScreenImage, row: i64, pick: fn(&ScreenImage, i64) -> usize) -> Pos {
    if row >= last_row(image) {
        Pos::new(row, line_end(image, row))
    } else {
        Pos::new(row + 1, pick(image, row + 1))
    }
}

fn word_cross_prev(image: &ScreenImage, row: i64, pick: fn(&ScreenImage, i64) -> usize) -> Pos {
    if row <= window_start(image) {
        Pos::new(row, 0)
    } else {
        Pos::new(row - 1, pick(image, row - 1))
    }
}

/// 同一类字符的连续区间 `[start, end]`（含两端，字形下标）。
pub(super) fn run_bounds(gs: &[Glyph], idx: usize) -> (usize, usize) {
    let class = Class::of(gs[idx].ch);
    let mut start = idx;
    while start > 0 && Class::of(gs[start - 1].ch) == class {
        start -= 1;
    }
    let mut end = idx;
    while end + 1 < gs.len() && Class::of(gs[end + 1].ch) == class {
        end += 1;
    }
    (start, end)
}

/// 光标所在（或右侧第一个）字形的下标。
fn index_at(gs: &[Glyph], col: usize) -> Option<usize> {
    gs.iter().position(|g| g.to > col)
}

pub(super) fn word_start_next(image: &ScreenImage, at: Pos) -> Pos {
    let Some(line) = image.line_at(at.row) else {
        return at;
    };
    let gs = glyphs(&line.cells);
    if let Some(idx) = index_at(&gs, at.col) {
        let (_, end) = run_bounds(&gs, idx);
        let mut j = end + 1;
        while j < gs.len() && Class::of(gs[j].ch) == Class::Space {
            j += 1;
        }
        if j < gs.len() {
            return Pos::new(at.row, gs[j].from);
        }
    }
    word_cross_next(image, at.row, first_word_col)
}

pub(super) fn word_end_next(image: &ScreenImage, at: Pos) -> Pos {
    let Some(line) = image.line_at(at.row) else {
        return at;
    };
    let gs = glyphs(&line.cells);
    if let Some(idx) = index_at(&gs, at.col) {
        let mut j = idx;
        while j < gs.len() && Class::of(gs[j].ch) == Class::Space {
            j += 1;
        }
        if j < gs.len() {
            let (_, end) = run_bounds(&gs, j);
            return Pos::new(at.row, gs[end].to);
        }
    }
    word_cross_next(image, at.row, first_word_end)
}

pub(super) fn word_start_prev(image: &ScreenImage, at: Pos) -> Pos {
    let Some(line) = image.line_at(at.row) else {
        return at;
    };
    let gs = glyphs(&line.cells);
    if let Some(idx) = index_at(&gs, at.col) {
        let (start, _) = run_bounds(&gs, idx);
        // 词中间 → 先回本词词首；已经在词首或落在空白里 → 再往左找一个词。
        if Class::of(gs[idx].ch) != Class::Space && gs[start].from < at.col {
            return Pos::new(at.row, gs[start].from);
        }
        let mut j = start;
        while j > 0 && Class::of(gs[j - 1].ch) == Class::Space {
            j -= 1;
        }
        if j > 0 {
            let (s, _) = run_bounds(&gs, j - 1);
            return Pos::new(at.row, gs[s].from);
        }
    }
    word_cross_prev(image, at.row, last_word_col)
}

pub(super) fn word_end_prev(image: &ScreenImage, at: Pos) -> Pos {
    let Some(line) = image.line_at(at.row) else {
        return at;
    };
    let gs = glyphs(&line.cells);
    if let Some(idx) = index_at(&gs, at.col) {
        let (start, _) = run_bounds(&gs, idx);
        let mut j = start;
        while j > 0 && Class::of(gs[j - 1].ch) == Class::Space {
            j -= 1;
        }
        if j > 0 {
            let (_, e) = run_bounds(&gs, j - 1);
            return Pos::new(at.row, gs[e].to);
        }
    }
    cross_prev(image, at.row)
}

// ------------------------------------------- 2026-11-13：词段（Word Part）切分
//
// 「词的一部分」在终端里没有现成的定义——`Op::WordStartPrev` 那一族的单位是
// 「一个 `Class::Word` 连写」，对 `fooBar_baz2` 会整段跳过。WindTerm 的
// `Previous Word Part` 要的是更细的一级：驼峰、下划线、字母 / 数字交界各断一次，
// 于是 `fooBar_baz2` 切成 `foo` / `Bar` / `baz` / `2` 四段。
//
// 为什么值得单独一层：这一级只在这两条命令上出现（双击选词、按词移动都不用它），
// 但它的判据有三条（大小写交界 / 下划线 / 字母数字交界），塞进 `apply` 的 match
// 里就会变成「只有一条命令走这条判断」的隐藏规则。

/// 词段里的字符：字母与数字。**不含下划线**——下划线是分隔符，自己不成段。
fn is_part_char(ch: char) -> bool {
    ch.is_alphanumeric()
}

/// `gs[i-1]` 与 `gs[i]` 之间是不是词段边界（调用方保证 `i > 0`）。
///
/// 三条判据，缺一条都会把常见的标识符切错：
///
/// - 任一侧不是字母数字（下划线、标点、空白）→ 断；
/// - 小写 → 大写（`fooBar` 的 `B`）→ 断；
/// - 连续大写之后第一次转小写（`HTTPServer` 的 `S`）→ 断，否则整个缩写会被
///   吞成一段 `HTTPS`，`Server` 再也跳不到；
/// - 字母 ↔ 数字（`baz2` 的 `2`）→ 断。
fn is_boundary_before(gs: &[Glyph], i: usize) -> bool {
    let (prev, cur) = (gs[i - 1].ch, gs[i].ch);
    if !is_part_char(prev) || !is_part_char(cur) {
        return true;
    }
    if cur.is_uppercase() && prev.is_lowercase() {
        return true;
    }
    if cur.is_uppercase()
        && prev.is_uppercase()
        && gs.get(i + 1).is_some_and(|g| g.ch.is_lowercase())
    {
        return true;
    }
    cur.is_ascii_digit() != prev.is_ascii_digit()
}

/// 一行里的词段，返回每段占的列区间 `[from, to)`，按阅读顺序。
///
/// 空行、纯标点行返回空表（没有段可跳）。宽字符按字形折叠，所以段边界永远落在
/// 字形边界上，不会切进「你」的延续格。
pub(super) fn part_spans(cells: &[Cell]) -> Vec<(usize, usize)> {
    let gs = glyphs(cells);
    let mut out: Vec<(usize, usize)> = Vec::new();
    let mut i = 0usize;
    while i < gs.len() {
        if !is_part_char(gs[i].ch) {
            i += 1;
            continue;
        }
        let start = i;
        let mut end = i;
        while end + 1 < gs.len() && !is_boundary_before(&gs, end + 1) {
            end += 1;
        }
        out.push((gs[start].from, gs[end].to));
        i = end + 1;
    }
    out
}

/// `Goto Next Word Part`：下一词段的**起点**。
///
/// 正在某段中间时先落到本段末尾——这与「词首 / 词尾」那一对的用法一致
/// （第一下退到边界，第二下才跨到下一段），连按两次不会跳过一整段。
pub(super) fn word_part_next(image: &ScreenImage, at: Pos) -> Pos {
    if let Some(line) = image.line_at(at.row) {
        let spans = part_spans(&line.cells);
        if let Some((_, to)) = spans.iter().find(|(f, t)| *f < at.col && at.col < *t) {
            return Pos::new(at.row, *to);
        }
        if let Some((from, _)) = spans.iter().find(|(f, _)| *f > at.col) {
            return Pos::new(at.row, *from);
        }
    }
    // 本行没有下一段：往下找第一个有词段的行（与按词移动一样会跨行）。
    let mut r = at.row + 1;
    while r <= last_row(image) {
        if let Some(line) = image.line_at(r) {
            if let Some((from, _)) = part_spans(&line.cells).first() {
                return Pos::new(r, *from);
            }
        }
        r += 1;
    }
    Pos::new(at.row, line_end(image, at.row))
}

/// `Goto Previous Word Part`：上一词段的起点；正在某段中间 / 段末时先回到本段起点。
pub(super) fn word_part_prev(image: &ScreenImage, at: Pos) -> Pos {
    if let Some(line) = image.line_at(at.row) {
        let spans = part_spans(&line.cells);
        // 段中间或段末 → 本段起点（段末也回本段，不然「上一段」会跳过一整段）。
        if let Some((from, _)) = spans.iter().find(|(f, t)| *f < at.col && at.col <= *t) {
            return Pos::new(at.row, *from);
        }
        // 已经在某段起点（或落在它左侧的空白 / 标点里）：退到再上一段的起点。
        if let Some((from, _)) = spans.iter().rev().find(|(_, t)| *t <= at.col) {
            return Pos::new(at.row, *from);
        }
    }
    // 本行没有上一段：往上找最后一个有词段的行。
    let mut r = at.row - 1;
    while r >= window_start(image) {
        if let Some(line) = image.line_at(r) {
            if let Some((from, _)) = part_spans(&line.cells).last() {
                return Pos::new(r, *from);
            }
        }
        r -= 1;
    }
    Pos::new(at.row, 0)
}


#[cfg(test)]
mod tests {
    use super::*;
    use term_core::grid::{Attrs, Colour, LineSlice};

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

    fn image(lines: &[&str], cols: usize) -> ScreenImage {
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
            history: 0,
        }
    }

    #[test]
    fn part_spans_split_camel_underscore_and_digit_boundaries() {
        // `fooBar_baz2` → foo / Bar / baz / 2，下划线自己不成段。
        let spans = part_spans(&cells("fooBar_baz2", 11));
        assert_eq!(spans, vec![(0, 3), (3, 6), (7, 10), (10, 11)]);
    }

    #[test]
    fn part_spans_split_acronyms_and_skip_punctuation() {
        // 连续大写之后的第一次小写是一道边界，否则缩写会把后面整个词吞掉。
        assert_eq!(
            part_spans(&cells("HTTPServer", 10)),
            vec![(0, 4), (4, 10)]
        );
        // 标点不是词段，只是分隔符。
        assert_eq!(part_spans(&cells("a.b", 3)), vec![(0, 1), (2, 3)]);
        assert_eq!(part_spans(&cells("-->", 3)), Vec::new(), "纯标点没有词段");
        assert_eq!(part_spans(&cells("   ", 3)), Vec::new(), "空行没有词段");
    }

    #[test]
    fn word_part_next_steps_to_the_part_end_then_the_next_start() {
        let img = image(&["fooBar_baz2"], 11);
        let at = |col: usize| word_part_next(&img, Pos::new(0, col));
        assert_eq!(at(1), Pos::new(0, 3), "段中间 → 本段末尾");
        assert_eq!(at(3), Pos::new(0, 7), "段首 → 下一段起点（跳过下划线）");
        assert_eq!(at(7), Pos::new(0, 10), "baz → 2");
        assert_eq!(at(10), Pos::new(0, 11), "最后一段之后 → 行尾");
        // 落在分隔符上也是「往右找下一个段」。
        assert_eq!(at(6), Pos::new(0, 7), "下划线上 → 下一段起点");
        // 落在标点上同样是往右找。
        let dotted = image(&["a.b"], 3);
        assert_eq!(word_part_next(&dotted, Pos::new(0, 1)), Pos::new(0, 2));
    }

    #[test]
    fn word_part_prev_returns_to_the_current_start_then_back_one_part() {
        let img = image(&["fooBar_baz2"], 11);
        let at = |col: usize| word_part_prev(&img, Pos::new(0, col));
        assert_eq!(at(4), Pos::new(0, 3), "段中间 → 本段起点");
        assert_eq!(at(6), Pos::new(0, 3), "段末 → 本段起点（不跳过整段）");
        assert_eq!(at(3), Pos::new(0, 0), "段首 → 上一段起点");
        assert_eq!(at(7), Pos::new(0, 3), "baz 段首 → 上一段 Bar 的起点");
        assert_eq!(at(0), Pos::new(0, 0), "已经在行首 → 不动");
        assert_eq!(at(9), Pos::new(0, 7), "段中间（baz 里）→ 本段起点");
    }

    #[test]
    fn word_parts_cross_lines_like_word_moves() {
        let img = image(&["ab", "", "cd"], 4);
        assert_eq!(
            word_part_next(&img, Pos::new(0, 3)),
            Pos::new(2, 0),
            "本行没有下一段 → 跳过空行，落到下一行第一个词段起点"
        );
        assert_eq!(
            word_part_prev(&img, Pos::new(2, 0)),
            Pos::new(0, 0),
            "本行没有上一段 → 上一行最后一个词段起点"
        );
        // 窗口两头都不动。
        assert_eq!(
            word_part_next(&img, Pos::new(2, 4)),
            Pos::new(2, 2),
            "最后一行之后没有词段 → 停在行尾"
        );
        assert_eq!(word_part_prev(&img, Pos::new(0, 0)), Pos::new(0, 0));
    }
}

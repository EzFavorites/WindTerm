//! 「选中一整个语义单位」与「选到折叠块边界」的纯位移规则。
//!
//! 与 `ops.rs` 里那 43 条「挪 head 到某一边界」互补：这里五条 Select 命令
//! 取 head 所在的**整段**（与 `Word` / `Line` 同款），Fold 两方向要读折叠
//! 引擎的输入行表，所以从带上下文的入口进来。所有边界都来自既有纯函数
//! （`geometry` / `folds`），这里不重新定义「段落 / 作用域 / 折叠块是什么」。

use term_core::coords::HistoryRow;
use term_core::grid::ScreenImage;
use term_core::osc133::CommandBlock;

use crate::vocab::Op;

use super::geometry::{
    fold_rows, indentation_target, last_row, line_end, pair_end_with, pair_start_with,
    paragraph_end, paragraph_start, scope_rows, window_start, Pairs,
};
use super::{Pos, Selection, Shape};

/// Select 五条中非折叠的四条：选中 head 所在的整个语义单位。
///
/// 已有选区时取并集而不是替换——「在当前选区基础上扩展一次」不能把用户
/// 已经选中的东西缩小；形状沿用旧选区（矩形扩出来还是矩形）。
pub(super) fn select_unit(
    op: Op,
    image: &ScreenImage,
    head: Pos,
    current: Option<Selection>,
    blocks: &[CommandBlock<i64>],
    separators: &str,
    pairs: &Pairs,
) -> Option<Selection> {
    let (start, end) = match op {
        Op::Paragraph => {
            let from = paragraph_start(image, head.row)?;
            let to = paragraph_end(image, head.row)?;
            (Pos::new(from, 0), Pos::new(to, line_end(image, to)))
        }
        Op::Pair => {
            let from = pair_start_with(image, head, pairs)?;
            let to = pair_end_with(image, head, pairs)?;
            (from, Pos::new(to.row, to.col + 1))
        }
        Op::Scope => {
            let rows = scope_rows(image, blocks);
            let (from, to) = unit_bounds(&rows, head.row, window_start(image), last_row(image));
            (Pos::new(from, 0), Pos::new(to, line_end(image, to)))
        }
        Op::Indentation => {
            let from = indentation_target(image, head.row, -1, separators)
                .map(|p| p.row + 1)
                .unwrap_or_else(|| window_start(image));
            let to = indentation_target(image, head.row, 1, separators)
                .map(|p| p.row - 1)
                .unwrap_or_else(|| last_row(image));
            if to < from {
                return current;
            }
            (Pos::new(from, 0), Pos::new(to, line_end(image, to)))
        }
        _ => return current,
    };
    Some(expand(current, start, end))
}

/// 折叠单位：`Select Fold` 选整块，`Select To Previous/Next Fold` 挪到块的头行。
///
/// 没有块表（用户没敲过回车、shell 也没报提示符）时返回原选区——与
/// `Op::CommandPrev` 的口径一致：没数据就不动，不假装跳到了第 0 行。
pub(super) fn fold_unit(
    op: Op,
    image: &ScreenImage,
    head: Pos,
    current: Option<Selection>,
    inputs: &[HistoryRow],
) -> Option<Selection> {
    let rows = fold_rows(image, inputs);
    match op {
        Op::Fold => {
            let (from, to) = rows
                .into_iter()
                .find(|(start, end)| *start <= head.row && head.row <= *end)?;
            Some(expand(
                current,
                Pos::new(from, 0),
                Pos::new(to, line_end(image, to)),
            ))
        }
        Op::FoldPrev => {
            let to = rows
                .iter()
                .copied()
                .filter(|(_, end)| *end < head.row)
                .map(|(start, _)| start)
                .max()?;
            super::ops::extend(current, head, Pos::new(to, 0))
        }
        Op::FoldNext => {
            let to = rows
                .iter()
                .copied()
                .filter(|(start, _)| *start > head.row)
                .map(|(start, _)| start)
                .min()?;
            super::ops::extend(current, head, Pos::new(to, 0))
        }
        _ => current,
    }
}

/// 把单位区间并入已有选区；没有旧选区就直接用单位区间。
fn expand(current: Option<Selection>, start: Pos, end: Pos) -> Selection {
    let (mut from, mut to) = (start, end);
    let shape = current.map(|s| s.shape());
    if let Some(cur) = current {
        let (cur_from, cur_to) = cur.normalized();
        if (cur_from.row, cur_from.col) < (from.row, from.col) {
            from = cur_from;
        }
        if (cur_to.row, cur_to.col) > (to.row, to.col) {
            to = cur_to;
        }
    }
    let mut out = Selection::span(from, to);
    if shape == Some(Shape::Block) {
        out = out.block();
    }
    out
}

/// 由「标记行」表圈出 head 所在的单位：`[上一个标记, 下一个标记 - 1]`。
/// 没有标记时退化到整个可见窗口（终端里「没有作用域」= 整个可见窗口）。
fn unit_bounds(markers: &[i64], row: i64, first: i64, last: i64) -> (i64, i64) {
    let from = markers
        .iter()
        .copied()
        .filter(|r| *r <= row)
        .max()
        .unwrap_or(first);
    let to = markers
        .iter()
        .copied()
        .find(|r| *r > row)
        .map(|r| r - 1)
        .unwrap_or(last);
    (from, to.max(from))
}

#[cfg(test)]
mod tests {
    use super::*;
    use term_core::grid::{Attrs, Cell, Colour, LineSlice};

    fn img(lines: &[&str]) -> ScreenImage {
        let cols = 16;
        let rows = lines
            .iter()
            .enumerate()
            .map(|(i, text)| {
                let mut cells: Vec<Cell> = text
                    .chars()
                    .map(|ch| Cell {
                        ch,
                        fg: Colour::Default,
                        bg: Colour::Default,
                        attrs: Attrs::EMPTY,
                        wide: false,
                    })
                    .collect();
                cells.resize(cols, Cell::default());
                LineSlice {
                    row: i as i64,
                    cells,
                }
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

    fn span(sel: Option<Selection>) -> ((i64, usize), (i64, usize)) {
        let (a, b) = sel.expect("这条命令必须真的改出选区").normalized();
        ((a.row, a.col), (b.row, b.col))
    }

    #[test]
    fn select_paragraph_covers_one_paragraph_and_never_shrinks() {
        let image = img(&["one", "two", "", "three", "four"]);
        let got = select_unit(
            Op::Paragraph,
            &image,
            Pos::new(1, 1),
            None,
            &[],
            crate::selection::DEFAULT_SEPARATORS,
            &Pairs::default_pairs(),
        );
        assert_eq!(span(got), ((0, 0), (1, 3)));
        // 已有选区延伸到第四行时，扩一次只会变大。
        let wide = Selection::span(Pos::new(1, 1), Pos::new(3, 2));
        let got = select_unit(
            Op::Paragraph,
            &image,
            Pos::new(1, 1),
            Some(wide),
            &[],
            crate::selection::DEFAULT_SEPARATORS,
            &Pairs::default_pairs(),
        );
        assert_eq!(span(got), ((0, 0), (3, 2)));
        // 空行上没有「这一段」，保持原样而不是造一个假选区。
        assert_eq!(
            select_unit(
                Op::Paragraph,
                &image,
                Pos::new(2, 0),
                None,
                &[],
                crate::selection::DEFAULT_SEPARATORS,
                &Pairs::default_pairs(),
            ),
            None
        );
    }

    #[test]
    fn select_pair_selects_the_whole_bracket_pair() {
        let image = img(&["foo(bar)baz"]);
        let got = select_unit(
            Op::Pair,
            &image,
            Pos::new(0, 6),
            None,
            &[],
            crate::selection::DEFAULT_SEPARATORS,
            &Pairs::default_pairs(),
        );
        assert_eq!(span(got), ((0, 3), (0, 8)));
        // 与 `GotoPairStart` 同口径：head 不在括号上也取最近的那一对，
        // 而不是什么都不选（那会让菜单看起来没反应）。
        assert_eq!(
            span(select_unit(
                Op::Pair,
                &image,
                Pos::new(0, 0),
                None,
                &[],
                crate::selection::DEFAULT_SEPARATORS,
                &Pairs::default_pairs(),
            )),
            ((0, 3), (0, 8))
        );
    }

    #[test]
    fn select_scope_falls_back_to_the_whole_window_without_blocks() {
        let image = img(&["a", "bb"]);
        let got = select_unit(
            Op::Scope,
            &image,
            Pos::new(1, 0),
            None,
            &[],
            crate::selection::DEFAULT_SEPARATORS,
            &Pairs::default_pairs(),
        );
        assert_eq!(span(got), ((0, 0), (1, 2)));
        let block = CommandBlock::<i64> {
            prompt_start: 0,
            prompt_end: Some(0),
            command_start: 0,
            end: None,
            exit: None,
        };
        let other = CommandBlock::<i64> {
            prompt_start: 2,
            prompt_end: Some(2),
            command_start: 2,
            end: None,
            exit: None,
        };
        let two_scopes = img(&["$ ls", "out", "$ pwd"]);
        let got = select_unit(
            Op::Scope,
            &two_scopes,
            Pos::new(1, 0),
            None,
            &[block, other],
            crate::selection::DEFAULT_SEPARATORS,
            &Pairs::default_pairs(),
        );
        assert_eq!(span(got), ((0, 0), (1, 3)));
    }

    #[test]
    fn select_indentation_covers_contiguous_same_width_rows() {
        let image = img(&["  a", "  b", "    c", "      d"]);
        let got = select_unit(
            Op::Indentation,
            &image,
            Pos::new(1, 2),
            None,
            &[],
            crate::selection::DEFAULT_SEPARATORS,
            &Pairs::default_pairs(),
        );
        assert_eq!(span(got), ((0, 0), (1, 3)));
    }

    #[test]
    fn fold_unit_selects_the_block_and_moves_to_its_header() {
        // 三次输入 → 两块：[0,2] 与 [3,5]；最后一条输入之后的输出不折。
        let image = img(&["$ a", "out1", "out2", "$ b", "out4", "out5", "$ c"]);
        let inputs = [
            HistoryRow::new(0).expect("测试行号"),
            HistoryRow::new(3).expect("测试行号"),
            HistoryRow::new(6).expect("测试行号"),
        ];
        let whole = fold_unit(Op::Fold, &image, Pos::new(1, 0), None, &inputs);
        assert_eq!(span(whole), ((0, 0), (2, 4)));
        let prev = fold_unit(Op::FoldPrev, &image, Pos::new(6, 0), None, &inputs);
        assert_eq!(span(prev), ((3, 0), (6, 0)), "锚点留在原处，head 退到最近的上一个折叠头");
        let next = fold_unit(Op::FoldNext, &image, Pos::new(0, 0), None, &inputs);
        assert_eq!(span(next), ((0, 0), (3, 0)), "下一块的折叠头在第 3 行");
        assert_eq!(
            fold_unit(Op::Fold, &image, Pos::new(6, 0), None, &inputs),
            None,
            "最后一条输入之后的正在产生的输出不属于任何折叠块"
        );
        assert_eq!(
            fold_unit(Op::FoldPrev, &image, Pos::new(1, 0), None, &[]),
            None,
            "没有折叠块时不动，不造第 0 行的假跳转"
        );
    }
}

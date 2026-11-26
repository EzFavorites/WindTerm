//! 带额外上下文的选区命令入口：`Select To Fold` / `Select Fold` 要知道
//! 「两次用户输入之间」的块边界，而那个边界只在折叠引擎的输入行表里。
//!
//! 不改动 `apply_with_pairs` / `apply_cmd_with_pairs` 的签名：键盘与既有的
//! 十几处调用点不该为了三条菜单命令多背一个参数。这里只多接一个 `inputs`
//! 参数，其余命令原样委托给旧入口——位移规则仍然只有一份。

use term_core::coords::HistoryRow;
use term_core::grid::ScreenImage;
use term_core::osc133::CommandBlock;

use crate::vocab::{Cmd, Op};

use super::geometry::{caret, Pairs};
use super::{Selection, Selections};

/// 与 `apply_with_pairs` 同形，但 Fold 三命令从 `inputs` 算块边界。
pub fn apply_with_context(
    op: Op,
    image: &ScreenImage,
    current: Option<Selection>,
    blocks: &[CommandBlock<i64>],
    separators: &str,
    pairs: &Pairs,
    inputs: &[HistoryRow],
) -> Option<Selection> {
    if matches!(op, Op::Fold | Op::FoldPrev | Op::FoldNext) {
        let head = current.map(|s| s.head()).or_else(|| caret(image))?;
        return super::units::fold_unit(op, image, head, current, inputs);
    }
    super::apply_with_pairs(op, image, current, blocks, separators, pairs)
}

/// 与 `apply_cmd_with_pairs` 同形，但把 `inputs` 透传给 Fold 三命令。
/// 集合命令走旧入口——它们从来不看网格，更不看折叠块。
pub fn apply_cmd_with_context(
    cmd: Cmd,
    image: &ScreenImage,
    blocks: &[CommandBlock<i64>],
    set: &mut Selections,
    separators: &str,
    pairs: &Pairs,
    inputs: &[HistoryRow],
) {
    match cmd {
        Cmd::Move(op) => {
            if set.is_locked() {
                return;
            }
            let updated = apply_with_context(op, image, set.active(), blocks, separators, pairs, inputs);
            set.assign_active(updated);
        }
        Cmd::Set(_) => super::apply_cmd_with_pairs(cmd, image, blocks, set, separators, pairs),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::selection::{apply_with_pairs, Pairs, DEFAULT_SEPARATORS};
    use term_core::grid::{Attrs, Cell, Colour, LineSlice};

    fn image(lines: &[&str]) -> ScreenImage {
        let cols = 12;
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
                LineSlice { row: i as i64, cells }
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
    fn fold_ops_only_move_when_context_supplies_real_blocks() {
        let img = image(&["$ a", "out", "$ b"]);
        let inputs = [
            HistoryRow::new(0).expect("测试行号"),
            HistoryRow::new(2).expect("测试行号"),
        ];
        let with = apply_with_context(
            Op::Fold,
            &img,
            None,
            &[],
            DEFAULT_SEPARATORS,
            &Pairs::default_pairs(),
            &inputs,
        )
        .expect("有输入行表就该选中一个折叠块");
        assert_eq!(with.normalized().0.row, 0, "块的折叠头是第 0 行");
        // 同一个 Op 走没有上下文的旧入口时什么都不做——这正说明新增的
        // 参数真的被用上了，而不是「挂了条永远走不到的分支」。
        assert_eq!(
            apply_with_pairs(
                Op::Fold,
                &img,
                None,
                &[],
                DEFAULT_SEPARATORS,
                &Pairs::default_pairs(),
            ),
            None
        );
    }

    #[test]
    fn non_fold_ops_delegate_to_the_one_existing_displacement_table() {
        let img = image(&["abc"]);
        let want = apply_with_pairs(
            Op::CharNext,
            &img,
            None,
            &[],
            DEFAULT_SEPARATORS,
            &Pairs::default_pairs(),
        );
        let got = apply_with_context(
            Op::CharNext,
            &img,
            None,
            &[],
            DEFAULT_SEPARATORS,
            &Pairs::default_pairs(),
            &[],
        );
        assert_eq!(got, want);
    }
}

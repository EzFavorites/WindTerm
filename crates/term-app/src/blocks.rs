//! 命令块边界（总前置）。
//!
//! `term_core::osc133` 把字节流翻成标记；这里把标记流翻成**可查询的块表**，
//! 让 Select Command / 命令块折叠 / 跨标签搜索共用同一份坐标。纯层，
//! 不碰事实窗口、不碰 UI、不缓存会过期的匹配数（F2 教训）。
//!
//! 关键取舍：块边界按**字节偏移**记录，而不是按网格行号。字节偏移来自
//! 输出流，换窗口尺寸也不会漂；真正要显示时再由投影层把偏移映射到行。

use term_core::coords::{history_to_view, HistoryRow};
use term_core::grid::ScreenImage;
use term_core::osc133::{blocks, MarkEvent};

/// 一条命令在事实窗口中的语义摘要。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Block {
    pub index: usize,
    pub prompt_start: usize,
    pub command_start: usize,
    pub end: Option<usize>,
    pub exit: Option<i32>,
    pub running: bool,
}

impl Block {
    /// 命令输出的字节区间（`C` 之后到 `D` 之前）。还在跑的命令到流末尾。
    pub fn output_range(&self, stream_len: usize) -> std::ops::Range<usize> {
        let start = self.command_start.min(stream_len);
        let end = self.end.unwrap_or(stream_len).min(stream_len).max(start);
        start..end
    }
}

/// 从标记流建成块表。
pub fn index(marks: &[MarkEvent]) -> Vec<Block> {
    blocks(marks)
        .into_iter()
        .enumerate()
        .map(|(i, b)| Block {
            index: i,
            prompt_start: b.prompt_start,
            command_start: b.command_start,
            end: b.end,
            exit: b.exit,
            running: b.end.is_none(),
        })
        .collect()
}

/// 找到包含某个字节偏移的块（`Select Command` 的落点判定）。
pub fn block_at(blocks: &[Block], offset: usize, stream_len: usize) -> Option<usize> {
    blocks
        .iter()
        .find(|b| b.output_range(stream_len).contains(&offset))
        .map(|b| b.index)
}

/// 只要成功结束的块（折叠视图用）。
pub fn finished(blocks: &[Block]) -> Vec<&Block> {
    blocks.iter().filter(|b| !b.running).collect()
}

/// 最近一条**用户真的打过**的命令所占的行（视口坐标，`0` = 屏顶、负数 = 回看）。
///
/// 取 `B..C`（提示符结束 → 命令输出开始），而不是 `A..D`：`Select Command`
/// 要的是「我刚敲的那行命令」，不是整段输出，也不该把提示符一起选进来。
/// 没有 `B` 的块（shell 只发 `A`/`C`/`D`）不算——宁可什么都不选，也不猜。
/// `history` 是当时的可回看行数，用来把绝对行号换算成视口坐标。
pub fn typed_command_rows(
    blocks: &[term_core::osc133::CommandBlock<i64>],
    history: usize,
) -> Option<(i64, i64)> {
    let b = blocks.iter().rev().find(|b| b.prompt_end.is_some())?;
    let start_row = b.prompt_end?;
    let end_row = b.command_start;
    // 绝对行号 → 视口行，换算只走 `term_core::coords` 这一个出口。
    // 内核保证绝对行号非负（历史被裁剪时负值项已整体丢弃）；真出现负值说明
    // 数据坏了，返回 None 比按负数算出一个视口行更安全。
    let start = history_to_view(HistoryRow::new(start_row)?, history as u64).get();
    let end = history_to_view(HistoryRow::new(end_row)?, history as u64)
        .get()
        .max(start);
    Some((start, end))
}

/// 全部块的**命令行起始行**（历史绝对行，升序）。
///
/// 与 [`typed_command_rows`] 的分工：那个只回答「最近一条命令在哪两行之间」，
/// 给 `Select Command` 用；这里回答「用户都在哪些行敲过命令」，给折叠算块用。
///
/// 用 `command_start` 而不是 `prompt_start`：折叠头应当是**命令行本身**，
/// 提示符跟着它一起留着才读得懂「这条命令的输出去哪了」。`prompt_start` 会让
/// 折叠头变成一串 `$`，收起来之后看不出折的是哪一条。
///
/// 仍在跑的最后一个块（`end == None`）也算进来：它的命令行已经是既成事实，
/// 只是输出还没写完。真正「最后一条输入之后不折叠」由
/// [`crate::folds::blocks_from_inputs`] 保证——它只用相邻两行之间的区间，
/// 最后一行后面根本没有下一个边界。
///
/// 负行丢弃（内核保证非负，出现负值说明数据坏了），并**去重且保序**：
/// 同一条命令如果有 `C` 重发（例如 shell 补发标记），这里不能给出两个相邻
/// 输入行——那会造出一个「零长度块」的假折叠点。
pub fn command_rows(blocks: &[term_core::osc133::CommandBlock<i64>]) -> Vec<HistoryRow> {
    let mut rows: Vec<HistoryRow> = blocks
        .iter()
        .filter_map(|b| HistoryRow::new(b.command_start))
        .collect();
    rows.dedup();
    rows
}

/// 把行区间裁到事实窗口里。完全在窗口外就返回 `None`——那时「选中了但看不见」
/// 比什么都不做更让人困惑，调用方据此给一句提示。
pub fn clamp_to_image(rows: (i64, i64), image: &ScreenImage) -> Option<(i64, i64)> {
    let view = image.view_range();
    let start = rows.0.max(view.start().get());
    // 尾行半开，所以这里刻意留着裸 `- 1`：换成 saturating_sub 是行为修正，
    // 不属于「零行为变化」的一刀，留给需要它的那个人单独提。
    let end = rows.1.min(view.end().get() - 1);
    if start > end {
        return None;
    }
    Some((start, end))
}

#[cfg(test)]
mod tests {
    use super::*;
    use term_core::osc133::{emit, parse_marks, CommandBlock};

    fn two_commands() -> Vec<u8> {
        let mut v = Vec::new();
        for exit in ["0", "3"] {
            v.extend(emit('A', None));
            v.extend(emit('B', None));
            v.extend(emit('C', None));
            v.extend_from_slice(b"output\n");
            v.extend(emit('D', Some(exit)));
        }
        v
    }

    #[test]
    fn two_commands_become_two_indexed_blocks_with_exit_codes() {
        let bytes = two_commands();
        let table = index(&parse_marks(&bytes));
        assert_eq!(table.len(), 2);
        assert_eq!(table[0].index, 0);
        assert_eq!(table[0].exit, Some(0));
        assert_eq!(table[1].exit, Some(3));
        assert!(table.iter().all(|b| !b.running));
    }

    #[test]
    fn block_at_finds_the_command_owning_an_offset() {
        let bytes = two_commands();
        let table = index(&parse_marks(&bytes));
        let mid0 = (table[0].command_start + table[0].end.unwrap()) / 2;
        let mid1 = (table[1].command_start + table[1].end.unwrap()) / 2;
        assert_eq!(block_at(&table, mid0, bytes.len()), Some(0));
        assert_eq!(block_at(&table, mid1, bytes.len()), Some(1));
        assert_eq!(block_at(&table, bytes.len() + 100, bytes.len()), None);
    }

    #[test]
    fn a_running_command_has_an_open_output_range() {
        let mut v = emit('A', None);
        v.extend(emit('C', None));
        v.extend_from_slice(b"still going");
        let table = index(&parse_marks(&v));
        assert_eq!(table.len(), 1);
        assert!(table[0].running);
        assert_eq!(table[0].output_range(v.len()).end, v.len());
        assert!(finished(&table).is_empty());
    }

    #[test]
    fn no_shell_integration_means_no_blocks_not_a_guess() {
        assert!(index(&parse_marks(b"plain output\r\n")).is_empty());
    }

    #[test]
    fn block_shape_matches_the_core_layer_exactly() {
        let marks = parse_marks(&two_commands());
        let core = blocks(&marks);
        let mine = index(&marks);
        assert_eq!(mine.len(), core.len());
        for (a, b) in mine.iter().zip(core.iter()) {
            let CommandBlock { prompt_start, prompt_end, command_start, end, exit } = *b;
            assert_eq!(a.prompt_start, prompt_start);
            assert_eq!(a.command_start, command_start);
            assert_eq!(a.end, end);
            assert_eq!(a.exit, exit);
            let _ = prompt_end; // 行坐标版本才用它（见 `typed_command_rows`）
        }
    }

    #[test]
    fn typed_command_rows_prefer_b_to_c_and_follow_history() {
        let mut v = emit('A', None);
        v.extend_from_slice(b"$ ");
        v.extend(emit('B', None));
        v.extend_from_slice(b"ls\r\n");
        v.extend(emit('C', None));
        // 字节偏移口径：B 必须落在 A 与 C 之间，才谈得上「只选命令」。
        let core = term_core::osc133::blocks(&parse_marks(&v));
        let b = core[0].prompt_end.expect("B 必须被记下");
        assert!(b > core[0].prompt_start && b < core[0].command_start);

        // 行号口径：`history = 5` 时，第 8 行的命令换算成视口第 3 行。
        let rows = term_core::osc133::blocks_at([
            (6i64, term_core::osc133::Mark::PromptStart),
            (8, term_core::osc133::Mark::PromptEnd),
            (8, term_core::osc133::Mark::CommandStart),
            (9, term_core::osc133::Mark::CommandEnd(Some(0))),
        ]);
        assert_eq!(typed_command_rows(&rows, 5), Some((3, 3)));
        // 没有 B 的块不算：宁可什么都不选，也不猜。
        let no_b = term_core::osc133::blocks_at([
            (8i64, term_core::osc133::Mark::PromptStart),
            (9, term_core::osc133::Mark::CommandStart),
        ]);
        assert_eq!(typed_command_rows(&no_b, 0), None);
    }

    #[test]
    fn typed_command_rows_matches_the_bare_arithmetic_within_reachable_domain() {
        // 可达域内（绝对行号 ≥ 0、history 不超 i64::MAX）：新旧逐值必须相等。
        // 这是迁移唯一被允许的判据——不靠「看着对」。
        for start_row in 0..=32i64 {
            for end_row in start_row..=32i64 {
                for history in 0..=32usize {
                    let rows = term_core::osc133::blocks_at([
                        (start_row, term_core::osc133::Mark::PromptStart),
                        (start_row, term_core::osc133::Mark::PromptEnd),
                        (end_row, term_core::osc133::Mark::CommandStart),
                    ]);
                    let h = history as i64;
                    let expected = (start_row - h, (end_row - h).max(start_row - h));
                    assert_eq!(
                        typed_command_rows(&rows, history),
                        Some(expected),
                        "start={start_row} end={end_row} history={history}"
                    );
                }
            }
        }
    }

    #[test]
    fn typed_command_rows_refuses_a_negative_absolute_row() {
        // 内核保证绝对行号非负（历史裁剪时负值项已整体丢弃）。真出现负值说明
        // 数据坏了：返回 None（不选）比按负数算出一个视口行更安全。
        // 旧实现会给出 Some((-1, 0))，这是本刀唯一刻意的行为修正。
        let rows = term_core::osc133::blocks_at([
            (-1i64, term_core::osc133::Mark::PromptStart),
            (-1, term_core::osc133::Mark::PromptEnd),
            (0, term_core::osc133::Mark::CommandStart),
        ]);
        assert_eq!(typed_command_rows(&rows, 0), None);
    }

    #[test]
    fn clamp_to_image_rejects_a_block_scrolled_out_of_view() {
        let image = ScreenImage::blank(4, 1, -2..2);
        assert_eq!(clamp_to_image((-10, -8), &image), None);
        assert_eq!(clamp_to_image((-1, 1), &image), Some((-1, 1)));
        assert_eq!(clamp_to_image((-5, -1), &image), Some((-2, -1)));
    }
}

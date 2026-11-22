//! Outline 面板的数据源：命令大纲（OSC 133 命令块）的纯提取。
//!
//! 文档里的 Outline 是「命令大纲」，不是终端正文的副本。旧实现退化的 `# ` 标题
//! 行过滤只是没有命令结构时的兜底：真有 OSC 133 集成时，大纲必须列**用户敲过的
//! 命令**（`CommandBlock.command_start` 那一行），而不是碰巧以 `#` 开头的输出。
//! 这里只做纯数据提取，面板绘制与 Filter 状态仍在 `tools.rs`。

use term_core::grid::LineSlice;
use term_core::osc133::CommandBlock;

/// 从命令块 + 当前事实窗口的行表提取大纲条目 `(绝对行, 摘要)`。
/// 没有命令块时回退到老的 `# ` 标题行口径（无 OSC 133 集成的会话也有个大纲）。
pub(in crate::ui) fn outline_items(
    blocks: &[CommandBlock<i64>],
    lines: &[LineSlice],
    history: usize,
    filter: &str,
) -> Vec<(i64, String)> {
    let needle = filter.trim().to_lowercase();
    let mut out = Vec::new();
    if blocks.is_empty() {
        for line in lines {
            let text = line.text().trim().to_string();
            if !text.starts_with("# ") {
                continue;
            }
            if !needle.is_empty() && !text.to_lowercase().contains(&needle) {
                continue;
            }
            out.push((line.row, super::tools::snippet(&text, 48)));
        }
        return out;
    }
    for block in blocks {
        let abs = block.command_start;
        let text = lines
            .iter()
            .find(|line| history as i64 + line.row == abs)
            .map(|line| line.text().trim().to_string())
            .unwrap_or_default();
        if text.is_empty() {
            continue;
        }
        if !needle.is_empty() && !text.to_lowercase().contains(&needle) {
            continue;
        }
        out.push((abs, super::tools::snippet(&text, 48)));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use term_core::grid::LineSlice;

    fn line(row: i64, text: &str) -> LineSlice {
        LineSlice {
            row,
            cells: text.chars().map(|ch| term_core::grid::Cell { ch, ..Default::default() }).collect(),
        }
    }

    #[test]
    fn command_blocks_win_over_the_hash_heading_fallback() {
        let lines = vec![line(0, "$ echo hi"), line(1, "# a heading")];
        let blocks = vec![CommandBlock {
            prompt_start: 0,
            prompt_end: Some(0),
            command_start: 0,
            end: Some(1),
            exit: Some(0),
        }];
        let items = outline_items(&blocks, &lines, 0, "");
        assert_eq!(items.len(), 1);
        assert!(items[0].1.contains("echo hi"), "大纲列的是命令，不是 # 标题");
        // 没有块时才回退到 # 标题（无 OSC 133 的会话）。
        let fallback = outline_items(&[], &lines, 0, "");
        assert_eq!(fallback.len(), 1);
        assert!(fallback[0].1.contains("a heading"));
    }

    #[test]
    fn outline_filter_is_case_insensitive() {
        let lines = vec![line(0, "$ echo HI")];
        let blocks = vec![CommandBlock {
            prompt_start: 0,
            prompt_end: Some(0),
            command_start: 0,
            end: Some(1),
            exit: None,
        }];
        assert_eq!(outline_items(&blocks, &lines, 0, "hi").len(), 1);
        assert!(outline_items(&blocks, &lines, 0, "zzz").is_empty());
    }
}

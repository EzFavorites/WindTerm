//! 按词折行 + 可调 Wrap column（纯函数，零 UI 依赖）。
//!
//! [`crate::wrap`] 交付「一行 → 若干段」的映射：老入口 `wrap_lines` / `segments_for`
//! 是**逐格贪心**（char mode），列宽固定 40。这里补两根轴：
//!
//! - **mode**：`Char`（老行为，逐格折）/ `Word`（折点回退到空白字形之后，不劈词）。
//! - **column**：`0` = 出厂口径（关软折时窗口边缘、开时 [`crate::wrap::DEFAULT_WRAP_COLUMN`]
//!   即 40）；非 0 = 用户指定的折行列宽，最终仍被内核列宽夹住。
//!
//! 老函数签名一律保持不变（等于 char + 0）；带 mode / 列宽的版本都是新增，
//! 这样仓库里已有的纯层单测一条都不用改。六个 UI 消费者统一走 [`segments_for_wrap`]：
//! 同一份 [`WrapConfig`] 产出同一张段表，正文 / 选区 / 查找 / margin / 预览 / 指针
//! 不会各自算出不同折点。

use term_core::grid::{glyph_columns, Cell};

use crate::wrap::{
    glyph_widths, glyph_widths_trimmed, segments_for, wrap_lines, SegmentTable, VisualRow,
    DEFAULT_WRAP_COLUMN,
};

/// 折行模式。`u8` 口径与 `Prefs.wrap_mode` 一致：0 = 按字符、1 = 按词。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WrapMode {
    Char,
    Word,
}

impl WrapMode {
    /// 越界值（手改过的 prefs 写了 9）回落 `Char` —— 接线前的行为。
    pub fn from_u8(v: u8) -> Self {
        if v == 1 { WrapMode::Word } else { WrapMode::Char }
    }

    pub fn as_u8(self) -> u8 {
        match self { WrapMode::Char => 0, WrapMode::Word => 1 }
    }
}

/// `Wrap column` 的可选区间（列）。设置页 `settings_live_wrap::parse_wrap_column`
/// 的输入框也夹进这同一区间：0 是 `Window edge` 哨兵，非零必须落在 8..=512。
/// 两处阈值写死为同一个数，读侧的 [`WrapSettings::from_prefs`] 保证手改过的
/// `prefs.json` 不会绕过设置页的下限。
pub const WRAP_COLUMN_MIN: usize = 8;
pub const WRAP_COLUMN_MAX: usize = 512;

/// 窗口级折行设置（来自 prefs，**不含**每标签的软折开关）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct WrapSettings {
    pub mode: WrapMode,
    /// `0` = Window edge（出厂：开软折时回落默认 40）。
    pub column: usize,
}

impl Default for WrapSettings {
    fn default() -> Self {
        Self { mode: WrapMode::Char, column: 0 }
    }
}

impl WrapSettings {
    /// 从落盘值构造。`0` = Window edge 哨兵原样保留；非零夹进
    /// [`WRAP_COLUMN_MIN`]..=[`WRAP_COLUMN_MAX`]，与设置页输入框同一档位：
    /// 手改 prefs 写了 `3` 或 `9999` 时，六个渲染消费者拿到的都是同一个合法值。
    pub fn from_prefs(mode: u8, column: usize) -> Self {
        let column = if column == 0 {
            0
        } else {
            column.clamp(WRAP_COLUMN_MIN, WRAP_COLUMN_MAX)
        };
        Self { mode: WrapMode::from_u8(mode), column }
    }

    /// 合上当前标签的软折开关，得到一帧的配置。
    pub fn with(self, enabled: bool) -> WrapConfig {
        WrapConfig::new(enabled, self)
    }
}

/// 一帧的折行配置：窗口设置 + 当前标签的软折开关。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct WrapConfig {
    pub enabled: bool,
    pub settings: WrapSettings,
}

impl WrapConfig {
    pub fn new(enabled: bool, settings: WrapSettings) -> Self {
        Self { enabled, settings }
    }

    /// 老行为：`Char` + 列宽 0。关时恒等（窗口边缘），开时折到默认 40。
    pub fn char_default(enabled: bool) -> Self {
        Self::new(enabled, WrapSettings::default())
    }

    /// 本帧有效列宽。关软折时原样返回 `image_cols`（恒等映射、零像素变化）；
    /// 开时取 `min(image_cols, column)`，其中 `column == 0` 回落默认 40。
    pub fn effective_cols(self, image_cols: usize) -> usize {
        if !self.enabled {
            return image_cols;
        }
        let wanted = if self.settings.column == 0 {
            DEFAULT_WRAP_COLUMN
        } else {
            self.settings.column.max(1)
        };
        image_cols.min(wanted)
    }
}

/// [`WrapConfig::effective_cols`] 的函数式入口，便于调用方不构造临时变量。
pub fn effective_wrap_cols_with(image_cols: usize, wrap: WrapConfig) -> usize {
    wrap.effective_cols(image_cols)
}

/// 按字形取「是否空白」标志，与 `glyph_widths` 平行（含行尾 padding 格）。
pub fn glyph_whitespace(cells: &[Cell]) -> Vec<bool> {
    let mut out = Vec::with_capacity(cells.len());
    let mut skip_placeholder = false;
    for cell in cells {
        if skip_placeholder {
            skip_placeholder = false;
            continue;
        }
        out.push(cell.ch.is_whitespace());
        if cell.wide {
            skip_placeholder = true;
        }
    }
    out
}

/// 按字形取「是否空白」标志，与 `glyph_widths_trimmed` 平行（裁行尾空白）。
pub fn glyph_whitespace_trimmed(cells: &[Cell]) -> Vec<bool> {
    glyph_columns(cells)
        .into_iter()
        .map(|col| cells[col].ch.is_whitespace())
        .collect()
}

/// 按词贪心：一格一格塞，塞不下时回退到本段最后一个空白字形**之后**收段。
///
/// 空白本身留在前一段末尾（不丢列，段区间始终连续），下一词整词落到下一段；
/// 段内没有空白（一个超长词）时退回逐格折，与 char mode 完全相同。
/// 宽字符按字形计，绝不劈成两半；比视口还宽的格子独占一段，保证循环前进。
fn pack_word(widths: &[u8], breaks: &[bool], cols: usize) -> Vec<(usize, usize)> {
    if cols == 0 {
        return Vec::new();
    }
    // 绝对 cell 列偏移：段起点 / 段宽都从这张表取，天然守恒、不重叠、不留缝。
    let mut offsets = Vec::with_capacity(widths.len() + 1);
    offsets.push(0usize);
    for &w in widths {
        offsets.push(offsets[offsets.len() - 1] + w as usize);
    }
    let mut out: Vec<(usize, usize)> = Vec::new();
    let mut seg_start = 0usize;
    let mut used = 0usize;
    for (i, &w) in widths.iter().enumerate() {
        let w = w as usize;
        if used == 0 && w > cols {
            // 比视口还宽：独占一段（并保证 cursor 前进，否则死循环）。
            out.push((offsets[i], w));
            seg_start = i + 1;
            used = 0;
            continue;
        }
        if used > 0 && used + w > cols {
            let fallback = (seg_start..i)
                .rev()
                .find(|&k| k > seg_start && breaks.get(k).copied().unwrap_or(false));
            let break_at = fallback.map_or(i, |k| k + 1);
            out.push((offsets[seg_start], offsets[break_at] - offsets[seg_start]));
            seg_start = break_at;
            used = offsets[i] - offsets[seg_start];
        }
        used += w;
    }
    // 收尾：在途段有内容就产出；整行只有超宽格时 seg_start == len，别补空段。
    if used > 0 || (out.is_empty() && widths.is_empty()) {
        out.push((offsets[seg_start], offsets[widths.len()] - offsets[seg_start]));
    }
    out
}

/// [`wrap_lines`] 的带模式版本。`Char` 走老实现（逐字节等价），`Word` 走空白回退。
pub fn wrap_lines_with_mode(
    row_widths: &[Vec<u8>],
    cols: usize,
    mode: WrapMode,
    whitespace: &[Vec<bool>],
) -> Vec<VisualRow> {
    if mode == WrapMode::Char {
        return wrap_lines(row_widths, cols);
    }
    let mut rows = Vec::new();
    for (line, widths) in row_widths.iter().enumerate() {
        let default_breaks: Vec<bool> = Vec::new();
        let breaks = whitespace.get(line).unwrap_or(&default_breaks);
        for (i, &(start_col, width)) in pack_word(widths, breaks, cols).iter().enumerate() {
            rows.push(VisualRow {
                line,
                start_col,
                width,
                soft_continued: i > 0,
                hard_continued: false, // 由 segments_for_with_mode 按 wraps 表回填
            });
        }
    }
    rows
}

/// [`segments_for`] 的带模式版本。`Char` 完全委托老实现，`Word` 才读 cell 判空白。
pub fn segments_for_with_mode(
    lines_cells: &[Vec<Cell>],
    wrap_cols: usize,
    trim: bool,
    hard_continued: &[bool],
    mode: WrapMode,
) -> SegmentTable {
    if mode == WrapMode::Char {
        return segments_for(lines_cells, wrap_cols, trim, hard_continued);
    }
    let mut widths = Vec::with_capacity(lines_cells.len());
    let mut whitespace = Vec::with_capacity(lines_cells.len());
    for cells in lines_cells {
        if trim {
            widths.push(glyph_widths_trimmed(cells));
            whitespace.push(glyph_whitespace_trimmed(cells));
        } else {
            widths.push(glyph_widths(cells));
            whitespace.push(glyph_whitespace(cells));
        }
    }
    let mut table = SegmentTable {
        rows: wrap_lines_with_mode(&widths, wrap_cols, mode, &whitespace),
        cols: wrap_cols,
    };
    for seg in &mut table.rows {
        seg.hard_continued = hard_continued.get(seg.line).copied().unwrap_or(false);
    }
    table
}

/// [`crate::wrap::segments_from_image`] 的带模式版本。
pub fn segments_from_image_with_mode(
    image: &term_core::grid::ScreenImage,
    wrap_cols: usize,
    trim: bool,
    mode: WrapMode,
) -> SegmentTable {
    let cells: Vec<Vec<Cell>> = image.lines.iter().map(|l| l.cells.clone()).collect();
    segments_for_with_mode(&cells, wrap_cols, trim, &vec![false; cells.len()], mode)
}

/// 六个消费者共用的**唯一**入口：由事实窗口 + 本帧配置算出段表。
///
/// 关软折时强制走 char + 不裁 padding，与接线前逐字节等价（零像素基线）；
/// 开软折时用同一份 mode / column，正文、选区、查找、margin、预览、指针同源。
pub fn segments_for_wrap(
    image: &term_core::grid::ScreenImage,
    wrap: WrapConfig,
) -> SegmentTable {
    let cols = wrap.effective_cols(image.cols.max(1));
    let mode = if wrap.enabled { wrap.settings.mode } else { WrapMode::Char };
    segments_from_image_with_mode(image, cols, wrap.enabled, mode)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn widths(s: &str) -> Vec<u8> {
        s.chars().map(|c| if c.is_ascii() { 1 } else { 2 }).collect()
    }

    fn breaks(s: &str) -> Vec<bool> {
        s.chars().map(|c| c.is_whitespace()).collect()
    }

    fn segs(rows: &[VisualRow]) -> Vec<(usize, usize)> {
        rows.iter().map(|r| (r.start_col, r.width)).collect()
    }

    // ---- 列宽：0 守旧，非 0 自定义 ----

    /// 手改 prefs 绕过设置页写出的越界列宽，必须在**读侧**被夹回同一档位，
    /// 否则六个渲染消费者会各按各的兜底折出不同段表。
    #[test]
    fn from_prefs_clamps_nonzero_columns_to_the_settings_range() {
        assert_eq!(WRAP_COLUMN_MIN, 8);
        assert_eq!(WRAP_COLUMN_MAX, 512);
        assert_eq!(WrapSettings::from_prefs(1, 0).column, 0, "Window edge 哨兵原样保留");
        assert_eq!(WrapSettings::from_prefs(1, 3).column, 8, "低于下限夹到 8");
        assert_eq!(WrapSettings::from_prefs(1, 9999).column, 512, "高于上限夹到 512");
        assert_eq!(WrapSettings::from_prefs(0, 80).column, 80, "合法值原样生效");
        // 生效列宽也随之被夹住，不会出现「设置页显示 8、段表按 3 折」。
        let settings = WrapSettings::from_prefs(1, 3);
        assert_eq!(WrapConfig::new(true, settings).effective_cols(120), 8);
    }

    #[test]
    fn column_zero_keeps_the_legacy_behaviour() {
        let settings = WrapSettings::default();
        assert_eq!(WrapConfig::new(false, settings).effective_cols(120), 120);
        assert_eq!(WrapConfig::new(true, settings).effective_cols(120), 40);
        assert_eq!(WrapConfig::new(true, settings).effective_cols(20), 20);
        // 老函数与新入口逐值一致。
        assert_eq!(
            crate::wrap::effective_wrap_cols(120, true),
            WrapConfig::new(true, settings).effective_cols(120)
        );
    }

    #[test]
    fn custom_column_is_clamped_to_the_grid_width() {
        let settings = WrapSettings::from_prefs(0, 12);
        assert_eq!(WrapConfig::new(true, settings).effective_cols(120), 12);
        assert_eq!(WrapConfig::new(true, settings).effective_cols(8), 8);
        // 关软折时自定义列不生效（窗口边缘）。
        assert_eq!(WrapConfig::new(false, settings).effective_cols(120), 120);
        // 手改成 0 之外的越界值 1：仍合法，但不会比 1 更小。
        let tiny = WrapSettings::from_prefs(0, 0);
        assert_eq!(WrapConfig::new(true, tiny).effective_cols(4), 4);
    }

    // ---- 按词折行 ----

    #[test]
    fn word_mode_falls_back_to_the_whitespace_boundary() {
        let text = "aaa bbb ccc ddd";
        let rows = wrap_lines_with_mode(
            &[widths(text)],
            10,
            WrapMode::Word,
            &[breaks(text)],
        );
        // char 会在第 10 格硬切，把 ccc 劈成 cc|c；word 退到空白后收段。
        assert_eq!(segs(&rows), vec![(0, 8), (8, 7)], "空白格留在前段末尾");
        assert_eq!(
            rows.iter().map(|r| r.soft_continued).collect::<Vec<_>>(),
            vec![false, true]
        );
        assert_eq!(
            wrap_lines(&[widths(text)], 10).len(),
            2,
            "char 也是两段，但折点不同"
        );
    }

    #[test]
    fn word_mode_keeps_a_long_word_intact_by_falling_back_to_char() {
        let text = "abcdefghijkl";
        let rows = wrap_lines_with_mode(
            &[widths(text)],
            5,
            WrapMode::Word,
            &[breaks(text)],
        );
        assert_eq!(segs(&rows), vec![(0, 5), (5, 5), (10, 2)]);
        assert_eq!(segs(&rows), segs(&wrap_lines(&[widths(text)], 5)));
    }

    #[test]
    fn word_mode_splits_at_the_last_whitespace_not_the_first() {
        let text = "one two three four";
        let rows = wrap_lines_with_mode(
            &[widths(text)],
            12,
            WrapMode::Word,
            &[breaks(text)],
        );
        // 12 列 = "one two thre"，回退到 three 之前的空格（下标 7）之后收段。
        assert_eq!(segs(&rows), vec![(0, 8), (8, 10)]);
    }

    #[test]
    fn word_mode_never_loses_a_column_and_keeps_wide_glyphs_whole() {
        let text = "ab汉 cd汉 e";
        let rows = wrap_lines_with_mode(
            &[widths(text)],
            5,
            WrapMode::Word,
            &[breaks(text)],
        );
        let total: usize = widths(text).iter().map(|&w| w as usize).sum();
        assert_eq!(rows.iter().map(|r| r.width).sum::<usize>(), total);
        for pair in rows.windows(2) {
            assert_eq!(pair[1].start_col, pair[0].start_col + pair[0].width);
        }
    }

    #[test]
    fn word_mode_of_an_empty_line_still_yields_one_segment() {
        let rows = wrap_lines_with_mode(&[vec![]], 8, WrapMode::Word, &[vec![]]);
        assert_eq!(segs(&rows), vec![(0, 0)]);
        assert!(wrap_lines_with_mode(&[vec![]], 0, WrapMode::Word, &[vec![]]).is_empty());
    }

    #[test]
    fn char_mode_with_mode_entry_is_byte_identical_to_the_old_entry() {
        let lines = vec![widths("abc"), widths("a汉b汉c"), vec![], widths("0123456789")];
        let blanks: Vec<Vec<bool>> = lines.iter().map(|w| vec![false; w.len()]).collect();
        for cols in [1, 3, 5, 10] {
            assert_eq!(
                wrap_lines_with_mode(&lines, cols, WrapMode::Char, &blanks),
                wrap_lines(&lines, cols)
            );
        }
    }
}

//! 空白与符号可视化：**参数**（[`MarksParams`]）与**纯决策**（[`push_row_marks`]）。
//!
//! 本文件零 `egui`，可以无头单测；把 `Mark` 变成 `painter` 调用的是
//! `marks_paint.rs`。这条分界不是洁癖：`view.rs` 的逐行缓存（`LineCache`）只缓存
//! `spans` / `bg_runs`，可视化开关**不在缓存里**——它每帧都要重新判一次，
//! 于是它的判据必须便宜、可单测、且不依赖任何绘制上下文。
//!
//! # 六项各画在哪一像素
//!
//! | 开关 | 画在哪 | 谁决定 |
//! |------|--------|--------|
//! | [`MarksParams::line_endings`] | 逻辑行**最后一个非空白格之后**那一格，一个 `$` | [`Mark::LineEnding`] |
//! | [`MarksParams::column_marker`] | 第 N 列的**右边界**一条贯穿全屏的竖线 | [`MarksParams::column_marker_col`] |
//! | [`MarksParams::indent_guides`] | 行首连续空白里每 4 列一条竖线（`4/8/12…`，不超过缩进宽度） | [`Mark::IndentGuide`] |
//! | [`MarksParams::wrap_symbols`] | **自动换行续行**的最后一列右下角一个 L 形勾 | [`Mark::WrapSymbol`] |
//! | [`MarksParams::whitespace`] | 空格画小方点、Tab 画箭头（线段），范围由五档模式定 | [`Mark::SpaceDot`] / [`Mark::TabArrow`] |
//! | [`MarksParams::caret_line`] | 光标所在行铺一层极淡底（压在字形之下，所以画在 `view.rs`） | [`caret_band_row`] |
//!
//! # 两层模型（与 `hex_params.rs` 同一套）
//!
//! | 谁 | 存在哪 | 落盘 | 生效范围 |
//! |----|--------|------|----------|
//! | `Prefs`（`view_*` / `text_highlight_caret_line`） | 盘上 | 是 | **新标签** |
//! | `View ▸ *` / `Text ▸ HighlightCaretLine` | `Tab.marks` | 否 | 只改当前标签 |
//!
//! 夹值只发生在 [`MarksParams::from_prefs`] 一处：`prefs.json` 是手可改的，
//! 列标记写个 `9999` 或 `3` 都必须在进渲染前落回 `8..=512`。
//!
//! # 「行尾空白」在终端里是什么意思（这一档唯一的诚实解释）
//!
//! 网格右侧的空白格既是「程序写进去的尾部空格」也是「这一行没写到的补白」，
//! 两者在 `Cell` 上**逐字节相同**（都是 `' '` + 默认样式），内核也没有记录
//! 「这一逻辑行写到第几列」。所以：
//!
//! - [`WhitespaceMode::ShowAll`] 画整行所有空白格（含右侧补白）——「全部」就是全部；
//! - [`WhitespaceMode::InLineEnd`] 只画内容末尾之后那一段（`content_end..cols`），
//!   也就是「行尾连续空白」。
//!
//! 副作用是短行右侧会出现一排点。这不是 bug，是「这一档正在画行尾空白」的唯一
//! 可见证据；把右侧补白排掉的话这一档就永远画不出任何东西——那正是本仓库
//! 反复回退过的那种「点了屏幕不变」的假开关。

use term_core::grid::Cell;

/// 参考列标记的合法区间（含两端）。
///
/// 下界 `8` 是「比最短的缩进还窄的参考线没有意义」；上界 `512` 是「再宽的窗口
/// 也不会有第 513 列可标」。越界由 [`MarksParams::from_prefs`] 夹掉，不是回落
/// 出厂值——用户填 `1000` 的意思是「尽量靠右」，夹到 `512` 比退回 `80` 更贴近
/// 他的意图。
pub(in crate::ui) const COLUMN_MARKER_RANGE: std::ops::RangeInclusive<usize> = 8..=512;

/// 参考列标记的出厂列号（视觉基线里那一格的默认值就是 80）。
pub(in crate::ui) const COLUMN_MARKER_DEFAULT: usize = 80;

/// 缩进参考线的步长：每 4 列一档。写死 4 而不是做成参数——视觉基线没有这个设置项，
/// 多一个可配维度就多一处「改了没反应」的可能。
pub(in crate::ui) const INDENT_STEP: usize = 4;

/// 空白符号的五档**互斥**模式（`View ▸ White Spaces`）。
///
/// 互斥是菜单的语义：五个菜单项里永远只有一个勾。所以它是**一个枚举**而不是
/// 五个 bool——五个 bool 会出现「全勾上该按谁算」这种没有答案的状态。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::ui) enum WhitespaceMode {
    /// 不画（出厂态 = 现状，任何一个字节都不变）。
    HideAll,
    /// 整行所有空白格都画。
    ShowAll,
    /// 只画落在选区内的那些。
    InSelection,
    /// 只画行首连续空白。
    InLineStart,
    /// 只画行尾连续空白（内容末尾之后）。
    InLineEnd,
}

impl WhitespaceMode {
    /// 五档的文件顺序：菜单按它排、`prefs` 的整数按它编号。
    pub(in crate::ui) const ALL: [WhitespaceMode; 5] = [
        WhitespaceMode::HideAll,
        WhitespaceMode::ShowAll,
        WhitespaceMode::InSelection,
        WhitespaceMode::InLineStart,
        WhitespaceMode::InLineEnd,
    ];

    /// 落盘用的编号（`Prefs.view_whitespace`）。
    pub(in crate::ui) fn index(self) -> usize {
        Self::ALL.iter().position(|m| *m == self).unwrap_or(0)
    }

    /// 从编号还原；越界回落 [`WhitespaceMode::HideAll`]（手改过的文件写个 `9`
    /// 不该让整份 prefs 作废，也不该让屏幕出现某种没人定义过的画法）。
    pub(in crate::ui) fn from_index(n: usize) -> Self {
        Self::ALL.get(n).copied().unwrap_or(WhitespaceMode::HideAll)
    }
}

/// 一个标签的六项可视化参数。
///
/// 与 `HexParams` 同构：都是「每条会话一份、初值从 `Prefs` 拷、菜单只改当前标签」。
/// 区别只在有没有可夹的数——这里有一个（列标记）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::ui) struct MarksParams {
    /// 逻辑行行尾画 `$`。
    pub line_endings: bool,
    /// 第 N 列（1 起）的右边界画一条贯穿竖线；`None` = 不画。
    pub column_marker: Option<usize>,
    /// 按行首缩进画竖参考线，每 4 列一档。
    pub indent_guides: bool,
    /// 自动换行的续行画折行标记。
    pub wrap_symbols: bool,
    /// 空白符号的五档互斥模式。
    pub whitespace: WhitespaceMode,
    /// 光标行铺一层极淡底。
    pub caret_line: bool,
}

impl Default for MarksParams {
    /// 出厂 = **六项全关**。这不是「还没实现」，是必须的默认：六项里任何一项
    /// 默认打开都会让五张视觉基线当场变样，而基线的判据是「逐字节一致」。
    fn default() -> Self {
        Self {
            line_endings: false,
            column_marker: None,
            indent_guides: false,
            wrap_symbols: false,
            whitespace: WhitespaceMode::HideAll,
            caret_line: false,
        }
    }
}

impl MarksParams {
    /// 由盘上六项造一份：列标记 `0` = 关，其余值夹进 [`COLUMN_MARKER_RANGE`]。
    ///
    /// 六个参数收在同一个入口，理由与 `HexParams::from_prefs` 一字不差：建标签
    /// 那条路上漏掉一项，新标签就会静默退回出厂值，而那正是「设置页改了没反应」
    /// 的形状。
    pub(in crate::ui) fn from_prefs(
        line_endings: bool,
        column_marker: usize,
        indent_guides: bool,
        wrap_symbols: bool,
        whitespace: usize,
        caret_line: bool,
    ) -> Self {
        Self {
            line_endings,
            column_marker: column_marker_from_prefs(column_marker),
            indent_guides,
            wrap_symbols,
            whitespace: WhitespaceMode::from_index(whitespace),
            caret_line,
        }
    }

    /// 六项里任一开着。给调用方一句判据，免得每处各列一遍六个字段。
    pub(in crate::ui) fn any_enabled(self) -> bool {
        self.line_endings
            || self.column_marker.is_some()
            || self.indent_guides
            || self.wrap_symbols
            || self.whitespace != WhitespaceMode::HideAll
            || self.caret_line
    }

    /// 需要**逐行**画的那四项（列标记是整屏一条，光标行底色压在字形之下）。
    ///
    /// `marks_paint::paint` 用它做整段早退：六项全关时这个函数一次都不进循环，
    /// 关掉的代价是零。
    pub(in crate::ui) fn paints_row_marks(self) -> bool {
        self.line_endings
            || self.indent_guides
            || self.wrap_symbols
            || self.whitespace != WhitespaceMode::HideAll
    }

    /// 列标记这一帧要画在第几列（1 起的列号）；不画时 `None`。
    ///
    /// 网格比 `N` 窄时返回 `None`：那条线会落在网格右边界之外，画出来就是
    /// 「一条出现在空白区的线」，比不画更让人困惑。等用户把窗口拉宽它会自己回来。
    pub(in crate::ui) fn column_marker_col(self, cols: usize) -> Option<usize> {
        self.column_marker.filter(|n| *n <= cols.max(1))
    }

    /// 换一列标记（`None` = 关）。夹值与 prefs 那一侧共用一处。
    pub(in crate::ui) fn with_column_marker(self, col: Option<usize>) -> Self {
        Self {
            column_marker: col.map(clamp_column_marker),
            ..self
        }
    }
}

/// 盘上的列标记值 → 生效值：`0` = 关，其余夹进 `8..=512`。
///
/// `0` 是「老文件没这一项」的哨兵（与本仓库其余 prefs 字段同一套写法），
/// 同时也正好是「关」——两者要的效果一样，所以不需要 `default_*` 特判。
pub(in crate::ui) fn column_marker_from_prefs(n: usize) -> Option<usize> {
    (n > 0).then(|| clamp_column_marker(n))
}

/// 把一个列号夹进 [`COLUMN_MARKER_RANGE`]。
pub(in crate::ui) fn clamp_column_marker(n: usize) -> usize {
    n.clamp(*COLUMN_MARKER_RANGE.start(), *COLUMN_MARKER_RANGE.end())
}

/// 一行要画的一个标记。坐标是**列**，换算成像素是 `marks_paint.rs` 的事。
///
/// 为什么是一个枚举而不是直接画：判据要能单测。`Mark` 可比较、可打印，
/// 于是「五档 whitespace 各画哪几格」就是一句 `assert_eq!`，不需要窗口。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::ui) enum Mark {
    /// 空格：这一格中央一个小方点。
    SpaceDot { col: usize },
    /// Tab：这一格里一个箭头（横杆 + 两笔斜线，**线段**画的，不依赖字体字形）。
    TabArrow { col: usize },
    /// 缩进参考线：画在 `col` 这一格的**左边界**，贯穿整行高。
    IndentGuide { col: usize },
    /// 逻辑行行尾的 `$`，画在 `col` 这一格。
    LineEnding { col: usize },
    /// 自动换行续行的折行标记，画在 `col` 这一格的右下角。
    WrapSymbol { col: usize },
}

/// 判「这一行该画什么」所需的全部输入。
///
/// 显式传入而不是在函数里读 `GridSnapshot`：快照里有 `wraps` 的**绝对行号**，
/// 而绘制层才拿得到「窗口起点 + `image.history`」的换算。把换算留在绘制层之后，
/// 这里的用例可以只构造两个 bool（本行是续行 / 下一行是续行），不必造一整张网格。
pub(in crate::ui) struct RowLine<'a> {
    /// 这一行的 cell（长度 = 网格列数）。
    pub cells: &'a [Cell],
    /// 本行是自动换行产生的**续行**（`wraps` 里有本行的绝对行号）。
    pub continuation: bool,
    /// 下一行是续行——即本行被硬折了，本行不是逻辑行的结尾。
    pub next_continuation: bool,
    /// 本行被选中的列区间（可能有多段：一批选区）。只在 `InSelection` 档用。
    pub selected: &'a [(usize, usize)],
}

/// 这一格是不是「空白」——空格、Tab、NUL 占位都算。
///
/// 与 `view.rs::is_blank` 分开写：那边判的是「要不要断开 span 合批」，`\t` 在那里
/// **不是**空白（它要进 span 交给字体排，否则一个 Tab 就把前后两个词分开成两次
/// `text()` 调用）；这边判的是「要不要画白空符号」，`\t` 当然算。
pub(in crate::ui) fn is_whitespace_cell(cell: &Cell) -> bool {
    matches!(cell.ch, ' ' | '\0' | '\t')
}

/// 最后一个非空白格之后的那一列。整行空白时是 `0`。
///
/// 宽字符占**两格**（第二格是占位），所以它给出的是 `col + 2`：`$` 要落在整个
/// 字形之后，落在占位格上就是把标记戳进了「中」的右半边。
pub(in crate::ui) fn content_end(cells: &[Cell]) -> usize {
    let mut end = 0usize;
    let mut prev_wide = false;
    for (col, cell) in cells.iter().enumerate() {
        let placeholder = prev_wide;
        prev_wide = cell.wide;
        if placeholder || is_whitespace_cell(cell) {
            continue;
        }
        end = col + if cell.wide { 2 } else { 1 };
    }
    end
}

/// 行首连续空白格的个数（缩进宽度，单位是格）。
pub(in crate::ui) fn leading_blanks(cells: &[Cell]) -> usize {
    let mut n = 0usize;
    let mut prev_wide = false;
    for cell in cells {
        let placeholder = prev_wide;
        prev_wide = cell.wide;
        if placeholder || !is_whitespace_cell(cell) {
            break;
        }
        n += 1;
    }
    n
}

/// 这一行的绝对行号在不在 `wraps` 里（= 是不是续行）。
///
/// 判据本身在 `term_core::grid::is_continued`（`margin.rs` 的 `~` 与选区复制
/// 用的是同一句），这里只保留原来的函数名与签名，调用点不动。
pub(in crate::ui) fn is_wrapped(wraps: &[i64], abs_row: i64) -> bool {
    term_core::grid::is_continued(wraps, abs_row)
}

/// 光标行底色该画在视口的第几行；`None` = 不画。
///
/// 三条判据缺一不可：
///
/// - 开关开着；
/// - 视口**贴底**——回看时光标不在这一屏上，给某一行铺底色就成了「光标在这里」的假话；
/// - 行号夹进本屏行数（`cursor.0` 理论上总在屏内，但夹具 / resize 的中间态不保证）。
pub(in crate::ui) fn caret_band_row(
    caret_line: bool,
    at_bottom: bool,
    cursor_row: usize,
    rows: usize,
) -> Option<usize> {
    if !caret_line || !at_bottom || rows == 0 {
        return None;
    }
    Some(cursor_row.min(rows - 1))
}

/// 求出这一行要画的所有标记，按「空白符号（列升序）→ 缩进线 → 行尾 → 折行」排列。
///
/// `out` 会被清空后复用：绘制层每帧只有这一个缓冲，24 行不产生 24 次分配。
/// 顺序固定是为了让单测能整段 `assert_eq!`——
/// 判据不是「画了哪些」而是「按什么次序画在哪一格」。
pub(in crate::ui) fn push_row_marks(params: &MarksParams, line: &RowLine<'_>, out: &mut Vec<Mark>) {
    out.clear();
    if !params.paints_row_marks() {
        return;
    }
    let cells = line.cells;
    let cols = cells.len();
    if cols == 0 {
        return;
    }
    let end = content_end(cells);
    let indent = leading_blanks(cells);

    // 1. 空白符号。
    if params.whitespace != WhitespaceMode::HideAll {
        let mut prev_wide = false;
        for (col, cell) in cells.iter().enumerate() {
            let placeholder = prev_wide;
            prev_wide = cell.wide;
            // 宽字符的后半格不是空白，是同一个字形：给它画点会把「中」戳一个洞。
            if placeholder || !is_whitespace_cell(cell) {
                continue;
            }
            let wanted = match params.whitespace {
                WhitespaceMode::HideAll => false,
                WhitespaceMode::ShowAll => true,
                WhitespaceMode::InSelection => {
                    line.selected.iter().any(|(from, to)| col >= *from && col < *to)
                }
                WhitespaceMode::InLineStart => col < indent,
                WhitespaceMode::InLineEnd => col >= end,
            };
            if !wanted {
                continue;
            }
            // Tab 与空格分成两种标记：绘制层的画法完全不同（箭头是三条线段）。
            if cell.ch == '\t' {
                out.push(Mark::TabArrow { col });
            } else {
                out.push(Mark::SpaceDot { col });
            }
        }
    }

    // 2. 缩进参考线：每 `INDENT_STEP` 列一条，只画到缩进宽度为止。
    //
    // 「只画到该行真正有内容的列为止」在这里就是 `col <= indent`：缩进 6 格时
    // 画第 4 列那条，第 8 列不画——第 8 列已经越过内容起点，那条线会压在字上。
    if params.indent_guides {
        let mut col = INDENT_STEP;
        while col <= indent && col < cols {
            out.push(Mark::IndentGuide { col });
            col += INDENT_STEP;
        }
    }

    // 3. 行尾标记：**逻辑行**才有，续行与「被硬折的行」都没有。
    //
    // `next_continuation` 那一条容易漏：本行自己不在 `wraps` 里（它是逻辑行的
    // 开头），但它写满了、下一行是续行——这时它也不是逻辑行的结尾，画 `$` 就是
    // 在半个句子的中间画了个句号。
    if params.line_endings && !line.continuation && !line.next_continuation {
        out.push(Mark::LineEnding {
            col: end.min(cols - 1),
        });
    }

    // 4. 折行标记：只有续行有（`wraps` 的语义就是「这一行是上一行的后半截」）。
    if params.wrap_symbols && line.continuation {
        out.push(Mark::WrapSymbol { col: cols - 1 });
    }
}

#[cfg(test)]
#[path = "marks_tests.rs"]
mod tests;

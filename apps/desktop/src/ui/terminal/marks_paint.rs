//! 空白与符号可视化的**绘制**：[`marks::Mark`] → `painter` 调用。
//!
//! 与 `hex_params.rs` / `bin.rs` 同一刀法：`marks.rs` 回答「这一行该画哪些标记、
//! 画在第几列」（可无头单测），本文件回答「那一格换算成哪几个像素」。拆开的
//! 理由是 `view.rs` 已经贴着 4j 的单文件上限，而且这两件事的改动频率完全不同。
//!
//! # 为什么整段画在 `view.rs` 的 span 循环**之后**
//!
//! `LineCache` 只缓存 `spans` / `bg_runs`，六个可视化开关**不在缓存里**。
//! 把标记塞进 `build_spans` 会让「每个空格一段」——80 列的行从 1–3 次 `text()`
//! 变成 80 次，合批白做了。所以这里只**追加**绘制，一格字形都没动。
//!
//! 而光标行底色是唯一的例外：它必须压在字形**之下**，所以由 `view.rs` 在
//! 逐行循环里画（判据 [`marks::caret_band_row`] 仍然在本文件之外定义）。
//!
//! # 颜色
//!
//! 一律取当前调色板的**前景色**按 alpha 叠，不写死 RGB：换主题（尤其浅底主题）
//! 时写死的灰会变成突兀的色条，而前景色按 alpha 叠在深底上是提亮、在浅底上是
//! 压暗，始终只是「同一块底色稍微亮/暗一点」。浓度见下面几个常量。

use egui::{pos2, Align2, Color32, FontId, Painter, Pos2, Rect, Shape, Stroke, Vec2};
use term_app::selection::Selections;
use term_core::grid::{Cell, LineSlice, ScreenImage};
use term_core::theme::Palette;

use super::marks::{self, Mark, MarksParams, WhitespaceMode};
use super::overlay_segments::segment_cell_range;
use crate::theme::to_color32;
use term_app::wrap::SegmentTable;

/// 空格小方点的浓度。
const DOT_ALPHA: f32 = 0.35;

/// 缩进参考线 / 折行标记的浓度（要比文字淡得多，否则满屏竖线盖住正文）。
const GUIDE_ALPHA: f32 = 0.16;

/// 列标记的浓度：比缩进线稍实一点，它是一条「别超过这里」的界线。
const COLUMN_ALPHA: f32 = 0.30;

/// 行尾 `$` 的浓度：它替代的是一个字符的位置，要比参考线更清楚。
const END_ALPHA: f32 = 0.55;

/// 折行标记的浓度。
const WRAP_ALPHA: f32 = 0.40;

/// 本帧要用的四个颜色。算一次传下去，不必每个标记都 `gamma_multiply` 一遍。
struct MarkColours {
    dot: Color32,
    guide: Color32,
    column: Color32,
    end: Color32,
    wrap: Color32,
}

impl MarkColours {
    fn new(palette: &Palette) -> Self {
        let fg = to_color32(palette.default_fg);
        Self {
            dot: fg.gamma_multiply(DOT_ALPHA),
            guide: fg.gamma_multiply(GUIDE_ALPHA),
            column: fg.gamma_multiply(COLUMN_ALPHA),
            end: fg.gamma_multiply(END_ALPHA),
            wrap: fg.gamma_multiply(WRAP_ALPHA),
        }
    }
}

/// 把本帧的所有可视化标记画上去。
///
/// `origin` 是网格左上角、`cell` 是一个格子的尺寸——与 `view::paint_rows` 收到的
/// 是同两个值，所以标记与字形永远不会错开半格。
///
/// `wraps` 是 `GridSnapshot.wraps`（自动换行续行的**绝对行号**，升序）。它**不**
/// 属于事实窗口：`ScreenImage` 里没有它，回看时它仍然是整条会话的那一份。
///
/// 六项全关时整段早退（一次循环都不进）：默认状态下的代价与这段代码不存在时相同。
#[allow(clippy::too_many_arguments)]
pub(in crate::ui) fn paint(
    painter: &Painter,
    image: &ScreenImage,
    origin: Pos2,
    cell: Vec2,
    params: &MarksParams,
    wraps: &[i64],
    selections: &Selections,
    palette: &Palette,
    font: &FontId,
    // 与正文 / margin / 查找高亮同源的一帧段表：软折后一条逻辑行占多段。
    segments: &SegmentTable,
) {
    if !params.paints_row_marks() && params.column_marker.is_none() {
        return;
    }
    let colours = MarkColours::new(palette);
    // 竖线贯穿的是**屏幕行数**（软折后一段一行），不是逻辑行数。
    let grid_bottom = origin.y + segments.rows.len() as f32 * cell.y;

    // 列标记先画：它是一条贯穿全屏的参考线，压在任何一行标记**之下**才不会
    // 在交点上盖住那些更细的符号。
    if let Some(n) = params.column_marker_col(image.cols) {
        let x = column_marker_x(origin, cell, n);
        let width = (cell.x * 0.10).clamp(1.0, 2.0);
        painter.add(Shape::line(
            vec![pos2(x, origin.y), pos2(x, grid_bottom)],
            Stroke::new(width, colours.column),
        ));
    }

    // 一行一个缓冲，24 行不产生 24 次分配。
    let mut marks: Vec<Mark> = Vec::with_capacity(8);
    // 一批选区在同一行可能给出多段区间：`InSelection` 档要的是它们的并集判据。
    let mut selected: Vec<(usize, usize)> = Vec::with_capacity(selections.items().len());

    for (i, seg) in segments.rows.iter().enumerate() {
        let Some(line) = image.lines.get(seg.line) else {
            continue;
        };
        let y = origin.y + i as f32 * cell.y;
        let abs = abs_row(image, line);
        // 段内夹取：标记只认这一段占的 cell，宽字符占位格也跟着段表走。
        let (from, to) = segment_cell_range(seg, line.cells.len());
        let (cells, col_shift) = segment_view(&line.cells, from, to);
        selected.clear();
        // 只有 `InSelection` 档要算选区区间：其余四档每帧白扫一遍选区没有意义。
        if params.whitespace == WhitespaceMode::InSelection {
            for sel in selections.items() {
                if let Some(range) = sel.columns_on(line.row) {
                    if let Some(rel) = selected_range_in_segment(range, from, to, col_shift) {
                        selected.push(rel);
                    }
                }
            }
        }
        let row = marks::RowLine {
            cells,
            continuation: marks::is_wrapped(wraps, abs),
            // 下一行是续行 → 本行被硬折了，不是逻辑行结尾。查 `wraps` 而不是
            // 查「下一行在不在视口里」：屏幕最后一行折下去的那一半在视口之外，
            // 但它的行号照样在 `wraps` 里。
            next_continuation: marks::is_wrapped(wraps, abs + 1),
            selected: &selected,
        };
        marks::push_row_marks(params, &row, &mut marks);
        for mark in &marks {
            draw_mark(painter, shift_mark(*mark, col_shift), origin, y, cell, font, &colours);
        }
    }
}

/// 选区的内容列区间 → 本段内的相对列区间；不相交或夹完为空时返回 `None`（不画）。
///
/// 段内夹取是软折语义的一部分：选区跨段时，本段只画自己那一段区间。
///
/// 关键在 [`segment_view`] 已经把段首的宽字符占位格切掉了：传给 `RowLine.cells`
/// 的切片从 `from + col_shift` 开始，标记列也是相对这个起点的。夹取原点必须同步
/// 右移 `col_shift`，否则段首一遇宽字符，选区整体左错一列（把占位格那一格算进
/// 选区）。夹完两端重合即空区间，必须丢掉。
///
/// 原点右移后可能**越过段尾**：零宽段（`from == to`）或段里只有占位半格时
/// `from + col_shift == to + 1`。这一步必须和 [`segment_view`] 一样先 `.min(to)`，
/// 并在 `start >= to` 时直接丢掉——否则 `range.1.min(to) - start` 是 `usize` 下溢
/// （debug 直接 panic，release 环绕成一个巨大区间，画出完全错位的选区）。
fn selected_range_in_segment(
    range: (usize, usize),
    from: usize,
    to: usize,
    col_shift: usize,
) -> Option<(usize, usize)> {
    let start = (from + col_shift).min(to);
    if start >= to || range.0 >= to || range.1 <= start {
        return None;
    }
    let a = range.0.max(start) - start;
    let b = range.1.min(to) - start;
    (a < b).then_some((a, b))
}

/// 段首占位格：段可能正好起于宽字符的**右半格**（软折把两格切到两段上）。
///
/// 判据只看**段外前驱**：`cells[from - 1].wide` 为真则 `from` 是占位格。占位格
/// 自己的 `wide` 是 `false`，`attrs` / `fg` / `bg` 又与字形格逐字节相同，看它
/// 自己永远判不出来（与 `debug_paint::row_marks` 同源）。跳过它，否则「中」的
/// 右半边会被画上一个空白方点。
///
/// 返回 `(本段真正可画的 cell, 它们相对段首的屏幕列偏移)`。
fn segment_view<'a>(cells: &'a [Cell], from: usize, to: usize) -> (&'a [Cell], usize) {
    let shift = usize::from(from > 0 && cells.get(from - 1).is_some_and(|c| c.wide));
    let start = (from + shift).min(to);
    (&cells[start..to], shift)
}

/// 跳掉段首占位格后，段内相对列整体右移一格才落回屏幕列。
fn shift_mark(mark: Mark, by: usize) -> Mark {
    match mark {
        Mark::SpaceDot { col } => Mark::SpaceDot { col: col + by },
        Mark::TabArrow { col } => Mark::TabArrow { col: col + by },
        Mark::IndentGuide { col } => Mark::IndentGuide { col: col + by },
        Mark::LineEnding { col } => Mark::LineEnding { col: col + by },
        Mark::WrapSymbol { col } => Mark::WrapSymbol { col: col + by },
    }
}

/// 事实窗口里的一行 → **内容绝对行号**（`wraps` / `row_times` / `blocks` 用的是这一套）。
///
/// 直接取内核给的行号（`line.row`），不做「窗口起点 + 下标」的减法：折叠把行收起来
/// 之后，窗口里的行号**不再连续**（`lines[i].row != range.start + i`），减法会指到
/// 相邻的另一行，`~` 与空白标记就画到错的行上。行连续时两者逐值相等，因此这是
/// 零行为变化的改法。
///
/// 与 `margin.rs` 里那一句同源：`image.history`（回看行数）+ 窗口内行号。
/// 两处必须一致，否则边栏的 `~` 与这里的折行标记会错开整整一屏。
fn abs_row(image: &ScreenImage, line: &LineSlice) -> i64 {
    image.history as i64 + line.row
}

/// 单个标记 → 像素。
fn draw_mark(
    painter: &Painter,
    mark: Mark,
    origin: Pos2,
    y: f32,
    cell: Vec2,
    font: &FontId,
    colours: &MarkColours,
) {
    match mark {
        Mark::SpaceDot { col } => {
            let cell_origin = cell_origin(origin, cell, col, y);
            painter.rect_filled(space_dot_rect(cell_origin, cell), 0.0, colours.dot);
        }
        Mark::TabArrow { col } => {
            let cell_origin = cell_origin(origin, cell, col, y);
            for [a, b] in tab_arrow_strokes(cell_origin, cell) {
                painter.add(Shape::line(
                    vec![a, b],
                    Stroke::new((cell.y * 0.06).max(1.0), colours.dot),
                ));
            }
        }
        Mark::IndentGuide { col } => {
            let [a, b] = indent_guide_segment(origin, y, cell, col);
            painter.add(Shape::line(
                vec![a, b],
                Stroke::new(1.0, colours.guide),
            ));
        }
        Mark::LineEnding { col } => {
            let cell_origin = cell_origin(origin, cell, col, y);
            painter.text(
                cell_origin,
                Align2::LEFT_TOP,
                "$",
                // 比正文小一号：这一格本来可能是空的，一个满字号 `$` 会盖住
                // 相邻两格之间的气口，看起来像内容而不是标记。
                // 这里刻意继承传入的**终端网格字体**（不是 UI type_scale）：
                // 标记画在终端格子里，必须跟着用户选的终端字体一起变。
                FontId::new(font.size * 0.85, font.family.clone()),
                colours.end,
            );
        }
        Mark::WrapSymbol { col } => {
            let cell_origin = cell_origin(origin, cell, col, y);
            for [a, b] in wrap_hook_strokes(cell_origin, cell) {
                painter.add(Shape::line(
                    vec![a, b],
                    Stroke::new((cell.y * 0.07).max(1.0), colours.wrap),
                ));
            }
        }
    }
}

/// 第 `col` 列的格子左上角。
fn cell_origin(origin: Pos2, cell: Vec2, col: usize, y: f32) -> Pos2 {
    pos2(origin.x + col as f32 * cell.x, y)
}

/// 空格符号：格子中央一个边长 `cell.x * 0.15` 的方点。
///
/// 用方块不用圆点：`rect_filled` 一次调用、无三角化，且在任何字号下都不会
/// 糊成半像素的圆。
fn space_dot_rect(cell_origin: Pos2, cell: Vec2) -> Rect {
    let side = (cell.x * 0.15).max(1.0);
    Rect::from_center_size(
        pos2(cell_origin.x + cell.x * 0.5, cell_origin.y + cell.y * 0.5),
        Vec2::splat(side),
    )
}

/// Tab 符号：三笔直线（横杆 + 两笔斜线）组成的箭头。
///
/// **不用字形**：`→` 在等宽等宽编程字体里常常缺字（缺了就是个豆腐块），
/// 而这一笔要表达的东西只有几何形状。与 `margin.rs` 画提示符 `›` 的做法相同。
fn tab_arrow_strokes(cell_origin: Pos2, cell: Vec2) -> [[Pos2; 2]; 3] {
    let cy = cell_origin.y + cell.y * 0.5;
    let x0 = cell_origin.x + cell.x * 0.12;
    let x1 = cell_origin.x + cell.x * 0.88;
    let head_x = cell_origin.x + cell.x * 0.60;
    let head_dy = cell.y * 0.22;
    [
        // 横杆
        [pos2(x0, cy), pos2(x1, cy)],
        // 上半笔
        [pos2(head_x, cy - head_dy), pos2(x1, cy)],
        // 下半笔
        [pos2(head_x, cy + head_dy), pos2(x1, cy)],
    ]
}

/// 缩进参考线：这一格**左边界**上的一条竖线，贯穿整行高。
fn indent_guide_segment(origin: Pos2, y: f32, cell: Vec2, col: usize) -> [Pos2; 2] {
    let x = origin.x + col as f32 * cell.x;
    [pos2(x, y), pos2(x, y + cell.y)]
}

/// 第 `n` 列（1 起）**右边界**的 x：正文写到第 `n` 个字符为止就停在它左边。
fn column_marker_x(origin: Pos2, cell: Vec2, n: usize) -> f32 {
    origin.x + n as f32 * cell.x
}

/// 折行标记：格子右下角的 L 形勾（一竖一横）。
///
/// 为什么画在**最后一列的右下角**：续行的每一格都被上一行的内容填满了，
/// 整格只有右下那一小块像素是空的。L 形勾比 `~` 更不容易和字母的下缘混在一起。
fn wrap_hook_strokes(cell_origin: Pos2, cell: Vec2) -> [[Pos2; 2]; 2] {
    let x_right = cell_origin.x + cell.x * 0.88;
    let x_left = cell_origin.x + cell.x * 0.58;
    let y_top = cell_origin.y + cell.y * 0.55;
    let y_bottom = cell_origin.y + cell.y * 0.86;
    [
        [pos2(x_right, y_top), pos2(x_right, y_bottom)],
        [pos2(x_right, y_bottom), pos2(x_left, y_bottom)],
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cell(x: f32, y: f32) -> Vec2 {
        Vec2::new(x, y)
    }

    fn test_palette() -> Palette {
        Palette {
            ansi: [term_core::grid::Rgb::new(0x33, 0x33, 0x33); 16],
            default_fg: term_core::grid::Rgb::new(0xCC, 0xCC, 0xCC),
            default_bg: term_core::grid::Rgb::new(0x1C, 0x1C, 0x1C),
            ansi_source: String::new(),
            theme_name: String::new(),
            font_family: None,
            font_weight: None,
        }
    }

    /// 空格符号是**居中的小方块**，边长 15% 格宽，且不越出格子。
    #[test]
    fn the_space_dot_is_a_small_square_centred_in_the_cell() {
        let c = cell(10.0, 20.0);
        let rect = space_dot_rect(pos2(100.0, 200.0), c);
        assert!((rect.width() - 1.5).abs() < 0.01, "15% of 10px = 1.5px");
        assert!((rect.height() - 1.5).abs() < 0.01, "方点：宽高相等");
        // 居中：四边留白一样多。
        assert!((rect.center().x - 105.0).abs() < 0.01);
        assert!((rect.center().y - 210.0).abs() < 0.01);
        // 格子窄到 1px 时也至少 1px：否则「显示空白」在小字号下会变成看不见。
        let tiny = space_dot_rect(pos2(0.0, 0.0), cell(2.0, 2.0));
        assert!(tiny.width() >= 1.0 && tiny.height() >= 1.0);
    }

    /// Tab 箭头是**三条线段**，不是字形——这就是「不依赖字体有没有 `→`」的证据。
    ///
    /// 端点全部落在格子里、横杆水平、两笔斜线交于右端同一点：这三点缺一个，
    /// 画出来就不是箭头（斜线不交会变成两道斜杠）。
    #[test]
    fn the_tab_arrow_is_three_straight_segments_inside_the_cell() {
        let c = cell(10.0, 20.0);
        let origin = pos2(0.0, 0.0);
        let strokes = tab_arrow_strokes(origin, c);
        assert_eq!(strokes.len(), 3, "横杆 + 两笔斜线");
        for [a, b] in strokes {
            for p in [a, b] {
                assert!(
                    p.x >= origin.x && p.x <= origin.x + c.x,
                    "端点越出格子：{p:?}"
                );
                assert!(p.y >= origin.y && p.y <= origin.y + c.y);
            }
        }
        // 横杆水平（两端 y 相同）。
        assert!((strokes[0][0].y - strokes[0][1].y).abs() < 0.01);
        // 两笔斜线在横杆右端交汇。
        assert!((strokes[1][1].x - strokes[0][1].x).abs() < 0.01);
        assert!((strokes[2][1].x - strokes[0][1].x).abs() < 0.01);
        // 斜线各自回到左侧，且上下对称——不对称的箭头看起来像「大于号」。
        assert!((strokes[1][0].x - strokes[2][0].x).abs() < 0.01);
        let up = strokes[0][0].y - strokes[1][0].y;
        let down = strokes[2][0].y - strokes[0][0].y;
        assert!((up - down).abs() < 0.01);
    }

    /// 缩进参考线画在列的**左边界**，高度正好一格。
    #[test]
    fn an_indent_guide_sits_on_the_left_edge_of_its_column() {
        let c = cell(10.0, 20.0);
        let [a, b] = indent_guide_segment(pos2(100.0, 50.0), 70.0, c, 4);
        assert!((a.x - 140.0).abs() < 0.01, "第 4 列的左边界 = 100 + 4*10");
        assert!((b.x - a.x).abs() < 0.01, "竖线：两端 x 相同");
        assert!((a.y - 70.0).abs() < 0.01 && (b.y - 90.0).abs() < 0.01, "一格高");
    }

    /// 列标记在第 N 列的**右边界**：正文写到第 N 个字符为止就停在它左边。
    #[test]
    fn the_column_marker_sits_on_the_right_edge_of_column_n() {
        let c = cell(8.0, 16.0);
        // 第 80 列的右边界 = 80 个字符宽。
        assert!((column_marker_x(pos2(10.0, 0.0), c, 80) - 650.0).abs() < 0.01);
        // 第 1 列的右边界就在第一个字符右边一格。
        assert!((column_marker_x(pos2(0.0, 0.0), c, 1) - 8.0).abs() < 0.01);
    }

    /// 折行标记是两笔（一竖一横），在格子右下角，不越出格子。
    #[test]
    fn the_wrap_hook_is_an_l_in_the_bottom_right_of_the_cell() {
        let c = cell(10.0, 20.0);
        let origin = pos2(0.0, 0.0);
        let strokes = wrap_hook_strokes(origin, c);
        assert_eq!(strokes.len(), 2, "一竖 + 一横");
        for [a, b] in strokes {
            for p in [a, b] {
                assert!(p.x >= origin.x && p.x <= origin.x + c.x);
                assert!(p.y >= origin.y && p.y <= origin.y + c.y);
            }
        }
        // 勾在**下半格**：上半格是字形本体。
        assert!(strokes[0][0].y > origin.y + c.y * 0.5);
        // 竖笔在最右边、横笔从它往左收：L 形的开口朝左上。
        assert!(
            strokes[1][1].x < strokes[1][0].x,
            "横笔必须往左收，否则画出来是个反向的 L"
        );
        assert!((strokes[1][0].x - strokes[0][1].x).abs() < 0.01, "两笔在右下角相接");
    }

    /// 段起于宽字符占位半格时，那一格不许挂第二套空白标记。
    /// 走生产判据 `segment_view` + `push_row_marks` + `shift_mark`，不抄复印件。
    #[test]
    fn a_segment_starting_on_a_wide_placeholder_does_not_dot_the_right_half() {
        let mut params = MarksParams::default();
        params.whitespace = WhitespaceMode::ShowAll;
        let glyph = Cell { ch: '中', wide: true, ..Cell::default() };
        // 占位格：ch 是空白、wide=false、样式与字形格完全一样（vt.rs 的真实形状）。
        let placeholder = Cell { ch: '\0', wide: false, ..Cell::default() };
        let cells = [glyph, placeholder, Cell { ch: ' ', ..Cell::default() }];
        let marks_for = |from, to| {
            let (visible, shift) = segment_view(&cells, from, to);
            let row = marks::RowLine { cells: visible, continuation: false, next_continuation: false, selected: &[] };
            let mut out = Vec::new();
            marks::push_row_marks(&params, &row, &mut out);
            out.into_iter().map(|m| shift_mark(m, shift)).collect::<Vec<Mark>>()
        };

        // 段 1..3 起于占位格：只画后面的空格，屏幕列 1（占位格那一列 0 空着）。
        assert_eq!(marks_for(1, 3), vec![Mark::SpaceDot { col: 1 }]);
        // 段里只有占位格：一格都不画。
        assert!(marks_for(1, 2).is_empty());
        // 段 0..2 从字形格开始：`push_row_marks` 自己的 prev_wide 判据跳过占位格。
        assert!(marks_for(0, 2).is_empty(), "一个汉字只挂一套标记");
    }

    /// 段内选区夹取：内容列 → 段内相对列，跨段的部分不许渗进别的段。
    #[test]
    fn selection_ranges_are_clipped_to_the_segment_before_drawing() {
        // 段 10..20（段首不是占位格，col_shift = 0）。前后两段一律不画。
        assert_eq!(selected_range_in_segment((0, 10), 10, 20, 0), None);
        assert_eq!(selected_range_in_segment((20, 30), 10, 20, 0), None);
        // 跨段：左边露出的夹掉，右边按段尾截断，再减段起点。
        assert_eq!(selected_range_in_segment((0, 15), 10, 20, 0), Some((0, 5)));
        assert_eq!(selected_range_in_segment((15, usize::MAX), 10, 20, 0), Some((5, 10)));
        // 整段命中。
        assert_eq!(selected_range_in_segment((0, usize::MAX), 10, 20, 0), Some((0, 10)));
    }

    /// 段起于宽字符占位格时，选区起点要跟着 `segment_view` 一起右移一格。
    /// 只选中占位格 / 夹完为空的区间都不许产出，否则 InSelection 会错一列。
    #[test]
    fn selection_clipping_skips_the_wide_placeholder_at_the_segment_start() {
        let (from, to, shift) = (1, 3, 1);
        // 选区只盖住占位格（内容列 1）：不画。
        assert_eq!(selected_range_in_segment((1, 2), from, to, shift), None);
        // 选区止于占位格：夹完是空区间，不画。
        assert_eq!(selected_range_in_segment((0, 2), from, to, shift), None);
        // 选区含占位格 + 后面的空格：空格在 cells 里是第 0 列，不是第 1 列。
        assert_eq!(selected_range_in_segment((1, 3), from, to, shift), Some((0, 1)));
        assert_eq!(selected_range_in_segment((0, 3), from, to, shift), Some((0, 1)));
        // 段里只有占位格：任何选区都落空。
        assert_eq!(selected_range_in_segment((0, usize::MAX), 1, 2, 1), None);
    }

    /// 零宽段夹在宽字符前驱之后：`segment_view` 的 `col_shift` 会把起点推过段尾。
    /// 选区夹取必须同样 `.min(to)` 并在 `start >= to` 时丢掉，不能 `usize` 下溢
    /// （旧写法在 debug 下这里直接 panic）。
    #[test]
    fn a_zero_width_segment_after_a_wide_cell_never_underflows() {
        // 段 [1,1) 起于「中」的占位半格（cells[0].wide），shift = 1 → start = 2 > to。
        for range in [(0, 1), (1, 2), (0, usize::MAX), (usize::MAX - 1, usize::MAX)] {
            assert_eq!(
                selected_range_in_segment(range, 1, 1, 1),
                None,
                "零宽段里任何选区都不该产出（range = {range:?}）"
            );
        }
        // 段里只有占位半格：[1,2) + shift=1 → start == to，同样是空区间。
        assert_eq!(selected_range_in_segment((0, usize::MAX), 1, 2, 1), None);
        // 没有宽前驱的零宽段：start == from == to，也不产出。
        assert_eq!(selected_range_in_segment((0, usize::MAX), 4, 4, 0), None);
    }

    /// 跨段：选区顺着内容列伸进本段，起点先跳过占位格，终点夹在本段尾，
    /// 绝不把下一段的列算进来（第四轮的「减 col_shift」口径保持不动）。
    #[test]
    fn selection_clipping_across_segments_stops_at_the_segment_tail() {
        // 段 3..8，段首是宽字符占位格 → 可画内容从内容列 4 起，共 4 格。
        let (from, to, shift) = (3, 8, 1);
        assert_eq!(selected_range_in_segment((0, 4), from, to, shift), None, "只盖到占位格");
        assert_eq!(selected_range_in_segment((0, 5), from, to, shift), Some((0, 1)));
        assert_eq!(selected_range_in_segment((7, usize::MAX), from, to, shift), Some((3, 4)));
        assert_eq!(selected_range_in_segment((8, usize::MAX), from, to, shift), None, "下一段的事");
        assert_eq!(selected_range_in_segment((0, usize::MAX), from, to, shift), Some((0, 4)));
    }

    /// 接线级：`selected_range_in_segment` 直接喂 `push_row_marks` 的 `RowLine.selected`，
    /// 宽字符段里只有真被选中的空格出方点，位置不偏。
    #[test]
    fn in_selection_marks_follow_the_clipped_range_into_the_wide_segment() {
        let mut params = MarksParams::default();
        params.whitespace = WhitespaceMode::InSelection;
        let cells = [
            Cell { ch: '中', wide: true, ..Cell::default() },
            Cell::default(), // 占位格
            Cell { ch: ' ', ..Cell::default() },
        ];
        let (visible, shift) = segment_view(&cells, 1, 3);
        let selected: Vec<(usize, usize)> = selected_range_in_segment((1, 3), 1, 3, shift)
            .into_iter()
            .collect();
        let row = marks::RowLine {
            cells: visible,
            continuation: false,
            next_continuation: false,
            selected: &selected,
        };
        let mut out = Vec::new();
        marks::push_row_marks(&params, &row, &mut out);
        let on_screen = out.into_iter().map(|m| shift_mark(m, shift)).collect::<Vec<_>>();
        assert_eq!(on_screen, vec![Mark::SpaceDot { col: 1 }], "方点落在空格那一列，不是占位格");
    }

    /// 全关时颜色表也照算不误——它只是取前景色乘 alpha，没有状态。
    /// 断言的是「浓度真的比正文淡」，否则标记会盖过正文。
    #[test]
    fn every_mark_colour_is_fainter_than_the_foreground() {
        let c = MarkColours::new(&test_palette());
        for (name, col) in [
            ("dot", c.dot),
            ("guide", c.guide),
            ("column", c.column),
            ("end", c.end),
            ("wrap", c.wrap),
        ] {
            assert!(col.a() < 255, "{name} 必须是半透明的");
        }
        assert!(c.guide.a() < c.dot.a(), "参考线要比空白符号更淡");
        assert!(c.dot.a() < c.end.a(), "行尾标记要比空白符号更实");
    }

    /// 接线级：走完整 `paint`。段从宽字符占位格起，选区只选中它后面的空格；
    /// 若 `paint` 没把 `segment_view` 的 `col_shift` 传给选区夹取，这格根本不会出点。
    #[test]
    fn paint_passes_the_wide_placeholder_shift_into_selection_clipping() {
        let ctx = egui::Context::default();
        let screen = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(400.0, 240.0));
        let mut image = ScreenImage::blank(4, 1, 0..1);
        image.lines[0] = term_core::grid::LineSlice {
            row: 0,
            cells: vec![
                Cell { ch: '中', wide: true, ..Cell::default() },
                Cell::default(), // 占位格
                Cell { ch: ' ', ..Cell::default() },
                Cell { ch: 'x', ..Cell::default() },
            ],
        };
        // 段 1..3：起点正是占位格，cells 切片里只有内容列 2 那个空格。
        let segments = SegmentTable {
            rows: vec![term_app::wrap::VisualRow {
                line: 0,
                start_col: 1,
                width: 2,
                ..term_app::wrap::VisualRow::default()
            }],
            cols: 4,
        };
        let selections = Selections::from_active(Some(term_app::selection::Selection::span(
            term_app::selection::Pos::new(0, 2),
            term_app::selection::Pos::new(0, 3),
        )));
        let mut params = MarksParams::default();
        params.whitespace = WhitespaceMode::InSelection;
        let origin = egui::pos2(20.0, 20.0);
        let size = cell(10.0, 20.0);
        let font = FontId::new(12.0, egui::FontFamily::Monospace);
        let out = ctx.run(
            egui::RawInput { screen_rect: Some(screen), ..Default::default() },
            |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    paint(ui.painter(), &image, origin, size, &params, &[], &selections,
                        &test_palette(), &font, &segments);
                });
            },
        );
        let dot_centres: Vec<f32> = out.shapes.iter().filter_map(|clipped| match &clipped.shape {
            egui::Shape::Rect(r)
                if (r.rect.width() - 1.5).abs() < 0.01 && (r.rect.height() - 1.5).abs() < 0.01 =>
            {
                Some(r.rect.center().x)
            }
            _ => None,
        }).collect();
        // 空格是段内第 1 列（占位格占第 0 列）：中心 x = 20 + 1*10 + 5。
        assert_eq!(dot_centres, vec![35.0], "只选中的空格必须出点，且不许左错一列");
    }

    /// 接线级：零宽段 + 宽字符前驱 + 覆盖全局的选区，走完整 `paint` 必须安静地
    /// 画出零个图元。旧写法在 `range.1.min(to) - start` 这一帧直接 `usize` 下溢。
    #[test]
    fn paint_with_a_zero_width_segment_and_a_global_selection_draws_nothing() {
        let ctx = egui::Context::default();
        let screen = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(400.0, 240.0));
        let mut image = ScreenImage::blank(4, 1, 0..1);
        image.lines[0] = term_core::grid::LineSlice {
            row: 0,
            cells: vec![
                Cell { ch: '中', wide: true, ..Cell::default() },
                Cell::default(), // 占位半格
                Cell { ch: ' ', ..Cell::default() },
                Cell { ch: 'x', ..Cell::default() },
            ],
        };
        // 段 [1,1) 正落在占位半格上，宽度为 0：`segment_view` 的 shift=1 把起点推到 2。
        let segments = SegmentTable {
            rows: vec![term_app::wrap::VisualRow {
                line: 0,
                start_col: 1,
                width: 0,
                ..term_app::wrap::VisualRow::default()
            }],
            cols: 4,
        };
        let selections = Selections::from_active(Some(term_app::selection::Selection::span(
            term_app::selection::Pos::new(0, 0),
            term_app::selection::Pos::new(0, 4),
        )));
        let mut params = MarksParams::default();
        params.whitespace = WhitespaceMode::InSelection;
        let font = FontId::new(12.0, egui::FontFamily::Monospace);
        let out = ctx.run(
            egui::RawInput { screen_rect: Some(screen), ..Default::default() },
            |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    paint(
                        ui.painter(), &image, egui::pos2(20.0, 20.0), cell(10.0, 20.0),
                        &params, &[], &selections, &test_palette(), &font, &segments,
                    );
                });
            },
        );
        // 只数空白方点那种 1.5px 小方块——CentralPanel 自己会铺一块背景 rect。
        let dots = out.shapes.iter().filter(|clipped| match &clipped.shape {
            egui::Shape::Rect(r) => {
                (r.rect.width() - 1.5).abs() < 0.01 && (r.rect.height() - 1.5).abs() < 0.01
            }
            _ => false,
        }).count();
        assert_eq!(dots, 0, "零宽段不该画出任何空白方点");
    }
}

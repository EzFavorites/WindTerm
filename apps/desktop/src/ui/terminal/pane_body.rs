//! 正文二选一：字符网格（`view::paint_rows`）还是 hex 转储（`bin::paint`）。
//!
//! # 为什么从 `pane.rs` 拆出来
//!
//! 从 `pane.rs` 拆出，理由与前两次同因（`pane_pointer.rs` 的行数上限、
//! `paste.rs` 的 ws-b1）——4j 的单文件 500 代码行上限。搬之前 `pane.rs` 是 492
//! 代码行，接下来三批工作（段驱动渲染 / reflow / 坐标系联动）都要往 `draw_tab`
//! 里加代码。这一段是「本帧画什么」的全部决策，切出来之后 `pane.rs` 只留一个
//! 分叉点的名字，本文件 73 代码行。
//!
//! # 为什么必须是自由函数
//!
//! `draw_tab` 手里握着借自 `self.tabs` 的 `&mut Tab`，再调 `self.xxx(...)` 就是
//! E0499（同时要 `&mut self` 与 `&mut self.tabs`）。所以这里只收**拆分好的借用**：
//! `&Tab`（调用点用 `&*tab` 重借）、`&Palette` / `&FontId`、以及一句 `&mut LineCache`。
//!
//! 那句 `&mut LineCache` 是**在调用点先算好**再传进来的，不是把 `TabSlot` 交给
//! 本文件自己取：`line_cache(&mut self.term_view, &mut self.term_view_shell, slot)`
//! 要借窗口的两个字段，让本文件自己取就等于给它 `&mut TerminalWindow`，整个拆分
//! 的目的（避开 E0499）当场作废。
//!
//! # `bin` 是参数
//!
//! 不在本文件里从 `tab.mode` 重算：`draw_tab` 是在视觉基线夹具**之前**算出这个值
//! 的，而夹具会把 `tab.mode` 换成 `fixture.mode`（静态画面）。重算就是换了一个
//! 判定时刻，搬移也就不再是「零行为变化」。

use super::bin;
use super::hex_params::HexParams;
use super::marks;
use super::pane::screen_index_of_grid_row;
use super::pane_pointer::GridGeom;
use super::view::{self, LineCache};
use crate::ui::*;
use term_app::i18n::Lang;
use term_app::wrap;

/// 正文二选一：`bin` 为真时画 hex 转储，否则画字符网格。
///
/// `geom` 复用 `pane_pointer::GridGeom`：原点 + 格子大小 + 本屏行数三样总是一起
/// 算、一起用，再新造一个几乎一样的结构体只是多一处漂移点。
#[allow(clippy::too_many_arguments)]
pub(super) fn paint_body(
    painter: &egui::Painter,
    cache: &mut LineCache,
    tab: &Tab,
    lang: Lang,
    palette: &Palette,
    font_id: &egui::FontId,
    rect: Rect,
    geom: GridGeom,
    hex: HexParams,
    alt_row_bg: bool,
    bin: bool,
) {
    let image = &tab.view;
    let start = geom.start;
    let cell = geom.cell;
    // 选区底色的浓度：调色板前景色按 alpha 叠。由本函数自己算，因为它是
    // `paint_rows` 的**唯一**输入——调用方再算一遍只能多一处可能漂移的常量。
    let selection_bg = to_color32(palette.default_fg).gamma_multiply(0.25);

    if bin {
        // hex 渲染器：只吃环、几何、调色板与字体（见 `bin.rs` 模块头）。
        bin::paint(
            painter,
            &tab.raw,
            rect,
            cell,
            hex,
            // 被窗宽夹过时表头要说出「你选的那一档是多少」，
            // 所以把标签上的原值也传进去。
            tab.hex.columns,
            // 斑马线跟着**这条会话的参数**走（`Tab.hex.zebra`，与上面四个
            // 排版数同一份值、同一个来源）。窗口上不再有第二份：那是一份
            // 「默认参数」的渲染期副本，读它等于让设置页的勾选框当场改掉
            // 所有已开标签——同一个设置页五个同类参数两套语义。
            // 分几段由 `hex.division` 决定，开关本身跟它一起从 `hex` 里取。
            hex.zebra,
            palette,
            font_id,
            lang,
        );
    } else {
        // `Text ▸ Highlight Caret Line` 的判据只算这一次：整屏行数、是否贴底、
        // 光标在第几行都在手上。命中时给 `paint_rows` 一个视口行号，它负责
        // 在**字形之下**铺那层底色（标记都画在字上，只有底色要在字下）。
        // 网格行（`0` = 屏首）先夹一次再换成屏内下标。夹的上界是**网格行数**
        // （`snapshot.rows`），不是本帧看得见的行数：折叠之后两者不再相等，
        // 按看得见的行数夹会把光标从 `rows-1` 拽到 `line_count-1`，光标行
        // 底色就画到了上面若干行去——行数越少错得越远。
        //
        // 夹取本身不能省：夹具 / resize 的中间态里 `cursor.0` 可能越界，
        // 那时应当钳到末行，而不是整条不画（与 `caret_band_row` 内部那句
        // `cursor_row.min(rows - 1)` 是同一个口径）。
        let line_count = image.lines.len();
        let caret_grid_row = tab.snapshot.cursor.0.min(tab.snapshot.rows.saturating_sub(1));
        let caret_row = screen_index_of_grid_row(image, caret_grid_row).and_then(|row| {
            marks::caret_band_row(
                tab.marks.caret_line,
                tab.viewport.is_bottom(),
                row,
                line_count,
            )
        });
        // 字形缓存按槽位分两份（`term_view` / `term_view_shell`）：
        // `LineCache` 的键是「行号 + 内容 hash」，两块区域交替写同一下标会
        // **每帧互相失效**——画得没错，但缓存等于白养。取哪一份由调用点
        // （`tab_slot::line_cache`）算好传进来，见本文件模块头。
        //
        // ws-wrap：段表在这里算，因为「一条 `LineSlice` 折成几段」是**本帧画
        // 什么**的一部分，不是像素细节——`view::paint_rows` 只负责按段落笔。
        //
        // 列宽用 `image.cols`（内核列宽），**不是** `rect` 或 PTY 尺寸算出来的
        // 那个 `cols`：后者是面板尺寸，resize 中间态里与快照不同步，两者混用会
        // 让折行宽度在拖动窗口时一帧一个值——而软换行的判据是「折点稳定」。
        //
        // 已知开销：每帧一次 `Vec<Vec<u8>>`（约 行数 × 列数 字节）+ 一份段表。
        // 本批不优化：下一批 reflow 会把「拼逻辑行 → 重折 → 缓存」一起重构，
        // 现在先做局部优化等于给马上要拆的代码加一层缓存。留在这里是为了让它
        // 是一个**写下来的**已知项，而不是一个没人知道的秘密。
        let widths: Vec<Vec<u8>> = image
            .lines
            .iter()
            .map(|line| wrap::glyph_widths(&line.cells))
            .collect();
        let segments = wrap::wrap_lines(&widths, image.cols.max(1));
        view::paint_rows(
            painter,
            cache,
            image,
            start,
            cell,
            palette,
            font_id,
            &tab.selections,
            selection_bg,
            // `Text ▸ Alternate line background color`：窗口上那份副本，每行读一次。
            alt_row_bg,
            caret_row,
            &tab.trigger_highlight_rows,
            &segments,
        );
    }
}

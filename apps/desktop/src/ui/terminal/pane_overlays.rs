//! 正文之后的**全部叠层**：查找高亮、常驻词高亮、空白与符号标记、调试覆盖层、
//! 光标。（回看药丸已于 2026-11-19 按用户比对去掉；Bin 下的字节药丸仍在
//! `bin::paint` 内。）
//!
//! # 为什么从 `pane.rs` 拆出来
//!
//! 从 `pane.rs` 拆出。理由与前两次同因（`pane_pointer.rs` 的行数上限、`paste.rs`
//! 的 ws-b1）——4j 的单文件 500 代码行上限。搬之前 `pane.rs` 是 492 代码行，接下来
//! 三批工作（段驱动渲染 / reflow / 坐标系联动）都要往 `draw_tab` 里加代码，一行都
//! 加不进去。挑这一块搬，是因为它是一段**只读的尾巴**：只往 `painter` 上追加图形，
//! 不写任何状态；本文件 135 代码行，搬完之后 `pane.rs` 是 378 代码行（两块都搬走
//! 之前是 492）。调试覆盖层那一段是后来插进来的（`Mode ▸ Debug Mode`），只多一个
//! 调用点，几何全在 `debug_paint.rs`。
//!
//! # 为什么必须是自由函数，不能是 `impl TerminalWindow` 的方法
//!
//! `draw_tab` 在 `tab_of(&mut self.tabs, &mut self.shell_tabs, slot)` 之后就一直
//! 握着 `&mut Tab`（借的是 `self.tabs`），整个函数体都在这个借用里。此时再写
//! `self.some_method(tab, ...)` 就是同时要 `&mut self` 和 `&mut self.tabs`——E0499，
//! 编译不过。所以这里收的是**拆分好的借用**：`&Tab`（调用点用 `&*tab` 重借，把那
//! 个 `&mut` 降成 `&`）、`&ViewState` / `&TerminalFonts` / `&Palette` 三份字段级
//! 不可变借用——它们与 `self.tabs` 是不同字段，编译器能看出互不相干。
//!
//! 与 `marks_paint::paint`、`pane_pointer::pointer_selection`、`bin::paint` 是同一个
//! 形状：那三个文件手上都没有 `&mut TerminalWindow`，只吃「画什么」与「画在哪」。
//!
//! # 顺序决定像素（一条都不许换）
//!
//! 查找高亮 → 常驻词高亮 → 空白/符号标记 → **调试覆盖层** → 光标。
//! 第三段夹在第二与第四之间：标记是「内容之上的注释」，要压住那两层半透明底色，
//! 又必须让光标永远在最上层（那段判据的原文跟着代码搬过来了，见函数体内）。
//!
//! 调试层插在第三与第四之间，三条理由各有具体几何依据，不是「顺着放」：
//!
//! 1. **在光标之下**：光标是实心块，6×2 的 pip 架 + 两条贯穿线画在它上面，一是
//!    光标看起来是脏的，二是 pip 铺在反转色块上没有对比度。
//! 2. **在两种高亮之下**（所以必须在 `marks_paint` 之后）：同一条理由——反过来会被
//!    那两层 `gamma_multiply` 的底色糊住。
//! 3. **在 `marks_paint` 之后，因为撞的时候调试层要让赢**：空格小方点居中于格子
//!    中央，而 `STRIKE` 是一条横贯中线的线，中心重合；折行勾 L 在右下角
//!    （`x ∈ [0.58,0.88]`、`y ∈ [0.55,0.86]`），pip 架在 `x ∈ [0.10,0.86]`、
//!    `y ∈ [0.74,0.90]`，`x` 与 `y` 都重叠。判据是两者的**性质不同**：空白/符号
//!    标记是装饰，调试标记是**诊断读数**——诊断模式开着时读数压过装饰。这也让
//!    「谁在上面」成为一个写下来的裁决，而不是碰运气。

use crate::ui::*;
use super::debug_paint;
use super::marks_paint;
use super::pane::screen_index_of;
use super::pane_pointer::GridGeom;
use super::view;
use crate::fonts::TerminalFonts;

/// 窗口级的两个叠层开关（`paint_overlays` 的最后一项参数）。
///
/// 为什么是结构体而不是两个 `bool` 参数：这两项都不在 `Tab` 上——不是「每条标签
/// 各一份」，而是整窗一份，总是同时从 `TerminalWindow` 取。绑成一组让调用点只占
/// **一行**：`ui/terminal/pane.rs` 正好贴着 500 代码行上限（一个参数一行就
/// 把它顶到 501），而这一组的语义本来就是「叠层里那些窗口级开关」。
///
/// **别往里塞第三个不相干的东西**：`bin` 看着像同类，但它是从 `tab.mode` 算出来的
/// （见下面 `paint_overlays` 的开头注释），归属不同，单独一个参数。
#[derive(Clone, Copy)]
pub(super) struct OverlayToggles {
    /// `Text ▸ Hide cursor when typing`：刚打完字，光标先别闪。
    pub(super) cursor_typing: bool,
    /// `Preferences ▸ Highlight ▸ Highlight the incremental search`：
    /// 查找命中铺底色（出厂 `true`）。
    pub(super) search: bool,
}

/// 正文画完之后的六段叠层，按像素顺序一次画完。
///
/// `bin` 由调用方传入而**不在这里**从 `tab.mode` 重算：`draw_tab` 是在视觉基线
/// 夹具之前算好这个值的，而夹具会把 `tab.mode` 换成 `fixture.mode`（静态画面）。
/// 在这里重算就会读到夹具之后的值，与搬走之前的那一份不是同一个判定。
///
/// `toggles` 里的两项同理：它们是窗口级字段（不在 `tab` 上），只能从调用方传进来。
/// `search` 的**出厂值是 `true`**，所以关掉时那整段一个字节都不画，开着时与
/// 接线前逐字节相同。
#[allow(clippy::too_many_arguments)]
pub(super) fn paint_overlays(
    painter: &egui::Painter,
    ctx: &egui::Context,
    _rect: Rect,
    tab: &Tab,
    view: &ViewState,
    fonts: &TerminalFonts,
    palette: &Palette,
    geom: GridGeom,
    bin: bool,
    // 活动标签的身份（分屏时只有它写 Find All 快照）。
    find_owner: Option<term_core::session_runtime::SessionId>,
    toggles: OverlayToggles,
    // 与正文 / margin 同源的一帧折行配置（查找高亮与光标都按段表定位）。
    wrap_cfg: term_app::wrap::WrapConfig,
) {
    // 拆回两个局部的名字：下面几段各用其中一项，`hide_cursor_typing` 还要往下
    // 传给 `view::paint_cursor`。这样函数体的读法与加这个结构体之前一字不差。
    let OverlayToggles { cursor_typing: hide_cursor_typing, search: highlight_search } = toggles;
    let image = &tab.view;
    // Find All 列表的数据源：Find 栏在 CentralPanel 之前声明，拿不到本帧窗口，
    // 所以这里每帧把「当前事实窗口 + FindState」算出的结果写进 ctx，供它下一帧
    // 读取（find_bar 会校验 query，隔帧残留不会冒充新查询的结果）。不缓存计数：
    // 结果仍是 `app::find::matches` 的纯函数值，滚动后下一帧自然换掉。
    super::find_results_slot::publish(ctx, find_owner, find::result_rows(&view.find, image));
    let image_cols = image.cols.max(1);
    let start_x = geom.start.x;
    let start_y = geom.start.y;
    // 与正文 / margin 同一套段表口径（纯函数，同输入同输出）。
    let segment_table = term_app::wrap::segments_for_wrap(image, wrap_cfg);
    let cell_w = geom.cell.x;
    let cell_h = geom.cell.y;

    // F2：高亮跟着事实窗口走——回看时不隐藏，因为搜的就是看得见的那一屏。
    // 行号是内容绝对行号，减掉窗口起点才得到屏上第几行。
    // Bin 下整块跳过：查找命中的是「字符列」，而屏幕上画的是字节列，
    // 两者没有换算关系；硬画只会把高亮糊在不相干的字节上。
    //
    // `highlight_search` 是 2026-11-19 补的**关掉它的开关**（设置页
    // `Highlight the incremental search`）：那之前这段无条件在画，设置页却挂着
    // 一个灰空框说「没有渲染器」——反着的谎。出厂开着，所以默认路径与以前
    // 逐字节相同（十一张整窗基线零变化那张就是这条的证据）。
    if highlight_search && !bin && view.find.visible && !view.find.query.is_empty() {
        let found = find::matches(&view.find, image);
        let current = current_index(&view.find, found.len());
        for (idx, m) in found.iter().enumerate() {
            let Some(line_idx) = screen_index_of(image, m.row) else {
                continue;
            };
            if line_idx >= image.lines.len() {
                continue;
            }
            let is_current = Some(idx) == current;
            let hl_color = if is_current {
                to_color32(palette.default_fg) // 当前匹配：前景色做背景
            } else {
                to_color32(Rgb::new(0xFF, 0xD7, 0x00)) // 其他匹配：金黄色
            };
            let col_start = m.col_start.min(image_cols);
            let col_end = m.col_end.min(image_cols);
            // 一次匹配可能横跨多个软折段——每段夹到 [start_col, start_col+width)
            // 各画一块，y 用段序号（与正文同口径）。关软折时只有一段，等价旧行为。
            for seg_idx in segment_table.row_to_segments(line_idx) {
                let seg = &segment_table.rows[seg_idx];
                let from = col_start.max(seg.start_col);
                let to = col_end.min(seg.start_col + seg.width);
                if to <= from {
                    continue;
                }
                let match_y = start_y + (seg_idx as f32) * cell_h;
                let x = start_x + ((from - seg.start_col) as f32) * cell_w;
                let w = ((to - from) as f32) * cell_w;
                painter.rect_filled(
                    Rect::from_min_size(egui::pos2(x, match_y), Vec2::new(w, cell_h)),
                    0.0,
                    hl_color.gamma_multiply(0.3), // 半透明高亮
                );
            }
        }
    }

    // 常驻高亮。词表只在 `ViewState.center.highlights` 里，匹配走 term-app 的
    // 缓存入口（对事实窗口，字符列坐标与查找同一套），不自己扫字节——
    // 中文词的列偏移不会算错（F2 / F3）；同一个词跨帧不再每帧重编译正则。
    if !bin && !view.center.highlights.is_empty() {
        let words = view.center.highlights.clone();
        let hl = to_color32(Rgb::new(0x20, 0x90, 0x90)).gamma_multiply(0.35);
        for word in words {
            for m in term_app::find::find_matches_literal_cached(image, &word, false, false) {
                let Some(line_idx) = screen_index_of(image, m.row) else {
                    continue;
                };
                if line_idx >= image.lines.len() {
                    continue;
                }
                let from = m.col_start.min(image_cols);
                let to = m.col_end.min(image_cols);
                if to <= from {
                    continue;
                }
                for seg_idx in segment_table.row_to_segments(line_idx) {
                    let seg = &segment_table.rows[seg_idx];
                    let seg_from = from.max(seg.start_col);
                    let seg_to = to.min(seg.start_col + seg.width);
                    if seg_to <= seg_from {
                        continue;
                    }
                    let y = start_y + seg_idx as f32 * cell_h;
                    let x = start_x + ((seg_from - seg.start_col) as f32) * cell_w;
                    let rect = Rect::from_min_size(
                        egui::pos2(x, y),
                        Vec2::new((seg_to - seg_from) as f32 * cell_w, cell_h),
                    );
                    painter.rect_filled(rect, 0.0, hl);
                }
            }
        }
    }

    // 空白与符号可视化（`View ▸ Toggle Line Endings / Column Marker /
    // Indent Guides / Wrap Symbols`、`View ▸ White Spaces` 五档）：
    // 行尾 `$`、参考列竖线、缩进参考线、折行勾、空格点与 Tab 箭头。
    //
    // 画在这里（正文、查找高亮、常驻高亮**之后**，回看药丸与光标之前）：
    //
    // - 在正文之后 —— 它们是**标记**，不是内容；`LineCache` 也不缓存它们
    //   （六个开关不进缓存，见 `marks.rs` 模块头）；
    // - 在两种高亮之后 —— 反过来会被那两层半透明底色糊住，而「这一格是空格」
    //   正是用户打开这一档想确认的事；
    // - 在光标之前 —— 光标永远是最上层。
    //
    // Bin 下整块跳过：hex 视图没有字符网格，空白/行尾/缩进这些概念在那儿
    // 一个都不成立（一行是 16 个字节，不是若干字符列）。
    //
    // ws-wrap：这一段搬到本文件之后**夹在第二与第四段之间**这件事尤其要紧——
    // 文件是按语义切的，不是按「顺手挨着谁」，重排一次就是一次像素回归。
    if !bin {
        marks_paint::paint(
            painter,
            image,
            egui::pos2(start_x, start_y),
            Vec2::new(cell_w, cell_h),
            &tab.marks,
            &tab.snapshot.wraps,
            &tab.selections,
            palette,
            &fonts.font_id(),
            &segment_table,
        );
    }

    // `Mode ▸ Debug Mode`：把每格的属性八位 / 色号种类 / 宽字符标成像素。
    //
    // 位置理由见模块头的「顺序决定像素」四条——一句话：在两种高亮与空白标记
    // **之后**（否则被半透明底色糊住）、在药丸与光标**之前**（chrome 与光标永远
    // 在最上层）。压在 `marks_paint` 之上是有意的：空格小方点与 `STRIKE` 中线、
    // 折行勾右下与 pip 架右端会撞，撞的时候诊断读数压过装饰。
    //
    // Bin 下整块跳过：hex 视图没有字符网格，`Tab` 的字符格坐标系和字节环坐标系
    // 之间没有任何映射（与查找高亮整块跳过是同一条理由）。所以 `debug=true` +
    // `mode=Bin` 时屏幕上不会有变化——这是定的，切回 Text 就看见。
    if !bin && tab.debug {
        debug_paint::paint(
            painter,
            image,
            egui::pos2(start_x, start_y),
            Vec2::new(cell_w, cell_h),
            palette,
            &segment_table,
        );
    }

    // 2026-11-19：回看药丸已去掉。兼容终端在 Sysmon 细带上只放一枚折叠标签，
    // 没有「回看 N 行 · 滚回底部继续跟随输出」这颗提示（见用户比对图）；
    // 回看状态由滚动条滑块位置与 N 状态留存自身表达，不再单独画一层 chrome。
    // Bin 下字节数的那颗药丸仍由 `bin::paint` 自己画，与本段无关。

    // 光标（含 `Text ▸ Hide cursor when typing` 的判定）——画法在
    // `view::paint_cursor`：这里只是在正确的时刻、用本帧的坐标调它一次。
    // Bin 下不画：屏幕上没有字符网格，光标该落在哪一格没有定义。
    if !bin {
        view::paint_cursor(
            painter,
            ctx,
            tab,
            egui::pos2(start_x, start_y),
            Vec2::new(cell_w, cell_h),
            to_color32(palette.default_fg),
            hide_cursor_typing,
            wrap_cfg,
        );
    }
}

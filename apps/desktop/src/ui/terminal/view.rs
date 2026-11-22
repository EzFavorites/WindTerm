//! 逐行 span 合批与行级缓存。
//!
//! 改造前：80×24 网格每帧最多 1,920 次 `painter.text()` + 1,920 次背景绘制。
//! 改造后：一行里**连续、同样式、非空白**的 cell 合成一个 `Span`，一次
//! `painter.text()` 画完；连续相同背景的 cell 合成一个 `BgRun`，一次
//! `rect_filled()` 画完。典型行从 80 次调用降到 1–3 次。
//!
//! 行内容哈希作为脏区判据：未变化的行直接复用缓存的 spans / bg_runs，
//! 连字符串构建都省掉。宽字符单独成段，避免 CJK 与后续 ASCII 混排导致
//! 字距与逐 cell 绘制不一致。

use term_core::grid::{Attrs, Cell, Colour, ScreenImage};
use term_core::theme::Palette;

use egui::{pos2, Color32, FontId, Painter, Rect, Vec2};
use term_app::selection::Selections;

use crate::theme::to_color32;

/// 一段连续、同样式、非空白的字符。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Span {
    /// 段首列（0-based），绘制时换算成 x。
    pub col: usize,
    pub text: String,
    pub fg: Colour,
    pub bg: Colour,
    pub attrs: Attrs,
}

/// 一段连续的非默认背景。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BgRun {
    pub col: usize,
    pub end_col: usize,
    pub bg: Colour,
}

fn attrs_hash(a: Attrs) -> u64 {
    const FLAGS: [u8; 8] = [
        Attrs::BOLD, Attrs::DIM, Attrs::ITALIC, Attrs::UNDERLINE,
        Attrs::BLINK, Attrs::INVERSE, Attrs::HIDDEN, Attrs::STRIKE,
    ];
    let mut v = 0u64;
    for (i, flag) in FLAGS.iter().enumerate() {
        if a.contains(*flag) {
            v |= 1 << i;
        }
    }
    v
}

fn is_blank(c: &Cell) -> bool {
    c.ch == ' ' || c.ch == '\0'
}

/// 把一行合并成文字段。宽字符占位 cell 被跳过。
pub fn build_spans(row: &[Cell]) -> Vec<Span> {
    let mut out = Vec::new();
    let mut cur: Option<Span> = None;
    let mut skip_placeholder = false;

    for (col, cell) in row.iter().enumerate() {
        if skip_placeholder {
            skip_placeholder = false;
            continue;
        }
        if is_blank(cell) {
            if let Some(s) = cur.take() {
                out.push(s);
            }
            continue;
        }
        // 宽字符单独成段：CJK 字形的实际宽度由字体决定，不与 ASCII 合排。
        if cell.wide {
            if let Some(s) = cur.take() {
                out.push(s);
            }
            out.push(Span {
                col,
                text: cell.ch.to_string(),
                fg: cell.fg,
                bg: cell.bg,
                attrs: cell.attrs,
            });
            skip_placeholder = true;
            continue;
        }
        let compatible = cur
            .as_ref()
            .map(|s| s.fg == cell.fg && s.bg == cell.bg && s.attrs == cell.attrs)
            .unwrap_or(false);
        if compatible {
            cur.as_mut().unwrap().text.push(cell.ch);
        } else {
            if let Some(s) = cur.take() {
                out.push(s);
            }
            cur = Some(Span {
                col,
                text: cell.ch.to_string(),
                fg: cell.fg,
                bg: cell.bg,
                attrs: cell.attrs,
            });
        }
    }
    if let Some(s) = cur.take() {
        out.push(s);
    }
    out
}

/// 把一行里连续的非默认背景合并成矩形段。
pub fn build_bg_runs(row: &[Cell]) -> Vec<BgRun> {
    let mut out: Vec<BgRun> = Vec::new();
    for (col, cell) in row.iter().enumerate() {
        if cell.bg == Colour::Default {
            continue;
        }
        if let Some(last) = out.last_mut() {
            if last.end_col == col && last.bg == cell.bg {
                last.end_col = col + 1;
                continue;
            }
        }
        out.push(BgRun {
            col,
            end_col: col + 1,
            bg: cell.bg,
        });
    }
    out
}

fn hash_row(cells: &[Cell]) -> u64 {
    // FNV-1a，足够做「这一行变没变」的判据，不用于安全场景。
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    let mut mix = |v: u64| {
        h ^= v;
        h = h.wrapping_mul(0x100_0000_01b3);
    };
    for cell in cells {
        mix(cell.ch as u64);
        mix(attrs_hash(cell.attrs));
        mix(match cell.fg {
            Colour::Default => 0,
            Colour::Indexed(i) => 1 + i as u64,
            Colour::Rgb(r, g, b) => 0x1000 | ((r as u64) << 16) | ((g as u64) << 8) | b as u64,
        });
        mix(match cell.bg {
            Colour::Default => 0,
            Colour::Indexed(i) => 1 + i as u64,
            Colour::Rgb(r, g, b) => 0x1000 | ((r as u64) << 16) | ((g as u64) << 8) | b as u64,
        });
        mix(cell.wide as u64);
    }
    h
}

#[derive(Clone, Debug)]
pub struct CachedLine {
    pub key: u64,
    pub spans: Vec<Span>,
    pub bg_runs: Vec<BgRun>,
}

/// 每**段**的缓存。`line()` 命中时零分配。
///
/// 下标语义是「本帧第几段」而不是「第几行」：把绘制改成段驱动之后，
/// 一条 `LineSlice` 可以折成多段，键必须是段（见 `paint_rows` 里的调用处）。
/// 今天行与段一一对应，所以两份语义给出同一份缓存。
#[derive(Default)]
pub struct LineCache {
    lines: Vec<Option<CachedLine>>,
}

impl LineCache {
    pub fn line(&mut self, row: usize, cells: &[Cell]) -> &CachedLine {
        let key = hash_row(cells);
        if self.lines.len() <= row {
            self.lines.resize_with(row + 1, || None);
        }
        let slot = &mut self.lines[row];
        if slot.as_ref().map(|e| e.key) != Some(key) {
            *slot = Some(CachedLine {
                key,
                spans: build_spans(cells),
                bg_runs: build_bg_runs(cells),
            });
        }
        slot.as_ref().unwrap()
    }

    pub fn clear(&mut self) {
        for slot in &mut self.lines {
            *slot = None;
        }
    }
}

/// 隔行底色叠的是**前景色**、浓度约 4%。
///
/// 为什么不是固定 RGB：换主题（尤其是浅底主题）时一块写死的深灰会变成突兀的
/// 色条；取当前调色板的前景色按 alpha 叠，浅底上自动变成淡灰、深底上是极淡的
/// 白，始终只是「同一块底色稍微亮/暗一点」。
const ALT_ROW_ALPHA: f32 = 0.04;

/// `Text ▸ Highlight Caret Line` 那层底色的浓度。
///
/// 比隔行底色（4%）稍浓一点，但仍然是**极淡**：它铺在整行文字底下，浓一点就
/// 会把 `ls` 的彩色输出洗掉一块。7% 是在深底与浅底两种主题上都能看见、又都
/// 不抢字的最低一档。
const CARET_LINE_ALPHA: f32 = 0.07;

/// 这一行要不要叠隔行底色。`abs_row` 是**内容绝对行号**（不是视口内第几行）。
///
/// 抽成纯函数是为了让「口径」能被单测钉住：奇数行着色、第 0 行不着色，且
/// 负数行（回看到历史起点之前）也不会翻符号。绘制处只负责把绝对行号算对。
pub fn shades_row(abs_row: i64) -> bool {
    abs_row % 2 != 0
}

/// 隔行底色本身：把当前前景色降到 [`ALT_ROW_ALPHA`] 的透明度。
///
/// # 为什么手算 premultiplied，而不是 `from_rgba_unmultiplied`
///
/// 那个构造函数在 egui 0.29 里按**线性空间**乘 alpha 再转回 sRGB，得到的字节
/// 是 `(10/255)^(1/2.2) × 255 ≈ 56`，而 epaint 的混合是 `src + dst × (1 − a)`
/// （premultiplied 语义），于是 4% 的意图画出来是 22% 的白——实测底色 28 → 81，
/// 条纹比正文还显眼。这里自己乘：4% 就是 4%。
///
/// # 为什么要用前景色而不是固定 RGB
///
/// 前置乘法对黑与白都成立：深底主题（前景近白）叠上去是「提亮 4%」，浅底主题
/// （前景近黑）叠上去是「压暗 4%」。写死一个灰，在另一种主题上就是一块突兀的
/// 色条——而主题是运行时可换的。
/// `Color32` 的调色板前景色本身不透明，所以 `to_array()` 给的就是直通道。
pub fn alternate_row_tint(fg: Color32) -> Color32 {
    faint_tint(fg, ALT_ROW_ALPHA)
}

/// [`Text ▸ Highlight Caret Line`](CARET_LINE_ALPHA) 的行底色。
///
/// 与隔行底色同一套前置乘法（理由见 [`alternate_row_tint`]），只是浓度不同：
/// 两种「铺一整行」的淡底必须走同一个算法，否则它们在同一屏上会一个偏亮一个
/// 偏暗，看着像两种材质。
pub fn caret_line_tint(fg: Color32) -> Color32 {
    faint_tint(fg, CARET_LINE_ALPHA)
}

/// 把前景色按 `alpha` 前置乘法压成一个半透明色（手算，理由见上）。
fn faint_tint(fg: Color32, alpha: f32) -> Color32 {
    let [r, g, b, _] = fg.to_array();
    let alpha = (alpha * 255.0).round() as u8;
    let premultiply = |c: u8| (u16::from(c) * u16::from(alpha) / 255) as u8;
    Color32::from_rgba_premultiplied(
        premultiply(r),
        premultiply(g),
        premultiply(b),
        alpha,
    )
}

/// 把事实窗口逐行画出来：cell 背景合批 → 选区底色 → 字形。
///
/// 2026-11-07 从 `pane.rs` 搬出来——那边加了 margin 边栏后贴着 450 行
/// 上限。**只搬不改**：绘制顺序、坐标、颜色一个字节都没动，五张整窗基线是判据。
///
/// 选区从「一份」变成「一批」。**活动那份满档、其余淡一档**——只改模型
/// 不改画法的话，「Activate Next Selection」点了就是没有效果。浓度口径由
/// `select::tint` 给，这里只负责按行把每一份的列区间画出来。
///
/// 三种东西共用同一条 `origin + col*cell_w` 换算，所以选区、查找高亮、正文
/// 永远不会互相错开半格。
///
/// `alt_row_bg` 是 `Text ▸ Alternate line background color` 的开关（`Prefs`
/// 的同名字段经窗口副本传进来）；它只多画一层极淡的行底色，不改变任何坐标。
///
/// `caret_row` 是 `Text ▸ Highlight Caret Line` 命中的**视口行号**
/// （`Some(i)` = 给第 i 行铺底色），判据在 `marks::caret_band_row`。它必须画在
/// 这里而不是 `marks_paint.rs`：这层底色要压在字形**之下**，而那个文件的所有
/// 标记都追加在 span 循环之后（压在字上）。两种东西的叠放次序不同，所以落点不同。
///
/// # 按**段**画，不按行画
///
/// `segments` 是调用方折好的段表（`pane_body::paint_body` 用
/// `wrap::glyph_widths` + `wrap::wrap_lines` 算出来）。一条 `LineSlice` 现在可以
/// 折成 n 段、每段占一行屏幕，所以循环变量是**段序号**而不是行下标，`y` 也按段
/// 序号算。
///
/// 今天（软换行还没接线）每条 `LineSlice` 只折出 1 段：内核本来就在**同一个
/// 列宽**上硬折过行，所以 `start_col == 0`、`width == cells.len()`、
/// `continued == false`，段序号恒等于行下标，本函数画出来的像素与改造前逐字节
/// 相同。这个身份性由 `every_grid_row_wraps_into_exactly_one_segment` 钉住——
/// 它不是注释里的一句好话，是「零像素变化」这个验收判据的可证伪前提。
///
/// 只留一条路径，不做 `if wrap { … } else { … }`：两条路径意味着两份画法要各自
/// 维护，而它们本该是同一份。
///
/// `segments` 为空是退化情形（`image.cols == 0`），那时一行都不画——不 panic、
/// 不越界。
#[allow(clippy::too_many_arguments)]
pub fn paint_rows(
    painter: &Painter,
    cache: &mut LineCache,
    image: &ScreenImage,
    origin: egui::Pos2,
    cell: Vec2,
    palette: &Palette,
    font: &FontId,
    selections: &Selections,
    selection_bg: Color32,
    alt_row_bg: bool,
    caret_row: Option<usize>,
    highlight_rows: &[i64],
    segments: &[term_app::wrap::VisualRow],
) {
    let active = selections.active_index();
    for (i, seg) in segments.iter().enumerate() {
        // `seg.line` 是 `image.lines` 的下标。取不到就跳过：段表与图像本该是同
        // 一次投影的产物，但夹具 / resize 的中间态里没有这个保证，宁可少画一行
        // 也不许在这里 panic。
        let Some(line) = image.lines.get(seg.line) else {
            continue;
        };
        // 切段：`start_col` 是**显示列**，而 `cells` 恰好按显示列一格一项，所以
        // 直接按下标切，中间不需要任何换算表。右界要夹一次——`width` 是排版宽度
        // （含段末那格宽字符留出的空白），可能比 cell 个数大 1。左界跟着右界夹，
        // 这样两界永远有序，切片不会 panic。
        let len = line.cells.len();
        let to = (seg.start_col + seg.width).min(len);
        let from = seg.start_col.min(to);
        let cells = &line.cells[from..to];

        // y 按**段序号**算：一行折成 n 段就占 n 行屏幕。今天段序号 == 行下标。
        let y = origin.y + (i as f32) * cell.y;
        // 缓存键从此是**段**（段序号 + 段内容 hash），不是行。下一批 reflow 之后
        // 同一行会折出多段，键必须能区分它们：否则第 2 段起会命中第 1 段的
        // spans / bg_runs，把它们画到错误的 y 上（看上去只是「某几行重影」，
        // 极难归因）。今天段序号 == 行下标、切片 == 整行，键与 hash 与改造前一致。
        let cached = cache.line(i, cells);

        // 隔行底色画在 cell 背景**之前**：程序自己设的 ANSI 背景必须压住它，
        // 否则 `ls` 的彩色块会被这层淡色洗掉一块。
        //
        // 行号用**绝对行**（`line.row`，贴底时就是 `range.start + 视口行号`），
        // 不是视口内第几行：回看时窗口整体平移，按视口号算会让底色在滚动中
        // 一帧一个样（整屏闪）。
        //
        // 也不能用循环下标 `i` 代替：折叠后行数不连续，`i` 是**屏幕位置**而不是
        // 内容位置，底色会按屏幕奇偶而不是内容奇偶取，收起一行之后整屏都会错位。
        //
        // 取奇数行（1/3/5…）：第 0 行是每屏最上面那一行，让它保持原底色——
        // 顶端第一行就变色看着像「整个终端换了背景」，从第二行起才像行间分隔。
        //
        // 宽度仍是**整行**（`image.cols`）而不是本段宽度：隔行底色是「这一行内容」
        // 的属性，不是「这一段」的属性；窄于网格时右侧会缺一块。
        if alt_row_bg && shades_row(line.row) {
            let w = (image.cols as f32) * cell.x;
            painter.rect_filled(
                Rect::from_min_size(pos2(origin.x, y), Vec2::new(w, cell.y)),
                0.0,
                alternate_row_tint(to_color32(palette.default_fg)),
            );
        }

        // 触发器 Highlight：绝对行命中时铺一层淡蓝底。同样画在 ANSI 背景之前，
        // 程序自己的彩色块仍然压得住它；它不是选区，不进复制文本。
        if highlight_rows.contains(&line.row) {
            let w = (image.cols as f32) * cell.x;
            painter.rect_filled(
                Rect::from_min_size(pos2(origin.x, y), Vec2::new(w, cell.y)),
                0.0,
                Color32::from_rgba_unmultiplied(0x2E, 0x7C, 0xD2, 48),
            );
        }

        // `span.col` / `run.col` 都是**段内**列，所以 x 从 `origin.x` 起算
        // （段起点就是 `origin.x`，不是 `origin.x + start_col * cell.x`）。
        for run in &cached.bg_runs {
            let bg = to_color32(term_core::theme::resolve(palette, run.bg, false));
            let x = origin.x + (run.col as f32) * cell.x;
            let w = ((run.end_col - run.col) as f32) * cell.x;
            painter.rect_filled(
                Rect::from_min_size(pos2(x, y), Vec2::new(w, cell.y)),
                0.0,
                bg,
            );
        }

        // `Text ▸ Highlight Caret Line`：光标行铺一层极淡底。
        //
        // 位置在 cell 背景**之后**、选区与字形**之前**：
        // - 在 cell 背景之后 —— 否则程序设的 ANSI 底色会把整行盖掉，用户看不到
        //   这层底色（`ls` 的彩色行正好是看代码时最需要「我在第几行」的地方）；
        // - 在字形之前 —— 底色永远不许盖住文字，这是「底色」与「标记」的分界；
        // - 在选区之前 —— 选中一段时选区那档更亮，选择本身要压过光标行提示。
        //
        // 判据用**段序号** `i`（`caret_row` 是视口行号）。今天两者恒等；等 reflow
        // 接线、一行折出多段之后，这里要换成「本段所在的逻辑行 == 光标行」，
        // 那是下一批的事，本批不动。
        if caret_row == Some(i) {
            let w = (image.cols as f32) * cell.x;
            painter.rect_filled(
                Rect::from_min_size(pos2(origin.x, y), Vec2::new(w, cell.y)),
                0.0,
                caret_line_tint(to_color32(palette.default_fg)),
            );
        }

        // 选区底色画在文字之前，避免盖住字形。
        // 一批选区都要画，活动那份不透明档。
        //
        // `columns_on` 给的是**整行**的绝对列，这里折成**段内**列：先夹进本段的
        // `[start_col, start_col + width)`，再减掉段起点。今天（`start_col == 0`、
        // `width == len`）折出来的数与改造前逐字节相同；但不折的话，等 reflow
        // 接线之后每一段都会把整行的高亮重画一遍——一条逻辑行折 3 段就会在 3 行
        // 屏幕上都亮起同一段列，看着像选区「糊」到了相邻行。
        for (idx, sel) in selections.items().iter().enumerate() {
            let Some((sel_from, sel_to)) = sel.columns_on(line.row) else {
                continue;
            };
            // 先夹到网格宽度（`sel_to` 可能是 `usize::MAX`），再夹到本段。
            let sel_from = sel_from.min(image.cols).max(seg.start_col);
            let sel_to = sel_to.min(image.cols).min(seg.start_col + seg.width);
            if sel_to > sel_from {
                let col = sel_from - seg.start_col;
                let x = origin.x + (col as f32) * cell.x;
                let w = ((sel_to - sel_from) as f32) * cell.x;
                painter.rect_filled(
                    Rect::from_min_size(pos2(x, y), Vec2::new(w, cell.y)),
                    0.0,
                    super::select::tint(Some(idx) == active, selection_bg),
                );
            }
        }

        for span in &cached.spans {
            let fg = to_color32(term_core::theme::resolve(palette, span.fg, true));
            let x = origin.x + (span.col as f32) * cell.x;
            painter.text(
                pos2(x, y),
                egui::Align2::LEFT_TOP,
                span.text.clone(),
                font.clone(),
                fg,
            );
        }
    }
}

/// 画终端光标。回看（视口不在底部）或内核说不可见时整段跳过。
///
/// 从 `pane.rs` 搬过来：那边加了「打字隐光标」之后顶到 500 行上限。
/// **只搬不改**——形状映射、闪烁相位、坐标换算一个字节都没动；`draw_caret`
/// 仍是 `terminal.rs` 里那一份（设置页 Caret 的预览也用同一个函数）。
///
/// `hide_while_typing` = `Text ▸ Hide cursor when typing` 开着；真的藏不藏由
/// `tab.typing_pause_left()` 决定（最近 500ms 内有过输入就不画）。光标藏起来
/// 只影响这一笔绘制，不改变网格尺寸、滚动位置或选区。
pub fn paint_cursor(
    painter: &Painter,
    ctx: &egui::Context,
    tab: &crate::ui::Tab,
    origin: egui::Pos2,
    cell: Vec2,
    fg: Color32,
    hide_while_typing: bool,
) {
    let typing_pause = if hide_while_typing {
        tab.typing_pause_left()
    } else {
        None
    };
    if let Some(left) = typing_pause {
        // 500ms 之后光标要自己回来：egui 空闲时不重画，不在这里排一次重绘
        // 它就会一直不出现，直到下一次有事件——正是「光标丢了」的那种观感。
        ctx.request_repaint_after(left);
    }
    if !tab.viewport.is_bottom() || !tab.snapshot.cursor_visible || typing_pause.is_some() {
        return;
    }
    // 2026-11-19 只读查看（`Window ▸ Local Mode`）：不画光标。
    //
    // 光标说的是「下一个字符会插在这里」——只读态下没有任何字符会插进去，
    // 画一个闪烁的方块等于承诺一个做不到的动作。判据放在这里而不是复用
    // `hide_cursor_typing`：那是「刚打完字，先别闪」的临时静默，两者语义不同，
    // 混用会让设置页那个开关的解释变成两件事。
    if tab.local_mode {
        return;
    }

    let snap = &tab.snapshot;
    let cr = snap.cursor.0.min(snap.rows.saturating_sub(1));
    let cc = snap.cursor.1.min(snap.cols.saturating_sub(1));
    // `cursor.0` 是**网格行**（0 = 屏首），不是屏内下标：折叠后两者不再相等。
    // 查不到 = 光标那一行被折叠收起，此时**不画光标**是正确行为而非退化——那一行
    // 用户看不见，硬按 `cr` 画会把它浮在别的行上，等于指着另一行说「光标在这」。
    let Some(screen_row) = super::pane::screen_index_of_grid_row(&tab.view, cr) else {
        return;
    };
    let cell_rect = Rect::from_min_size(
        pos2(
            origin.x + (cc as f32) * cell.x,
            origin.y + (screen_row as f32) * cell.y,
        ),
        cell,
    );

    // 会话偏好 `caret_style`（0-7）现在真正生效——映射到
    // (形状, 是否闪烁)。此前这里硬编码成空心描边，八种样式名存实亡。
    let (shape, blink) = term_core::config::caret_style_to_shape_blink(tab.config.caret_style);
    // 闪烁：用墙钟相位切方波；开启时持续请求重绘，否则光标会卡在某一帧。
    // 2026-11-19：周期改由 `SessionConfig.caret_period` 决定（此前硬编码 1 Hz）。
    // `caret_lit_at` 是纯函数，`period=0.5` 时与旧的 `time*2 % 1 < 0.5` 等价。
    let lit = if blink {
        ctx.request_repaint();
        term_core::config::caret_lit_at(ctx.input(|i| i.time), tab.config.caret_period)
    } else {
        true
    };
    if lit {
        super::draw_caret(
            painter,
            cell_rect,
            shape,
            fg,
            cell.x,
            cell.y,
            term_core::config::clamp_caret_width(tab.config.caret_width),
        );
    }
}

#[cfg(test)]
mod tests {    use super::*;

    fn row(text: &str) -> Vec<Cell> {
        let mut v: Vec<Cell> = text.chars().map(|ch| Cell { ch, ..Cell::default() }).collect();
        v.resize(80, Cell::default());
        v
    }

    /// 「段驱动」零像素变化的身份性前提。
    ///
    /// `paint_rows` 现在按段画，`y` 与缓存键都用**段序号**。它之所以与改造前
    /// 逐字节相同，靠的是一条外部事实：内核本来就在**同一个列宽**上硬折过行，
    /// 所以每条 `LineSlice` 的显示宽度 ≤ `image.cols`，按 `image.cols` 折出来
    /// 恰好 1 段、且覆盖整行。
    ///
    /// 这条测试就是那个前提的可证伪形式：哪天内核改成不在同宽折行（或投影给了
    /// 比网格更宽的行），它先红——而不是等到人眼在某张截图上看出「行距变了」
    /// 再去反推坐标系。
    #[test]
    fn every_grid_row_wraps_into_exactly_one_segment() {
        let cols = 12usize;
        // 三行覆盖三种真实形状：全空白（内核对每一行都补齐到 cols；这些空格是
        // 真实的格子，不许当尾随空白裁掉）、满格 ASCII、宽字符。
        let blank = vec![Cell::default(); cols];
        let mut full = blank.clone();
        // 恰好 12 个字符 = 满格。
        for (i, ch) in "hello world!".chars().enumerate() {
            full[i].ch = ch;
        }
        // 宽字符占**两**格（字形格 + 占位格）：字形数与显示列数不相等，
        // 「逐 cell 一项」的宽度表在这一行会多算一列。
        let mut cjk = blank.clone();
        cjk[0] = Cell { ch: '中', wide: true, ..Cell::default() };
        cjk[2] = Cell { ch: 'a', ..Cell::default() };
        let lines: Vec<term_core::grid::LineSlice> = [blank, full, cjk]
            .into_iter()
            .enumerate()
            .map(|(i, cells)| term_core::grid::LineSlice { row: i as i64, cells })
            .collect();
        let image = ScreenImage {
            cols,
            version: 0,
            range: 0..lines.len() as i64,
            lines,
            cursor: None,
            history: 0,
        };

        // 与 `pane_body::paint_body` 完全相同的一条数据流。
        let widths: Vec<Vec<u8>> = image
            .lines
            .iter()
            .map(|l| term_app::wrap::glyph_widths(&l.cells))
            .collect();
        let segments = term_app::wrap::wrap_lines(&widths, image.cols);

        assert_eq!(
            segments.len(),
            image.lines.len(),
            "每条 LineSlice 必须恰好折出 1 段，否则 y 坐标与缓存键不再与改造前等价"
        );
        for (seg, line) in segments.iter().zip(&image.lines) {
            let want = term_app::wrap::VisualRow {
                line: seg.line,
                start_col: 0,
                width: line.cells.len(),
                continued: false,
            };
            assert_eq!(*seg, want, "第 {} 行折出了不止一段或不是整行", line.row);
        }
    }

    #[test]
    fn blank_row_has_no_spans() {
        assert!(build_spans(&row("")).is_empty());
    }

    #[test]
    fn plain_text_merges_into_one_span() {
        let spans = build_spans(&row("hello"));
        assert_eq!(spans.len(), 1);
        assert_eq!(spans[0].text, "hello");
        assert_eq!(spans[0].col, 0);
    }

    #[test]
    fn a_space_breaks_the_span() {
        let spans = build_spans(&row("hello world"));
        assert_eq!(spans.len(), 2);
        assert_eq!(spans[0].text, "hello");
        assert_eq!(spans[1].col, 6);
        assert_eq!(spans[1].text, "world");
    }

    #[test]
    fn a_colour_change_breaks_the_span() {
        let mut cells = row("abcd");
        cells[2].fg = Colour::Indexed(1);
        let spans = build_spans(&cells);
        // 默认色 "ab" / 变色 "c" / 恢复默认 "d" → 3 段
        assert_eq!(spans.len(), 3);
        assert_eq!(spans[0].text, "ab");
        assert_eq!(spans[1].text, "c");
        assert_eq!(spans[2].text, "d");
    }

    #[test]
    fn wide_glyph_is_its_own_span_and_skips_placeholder() {
        let mut cells = row("中 x");
        cells[0].wide = true;
        // cells[1] 是宽字符占位（空格），cells[2] = 'x'
        let spans = build_spans(&cells);
        assert_eq!(spans.len(), 2, "{spans:?}");
        assert_eq!(spans[0].text, "中");
        assert_eq!(spans[1].text, "x");
        assert_eq!(spans[1].col, 2);
    }

    #[test]
    fn bg_runs_merge_consecutive_cells() {
        let mut cells = row("abcd");
        cells[1].bg = Colour::Indexed(2);
        cells[2].bg = Colour::Indexed(2);
        let runs = build_bg_runs(&cells);
        assert_eq!(runs.len(), 1, "连续同色应合成一个矩形");
        assert_eq!(runs[0], BgRun { col: 1, end_col: 3, bg: Colour::Indexed(2) });
    }

    #[test]
    fn bg_runs_split_on_colour_change() {
        let mut cells = row("ab");
        cells[0].bg = Colour::Indexed(1);
        cells[1].bg = Colour::Indexed(2);
        assert_eq!(build_bg_runs(&cells).len(), 2);
    }

    #[test]
    fn cache_hit_does_not_rebuild() {
        let mut cache = LineCache::default();
        let cells = row("hello world");
        let first = cache.line(0, &cells).clone();
        let second = cache.line(0, &cells).clone();
        assert_eq!(first.key, second.key);
        assert_eq!(first.spans, second.spans);
    }

    #[test]
    fn cache_invalidates_when_the_row_changes() {
        let mut cache = LineCache::default();
        let a = cache.line(0, &row("hello")).clone();
        let b = cache.line(0, &row("world")).clone();
        assert_ne!(a.key, b.key);
        assert_eq!(b.spans[0].text, "world");
    }

    #[test]
    fn typical_grid_needs_far_fewer_draw_calls_than_cells() {
        // 24 行 × 80 列，每行 "hello world " 重复：1920 个 cell。
        let mut cache = LineCache::default();
        let mut calls = 0usize;
        for r in 0..24 {
            let cells = row(&"hello world ".repeat(6));
            let line = cache.line(r, &cells);
            calls += line.spans.len() + line.bg_runs.len();
        }
        // 12 个单词/行 × 24 行 = 288 次文字调用，背景 0 次。
        assert_eq!(calls, 288);
        assert!(calls < 1920 / 4, "合批后调用数应显著低于逐 cell 绘制");
    }

    #[test]
    fn alternate_row_background_shades_odd_absolute_rows_only() {
        // 第 0 行保持原底色（每屏最上面那一行），从第 1 行起隔行着色。
        assert!(!shades_row(0), "顶端第一行不该变色");
        assert!(shades_row(1));
        assert!(!shades_row(2));
        assert!(shades_row(3));
        // 回看到历史起点之前时行号可能是负的：仍然只按奇偶分，不会因为符号翻掉
        // （`-1 % 2 == -1`，所以这里断言的是「非零即着色」，不是「等于 1」）。
        assert!(shades_row(-1));
        assert!(!shades_row(-2));
        // 相邻两行一个着色一个不着色——这正是「隔行」的定义，写死成常量就会红。
        for r in -4..4 {
            assert_ne!(shades_row(r), shades_row(r + 1), "第 {r} 行与下一行同色");
        }
    }

    #[test]
    fn the_alternate_row_tint_is_a_faint_alpha_of_the_foreground() {
        // 断言看的是 **premultiplied 的原始字节**：epaint 的混合是
        // `src.rgb + dst.rgb × (1 − a)`，所以真正加到画面上的是 rgb 这几个字节
        // （4% ≈ 10/255），不是 `to_srgba_unmultiplied` 换算回来的值——那个函数
        // 走的是线性空间，与绘制时的 sRGB 混合不是同一套口径。
        let tint = alternate_row_tint(Color32::WHITE);
        assert_eq!(tint.a(), 10, "4% 的 alpha ≈ 10/255");
        assert_eq!(
            (tint.r(), tint.g(), tint.b()),
            (10, 10, 10),
            "白前景叠出来的是「提亮 4%」"
        );
        // 浅底主题的前景是近黑：同一个 alpha、方向相反（压暗 4%）。这就是为什么
        // 用前景色而不是写死一个灰——写死的灰在另一种主题上是「一块色条」。
        let dark = alternate_row_tint(Color32::BLACK);
        assert_eq!(dark.a(), tint.a(), "浓度与主题无关");
        assert_eq!((dark.r(), dark.g(), dark.b()), (0, 0, 0), "纯黑只压 alpha");
        // premultiplied 的合法条件：每个通道都不超过 alpha。
        for c in [tint, dark] {
            assert!(c.r() <= c.a() && c.g() <= c.a() && c.b() <= c.a());
        }
    }
}

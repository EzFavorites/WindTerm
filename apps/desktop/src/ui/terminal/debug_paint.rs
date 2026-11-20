//! 调试覆盖层的**绘制**：`Cell` → 属性位 / 色号种类 / 宽字符标记 → `painter` 调用。
//!
//! 读者是 `Mode ▸ Debug Mode` 这一条命令（`Window.ToggleDebugMode`）：打开之后，
//! SGR 1/2/3/4/5/7/8/9 与 `SGR 38`/`48` 色号、以及「这一格是宽字符字形」这三件事，
//! 从「屏幕上完全不可见」变成**可读的几何标记**。关掉逐像素回到原样。
//!
//! # 为什么单独一个文件，不塞进 `view.rs`
//!
//! `view.rs` 现在是 476 代码行（`scripts/verify.sh` 的 4j 口径：非空且非注释，
//! 上限 500）。只剩 24 行余量，放不下九个颜色常量 + 六个区的几何 + 七条单测。
//! `marks_paint.rs`（269 代码行）是同一个先例：`LineCache` 只缓存 `spans` /
//! `bg_runs`，所有标记都追加在 span 循环之后，所以它们天然是「另一个文件的事」。
//!
//! # 为什么整段画在 `view.rs` 的 span 循环之后
//!
//! 同 `marks_paint.rs` 的理由：塞进 `build_spans` 会把一格拆成多段，80 列的行从
//! 1–3 次 `text()` 变成几十次，合批白做。这里只**追加**方块与横线，一格
//! 字形都没动，一个字的内容也没改。
//!
//! # 分区（八位同时置位也不许互相覆盖）
//!
//! 一格被切成六个**两两不相交**（闭集意义上只在边界相切）的区。**位置**是唯一的
//! 排布手段——不靠调色、不靠加粗区分。
//!
//! ```text
//!          x = 0     0.10                 0.86 0.90      1.00
//!   y = 0.00 ┌────────────────────────────────────────────┐
//!            │ A  fg 色号：顶栏（0 / 1 / 3 个方块）        │
//!   y = 0.08 ├──┬─────────────────────────────────────┬───┤
//!            │B │                                     │ D │
//!            │  │                                     │   │
//!   y = 0.47 │  ├──────── E  STRIKE  ─────────────────┤   │
//!   y = 0.53 │  │                                     │   │
//!            │  │                                     │   │
//!   y = 0.74 │  ├── C  pip 架（3 列 × 2 行）──────────┤   │
//!   y = 0.90 │  │                                     │   │
//!   y = 0.92 ├──┴─────────────────────────────────────┴───┤
//!   y = 0.93 │     F  UNDERLINE                        │
//!   y = 0.99 └────────────────────────────────────────────┘
//!
//!   B = 左栏：bg 色号      D = 右栏：Cell.wide
//! ```
//!
//! | 区 | x 范围 | y 范围 | 归谁 |
//! |---|---|---|---|
//! | **A** 顶栏 | `[0.00, 1.00]` | `[0.00, 0.08]` | `fg` 色号 |
//! | **B** 左栏 | `[0.00, 0.10]` | `[0.08, 0.92]` | `bg` 色号 |
//! | **C** pip 架 | `[0.10, 0.86]` | `[0.74, 0.90]` | 六个属性位（3 列 × 2 行） |
//! | **D** 右栏 | `[0.88, 1.00]`（实画 `x0 = 0.90`、宽 `max(0.08, 1px)`） | `[0.10, 0.90]` | `Cell.wide` |
//! | **E** 中贯线 | `[0.10, 0.86]` | 中心 `0.50`，厚 `max(6%, 1px)` | `STRIKE` |
//! | **F** 底贯线 | `[0.10, 0.86]` | 顶 `0.93`，厚同上 | `UNDERLINE` |
//!
//! 选这四条边的理由：等宽字体里顶边与左边是**唯一两侧都不带墨**的地方（字形有
//! 左边距、行距在上下），而底边被 `UNDERLINE` 占了、右边被 `wide` 占了。四件
//! 事一件一条边，零冲突。
//!
//! # 解码顺序
//!
//! **位置是唯一解码通道。** pip 架 3 列 × 2 行，行优先：
//! `(0,0)=BOLD (0,1)=DIM (0,2)=ITALIC (1,0)=BLINK (1,1)=INVERSE (1,2)=HIDDEN`。
//! `UNDERLINE` / `STRIKE` 不占槽，走两条贯穿线（底缘 / 中线）。
//! 顺序取自 `term_core::grid::Attrs` 的位序（bit 0–7），去掉两条有天然几何的位。
//!
//! alpha 只是「同一位置上的浓度」，**不承载信息**：`DIM` 画得最淡（0.22）、
//! `BOLD` 最实（0.55）、`INVERSE` 最高一档（0.60），但「是哪一位」永远只看槽位。
//! 色号也一样靠**方块个数**分：`Default` 不画、`Indexed` 一个、`Rgb` 三个。
//!
//! # 已知的诚实限制（默认字号下读不清，别当 bug 修）
//!
//! 默认度量 `cell = 7.8 × 16.0`（`apps/desktop/src/fonts.rs` 的出厂值）下，pip 宽
//! 1.38px、列间距 1.98px，**相邻两枚 pip 之间的净空只有 0.59px**。所以六位全开
//! 时 pip 架看起来是一根有点的短横条。这是可接受的：「全开」本身是一个有意义的
//! 状态；要逐位读就放大字号到 150%（`cell_w ≈ 11.7`）或上 2× 屏。
//!
//! 同样在默认字号下，B 栏（`bg` 色号）只有 `0.10 * 7.8 = 0.78px` 宽，而方块有
//! 1px 下限——**1px 放不进 0.78px 的栏**。本文件的做法是把每个标记**夹进它自己
//! 那个区**（[`clamp_to_zone`]），于是 `bg` 方块在默认字号下是 0.78 × 1.0 的窄条。
//! 不夹的话它会向右吃掉 0.11px 的 E 区，八位全开时与 `STRIKE` 中线真的相交——
//! 那会当场破掉本文件的核心不变量。**不要**为此去加 DPI 吸附或 device-pixel 对齐：
//! `marks_paint.rs` 全篇一个都不用，在这里单独引入一套会让标记与字形在不同 ppp 下
//! 相对位置漂移，比「字小时读不清」难查得多。
//!
//! # 尺寸不足时降级
//!
//! [`too_small`] 为真（`cell.x < 6.0 || cell.y < 10.0`）时**六个槽分不到 6px**，
//! 「是哪一位」在物理上读不出来。此时只画两样：(a) 一格被修饰过就在左上角画**一个**
//! 小方块；(b) 两条贯穿线照旧（它们不依赖分槽，1px 下也读得出来）。pip 架、颜色栏、
//! `wide` 竖线都不画。降级不是丢信息，是**选择说能说准的那部分**——看到 6px 字体下
//! 满屏糊在一起时请回来看这一段，那不是 bug。
//!
//! # 两类覆盖不到的东西（写下来，不假装有）
//!
//! **(a) `tab.debug → paint` 这一跳没有任何机器门禁。** 从菜单 ID 到本文件之间有
//! 十几跳，除最后一跳外每一跳漏了都会编译红或测试红（穷举 match、集合相等断言、
//! 结构体字面量缺字段）。唯独 `pane_overlays.rs` 里那句 `if !bin && tab.debug`
//! 不属于任何一类：把它连同 `use super::debug_paint;` 一起删掉，`cargo test` 与
//! `cargo clippy --all-targets -D warnings` **双双全绿**（`main.rs` 有
//! `#![allow(dead_code)]`，于是连「`paint` 没有调用者」都不会报）。补测试要造一个
//! egui 上下文去数像素，本仓没有这种范式（`marks_paint.rs` 也只测纯几何，不测
//! 「谁调了它」），硬造出来的东西会比它守的东西更脆。所以这一跳**只能靠人眼验收**：
//! 打开 `Mode ▸ Debug Mode` 贴一行 `printf '\e[1;4mX\e[0m'`，屏幕上没有新标记就是
//! 这一跳断了。这是本仓的一贯口径——说清缺什么，而不是造一个看起来很绿的假门禁。
//!
//! **(b) `SLOTS` 的八位是手写列举的。** `term_core::grid::Attrs` 的八位是 `u8`
//! 常量，没有可枚举的形态，所以本文件只能照着抄一遍。若 `grid.rs` 将来加第九位，
//! 这里不会红、屏幕上是「那一位置了但一个标记都没有」。真正的护栏应当落在
//! `term-core`（断言 `Attrs` 的位数），本仓目前没有。
//!
//! # 本文件**不**做的事
//!
//! 不让属性生效（不加粗 / 不倾斜 / 不隐藏 / 不闪烁），只把它们标出来。真渲染
//! SGR 1/2/3/8/5 是终端渲染功能的另一个批次。也**不**画具体色号数字（`Indexed(208)`
//! 的 `208`、`Rgb` 的三元组）：默认字号下一格 7.8×16 px，七段数字塞进格子里必然
//! 压在字形上，读出来的东西比不画更糟。要读具体值请回看程序日志或 `SGR` 原文。
//!

use egui::{pos2, Color32, Painter, Pos2, Rect, Vec2};
use term_core::grid::{Attrs, Cell, Colour, ScreenImage};
use term_core::theme::Palette;

use crate::theme::to_color32;

/// 贯穿线线宽：`cell.y` 的 6%，至少 1px。
///
/// 与 `marks_paint.rs` 里 Tab 箭头 `(cell.y * 0.06).max(1.0)` 同一口径。
const RULE_W: f32 = 0.06;

/// 颜色栏 / 顶栏上那些小方块的边长：`cell.x` 与 `cell.y` 中**较小者**的 10%，至少 1px。
const SQUARE: f32 = 0.10;

/// pip 架 3 列 × 2 行（槽号 `0..6`，行优先），六个槽刚好放六个「无天然几何」的属性位。
/// 行数没有单独一个常量：行距固定 `0.08`（架高 `0.16` 的一半），第二行由
/// `slot / PIP_COLS` 得到，多一个常量只是多一处能忘的地方。
const PIP_COLS: usize = 3;

/// `BOLD` pip 的浓度：满高最实，最强的位给最强的浓度。
const BOLD_ALPHA: f32 = 0.55;
/// `DIM` pip 的浓度：「暗」的位画得最淡——浓度本身是第二通道。
const DIM_ALPHA: f32 = 0.22;
/// `ITALIC` pip 的浓度。
const ITALIC_ALPHA: f32 = 0.45;
/// `BLINK` pip 的浓度。
const BLINK_ALPHA: f32 = 0.45;
/// `INVERSE` pip 的浓度：反转影响整格，给最高一档。
const INVERSE_ALPHA: f32 = 0.60;
/// `HIDDEN` pip 的浓度：介于 `DIM` 与常规之间。
const HIDDEN_ALPHA: f32 = 0.30;
/// 两条贯穿线的浓度：覆盖面积大，必须比 pip 低一档才不会抢字。
const RULE_ALPHA: f32 = 0.70;
/// `fg` / `bg` 色号方块的浓度。
const KIND_ALPHA: f32 = 0.60;
/// `wide` 竖线的浓度：满高一条，必须比色号方块与两条贯穿线都淡，否则每一行宽字
/// 之间都是竖栅栏。
const WIDE_ALPHA: f32 = 0.30;

/// 六个 pip 槽 ↔ `Attrs` 的位。
///
/// **顺序就是 `Attrs` 的位序**（bit 0–7）去掉两条走贯穿线的位。解码器只有这一份，
/// 画与测都读它，所以「槽 3 是 BLINK 还是 INVERSE」不可能在两处说法不同。
///
/// **这八个常量是手写列举的，不是从 `term_core::grid::Attrs` 推出来的**（那八个
/// 位是 `u8` 常量，没有 `&[u8]` 之类的可枚举形态）。所以「`grid.rs` 加了第九位、
/// 这里忘了加槽」**没有**任何自动护栏：本文件只会照旧画六枚 pip，测试也照旧全绿。
/// 真正的护栏应当落在 `term-core`（断言 `Attrs` 的位数 == 8），本仓目前没有——
/// 这里把它写成实话，不假装有。
const SLOTS: [(u8, usize); 6] = [
    (Attrs::BOLD, 0),
    (Attrs::DIM, 1),
    (Attrs::ITALIC, 2),
    (Attrs::BLINK, 3),
    (Attrs::INVERSE, 4),
    (Attrs::HIDDEN, 5),
];

/// 这一格要不要画任何标记。
///
/// 性能前提，不是可选优化：80×24 = 1920 格，若每格画 6 pip + 2 线 + 颜色栏，
/// 最坏 ≈ 10 次 `painter` 调用/格 = 19200 次/帧。绝大多数格子是 `Cell::default()`，
/// 走这一步就出去，于是代价正比于**被修饰的格子数**而不是格子总数：一屏纯文本
/// （`ls` 的普通输出）≈ 0 次调用。
pub(super) fn decorate(cell: &Cell) -> bool {
    !cell.attrs.is_empty()
        || cell.fg != Colour::Default
        || cell.bg != Colour::Default
        || cell.wide
}

/// 字号小到「六个槽彼此粘连到读不出哪一位」。
///
/// 这是一个**可读性**阈值，不是碰撞阈值：物理上 overlap 要到 `cell.x < 4.0`
/// （pip 宽被 `max(1.0)` 顶住，间距 `0.2533*cell.x` 小于 1px 才真的叠）。
/// 取 6.0 是因为 4.0–6.0 之间虽然不重叠，但相邻 pip 的间隙 < 0.5px，画出来是
/// 一根连笔的虚线，「哪一位亮了」已经读不准了。
pub(super) fn too_small(cell: Vec2) -> bool {
    cell.x < 6.0 || cell.y < 10.0
}

/// 本帧要用的九个颜色。算一次传下去，不必每个标记都 `gamma_multiply` 一遍。
///
/// 一律取调色板**前景色**按 alpha 叠，不写死 RGB：换主题（尤其浅底主题）时写死的
/// 灰会变成突兀的色条。`gamma_multiply` 四通道一律乘 factor（含 alpha），与
/// `view.rs::faint_tint` 逐字节等价，所以这里不另写一份淡色函数；**不要**用
/// `from_rgba_unmultiplied`（`view.rs` 记过那次实测：4% 画成 22%）。
struct DebugColours {
    bold: Color32,
    dim: Color32,
    italic: Color32,
    blink: Color32,
    inverse: Color32,
    hidden: Color32,
    rule: Color32,
    kind: Color32,
    wide: Color32,
}

impl DebugColours {
    fn new(palette: &Palette) -> Self {
        let fg = to_color32(palette.default_fg);
        Self {
            bold: fg.gamma_multiply(BOLD_ALPHA),
            dim: fg.gamma_multiply(DIM_ALPHA),
            italic: fg.gamma_multiply(ITALIC_ALPHA),
            blink: fg.gamma_multiply(BLINK_ALPHA),
            inverse: fg.gamma_multiply(INVERSE_ALPHA),
            hidden: fg.gamma_multiply(HIDDEN_ALPHA),
            rule: fg.gamma_multiply(RULE_ALPHA),
            kind: fg.gamma_multiply(KIND_ALPHA),
            wide: fg.gamma_multiply(WIDE_ALPHA),
        }
    }

    /// 一个属性位对应的 pip 颜色。`UNDERLINE` / `STRIKE` 不占槽（走 [`Self::rule`]），
    /// 所以末尾那条 `_` 兜底只在该位被误加进 `SLOTS` 时才会命中——它会安静地画成
    /// 贯穿线的颜色，而不是 panic。`u8` 的取值空间没法穷举，这条 arm 省不掉。
    fn pip(&self, bit: u8) -> Color32 {
        match bit {
            Attrs::BOLD => self.bold,
            Attrs::DIM => self.dim,
            Attrs::ITALIC => self.italic,
            Attrs::BLINK => self.blink,
            Attrs::INVERSE => self.inverse,
            Attrs::HIDDEN => self.hidden,
            _ => self.rule,
        }
    }
}

/// 把本帧的所有调试标记画上去。
///
/// `origin` 是网格左上角、`cell` 是一个格子的尺寸——与 `view::paint_rows` 收到的是
/// 同两个值，所以标记与字形永远不会错开半格。
///
/// 空镜像（`lines` 为空）一次循环都不进：零绘制、零 panic。行不满 `cols` 时只遍历
/// `line.cells`，缺口右侧**不补画**——没有格子就没有属性，补画等于编造。
pub(in crate::ui) fn paint(
    painter: &Painter,
    image: &ScreenImage,
    origin: Pos2,
    cell: Vec2,
    palette: &Palette,
) {
    if image.lines.is_empty() {
        return;
    }
    let colours = DebugColours::new(palette);
    let degrade = too_small(cell);
    let cols = image.cols;
    for (i, line) in image.lines.iter().enumerate() {
        let y = origin.y + i as f32 * cell.y;
        let groups = row_marks(&line.cells, cols, pos2(origin.x, y), cell, &colours, degrade);
        for (_, marks) in groups {
            for (rect, colour) in marks {
                painter.rect_filled(rect, 0.0, colour);
            }
        }
    }
}

/// 一行 → 每格一组的标记：`(列号, [(矩形, 颜色)])`。
///
/// **宽字符的占位格判定就在这里**，判据只看**前驱**：`cells[i-1].wide == true` 则
/// `cells[i]` 是占位格。不许看它自己的 `wide`——占位格的 `wide` 是 `false`
/// （`vt.rs:563-573` 建它时写的就是 `wide: false`），而它的 `attrs` / `fg` / `bg`
/// 与字形格**完全一样**。用自己的 `wide` 判断会让一个汉字挂两套标记。
///
/// 抽成纯函数（零 `painter`）不只是为了好看：早先这段循环在单测里被**抄了一遍**，
/// 于是把 `is_placeholder` 改成恒 `false`（占位格照画）也只有测试里的复印件会变，
/// 生产路径改成什么样都是绿的。现在 `paint` 与单测走同一条判据，抄本没有了。
fn row_marks(
    cells: &[Cell],
    cols: usize,
    origin: Pos2,
    cell: Vec2,
    colours: &DebugColours,
    degrade: bool,
) -> Vec<(usize, Vec<(Rect, Color32)>)> {
    let mut out: Vec<(usize, Vec<(Rect, Color32)>)> = Vec::new();
    let mut skip_placeholder = false;
    for (col, cell_data) in cells.iter().enumerate() {
        let is_placeholder = skip_placeholder;
        skip_placeholder = cell_data.wide;
        if is_placeholder || col >= cols || !decorate(cell_data) {
            continue;
        }
        let cell_origin = pos2(origin.x + col as f32 * cell.x, origin.y);
        out.push((col, cell_marks(cell_data, cell_origin, cell, colours, degrade)));
    }
    out
}

/// 单个格子 → 它的全部标记（几何 + 颜色）。零 `painter`，可无头单测。
fn cell_marks(
    cell_data: &Cell,
    origin: Pos2,
    cell: Vec2,
    colours: &DebugColours,
    degrade: bool,
) -> Vec<(Rect, Color32)> {
    let mut marks: Vec<(Rect, Color32)> = Vec::with_capacity(9);
    if degrade {
        // 降级：「这一格被修饰过」+ 两条贯穿线。pip 架 / 颜色栏 / wide 竖线在这个
        // 字号下分不开，画了反而读错，所以整块不画。
        marks.push((degrade_dot_rect(origin, cell), colours.kind));
    } else {
        for (bit, slot) in SLOTS {
            if cell_data.attrs.contains(bit) {
                marks.push((pip_rect(slot, origin, cell), colours.pip(bit)));
            }
        }
        for rect in kind_squares(cell_data.fg, true, origin, cell) {
            marks.push((rect, colours.kind));
        }
        for rect in kind_squares(cell_data.bg, false, origin, cell) {
            marks.push((rect, colours.kind));
        }
        if cell_data.wide {
            marks.push((wide_rail_rect(origin, cell), colours.wide));
        }
    }
    // 两条贯穿线两种字号下都画：它们不占槽，1px 反而更清楚。
    if cell_data.attrs.contains(Attrs::STRIKE) {
        marks.push((strike_rect(origin, cell), colours.rule));
    }
    if cell_data.attrs.contains(Attrs::UNDERLINE) {
        marks.push((underline_rect(origin, cell), colours.rule));
    }
    marks
}

/// 一个格子自己在屏幕上的范围（**格**，不是区）。
fn cell_rect(origin: Pos2, cell: Vec2) -> Rect {
    Rect::from_min_size(origin, cell)
}

/// 一个**区**在屏幕上占的矩形：比例 → 像素。
fn zone_rect(origin: Pos2, cell: Vec2, x: (f32, f32), y: (f32, f32)) -> Rect {
    Rect::from_min_max(
        pos2(origin.x + x.0 * cell.x, origin.y + y.0 * cell.y),
        pos2(origin.x + x.1 * cell.x, origin.y + y.1 * cell.y),
    )
}

/// 把一个矩形**夹进它自己那个区**。
///
/// 这不是美化，是「六个区两两不交」的唯一保证：`1px` 下限比 B 栏（`0.10 * 7.8 =
/// 0.78px`）宽，不夹的话 `bg` 方块会向右压进 E 区，八位全开时与 `STRIKE` 中线真的
/// 相交。夹的只是**像素**，解码通道（方块个数 / 槽位顺序）一位都没动：默认字号下
/// 夹之前宽 1.0px、夹之后 0.78px，看的是一个左栏小点。
fn clamp_to_zone(rect: Rect, zone: Rect) -> Rect {
    let x0 = rect.min.x.max(zone.min.x).min(zone.max.x);
    let x1 = rect.max.x.min(zone.max.x).max(x0);
    let y0 = rect.min.y.max(zone.min.y).min(zone.max.y);
    let y1 = rect.max.y.min(zone.max.y).max(y0);
    Rect::from_min_max(pos2(x0, y0), pos2(x1, y1))
}

/// 顶栏（A 区）：`fg` 色号。
fn zone_a(origin: Pos2, cell: Vec2) -> Rect {
    zone_rect(origin, cell, (0.0, 1.0), (0.0, 0.08))
}

/// 左栏（B 区）：`bg` 色号。
fn zone_b(origin: Pos2, cell: Vec2) -> Rect {
    zone_rect(origin, cell, (0.0, 0.10), (0.08, 0.92))
}

/// pip 架（C 区）：六个属性位。
fn zone_c(origin: Pos2, cell: Vec2) -> Rect {
    zone_rect(origin, cell, (0.10, 0.86), (0.74, 0.90))
}

/// 两条贯穿线共用的横向区间（E / F 区的 x）。
fn zone_rule_x(origin: Pos2, cell: Vec2) -> (f32, f32) {
    (
        origin.x + 0.10 * cell.x,
        origin.x + 0.86 * cell.x,
    )
}

/// 第 `slot` 个 pip（0–5，行优先）的矩形。
///
/// 槽宽 `0.76/3`、槽高 `0.08`；pip 占槽内 70%，四周各留 15% 的槽内边距，于是
/// 「哪位亮了」由位置回答，pip 之间还留着缝。宽高各有 1px 下限（`max(1.0)`）。
fn pip_rect(slot: usize, origin: Pos2, cell: Vec2) -> Rect {
    let col = slot % PIP_COLS;
    let row = slot / PIP_COLS;
    let slot_w = (0.86 - 0.10) / PIP_COLS as f32;
    let x0 = origin.x + (0.10 + (col as f32 + 0.15) * slot_w) * cell.x;
    let y0 = origin.y + (0.74 + (row as f32 + 0.15) * 0.08) * cell.y;
    let w = (slot_w * 0.70 * cell.x).max(1.0);
    let h = (0.08 * 0.70 * cell.y).max(1.0);
    clamp_to_zone(
        Rect::from_min_size(pos2(x0, y0), Vec2::new(w, h)),
        zone_c(origin, cell),
    )
}

/// `STRIKE`：字身中线一条横贯线，`x` 与 pip 架同宽（`[0.10, 0.86]`）。
fn strike_rect(origin: Pos2, cell: Vec2) -> Rect {
    let (x0, x1) = zone_rule_x(origin, cell);
    let h = (cell.y * RULE_W).max(1.0);
    let y0 = origin.y + 0.50 * cell.y - h * 0.5;
    clamp_to_zone(
        Rect::from_min_max(pos2(x0, y0), pos2(x1, y0 + h)),
        cell_rect(origin, cell),
    )
}

/// `UNDERLINE`：格子底缘一条横贯线。
///
/// 与小字号下的 `1px` 下限同样会顶出底边，所以夹进格子——夹的是像素，不是语义。
fn underline_rect(origin: Pos2, cell: Vec2) -> Rect {
    let (x0, x1) = zone_rule_x(origin, cell);
    let h = (cell.y * RULE_W).max(1.0);
    let y0 = origin.y + 0.93 * cell.y;
    clamp_to_zone(
        Rect::from_min_max(pos2(x0, y0), pos2(x1, y0 + h)),
        cell_rect(origin, cell),
    )
}

/// `wide` 右栏（D 区）：一条满高竖线，只画在**字形格**上。
///
/// 语义是「这条右边界落在字形内部」= 这一个字还没写完。它**不夹进格子**：右缘允许
/// 越出格子 ≤0.3px（默认字号下 8.02 vs 7.8），这是全文件唯一允许的越界——这点出血
/// 落在终端区的 `GRID_PAD = 4.0px` 余量里，`painter` 的 clip rect 又正好是终端区，
/// 所以永远不会越出终端。
fn wide_rail_rect(origin: Pos2, cell: Vec2) -> Rect {
    let x0 = origin.x + 0.90 * cell.x;
    let w = (0.08 * cell.x).max(1.0);
    let y0 = origin.y + 0.10 * cell.y;
    let y1 = origin.y + 0.90 * cell.y;
    Rect::from_min_max(pos2(x0, y0), pos2(x0 + w, y1))
}

/// 降级状态下那一个「这一格被修饰过」的小方块：格子左上角。
fn degrade_dot_rect(origin: Pos2, cell: Vec2) -> Rect {
    let side = (cell.x * 0.12).max(1.0);
    clamp_to_zone(
        Rect::from_min_size(pos2(origin.x + 0.05 * cell.x, origin.y + 0.05 * cell.y), Vec2::splat(side)),
        cell_rect(origin, cell),
    )
}

/// 色号标记：`Default` → 0 个；`Indexed` → 1 个；`Rgb` → 3 个。
///
/// 三种情况靠**方块个数**一眼分开，`fg` 与 `bg` 靠**边**分开（顶栏 / 左栏）。
/// `is_fg` 为真走顶栏（方块水平排、居中于 `y = 0.04`）；为假走左栏（竖直排、
/// 居中于 `x = 0.05`）。
pub(super) fn kind_squares(kind: Colour, is_fg: bool, origin: Pos2, cell: Vec2) -> Vec<Rect> {
    let side = (cell.x.min(cell.y) * SQUARE).max(1.0);
    // 三种情况靠方块**个数**一眼分开：0 / 1 / 3。位置表是两套，不互相换算——
    // 顶栏水平排在 0.30 / 0.50 / 0.70，左栏竖直排在 0.20 / 0.50 / 0.80。
    let along: &[f32] = match (kind, is_fg) {
        (Colour::Default, _) => &[],
        (Colour::Indexed(_), _) => &[0.50],
        (Colour::Rgb(..), true) => &[0.30, 0.50, 0.70],
        (Colour::Rgb(..), false) => &[0.20, 0.50, 0.80],
    };
    along
        .iter()
        .map(|at| one_square(origin, cell, is_fg, *at, side))
        .collect()
}

/// 一个色号方块：`is_fg` 在顶栏水平排，否则在左栏竖直排。
fn one_square(origin: Pos2, cell: Vec2, is_fg: bool, along: f32, side: f32) -> Rect {
    let (centre, zone) = if is_fg {
        (
            pos2(origin.x + along * cell.x, origin.y + 0.04 * cell.y),
            zone_a(origin, cell),
        )
    } else {
        (
            pos2(origin.x + 0.05 * cell.x, origin.y + along * cell.y),
            zone_b(origin, cell),
        )
    };
    clamp_to_zone(Rect::from_center_size(centre, Vec2::splat(side)), zone)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 默认度量：字体测量完成前的出厂值（`apps/desktop/src/fonts.rs`）。
    fn default_cell() -> Vec2 {
        Vec2::new(7.8, 16.0)
    }

    fn palette() -> Palette {
        Palette {
            ansi: [term_core::grid::Rgb::new(0x33, 0x33, 0x33); 16],
            default_fg: term_core::grid::Rgb::new(0xCC, 0xCC, 0xCC),
            default_bg: term_core::grid::Rgb::new(0x1C, 0x1C, 0x1C),
            ansi_source: String::new(),
            theme_name: String::new(),
        }
    }

    /// 八个属性位全置的一格。
    fn all_bits_cell() -> Cell {
        let mut attrs = Attrs::EMPTY;
        for bit in [
            Attrs::BOLD,
            Attrs::DIM,
            Attrs::ITALIC,
            Attrs::UNDERLINE,
            Attrs::BLINK,
            Attrs::INVERSE,
            Attrs::HIDDEN,
            Attrs::STRIKE,
        ] {
            attrs.insert(bit);
        }
        Cell {
            ch: 'X',
            fg: Colour::Rgb(0xFF, 0x00, 0x80),
            bg: Colour::Rgb(0x0A, 0x3C, 0x5A),
            attrs,
            wide: true,
        }
    }

    /// 一格在默认字号下会画出的**全部**矩形——**走生产路径 `cell_marks`**。
    ///
    /// 这里不重抄「该画哪些标记」的判断：抄一遍就等于把测试写成复印件，生产那边
    /// 改坏了复印件照样绿（占位格那条测试就吃过这个亏）。
    fn all_rects(cell_data: &Cell, origin: Pos2, cell: Vec2) -> Vec<Rect> {
        let colours = DebugColours::new(&palette());
        cell_marks(cell_data, origin, cell, &colours, too_small(cell))
            .into_iter()
            .map(|(rect, _)| rect)
            .collect()
    }

    /// 交面积（`0` = 只在边界相切，那是允许的）。
    fn overlap_area(a: Rect, b: Rect) -> f32 {
        let inter = a.intersect(b);
        (inter.width().max(0.0)) * (inter.height().max(0.0))
    }

    /// **本文件的核心不变量**：八位同时置位时，六个区画出来的东西两两不相交。
    ///
    /// 这是「分区方案真的成立」的可证伪形式——任何一枚 pip 的 x 偏移画到邻槽、
    /// 任何一条贯穿线的 y 挪进 pip 架，这条立刻红。
    #[test]
    fn the_six_zones_are_pairwise_disjoint() {
        let cell = default_cell();
        let origin = pos2(100.0, 200.0);
        let rects = all_rects(&all_bits_cell(), origin, cell);
        // 6 pip + STRIKE + UNDERLINE + fg 3 + bg 3 + wide 1 = 15 个。
        assert_eq!(rects.len(), 15, "八位全开的矩形总数");
        for (i, a) in rects.iter().enumerate() {
            for (j, b) in rects.iter().enumerate().skip(i + 1) {
                let area = overlap_area(*a, *b);
                assert!(
                    area <= 0.0,
                    "第 {i} 个与第 {j} 个标记相交（面积 {area}）：{a:?} × {b:?}"
                );
            }
        }
    }

    /// 除 `wide` 竖线右缘外，一切标记都在格子内；那条右缘的出血 ≤0.3px。
    #[test]
    fn every_mark_stays_inside_its_cell_within_one_device_pixel() {
        let cell = default_cell();
        let origin = pos2(0.0, 0.0);
        let bounds = cell_rect(origin, cell);
        let rail = wide_rail_rect(origin, cell);
        for rect in all_rects(&all_bits_cell(), origin, cell) {
            if rect == rail {
                assert!(
                    rect.max.x - bounds.max.x <= 0.3,
                    "wide 竖线的右缘出血 {:.2}px 超过 0.3px",
                    rect.max.x - bounds.max.x
                );
                continue;
            }
            assert!(
                rect.min.x >= bounds.min.x - 0.01
                    && rect.min.y >= bounds.min.y - 0.01
                    && rect.max.x <= bounds.max.x + 0.01
                    && rect.max.y <= bounds.max.y + 0.01,
                "标记越出格子：{rect:?}（格子 {bounds:?}）"
            );
        }
    }

    /// 普通格子（`Cell::default()`）不画任何东西。这是性能前提，不是优化建议。
    #[test]
    fn a_plain_cell_draws_nothing() {
        let cell = Cell::default();
        assert!(!decorate(&cell), "默认格子必须走早退");
        assert!(all_rects(&cell, pos2(0.0, 0.0), default_cell()).is_empty());
    }

    /// 宽字符字形格 + 它的占位格，遍历后只产出**一组**标记。
    ///
    /// 占位格按 `vt.rs:563-573` 的**真实形状**造：`ch` 是空格、`attrs` / `fg` / `bg`
    /// 与字形格完全相同、而 `wide` 是 `false`。所以「它该不该画」只能由**前驱**
    /// 回答——这条测试调的是生产判定 `row_marks`，不是它的复印件。
    #[test]
    fn a_continuation_cell_draws_nothing() {
        let cell = default_cell();
        let colours = DebugColours::new(&palette());
        let mut attrs = Attrs::EMPTY;
        attrs.insert(Attrs::BOLD);
        let glyph = Cell {
            ch: '中',
            fg: Colour::Default,
            bg: Colour::Default,
            attrs,
            wide: true,
        };
        let placeholder = Cell {
            ch: ' ',
            fg: Colour::Default,
            bg: Colour::Default,
            attrs,
            wide: false,
        };
        let cells = [glyph, placeholder];
        let groups = row_marks(&cells, 2, pos2(0.0, 0.0), cell, &colours, false);
        assert_eq!(
            groups.len(),
            1,
            "占位格必须一格像素都不画——一个汉字只挂一套标记，不许两套 pip"
        );
        assert_eq!(groups[0].0, 0, "留下的必须是字形格那一列");
        // 字形格：1 枚 BOLD pip + 1 条 wide 竖线 = 2 个矩形。
        assert_eq!(groups[0].1.len(), 2);
    }

    /// 每个颜色都比正文淡，且 `dim < bold < rule` 这条浓度序成立。
    #[test]
    fn every_debug_alpha_is_fainter_than_the_foreground() {
        let c = DebugColours::new(&palette());
        for (name, col) in [
            ("bold", c.bold),
            ("dim", c.dim),
            ("italic", c.italic),
            ("blink", c.blink),
            ("inverse", c.inverse),
            ("hidden", c.hidden),
            ("rule", c.rule),
            ("kind", c.kind),
            ("wide", c.wide),
        ] {
            assert!(col.a() < 255, "{name} 必须是半透明的");
        }
        assert!(c.dim.a() < c.bold.a(), "DIM 要比 BOLD 淡");
        assert!(c.bold.a() < c.rule.a(), "贯穿线要比 BOLD pip 实一点");
        assert!(c.wide.a() < c.kind.a(), "wide 竖线要比色号方块淡，否则满屏竖栅栏");
    }

    /// 字号降到阈值以下就切到降级画法；阈值之上不降级。
    #[test]
    fn the_threshold_degrades_below_six_by_ten() {
        assert!(too_small(Vec2::new(5.9, 16.0)), "cell.x < 6.0 降级");
        assert!(too_small(Vec2::new(7.8, 9.9)), "cell.y < 10.0 降级");
        assert!(!too_small(Vec2::new(6.0, 10.0)), "等于阈值不降级");
        assert!(!too_small(default_cell()), "默认字号不降级");
    }
}

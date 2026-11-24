//! 鼠标 → 选区的**全部**手势（F1 / F3 / F4），以及它需要的窗格几何。
//!
//! 2026-11-13 从 `pane.rs` 拆出。理由是单文件 500 行上限——栅格可视化
//! 那批接线往 `pane.rs` 里塞进了两处 `apply_cmd_with_blocks`（菜单条路径与
//! Shift+方向键路径），把那个文件顶到 527 代码行。挑这一块搬是因为它本来就已经
//! 是一个**自由函数**（`pointer_selection`）加一个只为它服务的几何结构体
//! （`GridGeom`）——借用的拆法、Bin 的分叉点、五档手势的判定一行都没改。
//!
//! 上一条同因拆分是 `paste.rs`（ws-b1 把 `pane.rs` 顶到 518 时）。

use crate::ui::*;

/// 当前可见屏使用的选区分隔符。
///
/// 备用屏有独立字段；它为空时与主屏同值（字段文档的口径），主屏空串再回落到
/// 出厂串。三个分支写在一处，避免「双击用主屏串、按词移动用出厂串」的分裂。
///
/// 参数故意收 `&SessionConfig` + `alt` 而不是 `&Tab`：这样调用方可以同时借
/// `tab.selections` / `tab.viewport`（Rust 允许借用同一结构体的不同字段），
/// 不必先把字符串拷出来。函数放在本文件是因为它服务的就是双击选词这条路。
pub(super) fn visible_separators(config: &term_core::session::SessionConfig, alt: bool) -> &str {
    let main = term_app::selection::separators_or_default(&config.word_separators);
    if alt && !config.word_separators_alt.is_empty() {
        &config.word_separators_alt
    } else {
        main
    }
}

/// 一个窗格的网格几何：原点、格子大小、本屏行数。
///
/// 三样总是一起算、一起用，分散成三个参数只会让调用点长到没人读
/// （9 个参数也过不了 `clippy::too_many_arguments`）。
///
/// `Copy`：ws-wrap 起 `draw_tab` 把同一份几何**算一次**、给手势 / 正文 / 叠层
/// 三处共用（三份各写字面量就是三处漂移点）。三个字段都是 `Copy`，派生出来
/// 不会多一次分配，也没有任何语义变化。
#[derive(Clone, Copy)]
pub(super) struct GridGeom {
    pub(super) start: egui::Pos2,
    pub(super) cell: Vec2,
    pub(super) rows: usize,
    /// 本屏列数。右边界守卫用：鼠标落在右侧 15pt 滚动条命中区时，x 越界不能当成
    /// 「最后一列」产生选区（设计稿 §3.4 / §5.5）。
    pub(super) cols: usize,
}

/// 鼠标 → 选区的**全部**手势（F1 / F3 / F4）。
///
/// 抽成函数的理由不是「复用」，是**让 Bin 的分叉点只剩一个**：不抽的话
/// `draw_terminal_area_for` 里要在八个分支上各贴一次 `if !bin`，
/// 漏掉任何一处就是「切到 Bin 还能拖着选中一片网格坐标」。
///
/// 借用是拆开的：选区 / 视口 / 事实窗口各拿一个 `&mut` / `&`。整体借 `&mut Tab`
/// 不成立——`image` 本身就是从 `tab.view` 借出来的。
/// 左边距的点击命中区：只在 margin 可见时注册，矩形右缘是正文起点 `start_x`。
/// 抽出来是为了让 draw 函数少堆几行几何代码（500 行门禁）。
pub(super) fn margin_context_resp(
    ui: &egui::Ui,
    rect: egui::Rect,
    start_x: f32,
    slot_index: usize,
    width: f32,
) -> Option<egui::Response> {
    if width <= 0.0 {
        return None;
    }
    let margin_rect = egui::Rect::from_min_max(rect.min, egui::pos2(start_x, rect.max.y));
    Some(ui.interact(
        margin_rect,
        egui::Id::new(("term_margin_ctx", slot_index)),
        egui::Sense::click(),
    ))
}

pub(super) fn pointer_selection(
    selections: &mut Selections,
    viewport: &mut Viewport,
    history: i64,
    resp: &egui::Response,
    ui: &egui::Ui,
    image: &ScreenImage,
    geom: GridGeom,
    separators: &str,
    pairs: &term_app::selection::Pairs,
    word_wrap: bool,
) {
    let image_cols = image.cols.max(1);
    // 鼠标命中用的段表，与正文 / margin 同一口径：软折开时屏幕行 ≠ 逻辑行，
    // 屏幕第 n 行要经段表反查内容 (行, 列)。关软折时段表恒等，与旧行为逐像素相同。
    let wrap_cols = term_app::wrap::effective_wrap_cols(image_cols, word_wrap);
    let segments = term_app::wrap::segments_from_image(image, wrap_cols, word_wrap);
    // F1：鼠标 → 内容行列。行号用事实窗口的绝对行号，滚动不会让选区漂。
    let to_pos = |p: egui::Pos2| -> Pos {
        let screen_row = ((p.y - geom.start.y) / geom.cell.y).floor().max(0.0) as usize;
        // 右边界守卫：鼠标越过网格右缘（落在右侧 15pt 滚动条命中区）时，列号不参与——
        // 退到「最后一列之后」（哨兵 `image_cols`），避免被下面的 `.min` 钳成「最后一列」
        // 而误产生选区（设计稿 §3.4 / §5.5）。滚动条 widget 已注册在 content `resp` 之上
        // 会抢走按下事件，这里是兜底，二者不冲突。
        let right_edge = geom.start.x + (geom.cols as f32) * geom.cell.x;
        let local_col = if p.x > right_edge {
            image_cols
        } else {
            ((p.x - geom.start.x) / geom.cell.x).floor().max(0.0) as usize
        };
        // 屏幕 (行, 段内列) → 内容 (行, 绝对列)，经段表反查。关软折时段表恒等：
        // screen_row == 逻辑行下标、绝对列 == local_col，与旧行为逐像素相同。
        let (content_row, col_out) = segments
            .screen_to_content(screen_row, local_col)
            .map(|(line_idx, abs_col)| {
                let row = image
                    .lines
                    .get(line_idx)
                    .map_or_else(|| image.view_range().start().get(), |l| l.row);
                (row, abs_col.min(image_cols.saturating_sub(1)))
            })
            .unwrap_or_else(|| {
                let row = image
                    .lines
                    .get(screen_row)
                    .map_or_else(|| image.view_range().start().get(), |l| l.row);
                (row, local_col.min(image_cols.saturating_sub(1)))
            });
        Pos::new(content_row, col_out)
    };

    // F4：形状在**按下时**定一次（Alt = 矩形），拖动过程中不再变——
    // 中途松/按 Alt 就换形状会让已经画出来的高亮来回跳。
    // W2：`set_active` / `drag_active_to` 自己认锁——锁住时拖动与扩选都不改选区。
    let mods = ui.input(|i| i.modifiers);
    if resp.drag_started() {
        if let Some(p) = resp.interact_pointer_pos() {
            let sel = Selection::new(to_pos(p));
            selections.set_active(Some(if mods.alt { sel.block() } else { sel }));
        }
    }
    if resp.dragged() {
        if let Some(p) = resp.interact_pointer_pos() {
            selections.drag_active_to(to_pos(p));
        }
    }

    // F3：拖出上下边缘时让视口自己跟上——否则「选到屏外那几行」做不到。
    // 速度按越界的距离算（1–5 行/帧），并主动请求重绘：
    // 指针不动时 egui 不会自发重绘（空闲 0 CPU 的约定下），不请求就会卡在一行。
    if resp.dragged() {
        if let Some(p) = resp.interact_pointer_pos() {
            let top = geom.start.y;
            let bottom = top + geom.rows as f32 * geom.cell.y;
            let speed = |over: f32| ((over / geom.cell.y).ceil() as i64).clamp(1, 5);
            let before = viewport.scroll();
            if p.y < top {
                viewport.scroll_up(speed(top - p.y), history);
            } else if p.y > bottom {
                viewport.scroll_down(speed(p.y - bottom), history);
            }
            // 只有真的滚动了才请求重绘：滚到顶 / 没有历史时指针停在边缘
            // 不该把进程拖进「永远重绘」的空转。
            if viewport.scroll() != before {
                ui.ctx().request_repaint();
            }
        }
    }

    // F3：双击选词、三击选行。egui 对同一次点击会同时置 `clicked`，
    // 所以顺序必须是「先判更具体的点击」——否则选词/选行会立刻被清空。
    if resp.triple_clicked() {
        if let Some(p) = resp.interact_pointer_pos() {
            selections.set_active(line_span_at(image, to_pos(p).row));
        }
    } else if resp.double_clicked() {
        if let Some(p) = resp.interact_pointer_pos() {
            let pos = to_pos(p);
            // 双击括号 → 选整对（含两端）；不在括号上才回退双击选词。
            let sel = term_app::selection::pair_selection_at_with(image, pos, pairs)
                .or_else(|| word_at_with(image, pos, separators));
            selections.set_active(sel);
        }
    } else if resp.clicked() {
        match (mods.shift, selections.has_active()) {
            // F4：shift-click 扩选——保留锚点与形状，只把头部挪过去。
            // 没有现成选区时退化成「重新起一个锚点」。
            (true, true) => {
                if let Some(p) = resp.interact_pointer_pos() {
                    selections.drag_active_to(to_pos(p));
                }
            }
            (true, false) => {
                if let Some(p) = resp.interact_pointer_pos() {
                    selections.set_active(Some(Selection::new(to_pos(p))));
                }
            }
            // 单击不构成选区，同时用来取消上一次选区（整批一起清）。
            _ => selections.clear(),
        }
    }
}

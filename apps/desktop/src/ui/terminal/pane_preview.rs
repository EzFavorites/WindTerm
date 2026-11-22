//! 终端悬停预览窗：轨道 hover → 第二事实窗口（独立槽）→ 历史快照框。
//!
//! 浮动 chrome（第 8 段，在 `paint_scrollbar` 之后）。只读 `tab.preview_screen`，
//! 绝不碰 `tab.view` / `tab.viewport` / `tab.last_request`——这就是 §5.6 不变式
//! （Previewing 四者都不变，只有 `preview_screen` / `preview_last_request` 变）。
//!
//! 框内是「同一套配置下的一个历史视口」：左侧边栏复用真实终端的 `margin::paint`，
//! 正文从 `margin_met.width` 之后起画，底色取主题背景——行号、时间戳、符号列与
//! 真实终端逐列同源，配置一变两边一起变（2026-11-19 用户比对修正）。
//!
//! bin 模式无 `ScreenImage` 事实窗口，首版不画预览（见 design §5.5）。
//!
//! 关联设计稿：`docs/design/03-selection-find-scrollback.md`。

use super::margin::{self, MarginMetrics};
use super::view::{self, LineCache};
use crate::ui::*;
use term_app::folds::Folds;
use term_app::margin::MarginState;
use term_app::scrollbar::preview_box_top;
use term_app::selection::Selections;
use term_app::wrap;
use term_core::grid::{GridSnapshot, ScreenImage};

/// 预览框高占内容区高的比例（兼容终端 GIF 标定：576 / 1022 ≈ 0.564）。
pub(super) const PREVIEW_FRACTION: f64 = 576.0 / 1022.0;

/// 预览框内的可见行数（用于请求 `ViewRange`）。框高由 `box_h / cell_h` 反算，
/// 与框的屏幕高度同源（设计稿 §8）。
pub(super) fn preview_rows(rect: egui::Rect, cell_h: f32) -> usize {
    let content_h = (rect.height() - GRID_PAD * 2.0).max(0.0);
    let box_h = content_h * PREVIEW_FRACTION as f32;
    ((box_h / cell_h).round().max(1.0)) as usize
}

/// 预览框矩形：占满内容区宽、恒高 `PREVIEW_FRACTION`、中心跟随 hover y 并钳进内容区
/// （复用纯几何 [`preview_box_top`]，复现 GIF frame 30 的框位）。
pub(super) fn box_rect(rect: egui::Rect, cell_h: f32, hover_y: f32) -> egui::Rect {
    let content_top = rect.min.y + GRID_PAD;
    let content_h = (rect.height() - GRID_PAD * 2.0).max(0.0);
    let box_h = ((content_h as f64) * PREVIEW_FRACTION).max(cell_h as f64);
    let top = preview_box_top(content_top as f64, content_h as f64, box_h, hover_y as f64) as f32;
    let x = rect.min.x + GRID_PAD;
    let w = (rect.width() - GRID_PAD * 2.0).max(0.0);
    egui::Rect::from_min_size(egui::pos2(x, top), egui::vec2(w, box_h as f32))
}

/// 绘制预览框（只读 `image` / 配置，不写任何状态）。
///
/// 框内是「同一套配置下的一个历史视口」：底色取主题背景（与真实终端同源），再
/// 复用真实终端**同一个** `margin::paint` 画左侧时间戳 / 行号 / 折叠 / 符号列，
/// 正文从 `box_rect.min.x + margin_met.width` 起画。这样行号右边界、数字、时间戳、
/// 符号列与真实终端逐列同源——配置（行号列宽 / 时间戳开关 / 折叠列 / 主题色）
/// 一变两边一起变，不会各自漂移（2026-11-19 用户比对修正）。
///
/// 栈上独立 `LineCache`（不与 `self.term_view` 交替写），空选 / 无光标；隔行底色
/// 跟随 `alt_row_bg` 配置。最后补上下 1px 亮边线。
#[allow(clippy::too_many_arguments)]
pub(super) fn paint(
    painter: &egui::Painter,
    box_rect: egui::Rect,
    image: &ScreenImage,
    cell: Vec2,
    palette: &Palette,
    font_id: &egui::FontId,
    margin_state: &MarginState,
    margin_met: &MarginMetrics,
    snap: &GridSnapshot,
    folds: &Folds,
    bg: egui::Color32,
    alt_row_bg: bool,
) {
    // 框底两层：先用不透明主题色盖掉下层实时视口的字（`paint_rows` 只画非默认
    // 背景的格，框底透明会重影），再叠与真实终端同源的窗口背景（含窗口 alpha）。
    // 两者都来自配置，不再有与配置无关的硬编码橄榄色。
    painter.rect_filled(box_rect, 0.0, to_color32(palette.default_bg));
    painter.rect_filled(box_rect, 0.0, bg);

    let mut cache = LineCache::default();
    let widths: Vec<Vec<u8>> = image
        .lines
        .iter()
        .map(|line| wrap::glyph_widths(&line.cells))
        .collect();
    let segments = wrap::wrap_lines(&widths, image.cols.max(1));
    let selection_bg = to_color32(palette.default_fg).gamma_multiply(0.25);
    // egui 0.29：`with_clip_rect` 返回**新的** `Painter`（clip 取交集），不是闭包式。
    let clipped = painter.with_clip_rect(box_rect);
    // 内容起点下移 2px：给顶边亮线留出裸露的底色，否则首行字形顶到 `box_rect.min`
    // 会把亮线盖掉（上下不对称）。margin 与正文共用这个 y，逐行严格对齐。
    let origin = egui::pos2(box_rect.min.x, box_rect.min.y + 2.0);

    // 左侧边栏与真实终端用同一个 `MarginMetrics` / `MarginState` / 快照 / 折叠集合：
    // 行号右边界 `num_right` 同源 → 数字对齐；时间戳 / 符号列同源 → 显示一致。
    // 历史快照没有「当前输入行」，与回看态一致传 `None`。
    margin::paint(
        &clipped,
        margin_state,
        margin_met,
        box_rect,
        origin.y,
        cell.y,
        image,
        snap,
        folds,
        font_id,
        None,
    );

    // 正文从边栏之后起画：`origin.x + margin_met.width` 与真实终端的 `start_x`
    // （`rect.min.x + GRID_PAD + margin_met.width`）同源，只差框与 rect 的起点。
    view::paint_rows(
        &clipped,
        &mut cache,
        image,
        egui::pos2(origin.x + margin_met.width, origin.y),
        cell,
        palette,
        font_id,
        &Selections::new(),
        selection_bg,
        alt_row_bg,
        None,
        &[],
        &segments,
    );

    // 上下 1px 亮边线（设计稿 §5.4），框出取景范围。用满强度前景色、加 1px 宽度，
    // 在底色上拉开对比——`gamma_multiply(0.5)` 太暗，肉眼几乎看不出边界。
    let stroke = egui::Stroke::new(2.0, to_color32(palette.default_fg));
    let x_range = box_rect.x_range();
    painter.hline(x_range, box_rect.top(), stroke);
    painter.hline(x_range, box_rect.bottom() - 1.0, stroke);
}

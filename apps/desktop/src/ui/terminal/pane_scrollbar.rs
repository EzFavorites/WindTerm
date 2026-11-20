//! P3 · 终端滚动条：egui 几何适配、轨道/滑块绘制、命中区、拖拽、点击跳转、滚轮补位。
//!
//! 浮动叠加（D1）：不改 `avail_w` / `cols` / `tab.resize`，贴底视觉基线可零 diff。
//! 轨道**无底色**（对齐兼容终端 §2.3）：只画滑块，hover / 拖拽时提亮（指向指示色，
//! 只在真实 hover 时出现——静态夹具用 `pointed` 定格，生产路径传 `None`）。
//!
//! bin 模式首版不做滚动条（坐标来自 `RawLog.anchor` 而非 `Viewport`，见设计稿 §5.5
//! 后续）；`pane.rs` 仅在 `!bin` 时调用本模块，避免半套 bin 映射污染 Text 路径。
//!
//! 关联设计稿：`docs/design/03-selection-find-scrollback.md`。

use crate::ui::*;
use term_app::scrollbar::{scroll_top_of, thumb_of, Track};

/// 滑块 / 轨道宽（兼容终端 GIF 观测约 15pt）。`pub(super)` 让药丸让位时复用同一常量。
pub(super) const SB_W: f32 = 15.0;
/// 滑块最小高度下限；与 egui `ScrollStyle::handle_min_length` 对齐（`Track::min_thumb`）。
const SB_MIN_THUMB: f64 = 12.0;

/// 滚动条每帧所需的视口快照参数（几何 + 滚轮）。打包成一个值，避免 `interact`
/// 越过 clippy `too_many_arguments` 的 7 参数上限；`paint` 只读其中两项。
#[derive(Clone, Copy)]
pub(super) struct ScrollbarView {
    pub(super) rows: usize,
    pub(super) history: i64,
    pub(super) cell_h: f32,
    pub(super) wheel_lines: i64,
}

/// 轨道矩形：贴右边缘、占满内容高度（浮动叠加，不改 `avail_w` / `cols`）。
/// `pub(super)`：预览窗算 `ratio` 也要用它（指针 y 落在轨道的归一化位置）。
pub(super) fn track_rect(rect: egui::Rect) -> egui::Rect {
    let x = rect.max.x - GRID_PAD - SB_W;
    let y = rect.min.y + GRID_PAD;
    let h = (rect.height() - GRID_PAD * 2.0).max(0.0);
    egui::Rect::from_min_size(egui::pos2(x, y), egui::vec2(SB_W, h))
}

/// 滑块矩形：纯几何 [`thumb_of`] 投影到屏幕坐标。
fn thumb_rect(rect: egui::Rect, rows: usize, history: usize, scroll_top: u64) -> egui::Rect {
    let tr = track_rect(rect);
    let track = Track {
        height: tr.height() as f64,
        min_thumb: SB_MIN_THUMB,
    };
    let th = thumb_of(track, rows, history, scroll_top);
    egui::Rect::from_min_size(
        egui::pos2(tr.min.x, tr.min.y + th.top as f32),
        egui::vec2(SB_W, th.height as f32),
    )
}

/// 绝对指针 y，换算成相对轨道顶的偏移；拿不到指针时返回 `None`。
///
/// 单独成自由函数（不 capture `ui` 的闭包），避免与下面的 `ui.data_mut` 可变借用打架。
fn pointer_y(ui: &egui::Ui, resp: &egui::Response, tr: egui::Rect) -> Option<f32> {
    ui.input(|i| i.pointer.interact_pos())
        .or_else(|| resp.hover_pos())
        .map(|p| p.y - tr.min.y)
}

/// 命中区 + 拖拽 / 点击 / 滚轮交互。**必须在 `tab` 仍可变时调用**（直接改 `viewport`）。
///
/// `history <= 0`（无历史可看）时整条滚动条不画不交互——贴底视觉基线保持零 diff。
pub(super) fn interact(
    ui: &mut egui::Ui,
    id: egui::Id,
    rect: egui::Rect,
    view: ScrollbarView,
    viewport: &mut Viewport,
) -> Option<f32> {
    let (rows, history, cell_h, wheel_lines) = (view.rows, view.history, view.cell_h, view.wheel_lines);
    if history <= 0 {
        return None;
    }
    let history = history as usize;
    let tr = track_rect(rect);
    // 注册在 content `resp` 之后（照 margin 菜单先例）→ 重叠区压在上面，抢到按下事件。
    let resp = ui.interact(tr, id, egui::Sense::click_and_drag());

    let track = Track {
        height: tr.height() as f64,
        min_thumb: SB_MIN_THUMB,
    };
    let st0 = viewport.scroll_top(history as u64);
    let th = thumb_of(track, rows, history, st0);
    let thumb_top = th.top as f32;
    let thumb_bottom = (th.top + th.height) as f32;

    // 抓取偏移：按下在滑块上才允许拖拽，并记录指针与滑块上缘的差；
    // 按下在轨道（滑块之外）则不拖，留作「点击跳转」。
    let grab_id = id.with("grab");
    if resp.drag_started() {
        if let Some(y) = pointer_y(ui, &resp, tr) {
            let grab = if y >= thumb_top && y <= thumb_bottom {
                Some(y - thumb_top)
            } else {
                None
            };
            ui.data_mut(|d| d.insert_temp(grab_id, grab));
        }
    }
    if resp.dragged() {
        if let Some(grab) = ui.data(|d| d.get_temp::<Option<f32>>(grab_id)).flatten() {
            if let Some(y) = pointer_y(ui, &resp, tr) {
                let new_top = (y - grab).max(0.0);
                let st = scroll_top_of(track, rows, history, new_top as f64);
                viewport.apply_scroll_top(st, history as u64);
                ui.ctx().request_repaint();
            }
        }
    }

    // 点击轨道（点在滑块之外）→ 跳到该位置（用户裁定：点击跳转）。
    // `!resp.dragged()` 把「拖完松手」排除掉，避免和上面的拖拽重复跳。
    if resp.clicked() && !resp.dragged() {
        if let Some(y) = pointer_y(ui, &resp, tr) {
            if y < thumb_top || y > thumb_bottom {
                let ratio = (y as f64 / tr.height() as f64).clamp(0.0, 1.0);
                let st = (ratio * history as f64).round() as u64;
                viewport.apply_scroll_top(st, history as u64);
                ui.ctx().request_repaint();
            }
        }
    }

    // 滚轮补位：指针在轨道上时，content 的 `resp.hovered()` 已不再是它（滚动条更靠上
    // 注册），原滚轮分支不触发；这里显式接住，避免「滚不动」死角（设计稿 §5.5）。
    if resp.hovered() {
        let dy = ui.input(|i| i.raw_scroll_delta.y);
        if dy.abs() > f32::EPSILON {
            let lines = ((dy / cell_h) * wheel_lines as f32).round() as i64;
            if lines != 0 {
                let max = history as i64;
                if lines > 0 {
                    viewport.scroll_up(lines, max);
                } else {
                    viewport.scroll_down(-lines, max);
                }
                ui.ctx().request_repaint();
            }
        }
    }

    // 悬停预览触发：指针在轨道上、且没在拖拽时，把 hover y 交回 `pane.rs` 去请求
    // 第二事实窗口（独立槽）。拖拽时走 Scrolling 分支、不触发 Previewing（§5.6）。
    if resp.hovered() && !resp.dragged() {
        resp.hover_pos()
            .or_else(|| ui.input(|i| i.pointer.interact_pos()))
            .map(|p| p.y)
    } else {
        None
    }
}

/// 第 7 段 chrome：只画滑块，不画轨道底色（对齐兼容终端 §2.3）。
/// 只读 `viewport`，可在 `paint_overlays` 之后调用。
pub(super) fn paint(
    painter: &egui::Painter,
    rect: egui::Rect,
    rows: usize,
    history: i64,
    viewport: &Viewport,
) {
    if history <= 0 {
        return;
    }
    let history = history as usize;
    let tr = track_rect(rect);
    let st = viewport.scroll_top(history as u64);
    let thumb = thumb_rect(rect, rows, history, st);
    // 常态中灰；hover 时提亮（指向指示色，真实 hover 才出现）。
    let hovered = painter
        .ctx()
        .pointer_interact_pos()
        .is_some_and(|p| tr.contains(p));
    let color = if hovered {
        egui::Color32::from_gray(200)
    } else {
        egui::Color32::from_gray(140)
    };
    painter.rect_filled(thumb, 0.0, color);
}

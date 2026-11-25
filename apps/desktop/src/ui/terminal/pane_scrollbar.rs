//! 终端滚动条：egui 几何适配、轨道/滑块绘制、命中区、拖拽、点击跳转、滚轮补位。
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
use term_app::scrollbar::{hover_in_invisible_area, scroll_top_of, thumb_of, Track};

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
    /// 鼠标报告开关 / Shift 豁免与正文共用同一份快照：滚滚动条时也不能
    /// 「既滚程序又滚回看」（xterm 惯例，见 `pane.rs` 的 `wheel_to_program`）。
    pub(super) mouse_reporting: bool,
    pub(super) shift: bool,
}

/// 滚轮归本地回看还是归程序，与正文同一把尺子。
///
/// 裁决：滚动条是本地 chrome，但**滚轮手势**仍受鼠标报告门控——不开报告或
/// 按住 Shift 才是本地手势；否则指针在轨道上时不能绕过豁免偷偷滚回看。
/// 拖拽 / 点击跳转仍是本地操作，不走这条判据。
fn wheel_is_local(mouse_reporting: bool, shift: bool) -> bool {
    !mouse_reporting || shift
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

/// 点击轨道的逆映射：把「点在哪」当成滑块**中心**，与拖拽时的
/// `y - grab`（grab 记的就是按下点到滑块上缘的距离）同一把尺子。
/// 原先的 `ratio * history` 与滑块几何不同源，点一下会跳到不相干的位置。
fn click_scroll_top(track: term_app::scrollbar::Track, rows: usize, history: usize, y: f64, thumb_height: f64) -> u64 {
    scroll_top_of(track, rows, history, y - thumb_height / 2.0)
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
    let wheel_local = wheel_is_local(view.mouse_reporting, view.shift);
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
                let st = click_scroll_top(track, rows, history, y as f64, th.height);
                viewport.apply_scroll_top(st, history as u64);
                ui.ctx().request_repaint();
            }
        }
    }

    // 滚轮补位：指针在轨道上时，content 的 `resp.hovered()` 已不再是它（滚动条更靠上
    // 注册），原滚轮分支不触发；这里显式接住，避免「滚不动」死角（设计稿 §5.5）。
    // 滚动条是本地 chrome：这一支**不读 mouse_reporting / Shift**——鼠标报告开着时
    // content 把滚轮交给程序、mouse-report 帧也不上报轨道位置，只有这里能滚回看。
    // Shift 的本地豁免语义仍由正文分支独占，此处不重复门控。
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
    //
    // 只在**屏幕不可见区域**（滑块之外的轨道段）触发。滑块投影的
    // 正是当前可见的 rows 行——悬停在它上面时那一段已经在屏上，弹预览既挡住
    // 真实视口又没有新信息；只有悬停在滑块上方（更旧历史）或下方（更新的行）
    // 才有必要预览。判定收在纯函数 `hover_in_invisible_area` 里（可单测）。
    if resp.hovered() && !resp.dragged() {
        resp.hover_pos()
            .or_else(|| ui.input(|i| i.pointer.interact_pos()))
            .and_then(|p| {
                let y_in_track = (p.y - tr.min.y) as f64;
                hover_in_invisible_area(th, y_in_track).then_some(p.y)
            })
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

#[cfg(test)]
mod tests {
    use super::*;

    fn track() -> term_app::scrollbar::Track {
        term_app::scrollbar::Track { height: 400.0, min_thumb: SB_MIN_THUMB }
    }

    #[test]
    fn a_click_at_the_thumb_centre_is_the_same_inverse_as_a_drag() {
        let tr = track();
        let (rows, history) = (24usize, 100usize);
        for st in [0u64, 25, 50, 100] {
            let th = thumb_of(tr, rows, history, st);
            let centre = th.top + th.height / 2.0;
            // 拖拽时 grab = y - thumb_top；点中心同样落在滑块中心。
            assert_eq!(click_scroll_top(tr, rows, history, centre, th.height), st);
            assert_eq!(scroll_top_of(tr, rows, history, centre - th.height / 2.0), st);
        }
    }

    #[test]
    fn the_wheel_gate_matches_the_content_branch_and_shift_exempts_the_scrollbar() {
        // 与 pane.rs 的 `wheel_to_program = mouse_reporting && !shift` 互补。
        assert!(wheel_is_local(false, false), "关报告：滚轮永远归本地");
        assert!(wheel_is_local(false, true));
        assert!(!wheel_is_local(true, false), "开报告又没按 Shift：滚轮归程序");
        assert!(wheel_is_local(true, true), "Shift 是本地手势的豁免键");
    }

    #[test]
    fn clicking_the_track_top_and_bottom_reaches_both_ends() {
        let tr = track();
        let (rows, history) = (24usize, 100usize);
        let th = thumb_of(tr, rows, history, 50);
        assert_eq!(click_scroll_top(tr, rows, history, 0.0, th.height), 0);
        assert_eq!(click_scroll_top(tr, rows, history, tr.height, th.height), history as u64);
    }

    /// egui 的 hover 命中用的是**上一帧**的 widget rect：先跑一帧把轨道热区
    /// 注册进 `prev_pass`，第二帧再投递滚轮，才是生产路径的真实时序。
    fn wheel_after_a_warmup_frame(pointer: egui::Pos2) -> i64 {
        let rect = egui::Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(800.0, 420.0));
        let view = ScrollbarView { rows: 24, history: 100, cell_h: 16.0, wheel_lines: 1 };
        let mut viewport = Viewport::new();
        let ctx = egui::Context::default();
        for events in [
            vec![egui::Event::PointerMoved(pointer)],
            vec![
                egui::Event::PointerMoved(pointer),
                // 一整格 16pt：dy / cell_h * wheel_lines = 1 行。
                egui::Event::MouseWheel {
                    unit: egui::MouseWheelUnit::Point,
                    delta: egui::vec2(0.0, 16.0),
                    modifiers: egui::Modifiers::default(),
                },
            ],
        ] {
            let _ = ctx.run(
                egui::RawInput {
                    screen_rect: Some(rect),
                    events,
                    ..Default::default()
                },
                |ctx| {
                    egui::CentralPanel::default().show(ctx, |ui| {
                        interact(ui, egui::Id::new(("term_scrollbar", 0)), rect, view, &mut viewport);
                    });
                },
            );
        }
        viewport.scroll()
    }

    /// 真实几何下的轨道滚轮：指针压在滚动条上时 content 的 `resp.hovered()` 已经
    /// 不是它（滚动条更靠上注册），mouse-report 帧也不把这一击当程序手势——
    /// 滚动条是本地 chrome，轨道滚轮必须无条件滚回看。
    ///
    /// 回归：这里曾加过 `!mouse_reporting || shift` 门控（与正文共用一把尺子），
    /// 于是开着鼠标报告时三处都不处理，滚轮在滚动条上成了死区。
    #[test]
    fn a_wheel_over_the_track_scrolls_back_as_local_chrome() {
        let rect = egui::Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(800.0, 420.0));
        assert_eq!(wheel_after_a_warmup_frame(track_rect(rect).center()), 1);
    }

    /// 同一帧里指针落在正文（轨道之外）时，滚动条不许凭空吃下这次滚轮。
    /// 门是几何 hover，不是 mouse_reporting / Shift。
    #[test]
    fn a_wheel_outside_the_track_does_not_touch_the_scrollbar() {
        assert_eq!(wheel_after_a_warmup_frame(egui::pos2(100.0, 200.0)), 0);
    }
}

//! P · 终端滚动条与悬停预览窗 —— 纯几何层（零 egui、零 IO、可单测）。
//!
//! 这一层只做「数值 → 数值」的换算，不认识像素坐标系、不认识 egui、不碰
//! 任何内核状态。它把三件参考实现可观测行为翻译成纯函数：
//!
//! 1. **滑块几何**：给定可见行数 `rows`、历史总行数 `history`、当前回看位置
//!    `scroll_top`，算出滑块在轨道上的 `top` 与 `height`。滑块高 ∝
//!    `rows / (history + rows)`（和参考实现 GIF 里「滑块占轨道 ~53%」同量级），
//!    并有最小高度下限（`Track.min_thumb`，与 egui `handle_min_length` 对齐）。
//! 2. **拖拽反解**：给定滑块上缘像素位置，反算回 `scroll_top`，与 `thumb_of`
//!    互为逆（边界钳制，允许 ±1 行取整误差）。
//! 3. **预览映射**：鼠标在轨道上的归一化位置 `ratio` → 历史位置 →
//!    [`ViewRange`]。复用 [`Viewport::apply_scroll_top`] / [`Viewport::rows`]，
//!    **不引入第二套行号语义**（N 状态留存已经验过这套换算）。
//!
//! 颜色 / 绘制 / 命中区全在 `apps/desktop` 的 `pane_scrollbar.rs` / `pane_preview.rs`，
//! 本层一个像素常量都没有。
//!
//! 关联设计稿：`docs/design/03-selection-find-scrollback.md`。
//! 关联决策：`docs/design/03-selection-find-scrollback.md`。

use term_core::coords::ViewRange;

use crate::viewport::Viewport;

/// 滚动条轨道的纯数值描述。
///
/// 全部以「像素」为单位，本层不关心屏幕坐标从哪起算——那是投影层的活。
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Track {
    /// 轨道（滑块可被拖动的总高）。
    pub height: f64,
    /// 滑块最小高度；与 egui `ScrollStyle::handle_min_length` 对齐，防止长会话
    /// 时滑块缩成看不见的一条线（见设计稿 §8 R5）。
    pub min_thumb: f64,
}

/// 滑块在轨道上的纯数值位置。
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Thumb {
    /// 滑块上缘相对轨道顶部的偏移（像素）。
    pub top: f64,
    /// 滑块高度（像素）。
    pub height: f64,
}

/// 滑块高与位置：**`history == 0`（无历史可看）时滑块占满整条轨道、且不可拖**；
/// 否则滑块高 ∝ 可见行占比，并按 [`Track::min_thumb`] 托底。
///
/// 换算：
/// - `total = history + rows`；
/// - `h = max(min_thumb, rows / total * track)`（再被 `track` 拦一道，防御负值）；
/// - `travel = track - h`（滑块可移动的净高度）；
/// - `frac = scroll_top / history`（0 = 历史顶端，history = 贴底）；
/// - `top = frac * travel`。
///
/// 与 [`scroll_top_of`] 共用同一个 `h` 与 `travel` 公式，保证两者互为逆。
pub fn thumb_of(track: Track, rows: usize, history: usize, scroll_top: u64) -> Thumb {
    let track_h = track.height.max(0.0);
    // 没有可见行或没有历史：滑块占满轨道、不可拖。
    if rows == 0 || history == 0 {
        return Thumb {
            top: 0.0,
            height: track_h,
        };
    }
    let total = (history + rows) as f64;
    let h = (rows as f64 / total * track_h)
        .max(track.min_thumb.max(0.0))
        .min(track_h);
    let travel = (track_h - h).max(0.0);
    let frac = if history == 0 {
        0.0
    } else {
        (scroll_top as f64 / history as f64).clamp(0.0, 1.0)
    };
    let top = frac * travel;
    Thumb { top, height: h }
}

/// [`thumb_of`] 的逆：给定滑块上缘像素位置，反算 `scroll_top`。
///
/// 反解用「有效轨道高 = `track - h`」，避免在上下端因 `h` 被钳制而产生跳变
/// （设计稿 §8 R5）。`history == 0` 或 `rows == 0` 时恒返回 0（贴底）。
pub fn scroll_top_of(track: Track, rows: usize, history: usize, thumb_top: f64) -> u64 {
    let track_h = track.height.max(0.0);
    if rows == 0 || history == 0 {
        return 0;
    }
    let total = (history + rows) as f64;
    let h = (rows as f64 / total * track_h)
        .max(track.min_thumb.max(0.0))
        .min(track_h);
    let travel = (track_h - h).max(0.0);
    if travel <= 0.0 {
        return 0;
    }
    let frac = (thumb_top / travel).clamp(0.0, 1.0);
    (frac * history as f64).round() as u64
}

/// 鼠标在轨道上的归一化位置 `ratio`（0 = 轨道顶 / 历史最老，1 = 轨道底 / 贴底）
/// → 预览视口的 [`ViewRange`]（`preview_rows` 行）。
///
/// 不新增滚动语义：构造一个临时 [`Viewport`]，调既有
/// [`Viewport::apply_scroll_top`] 再 [`Viewport::rows`]，与真实视口共用同一套
/// 「`scroll_top` ↔ `scroll`」换算（设计稿 §5.2 / §7.1.6）。
///
/// `ratio` 会被钳进 `[0, 1]`；`history == 0` 时恒落在贴底（range 起点 0）。
///
/// 注：签名里**没有** `rows` 参数——可见行数不参与换算（预览框高度由 `box_h /
/// cell_h` 反算，见 §8 R4），带上只会触发 clippy 的未使用参数告警。
pub fn preview_range(history: usize, ratio: f64, preview_rows: usize) -> ViewRange {
    let ratio = ratio.clamp(0.0, 1.0);
    let scroll_top = (ratio * history as f64).round() as u64;
    let mut vp = Viewport::new();
    vp.apply_scroll_top(scroll_top, history as u64);
    vp.rows(preview_rows)
}

/// 预览框（占满内容区宽、恒高 `box_h` 的半透明取景框）的**上缘**。
///
/// 设计稿 §2.6 / §7.1.5：框中心跟随 hover y（`desired = hover_y - box_h / 2`），
/// 再把整块框钳进内容区 `[content_top, content_top + content_h - box_h]`，避免
/// 框溢出到菜单 / 状态栏。复现官方 GIF frame 30：`hover_y=426, box_h=576,
/// content_top=195, content_h=1022 → 195`。
pub fn preview_box_top(content_top: f64, content_h: f64, box_h: f64, hover_y: f64) -> f64 {
    let box_h = box_h.max(0.0);
    let content_h = content_h.max(0.0);
    let max_top = (content_top + content_h - box_h).max(content_top);
    let desired = hover_y - box_h / 2.0;
    desired.clamp(content_top, max_top)
}

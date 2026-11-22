//! Vim 导航意图在**本地模式**下的落地：只移动视口 / 选区，绝不写 PTY。
//!
//! 纯解析在 `term_app::vim`；这里把它产出的 motion 翻译成已有的视口与选区
//! API，所以「Vim 的 j」和「方向键」走的是同一条滚动路径，不会出现两套坐标系。
//! 借用拆成字段而不是收 `&mut Tab`：调用点手上还握着 `&tab.view` 画的那一帧，
//! 整体借 `Tab` 会与它冲突（draw 函数里 `image` 的生命周期跨过按键循环）。

use super::pane_pointer::visible_separators;
use eframe::egui;
use term_app::selection::{apply_cmd_with_blocks_seps, Cmd, Op, Selections};
use term_app::vim::Motion;
use term_app::viewport::Viewport;
use term_core::grid::{GridSnapshot, ScreenImage};
use term_core::session::SessionConfig;

/// 把这一帧攒下的字符逐个喂给纯解析层，命中的 motion 立刻落地。
/// 收拢到一处是为了不让 draw 函数再长十几行（500 行门禁）。
#[allow(clippy::too_many_arguments)]
pub(super) fn drain(
    viewport: &mut Viewport,
    view: &ScreenImage,
    snapshot: &GridSnapshot,
    selections: &mut Selections,
    config: &SessionConfig,
    vim: &mut term_app::vim::VimState,
    keys: Vec<char>,
    ctx: &egui::Context,
) {
    for ch in keys {
        if let Some((motion, count)) = vim.feed(ch) {
            apply(viewport, view, snapshot, selections, config, motion, count);
            ctx.request_repaint();
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn apply(
    viewport: &mut Viewport,
    view: &ScreenImage,
    snapshot: &GridSnapshot,
    selections: &mut Selections,
    config: &SessionConfig,
    motion: Motion,
    count: usize,
) {
    let n = count.max(1) as i64;
    let history = snapshot.history as i64;
    match motion {
        Motion::Up => viewport.scroll_up(n, history),
        Motion::Down => viewport.scroll_down(n, history),
        Motion::FirstLine => viewport.jump_to_row(-history, history),
        Motion::LastLine => viewport.jump_to_row(0, history),
        Motion::Left => move_selection(view, snapshot, selections, config, Op::CharPrev, count),
        Motion::Right => move_selection(view, snapshot, selections, config, Op::CharNext, count),
        Motion::WordForward => {
            move_selection(view, snapshot, selections, config, Op::WordStartNext, count)
        }
        Motion::WordBack => {
            move_selection(view, snapshot, selections, config, Op::WordStartPrev, count)
        }
        Motion::LineStart => {
            move_selection(view, snapshot, selections, config, Op::ToLineHome, count)
        }
        Motion::LineEnd => move_selection(view, snapshot, selections, config, Op::ToLineEnd, count),
    }
}

fn move_selection(
    view: &ScreenImage,
    snapshot: &GridSnapshot,
    selections: &mut Selections,
    config: &SessionConfig,
    op: Op,
    count: usize,
) {
    let seps = visible_separators(config, snapshot.alt).to_string();
    // 上限防止 `999999999h` 把主线程卡死；Vim 自己也有类似的实际限制。
    for _ in 0..count.min(1000) {
        apply_cmd_with_blocks_seps(Cmd::Move(op), view, &snapshot.blocks, selections, &seps);
    }
}

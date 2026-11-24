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

/// Vim 大写词键（W/B/E/gE）按空白分词；小写键沿用会话配置的分隔符。
const BLANK_SEPS: &str = " \t";

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
    let seps = visible_separators(config, snapshot.alt).to_string();
    match motion {
        Motion::Up => viewport.scroll_up(n, history),
        Motion::Down => viewport.scroll_down(n, history),
        Motion::FirstLine => viewport.jump_to_row(-history, history),
        // 裸 G 到当前屏首行（最后一行）；nG 到会话第 n 行（历史从 -history 起）。
        Motion::LastLine => {
            let row = if count <= 1 { 0 } else { (-history + n - 1).clamp(-history, 0) };
            viewport.jump_to_row(row, history);
        }
        Motion::Left => move_selection(view, snapshot, selections, &seps, Op::CharPrev, count),
        Motion::Right => move_selection(view, snapshot, selections, &seps, Op::CharNext, count),
        Motion::WordForward => {
            move_selection(view, snapshot, selections, &seps, Op::WordStartNext, count)
        }
        Motion::WordForwardBlank => {
            move_selection(view, snapshot, selections, BLANK_SEPS, Op::WordStartNext, count)
        }
        Motion::WordBack => {
            move_selection(view, snapshot, selections, &seps, Op::WordStartPrev, count)
        }
        Motion::WordBackBlank => {
            move_selection(view, snapshot, selections, BLANK_SEPS, Op::WordStartPrev, count)
        }
        Motion::WordEnd => {
            move_selection(view, snapshot, selections, &seps, Op::WordEndNext, count)
        }
        Motion::WordEndBlank => {
            move_selection(view, snapshot, selections, BLANK_SEPS, Op::WordEndNext, count)
        }
        Motion::WordEndBack => {
            move_selection(view, snapshot, selections, &seps, Op::WordEndPrev, count)
        }
        Motion::WordEndBackBlank => {
            move_selection(view, snapshot, selections, BLANK_SEPS, Op::WordEndPrev, count)
        }
        Motion::LineStart => {
            move_selection(view, snapshot, selections, &seps, Op::ToLineHome, count)
        }
        Motion::LineStartNonBlank => {
            move_selection(view, snapshot, selections, &seps, Op::ToLineFirstNonBlank, count)
        }
        Motion::LineEnd => {
            move_selection(view, snapshot, selections, &seps, Op::ToLineEnd, count)
        }
        Motion::SentencePrev => {
            move_selection(view, snapshot, selections, &seps, Op::LinePrev, count)
        }
        Motion::SentenceNext => {
            move_selection(view, snapshot, selections, &seps, Op::LineNext, count)
        }
        Motion::ParagraphPrev => {
            move_selection(view, snapshot, selections, &seps, Op::ParagraphPrev, count)
        }
        Motion::ParagraphNext => {
            move_selection(view, snapshot, selections, &seps, Op::ParagraphNext, count)
        }
    }
}

fn move_selection(
    view: &ScreenImage,
    snapshot: &GridSnapshot,
    selections: &mut Selections,
    seps: &str,
    op: Op,
    count: usize,
) {
    // 上限防止 `999999999h` 把主线程卡死；Vim 自己也有类似的实际限制。
    for _ in 0..count.min(1000) {
        apply_cmd_with_blocks_seps(
            Cmd::Move(op),
            view,
            &snapshot.blocks,
            selections,
            seps,
        );
    }
}

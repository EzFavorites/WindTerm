//! Vim 本地模式意图的落地：只移动视口 / 选区、取文本、写剪贴板，绝不写 PTY。
//!
//! 纯解析在 `term_app::vim`；这里把 motion 翻译成已有视口与选区 API，所以
//! 「Vim 的 j」和「方向键」走同一条滚动路径。复制键先扩选区再取文本，
//! 随后把选区还原成 yank 前的样子（Vim 的 y 不动光标）。粘贴返回意图给
//! `pane.rs`，由它复用 Cmd+V 的确认/写入漏斗。

use super::pane_pointer::visible_separators;
use eframe::egui;
use std::borrow::Cow;
use term_app::selection::{apply_cmd_with_blocks_seps, Cmd, Op, Selections};
use term_app::vim::{Motion, VimAction};
use term_app::viewport::Viewport;
use term_core::grid::{GridSnapshot, ScreenImage};
use term_core::session::SessionConfig;

const BLANK_SEPS: &str = " \t";

/// 粘贴意图。`remote=true` 的 `p` 粘完回远端；`P` 留在本地（调用方粘完再切回）。
pub(super) struct VimPaste {
    pub text: String,
    pub remote: bool,
    pub alt: bool,
}

/// 把这一帧的字符逐个喂给纯解析层。返回这一帧是否攒出了一次粘贴。
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
) -> Option<VimPaste> {
    let mut to_paste = None;
    for ch in keys {
        let Some(action) = vim.feed(ch) else { continue };
        match action {
            VimAction::Motion(motion, count) => {
                apply_motion(viewport, view, snapshot, selections, config, motion, count)
            }
            VimAction::Yank { motion, count, register } => {
                let seps = visible_separators(config, snapshot.alt).to_string();
                let before = selections.active();
                if let Some((op, s)) = motion_step(motion, &seps) {
                    move_selection(view, snapshot, selections, &s, op, count);
                }
                finish_yank(selections, view, vim, register, before, ctx);
            }
            VimAction::YankLine { count, register } => {
                let seps = visible_separators(config, snapshot.alt).to_string();
                let before = selections.active();
                move_selection(view, snapshot, selections, &seps, Op::Line, count);
                finish_yank(selections, view, vim, register, before, ctx);
            }
            VimAction::YankToChar { before, ch, count, register } => {
                let seps = visible_separators(config, snapshot.alt).to_string();
                let old = selections.active();
                let n = chars_until(view, selections, ch, before).unwrap_or(1) * count.max(1);
                move_selection(view, snapshot, selections, &seps, Op::CharNext, n);
                finish_yank(selections, view, vim, register, old, ctx);
            }
            VimAction::Paste { text_from, remote } => {
                let text = vim
                    .register_text(text_from)
                    .or_else(|| vim.register_text(term_app::vim::UNNAMED_REGISTER))
                    .unwrap_or("")
                    .to_string();
                if !text.is_empty() {
                    to_paste = Some(VimPaste { text, remote, alt: snapshot.alt });
                }
            }
        }
        ctx.request_repaint();
    }
    to_paste
}

/// 取完文本后恢复光标/选区：y 是纯复制，不该把扩出来的选区留在屏上。
fn finish_yank(
    selections: &mut Selections,
    view: &ScreenImage,
    vim: &mut term_app::vim::VimState,
    register: char,
    before: Option<term_app::selection::Selection>,
    ctx: &egui::Context,
) {
    let text = selections.text(view, &[]);
    if !text.is_empty() {
        vim.set_register(register, text.clone());
        // `"+y` 才写系统剪贴板；未命名寄存器也顺手写，和普通 Cmd+C 一致。
        if register == term_app::vim::CLIPBOARD_REGISTER
            || register == term_app::vim::UNNAMED_REGISTER
        {
            ctx.copy_text(text);
        }
    }
    selections.set_active(before);
}

/// 从活动选区的头部找到目标字符，返回要跨过的格数。`before=false`（yf）
/// 要多走一格把字符本身也包含进来。
fn chars_until(
    view: &ScreenImage,
    selections: &Selections,
    target: char,
    before: bool,
) -> Option<usize> {
    let head = selections.active()?.head();
    let line = view.line_at(head.row)?;
    let idx = line
        .cells
        .iter()
        .enumerate()
        .skip(head.col)
        .find(|(_, c)| c.ch == target)
        .map(|(i, _)| i)?;
    Some((idx - head.col + usize::from(!before)).max(1))
}

/// 视口类 motion 走 `Viewport`，其余走选区。nG 的 n>1 落在会话第 n 行。
#[allow(clippy::too_many_arguments)]
fn apply_motion(
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
        Motion::LastLine => {
            let row = if count <= 1 { 0 } else { (-history + n - 1).clamp(-history, 0) };
            viewport.jump_to_row(row, history);
        }
        _ => {
            if let Some((op, s)) = motion_step(motion, &seps) {
                move_selection(view, snapshot, selections, &s, op, count);
            }
        }
    }
}

/// motion → 选区命令 + 分词符。`FirstLine/LastLine` 在选区里退化为文首/文末，
/// 只有普通导航路径才去动视口。
fn motion_step<'a>(motion: Motion, seps: &'a str) -> Option<(Op, Cow<'a, str>)> {
    let step = |op, s: &'a str| Some((op, Cow::Borrowed(s)));
    match motion {
        Motion::Left => step(Op::CharPrev, seps),
        Motion::Right => step(Op::CharNext, seps),
        Motion::Up => step(Op::LinePrev, seps),
        Motion::Down => step(Op::LineNext, seps),
        Motion::FirstLine => step(Op::ToDocumentStart, seps),
        Motion::LastLine => step(Op::ToDocumentEnd, seps),
        Motion::WordForward => step(Op::WordStartNext, seps),
        Motion::WordForwardBlank => step(Op::WordStartNext, BLANK_SEPS),
        Motion::WordBack => step(Op::WordStartPrev, seps),
        Motion::WordBackBlank => step(Op::WordStartPrev, BLANK_SEPS),
        Motion::WordEnd => step(Op::WordEndNext, seps),
        Motion::WordEndBlank => step(Op::WordEndNext, BLANK_SEPS),
        Motion::WordEndBack => step(Op::WordEndPrev, seps),
        Motion::WordEndBackBlank => step(Op::WordEndPrev, BLANK_SEPS),
        Motion::LineStart => step(Op::ToLineHome, seps),
        Motion::LineStartNonBlank => step(Op::ToLineFirstNonBlank, seps),
        Motion::LineEnd => step(Op::ToLineEnd, seps),
        Motion::SentencePrev => step(Op::LinePrev, seps),
        Motion::SentenceNext => step(Op::LineNext, seps),
        Motion::ParagraphPrev => step(Op::ParagraphPrev, seps),
        Motion::ParagraphNext => step(Op::ParagraphNext, seps),
        Motion::PromptPrev => step(Op::CommandPrev, seps),
        Motion::PromptNext => step(Op::CommandNext, seps),
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
        apply_cmd_with_blocks_seps(Cmd::Move(op), view, &snapshot.blocks, selections, seps);
    }
}

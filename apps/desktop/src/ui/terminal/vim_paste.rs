//! Vim `p` / `P` 粘贴的**单一漏斗**（从 `pane.rs` 拆出：那边的代码行已顶到上限）。
//!
//! `Tab::write_paste` 在只读查看态（`local_mode=true`）第一行就返回，所以本地模式里
//! 敲 `p` / `P` 攒出的粘贴意图，必须先临时清掉 `local_mode` 才能送出字节，落地之后
//! 再恢复。两条路径共用这一处翻转：
//!
//! - 不弹确认：当帧清 → `commit_paste` → 恢复；
//! - 弹确认：**不在点击 Yes 之前**翻转。`PendingPaste.restore_local` 把「粘完要恢复」
//!   记进那条待办，用户在弹窗里点 Yes 时才由 `confirm_paste` 调同一个漏斗。
//!
//! 非 Vim 的 Cmd+V / 右键 / 中键从不带 `restore_local`，走原来的直通 `commit_paste`。

use crate::ui::dialogs::confirm_paste::{paste_needs_confirm, PendingPaste};
use crate::ui::*;
use super::session::PasteWrite;
use super::tab_slot::{tab_of, TabSlot};

/// 对一个标签执行一次带「临时离开只读」的粘贴。
///
/// 只动 `local_mode` 这一个字段：输入行、命令行历史与编码全走 `Tab::write_paste`。
/// `restore_local=false` 时连这一个字段都不碰（普通粘贴）。
pub(in crate::ui) fn commit_for_tab(tab: &mut Tab, text: &str, restore_local: bool) -> PasteWrite {
    if restore_local {
        tab.local_mode = false;
    }
    let written = tab.write_paste(text);
    if restore_local {
        tab.local_mode = true;
    }
    written
}

/// 带窗口级副作用的落地：翻转 + `commit_paste`（命令行历史在里面写）+ 翻转回来。
///
/// 标签在弹窗期间被关掉时 `tab_of` 拿不到，两次翻转与写入都自然跳过，不 panic。
pub(in crate::ui) fn commit_with_local_cleared(
    win: &mut TerminalWindow,
    slot: TabSlot,
    text: &str,
    restore_local: bool,
) {
    if restore_local {
        if let Some(tab) = tab_of(&mut win.tabs, &mut win.shell_tabs, slot) {
            tab.local_mode = false;
        }
    }
    win.commit_paste(slot, text);
    if restore_local {
        if let Some(tab) = tab_of(&mut win.tabs, &mut win.shell_tabs, slot) {
            tab.local_mode = true;
        }
    }
}

/// 帧末统一落地：`pending_ip_hit` 里的 `RunCommand` 不再随临时 `Vec` 一起丢掉，
/// 而是收集起来交窗口级 `dispatch`；Vim 粘贴与普通粘贴走同一个确认漏斗。
pub(in crate::ui) fn finish_actions(
    win: &mut TerminalWindow,
    slot: TabSlot,
    vim: Option<(String, bool)>,
    paste: Option<(String, bool)>,
) {
    // `pending_ip_hit` 就是帧间的待触发槽：它按槽位写、帧末在这里取。
    let mut run_commands: Vec<super::trigger_dispatch::CommandCall> = Vec::new();
    let mut note: Option<String> = None;
    if let Some(hit) = win.pending_ip_hit.take() {
        if let Some(tab) = tab_of(&mut win.tabs, &mut win.shell_tabs, slot) {
            for action in &hit.actions {
                if let Some(core) = super::trigger_menu::to_core_action(action) {
                    let effect = term_core::automation::Effect {
                        rule: hit.rule_id.clone(),
                        action: core,
                        span: None,
                        release: term_core::automation::Release::Immediate,
                    };
                    // 交互命中不等门控：当帧落地，转义错误进本帧 note。
                    tab.dispatch_effect(&effect, &mut run_commands, &mut note, false);
                }
            }
            crate::ui::trigger_intents::drain_tab(
                tab,
                super::session::now_secs(),
                None,
                &mut run_commands,
                &mut note,
            );
        }
    }
    flush_trigger_note(win, &mut note);
    // `p`（remote=true）与 `P`（remote=false）都走同一条漏斗：只有「要不要恢复」
    // 这一种后续状态，不再按 remote 分叉（两条路径都必须先离开只读才能写入）。
    let (text, alt, restore_local) = match vim {
        Some((text, alt)) => (text, alt, true),
        None => match paste {
            Some((text, alt)) => (text, alt, false),
            None => {
                dispatch_all(win, run_commands);
                return;
            }
        },
    };
    if paste_needs_confirm(alt, win.paste_confirm_main, win.paste_confirm_alt) {
        win.pending_paste = Some(PendingPaste { tab: slot, text, restore_local });
    } else if restore_local {
        commit_with_local_cleared(win, slot, &text, true);
    } else {
        win.commit_paste(slot, &text);
    }
    dispatch_all(win, run_commands);
}

/// 触发器转义写错的提示：帧末统一交回窗口，与帧首的 `sync_prefs_rev` 走
/// **同一个** `commit_note`。取走而不是克隆，保证同一条提示只显示一次；
/// 错误类同时抬到前景 Notice，状态栏被连接 / 只读标签占住时也看得见。
/// 过去直接写 `log_note`，转义错误既进不了前景条，还会吃掉帧首那条错误。
fn flush_trigger_note(win: &mut TerminalWindow, note: &mut Option<String>) {
    win.commit_note(note.take());
}

fn dispatch_all(win: &mut TerminalWindow, calls: Vec<super::trigger_dispatch::CommandCall>) {
    for call in calls {
        win.dispatch_call(&call);
    }
}

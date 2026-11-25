//! `p` / `P` × 粘贴确认开 / 关的四象限回归。
//!
//! 两条轴各有两种取值，缺任何一格都可能是「p 是 no-op」或「弹窗还没点 Yes 就
//! 已经翻转只读态」这类只在组合下显形的故障。判据读的是真实 `Tab.write_paste`
//! 的输入行与 `local_mode`，不是某个与生产路径平行的复印件。

use super::vim_paste::{commit_with_local_cleared, finish_actions};
use crate::ui::lifecycle_test_support::window_with;
use crate::ui::terminal::setup::tab_from;
use crate::ui::terminal::TabSlot;
use crate::ui::*;

fn fixture(confirm_main: bool) -> TerminalWindow {
    let pool = std::sync::Arc::new(term_session::SessionPool::new());
    let ctx = egui::Context::default();
    let mut tab = tab_from(
        &pool,
        SessionConfig::new(Protocol::Shell),
        None,
        egui::Id::new("vim_paste_four_quadrants"),
        &ctx,
    );
    // 夹具没有会话，`write_paste` 仍会编码并喂输入行，足够验漏斗。
    tab.local_mode = true;
    let mut win = window_with(pool, &ctx, vec![tab]);
    win.paste_confirm_main = confirm_main;
    win.paste_confirm_alt = false;
    win
}

fn local_mode(win: &TerminalWindow) -> bool {
    win.tabs[0].local_mode
}

fn input(win: &TerminalWindow) -> String {
    win.tabs[0].input_line.current()
}

/// 四象限：remote ∈ {p, P} × confirm ∈ {开, 关}。
///
/// 每一格都要满足三件事：粘完回到只读、输入行收到全文、确认开时点 Yes 之前
/// 一律不翻转（否则弹窗还在屏幕上，字节已经写出去了）。
#[test]
fn p_and_p_both_commit_and_restore_local_mode_with_confirm_on_or_off() {
    for remote in [true, false] {
        for confirm in [true, false] {
            let mut win = fixture(confirm);
            let identity = win.tabs[0].identity;
            let text = format!("echo {}remote", if remote { "p-" } else { "P-" });
            finish_actions(
                &mut win,
                TabSlot::Main(identity),
                Some((text.clone(), false)),
                None,
            );
            if confirm {
                assert!(win.pending_paste.is_some(), "remote={remote}: 确认开必须挂弹窗");
                assert!(local_mode(&win), "remote={remote}: 点 Yes 之前不许离开只读");
                assert_eq!(input(&win), "", "remote={remote}: 没确认就不许写输入行");
                let pending = win.pending_paste.take().expect("夹具里刚挂上");
                assert!(pending.restore_local, "Vim 粘贴必须带上恢复标志");
                commit_with_local_cleared(&mut win, pending.tab, &pending.text, pending.restore_local);
            } else {
                assert!(win.pending_paste.is_none(), "remote={remote}: 确认关就直通");
            }
            assert!(local_mode(&win), "remote={remote}/confirm={confirm}: 粘完回到只读");
            assert!(input(&win).contains("remote"), "remote={remote}/confirm={confirm}: 文本真的落地");
        }
    }
}

/// 普通粘贴没有恢复标志：直通路径一个字段都不碰（只读态本就不发粘贴事件）。
#[test]
fn ordinary_paste_never_carries_the_restore_flag() {
    let mut win = fixture(false);
    let identity = win.tabs[0].identity;
    finish_actions(
        &mut win,
        TabSlot::Main(identity),
        None,
        Some(("echo ordinary\r".to_string(), false)),
    );
    assert!(win.pending_paste.is_none());
    assert!(local_mode(&win), "普通粘贴不翻转 local_mode：夹具是只读则仍是只读");
    assert_eq!(input(&win), "", "只读态普通粘贴本来就不会到达漏斗");
}

/// 真实调用形态：帧末 `finish_actions` 取走 `pending_ip_hit`，转义错误攒进
/// 局部 note，再由 `flush_trigger_note` 交回窗口。过去它直接写 `log_note`，
/// 错误进不了前景条；现在必须走 `commit_note`，两条通道都看得见。
#[test]
fn a_bad_escape_note_from_the_ip_hit_reaches_the_foreground_notice() {
    let mut win = fixture(false);
    win.pending_ip_hit = Some(term_core::trigger::Hit {
        rule_id: "ip-hit".into(),
        actions: vec![term_core::trigger::TriggerAction::SendText(r"\x0".into())],
        matched: String::new(),
    });
    let slot = TabSlot::Main(win.tabs[0].identity);
    finish_actions(&mut win, slot, None, None);

    let note = win.log_note.as_deref().expect("转义错误不能静默丢掉");
    assert!(note.contains("转义写错"), "note={note:?}");
    assert!(
        win.error_notice_text()
            .is_some_and(|t| t.contains("转义写错")),
        "flush_trigger_note 必须经 commit_note，错误同时进前景 Notice"
    );
    // 本帧的局部缓冲已经被 take 走：后续空提交不会再抬一次。
    let before = win.error_notice_text().map(str::to_string);
    win.commit_note(None);
    assert_eq!(win.error_notice_text().map(str::to_string), before);
}

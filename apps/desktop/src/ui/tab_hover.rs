//! 悬停激活标签：计时器 + 纯判据。
//!
//! 只有「用户把指针停在某条标签上足够久」才切换：点击、拖动中的标签不算，
//! 已经激活的标签也不反复派发。时钟用 egui 的帧时钟，不读墙钟。

use eframe::egui;
use term_core::session_runtime::SessionId;

use crate::ui::*;

/// 纯判据：`None` = 不切换；`Some(true)` = 切到这条；`Some(false)` = 只记录起点。
pub(super) fn decide(
    enabled: bool,
    hovered: bool,
    active: bool,
    dragging: bool,
    same: bool,
    elapsed_ms: f64,
    delay_ms: u32,
) -> Option<bool> {
    if !enabled || !hovered || active || dragging {
        return None;
    }
    if same && elapsed_ms >= f64::from(delay_ms) {
        Some(true)
    } else if same {
        None
    } else {
        Some(false)
    }
}

/// 一帧一条标签的悬停判定。`slot` 是标签下标，`session` 是稳定身份。
pub(in crate::ui) fn note_hover(
    win: &mut TerminalWindow,
    ui: &egui::Ui,
    slot: usize,
    session: SessionId,
    hovered: bool,
    active: bool,
    dragging: bool,
) {
    if !win.tab_hover_activate {
        win.tab_hover = None;
        return;
    }
    let now = ui.input(|i| i.time);
    let same = win.tab_hover.is_some_and(|(id, _)| id == session);
    let elapsed_ms = win
        .tab_hover
        .filter(|(id, _)| *id == session)
        .map_or(0.0, |(_, since)| (now - since) * 1000.0);
    match decide(
        true,
        hovered,
        active,
        dragging,
        same,
        elapsed_ms,
        win.tab_hover_delay_ms,
    ) {
        Some(true) => {
            win.tab_hover = None;
            win.set_active_slot(slot);
        }
        Some(false) => win.tab_hover = Some((session, now)),
        None => win.tab_hover = None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_hover_only_switches_after_the_delay_and_never_while_dragging() {
        assert_eq!(decide(true, true, false, false, false, 0.0, 200), Some(false));
        assert_eq!(decide(true, true, false, false, true, 199.0, 200), None);
        assert_eq!(decide(true, true, false, false, true, 200.0, 200), Some(true));
        assert_eq!(decide(true, true, false, true, true, 500.0, 200), None);
        assert_eq!(decide(true, true, true, false, true, 500.0, 200), None);
        assert_eq!(decide(false, true, false, false, true, 500.0, 200), None);
        assert_eq!(decide(true, false, false, false, true, 500.0, 200), None);
    }
}

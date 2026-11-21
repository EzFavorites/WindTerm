//! 空闲自动锁屏：最后一帧真实输入之后超过会话配置的窗口就锁。
//!
//! 「真实输入」只认指针移动 / 按下 / 按键 / 文本 / 滚轮，不认
//! `pointer.hover_pos()`——指针停在窗口里不动时那个值每帧都在，拿它当活动
//! 会让超时永不触发。时钟用 `ui.input.time`（egui 的单调帧时钟），不在 UI
//! 里读墙钟。状态本身仍由 `lock::reduce` 写，这里只派发意图。

use crate::ui::*;
use eframe::egui;
use term_app::lock::LockAction;

/// 这一帧有没有用户活动。
fn activity(input: &egui::InputState) -> bool {
    if input.pointer.any_down() || input.raw_scroll_delta != egui::Vec2::ZERO {
        return true;
    }
    input.events.iter().any(|e| {
        matches!(
            e,
            egui::Event::PointerMoved(_)
                | egui::Event::Key { pressed: true, .. }
                | egui::Event::Text(_)
        )
    })
}

/// 纯超时判据：`0` = 关闭；时钟回拨（新帧比上一帧早）时不误锁。
fn timed_out(time: f64, last: f64, timeout_secs: u32) -> bool {
    timeout_secs > 0 && time >= last && time - last >= f64::from(timeout_secs)
}

impl TerminalWindow {
    /// 在 `draw()` 开头调用：锁屏之前先判定是否该自动锁。
    pub(in crate::ui) fn poll_lock_timeout(&mut self, ctx: &egui::Context) {
        if self.view.lock.locked || self.lock_timeout_secs == 0 {
            return;
        }
        let (time, active) = ctx.input(|i| (i.time, activity(i)));
        if active {
            self.last_lock_activity = time;
        }
        if timed_out(time, self.last_lock_activity, self.lock_timeout_secs) {
            self.dispatch_action(Action::Lock(LockAction::Lock));
            // 锁上之后时钟还在走；活动点挪到当前，避免解锁前反复派发。
            self.last_lock_activity = time;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_zero_timeout_never_locks() {
        assert!(!timed_out(100.0, 0.0, 0));
    }

    #[test]
    fn the_window_opens_only_after_the_full_idle_span() {
        assert!(!timed_out(9.9, 0.0, 10));
        assert!(timed_out(10.0, 0.0, 10));
        // 时钟回拨不触发（帧时钟本不该回拨，但夹具可能钉成 0）。
        assert!(!timed_out(-1.0, 0.0, 10));
    }
}

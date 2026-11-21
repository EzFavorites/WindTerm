//! 自动重连的退避策略（纯函数）。
//!
//! 断开边沿才排一次尝试；连上就清零。第 5 次之后不再排——无限重试会让
//! 一个填错地址的会话永远占着界面，用户还以为它在「努力连」。

use crate::ui::*;

/// 前四次等待秒数：1 / 2 / 4 / 8。
const BACKOFF_SECS: [u64; 4] = [1, 2, 4, 8];

/// `attempt` 是**本次断开已经退避过几次**（从 0 开始）。
/// 返回 `None` = 超过上限，不再自动重连。
pub(super) fn backoff(attempt: u8) -> Option<u64> {
    let index = usize::from(attempt);
    (index < BACKOFF_SECS.len()).then(|| BACKOFF_SECS[index])
}

impl TerminalWindow {
    /// 每帧一次：到点的断开标签在原位重连，不改活动标签。
    ///
    /// 放在所有 `Tab::poll` 之后调用：连上边沿会在那里清零退避；几行代码
    /// 留在这里而不是 `ui/mod.rs` 是为了守住那个文件的单文件行数门禁。
    pub(in crate::ui) fn poll_auto_reconnect(&mut self) {
        if !self.auto_reconnect {
            return;
        }
        let now = crate::now_secs();
        let due: Vec<usize> = (0..self.tabs.len())
            .filter(|i| self.tabs[*i].auto_reconnect_at.is_some_and(|at| now >= at))
            .collect();
        for i in due {
            self.tabs[i].auto_reconnect_at = None;
            self.reconnect_tab_at(i, false);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_first_four_attempts_back_off_then_stop() {
        assert_eq!(backoff(0), Some(1));
        assert_eq!(backoff(1), Some(2));
        assert_eq!(backoff(2), Some(4));
        assert_eq!(backoff(3), Some(8));
        assert_eq!(backoff(4), None);
        assert_eq!(backoff(u8::MAX), None);
    }
}

//! 内置触发器 `LocalSendLine` 动作的帧末落地。
//!
//! 绘制期只产出 `Hit`（经 `Tab::apply_trigger` 执行），本地命令落到每条标签的
//! `pending_local_sends` 队列；这里在帧末统一收拢，写进底带 Shell 面板的活动标签。
//! 远端命令不经过这里——它已由同一执行器的 `SendLine` 分支写进当前会话 PTY。

use crate::ui::TerminalWindow;

impl TerminalWindow {
    pub(in crate::ui) fn flush_local_sends(&mut self) {
        let mut lines = Vec::new();
        for tab in self.tabs.iter_mut().chain(self.shell_tabs.iter_mut()) {
            lines.append(&mut tab.pending_local_sends);
        }
        if lines.is_empty() {
            return;
        }
        let idx = self.shell_active_index();
        if let Some(tab) = self.shell_tabs.get_mut(idx) {
            for line in lines {
                tab.write(line.as_bytes());
            }
        }
    }
}

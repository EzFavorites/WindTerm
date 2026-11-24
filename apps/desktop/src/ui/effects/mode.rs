//! `Mode ▸ Local/Remote Mode` 的落点：两个方向共用 `Tab.local_mode` 一位。
//! 单独成文件是为了不让 `effects.rs` 顶过 500 代码行门禁。

use crate::ui::TerminalWindow;

impl TerminalWindow {
    /// `Window ▸ Local Mode`：把活动标签切成只读查看 / 恢复可输入。
    ///
    /// 只翻 `Tab.local_mode` 一位，不写盘。四条判据（不写 PTY / 不自动滚动 /
    /// 触发器不执行 / 脚本不执行）全是读这一位的消费者，这里不能多写第二份。
    /// 进入只读时取消本窗口正在跑的脚本——脚本拿的是 `SessionPool`，不读 `Tab`，
    /// 不取消的话「脚本不执行」这句话就是假的。
    pub(in crate::ui) fn toggle_local_mode(&mut self) {
        let active = self.active_slot();
        let now_local = {
            let Some(tab) = self.tabs.get_mut(active) else {
                return;
            };
            tab.local_mode = !tab.local_mode;
            tab.vim.reset();
            tab.local_mode
        };
        if now_local {
            if let Some(run) = self.script.as_ref().and_then(|dlg| dlg.run.as_ref()) {
                run.cancel();
            }
            for run in &self.logon_runs {
                run.cancel();
            }
            // 停靠面板与编辑器各有自己的运行句柄，也必须一起停；它们拿的是
            // SessionPool，不经 Tab::write，不在这里取消就会绕过只读。
            for tab in &mut self.script_panel.tabs {
                if let Some(run) = tab.state.run.as_ref() {
                    run.cancel();
                }
            }
            if let Some(state) = self.script_editor.as_ref() {
                if let Some(run) = state.run.as_ref() {
                    run.cancel();
                }
            }
            self.log_note = Some(
                "Local mode: this session is read-only — no input, no auto-scroll, triggers and \
                 scripts paused."
                    .to_string(),
            );
        } else {
            self.log_note = Some("Local mode off — input restored.".to_string());
        }
        self.ctx.request_repaint();
    }

    /// `Mode ▸ Remote Mode`：退出只读。与上面共用同一位，单向清成 false；
    /// 已是远端态时空操作。菜单里 `CheckState::Live` 的勾由 `local_mode_on` 现读。
    pub(in crate::ui) fn set_remote_mode(&mut self) {
        let active = self.active_slot();
        let Some(tab) = self.tabs.get_mut(active) else {
            return;
        };
        if !tab.local_mode {
            return;
        }
        tab.local_mode = false;
        tab.vim.reset();
        self.log_note = Some("Remote mode — input restored.".to_string());
        self.ctx.request_repaint();
    }

    /// `Text.ToggleWordWrap`：翻活动标签的 `Tab.word_wrap` 一位。
    ///
    /// 只记「用户点了」，不改段表、不碰内核、不落盘——段表每帧在 `pane.rs`
    /// 按这一位现算（关 = 恒等映射、零像素；开 = 窄列 + 裁 padding）。
    pub(in crate::ui) fn toggle_word_wrap(&mut self) {
        let active = self.active_slot();
        let Some(tab) = self.tabs.get_mut(active) else {
            return;
        };
        tab.word_wrap = !tab.word_wrap;
        self.ctx.request_repaint();
    }
}

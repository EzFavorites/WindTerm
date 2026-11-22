//! 整窗主题切换的落点：加载调色板 + 落盘偏好 + 请求重绘。
//! 单独一个文件是为了不让 `effects.rs` 顶过 500 代码行门禁。

use crate::ui::dialogs::save_prefs;
use crate::ui::TerminalWindow;

impl TerminalWindow {
    /// 切换整窗主题：重新加载调色板并让下一帧用新色重画。偏好落盘，
    /// 重启后仍用用户选的那套；没有第二份「当前主题名」散在 UI 里。
    pub(in crate::ui) fn apply_theme(&mut self, name: &str) {
        self.palette = term_import::theme::load_palette_named(name);
        save_prefs(|prefs| prefs.theme_name = name.to_string());
        self.ctx.request_repaint();
    }
}

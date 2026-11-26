//! 整窗主题与终端配色方案（16 色 ANSI）的落点：加载调色板 + 落盘偏好 + 请求重绘。
//! 单独一个文件是为了不让 `effects.rs` 顶过 500 代码行门禁。

use crate::ui::dialogs::save_prefs;
use crate::ui::TerminalWindow;

/// 启动时选哪份调色板：用户在 `View ▸ Syntax Schemes` 选过的方案优先，
/// 没选过才走整窗主题。两份偏好互不顶替——scheme 只决定终端格子的 16 色，
/// theme 还决定窗口 chrome；把 scheme 写进 `theme_name` 会让设置页显示错名字。
pub(in crate::ui) fn startup_palette(
    scheme_name: &str,
    theme_name: &str,
) -> term_core::theme::Palette {
    term_core::theme::builtin_scheme(scheme_name)
        .unwrap_or_else(|| term_import::theme::load_palette_named(theme_name))
}

impl TerminalWindow {
    /// 切换整窗主题：重新加载调色板并让下一帧用新色重画。偏好落盘，
    /// 重启后仍用用户选的那套；没有第二份「当前主题名」散在 UI 里。
    pub(in crate::ui) fn apply_theme(&mut self, name: &str) {
        self.palette = term_import::theme::load_palette_named(name);
        save_prefs(|prefs| prefs.theme_name = name.to_string());
        self.ctx.request_repaint();
    }

    /// `View ▸ Syntax Schemes` 的七条命令：按名字取内置的 16 色方案，
    /// 翻运行中调色板并 load-modify-save 写回 `Prefs.scheme_name`（重启仍生效）。
    /// 未知名字**什么也不写**——builtin_scheme 返回 `None`，不拿默认主题冒充；
    /// 菜单 ID 由 parse 固定，真走到这里说明表名与菜单漂移了。
    pub(in crate::ui) fn apply_scheme(&mut self, name: &str) {
        let Some(palette) = term_core::theme::builtin_scheme(name) else {
            return;
        };
        self.palette = palette;
        save_prefs(|prefs| prefs.scheme_name = name.to_string());
        self.ctx.request_repaint();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_chosen_scheme_beats_the_window_theme_at_startup() {
        let palette = startup_palette("linux", "dige-black");
        assert_eq!(palette.ansi_source, "built-in scheme 'linux'");
        assert_eq!(palette.theme_name, "linux");
    }

    #[test]
    fn no_scheme_falls_back_to_the_window_theme() {
        let palette = startup_palette("", "dige-black");
        assert_ne!(palette.ansi_source, "built-in scheme ''");
        assert_eq!(palette.theme_name, "dige-black");
    }
}

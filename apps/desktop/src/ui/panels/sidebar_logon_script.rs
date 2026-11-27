//! 会话树右键「Logon Script…」的专用编辑框。
//!
//! 会话表单（`dialogs/session_form.rs`）里的同名字段刻意不摆到 1:1 表单上，
//! 那条 24 页的 PNG 基线不能动；这里另开一个小对话框，只编辑**存储的会话**：
//! 右键会话行 → 填路径 → Ok 写回 `sessions[i].logon_script` 并落盘。
//!
//! 语义只有一档：**下一次连接才生效**。已开标签的 `tab.config` 是建标签时
//! 的快照，`fire_logon_scripts()` 读的就是那份快照；`logon_fired` 又是按
//! `SessionId` 记的一次性事实。当场改活标签既不会重跑脚本，又会把这笔
//! 一次性记账搞乱，所以这里只碰磁盘上的会话配置，两者都不动。

use crate::ui::dialogs::dialog_chrome::framed_button;
use crate::ui::kit::window::{DialogId, DialogWindow};
use crate::ui::*;
use term_core::session::SessionConfig;

/// 跨帧草稿。会话**名字**就是这里的身份：它跟着 `SessionConfig` 一起存盘，
/// 不像右键时的行下标那样会被排序 / 删除 / 连接带偏。
#[derive(Default)]
pub(in crate::ui) struct State {
    name: String,
    path: String,
    error: Option<String>,
}

/// 纯写回助手：按名字找会话，命中才写 `logon_script`（去首尾空白）。
/// 只改 `sessions` 里那份存储配置——**不**patch 活 `tab.config`，**不**碰
/// `logon_fired`：编辑只对下一次连接生效。返回是否命中。
pub(in crate::ui) fn update_logon_script_by_name(
    sessions: &mut [SessionConfig],
    name: &str,
    path: &str,
) -> bool {
    let Some(cfg) = sessions.iter_mut().find(|cfg| cfg.name == name) else {
        return false;
    };
    cfg.logon_script = path.trim().to_string();
    true
}

impl TerminalWindow {
    /// 从会话树右键菜单进入。已开标签同名也照旧按名字找存储配置。
    pub(in crate::ui) fn open_logon_script_editor(&mut self, index: usize) {
        let Some(cfg) = self.sessions.get(index) else {
            return;
        };
        self.logon_script_editor = Some(State {
            name: cfg.name.clone(),
            path: cfg.logon_script.clone(),
            error: None,
        });
        self.ctx.request_repaint();
    }

    /// 模态编辑框。默认 `None`，一行都不画，视觉基线不受影响。
    pub(in crate::ui) fn draw_logon_script_editor(&mut self, ctx: &egui::Context) {
        let Some(mut state) = self.logon_script_editor.take() else {
            return;
        };
        let lang = self.lang();
        let outcome = DialogId::LogonScript.keys().poll(ctx);
        let mut close = false;

        let open = DialogWindow::new(DialogId::LogonScript, t(lang, "Logon Script…")).show(ctx, |ui| {
            // 会话名是数据，不是待翻译的 chrome。
            ui.label(state.name.as_str());
            ui.label(t(lang, "Logon script:"));
            ui.add_sized(
                egui::vec2(360.0, 20.0),
                egui::TextEdit::singleline(&mut state.path),
            );
            // 下次连接才生效——别让用户以为改一下当前会话就会重跑脚本。
            ui.weak(t(lang, "Takes effect on the next connect"));
            if let Some(error) = &state.error {
                ui.colored_label(egui::Color32::from_rgb(0xE0, 0x60, 0x60), error);
            }
            ui.separator();
            ui.horizontal(|ui| {
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if framed_button(ui, t(lang, "Ok"), true, true).clicked() {
                        if update_logon_script_by_name(&mut self.sessions, &state.name, &state.path) {
                            self.save_sessions();
                            close = true;
                        } else {
                            state.error = Some(t(lang, "Session not found").to_string());
                        }
                    }
                    if framed_button(ui, t(lang, "Cancel"), true, false).clicked() {
                        close = true;
                    }
                });
            });
        });

        if outcome.cancel || !open {
            close = true;
        }
        if close {
            self.ctx.request_repaint();
        } else {
            self.logon_script_editor = Some(state);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use term_core::session::Protocol;

    fn named(name: &str) -> SessionConfig {
        let mut cfg = SessionConfig::new(Protocol::Ssh);
        cfg.name = name.to_string();
        cfg
    }

    #[test]
    fn updates_only_the_named_stored_session_and_trims_the_path() {
        let mut sessions = vec![named("alpha"), named("beta")];
        sessions[1].logon_script = "old.rhai".to_string();
        assert!(update_logon_script_by_name(
            &mut sessions,
            "beta",
            "  scripts/next.rhai  "
        ));
        assert_eq!(sessions[1].logon_script, "scripts/next.rhai");
        assert!(sessions[0].logon_script.is_empty());
    }

    #[test]
    fn a_missing_session_name_is_a_no_op() {
        let mut sessions = vec![named("alpha")];
        sessions[0].logon_script = "keep.rhai".to_string();
        assert!(!update_logon_script_by_name(&mut sessions, "gone", "other.rhai"));
        assert_eq!(sessions[0].logon_script, "keep.rhai");
    }
}

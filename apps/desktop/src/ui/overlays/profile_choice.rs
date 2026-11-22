//! 首次启动的「Choose Profiles Directory」对话框。
//!
//! 这是**引导之前**的一步：先决定 profile 目录放在哪，再走
//! `onboarding.rs` 的 Welcome → Detect。选择结果写在平台配置目录下一个固定
//! 标记文件里（`term_paths::write_profile_dir_choice`），下次启动由
//! `term_paths::profile_dir()` 在算目录时先读它——标记不能放在用户选的目录里，
//! 否则「还没算出目录就要先读标记」会成环。
//!
//! 与其余覆盖层同一套受控契约：绘制期间只改本地副本，确认意图在闭包外落地。

use crate::ui::dialogs::dialog_chrome;
use crate::ui::kit::field::{browse_field, choice_row_styled, ChoiceKind, UnderlineStyle};
use crate::ui::kit::footer::widget_button;
use crate::ui::kit::spec::FooterButton;
use crate::ui::kit::window::{Chrome, DialogId, DialogWindow};
use crate::ui::path_picker::PathTarget;
use crate::ui::*;
use std::path::PathBuf;

/// 选中档位：0 = 应用程序目录，1 = 用户目录，2 = 指定目录。
/// 出厂选「用户目录」，与截图一致（也是唯一在所有平台都成立的一档）。
#[derive(Clone, Debug)]
pub(in crate::ui) struct ProfileChoiceState {
    pub option: usize,
    pub custom_dir: String,
    pub error: Option<String>,
}

impl Default for ProfileChoiceState {
    fn default() -> Self {
        Self {
            option: 1,
            custom_dir: String::new(),
            error: None,
        }
    }
}

/// 各档位的父目录。实际 profile 目录是它下面的 `.wind`（见截图里的橙色提示）。
fn option_parent(option: usize, custom: &str) -> Option<PathBuf> {
    match option {
        0 => {
            #[cfg(target_os = "macos")]
            {
                // 安装形态下的应用目录；开发机没装到 /Applications 时退回 exe 目录。
                let installed = PathBuf::from("/Applications/WindTerm.app/Contents/MacOS");
                if installed.is_dir() {
                    Some(installed)
                } else {
                    std::env::current_exe()
                        .ok()
                        .and_then(|p| p.parent().map(PathBuf::from))
                }
            }
            #[cfg(not(target_os = "macos"))]
            {
                std::env::current_exe()
                    .ok()
                    .and_then(|p| p.parent().map(PathBuf::from))
            }
        }
        1 => term_io::local_dir::home().into(),
        2 => {
            let p = PathBuf::from(custom.trim());
            (!p.as_os_str().is_empty()).then_some(p)
        }
        _ => None,
    }
}

/// 画一帧对话框。`show_profile_choice` 为假时零窗口、零 IO。
pub(in crate::ui) fn draw(ctx: &egui::Context, win: &mut TerminalWindow) {
    if !win.view.show_profile_choice {
        return;
    }
    let lang = win.view.lang;
    let mut option = win.profile_choice.option;
    let mut custom_dir = win.profile_choice.custom_dir.clone();
    let mut error = win.profile_choice.error.clone();
    let mut confirm = false;

    let app_dir = option_parent(0, "")
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_default();
    let home_dir = option_parent(1, "")
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_default();
    let labels = [
        format!("{}  –  {}", t(lang, "Application directory (A)"), app_dir),
        format!("{}  –  {}", t(lang, "User directory (H)"), home_dir),
        t(lang, "Specified directory: (S)").to_string(),
    ];
    let accs = ["Application", "User", "Specified"];

    let open = DialogWindow::new(DialogId::ProfileChoice, t(lang, "Choose Profiles Directory"))
        .with_chrome(Chrome::CHOOSER)
        .show_with_rect(ctx, |ui, rect| {
            ui.label(t(
                lang,
                "Please choose your personal profiles directory; it will be used to store user preferences.",
            ));
            ui.add_space(10.0);
            for (i, (label, acc)) in labels.iter().zip(accs).enumerate() {
                let mut on = option == i;
                if choice_row_styled(
                    ui,
                    ChoiceKind::Radio,
                    label,
                    acc,
                    &mut on,
                    true,
                    UnderlineStyle::Dialog,
                ) {
                    option = i;
                    error = None;
                }
                ui.add_space(8.0);
            }
            // 指定目录框始终可见（金标准里第三档下面就是它）；选中其它档位
            // 时它只是未使用的输入，不影响最终目录。
            let (_, browse) = browse_field(
                ui,
                &mut custom_dir,
                t(lang, "Directory path"),
                "...",
                24.0,
                40.0,
            );
            if browse
                .on_hover_text(t(lang, "Browse for the profiles directory"))
                .clicked()
            {
                win.open_path_picker(PathTarget::ProfileDir, &custom_dir);
            }
            ui.add_space(6.0);
            ui.colored_label(
                egui::Color32::from_rgb(0xF3, 0xBE, 0x25),
                t(lang, "A \".wind\" directory will be created in the selected directory."),
            );
            if let Some(err) = &error {
                ui.colored_label(egui::Color32::from_rgb(0xE0, 0x60, 0x60), err);
            }
            // OK 与其他弹窗的页脚同规格、同右下角边距。
            let spec = FooterButton::DialogOk.spec();
            let footer = egui::Rect::from_min_max(
                egui::pos2(rect.right() - 9.0 - spec.size.x, rect.bottom() - 9.0 - spec.size.y),
                egui::pos2(rect.right() - 9.0, rect.bottom() - 9.0),
            );
            confirm |= dialog_chrome::place(ui, footer, |ui| {
                widget_button(ui, "OK", spec, true)
            })
            .clicked();
        });
    let _ = open; // 深灰标题条没有关闭按钮，恒为 true。

    win.profile_choice.option = option;
    win.profile_choice.custom_dir = custom_dir.clone();
    win.profile_choice.error = error.clone();
    if confirm {
        win.confirm_profile_choice(option, &custom_dir);
    }
}

impl TerminalWindow {
    /// 确认选择：落盘标记 + 让当前进程立刻改用新目录，再继续首次引导。
    fn confirm_profile_choice(&mut self, option: usize, custom: &str) {
        let Some(parent) = option_parent(option, custom) else {
            self.profile_choice.error = Some(t(self.view.lang, "Choose a valid directory.").into());
            return;
        };
        let profile_dir = parent.join(".wind");
        if std::fs::create_dir_all(&profile_dir).is_err() {
            self.profile_choice.error =
                Some(t(self.view.lang, "The selected directory is not writable.").into());
            return;
        }
        if !term_paths::write_profile_dir_choice(&profile_dir) {
            self.profile_choice.error =
                Some(t(self.view.lang, "Failed to save the profile directory choice.").into());
            return;
        }
        // 进程内后续所有 `profile_dir()` 调用都读这个变量，无需重启。
        std::env::set_var("WINDTERM_PROFILE_DIR", &profile_dir);
        self.profile_choice.error = None;
        self.view.show_profile_choice = false;
        if self.view.show_onboarding {
            self.open_onboarding();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_is_the_user_directory() {
        let state = ProfileChoiceState::default();
        assert_eq!(state.option, 1);
        assert!(state.custom_dir.is_empty());
        assert!(state.error.is_none());
    }

    #[test]
    fn a_custom_directory_is_the_parent_of_the_profile_dir() {
        assert_eq!(
            option_parent(2, " /tmp/my profiles "),
            Some(PathBuf::from("/tmp/my profiles"))
        );
        assert_eq!(option_parent(2, "   "), None);
    }

    #[test]
    fn the_user_directory_always_resolves() {
        assert!(option_parent(1, "").is_some());
    }
}

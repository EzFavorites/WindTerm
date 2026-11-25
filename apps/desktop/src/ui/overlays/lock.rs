//! 锁屏遮罩（E-LOCK）。
//!
//! 视觉对齐原版：整窗暗底，只有一行蓝色 `Screen Locked` 和一个带小眼睛的密码
//! 输入框；Enter 提交，输错才在框下出一行红字。没有 emoji、说明段和按钮组——
//! 那些东西把「屏已锁」这件事淹没了。
//!
//! 语义仍复用 `term-vault` 的主密码：解锁时对 `vault.ztv` 做一次 `Vault::unlock`，
//! 成功才把内存里的保险库重新挂上。没建过库时输入任意非空口令即创建，Esc 是
//! **无库时**唯一的逃生口（有库时没有可见 Cancel，避免给人「点一下就不用密码」
//! 的错觉；本帧仍会先消费后台脚本的非键盘往返队列，见 `pre_update_gate.rs`）。

use crate::ui::*;
use term_app::lock::LockAction;
use term_runtime::persist;
use term_vault::Vault;

/// 原版锁屏标题的蓝，取自金图而非主题色——这是暗底上唯一的强调色。
const TITLE_BLUE: egui::Color32 = egui::Color32::from_rgb(0x4C, 0xA6, 0xFF);

impl TerminalWindow {
    /// 画在所有面板之上的全屏遮罩。`locked` 为真时其余 UI 都不画（见 `mod.rs`）。
    pub(in crate::ui) fn draw_lock(&mut self, ctx: &egui::Context) {
        if !self.view.lock.locked {
            return;
        }
        let has_vault = persist::has_vault();
        let lang = self.lang();
        let title = t(lang, "Screen Locked");
        let hint = t(lang, "Password");
        let wrong = t(lang, "Wrong master password");
        let mut submit = false;
        let mut cancel = false;

        egui::CentralPanel::default()
            .frame(egui::Frame::none())
            .show(ctx, |ui| {
                let size = ui.available_size();
                let field_width = (size.x * 0.32).clamp(240.0, 580.0);
                ui.vertical_centered(|ui| {
                    // 标题落在窗口上三分之一处，与金图同一比例；内容不随窗口
                    // 高度拉伸，只把留白按比例缩放。
                    ui.add_space((size.y * 0.26).max(40.0));
                    ui.label(egui::RichText::new(title).color(TITLE_BLUE).size(18.0));
                    ui.add_space((size.y * 0.09).max(24.0));
                    ui.horizontal(|ui| {
                        let field = egui::TextEdit::singleline(&mut self.lock_password)
                            .password(!self.show_lock_password)
                            .hint_text(hint)
                            .desired_width(field_width - 26.0);
                        let resp = ui.add(field);
                        eye(ui, &mut self.show_lock_password);
                        if resp.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                            submit = true;
                        }
                        // 第一次进入锁屏、以及输错清空之后，把光标交回密码框。
                        if self.lock_password.is_empty() && !resp.has_focus() {
                            resp.request_focus();
                        }
                    });
                    if self.view.lock.error {
                        ui.add_space(8.0);
                        ui.colored_label(egui::Color32::LIGHT_RED, wrong);
                    }
                });
                // 无库时给一条不占界面的退路；有库时 Esc 不解锁。
                if !has_vault && ui.input(|i| i.key_pressed(egui::Key::Escape)) {
                    cancel = true;
                }
            });

        if cancel {
            self.lock_password.clear();
            self.show_lock_password = false;
            self.dispatch_action(Action::Lock(LockAction::Cancel));
            return;
        }
        if submit {
            let ok = self.verify_lock_password(has_vault);
            self.lock_password.clear();
            self.show_lock_password = false;
            if ok {
                self.dispatch_action(Action::Lock(LockAction::Unlock));
            } else {
                self.dispatch_action(Action::Lock(LockAction::Failed));
            }
        }
    }

    /// 校验主密码：有库就解密一次，没库就创建一个空库。
    ///
    /// **先挂载解锁成功的 Vault，再尽力写盘。** 写盘失败只记一条错误：
    /// 主密码是对的，磁盘暂时写不进去（只读盘 / 满盘 / 权限）不该把人困在
    /// 锁屏里——那会逼他反复输对一个密码，还是出不去。
    fn verify_lock_password(&mut self, has_vault: bool) -> bool {
        let password = self.lock_password.clone();
        if password.is_empty() {
            return false;
        }
        let unlocked = if has_vault {
            persist::load_vault()
                .ok_or(())
                .and_then(|bytes| Vault::unlock(&bytes, &password).map_err(|_| ()))
                .ok()
        } else {
            Vault::create(&password).ok()
        };
        let Some(mut vault) = unlocked else {
            return false;
        };
        match vault.seal() {
            Ok(bytes) if !persist::save_vault(&bytes) => {
                self.log_note = Some("保险库已解锁，但写盘失败；本次运行仍可用".to_string());
            }
            Err(e) => {
                self.log_note = Some(format!("保险库已解锁，但重新封印失败：{e}"));
            }
            _ => {}
        }
        self.vault = Some(vault);
        // 与主 Authentication 页解锁路径一致：历史纳入主密码模型。
        self.load_history_from_vault();
        true
    }
}

/// 密码框右侧的小眼睛：闭眼时画一道斜杠。用 egui 的 shape 画，不依赖 emoji 字体。
fn eye(ui: &mut egui::Ui, shown: &mut bool) {
    let resp = ui.add(
        egui::Button::new("")
            .frame(false)
            .min_size(egui::Vec2::splat(20.0)),
    );
    let color = ui.visuals().text_color();
    let center = resp.rect.center();
    let stroke = egui::Stroke::new(1.2, color);
    ui.painter().add(egui::Shape::Ellipse(
        egui::epaint::EllipseShape::stroke(center, egui::Vec2::new(7.5, 4.5), stroke),
    ));
    ui.painter()
        .add(egui::Shape::circle_filled(center, 2.8, color));
    if !*shown {
        ui.painter().add(egui::Shape::line_segment(
            [
                resp.rect.min + egui::Vec2::new(2.0, resp.rect.height() - 3.0),
                resp.rect.max - egui::Vec2::new(2.0, resp.rect.height() - 3.0),
            ],
            stroke,
        ));
    }
    if resp.clicked() {
        *shown = !*shown;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 锁屏的密码校验必须走 Vault 自身，不能自己比一段明文。
    /// 这里用真 `Vault::create` / `unlock` 验一次往返语义。
    #[test]
    fn a_created_vault_unlocks_with_the_same_password_only() {
        let mut v = Vault::create("correct horse").unwrap();
        let bytes = v.seal().unwrap();
        assert!(Vault::unlock(&bytes, "correct horse").is_ok());
        assert!(Vault::unlock(&bytes, "wrong").is_err());
    }

    /// 视觉回归的纯数据断言：标题用金图的原词，别再塞回 emoji / 前缀图标。
    #[test]
    fn the_lock_title_uses_the_original_wording() {
        use term_app::i18n::{t, Lang};
        let title = t(Lang::En, "Screen Locked");
        assert_eq!(title, "Screen Locked");
        assert!(title.chars().all(|c| c.is_ascii_alphabetic() || c == ' '));
    }
}

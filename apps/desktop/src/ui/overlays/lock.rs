//! 锁屏遮罩（E-LOCK）。
//!
//! 复用 `term-vault` 的主密码：解锁时对 `vault.ztv` 做一次 `Vault::unlock`，
//! 成功才把内存里的保险库重新挂上。没有第二套口令，也不把密码留在
//! `ViewState` 里（输入框用跨帧缓冲，同款）。
//!
//! 没建过保险库时允许 Cancel：否则用户会被一个永远解不开的遮罩困住。
//! 这个逃生口只存在于「无库」这一种情况，见 `has_vault()`。

use crate::ui::*;
use term_app::lock::LockAction;
use term_runtime::persist;
use term_vault::Vault;

impl TerminalWindow {
    /// 画在所有面板之上的全屏遮罩。`locked` 为真时其余 UI 都不画（见 `mod.rs`）。
    pub(in crate::ui) fn draw_lock(&mut self, ctx: &egui::Context) {
        if !self.view.lock.locked {
            return;
        }
        let has_vault = persist::has_vault();
        let mut submit = false;
        let mut cancel = false;
        // 闭包里要借 `self.lock_password`，文案先在外面取好。
        let lang = self.lang();
        let tr = |en: &'static str| t(lang, en);
        let title = tr("🔒 Screen locked");
        let ask = tr("Enter the vault master password to unlock.");
        let no_vault = tr("No vault yet — set a master password, or cancel to leave.");
        let hint = tr("Master password");
        let wrong = tr("Wrong master password");
        let unlock = tr("Unlock");
        let create = tr("Create vault & unlock");
        let cancel_label = tr("Cancel");

        egui::CentralPanel::default().show(ctx, |ui| {
            ui.vertical_centered(|ui| {
                ui.add_space(60.0);
                ui.heading(title);
                ui.add_space(8.0);
                ui.label(if has_vault { ask } else { no_vault });
                ui.add_space(16.0);
                let resp = ui.add(
                    egui::TextEdit::singleline(&mut self.lock_password)
                        .password(true)
                        .hint_text(hint)
                        .desired_width(240.0),
                );
                if resp.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                    submit = true;
                }
                if self.view.lock.error {
                    ui.colored_label(egui::Color32::LIGHT_RED, wrong);
                }
                ui.add_space(10.0);
                ui.horizontal(|ui| {
                    if ui.button(if has_vault { unlock } else { create }).clicked() {
                        submit = true;
                    }
                    if !has_vault && ui.button(cancel_label).clicked() {
                        cancel = true;
                    }
                });
            });
        });

        if cancel {
            self.lock_password.clear();
            self.dispatch_action(Action::Lock(LockAction::Cancel));
            return;
        }
        if submit {
            let ok = self.verify_lock_password(has_vault);
            self.lock_password.clear();
            if ok {
                self.dispatch_action(Action::Lock(LockAction::Unlock));
            } else {
                self.dispatch_action(Action::Lock(LockAction::Failed));
            }
        }
    }

    /// 校验主密码：有库就解密一次，没库就创建一个空库。
    /// 两种情况下内存里的 `vault` 都被重新挂上，连接流程不会突然发现锁着。
    fn verify_lock_password(&mut self, has_vault: bool) -> bool {
        let password = self.lock_password.clone();
        if password.is_empty() {
            return false;
        }
        let result = if has_vault {
            persist::load_vault()
                .ok_or(())
                .and_then(|bytes| Vault::unlock(&bytes, &password).map_err(|_| ()))
                .and_then(|mut v| v.seal().map(|bytes| (v, bytes)).map_err(|_| ()))
        } else {
            Vault::create(&password)
                .map_err(|_| ())
                .and_then(|mut v| v.seal().map(|bytes| (v, bytes)).map_err(|_| ()))
        };
        match result {
            Ok((vault, bytes)) if persist::save_vault(&bytes) => {
                self.vault = Some(vault);
                true
            }
            _ => false,
        }
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
}

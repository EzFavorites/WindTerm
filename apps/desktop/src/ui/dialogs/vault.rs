//! 凭据保险库的解锁与保存（`apps/*` 持有实例，ADR-0017 §未决问题 5）。
//!
//! 边界很窄：
//! - 加解密在 `term-vault`（`Vault` / `Entry`）；
//! - 「这条会话的密码在哪」在 `term-store`（`vault_entry_id` 一列）；
//! - 文件读写与目录在 `term-runtime::persist`；
//! - 这一层只做**流程**：解锁一次、本次运行可用；连接时先问保险库；
//!   用户在密码对话框里勾了「存入保险库」才写。
//!
//! 为什么解锁界面在会话表单的 Authentication 页而不是一个独立对话框：
//! 独立对话框要与密码对话框叠在一起（保存密码 → 发现锁着 → 再问主密码），
//! 两个模态窗口互相嵌套是最容易写出「关不掉」的地方。放在表单里，
//! 用户在看凭据的地方就能把库打开，顺序是单向的。

use crate::ui::*;
use term_core::onekey;
use term_runtime::persist;
use term_vault::{Entry, Vault};
use super::settings_live::save_prefs;

/// Authentication 页里那块解锁区。
#[derive(Default)]
pub(in crate::ui) struct VaultForm {
    pub(in crate::ui) password: String,
    pub(in crate::ui) confirm: String,
    pub(in crate::ui) error: Option<String>,
    /// 解锁后的「改主密码」表单：展开时显示新密码 + 确认两格。
    pub(in crate::ui) changing: bool,
    pub(in crate::ui) new_password: String,
    pub(in crate::ui) new_confirm: String,
}

impl TerminalWindow {
    /// 保险库现在可用吗（解锁态）。
    pub(in crate::ui) fn vault_unlocked(&self) -> bool {
        self.vault.is_some()
    }

    /// 一句状态：没建 / 锁着 / 已解锁（带条目数）。
    pub(in crate::ui) fn vault_status(&self) -> String {
        match &self.vault {
            Some(vault) => format!("Unlocked — {} credential(s)", vault.entries().len()),
            None if persist::has_vault() => "Locked".to_string(),
            None => "Not created yet".to_string(),
        }
    }

    /// Account 页 Password 组的自动应答（保险库里的 `onekey:<name>:autofill`）。
    pub(in crate::ui) fn onekey_autofill(&self, name: &str) -> Option<String> {
        self.vault
            .as_ref()?
            .find(&onekey::autofill_vault_id(name))
            .and_then(|e| e.secret())
            .map(str::to_string)
    }

    /// 本帧所有 OneKey 方案的密码应答表（方案名 → 密码）。
    /// `Tab::poll` 不认识保险库，由这里一次性派生，锁着时是空表。
    pub(in crate::ui) fn onekey_autofill_map(&self) -> std::collections::HashMap<String, String> {
        self.onekeys
            .iter()
            .filter_map(|k| self.onekey_autofill(&k.name).map(|v| (k.name.clone(), v)))
            .collect()
    }

    /// 这条会话的密码（保险库锁着、没存、条目丢了都返回 `None`）。
    ///
    /// 只读：查不到就是查不到，**不**在这里顺手弹解锁框——连接流程有自己的
    /// 对话框，再插一个只会让人不知道自己在回答哪个问题。
    pub(in crate::ui) fn stored_password(&self, session: &str) -> Option<String> {
        let vault = self.vault.as_ref()?;
        let id = persist::vault_entry(session)?;
        let entry = vault.find(&id)?;
        entry.secret().map(|s| s.to_string())
    }

    /// 改主密码。`Vault::change_password` 只换内存里的密钥和 KDF 盐，
    /// 必须紧接着 `seal()` 落盘，否则下次解锁还是旧密码。
    pub(in crate::ui) fn submit_change_password(&mut self) {
        if self.vault_form.new_password != self.vault_form.new_confirm {
            self.vault_form.error = Some("两次输入的新主密码不一致".to_string());
            return;
        }
        let Some(vault) = self.vault.as_mut() else {
            return;
        };
        if let Err(e) = vault.change_password(&self.vault_form.new_password) {
            self.vault_form.error = Some(e.to_string());
            return;
        }
        match vault.seal() {
            Ok(bytes) if persist::save_vault(&bytes) => {
                self.vault_form.changing = false;
                self.vault_form.new_password.clear();
                self.vault_form.new_confirm.clear();
                self.vault_form.error = None;
                self.log_note = Some("Master password changed".to_string());
            }
            Ok(_) => self.vault_form.error = Some("写不进保险库文件".to_string()),
            Err(e) => self.vault_form.error = Some(e.to_string()),
        }
    }

    /// 解锁 / 创建。`creating` 由调用方按 `persist::has_vault ` 决定。
    pub(in crate::ui) fn submit_vault_form(&mut self, creating: bool) {
        let password = self.vault_form.password.clone();
        if creating {
            if password != self.vault_form.confirm {
                self.vault_form.error = Some("两次输入的主密码不一致".to_string());
                return;
            }
            match Vault::create(&password) {
                Ok(vault) => {
                    // 空库也要立刻落盘：否则下次进来又是「创建」，用户会以为没建成。
                    self.adopt_vault(vault);
                }
                Err(e) => self.vault_form.error = Some(e.to_string()),
            }
            return;
        }
        let Some(bytes) = persist::load_vault() else {
            self.vault_form.error = Some("找不到保险库文件".to_string());
            return;
        };
        match Vault::unlock(&bytes, &password) {
            Ok(vault) => self.adopt_vault(vault),
            Err(e) => self.vault_form.error = Some(e.to_string()),
        }
    }

    fn adopt_vault(&mut self, mut vault: Vault) {
        match vault.seal() {
            Ok(bytes) if persist::save_vault(&bytes) => {
                self.vault = Some(vault);
                self.vault_form = VaultForm::default();
                // 解锁 / 创建成功后把历史一并纳入主密码模型：保险库里有条目
                // 就以它为准（它可能比明文文件新），否则把明文历史吸收进来。
                self.load_history_from_vault();
            }
            Ok(_) => self.vault_form.error = Some("写不进保险库文件".to_string()),
            Err(e) => self.vault_form.error = Some(e.to_string()),
        }
    }

    /// 解锁后从保险库读历史。库里的条目是权威源；读到了就替换内存副本并删掉
    /// 旧的明文文件——这是一次性迁移，之后再也没有明文 `command_history.json`。
    pub(in crate::ui) fn load_history_from_vault(&mut self) {
        let raw = self
            .vault
            .as_ref()
            .and_then(|v| v.find(term_core::command_history::HISTORY_VAULT_ID))
            .and_then(|e| e.secret())
            .map(str::to_string);
        if let Some(raw) = raw {
            if let Some(mut history) = term_core::command_history::CommandHistory::from_json(&raw) {
                let prefs = term_profile::prefs::load().unwrap_or_default();
                history.reconfigure(
                    prefs.history_capacity(),
                    prefs.history_ttl_days(),
                    crate::now_secs(),
                );
                self.command_history = history;
            }
            persist::delete_command_history();
        }
    }

    /// 把命令行历史作为保险库条目加密保存，成功则删掉明文文件。
    /// 返回 `false` 表示保险库不可用（未建 / 锁着），调用方按旧路回落明文。
    pub(crate) fn save_history_to_vault(&mut self) -> bool {
        let Some(vault) = self.vault.as_mut() else {
            return false;
        };
        let json = self.command_history.to_json();
        if vault
            .upsert(Entry::password(
                term_core::command_history::HISTORY_VAULT_ID,
                "Command history",
                &json,
            ))
            .is_err()
        {
            return false;
        }
        let Ok(bytes) = vault.seal() else {
            return false;
        };
        if !persist::save_vault(&bytes) {
            return false;
        }
        persist::delete_command_history();
        true
    }

    /// 把密码存进保险库，并把引用挂到会话上。
    ///
    /// 顺序是有讲究的：**先落会话列表**再写引用——`set_vault_entry` 是一次
    /// `UPDATE`，行不在就什么都不会发生（新建的会话还没存过库时就是这种情况）。
    pub(in crate::ui) fn remember_password(&mut self, session: &str, password: &str) -> Result<(), String> {
        let bytes = {
            let Some(vault) = self.vault.as_mut() else {
                return Err("保险库锁着".to_string());
            };
            vault
                .upsert(Entry::password(session, session, password))
                .map_err(|e| e.to_string())?;
            vault.seal().map_err(|e| e.to_string())?
        };
        if !persist::save_vault(&bytes) {
            return Err("写不进保险库文件".to_string());
        }
        self.save_sessions();
        persist::set_vault_entry(session, Some(session));
        Ok(())
    }

    /// OneKey 方案的密码（vault 锁着 / 没存都返回 `None`）。条目 id 固定为
    /// `onekey:<name>`（见 `term_core::onekey::vault_id`）——方案与条目的
    /// 关联就是这条命名约定，不在 prefs 里留第二份引用。
    pub(in crate::ui) fn onekey_password(&self, name: &str) -> String {
        let Some(vault) = self.vault.as_ref() else {
            return String::new();
        };
        let id = onekey::vault_id(name);
        vault
            .find(&id)
            .and_then(|e| e.secret().map(|s| s.to_string()))
            .unwrap_or_default()
    }

    /// 把 OneKeys 编辑页的改动落盘：方案列表写 `prefs.onekeys`，密码草稿
    ///（若 vault 解锁）写进 vault 条目 `onekey:<name>`。锁着时草稿**丢失**
    /// ——页脚已写清「credentials not saved」。
    pub(in crate::ui) fn save_onekeys(&mut self) {
        let names: Vec<String> = self.onekeys.iter().map(|k| k.name.clone()).collect();
        // 密码草稿 → vault（解锁才写）。草稿按**方案名**键控：下标在删/复制之后
        // 会错位，早先的 `(usize, String)` 会把密码写到隔壁方案上。
        if let Some((name, password)) = self.onekeys_password_draft.take() {
            if self.vault_unlocked() && !password.is_empty() && names.contains(&name) {
                let id = onekey::vault_id(&name);
                if let Some(vault) = self.vault.as_mut() {
                    if vault.upsert(Entry::password(&id, &name, &password)).is_ok() {
                        if let Ok(bytes) = vault.seal() {
                            persist::save_vault(&bytes);
                        }
                    }
                }
            }
        }
        // 键盘交互应答同样只进 vault：JSON 数组存在 `onekey:<name>:ki`。
        if let Some((name, answers)) = self.onekeys_ki_draft.take() {
            if self.vault_unlocked() && names.contains(&name) {
                let id = onekey::ki_vault_id(&name);
                if let Some(vault) = self.vault.as_mut() {
                    if vault
                        .upsert(Entry::password(
                            &id,
                            &name,
                            &onekey::encode_ki_answers(&answers),
                        ))
                        .is_ok()
                    {
                        if let Ok(bytes) = vault.seal() {
                            persist::save_vault(&bytes);
                        }
                    }
                }
            }
        }
        // Account 页 Password 组的应答同样是密码：只进 vault，prefs 里那一格
        // 永远是空串。不这么做就等于把 OneKey 的登录密码明文写进 prefs.json。
        if let Some((name, password)) = self.onekeys_autofill_draft.take() {
            if self.vault_unlocked() && names.contains(&name) && !password.is_empty() {
                if let Some(vault) = self.vault.as_mut() {
                    if vault
                        .upsert(Entry::password(
                            &onekey::autofill_vault_id(&name),
                            &name,
                            &password,
                        ))
                        .is_ok()
                    {
                        if let Ok(bytes) = vault.seal() {
                            persist::save_vault(&bytes);
                        }
                    }
                }
            }
        }
        // 方案名改过之后，vault 里的条目 id 也要跟着改（旧 id 变成孤儿）。
        // 简单而诚实：只写 prefs；旧 id 残留由 `onekey_password` 的
        //「找不到就 None（空密码）」自然兜住，不悄悄删 vault 条目。
        save_prefs(|p| p.onekeys = self.onekeys.clone());
        // 控制面持有的是方案表的**副本**（连接时读的是它，不是 `Prefs`）。
        // 不在这里推一份，用户改完方案之后已开的窗口仍然按旧方案连接——
        // 「改一处，所有用它的服务器都跟着变」当场失效。
        self.pool.set_onekeys(self.onekeys.clone());
    }

    /// 锁定：内存里的条目与密钥就地清零，文件不动。
    pub(in crate::ui) fn lock_vault(&mut self) {
        if let Some(mut vault) = self.vault.take() {
            vault.lock();
        }
    }

    /// Authentication 页里那块「凭据保险库」。
    ///
    /// 排版跟 Authentication 页其余部分同一套：**标题在上、字段占满整行**。
    /// 原来这里是「标签 + 右侧跟随输入框」的横排，和一个已经改成上下结构的
    /// 页面拼在一起时，两个框宽度还不一样（`Master password` 那个被右边界
    /// 切掉一截），看起来像两个控件打架。
    pub(in crate::ui) fn draw_vault_section(&mut self, ui: &mut egui::Ui) {
        use super::session_form_body::{heading, FORM_W, ROW_H};
        let creating = !persist::has_vault();
        ui.separator();
        ui.horizontal(|ui| {
            ui.label("Credentials vault:");
            ui.strong(self.vault_status());
        });
        if self.vault_unlocked() {
            ui.horizontal(|ui| {
                if ui.button("Lock vault").clicked() {
                    self.lock_vault();
                }
                if ui.button("Change master password").clicked() {
                    self.vault_form.changing = !self.vault_form.changing;
                }
            });
            if self.vault_form.changing {
                heading(ui, "New master password:");
                ui.add_sized(
                    egui::vec2(FORM_W, ROW_H),
                    TextEdit::singleline(&mut self.vault_form.new_password).password(true),
                );
                heading(ui, "Again:");
                ui.add_sized(
                    egui::vec2(FORM_W, ROW_H),
                    TextEdit::singleline(&mut self.vault_form.new_confirm).password(true),
                );
                if ui.button("Change password").clicked() {
                    self.submit_change_password();
                }
                if let Some(error) = &self.vault_form.error {
                    ui.colored_label(egui::Color32::from_rgb(220, 90, 90), error);
                }
            }
            ui.add(
                egui::Label::new(
                    "Passwords are stored in the vault; a session only remembers the entry id. \
                     Locking clears them from memory until the next unlock.",
                )
                .wrap(),
            );
            return;
        }
        ui.add(
            egui::Label::new(if creating {
                "Create a master password (12+ characters). It is never stored anywhere — if you \
                 lose it, the saved passwords are unrecoverable."
            } else {
                "Unlock to let saved sessions connect without asking for their password."
            })
            .wrap(),
        );
        ui.add_space(4.0);
        heading(ui, "Master password:");
        ui.add_sized(
            egui::vec2(FORM_W, ROW_H),
            TextEdit::singleline(&mut self.vault_form.password)
                .password(true)
                .desired_width(FORM_W),
        );
        if creating {
            ui.add_space(5.0);
            heading(ui, "Again:");
            ui.add_sized(
                egui::vec2(FORM_W, ROW_H),
                TextEdit::singleline(&mut self.vault_form.confirm)
                    .password(true)
                    .desired_width(FORM_W),
            );
        }
        ui.add_space(6.0);
        let label = if creating { "Create vault" } else { "Unlock" };
        if ui.button(label).clicked() {
            self.submit_vault_form(creating);
        }
        if let Some(error) = &self.vault_form.error {
            ui.colored_label(egui::Color32::from_rgb(220, 90, 90), error);
        }
    }
}

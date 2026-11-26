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
use std::cell::RefCell;
use std::collections::HashMap;
use term_core::onekey;
use term_runtime::persist;
use term_vault::{Entry, Vault};
use super::settings_live::save_prefs;

/// 运行时凭据寄存处的「引用」前缀。`Tab.auth` / `Connector.auth` 只存这个字符串，
/// 真正的明文放在下面线程本地的寄存器里，lock / 断开时清零。
const SECRET_REF_PREFIX: &str = "vaultref:";

struct RuntimeSecret(Vec<u8>);

impl Drop for RuntimeSecret {
    fn drop(&mut self) {
        for byte in &mut self.0 {
            *byte = 0;
        }
    }
}

/// 同一个引用要供几个消费者使用：隧道、Filer/SFTP、终端连接都从 `Tab.auth`
/// 里那份引用取凭据。`take_secret` 仍是一次性消耗（保持「取走即清」的语义），
/// 但另存一份**按「用途 + 会话」的 id 可重建**的副本（等价于从**已解锁**的
/// 保险库按会话名再读一次）：隧道先取走之后，Filer 再取能通过这条重建路拿到
/// 自己的凭据，而不是报「凭据已清理」。锁屏 / 断线把两份一起清掉。
thread_local! {
    static RUNTIME_SECRETS: RefCell<HashMap<String, RuntimeSecret>> =
        RefCell::new(HashMap::new());
    /// 只活到锁屏 / 断线的重建副本；不落盘、不进 `Tab.auth`。
    static SESSION_REBUILD: RefCell<HashMap<String, String>> = RefCell::new(HashMap::new());
}

/// 把一个凭据存进运行时寄存器，返回只含 id 的引用串。
pub(in crate::ui) fn stash_secret(id: &str, value: &str) -> String {
    RUNTIME_SECRETS.with(|secrets| {
        secrets
            .borrow_mut()
            .insert(id.to_string(), RuntimeSecret(value.as_bytes().to_vec()));
    });
    // 重建副本按 id 键控：`:ki` / `:pk` 变体也各存一份，resolve 时原样拿回。
    SESSION_REBUILD.with(|rebuild| {
        rebuild
            .borrow_mut()
            .insert(id.to_string(), value.to_string());
    });
    format!("{SECRET_REF_PREFIX}{id}")
}

/// 引用串吗？（`options_for` 据此决定要不要从寄存器里取。）
pub(in crate::ui) fn is_secret_ref(value: &str) -> bool {
    value.starts_with(SECRET_REF_PREFIX)
}

/// 取出并**消耗**一份凭据：首个消费者取走后寄存器里不再留明文。
/// 同一会话的后续消费者走 [`rebuild_secret`]，不会因此报「凭据已清理」。
pub(in crate::ui) fn take_secret(reference: &str) -> Option<String> {
    let id = reference.strip_prefix(SECRET_REF_PREFIX)?;
    RUNTIME_SECRETS.with(|secrets| {
        secrets
            .borrow_mut()
            .remove(id)
            .map(|secret| String::from_utf8_lossy(&secret.0).into_owned())
    })
}

/// 句柄取空之后按会话（引用里的 id）重建一份明文。
///
/// 等价于「保险库还解锁着，就按这条会话再读一次密码」：它只在内存里活到
/// 锁屏 / 断线，绝不写回 `Tab.auth`，也绝不落盘。
pub(in crate::ui) fn rebuild_secret(reference: &str) -> Option<String> {
    let id = reference.strip_prefix(SECRET_REF_PREFIX)?;
    SESSION_REBUILD.with(|rebuild| rebuild.borrow().get(id).cloned())
}

/// 把一次认证里的密码类字段换成引用；私钥路径等非秘密原样留着。
pub(in crate::ui) fn stash_auth(auth: &term_ssh::AuthMethod, id: &str) -> term_ssh::AuthMethod {
    use term_ssh::AuthMethod as A;
    match auth {
        A::Password { password } if !is_secret_ref(password) => A::Password {
            password: stash_secret(id, password),
        },
        A::KeyboardInteractive { answers }
            if !answers.first().is_some_and(|a| is_secret_ref(a)) =>
        {
            let raw = serde_json::to_string(answers).unwrap_or_else(|_| "[]".to_string());
            A::KeyboardInteractive {
                answers: vec![stash_secret(&format!("{id}:ki"), &raw)],
            }
        }
        A::PrivateKey {
            path,
            passphrase: Some(passphrase),
        } if !is_secret_ref(passphrase) => A::PrivateKey {
            path: path.clone(),
            passphrase: Some(stash_secret(&format!("{id}:pk"), passphrase)),
        },
        other => other.clone(),
    }
}

/// 清空运行时凭据：锁屏 / 断开时调用。之后任何复用（SFTP / 隧道）都会
/// 明确报「凭据已清理」，而不是偷偷拿一份活了整会话的明文去连。
pub(in crate::ui) fn clear_runtime_secrets() {
    RUNTIME_SECRETS.with(|secrets| secrets.borrow_mut().clear());
    SESSION_REBUILD.with(|rebuild| rebuild.borrow_mut().clear());
}

/// 只清一条会话的（断线重连时其它会话仍可复用自己的凭据）。
pub(in crate::ui) fn clear_session_secrets(session: &str) {
    for id in [session, &format!("{session}:ki"), &format!("{session}:pk")] {
        RUNTIME_SECRETS.with(|secrets| {
            secrets.borrow_mut().remove(id);
        });
        SESSION_REBUILD.with(|rebuild| {
            rebuild.borrow_mut().remove(id);
        });
    }
}

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
            .map(|(name, value)| {
                // 应答会经过终端回显 / 自动日志：登记后日志写入端统一脱敏。
                term_io::log::register_secret(&value);
                (name, value)
            })
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
        let Some(vault) = self.vault.as_ref() else {
            return;
        };
        // 在临时副本里换盐 / 派生 / 封印；只有落盘成功才把新 key 提交到
        // `self.vault`。旧实现先改内存再写盘，写失败时内存认新密码、磁盘只认
        // 旧密码，下次解锁必失败——那是把用户关在门外。
        let prepared = vault.prepare_password_change(&self.vault_form.new_password);
        let (next, bytes) = match prepared {
            Ok(pair) => pair,
            Err(e) => {
                self.vault_form.error = Some(e.to_string());
                return;
            }
        };
        if !persist::save_vault(&bytes) {
            self.vault_form.error = Some("写不进保险库文件；主密码未更改，原密码仍然有效".to_string());
            return;
        }
        self.vault = Some(next);
        self.vault_form.changing = false;
        self.vault_form.new_password.clear();
        self.vault_form.new_confirm.clear();
        self.vault_form.error = None;
        self.log_note = Some("Master password changed".to_string());
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
        // 主密码正确 ⇒ 先挂载；seal / save 只是「顺手刷新文件」，失败不该
        // 把这次解锁判成失败（与锁屏那条路径同一纪律）。
        match vault.seal() {
            Ok(bytes) if !persist::save_vault(&bytes) => {
                self.vault_form.error = Some("已解锁，但写不进保险库文件；本次运行仍可用".to_string());
            }
            Err(e) => self.vault_form.error = Some(format!("已解锁，但重新封印失败：{e}")),
            _ => {}
        }
        self.vault = Some(vault);
        self.vault_form.password.clear();
        self.vault_form.confirm.clear();
        // 解锁 / 创建成功后把历史一并纳入主密码模型：保险库里有条目
        // 就以它为准（它可能比明文文件新），否则把明文历史吸收进来。
        self.load_history_from_vault();
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
                // 解析成功才删明文：解析失败时明文是用户唯一的历史副本，
                // 删掉它等于「迁移失败还顺手毁掉原件」。
                persist::delete_command_history();
            } else {
                self.vault_form.error = Some(
                    "保险库里的历史解析失败；明文历史已保留，请先修复或重建历史".to_string(),
                );
            }
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
        term_io::log::register_secret(password);
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
        // 应答 / 密码先登记到日志脱敏表（即使写盘失败也不该在日志里显形）。
        if let Some((_, password)) = &self.onekeys_password_draft {
            term_io::log::register_secret(password);
        }
        if let Some((_, password)) = &self.onekeys_autofill_draft {
            term_io::log::register_secret(password);
        }
        if let Some((_, answers)) = &self.onekeys_ki_draft {
            for answer in answers {
                term_io::log::register_secret(answer);
            }
        }
        let names: Vec<String> = self.onekeys.iter().map(|k| k.name.clone()).collect();
        // 三处写盘**累计错误**，不再静默吞掉：任何一处失败都要让用户看到
        // 「credentials not saved」，否则 prefs 存了方案、密码却没进 vault。
        let mut errors: Vec<String> = Vec::new();
        // 密码草稿 → vault（解锁才写）。草稿按**方案名**键控：下标在删/复制之后
        // 会错位，早先的 `(usize, String)` 会把密码写到隔壁方案上。
        if let Some((name, password)) = self.onekeys_password_draft.take() {
            if self.vault_unlocked() && !password.is_empty() && names.contains(&name) {
                let id = onekey::vault_id(&name);
                if let Some(vault) = self.vault.as_mut() {
                    match vault.upsert(Entry::password(&id, &name, &password)) {
                        Ok(()) => match vault.seal() {
                            Ok(bytes) if !persist::save_vault(&bytes) => {
                                errors.push(format!("{name}: 写不进保险库文件"))
                            }
                            Err(e) => errors.push(format!("{name}: {e}")),
                            _ => {}
                        },
                        Err(e) => errors.push(format!("{name}: {e}")),
                    }
                }
            }
        }
        // 键盘交互应答同样只进 vault：JSON 数组存在 `onekey:<name>:ki`。
        if let Some((name, answers)) = self.onekeys_ki_draft.take() {
            if self.vault_unlocked() && names.contains(&name) {
                let id = onekey::ki_vault_id(&name);
                if let Some(vault) = self.vault.as_mut() {
                    match vault.upsert(Entry::password(&id, &name, &onekey::encode_ki_answers(&answers))) {
                        Ok(()) => match vault.seal() {
                            Ok(bytes) if !persist::save_vault(&bytes) => {
                                errors.push(format!("{name}: 写不进保险库文件"))
                            }
                            Err(e) => errors.push(format!("{name}: {e}")),
                            _ => {}
                        },
                        Err(e) => errors.push(format!("{name}: {e}")),
                    }
                }
            }
        }
        // Account 页 Password 组的应答同样是密码：只进 vault，prefs 里那一格
        // 永远是空串。不这么做就等于把 OneKey 的登录密码明文写进 prefs.json。
        if let Some((name, password)) = self.onekeys_autofill_draft.take() {
            if self.vault_unlocked() && names.contains(&name) && !password.is_empty() {
                if let Some(vault) = self.vault.as_mut() {
                    match vault.upsert(Entry::password(&onekey::autofill_vault_id(&name), &name, &password)) {
                        Ok(()) => match vault.seal() {
                            Ok(bytes) if !persist::save_vault(&bytes) => {
                                errors.push(format!("{name}: 写不进保险库文件"))
                            }
                            Err(e) => errors.push(format!("{name}: {e}")),
                            _ => {}
                        },
                        Err(e) => errors.push(format!("{name}: {e}")),
                    }
                }
            }
        }
        // Send 栏不是密码栏：谁把保险库里的密码 / 应答抄进 `expect.send`，
        // 就等于把明文写进 prefs.json。保存时清掉这些值并明确报错。
        for scheme in &mut self.onekeys {
            let mut protected: Vec<String> = Vec::new();
            if let Some(vault) = self.vault.as_ref() {
                for id in [
                    onekey::vault_id(&scheme.name),
                    onekey::ki_vault_id(&scheme.name),
                    onekey::autofill_vault_id(&scheme.name),
                ] {
                    if let Some(raw) = vault.find(&id).and_then(|e| e.secret()) {
                        protected.push(raw.to_string());
                        protected.extend(onekey::decode_ki_answers(raw));
                    }
                }
            }
            for expect in &mut scheme.expects {
                if !expect.send.is_empty() && protected.contains(&expect.send) {
                    expect.send.clear();
                    errors.push(format!("{}: Send 不能填密码，请用 Password 组", scheme.name));
                }
            }
        }
        if errors.is_empty() {
            self.vault_form.error = None;
        } else {
            let detail = errors.join("；");
            // 页脚/状态栏可见的一次性提示：明文凭据没写进保险库就是没保存。
            self.vault_form.error = Some(format!("credentials not saved: {detail}"));
            self.log_note = Some(format!("credentials not saved: {detail}"));
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
    /// 同时清掉运行时凭据寄存器——`Tab.auth` 里的引用没有密码就取不到东西了。
    pub(in crate::ui) fn lock_vault(&mut self) {
        if let Some(mut vault) = self.vault.take() {
            vault.lock();
        }
        clear_runtime_secrets();
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
            }
            // 写盘失败（改密码 / OneKey 凭据没落库）必须一直可见，不能只在
            // 「改密码」展开时才显示——那等于让用户以为凭据保存好了。
            if let Some(error) = &self.vault_form.error {
                ui.colored_label(egui::Color32::from_rgb(220, 90, 90), error);
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

//! OneKey 登录方案的数据层（Tool ▸ OneKeys… 对话框的存储模型）。
//!
//! 放在 `term-core` 而不是 `term-app`：方案要落盘进 `Prefs`（`term-profile`
//! 拥有），而 `term-profile` 是低层 crate，按 ADR-0014 不得依赖 `term-app`。
//! 编辑页（`apps/desktop`）与连接层（`session_open`）都只消费这里的类型，
//! 分层不被破坏。
//!
//! OneKey 是什么：**一整套自动登录方案**——一次点击（或连接时按该方案的
//! 期望匹配）完成 SSH 登录。三块内容与 WindTerm 的 OneKeys 对话框
//! 逐项对应：
//!
//! - SSH：用户名 / 密码 / 公钥（每系统一份 Identity File）/
//!   键盘交互（Challenge → Response 表）；
//! - Account：期望（prompt）→ 应答（发送文本）的自动回复对（区分大小写 /
//!   正则 / 整词三个匹配开关）；
//! - Sessions：方案到会话的分配（会话表单里的 OneKey 下拉选的就是它）。
//!
//! 密码与键盘交互应答都属于凭据，**不进这份 JSON**：它们经 `term-vault`
//! 以 `onekey:<name>` 为条目 id 加密保存（与 `SessionConfig` 的密码同一条
//! 纪律——明文不落盘、锁着不可读）。这里只存引用字段名。

use serde::{Deserialize, Serialize};

/// 匹配开关：Account 里一条「看到什么 → 发什么」的三个可选项。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExpectFlags {
    pub case_sensitive: bool,
    pub regex: bool,
    pub whole_word: bool,
}

impl Default for ExpectFlags {
    fn default() -> Self {
        Self {
            case_sensitive: true,
            regex: false,
            whole_word: false,
        }
    }
}

/// 一条自动应答：匹配到 `expect` 就把 `send` 发给远端。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExpectSend {
    pub expect: String,
    pub send: String,
    pub flags: ExpectFlags,
}

/// 一套登录方案（OneKey 对话框里的一行）。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct OneKey {
    /// 方案名。会话表单的 OneKey 下拉显示的就是它，连接时按名取方案。
    /// 同批内必须唯一（编辑页用 `name` 当 key，重名会互相覆盖）。
    pub name: String,
    /// SSH 登录用户名。
    pub username: String,
    /// 公钥 Identity File，按下标对应 `System`（0 = Linux / 1 = macOS / 2 = Windows）。
    /// 空串 = 该系统未指定（回到会话自身的 identity_file）。
    pub identity_files: [String; 3],
    /// SSH 键盘交互的 Challenge → Response 表（同样只存引用，明文在 vault）。
    pub keyboard_interactive: Vec<ChallengeResponse>,
    /// Account：自动应答对。
    pub expects: Vec<ExpectSend>,
    /// 本方案是否启用。关掉 = 连接时跳过（与会话没选它等价，但保留编辑内容）。
    pub enabled: bool,
}

/// 键盘交互的一对问答。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChallengeResponse {
    pub challenge: String,
    /// vault 条目 id（`onekey:<name>` 的键从 `challenge` 取）。
    pub response: String,
}

/// 三个系统的显示名，与 WindTerm 视觉基线的表格列一致。
pub const SYSTEM_NAMES: [&str; 3] = ["Linux", "macOS", "Windows"];

/// 一个全空的方案（编辑页「Add」时的初值）。
impl Default for OneKey {
    fn default() -> Self {
        Self {
            name: String::new(),
            username: String::new(),
            identity_files: Default::default(),
            keyboard_interactive: Vec::new(),
            expects: Vec::new(),
            enabled: true,
        }
    }
}

/// vault 里存这份方案密码 / 应答所用的条目 id。
pub fn vault_id(name: &str) -> String {
    format!("onekey:{name}")
}

/// 从 vault 条目 id 还原方案名（`vault_id` 的逆）。
pub fn name_from_vault_id(id: &str) -> Option<&str> {
    id.strip_prefix("onekey:")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn vault_id_round_trips_with_its_inverse() {
        // 条目 id 是方案名 ↔ vault 条目的唯一桥梁：正反必须严格往返。
        let name = "prod";
        let id = vault_id(name);
        assert_eq!(id, "onekey:prod");
        assert_eq!(name_from_vault_id(&id), Some(name));
        // 不是 onekey: 前缀的 id 不该被误认成某个方案。
        assert_eq!(name_from_vault_id("session:prod"), None);
        assert_eq!(name_from_vault_id("onekey:"), Some(""));
    }

    #[test]
    fn a_new_onekey_is_empty_and_enabled() {
        let k = OneKey::default();
        assert!(k.name.is_empty());
        assert!(k.username.is_empty());
        assert_eq!(k.identity_files, ["", "", ""]);
        assert!(k.keyboard_interactive.is_empty());
        assert!(k.expects.is_empty());
        assert!(k.enabled, "新方案默认启用");
    }

    #[test]
    fn expect_flags_default_to_case_sensitive_plain_text() {
        let f = ExpectFlags::default();
        assert!(f.case_sensitive);
        assert!(!f.regex);
        assert!(!f.whole_word);
    }
}
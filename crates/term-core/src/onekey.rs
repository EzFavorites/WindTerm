//! OneKey 登录方案的数据层（Tool ▸ OneKeys… 对话框的存储模型）。
//!
//! 放在 `term-core` 而不是 `term-app`：方案要落盘进 `Prefs`（`term-profile`
//! 拥有），而 `term-profile` 是低层 crate，按 ADR-0014 不得依赖 `term-app`。
//! 编辑页（`apps/desktop`）与连接层（`session_open`）都只消费这里的类型，
//! 分层不被破坏。
//!
//! OneKey 是什么：**一整套连接期自动登录方案**。三块内容与 WindTerm 2.6.1
//! 的 OneKeys 对话框逐项对应：
//!
//! - SSH：用户名 / 密码 / 公钥（每系统一份 Identity File）/
//!   键盘交互（Challenge → Response 表）；
//! - Account：用户名与密码——密码只有保险库条目 `onekey:<name>`，方案本身
//!   不存任何凭据字段；
//! - Sessions：方案到会话的分配（会话表单里的 OneKey 下拉选的就是它）。
//!
//! 密码与键盘交互应答都属于凭据，**不进这份 JSON**：它们经 `term-vault`
//! 以 `onekey:<name>` 为条目 id 加密保存（与 `SessionConfig` 的密码同一条
//! 纪律——明文不落盘、锁着不可读）。这里只存引用字段名。
//!
//! 会话期的 `expect → send` 自动应答已删除：OneKey 只在连接期生效，登录
//! 之后的输出不再由方案回复。

use serde::{Deserialize, Serialize};

use crate::session::SessionConfig;

/// 一套登录方案（OneKey 对话框里的一行）。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct OneKey {
    /// 方案名。会话表单的 OneKey 下拉显示的就是它，连接时按名取方案。
    /// 同批内必须唯一（编辑页用 `name` 当 key，重名会互相覆盖）。
    pub name: String,
    /// SSH 登录用户名（Account 页也直接画这一格）。
    pub username: String,
    /// 公钥 Identity File，按下标对应 `System`（0 = Linux / 1 = macOS / 2 = Windows）。
    /// 空串 = 该系统未指定（回到会话自身的 identity_file）。
    pub identity_files: [String; 3],
    /// SSH 键盘交互的 Challenge → Response 表（同样只存引用，明文在 vault）。
    pub keyboard_interactive: Vec<ChallengeResponse>,
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

/// 键盘交互应答单独存一个保险库条目，避免把多条答案塞进密码字段。
pub fn ki_vault_id(name: &str) -> String {
    format!("onekey:{name}:ki")
}

/// 保险库条目里应答的序列化格式：一个 JSON 字符串数组。
/// 解析失败按空表处理——凭据读不出来时宁可不答，也不能静默发空串冒充。
pub fn encode_ki_answers(answers: &[String]) -> String {
    serde_json::to_string(answers).unwrap_or_else(|_| "[]".to_string())
}

pub fn decode_ki_answers(raw: &str) -> Vec<String> {
    serde_json::from_str::<Vec<String>>(raw).unwrap_or_default()
}

impl OneKey {
    /// 会话 `onekey` 字段选中的**可用**方案（`None` = 没选 / 不存在 / 已停用）。
    ///
    /// 「停用」与「没选」在这里是**同一个结果**，不是两档：`enabled` 的语义是
    /// 「连接时跳过，但保留编辑内容」（OneKeys 编辑页的说明就这么写），所以
    /// 连接路径上它不该留下任何痕迹。此前两处调用点各自 `find(|k| k.name == …)`，
    /// 于是「停用」这个开关对连接毫无作用——填了不生效。
    pub fn active<'a>(schemes: &'a [Self], name: &str) -> Option<&'a Self> {
        schemes.iter().find(|k| k.name == name && k.enabled)
    }

    /// 把本方案应用到一份会话配置上（返回克隆，不改原配置）。
    ///
    /// 只覆盖**方案里明确填了的**字段：`username` 非空才覆盖、
    /// `identity_files[当前系统]` 非空才覆盖。方案的「留空」语义就是
    /// 「回落会话自己的值」（与 OneKeys 编辑页的说明一致）。
    ///
    /// 凭据不在这里：密码与键盘交互应答属于保险库，而 `term-core` 不认识
    /// 保险库。调用方（界面 / headless）各自从自己的凭据来源补。
    pub fn apply_to(&self, config: &SessionConfig) -> SessionConfig {
        let mut out = config.clone();
        if !self.username.is_empty() {
            out.username = self.username.clone();
        }
        if let Some(path) = self.identity_files.get(system_index(&config.system)) {
            if !path.is_empty() {
                out.identity_file = path.clone();
            }
        }
        out
    }
}

/// `SessionConfig.system` → `identity_files` 的下标（0 = Linux / 1 = macOS /
/// 2 = Windows）。没写或写了不认识的值一律按 Linux——那是出厂值。
///
/// 这是这张表**唯一**的下标换算处：编辑页按 `SYSTEM_NAMES` 顺序画三列，
/// 连接时按这里取。两处各写一份 `match` 迟早会分叉。
pub fn system_index(system: &str) -> usize {
    match system {
        "macos" => 1,
        "windows" => 2,
        _ => 0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keyboard_answers_round_trip_through_a_vault_string() {
        let answers = vec!["token".to_string(), "yes".to_string()];
        let raw = encode_ki_answers(&answers);
        assert_eq!(decode_ki_answers(&raw), answers);
        assert_eq!(decode_ki_answers("not json"), Vec::<String>::new());
        assert_eq!(ki_vault_id("prod"), "onekey:prod:ki");
    }

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
        assert!(k.enabled, "新方案默认启用");
    }

    /// 三列与三个系统名的对应：编辑页按 `SYSTEM_NAMES` 顺序画，连接时按这里取。
    #[test]
    fn the_system_index_matches_the_three_column_table() {
        assert_eq!(system_index("linux"), 0);
        assert_eq!(system_index("macos"), 1);
        assert_eq!(system_index("windows"), 2);
        // 没写 / 写错：按出厂值 Linux，不 panic、不取到 Windows 那一列。
        assert_eq!(system_index(""), 0);
        assert_eq!(system_index("freebsd"), 0);
    }

    fn ssh() -> SessionConfig {
        let mut c = SessionConfig::new(crate::session::Protocol::Ssh);
        c.username = "local".to_string();
        c.identity_file = "/local/id".to_string();
        c.system = "macos".to_string();
        c.onekey = "prod".to_string();
        c
    }

    #[test]
    fn an_enabled_scheme_is_the_one_that_gets_used() {
        let mut off = OneKey {
            name: "prod".to_string(),
            ..OneKey::default()
        };
        off.enabled = false;
        // 停用 = 连接时跳过（与没选等价），不返回方案。
        assert!(OneKey::active(&[off], "prod").is_none());
        // 名字对不上同样不返回：不能因为列表里有别的方案就套上去。
        let on = OneKey {
            name: "prod".to_string(),
            ..OneKey::default()
        };
        assert!(OneKey::active(std::slice::from_ref(&on), "staging").is_none());
        assert_eq!(OneKey::active(&[on], "prod").map(|k| k.name.as_str()), Some("prod"));
    }

    #[test]
    fn applying_a_scheme_overrides_only_the_filled_fields() {
        let config = ssh();
        let scheme = OneKey {
            name: "prod".to_string(),
            username: "ops".to_string(),
            identity_files: ["/l".to_string(), "/m".to_string(), "/w".to_string()],
            ..OneKey::default()
        };
        let out = scheme.apply_to(&config);
        assert_eq!(out.username, "ops");
        assert_eq!(out.identity_file, "/m", "system=macos 取第二列");
        // 覆盖发生在克隆上：原配置一个字都不动。
        assert_eq!(config.username, "local");
        assert_eq!(config.identity_file, "/local/id");

        // 留空 = 回落会话自己的值，不是覆盖成空串。
        let partial = OneKey {
            name: "prod".to_string(),
            username: String::new(),
            identity_files: ["".to_string(), "".to_string(), "".to_string()],
            ..OneKey::default()
        };
        let out = partial.apply_to(&config);
        assert_eq!(out.username, "local");
        assert_eq!(out.identity_file, "/local/id");
    }
}

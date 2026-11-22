//! `term-store` — SQLite 会话库。
//!
//! ```text
//! sessions.db
//! ├─ groups    ：分组与顺序
//! ├─ sessions  ：身份列 + 整份 JSON
//! │              └ vault_entry_id ─▶ term-vault 的条目
//! └─ meta      ：schema 版本
//! ```
//!
//! 三条边界：
//! 1. **纯同步、无运行时**。依赖表里没有 tokio、没有 UI、没有 term-runtime。
//! 2. **不认识保险库的内容，只记引用**。密码永远不进这个库；库里只有
//!    `vault_entry_id`（一个字符串）。所以「保险库没解锁」时会话列表照样能读
//!    ——ADR-0017 §未决问题 2 的理由之一。
//! 3. **不猜**：schema 版本对不上就报错，不认识的行宁可失败也不猜着一个默认值写回去。
//!
//! 没有迁移代码：库只认自己这一种格式（用户拍板「不留历史包袱」，旧 JSON 不再读）。
//!
//! 为什么全量重写而不是增量：会话列表最多几百条，一次事务写几百行是毫秒级；
//! 增量要维护「哪些行没了」，而 UI 的编辑模型本来就是「一份完整列表」。
//! 全量写还有个好处——它天然是幂等的，崩在中间事务不会提交。

mod error;
mod schema;

pub use error::StoreError;

use rusqlite::{Connection, params};
use schema::{DDL, SCHEMA_VERSION, Row_, session_of};
use term_core::session::SessionConfig;

/// 一次读出的完整会话列表。
///
/// 不派生 `PartialEq`：`SessionConfig` 没有实现它（那边的类型只为序列化服务），
/// 而为了一个测试断言去给核心类型加 trait，是把测试的便利塞进语义层。
/// 要比较就比字段——测试里本来也是这么写的。
#[derive(Debug, Clone, Default)]
pub struct Snapshot {
    pub groups: Vec<String>,
    pub sessions: Vec<SessionConfig>,
}

/// 会话库。`open()` 建表并核对版本；之后的读写都在这个连接上。
pub struct Store {
    conn: Connection,
}

impl Store {
    /// 打开（或新建）一个库文件。目录不存在时**不**替你建——
    /// 建目录是调用方的事，悄悄建到别处比报错更难查。
    pub fn open(path: impl AsRef<std::path::Path>) -> Result<Self, StoreError> {
        let path = path.as_ref();
        let conn = Connection::open(path).map_err(|e| StoreError::Io {
            path: path.display().to_string(),
            source: std::io::Error::other(e.to_string()),
        })?;
        Self::prepare(conn)
    }

    /// 全内存库：测试用。
    pub fn open_in_memory() -> Result<Self, StoreError> {
        Self::prepare(Connection::open_in_memory()?)
    }

    fn prepare(conn: Connection) -> Result<Self, StoreError> {
        // WAL：读不阻塞写。会话列表的读者（UI）与写者（保存）在同一进程，
        // 但 WAL 也让「另一个进程正在读」不至于把保存卡住。
        // busy_timeout 兜住短暂争用——会话保存失败一次的代价是丢掉用户改动。
        conn.execute_batch(
            "PRAGMA journal_mode=WAL;
             PRAGMA synchronous=NORMAL;
             PRAGMA busy_timeout=2000;",
        )?;
        conn.execute_batch(DDL)?;
        let store = Self { conn };
        store.check_version()?;
        Ok(store)
    }

    fn check_version(&self) -> Result<(), StoreError> {
        let found: Option<String> = self
            .conn
            .query_row("SELECT value FROM meta WHERE key = 'version'", [], |row| {
                row.get(0)
            })
            .ok();
        match found {
            None => {
                self.conn.execute(
                    "INSERT INTO meta(key, value) VALUES('version', ?1)",
                    params![SCHEMA_VERSION.to_string()],
                )?;
                Ok(())
            }
            Some(v) if v == SCHEMA_VERSION.to_string() => Ok(()),
            Some(v) => Err(StoreError::Schema {
                found: v,
                expected: SCHEMA_VERSION,
            }),
        }
    }

    /// 读出全部会话与分组。**不需要保险库**：密码不在这个库里。
    pub fn load(&self) -> Result<Snapshot, StoreError> {
        let mut group_stmt = self
            .conn
            .prepare("SELECT name FROM groups ORDER BY position, name")?;
        let groups = group_stmt
            .query_map([], |row| row.get::<_, String>(0))?
            .collect::<rusqlite::Result<Vec<String>>>()?;

        let mut sess_stmt = self.conn.prepare(
            "SELECT name, group_name, protocol, host, port, username, vault_entry_id, position, config
             FROM sessions ORDER BY position, name",
        )?;
        let sessions = sess_stmt
            .query_map([], session_of)?
            .collect::<rusqlite::Result<Vec<SessionConfig>>>()?;

        Ok(Snapshot { groups, sessions })
    }

    /// 用一个完整列表替换库内容（单事务）。
    ///
    /// 保留每条的 `vault_entry_id`：那一列不属于「会话列表」，是凭据引用，
    /// 由 `set_vault_entry` 独立维护——保存列表不该把密码引用抹掉。
    pub fn save(&mut self, groups: &[String], sessions: &[SessionConfig]) -> Result<(), StoreError> {
        let kept = self.vault_entries()?;
        let tx = self.conn.transaction()?;
        tx.execute("DELETE FROM groups", [])?;
        tx.execute("DELETE FROM sessions", [])?;
        for (position, name) in groups.iter().enumerate() {
            tx.execute(
                "INSERT INTO groups(position, name) VALUES(?1, ?2)",
                params![position as i64, name],
            )?;
        }
        for (position, session) in sessions.iter().enumerate() {
            let mut row = Row_::of(session, position);
            row.vault_entry_id = kept
                .iter()
                .find(|(name, _)| name == &session.name)
                .and_then(|(_, id)| id.clone());
            tx.execute(
                "INSERT INTO sessions
                   (name, group_name, protocol, host, port, username, vault_entry_id, position, config)
                 VALUES(?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
                params![
                    row.name,
                    row.group_name,
                    row.protocol,
                    row.host,
                    row.port,
                    row.username,
                    row.vault_entry_id,
                    row.position,
                    row.config,
                ],
            )?;
        }
        tx.commit()?;
        Ok(())
    }

    /// 某条会话的密码在哪（`None` = 本机没存，连接时现问）。
    pub fn vault_entry(&self, session: &str) -> Result<Option<String>, StoreError> {
        Ok(self
            .conn
            .query_row(
                "SELECT vault_entry_id FROM sessions WHERE name = ?1",
                params![session],
                |row| row.get::<_, Option<String>>(0),
            )
            .ok()
            .flatten())
    }

    /// 记下（或清掉）某条会话的凭据引用。
    ///
    /// 会话不存在的返回 `Ok(false)` 而不是报错：调用顺序上「先保存列表、
    /// 再存密码」很常见，缺行不该让保存密码这一步失败。
    pub fn set_vault_entry(&mut self, session: &str, entry: Option<&str>) -> Result<bool, StoreError> {
        let n = self.conn.execute(
            "UPDATE sessions SET vault_entry_id = ?1 WHERE name = ?2",
            params![entry, session],
        )?;
        Ok(n > 0)
    }

    fn vault_entries(&self) -> Result<Vec<(String, Option<String>)>, StoreError> {
        let mut stmt = self
            .conn
            .prepare("SELECT name, vault_entry_id FROM sessions ORDER BY position")?;
        let rows = stmt
            .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(rows)
    }

    /// 库里有几条会话（诊断用；将来的 API 客户端可以报这个数）。
    pub fn len(&self) -> Result<usize, StoreError> {
        let n: i64 = self
            .conn
            .query_row("SELECT COUNT(*) FROM sessions", [], |row| row.get(0))?;
        Ok(n as usize)
    }

    pub fn is_empty(&self) -> Result<bool, StoreError> {
        Ok(self.len()? == 0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use term_core::session::Protocol;

    fn ssh_session(name: &str, group: &str) -> SessionConfig {
        let mut s = SessionConfig::new(Protocol::Ssh);
        s.name = name.to_string();
        s.group = group.to_string();
        s.host = "10.0.0.9".to_string();
        s.port = 2222;
        s.username = "deploy".to_string();
        s.keep_alive = 45;
        s.charset = "GBK".to_string();
        s
    }

    #[test]
    fn a_saved_list_comes_back_with_every_field() {
        let mut store = Store::open_in_memory().unwrap();
        let groups = vec!["Servers".to_string(), "Lab".to_string()];
        let sessions = vec![ssh_session("Prod", "Servers"), ssh_session("Box", "Lab")];
        store.save(&groups, &sessions).unwrap();

        let back = store.load().unwrap();
        assert_eq!(back.groups, groups);
        assert_eq!(back.sessions.len(), 2);
        assert_eq!(back.sessions[0].name, "Prod");
        // 四个「一字段一列最容易写丢」的字段。
        assert_eq!(back.sessions[0].port, 2222);
        assert_eq!(back.sessions[0].keep_alive, 45);
        assert_eq!(back.sessions[0].charset, "GBK");
        assert!(back.sessions[0].builtin_ssh);
        // X/Y/Z Modem 三页签的命令与包长是新增的序列化字段，往返不能丢。
        assert_eq!(back.sessions[0].modem_commands, ["rx", "rb", "rz"]);
        assert_eq!(back.sessions[0].modem_large_packets, [false, true, false]);
    }

    #[test]
    fn saving_twice_replaces_instead_of_appending() {
        let mut store = Store::open_in_memory().unwrap();
        store
            .save(&["Servers".to_string()], &[ssh_session("Prod", "Servers")])
            .unwrap();
        store
            .save(&["Servers".to_string()], &[ssh_session("Prod", "Servers")])
            .unwrap();
        assert_eq!(store.len().unwrap(), 1);
    }

    #[test]
    fn a_removed_session_is_really_gone() {
        let mut store = Store::open_in_memory().unwrap();
        store
            .save(
                &["Servers".to_string()],
                &[ssh_session("Prod", "Servers"), ssh_session("Old", "Servers")],
            )
            .unwrap();
        store
            .save(&["Servers".to_string()], &[ssh_session("Prod", "Servers")])
            .unwrap();
        let back = store.load().unwrap();
        assert_eq!(back.sessions.len(), 1);
        assert_eq!(back.sessions[0].name, "Prod");
    }

    #[test]
    fn the_credential_reference_is_not_part_of_the_session_list() {
        let mut store = Store::open_in_memory().unwrap();
        store
            .save(&["Servers".to_string()], &[ssh_session("Prod", "Servers")])
            .unwrap();
        assert!(store.set_vault_entry("Prod", Some("vault-7")).unwrap());
        // 再存一次列表：引用必须还在（它不属于列表）。
        store
            .save(&["Servers".to_string()], &[ssh_session("Prod", "Servers")])
            .unwrap();
        assert_eq!(store.vault_entry("Prod").unwrap().as_deref(), Some("vault-7"));
        assert!(store.set_vault_entry("Prod", None).unwrap());
        assert_eq!(store.vault_entry("Prod").unwrap(), None);
    }

    #[test]
    fn setting_a_reference_for_a_missing_session_is_not_an_error() {
        let mut store = Store::open_in_memory().unwrap();
        assert!(!store.set_vault_entry("nope", Some("x")).unwrap());
    }

    #[test]
    fn the_database_file_holds_no_password_bytes() {
        // 库只存 entry_id。会话配置里就算被人手塞了个密码字段，
        // 也走 `SessionConfig` 的序列化——不认识的键不会被写进去。
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("sessions.db");
        let mut store = Store::open(&db).unwrap();
        let mut session = serde_json::to_value(ssh_session("Prod", "Servers")).unwrap();
        session["password"] = serde_json::Value::String("hunter2".to_string());
        // 直接按 JSON 写行是绕过 `Schema::of` 的唯一办法——正是为了证明
        // 「即使有人这么干，库里也只有结构与 entry_id，没有明文密码列」。
        let back: SessionConfig = serde_json::from_value(session).unwrap();
        store
            .save(&["Servers".to_string()], &[back])
            .unwrap();
        store.set_vault_entry("Prod", Some("vault-1")).unwrap();
        for name in ["sessions.db", "sessions.db-wal"] {
            let path = dir.path().join(name);
            if let Ok(bytes) = std::fs::read(&path) {
                let text = String::from_utf8_lossy(&bytes);
                assert!(!text.contains("hunter2"), "明文密码进了 {name}");
            }
        }
    }
}

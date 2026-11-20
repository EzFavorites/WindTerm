//! 库结构与行映射。
//!
//! 表设计的一条判断：**身份列 + 整份 JSON**，而不是「一个字段一列」。
//!
//! | 方案 | 好处 | 坏处 |
//! |---|---|---|
//! | 一字段一列（20+ 列） | 纯关系式，可直接 SQL 过滤任意字段 | `SessionConfig` 每加一个字段就要改表 + 写迁移 + 补映射，三处都容易漏 |
//! | 只有一个 JSON 列 | 加字段零成本 | 不可查询，SQLite 退化成「一个文件」 |
//! | **身份列 + JSON**（本实现） | 会话树要用的（名字 / 分组 / 协议 / 主机 / 端口 / 凭据引用）都能直接查；其余字段跟着 `SessionConfig` 自动演化 | 其余字段不能 SQL 过滤——等真有那个查询需求再升列 |
//!
//! `vault_entry_id` **不在** JSON 里，单独一列：它是「密码在哪」这件事的
//! 唯一记录点，必须能在不解锁保险库时被读出来（ADR-0017 §未决问题 2）。

use rusqlite::Row;
use term_core::session::SessionConfig;

/// 本构建认识的库版本。写在 `meta.version`，读库时核对。
pub(crate) const SCHEMA_VERSION: u32 = 1;

/// 建表语句。全部 `IF NOT EXISTS`：`open()` 会对已有库重跑一遍，
/// 这一步顺带把「旧库缺表」修好，不需要单独的迁移框架。
pub(crate) const DDL: &str = r#"
CREATE TABLE IF NOT EXISTS meta (
    key   TEXT PRIMARY KEY,
    value TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS groups (
    position INTEGER PRIMARY KEY,
    name     TEXT NOT NULL UNIQUE
);

CREATE TABLE IF NOT EXISTS sessions (
    name           TEXT PRIMARY KEY,
    group_name     TEXT NOT NULL,
    protocol       TEXT NOT NULL,
    host           TEXT NOT NULL,
    port           INTEGER NOT NULL,
    username       TEXT NOT NULL,
    -- 密码所在的保险库条目 id。**永远不放密码本身**。
    vault_entry_id TEXT,
    position       INTEGER NOT NULL,
    -- 其余字段的完整 JSON（含 builtin_ssh / term / charset / 串口位宽……）。
    config         TEXT NOT NULL
);

CREATE INDEX IF NOT EXISTS sessions_by_group ON sessions(group_name);
"#;

/// 一条会话在 `sessions` 表里的那几列。
pub(crate) struct Row_ {
    pub name: String,
    pub group_name: String,
    pub protocol: String,
    pub host: String,
    pub port: u16,
    pub username: String,
    pub vault_entry_id: Option<String>,
    pub position: i64,
    pub config: String,
}

impl Row_ {
    pub(crate) fn of(session: &SessionConfig, position: usize) -> Self {
        Self {
            name: session.name.clone(),
            group_name: session.group.clone(),
            protocol: session.protocol.label().to_string(),
            host: session.host.clone(),
            port: session.port,
            username: session.username.clone(),
            // 凭据引用由 `Store::set_vault_entry` 单独维护；`save()` 不动这一列
            // （否则「保存会话列表」会把密码引用抹掉）。
            vault_entry_id: None,
            position: position as i64,
            config: serde_json::to_string(session).unwrap_or_else(|_| "{}".to_string()),
        }
    }
}

/// 从一行还原 `SessionConfig`。
///
/// `config` 列是权威：它由 `SessionConfig` 序列化而来，字段最全。身份列只用于
/// 排序与查询——所以还原时**不**拿它们覆盖 JSON 里的值（两者本应一致，不一致
/// 说明有人手改过库；那种情况宁可相信整份 JSON）。
pub(crate) fn session_of(row: &Row<'_>) -> rusqlite::Result<SessionConfig> {
    let raw: String = row.get("config")?;
    serde_json::from_str(&raw).map_err(|e| {
        rusqlite::Error::FromSqlConversionFailure(0, rusqlite::types::Type::Text, Box::new(e))
    })
}

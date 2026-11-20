//! 存储层的错误分类（R9-a 的规矩：crate 边界不许退回 `String`）。
//!
//! 三个维度对应调用方要做的三种事：
//! - `Io`：磁盘不配合 —— 权限、空间、路径不存在，重试或报障；
//! - `Sql`：库不配合（含「这一行的 config 列读不出来」）—— 库被手改过或损坏；
//! - `Schema`：库里是别的版本的库 —— 拒绝写入，别把新数据灌进旧结构。

use std::fmt;

#[derive(Debug)]
pub enum StoreError {
    /// 打开库文件失败。
    Io {
        path: String,
        source: std::io::Error,
    },
    /// SQLite 报错（含行内容解析失败）。
    Sql(rusqlite::Error),
    /// `meta.version` 与本次构建不一致。
    Schema { found: String, expected: u32 },
}

impl fmt::Display for StoreError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            StoreError::Io { path, source } => write!(f, "打开会话库 {path} 失败：{source}"),
            StoreError::Sql(e) => write!(f, "会话库错误：{e}"),
            StoreError::Schema { found, expected } => write!(
                f,
                "会话库版本是 {found}，本次构建只认 {expected}（拒绝写入，免得把旧库写成半新半旧）"
            ),
        }
    }
}

impl std::error::Error for StoreError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            StoreError::Io { source, .. } => Some(source),
            StoreError::Sql(e) => Some(e),
            _ => None,
        }
    }
}

impl From<rusqlite::Error> for StoreError {
    fn from(e: rusqlite::Error) -> Self {
        StoreError::Sql(e)
    }
}

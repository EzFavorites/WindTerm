//! 保险库的错误分类。
//!
//! 为什么不是一个 `String`（R9 的规矩）：调用方要按类别**做不同的事**——
//! 密码错就重试、文件损坏就引导从备份恢复、版本高就提示升级、锁定就先去解锁。
//! 拍平成字符串之后，这四种情况只能显示同一句话。

use std::fmt;

/// 主密码最短长度。12 是按「人能记住 + 不是常见词」给的实用下限，
/// 真正的强度校验（字典、熵估算）留到有 UI 的时候按提示做。
pub const MIN_PASSWORD_LEN: usize = 12;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum VaultError {
    /// 文件结构不合法：magic 不对、太短、header 非法。`&'static str` 说明是哪一步。
    Corrupt(&'static str),
    /// 算法或版本不被支持（文件比程序新，或是我们不认识的 KDF）。
    Unsupported(String),
    /// 密钥派生成功但 AEAD 校验失败。**无法区分「密码错」与「文件被改过」**——
    /// 这正是 AEAD 的性质，所以错误文案同时覆盖两种可能。
    BadPassword,
    /// 主密码太短。
    WeakPassword,
    /// 同一个 id 已经存在。
    DuplicateEntry(String),
    /// 要删的条目不存在。
    EntryNotFound(String),
    /// 保险库已锁定（未 `unlock` / 已 `lock`）。
    Locked,
    /// 加密库内部错误（参数非法、序列化失败）。正常情况下不该出现。
    Crypto(String),
}

impl fmt::Display for VaultError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            VaultError::Corrupt(what) => {
                write!(f, "保险库文件已损坏（{what}）——请从备份恢复")
            }
            VaultError::Unsupported(what) => {
                write!(f, "保险库格式不受支持（{what}）——请升级程序")
            }
            VaultError::BadPassword => write!(
                f,
                "无法解密：主密码不对，或文件被改动过（两者无法区分）"
            ),
            VaultError::WeakPassword => {
                write!(f, "主密码太短（至少 {MIN_PASSWORD_LEN} 个字符）")
            }
            VaultError::DuplicateEntry(id) => write!(f, "条目 {id} 已存在"),
            VaultError::EntryNotFound(id) => write!(f, "条目 {id} 不存在"),
            VaultError::Locked => write!(f, "保险库已锁定——先解锁再操作"),
            VaultError::Crypto(msg) => write!(f, "加密操作失败：{msg}"),
        }
    }
}

impl std::error::Error for VaultError {}

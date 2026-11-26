//! `term-sftp` 的错误只有两类。
//!
//! 分类的依据是「谁该负责」：
//! - `Ssh` —— 连不上 / 认证不过 / 服务器不给开通道。这与终端那条连接同一类问题，
//!   直接透传 `SshError`，让 UI 用同一套措辞；
//! - `Session` —— 连上了，但 SFTP 子系统没起来（服务器没装 `sftp-server`，
//!   或 `Subsystem sftp` 被注释掉了）。这一条必须能和「连不上」区分开：用户看到
//!   「连上了但没有 SFTP」才知道该找管理员要什么。

use std::fmt;

/// SFTP 操作失败。
#[derive(Debug)]
pub enum SftpError {
    /// 连接层的问题（host key / 认证 / 开通道）。
    Ssh(term_ssh::SshError),
    /// 连上了，但 SFTP 子系统谈不起来。
    Session(String),
}

impl fmt::Display for SftpError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            SftpError::Ssh(e) => write!(f, "{e}"),
            SftpError::Session(m) => {
                write!(f, "服务器的 SFTP 子系统用不了：{m}（sshd_config 里的 Subsystem 一行？）")
            }
        }
    }
}

impl std::error::Error for SftpError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            SftpError::Ssh(e) => Some(e),
            SftpError::Session(_) => None,
        }
    }
}

impl From<term_ssh::SshError> for SftpError {
    fn from(e: term_ssh::SshError) -> Self {
        SftpError::Ssh(e)
    }
}

/// 这一条错误信息是不是「会话已经没了」，而不是「这一次请求失败」。
///
/// `russh-sftp` 把 socket 错误原样塞进 `IO(String)`（`Broken pipe` /
/// `Connection reset by peer` / `Unexpected EOF`），把队列断开塞进
/// `UnexpectedBehavior("SendError…" / "RecvError…")`。这些再发一条
/// `Event::Failed` 只会让 UI 停在「同步中 / 预览等待中」——必须是 `Closed`。
/// 服务端明确的业务错误（`Permission denied`、`No such file`）不包含这些
/// 字样，仍走 `Failed`，两边不能串味。
pub(crate) fn is_disconnect(message: &str) -> bool {
    let lowered = message.to_ascii_lowercase();
    ["broken pipe", "connection reset", "connection closed", "unexpected eof", "not connected"]
        .iter()
        .any(|needle| lowered.contains(needle))
        || message.contains("SendError")
        || message.contains("RecvError")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 断连类与业务错误必须分开：前者让 UI 清预览/同步态，后者只报一条红字。
    #[test]
    fn connection_loss_is_distinguished_from_a_business_error() {
        for message in [
            "I/O: Broken pipe (os error 32)",
            "I/O: Connection reset by peer (os error 54)",
            "UnexpectedBehavior(SendError(...))",
            "UnexpectedBehavior(RecvError(...))",
        ] {
            assert!(is_disconnect(message), "{message} 应判为断连");
        }
        for message in [
            "Status(No such file)",
            "Permission denied",
            "Timeout",
            "Limit exceeded: 42",
        ] {
            assert!(!is_disconnect(message), "{message} 不该判为断连");
        }
    }
}

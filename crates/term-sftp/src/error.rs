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

//! 隧道的错误分类。与存储层同一条规矩：边界上不退回 `String`。
//!
//! 分五类，因为调用方要做五件不同的事：
//! - `Config`：规则写得不对（端口 0、绑定地址空）—— 校验表单；
//! - `Ssh`：连不上 / 认证失败 —— 这和「会话连不上」是同一件事，直接透传；
//! - `Listen`：本地端口占不上 —— 提示换端口；
//! - `Forward`：服务器不同意某条转发 —— 多半是服务端 `AllowTcpForwarding no`；
//! - `Local`：后台线程没留话就退了 —— 本机资源问题，不是网络问题。

use std::fmt;

use term_ssh::SshError;

#[derive(Debug)]
pub enum TunnelError {
    Config(String),
    Ssh(SshError),
    Listen {
        addr: String,
        source: std::io::Error,
    },
    Forward(String),
    /// 后台线程意外结束（没返回结果）。
    Local(String),
}

impl fmt::Display for TunnelError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            TunnelError::Config(why) => write!(f, "隧道规则不成立：{why}"),
            TunnelError::Ssh(e) => write!(f, "{e}"),
            TunnelError::Listen { addr, source } => {
                write!(f, "监听 {addr} 失败：{source}（端口被占用？）")
            }
            TunnelError::Forward(why) => write!(f, "{why}"),
            TunnelError::Local(why) => write!(f, "{why}"),
        }
    }
}

impl std::error::Error for TunnelError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            TunnelError::Ssh(e) => Some(e),
            TunnelError::Listen { source, .. } => Some(source),
            _ => None,
        }
    }
}

impl From<SshError> for TunnelError {
    fn from(e: SshError) -> Self {
        TunnelError::Ssh(e)
    }
}

/// 起隧道时线程起不来（本机资源枯竭，不是可恢复的连接错误）。
pub(crate) fn thread_failed(e: std::io::Error) -> TunnelError {
    TunnelError::Config(format!("无法启动隧道线程：{e}"))
}

/// 握手上限：与 `term_ssh` 的默认连接超时同值，但隧道没有 UI 可以等，
/// 所以单独写一条，免得哪天 `term-ssh` 改了默认值、这边悄悄跟着变。
pub(crate) const START_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(15);

//! 既不是 unix 也不是 Windows 的目标：没有本机进程间准入可用。
//!
//! 显式 Unsupported；绝不退化成 TCP / token（那会把控制面挂到局域网上）。

use std::io;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use term_script::ScriptSessions;
use term_session::SessionPool;

use super::super::{Config, ControlStream, ServeError};
use crate::bridge::lock;

/// 占位流：只为了让 `Client` 的结构在所有平台上形状一致；`connect_client`
/// 在任何调用都会先返回 Unsupported，不会真的读写它。
pub type ClientStream = io::Cursor<Vec<u8>>;

impl ControlStream for ClientStream {
    fn try_clone_stream(&self) -> io::Result<Self> {
        Ok(io::Cursor::new(self.get_ref().clone()))
    }
}

pub fn default_socket_path() -> Option<PathBuf> {
    None
}

pub fn endpoint_description() -> String {
    "本机控制端点（当前平台不支持）".to_string()
}

pub struct Bound;

pub fn bind(_cfg: &Config) -> Result<Bound, ServeError> {
    Err(ServeError::Unsupported(
        "当前平台既不是 unix 也不是 Windows，无法提供本机控制端点",
    ))
}

pub fn serve_bound(
    _bound: Bound,
    _probe: Arc<dyn lock::LockProbe>,
    _pool: Arc<SessionPool>,
    _sessions: ScriptSessions,
) -> Result<(), ServeError> {
    Err(ServeError::Unsupported(
        "当前平台既不是 unix 也不是 Windows，无法提供本机控制端点",
    ))
}

pub fn connect_client(_path: &Path) -> io::Result<ClientStream> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "当前平台既不是 unix 也不是 Windows，无法提供本机控制端点",
    ))
}

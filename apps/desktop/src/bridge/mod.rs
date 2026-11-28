//! 前台控制端点（ADR-0030 §决策 2，loopback TCP 版）。
//!
//! 传输层只做一件事：**全平台唯一**的 `TcpListener::bind(("127.0.0.1", port))`。
//! 默认端口 47653，`WINDTERM_CONTROL_PORT` 可覆盖；绑定失败只打 stderr，
//! GUI 照常起来。没有 UDS、没有 Windows 命名管道、没有 unsupported 平台分支。
//!
//! 准入不再靠平台进程身份，而靠两道与平台无关的门：
//!
//! 1. `~/.zterm/control-key.local-<port>`：256-bit 随机 hex（unix 0600，
//!    Windows 继承用户目录 ACL），只有同一用户读得到；
//! 2. 传输级 `auth.challenge` → `auth.proof`（HMAC-SHA256）→ `auth.key` 三帧。
//!    服务端先签名证明自己持有 key，客户端验过证明才回送 key；服务端比对
//!    一致才进能力面。这三帧没有 `jsonrpc` / `id` 字段，在 `serve_stdio`
//!    之前就被消费，**不是**能力面的 `auth` 方法，不进 `METHODS` / `describe`。
//!
//! # 一条连接的顺序
//!
//! 1. accept 后先验对端是 loopback（127.0.0.0/8 或 ::1），否则静默关；
//! 2. 锁屏？→ 静默关（不回错误，不泄露端点细节）；
//! 3. 收 `auth.challenge{nonce}`，回 HMAC-SHA256 签名证明；
//! 4. 收到 key 并与自己的比对，一致才交给 dispatch，逐行一问一答；
//! 5. 每次调用前再问一次锁屏（2 秒缓存，见 `lock::Cached`）：中途锁屏的会话
//!    立刻开始拒绝，但不主动断连接。
//!
//! 首请求（准入三帧 + 第一条 JSON-RPC）30 秒超时；单行上限 1 MiB，超限回
//! 解析错误并关连接。
//!
//! # 会话池只有一个
//!
//! 端点不自建 `SessionPool`。GUI 启动时把 `window.pool.clone()` 交给
//! [`serve_on_with_pool`]，远程客户端看到的 `session_list` 就是窗口里的那张表。
//! 每个 accept 的连接各自持有一个 `DeskSurface`（per-connection + 全 caps：
//! 这条路径不做脚本授权信封检查；port-based HostEnv 留给后续的 invoke / 端口
//! 任务，不在本步实现）。仲裁器仍是 [`surface::shared_arbiter`]。

pub mod client;
pub mod control;
pub mod lock;
pub mod surface;

pub use control::{bind, default_socket_path, endpoint_description, serve_bound, Bound, Config};

use std::io;
use std::sync::Arc;

use term_core::contract::Surface;
use term_core::surface_error::SurfaceError;
use term_script::ScriptSessions;
use term_session::SessionPool;

/// 定位不到家目录，key 文件无处安放。端口解析失败会回落默认值，不算错误。
#[derive(Debug)]
pub enum ServeError {
    NoHome,
    Io(io::Error),
}

impl std::fmt::Display for ServeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ServeError::NoHome => write!(f, "定位不到家目录，无法写入控制 key 文件"),
            ServeError::Io(err) => write!(f, "控制端点 IO 失败：{err}"),
        }
    }
}

impl std::error::Error for ServeError {}

/// GUI 启动路径调用：把**窗口那个池**交给端点，不再自建第二个池。
pub fn start_control_endpoint(pool: Arc<SessionPool>) {
    let bound = match bind(&Config::defaulted()) {
        Ok(bound) => bound,
        Err(err) => {
            // GUI 照常起来；控制面只是「附加能力」，不该因为端口占用把窗口带走。
            eprintln!("控制端点未启动：{err}");
            return;
        }
    };
    let probe = Arc::new(lock::Cached::new(lock::Ioreg, std::time::Duration::from_secs(2)));
    let sessions = ScriptSessions::shared().handle();
    std::thread::spawn(move || {
        let _ = serve_on_with_pool(bound, probe, pool, sessions);
    });
}

/// accept 循环的实际入口：传输层负责绑定与准入，本模块负责能力面接线。
pub fn serve_on_with_pool(
    bound: Bound,
    probe: Arc<dyn lock::LockProbe>,
    pool: Arc<SessionPool>,
    sessions: ScriptSessions,
) -> Result<(), ServeError> {
    serve_bound(bound, probe, pool, sessions)
}

/// 锁屏门：包一层后端，每次调用前问一次锁屏。
///
/// 不主动断连接：客户端可能只是暂时拿不到结果，等用户回来解锁再继续。
pub(crate) struct LockedGate {
    pub(crate) inner: surface::DeskSurface,
    pub(crate) probe: Arc<dyn lock::LockProbe>,
}

impl Surface for LockedGate {
    fn call(&self, method: &str, params: &serde_json::Value) -> Result<serde_json::Value, SurfaceError> {
        if self.probe.locked() == Some(true) {
            return Err(SurfaceError::locked(
                "屏幕已锁定：前台控制面在锁屏期间不执行任何能力面调用",
            ));
        }
        self.inner.call(method, params)
    }
}

#[cfg(test)]
#[path = "mod_tests.rs"]
mod tests;

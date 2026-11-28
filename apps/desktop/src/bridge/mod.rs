//! 前台控制端点（ADR-0030 §决策 2）。
//!
//! 桥接协议本身与传输无关——`term-bridge::transport::serve_stdio` 只要求
//! `R: BufRead, W: Write`。本模块负责的是 **stdio 没有的那一圈**：进程间
//! 准入、锁屏门、以及「只服务前台窗口持有的那一个会话池」。
//!
//! # 为什么不是 TCP + token
//!
//! 控制面只在**本机同一用户**面前暴露，而且协议的第一条请求就是普通方法：
//! 没有 token 文件、没有 `auth` 握手，`describe` 直接能问，`call` 直接能调。
//! 准入完全交给平台的进程间原语：
//!
//! - unix：固定 `~/.zterm/control.sock` + 文件权限 `0600` + 内核 peercred（uid）。
//! - windows：`\\.\pipe\windterm-control-<SID 哈希>` + 只放行当前用户 SID 的
//!   DACL + `GetNamedPipeClientProcessId` → `OpenProcessToken` → `TokenUser` 比对 SID。
//!
//! 两条路径的安全性质一一对应；Windows 不是「先连上再想想」的弱版本。
//!
//! # 一条连接的顺序
//!
//! 1. accept 后先验对端身份：不是同一用户，**静默关**（不回错误，不泄露端点细节）；
//! 2. 锁屏？→ 直接关（连错误都不回：回了等于告诉对方「这里有个控制面」）；
//! 3. 之后交给 `serve_stdio`，逐行一问一答；
//! 4. 每次调用前再问一次锁屏（2 秒缓存，见 `lock::Cached`）：中途锁屏的会话
//!    立刻开始拒绝，但不主动断连接。
//!
//! # 会话池只有一个
//!
//! 端点不自建 `SessionPool`。GUI 启动时把 `window.pool.clone()` 交给
//! [`serve_on_with_pool`]，远程客户端看到的 `session_list` 就是窗口里的那张表。
//! 每个 accept 的连接各自持有一个 `DeskSurface`（per-connection + 全 caps：
//! 这条路径不做脚本授权信封检查；port-based HostEnv 留给后续的 invoke / 端口
//! 任务，不在本步实现）。
//!
//! # 平台模块
//!
//! 平台相关代码全部收在 [`platform`] 下（`unix` / `windows` / 其它目标的
//! `unsupported`），每个平台只有一个绑定调用点；本文件与 `client` 保持平台中立。

pub mod client;
pub mod lock;
pub mod surface;

mod platform;

pub use platform::{bind, default_socket_path, endpoint_description, Bound};

use std::io::{self, BufReader, BufWriter, Read, Write};
use std::path::PathBuf;
use std::sync::Arc;

use term_core::contract::Surface;
use term_core::surface_error::SurfaceError;
use term_script::ScriptSessions;
use term_session::SessionPool;

/// socket 所在目录与文件名（unix）。Windows 不用路径，但常量留作单一事实源：
/// CLI 的用法文本从 [`endpoint_description`] 生成，不自己拼这个字符串。
pub const SOCKET_DIR: &str = ".zterm";
pub const SOCKET_FILE: &str = "control.sock";

/// 一次控制端点的配置。生产路径只有一个取值；测试可指定临时路径。
#[derive(Clone, Debug)]
pub struct Config {
    pub socket_path: PathBuf,
}

impl Config {
    pub fn new(socket_path: PathBuf) -> Self {
        Self { socket_path }
    }

    /// 生产配置：unix 是固定 `~/.zterm/control.sock`；Windows 忽略这个字段，
    /// 按当前用户 SID 决定管道名（见 `platform::windows::bind`）。
    pub fn defaulted() -> Result<Self, ServeError> {
        let path = default_socket_path().ok_or(ServeError::NoHome)?;
        Ok(Self::new(path))
    }
}

#[derive(Debug)]
pub enum ServeError {
    /// 定位不到家目录 / 用户 SID——固定端点无处安放。
    NoHome,
    /// 既不是 unix 也不是 Windows 的平台没有本机准入可用。
    Unsupported(&'static str),
    /// 同一个人已经起了前台：保留先来者的端点，不去覆盖它。
    AlreadyRunning(PathBuf),
    Io(io::Error),
}

impl std::fmt::Display for ServeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ServeError::NoHome => write!(f, "定位不到家目录，无法决定控制端点放在哪里"),
            ServeError::Unsupported(why) => write!(f, "当前平台不支持控制端点：{why}"),
            ServeError::AlreadyRunning(path) => write!(
                f,
                "已有前台在监听 {}（本进程不再抢这个端点）",
                path.display()
            ),
            ServeError::Io(err) => write!(f, "控制端点 IO 失败：{err}"),
        }
    }
}

impl std::error::Error for ServeError {}

/// 平台流：两端可克隆的**阻塞**字节流。没有 async，也没有 tokio。
pub(crate) trait ControlStream: Read + Write + Send + 'static {
    fn try_clone_stream(&self) -> io::Result<Self>
    where
        Self: Sized;
}

/// 连续 accept 失败多少次就认定监听器废了（不计数会 100% CPU 空转）。
pub(crate) const MAX_CONSECUTIVE_ACCEPT_ERRORS: u32 = 128;

/// GUI 启动路径调用：把**窗口那个池**交给端点，不再自建第二个池。
pub fn start_control_endpoint(pool: Arc<SessionPool>) {
    let cfg = match Config::defaulted() {
        Ok(cfg) => cfg,
        Err(err) => {
            eprintln!("控制端点未启动：{err}");
            return;
        }
    };
    let bound = match bind(&cfg) {
        Ok(bound) => bound,
        Err(err) => {
            // GUI 照常起来；控制面只是「附加能力」，不该因为端点占用把窗口带走。
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

/// accept 循环的实际入口：平台实现负责 accept + 准入，本模块负责逐行服务。
pub fn serve_on_with_pool(
    bound: Bound,
    probe: Arc<dyn lock::LockProbe>,
    pool: Arc<SessionPool>,
    sessions: ScriptSessions,
) -> Result<(), ServeError> {
    platform::serve_bound(bound, probe, pool, sessions)
}

/// 一条已准入的连接：per-connection `DeskSurface`，共享窗口的同一个 SessionPool。
pub(crate) fn serve_stream<S: ControlStream>(
    stream: S,
    probe: Arc<dyn lock::LockProbe>,
    pool: Arc<SessionPool>,
    sessions: ScriptSessions,
) -> io::Result<()> {
    let mut writer = BufWriter::new(stream.try_clone_stream()?);
    let reader = BufReader::new(stream);
    let gated = LockedGate {
        inner: surface::DeskSurface::with_sessions(pool, sessions),
        probe,
    };
    term_bridge::transport::serve_stdio(&gated, reader, &mut writer)
}

/// 锁屏门：包一层后端，每次调用前问一次锁屏。
///
/// 不主动断连接：客户端可能只是暂时拿不到结果，等用户回来解锁再继续。
struct LockedGate {
    inner: surface::DeskSurface,
    probe: Arc<dyn lock::LockProbe>,
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

//! 前台控制端点：Unix domain socket（ADR-0030 §决策 2）。
//!
//! 桥接协议本身与传输无关——`term-bridge::transport::serve_stdio` 只要求
//! `R: BufRead, W: Write`。本模块负责的是 **stdio 没有的那一圈**：进程间
//! 准入、锁屏门、以及「只服务前台窗口持有的那一个会话池」。
//!
//! # 为什么是 UDS，不是 TCP + token
//!
//! 控制面只在**本机同一用户**面前暴露。socket 路径 `~/.zterm/control.sock`
//! 天然带两道门：文件系统权限（`0600`）与内核的 peercred（连接对端的 uid）。
//! 于是不再有 token 文件、不再有 `auth` 握手，协议的第一条请求就是普通方法
//! ——`describe` 直接能问，`call` 直接能调。
//!
//! # 一条连接的顺序
//!
//! 1. accept 后先问 peercred：不是同一用户，**静默关**（不回错误，不泄露端点细节）；
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
//! # 平台
//!
//! unix 走 UDS + peercred + `0600`。Windows 的命名管道仍待实现，
//! 目前**显式返回 Unsupported**，绝不悄悄退回 TCP（那会把控制面挂到局域网上）。

pub mod client;
pub mod lock;
pub mod surface;

use std::io;
use std::path::PathBuf;
use std::sync::Arc;

#[cfg(unix)]
use std::fs;
#[cfg(unix)]
use std::os::unix::fs::{FileTypeExt, PermissionsExt};
#[cfg(unix)]
use std::os::unix::net::{UnixListener, UnixStream};

use term_core::contract::Surface;
use term_core::surface_error::SurfaceError;
use term_session::SessionPool;

/// socket 所在目录与文件名。路径固定：客户端不需要 `--socket` 参数。
pub const SOCKET_DIR: &str = ".zterm";
pub const SOCKET_FILE: &str = "control.sock";

/// 默认端点路径：`~/.zterm/control.sock`。
pub fn default_socket_path() -> Option<PathBuf> {
    dirs::home_dir().map(|home| home.join(SOCKET_DIR).join(SOCKET_FILE))
}

/// 一次控制端点的配置。生产路径只有一个取值；测试可指定临时路径。
#[derive(Clone, Debug)]
pub struct Config {
    pub socket_path: PathBuf,
}

impl Config {
    pub fn new(socket_path: PathBuf) -> Self {
        Self { socket_path }
    }

    /// 生产配置：固定 `~/.zterm/control.sock`。
    pub fn defaulted() -> Result<Self, ServeError> {
        let path = default_socket_path().ok_or(ServeError::NoHome)?;
        Ok(Self::new(path))
    }
}

#[derive(Debug)]
pub enum ServeError {
    /// 定位不到家目录——固定路径无处安放。
    NoHome,
    /// 平台还没有对应传输（Windows 命名管道 TODO）。
    Unsupported(&'static str),
    /// 同一个人已经起了前台：保留先来者的 socket，不去覆盖它。
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

/// 已绑定但还没开始 accept 的端点。`Drop` 清掉自己创建的 socket 文件。
#[cfg(unix)]
pub struct Bound {
    listener: UnixListener,
    path: PathBuf,
}

#[cfg(unix)]
impl Drop for Bound {
    fn drop(&mut self) {
        // 只删自己那份：别人已经接管这个路径时不能把它的 socket 拿走。
        if fs::symlink_metadata(&self.path)
            .map(|meta| meta.file_type().is_socket())
            .unwrap_or(false)
        {
            let _ = fs::remove_file(&self.path);
        }
    }
}

/// 非 unix 的占位：`bind` 只会返回 [`ServeError::Unsupported`]。
#[cfg(not(unix))]
pub struct Bound;

/// 创建 `~/.zterm`、处理陈旧 socket，绑定并收紧到 `0600`。
#[cfg(unix)]
pub fn bind(cfg: &Config) -> Result<Bound, ServeError> {
    let path = &cfg.socket_path;
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(ServeError::Io)?;
    }
    if path.exists() {
        let meta = fs::symlink_metadata(path).map_err(ServeError::Io)?;
        if !meta.file_type().is_socket() {
            return Err(ServeError::Io(io::Error::new(
                io::ErrorKind::AlreadyExists,
                format!("{} 不是 socket，拒绝覆盖", path.display()),
            )));
        }
        // 连得上 = 前台还活着；连不上 = 上次异常退出留下的壳，可以清掉。
        if UnixStream::connect(path).is_ok() {
            return Err(ServeError::AlreadyRunning(path.clone()));
        }
        fs::remove_file(path).map_err(ServeError::Io)?;
    }
    let listener = UnixListener::bind(path).map_err(ServeError::Io)?;
    // `UnixListener::bind` 建出来的 socket 受 umask 影响（常见 0755），
    // 必须显式收紧；权限和 peercred 是两道独立的门，谁都不能省。
    fs::set_permissions(path, fs::Permissions::from_mode(0o600)).map_err(ServeError::Io)?;
    Ok(Bound {
        listener,
        path: path.clone(),
    })
}

#[cfg(not(unix))]
pub fn bind(_cfg: &Config) -> Result<Bound, ServeError> {
    // TODO(Windows): `\\.\pipe\windterm-control` + 管道 ACL 等价 peercred。
    Err(ServeError::Unsupported(
        "Windows 命名管道尚未实现；当前只支持 unix domain socket",
    ))
}

/// 连续 accept 失败多少次就认定监听器废了（不计数会 100% CPU 空转）。
const MAX_CONSECUTIVE_ACCEPT_ERRORS: u32 = 128;

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
            // GUI 照常起来；控制面只是「附加能力」，不该因为端口占用把窗口带走。
            eprintln!("控制端点未启动：{err}");
            return;
        }
    };
    let probe = Arc::new(lock::Cached::new(lock::Ioreg, std::time::Duration::from_secs(2)));
    std::thread::spawn(move || {
        let _ = serve_on_with_pool(bound, probe, pool);
    });
}

/// accept 循环。一个连接一个线程，只隔离「某个客户端卡在 wait 上」。
pub fn serve_on_with_pool(
    bound: Bound,
    probe: Arc<dyn lock::LockProbe>,
    pool: Arc<SessionPool>,
) -> Result<(), ServeError> {
    #[cfg(unix)]
    {
        let mut consecutive_errors = 0u32;
        for incoming in bound.listener.incoming() {
            let stream = match incoming {
                Ok(stream) => {
                    consecutive_errors = 0;
                    stream
                }
                Err(err) => {
                    consecutive_errors += 1;
                    if consecutive_errors >= MAX_CONSECUTIVE_ACCEPT_ERRORS {
                        return Err(ServeError::Io(err));
                    }
                    continue;
                }
            };
            // 准入在 accept 这条串行路径上完成：不合格的连接根本不占线程。
            let ready = match admit(&stream, probe.as_ref()) {
                Ok(ready) => ready,
                Err(err) => {
                    let _ = err;
                    continue;
                }
            };
            if !ready {
                continue;
            }
            let probe = Arc::clone(&probe);
            let pool = Arc::clone(&pool);
            let mut writer = std::io::BufWriter::new(stream.try_clone().map_err(ServeError::Io)?);
            let reader = std::io::BufReader::new(stream);
            std::thread::spawn(move || {
                // per-connection + 全 caps：每个连接一个 DeskSurface，共享窗口的
                // 同一个 SessionPool；这里不做脚本授权信封检查，port-based
                // HostEnv 留到后续 invoke / 端口任务再接。
                let gated = LockedGate {
                    inner: surface::DeskSurface::new(pool),
                    probe,
                };
                let _ = term_bridge::transport::serve_stdio(&gated, reader, &mut writer);
            });
        }
        Ok(())
    }
    #[cfg(not(unix))]
    {
        let _ = (bound, probe, pool);
        Err(ServeError::Unsupported("Windows 命名管道尚未实现"))
    }
}

/// 同一 uid + 未锁屏才放行。两种拒绝都**静默**：不回协议错误。
#[cfg(unix)]
fn admit(stream: &UnixStream, probe: &dyn lock::LockProbe) -> io::Result<bool> {
    if !same_user(peer_uid(stream)?)? {
        return Ok(false);
    }
    Ok(probe.locked() != Some(true))
}

// `UnixStream::peer_cred()` 在本工具链上仍是 unstable feature
// （peer_credentials_unix_socket），不能靠它。这里只声明两个稳定的系统符号：
// 不用新增依赖，也比「读环境变量里的 USER」可靠（那是可以伪造的）。
#[cfg(unix)]
extern "C" {
    fn getuid() -> u32;
}

/// macOS / BSD：`getpeereid` 直接给 socket 对端的 euid。
#[cfg(all(unix, not(target_os = "linux")))]
fn peer_uid(stream: &UnixStream) -> io::Result<u32> {
    use std::os::unix::io::AsRawFd;

    extern "C" {
        fn getpeereid(fd: i32, euid: *mut u32, egid: *mut u32) -> i32;
    }
    let mut uid = 0u32;
    let mut gid = 0u32;
    // SAFETY: fd 来自活着的 UnixStream，两个输出指针都有效。
    if unsafe { getpeereid(stream.as_raw_fd(), &mut uid, &mut gid) } != 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(uid)
}

/// Linux：`SO_PEERCRED` 返回 `struct ucred { pid, uid, gid }`。
#[cfg(target_os = "linux")]
fn peer_uid(stream: &UnixStream) -> io::Result<u32> {
    use std::os::fd::AsRawFd;
    use std::os::raw::{c_int, c_void};

    #[repr(C)]
    struct UCred {
        pid: c_int,
        uid: u32,
        gid: u32,
    }
    const SOL_SOCKET: c_int = 1;
    const SO_PEERCRED: c_int = 17;
    extern "C" {
        fn getsockopt(
            fd: c_int,
            level: c_int,
            optname: c_int,
            optval: *mut c_void,
            optlen: *mut u32,
        ) -> c_int;
    }
    let mut cred = UCred {
        pid: 0,
        uid: 0,
        gid: 0,
    };
    let mut len = std::mem::size_of::<UCred>() as u32;
    // SAFETY: fd 有效；optval / optlen 指向栈上的有效内存且长度正确。
    if unsafe {
        getsockopt(
            stream.as_raw_fd(),
            SOL_SOCKET,
            SO_PEERCRED,
            &mut cred as *mut UCred as *mut c_void,
            &mut len,
        )
    } != 0
    {
        return Err(io::Error::last_os_error());
    }
    Ok(cred.uid)
}

/// 自己的 uid：libc `getuid()`，与对端 peercred 同一套身份口径。
#[cfg(unix)]
pub fn self_uid() -> io::Result<u32> {
    // SAFETY: getuid 无参、无失败返回。
    Ok(unsafe { getuid() })
}

#[cfg(unix)]
fn same_user(peer: u32) -> io::Result<bool> {
    Ok(peer == self_uid()?)
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

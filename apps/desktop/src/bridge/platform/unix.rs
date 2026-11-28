//! Unix 控制端点：Unix domain socket + 文件路径 `0600` + peercred 同用户校验。
//!
//! 行为从 `bridge/mod.rs` 原样搬过来，未改语义：固定 `~/.zterm/control.sock`，
//! 陈旧 socket 可清、活着的前台不可抢；accept 后的准入是「同 uid + 未锁屏」，
//! 两种拒绝都静默关（不回协议错误、不泄露端点细节）。

use std::fs;
use std::io;
use std::os::unix::fs::{FileTypeExt, PermissionsExt};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use term_script::ScriptSessions;
use term_session::SessionPool;

use super::super::{serve_stream, Config, ControlStream, ServeError, MAX_CONSECUTIVE_ACCEPT_ERRORS};
use crate::bridge::lock;

pub type ClientStream = UnixStream;

impl ControlStream for UnixStream {
    fn try_clone_stream(&self) -> io::Result<Self> {
        self.try_clone()
    }
}

/// `~/.zterm/control.sock`。客户端没有 `--socket` 参数，这是双方唯一会合点。
pub fn default_socket_path() -> Option<PathBuf> {
    dirs::home_dir().map(|home| home.join(super::super::SOCKET_DIR).join(super::super::SOCKET_FILE))
}

/// CLI 用法里展示的端点描述：路径由常量推导，不在 CLI 侧硬编码。
pub fn endpoint_description() -> String {
    let where_ = dirs::home_dir()
        .map(|home| home.display().to_string())
        .unwrap_or_else(|| "~".to_string());
    format!(
        "{where_}/{}{}{}（Unix domain socket，0600 + peercred 同用户）",
        super::super::SOCKET_DIR,
        std::path::MAIN_SEPARATOR,
        super::super::SOCKET_FILE
    )
}

/// 已绑定但还没开始 accept 的端点。`Drop` 清掉自己创建的 socket 文件。
pub struct Bound {
    listener: UnixListener,
    path: PathBuf,
}

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

/// 创建 `~/.zterm`、处理陈旧 socket，绑定并收紧到 `0600`。
///
/// 这里是 unix 生产路径**唯一**的 `UnixListener::bind` 调用点。
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

/// accept 循环。一个连接一个线程，只隔离「某个客户端卡在 wait 上」。
pub fn serve_bound(
    bound: Bound,
    probe: Arc<dyn lock::LockProbe>,
    pool: Arc<SessionPool>,
    sessions: ScriptSessions,
) -> Result<(), ServeError> {
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
        let sessions = sessions.handle();
        std::thread::spawn(move || {
            // per-connection + 全 caps：每个连接一个 DeskSurface，共享窗口的
            // 同一个 SessionPool；这里不做脚本授权信封检查，port-based
            // HostEnv 留到后续 invoke / 端口任务再接。
            let _ = serve_stream(stream, probe, pool, sessions);
        });
    }
    Ok(())
}

/// 同一 uid + 未锁屏才放行。两种拒绝都**静默**：不回协议错误。
pub fn admit(stream: &UnixStream, probe: &dyn lock::LockProbe) -> io::Result<bool> {
    if !same_user(peer_uid(stream)?)? {
        return Ok(false);
    }
    Ok(probe.locked() != Some(true))
}

// `UnixStream::peer_cred()` 在本工具链上仍是 unstable feature
// （peer_credentials_unix_socket），不能靠它。这里只声明两个稳定的系统符号：
// 不用新增依赖，也比「读环境变量里的 USER」可靠（那是可以伪造的）。
extern "C" {
    fn getuid() -> u32;
}

/// macOS / BSD：`getpeereid` 直接给 socket 对端的 euid。
#[cfg(not(target_os = "linux"))]
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
pub fn self_uid() -> io::Result<u32> {
    // SAFETY: getuid 无参、无失败返回。
    Ok(unsafe { getuid() })
}

pub fn same_user(peer: u32) -> io::Result<bool> {
    Ok(peer == self_uid()?)
}

/// 客户端连接固定路径。socket 不存在时映射成 `NotFound`，由上层报「前台未运行」。
pub fn connect_client(path: &Path) -> io::Result<ClientStream> {
    if !path.exists() {
        return Err(io::Error::new(
            io::ErrorKind::NotFound,
            format!("{} 不存在", path.display()),
        ));
    }
    UnixStream::connect(path)
}

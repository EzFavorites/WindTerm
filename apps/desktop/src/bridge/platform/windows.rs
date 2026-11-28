//! Windows 控制端点：命名管道 + DACL 只放行当前用户 SID + 对端 PID→SID 复核。
//!
//! 这是 UDS 那条路的**安全等价物**，不是简化版：
//!   · 管道名 `\\.\pipe\windterm-control-<SID 稳定哈希>` 按用户隔离（等价固定路径）；
//!   · DACL 只有当前用户 SID 一条 Allow ACE（等价 `0600`），其他用户连 CreateFile
//!     都拿不到句柄；
//!   · accept 后再用 `GetNamedPipeClientProcessId` → `OpenProcessToken` →
//!     `GetTokenInformation(TokenUser)` 拿到对端 SID 并逐字节比对（等价 peercred）。
//!   任一步失败都**显式拒绝**（关掉这个实例，不读不回），绝不因为「问不出来」
//!   而静默放行。
//!
//! `CreateNamedPipe` 返回 `ERROR_PIPE_BUSY`（或 first-instance 的
//! `ERROR_ACCESS_DENIED`）就是前台已经在跑 —— 对应 UDS 的 `AlreadyRunning`。

use std::ffi::c_void;
use std::fs::File;
use std::io;
use std::os::windows::io::{AsRawHandle, FromRawHandle, IntoRawHandle, OwnedHandle, RawHandle};
use std::path::{Path, PathBuf};
use std::ptr::{null, null_mut};
use std::sync::Arc;
use std::time::{Duration, Instant};

use term_script::ScriptSessions;
use term_session::SessionPool;
use windows_sys::Win32::Foundation::{
    CloseHandle, ERROR_ACCESS_DENIED, ERROR_FILE_NOT_FOUND, ERROR_NO_DATA, ERROR_PIPE_BUSY,
    ERROR_PIPE_NOT_CONNECTED, GENERIC_ALL, GENERIC_READ, GENERIC_WRITE, GetLastError, HANDLE,
    INVALID_HANDLE_VALUE, LocalFree,
};
use windows_sys::Win32::Security::Authorization::{
    SetEntriesInAclW, EXPLICIT_ACCESS_W, GRANT_ACCESS, NO_MULTIPLE_TRUSTEE, TRUSTEE_IS_SID,
    TRUSTEE_IS_USER, TRUSTEE_W,
};
use windows_sys::Win32::Security::{
    GetLengthSid, GetTokenInformation, InitializeSecurityDescriptor, SetSecurityDescriptorDacl,
    ACL, NO_INHERITANCE, PSECURITY_DESCRIPTOR, SECURITY_ATTRIBUTES, SECURITY_DESCRIPTOR, TOKEN_QUERY,
    TOKEN_USER, TokenUser,
};
use windows_sys::Win32::Storage::FileSystem::{
    CreateFileW, FILE_FLAG_FIRST_PIPE_INSTANCE, OPEN_EXISTING, PIPE_ACCESS_DUPLEX,
};
use windows_sys::Win32::System::Pipes::{
    ConnectNamedPipe, CreateNamedPipeW, DisconnectNamedPipe, GetNamedPipeClientProcessId, WaitNamedPipeW,
    PIPE_READMODE_BYTE, PIPE_REJECT_REMOTE_CLIENTS, PIPE_TYPE_BYTE, PIPE_UNLIMITED_INSTANCES,
    PIPE_WAIT,
};
use windows_sys::Win32::System::SystemServices::SECURITY_DESCRIPTOR_REVISION;
use windows_sys::Win32::System::Threading::{
    GetCurrentProcess, OpenProcess, OpenProcessToken, PROCESS_QUERY_LIMITED_INFORMATION,
};

use super::super::{serve_stream, Config, ControlStream, ServeError};
use crate::bridge::lock;

pub type ClientStream = File;

impl ControlStream for File {
    fn try_clone_stream(&self) -> io::Result<Self> {
        self.try_clone()
    }
}

const PIPE_PREFIX: &str = r"\\.\pipe\windterm-control-";
const BUFFER_SIZE: u32 = 4096;
/// 服务端默认等待 + 客户端总等待上限：忙等必须有上限，不能无限 `WaitNamedPipe`。
const PIPE_WAIT_MS: u32 = 5_000;
const CLIENT_WAIT_SLICE_MS: u32 = 200;

fn last_error() -> io::Error {
    io::Error::from_raw_os_error(unsafe { GetLastError() } as i32)
}

fn wide(text: &str) -> Vec<u16> {
    text.encode_utf16().chain(std::iter::once(0)).collect()
}

// ---- 当前用户 SID：管道名、DACL、对端比对共用同一套身份口径 ----

fn current_token() -> io::Result<OwnedHandle> {
    let mut token: HANDLE = null_mut();
    // GetCurrentProcess() 返回伪句柄，不需要（也不可以）CloseHandle。
    if unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) } == 0 {
        return Err(last_error());
    }
    // SAFETY: OpenProcessToken 成功返回的 token 是新建的独占句柄。
    Ok(unsafe { OwnedHandle::from_raw_handle(token) })
}

/// 从令牌读 `TokenUser` 里的 SID，复制成自有字节（不依赖令牌缓冲区继续存活）。
fn token_user_sid(token: RawHandle) -> io::Result<Vec<u8>> {
    let mut needed = 0u32;
    // 第一次必然失败并给出长度；用 u64 作缓冲区保证指针对齐。
    unsafe { GetTokenInformation(token, TokenUser, null_mut(), 0, &mut needed) };
    let words = (needed as usize).div_ceil(8).max(32);
    let mut buf = vec![0u64; words];
    if unsafe {
        GetTokenInformation(
            token,
            TokenUser,
            buf.as_mut_ptr().cast::<c_void>(),
            (words * 8) as u32,
            &mut needed,
        )
    } == 0
    {
        return Err(last_error());
    }
    let user = unsafe { &*buf.as_ptr().cast::<TOKEN_USER>() };
    let sid_len = unsafe { GetLengthSid(user.User.Sid) };
    if sid_len == 0 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "令牌里没有可用的用户 SID",
        ));
    }
    // SAFETY: TokenUser 保证 Sid 指向令牌缓冲区内一个完整 SID。
    let bytes = unsafe { std::slice::from_raw_parts(user.User.Sid.cast::<u8>(), sid_len as usize) };
    Ok(bytes.to_vec())
}

fn current_user_sid() -> io::Result<Vec<u8>> {
    let token = current_token()?;
    token_user_sid(token.as_raw_handle())
}

struct HandleGuard(HANDLE);

impl Drop for HandleGuard {
    fn drop(&mut self) {
        if !self.0.is_null() {
            unsafe { CloseHandle(self.0) };
        }
    }
}

/// 对端进程的用户 SID。任一步失败返回 None —— 调用方必须据此**拒绝**，不是放行。
fn process_user_sid(pid: u32) -> Option<Vec<u8>> {
    let process = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid) };
    if process.is_null() {
        return None;
    }
    let _process = HandleGuard(process);
    let mut token: HANDLE = null_mut();
    if unsafe { OpenProcessToken(process, TOKEN_QUERY, &mut token) } == 0 {
        return None;
    }
    let _token = HandleGuard(token);
    token_user_sid(token).ok()
}

/// SID 的稳定哈希：管道名带用户信息但不直接泄露 SID 文本。
fn pipe_name(sid: &[u8]) -> String {
    let mut hash = 0xcbf2_9ce4_8422_2325u64;
    for byte in sid {
        hash ^= *byte as u64;
        hash = hash.wrapping_mul(0x0100_0000_01b3);
    }
    format!("{PIPE_PREFIX}{hash:016x}")
}

pub fn default_socket_path() -> Option<PathBuf> {
    current_user_sid().ok().map(|sid| PathBuf::from(pipe_name(&sid)))
}

/// CLI 用法里展示的端点描述：由 SID 推导，不在 CLI 侧硬编码任何路径。
pub fn endpoint_description() -> String {
    match default_socket_path() {
        Some(path) => format!(
            "命名管道 {}（DACL 只放行当前用户 SID；服务端按对端进程 SID 复核）",
            path.display()
        ),
        None => format!("命名管道 {PIPE_PREFIX}<当前用户 SID>（DACL 只放行当前用户 SID）"),
    }
}

// ---- 绑定：唯一的 CreateNamedPipe 调用点 ----

pub struct Bound {
    name: String,
    instance: OwnedHandle,
}

struct AclGuard(*mut ACL);

impl Drop for AclGuard {
    fn drop(&mut self) {
        unsafe { LocalFree(self.0.cast::<c_void>()) };
    }
}

/// 建一个管道实例。DACL 只有当前用户 SID 一条 Allow ACE，等价 UDS 的 `0600`。
fn create_instance(name: &str, first: bool) -> io::Result<OwnedHandle> {
    let sid = current_user_sid()?;
    let mut sd = SECURITY_DESCRIPTOR::default();
    if unsafe {
        InitializeSecurityDescriptor(
            &mut sd as *mut SECURITY_DESCRIPTOR as PSECURITY_DESCRIPTOR,
            SECURITY_DESCRIPTOR_REVISION,
        )
    } == 0
    {
        return Err(last_error());
    }
    let trustee = TRUSTEE_W {
        pMultipleTrustee: null_mut(),
        MultipleTrusteeOperation: NO_MULTIPLE_TRUSTEE,
        TrusteeForm: TRUSTEE_IS_SID,
        TrusteeType: TRUSTEE_IS_USER,
        // TrusteeForm=TRUSTEE_IS_SID 时这个字段按 PSID 解释。
        ptstrName: sid.as_ptr() as *mut u16,
    };
    let entry = EXPLICIT_ACCESS_W {
        grfAccessPermissions: GENERIC_ALL,
        grfAccessMode: GRANT_ACCESS,
        grfInheritance: NO_INHERITANCE,
        Trustee: trustee,
    };
    let mut acl: *mut ACL = null_mut();
    let status = unsafe { SetEntriesInAclW(1, &entry, null(), &mut acl) };
    if status != 0 {
        return Err(io::Error::from_raw_os_error(status as i32));
    }
    let _acl = AclGuard(acl);
    if unsafe {
        SetSecurityDescriptorDacl(
            &mut sd as *mut SECURITY_DESCRIPTOR as PSECURITY_DESCRIPTOR,
            1,
            acl,
            0,
        )
    } == 0
    {
        return Err(last_error());
    }
    let mut attrs = SECURITY_ATTRIBUTES::default();
    attrs.nLength = std::mem::size_of::<SECURITY_ATTRIBUTES>() as u32;
    attrs.lpSecurityDescriptor = &mut sd as *mut SECURITY_DESCRIPTOR as *mut c_void;
    let flags = PIPE_ACCESS_DUPLEX | if first { FILE_FLAG_FIRST_PIPE_INSTANCE } else { 0 };
    let wide_name = wide(name);
    // SAFETY: wide_name 以 NUL 结尾；attrs 里的 SD / DACL 到调用返回前一直有效。
    let raw = unsafe {
        CreateNamedPipeW(
            wide_name.as_ptr(),
            flags,
            PIPE_TYPE_BYTE | PIPE_READMODE_BYTE | PIPE_WAIT | PIPE_REJECT_REMOTE_CLIENTS,
            // 服务一个客户端时旧实例还开着，下一位的新实例必须能同时存在。
            // 「是否已有前台」靠 bind 时的 FILE_FLAG_FIRST_PIPE_INSTANCE 判定。
            PIPE_UNLIMITED_INSTANCES,
            BUFFER_SIZE,
            BUFFER_SIZE,
            PIPE_WAIT_MS,
            &attrs,
        )
    };
    if raw == INVALID_HANDLE_VALUE {
        return Err(last_error());
    }
    // SAFETY: 成功返回的 raw 是新建的独占管道句柄。
    Ok(unsafe { OwnedHandle::from_raw_handle(raw) })
}

pub fn bind(_cfg: &Config) -> Result<Bound, ServeError> {
    let sid = current_user_sid().map_err(ServeError::Io)?;
    let name = pipe_name(&sid);
    let instance = create_instance(&name, true).map_err(|err| match err.raw_os_error() {
        // first-instance 已存在时 Windows 可能回 BUSY 或 ACCESS_DENIED。
        Some(code) if code == ERROR_PIPE_BUSY as i32 || code == ERROR_ACCESS_DENIED as i32 => {
            ServeError::AlreadyRunning(PathBuf::from(&name))
        }
        _ => ServeError::Io(err),
    })?;
    Ok(Bound { name, instance })
}

// ---- 准入：对端 PID → SID，显式拒绝 ----

fn admit(instance: RawHandle, probe: &dyn lock::LockProbe) -> bool {
    // 「问不出来」一律拒绝；这与 unix 侧 peercred 失败就静默关同一口径。
    let Ok(server_sid) = current_user_sid() else {
        return false;
    };
    let mut pid = 0u32;
    if unsafe { GetNamedPipeClientProcessId(instance, &mut pid) } == 0 {
        return false;
    }
    let Some(client_sid) = process_user_sid(pid) else {
        return false;
    };
    if client_sid != server_sid {
        return false;
    }
    probe.locked() != Some(true)
}

pub fn serve_bound(
    bound: Bound,
    probe: Arc<dyn lock::LockProbe>,
    pool: Arc<SessionPool>,
    sessions: ScriptSessions,
) -> Result<(), ServeError> {
    let Bound { name, mut instance } = bound;
    loop {
        let connected = unsafe { ConnectNamedPipe(instance.as_raw_handle(), null_mut()) };
        if connected == 0 {
            let err = last_error();
            if err.raw_os_error() == Some(ERROR_NO_DATA as i32) {
                // 上一个客户端在 connect 前就走了：断开后继续等，别白白丢掉实例。
                unsafe { DisconnectNamedPipe(instance.as_raw_handle()) };
                continue;
            }
            return Err(ServeError::Io(err));
        }
        if !admit(instance.as_raw_handle(), probe.as_ref()) {
            // 显式拒绝：断开、换一个同名新实例；不读、不回、不进 serve_stdio。
            unsafe { DisconnectNamedPipe(instance.as_raw_handle()) };
            instance = create_instance(&name, false).map_err(ServeError::Io)?;
            continue;
        }
        // 句柄所有权转给 File，由 serve_stream 的线程读完关闭。
        let stream = unsafe { File::from_raw_handle(instance.into_raw_handle()) };
        let probe = Arc::clone(&probe);
        let pool = Arc::clone(&pool);
        let sessions = sessions.handle();
        std::thread::spawn(move || {
            let _ = serve_stream(stream, probe, pool, sessions);
        });
        // 同一个名字再开一个实例给下一位客户端（仍只走 create_instance 那一处绑定）。
        instance = create_instance(&name, false).map_err(ServeError::Io)?;
    }
}

// ---- 客户端：CreateFile + 有上限的 WaitNamedPipe 循环 ----

pub fn connect_client(path: &Path) -> io::Result<ClientStream> {
    let name = path.to_string_lossy();
    let wide_name = wide(&name);
    let deadline = Instant::now() + Duration::from_millis(PIPE_WAIT_MS as u64);
    loop {
        // SAFETY: wide_name NUL 结尾；同步句柄（不开 OVERLAPPED）可直接交给 File。
        let raw = unsafe {
            CreateFileW(
                wide_name.as_ptr(),
                GENERIC_READ | GENERIC_WRITE,
                0,
                null(),
                OPEN_EXISTING,
                0,
                null_mut(),
            )
        };
        if raw != INVALID_HANDLE_VALUE {
            return Ok(unsafe { File::from_raw_handle(raw) });
        }
        let err = last_error();
        match err.raw_os_error() {
            Some(code) if code == ERROR_FILE_NOT_FOUND as i32 || code == ERROR_PIPE_NOT_CONNECTED as i32 => {
                return Err(io::Error::new(io::ErrorKind::NotFound, err));
            }
            Some(code) if code == ERROR_PIPE_BUSY as i32 => {
                if Instant::now() >= deadline {
                    return Err(io::Error::new(
                        io::ErrorKind::NotFound,
                        format!("命名管道 {name} 在 {PIPE_WAIT_MS}ms 内一直忙：前台未运行或正忙"),
                    ));
                }
                unsafe { WaitNamedPipeW(wide_name.as_ptr(), CLIENT_WAIT_SLICE_MS) };
                std::thread::sleep(Duration::from_millis(10));
                continue;
            }
            _ => return Err(err),
        }
    }
}

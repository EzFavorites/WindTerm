//! 控制端点的传输层：全平台唯一的 loopback TCP 绑定 + key 文件 + 挑战证明。
//!
//! # 为什么不是 UDS / 命名管道
//!
//! 那条路把「谁在监听」和「谁能连」都绑在平台原语上（unix 的 0600 + peercred、
//! Windows 的 DACL + PID→SID），两套代码、两套判据，永远要证明它们安全等价。
//! 这里改成一条**平台中立**的路：只监听 `127.0.0.1:<port>`，准入靠两样东西——
//!
//! 1. `~/.zterm/control-key.local-<port>`：256-bit 随机 hex，unix 0600，
//!    Windows 落在用户目录里继承该目录的 ACL；只有同一用户读得到；
//! 2. 传输级的 `auth.challenge` / `auth.proof` / `auth.key` 三帧：客户端先发
//!    nonce，服务端用 HMAC-SHA256 签名证明自己持有同一把 key，客户端验过证明
//!    **之后**才回送 key。服务端比对一致才进能力面。
//!
//! 这三帧是**传输外壳**，不是能力面的 `auth` 方法：它们没有 `jsonrpc` / `id`
//! 字段，在 `serve_stdio` 之前就被消费掉，绝不出现在 `describe` / `METHODS` 里。
//! 旧 ADR-0022 的 TCP + token 握手正是被这套挑战证明替换掉的形态，别把
//! 「有 TCP」本身当成新判据。
//!
//! # 绑定失败不影响 GUI
//!
//! [`bind`] 只做一件事：`TcpListener::bind(("127.0.0.1", port))`，紧接着写 key
//! 文件。端口被占 / 家目录不可用时返回 `ServeError`，由 `start_control_endpoint`
//! 打到 stderr 后返回——窗口照常起来，控制面只是附加能力。

use std::fs::{self, OpenOptions};
use std::io::{self, BufRead, BufReader, BufWriter, Write};
use std::net::{IpAddr, TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use hmac::{Hmac, KeyInit, Mac};
use serde_json::{json, Value};
use sha2::Sha256;
use term_script::{CancelToken, ScriptSessions};
use term_session::SessionPool;

use super::lock;
use super::{LockedGate, ServeError};

/// 默认控制端口。环境变量 `WINDTERM_CONTROL_PORT` 可覆盖；0 与非法值回落默认。
pub const DEFAULT_CONTROL_PORT: u16 = 47_653;
pub const PORT_ENV: &str = "WINDTERM_CONTROL_PORT";

/// key 文件所在的目录名与文件名前缀。端口在文件名里：不同端口的 key 互不串。
pub const KEY_DIR: &str = ".zterm";
pub const KEY_FILE_PREFIX: &str = "control-key.local-";
/// 全平台唯一的监听地址。绑定点只有 [`bind`] 里那一处。
pub const LOOPBACK_IP: &str = "127.0.0.1";

/// 单行上限 1 MiB；超限回一条解析错误并**关连接**（与 stdio 的「丢行继续」不同）。
pub const LINE_LIMIT: usize = 1024 * 1024;
/// 首请求（含三帧准入）的墙钟上限。读到第一条 RPC 请求行后即解除。
pub const FIRST_REQUEST_TIMEOUT: Duration = Duration::from_secs(30);

/// 传输外壳的三个字段名。刻意带点号且没有 `jsonrpc` / `id`，不会混进能力面。
pub const CHALLENGE_FIELD: &str = "auth.challenge";
pub const PROOF_FIELD: &str = "auth.proof";
pub const KEY_FIELD: &str = "auth.key";

type HmacSha256 = Hmac<Sha256>;

/// 一次控制端点的配置。生产路径只有 defaulted() 一个取值；测试用临时端口。
#[derive(Clone, Debug)]
pub struct Config {
    pub port: u16,
}

impl Config {
    pub fn new(port: u16) -> Self {
        Self { port }
    }

    pub fn defaulted() -> Self {
        Self::new(control_port())
    }
}

/// 端口解析的纯函数形态，便于不碰环境变量地钉住覆盖口径。
pub fn parse_port(text: &str) -> Option<u16> {
    text.trim().parse::<u16>().ok().filter(|port| *port != 0)
}

pub fn control_port() -> u16 {
    std::env::var(PORT_ENV).ok().and_then(|text| parse_port(&text)).unwrap_or(DEFAULT_CONTROL_PORT)
}

/// 客户端 / 服务端双方唯一的 key 文件会合点。
pub fn key_path(port: u16) -> Option<PathBuf> {
    dirs::home_dir().map(|home| home.join(KEY_DIR).join(format!("{KEY_FILE_PREFIX}{port}")))
}

/// 兼容只读设置页沿用的旧名字：现在返回的是 key 文件路径，不再是 socket 路径。
pub fn default_socket_path() -> Option<PathBuf> {
    key_path(control_port())
}

/// CLI / MCP 用法里展示的端点描述。不硬编码到命令实现里。
pub fn endpoint_description() -> String {
    format!(
        "{}:{}（loopback TCP；{} key 文件 + HMAC-SHA256 挑战证明准入；端口可用 {} 覆盖）",
        LOOPBACK_IP,
        control_port(),
        KEY_FILE_PREFIX,
        PORT_ENV
    )
}

// ---- key 的生成 / 落盘 / 读取 ----

fn hex(bytes: &[u8]) -> String {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        out.push(DIGITS[(byte >> 4) as usize] as char);
        out.push(DIGITS[(byte & 0x0f) as usize] as char);
    }
    out
}

pub(crate) fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut diff = 0u8;
    for (left, right) in a.iter().zip(b) {
        diff |= left ^ right;
    }
    diff == 0
}

/// 256-bit（32 字节）随机 key 的 hex 文本。复用已有的 `getrandom` 叶子依赖，
/// Windows 后端是 `RtlGenRandom`，不引新依赖。
pub(crate) fn random_hex() -> io::Result<String> {
    let mut bytes = [0u8; 32];
    getrandom::getrandom(&mut bytes).map_err(io::Error::other)?;
    Ok(hex(&bytes))
}

/// 挑战证明的签名输入。域分隔前缀让这份 HMAC 不能挪作别的用途。
pub(crate) fn proof_message(nonce: &str) -> Vec<u8> {
    format!("windterm-control-proof-v1|{nonce}").into_bytes()
}

pub(crate) fn hmac_hex(key: &[u8], message: &[u8]) -> io::Result<String> {
    let mut mac = HmacSha256::new_from_slice(key).map_err(io::Error::other)?;
    mac.update(message);
    Ok(hex(&mac.finalize().into_bytes()))
}

/// 落盘：unix 显式 0600（即使旧文件权限被改过也收紧）；Windows 靠用户目录 ACL。
pub(crate) fn write_key_file(path: &Path, key: &str) -> io::Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(parent, fs::Permissions::from_mode(0o700))?;
        }
        #[cfg(not(unix))]
        let _ = parent;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
        let mut file = OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .mode(0o600)
            .open(path)?;
        file.write_all(key.as_bytes())?;
        file.flush()?;
        // `create` 不会改已存在文件的权限：旧 key 可能遗留了更宽的 mode。
        fs::set_permissions(path, fs::Permissions::from_mode(0o600))?;
    }
    #[cfg(not(unix))]
    {
        // 文件建在 %USERPROFILE%\.zterm 下，继承用户目录只放行当前用户
        // （以及管理员 / System）的 ACL；不引 windows-sys，也不手工构造 DACL。
        let mut file = OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .open(path)?;
        file.write_all(key.as_bytes())?;
        file.flush()?;
    }
    Ok(())
}

/// 客户端读 key。格式不对（半截写入 / 被人塞了别的东西）按拒绝处理。
pub(crate) fn read_key(path: &Path) -> io::Result<String> {
    let text = fs::read_to_string(path)?;
    let key = text.trim();
    if key.len() != 64 || !key.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("{} 里的控制 key 不是 64 位 hex", path.display()),
        ));
    }
    Ok(key.to_ascii_lowercase())
}

// ---- 传输外壳：三帧挑战证明 ----

pub(crate) fn write_frame<W: Write>(writer: &mut W, frame: &Value) -> io::Result<()> {
    let line = serde_json::to_string(frame).map_err(io::Error::other)?;
    writer.write_all(line.as_bytes())?;
    writer.write_all(b"\n")?;
    writer.flush()
}

/// 一行读结果。`TooLong` 必须让调用方回解析错误并关连接。
#[derive(Debug, PartialEq)]
pub(crate) enum LineRead {
    Line,
    TooLong,
    Eof,
}

/// 带 1 MiB 硬上限地读一行：不靠 `read_line` 无界累积。
pub(crate) fn read_capped_line<R: BufRead>(reader: &mut R, out: &mut Vec<u8>) -> io::Result<LineRead> {
    loop {
        let buffered = reader.fill_buf()?;
        if buffered.is_empty() {
            return Ok(if out.is_empty() { LineRead::Eof } else { LineRead::Line });
        }
        if let Some(at) = buffered.iter().position(|&byte| byte == b'\n') {
            if out.len() + at + 1 > LINE_LIMIT {
                return Ok(LineRead::TooLong);
            }
            out.extend_from_slice(&buffered[..=at]);
            reader.consume(at + 1);
            return Ok(LineRead::Line);
        }
        if out.len() + buffered.len() > LINE_LIMIT {
            return Ok(LineRead::TooLong);
        }
        let len = buffered.len();
        out.extend_from_slice(buffered);
        reader.consume(len);
    }
}

pub(crate) fn read_frame<R: BufRead>(reader: &mut R) -> io::Result<Option<Value>> {
    let mut line = Vec::new();
    if read_capped_line(reader, &mut line)? != LineRead::Line {
        return Ok(None);
    }
    let text = String::from_utf8_lossy(&line);
    Ok(serde_json::from_str(text.trim()).ok())
}

fn nonce_of(field: &Value) -> Option<&str> {
    field.as_str().or_else(|| field.get("nonce").and_then(Value::as_str))
}

/// 服务端准入：loopback 对端 → 未锁屏 → 发证明 → 收 key 并比对。
/// 任一步失败都静默关（不回协议错误、不泄露端点细节）。
fn admit<R: BufRead, W: Write>(
    reader: &mut R,
    writer: &mut W,
    key: &[u8],
    probe: &dyn lock::LockProbe,
) -> io::Result<bool> {
    if probe.locked() == Some(true) {
        return Ok(false);
    }
    let Some(challenge) = read_frame(reader)? else {
        return Ok(false);
    };
    let Some(nonce) = challenge.get(CHALLENGE_FIELD).and_then(nonce_of) else {
        return Ok(false);
    };
    // 服务端用 HMAC-SHA256 签名证明自己持有 key；客户端验过之后才会回送 key。
    let signature = hmac_hex(key, &proof_message(nonce))?;
    write_frame(
        writer,
        &json!({PROOF_FIELD: {"alg": "hmac-sha256", "signature": signature}}),
    )?;
    let Some(reply) = read_frame(reader)? else {
        return Ok(false);
    };
    let Some(client_key) = reply.get(KEY_FIELD).and_then(|field| field.as_str()).or_else(|| {
        reply.get(KEY_FIELD).and_then(|field| field.get("key")).and_then(Value::as_str)
    }) else {
        return Ok(false);
    };
    // key 传进来的就是 hex 文本的字节，不能再编码一次。
    Ok(constant_time_eq(client_key.as_bytes(), key))
}

/// 对端必须是 loopback：127.0.0.0/8 或 ::1。问不出来一律拒绝。
pub(crate) fn ip_is_loopback(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => v4.octets()[0] == 127,
        IpAddr::V6(v6) => v6.is_loopback(),
    }
}

fn peer_is_loopback(stream: &TcpStream) -> bool {
    let Ok(peer) = stream.peer_addr() else {
        return false;
    };
    let Ok(local) = stream.local_addr() else {
        return false;
    };
    ip_is_loopback(peer.ip()) && ip_is_loopback(local.ip())
}

// ---- 绑定与 accept ----

/// 已绑定的端点。Drop 时尽力删掉自己写的 key 文件（进程异常退出则由下次绑定覆盖）。
pub struct Bound {
    listener: Arc<TcpListener>,
    key: Arc<str>,
    key_path: Arc<PathBuf>,
}

impl Drop for Bound {
    fn drop(&mut self) {
        let _ = fs::remove_file(self.key_path.as_path());
    }
}

impl Bound {
    /// 测试夹具用：把临时 listener / 临时 key 路径装进同一个生命周期。
    pub(crate) fn from_parts(listener: TcpListener, key: String, key_path: PathBuf) -> Self {
        Self {
            listener: Arc::new(listener),
            key: Arc::from(key),
            key_path: Arc::new(key_path),
        }
    }
}

/// 全仓**唯一**的控制端点绑定调用点。端口被占 / 家目录不可用只报错。
pub fn bind(cfg: &Config) -> Result<Bound, ServeError> {
    let listener = TcpListener::bind((LOOPBACK_IP, cfg.port)).map_err(ServeError::Io)?;
    let key_path = key_path(cfg.port).ok_or(ServeError::NoHome)?;
    let key = random_hex().map_err(ServeError::Io)?;
    write_key_file(&key_path, &key).map_err(ServeError::Io)?;
    Ok(Bound::from_parts(listener, key, key_path))
}

/// 连续 accept 失败多少次就认定监听器废了（不计数会 100% CPU 空转）。
pub(crate) const MAX_CONSECUTIVE_ACCEPT_ERRORS: u32 = 128;

/// accept 循环：一条连接一个线程，某个客户端卡住不会挡住别人。
pub fn serve_bound(
    bound: Bound,
    probe: Arc<dyn lock::LockProbe>,
    pool: Arc<SessionPool>,
    sessions: ScriptSessions,
) -> Result<(), ServeError> {
    let listener = Arc::clone(&bound.listener);
    let key = Arc::clone(&bound.key);
    let mut consecutive_errors = 0u32;
    for incoming in listener.incoming() {
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
        if !peer_is_loopback(&stream) {
            continue;
        }
        let key = Arc::clone(&key);
        let probe = Arc::clone(&probe);
        let pool = Arc::clone(&pool);
        // 宿主级登记处留在签名里（脚本侧共用）。连接游标按连接新建：只有
        // 这样每条连接注入的 CancelToken 才会真正落到自己的 ScriptSession。
        let _ = sessions;
        std::thread::spawn(move || {
            let _ = serve_connection(stream, key.as_bytes(), probe, pool);
        });
    }
    Ok(())
}

/// 一条连接：准入之后**读行与派发分线程**。
///
/// 这是 ADR §4「连接断开即取消」的结构前提：如果读行与 `dispatch` 在同一
/// 线程，一条 `wait` 把它卡住之后，它就再也看不见对端 EOF，取消令牌永远
/// 等不到信号，退化成「等 timeout_ms 到期」。现在读线程一发现 EOF / IO
/// 错误就 drop 通道并请求取消，工作线程里正在阻塞的 `wait` / `read` 会在
/// 下一个 50ms 切片退出（未取消的旧路径仍保持原语义）。
fn serve_connection(
    stream: TcpStream,
    key: &[u8],
    probe: Arc<dyn lock::LockProbe>,
    pool: Arc<SessionPool>,
) -> io::Result<()> {
    stream.set_read_timeout(Some(FIRST_REQUEST_TIMEOUT))?;
    let mut writer = BufWriter::new(stream.try_clone()?);
    let mut reader = BufReader::new(stream.try_clone()?);
    if !admit(&mut reader, &mut writer, key, probe.as_ref())? {
        return Ok(());
    }

    let cancel = CancelToken::new();
    let _cancel_on_return = CancelOnDrop(cancel.clone());
    let (tx, rx) = std::sync::mpsc::channel::<RequestLine>();
    let worker_cancel = cancel.clone();
    let worker = std::thread::spawn(move || {
        serve_lines(rx, &mut writer, pool, worker_cancel, probe)
    });

    let mut first = true;
    loop {
        let mut line = Vec::new();
        let request = match read_capped_line(&mut reader, &mut line)? {
            LineRead::Eof => break,
            LineRead::TooLong => {
                let _ = tx.send(RequestLine::TooLong);
                break;
            }
            LineRead::Line => {
                if first {
                    first = false;
                    // 首条请求行到手才解除 30 秒首请求上限；wait 可以等很久。
                    stream.set_read_timeout(None)?;
                }
                RequestLine::Line(line)
            }
        };
        if tx.send(request).is_err() {
            break;
        }
    }
    // 先 cancel 再 drop 发送端 → 工作线程里还没落地的 wait / read 会立刻
    // 走 *_cancellable 的 Cancelled 分支，然后读完队列剩余请求自然结束。
    // 顺序不能反：join 等的是工作线程，令牌若等 guard 在 return 时才取消，
    // 就会在这里先卡到 wait 的 timeout_ms 到期（原 P1 的形状）。
    cancel.cancel();
    drop(tx);
    let _ = worker.join();
    Ok(())
}

/// 读线程交给工作线程的一格。超长行走专门分支：回 parse_error 后关连接。
pub(crate) enum RequestLine {
    Line(Vec<u8>),
    TooLong,
}

/// 连接结束（含 EOF / IO 错误）时无条件请求取消。
///
/// 放在 drop 上而不是在几个 `return Ok(())` 处手工调用：新增提前返回的路径
/// 也自动满足「断连即取消」，不会因为漏写一个分支而重新长出超时黑洞。
struct CancelOnDrop(CancelToken);

impl Drop for CancelOnDrop {
    fn drop(&mut self) {
        self.0.cancel();
    }
}

/// 与 `term_bridge::transport::serve_stdio` 同协议（同一份 `dispatch`），
/// 但超长行回解析错误**并关连接**，而不是丢行继续。
pub(crate) fn serve_lines<W: Write>(
    requests: std::sync::mpsc::Receiver<RequestLine>,
    writer: &mut W,
    pool: Arc<SessionPool>,
    cancel: CancelToken,
    probe: Arc<dyn lock::LockProbe>,
) -> io::Result<()> {
    let _cancel_on_drop = CancelOnDrop(cancel.clone());
    let gated = LockedGate {
        inner: super::surface::DeskSurface::with_cancel(pool, ScriptSessions::new(), cancel),
        probe,
    };
    for request in requests {
        match request {
            RequestLine::TooLong => {
                // 空串一定解析失败 → id:null 的 parse_error，写完后关连接。
                if let Some(reply) = term_bridge::dispatch(&gated, "") {
                    writer.write_all(reply.as_bytes())?;
                    writer.write_all(b"\n")?;
                    writer.flush()?;
                }
                return Ok(());
            }
            RequestLine::Line(line) => {
                let text = String::from_utf8_lossy(&line);
                let text = text.trim_end_matches('\n').trim_end_matches('\r');
                if let Some(reply) = term_bridge::dispatch(&gated, text) {
                    writer.write_all(reply.as_bytes())?;
                    writer.write_all(b"\n")?;
                    writer.flush()?;
                }
            }
        }
    }
    Ok(())
}

//! `desk serve`：把能力面暴露到本机 TCP（ADR-0022）。
//!
//! 桥接协议本身与传输无关——`term-bridge::transport::serve_stdio` 只要求
//! `R: BufRead, W: Write`，所以同一条管道既能接 stdio 也能接 socket。
//! 本模块提供的是 **stdio 没有的那一圈**：网络、凭证、锁屏门。
//!
//! # 为什么鉴权在外壳而不是协议里
//!
//! stdio 管道的对端天然是「拉起我的那个进程」，不存在冒充者；TCP 不是。
//! 把 `auth` 塞进 `term_core::surface::METHODS` 会污染能力面（那八条是
//! **会话能力**，`auth` 是**传输外壳**的准入），所以它是本模块的一条前置
//! 握手，`describe` 里不会出现它。
//!
//! # 一条连接的顺序
//!
//! 1. 锁屏？→ 直接关（连错误都不回：回了等于告诉对方「这里有个桥接」）；
//! 2. 第一条必须 `{"method":"auth","params":{"token":"…"}}`，答 `{"result":{"ok":true}}`；
//!    不是 `auth` 或 token 不对 → 回一个 `locked`（-32003）错误并关连接，**一次机会**；
//! 3. 之后交给 `serve_stdio`，逐行一问一答；
//! 4. 每次调用前再问一次锁屏（纵深防御）：中途锁屏的会话立刻开始拒绝，
//!    但**不主动断连接**——断不断由客户端决定，我们只拒绝执行。
//!
//! # 本 ADR 显式承认的限制
//!
//! `desk serve` 与 GUI 是**两个进程，各自持有自己的 `SessionPool`**：通过
//! `desk serve` 打开的 tab 不会出现在正在运行的窗口里。跨进程共享要等一个
//! broker，现在不假装支持（`session_list` 只列本进程的）。

pub mod lock;
pub mod surface;
pub mod token;

use std::io::{BufRead, BufReader, BufWriter, Write};
use std::net::{Ipv4Addr, Shutdown, TcpListener, TcpStream};
use std::path::PathBuf;
use std::sync::Arc;

use serde_json::{json, Value};

use term_bridge::error::SurfaceError;
use term_bridge::handler::Surface;
use term_bridge::jsonrpc::ErrorObject;
use term_session::SessionPool;

/// 握手方法名。**不在** `term_core::surface::METHODS` 里（见模块文档）。
pub const AUTH_METHOD: &str = "auth";

/// 默认端口。选一个高位、没有知名服务占用的号。
pub const DEFAULT_PORT: u16 = 47653;

/// 一次 `serve` 的配置。
#[derive(Clone, Debug)]
pub struct Config {
    /// `0` 表示让内核分配（测试与「不想记端口」的场景）。
    pub port: u16,
    pub token_path: PathBuf,
}

impl Config {
    pub fn new(port: u16, token_path: PathBuf) -> Self {
        Self { port, token_path }
    }

    /// 默认配置：端口 [`DEFAULT_PORT`]，token 在 `~/.zterm/bridge.token`。
    pub fn defaulted() -> Result<Self, ServeError> {
        let path = token::default_path().ok_or(ServeError::NoHome)?;
        Ok(Self::new(DEFAULT_PORT, path))
    }
}

#[derive(Debug)]
pub enum ServeError {
    /// 找不到家目录——`~/.zterm/bridge.token` 无处安放。
    NoHome,
    Token(token::TokenError),
    Io(std::io::Error),
}

impl std::fmt::Display for ServeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ServeError::NoHome => write!(f, "定位不到家目录，无法决定 token 放在哪里"),
            ServeError::Token(err) => write!(f, "{err}"),
            ServeError::Io(err) => write!(f, "网络层失败：{err}"),
        }
    }
}

impl std::error::Error for ServeError {}

/// 已绑定但还没开始 accept 的服务。
///
/// 拆出这一步是为了让「端口是多少」可读：`--port 0` 时真实端口由内核决定，
/// 只有 bind 之后才知道。测试也靠它拿端口。
pub struct Bound {
    listener: TcpListener,
    pub port: u16,
    pub token: String,
}

/// 读/建 token，并绑到 `127.0.0.1`。
///
/// **只绑回环**：没有 `--host` 这个参数是刻意的。要远程访问请走 SSH 隧道
/// （`ssh -L 47653:127.0.0.1:47653 host`），那是已经被审计过的通道；
/// 让本服务直接监听 `0.0.0.0` 等于把终端能力面挂到局域网上。
pub fn bind(cfg: &Config) -> Result<Bound, ServeError> {
    let token = token::load_or_create(&cfg.token_path).map_err(ServeError::Token)?;
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, cfg.port)).map_err(ServeError::Io)?;
    let port = listener.local_addr().map_err(ServeError::Io)?.port();
    Ok(Bound {
        listener,
        port,
        token,
    })
}

/// `desk serve` 的阻塞入口：绑定、报一行监听信息、然后一直服务。
pub fn serve(cfg: Config) -> Result<(), ServeError> {
    let bound = bind(&cfg)?;
    // 这一行是给「端口由内核分配」的场景用的：脚本从 stdout 抓它。
    // stdout 不是协议流（协议在 TCP 上），所以往这里写日志不破坏任何东西。
    println!(
        "term-desk serve 监听 127.0.0.1:{}（token：{}）",
        bound.port,
        cfg.token_path.display()
    );
    let _ = std::io::stdout().flush();
    // 锁屏探测带 2 秒缓存：每次能力面调用都 fork 一次 `ioreg` 会把
    // 安全检查变成吞吐税（详见 `lock::Cached`）。
    let probe = lock::Cached::new(lock::Ioreg, std::time::Duration::from_secs(2));
    serve_on(bound, Arc::new(probe))
}

/// 连续 accept 失败多少次就认定监听器废了。
///
/// `incoming()` 在监听器坏掉后会**立刻**返回 `Err`，不计数就是一个 100% CPU
/// 的空转循环——它既不崩也不报错，比崩溃更难被发现。
const MAX_CONSECUTIVE_ACCEPT_ERRORS: u32 = 128;

/// accept 循环。一个连接一个线程。
///
/// 协议本身是串行的（一条连接 = 一个事实流），线程只为隔离「某个客户端卡在
/// `wait` 上」——不能让一个卡住的脚本挡掉其他客户端。
pub fn serve_on(bound: Bound, probe: Arc<dyn lock::LockProbe>) -> Result<(), ServeError> {
    let pool = Arc::new(SessionPool::new());
    // W1-c：`desk serve` 是无窗口的，内置 SSH 会话只能靠控制面自己的
    // 通道工厂开起来（没有对话框可以问凭据，所以工厂只认 Agent / 私钥）。
    crate::session_open::install_channel_factory(&pool);
    let mut consecutive_errors = 0u32;
    for incoming in bound.listener.incoming() {
        let stream = match incoming {
            Ok(stream) => {
                consecutive_errors = 0;
                stream
            }
            Err(err) => {
                // 单条连接握手失败不该杀掉服务（客户端可以重连），
                // 但连续失败说明监听器本身不可用了。
                consecutive_errors += 1;
                if consecutive_errors >= MAX_CONSECUTIVE_ACCEPT_ERRORS {
                    return Err(ServeError::Io(err));
                }
                continue;
            }
        };
        let token = bound.token.clone();
        let probe = Arc::clone(&probe);
        let pool = Arc::clone(&pool);
        std::thread::spawn(move || {
            let _ = handle(stream, &token, pool, probe);
        });
    }
    Ok(())
}

fn handle(
    stream: TcpStream,
    token: &str,
    pool: Arc<SessionPool>,
    probe: Arc<dyn lock::LockProbe>,
) -> std::io::Result<()> {
    if probe.locked() == Some(true) {
        // 锁屏：连握手都不给。不回错误——回错误等于确认「这里有个桥接」。
        let _ = stream.shutdown(Shutdown::Both);
        return Ok(());
    }
    stream.set_nodelay(true)?;
    let mut writer = BufWriter::new(stream.try_clone()?);
    let mut reader = BufReader::new(stream);
    if !authenticate(&mut reader, &mut writer, token)? {
        return Ok(());
    }
    let gated = LockedGate {
        inner: surface::DeskSurface::new(pool),
        probe,
    };
    term_bridge::transport::serve_stdio(&gated, reader, &mut writer)
}

/// 读第一条请求并答复。返回 `true` 表示已认证、连接可以继续。
fn authenticate<R: BufRead, W: Write>(
    reader: &mut R,
    writer: &mut W,
    token: &str,
) -> std::io::Result<bool> {
    let mut line = String::new();
    if reader.read_line(&mut line)? == 0 {
        // 连上就挂（探测端口是否在听）：正常，不是错误。
        return Ok(false);
    }
    let (reply, ok) = auth_reply(line.trim_end_matches(['\n', '\r']), token);
    writer.write_all(reply.as_bytes())?;
    writer.write_all(b"\n")?;
    writer.flush()?;
    Ok(ok)
}

/// 握手判定（纯函数，可单测）。
///
/// 返回 `(回给客户端的整行 JSON, 是否通过)`。不通过时**只给一次机会**：
/// 调用方拿到 `false` 就关连接，不做重试计数——256 bit 的 token 不值得为
/// 它维护限流状态机，而「一次机会」让暴力尝试必须重新建连。
pub fn auth_reply(line: &str, token: &str) -> (String, bool) {
    let value: Value = match serde_json::from_str(line) {
        Ok(value) => value,
        Err(err) => {
            let response = error_response(Value::Null, SurfaceError::parse_error(err.to_string()));
            return (response, false);
        }
    };
    let id = value.get("id").cloned().unwrap_or(Value::Null);
    let method = value.get("method").and_then(Value::as_str).unwrap_or("");
    if method != AUTH_METHOD {
        let response = error_response(
            id,
            SurfaceError::locked(format!(
                "未认证：一条连接的第一条请求必须是 {AUTH_METHOD}，实际是 {method:?}"
            )),
        );
        return (response, false);
    }
    let got = value
        .get("params")
        .and_then(|params| params.get("token"))
        .and_then(Value::as_str)
        .unwrap_or("");
    if !token::tokens_match(token, got) {
        let response = error_response(
            id,
            SurfaceError::locked("token 不匹配；正确的值在服务端启动时打印的 token 文件里"),
        );
        return (response, false);
    }
    (
        json!({"jsonrpc": "2.0", "id": id, "result": {"ok": true}}).to_string(),
        true,
    )
}

/// 与 `jsonrpc::dispatch` 的失败信封同形（`{code, message, kind}`）。
fn error_response(id: Value, err: SurfaceError) -> String {
    json!({
        "jsonrpc": "2.0",
        "id": id,
        "error": ErrorObject::from(&err).to_value(),
    })
    .to_string()
}

/// 锁屏门：包一层后端，每次调用前问一次锁屏。
///
/// 不主动断连接：客户端可能只是暂时拿不到结果，等用户回来解锁再继续，
/// 比「连接莫名其妙断了」更好排障。拒绝本身带 `locked`（-32003）这个可读分类。
struct LockedGate {
    inner: surface::DeskSurface,
    probe: Arc<dyn lock::LockProbe>,
}

impl Surface for LockedGate {
    fn call(&self, method: &str, params: &Value) -> Result<Value, SurfaceError> {
        if self.probe.locked() == Some(true) {
            return Err(SurfaceError::locked(
                "屏幕已锁定：desk serve 在锁屏期间不执行任何能力面调用",
            ));
        }
        self.inner.call(method, params)
    }
}

#[cfg(test)]
#[path = "mod_tests.rs"]
mod tests;

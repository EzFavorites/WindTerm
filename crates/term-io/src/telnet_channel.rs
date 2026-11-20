//! 原生 Telnet 通道：把 [`term_core::telnet`] 的协商语义接到真实 TCP 上（W6-C1）。
//!
//! W6-C1 的前半段（语义层）早已写好却零消费者：`Protocol::Telnet` 一直经
//! `spawn_plan()` 拉系统 `telnet` 客户端塞进 PTY。这里补上另一半——用
//! `std::net::TcpStream` 直连，把「原始字节流 → 纯数据 + 应答」真正拆开：
//!
//! ```text
//! [读线程]  socket.read  →  TelnetNegotiator::feed(chunk)
//!                            ├─ data   → mpsc → drain() → VT
//!                            └─ reply  → 立刻写回 socket
//! [Supervisor] write()   → 数据里的 0xFF 双写 → socket
//!               resize() → set_window_size() → NAWS 子协商 → socket
//! ```
//!
//! # 三条硬约束
//!
//! 1. **只用 std**。不引 tokio / 任何异步面（ADR-0017 的「tokio 笼子」），
//!    也不改 `Cargo.toml`。协商状态机本身是纯语义、零 IO，放在 `term-core`。
//! 2. **协商字节绝不进数据流**。`drain()` 只从读线程派发的 `data` 里取字节；
//!    `IAC ...` / `SB ... SE` 在语义层就被剥掉，`IAC IAC` 在那里还原成单个
//!    `0xFF`。本文件从不把裸 `read()` 的结果交给调用方。
//! 3. **`drain()` 不碰 IO**。它只从 `mpsc::Receiver::try_recv` 取已经到手的
//!    数据；socket 上的等待全部发生在读线程里（每次 `read` 带 50ms 超时，
//!    好让 `close()` 能及时叫停线程）。
//!
//! # 与串口通道的异同
//!
//! 相同：进程内直连、后台读线程、失败**不回退**系统客户端、RX/TX 计数、
//! 断开时 `poll()` 报一次。不同：Telnet 有窗口尺寸（NAWS），`resize()` 会
//! 真的往线上写字；写方向的数据要按 RFC 854 §3 把 `0xFF` 双写转义。
//!
//! # NAWS 的发送时机（RFC 1073）
//!
//! - `open()` 先用 `set_window_size(cols, rows)` **记住**初始尺寸但不发送：
//!   对端还没发 `DO NAWS`。等它发来时，语义层会在 `reply` 里自动补上当前
//!   尺寸（`TelnetNegotiator::negotiate` 的 NAWS 分支），读线程照常写回。
//! - `resize()` 更新尺寸；只有对端已同意 NAWS（`local_enabled(NAWS)`）时才
//!   立即把新的子协商写回，否则仍由上面的「等到 `DO NAWS` 再补发」路径负责。
//!   这样既不会在对端拒绝/未协商时发非法 SB，也不会漏掉初始尺寸。

use std::fmt;
use std::io::{ErrorKind, Read, Write};
use std::net::{Shutdown, SocketAddr, TcpStream, ToSocketAddrs};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{mpsc, Arc, Mutex, MutexGuard};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use term_core::session::{Protocol, SessionConfig};
use term_core::telnet::{TelnetNegotiator, IAC, NAWS};

use crate::channel::TermChannel;

/// 读线程一次 `read` 的上限。终端流量是小块，8 KiB 足够且不会撑爆一帧。
const READ_CHUNK: usize = 8 * 1024;

/// 读超时：读线程每 50ms 至少醒来一次看停止标志，`close()` 才不会干等。
const READ_TIMEOUT: Duration = Duration::from_millis(50);

/// 写超时：对端不读、内核发送缓冲塞满时，`write_all` 不能把 Supervisor 线程
/// 一起钉住。出方向都是小块（键入 / reply / NAWS），500ms 足够宽裕。
const WRITE_TIMEOUT: Duration = Duration::from_millis(500);

/// 连接超时的夹逼范围（秒）。`SessionConfig::connection_timeout` 默认 20s，
/// 对 UI 启动来说太久；这里保留用户配置但夹到 1..=10 秒。
const MIN_CONNECT_SECS: u32 = 1;
const MAX_CONNECT_SECS: u32 = 10;

/// 打开原生 Telnet 通道失败的原因。
///
/// 与 [`crate::error::SpawnError`] 的分工：这里是**连接内部**的细节（含主机名、
/// 端口、OS 错误），`open_channel` 再把它折成 `SpawnError::Telnet(String)`。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TelnetError {
    /// 主机名为空（正常路径会被 `spawn_plan()` 的 `MissingHost` 先挡下）。
    MissingHost,
    /// 主机名解析不出地址。
    Address {
        host: String,
        port: u16,
        detail: String,
    },
    /// 所有解析出的地址都连不上。
    Connect {
        host: String,
        port: u16,
        detail: String,
    },
    /// 连上了但设置 socket / 起读线程失败。
    Setup { detail: String },
}

impl fmt::Display for TelnetError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            TelnetError::MissingHost => write!(f, "Telnet 会话缺少主机名"),
            TelnetError::Address { host, port, detail } => {
                write!(f, "无法解析 {host}:{port}：{detail}")
            }
            TelnetError::Connect { host, port, detail } => {
                write!(f, "{host}:{port} 连接失败：{detail}")
            }
            TelnetError::Setup { detail } => write!(f, "初始化 Telnet 连接失败：{detail}"),
        }
    }
}

impl std::error::Error for TelnetError {}

/// 读线程 → 通道的事件。`Data` 是**已剥掉协商**的纯数据；`End` 只发一次。
enum ReaderEvent {
    Data(Vec<u8>),
    End(String),
}

/// 一条原生 Telnet TCP 连接，实现 [`TermChannel`]。
///
/// 所有 socket 等待都在读线程里；`drain()` / `write()` / `resize()` 由
/// Supervisor 线程独占调用（trait 契约），本结构体因此 `Send`。
pub struct TelnetChannel {
    /// 读线程派发的数据与结束事件。
    events: mpsc::Receiver<ReaderEvent>,
    /// `drain()` 之间攒着的纯数据字节。
    pending: Vec<u8>,
    /// 断开原因，`poll()` 取走即清空。
    notice: Option<String>,
    /// 生命周期是否已结束（对端断开 / 写失败 / `close()`）。
    over: bool,
    /// 用户显式 `close()` 过：之后 `poll()` 恒 `None`，不再报第二次原因。
    closed: bool,
    /// 协商状态机。读线程 `feed()` 与 `resize()` 共用一把锁，保证命令不会
    /// 被拆到两个线程里各解析一半。
    negotiator: Arc<Mutex<TelnetNegotiator>>,
    /// 写半边。读线程写 `reply`、Supervisor 写输入 / NAWS 都经这把锁串行化，
    /// 避免两条 `write_all` 在 TCP 流上交错。
    writer: Arc<Mutex<TcpStream>>,
    /// 线上收到的**原始**字节数（含协商命令，不是 VT 看到的数据量）。
    rx_bytes: Arc<AtomicU64>,
    /// 线上发出的**原始**字节数（含 reply / NAWS）。
    tx_bytes: Arc<AtomicU64>,
    /// 通知读线程收摊。
    stop: Arc<AtomicBool>,
    /// 读线程句柄，`close()` / `Drop` 时 join。
    reader: Option<JoinHandle<()>>,
    /// `telnet <host>:<port>`，状态栏与诊断用。
    description: String,
}

impl TelnetChannel {
    /// 连接 `config.host:config.port` 并起读线程。
    ///
    /// `port == 0` 时回落到 [`Protocol::default_port`]（Telnet = 23）：老配置
    /// 里端口没填不该变成「连 0 端口」。`cols` / `rows` 只用于记住初始窗口
    /// 尺寸，不会在对端 `DO NAWS` 之前往线上发任何字节。
    pub fn open(config: &SessionConfig, cols: usize, rows: usize) -> Result<Self, TelnetError> {
        let host = config.host.trim();
        if host.is_empty() {
            return Err(TelnetError::MissingHost);
        }
        let port: u16 = if config.port == 0 {
            Protocol::Telnet.default_port()
        } else {
            config.port
        };
        let description = format!("telnet {host}:{port}");

        // 先解析再逐个 connect_timeout：DNS 失败与连接失败要分开报，且每个地址
        // 都有上界，UI 不会卡在一个不响应的主机上。
        let addrs: Vec<SocketAddr> = (host, port)
            .to_socket_addrs()
            .map_err(|err| TelnetError::Address {
                host: host.to_string(),
                port,
                detail: err.to_string(),
            })?
            .collect();
        if addrs.is_empty() {
            return Err(TelnetError::Address {
                host: host.to_string(),
                port,
                detail: "主机名没有解析出任何地址".to_string(),
            });
        }
        let timeout = Duration::from_secs(u64::from(
            config
                .connection_timeout
                .clamp(MIN_CONNECT_SECS, MAX_CONNECT_SECS),
        ));
        let mut connected: Option<TcpStream> = None;
        let mut last_detail = "没有可连接的地址".to_string();
        for addr in &addrs {
            match TcpStream::connect_timeout(addr, timeout) {
                Ok(stream) => {
                    connected = Some(stream);
                    break;
                }
                Err(err) => last_detail = err.to_string(),
            }
        }
        let stream = connected.ok_or_else(|| TelnetError::Connect {
            host: host.to_string(),
            port,
            detail: last_detail,
        })?;

        stream
            .set_read_timeout(Some(READ_TIMEOUT))
            .map_err(|err| TelnetError::Setup {
                detail: err.to_string(),
            })?;
        // 交互式终端默认开 TCP_NODELAY；这个开关本来就在会话配置里。
        let _ = stream.set_nodelay(config.tcp_low_delay);
        let writer = stream.try_clone().map_err(|err| TelnetError::Setup {
            detail: err.to_string(),
        })?;
        // 只作用于写半边；读超时在 stream（读线程持有）上。
        let _ = writer.set_write_timeout(Some(WRITE_TIMEOUT));

        // 初始尺寸只记住、不发送；对端 DO NAWS 时语义层会在 reply 里补发。
        let mut negotiator = TelnetNegotiator::new();
        let _ = negotiator.set_window_size(clamp_u16(cols), clamp_u16(rows));

        let (events_tx, events_rx) = mpsc::channel();
        let negotiator = Arc::new(Mutex::new(negotiator));
        let writer = Arc::new(Mutex::new(writer));
        let rx_bytes = Arc::new(AtomicU64::new(0));
        let tx_bytes = Arc::new(AtomicU64::new(0));
        let stop = Arc::new(AtomicBool::new(false));

        let thread_negotiator = Arc::clone(&negotiator);
        let thread_writer = Arc::clone(&writer);
        let thread_rx = Arc::clone(&rx_bytes);
        let thread_tx = Arc::clone(&tx_bytes);
        let thread_stop = Arc::clone(&stop);
        let thread_description = description.clone();
        let reader = thread::Builder::new()
            .name(format!("telnet-reader {host}:{port}"))
            .spawn(move || {
                reader_loop(
                    stream,
                    thread_writer,
                    thread_negotiator,
                    events_tx,
                    thread_rx,
                    thread_tx,
                    thread_stop,
                    thread_description,
                );
            })
            .map_err(|err| TelnetError::Setup {
                detail: err.to_string(),
            })?;

        Ok(Self {
            events: events_rx,
            pending: Vec::new(),
            notice: None,
            over: false,
            closed: false,
            negotiator,
            writer,
            rx_bytes,
            tx_bytes,
            stop,
            reader: Some(reader),
            description,
        })
    }

    /// 把读线程已经派发的事件收进本对象的缓冲。
    ///
    /// 这是 `drain()` 与 `poll()` 唯一的「等待」来源——`try_recv` 立刻返回，
    /// 绝不碰 socket，因此不会把 UI 帧钉住。
    fn collect(&mut self) {
        while let Ok(event) = self.events.try_recv() {
            match event {
                ReaderEvent::Data(bytes) => self.pending.extend_from_slice(&bytes),
                ReaderEvent::End(why) => {
                    self.over = true;
                    if !self.closed && self.notice.is_none() {
                        self.notice = Some(why);
                    }
                }
            }
        }
    }

    /// 写方向出错时收摊，并留下一次给用户看的原因。
    fn end_with(&mut self, why: String) {
        self.over = true;
        if !self.closed && self.notice.is_none() {
            self.notice = Some(why);
        }
    }

    /// 叫停读线程并 join。`close()` 与 `Drop` 共用，保证不留下后台线程。
    fn shutdown_reader(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        // shutdown 会同时唤醒另一 clone 上阻塞的 read()。
        let stream = lock(&self.writer);
        let _ = stream.shutdown(Shutdown::Both);
        drop(stream);
        if let Some(handle) = self.reader.take() {
            let _ = handle.join();
        }
    }
}

impl TermChannel for TelnetChannel {
    fn drain(&mut self) -> Vec<u8> {
        if self.closed {
            self.pending.clear();
            return Vec::new();
        }
        self.collect();
        // 对端断开（over）时仍要把已到手的最后一帧数据交出去，不能丢屏。
        std::mem::take(&mut self.pending)
    }

    fn write(&mut self, bytes: &[u8]) {
        if self.over || self.closed || bytes.is_empty() {
            return;
        }
        // RFC 854 §3：数据里的 0xFF 必须写成 IAC IAC，否则对端会把键入的
        // 字节误当命令。reply / NAWS 已经是编码好的命令，不走这里。
        let escaped = escape_iac(bytes);
        if let Err(err) = write_wire(&self.writer, &self.tx_bytes, &escaped) {
            self.end_with(format!("telnet write failed: {}: {err}", self.description));
        }
    }

    fn resize(&mut self, cols: usize, rows: usize) {
        if self.over || self.closed {
            return;
        }
        // 与读线程共用状态机：set_window_size 记住尺寸并返回 NAWS 子协商，
        // local_enabled 决定此刻该不该真的发出去（见模块文档）。
        let naws: Option<Vec<u8>> = {
            let mut negotiator = lock(&self.negotiator);
            let encoded = negotiator.set_window_size(clamp_u16(cols), clamp_u16(rows));
            if negotiator.local_enabled(NAWS) {
                Some(encoded)
            } else {
                None
            }
        };
        if let Some(bytes) = naws {
            // 已由语义层做过 0xFF 双写，这里绝不能再转义一次。
            if let Err(err) = write_wire(&self.writer, &self.tx_bytes, &bytes) {
                self.end_with(format!("telnet write failed: {}: {err}", self.description));
            }
        }
    }

    fn poll(&mut self) -> Option<String> {
        self.collect();
        if self.closed {
            return None;
        }
        self.notice.take()
    }

    fn finished(&self) -> bool {
        self.over || self.closed
    }

    fn close(&mut self) {
        self.closed = true;
        self.over = true;
        self.notice = None;
        self.pending.clear();
        self.shutdown_reader();
    }

    fn description(&self) -> &str {
        &self.description
    }

    fn counters(&self) -> Option<(u64, u64)> {
        Some((
            self.rx_bytes.load(Ordering::SeqCst),
            self.tx_bytes.load(Ordering::SeqCst),
        ))
    }

    fn status_note(&self) -> Option<String> {
        let rx = self.rx_bytes.load(Ordering::SeqCst);
        let tx = self.tx_bytes.load(Ordering::SeqCst);
        Some(format!("rx {rx} B / tx {tx} B"))
    }
}

impl Drop for TelnetChannel {
    fn drop(&mut self) {
        // 没走 close() 也要收掉读线程：测试夹具、Supervisor 提前退出都靠它。
        self.closed = true;
        self.over = true;
        self.shutdown_reader();
    }
}

/// `Mutex` 被污染时也要继续：读线程不 panic，污染只可能是别处的意外，
/// 直接吞掉会让会话永远卡住。
fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// 把所有出方向字节串行地写到 socket，成功才累加 TX 计数。
///
/// 返回 `io::Error` 而不是 `String`：本 crate 的对外边界错误必须是分类枚举，
/// 内部辅助函数同样不把错误拍平成字符串（4i 判据）。
fn write_wire(
    writer: &Mutex<TcpStream>,
    tx_bytes: &AtomicU64,
    bytes: &[u8],
) -> Result<(), std::io::Error> {
    if bytes.is_empty() {
        return Ok(());
    }
    let mut stream = lock(writer);
    stream.write_all(bytes)?;
    tx_bytes.fetch_add(bytes.len() as u64, Ordering::SeqCst);
    Ok(())
}

/// 数据方向转义：每个 `0xFF` 写成 `IAC IAC`（RFC 854 §3）。
fn escape_iac(bytes: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(bytes.len());
    for &byte in bytes {
        out.push(byte);
        if byte == IAC {
            out.push(IAC);
        }
    }
    out
}

/// 终端的 `usize` 行列 → NAWS 的 `u16`，超界夹到上限而不是回绕。
fn clamp_u16(value: usize) -> u16 {
    value.min(u16::MAX as usize) as u16
}

/// 读超时 / 信号中断不是故障，继续下一轮并检查停止标志。
fn is_transient(kind: ErrorKind) -> bool {
    matches!(
        kind,
        ErrorKind::WouldBlock | ErrorKind::TimedOut | ErrorKind::Interrupted
    )
}

/// 读线程主体：收字节 → 语义层分流 → `data` 入队、`reply` 立即写回。
#[allow(clippy::too_many_arguments)]
fn reader_loop(
    mut stream: TcpStream,
    writer: Arc<Mutex<TcpStream>>,
    negotiator: Arc<Mutex<TelnetNegotiator>>,
    events: mpsc::Sender<ReaderEvent>,
    rx_bytes: Arc<AtomicU64>,
    tx_bytes: Arc<AtomicU64>,
    stop: Arc<AtomicBool>,
    description: String,
) {
    let mut buf = [0u8; READ_CHUNK];
    let mut ending: Option<String> = None;
    loop {
        if stop.load(Ordering::SeqCst) {
            break;
        }
        match stream.read(&mut buf) {
            Ok(0) => {
                ending = Some(format!("telnet disconnected: {description}"));
                break;
            }
            Ok(n) => {
                rx_bytes.fetch_add(n as u64, Ordering::SeqCst);
                // 整块喂给语义层：它自己跨 feed 保留 IAC / SB 前缀状态，
                // 所以 TCP 把命令拆到两个包里也不会漏解析。
                let mut output = {
                    let mut negotiator = lock(&negotiator);
                    negotiator.feed(&buf[..n])
                };
                if !output.data.is_empty() {
                    let data = std::mem::take(&mut output.data);
                    if events.send(ReaderEvent::Data(data)).is_err() {
                        break; // 通道已被丢弃
                    }
                }
                if !output.reply.is_empty() {
                    let reply = std::mem::take(&mut output.reply);
                    if let Err(err) = write_wire(&writer, &tx_bytes, &reply) {
                        ending = Some(format!("telnet write failed: {description}: {err}"));
                        break;
                    }
                }
            }
            Err(err) if is_transient(err.kind()) => {
                // read_timeout 已经让 WouldBlock/TimedOut 等了 50ms；Interrupted
                // 可能立刻返回，补一个极短的让出避免空转烧 CPU。
                thread::sleep(Duration::from_millis(1));
            }
            Err(err) => {
                ending = Some(format!("telnet read failed: {description}: {err}"));
                break;
            }
        }
    }
    // close() 主动收摊时不报「断开」——那不是用户需要处理的事件。
    if stop.load(Ordering::SeqCst) {
        return;
    }
    let why = ending.unwrap_or_else(|| format!("telnet closed: {description}"));
    let _ = events.send(ReaderEvent::End(why));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn escape_iac_doubles_only_0xff() {
        assert_eq!(escape_iac(b"ls\r\n"), b"ls\r\n".to_vec());
        assert_eq!(
            escape_iac(&[b'a', 0xFF, b'b']),
            vec![b'a', 0xFF, 0xFF, b'b']
        );
        assert_eq!(escape_iac(&[0xFF, 0xFF]), vec![0xFF, 0xFF, 0xFF, 0xFF]);
    }

    #[test]
    fn clamp_u16_saturates_at_u16_max() {
        assert_eq!(clamp_u16(0), 0);
        assert_eq!(clamp_u16(80), 80);
        assert_eq!(clamp_u16(u16::MAX as usize), u16::MAX);
        assert_eq!(clamp_u16(usize::MAX), u16::MAX);
    }

    #[test]
    fn telnet_error_display_is_actionable() {
        let err = TelnetError::Connect {
            host: "127.0.0.1".to_string(),
            port: 23,
            detail: "Connection refused".to_string(),
        };
        let text = err.to_string();
        assert!(text.contains("127.0.0.1:23"), "{text}");
        assert!(text.contains("Connection refused"), "{text}");
        assert!(TelnetError::MissingHost.to_string().contains("主机名"));
    }
}

//! 原生 Telnet 通道的进程内集成测试。
//!
//! 夹具只在 `127.0.0.1:0` 上 bind 一个 `TcpListener`（回环、不对外提供服务），
//! 客户端与服务端都在本测试进程里：不需要真 telnet 服务端，也不需要任何新依赖。
//! 服务端脚本通过 `mpsc` 控制，测试可以精确决定「何时发协商、何时断开」。
//!
//! # 为什么改成确定性同步（2026-11-26）
//!
//! 旧夹具的 `MockServer::new()` 只 `thread::spawn` 就返回：accept 线程什么时候
//! 跑到 `accept()` 完全交给 OS 调度。而它只 accept **一次**，随后独占唯一的
//! `script_rx` 进入读循环。于是有两处真实竞态：
//!
//! ① 全量验收的并发负载下，accept 线程可能被推迟很久。客户端的 connect 会
//!    进内核 backlog、握手成功，但数据要等线程被调度才流动；BUDGET(2s) 一到
//!    就报成「连接超时」。把 BUDGET 放大到 10s、connect 重试 3 次只是把症状
//!    推后，并没有消除竞态。
//!
//! ② 任何**第二条**到同一端口的连接都会把那一次 accept 消耗掉。此后真正的
//!    客户端虽然握手成功，却永远卡在 backlog 里收不到字节；`script_tx` 的
//!    唯一接收端也随线程退出而消失，`send()` 直接 panic。独立复现实验见下方
//!    `single-shot accept` 的证据：第二条连接 read 200ms 得到 WouldBlock，
//!    而端口明明仍在监听。这正是偶发 `Connection refused` / `connection timed
//!    out` 的形状——不是网络问题，是夹具自己把服务端弄死了。
//!
//! 现在：
//! - accept 用**阻塞** socket，线程进入 `accept()` 前先发就绪信号，`new()`
//!   必须收到信号才返回（有上界，失败立刻 panic，绝不挂死）；
//! - 监听器始终归夹具所有，线程只持 `try_clone()` 句柄，生命周期彻底解耦；
//! - `Drop` 用一次「自己连自己」确定性唤醒阻塞的 accept，线程看到 stop 立刻
//!   收摊，不依赖 `WouldBlock` 轮询或 2ms sleep；
//! - 错误分类用例不再 `bind → drop → connect`（那个空窗会被并行测试抢走端口，
//!   当时的「重试 5 次」正是掩盖式补丁），改为「IPv4 回环绑住不放 + 连同一
//!   端口的 `::1`」：内核确定返回 ECONNREFUSED，一个重试都不需要。

use std::io::{ErrorKind, Read, Write};
use std::net::{Shutdown, SocketAddr, TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use term_core::channel::TermChannel;
use term_core::session::{Protocol, SessionConfig, SpawnPlan};
use term_core::telnet::{DO, DONT, ECHO, IAC, NAWS, SB, SE, SGA, WILL, WONT};
use term_io::channel::open_channel;
use term_io::error::SpawnError;
use term_io::telnet_channel::TelnetChannel;

/// 所有等待的**硬上界**：拿不到就 fail-fast，测试绝不挂死。
///
/// 这是诊断用的断路阈值，不是「多等一会儿」的宽容度：就绪握手、数据到达、
/// 断连上报全都必须在 2 秒内发生，否则就是夹具或实现坏了。
const BUDGET: Duration = Duration::from_secs(2);

/// 服务端脚本动作。
enum ServerAction {
    /// 往客户端写一段原始字节（协商 / 文本皆可）。
    Send(Vec<u8>),
    /// 关掉写半边，模拟对端主动断开。
    Close,
}

/// 单连接的 mock telnet 服务端。
struct MockServer {
    addr: SocketAddr,
    /// 监听器**归夹具所有**。曾经把它直接 move 进 accept 线程：线程一旦
    /// 提前退出，socket 随之 drop，后续连接拿到的就是 `Connection refused`。
    /// 现在线程只持有 `try_clone()` 的句柄，两件事彻底解耦。
    _listener: TcpListener,
    /// 从客户端收到的全部原始字节（按到达顺序）。
    received: Arc<Mutex<Vec<u8>>>,
    script: mpsc::Sender<ServerAction>,
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl MockServer {
    /// 绑定回环、起 accept 线程，并**等线程报到**才返回。
    ///
    /// `new()` 返回时保证 accept 线程已经在阻塞 `accept()`：客户端随后
    /// connect 只是把连接交给内核 backlog，不依赖用户态线程何时被调度。
    fn new() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0")
            .unwrap_or_else(|err| panic!("bind 回环 mock 服务端失败：{err}"));
        let addr = listener
            .local_addr()
            .unwrap_or_else(|err| panic!("取监听地址失败：{err}"));
        // 阻塞式 accept：连接一到，内核直接唤醒线程；不再有非阻塞轮询 +
        // sleep(2ms) 那套「赌调度」的时序依赖。
        let thread_listener = listener
            .try_clone()
            .unwrap_or_else(|err| panic!("克隆 mock 监听器失败：{err}"));

        let received = Arc::new(Mutex::new(Vec::new()));
        let stop = Arc::new(AtomicBool::new(false));
        let (script_tx, script_rx) = mpsc::channel();

        // 确定性就绪握手：线程在调用 accept() **之前**报到，`new()` 必须等到
        // 这个信号才返回。上界 BUDGET 保证线程起不来时当场失败，不挂死。
        let (ready_tx, ready_rx) = mpsc::channel();
        let thread_received = Arc::clone(&received);
        let thread_stop = Arc::clone(&stop);
        let thread = thread::spawn(move || {
            let _ = ready_tx.send(());
            let mut conn = match thread_listener.accept() {
                Ok((conn, _)) => conn,
                // 监听器关闭 / 唤醒用的自连接：收摊即可，不再重试。
                Err(_) => return,
            };
            // Drop 的自连接唤醒不能冒充客户端连接。
            if thread_stop.load(Ordering::SeqCst) {
                return;
            }
            let _ = conn.set_read_timeout(Some(Duration::from_millis(20)));
            let mut buf = [0u8; 4096];
            loop {
                if thread_stop.load(Ordering::SeqCst) {
                    break;
                }
                while let Ok(action) = script_rx.try_recv() {
                    match action {
                        ServerAction::Send(bytes) => {
                            if conn.write_all(&bytes).is_err() {
                                return;
                            }
                        }
                        ServerAction::Close => {
                            let _ = conn.shutdown(Shutdown::Write);
                        }
                    }
                }
                match conn.read(&mut buf) {
                    Ok(0) => break,
                    Ok(n) => {
                        let mut guard = thread_received
                            .lock()
                            .unwrap_or_else(|poisoned| poisoned.into_inner());
                        guard.extend_from_slice(&buf[..n]);
                    }
                    Err(err)
                        if matches!(
                            err.kind(),
                            ErrorKind::WouldBlock | ErrorKind::TimedOut | ErrorKind::Interrupted
                        ) =>
                    {
                        // 读超时是让 stop 标志得以被检查的节奏，不是重试补丁。
                        thread::sleep(Duration::from_millis(2));
                    }
                    Err(_) => break,
                }
            }
        });
        ready_rx
            .recv_timeout(BUDGET)
            .unwrap_or_else(|err| panic!("mock 服务端 accept 线程未就绪：{err}"));

        Self {
            addr,
            _listener: listener,
            received,
            script: script_tx,
            stop,
            thread: Some(thread),
        }
    }

    fn addr(&self) -> SocketAddr {
        self.addr
    }

    fn send(&self, bytes: &[u8]) {
        self.script
            .send(ServerAction::Send(bytes.to_vec()))
            .unwrap_or_else(|_| panic!("服务端脚本已退出"));
    }

    fn close_connection(&self) {
        self.script
            .send(ServerAction::Close)
            .unwrap_or_else(|_| panic!("服务端脚本已退出"));
    }

    fn received(&self) -> Vec<u8> {
        self.received
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
    }

    /// 等到 `needle` 出现在已收字节里，返回当时的完整快照。
    fn wait_for(&self, needle: &[u8]) -> Vec<u8> {
        let deadline = Instant::now() + BUDGET;
        loop {
            let got = self.received();
            if contains(&got, needle) {
                return got;
            }
            assert!(
                Instant::now() < deadline,
                "服务端没等到 {needle:?}，目前收到 {got:?}"
            );
            thread::sleep(Duration::from_millis(2));
        }
    }
}

impl Drop for MockServer {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        // 确定性唤醒阻塞在 accept() 上的线程：一次自连接让 accept 立刻返回，
        // 线程看到 stop 就退出。不需要等超时，也不靠「等一会儿」。
        let _ = TcpStream::connect(self.addr);
        if let Some(handle) = self.thread.take() {
            let _ = handle.join();
        }
    }
}

/// 在 `haystack` 里找连续子序列（TCP 可能把多次写粘成一个包）。
fn contains(haystack: &[u8], needle: &[u8]) -> bool {
    if needle.is_empty() {
        return true;
    }
    if needle.len() > haystack.len() {
        return false;
    }
    haystack
        .windows(needle.len())
        .any(|window| window == needle)
}

/// 用会话配置连本地 mock 服务端；`cols/rows` 是 `open_channel` 传进来的初始尺寸。
fn open_telnet(addr: &SocketAddr, cols: usize, rows: usize) -> TelnetChannel {
    let mut config = SessionConfig::new(Protocol::Telnet);
    config.host = addr.ip().to_string();
    config.port = addr.port();
    // `new()` 已保证 accept 线程就位；这里只连一次，没有重试、没有加时。
    TelnetChannel::open(&config, cols, rows)
        .unwrap_or_else(|err| panic!("连本地 mock 服务端失败：{err}"))
}

/// 反复 `drain()` 累积数据，直到 `needle` 出现（读线程是异步的，一帧拿不全）。
fn wait_for_data(ch: &mut TelnetChannel, needle: &[u8]) -> Vec<u8> {
    let deadline = Instant::now() + BUDGET;
    let mut got = Vec::new();
    loop {
        got.extend(ch.drain());
        if contains(&got, needle) {
            return got;
        }
        assert!(
            Instant::now() < deadline,
            "drain 没等到 {needle:?}，目前拿到 {got:?}"
        );
        thread::sleep(Duration::from_millis(2));
    }
}

// ---------------------------------------------------------------------------
// ① 协商应答符合语义层策略；② drain() 只有普通文本
// ---------------------------------------------------------------------------

#[test]
fn negotiation_replies_follow_policy_and_only_text_reaches_drain() {
    let server = MockServer::new();
    let mut ch = open_telnet(&server.addr(), 80, 24);

    let mut greeting = Vec::new();
    greeting.extend_from_slice(&[IAC, DO, SGA]); // → IAC WILL SGA（接受）
    greeting.extend_from_slice(&[IAC, WILL, ECHO]); // → IAC DO ECHO（接受）
    greeting.extend_from_slice(&[IAC, DO, ECHO]); // → IAC WONT ECHO（拒绝本地回显）
    greeting.extend_from_slice(&[IAC, WILL, 99]); // → IAC DONT 99（未知选项拒绝）
    server.send(&greeting);
    server.send(b"hello\r\n");

    let text = wait_for_data(&mut ch, b"hello\r\n");
    assert_eq!(text, b"hello\r\n".to_vec(), "协商字节绝不能泄进终端数据流");

    let expected = vec![
        IAC, WILL, SGA, IAC, DO, ECHO, IAC, WONT, ECHO, IAC, DONT, 99,
    ];
    let reply = server.wait_for(&expected);
    assert_eq!(reply, expected, "WILL/WONT/DO/DONT 必须与语义层策略一致");
}

#[test]
fn subnegotiation_bytes_never_reach_the_terminal() {
    let server = MockServer::new();
    let mut ch = open_telnet(&server.addr(), 80, 24);

    let mut wire = b"x".to_vec();
    wire.extend_from_slice(&[IAC, SB, NAWS, 0, 80, 0, 24, IAC, SE]);
    wire.extend_from_slice(b"y");
    server.send(&wire);

    assert_eq!(wait_for_data(&mut ch, b"xy"), b"xy".to_vec());
    assert!(server.received().is_empty(), "对端 SB…SE 不该触发任何应答");
}

// ---------------------------------------------------------------------------
// ③ resize() 发出合法 NAWS，且每个 0xFF 都被双写转义
// ---------------------------------------------------------------------------

#[test]
fn resize_sends_naws_with_every_0xff_doubled() {
    let server = MockServer::new();
    let mut ch = open_telnet(&server.addr(), 80, 24);

    // 先让对端同意 NAWS；语义层会顺带补发 open() 记住的初始尺寸（80×24）。
    server.send(&[IAC, DO, NAWS]);
    server.wait_for(&[IAC, WILL, NAWS]);

    // cols=0x00FF、rows=0xFFFF：五个 0xFF 字节，每一个都要写成 IAC IAC。
    ch.resize(0x00FF, 0xFFFF);
    let expected = vec![
        IAC, SB, NAWS, 0x00, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, IAC, SE,
    ];
    let got = server.wait_for(&expected);
    assert!(
        contains(&got, &expected),
        "服务端应收到 IAC SB NAWS <cols:2> <rows:2> IAC SE（0xFF 双写）：{got:?}"
    );
}

#[test]
fn initial_window_size_is_reported_when_peer_asks_for_naws() {
    let server = MockServer::new();
    let mut ch = open_telnet(&server.addr(), 120, 40);

    server.send(&[IAC, DO, NAWS]);
    let expected = vec![IAC, WILL, NAWS, IAC, SB, NAWS, 0, 120, 0, 40, IAC, SE];
    let got = server.wait_for(&expected);
    assert!(
        contains(&got, &expected),
        "对端 DO NAWS 时应在 reply 里补发初始尺寸：{got:?}"
    );
    // 这条路径不产生任何终端数据。
    assert!(ch.drain().is_empty());
}

// ---------------------------------------------------------------------------
// ④ IAC IAC 还原成单个 0xFF 数据字节
// ---------------------------------------------------------------------------

#[test]
fn iac_iac_is_restored_to_a_single_0xff_data_byte() {
    let server = MockServer::new();
    let mut ch = open_telnet(&server.addr(), 80, 24);

    server.send(b"a");
    server.send(&[IAC, IAC]);
    server.send(b"b");

    let got = wait_for_data(&mut ch, &[b'a', 0xFF, b'b']);
    assert_eq!(got, vec![b'a', 0xFF, b'b']);
    assert!(server.received().is_empty(), "转义不是协商，不该有应答");
}

// ---------------------------------------------------------------------------
// 写方向、生命周期、描述与计数
// ---------------------------------------------------------------------------

#[test]
fn write_sends_input_and_escapes_iac_bytes() {
    let server = MockServer::new();
    let mut ch = open_telnet(&server.addr(), 80, 24);

    ch.write(b"ls\r\n");
    assert_eq!(server.wait_for(b"ls\r\n"), b"ls\r\n".to_vec());

    // 终端输入里的 0xFF 也要按 RFC 854 §3 双写，别让对端误当命令。
    ch.write(&[b'a', 0xFF, b'b']);
    let got = server.wait_for(&[b'a', 0xFF, 0xFF, b'b']);
    assert!(contains(&got, &[b'a', 0xFF, 0xFF, b'b']), "{got:?}");
}

#[test]
fn peer_close_is_reported_once_and_finishes_the_channel() {
    let server = MockServer::new();
    let mut ch = open_telnet(&server.addr(), 80, 24);

    server.send(b"bye");
    wait_for_data(&mut ch, b"bye");
    server.close_connection();

    let deadline = Instant::now() + BUDGET;
    let mut note: Option<String> = None;
    while note.is_none() {
        let _ = ch.drain();
        note = ch.poll();
        if note.is_none() {
            assert!(Instant::now() < deadline, "对端关闭后 poll 没有上报");
            thread::sleep(Duration::from_millis(2));
        }
    }
    let text = note.unwrap_or_default();
    assert!(text.contains("telnet"), "{text}");
    assert!(ch.finished());
    assert_eq!(ch.poll(), None, "断开只报一次");
}

#[test]
fn explicit_close_does_not_emit_a_second_notice() {
    let server = MockServer::new();
    let mut ch = open_telnet(&server.addr(), 80, 24);
    ch.close();
    assert!(ch.finished());
    assert_eq!(ch.poll(), None);
    assert!(ch.drain().is_empty());
    assert!(ch.counters().is_some(), "关掉之后计数仍可读");
}

#[test]
fn description_counters_and_status_note_reflect_the_wire() {
    let server = MockServer::new();
    let addr = server.addr();
    let mut ch = open_telnet(&addr, 80, 24);
    assert_eq!(
        ch.description(),
        format!("telnet 127.0.0.1:{}", addr.port())
    );

    server.send(b"ping");
    assert_eq!(wait_for_data(&mut ch, b"ping"), b"ping".to_vec());
    ch.write(b"pong");
    server.wait_for(b"pong");

    let (rx, tx) = ch
        .counters()
        .unwrap_or_else(|| panic!("Telnet 通道应统计 RX/TX"));
    assert_eq!(rx, 4, "RX 是线上收到的原始字节数");
    assert_eq!(tx, 4, "TX 是线上发出的原始字节数");
    let note = ch.status_note().unwrap_or_default();
    assert!(note.contains("4 B"), "{note}");
}

// ---------------------------------------------------------------------------
// 兼容路径与失败分类（不回退系统 telnet）
// ---------------------------------------------------------------------------

#[test]
fn spawn_plan_still_builds_the_system_telnet_argv() {
    // 只改 open_channel() 的分派；spawn_plan() 的系统客户端分支原样保留。
    let mut config = SessionConfig::new(Protocol::Telnet);
    config.host = "example.net".to_string();
    match config
        .spawn_plan()
        .unwrap_or_else(|err| panic!("Telnet 计划仍可构造：{err}"))
    {
        SpawnPlan::External(argv) => assert_eq!(
            argv,
            vec![
                "telnet".to_string(),
                "example.net".to_string(),
                "23".to_string(),
            ]
        ),
        SpawnPlan::LocalShell => panic!("Telnet 不该变成 LocalShell"),
        SpawnPlan::ShellProgram(_) => panic!("Telnet 不该变成 ShellProgram"),
    }
}

#[test]
fn telnet_spawn_error_has_its_own_readable_class() {
    let err = SpawnError::Telnet("127.0.0.1:23 连接失败：Connection refused".to_string());
    let text = err.to_string();
    assert!(text.contains("无法连接 Telnet"), "{text}");
    assert!(text.contains("Connection refused"), "{text}");
}

#[test]
fn open_channel_reports_telnet_errors_without_falling_back() {
    // 确定性制造「连接被拒」：在 IPv4 回环上绑一个监听器并**全程持有**，然后
    // 让客户端连同一端口的 IPv6 回环地址 `::1`。该端口只在 IPv4 上监听，
    // ::1 上没有 socket，内核确定返回 ECONNREFUSED（本机实测 os error 61）。
    //
    // 旧写法是 `bind(0) → 读端口 → drop → connect`：端口一旦放掉就出现空窗，
    // 并行测试（或同进程另一个用例）的 `bind(0)` 可能抢走它，于是 connect
    // 反而成功，测试靠 `for _ in 0..5` 重试去赌——那正是要删掉的掩盖式补丁。
    // 现在端口从头到尾都被本用例占着，空窗消失，一次连接、一次判定。
    let _listener =
        TcpListener::bind("127.0.0.1:0").unwrap_or_else(|err| panic!("占一个临时端口失败：{err}"));
    let port = _listener
        .local_addr()
        .unwrap_or_else(|err| panic!("读取端口失败：{err}"))
        .port();

    let mut config = SessionConfig::new(Protocol::Telnet);
    config.host = "::1".to_string();
    config.port = port;
    match open_channel(config, 80, 24) {
        Err(err) => {
            assert!(matches!(err, SpawnError::Telnet(_)), "{err:?}");
            assert!(err.to_string().contains("Telnet"), "{err}");
        }
        Ok(_) => panic!("连 ::1 不该成功：该端口只在 IPv4 回环上监听"),
    }
}

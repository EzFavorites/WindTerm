//! 一个已经连上的 SSH 会话，对外表现为同步字节通道。
//!
//! ```text
//! [Supervisor 线程]                     [本 crate 的专用线程]
//!   drain()  ←── std::mpsc  ←──────────── ChannelMsg::Data ──┐
//!   write()  ── tokio unbounded ──▶ data_bytes()             │ tokio current_thread
//!   resize() ── 同上 ─────────────▶ window_change()          │ 运行在这个线程里
//!   poll()   ←── Ended(状态文本) ── 对端关闭 / 出错 ─────────┘
//! ```
//!
//! 两条队列的分工：
//! - **上行**（键盘 → 服务器）用 `tokio::mpsc::unbounded_channel`：它的 `send`
//!   是同步的，Supervisor 线程不用等；而且 unbounded 不会因为「服务器卡住」
//!   把 UI 线程堵死（体积上限由 SSH 窗口机制在协议层兜着）。
//! - **下行**（服务器 → 屏幕）用 `std::sync::mpsc`：`drain()` 是纯 `try_recv`，
//!   不阻塞、不 `.await`，与本地 PTY 那条路径完全同形。
//!
//! **结束的顺序**是这里最容易写错的地方：最后一屏字节必须比「已结束」先到。
//! 所以收尾也走同一条下行队列（`Emit::Ended`），而不是靠一个 `bool`——
//! 用 `bool` 的话，对端关连接那一下正好有数据在队列里，就会丢掉最后一行
//! （比如 `logout` 之后那句 `Connection closed.`）。

use std::net::SocketAddr;
use std::sync::mpsc::{Receiver, Sender, TryRecvError, channel};
use std::thread::JoinHandle;
use std::time::Duration;

use russh::ChannelMsg;
use russh::client::{self, Handle};
use term_core::channel::TermChannel;

use crate::error::SshError;
use crate::handler::HostKeyHandler;
use crate::options::SshOptions;

/// 每帧最多搬多少字节，免得一次 `find /` 把一帧卡住。
/// 与 `term-runtime` 的 `FRAME_BUDGET` 同值：两处都是「一帧的预算」，不该不一样。
const FRAME_BUDGET: usize = 256 * 1024;

/// Supervisor → 工作线程。
enum Command {
    Write(Vec<u8>),
    Resize(u32, u32),
    Close,
}

/// 工作线程 → Supervisor。
enum Emit {
    Data(Vec<u8>),
    /// 会话结束，附带状态栏要显示的那句文本。
    Ended(String),
}

/// 一条已建立的 SSH 会话。`Send`，可以直接交给 `Supervisor` 的线程。
pub struct SshChannel {
    out: Receiver<Emit>,
    cmd: tokio::sync::mpsc::UnboundedSender<Command>,
    status: Option<String>,
    ended: bool,
    description: String,
    /// 线程句柄只用来「别在收尾时丢掉它」；连接靠 drop 关闭（见 `Drop`）。
    _worker: Option<JoinHandle<()>>,
}

impl std::fmt::Debug for SshChannel {
    /// 手工实现：只印「这条会话是什么」，不印内部队列——
    /// 队列里是屏幕字节，`{:?}` 出来是一堆噪声，而且 #[] 会让日志没法看。
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SshChannel")
            .field("description", &self.description)
            .field("ended", &self.ended)
            .finish()
    }
}

/// 连上去并开一个挂着 PTY 的 shell 通道。**同步**，阻塞到握手完成为止。
///
/// 为什么阻塞：调用方是 UI 或 CLI，它们需要立刻知道「连上了没有」。异步接口
/// 会把这一步变成「先返回一句『连接中』」，然后所有错误都得排队再来一次。
/// 耗时的部分在内部线程里跑，`connect` 只是在等一个 `Result`。
pub fn connect(opts: SshOptions) -> Result<SshChannel, SshError> {
    opts.validate()?;
    if opts.proxy_jump.is_empty() && opts.outbound_proxy.is_none() {
        // 先解析一次：DNS 失败与「连上了但被拒」是两类不同的错，用户要做的事也不同。
        //
        // **有跳或出网代理时不能这么做**：这两种用法的典型场景恰恰是「目标名字
        // 只在内网 / 代理那一侧能解析」，那时本地 DNS 必然失败；目标主机名该由
        // 最后一跳或代理去解析（W1-b / 官文「DNS 可走代理」）。
        resolve(&opts)?;
    }
    let description = opts.description();
    let (cmd_tx, cmd_rx) = tokio::sync::mpsc::unbounded_channel();
    let (emit_tx, emit_rx) = channel();
    let (ready_tx, ready_rx) = channel::<Result<(), SshError>>();

    let name = format!("term-ssh-{}", opts.host);
    let worker = std::thread::Builder::new()
        .name(name)
        .spawn(move || run_worker(opts, cmd_rx, emit_tx, ready_tx))
        .map_err(|e| SshError::Local(format!("无法启动 SSH 线程：{e}")))?;

    match ready_rx.recv() {
        Ok(Ok(())) => Ok(SshChannel {
            out: emit_rx,
            cmd: cmd_tx,
            status: None,
            ended: false,
            description,
            _worker: Some(worker),
        }),
        Ok(Err(e)) => {
            let _ = worker.join();
            Err(e)
        }
        // 线程连 `ready` 都没发就没了：通常是 panic 在建运行时之前。
        Err(_) => {
            let _ = worker.join();
            Err(SshError::Local(
                "SSH 线程在握手完成前退出（可能是平台不支持或内存不足）".to_string(),
            ))
        }
    }
}

/// 主机名 + 端口 → 一个具体的地址。
fn resolve(opts: &SshOptions) -> Result<SocketAddr, SshError> {
    crate::proxy::resolve_host(&opts.host, opts.port)
}

fn run_worker(
    opts: SshOptions,
    cmd_rx: tokio::sync::mpsc::UnboundedReceiver<Command>,
    emit: Sender<Emit>,
    ready: Sender<Result<(), SshError>>,
) {
    let runtime = match tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    {
        Ok(rt) => rt,
        Err(e) => {
            let _ = ready.send(Err(SshError::Local(format!("无法创建异步运行时：{e}"))));
            return;
        }
    };
    runtime.block_on(async move { session(opts, cmd_rx, emit, ready).await });
}

async fn session(
    opts: SshOptions,
    mut cmd_rx: tokio::sync::mpsc::UnboundedReceiver<Command>,
    emit: Sender<Emit>,
    ready: Sender<Result<(), SshError>>,
) {
    let (handle, mut channel) = match handshake(&opts).await {
        Ok(pair) => pair,
        Err(e) => {
            let _ = ready.send(Err(e));
            return;
        }
    };
    // 到这里「连上了」才是事实：之前所有失败都得回给 `connect()`。
    let _ = ready.send(Ok(()));

    let text = pump(&mut channel, &mut cmd_rx, &emit).await;
    let _ = emit.send(Emit::Ended(text));
    // 主动挂断：不发这一下，服务端会留一个半死的会话直到超时。
    let _ = handle
        .disconnect(russh::Disconnect::ByApplication, "", "en")
        .await;
}

/// 连上、认证，然后开一个挂着 PTY 的 shell 通道。
///
/// 代理链（`ProxyJump`）在 [`crate::proxy::connect`] 里消化：这里拿到的永远是
/// 最终目标那条连接的句柄，跳板机对它不可见。
async fn handshake(
    opts: &SshOptions,
) -> Result<(Handle<HostKeyHandler>, russh::Channel<client::Msg>), SshError> {
    let handle = crate::proxy::connect(opts, None).await?;
    let channel = handle.channel_open_session().await.map_err(|e| {
        SshError::Channel(format!("服务器不允许开会话通道（可能已达上限）：{e}"))
    })?;
    // PTY 必须在 shell 之前要：顺序反了服务器会认为这是个非交互会话，
    // 于是没有回显、没有行编辑，`vim` 直接报 "not a terminal"。
    channel
        .request_pty(true, &opts.term, opts.cols, opts.rows, 0, 0, &[])
        .await
        .map_err(|e| SshError::Channel(format!("申请伪终端失败：{e}")))?;
    channel
        .request_shell(true)
        .await
        .map_err(|e| SshError::Channel(format!("申请 shell 失败：{e}")))?;
    // X11 转发（等价 `ssh -X`）：会话通道一开就下发 `x11-req` 全局请求，
    // 告诉服务端「把远端连到 `$DISPLAY` 的 X 连接回开成 `x11` 通道」。
    // 这是尽力而为：服务端若禁了 X11 转发（`x11-req` 被拒），shell 照样工作，
    // 只记一条 warn，不把整条连接搞挂。回开的通道由 `HostKeyHandler::
    // server_channel_open_x11` 接住并桥到本机 X server。
    if opts.x11_forward {
        let payload = x11_request_payload(opts.x11_screen);
        if let Err(e) = handle.send_global_request("x11-req", &payload, true).await {
            log::warn!("x11-req 下发失败（服务端可能禁了 X11 转发）：{e}");
        }
    }
    Ok((handle, channel))
}

/// 拼 `x11-req` 全局请求的载荷（RFC 4254 §6.2 之后的那段）。
///
/// 顺序：`boolean single_connection` + `string auth_protocol`
/// + `string auth_cookie` + `uint32 screen_number`。
fn x11_request_payload(screen: u32) -> Vec<u8> {
    let mut buf = Vec::with_capacity(40);
    buf.push(0u8); // single_connection = false：一条 SSH 连接上可转发多个 X 连接
    put_ssh_string(&mut buf, b"MIT-MAGIC-COOKIE-1");
    let cookie = x11_cookie();
    put_ssh_string(&mut buf, cookie.as_bytes());
    buf.extend_from_slice(&screen.to_be_bytes());
    buf
}

/// SSH 线格式字符串：`uint32` 长度前缀 + 字节。
fn put_ssh_string(buf: &mut Vec<u8>, s: &[u8]) {
    buf.extend_from_slice(&(s.len() as u32).to_be_bytes());
    buf.extend_from_slice(s);
}

/// 16 字符十六进制 X11 认证 cookie。
///
/// 它只随 `x11-req` 下发给服务端，供 sshd 在远端 xauth 登记转发显示用，
/// **不影响本机桥接**（本机那段只是把回开通道的字节原样搬到本地 X socket）。
/// 用「时间 + 进程内计数器」做一个会话内唯一的串，避免引入额外随机源依赖；
/// 真正的随机性由远端 sshd 在 xauth 侧负责，这里只需形态合法（16 hex）。
fn x11_cookie() -> String {
    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    // 取低 64 位与计数器混合（再乘一个奇数打乱），保证会话内唯一且形态为 16 hex。
    // cookie 只随 `x11-req` 下发给服务端登记远端 xauth，不影响本机桥接，故无需密码学随机。
    let mixed = (nanos as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15)
        ^ COUNTER.fetch_add(1, Ordering::Relaxed);
    format!("{mixed:016x}")
}

/// 双向搬运，直到对端关闭或本端要求关闭。返回状态栏文本。
async fn pump(
    channel: &mut russh::Channel<client::Msg>,
    cmd_rx: &mut tokio::sync::mpsc::UnboundedReceiver<Command>,
    emit: &Sender<Emit>,
) -> String {
    // 服务器报上来的退出码。有它就用它，没有（连接被砍）就是另一种措辞。
    let mut exit_status: Option<u32> = None;
    let mut signalled = false;

    loop {
        tokio::select! {
            msg = channel.wait() => match msg {
                Some(ChannelMsg::Data { data }) => {
                    if emit.send(Emit::Data(data.to_vec())).is_err() {
                        // Supervisor 已经不在了：没人看屏幕，直接收摊。
                        return finish(exit_status, signalled);
                    }
                }
                // ext=1 是 stderr。对终端来说 stdout/stderr 都上屏，
                // 分开处理只对「把 stderr 染红」有意义，那是 UI 的事。
                Some(ChannelMsg::ExtendedData { data, .. }) => {
                    if emit.send(Emit::Data(data.to_vec())).is_err() {
                        return finish(exit_status, signalled);
                    }
                }
                Some(ChannelMsg::ExitStatus { exit_status: code }) => exit_status = Some(code),
                Some(ChannelMsg::ExitSignal { .. }) => signalled = true,
                Some(ChannelMsg::Eof) | Some(ChannelMsg::Close) => break,
                // 通道没了（连接断掉）：与对端关闭同等对待，但措辞不同。
                None => return finish_lost(exit_status),
                _ => {}
            },
            cmd = cmd_rx.recv() => match cmd {
                Some(Command::Write(bytes)) => {
                    if channel.data_bytes(bytes).await.is_err() {
                        return finish_lost(exit_status);
                    }
                }
                Some(Command::Resize(cols, rows)) => {
                    // 改尺寸失败不该杀掉会话：窗口拉一下不成功，画面还是能看。
                    let _ = channel.window_change(cols, rows, 0, 0).await;
                }
                Some(Command::Close) | None => {
                    let _ = channel.eof().await;
                    let _ = channel.close().await;
                    break;
                }
            }
        }
    }
    finish(exit_status, signalled)
}

/// 正常收尾的措辞。与 `term-runtime::pty` 的 `" exited"` / `" exited with code N"`
/// 保持同一套写法：状态栏只有一条渲染路径，两条措辞不一致就是两种样子。
fn finish(exit_status: Option<u32>, signalled: bool) -> String {
    match (exit_status, signalled) {
        (Some(0), _) => " exited".to_string(),
        (Some(code), _) => format!(" exited with code {code}"),
        (None, true) => " stopped".to_string(),
        (None, false) => " connection closed".to_string(),
    }
}

/// 连接被非正常掐断时的措辞：这时**不能**说 " exited"，
/// 否则用户会以为远端命令跑完了（真实原因往往是网络）。
fn finish_lost(exit_status: Option<u32>) -> String {
    match exit_status {
        Some(code) => finish(Some(code), false),
        None => " connection lost".to_string(),
    }
}

impl TermChannel for SshChannel {
    fn drain(&mut self) -> Vec<u8> {
        let mut out = Vec::new();
        while out.len() < FRAME_BUDGET {
            match self.out.try_recv() {
                Ok(Emit::Data(bytes)) => out.extend_from_slice(&bytes),
                Ok(Emit::Ended(text)) => {
                    self.ended = true;
                    self.status = Some(text);
                    break;
                }
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => {
                    // 工作线程退出了。**只有**它没发过 `Ended` 才算「连接掉了」——
                    // 正常收尾之后线程也会退出并丢掉发送端，那时再补一句
                    // "connection lost" 就等于把「命令退出码 3」改写成「连接断了」。
                    if !self.ended {
                        self.ended = true;
                        self.status = Some(" connection lost".to_string());
                    }
                    break;
                }
            }
        }
        out
    }

    fn write(&mut self, bytes: &[u8]) {
        // 发送端只在工作线程退出后才失败，那时也没什么可写的了。
        let _ = self.cmd.send(Command::Write(bytes.to_vec()));
    }

    fn resize(&mut self, cols: usize, rows: usize) {
        let _ = self
            .cmd
            .send(Command::Resize(cols as u32, rows as u32));
    }

    fn poll(&mut self) -> Option<String> {
        // 只给一次：重复上报会让状态栏每轮刷一条「已结束」。
        self.status.take()
    }

    fn finished(&self) -> bool {
        self.ended
    }

    fn close(&mut self) {
        let _ = self.cmd.send(Command::Close);
    }

    fn description(&self) -> &str {
        &self.description
    }
}

impl Drop for SshChannel {
    fn drop(&mut self) {
        // 关标签页 = 挂断。丢一个 Close 进去，工作线程会走 `eof → close → disconnect`；
        // 不等它（join 会把 UI 卡住几十毫秒），线程自己收尾后结束。
        let _ = self.cmd.send(Command::Close);
    }
}

/// 供测试与诊断：连接超时的默认值，避免测试里写死秒数。
pub const DEFAULT_CONNECT_TIMEOUT: Duration = crate::options::DEFAULT_CONNECT_TIMEOUT;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_clean_remote_exit_has_no_code_in_the_text() {
        assert_eq!(finish(Some(0), false), " exited");
        assert_eq!(finish(Some(0), true), " exited");
    }

    #[test]
    fn a_failing_remote_exit_carries_its_number() {
        assert_eq!(finish(Some(3), false), " exited with code 3");
    }

    #[test]
    fn a_signal_is_not_reported_as_an_exit_code() {
        assert_eq!(finish(None, true), " stopped");
    }

    #[test]
    fn a_plain_close_without_status_says_so() {
        assert_eq!(finish(None, false), " connection closed");
    }

    #[test]
    fn a_dropped_connection_is_never_worded_like_a_finished_command() {
        assert_eq!(finish_lost(None), " connection lost");
        // 已知退出码时，仍然报那个码：命令真的跑完了，只是连接随后断了。
        assert_eq!(finish_lost(Some(7)), " exited with code 7");
        assert!(!finish_lost(None).contains("exited"));
    }

    #[test]
    fn a_closed_port_is_reported_as_a_connect_failure() {
        // 先占一个端口再放掉：这个端口此刻一定没人监听，连接会被立刻拒绝。
        // 不用伪造域名——那要靠 DNS 不解析，而 DNS 在 CI 里是最不可靠的一环。
        let port = std::net::TcpListener::bind(("127.0.0.1", 0))
            .expect("bind")
            .local_addr()
            .expect("addr")
            .port();
        let mut opts = SshOptions::new(
            "127.0.0.1",
            "root",
            crate::options::AuthMethod::Password {
                password: "x".to_string(),
            },
        );
        opts.port = port;
        opts.connect_timeout = Duration::from_secs(5);
        let e = connect(opts).unwrap_err();
        assert!(matches!(e, SshError::Connect(_)), "{e:?}");
        assert!(e.to_string().contains("无法连接"), "{e}");
    }

    #[test]
    fn an_incomplete_option_fails_before_any_thread_starts() {
        let mut opts = SshOptions::new(
            "",
            "root",
            crate::options::AuthMethod::Password {
                password: "x".to_string(),
            },
        );
        opts.host.clear();
        let e = connect(opts).unwrap_err();
        assert!(matches!(e, SshError::Config(_)), "{e:?}");
    }

    #[test]
    fn x11_request_payload_is_well_formed() {
        let payload = x11_request_payload(0);
        // single_connection = false
        assert_eq!(payload[0], 0);
        // auth protocol: len-prefixed "MIT-MAGIC-COOKIE-1"
        let proto_len = u32::from_be_bytes([payload[1], payload[2], payload[3], payload[4]]) as usize;
        assert_eq!(proto_len, 18);
        let proto = &payload[5..5 + proto_len];
        assert_eq!(proto, b"MIT-MAGIC-COOKIE-1");
        // auth cookie: 16 hex chars
        let cookie_off = 5 + proto_len;
        let cookie_len =
            u32::from_be_bytes(payload[cookie_off..cookie_off + 4].try_into().unwrap()) as usize;
        assert_eq!(cookie_len, 16);
        let cookie = &payload[cookie_off + 4..cookie_off + 4 + cookie_len];
        assert!(cookie.iter().all(|b| b.is_ascii_hexdigit()));
        // screen number: trailing u32 BE
        let screen_off = cookie_off + 4 + cookie_len;
        let screen = u32::from_be_bytes(
            payload[screen_off..screen_off + 4]
                .try_into()
                .unwrap(),
        );
        assert_eq!(screen, 0);
    }

    #[test]
    fn x11_cookie_is_sixteen_hex_digits() {
        let c = x11_cookie();
        assert_eq!(c.len(), 16);
        assert!(c.chars().all(|ch| ch.is_ascii_hexdigit()));
        // 两次调用不应撞车（计数器在起作用）
        assert_ne!(c, x11_cookie());
    }
}

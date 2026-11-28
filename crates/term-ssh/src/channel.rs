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

use std::sync::mpsc::{Receiver, RecvTimeoutError, Sender, TryRecvError, channel};
use std::thread::JoinHandle;
use std::time::Duration;

use russh::ChannelMsg;
use russh::client::{self, Handle};
use term_core::channel::TermChannel;

use crate::channel_x11::x11_request_payload;
use crate::error::SshError;
use crate::handler::HostKeyHandler;
use crate::options::SshOptions;

/// 每帧最多搬多少字节，免得一次 `find /` 把一帧卡住。
/// 与 `term-runtime` 的 `FRAME_BUDGET` 同值：两处都是「一帧的预算」，不该不一样。
const FRAME_BUDGET: usize = 256 * 1024;

/// 会话层握手（建链 + 认证 + open_session + pty + shell）的总预算。
///
/// 传输层每一段各自有超时，但服务端可以在**认证之后**对通道请求一直不回：
/// `channel_open_session` / `request_pty` / `request_shell` 的 russh 回复等待
/// 没有内建超时，于是 `connect()` 会永久阻塞在 `ready_rx.recv()`。
/// `connect_timeout = 0` 的「不设超时」在这里按 120 秒兜底——界面不能转圈 forever。
pub const HANDSHAKE_FALLBACK: Duration = Duration::from_secs(120);

/// 收到 `Eof` 之后再等多久 `Close` / `ExitStatus`。
///
/// SSH 的半关闭允许服务端先发 EOF、随后才送退出码或最后一批反向数据；
/// 一收到 EOF 就 `break` 会把它们全吞掉（状态栏显示 "connection closed" 而不是
/// 真实退出码）。兜底超时保证服务端只发 EOF 不关通道时也不会把会话挂死。
pub const EOF_GRACE: Duration = Duration::from_secs(2);

/// 收尾时观察 worker 自己退出的窗口。`connect()` 的超时分支只「尽量回收」——
/// 观察窗一过就放手（detach），绝不让自己变成第二个无限阻塞点。
const CLOSE_GRACE: Duration = Duration::from_secs(2);

/// 本次会话握手实际用的预算。0 = 不设超时，按 [`HANDSHAKE_FALLBACK`] 兜底。
fn handshake_budget(opts: &SshOptions) -> Duration {
    if opts.connect_timeout.is_zero() {
        HANDSHAKE_FALLBACK
    } else {
        opts.connect_timeout
    }
}

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

impl SshChannel {
    /// 取走最近一次「用户眼前那把服务器公钥」（openssh 单行）。
    ///
    /// 指纹确认之后用它把**同一把钥匙** pin 进 known_hosts，然后以 Strict
    /// 重连做真校验；绝不在重连那一刻让 Tofu 把服务器临时换给的钥匙记进去。
    pub fn take_presented_host_key(host: &str, port: u16) -> Option<String> {
        crate::handler::take_presented(host, port)
    }
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
    // DNS 解析**不在这里做**：`ToSocketAddrs` 是阻塞调用且没有内建超时，
    // 在 UI 线程上直接调用它，等于把「名字解析不出来」变成永久转圈。
    // 解析统一放进工作线程，由 `proxy::dial` 的带超时版本完成；这样有跳 /
    // 出网代理时也不会误在本地解析「只有代理那一侧才认识」的内网名字。
    let description = opts.description();
    let (cmd_tx, cmd_rx) = tokio::sync::mpsc::unbounded_channel();
    let (emit_tx, emit_rx) = channel();
    let (ready_tx, ready_rx) = channel::<Result<(), SshError>>();
    // worker 真正退出的信号。裸 `join()` 在线程卡死时会把 UI 一起钉住，
    // 有这个信号才能做「有界 join」。
    let (done_tx, done_rx) = channel::<()>();

    let budget = handshake_budget(&opts) + Duration::from_secs(5);
    let name = format!("term-ssh-{}", opts.host);
    let worker = std::thread::Builder::new()
        .name(name)
        .spawn(move || {
            run_worker(opts, cmd_rx, emit_tx, ready_tx);
            let _ = done_tx.send(());
        })
        .map_err(|e| SshError::Local(format!("无法启动 SSH 线程：{e}")))?;

    // 内层 `handshake()` 已有超时；这里再兜一次「线程还没进 runtime 就卡住」
    // 的内层管不到的窗口（建 runtime 之前不会执行 tokio::time::timeout）。
    let outcome = ready_rx.recv_timeout(budget);
    match outcome {
        Ok(Ok(())) => Ok(SshChannel {
            out: emit_rx,
            cmd: cmd_tx,
            status: None,
            ended: false,
            description,
            _worker: Some(worker),
        }),
        Ok(Err(e)) => {
            bounded_join(worker, done_rx, CLOSE_GRACE);
            Err(e)
        }
        // 线程连 `ready` 都没发就没了：通常是 panic 在建运行时之前。
        Err(RecvTimeoutError::Disconnected) => {
            bounded_join(worker, done_rx, CLOSE_GRACE);
            Err(SshError::Local(
                "SSH 线程在握手完成前退出（可能是平台不支持或内存不足）".to_string(),
            ))
        }
        // 二次兜底：先发取消信号，再有界回收。裸 join 会重新变成无限阻塞，
        // 那正是这条分支要消灭的故障。
        Err(RecvTimeoutError::Timeout) => {
            let _ = cmd_tx.send(Command::Close);
            bounded_join(worker, done_rx, CLOSE_GRACE);
            Err(SshError::Channel(format!(
                "{budget:?} 内没有收到握手结果（服务端不回应或本机资源不足）"
            )))
        }
    }
}

/// 等 worker 在 `bound` 内自己退出：收到完成信号才 `join()` 回收线程句柄，
/// 等不到就丢掉句柄让它继续收摊。调用方**不会**因为一个卡死的 worker 卡住。
fn bounded_join(worker: JoinHandle<()>, done: Receiver<()>, bound: Duration) {
    if done.recv_timeout(bound).is_ok() {
        let _ = worker.join();
    } else {
        drop(worker);
    }
}

/// `SshOptions` 会被 worker 的 async future 持有整条会话。握手一结束就把
/// 凭据清掉：此后留在内存里的只有 host / 端口 / 超时这些非敏感事实。
fn scrub_credentials(opts: &mut SshOptions) {
    fn clear(method: &mut crate::options::AuthMethod) {
        match method {
            crate::options::AuthMethod::Password { password } => password.clear(),
            crate::options::AuthMethod::PrivateKey { passphrase, .. } => {
                // 连「给没给过口令」这一位一起丢掉，不留下空壳。
                passphrase.take();
            }
            crate::options::AuthMethod::Agent => {}
            crate::options::AuthMethod::KeyboardInteractive { answers } => {
                for answer in answers {
                    answer.clear();
                }
            }
        }
    }
    clear(&mut opts.auth);
    for method in &mut opts.auth_chain {
        clear(method);
    }
    if let Some(proxy) = opts.outbound_proxy.as_mut() {
        if let Some(auth) = proxy.auth.as_mut() {
            auth.password.clear();
        }
    }
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
    mut opts: SshOptions,
    mut cmd_rx: tokio::sync::mpsc::UnboundedReceiver<Command>,
    emit: Sender<Emit>,
    ready: Sender<Result<(), SshError>>,
) {
    let budget = handshake_budget(&opts);
    let (handle, mut channel) = match tokio::time::timeout(budget, handshake(&opts)).await {
        Ok(Ok(pair)) => pair,
        Ok(Err(e)) => {
            let _ = ready.send(Err(e));
            return;
        }
        // 服务端在认证之后对 open_session / pty / shell 一直不回时，russh 的
        // 回复等待没有内建超时——这一层必须自己收口，否则 `connect()` 永久阻塞。
        Err(_) => {
            let _ = ready.send(Err(SshError::Channel(format!(
                "{budget:?} 内没有完成会话握手（服务端不回应通道请求）"
            ))));
            return;
        }
    };
    // 握手已经把凭据用完了；在 bona fide 连接上也不让密码多留一分钟。
    scrub_credentials(&mut opts);
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
    //
    // 唯一不肯「尽力而为」的地方是随机数：cookie 可预测等于白送远端 X display
    // 的访问控制，所以取不到熵时**不开 X11**（shell 照常），而不是发一个弱 cookie。
    if opts.x11_forward {
        match x11_request_payload(opts.x11_screen) {
            Ok(payload) => {
                if let Err(e) = handle.send_global_request("x11-req", &payload, true).await {
                    log::warn!("x11-req 下发失败（服务端可能禁了 X11 转发）：{e}");
                }
            }
            Err(e) => log::warn!("拒绝开启 X11 转发：{e}"),
        }
    }
    Ok((handle, channel))
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
    // SSH 半关闭：Eof 只表示服务端不再写，ExitStatus / 反向数据可能还在路上。
    let mut eof = false;

    loop {
        tokio::select! {
            msg = async {
                let waiting = channel.wait();
                if eof {
                    // 只发 Eof 不发 Close 的服务端不能把会话挂死；兜底按 Close 收尾。
                    match tokio::time::timeout(EOF_GRACE, waiting).await {
                        Ok(msg) => msg,
                        Err(_) => Some(ChannelMsg::Close),
                    }
                } else {
                    waiting.await
                }
            } => match msg {
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
                Some(ChannelMsg::Eof) => eof = true,
                Some(ChannelMsg::Close) => break,
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
    fn a_zero_connect_timeout_uses_the_handshake_fallback() {
        let mut opts = SshOptions::new(
            "h",
            "u",
            crate::options::AuthMethod::Password {
                password: "x".into(),
            },
        );
        opts.connect_timeout = Duration::ZERO;
        assert_eq!(handshake_budget(&opts), HANDSHAKE_FALLBACK);
        opts.connect_timeout = Duration::from_secs(7);
        assert_eq!(handshake_budget(&opts), Duration::from_secs(7));
    }

    #[test]
    fn credentials_are_wiped_after_the_handshake() {
        use crate::options::{AuthMethod, OutboundProxy, ProxyAuth, ProxyKind};
        let mut opts = SshOptions::new(
            "h",
            "u",
            AuthMethod::Password {
                password: "secret".into(),
            },
        );
        opts.auth_chain = vec![
            AuthMethod::PrivateKey {
                path: "/tmp/key".into(),
                passphrase: Some("phrase".into()),
            },
            AuthMethod::KeyboardInteractive {
                answers: vec!["answer".into()],
            },
        ];
        opts.outbound_proxy = Some(OutboundProxy {
            kind: ProxyKind::Http,
            host: "proxy".into(),
            port: 8080,
            auth: Some(ProxyAuth {
                username: "p".into(),
                password: "proxy-secret".into(),
            }),
            dns_over_proxy: true,
        });

        scrub_credentials(&mut opts);

        assert!(matches!(&opts.auth, AuthMethod::Password { password } if password.is_empty()));
        assert!(matches!(
            &opts.auth_chain[0],
            AuthMethod::PrivateKey { passphrase: None, .. }
        ));
        assert!(matches!(
            &opts.auth_chain[1],
            AuthMethod::KeyboardInteractive { answers } if answers.iter().all(String::is_empty)
        ));
        assert!(opts
            .outbound_proxy
            .as_ref()
            .and_then(|p| p.auth.as_ref())
            .is_some_and(|a| a.password.is_empty()));
        // 非凭据字段不能被顺手清掉：worker 之后还要用 timeout / 尺寸这些。
        assert_eq!(opts.host, "h");
        assert_eq!(opts.port, 22);
    }

    #[test]
    fn bounded_join_detaches_a_worker_that_never_signals_done() {
        // 模拟卡在内核里、永远不发完成信号的 worker。
        let (release_tx, release_rx) = channel::<()>();
        let worker = std::thread::spawn(move || {
            let _ = release_rx.recv();
        });
        let (_keep_done_alive, done_rx) = channel::<()>();
        let started = std::time::Instant::now();
        bounded_join(worker, done_rx, Duration::from_millis(30));
        assert!(
            started.elapsed() < Duration::from_secs(1),
            "有界 join 不能把调用方钉住"
        );
        let _ = release_tx.send(());
    }

    #[test]
    fn bounded_join_reaps_a_worker_that_finishes() {
        let (done_tx, done_rx) = channel::<()>();
        let worker = std::thread::spawn(move || {
            let _ = done_tx.send(());
        });
        bounded_join(worker, done_rx, Duration::from_secs(1));
    }
}

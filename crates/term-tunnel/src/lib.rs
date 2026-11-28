//! `term-tunnel` — SSH 隧道：本地转发 / 远程转发 / SOCKS5。
//!
//! ```text
//! 本地程序 ──TCP──▶ [本机监听] ──direct-tcpip──▶ [服务器] ──▶ 目标
//! 远端程序 ──TCP──▶ [服务器监听] ──forwarded-tcpip──▶ [本机] ──▶ 目标
//! SOCKS5 客户端 ──▶ [本机 SOCKS5] ──direct-tcpip──▶ [服务器] ──▶ 客户端指定的目标
//! ```
//!
//! 三条边界：
//! 1. **骑在 `term-ssh` 上**：握手 / 认证 / known_hosts 都用现成的（`Authenticated`），
//!    这里只负责「开通道 + 搬字节」。复制一份握手等于把 known_hosts 写两遍。
//! 2. **对外同步**：`Tunnel::start()` 阻塞到「所有监听都建好了」或「建不起来」为止。
//!    异步只在内部一条线程里（ADR-0017 §决策 2 的笼子）。
//! 3. **失败是局部的**：一条转发建不起来（端口被占、服务端不允许），
//!    `start()` 直接报错——半个隧道比没有隧道更难查。建起来之后某条连接断了，
//!    只影响那一条连接，不动其余。

mod background;
mod error;
mod forward;
mod socks5;

pub use background::{TunnelHandle, start_in_background};
pub use error::TunnelError;
pub use forward::{Forward, Kind, Target};

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;

use russh::Channel;
use russh::ChannelMsg;
use term_ssh::{Authenticated, ChannelOpener, Forwarded, SshOptions};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::Notify;

/// 收到远端 `Eof` 之后等 `Close` / 末尾数据的短兜底（见 `pump`）。
const EOF_GRACE: std::time::Duration = std::time::Duration::from_secs(2);

/// 「隧道还活着吗」拆成两件事：SSH 那条连接，以及每条规则的监听。
///
/// 过去一个 `AtomicBool` 谁都能写：某条 `-L` 的 accept 失败就把整条隧道
/// 标死，而 SSH 其实还好着；反过来 SSH 已经被服务端断开时，只要没有连接
/// 进来，监听任务永远不报错，界面就一直显示「在跑」。现在两类状态分开记，
/// `is_alive()` 两者都要求为真，SSH 那一半由 `ChannelOpener::is_closed()`
/// 主动轮询驱动，不再等「某条连接恰好失败」。
#[derive(Clone)]
struct Liveness {
    ssh: Arc<AtomicBool>,
    rules: Arc<Vec<Arc<AtomicBool>>>,
}

impl Liveness {
    fn new(rule_count: usize) -> Self {
        Self {
            ssh: Arc::new(AtomicBool::new(true)),
            rules: Arc::new(
                (0..rule_count)
                    .map(|_| Arc::new(AtomicBool::new(true)))
                    .collect(),
            ),
        }
    }

    fn is_alive(&self) -> bool {
        self.ssh.load(Ordering::Relaxed)
            && self.rules.iter().all(|r| r.load(Ordering::Relaxed))
    }

    fn close(&self) {
        self.ssh.store(false, Ordering::Relaxed);
        for rule in self.rules.iter() {
            rule.store(false, Ordering::Relaxed);
        }
    }
}

/// 一条在跑着的隧道。
pub struct Tunnel {
    stop: Arc<Notify>,
    alive: Liveness,
    /// 每条规则实际落在哪个端口（`-R` 让服务器挑端口时，这里才有意义）。
    ports: Arc<Mutex<Vec<u16>>>,
    rules: Vec<Forward>,
    worker: Option<JoinHandle<()>>,
}

impl std::fmt::Debug for Tunnel {
    /// 只印规则与端口：内部的 `Notify` / 线程句柄对日志没有信息量。
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Tunnel")
            .field("rules", &self.rules)
            .field("ports", &self.ports())
            .field("alive", &self.is_alive())
            .finish()
    }
}

impl Tunnel {
    /// 起隧道。返回时**所有监听都已就绪**（或已明确失败）。
    pub fn start(opts: SshOptions, forwards: Vec<Forward>) -> Result<Self, TunnelError> {
        if forwards.is_empty() {
            return Err(TunnelError::Config("没有任何转发规则".into()));
        }
        for f in &forwards {
            f.validate()?;
        }

        let stop = Arc::new(Notify::new());
        let alive = Liveness::new(forwards.len());
        let ports = Arc::new(Mutex::new(Vec::new()));
        let (ready_tx, ready_rx) = std::sync::mpsc::channel::<Result<Vec<u16>, TunnelError>>();

        let worker = {
            let stop = Arc::clone(&stop);
            let alive = alive.clone();
            let ports = Arc::clone(&ports);
            let rules = forwards.clone();
            std::thread::Builder::new()
                .name(format!("term-tunnel-{}", opts.host))
                .spawn(move || run(opts, rules, stop, alive, ports, ready_tx))
                .map_err(error::thread_failed)?
        };

        match ready_rx.recv_timeout(error::START_TIMEOUT) {
            Ok(Ok(found)) => {
                if let Ok(mut slot) = ports.lock() {
                    *slot = found;
                }
                Ok(Self {
                    stop,
                    alive,
                    ports,
                    rules: forwards,
                    worker: Some(worker),
                })
            }
            Ok(Err(e)) => {
                let _ = worker.join();
                Err(e)
            }
            Err(_) => {
                // 超时：线程可能还在握手。让它自己收尾（stop 一下），这里不等了。
                stop.notify_one();
                Err(TunnelError::Config(format!(
                    "{} 秒内没有把隧道建起来（对端不响应？）",
                    error::START_TIMEOUT.as_secs()
                )))
            }
        }
    }

    /// 每条规则实际绑定的端口，顺序与 `start()` 收到的规则一致。
    pub fn ports(&self) -> Vec<u16> {
        self.ports.lock().map(|p| p.clone()).unwrap_or_default()
    }

    /// 隧道还活着吗（SSH 连接断了 / 任意一条监听挂了就变 false）。
    pub fn is_alive(&self) -> bool {
        self.alive.is_alive()
    }

    /// 规则的可读描述（状态栏 / 日志）。
    pub fn describe(&self) -> Vec<String> {
        self.rules.iter().map(|f| f.describe()).collect()
    }

    /// 收摊：停监听、撤 `-R`、挂断连接。重复调用安全。
    pub fn close(&mut self) {
        self.stop.notify_one();
        self.alive.close();
        // 不等线程：join 会把调用方（UI）卡住几十毫秒。线程自己收尾后退出。
        self.worker.take();
    }
}

impl Drop for Tunnel {
    fn drop(&mut self) {
        self.close();
    }
}

fn run(
    opts: SshOptions,
    rules: Vec<Forward>,
    stop: Arc<Notify>,
    alive: Liveness,
    ports: Arc<Mutex<Vec<u16>>>,
    ready: std::sync::mpsc::Sender<Result<Vec<u16>, TunnelError>>,
) {
    let runtime = match tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    {
        Ok(rt) => rt,
        Err(e) => {
            let _ = ready.send(Err(TunnelError::Config(format!("无法创建异步运行时：{e}"))));
            alive.close();
            return;
        }
    };

    runtime.block_on(async move {
        let auth = match Authenticated::connect(&opts).await {
            Ok(a) => a,
            Err(e) => {
                let _ = ready.send(Err(e.into()));
                alive.close();
                return;
            }
        };
        let (opener, forwarded) = split(auth);

        // 先把所有监听 / 服务器侧监听建起来，再回报「就绪」。
        let mut bound: Vec<u16> = Vec::with_capacity(rules.len());
        let mut remote: Vec<(String, u16, Target)> = Vec::new();
        for (i, rule) in rules.iter().enumerate() {
            match rule {
                Forward::Local { bind, target } => match TcpListener::bind(bind).await {
                    Ok(listener) => {
                        let actual = listener.local_addr().map(|a| a.port()).unwrap_or(0);
                        bound.push(actual);
                        let opener = opener.clone();
                        // 每条规则一面自己的旗：一条挂了不该把整条隧道标死。
                        let alive = Arc::clone(&alive.rules[i]);
                        let listener_stop = Arc::clone(&stop);
                        let target = target.clone();
                        tokio::spawn(async move {
                            serve_local(listener, target, opener, alive, listener_stop).await
                        });
                    }
                    Err(e) => {
                        let _ = ready.send(Err(TunnelError::Listen {
                            addr: bind.clone(),
                            source: e,
                        }));
                        opener.disconnect().await;
                        alive.close();
                        return;
                    }
                },
                Forward::Socks5 { bind } => match TcpListener::bind(bind).await {
                    Ok(listener) => {
                        let actual = listener.local_addr().map(|a| a.port()).unwrap_or(0);
                        bound.push(actual);
                        let opener = opener.clone();
                        let alive = Arc::clone(&alive.rules[i]);
                        let listener_stop = Arc::clone(&stop);
                        tokio::spawn(async move {
                            serve_socks5(listener, opener, alive, listener_stop).await
                        });
                    }
                    Err(e) => {
                        let _ = ready.send(Err(TunnelError::Listen {
                            addr: bind.clone(),
                            source: e,
                        }));
                        opener.disconnect().await;
                        alive.close();
                        return;
                    }
                },
                Forward::Remote {
                    bind_addr,
                    bind_port,
                    target,
                } => match opener.remote_forward(bind_addr, *bind_port).await {
                    Ok(actual) => {
                        bound.push(actual);
                        remote.push((bind_addr.clone(), actual, target.clone()));
                    }
                    Err(e) => {
                        let _ = ready.send(Err(TunnelError::Forward(e.to_string())));
                        opener.disconnect().await;
                        alive.close();
                        return;
                    }
                },
            }
        }

        if !remote.is_empty() {
            let opener = opener.clone();
            let alive = Arc::clone(&alive.ssh);
            let listener_stop = Arc::clone(&stop);
            let rules_for_task = remote.clone();
            tokio::spawn(async move {
                serve_remote(forwarded, rules_for_task, opener, alive, listener_stop).await
            });
        }
        // SSH 可能在没有任何连接进来时被服务端断开；监听任务不会报错，
        // 所以主动盯着 opener 的 is_closed()，别让界面一直显示「在跑」。
        let watcher = opener.clone();
        let watcher_alive = alive.clone();
        let watcher_stop = stop.clone();
        tokio::spawn(async move {
            loop {
                if watcher.is_closed() {
                    watcher_alive.ssh.store(false, Ordering::Relaxed);
                    watcher_stop.notify_one();
                    break;
                }
                tokio::time::sleep(std::time::Duration::from_millis(500)).await;
            }
        });
        if let Ok(mut slot) = ports.lock() {
            *slot = bound.clone();
        }
        let _ = ready.send(Ok(bound));

        // 等到显式关闭，**或**任一监听任务退出。后者会先把自己的 `alive`
        // 置 false 再 notify——过去只置旗不叫醒，主任务就抱着 SSH 连接永远等下去。
        stop.notified().await;
        for (addr, port, _) in &remote {
            opener.cancel_remote_forward(addr, *port).await;
        }
        opener.disconnect().await;
        alive.close();
    });
}

/// 把 `Authenticated` 拆成「开通道手柄」+「远端推来的连接队列」。
fn split(auth: Authenticated) -> (ChannelOpener, tokio::sync::mpsc::UnboundedReceiver<Forwarded>) {
    let opener = auth.opener();
    (opener, auth.into_forwarded())
}

/// 监听任务退出的统一收尾：先把自己这条旗记死，再叫醒主任务。
///
/// 顺序很要紧：主任务被叫醒后走 `alive.close()`，若先 notify 后 store，
/// 它会看到一条还「活着」的旗。反过来没问题——`is_alive()` 此刻已经为假，
/// 主任务醒来就撤 `-R`、挂断 SSH，而不是抱着一条已死的隧道不释放。
fn listener_gone(alive: &AtomicBool, stop: &Notify) {
    alive.store(false, Ordering::Relaxed);
    stop.notify_one();
}

/// `-L`：本地监听，每条进来的连接开一条 `direct-tcpip` 通道。
async fn serve_local(
    listener: TcpListener,
    target: Target,
    opener: ChannelOpener,
    alive: Arc<AtomicBool>,
    stop: Arc<Notify>,
) {
    loop {
        let Ok((tcp, peer)) = listener.accept().await else {
            listener_gone(&alive, &stop);
            return;
        };
        let opener = opener.clone();
        let target = target.clone();
        tokio::spawn(async move {
            let peer_ip = peer.ip().to_string();
            match opener.direct_tcpip(&target.host, target.port, (&peer_ip, peer.port())).await {
                Ok(channel) => pump(channel, tcp).await,
                // 服务器拒绝（目标不可达 / 不允许转发）：直接关掉本地这条连接。
                // 不往 TCP 里写任何东西——这不是应用层协议，没有「错误帧」可发。
                Err(_) => drop(tcp),
            }
        });
    }
}

/// 从 `reader` 读到 `parse` 认为「消息完整」为止。
///
/// greeting 与 request 都可能半包到达——客户端把 VER、NMETHODS、方法列表
/// 拆到三个 TCP 段是常态，只读一次就把 `Short` 当协议错误是在制造假失败。
/// 两阶段共用这一条 `filled` 循环：先解析已有字节，`Short` 才继续读；
/// 真正的协议错误原样交回调用方回失败帧。
async fn read_complete<R, T, F>(
    reader: &mut R,
    buf: &mut [u8],
    filled: &mut usize,
    parse: F,
) -> Result<(T, usize), socks5::Socks5Error>
where
    R: AsyncRead + Unpin,
    F: Fn(&[u8]) -> Result<(T, usize), socks5::Socks5Error>,
{
    loop {
        match parse(&buf[..*filled]) {
            Ok(parsed) => return Ok(parsed),
            Err(socks5::Socks5Error::Short) => {}
            Err(e) => return Err(e),
        }
        if *filled == buf.len() {
            return Err(socks5::Socks5Error::Short);
        }
        match reader.read(&mut buf[*filled..]).await {
            Ok(0) | Err(_) => return Err(socks5::Socks5Error::Short),
            Ok(n) => *filled += n,
        }
    }
}

/// `-D`：SOCKS5，只做 `CONNECT`。
async fn serve_socks5(
    listener: TcpListener,
    opener: ChannelOpener,
    alive: Arc<AtomicBool>,
    stop: Arc<Notify>,
) {
    loop {
        let Ok((mut tcp, peer)) = listener.accept().await else {
            listener_gone(&alive, &stop);
            return;
        };
        let opener = opener.clone();
        tokio::spawn(async move {
            use socks5::{failure_reply, greeting_reply, read_greeting, read_request, success_reply};
            let mut buf = [0u8; 512];
            let mut filled = 0usize;

            // 1. 问候：半包也继续读，不再「只读一次」。
            let no_auth = match read_complete(&mut tcp, &mut buf, &mut filled, read_greeting).await {
                Ok((no_auth, used)) => {
                    // 客户端可能把问候和请求塞进同一个段：把问候没吃掉的字节
                    // 挪到缓冲开头留给下一阶段，绝不丢字节。
                    buf.copy_within(used..filled, 0);
                    filled -= used;
                    no_auth
                }
                Err(_) => {
                    let _ = tcp.write_all(&greeting_reply(false)).await;
                    return;
                }
            };
            if tcp.write_all(&greeting_reply(no_auth)).await.is_err() || !no_auth {
                return;
            }

            // 2. 请求：复用同一块缓冲与 filled，半包继续读到完整为止。
            let req = match read_complete(&mut tcp, &mut buf, &mut filled, read_request).await {
                Ok((req, _)) => req,
                Err(e) => {
                    let code = match e {
                        socks5::Socks5Error::UnsupportedCommand(_) => {
                            socks5::REP_COMMAND_NOT_SUPPORTED
                        }
                        socks5::Socks5Error::UnsupportedAddress(_) => {
                            socks5::REP_ADDRESS_NOT_SUPPORTED
                        }
                        _ => socks5::REP_GENERAL_FAILURE,
                    };
                    let _ = tcp.write_all(&failure_reply(code)).await;
                    return;
                }
            };

            // 3. 开通道并回「成功」，然后原样搬字节。
            let peer_ip = peer.ip().to_string();
            match opener.direct_tcpip(&req.host, req.port, (&peer_ip, peer.port())).await {
                Ok(channel) => {
                    if tcp.write_all(&success_reply()).await.is_ok() {
                        pump(channel, tcp).await;
                    }
                }
                Err(_) => {
                    let _ = tcp.write_all(&failure_reply(socks5::REP_HOST_UNREACHABLE)).await;
                }
            }
        });
    }
}

/// `-R`：服务器推来的每条连接，回到本机连 `target`。
async fn serve_remote(
    mut forwarded: tokio::sync::mpsc::UnboundedReceiver<Forwarded>,
    rules: Vec<(String, u16, Target)>,
    opener: ChannelOpener,
    alive: Arc<AtomicBool>,
    stop: Arc<Notify>,
) {
    while let Some(conn) = forwarded.recv().await {
        let Some(target) = pick_target(&rules, conn.connected_port) else {
            // 没有匹配的规则：这条连接不该存在（我们没请求过这个端口）。
            let _ = conn.channel.close().await;
            continue;
        };
        tokio::spawn(async move {
            match TcpStream::connect((target.host.as_str(), target.port)).await {
                Ok(tcp) => pump(conn.channel, tcp).await,
                Err(_) => {
                    let _ = conn.channel.close().await;
                }
            }
        });
    }
    let _ = opener;
    // 服务端不再推连接 = 这条 `-R` 规则已经死了。同样要叫醒主任务收摊，
    // 否则 SSH 连接会一直留着。
    listener_gone(&alive, &stop);
}

/// 服务端报回来的 `connected_address/port` → 该走哪条规则。
///
/// 按端口匹配：服务器常常把 `0.0.0.0` 或 `localhost` 报成别的写法，
/// 端口是唯一稳定的东西。只有一条 `-R` 时端口也一定对得上（就是我们请求的那个）。
fn pick_target(rules: &[(String, u16, Target)], connected_port: u32) -> Option<Target> {
    rules
        .iter()
        .find(|(_, port, _)| u32::from(*port) == connected_port)
        .map(|(_, _, target)| target.clone())
        .or_else(|| {
            // 兜底只对「服务端把端口报成 0」生效，且只在只有一条规则时——
            // 报了一个我们没请求过的端口，说明这条连接不该被转发，
            // 硬套到唯一那条规则上就是把流量送到错误的地方。
            if connected_port == 0 && rules.len() == 1 {
                Some(rules[0].2.clone())
            } else {
                None
            }
        })
}

/// SSH 通道 ↔ TCP 连接，双向搬字节。
///
/// 任一方向结束就收摊：半开的转发连接没有用处，还会一直占着服务器那边的窗口。
async fn pump(mut channel: Channel<russh::client::Msg>, tcp: TcpStream) {
    let (mut read_half, mut write_half) = tcp.into_split();
    let mut buf = vec![0u8; 16 * 1024];
    // 本地读端到 EOF 之后就要**停止再 select 它**：已 EOF 的 socket 每次
    // `read` 都立刻返回 `Ok(0)`，留着这一臂等于把 loop 变成空转——CPU 打满，
    // 而且每个循环都往 SSH 通道灌一条 EOF。远端那一侧还没收尾时（回应还在
    // 路上），我们又不能直接 break，所以只撤掉这一臂，继续转发反方向。
    let mut local_eof = false;
    // 远端 Eof 只是半关闭：最后一批数据可能还在路上。一收到就 break 会把
    // 反向数据吞掉；再给一段短兜底超时，服务端只发 Eof 也不至于挂死。
    let mut remote_eof = false;
    loop {
        tokio::select! {
            msg = async {
                let waiting = channel.wait();
                if remote_eof {
                    match tokio::time::timeout(EOF_GRACE, waiting).await {
                        Ok(msg) => msg,
                        Err(_) => Some(ChannelMsg::Close),
                    }
                } else {
                    waiting.await
                }
            } => match msg {
                Some(ChannelMsg::Data { data }) => {
                    if write_half.write_all(&data).await.is_err() {
                        break;
                    }
                }
                Some(ChannelMsg::ExtendedData { data, .. }) => {
                    if write_half.write_all(&data).await.is_err() {
                        break;
                    }
                }
                Some(ChannelMsg::Eof) => remote_eof = true,
                Some(ChannelMsg::Close) | None => break,
                _ => {}
            },
            n = read_half.read(&mut buf), if !local_eof => match n {
                Ok(0) => {
                    // 本地写完了：告诉远端 EOF，但别急着关——回应可能还在路上。
                    let _ = channel.eof().await;
                    local_eof = true;
                }
                Ok(n) => {
                    if channel.data(&buf[..n]).await.is_err() {
                        break;
                    }
                }
                Err(_) => break,
            },
        }
    }
    let _ = write_half.shutdown().await;
    let _ = channel.eof().await;
    let _ = channel.close().await;
}

#[cfg(test)]
mod tests {
    use super::*;

    fn opts() -> SshOptions {
        SshOptions::new(
            "127.0.0.1",
            "root",
            term_ssh::AuthMethod::Password {
                password: "x".into(),
            },
        )
    }

    /// 活性由「SSH 连接 + 每条规则监听」共同决定：单独一条监听挂掉不能
    /// 冒充 SSH 断了，SSH 断了也不能靠「监听还在 accept」被判成活着。
    #[test]
    fn liveness_combines_ssh_connection_and_each_rule_listener() {
        let live = Liveness::new(2);
        assert!(live.is_alive());
        live.rules[0].store(false, Ordering::Relaxed);
        assert!(!live.is_alive(), "一条规则挂了整条隧道就不算活着");
        // 其它规则不受影响：把那条恢复，SSH 仍然好着。
        live.rules[0].store(true, Ordering::Relaxed);
        assert!(live.is_alive());
        live.ssh.store(false, Ordering::Relaxed);
        assert!(!live.is_alive(), "SSH 关了才算真的断了");
        // close() 把两类旗一起收，重复调用安全。
        live.close();
        assert!(!live.is_alive());
    }

    #[test]
    fn a_tunnel_needs_at_least_one_rule() {
        let e = Tunnel::start(opts(), Vec::new()).unwrap_err();
        assert!(matches!(e, TunnelError::Config(_)), "{e:?}");
    }

    #[test]
    fn a_bad_rule_is_refused_before_any_thread_starts() {
        // 端口 0 的本地监听：规则本身就错，连不上网络就该被挡下。
        let e = Tunnel::start(opts(), vec![Forward::local("127.0.0.1:0", "db", 5432)]).unwrap_err();
        assert!(matches!(e, TunnelError::Config(_)), "{e:?}");
    }

    #[test]
    fn a_remote_connection_without_a_matching_rule_goes_nowhere() {
        let rules = vec![("127.0.0.1".to_string(), 9000u16, Target::new("localhost", 3000))];
        assert_eq!(pick_target(&rules, 1234), None);
    }

    #[test]
    fn a_single_remote_rule_is_used_even_when_the_server_reports_port_zero() {
        let rules = vec![("127.0.0.1".to_string(), 0u16, Target::new("localhost", 3000))];
        assert_eq!(pick_target(&rules, 0), Some(Target::new("localhost", 3000)));
    }

    #[test]
    fn the_port_is_what_decides_between_two_remote_rules() {
        let rules = vec![
            ("127.0.0.1".to_string(), 9000u16, Target::new("localhost", 3000)),
            ("127.0.0.1".to_string(), 9001u16, Target::new("db.internal", 5432)),
        ];
        assert_eq!(pick_target(&rules, 9001), Some(Target::new("db.internal", 5432)));
        assert_eq!(pick_target(&rules, 9000), Some(Target::new("localhost", 3000)));
    }

    /// 监听任务死掉必须**同时**记死和叫醒主任务；少 notify 那一下，SSH 连接
    /// 会被主任务一直抱着（这正是 P2-2 的故障形状）。
    #[tokio::test]
    async fn a_dead_listener_wakes_the_worker_after_marking_itself_dead() {
        let alive = Arc::new(AtomicBool::new(true));
        let stop = Arc::new(Notify::new());
        let notified = stop.notified();
        tokio::pin!(notified);
        listener_gone(&alive, &stop);
        tokio::time::timeout(std::time::Duration::from_secs(1), notified)
            .await
            .expect("监听任务退出必须叫醒主任务");
        assert!(!alive.load(Ordering::Relaxed), "叫醒之前旗已经该是 false");
    }

    /// 可控分片的模拟流：chunk=1 是逐字节到达，chunk 很大则整段一起到达。
    struct ChunkedReader {
        bytes: Vec<u8>,
        pos: usize,
        chunk: usize,
    }

    impl AsyncRead for ChunkedReader {
        fn poll_read(
            mut self: std::pin::Pin<&mut Self>,
            _cx: &mut std::task::Context<'_>,
            buf: &mut tokio::io::ReadBuf<'_>,
        ) -> std::task::Poll<std::io::Result<()>> {
            if self.pos >= self.bytes.len() {
                return std::task::Poll::Ready(Ok(()));
            }
            let n = self.chunk.min(self.bytes.len() - self.pos);
            buf.put_slice(&self.bytes[self.pos..self.pos + n]);
            self.pos += n;
            std::task::Poll::Ready(Ok(()))
        }
    }

    #[tokio::test]
    async fn a_greeting_fed_one_byte_at_a_time_is_read_in_full() {
        // VER / NMETHODS=3 / 三个方法，无认证在第 2 个：拆成 5 次 read。
        let mut src = ChunkedReader {
            bytes: vec![5, 3, 2, 0, 1],
            pos: 0,
            chunk: 1,
        };
        let mut buf = [0u8; 512];
        let mut filled = 0usize;
        let (no_auth, used) = read_complete(&mut src, &mut buf, &mut filled, socks5::read_greeting)
            .await
            .expect("半包 greeting 不该被当成协议错误");
        assert!(no_auth);
        assert_eq!(used, 5);
        assert_eq!(filled, 5, "整条 greeting 都在缓冲里");
    }

    #[tokio::test]
    async fn greeting_and_request_in_one_segment_leaves_the_request_bytes_for_phase_two() {
        let mut wire = vec![5, 1, 0]; // greeting
        wire.extend_from_slice(&[5, 1, 0, 1, 10, 0, 0, 7, 0x1F, 0x90]); // request
        let mut src = ChunkedReader {
            bytes: wire,
            pos: 0,
            chunk: 512,
        };
        let mut buf = [0u8; 512];
        let mut filled = 0usize;
        let (no_auth, used) = read_complete(&mut src, &mut buf, &mut filled, socks5::read_greeting)
            .await
            .expect("greeting");
        assert!(no_auth);
        buf.copy_within(used..filled, 0);
        filled -= used;
        assert_eq!(filled, 10, "greeting 之后的请求字节不能被吞掉");
        let (req, _) = read_complete(&mut src, &mut buf, &mut filled, socks5::read_request)
            .await
            .expect("请求应当直接从未消费字节里解析出来");
        assert_eq!(req.host, "10.0.0.7");
        assert_eq!(req.port, 8080);
    }
}

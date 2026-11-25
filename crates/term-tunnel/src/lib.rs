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
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::Notify;

/// 收到远端 `Eof` 之后等 `Close` / 末尾数据的短兜底（见 `pump`）。
const EOF_GRACE: std::time::Duration = std::time::Duration::from_secs(2);

/// 一条在跑着的隧道。
pub struct Tunnel {
    stop: Arc<Notify>,
    alive: Arc<AtomicBool>,
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
        let alive = Arc::new(AtomicBool::new(true));
        let ports = Arc::new(Mutex::new(Vec::new()));
        let (ready_tx, ready_rx) = std::sync::mpsc::channel::<Result<Vec<u16>, TunnelError>>();

        let worker = {
            let stop = Arc::clone(&stop);
            let alive = Arc::clone(&alive);
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

    /// 隧道还活着吗（连接断了 / 某条监听挂了就变 false）。
    pub fn is_alive(&self) -> bool {
        self.alive.load(Ordering::Relaxed)
    }

    /// 规则的可读描述（状态栏 / 日志）。
    pub fn describe(&self) -> Vec<String> {
        self.rules.iter().map(|f| f.describe()).collect()
    }

    /// 收摊：停监听、撤 `-R`、挂断连接。重复调用安全。
    pub fn close(&mut self) {
        self.stop.notify_one();
        self.alive.store(false, Ordering::Relaxed);
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
    alive: Arc<AtomicBool>,
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
            alive.store(false, Ordering::Relaxed);
            return;
        }
    };

    runtime.block_on(async move {
        let auth = match Authenticated::connect(&opts).await {
            Ok(a) => a,
            Err(e) => {
                let _ = ready.send(Err(e.into()));
                alive.store(false, Ordering::Relaxed);
                return;
            }
        };
        let (opener, forwarded) = split(auth);

        // 先把所有监听 / 服务器侧监听建起来，再回报「就绪」。
        let mut bound: Vec<u16> = Vec::with_capacity(rules.len());
        let mut remote: Vec<(String, u16, Target)> = Vec::new();
        for rule in &rules {
            match rule {
                Forward::Local { bind, target } => match TcpListener::bind(bind).await {
                    Ok(listener) => {
                        let actual = listener.local_addr().map(|a| a.port()).unwrap_or(0);
                        bound.push(actual);
                        let opener = opener.clone();
                        let alive = Arc::clone(&alive);
                        let target = target.clone();
                        tokio::spawn(async move {
                            serve_local(listener, target, opener, alive).await
                        });
                    }
                    Err(e) => {
                        let _ = ready.send(Err(TunnelError::Listen {
                            addr: bind.clone(),
                            source: e,
                        }));
                        opener.disconnect().await;
                        alive.store(false, Ordering::Relaxed);
                        return;
                    }
                },
                Forward::Socks5 { bind } => match TcpListener::bind(bind).await {
                    Ok(listener) => {
                        let actual = listener.local_addr().map(|a| a.port()).unwrap_or(0);
                        bound.push(actual);
                        let opener = opener.clone();
                        let alive = Arc::clone(&alive);
                        tokio::spawn(async move { serve_socks5(listener, opener, alive).await });
                    }
                    Err(e) => {
                        let _ = ready.send(Err(TunnelError::Listen {
                            addr: bind.clone(),
                            source: e,
                        }));
                        opener.disconnect().await;
                        alive.store(false, Ordering::Relaxed);
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
                        alive.store(false, Ordering::Relaxed);
                        return;
                    }
                },
            }
        }

        if !remote.is_empty() {
            let opener = opener.clone();
            let alive = Arc::clone(&alive);
            let rules_for_task = remote.clone();
            tokio::spawn(async move {
                serve_remote(forwarded, rules_for_task, opener, alive).await
            });
        }
        if let Ok(mut slot) = ports.lock() {
            *slot = bound.clone();
        }
        let _ = ready.send(Ok(bound));

        // 一直到被要求关闭。监听任务自己死掉也算结束（`alive` 被置 false）。
        stop.notified().await;
        for (addr, port, _) in &remote {
            opener.cancel_remote_forward(addr, *port).await;
        }
        opener.disconnect().await;
        alive.store(false, Ordering::Relaxed);
    });
}

/// 把 `Authenticated` 拆成「开通道手柄」+「远端推来的连接队列」。
fn split(auth: Authenticated) -> (ChannelOpener, tokio::sync::mpsc::UnboundedReceiver<Forwarded>) {
    let opener = auth.opener();
    (opener, auth.into_forwarded())
}

/// `-L`：本地监听，每条进来的连接开一条 `direct-tcpip` 通道。
async fn serve_local(
    listener: TcpListener,
    target: Target,
    opener: ChannelOpener,
    alive: Arc<AtomicBool>,
) {
    loop {
        let Ok((tcp, peer)) = listener.accept().await else {
            alive.store(false, Ordering::Relaxed);
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

/// `-D`：SOCKS5，只做 `CONNECT`。
async fn serve_socks5(listener: TcpListener, opener: ChannelOpener, alive: Arc<AtomicBool>) {
    use socks5::{failure_reply, greeting_reply, read_greeting, read_request, success_reply};
    loop {
        let Ok((mut tcp, peer)) = listener.accept().await else {
            alive.store(false, Ordering::Relaxed);
            return;
        };
        let opener = opener.clone();
        tokio::spawn(async move {
            let mut buf = [0u8; 512];

            // 1. 问候
            let n = match tcp.read(&mut buf).await {
                Ok(0) | Err(_) => return,
                Ok(n) => n,
            };
            let no_auth = match read_greeting(&buf[..n]) {
                Ok((no_auth, _)) => no_auth,
                Err(_) => {
                    let _ = tcp.write_all(&greeting_reply(false)).await;
                    return;
                }
            };
            if tcp.write_all(&greeting_reply(no_auth)).await.is_err() || !no_auth {
                return;
            }

            // 2. 请求（可能半包，读到完整为止）
            let mut filled = 0usize;
            let (req, used) = loop {
                if filled == buf.len() {
                    let _ = tcp.write_all(&failure_reply(socks5::REP_GENERAL_FAILURE)).await;
                    return;
                }
                match tcp.read(&mut buf[filled..]).await {
                    Ok(0) | Err(_) => return,
                    Ok(n) => filled += n,
                }
                match read_request(&buf[..filled]) {
                    Ok(pair) => break pair,
                    Err(socks5::Socks5Error::Short) => continue,
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
                }
            };
            let _ = used;

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
    alive.store(false, Ordering::Relaxed);
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
}

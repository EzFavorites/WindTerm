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
mod tunnel_serve;

pub use background::{TunnelHandle, start_in_background};
pub use error::TunnelError;
pub use forward::{Forward, Kind, Target};

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;

use term_ssh::{Authenticated, ChannelOpener, Forwarded, SshOptions};
use tokio::net::TcpListener;
use tokio::sync::Notify;

use tunnel_serve::{serve_local, serve_remote, serve_socks5};
#[cfg(test)]
use tokio::io::AsyncRead;
#[cfg(test)]
use tunnel_serve::{listener_gone, pick_target, read_complete};

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

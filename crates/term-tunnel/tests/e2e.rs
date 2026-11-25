//! 端到端：真起一个 SSH 服务端（russh 的 server 端，同一份库），
//! 经隧道把字节送到一个真 TCP 回显服务，再读回来。
//!
//! 为什么值得这样测：隧道最容易错的地方全在接缝上——`direct-tcpip` 的参数顺序、
//! 「本地连接什么时候知道自己该关了」、SOCKS5 的半包处理。把这些 mock 掉，
//! 测的就只是自己的想象。这里既不碰外网、也不跑 `sshd`：服务端只监听 `127.0.0.1`。
//!
//! 覆盖范围：`-L`（本地转发）、`-D`（SOCKS5 CONNECT）与 `-R`（远程转发）。
//! `-R` 的做法与另两条不同：连接是**服务端**推过来的，所以 mock 服务端要主动
//! 开一条 `forwarded-tcpip` 通道（`session.handle()` 上就能开），这也正是
//! 客户端 `server_channel_open_forwarded_tcpip` 那个回调存在的理由。

use std::io::{Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use russh::server::{self, Auth, Msg, Server as _, Session};
use russh::{Channel, ChannelId};

use term_ssh::{AuthMethod, HostKeyPolicy, SshOptions};
use term_tunnel::{Forward, Tunnel};

// 原先这里 `include_str!` 一份入库的测试私钥；公开化清理删掉了所有私钥材料，
// 所以改成运行时生成（与 `term-ssh/tests/e2e.rs::host_key` 同一套做法）。
// 私钥绝不进仓库：GitHub 的密钥扫描会拦，且测试本来也不需要固定密钥。
use ssh_key::{Algorithm, LineEnding, PrivateKey as SshPrivKey};
use ssh_key::getrandom::SysRng;
use ssh_key::rand_core::UnwrapErr;

fn host_key() -> russh::keys::PrivateKey {
    let mut rng = UnwrapErr(SysRng);
    let k = SshPrivKey::random(&mut rng, Algorithm::Ed25519).expect("生成测试主机密钥");
    russh::keys::PrivateKey::from_openssh(
        k.to_openssh(LineEnding::default())
            .expect("序列化测试主机密钥"),
    )
    .expect("解析测试主机密钥")
}

/// 服务端记下的事实，供测试断言。
#[derive(Debug, Default)]
struct Observed {
    /// 客户端请求转发的目标（`host_to_connect` / `port_to_connect`）。
    targets: Vec<(String, u32)>,
    /// 客户端请求的 `-R` 监听（`tcpip-forward` 的 address / port）。
    forward_requests: Vec<(String, u32)>,
    /// `-R` 场景里，服务端经隧道发出去再收回来的字节（空 = 没走通）。
    remote_echo: Vec<u8>,
}

#[derive(Clone)]
struct MockServer {
    password: String,
    backend: SocketAddr,
    observed: Arc<Mutex<Observed>>,
}

impl server::Server for MockServer {
    type Handler = MockHandler;

    fn new_client(&mut self, _: Option<SocketAddr>) -> Self::Handler {
        MockHandler {
            password: self.password.clone(),
            backend: self.backend,
            observed: Arc::clone(&self.observed),
        }
    }
}

struct MockHandler {
    password: String,
    backend: SocketAddr,
    observed: Arc<Mutex<Observed>>,
}

impl server::Handler for MockHandler {
    type Error = russh::Error;

    async fn auth_password(&mut self, _user: &str, password: &str) -> Result<Auth, Self::Error> {
        Ok(if password == self.password {
            Auth::Accept
        } else {
            Auth::reject()
        })
    }

    async fn auth_publickey(
        &mut self,
        _user: &str,
        _key: server::VerifiedPublicKey<'_>,
    ) -> Result<Auth, Self::Error> {
        Ok(Auth::Accept)
    }

    /// 服务端根本不看客户端要连哪：一律代理到测试自己的回显后端。
    /// 但**记下**客户端请求的目标——参数顺序写错时，这条断言会红。
    #[allow(clippy::too_many_arguments)]
    async fn channel_open_direct_tcpip(
        &mut self,
        channel: Channel<Msg>,
        host_to_connect: &str,
        port_to_connect: u32,
        _originator_address: &str,
        _originator_port: u32,
        reply: server::ChannelOpenHandle,
        _session: &mut Session,
    ) -> Result<(), Self::Error> {
        if let Ok(mut o) = self.observed.lock() {
            o.targets.push((host_to_connect.to_string(), port_to_connect));
        }
        reply.accept().await;
        let backend = self.backend;
        tokio::spawn(async move {
            let Ok(tcp) = tokio::net::TcpStream::connect(backend).await else {
                return;
            };
            proxy(channel, tcp).await;
        });
        Ok(())
    }

    async fn channel_open_session(
        &mut self,
        _channel: Channel<Msg>,
        reply: server::ChannelOpenHandle,
        _session: &mut Session,
    ) -> Result<(), Self::Error> {
        reply.accept().await;
        Ok(())
    }

    async fn shell_request(
        &mut self,
        channel: ChannelId,
        session: &mut Session,
    ) -> Result<(), Self::Error> {
        let _ = session.channel_success(channel);
        Ok(())
    }

    /// `-R`：客户端请求我们在某个端口上监听。
    ///
    /// 真的 `sshd` 会去 bind 那个端口、等外来连接；这里只做**最关键的一半**：
    /// 接受请求，然后主动开一条 `forwarded-tcpip` 通道推到客户端，
    /// 看看字节能不能走到客户端那一侧的 target 再回来。
    async fn tcpip_forward(
        &mut self,
        address: &str,
        port: &mut u32,
        session: &mut Session,
    ) -> Result<bool, Self::Error> {
        let requested = *port;
        if let Ok(mut o) = self.observed.lock() {
            o.forward_requests.push((address.to_string(), requested));
        }
        // 真实服务端在 `port == 0` 时会自己挑一个再填回去；这里端口是测试指定的，
        // 原样回给客户端即可。
        let handle = session.handle();
        let observed = Arc::clone(&self.observed);
        tokio::spawn(async move {
            // 等客户端的 `serve_remote` 起起来（它要等上面这个应答）。
            tokio::time::sleep(Duration::from_millis(200)).await;
            let Ok(mut channel) = handle
                .channel_open_forwarded_tcpip("127.0.0.1", requested, "203.0.113.7", 5555)
                .await
            else {
                return;
            };
            let _ = channel.data(&b"ping\n"[..]).await;
            let mut got = Vec::new();
            while got.len() < 5 {
                match channel.wait().await {
                    Some(russh::ChannelMsg::Data { data }) => got.extend_from_slice(&data),
                    Some(russh::ChannelMsg::Eof) | Some(russh::ChannelMsg::Close) | None => break,
                    _ => {}
                }
            }
            if let Ok(mut o) = observed.lock() {
                o.remote_echo = got;
            }
            let _ = channel.close().await;
        });
        Ok(true)
    }
}

/// 服务端侧的双向搬运（与 `term-tunnel` 里那份同形，但它是独立的实现——
/// 两边各写一遍，谁写错了另一个方向就会暴露）。
async fn proxy(mut channel: Channel<Msg>, tcp: tokio::net::TcpStream) {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let (mut rd, mut wr) = tcp.into_split();
    let mut buf = vec![0u8; 8192];
    loop {
        tokio::select! {
            msg = channel.wait() => match msg {
                Some(russh::ChannelMsg::Data { data }) => {
                    if wr.write_all(&data).await.is_err() { break; }
                }
                Some(russh::ChannelMsg::Eof) | Some(russh::ChannelMsg::Close) | None => break,
                _ => {}
            },
            n = rd.read(&mut buf) => match n {
                Ok(0) => { let _ = channel.eof().await; }
                Ok(n) => { if channel.data(&buf[..n]).await.is_err() { break; } }
                Err(_) => break,
            },
        }
    }
    let _ = wr.shutdown().await;
    let _ = channel.close().await;
}

/// 起一个只监听本机的 mock 服务端，返回地址与观测记录。
fn start_server(backend: SocketAddr) -> (SocketAddr, Arc<Mutex<Observed>>) {
    let observed = Arc::new(Mutex::new(Observed::default()));
    let server = MockServer {
        password: "test-password".to_string(),
        backend,
        observed: Arc::clone(&observed),
    };
    // 服务端自己在 tokio runtime 里 bind 并把真实地址回传：不再"先占端口再放掉"
    // ——那一招在全量门禁下有 bind 竞态，会让本地转发 e2e 偶发红。
    let (addr_tx, addr_rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("建测试运行时");
        rt.block_on(async move {
            let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0))
                .await
                .expect("mock 服务端 bind");
            let addr = listener.local_addr().expect("mock 服务端 addr");
            addr_tx.send(addr).expect("回传服务端地址");
            let key = host_key();
            let config = Arc::new(server::Config {
                keys: vec![key],
                ..Default::default()
            });
            let mut srv = server;
            srv.run_on_socket(config, &listener).await.expect("mock 服务端退出");
        });
    });
    let addr = addr_rx
        .recv_timeout(Duration::from_secs(30))
        .expect("mock 服务端 30 秒内没绑好");
    (addr, observed)
}

/// 本地 TCP 回显服务，返回它的地址。
fn echo_backend() -> SocketAddr {
    let listener = TcpListener::bind(("127.0.0.1", 0)).expect("bind 回显服务");
    let addr = listener.local_addr().expect("addr");
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut s) = stream else { return };
            std::thread::spawn(move || {
                let mut buf = [0u8; 1024];
                loop {
                    match s.read(&mut buf) {
                        Ok(0) | Err(_) => return,
                        Ok(n) => {
                            if s.write_all(&buf[..n]).is_err() {
                                return;
                            }
                        }
                    }
                }
            });
        }
    });
    addr
}

/// 「先占一个端口再放掉」：russh 的 `run_on_address` 自己 bind，
/// 拿不到「它绑到哪」；这是它自己测试里的同一招。
fn free_port() -> SocketAddr {
    TcpListener::bind(("127.0.0.1", 0))
        .expect("bind 临时端口")
        .local_addr()
        .expect("取本地地址")
}

/// 等服务端真的在监听。
///
/// `run_on_address` 在另一个线程里 bind，测试线程可能先跑到 connect 那一步——
/// 服务端地址已由 start_server 回传，这里只做一次快速探活，给到 15 秒上限，
/// 起不来就直接失败（而不是让测试随机红）。
fn wait_for_server(addr: SocketAddr) {
    let deadline = Instant::now() + Duration::from_secs(15);
    loop {
        match TcpStream::connect_timeout(&addr, Duration::from_millis(200)) {
            Ok(_) => return,
            Err(_) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(20)),
            Err(e) => panic!("mock 服务端 15 秒内没起来：{e}"),
        }
    }
}

fn ssh_options(addr: SocketAddr) -> SshOptions {
    let mut opts = SshOptions::new(
        "127.0.0.1",
        "tester",
        AuthMethod::Password {
            password: "test-password".to_string(),
        },
    );
    opts.port = addr.port();
    // 每个测试都写一份自己的 known_hosts，绝不碰用户的 ~/.ssh/known_hosts。
    opts.host_key = HostKeyPolicy::Insecure;
    opts.connect_timeout = Duration::from_secs(10);
    opts
}

/// 连上去、说一句话、读回同一句话。
fn roundtrip(addr: SocketAddr, line: &[u8]) -> Vec<u8> {
    let mut stream = TcpStream::connect(addr).expect("连隧道端口");
    // 30s 读上限只吸收全量门禁并发时的线程调度延迟（mock 服务端/隧道线程可能晚被调度），不是协议等待；实测已够，不要继续放宽。
    stream.set_read_timeout(Some(Duration::from_secs(30))).unwrap();
    stream.write_all(line).expect("写");
    let mut back = vec![0u8; line.len()];
    stream.read_exact(&mut back).expect("读回显");
    back
}

#[test]
fn a_local_forward_carries_bytes_both_ways() {
    let backend = echo_backend();
    let (ssh, observed) = start_server(backend);
    wait_for_server(ssh);
    let listen = free_port();

    let mut tunnel = Tunnel::start(
        ssh_options(ssh),
        vec![Forward::local(
            format!("127.0.0.1:{}", listen.port()),
            "db.internal",
            5432,
        )],
    )
    .expect("隧道应当起得来");

    assert!(tunnel.is_alive());
    assert_eq!(tunnel.ports(), vec![listen.port()]);

    let got = roundtrip(listen, b"hello through the tunnel\n");
    assert_eq!(got, b"hello through the tunnel\n");

    // 服务端收到的目标必须与我们写的一致（参数顺序写错会在这里暴露）。
    let targets = observed.lock().unwrap().targets.clone();
    assert_eq!(targets, vec![("db.internal".to_string(), 5432)]);

    tunnel.close();
}

#[test]
fn a_socks5_proxy_connects_to_the_target_the_client_names() {
    let backend = echo_backend();
    let (ssh, observed) = start_server(backend);
    wait_for_server(ssh);
    let listen = free_port();

    let mut tunnel = Tunnel::start(
        ssh_options(ssh),
        vec![Forward::socks5(format!("127.0.0.1:{}", listen.port()))],
    )
    .expect("Socks5 隧道应当起得来");

    // --- SOCKS5 握手：只提供「无认证」 ---
    let mut stream = TcpStream::connect(listen).expect("连 SOCKS5 端口");
    stream.set_read_timeout(Some(Duration::from_secs(30))).unwrap();
    stream.write_all(&[5, 1, 0]).unwrap();
    let mut pick = [0u8; 2];
    stream.read_exact(&mut pick).unwrap();
    assert_eq!(pick, [5, 0]);

    // --- CONNECT 到一个**域名**目标：证明域名是原样交给服务器的 ---
    let host = b"db.internal";
    let mut req = vec![5, 1, 0, 3, host.len() as u8];
    req.extend_from_slice(host);
    req.extend_from_slice(&443u16.to_be_bytes());
    stream.write_all(&req).unwrap();

    let mut reply = [0u8; 10];
    stream.read_exact(&mut reply).unwrap();
    assert_eq!(reply[1], 0, "SOCKS5 应答应当是成功：{reply:?}");

    // --- 之后就是一条裸 TCP 通道 ---
    stream.write_all(b"ping\n").unwrap();
    let mut back = [0u8; 5];
    stream.read_exact(&mut back).unwrap();
    assert_eq!(&back, b"ping\n");

    let targets = observed.lock().unwrap().targets.clone();
    assert_eq!(targets, vec![("db.internal".to_string(), 443)]);

    tunnel.close();
}

#[test]
fn a_tunnel_closes_its_listener_so_nothing_is_left_behind() {    let backend = echo_backend();
    let (ssh, _) = start_server(backend);
    wait_for_server(ssh);
    let listen = free_port();

    let mut tunnel = Tunnel::start(
        ssh_options(ssh),
        vec![Forward::local(format!("127.0.0.1:{}", listen.port()), "x", 22)],
    )
    .expect("隧道应当起得来");
    roundtrip(listen, b"ok\n");
    tunnel.close();

    // 端口应当很快被释放：给它一点时间，然后必须能重新绑上。
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        match TcpListener::bind(listen) {
            Ok(_) => break,
            Err(_) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(50)),
            Err(e) => panic!("关闭后端口仍被占用：{e}"),
        }
    }
}

#[test]
fn a_remote_forward_carries_bytes_from_the_server_side() {
    // 场景：服务器上「有人连了那个端口」→ 服务器把它推给我们 →
    // 我们连本机 target（这里的回显服务）→ 字节原路返回。
    let backend = echo_backend();
    let (ssh, observed) = start_server(backend);
    wait_for_server(ssh);
    let bind = free_port();

    let mut tunnel = Tunnel::start(
        ssh_options(ssh),
        vec![Forward::remote(
            "127.0.0.1",
            bind.port(),
            "127.0.0.1",
            backend.port(),
        )],
    )
    .expect("远程转发应当请求得下来");

    assert_eq!(tunnel.ports(), vec![bind.port()], "服务器应回报我们请求的端口");
    // 服务端收到的请求就是命令行里写的那个地址与端口。
    let requests = observed.lock().unwrap().forward_requests.clone();
    assert_eq!(requests, vec![("127.0.0.1".to_string(), u32::from(bind.port()))]);

    // 服务端推过来的那条连接走完了整条路：它写进去的 `ping` 必须原样回来。
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let got = observed.lock().unwrap().remote_echo.clone();
        if !got.is_empty() {
            assert_eq!(got, b"ping\n");
            break;
        }
        if Instant::now() >= deadline {
            panic!("10 秒内没有从 -R 通路上收回字节");
        }
        std::thread::sleep(Duration::from_millis(20));
    }

    tunnel.close();
}

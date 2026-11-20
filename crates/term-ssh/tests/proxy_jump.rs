//! ProxyJump 端到端：进程内起 N 台「能当跳板、也能当目标」的 SSH 服务端，
//! 真走一遍 `TCP → 第 1 跳 → … → 目标` 的链路。
//!
//! 为什么值得单独一个文件：链路上真正容易错的不是「跳的顺序」（那是纯函数，
//! 在 `proxy.rs` 的单测里钉死了），而是**每一条 direct-tcpip 通道能否承载下一次
//! SSH 握手**——即 russh 的通道流是否真的是双向字节流、上游 `Handle` 会不会
//! 在第二跳握手中途被丢掉。这只能靠真连一遍来证明。
//!
//! 与 `docker_sshd.rs` 的分工：那边要 Docker 与真 sshd（默认 `#[ignore]`），
//! 这边只要进程内两三个 mock 服务端，所以**一直跑**。mock 服务端的
//! `direct-tcpip` 行为与 OpenSSH 一致：接受通道 → 连 `host:port` → 原样搬字节。

use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use russh::server::{self, Auth, Msg, Server as _, Session};
use russh::{Channel, ChannelId, Pty};
use ssh_key::{Algorithm, LineEnding, PrivateKey as SshPrivKey};
use ssh_key::getrandom::SysRng;
use ssh_key::rand_core::UnwrapErr;

use term_core::channel::TermChannel;
use term_ssh::{AuthMethod, HostKeyPolicy, SshError, SshOptions, connect};

/// 测试用 ed25519 私钥：运行时生成，**不再入库**（避免公开仓库被 GitHub 密钥扫描拦截）。
fn gen_ed25519_key() -> SshPrivKey {
    let mut rng = UnwrapErr(SysRng);
    SshPrivKey::random(&mut rng, Algorithm::Ed25519).expect("生成测试私钥")
}

/// 服务端用的主机私钥：运行时生成后转成 russh 需要的类型。
fn host_key() -> russh::keys::PrivateKey {
    let k = gen_ed25519_key();
    russh::keys::PrivateKey::from_openssh(
        k.to_openssh(LineEnding::default())
            .expect("序列化测试主机密钥"),
    )
    .expect("解析测试主机密钥")
}

/// 服务端记下的事实。跳板机与目标机用的是同一个实现，靠这些记录区分角色。
#[derive(Debug, Default)]
struct Observed {
    /// 每一次成功的密码认证用的用户名。
    users: Vec<String>,
    /// 每一次 `direct-tcpip` 请求的（主机，端口）。
    tunnels: Vec<(String, u32)>,
}

#[derive(Clone)]
struct MockServer {
    password: String,
    observed: Arc<Mutex<Observed>>,
}

impl server::Server for MockServer {
    type Handler = MockHandler;

    fn new_client(&mut self, _: Option<SocketAddr>) -> Self::Handler {
        MockHandler {
            password: self.password.clone(),
            observed: Arc::clone(&self.observed),
            session: None,
            agent: None,
        }
    }
}

struct MockHandler {
    password: String,
    observed: Arc<Mutex<Observed>>,
    /// 这条连接上的 shell 通道。russh 会把**所有**通道的数据都送进
    /// `Handler::data`（包括 direct-tcpip 隧道），所以必须按 id 区分：
    /// 照着全部回显会把隧道变成一面镜子（客户端读到自己刚写的 SSH 标识，
    /// 第二次握手直接摔在 `Bad packet size` 上）。
    session: Option<ChannelId>,
    /// 服务端主动开的那条 agent 通道（`-A` 用例）。
    agent: Option<ChannelId>,
}

impl MockHandler {
    fn record(&self, f: impl FnOnce(&mut Observed)) {
        if let Ok(mut o) = self.observed.lock() {
            f(&mut o);
        }
    }
}

impl server::Handler for MockHandler {
    type Error = russh::Error;

    async fn auth_password(&mut self, user: &str, password: &str) -> Result<Auth, Self::Error> {
        if password != self.password {
            return Ok(Auth::reject());
        }
        let user = user.to_string();
        self.record(move |o| o.users.push(user));
        Ok(Auth::Accept)
    }

    async fn auth_publickey(
        &mut self,
        _user: &str,
        _key: server::VerifiedPublicKey<'_>,
    ) -> Result<Auth, Self::Error> {
        Ok(Auth::Accept)
    }

    async fn channel_open_session(
        &mut self,
        channel: Channel<Msg>,
        reply: server::ChannelOpenHandle,
        _session: &mut Session,
    ) -> Result<(), Self::Error> {
        self.session = Some(channel.id());
        reply.accept().await;
        Ok(())
    }

    /// 这就是「跳板机」的全部职责：把这条通道接到 `host:port` 上，然后搬字节。
    ///
    /// 与 OpenSSH 相比少了两件事（对测试无影响，写在这里免得以后误读）：
    /// 不做 `AllowTcpForwarding` 之类的策略判断，也不限制目标地址。
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
        let host = host_to_connect.to_string();
        let port = port_to_connect;
        self.record(|o| o.tunnels.push((host.clone(), port)));

        reply.accept().await;
        // 连不上时直接丢掉 channel：它被 drop 就会发 Close，
        // 客户端会看到「通道开出来了但立刻断了」——这正是真 sshd 的表现。
        tokio::spawn(async move {
            let Ok(tcp) = tokio::net::TcpStream::connect((host.as_str(), port as u16)).await else {
                return;
            };
            let mut remote = channel.into_stream();
            let mut tcp = tcp;
            let _ = tokio::io::copy_bidirectional(&mut remote, &mut tcp).await;
            eprintln!("relay to {host}:{port} finished");
        });
        Ok(())
    }

    /// 客户端确认了 agent 通道：这下可以往里写请求了。
    async fn channel_open_confirmation(
        &mut self,
        id: ChannelId,
        _max_packet_size: u32,
        _window_size: u32,
        session: &mut Session,
    ) -> Result<(), Self::Error> {
        if Some(id) == self.agent {
            session.data(id, &b"PING"[..])?;
        }
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    async fn pty_request(
        &mut self,
        channel: ChannelId,
        _term: &str,
        _col_width: u32,
        _row_height: u32,
        _pix_width: u32,
        _pix_height: u32,
        _modes: &[(Pty, u32)],
        session: &mut Session,
    ) -> Result<(), Self::Error> {
        let _ = session.channel_success(channel);
        Ok(())
    }

    async fn shell_request(
        &mut self,
        channel: ChannelId,
        session: &mut Session,
    ) -> Result<(), Self::Error> {
        let _ = session.channel_success(channel);
        session.data(channel, &b"mock shell ready\r\n"[..])?;
        Ok(())
    }

    async fn data(
        &mut self,
        channel: ChannelId,
        data: &[u8],
        session: &mut Session,
    ) -> Result<(), Self::Error> {
        // agent 通道：把「本地 agent 回了什么」转述到 shell 通道上，
        // 让测试能从屏幕这一侧看到它。这是 `-A` 用例的观察窗。
        if Some(channel) == self.agent {
            let text = format!("AGENT-REPLY:{}\r\n", String::from_utf8_lossy(data));
            if let Some(shell) = self.session {
                session.data(shell, text.into_bytes())?;
            }
            return Ok(());
        }

        if Some(channel) == self.session {
            // 真 sshd 在 `-A` 下会替远端把 `$SSH_AUTH_SOCK` 指过来，
            // 这里用一个显式命令模拟「远端有人去用 agent」。
            if data.windows(8).any(|w| w == b"GO-AGENT") {
                // 只开通道，不立刻发数据：russh 要求通道被对端确认之后才能写
                // （提前写会撞上库里的 `assert!(channel.confirmed)`），
                // 所以真正的请求放在 `channel_open_confirmation` 里发。
                self.agent = Some(session.channel_open_agent()?);
                return Ok(());
            }
            // 只回显 shell 通道：隧道里的字节由 `channel_open_direct_tcpip`
            // 那条通道流负责搬运，在这里再回显一遍就变成镜子了。
            session.data(channel, data.to_vec())?;
        }
        Ok(())
    }
}

/// 起一台只监听本机的 mock 服务端，返回地址与共享的观测记录。
fn start_server(password: &str) -> (SocketAddr, Arc<Mutex<Observed>>) {
    let addr = std::net::TcpListener::bind(("127.0.0.1", 0))
        .expect("bind 临时端口")
        .local_addr()
        .expect("取本地地址");

    let observed = Arc::new(Mutex::new(Observed::default()));
    let server = MockServer {
        password: password.to_string(),
        observed: Arc::clone(&observed),
    };

    std::thread::spawn(move || {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("建测试运行时");
        rt.block_on(async move {
            let key = host_key();
            let config = Arc::new(server::Config {
                keys: vec![key],
                ..Default::default()
            });
            let mut srv = server;
            if let Err(e) = srv.run_on_address(config, addr).await {
                panic!("mock 服务端退出：{e}");
            }
        });
    });

    let deadline = Instant::now() + Duration::from_secs(5);
    while std::net::TcpStream::connect(addr).is_err() {
        assert!(Instant::now() < deadline, "mock 服务端 5 秒内没起来");
        std::thread::sleep(Duration::from_millis(10));
    }
    (addr, observed)
}

fn password(pw: &str) -> AuthMethod {
    AuthMethod::Password {
        password: pw.to_string(),
    }
}

/// 连 `target`，中间按 `jump` 给出的 ProxyJump 串走。
fn options(target: SocketAddr, jump: &str) -> SshOptions {
    let mut opts = SshOptions::new("127.0.0.1", "alice", password("pw"));
    opts.port = target.port();
    // 链路本身的验证不掺 known_hosts：那三条分支在 `known_hosts` 里有专门的测试。
    opts.host_key = HostKeyPolicy::Insecure;
    opts.connect_timeout = Duration::from_secs(10);
    opts.keepalive = None;
    opts.proxy_jump = term_ssh::parse_proxy_jump(jump).expect("ProxyJump 串应能解析");
    opts
}

/// 一直 drain 到出现 `needle` 或者超时。
fn wait_for(ch: &mut dyn TermChannel, needle: &str, timeout: Duration) -> String {
    let deadline = Instant::now() + timeout;
    let mut seen = String::new();
    while Instant::now() < deadline {
        let bytes = ch.drain();
        if !bytes.is_empty() {
            seen.push_str(&String::from_utf8_lossy(&bytes));
            if seen.contains(needle) {
                return seen;
            }
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    panic!("{timeout:?} 内没等到 {needle:?}，只收到 {seen:?}");
}

#[test]
fn one_hop_opens_a_tunnel_and_carries_bytes_to_the_target() {
    let (target, target_seen) = start_server("pw");
    let (bastion, bastion_seen) = start_server("pw");

    let hop = format!("deploy@127.0.0.1:{}", bastion.port());
    let mut ch = connect(options(target, &hop)).expect("一跳应当能连上");
    let banner = wait_for(&mut ch, "mock shell ready", Duration::from_secs(10));
    assert!(banner.contains("ready"), "{banner:?}");

    // 跳板机确实替我们连了目标的地址。
    let tunnels = bastion_seen.lock().unwrap().tunnels.clone();
    assert_eq!(tunnels.len(), 1, "跳板机应当恰好开一条隧道：{tunnels:?}");
    assert_eq!(tunnels[0].0, "127.0.0.1");
    assert_eq!(tunnels[0].1, u32::from(target.port()));

    // 用户名各是各的：跳用跳上写的，目标用会话的。
    assert_eq!(bastion_seen.lock().unwrap().users.clone(), vec!["deploy"]);
    assert_eq!(target_seen.lock().unwrap().users.clone(), vec!["alice"]);

    // 端到端跑一次字节：键盘 → 跳板机的隧道 → 目标 → 回显。
    ch.write(b"hello-through-jump\r");
    let echoed = wait_for(&mut ch, "hello-through-jump", Duration::from_secs(10));
    assert!(echoed.contains("hello-through-jump"), "{echoed:?}");
}

#[test]
fn a_hop_without_a_user_inherits_the_session_user() {
    let (target, _target_seen) = start_server("pw");
    let (bastion, bastion_seen) = start_server("pw");

    let hop = format!("127.0.0.1:{}", bastion.port());
    let mut ch = connect(options(target, &hop)).expect("一跳应当能连上");
    wait_for(&mut ch, "mock shell ready", Duration::from_secs(10));

    assert_eq!(
        bastion_seen.lock().unwrap().users.clone(),
        vec!["alice"],
        "跳上没写用户名时应当沿用会话的用户名"
    );
}

#[test]
fn two_hops_chain_through_each_other() {
    let (target, target_seen) = start_server("pw");
    let (far, far_seen) = start_server("pw");
    let (near, near_seen) = start_server("pw");

    // near 是第 1 跳，far 是第 2 跳：第 1 跳只看得见 far，far 才看得见 target。
    let jump = format!("deploy@127.0.0.1:{},ops@127.0.0.1:{}", near.port(), far.port());
    let mut ch = connect(options(target, &jump)).expect("两跳应当能连上");
    let banner = wait_for(&mut ch, "mock shell ready", Duration::from_secs(10));
    assert!(banner.contains("ready"), "{banner:?}");

    let near_tunnels = near_seen.lock().unwrap().tunnels.clone();
    assert_eq!(near_tunnels.len(), 1);
    assert_eq!(near_tunnels[0].1, u32::from(far.port()), "第 1 跳应当连第 2 跳");

    let far_tunnels = far_seen.lock().unwrap().tunnels.clone();
    assert_eq!(far_tunnels.len(), 1);
    assert_eq!(
        far_tunnels[0].1,
        u32::from(target.port()),
        "第 2 跳应当连目标"
    );

    assert_eq!(near_seen.lock().unwrap().users.clone(), vec!["deploy"]);
    assert_eq!(far_seen.lock().unwrap().users.clone(), vec!["ops"]);
    assert_eq!(target_seen.lock().unwrap().users.clone(), vec!["alice"]);

    ch.write(b"deep\r");
    wait_for(&mut ch, "deep", Duration::from_secs(10));
}

#[test]
fn a_dead_jump_is_reported_with_the_hop_that_failed() {
    // 借一个端口再放掉：此刻一定没人监听，连接会被立刻拒绝。
    let port = std::net::TcpListener::bind(("127.0.0.1", 0))
        .expect("bind")
        .local_addr()
        .expect("addr")
        .port();
    let (target, _seen) = start_server("pw");

    let hop = format!("deploy@127.0.0.1:{port},ops@127.0.0.1:{}", target.port());
    let e = connect(options(target, &hop)).expect_err("第 1 跳不可达时必须失败");
    match &e {
        SshError::Connect(_) => {}
        other => panic!("应当是 Connect 错误，实际是 {other:?}"),
    }
    let text = e.to_string();
    assert!(text.contains("ProxyJump 第 1 跳"), "{text}");
    assert!(text.contains("deploy"), "{text}");
}

/// 直连也走同一条代码路径（0 跳），确认链式改造没有把最简单的情形弄坏。
#[test]
fn zero_hops_still_means_a_plain_direct_connection() {
    let (target, seen) = start_server("pw");
    let mut ch = connect(options(target, "")).expect("直连应当能连上");
    wait_for(&mut ch, "mock shell ready", Duration::from_secs(10));

    assert!(seen.lock().unwrap().tunnels.is_empty(), "直连不该开隧道");
    assert_eq!(seen.lock().unwrap().users.clone(), vec!["alice"]);
}

/// `ssh -A`：服务端开 agent 通道 → 桥到本地 `$SSH_AUTH_SOCK` → 字节来回都通。
///
/// 这条用例会改 `SSH_AUTH_SOCK`，所以它是**本进程里唯一**读这个变量的测试：
/// 别的用例都不开 `agent_forward`，`agent_forward_error` 根本不会去看环境。
#[cfg(unix)]
#[test]
fn agent_forwarding_bridges_the_local_agent() {
    use std::io::{Read, Write};
    use std::os::unix::net::UnixListener;

    let dir = std::env::temp_dir().join(format!("term-ssh-agent-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("建临时目录");
    let sock = dir.join("agent.sock");
    let listener = UnixListener::bind(&sock).expect("bind 假 agent");

    // 假 agent：收到 4 个字节就回一个固定的答复。
    let agent = std::thread::spawn(move || {
        let (mut conn, _) = listener.accept().expect("agent 应被连上");
        let mut buf = [0u8; 4];
        conn.read_exact(&mut buf).expect("agent 应收到请求");
        assert_eq!(&buf, b"PING", "远端应当把 agent 请求原样送到本地");
        conn.write_all(b"PONG").expect("agent 应能回话");
    });

    let (target, _seen) = start_server("pw");
    let mut opts = options(target, "");
    opts.agent_forward = true;
    std::env::set_var("SSH_AUTH_SOCK", &sock);

    let mut ch = connect(opts).expect("开了 -A 也应当能连上");
    wait_for(&mut ch, "mock shell ready", Duration::from_secs(10));
    // 让服务端去开 agent 通道（见 mock 里的 `GO-AGENT`）。
    ch.write(b"GO-AGENT\r");
    let seen = wait_for(&mut ch, "AGENT-REPLY:PONG", Duration::from_secs(10));
    assert!(seen.contains("AGENT-REPLY:PONG"), "{seen:?}");

    agent.join().expect("假 agent 线程不应 panic");
    let _ = std::fs::remove_dir_all(&dir);
}

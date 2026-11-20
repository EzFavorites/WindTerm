//! 端到端：真开一个 SSH 服务端（russh 的 server 端，同一份库），真连上去，
//! 真收发字节。
//!
//! 为什么不用「mock 掉 russh」的办法：这一层最容易错的地方恰恰是与库的接缝——
//! `check_server_key` 的返回值、`request_pty` 的参数顺序、`ExitStatus` 的时机。
//! 把库 mock 掉就等于把这些接缝全测成自己的想象。进程内起一个真服务端，
//! 既不碰网络（127.0.0.1）、也不需要跑 `sshd`，CI 上照样能跑。
//!
//! 服务端只做四件事：认密码 / 认公钥、给 PTY、给 shell、把收到的字节原样回显。
//! 收到 `exit` 时按预设的退出码收尾——这样「退出码 → 状态栏文本」也能验到。

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

/// 带口令的加密版测试私钥，用于「缺口令应报错」的用例。
fn gen_ed25519_key_encrypted(pass: &str) -> SshPrivKey {
    let mut rng = UnwrapErr(SysRng);
    let k = SshPrivKey::random(&mut rng, Algorithm::Ed25519).expect("生成测试私钥");
    k.encrypt(&mut SysRng, pass).expect("加密测试私钥")
}

/// 把运行时生成的私钥写成 OpenSSH 格式文件，返回路径。
fn write_key_file(name: &str, key: &SshPrivKey) -> std::path::PathBuf {
    let path =
        std::env::temp_dir().join(format!("term-ssh-e2e-{}-{}", std::process::id(), name));
    key.write_openssh_file(&path, LineEnding::default())
        .expect("写入测试私钥文件");
    path
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

/// 服务端记下的事实，供测试断言。
#[derive(Debug, Default)]
struct Observed {
    /// 客户端发来的原始字节。
    received: Vec<u8>,
    /// 客户端申请的 PTY 尺寸。
    pty: Option<(u32, u32)>,
    /// 之后的每一次改尺寸。
    resizes: Vec<(u32, u32)>,
}

#[derive(Clone)]
struct MockServer {
    password: String,
    exit_code: u32,
    observed: Arc<Mutex<Observed>>,
}

impl server::Server for MockServer {
    type Handler = MockHandler;

    fn new_client(&mut self, _: Option<SocketAddr>) -> Self::Handler {
        MockHandler {
            password: self.password.clone(),
            exit_code: self.exit_code,
            observed: Arc::clone(&self.observed),
        }
    }
}

struct MockHandler {
    password: String,
    exit_code: u32,
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

    async fn channel_open_session(
        &mut self,
        _channel: Channel<Msg>,
        reply: server::ChannelOpenHandle,
        _session: &mut Session,
    ) -> Result<(), Self::Error> {
        reply.accept().await;
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    async fn pty_request(
        &mut self,
        channel: ChannelId,
        _term: &str,
        col_width: u32,
        row_height: u32,
        _pix_width: u32,
        _pix_height: u32,
        _modes: &[(Pty, u32)],
        session: &mut Session,
    ) -> Result<(), Self::Error> {
        self.record(|o| o.pty = Some((col_width, row_height)));
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

    async fn window_change_request(
        &mut self,
        _channel: ChannelId,
        col_width: u32,
        row_height: u32,
        _pix_width: u32,
        _pix_height: u32,
        _session: &mut Session,
    ) -> Result<(), Self::Error> {
        self.record(|o| o.resizes.push((col_width, row_height)));
        Ok(())
    }

    async fn data(
        &mut self,
        channel: ChannelId,
        data: &[u8],
        session: &mut Session,
    ) -> Result<(), Self::Error> {
        self.record(|o| o.received.extend_from_slice(data));
        if data == b"exit\r" || data == b"exit" {
            session.exit_status_request(channel, self.exit_code)?;
            session.eof(channel)?;
            session.close(channel)?;
            return Ok(());
        }
        // 回显：证明「键盘 → 服务器 → 屏幕」这一趟是通的。
        session.data(channel, data.to_vec())?;
        Ok(())
    }
}

impl MockHandler {
    fn record(&self, f: impl FnOnce(&mut Observed)) {
        if let Ok(mut o) = self.observed.lock() {
            f(&mut o);
        }
    }
}

/// 起一个只监听本机的 mock 服务端，返回它的地址与共享的观测记录。
fn start_server(password: &str, exit_code: u32) -> (SocketAddr, Arc<Mutex<Observed>>) {
    // 先借系统分一个空闲端口再放掉：russh 的 `run_on_address` 自己 bind，
    // 没法从外面拿「它绑到了哪」。这是它自己测试里用的同一招。
    let addr = std::net::TcpListener::bind(("127.0.0.1", 0))
        .expect("bind 临时端口")
        .local_addr()
        .expect("取本地地址");

    let observed = Arc::new(Mutex::new(Observed::default()));
    let server = MockServer {
        password: password.to_string(),
        exit_code,
        observed: Arc::clone(&observed),
    };
    let log = Arc::clone(&observed);

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
                // 服务端起不来时把话说清楚，否则测试只会卡在「等不到 banner」。
                panic!("mock 服务端退出：{e}");
            }
        });
    });

    // 等服务端真的开始 listen（否则第一个 connect 会撞上 ECONNREFUSED）。
    let deadline = Instant::now() + Duration::from_secs(5);
    while std::net::TcpStream::connect(addr).is_err() {
        assert!(Instant::now() < deadline, "mock 服务端 5 秒内没起来");
        std::thread::sleep(Duration::from_millis(10));
    }
    (addr, log)
}

fn options(addr: SocketAddr, auth: AuthMethod) -> SshOptions {
    let mut opts = SshOptions::new("127.0.0.1", "tester", auth);
    opts.port = addr.port();
    // 端到端测试不校验 known_hosts：那三条分支在 `known_hosts` 的单测里已经覆盖，
    // 这里要验的是通道本身。
    opts.host_key = HostKeyPolicy::Insecure;
    opts.cols = 100;
    opts.rows = 30;
    opts.connect_timeout = Duration::from_secs(10);
    opts
}

fn password(pw: &str) -> AuthMethod {
    AuthMethod::Password {
        password: pw.to_string(),
    }
}

/// 一直 drain 到出现 `needle` 或者超时。返回这段时间收到的全部字节。
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
fn a_password_session_carries_bytes_in_both_directions() {
    let (addr, observed) = start_server("s3cret", 0);
    let mut ch = connect(options(addr, password("s3cret"))).expect("应当能连上");

    let banner = wait_for(&mut ch, "mock shell ready", Duration::from_secs(10));
    assert!(banner.contains("mock shell ready"), "{banner:?}");
    assert_eq!(ch.description(), format!("ssh://tester@127.0.0.1:{}", addr.port()));

    // 上行：键盘 → 服务器；下行：服务器回显 → 屏幕。
    ch.write(b"hello\r");
    let echoed = wait_for(&mut ch, "hello", Duration::from_secs(10));
    assert!(echoed.contains("hello"), "{echoed:?}");

    let got = observed.lock().unwrap().received.clone();
    assert_eq!(got, b"hello\r", "服务端收到的字节应与敲进去的一致");
    assert!(!ch.finished(), "会话还开着，不该报结束");
}

#[test]
fn the_pty_size_is_requested_up_front_and_resizes_propagate() {
    let (addr, observed) = start_server("s3cret", 0);
    let mut ch = connect(options(addr, password("s3cret"))).expect("应当能连上");
    wait_for(&mut ch, "mock shell ready", Duration::from_secs(10));

    assert_eq!(
        observed.lock().unwrap().pty,
        Some((100, 30)),
        "申请 PTY 时带过去的尺寸应当是初始的 100x30"
    );

    ch.resize(120, 40);
    let deadline = Instant::now() + Duration::from_secs(10);
    while Instant::now() < deadline {
        if observed.lock().unwrap().resizes.contains(&(120, 40)) {
            return;
        }
        let _ = ch.drain();
        std::thread::sleep(Duration::from_millis(5));
    }
    panic!(
        "改尺寸没有传到服务端，只收到 {:?}",
        observed.lock().unwrap().resizes
    );
}

#[test]
fn a_remote_exit_code_becomes_the_status_text() {
    let (addr, _observed) = start_server("s3cret", 3);
    let mut ch = connect(options(addr, password("s3cret"))).expect("应当能连上");
    wait_for(&mut ch, "mock shell ready", Duration::from_secs(10));

    ch.write(b"exit\r");

    let deadline = Instant::now() + Duration::from_secs(10);
    let mut status = None;
    while Instant::now() < deadline {
        let _ = ch.drain();
        if let Some(text) = ch.poll() {
            status = Some(text);
            break;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    assert_eq!(status.as_deref(), Some(" exited with code 3"));

    // 收尾之后再 drain 一次，`finished()` 必须是稳定的真值——
    // Supervisor 靠它决定「这一轮之后可以拆会话」。
    let _ = ch.drain();
    assert!(ch.finished());
    assert_eq!(ch.poll(), None, "状态文本只上报一次，否则状态栏会一直刷");
}

#[test]
fn a_wrong_password_is_reported_as_an_auth_failure() {
    let (addr, _observed) = start_server("s3cret", 0);
    let e = connect(options(addr, password("wrong"))).unwrap_err();
    match e {
        SshError::Auth(p) => {
            assert_eq!(p.user, "tester");
            assert_eq!(p.method, "password");
        }
        other => panic!("应当是 Auth 失败，实际是 {other:?}"),
    }
}

#[test]
fn a_private_key_file_authenticates() {
    let (addr, _observed) = start_server("unused", 0);
    let path = write_key_file("id_ed25519", &gen_ed25519_key());
    let mut opts = options(
        addr,
        AuthMethod::PrivateKey {
            path,
            passphrase: None,
        },
    );
    opts.connect_timeout = Duration::from_secs(10);
    let mut ch = connect(opts).expect("私钥认证应当成功");
    let banner = wait_for(&mut ch, "mock shell ready", Duration::from_secs(10));
    assert!(banner.contains("ready"), "{banner:?}");
}

#[test]
fn a_locked_key_without_its_passphrase_says_which_file_needs_one() {
    let (addr, _observed) = start_server("unused", 0);
    let path = write_key_file(
        "id_ed25519_locked",
        &gen_ed25519_key_encrypted("test-passphrase"),
    );
    let opts = options(
        addr,
        AuthMethod::PrivateKey {
            path: path.clone(),
            passphrase: None,
        },
    );
    let e = connect(opts).unwrap_err();
    match e {
        SshError::Key(msg) => {
            assert!(msg.contains("口令"), "{msg}");
            assert!(msg.contains("id_ed25519_locked"), "{msg}");
        }
        other => panic!("应当是 Key 错误，实际是 {other:?}"),
    }
}

#[test]
fn tofu_records_the_server_key_on_the_first_connect() {
    let (addr, _observed) = start_server("s3cret", 0);
    let dir = std::env::temp_dir().join(format!("term-ssh-e2e-tofu-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("建临时目录");
    let known = dir.join("known_hosts");
    let _ = std::fs::remove_file(&known);

    let mut opts = options(addr, password("s3cret"));
    opts.host_key = HostKeyPolicy::Tofu;
    opts.known_hosts = Some(known.clone());

    let mut ch = connect(opts).expect("TOFU 第一次连接应当成功");
    wait_for(&mut ch, "mock shell ready", Duration::from_secs(10));
    drop(ch);

    let text = std::fs::read_to_string(&known).expect("应当记下了主机密钥");
    assert!(text.contains("127.0.0.1"), "{text}");

    // 同一台主机、策略改成 Strict：这下必须靠账本放行。
    let mut strict = options(addr, password("s3cret"));
    strict.host_key = HostKeyPolicy::Strict;
    strict.known_hosts = Some(known);
    let mut ch = connect(strict).expect("账本里已有这把钥匙，Strict 应当放行");
    wait_for(&mut ch, "mock shell ready", Duration::from_secs(10));

    let _ = std::fs::remove_dir_all(&dir);
}

/// 反向用例：`Strict` + 空账本 → 拒绝，而且报的是「未知主机 + 指纹」，
/// 不是笼统的「连接失败」。这条要穿真握手，因为拒绝发生在 `check_server_key`
/// 里，库把 `false` 变成了什么错误只有跑一遍才知道。
#[test]
fn strict_refuses_a_host_that_is_not_in_the_ledger() {
    let (addr, _observed) = start_server("s3cret", 0);
    let dir = std::env::temp_dir().join(format!("term-ssh-e2e-strict-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("建临时目录");
    let known = dir.join("known_hosts");
    let _ = std::fs::remove_file(&known);

    let mut opts = options(addr, password("s3cret"));
    opts.host_key = HostKeyPolicy::Strict;
    opts.known_hosts = Some(known.clone());

    let e = connect(opts).unwrap_err();
    match e {
        SshError::HostKey(term_ssh::HostKeyProblem::Unknown { fingerprint, .. }) => {
            assert!(fingerprint.starts_with("SHA256:"), "{fingerprint}");
        }
        other => panic!("应当是 HostKey::Unknown，实际是 {other:?}"),
    }
    // 拒绝的时候不许往账本里写东西。
    assert!(!known.exists(), "Strict 拒绝后不该建 known_hosts");

    let _ = std::fs::remove_dir_all(&dir);
}

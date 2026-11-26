//! 真握手用例：进程内起一个真 russh 服务端，把 `ChannelOpener::exec` →
//! `exec_capture` 这条链整个跑一遍。
//!
//! 不 mock 掉 russh，是因为这一刀最值钱的恰恰是「库到底把 exec 请求翻译成什么」：
//! `Channel::exec(want_reply, cmd)` 的回执时机、`extended_data(ext=1)` 落到哪个
//! `ChannelMsg`、`exit_status` 与 EOF 的先后顺序。只有真握手才验得到。
//!
//! 服务端只认密码、只接 exec（不开 PTY / shell），按 [`ExecSpec`] 吐预设输出；
//! 既不需要 `sshd`，也不碰外网（127.0.0.1）。

use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use russh::server::{self, Auth, Msg, Server as _, Session};
use russh::{Channel, ChannelId};
use ssh_key::getrandom::SysRng;
use ssh_key::rand_core::UnwrapErr;
use ssh_key::{Algorithm, LineEnding, PrivateKey as SshPrivKey};
use term_ssh::{
    exec_capture, AuthMethod, ExecOutput, HostKeyPolicy, SshError, SshOptions,
    EXEC_MAX_OUTPUT_BYTES,
};

/// 与 russh 的 `maximum_packet_size` 默认值对齐；大输出按这个粒度喂，
/// 免得一次 `session.data` 把写缓冲撑得过大。
const EXEC_CHUNK: usize = 32 * 1024;

/// 一条 exec 的服务端剧本。
#[derive(Clone, Debug)]
struct ExecSpec {
    /// 认哪个密码。
    password: String,
    /// stdout（`ChannelMsg::Data`）。
    stdout: Vec<u8>,
    /// stderr（`ChannelMsg::ExtendedData`，ext=1）。
    stderr: Vec<u8>,
    exit_code: u32,
    /// 是否发 `exit_status_request`；关掉用来验 `exit_code: None`。
    send_exit_status: bool,
    /// 是否拒绝 exec（`channel_failure`）。
    reject: bool,
    /// 额外再吐这么多 stdout 字节，用来压输出上限。
    flood: usize,
    /// 只发 Eof，不发 Close：验半关闭仍能拿到 ExitStatus。
    half_close: bool,
}

impl ExecSpec {
    fn new() -> Self {
        Self {
            password: "s3cret".to_string(),
            stdout: Vec::new(),
            stderr: Vec::new(),
            exit_code: 0,
            send_exit_status: true,
            reject: false,
            flood: 0,
            half_close: false,
        }
    }
}

/// 服务端记下的事实：客户端到底把什么命令发过来了。
#[derive(Debug, Default)]
struct Observed {
    command: Vec<u8>,
}

#[derive(Clone)]
struct ExecServer {
    spec: ExecSpec,
    observed: Arc<Mutex<Observed>>,
}

impl server::Server for ExecServer {
    type Handler = ExecHandler;

    fn new_client(&mut self, _: Option<SocketAddr>) -> Self::Handler {
        ExecHandler {
            spec: self.spec.clone(),
            observed: Arc::clone(&self.observed),
        }
    }
}

struct ExecHandler {
    spec: ExecSpec,
    observed: Arc<Mutex<Observed>>,
}

impl server::Handler for ExecHandler {
    type Error = russh::Error;

    async fn auth_password(&mut self, _user: &str, password: &str) -> Result<Auth, Self::Error> {
        Ok(if password == self.spec.password {
            Auth::Accept
        } else {
            Auth::reject()
        })
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

    async fn exec_request(
        &mut self,
        channel: ChannelId,
        data: &[u8],
        session: &mut Session,
    ) -> Result<(), Self::Error> {
        if let Ok(mut observed) = self.observed.lock() {
            observed.command = data.to_vec();
        }

        if self.spec.reject {
            // want_reply=true 时服务端**必须**明确回失败；客户端会收到
            // ChannelMsg::Failure，exec_capture 据此分到 SshError::Channel。
            let _ = session.channel_failure(channel);
            session.eof(channel)?;
            session.close(channel)?;
            return Ok(());
        }

        let _ = session.channel_success(channel);

        for chunk in self.spec.stdout.chunks(EXEC_CHUNK) {
            session.data(channel, chunk.to_vec())?;
        }
        if self.spec.flood > 0 {
            let block = vec![b'z'; EXEC_CHUNK];
            let mut left = self.spec.flood;
            while left > 0 {
                let n = left.min(EXEC_CHUNK);
                session.data(channel, block[..n].to_vec())?;
                left -= n;
            }
        }
        for chunk in self.spec.stderr.chunks(EXEC_CHUNK) {
            // ext=1 = SSH_EXTENDED_DATA_STDERR（RFC 4254 §5.2）。
            session.extended_data(channel, 1, chunk.to_vec())?;
        }

        // 真实 sshd 的顺序如此：exit-status 在 eof / close 之前。
        if self.spec.send_exit_status {
            session.exit_status_request(channel, self.spec.exit_code)?;
        }
        session.eof(channel)?;
        if !self.spec.half_close {
            session.close(channel)?;
        }
        Ok(())
    }
}

/// 测试用 ed25519 主机私钥：运行时生成，不入库（与 `e2e.rs` 同一招）。
fn host_key() -> russh::keys::PrivateKey {
    let mut rng = UnwrapErr(SysRng);
    let k = SshPrivKey::random(&mut rng, Algorithm::Ed25519).expect("生成测试私钥");
    russh::keys::PrivateKey::from_openssh(
        k.to_openssh(LineEnding::default())
            .expect("序列化测试主机密钥"),
    )
    .expect("解析测试主机密钥")
}

/// 起一个只监听本机的 exec 服务端，返回地址与共享的命令记录。
fn start_server(spec: ExecSpec) -> (SocketAddr, Arc<Mutex<Observed>>) {
    let addr = std::net::TcpListener::bind(("127.0.0.1", 0))
        .expect("bind 临时端口")
        .local_addr()
        .expect("取本地地址");

    let observed = Arc::new(Mutex::new(Observed::default()));
    let server = ExecServer {
        spec,
        observed: Arc::clone(&observed),
    };

    std::thread::spawn(move || {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("建测试运行时");
        rt.block_on(async move {
            let config = Arc::new(server::Config {
                keys: vec![host_key()],
                ..Default::default()
            });
            let mut srv = server;
            if let Err(e) = srv.run_on_address(config, addr).await {
                panic!("exec mock 服务端退出：{e}");
            }
        });
    });

    // 等它真的 listen，否则第一个 connect 会撞 ECONNREFUSED。
    let deadline = Instant::now() + Duration::from_secs(5);
    while std::net::TcpStream::connect(addr).is_err() {
        assert!(Instant::now() < deadline, "exec mock 服务端 5 秒内没起来");
        std::thread::sleep(Duration::from_millis(10));
    }
    (addr, observed)
}

fn options(addr: SocketAddr, password: &str) -> SshOptions {
    let mut opts = SshOptions::new(
        "127.0.0.1",
        "tester",
        AuthMethod::Password {
            password: password.to_string(),
        },
    );
    opts.port = addr.port();
    opts.host_key = HostKeyPolicy::Insecure;
    opts.connect_timeout = Duration::from_secs(10);
    opts
}

#[test]
fn echo_hello_returns_exact_stdout_and_zero_exit() {
    let mut spec = ExecSpec::new();
    spec.stdout = b"hello\n".to_vec();
    let (addr, observed) = start_server(spec);

    let out: ExecOutput =
        exec_capture(&options(addr, "s3cret"), "echo hello").expect("真握手 exec 应当成功");

    assert_eq!(out.stdout, b"hello\n".to_vec(), "stdout 必须逐字节正确");
    assert!(
        out.stderr.is_empty(),
        "这条命令没有 stderr：{:?}",
        out.stderr
    );
    assert_eq!(out.exit_code, Some(0));
    assert!(!out.truncated);
    let command = observed.lock().expect("读命令记录").command.clone();
    assert_eq!(command, b"echo hello".to_vec(), "服务端应收到原始命令字节");
}

#[test]
fn stdout_and_stderr_are_kept_apart() {
    let mut spec = ExecSpec::new();
    spec.stdout = b"OUT".to_vec();
    spec.stderr = b"ERR".to_vec();
    let (addr, _observed) = start_server(spec);

    let out = exec_capture(&options(addr, "s3cret"), "printf OUT; printf ERR >&2")
        .expect("真握手 exec 应当成功");

    assert_eq!(out.stdout, b"OUT".to_vec());
    assert_eq!(
        out.stderr,
        b"ERR".to_vec(),
        "ext=1 必须落到 stderr 而不是混进 stdout"
    );
    assert_eq!(out.exit_code, Some(0));
    assert!(!out.truncated);
}

#[test]
fn a_nonzero_exit_code_is_preserved() {
    let mut spec = ExecSpec::new();
    spec.exit_code = 7;
    spec.stdout = b"partial work\n".to_vec();
    let (addr, _observed) = start_server(spec);

    let out = exec_capture(&options(addr, "s3cret"), "exit 7")
        .expect("非零退出码不是传输错误，仍应拿到输出");

    assert_eq!(out.exit_code, Some(7));
    assert_eq!(out.stdout, b"partial work\n".to_vec());
    assert!(!out.truncated);
}

#[test]
fn a_command_without_exit_status_leaves_the_code_unset() {
    let mut spec = ExecSpec::new();
    spec.send_exit_status = false;
    spec.stdout = b"no status\n".to_vec();
    let (addr, _observed) = start_server(spec);

    let out = exec_capture(&options(addr, "s3cret"), "true").expect("没报退出码也应当正常收尾");

    assert_eq!(out.stdout, b"no status\n".to_vec());
    assert_eq!(out.exit_code, None, "服务端没报 status 就不能凭空造一个 0");
}

#[test]
fn output_over_the_cap_is_truncated_and_still_captures_the_exit_code() {
    let mut spec = ExecSpec::new();
    // 超过 1 MiB 上限，但仍小于 russh 默认 2 MiB 窗口，避免把流控也卷进来。
    spec.flood = EXEC_MAX_OUTPUT_BYTES + 256 * 1024;
    spec.stdout = b"START".to_vec();
    let (addr, _observed) = start_server(spec);

    let out = exec_capture(&options(addr, "s3cret"), "cat /dev/zero")
        .expect("超上限应当截断后正常返回，而不是 panic / 报错");

    assert!(out.truncated, "输出触顶必须置截断标志");
    assert_eq!(
        out.stdout.len(),
        EXEC_MAX_OUTPUT_BYTES,
        "缓冲区必须停在硬顶上"
    );
    assert_eq!(&out.stdout[..5], b"START", "截断保留的是前缀");
    assert_eq!(
        out.exit_code,
        Some(0),
        "截断后仍要继续 drain 到 EOF，否则 exit_status 会丢"
    );
}

/// 服务端只发 Eof 不发 Close：一收到 Eof 就 break 的实现会丢掉 ExitStatus。
/// 这里断言完整的 stdout + 退出码，并且是有界返回（EOF_GRACE 兜底）。
#[test]
fn a_half_closed_exec_channel_still_yields_the_exit_code() {
    let mut spec = ExecSpec::new();
    spec.half_close = true;
    spec.stdout = b"tail\n".to_vec();
    spec.exit_code = 9;
    let (addr, _observed) = start_server(spec);
    let started = Instant::now();

    let out = exec_capture(&options(addr, "s3cret"), "tail").expect("半关闭应当正常收尾");
    assert!(started.elapsed() < Duration::from_secs(12), "应当有界返回");
    assert_eq!(out.stdout, b"tail\n".to_vec());
    assert_eq!(out.exit_code, Some(9), "Eof 之后不能吞掉 ExitStatus");
}

#[test]
fn utf8_command_bytes_reach_the_server_verbatim() {
    let mut spec = ExecSpec::new();
    spec.stdout = b"ok".to_vec();
    let (addr, observed) = start_server(spec);

    let out =
        exec_capture(&options(addr, "s3cret"), "echo '你好'").expect("非 ASCII 命令应当照常执行");
    assert_eq!(out.stdout, b"ok".to_vec());

    let command = observed.lock().expect("读命令记录").command.clone();
    assert_eq!(
        command,
        "echo '你好'".as_bytes().to_vec(),
        "命令必须按 UTF-8 原始字节上送，不能被改写"
    );
}

#[test]
fn a_wrong_password_is_a_typed_auth_error() {
    let (addr, _observed) = start_server(ExecSpec::new());

    // 返回值类型是 Result<ExecOutput, SshError>：编译期就排除了 String 错误。
    let err: SshError =
        exec_capture(&options(addr, "wrong"), "echo hello").expect_err("密码错必须失败");
    match err {
        SshError::Auth(problem) => {
            assert_eq!(problem.user, "tester");
            assert_eq!(problem.method, "password");
        }
        other => panic!("应当是 Auth 失败，实际是 {other:?}"),
    }
}

#[test]
fn a_server_refusing_exec_is_a_typed_channel_error() {
    let mut spec = ExecSpec::new();
    spec.reject = true;
    let (addr, _observed) = start_server(spec);

    let err: SshError = exec_capture(&options(addr, "s3cret"), "forbidden")
        .expect_err("服务端拒绝 exec 必须变成错误，不能当空输出成功返回");
    match err {
        SshError::Channel(msg) => assert!(msg.contains("拒绝"), "要说清是服务端拒了：{msg}"),
        other => panic!("应当是 Channel 错误，实际是 {other:?}"),
    }
}

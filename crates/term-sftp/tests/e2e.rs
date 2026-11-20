//! 端到端：真起一个 SSH 服务端（russh 的 server 端），客户端连上去请求 `sftp`
//! 子系统，服务端把这条通道接到**真实的 `sftp-server`** 上（OpenSSH 自带那个），
//! 然后列一个真目录。
//!
//! 为什么不用 `russh-sftp` 的 server 端自己实现一遍 `readdir`：那测的是「我们对
//! 协议的理解」，而不是「我们能不能和真的 SFTP 服务端谈」。这一层最容易错的
//! 恰恰是这种地方——版本交换的字段、`realpath` 的返回、`readdir` 的 EOF 判定。
//! 服务端进程是 `/usr/libexec/sftp-server`（macOS）或 `/usr/lib/openssh/sftp-server`
//! （Linux）。
//!
//! 服务端只做四件事：认密码 / 认公钥、接受会话通道、把 `sftp` 子系统接到子进程、
//! 双向搬字节。全程只监听 `127.0.0.1`，不碰外网、不跑 `sshd`。
//!
//! # 缺 `sftp-server` 时：默认 `#[ignore]`，绝不「静默跳过即绿」
//!
//! 旧写法是「`if sftp_server_binary().is_none() { eprintln!("跳过…"); return; }`」。
//! 后果和 ZMODEM 那两条一模一样（见 `crates/term-zmodem/tests/interop.rs` 的头注释，
//! 审查报告把它判成最严重的一档）：libtest 把这条用例算成 **passed**，`cargo test` 全绿，
//! 但一个字节都没验。机器上装了 `sftp-server` 时它真跑，没装时它假装通过了——
//! 「环境缺失」被伪装成「代码正确」，这是最贵的一种假绿。
//!
//! 现在 12 条要真服务端的用例都带 `#[ignore]`：
//!
//! - 默认 `cargo test -p term-sftp --test e2e` → `11 ignored`，libtest 会把 `SKIP`
//!   的理由原样打出来。**看到 `ignored` 就是「没验」**，不是「验过了」。
//! - 发布门必须显式点名 `-- --ignored`；此时缺 `sftp-server` 由
//!   [`require_sftp_server`] 直接 `panic!`（带 `[SFTP-E2E] FAIL` 与复现命令），
//!   门会红。
//! - 真跑通时每条打一行 `[SFTP-E2E] PASS <用例名>`，留痕里数这些行比只看
//!   `ok` 更可核对（`ok` 也可能是 `0 passed`）。
//!
//! 复现（本机有 `sftp-server` 时）：
//!
//! ```text
//! cargo test -p term-sftp --test e2e -- --ignored --test-threads=1 --nocapture
//! ```
//!
//! 第二代服务端（Debian 的 `/usr/lib/openssh/sftp-server`）在容器里，也该跑一遍：
//!
//! ```text
//! bash scripts/rust-in-docker.sh test -p term-sftp --test e2e -- --ignored --test-threads=1
//! ```
//!
//! 两代都过才算「能跟真的 SFTP 服务端谈」，而不是只跟 macOS 那一份兼容。
//! 只有 `closing_twice_is_idempotent_and_never_hangs` 不需要外部依赖（它连的是
//! 一个没人听的端口），所以它默认照跑，不加 `#[ignore]`。

use std::collections::HashMap;
use std::io::{Read, Write};
use std::net::{Shutdown, SocketAddr, TcpStream};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use russh::server::{self, Auth, Msg, Server as _, Session};
use russh::{Channel, ChannelId};
use std::fs::FileTimes;
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use term_ssh::{AuthMethod, HostKeyPolicy, SshOptions};
use term_sftp::{Event, SftpHandle, from_epoch_secs, join};

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

/// 发布留痕里可 grep 的标记。`SKIP` 写在 `#[ignore = "..."]` 的理由里，libtest
/// 默认会把理由连同 `ignored` 一起打印；`RUN` / `PASS` / `FAIL` 由本文件在真跑时
/// 打印。与 `crates/term-zmodem/tests/interop.rs` 的 `[ZMODEM-INTEROP]` 同一套规矩。
const E2E_MARK: &str = "[SFTP-E2E]";

/// 系统自带的 SFTP 服务端。两条常见路径（macOS / Linux 各一条）。
fn sftp_server_binary() -> Option<PathBuf> {
    ["/usr/libexec/sftp-server", "/usr/lib/openssh/sftp-server"]
        .iter()
        .map(PathBuf::from)
        .find(|p| p.is_file())
}

/// 缺 `sftp-server` 时**硬失败**，取代旧实现「打一行跳过然后 return」的假绿。
///
/// 这里对应 ZMODEM 的 `require_prog(prog)`，只是不按名字在 `PATH` 里找：`sftp-server`
/// 在 macOS 上不在 `PATH` 里（`/usr/libexec/sftp-server`），判据必须是「mock 服务端
/// 真能启动它」——所以用同一个 [`sftp_server_binary`]，不另立一套查找规则。
///
/// 12 条用例默认 `#[ignore]`，只有发布门用 `--ignored` 显式点名时才会执行到这里。
/// 此时缺服务端说明这次门根本没验到东西，必须红；panic 文本里带复现命令，便于从
/// CI 日志直接看出缺的是依赖还是代码回归。
fn require_sftp_server() {
    if sftp_server_binary().is_some() {
        eprintln!("{E2E_MARK} RUN 真实 sftp-server 可用");
        return;
    }
    panic!(
        "{E2E_MARK} FAIL 缺少系统 sftp-server：本次 --ignored 端到端门没有可连的服务端。\n\
         装 OpenSSH（macOS 自带 /usr/libexec/sftp-server；Debian/Ubuntu 是 \
         openssh-sftp-server 包，落在 /usr/lib/openssh/sftp-server）后重跑：\n\
         cargo test -p term-sftp --test e2e -- --ignored --test-threads=1 --nocapture"
    );
}

#[derive(Clone)]
struct MockServer {
    password: String,
    root: PathBuf,
    channels: Arc<Mutex<HashMap<ChannelId, Channel<Msg>>>>,
}

impl server::Server for MockServer {
    type Handler = MockHandler;

    fn new_client(&mut self, _: Option<SocketAddr>) -> Self::Handler {
        MockHandler {
            password: self.password.clone(),
            root: self.root.clone(),
            channels: Arc::clone(&self.channels),
        }
    }
}

struct MockHandler {
    password: String,
    root: PathBuf,
    channels: Arc<Mutex<HashMap<ChannelId, Channel<Msg>>>>,
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

    /// 收下会话通道并**先存着**：这时还不知道客户端要 shell 还是子系统。
    async fn channel_open_session(
        &mut self,
        channel: Channel<Msg>,
        reply: server::ChannelOpenHandle,
        _session: &mut Session,
    ) -> Result<(), Self::Error> {
        reply.accept().await;
        if let Ok(mut map) = self.channels.lock() {
            map.insert(channel.id(), channel);
        }
        Ok(())
    }

    /// 客户端请求 `sftp`：把通道接到真的 `sftp-server` 子进程上。
    ///
    /// 用 `current_dir(root)` 而不是 `-d`：子进程的工作目录就是它的「家」，
    /// 于是客户端的 `canonicalize(".")` 拿到的正是我们准备的测试目录。
    async fn subsystem_request(
        &mut self,
        channel_id: ChannelId,
        name: &str,
        session: &mut Session,
    ) -> Result<(), Self::Error> {
        let channel = self
            .channels
            .lock()
            .ok()
            .and_then(|mut m| m.remove(&channel_id));
        let Some(channel) = channel else {
            return Ok(());
        };
        let binary = sftp_server_binary();
        if name != "sftp" || binary.is_none() {
            session.channel_failure(channel_id)?;
            let _ = channel.close().await;
            return Ok(());
        }
        session.channel_success(channel_id)?;

        let binary = binary.expect("上面刚判过");
        let root = self.root.clone();
        tokio::spawn(async move {
            let mut cmd = tokio::process::Command::new(binary);
            cmd.stdin(std::process::Stdio::piped())
                .stdout(std::process::Stdio::piped())
                .kill_on_drop(true)
                .current_dir(root);
            let Ok(mut child) = cmd.spawn() else {
                return;
            };
            let mut child_io = ChildIo {
                stdin: child.stdin.take().expect("子进程 stdin"),
                stdout: child.stdout.take().expect("子进程 stdout"),
            };
            let mut stream = channel.into_stream();
            let _ = tokio::io::copy_bidirectional(&mut stream, &mut child_io).await;
            let _ = child.kill().await;
        });
        Ok(())
    }
}

/// 双向搬字节要的「子进程半边」：读 stdout、写 stdin。
struct ChildIo {
    stdin: tokio::process::ChildStdin,
    stdout: tokio::process::ChildStdout,
}

impl tokio::io::AsyncRead for ChildIo {
    fn poll_read(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        buf: &mut tokio::io::ReadBuf<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        std::pin::Pin::new(&mut self.stdout).poll_read(cx, buf)
    }
}

impl tokio::io::AsyncWrite for ChildIo {
    fn poll_write(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        buf: &[u8],
    ) -> std::task::Poll<std::io::Result<usize>> {
        std::pin::Pin::new(&mut self.stdin).poll_write(cx, buf)
    }

    fn poll_flush(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        std::pin::Pin::new(&mut self.stdin).poll_flush(cx)
    }

    fn poll_shutdown(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        std::pin::Pin::new(&mut self.stdin).poll_shutdown(cx)
    }
}

/// 起一个只监听本机的 mock 服务端，返回它的地址。
fn start_server(password: &str, root: PathBuf) -> SocketAddr {
    // 先借系统分一个空闲端口再放掉（`run_on_address` 自己 bind，拿不到端口）。
    let addr = std::net::TcpListener::bind(("127.0.0.1", 0))
        .expect("bind 临时端口")
        .local_addr()
        .expect("取本地地址");

    let server = MockServer {
        password: password.to_string(),
        root,
        channels: Arc::new(Mutex::new(HashMap::new())),
    };
    std::thread::spawn(move || {
        let rt = tokio::runtime::Builder::new_multi_thread()
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

    // 等服务端真的开始 listen（否则第一个 connect 会撞上 ECONNREFUSED）。
    let deadline = Instant::now() + Duration::from_secs(5);
    while std::net::TcpStream::connect(addr).is_err() {
        assert!(Instant::now() < deadline, "mock 服务端 5 秒内没起来");
        std::thread::sleep(Duration::from_millis(10));
    }
    addr
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
    // 端到端不校验 known_hosts：那三条分支在 `known_hosts` 的单测里已覆盖。
    opts.host_key = HostKeyPolicy::Insecure;
    opts.connect_timeout = Duration::from_secs(10);
    opts
}

/// 造一个测试目录：一个文件、一个子目录、子目录里再一个文件。
fn fixture_root() -> PathBuf {
    let base = std::env::temp_dir().join(format!(
        "term-sftp-e2e-{}-{:?}",
        std::process::id(),
        std::thread::current().id()
    ));
    let _ = std::fs::remove_dir_all(&base);
    std::fs::create_dir_all(base.join("sub")).expect("建测试目录");
    std::fs::write(base.join("hello.txt"), b"hello").expect("写文件");
    std::fs::write(base.join("sub/nested.txt"), b"1234567890").expect("写子目录文件");
    base
}

fn wait_listed_all(handle: &mut SftpHandle, path: &str) -> Vec<term_sftp::Entry> {
    handle.list(path);
    let deadline = Instant::now() + Duration::from_secs(10);
    while Instant::now() < deadline {
        match handle.try_event() {
            Some(Event::Listed { path: p, entries }) if p == path => return entries,
            Some(Event::Failed { path: p, message }) if p == path => {
                panic!("列 {p} 失败：{message}")
            }
            Some(Event::Closed { message }) => panic!("会话断了：{message}"),
            Some(_) => {}
            None => std::thread::sleep(Duration::from_millis(10)),
        }
    }
    panic!("10 秒内没拿到 {path} 的列表");
}

/// 等连接就绪，返回服务端认定的家目录。
fn wait_ready(handle: &mut SftpHandle) -> String {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        assert!(Instant::now() < deadline, "10 秒内没等到 Ready 事件");
        match handle.try_event() {
            Some(Event::Ready { home }) => return home,
            Some(Event::Closed { message }) => panic!("连不上：{message}"),
            Some(_) => {}
            None => std::thread::sleep(Duration::from_millis(10)),
        }
    }
}

/// 等一次传输收尾，返回 `Done` 报的大小。
///
/// 顺带守着两个不变量：进度不能超过总数、完成时的大小与最后一次进度对得上——
/// 这两条错了，界面上就是「进度条走到 130% 然后卡住」。
fn wait_transfer(handle: &mut SftpHandle, path: &str) -> u64 {
    let deadline = Instant::now() + Duration::from_secs(20);
    let mut last: Option<(u64, u64)> = None;
    loop {
        assert!(Instant::now() < deadline, "20 秒内没等到 {path} 的传输结果");
        match handle.try_event() {
            Some(Event::Progress {
                path: p,
                transferred,
                total,
            }) if p == path => {
                assert!(total == 0 || transferred <= total, "进度不该超过总数");
                last = Some((transferred, total));
            }
            Some(Event::Done { path: p, bytes }) if p == path => {
                if let Some((transferred, total)) = last {
                    assert_eq!(transferred, bytes, "最后一次进度应等于落盘后的大小");
                    assert!(total == 0 || total == bytes, "总数应等于最终大小");
                }
                return bytes;
            }
            Some(Event::Failed { path: p, message }) if p == path => {
                panic!("传 {p} 失败：{message}")
            }
            Some(Event::Cancelled { path: p, kept }) if p == path => {
                panic!("传 {p} 被取消了（kept={kept}），这不该发生")
            }
            Some(Event::Closed { message }) => panic!("会话断了：{message}"),
            Some(_) => {}
            None => std::thread::sleep(Duration::from_millis(10)),
        }
    }
}

/// 等一棵**树**的传输收尾，返回（完成的文件数，树根 Done 的总字节）。
///
/// 与 [`wait_transfer`] 的区别：树的进度流横跨多个文件（`path` 各不相同），
/// 不能按单一 path 等待；树根的 `Done.bytes` 是**全部文件的总和**，
/// 不等于最后一个文件的进度值——所以这里单独守「树根 Done 的字节 = 各文件
/// Done 字节之和」这条树专属的不变量。
fn wait_tree(handle: &mut SftpHandle, root: &str) -> (usize, u64) {
    let deadline = Instant::now() + Duration::from_secs(30);
    let mut files = 0usize;
    let mut summed = 0u64;
    loop {
        assert!(Instant::now() < deadline, "30 秒内没等到树 {root} 的传输结果");
        match handle.try_event() {
            Some(Event::Progress {
                transferred, total, ..
            }) => assert!(total == 0 || transferred <= total, "进度不该超过总数"),
            Some(Event::Done { path: p, bytes }) if p != root => {
                files += 1;
                summed += bytes;
            }
            Some(Event::Done { path: p, bytes }) if p == root => {
                assert_eq!(bytes, summed, "树根总字节应等于各文件字节之和");
                return (files, bytes);
            }
            Some(Event::Failed { path: p, message }) => panic!("传树 {p} 失败：{message}"),
            Some(Event::Closed { message }) => panic!("会话断了：{message}"),
            Some(_) => {}
            None => std::thread::sleep(Duration::from_millis(10)),
        }
    }
}

#[test]
#[ignore = "[SFTP-E2E] SKIP 需要系统 sftp-server；发布必跑：cargo test -p term-sftp --test e2e -- --ignored"]
fn listing_a_remote_directory_comes_back_with_names_sizes_and_kinds() {
    require_sftp_server();
    let root = fixture_root();
    // macOS 的临时目录在 /var 下面，而 /var 是指向 /private/var 的符号链接：
    // 服务端 canonicalize 出来的会是后者，所以比较前先自己规范化一次。
    let canonical_root = std::fs::canonicalize(&root).expect("规范化测试目录");

    let addr = start_server("s3cret", root.clone());
    let mut handle = term_sftp::start_in_background(options(addr, "s3cret"), || {});

    // 1) 连上之后必须先报家目录——UI 靠它决定第一屏画哪里。
    let home = {
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            assert!(Instant::now() < deadline, "10 秒内没等到 Ready 事件");
            match handle.try_event() {
                Some(Event::Ready { home }) => break home,
                Some(Event::Closed { message }) => panic!("连不上：{message}"),
                Some(other) => panic!("还没列目录就收到了 {other:?}"),
                None => std::thread::sleep(Duration::from_millis(10)),
            }
        }
    };
    assert_eq!(
        home,
        canonical_root.to_string_lossy(),
        "家目录应该就是服务端进程的工作目录"
    );

    // 2) 列家目录：目录在前、文件在后，大小是真的。
    let entries = wait_listed_all(&mut handle, &home);
    let names: Vec<&str> = entries.iter().map(|e| e.name.as_str()).collect();
    assert_eq!(names, ["sub", "hello.txt"], "目录排前面，且 . 与 .. 被丢掉");
    let sub = entries.iter().find(|e| e.name == "sub").expect("有 sub");
    assert!(sub.is_dir);
    let file = entries.iter().find(|e| e.name == "hello.txt").expect("有 hello.txt");
    assert!(!file.is_dir);
    assert_eq!(file.size, 5, "hello 是 5 个字节");

    // 3) 进子目录：路径拼接用的是 entry 自己的规则。
    let nested = wait_listed_all(&mut handle, &join(&home, "sub"));
    assert_eq!(nested.len(), 1);
    assert_eq!(nested[0].name, "nested.txt");
    assert_eq!(nested[0].size, 10);

    // 4) 不存在的目录是一个**事件**，不是 panic：界面要显示它。
    handle.list("/definitely/not/here");
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        assert!(Instant::now() < deadline, "10 秒内没等到 Failed 事件");
        match handle.try_event() {
            Some(Event::Failed { path, message }) => {
                assert_eq!(path, "/definitely/not/here");
                assert!(!message.is_empty(), "失败要说清原因");
                break;
            }
            Some(Event::Closed { message }) => panic!("会话提前断了：{message}"),
            Some(_) => {}
            None => std::thread::sleep(Duration::from_millis(10)),
        }
    }

    let _ = std::fs::remove_dir_all(&root);
    eprintln!("{E2E_MARK} PASS listing_a_remote_directory_comes_back_with_names_sizes_and_kinds");
}

#[test]
#[ignore = "[SFTP-E2E] SKIP 需要系统 sftp-server；发布必跑：cargo test -p term-sftp --test e2e -- --ignored"]
fn a_file_downloads_uploads_and_resumes_against_the_real_server() {
    require_sftp_server();
    let root = fixture_root();
    let addr = start_server("s3cret", root.clone());
    let mut handle = term_sftp::start_in_background(options(addr, "s3cret"), || {});
    let home = wait_ready(&mut handle);

    // 本地落点放在测试目录**外面**：远端根 = 服务端 cwd，放里面会污染列表断言。
    let local_dir = std::env::temp_dir().join(format!("term-sftp-put-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&local_dir);
    std::fs::create_dir_all(&local_dir).expect("建本地目录");
    let local = local_dir.join("hello.txt");
    let local_s = local.to_string_lossy().to_string();

    // 1) 下载：字节一致，`Done.bytes` 是落盘后的完整大小。
    let remote = join(&home, "hello.txt");
    handle.download(remote.clone(), local_s.clone(), false, false);
    assert_eq!(wait_transfer(&mut handle, &remote), 5);
    assert_eq!(std::fs::read(&local).expect("读回下载的文件"), b"hello");

    // 2) 下载续传：本地只剩前 2 个字节，`resume` 应该接在后面而不是覆盖。
    std::fs::write(&local, b"he").expect("造半截文件");
    handle.download(remote.clone(), local_s.clone(), true, false);
    assert_eq!(wait_transfer(&mut handle, &remote), 5);
    assert_eq!(
        std::fs::read(&local).expect("读回续传的文件"),
        b"hello",
        "续传必须接在已有前缀后面，而不是重写"
    );

    // 3) 上传：远端原来没有这个文件，传完落盘内容一致。
    std::fs::write(&local, b"0123456789").expect("写上传内容");
    let uploaded = join(&home, "uploaded.txt");
    handle.upload(local_s.clone(), uploaded.clone(), false, false);
    assert_eq!(wait_transfer(&mut handle, &uploaded), 10);
    assert_eq!(
        std::fs::read(root.join("uploaded.txt")).expect("读远端落盘的文件"),
        b"0123456789"
    );

    // 4) 上传续传：远端已有 4 字节前缀，只补剩下的 6 个字节。
    std::fs::write(root.join("partial.txt"), b"0123").expect("造远端半截文件");
    let partial = join(&home, "partial.txt");
    handle.upload(local_s.clone(), partial.clone(), true, false);
    assert_eq!(wait_transfer(&mut handle, &partial), 10);
    assert_eq!(
        std::fs::read(root.join("partial.txt")).expect("读远端续传后的文件"),
        b"0123456789",
        "远端前缀不能被丢掉"
    );

    // 5) 远端是目录时 `get` 要报错，且错误里点名是哪个路径。
    let sub = join(&home, "sub");
    handle.download(sub.clone(), local_dir.join("sub").to_string_lossy().to_string(), false, false);
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        assert!(Instant::now() < deadline, "10 秒内没等到 Failed 事件");
        match handle.try_event() {
            Some(Event::Failed { path, message }) => {
                assert_eq!(path, sub);
                assert!(message.contains("目录"), "错误要说清是目录：{message}");
                break;
            }
            Some(Event::Closed { message }) => panic!("会话断了：{message}"),
            Some(_) => {}
            None => std::thread::sleep(Duration::from_millis(10)),
        }
    }

    // 6) 保留时间戳：下载时远端 mtime 要原样落到本地。
    //
    // 远端根就是服务端的 cwd（真实路径 `root`），所以直接在本地设 mtime 就等于
    // 设远端文件的 mtime，不需要第二个会话。
    let remote_mtime: u32 = 1_600_000_000;
    let remote_path = root.join("hello.txt");
    {
        let mut f = std::fs::File::create(&remote_path).expect("重建远端文件");
        f.write_all(b"hello").expect("写");
        f.set_times(FileTimes::new().set_modified(from_epoch_secs(remote_mtime)))
            .expect("设远端 mtime");
    }
    // 不带 --preserve-time：本地 mtime 不该被碰（仍是创建时的此刻）。
    handle.download(remote.clone(), local_s.clone(), false, false);
    assert_eq!(wait_transfer(&mut handle, &remote), 5);
    assert_ne!(
        std::fs::metadata(&local).expect("读本地").mtime() as u32,
        remote_mtime,
        "不保留时间戳就不该改本地 mtime"
    );
    // 带 --preserve-time：本地 mtime 应该等于远端那个已知值。
    handle.download(remote.clone(), local_s.clone(), false, true);
    assert_eq!(wait_transfer(&mut handle, &remote), 5);
    assert_eq!(
        std::fs::metadata(&local).expect("读本地").mtime() as u32,
        remote_mtime,
        "下载保留时间戳要把远端 mtime 写回本地"
    );

    // 7) 保留时间戳：上传时本地 mtime 要原样落到远端。
    let local_mtime: u32 = 1_700_000_000;
    {
        let mut f = std::fs::File::create(&local).expect("重建本地文件");
        f.write_all(b"0123456789").expect("写");
        f.set_times(FileTimes::new().set_modified(from_epoch_secs(local_mtime)))
            .expect("设本地 mtime");
    }
    // 不保留：远端 mtime 不该变成这个已知值。
    handle.upload(local_s.clone(), uploaded.clone(), false, false);
    assert_eq!(wait_transfer(&mut handle, &uploaded), 10);
    assert_ne!(
        std::fs::metadata(root.join("uploaded.txt")).expect("读远端").mtime() as u32,
        local_mtime,
        "不保留时间戳就不该改远端 mtime"
    );
    // 保留：远端 mtime 应该等于本地那个已知值。
    handle.upload(local_s.clone(), uploaded.clone(), false, true);
    assert_eq!(wait_transfer(&mut handle, &uploaded), 10);
    assert_eq!(
        std::fs::metadata(root.join("uploaded.txt")).expect("读远端").mtime() as u32,
        local_mtime,
        "上传保留时间戳要把本地 mtime 写到远端"
    );

    let _ = std::fs::remove_dir_all(&root);
    let _ = std::fs::remove_dir_all(&local_dir);
    eprintln!("{E2E_MARK} PASS a_file_downloads_uploads_and_resumes_against_the_real_server");
}

/// 取消原语的句柄语义：**空闲时**点取消不许毒到下一条传输。
///
/// 这条守的是「清位紧跟传输开始」这个位置。写错会在两种症状里露头：
///
/// - 压根不清位 → 空闲时点的那一下，把下一条传输在第一个分片就打掉
///   （收到 `Cancelled` 而不是 `Done`）；
/// - 清位放在收请求**之前** → 不算错，但会让「点了取消、下一条已经在排队」
///   的那一下凭空消失。这是**刻意的取舍**（见 `SftpHandle::cancel` 文档），
///   不在这里钉。
///
/// 至于「传到一半按取消」，这里不测：本机 sftp-server 传 5 个字节是微秒级，
/// 想在第一个分片与收尾之间插进去只能靠运气，那种测试红起来没人敢信。
/// 三态处置的**判定**已在 `cancel::disposition` 与 `transfer::discard_partial`
/// 的单测里逐行钉住；真正的「跑到一半取消」要等 v0.2 的可注入通道
/// （那时能塞一个刻意慢的读写器，不靠时序赌）。
#[test]
#[ignore = "[SFTP-E2E] SKIP 需要系统 sftp-server；发布必跑：cargo test -p term-sftp --test e2e -- --ignored"]
fn cancelling_while_idle_does_not_poison_the_next_transfer() {
    require_sftp_server();
    let root = fixture_root();
    let addr = start_server("s3cret", root.clone());
    let mut handle = term_sftp::start_in_background(options(addr, "s3cret"), || {});
    let home = wait_ready(&mut handle);

    let local_dir = std::env::temp_dir().join(format!("term-sftp-cancel-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&local_dir);
    std::fs::create_dir_all(&local_dir).expect("建本地目录");
    let local = local_dir.join("after-cancel.txt");
    let local_s = local.to_string_lossy().to_string();
    let remote = join(&home, "hello.txt");

    // 一条传输都没在跑时点取消：不该改变任何东西。
    handle.cancel();
    handle.download(remote.clone(), local_s, false, false);
    assert_eq!(
        wait_transfer(&mut handle, &remote),
        5,
        "空闲时点的取消不能顺延到下一条传输"
    );
    assert_eq!(std::fs::read(&local).expect("读回下载的文件"), b"hello");

    let _ = std::fs::remove_dir_all(&root);
    let _ = std::fs::remove_dir_all(&local_dir);
    eprintln!("{E2E_MARK} PASS cancelling_while_idle_does_not_poison_the_next_transfer");
}

#[test]
#[ignore = "[SFTP-E2E] SKIP 需要系统 sftp-server；发布必跑：cargo test -p term-sftp --test e2e -- --ignored"]
fn a_tree_downloads_and_uploads_against_the_real_server() {
    require_sftp_server();
    let root = fixture_root();
    let addr = start_server("s3cret", root.clone());
    let mut handle = term_sftp::start_in_background(options(addr, "s3cret"), || {});
    let home = wait_ready(&mut handle);

    // 远端树：两层子目录 + 一个空目录 + 一个符号链接（链接必须被跳过——
    // 跟着它走，`link -> a.txt` 还算无害，但 `loop -> .` 能把遍历转成死循环）。
    let tree = root.join("logs");
    std::fs::create_dir_all(tree.join("sub/deep")).expect("建远端子目录");
    std::fs::create_dir_all(tree.join("empty")).expect("建远端空目录");
    std::fs::write(tree.join("a.txt"), b"hello").expect("写 a");
    std::fs::write(tree.join("sub/b.txt"), b"1234567890").expect("写 b");
    std::fs::write(tree.join("sub/deep/c.txt"), b"xyz").expect("写 c");
    std::os::unix::fs::symlink("a.txt", tree.join("link")).expect("建符号链接");
    let logs = join(&home, "logs");

    // 1) 递归下载：3 个文件（链接不算）、总字节 5+10+3、结构与内容一致。
    let out_dir = std::env::temp_dir().join(format!("term-sftp-tree-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&out_dir);
    let out_s = out_dir.to_string_lossy().to_string();
    handle.download_tree(logs.clone(), out_s.clone(), false, false);
    let (files, bytes) = wait_tree(&mut handle, &logs);
    assert_eq!(files, 3, "符号链接不算文件：a/b/c 三个");
    assert_eq!(bytes, 18);
    assert_eq!(std::fs::read(out_dir.join("a.txt")).unwrap(), b"hello");
    assert_eq!(std::fs::read(out_dir.join("sub/b.txt")).unwrap(), b"1234567890");
    assert_eq!(std::fs::read(out_dir.join("sub/deep/c.txt")).unwrap(), b"xyz");
    assert!(out_dir.join("empty").is_dir(), "空目录也要落地");
    assert!(
        !out_dir.join("link").exists(),
        "符号链接不该被当成文件传下来"
    );

    // 2) 递归上传：把刚下载的树原样传回远端另一个位置，远端要长出同一棵树
    //    （含空目录），链接依旧缺席。
    let uploaded = join(&home, "logs-back");
    handle.upload_tree(out_s.clone(), uploaded.clone(), false, false);
    let (files, bytes) = wait_tree(&mut handle, &uploaded);
    assert_eq!(files, 3);
    assert_eq!(bytes, 18);
    assert_eq!(std::fs::read(root.join("logs-back/a.txt")).unwrap(), b"hello");
    assert_eq!(
        std::fs::read(root.join("logs-back/sub/deep/c.txt")).unwrap(),
        b"xyz"
    );
    assert!(root.join("logs-back/empty").is_dir(), "空目录要在远端建出来");
    assert!(!root.join("logs-back/link").exists());

    // 3) `-r` 撞上单文件：明确失败，提示去掉 -r，而不是把文件当空目录传。
    let single = join(&home, "hello.txt");
    handle.download_tree(single.clone(), out_s, false, false);
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        assert!(Instant::now() < deadline, "10 秒内没等到 Failed 事件");
        match handle.try_event() {
            Some(Event::Failed { path, message }) => {
                assert_eq!(path, single);
                assert!(message.contains("目录"), "错误要说清不是目录：{message}");
                break;
            }
            Some(Event::Closed { message }) => panic!("会话断了：{message}"),
            Some(_) => {}
            None => std::thread::sleep(Duration::from_millis(10)),
        }
    }

    let _ = std::fs::remove_dir_all(&root);
    let _ = std::fs::remove_dir_all(&out_dir);
    eprintln!("{E2E_MARK} PASS a_tree_downloads_and_uploads_against_the_real_server");
}

/// 采一份**远端**清单：XFER-06 目录同步的远端侧（`scan_remote_entries`）。
///
/// 只看采集这一层——路径口径、类型、大小、mtime、软链跳过、根不对时报错。
/// 这里自建一条连接直接调那个 async 函数，**不经 `Request::ScanTree` 通道**：
/// 通道那一侧（`Request::ScanTree` / `Event::Scanned`）由 `apps/desktop` 的面板
/// 驱动，这条判据要的是「采集本身对不对」，多绕一层只会把失败原因搅浑。
///
/// 本地那一侧的同名判据在 `crates/term-sftp/src/walk.rs` 的单测里
/// （`scanning_a_local_tree_carries_relative_paths_sizes_and_mtimes`）。两条断言
/// 的**顺序列表必须一样**——那是「同步看到的树」与「本地看到的树」同构的唯一守卫。
#[test]
#[ignore = "[SFTP-E2E] SKIP 需要系统 sftp-server；发布必跑：cargo test -p term-sftp --test e2e -- --ignored"]
fn scanning_a_remote_tree_carries_relative_paths_sizes_and_mtimes() {
    require_sftp_server();
    let root = fixture_root();
    // 软链必须被跳过：跟着 `a -> .` 走能把遍历变成死循环，而同步里的「扫盘」
    // 一旦死循环，表现是界面一直转圈且取消不掉。
    std::os::unix::fs::symlink(root.join("hello.txt"), root.join("link.txt")).expect("建软链");
    let addr = start_server("s3cret", root.clone());
    let opts = options(addr, "s3cret");

    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("建运行时");
    let (scanned, file_as_root) = rt.block_on(async move {
        let auth = term_ssh::Authenticated::connect(&opts).await.expect("连上");
        let channel = auth
            .opener()
            .subsystem("sftp")
            .await
            .expect("请求 sftp 子系统");
        let sftp = russh_sftp::client::SftpSession::new(channel.into_stream())
            .await
            .expect("SFTP 版本握手");
        // macOS 的临时目录在 /var 下，而 /var 是 /private/var 的软链；服务端
        // canonicalize 出来的是后者，直接拿本地路径去比会全错。
        let home = sftp.canonicalize(".").await.expect("问家目录");
        let scanned = term_sftp::scan_remote_entries(&sftp, &home)
            .await
            .expect("扫远端目录");
        // 根是文件时必须报错：空清单会被同步读成「远端什么都没有」，
        // 接着把本地整棵树推上去——比报错贵得多。
        let file_as_root = term_sftp::scan_remote_entries(&sftp, &join(&home, "hello.txt"))
            .await
            .err()
            .map(|e| e.to_string());
        let _ = sftp.close().await;
        auth.disconnect().await;
        (scanned, file_as_root)
    });

    let paths: Vec<&str> = scanned.iter().map(|e| e.rel.as_str()).collect();
    assert_eq!(
        paths,
        ["hello.txt", "sub", "sub/nested.txt"],
        "顺序必须与本地扫盘逐条一致（目录内排序后逆序入栈的深度优先）；\
         link.txt 不该出现"
    );

    let sub = scanned.iter().find(|e| e.rel == "sub").expect("有 sub");
    assert!(sub.is_dir);
    assert_eq!(sub.size, 0, "目录的大小统一归零，服务端爱报 4096 也不管");
    assert!(
        sub.mtime.is_some_and(|t| t > 1_600_000_000),
        "目录也要带 mtime，否则同步判不出结构变化：{:?}",
        sub.mtime
    );

    let nested = scanned
        .iter()
        .find(|e| e.rel == "sub/nested.txt")
        .expect("有 sub/nested.txt");
    assert!(!nested.is_dir);
    assert_eq!(nested.size, 10);
    assert!(nested.mtime.is_some_and(|t| t > 1_600_000_000), "秒级 Unix 时间戳");

    let err = file_as_root.expect("根是文件时必须是错误，不能是空清单");
    assert!(err.contains("不是目录"), "错误要说清根不是目录：{err}");
    assert!(err.contains("hello.txt"), "错误要带上是哪个路径：{err}");

    let _ = std::fs::remove_dir_all(&root);
    eprintln!("{E2E_MARK} PASS scanning_a_remote_tree_carries_relative_paths_sizes_and_mtimes");
}

/// XFER-06 的收敛判据：真服务端上跑完一趟，第二趟必须全 Skip。
///
/// 这是设计稿 §7「首次全上传 / 二次全 Skip」原本拆成三段的那条合成判据。
/// 三段的单测各自都在，但「`preserve` 真的把 mtime 写到了对端、而采集真的把
/// 那个 mtime 读回来了」只有连起来才验得到——中间任何一处把秒截断、漏掉
/// setstat、或者把目录当成文件传，第二趟都会重新判成 Upload。
#[test]
#[ignore = "[SFTP-E2E] SKIP 需要系统 sftp-server；发布必跑：cargo test -p term-sftp --test e2e -- --ignored"]
fn a_full_sync_converges_to_all_skips_on_the_real_server() {
    require_sftp_server();
    use term_xfer::sync::{DeletePolicy, StepResult, SyncOp, SyncRun};

    let base = std::env::temp_dir().join(format!(
        "term-sftp-sync-{}-{:?}",
        std::process::id(),
        std::thread::current().id()
    ));
    let _ = std::fs::remove_dir_all(&base);
    let local_root = base.join("local");
    let remote_root = base.join("remote");
    std::fs::create_dir_all(local_root.join("sub/empty")).expect("建本地树");
    std::fs::create_dir_all(&remote_root).expect("建空的远端根");
    std::fs::write(local_root.join("hello.txt"), b"hello").expect("写文件");
    std::fs::write(local_root.join("sub/nested.txt"), b"1234567890").expect("写文件");

    // mtime 必须钉在过去。若用「刚创建」的此刻，preserve 失效时远端也是「现在」，
    // 两趟很可能撞在同一秒里——判据会假绿。设成 2020 年之后，只有真的把 mtime
    // 写过去，第二趟才对得上。
    let fixed = from_epoch_secs(1_600_000_000);
    for rel in ["hello.txt", "sub/nested.txt"] {
        std::fs::File::options()
            .write(true)
            .open(local_root.join(rel))
            .expect("开文件")
            .set_times(FileTimes::new().set_modified(fixed))
            .expect("设 mtime");
    }

    let addr = start_server("s3cret", remote_root.clone());
    let mut handle = term_sftp::start_in_background(options(addr, "s3cret"), || {});
    let home = wait_ready(&mut handle);

    // ① 首趟采集：本地一棵树（含一个空目录），远端什么都没有。
    let before_local = term_sftp::scan_local_entries(&local_root.to_string_lossy())
        .expect("扫本地树");
    let before_remote = wait_scanned(&mut handle, &home);
    assert!(
        before_remote.is_empty(),
        "远端起始必须是空目录：{before_remote:?}"
    );

    let first = term_xfer::sync::plan(
        &as_sync(&before_local),
        &as_sync(&before_remote),
        DeletePolicy::Keep,
    );
    assert_eq!(first.len(), before_local.len(), "每一项都要补到远端");
    assert!(
        first.iter().all(|i| i.op == SyncOp::Upload),
        "空远端的首趟必须全 Upload：{first:?}"
    );

    // ② 按 `execution_order` 执行。目录只建不传；文件一律
    // `preserve = true`（写 mtime）/ `resume = false`（整份重传）——
    // 与 UI 执行器 `pump_exec` 同一套参数，不是为测试另开一条路。
    let mut run = SyncRun::new(first);
    while let Some(item) = run.step().cloned() {
        let remote = join(&home, &item.entry.path);
        if item.entry.is_dir {
            handle.mkdir(remote.clone());
            assert_eq!(wait_done(&mut handle, &remote), 0, "建目录不报字节");
        } else {
            let local = local_root.join(&item.entry.path);
            handle.upload(
                local.to_string_lossy().to_string(),
                remote.clone(),
                false,
                true,
            );
            assert_eq!(
                wait_transfer(&mut handle, &remote),
                item.entry.size,
                "{} 传完的字节要等于本地大小",
                item.entry.path
            );
        }
        run.record(StepResult::Done);
    }

    // ③ 第二趟采集：两份清单逐条等价。目录不比 mtime——`sync::compare` 的
    // 既定口径是「目录的存在性由子项体现」，建目录那一刻不可能与本地同秒。
    let after_local = term_sftp::scan_local_entries(&local_root.to_string_lossy())
        .expect("再扫本地树");
    let after_remote = wait_scanned(&mut handle, &home);
    assert_eq!(after_local.len(), after_remote.len(), "两端条目数要一致");
    for (l, r) in after_local.iter().zip(&after_remote) {
        assert_eq!(l.rel, r.rel, "顺序口径要一致");
        assert_eq!(l.is_dir, r.is_dir, "{} 的类型要一致", l.rel);
        if !l.is_dir {
            assert_eq!(l.size, r.size, "{} 的大小要一致", l.rel);
            assert_eq!(
                l.mtime, r.mtime,
                "{} 的 mtime 必须被 preserve 写过去、再被采集读回来",
                l.rel
            );
        }
    }

    // ④ 把真采回来的等价清单喂决策层：全 Skip。
    let second = term_xfer::sync::plan(
        &as_sync(&after_local),
        &as_sync(&after_remote),
        DeletePolicy::Keep,
    );
    assert!(
        second.iter().all(|i| i.op == SyncOp::Skip),
        "第二趟必须收敛到全 Skip（否则 preserve 或采集有一环掉了）：{second:?}"
    );

    let _ = std::fs::remove_dir_all(&base);
    eprintln!("{E2E_MARK} PASS a_full_sync_converges_to_all_skips_on_the_real_server");
}

/// 读一个真实文件 / 目录的权限位（只取 `0o7777`，去掉类型位）。
///
/// 远端根就是服务端的 cwd，也就是本地那个目录，所以直接 `std::fs` 读它 ——
/// 与「读回远端落盘内容」的既有做法一致，不必再连一次只为了 stat。
fn mode_of(path: &std::path::Path) -> u32 {
    std::fs::metadata(path)
        .unwrap_or_else(|e| panic!("读 {} 的属性：{e}", path.display()))
        .permissions()
        .mode()
        & 0o7777
}

/// 新建落 `DEFAULT_FILE_MODE` / `DEFAULT_DIR_MODE`，**覆盖一律不动权限**。
///
/// 三条都在真服务端上跑：`create` 的 mode 会被 umask 再削一遍（umask 022 时
/// 0644 保得住，077 时会成 0600），所以「新建是 0644」这一条真正验的是传完之后
/// 那次 `setstat`，而不是 open 的 attrs。第二条守的是安全边界：把对端一个 0600
/// 的文件放宽成 0644 是数据事故，不是保真度。
#[test]
#[ignore = "[SFTP-E2E] SKIP 需要系统 sftp-server；发布必跑：cargo test -p term-sftp --test e2e -- --ignored"]
fn new_files_and_folders_take_the_default_modes_and_overwrites_keep_theirs() {
    require_sftp_server();
    let root = fixture_root();
    let addr = start_server("s3cret", root.clone());
    let mut handle = term_sftp::start_in_background(options(addr, "s3cret"), || {});
    let home = wait_ready(&mut handle);

    // 本地文件**故意设成 0600**：如果实现是「把本地 mode 照搬过去」，远端会变成
    // 0600；只有真的用 `DEFAULT_FILE_MODE` 才会是 0644。一条断言同时钉住两件事。
    let local_dir = std::env::temp_dir().join(format!("term-sftp-mode-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&local_dir);
    std::fs::create_dir_all(&local_dir).expect("建本地目录");
    let local = local_dir.join("payload.txt");
    std::fs::write(&local, b"payload-123").expect("写本地文件");
    std::fs::set_permissions(&local, std::fs::Permissions::from_mode(0o600)).expect("chmod 600");
    let local_s = local.to_string_lossy().to_string();

    // ① 新建文件 → 0644。
    let fresh = join(&home, "fresh.txt");
    handle.upload(local_s.clone(), fresh.clone(), false, false);
    assert_eq!(wait_transfer(&mut handle, &fresh), 11);
    assert_eq!(
        mode_of(&root.join("fresh.txt")),
        0o644,
        "新建文件要落 DEFAULT_FILE_MODE，且不受本地 0600 与服务端 umask 影响"
    );

    // ② 覆盖一个原本 0600 的文件 → 仍是 0600。
    let secret_local = root.join("secret.txt");
    std::fs::write(&secret_local, b"old").expect("造对端文件");
    std::fs::set_permissions(&secret_local, std::fs::Permissions::from_mode(0o600))
        .expect("chmod 600");
    let secret = join(&home, "secret.txt");
    handle.upload(local_s.clone(), secret.clone(), false, false);
    assert_eq!(wait_transfer(&mut handle, &secret), 11);
    assert_eq!(
        mode_of(&secret_local),
        0o600,
        "覆盖已有文件不许动权限——把对端的 0600 放宽成 0644 是数据事故"
    );

    // ③ 新建目录 → 0755。
    let made = join(&home, "made");
    handle.mkdir(made.clone());
    assert_eq!(wait_done(&mut handle, &made), 0, "建目录不报字节");
    assert_eq!(
        mode_of(&root.join("made")),
        0o755,
        "新建目录要落 DEFAULT_DIR_MODE"
    );

    // ④ 已经存在的目录再建一次会失败，那条路径不该改它的权限。
    std::fs::set_permissions(root.join("made"), std::fs::Permissions::from_mode(0o700))
        .expect("chmod 700");
    handle.mkdir(made.clone());
    let _ = wait_failed(&mut handle, &made);
    assert_eq!(
        mode_of(&root.join("made")),
        0o700,
        "建目录失败时不许顺手改掉既有目录的权限"
    );

    let _ = std::fs::remove_dir_all(&root);
    let _ = std::fs::remove_dir_all(&local_dir);
    eprintln!(
        "{E2E_MARK} PASS new_files_and_folders_take_the_default_modes_and_overwrites_keep_theirs"
    );
}

#[test]
#[ignore = "[SFTP-E2E] SKIP 需要系统 sftp-server；发布必跑：cargo test -p term-sftp --test e2e -- --ignored"]
fn a_wrong_password_ends_the_session_instead_of_hanging() {
    require_sftp_server();
    let root = fixture_root();
    let addr = start_server("s3cret", root.clone());
    let mut handle = term_sftp::start_in_background(options(addr, "wrong"), || {});

    let deadline = Instant::now() + Duration::from_secs(15);
    loop {
        assert!(Instant::now() < deadline, "错误密码应该在 15 秒内以 Closed 收场");
        match handle.try_event() {
            Some(Event::Closed { message }) => {
                assert!(!message.is_empty());
                break;
            }
            Some(Event::Ready { .. }) => panic!("错的密码不该连上"),
            Some(_) => {}
            None => std::thread::sleep(Duration::from_millis(10)),
        }
    }
    let _ = std::fs::remove_dir_all(&root);
    eprintln!("{E2E_MARK} PASS a_wrong_password_ends_the_session_instead_of_hanging");
}

// ── 写操作（mkdir / rename / remove / chmod）────────────────────────────────

/// 等某条路径的写操作收尾，返回 `Done` 报的 `bytes`（写操作一律 0）。
fn wait_done(handle: &mut SftpHandle, path: &str) -> u64 {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        assert!(Instant::now() < deadline, "10 秒内没等到 {path} 的完成事件");
        match handle.try_event() {
            Some(Event::Done { path: p, bytes }) if p == path => return bytes,
            Some(Event::Failed { path: p, message }) if p == path => {
                panic!("{path} 操作失败：{message}")
            }
            Some(Event::Closed { message }) => panic!("会话断了：{message}"),
            Some(_) => {}
            None => std::thread::sleep(Duration::from_millis(10)),
        }
    }
}

/// 等某条路径的失败事件，返回服务端的那句话。
fn wait_failed(handle: &mut SftpHandle, path: &str) -> String {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        assert!(Instant::now() < deadline, "10 秒内没等到 {path} 的失败事件");
        match handle.try_event() {
            Some(Event::Failed { path: p, message }) if p == path => return message,
            Some(Event::Done { path: p, .. }) if p == path => {
                panic!("{path} 本该失败，却报了成功")
            }
            Some(Event::Closed { message }) => panic!("会话断了：{message}"),
            Some(_) => {}
            None => std::thread::sleep(Duration::from_millis(10)),
        }
    }
}

/// 等一棵远端树的递归清单（`Request::ScanTree` / `Event::Scanned` 那条通道）。
///
/// 与 `wait_listed_all` 的区别：这里拿的是 `ScannedEntry`（带 mtime、相对路径），
/// 是目录同步的输入，不是浏览用的列表。生产界面走的也是这条通道。
fn wait_scanned(handle: &mut SftpHandle, path: &str) -> Vec<term_sftp::ScannedEntry> {
    handle.scan_tree(path);
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        assert!(Instant::now() < deadline, "10 秒内没等到 {path} 的树清单");
        match handle.try_event() {
            Some(Event::Scanned { path: p, entries }) if p == path => return entries,
            Some(Event::Failed { path: p, message }) if p == path => {
                panic!("扫 {p} 失败：{message}")
            }
            Some(Event::Closed { message }) => panic!("会话断了：{message}"),
            Some(_) => {}
            None => std::thread::sleep(Duration::from_millis(10)),
        }
    }
}

/// 采集层的清单 → 决策层的条目。
///
/// **刻意不复用 `apps/desktop::sync_job::build_plan`**：它在 bin-only 的 `term-desk`
/// 里，这个测试 crate 拿不到；在这里抄一遍 `effective_mtime` 更糟——判据会与
/// 生产分叉，映射写错了照样绿。本用例的前置条件是「真 sftp-server 两端都报得出
/// mtime」，此时那层口径映射退化为恒等，用 `expect` 把前置条件钉死。
/// `mtime` 可空的映射由 `sync_job` 的六条单测守，不在这条真机判据的射程内。
fn as_sync(entries: &[term_sftp::ScannedEntry]) -> Vec<term_xfer::sync::SyncEntry> {
    use term_xfer::sync::SyncEntry;
    entries
        .iter()
        .map(|e| {
            let mtime = e.mtime.unwrap_or_else(|| {
                panic!("真 sftp-server 必须报 mtime：{} 没报，本用例的前提不成立", e.rel)
            });
            if e.is_dir {
                SyncEntry::dir(e.rel.as_str(), mtime)
            } else {
                SyncEntry::file(e.rel.as_str(), e.size, mtime)
            }
        })
        .collect()
}

#[test]
#[ignore = "[SFTP-E2E] SKIP 需要系统 sftp-server；发布必跑：cargo test -p term-sftp --test e2e -- --ignored"]
fn the_four_write_operations_work_against_the_real_server() {
    require_sftp_server();
    let root = fixture_root();
    let addr = start_server("s3cret", root.clone());
    let mut handle = term_sftp::start_in_background(options(addr, "s3cret"), || {});
    let home = wait_ready(&mut handle);

    // 1) mkdir：真的在磁盘上出现，而且列得出来。
    let fresh = join(&home, "created");
    handle.mkdir(&fresh);
    assert_eq!(wait_done(&mut handle, &fresh), 0, "写操作不报字节数");
    assert!(root.join("created").is_dir(), "服务端目录没建出来");
    let names: Vec<String> = wait_listed_all(&mut handle, &home)
        .into_iter()
        .map(|e| e.name)
        .collect();
    assert!(names.contains(&"created".to_string()), "列表里应有新目录：{names:?}");

    // 2) rename：源没了、目标在，内容跟着走。
    let from = join(&home, "hello.txt");
    let to = join(&home, "renamed.txt");
    handle.rename(&from, &to);
    wait_done(&mut handle, &from);
    assert!(!root.join("hello.txt").exists(), "改名前的老名字还在");
    assert_eq!(
        std::fs::read_to_string(root.join("renamed.txt")).expect("改名后的文件"),
        "hello",
        "改名不该动内容"
    );

    // 3) chmod：600，且文件类型位没被抹掉（不是 0o000600 那种残废 mode）。
    let target = join(&home, "renamed.txt");
    handle.chmod(&target, 0o600);
    wait_done(&mut handle, &target);
    let mode = std::fs::metadata(root.join("renamed.txt"))
        .expect("stat")
        .mode()
        & 0o7777;
    assert_eq!(mode, 0o600, "权限应变成 600，实际 {mode:o}");

    // 4) remove_file：真删掉了。
    handle.remove_file(&target);
    wait_done(&mut handle, &target);
    assert!(!root.join("renamed.txt").exists(), "文件没被删掉");

    // 5) remove_dir：空目录可以删；非空目录服务端会拒绝——这里如实报失败。
    handle.remove_dir(&fresh);
    wait_done(&mut handle, &fresh);
    assert!(!root.join("created").exists());

    let non_empty = join(&home, "sub");
    handle.remove_dir(&non_empty);
    let message = wait_failed(&mut handle, &non_empty);
    assert!(
        !message.is_empty(),
        "非空目录删除失败必须带服务端的原话"
    );
    assert!(root.join("sub").is_dir(), "失败的删除不该真的动到目录");

    let _ = std::fs::remove_dir_all(&root);
    eprintln!("{E2E_MARK} PASS the_four_write_operations_work_against_the_real_server");
}

/// `blocking::Client` 的 put / get **方向**判据。
///
/// 这条用例的由来是实打实踩过的 bug：`put(local, remote)` 内部调
/// `transfer(Direction::Upload, remote, local)`，而 `transfer` 的形参顺序是
/// 「先远端、后本地」，于是 `put` 把两个参数原样搬过去就换了位——`upload`
/// 收到的「本地」其实是远端路径，第一句 `metadata()` 就报「读不到本地文件」。
///
/// 为什么只能放在端到端：参数错位的现象是「本地文件不存在」而不是编译错，
/// 也没有纯函数可以断言；只有真服务端跑一遍、再按**磁盘上的真实文件**验内容
/// 才抓得住。上传后查服务端磁盘，取回来再逐字节比对，任何换位都会在这里红。
#[test]
#[ignore = "[SFTP-E2E] SKIP 需要系统 sftp-server；发布必跑：cargo test -p term-sftp --test e2e -- --ignored"]
fn the_blocking_client_puts_and_gets_without_swapping_the_two_ends() {
    require_sftp_server();
    let root = fixture_root();
    let addr = start_server("s3cret", root.clone());
    let client =
        term_sftp::blocking::Client::connect(options(addr, "s3cret")).expect("阻塞客户端连不上");

    let payload = b"blocking-put-direction-0123456789".to_vec();
    let scratch = std::env::temp_dir().join(format!(
        "term-sftp-blocking-{}-{:?}",
        std::process::id(),
        std::thread::current().id()
    ));
    let _ = std::fs::remove_dir_all(&scratch);
    std::fs::create_dir_all(&scratch).expect("建本地临时目录");
    let src = scratch.join("payload.bin");
    std::fs::write(&src, &payload).expect("写本地载荷");
    let src = src.to_str().expect("本地路径不是 UTF-8").to_string();

    // `home` 在 mock 服务端里就是 root 的绝对路径，远端路径按它拼。
    let remote = format!("{}/blocking-put.txt", client.home().trim_end_matches('/'));
    let sent = client.put(&src, &remote).expect("put 把本地与远端传反了");
    assert_eq!(sent, payload.len() as u64, "put 应报出传了多少字节");
    assert_eq!(
        std::fs::read(root.join("blocking-put.txt")).expect("远端文件没落在磁盘上"),
        payload,
        "传上去的字节应与本地一致"
    );

    let back = scratch.join("back.bin");
    let back = back.to_str().expect("本地路径不是 UTF-8").to_string();
    let got = client.get(&remote, &back).expect("get 失败");
    assert_eq!(got, payload.len() as u64, "get 应报出取回多少字节");
    assert_eq!(
        std::fs::read(&back).expect("取回的文件读不到"),
        payload,
        "取回内容应与原文件一致"
    );

    let _ = std::fs::remove_dir_all(&scratch);
    let _ = std::fs::remove_dir_all(&root);
    eprintln!(
        "{E2E_MARK} PASS the_blocking_client_puts_and_gets_without_swapping_the_two_ends"
    );
}

#[test]
#[ignore = "[SFTP-E2E] SKIP 需要系统 sftp-server；发布必跑：cargo test -p term-sftp --test e2e -- --ignored"]
fn a_write_request_with_bad_arguments_fails_locally_without_a_round_trip() {
    require_sftp_server();
    let root = fixture_root();
    let addr = start_server("s3cret", root.clone());
    let mut handle = term_sftp::start_in_background(options(addr, "s3cret"), || {});
    let _home = wait_ready(&mut handle);

    // 空路径：必须在本地就被挡住。`rmdir("")` 在服务端等于删当前目录。
    handle.remove_dir("");
    let message = wait_failed(&mut handle, "");
    assert!(message.contains("空串"), "应指出路径为空：{message}");
    assert!(root.is_dir(), "测试目录不该被动到");

    // 越界的权限模式
    let path = join(&_home, "hello.txt");
    handle.chmod(&path, 0o77777);
    let message = wait_failed(&mut handle, &path);
    assert!(message.contains("权限模式"), "应指出模式越界：{message}");

    let _ = std::fs::remove_dir_all(&root);
    eprintln!("{E2E_MARK} PASS a_write_request_with_bad_arguments_fails_locally_without_a_round_trip");
}

// ---------------------------------------------------------------------------
// 收尾：显式 `close()` 而不是「丢句柄」
//
// 泄漏的形状：每次打开文件面板建一条已认证的 SSH 会话，关面板时只把句柄丢掉。
// 后台线程会关 SFTP 通道，但**从不发 `SSH_MSG_DISCONNECT`**，服务端因此把这条
// 会话一直记着，直到自己的超时或者 `MaxSessions` 顶满——用户看到的现象是
// 「反复开关几次文件面板之后连不上了」。
// ---------------------------------------------------------------------------

/// 在 mock 服务端前面架一层只搬字节的 TCP 代理，返回「代理地址」与
/// 「客户端那条连接已经被对端关掉」的观察位。
///
/// 为什么要多这一层：russh 0.65 的 server 端 `Handler` **没有**「对端挂断了」
/// 这类回调（没有 `disconnected`），所以「close() 之后那条连接到底还在不在」
/// 只能从 TCP 层看。代理读到客户端方向的 EOF 就置位——这是「收尾真的到了
/// 服务端」唯一能从外部观察到的证据，也正好对应泄漏的症状：会话挂在那儿不收。
fn proxy_to(upstream: SocketAddr) -> (SocketAddr, Arc<AtomicBool>) {
    let listener = std::net::TcpListener::bind(("127.0.0.1", 0)).expect("bind 代理端口");
    let addr = listener.local_addr().expect("取代理地址");
    let client_gone = Arc::new(AtomicBool::new(false));
    let flag = Arc::clone(&client_gone);
    std::thread::spawn(move || {
        let Ok((client, _)) = listener.accept() else {
            return;
        };
        let Ok(server) = TcpStream::connect(upstream) else {
            return;
        };
        let mut client_read = client.try_clone().expect("克隆客户端连接");
        let mut client_write = client;
        let mut server_read = server.try_clone().expect("克隆服务端连接");
        let mut server_write = server;

        // 客户端 → 服务端。读到 EOF（或出错）就是「对面把这条连接关了」。
        let up = std::thread::spawn(move || {
            let mut buf = [0u8; 8192];
            loop {
                match client_read.read(&mut buf) {
                    Ok(0) | Err(_) => break,
                    Ok(n) => {
                        if server_write.write_all(&buf[..n]).is_err() {
                            break;
                        }
                    }
                }
            }
            flag.store(true, Ordering::SeqCst);
            let _ = server_write.shutdown(Shutdown::Both);
        });

        // 服务端 → 客户端。
        let mut buf = [0u8; 8192];
        loop {
            match server_read.read(&mut buf) {
                Ok(0) | Err(_) => break,
                Ok(n) => {
                    if client_write.write_all(&buf[..n]).is_err() {
                        break;
                    }
                }
            }
        }
        let _ = client_write.shutdown(Shutdown::Both);
        let _ = up.join();
    });
    (addr, client_gone)
}

#[test]
fn closing_twice_is_idempotent_and_never_hangs() {
    // 借一个刚被放掉的本地端口：连上去必然 ECONNREFUSED。这条用例**不需要**
    // sftp-server，于是默认 `cargo test` 里也有一次真的覆盖——只是它验的是
    // 「后台线程已经自己退出」那条路径上的 close()。
    let port = {
        // 绑上再放掉：`TcpListener` 在这一句结束时就被丢弃，端口随之空出来。
        let bound = std::net::TcpListener::bind(("127.0.0.1", 0))
            .expect("bind 临时端口")
            .local_addr()
            .expect("取本地地址");
        bound.port()
    };

    let mut opts = SshOptions::new(
        "127.0.0.1",
        "tester",
        AuthMethod::Password {
            password: "nobody-home".to_string(),
        },
    );
    opts.port = port;
    opts.host_key = HostKeyPolicy::Insecure;
    opts.connect_timeout = Duration::from_secs(2);
    let mut handle = term_sftp::start_in_background(opts, || {});

    // 等它自己放弃：`Closed` 到了说明后台线程已经退出，`close()` 走的是
    // 「线程早就没了」这一支——它必须立刻返回，不能傻等满 2 秒。
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        assert!(Instant::now() < deadline, "10 秒内没等到 Closed 事件");
        match handle.try_event() {
            Some(Event::Closed { .. }) => break,
            Some(_) => {}
            None => std::thread::sleep(Duration::from_millis(10)),
        }
    }

    let started = Instant::now();
    let converged = handle.close();
    assert!(
        started.elapsed() < Duration::from_secs(1),
        "后台线程已经退出了，close() 不该等这么久（{:?}）",
        started.elapsed()
    );
    assert!(
        converged,
        "线程早就结束了，close() 应立刻确认收敛（而不是等到超时）"
    );
    assert!(handle.is_closed(), "close() 之后句柄应处于已关闭状态");

    // 第二次必须立刻返回：幂等不是「不 panic」而已，也不能再等一轮。
    let again = Instant::now();
    let converged_again = handle.close();
    assert!(
        again.elapsed() < Duration::from_millis(100),
        "第二次 close() 应立刻返回（{:?}）",
        again.elapsed()
    );
    assert_eq!(converged_again, converged, "第二次 close() 的结论不该变");

    // close() 之后发的请求静默丢弃：不 panic、也不会凭空冒出事件。
    handle.list("/");
    assert!(handle.try_event().is_none(), "close() 之后不该再有事件");
}

#[test]
#[ignore = "[SFTP-E2E] SKIP 需要系统 sftp-server；发布必跑：cargo test -p term-sftp --test e2e -- --ignored"]
fn closing_the_handle_hangs_up_the_server_side_session() {
    require_sftp_server();
    let root = fixture_root();
    let addr = start_server("s3cret", root.clone());
    let (proxy_addr, client_gone) = proxy_to(addr);

    let mut handle = term_sftp::start_in_background(options(proxy_addr, "s3cret"), || {});
    let home = wait_ready(&mut handle);
    let entries = wait_listed_all(&mut handle, &home);
    assert!(!entries.is_empty(), "夹具目录不该是空的");
    assert!(
        !client_gone.load(Ordering::SeqCst),
        "会话还用着，连接不该已经断了"
    );

    let started = Instant::now();
    let converged = handle.close();
    let elapsed = started.elapsed();
    eprintln!("{E2E_MARK} close() 用了 {elapsed:?}");
    assert!(
        elapsed < Duration::from_secs(5),
        "close() 应在收敛上限内返回，实际 {elapsed:?}"
    );
    assert!(
        converged,
        "close() 返回时后台线程应已收敛（收尾跑完），而不是被丢在半路"
    );
    assert!(handle.is_closed());
    // 幂等：第二次立刻返回。
    let again = Instant::now();
    assert!(handle.close(), "第二次 close() 的结论应与第一次一致");
    assert!(
        again.elapsed() < Duration::from_millis(100),
        "第二次 close() 应立刻返回（{:?}）",
        again.elapsed()
    );
    assert!(handle.try_event().is_none(), "close() 之后不该再有事件");

    // 服务端那边的证据：代理读到 EOF，说明 close() 之后这条连接真的不再开着。
    //
    // 说清楚这一条能证什么、不能证什么：进程内看不到 `SSH_MSG_DISCONNECT`
    // 这个包（连线和加密都在 term-ssh 里），所以「服务端立刻回收会话」这一步
    // 没法在这套夹具里直接断言。这里守的是它前面的必要条件——收尾之后连接
    // 不再挂着；将来谁把 `close()` 改成「只停线程、不碰连接」，这条会红。
    let deadline = Instant::now() + Duration::from_secs(10);
    while !client_gone.load(Ordering::SeqCst) {
        assert!(
            Instant::now() < deadline,
            "close() 之后 10 秒，服务端那条连接还开着——这就是泄漏"
        );
        std::thread::sleep(Duration::from_millis(10));
    }

    let _ = std::fs::remove_dir_all(&root);
    eprintln!("{E2E_MARK} PASS closing_the_handle_hangs_up_the_server_side_session");
}

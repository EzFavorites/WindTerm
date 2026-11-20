//! 真 OpenSSH 服务端上的握手验证（Docker）。
//!
//! 与 `tests/e2e.rs` 的分工：那边用同一份 russh 起服务端，证明的是
//! 「我们和自己兼容」；这边连的是**发行版自带的 sshd**，证明的是
//! 「和真实世界兼容」——认证方法协商、host key 类型、PTY 行为都由它决定。
//!
//! 默认不跑（`#[ignore]`）：需要容器。入口是 `scripts/docker-verify.sh`，
//! 它负责起容器并把下面的环境变量准备好。

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use term_core::channel::TermChannel;
use term_ssh::{AuthMethod, HostKeyPolicy, SshError, SshOptions, connect};

fn var(name: &str) -> String {
    std::env::var(name).unwrap_or_else(|_| {
        panic!("缺少环境变量 {name}：请用 scripts/docker-verify.sh 起容器后运行本测试")
    })
}

fn port(name: &str) -> u16 {
    var(name).parse().expect("端口应是数字")
}

/// 连目标的默认参数：不查 host key（TOFU 单独有一条测试），其余走真实默认。
fn target_opts(auth: AuthMethod) -> SshOptions {
    let mut opts = SshOptions::new(var("WINDTERM_DOCKER_HOST"), var("WINDTERM_DOCKER_USER"), auth);
    opts.port = port("WINDTERM_DOCKER_PORT");
    opts.host_key = HostKeyPolicy::Insecure;
    opts.connect_timeout = Duration::from_secs(10);
    opts.keepalive = None;
    opts
}

fn password() -> AuthMethod {
    AuthMethod::Password {
        password: var("WINDTERM_DOCKER_PASS"),
    }
}

/// 连上 → 发一句 `echo <marker>` → 等回显里出现 marker。
///
/// 为什么用回显而不是只看「连接成功」：`connect` 成功只说明握手完了，
/// 还不代表 PTY 要到了、shell 起来了、字节能双向流动。
fn expect_echo(opts: SshOptions, marker: &str) {
    let mut ch = connect(opts).expect("握手应成功");
    // 先等一小段欢迎语，避免在 shell 还没就绪时就写。
    drain_for(&mut ch, Duration::from_millis(300));

    ch.write(format!("echo {marker}\r").as_bytes());
    let mut seen = String::new();
    let deadline = Instant::now() + Duration::from_secs(10);
    while Instant::now() < deadline {
        let bytes = ch.drain();
        if !bytes.is_empty() {
            seen.push_str(&String::from_utf8_lossy(&bytes));
            if seen.contains(marker) {
                ch.close();
                return;
            }
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    panic!("10 秒内没等到 {marker} 的回显；实际收到：{seen:?}");
}

fn drain_for(ch: &mut term_ssh::SshChannel, dur: Duration) {
    let deadline = Instant::now() + dur;
    while Instant::now() < deadline {
        let _ = ch.drain();
        std::thread::sleep(Duration::from_millis(20));
    }
}

#[test]
#[ignore = "需要 Docker 里的真 sshd：bash scripts/docker-verify.sh ssh"]
fn password_auth_against_real_sshd() {
    expect_echo(target_opts(password()), "DOCKER-PASSWORD-OK");
}

#[test]
#[ignore = "需要 Docker 里的真 sshd：bash scripts/docker-verify.sh ssh"]
fn public_key_auth_against_real_sshd() {
    let key = PathBuf::from(var("WINDTERM_DOCKER_KEY"));
    assert!(key.exists(), "私钥不存在：{}", key.display());
    expect_echo(
        target_opts(AuthMethod::PrivateKey {
            path: key,
            passphrase: None,
        }),
        "DOCKER-PUBKEY-OK",
    );
}

#[test]
#[ignore = "需要 Docker 里的真 sshd：bash scripts/docker-verify.sh ssh"]
fn keyboard_interactive_against_real_sshd() {
    // 真 sshd 的 keyboard-interactive 在密码配置下会问一次 Password:，
    // 我们把密码作为第一条回答交出去。这一步在 R15 只能靠纯层单测，
    // 现在有了真实服务端才算真验过。
    expect_echo(
        target_opts(AuthMethod::KeyboardInteractive {
            answers: vec![var("WINDTERM_DOCKER_PASS")],
        }),
        "DOCKER-KBD-OK",
    );
}

/// 一次性的 known_hosts 文件（按 pid 区分，跑完删掉）。
fn temp_known_hosts(tag: &str) -> PathBuf {
    let path = std::env::temp_dir().join(format!(
        "windterm-known-{}-{tag}",
        std::process::id()
    ));
    let _ = std::fs::remove_file(&path);
    path
}

fn connect_with_ledger(port: u16, policy: HostKeyPolicy, ledger: &Path) -> Result<(), SshError> {
    let mut opts = target_opts(password());
    opts.port = port;
    opts.host_key = policy;
    opts.known_hosts = Some(ledger.to_path_buf());
    connect(opts).map(|_| ())
}

#[test]
#[ignore = "需要 Docker 里的真 sshd：bash scripts/docker-verify.sh ssh"]
fn tofu_records_then_rejects_a_changed_host_key() {
    let ledger = temp_known_hosts("tofu");
    let target_port = port("WINDTERM_DOCKER_PORT");

    // 1) 第一次：账本里没有这台主机，Tofu 记下来再连。
    connect_with_ledger(target_port, HostKeyPolicy::Tofu, &ledger).expect("首次连接应被接受");
    let written = std::fs::read_to_string(&ledger).expect("账本应被写出来");
    assert!(!written.trim().is_empty(), "Tofu 必须留下一条记录");

    // 2) 第二次：账本里已是同一把钥匙，Strict 也该放行。
    connect_with_ledger(target_port, HostKeyPolicy::Strict, &ledger).expect("同一把钥匙应被接受");

    // 3) 把账本里的钥匙换成**跳板机**的那把（同一算法、不同值）：
    //    这是「服务器换了一把钥匙」的最小可信模拟，比伪造一段乱码更接近真实。
    let bastion_ledger = temp_known_hosts("bastion");
    connect_with_ledger(
        port("WINDTERM_DOCKER_BASTION_PORT"),
        HostKeyPolicy::Tofu,
        &bastion_ledger,
    )
    .expect("跳板机首次连接应被接受");
    let bastion_line = std::fs::read_to_string(&bastion_ledger).expect("跳板机账本");
    let key_part: Vec<&str> = bastion_line
        .lines()
        .find(|l| !l.trim().is_empty())
        .expect("跳板机账本应有一行")
        .split_whitespace()
        .collect();
    assert!(key_part.len() >= 3, "known_hosts 行格式：host type key");
    let forged = format!(
        "[{}]:{} {} {}\n",
        var("WINDTERM_DOCKER_HOST"),
        target_port,
        key_part[1],
        key_part[2]
    );
    // 覆盖掉原来的那一行：同一台主机只保留一条冲突记录。
    std::fs::write(&ledger, forged).expect("写账本");

    match connect_with_ledger(target_port, HostKeyPolicy::Strict, &ledger) {
        Err(SshError::HostKey(_)) => {}
        other => panic!("换了钥匙必须被拒绝，实际：{other:?}"),
    }

    let _ = std::fs::remove_file(&ledger);
    let _ = std::fs::remove_file(&bastion_ledger);
}

#[test]
#[ignore = "需要 Docker 里的真 sshd：bash scripts/docker-verify.sh ssh"]
fn wrong_password_is_reported_as_auth_failure_not_a_hang() {
    let mut opts = target_opts(AuthMethod::Password {
        password: "definitely-not-the-password".to_string(),
    });
    opts.host_key = HostKeyPolicy::Insecure;
    let started = Instant::now();
    match connect(opts) {
        Err(SshError::Auth(_)) => {}
        other => panic!("错密码应是 Auth 错误，实际：{other:?}"),
    }
    assert!(
        started.elapsed() < Duration::from_secs(15),
        "认证失败不该拖到超时：{:?}",
        started.elapsed()
    );
}

#[test]
#[ignore = "需要 Docker 里的真 sshd：bash scripts/docker-verify.sh ssh"]
fn auth_chain_falls_back_from_a_rejected_password_to_a_key() {
    // D2 OneKey：密码被拒之后，在**同一个**连接上改用公钥，应该能登进去。
    // 这条只有真 sshd 能证：russh 自己的服务端不一定按同样的 `userauth`
    // 语义回放，而「被拒后再换一种」正是 OneKey 的全部意义。
    let key = PathBuf::from(var("WINDTERM_DOCKER_KEY"));
    assert!(key.exists(), "私钥不存在：{}", key.display());
    let mut opts = target_opts(AuthMethod::Password {
        password: "definitely-not-the-password".to_string(),
    });
    opts.auth_chain = vec![
        AuthMethod::Password {
            password: "definitely-not-the-password".to_string(),
        },
        AuthMethod::PrivateKey {
            path: key,
            passphrase: None,
        },
    ];
    expect_echo(opts, "DOCKER-CHAIN-OK");
}

#[test]
#[ignore = "需要 Docker 里的真 sshd：bash scripts/docker-verify.sh ssh"]
fn auth_chain_that_all_fail_reports_auth_not_a_hang() {
    // 整条链都被拒：应以 Auth 错误收场，且不拖到超时（否则用户会以为程序卡死）。
    let started = Instant::now();
    let mut opts = target_opts(AuthMethod::Password {
        password: "nope-1".to_string(),
    });
    opts.auth_chain = vec![
        AuthMethod::Password {
            password: "nope-1".to_string(),
        },
        AuthMethod::Password {
            password: "nope-2".to_string(),
        },
    ];
    match connect(opts) {
        Err(SshError::Auth(_)) => {}
        other => panic!("链全失败应是 Auth 错误，实际：{other:?}"),
    }
    assert!(
        started.elapsed() < Duration::from_secs(15),
        "链跑完不该拖到超时：{:?}",
        started.elapsed()
    );
}

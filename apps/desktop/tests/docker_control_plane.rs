//! 真机闭环：`desk script` 经**控制面通道工厂**连上真 sshd。
//!
//! 与 `script_run.rs` 的分工：那边只连「一个刚放掉的本地端口」，证明的是
//! 「没有被控制面拒掉」；这边连的是容器里发行版自带的 sshd，证明的是
//! 「脚本 → 会话库 → SessionPool::open → 工厂 → term_ssh::connect →
//! PTY → 回显」这一整条链路真的通了，而且字节能双向流动。
//!
//! 默认不跑（`#[ignore]`）：需要容器。入口是
//! `bash scripts/docker-verify.sh control`，它负责起容器并备好环境变量。
//!
//! 两条隔离纪律：
//! 1. `WINDTERM_PROFILE_DIR` 指到一次性目录，会话库不碰用户真实配置；
//! 2. 子进程的 `HOME` 也指到同一个临时目录——headless 路径用 `Tofu`
//!    首连策略，会把主机钥匙记进 `$HOME/.ssh/known_hosts`，必须让它写进
//!    沙箱而不是用户的 `~/.ssh`。

use std::io::Write;
use std::path::PathBuf;
use std::process::{Command, Stdio};

fn var(name: &str) -> String {
    std::env::var(name).unwrap_or_else(|_| {
        panic!("缺少环境变量 {name}：请用 scripts/docker-verify.sh control 起容器后运行本测试")
    })
}

/// 一次性 profile 目录（含一个假的 HOME）。
fn scratch() -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "desk-w1c-docker-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("时钟")
            .as_nanos()
    ));
    std::fs::create_dir_all(dir.join("home/.ssh")).expect("建临时 HOME/.ssh");
    dir
}

#[test]
#[ignore = "需要 Docker 里的真 sshd：bash scripts/docker-verify.sh control"]
fn desk_script_opens_a_builtin_ssh_session_through_the_control_plane() {
    let dir = scratch();
    let home = dir.join("home");

    let mut config =
        term_core::session::SessionConfig::new(term_core::session::Protocol::Ssh);
    config.name = "w1c-real-sshd".to_string();
    config.host = var("WINDTERM_DOCKER_HOST");
    config.port = var("WINDTERM_DOCKER_PORT")
        .parse()
        .expect("端口应是数字");
    config.username = var("WINDTERM_DOCKER_USER");
    // headless 没有对话框可以问密码：公钥文件是本地资产，不需要保险库。
    config.auth = term_core::session::AuthMethod::PublicKey;
    config.identity_file = var("WINDTERM_DOCKER_KEY");
    config.connection_timeout = 10;
    // `session_open::ssh_options` 会把端口、私钥、策略都带到连接参数上；
    // `builtin_ssh` 保持出厂的 true，才会走新接的工厂路径。
    assert!(config.builtin_ssh, "这条用例验的就是内置客户端");
    // `persist::save` 会拿走 config，后面断言还要用主机名，先留一份。
    let host = config.host.clone();

    // 会话库是 SQLite，用运行时的持久化层写（格式归 term-store）。
    // `WINDTERM_PROFILE_DIR` 是进程级的，集成测试多线程共享它，
    // 所以与 script_run.rs 一样用一把锁把「设 → 写 → 还原」串起来。
    static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
    {
        let _guard = ENV_LOCK.lock().unwrap_or_else(|poison| poison.into_inner());
        let previous = std::env::var_os("WINDTERM_PROFILE_DIR");
        std::env::set_var("WINDTERM_PROFILE_DIR", &dir);
        term_runtime::persist::save(&["Default".to_string()], &[config]);
        match previous {
            Some(value) => std::env::set_var("WINDTERM_PROFILE_DIR", value),
            None => std::env::remove_var("WINDTERM_PROFILE_DIR"),
        }
    }
    assert!(dir.join("sessions.db").is_file(), "会话库没写出来");

    // 发一条命令、等它的回显：只判断「连上了」不够，PTY 没要到、
    // shell 没起来、字节只单向流动，都会在这一步露出来。
    // 用 `write` 而不是 `send`：脚本面没有 `send` 这个原语（ADR-0027 的动词表里
    // 是 `write` / `send_keys`），这份用例曾因写错名字一直红着——真机用例
    // 默认 `#[ignore]`，没人跑就没人发现。
    let source = "write(\"echo W1C_CONTROL_PLANE_OK\\r\");\n\
                  let hit = wait(\"W1C_CONTROL_PLANE_OK\", 15.0);\n\
                  hit";
    let mut child = Command::new(env!("CARGO_BIN_EXE_term-desk"))
        .args([
            "script",
            "run",
            "-",
            "--session",
            "w1c-real-sshd",
            "--timeout",
            "30",
        ])
        .env("WINDTERM_PROFILE_DIR", &dir)
        .env("HOME", &home)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("拉起 term-desk");
    child
        .stdin
        .as_mut()
        .expect("stdin 管道")
        .write_all(source.as_bytes())
        .expect("写脚本");
    let out = child.wait_with_output().expect("等 term-desk 收场");

    let stderr = String::from_utf8_lossy(&out.stderr);
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert_eq!(
        out.status.code(),
        Some(0),
        "脚本应当跑完：stdout={stdout:?} stderr={stderr:?}"
    );
    assert!(
        stdout.contains("W1C_CONTROL_PLANE_OK"),
        "没等到真 sshd 的回显：stdout={stdout:?} stderr={stderr:?}"
    );
    // 已知的几种「假绿」都不能出现。
    assert!(!stderr.contains("需要注入"), "控制面不该再拒绝：{stderr:?}");
    assert!(!stderr.contains("解锁保险库"), "公钥认证不该要密码：{stderr:?}");
    assert!(
        !dir.join("home/.ssh/known_hosts").exists()
            || std::fs::read_to_string(dir.join("home/.ssh/known_hosts"))
                .unwrap_or_default()
                .contains(&host),
        "TOFU 记账应落在临时 HOME 里"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

//! ADR-0027 判据 ③ 的真机闭环：脚本的 `sftp_*` 原语在真 sshd 上传 / 取真字节。
//!
//! 为什么必须真机：这条链路是「脚本 → `SftpRemote`（`apps/desktop/src/script_sftp.rs`）
//! → `term_sftp::blocking::Client` → 真 `sftp-server`」，中间每一跳都能把参数顺序、
//! 路径解析、认证方式搞错，而**内存假实现一个都发现不了**。2026-11-18 就是这么
//! 抓到的：`Client::put` 把本地/远端传反，`sftp_put` 在真服务端上一直报
//! 「读不到本地文件」，而当时没有任何用例覆盖它。
//!
//! 默认不跑（`#[ignore]`）：需要容器。入口是 `bash scripts/docker-verify.sh control`，
//! 它负责起容器并备好 `WINDTERM_DOCKER_*`。
//!
//! 两条隔离纪律（与 `docker_control_plane.rs` 一致）：
//! 1. `WINDTERM_PROFILE_DIR` 指到一次性目录，会话库不碰用户真实配置；
//! 2. 子进程 `HOME` 也指到同一个临时目录——headless 路径用 `Tofu` 首连策略，
//!    会把主机钥匙记进 `$HOME/.ssh/known_hosts`。

use std::io::Write;
use std::path::PathBuf;
use std::process::{Command, Stdio};

fn var(name: &str) -> String {
    std::env::var(name).unwrap_or_else(|_| {
        panic!("缺少环境变量 {name}：请用 scripts/docker-verify.sh control 起容器后运行本测试")
    })
}

/// 一次性 profile 目录（含一个假的 HOME）。
fn scratch(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "desk-sftp-script-{tag}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("时钟")
            .as_nanos()
    ));
    std::fs::create_dir_all(dir.join("home/.ssh")).expect("建临时 HOME/.ssh");
    dir
}

/// 写一条内置 SSH 会话进一次性会话库，返回 profile 目录。
fn session_profile(tag: &str) -> PathBuf {
    let dir = scratch(tag);

    let mut config = term_core::session::SessionConfig::new(term_core::session::Protocol::Ssh);
    config.name = format!("sftp-real-{tag}");
    config.host = var("WINDTERM_DOCKER_HOST");
    config.port = var("WINDTERM_DOCKER_PORT").parse().expect("端口应是数字");
    config.username = var("WINDTERM_DOCKER_USER");
    // headless 没有对话框可以问密码：公钥是本地资产，不需要保险箱。
    config.auth = term_core::session::AuthMethod::PublicKey;
    config.identity_file = var("WINDTERM_DOCKER_KEY");
    config.connection_timeout = 10;
    // `SftpRemote::for_session` 明确拒绝系统 ssh 会话（拿不到连接句柄）。
    assert!(config.builtin_ssh, "这条用例验的就是内置客户端的 SFTP");

    // 会话库是 SQLite，用运行时的持久化层写。`WINDTERM_PROFILE_DIR` 是进程级
    // 环境变量、集成测试多线程共享，所以「设 → 写 → 还原」要串行。
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
    dir
}

#[test]
#[ignore = "需要 Docker 里的真 sshd：bash scripts/docker-verify.sh control"]
fn a_script_puts_and_gets_a_real_file_over_sftp() {
    let dir = session_profile("roundtrip");
    let home = dir.join("home");
    let payload = b"sftp-script-roundtrip-0123456789\n".to_vec();
    let local_in = dir.join("in.txt");
    let local_out = dir.join("out.txt");
    std::fs::write(&local_in, &payload).expect("写本地源文件");

    // 远端用**相对路径**：`sftp-server` 以服务端用户的家目录为基准，
    // 于是这条用例不假设容器里的绝对路径长什么样。
    let remote = format!("desk-sftp-{}.txt", std::process::id());
    // ① 先把会话真的开起来再碰 `sftp_*`：`ScriptHost` 的会话配置表
    // （`script_host.rs:116` 的 `remember`）只在会话被打开时登记，而
    // `remote_files(id)` 靠它拿连接参数。不先开就会得到
    // 「不知道会话 X 是怎么连的（没有它的配置），无法开 SFTP」——实测如此。
    // 顺带这也是真机判据的一部分：SFTP 用的凭据来自**已经连上的那条会话**。
    let source = format!(
        r#"write("echo SFTP_READY\r");
           wait("SFTP_READY", 15.0);
           sftp_put("{local}", "{remote}");
           let st = sftp_stat("{remote}");
           sftp_get("{remote}", "{out}");
           sftp_remove("{remote}");
           st.size"#,
        local = local_in.display(),
        out = local_out.display(),
        remote = remote,
    );

    // 本地端点（上传的来源、下载的目标）现在必须过授权信封：
    // sftp_put/get 经 workspace → LocalFs，不再绕开白名单。
    let allow = format!(
        "fs=read:{},fs=write:{}",
        local_in.display(),
        local_out.display()
    );
    let mut args = vec!["script", "run", "-", "--session", "sftp-real-roundtrip"];
    args.extend(["--allow", &allow, "--timeout", "30"]);
    let mut child = Command::new(env!("CARGO_BIN_EXE_term-desk"))
        .args(args)
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

    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert_eq!(
        out.status.code(),
        Some(0),
        "脚本应当跑完：stdout={stdout:?} stderr={stderr:?}"
    );
    // 远端大小由服务端报回来，必须等于本地字节数——传反 / 传空都会露馅。
    assert!(
        stdout.contains(&payload.len().to_string()),
        "`sftp_stat` 报的远端大小应等于 {} 字节：stdout={stdout:?} stderr={stderr:?}",
        payload.len()
    );
    // 取回来的字节逐字节比对：`sftp_get` 的本地/远端顺序同样在这里兜住。
    let back = std::fs::read(&local_out).expect("取回的文件读不到");
    assert_eq!(back, payload, "取回内容应与源文件一致");

    let _ = std::fs::remove_dir_all(&dir);
}

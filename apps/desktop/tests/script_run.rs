//! `desk script run` 的进程级验收（ADR-0019 腿 A 的第一个消费者）。
//!
//! 为什么要有这一层：`term-script` 自己的 e2e 是「库对库」——测试自己把
//! `SessionPool` 喂给引擎，绕过了 `apps/desktop` 这一侧的全部接线（子命令分发、
//! 参数解析、会话借出、退出码）。这一层只做一件事：**真的把二进制拉起来跑**，
//! 于是「脚本能跑」这句话第一次有无人值守的证据。
//!
//! 用 `--exec cat` 而不是 `echo`：`echo` 会在脚本登记等待之前就退出，
//! `wait` 于是诚实地报「会话已结束」——那是引擎的正确行为，不是这里要测的东西。
//! `cat` 回显并且一直活着，正好当「长寿命令」的例子。

use std::io::Write;
use std::path::PathBuf;
use std::process::{Command, Output, Stdio};

fn run_script(source: &str, extra: &[&str]) -> Output {
    run_script_in(source, extra, None)
}

/// 同 [`run_script`]，但可以给一个专属 profile 目录（`--session` 要读会话库）。
fn run_script_in(source: &str, extra: &[&str], profile: Option<&PathBuf>) -> Output {
    let mut command = Command::new(env!("CARGO_BIN_EXE_term-desk"));
    command
        .args(["script", "run", "-"])
        .args(extra)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    if let Some(dir) = profile {
        // 每个用例一个独立目录：既不读也不会写用户的真实会话库。
        command.env("WINDTERM_PROFILE_DIR", dir);
    }
    let mut child = command.spawn().expect("拉起 term-desk");
    child
        .stdin
        .as_mut()
        .expect("stdin 管道")
        .write_all(source.as_bytes())
        .expect("写脚本");
    child.wait_with_output().expect("等 term-desk 收场")
}

#[test]
fn a_script_drives_a_session_and_prints_its_last_expression() {
    let out = run_script(
        "write(\"hi\\n\");\nlet hit = wait(\"hi\", 5.0);\nhit",
        &["--exec", "cat", "--timeout", "10"],
    );
    assert_eq!(
        out.status.code(),
        Some(0),
        "脚本应当跑完：stderr={}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("hi"), "最后一个表达式应当打到 stdout：{stdout:?}");
}

#[test]
fn quiet_keeps_stdout_empty_but_still_succeeds() {
    let out = run_script(
        "write(\"hi\\n\");\nwait(\"hi\", 5.0);\n\"不该出现\"",
        &["--exec", "cat", "--timeout", "10", "--quiet"],
    );
    assert_eq!(out.status.code(), Some(0), "stderr={}", String::from_utf8_lossy(&out.stderr));
    assert!(
        out.stdout.is_empty(),
        "--quiet 下 stdout 必须是空的：{:?}",
        String::from_utf8_lossy(&out.stdout)
    );
}

#[test]
fn the_log_primitives_report_a_state_that_has_actually_happened() {
    // 回归：`Session::log_status` 是**缓存读**——只有 `poll()` 抽事件时才会
    // 更新。脚本与 JSON-RPC 客户端从不 poll，所以在 2026-11-14 之前
    // `log_start` 之后 `log_status()` 一直报 `off`、`log_stop` 误报
    // 「没在记录」（文件其实已经建出来了）。
    //
    // 这条用例钉的是「日志原语自洽」：开 → 报 recording → 关 → 文件在、
    // 内容里有脚本发出去的那一行。
    let dir = std::env::temp_dir().join(format!(
        "desk-script-log-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("时钟")
            .as_nanos()
    ));
    let _ = std::fs::remove_dir_all(&dir);

    // 三件事都要等到「监管者真的处理了」才算数：
    //   ① log_start 只是投命令，处理前到达的输出不进日志；
    //   ② log_stop 同理，且进程退出会打断未落盘的缓冲；
    //   ③ log_status 是缓存读，必须靠 poll 刷新（这正是本用例要钉的回归）。
    // 等法统一用**有限次 read 让出时间片**，不盲 sleep、不无限循环。
    let source = format!(
        r#"
        log_start("{dir}");
        let i = 0;
        while log_status().phase != "recording" && i < 100 {{
            read(0.02);
            i += 1;
        }}
        let during = log_status().phase;
        write("logged-line\n");
        read(0.5);
        log_stop();
        let j = 0;
        while log_status().phase == "recording" && j < 100 {{
            read(0.02);
            j += 1;
        }}
        during + "|" + log_status().phase
        "#,
        dir = dir.display()
    );
    let out = run_script(&source, &["--exec", "cat", "--timeout", "10"]);
    assert_eq!(
        out.status.code(),
        Some(0),
        "日志脚本应当跑通：stderr={}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("recording|off"),
        "日志状态必须真的走完 recording → off；缓存不刷新时报的是 off|off：{stdout:?}"
    );

    let entries: Vec<PathBuf> = std::fs::read_dir(&dir)
        .expect("日志目录应当存在")
        .filter_map(|e| e.ok().map(|e| e.path()))
        .collect();
    assert_eq!(entries.len(), 1, "应当有且只有一个日志文件：{entries:?}");
    let body = std::fs::read_to_string(&entries[0]).expect("读日志");
    assert!(
        body.contains("logged-line"),
        "脚本发出去的那行必须落盘：{body:?}"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_failing_script_exits_nonzero_and_says_why() {
    let out = run_script("wait(\"never-appears\", 0.2)", &["--exec", "cat", "--timeout", "10"]);
    assert_eq!(out.status.code(), Some(1), "脚本失败必须是 1，不能是 0");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("超时"), "原因要写在 stderr：{stderr:?}");
    assert!(out.stdout.is_empty(), "失败时不该往 stdout 写半截结果");
}

#[test]
fn a_usage_mistake_exits_two_without_opening_a_session() {
    let out = Command::new(env!("CARGO_BIN_EXE_term-desk"))
        .args(["script", "run"])
        .output()
        .expect("拉起 term-desk");
    assert_eq!(out.status.code(), Some(2), "用法错误是 2（与 serve 一致）");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("缺少脚本路径"), "{stderr:?}");
    assert!(
        stderr.contains("term-desk script run"),
        "要把用法打出来：{stderr:?}"
    );
}

// ── `--session`：脚本这条腿开**内置 SSH** 会话（ADR-0019 S3） ─────────────
//
// 这一组是 S3 的进程级证据。密码那条不需要网络：`open_session` 在碰 socket
// 之前就会报「需要保险库」。Agent 那条连一个刚放掉的本地端口——必然
// ECONNREFUSED，于是它证明的是「真的走了内置客户端，而不是被控制面拒掉」。

/// 建一个只属于本用例的 profile 目录，并往里写一条会话。
fn profile_with(name: &str, auth: term_core::session::AuthMethod, port: u16) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "desk-script-session-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("时钟")
            .as_nanos()
    ));
    std::fs::create_dir_all(&dir).expect("建 profile 目录");

    let mut session = term_core::session::SessionConfig::new(term_core::session::Protocol::Ssh);
    session.name = name.to_string();
    session.host = "127.0.0.1".to_string();
    session.port = port;
    session.username = "qa".to_string();
    session.auth = auth;
    // 短超时：没人监听的端口会立刻被拒，但别让一条挂住的连接拖死 CI。
    session.connection_timeout = 5;

    // 会话库是 SQLite；用运行时的持久化层写，不手搓 SQL
    // （格式归 term-store，测试不该跟着它漂）。
    // `WINDTERM_PROFILE_DIR` 是**进程级**的，而集成测试跑在同一进程的多条线程上：
    // 这一把锁把「设变量 → 写库 → 还原」串起来，否则会出现「A 写进 B 的目录」。
    static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
    {
        let _guard = ENV_LOCK.lock().unwrap_or_else(|poison| poison.into_inner());
        let previous = std::env::var_os("WINDTERM_PROFILE_DIR");
        std::env::set_var("WINDTERM_PROFILE_DIR", &dir);
        term_runtime::persist::save(&["Default".to_string()], &[session]);
        match previous {
            Some(value) => std::env::set_var("WINDTERM_PROFILE_DIR", value),
            None => std::env::remove_var("WINDTERM_PROFILE_DIR"),
        }
    }
    assert!(
        dir.join("sessions.db").is_file(),
        "会话库没写出来：{}",
        dir.display()
    );
    dir
}

#[test]
fn a_password_session_refuses_instead_of_silently_using_system_ssh() {
    let profile = profile_with(
        "prod-ssh",
        term_core::session::AuthMethod::Password,
        // 22 大概率没人监听；密码那条根本走不到 socket，端口只用于建库。
        1,
    );
    let out = run_script_in(
        "\"不该跑到这里\"",
        &["--session", "prod-ssh", "--timeout", "5"],
        Some(&profile),
    );

    let stderr = String::from_utf8_lossy(&out.stderr);
    assert_ne!(
        out.status.code(),
        Some(0),
        "密码认证的会话在 headless 下开不了，退出码必须非 0：stderr={stderr:?}"
    );
    assert!(
        stderr.contains("解锁保险库"),
        "要把「去解锁」这条路说出来：{stderr:?}"
    );
    assert!(
        stderr.contains("prod-ssh"),
        "要点名是哪一条会话：{stderr:?}"
    );
    assert!(
        out.stdout.is_empty(),
        "会话都没起来，不该往 stdout 写结果：{:?}",
        String::from_utf8_lossy(&out.stdout)
    );
    let _ = std::fs::remove_dir_all(&profile);
}

#[test]
fn an_agent_session_reaches_the_builtin_client() {
    // 端口先绑后放：此刻一定没人监听，连接会被立刻拒绝。
    let port = {
        let listener = std::net::TcpListener::bind(("127.0.0.1", 0)).expect("bind");
        listener.local_addr().expect("addr").port()
    };
    let profile = profile_with("agent-ssh", term_core::session::AuthMethod::Agent, port);
    let out = run_script_in("\"不该跑到这里\"", &["--session", "agent-ssh"], Some(&profile));

    let stderr = String::from_utf8_lossy(&out.stderr);
    assert_eq!(
        out.status.code(),
        Some(1),
        "会话起不来是 1（不是用法错误 2）：stderr={stderr:?}"
    );
    // 关键：报的是「内置 SSH 连接失败」，不是「需要注入通道」，也不是
    // 「需要保险库」——这条支路真的走进了 `term_ssh::connect`。
    assert!(
        stderr.contains("内置 SSH"),
        "应当由内置客户端去连：{stderr:?}"
    );
    assert!(
        !stderr.contains("解锁保险库"),
        "agent 认证不该要求保险库：{stderr:?}"
    );
    let _ = std::fs::remove_dir_all(&profile);
}

#[test]
fn an_unknown_session_name_is_a_usage_error_not_a_session_failure() {
    let out = run_script_in(
        "\"不该跑到这里\"",
        &["--session", "no-such-session"],
        Some(&std::env::temp_dir().join("desk-script-empty-profile")),
    );
    // 库不存在 → 输入问题 → 2（与会话起不来的 1 分开）。
    assert_eq!(
        out.status.code(),
        Some(2),
        "stderr={}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("会话库"), "{stderr:?}");
    assert!(
        stderr.contains("no-such-session"),
        "要点名找不到的那条：{stderr:?}"
    );
}

#[test]
fn a_script_can_open_switch_between_and_close_sessions() {
    // S-d 的进程级验收：真的拉起二进制，让脚本经 `session_open` 再开一条会话、
    // 在两条之间来回切、各自读自己的输出、最后关掉一条。
    //
    // 这条用例盯的是「多会话寻址」这件事本身：`read_until` 的游标按会话独立，
    // 所以两次读必须各拿到自己那条会话的行——若游标跟着脚本走，第二次读会
    // 撞上「没有新数据」而超时（那就是一次红）。
    let source = r#"
        let base = session_current();
        let second = session_open("cat");
        write("from-second\n");
        let got_second = read_until("from-second", 10);
        session_use(base);
        write("from-first\n");
        let got_first = read_until("from-first", 10);
        session_close(second.name);
        print(`base=${base} second=${second.name} sessions=${session_list().sessions.len()}`);
        print(`second=${got_second.contains("from-second")} first=${got_first.contains("from-first")}`);
        "done"
    "#;
    let out = run_script(source, &["--exec", "cat", "--timeout", "25"]);
    assert_eq!(
        out.status.code(),
        Some(0),
        "多会话脚本应当跑通：stdout={} stderr={}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("second=true first=true"),
        "两条会话必须各读到自己的输出（游标按会话独立）：{stdout:?}"
    );
    assert!(
        stdout.contains("sessions=1"),
        "关掉新开的那条之后应当只剩起始会话：{stdout:?}"
    );
}

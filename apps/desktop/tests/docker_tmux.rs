//! 真 tmux 的控制模式（`-CC`）验证（Docker）。
//!
//! 与 `term-app::tmux` 的单测分工：那边喂的是**金样本**（我们按文档写的行），
//！这边喂的是 tmux 自己吐出来的东西。区别很实在——控制模式的字段顺序、时间戳、
//！转义细节都是实现定义的，金样本只能证明「解析器符合我的理解」。
//！
//！这一层的价值在于：解析器第一次吃到不认识的输出时，是**降级透传**而不是
//！吞掉（见 `tmux.rs` 的 `Passthrough`）。
//！
//! **双向闭环**：attach → send `split-window -h` → 观察 `%layout-change` 含两个窗格
//! → send `detach-client` → 观察 `%exit`。这才是「真机闭环」，不是只看不说。

use std::io::{BufRead, BufReader, Write};
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use term_app::tmux::{ControlMode, Event};
use term_app::tmux_layout;

fn var(name: &str) -> String {
    std::env::var(name).unwrap_or_else(|_| {
        panic!("缺少环境变量 {name}：请用 scripts/docker-verify.sh 起容器后运行本测试")
    })
}

/// 起一条 `ssh -tt` 到容器里跑 `tmux -CC`，把 stdout 的行送进通道。
///
/// 为什么必须 `-tt`：没有远端 pty 时 tmux 会直接拒绝进控制模式（它需要一个
/// 终端来决定尺寸）。`-tt` 强制分配，即使本地 stdin 是管道。
fn spawn_control_mode() -> (std::process::Child, mpsc::Receiver<String>) {
    let host = var("WINDTERM_DOCKER_HOST");
    let port = var("WINDTERM_DOCKER_PORT");
    let user = var("WINDTERM_DOCKER_USER");
    let key = var("WINDTERM_DOCKER_KEY");

    let mut child = Command::new("ssh")
        .args([
            // `-F /dev/null` + `IdentityAgent=none` 是不弹授权框的关键：
            // 前者让 ssh 不读 `~/.ssh/config`（那里可能有这台机器自己的
            // IdentityFile / ProxyCommand），后者让它完全不去问 ssh-agent。
            // macOS 的「想使用钥匙串中的密钥」弹窗来自 agent，不来自 ssh 本身；
            // 少了 `IdentityAgent=none`，无人值守跑测试会卡在等用户点允许。
            // `IdentitiesOnly=yes` 再兜一层：只允许 `-i` 指定的那一把。
            "-tt",
            "-F",
            "/dev/null",
            "-p",
            &port,
            "-i",
            &key,
            "-o",
            "IdentityAgent=none",
            "-o",
            "StrictHostKeyChecking=no",
            "-o",
            "UserKnownHostsFile=/dev/null",
            "-o",
            "LogLevel=ERROR",
            "-o",
            "IdentitiesOnly=yes",
            "-o",
            "BatchMode=yes",
            &format!("{user}@{host}"),
            // `-A` 复用已存在的会话：重复跑不会堆一堆 session。
            "tmux -CC new-session -A -s docker",
        ])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("ssh 起不来（本机需要 ssh 客户端）");

    let stdout = child.stdout.take().expect("stdout");
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        for line in BufReader::new(stdout).lines() {
            match line {
                Ok(line) => {
                    if tx.send(line).is_err() {
                        break;
                    }
                }
                Err(_) => break,
            }
        }
    });
    (child, rx)
}

/// 发送一行到 tmux 控制模式（以 `\n` 结尾）。
fn send_line(stdin: &mut std::process::ChildStdin, line: &str) {
    use std::io::Write;
    let _ = stdin.write_all(line.as_bytes());
    let _ = stdin.flush();
}

#[test]
#[ignore = "需要 Docker 里的真 sshd + tmux：bash scripts/docker-verify.sh tmux"]
fn real_tmux_control_mode_is_parsed_into_events() {
    // 纯解析验证：attach 即可见 %begin/%end/%session-changed/%output
    let (mut child, rx) = spawn_control_mode();
    let mut mode = ControlMode::default();
    let mut events: Vec<Event> = Vec::new();
    let mut raw: Vec<String> = Vec::new();

    let deadline = Instant::now() + Duration::from_secs(15);
    while Instant::now() < deadline {
        match rx.recv_timeout(Duration::from_millis(200)) {
            Ok(line) => {
                raw.push(line.clone());
                events.push(mode.feed(&line));
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {
                if events.iter().any(|e| matches!(e, Event::Begin { .. }))
                    && events.iter().any(|e| matches!(e, Event::End { .. }))
                {
                    break;
                }
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
        }
    }

    if let Some(mut stdin) = child.stdin.take() {
        let _ = stdin.write_all(b"detach-client\n");
        let _ = stdin.flush();
    }
    std::thread::sleep(Duration::from_millis(200));
    let _ = child.kill();
    let _ = child.wait();

    assert!(
        !raw.is_empty(),
        "tmux 一行控制模式输出都没有：容器里没装 tmux，或 -CC 没进控制模式"
    );
    assert!(
        events.iter().any(|e| matches!(e, Event::Begin { .. })),
        "没解析出 %begin；实际输出前几行：{:?}",
        &raw[..raw.len().min(6)]
    );
    assert!(
        events.iter().any(|e| matches!(e, Event::End { .. })),
        "没解析出 %end；实际输出前几行：{:?}",
        &raw[..raw.len().min(6)]
    );
    assert!(
        events.iter().any(|e| matches!(
            e,
            Event::SessionChanged { .. } | Event::LayoutChange { .. } | Event::Output { .. }
        )),
        "只有 begin/end 没有实质事件；实际：{:?}",
        &raw[..raw.len().min(10)]
    );
    for (line, event) in raw.iter().zip(events.iter()) {
        let payload = line.strip_prefix("\u{1b}P1000p").unwrap_or(line);
        if !payload.starts_with('%') {
            assert!(
                matches!(event, Event::Passthrough(_)),
                "非控制模式行应透传：{line:?} -> {event:?}"
            );
        }
    }
}

#[test]
#[ignore = "需要 Docker 里的真 sshd + tmux：bash scripts/docker-verify.sh tmux"]
fn real_tmux_bidirectional_split_then_detach() {
    // 真机双向闭环
    // 1) attach，拿到首屏 %session-changed + %layout-change（单窗格）
    // 2) 发送 `split-window -h`，等待 %layout-change 包含 2 个 pane
    // 3) 发送 `detach-client`，等待 %exit
    let (mut child, rx) = spawn_control_mode();
    let mut mode = ControlMode::default();
    let mut events: Vec<Event> = Vec::new();
    let mut raw: Vec<String> = Vec::new();

    let deadline = Instant::now() + Duration::from_secs(20);

    // —— Phase 1: 等首屏（至少拿到 session-changed 与 shell prompt 表明 attach 成功） ——
    // 注意：初始 attach 未必立刻发 %layout-change（tmux 只在布局变化时发），
    // 这里只验证 session-changed 与 output（shell prompt）即可。
    let mut got_session = false;
    let mut got_prompt = false;
    while Instant::now() < deadline {
        match rx.recv_timeout(Duration::from_millis(200)) {
            Ok(line) => {
                raw.push(line.clone());
                let ev = mode.feed(&line);
                events.push(ev.clone());
                match &ev {
                    Event::SessionChanged { .. } => got_session = true,
                    Event::Output { .. } => got_prompt = true,
                    _ => {}
                }
                if got_session && got_prompt {
                    break;
                }
            }
            Err(mpsc::RecvTimeoutError::Timeout) => continue,
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
        }
    }
    assert!(got_session, "未收到 %session-changed；原始：{:?}", &raw);
    assert!(got_prompt, "未收到 shell prompt（%output）；原始：{:?}", &raw);
    assert!(mode.active, "首屏后控制模式应为 active");
    assert!(!mode.panes.is_empty(), "首屏应至少有一个 pane");

    let mut stdin = child.stdin.take().expect("stdin");

    // —— Phase 2: 发送 split-window -h，等待新的 layout-change（2 pane） ——
    send_line(&mut stdin, "split-window -h\n");
    let deadline2 = Instant::now() + Duration::from_secs(10);
    let mut got_split_layout = false;
    let mut pane_count = 0;
    while Instant::now() < deadline2 {
        match rx.recv_timeout(Duration::from_millis(200)) {
            Ok(line) => {
                raw.push(line.clone());
                let ev = mode.feed(&line);
                events.push(ev.clone());
                if let Event::LayoutChange { layout, .. } = &ev {
                    // 解析 layout，验证有两个 pane
                    if let Some(parsed) = tmux_layout::parse_layout(layout) {
                        let panes = parsed.panes();
                        // panes 类型：Vec<(u32, u16, u16, u16, u16)>
                        if panes.len() == 2 {
                            got_split_layout = true;
                            pane_count = panes.len();
                            break;
                        }
                    }
                }
            }
            Err(mpsc::RecvTimeoutError::Timeout) => continue,
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
        }
    }
    assert!(got_split_layout, "split-window -h 后未收到 2-pane layout-change；原始：{:?}", &raw);
    assert_eq!(pane_count, 2, "应有两个 pane");
    assert!(mode.active, "split 后控制模式仍应 active");

    // —— Phase 3: 发送 detach-client，等待 %exit ——
    send_line(&mut stdin, "detach-client\n");
    let deadline3 = Instant::now() + Duration::from_secs(10);
    let mut got_exit = false;
    while Instant::now() < deadline3 {
        match rx.recv_timeout(Duration::from_millis(200)) {
            Ok(line) => {
                raw.push(line.clone());
                let ev = mode.feed(&line);
                let is_exit = matches!(ev, Event::Exit { .. });
                events.push(ev);
                if is_exit {
                    got_exit = true;
                    break;
                }
            }
            Err(mpsc::RecvTimeoutError::Timeout) => continue,
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
        }
    }
    assert!(got_exit, "detach-client 后未收到 %exit；原始：{:?}", &raw);
    assert!(!mode.active, "%exit 后控制模式应 inactive");
    assert!(mode.exited, "应标记 exited");

    // 清理
    std::thread::sleep(Duration::from_millis(200));
    let _ = child.kill();
    let _ = child.wait();

    // 降级语义：非控制模式行必透传
    for (line, event) in raw.iter().zip(events.iter()) {
        let payload = line.strip_prefix("\u{1b}P1000p").unwrap_or(line);
        if !payload.starts_with('%') {
            assert!(
                matches!(event, Event::Passthrough(_)),
                "非控制模式行应透传：{line:?} -> {event:?}"
            );
        }
    }
}
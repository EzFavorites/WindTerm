//! 与真实 `lrzsz`（`sz` / `rz`）的互通测试。
//!
//! 这两条测试的**全部价值**在于「对方不是我自己」：之前的自环测试只能证明协议
//! 实现自洽，证明不了 `ZFILE` 的字段写法、`ZRINIT` 的 flags、帧尾的选择与真实
//! 实现看得懂彼此。
//!
//! # P0-3：默认 `#[ignore]`，绝不「静默跳过即绿」
//!
//! 旧实现在本机没有 `sz`/`rz` 时打一行「跳过」然后 `return`：libtest 把用例算成
//! passed，`cargo test` 全绿，但什么都没验；而能力面把两条 ZMODEM 菜单记为
//! `implemented`，发布签核因此拿不到可证伪的「过 / 不过」。现在两条都带
//! `#[ignore]`：
//!
//! - 默认 `cargo test -p term-zmodem --test interop` → `0 passed; 2 ignored`，
//!   且 libtest 会把 `#[ignore]` 的理由原样打出来（含 `[ZMODEM-INTEROP] SKIP`）。
//!   **看到 `ignored` 就是「没验」，不是「验过了」**，发布门不得拿这一行交差。
//! - 发布门必须显式点名 `-- --ignored`，且真的有 `sz`/`rz`。缺对端时
//!   `require_prog` 直接 `panic!`（带 `[ZMODEM-INTEROP] FAIL`），门会红。
//! - 真跑通时每条打一行 `[ZMODEM-INTEROP] PASS <用例名>`；留痕里数这两行，
//!   比只看 `cargo test` 的 `ok` 更可核对（`ok` 也可能是 0 passed）。
//!
//! # 发布必跑门：复现与留痕
//!
//! 脚本名以仓库里真实存在的文件为准（本任务不改 `scripts/**`）：
//!
//! ```text
//! # 0) 测试床先验：`xfer` 组确认远端 sz/rz 在位、ZRQINIT 帧头是真的
//! bash scripts/protocol-verify.sh xfer
//!
//! # 1) 容器互操作门（镜像 scripts/docker/rust.Dockerfile 自带 lrzsz 0.12 线）
//! bash scripts/rust-in-docker.sh test -p term-zmodem --test interop \
//!     -- --ignored --test-threads=1 --nocapture 2>&1 | tee /tmp/zmodem-interop.log
//!
//! # 2) 宿主 0.13 线（brew install lrzsz）同法复跑
//! cargo test -p term-zmodem --test interop -- --ignored --test-threads=1 --nocapture
//! ```
//!
//! 留痕判据：日志里 `2 passed`、`0 failed`，且恰好两行 `[ZMODEM-INTEROP] PASS`；
//! 出现 `[ZMODEM-INTEROP] FAIL` 或 `2 ignored` 一律不算过。
//!
//! `scripts/docker-verify.sh` 的分组是 ssh / proxyjump / tunnel / tmux / control，
//! **不含 ZMODEM**；`scripts/protocol-verify.sh xfer` 只验测试床与帧头，不跑 Rust
//! 状态机。两者都不能替代上面第 1 步。
//!
//! # 结构
//!
//! 每个方向都起一个子进程、`piped()` 接 stdin/stdout，另一头接状态机：
//!
//! ```text
//! sz stdout ──► 读线程 ──► mpsc ──► 主循环: Receiver::feed ──► Action::Send ──► sz stdin
//! ```
//!
//! 读操作放在独立线程里，主循环用 `recv_timeout` 卡总超时（20 秒）；超时或失败
//! 一律 `kill` + `wait`，不留僵尸。stderr 重定向到临时文件，失败时读出来贴进
//! 断言消息——lrzsz 的进度/错误都打在 stderr 上。

mod common;

use common::Sink;
use std::cell::{Cell, RefCell};
use std::fs::File;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};
use std::rc::Rc;
use std::sync::mpsc::{self, RecvTimeoutError};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use term_zmodem::{Config, Receiver, Sender};

/// 一次互通最多等多久。真实 lrzsz 在局域管道上通常 1 秒内结束。
const TIMEOUT: Duration = Duration::from_secs(20);

/// 发布留痕里可 grep 的标记：`SKIP` 写在 `#[ignore = "..."]` 的理由里，
/// libtest 默认会把理由连同 `ignored` 一起打印；`RUN` / `PASS` / `FAIL`
/// 由本文件在真跑时打印。
const INTEROP_MARK: &str = "[ZMODEM-INTEROP]";

fn have_prog(prog: &str) -> bool {
    Command::new(prog)
        .arg("--help")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok()
}

/// 真对端缺失时**硬失败**，取代旧实现里「打一行跳过然后 return」的假绿。
///
/// 两条用例默认 `#[ignore]`，只有发布门用 `--ignored` 显式点名时才会执行到这里。
/// 此时缺 `sz`/`rz` 说明这次门根本没验到东西，必须红；panic 文本里带复现命令，
/// 便于从 CI 日志直接看出缺的是依赖还是代码回归。
fn require_prog(prog: &str) {
    if have_prog(prog) {
        eprintln!("{INTEROP_MARK} RUN 真实对端 {prog} 可用");
        return;
    }
    panic!(
        "{INTEROP_MARK} FAIL 缺少真实对端 {prog}：本次 --ignored 联调门没有可验的对端。\n\
         容器（镜像自带 lrzsz）：bash scripts/rust-in-docker.sh test -p term-zmodem --test interop -- --ignored --test-threads=1 --nocapture\n\
         宿主（brew install lrzsz）：cargo test -p term-zmodem --test interop -- --ignored --nocapture"
    );
}

/// 会转义、会打断文件信息解析的载荷：控制字节 + `NUL` + 普通字节。
fn payload() -> Vec<u8> {
    (0..3000u32)
        .map(|i| match i % 7 {
            0 => 0x18,
            1 => 0x0D,
            2 => 0x10,
            3 => 0x11,
            4 => 0x13,
            5 => 0x00,
            _ => (i % 251) as u8,
        })
        .collect()
}

/// 进程唯一的临时目录，`Drop` 时删掉。
struct TempDir {
    path: PathBuf,
}

impl TempDir {
    fn new(tag: &str) -> Self {
        static SEQ: AtomicUsize = AtomicUsize::new(0);
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let seq = SEQ.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "term-zmodem-{tag}-{}-{seq}-{nanos}",
            std::process::id()
        ));
        std::fs::create_dir_all(&path).expect("建临时目录失败");
        Self { path }
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.path);
    }
}

fn tail(path: &Path) -> String {
    match std::fs::read(path) {
        Ok(bytes) => String::from_utf8_lossy(&bytes).into_owned(),
        Err(err) => format!("（读不到 stderr：{err}）"),
    }
}

/// 双向各留前 [`TRACE_LIMIT`] 字节，失败时打出来——真联调的价值就在这些字节上。
const TRACE_LIMIT: usize = 8192;

#[derive(Default)]
struct Trace {
    rx: Vec<u8>,
    tx: Vec<u8>,
}

impl Trace {
    fn push(buf: &mut Vec<u8>, bytes: &[u8]) {
        let room = TRACE_LIMIT.saturating_sub(buf.len());
        buf.extend_from_slice(&bytes[..bytes.len().min(room)]);
    }

    fn dump(&self, tag: &str) {
        eprintln!("[{tag}] 对端→我们（前 {TRACE_LIMIT} 字节）：\n{}", hex(&self.rx));
        eprintln!("[{tag}] 我们→对端（前 {TRACE_LIMIT} 字节）：\n{}", hex(&self.tx));
    }
}

fn hex(bytes: &[u8]) -> String {
    bytes
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<Vec<_>>()
        .join(" ")
}

/// 把子进程 stdout 的字节喂给 `on_chunk`，把返回的字节写回它的 stdin。
///
/// - `done` 由闭包置位（协议正常收尾或状态机报错）；
/// - 总超时 [`TIMEOUT`] 从进入本函数算起；
/// - 无论成败都 `kill` + `wait` 兜底（成功时先给它一点时间自己退）。
fn pump(
    mut child: Child,
    mut stdin: ChildStdin,
    stdout: ChildStdout,
    mut on_chunk: impl FnMut(&[u8]) -> Vec<u8>,
    done: Rc<Cell<bool>>,
) -> Result<(), String> {
    let (tx, rx) = mpsc::channel::<Vec<u8>>();
    let reader = thread::spawn(move || {
        let mut stdout = stdout;
        let mut buf = [0u8; 8192];
        loop {
            match stdout.read(&mut buf) {
                Ok(0) => break,
                Ok(n) => {
                    if tx.send(buf[..n].to_vec()).is_err() {
                        break;
                    }
                }
                Err(_) => break,
            }
        }
    });

    let deadline = Instant::now() + TIMEOUT;
    let mut result = Ok(());
    loop {
        if done.get() {
            break;
        }
        let Some(remaining) = deadline.checked_duration_since(Instant::now()) else {
            result = Err(format!("超时：{} 秒内协议未完成", TIMEOUT.as_secs()));
            break;
        };
        match rx.recv_timeout(remaining) {
            Ok(chunk) => {
                let reply = on_chunk(&chunk);
                if !reply.is_empty() {
                    if let Err(err) = stdin.write_all(&reply).and_then(|()| stdin.flush()) {
                        result = Err(format!("写子进程 stdin 失败：{err}"));
                        break;
                    }
                }
            }
            Err(RecvTimeoutError::Timeout) => {
                result = Err(format!("超时：{} 秒内协议未完成", TIMEOUT.as_secs()));
                break;
            }
            Err(RecvTimeoutError::Disconnected) => {
                if !done.get() {
                    result = Err("对端在协议完成前关闭了 stdout".to_string());
                }
                break;
            }
        }
    }

    // 关掉 stdin 相当于告诉子进程「没有更多输入了」。
    drop(stdin);
    if result.is_err() {
        let _ = child.kill();
        let _ = child.wait();
    } else if wait_until(&mut child, deadline).is_err() {
        let _ = child.kill();
        let _ = child.wait();
        result = Err(format!(
            "协议已完成，但子进程在 {} 秒内没有退出",
            TIMEOUT.as_secs()
        ));
    }
    let _ = reader.join();
    result
}

fn wait_until(child: &mut Child, deadline: Instant) -> Result<(), ()> {
    loop {
        match child.try_wait() {
            Ok(Some(_)) => return Ok(()),
            Ok(None) => {
                if Instant::now() >= deadline {
                    return Err(());
                }
                thread::sleep(Duration::from_millis(10));
            }
            Err(_) => return Ok(()),
        }
    }
}

/// 我们收，真实 `sz` 发。
///
/// `#[ignore]` 是**可控跳过**：默认 `cargo test` 显示 `ignored` 而不是 `passed`；
/// 容器互操作门用 `-- --ignored` 跑出 `passed` 才算过（见文件头复现命令）。
#[test]
#[ignore = "[ZMODEM-INTEROP] SKIP 需要真实 lrzsz(sz)；发布必跑：bash scripts/rust-in-docker.sh test -p term-zmodem --test interop -- --ignored"]
fn receive_from_real_sz() {
    require_prog("sz");

    let dir = TempDir::new("sz");
    let payload = payload();
    std::fs::write(dir.path.join("payload.bin"), &payload).expect("写源文件失败");
    let err_path = dir.path.join("sz.err");

    let mut child = Command::new("sz")
        .arg("--binary")
        .arg("--verbose")
        .arg("payload.bin")
        .current_dir(&dir.path)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::from(File::create(&err_path).expect("建 stderr 文件失败")))
        .spawn()
        .expect("启动 sz 失败");
    let mut stdin = child.stdin.take().expect("sz 没有 stdin");
    let stdout = child.stdout.take().expect("sz 没有 stdout");

    let sink = Rc::new(RefCell::new(Sink::default()));
    let mut rx = Receiver::new(Config::default());
    sink.borrow_mut().apply(rx.start());
    stdin
        .write_all(&sink.borrow_mut().take_out())
        .expect("写初始 ZRINIT 失败");
    stdin.flush().expect("flush 失败");

    let done = Rc::new(Cell::new(false));
    let flag = Rc::clone(&done);
    let sink_in = Rc::clone(&sink);
    let trace = Rc::new(RefCell::new(Trace::default()));
    let trace_in = Rc::clone(&trace);
    let got = Rc::new(Cell::new(0u64));
    let got_in = Rc::clone(&got);
    let result = pump(
        child,
        stdin,
        stdout,
        move |chunk| {
            Trace::push(&mut trace_in.borrow_mut().rx, chunk);
            let mut sink = sink_in.borrow_mut();
            sink.apply(rx.feed(chunk));
            got_in.set(rx.received_bytes());
            if sink.done || !sink.errors.is_empty() {
                flag.set(true);
            }
            let reply = sink.take_out();
            Trace::push(&mut trace_in.borrow_mut().tx, &reply);
            reply
        },
        done,
    );

    let sink = sink.borrow();
    let stderr = tail(&err_path);
    if result.is_err() || !sink.done {
        eprintln!("[receive] received_bytes={} files={} file_done={} errors={:?}", got.get(), sink.files.len(), sink.file_done, sink.errors);
        trace.borrow().dump("receive_from_real_sz");
    }
    assert!(
        result.is_ok(),
        "与 sz 互通失败：{result:?}\nsz stderr：{stderr}"
    );
    assert!(
        sink.done,
        "协议没有正常收尾；错误：{:?}\nsz stderr：{stderr}",
        sink.errors
    );
    assert_eq!(sink.files.len(), 1, "应当恰好收到一个文件");
    let file = &sink.files[0];
    assert!(
        file.name.ends_with("payload.bin"),
        "对端给的文件名不对：{:?}",
        file.name
    );
    assert_eq!(file.data, payload, "收到的内容与源文件不一致");
    eprintln!("{INTEROP_MARK} PASS receive_from_real_sz：sz → Receiver 逐字节一致");
}

/// 我们发，真实 `rz` 收。
///
/// `#[ignore]` 是**可控跳过**：默认 `cargo test` 显示 `ignored` 而不是 `passed`；
/// 容器互操作门用 `-- --ignored` 跑出 `passed` 才算过（见文件头复现命令）。
#[test]
#[ignore = "[ZMODEM-INTEROP] SKIP 需要真实 lrzsz(rz)；发布必跑：bash scripts/rust-in-docker.sh test -p term-zmodem --test interop -- --ignored"]
fn send_to_real_rz() {
    require_prog("rz");

    let dir = TempDir::new("rz");
    let payload = payload();
    let err_path = dir.path.join("rz.err");

    let mut child = Command::new("rz")
        .arg("--binary")
        .current_dir(&dir.path)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::from(File::create(&err_path).expect("建 stderr 文件失败")))
        .spawn()
        .expect("启动 rz 失败");
    let mut stdin = child.stdin.take().expect("rz 没有 stdin");
    let stdout = child.stdout.take().expect("rz 没有 stdout");

    let sink = Rc::new(RefCell::new(Sink::default()));
    let mut tx = Sender::new(Config::default());
    tx.add_file("payload.bin", payload.clone())
        .expect("排文件失败");
    sink.borrow_mut().apply(tx.start());
    stdin
        .write_all(&sink.borrow_mut().take_out())
        .expect("写初始 ZRQINIT 失败");
    stdin.flush().expect("flush 失败");

    let done = Rc::new(Cell::new(false));
    let flag = Rc::clone(&done);
    let sink_in = Rc::clone(&sink);
    let trace = Rc::new(RefCell::new(Trace::default()));
    let trace_in = Rc::clone(&trace);
    let result = pump(
        child,
        stdin,
        stdout,
        move |chunk| {
            Trace::push(&mut trace_in.borrow_mut().rx, chunk);
            let mut sink = sink_in.borrow_mut();
            sink.apply(tx.feed(chunk));
            if sink.done || !sink.errors.is_empty() {
                flag.set(true);
            }
            let reply = sink.take_out();
            Trace::push(&mut trace_in.borrow_mut().tx, &reply);
            reply
        },
        done,
    );

    let sink = sink.borrow();
    let stderr = tail(&err_path);
    if result.is_err() || !sink.done {
        trace.borrow().dump("send_to_real_rz");
    }
    assert!(
        result.is_ok(),
        "与 rz 互通失败：{result:?}\nrz stderr：{stderr}"
    );
    assert!(
        sink.done,
        "协议没有正常收尾；错误：{:?}\nrz stderr：{stderr}",
        sink.errors
    );
    let received = std::fs::read(dir.path.join("payload.bin")).unwrap_or_else(|err| {
        panic!("rz 没有落下 payload.bin：{err}\nrz stderr：{stderr}");
    });
    assert_eq!(received, payload, "rz 落盘的内容与源文件不一致");
    eprintln!("{INTEROP_MARK} PASS send_to_real_rz：Sender → rz 落盘逐字节一致");
}

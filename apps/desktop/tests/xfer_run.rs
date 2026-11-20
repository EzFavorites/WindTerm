//! `apps/desktop/src/xfer_run.rs` 的验收：真的会话、真的字节、真的落盘。
//!
//! 这一层故意**不用内存假的会话**：`attach_raw` 的语义（字节改道、屏幕冻结、
//! detach 之后 VT 恢复）只有在真的 `SessionPool` 上才有意义，而「取消之后会话
//! 有没有还回来」正是本轮最关键的证据。所以每条用例都起一个本地 PTY：
//!
//! ```text
//! sh -c 'stty raw -echo; echo READY; cat'
//! ```
//!
//! - `stty raw -echo`：对端必须原样吞吐二进制。默认（行规程）模式会把 0x04 当
//!   EOF、把 0x0A 当行尾、还会把每个字节回显一遍——那样芯子收到的字节就不对了。
//! - `echo READY` 是**就绪信号**：它在旁路登记之前就打出来了，我们轮询 VT 快照
//!   看到它才动手，于是「stty 生效了没有」不必靠 sleep 赌。
//! - `cat` 把收到的字节原样写回：这就是一个理想的协议对端（回环）。
//!
//! # 喂字节为什么要一帧一帧喂
//!
//! 会话是 PTY，写入缓冲有限（约 1KB）。一次 `write` 里塞进整段流会被拆成多次，
//! 而芯子同时在同一方向上写 ACK——两股字节会在 pty 输入队列里交错，回显之后
//! 就可能在**帧中间**插进一个 ACK，把这一帧的 CRC 打坏。按协议帧边界写就没有
//! 这个问题：ACK 只会落在两帧之间，那正是接收方要容忍的噪声。
//!
//! 所以这里用 128 字节块（`Xmodem`，或者数据 ≤128 字节的 `Ymodem`），保证每帧
//! 线上长度（133）小于 pty 缓冲，单次写入是原子的。

// 集成测试只能看到 crate 的 **lib** 目标，而 desk 是纯 bin（没有 lib.rs）。
// 用 `#[path]` 把驱动模块直接编进本测试二进制：测的就是 `src/` 里那份源码，
// 不复制、不改造。模块里的私有项（`resolve_name`）也因此在同一 crate 内可见。
#[path = "../src/xfer_run.rs"]
// 本测试二进制只用到驱动面的一部分（比如不跑 XMODEM-1K）；模块里其余公开项
// 在 bin 里由 `main.rs` 的 `#![allow(dead_code)]` 统一豁免，这里补同一条。
#[allow(dead_code)]
mod xfer_run;

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use term_core::session_runtime::SessionId;
use term_session::SessionPool;
use term_xfer::{Action, Config, Phase, Protocol, Sender};
use xfer_run::{XferHandle, XferKind, XferProtocol};

const SOH: u8 = 0x01;
const STX: u8 = 0x02;

// ── 夹具 ───────────────────────────────────────────────────────────────────

/// 每个用例一个独立目录，互不干扰，也方便断言「目录里到底多了什么」。
fn unique_dir(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "desk-xfer-{tag}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("时钟")
            .as_nanos()
    ));
    std::fs::create_dir_all(&dir).expect("建临时目录");
    dir
}

/// 起一个「二进制回环」会话，并等它真的进入 raw 模式。
fn session() -> (Arc<SessionPool>, SessionId) {
    let pool = Arc::new(SessionPool::new());
    let id = pool
        .open_exec("stty raw -echo; echo READY; cat", 80, 24)
        .expect("起会话");
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        // 会话提前死掉就立刻说清楚：机器上的 pty/进程被别的活儿挤爆时，
        // `openpty` 能成功但子进程起不来。干等 20 秒只会把现场糊掉。
        let state = pool.state(id).expect("会话应当在册");
        assert!(
            matches!(state, term_core::session_runtime::SessionState::Alive),
            "会话没进入运行态，READY 不会来：{state:?}"
        );
        let _ = pool.poll(id);
        if pool
            .snapshot(id)
            .is_some_and(|s| s.screen_text().contains("READY"))
        {
            return (pool, id);
        }
        assert!(Instant::now() < deadline, "会话 20 秒还没就绪");
        std::thread::sleep(Duration::from_millis(10));
    }
}

/// 用真的 `term_xfer::Sender` 造一段完整的线上字节流（对端按规矩应答）。
///
/// 发送方是「收方开口」的：先 `'C'`（我要 CRC），之后每个 ACK 推一帧，遇到
/// 「等对方再喊一声」的阶段（YMODEM 头块之后 / EOT 之后）改喂 `'C'`。
fn modem_stream(protocol: Protocol, name: &str, data: &[u8]) -> Vec<u8> {
    let mut tx = Sender::new(Config::for_protocol(protocol));
    tx.add_file(name, data.to_vec()).expect("装进待发队列");
    let mut wire = Vec::new();
    let mut first = true;
    let mut guard = 0;
    while !tx.phase().is_terminal() {
        guard += 1;
        assert!(guard < 10_000, "状态机没有收敛：{:?}", tx.phase());
        let byte = if first {
            first = false;
            b'C'
        } else {
            match tx.phase() {
                Phase::AwaitingDataRequest | Phase::AwaitingNextHeader => b'C',
                _ => 0x06, // ACK
            }
        };
        for action in tx.feed(&[byte]) {
            match action {
                Action::Send(bytes) => wire.extend_from_slice(&bytes),
                Action::Done => {}
                other => panic!("发送侧不该产出 {other:?}"),
            }
        }
    }
    assert!(!wire.is_empty(), "没有造出任何字节");
    wire
}

/// 按协议帧边界喂进会话（理由见模块注释「喂字节为什么要一帧一帧喂」）。
fn feed_framewise(pool: &SessionPool, id: SessionId, wire: &[u8]) {
    let mut at = 0;
    while at < wire.len() {
        let len = match wire[at] {
            SOH => 3 + 128 + 2,
            STX => 3 + 1024 + 2,
            _ => 1, // EOT 之类的单字节
        };
        let end = (at + len).min(wire.len());
        assert!(pool.send(id, &wire[at..end]), "写不进会话（id={id}）");
        at = end;
        std::thread::sleep(Duration::from_millis(2));
    }
}

fn wait_until(what: &str, mut done: impl FnMut() -> bool) {
    // 20 秒：回环传输本身是毫秒级的，这条线只是给「机器被别的构建挤爆」留余量。
    let deadline = Instant::now() + Duration::from_secs(20);
    while !done() {
        assert!(Instant::now() < deadline, "20 秒还没等到：{what}");
        std::thread::sleep(Duration::from_millis(10));
    }
}

/// 目录里唯一的文件（XMODEM 的落盘名由驱动自己起，用例不该把它写死）。
fn only_file(dir: &Path) -> PathBuf {
    let mut entries: Vec<_> = std::fs::read_dir(dir)
        .expect("读目录")
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .collect();
    entries.sort();
    assert_eq!(entries.len(), 1, "目录里应当正好一个文件：{entries:?}");
    entries.remove(0)
}

fn payload(len: usize) -> Vec<u8> {
    // 每 251 取模：既不是全 0 也不是全 SUB，且末字节不是 0x1A（XMODEM 无长度，
    // 末尾的 0x1A 会被当成块尾填充吃掉——那是协议缺陷，不是驱动的错）。
    (0..len).map(|i| (i % 251) as u8).collect()
}

// ── 1. 端到端落盘 ─────────────────────────────────────────────────────────

#[test]
fn xmodem_bytes_land_on_disk_byte_for_byte() {
    let dir = unique_dir("rx-xmodem");
    let (pool, id) = session();
    let data = payload(1500);
    let wire = modem_stream(Protocol::Xmodem, "unused.bin", &data);

    let handle = XferHandle::start(
        pool.clone(),
        id,
        XferKind::Receive,
        XferProtocol::Xmodem,
        dir.clone(),
    )
    .expect("开始接收");
    // 让线程先把握手字符发出去，再喂流（见模块注释里的交错说明）。
    std::thread::sleep(Duration::from_millis(50));
    feed_framewise(&pool, id, &wire);

    wait_until("传输结束", || handle.progress().finished);
    let progress = handle.progress();
    assert_eq!(progress.error, None, "{progress:?}");
    assert_eq!(progress.bytes, data.len() as u64, "{progress:?}");
    // XMODEM 不带文件名，也没有总大小。
    assert_eq!(progress.total, None);

    let path = only_file(&dir);
    let landed = std::fs::read(&path).expect("读落盘文件");
    assert_eq!(landed.len(), data.len(), "字节数不对：{}", path.display());
    assert!(landed == data, "内容不是逐字节相等：{}", path.display());

    // 已收尾的传输，再取消是空操作：不许把「成功」翻成「已取消」。
    handle.cancel();
    let after = handle.progress();
    assert!(after.finished, "{after:?}");
    assert_eq!(after.error, None, "成功之后取消不该留下错误：{after:?}");
    pool.close(id);
}

#[test]
fn ymodem_header_gives_the_name_and_the_size() {
    let dir = unique_dir("rx-ymodem");
    let (pool, id) = session();
    let data = payload(100); // ≤128：帧长 133，保证单次写入原子
    let wire = modem_stream(Protocol::Ymodem, "hello.bin", &data);

    let handle = XferHandle::start(
        pool.clone(),
        id,
        XferKind::Receive,
        XferProtocol::Ymodem,
        dir.clone(),
    )
    .expect("开始接收");
    std::thread::sleep(Duration::from_millis(50));
    feed_framewise(&pool, id, &wire);

    wait_until("传输结束", || handle.progress().finished);
    let progress = handle.progress();
    assert_eq!(progress.error, None, "{progress:?}");
    assert_eq!(progress.total, Some(data.len() as u64));
    assert_eq!(progress.bytes, data.len() as u64);
    assert_eq!(
        std::fs::read(dir.join("hello.bin")).expect("读落盘文件"),
        data
    );
    pool.close(id);
}

// ── 2. 路径穿越 ───────────────────────────────────────────────────────────

#[test]
fn a_traversal_name_lands_inside_the_target_directory_as_a_bare_name() {
    let dir = unique_dir("rx-traversal");
    let (pool, id) = session();
    let data = b"root:x:0:0".to_vec();
    let wire = modem_stream(Protocol::Ymodem, "../../etc/passwd", &data);

    let handle = XferHandle::start(
        pool.clone(),
        id,
        XferKind::Receive,
        XferProtocol::Ymodem,
        dir.clone(),
    )
    .expect("开始接收");
    std::thread::sleep(Duration::from_millis(50));
    feed_framewise(&pool, id, &wire);

    wait_until("传输结束", || handle.progress().finished);
    assert_eq!(handle.progress().error, None);
    // 关键：落在目标目录下、名字只有 basename；目标目录之外一个字节都没写。
    assert_eq!(
        std::fs::read(dir.join("passwd")).expect("应当落在目标目录下的 passwd"),
        data
    );
    assert!(!dir.join("etc").exists(), "不许把目录结构带进来");
    pool.close(id);
}

#[test]
fn a_name_that_is_only_a_traversal_is_refused_and_writes_nothing() {
    let dir = unique_dir("rx-refuse-dotdot");
    let (pool, id) = session();
    let wire = modem_stream(Protocol::Ymodem, "..", b"x");

    let handle = XferHandle::start(
        pool.clone(),
        id,
        XferKind::Receive,
        XferProtocol::Ymodem,
        dir.clone(),
    )
    .expect("开始接收");
    std::thread::sleep(Duration::from_millis(50));
    feed_framewise(&pool, id, &wire);

    wait_until("驱动报错", || handle.progress().error.is_some());
    let progress = handle.progress();
    assert!(!progress.finished, "拒绝之后不该说成功：{progress:?}");
    let error = progress.error.expect("应当有错误");
    assert!(error.contains("文件名") || error.contains("路径"), "{error}");
    assert_eq!(
        std::fs::read_dir(&dir).expect("读目录").count(),
        0,
        "拒绝的文件名不许落下任何文件"
    );
    pool.close(id);
}

#[test]
fn an_absolute_path_is_refused_and_writes_nothing() {
    let dir = unique_dir("rx-refuse-abs");
    let (pool, id) = session();
    let wire = modem_stream(Protocol::Ymodem, "/abs/passwd", b"x");

    let handle = XferHandle::start(
        pool.clone(),
        id,
        XferKind::Receive,
        XferProtocol::Ymodem,
        dir.clone(),
    )
    .expect("开始接收");
    std::thread::sleep(Duration::from_millis(50));
    feed_framewise(&pool, id, &wire);

    wait_until("驱动报错", || handle.progress().error.is_some());
    let progress = handle.progress();
    let error = progress.error.clone().expect("应当有错误");
    assert!(error.contains("绝对路径"), "{error}");
    assert_eq!(
        std::fs::read_dir(&dir).expect("读目录").count(),
        0,
        "拒绝的路径不许落下任何文件"
    );
    pool.close(id);
}

// ── 3. 取消 + 交还会话 ────────────────────────────────────────────────────

#[test]
fn cancel_stops_the_thread_and_gives_the_session_back_to_the_vt() {
    let dir = unique_dir("rx-cancel");
    let (pool, id) = session();
    let handle = XferHandle::start(
        pool.clone(),
        id,
        XferKind::Receive,
        XferProtocol::Xmodem,
        dir.clone(),
    )
    .expect("开始接收");

    // 先确认线程真的跑起来了（握手字符已发，阶段离开「待开始」）。
    wait_until("进入握手", || handle.progress().phase != "待开始");
    handle.cancel();

    wait_until("驱动收尾", || handle.progress().error.is_some());
    let progress = handle.progress();
    assert!(!progress.finished, "取消不是成功：{progress:?}");
    // 这个字面量是 UI 的契约（它按它区分「中性取消」与「真失败」），钉死。
    assert_eq!(progress.error.as_deref(), Some("传输已取消"), "{progress:?}");
    assert_eq!(progress.phase, "已中止", "{progress:?}");
    drop(handle); // 句柄也没了，线程必须照样退出

    // **这条是重点**：旁路交还之后，往会话里打的文本必须重新出现在 VT 上。
    // 如果 detach 漏了，字节会被（已经没人读的）sink 吃掉，屏幕永远不动。
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        pool.send(id, b"AFTER-DETACH\r\n");
        std::thread::sleep(Duration::from_millis(50));
        let _ = pool.poll(id);
        if pool
            .snapshot(id)
            .is_some_and(|s| s.screen_text().contains("AFTER-DETACH"))
        {
            break;
        }
        assert!(Instant::now() < deadline, "取消之后 VT 没有恢复（旁路没交还）");
    }
    pool.close(id);
}

// ── 4. 目标目录不可用 ─────────────────────────────────────────────────────

#[test]
fn a_missing_target_directory_is_refused_without_panicking() {
    let (pool, id) = session();
    let Err(err) = XferHandle::start(
        pool.clone(),
        id,
        XferKind::Receive,
        XferProtocol::Xmodem,
        PathBuf::from("/definitely/not/here/desk-xfer"),
    ) else {
        panic!("目录不存在应当当场拒绝");
    };
    assert!(err.contains("不可用"), "{err}");
    assert!(err.contains("/definitely/not/here/desk-xfer"), "要点名路径：{err}");
    pool.close(id);
}

#[test]
fn a_target_that_is_a_file_is_refused_too() {
    let dir = unique_dir("rx-not-a-dir");
    let file = dir.join("plain.txt");
    std::fs::write(&file, b"x").expect("写文件");
    let (pool, id) = session();
    let Err(err) = XferHandle::start(
        pool.clone(),
        id,
        XferKind::Receive,
        XferProtocol::Xmodem,
        file,
    ) else {
        panic!("目标不是目录应当当场拒绝");
    };
    assert!(err.contains("不是目录"), "{err}");
    pool.close(id);
}

// ── 5. Progress 初值 ──────────────────────────────────────────────────────

#[test]
fn a_fresh_handle_starts_at_zero_and_is_not_finished() {
    let dir = unique_dir("rx-initial");
    let (pool, id) = session();
    let handle = XferHandle::start(
        pool.clone(),
        id,
        XferKind::Receive,
        XferProtocol::Xmodem,
        dir.clone(),
    )
    .expect("开始接收");
    let progress = handle.progress();
    assert_eq!(progress.bytes, 0, "{progress:?}");
    assert!(!progress.finished, "{progress:?}");
    assert_eq!(progress.total, None, "XMODEM 没有总大小");
    assert_eq!(progress.error, None, "{progress:?}");
    handle.cancel();
    pool.close(id);
}

// ── 发送侧：读盘与总大小 ──────────────────────────────────────────────────

#[test]
fn send_reads_the_file_and_knows_its_size_before_the_peer_speaks() {
    let dir = unique_dir("tx-size");
    let file = dir.join("out.bin");
    std::fs::write(&file, vec![7u8; 300]).expect("写文件");
    let (pool, id) = session();
    let handle = XferHandle::start(pool.clone(), id, XferKind::Send, XferProtocol::Xmodem, file)
        .expect("开始发送");
    let progress = handle.progress();
    assert_eq!(progress.total, Some(300), "{progress:?}");
    assert_eq!(progress.bytes, 0, "{progress:?}");
    assert!(!progress.finished, "{progress:?}");
    handle.cancel();
    pool.close(id);
}

#[test]
fn send_names_the_missing_file_instead_of_starting() {
    let (pool, id) = session();
    let Err(err) = XferHandle::start(
        pool.clone(),
        id,
        XferKind::Send,
        XferProtocol::Xmodem,
        PathBuf::from("/definitely/not/here.bin"),
    ) else {
        panic!("读不到文件应当当场拒绝");
    };
    assert!(err.contains("/definitely/not/here.bin"), "要点名路径：{err}");
    pool.close(id);
}

// ── 纯函数：对端给的名字一律过这一关 ──────────────────────────────────────

#[test]
fn resolve_name_flattens_or_refuses() {
    assert_eq!(xfer_run::resolve_name("../../etc/passwd").unwrap(), "passwd");
    assert_eq!(xfer_run::resolve_name("..\\..\\win\\x.dll").unwrap(), "x.dll");
    for raw in ["", "..", "/", "/abs/path", "C:\\Windows\\x", "c:x", "."] {
        assert!(xfer_run::resolve_name(raw).is_err(), "{raw:?} 应当被拒");
    }
}

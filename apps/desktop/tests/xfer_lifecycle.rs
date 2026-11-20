//! 传输的**生命周期**边界：`xfer_run.rs` 的正常路径由 `xfer_run.rs` 自己那 14 条
//! 覆盖，这里专打「人不会主动去按、但一定会遇到」的那几种情况。
//!
//! # 为什么单独一个测试目标
//!
//! 这些用例的前提不是「协议对不对」，而是「环境什么时候坏」：会话在握手中间断掉、
//! 子进程退出、对端沉默、目标目录只读、用户连点两次取消、发送方向失败。每一条
//! 都要求**先造出一个坏环境**再观察句柄的终态，与 `xfer_run.rs` 里那些「喂一段
//! 合法字节看它收得对不对」的用例是两套夹具，混在一个文件里会互相拖慢。
//!
//! 九条对应九种坏法，断言只认**句柄的对外状态**（`Progress` 的 `finished` /
//! `error` / `bytes` / `phase`），不去读内部字段——内部实现随便改，这九条应当
//! 一直成立。唯一的例外是最后两条条形断言：它们画真的一帧 egui，量两个底部面板
//! 的矩形，因为「传输行会不会盖住查找栏」只有真的布一次局才答得上来。
//!
//! `#[path]` 引入 `xfer_run.rs` 是因为 `term-desk` 是 bin crate，集成测试没法
//! `use` 它的内部模块；这也意味着它自带的 2 条单测会在这里再跑一遍，是有意的
//! 重复，不做特殊处理。
//!
//! # 这里为什么值得有
//!
//! 自测能证明「协议实现了」，证明不了「会话一断它就不会永远转圈」。要证伪
//! 「永远 `finished == false`」这类故障，只能在测试里把会话掐掉。
#![allow(dead_code)]

#[path = "../src/xfer_run.rs"]
mod xfer_run;

use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use term_core::session_runtime::SessionId;
use term_session::SessionPool;
use term_xfer::{Action, Config, Phase, Protocol, Sender};
use xfer_run::{XferHandle, XferKind, XferProtocol};

const SOH: u8 = 0x01;
const STX: u8 = 0x02;

fn unique_dir(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "desk-xver-{tag}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("时钟")
            .as_nanos()
    ));
    std::fs::create_dir_all(&dir).expect("建临时目录");
    dir
}

fn session_cmd(cmd: &str, ready: &str) -> (Arc<SessionPool>, SessionId) {
    let pool = Arc::new(SessionPool::new());
    let id = pool.open_exec(cmd, 80, 24).expect("起会话");
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let _ = pool.poll(id);
        if pool
            .snapshot(id)
            .is_some_and(|s| s.screen_text().contains(ready))
        {
            return (pool, id);
        }
        assert!(Instant::now() < deadline, "会话 10 秒还没就绪");
        std::thread::sleep(Duration::from_millis(10));
    }
}

fn session() -> (Arc<SessionPool>, SessionId) {
    session_cmd("stty raw -echo; echo READY; cat", "READY")
}

fn modem_stream_multi(protocol: Protocol, files: &[(&str, &[u8])]) -> Vec<u8> {
    let mut tx = Sender::new(Config::for_protocol(protocol));
    for (name, data) in files {
        tx.add_file(*name, data.to_vec()).expect("装进待发队列");
    }
    let mut wire = Vec::new();
    let mut first = true;
    let mut guard = 0;
    while !tx.phase().is_terminal() {
        guard += 1;
        assert!(guard < 100_000, "状态机没有收敛：{:?}", tx.phase());
        let byte = if first {
            first = false;
            b'C'
        } else {
            match tx.phase() {
                Phase::AwaitingDataRequest | Phase::AwaitingNextHeader => b'C',
                _ => 0x06,
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
    wire
}

fn frame_len(byte: u8) -> usize {
    match byte {
        SOH => 3 + 128 + 2,
        STX => 3 + 1024 + 2,
        _ => 1,
    }
}

fn wait_until(what: &str, secs: u64, mut done: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(secs);
    while !done() {
        assert!(Instant::now() < deadline, "{secs} 秒还没等到：{what}");
        std::thread::sleep(Duration::from_millis(10));
    }
}

// ── 1a. 会话被 close（tab 关 / Session 被 drop） ───────────────────────────

#[test]
fn session_closed_mid_handshake_reports_failure() {
    let dir = unique_dir("kill-close");
    let (pool, id) = session();
    let handle = XferHandle::start(
        pool.clone(),
        id,
        XferKind::Receive,
        XferProtocol::Xmodem,
        dir,
    )
    .expect("开始接收");
    wait_until("进入握手", 5, || handle.progress().phase != "待开始");
    assert!(pool.close(id), "关闭会话");

    wait_until("驱动报错", 5, || handle.progress().error.is_some());
    let p = handle.progress();
    println!("[1a] error={:?} finished={} phase={}", p.error, p.finished, p.phase);
    assert!(!p.finished, "会话死了不能算成功：{p:?}");
}

// ── 1b. 会话进程自己退出（SSH 断开 / 远端 shell 退出） ────────────────────

#[test]
fn session_process_exit_mid_transfer_reports_failure() {
    let dir = unique_dir("kill-exit");
    let (pool, id) = session_cmd("stty raw -echo; echo READY; sleep 0.4", "READY");
    let handle = XferHandle::start(
        pool.clone(),
        id,
        XferKind::Receive,
        XferProtocol::Xmodem,
        dir,
    )
    .expect("开始接收");

    wait_until("驱动报错", 8, || handle.progress().error.is_some());
    let p = handle.progress();
    println!("[1b] error={:?} finished={} phase={}", p.error, p.finished, p.phase);
    assert!(!p.finished, "进程退出不能算成功：{p:?}");
}

// ── 1c. 对端一直不说话：发送侧会不会永远 running ─────────────────────────

#[test]
fn send_with_a_silent_peer_never_leaves_running_state() {
    let dir = unique_dir("silent");
    let file = dir.join("out.bin");
    std::fs::write(&file, vec![7u8; 300]).expect("写文件");
    let (pool, id) = session();
    let handle = XferHandle::start(pool.clone(), id, XferKind::Send, XferProtocol::Xmodem, file)
        .expect("开始发送");

    // 对端（cat）永远不会喊 'C'。等远超「10 次重试 × 200ms」的时间。
    std::thread::sleep(Duration::from_secs(4));
    let p = handle.progress();
    println!(
        "[1c] 4s 后：phase={} finished={} error={:?} bytes={}",
        p.phase, p.finished, p.error, p.bytes
    );
    // 记录现状：仍然在跑、没有任何错误 —— 界面会永久转圈 + 120ms 重绘。
    assert!(!p.finished && p.error.is_none(), "现状应当是永久 running：{p:?}");
    handle.cancel();
    pool.close(id);
}

// ── 2. 写盘阻塞时 progress()/cancel() 会不会冻住 UI ──────────────────────

/// 观察 `window` 时间，判断 `progress()` 是否**已经**卡住。
///
/// 判据：窗口**最后** `quiet` 时长内一次返回都没有。
///
/// 为什么不是「一次 1.5 秒采样」：那要求驱动线程恰好在采样之前走到 `open()`。
/// 机器上有别的编译任务抢 CPU 时，探测会先正常返回几十次，于是「驱动还没开始」
/// 被误判成「写盘不会冻 UI」——2026-11-10 的严格门禁上就这么挂过一次（同一台
/// 机器空载连跑三次全过）。持续探测 + 静默窗口把这两种情况分开：驱动还没到
/// `open()` 时探测会不停返回，真握着锁阻塞时一次都不返回。
fn converges_to_blocked(
    rx: &std::sync::mpsc::Receiver<()>,
    window: Duration,
    quiet: Duration,
) -> bool {
    let deadline = Instant::now() + window;
    let mut last_return = Instant::now();
    loop {
        // 静默够了就立刻判「卡住」，不必等满窗口——正常机器上一秒内就收敛。
        if last_return.elapsed() >= quiet {
            return true;
        }
        if Instant::now() >= deadline {
            return false;
        }
        match rx.recv_timeout(Duration::from_millis(50)) {
            Ok(()) => last_return = Instant::now(),
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
                panic!("探测线程挂了，测不出任何结论")
            }
        }
    }
}

#[test]
fn the_blockage_probe_tells_a_returning_call_from_a_blocked_one() {
    use std::sync::atomic::{AtomicBool, Ordering};

    let quiet = Duration::from_millis(200);

    // 一直返回：窗口跑满也必须判「没卡住」。这条守的是修复本身——把判据改回
    // 「一次采样超时就算卡住」，这里会因为收到信号而挂。
    let (tx, rx) = std::sync::mpsc::channel();
    let stop = Arc::new(AtomicBool::new(false));
    let s = stop.clone();
    std::thread::spawn(move || {
        while !s.load(Ordering::Relaxed) {
            if tx.send(()).is_err() {
                return;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
    });
    let returning = converges_to_blocked(&rx, Duration::from_millis(600), quiet);
    stop.store(true, Ordering::Relaxed);
    assert!(!returning, "一直在返回的调用不能被判成卡住");

    // 一次都不返回（发送端不断开）：静默窗口一满就必须判「卡住」。
    let (_tx, silent) = std::sync::mpsc::channel::<()>();
    assert!(
        converges_to_blocked(&silent, Duration::from_millis(600), quiet),
        "静默的调用必须判成卡住"
    );
}

#[test]
fn a_blocking_open_freezes_progress_and_cancel() {
    let dir = unique_dir("fifo");
    let fifo = dir.join("fifo.bin");
    let st = std::process::Command::new("mkfifo")
        .arg(&fifo)
        .status()
        .expect("mkfifo");
    assert!(st.success(), "mkfifo 失败");

    let (pool, id) = session();
    let wire = modem_stream_multi(Protocol::Ymodem, &[("fifo.bin", &[1u8; 100])]);
    let handle = Arc::new(
        XferHandle::start(
            pool.clone(),
            id,
            XferKind::Receive,
            XferProtocol::Ymodem,
            dir.clone(),
        )
        .expect("开始接收"),
    );
    std::thread::sleep(Duration::from_millis(50));
    // 只喂头块：驱动会在 Sink::open 上打开 FIFO 写端（没有读者 → 永久阻塞）。
    let head = (3 + 128 + 2).min(wire.len());
    assert!(pool.send(id, &wire[..head]), "写不进会话");
    std::thread::sleep(Duration::from_millis(300));

    // 用**分离线程**探测：progress() 在 UI 线程每帧调用一次。
    // 探测线程原地循环调用，每次返回就发一个信号；主线程只看它有没有收敛到卡住。
    let (tx, rx) = std::sync::mpsc::channel();
    {
        let h = handle.clone();
        std::thread::spawn(move || loop {
            let _ = h.progress();
            if tx.send(()).is_err() {
                return;
            }
        });
    }
    let progress_blocked =
        converges_to_blocked(&rx, Duration::from_secs(8), Duration::from_millis(600));
    println!("[2] progress() 收敛到卡住？{progress_blocked}");

    // 再测 cancel（Drop 走同一条路：engine.lock() 之后拿 progress 锁）。
    let (tx2, rx2) = std::sync::mpsc::channel();
    {
        let h = handle.clone();
        std::thread::spawn(move || {
            h.cancel();
            let _ = tx2.send("cancel");
        });
    }
    let cancel_blocked = rx2.recv_timeout(Duration::from_millis(1500)).is_err();
    println!("[2] cancel() 1.5s 内返回？{}", !cancel_blocked);

    assert!(
        progress_blocked && cancel_blocked,
        "如果返回了，说明写盘不会冻 UI（现状与预期不同）"
    );
    // 阻塞线程永久留在 open() 上；测试进程退出即回收。
    std::mem::forget(handle);
}

// ── 3. 只读目标目录：失败要现出来，不许说成功 ────────────────────────────

#[test]
fn a_readonly_target_directory_fails_loudly_not_done() {
    use std::os::unix::fs::PermissionsExt;
    let dir = unique_dir("readonly");
    std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o500)).expect("chmod");
    // 确认真的写不进（root 会写进去，那就跳过）。
    let probe = dir.join(".probe");
    if std::fs::write(&probe, b"x").is_ok() {
        let _ = std::fs::remove_file(&probe);
        println!("[3] 当前身份能写只读目录，跳过");
        return;
    }

    let (pool, id) = session();
    let data = vec![9u8; 100];
    let wire = modem_stream_multi(Protocol::Ymodem, &[("a.bin", &data)]);
    let handle = XferHandle::start(
        pool.clone(),
        id,
        XferKind::Receive,
        XferProtocol::Ymodem,
        dir.clone(),
    )
    .expect("开始接收");
    std::thread::sleep(Duration::from_millis(50));
    let mut at = 0;
    while at < wire.len() {
        let end = (at + frame_len(wire[at])).min(wire.len());
        assert!(pool.send(id, &wire[at..end]), "写不进会话");
        at = end;
        std::thread::sleep(Duration::from_millis(2));
    }
    wait_until("驱动报错", 5, || handle.progress().error.is_some());
    let p = handle.progress();
    println!("[3] error={:?} finished={}", p.error, p.finished);
    assert!(!p.finished, "只读目录不能说成功：{p:?}");
    assert!(
        p.error.as_deref().is_some_and(|e| e.contains("建不了文件")),
        "{:?}",
        p.error
    );
    let _ = std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700));
    pool.close(id);
}

// ── 5. YMODEM 多文件：进度单调？total 是谁的 total？ ─────────────────────

#[test]
fn receive_progress_is_monotonic_across_a_batch() {
    let dir = unique_dir("batch");
    let a = vec![1u8; 100];
    let b = vec![2u8; 100];
    let wire = modem_stream_multi(Protocol::Ymodem, &[("a.bin", &a), ("b.bin", &b)]);
    let (pool, id) = session();
    let handle = XferHandle::start(
        pool.clone(),
        id,
        XferKind::Receive,
        XferProtocol::Ymodem,
        dir.clone(),
    )
    .expect("开始接收");
    std::thread::sleep(Duration::from_millis(50));

    let mut seen = Vec::new();
    let mut at = 0;
    while at < wire.len() {
        let end = (at + frame_len(wire[at])).min(wire.len());
        assert!(pool.send(id, &wire[at..end]), "写不进会话");
        at = end;
        std::thread::sleep(Duration::from_millis(2));
        let p = handle.progress();
        seen.push((p.bytes, p.total));
    }
    wait_until("传输结束", 10, || handle.progress().finished);
    let p = handle.progress();
    println!("[5] final bytes={} total={:?} files={:?}", p.bytes, p.total, {
        let mut v: Vec<_> = std::fs::read_dir(&dir)
            .unwrap()
            .filter_map(|e| e.ok())
            .map(|e| e.file_name().to_string_lossy().to_string())
            .collect();
        v.sort();
        v
    });
    let mut prev = 0u64;
    for (i, (bytes, _total)) in seen.iter().enumerate() {
        assert!(*bytes >= prev, "第 {i} 次采样倒退：{prev} → {bytes}");
        prev = *bytes;
    }
    // 结束那一刻的快照：bytes 是两文件累计，total 只是最后一个文件的大小。
    println!(
        "[5] 收尾快照：bytes={} total={:?}（界面会写「{} / {}」）",
        p.bytes,
        p.total,
        p.bytes,
        p.total.unwrap_or(0)
    );
    assert_eq!(p.bytes, 200, "两个文件相加");
    assert_eq!(p.total, Some(100), "total 只是最后一个文件的大小");
    assert!(p.bytes > p.total.unwrap(), "收尾时已收字节 > total");
    assert_eq!(std::fs::read(dir.join("a.bin")).unwrap(), a);
    assert_eq!(std::fs::read(dir.join("b.bin")).unwrap(), b);
    pool.close(id);
}

// ── 6. 取消幂等 + 握手阶段取消是 Canceled 不是 Failed ────────────────────

#[test]
fn cancel_is_idempotent_and_handshake_cancel_means_canceled() {
    let dir = unique_dir("cancel");
    let (pool, id) = session();
    let handle = XferHandle::start(
        pool.clone(),
        id,
        XferKind::Receive,
        XferProtocol::Xmodem,
        dir,
    )
    .expect("开始接收");
    wait_until("进入握手", 5, || handle.progress().phase == "握手");

    handle.cancel();
    let first = handle.progress();
    handle.cancel();
    handle.cancel();
    let after = handle.progress();
    println!("[6] first={:?} after={:?}", first.error, after.error);
    assert_eq!(after.error.as_deref(), Some("传输已取消"), "取消不是失败");
    assert!(!after.finished);
    assert_eq!(after.phase, "已中止");
    assert_eq!(first.bytes, 0, "握手阶段一个字节都没传");
    // 幂等：再按不会改写原因。
    assert_eq!(first.error, after.error);
    pool.close(id);
}

// ── 7. xfer_bar 与 find_bar 同时可见时是否互撞 ────────────────────────────

#[test]
fn the_two_bottom_bars_stack_without_overlapping() {
    let ctx = egui::Context::default();
    let mut rects = None;
    let input = egui::RawInput {
        screen_rect: Some(egui::Rect::from_min_size(
            egui::pos2(0.0, 0.0),
            egui::vec2(1280.0, 760.0),
        )),
        ..Default::default()
    };
    let _ = ctx.run(input, |ctx| {
        egui::CentralPanel::default().show(ctx, |ui| {
            ui.label("terminal");
        });
        // 与 ui/mod.rs 相同的声明顺序：先 xfer_bar，后 find_bar。
        let a = egui::TopBottomPanel::bottom("xfer_bar")
            .exact_height(20.0)
            .show(ctx, |ui| {
                ui.label("xfer");
            });
        let b = egui::TopBottomPanel::bottom("find_bar")
            .exact_height(24.0)
            .show(ctx, |ui| {
                ui.label("find");
            });
        rects = Some((a.response.rect, b.response.rect));
    });
    let (a, b) = rects.expect("要跑到一帧");
    println!("[7] xfer_bar={a:?}\n    find_bar={b:?}");
    // 不能用 `!a.intersects(b)`：egui 的 `Rect::intersects` 把「贴边」也算相交
    // （源码里是 `<=`），两个相邻面板会被判成重叠。这里量的是**重叠面积**——
    // 贴边的交集高度是 0，真叠上去才是正数。
    let overlap = a.intersect(b);
    assert!(
        overlap.height() <= 0.5,
        "两个底栏真的重叠了：{a:?} / {b:?}，交集 {overlap:?}"
    );
    assert!(
        b.max.y <= a.min.y + 0.5,
        "后声明的 find_bar 应当叠在 xfer_bar 之上：{a:?} / {b:?}"
    );
}


// ── 8. 发送方向失败也要撤销旁路 ──────────────────────────────────────────

#[test]
fn send_failure_still_detaches_raw_bypass() {
    let dir = unique_dir("tx-abort");
    let file = dir.join("out.bin");
    std::fs::write(&file, vec![7u8; 10]).expect("写文件");
    let (pool, id) = session();
    let handle = XferHandle::start(pool.clone(), id, XferKind::Send, XferProtocol::Xmodem, file)
        .expect("开始发送");
    std::thread::sleep(Duration::from_millis(100));
    // 对端发两个 CAN：发送侧报 Error(Cancelled) → Step::Failed（不是取消）。
    assert!(pool.send(id, &[0x18, 0x18]), "写不进会话");
    wait_until("驱动收尾", 5, || handle.progress().error.is_some());
    let p = handle.progress();
    println!("[8] error={:?} finished={}", p.error, p.finished);
    assert!(!p.finished, "失败不是成功");

    // 旁路必须已经撤销：往会话里打文本，VT 要能看到（cat 回显）。
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        assert!(pool.send(id, b"AFTER-XFER-FAIL\r\n"), "会话应当还活着");
        std::thread::sleep(Duration::from_millis(50));
        let _ = pool.poll(id);
        if pool
            .snapshot(id)
            .is_some_and(|s| s.screen_text().contains("AFTER-XFER-FAIL"))
        {
            break;
        }
        assert!(Instant::now() < deadline, "发送失败后 VT 没有恢复（旁路没交还）");
    }
    pool.close(id);
}

//! ZMODEM 两条菜单的**真端到端**验收：真 PTY、真 `sz` / `rz`、真落盘。
//!
//! # 为什么这一层不能省
//!
//! R5 的判据是「已接线 = **点了有效果**」，不是「`Action::parse` 返回 `Some`」。
//! 上一刀（软换行）就是命令接上了、画面上什么都没变，最后整刀回退。所以这一刀
//! 的证据必须是「文件真的过去了」：
//!
//! ```text
//! 发送方向：真实 rz（对端）  ← 我们的 XferHandle::start(Send, Zmodem)  ← 本地文件
//! 接收方向：真实 sz（对端）  → 我们的 XferHandle::start(Receive, Zmodem) → 落盘文件
//! ```
//!
//! 两个方向都不 mock 协议、不 mock 会话：会话是真的 `term_session::SessionPool`
//! 里的一个 PTY，对端是真的 `lrzsz`，字节走的是驱动层真正的
//! `attach_raw` 旁路 → 状态机 → 落盘。断言是**逐字节相同**，不是「字节数相同」。
//!
//! # 对端怎么起（为什么不是 `sleep` 赌时间）
//!
//! 对端进程必须在我们 `attach_raw` **之后**才开口，否则它开场那几个字节会被 VT
//! 吃掉。所以会话是一个等触发文件的 shell：
//!
//! ```text
//! cd <对端目录>; stty raw -echo; echo READY
//! while [ ! -e <触发文件> ]; do sleep 0.05; done
//! exec sz|rz --binary <参数> 2><stderr 文件>
//! ```
//!
//! - `stty raw -echo`：PTY 必须原样吞吐二进制，且**不许回显**——回显会把我们
//!   刚写进去的帧再送回来一份，状态机会当成对端噪声（甚至把它当应答）。
//! - `echo READY` 是就绪信号：它在 `stty` 之后打，我们轮询 VT 快照看到它才动手，
//!   于是「stty 生效了没有」不必靠时间赌（与 `tests/xfer_run.rs` 同一招）。
//! - 触发文件：`start()` 返回（旁路已生效）之后才让对端开口。
//! - `2>` 重定向 **必须有**：`lrzsz` 的进度与错误打在 stderr 上，而 PTY 会话里
//!   stderr 默认就是同一个终端——不重定向，那些文字会混进协议字节流，落盘内容
//!   就不再逐字节相同。重定向到文件后失败时还能读出来贴进断言消息。
//!
//! # 没有 lrzsz 时
//!
//! **静默跳过，exit 0**（照 `crates/term-zmodem/tests/interop.rs` 的先例）：
//! `sz`/`rz` 不是构建依赖。路径**不写死**：宿主是 `/opt/homebrew/bin/sz`，
//! 容器是 `/usr/bin/sz`，用 `command -v` 那种探测法，同一份测试两边都能跑。

// 集成测试只能看到 crate 的 **lib** 目标，而 desk 是纯 bin（没有 lib.rs）。
// 用 `#[path]` 把驱动模块直接编进本测试二进制：测的就是 `src/` 里那份源码。
#[path = "../src/xfer_run.rs"]
#[allow(dead_code)]
mod xfer_run;

use std::fs::{FileTimes, OpenOptions};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use term_core::session_runtime::{SessionId, SessionState};
use term_session::SessionPool;
use term_zmodem::Config as ZConfig;
use xfer_run::{XferHandle, XferKind, XferProtocol};

/// 一次互通最多等多久。真实 `lrzsz` 在 PTY 上是秒级完成的；这条线只是给
/// 「机器被别的构建挤爆」和容器冷启动留余量。
const TIMEOUT: Duration = Duration::from_secs(60);

// ── 夹具 ───────────────────────────────────────────────────────────────────

/// 每个用例一个独立目录树，互不干扰（也方便断言「到底多了什么文件」）。
fn unique_dir(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "desk-zmodem-{tag}-{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("时钟")
            .as_nanos()
    ));
    std::fs::create_dir_all(&dir).expect("建临时目录");
    dir
}

/// 探测外部程序，返回**绝对路径**（`which` 语义）。
///
/// 为什么不直接 `Command::new("sz")`：`PATH` 在宿主与容器里不一样（homebrew /
/// 系统包各一份），把解析结果拿出来打印，失败时也能一眼看到「跑到的是哪个
/// `sz`」。用 `command -v`：`sh`/`dash`/`bash` 都认，不额外依赖 `which`。
fn resolve_prog(prog: &str) -> Option<String> {
    let out = Command::new("sh")
        .arg("-c")
        .arg(format!("command -v {prog}"))
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let path = String::from_utf8_lossy(&out.stdout).trim().to_string();
    (!path.is_empty()).then_some(path)
}

/// 起一个「等触发文件再 exec 对端」的会话，并等它真的进入 raw 模式。
fn peer_session(command: &str) -> (Arc<SessionPool>, SessionId) {
    let pool = Arc::new(SessionPool::new());
    let id = pool.open_exec(command, 80, 24).expect("起会话");
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        // 会话提前死掉就立刻说清楚（`sh` 都起不来时干等 20 秒只会糊掉现场）。
        let state = pool.state(id).expect("会话应当在册");
        assert!(
            matches!(state, SessionState::Alive),
            "会话没进入运行态，READY 不会来：{state:?}\ncommand: {command}"
        );
        let _ = pool.poll(id);
        if pool
            .snapshot(id)
            .is_some_and(|s| s.screen_text().contains("READY"))
        {
            return (pool, id);
        }
        assert!(Instant::now() < deadline, "会话 20 秒还没就绪：{command}");
        std::thread::sleep(Duration::from_millis(10));
    }
}

/// 对端要跑的那一行 shell。`prog` 是**绝对路径**（[`resolve_prog`] 给的），
/// `args` 是它自己的参数，`tag` 只用来给 stderr 文件起名。
fn peer_command(dir: &Path, trigger: &Path, tag: &str, prog: &str, args: &str) -> String {
    format!(
        "cd '{dir}' && stty raw -echo && echo READY && \
         while [ ! -e '{trigger}' ]; do sleep 0.05; done && \
         exec '{prog}' {args} 2>'{dir}/{tag}.err'",
        dir = dir.display(),
        trigger = trigger.display(),
        prog = prog,
        args = args,
        tag = tag,
    )
}

/// 会转义、会打断文件信息解析的载荷：`ZDLE`(0x18) / `CR` / `XON` / `XOFF` /
/// `NUL` 混在普通字节里（同 `term-zmodem` 的互通测试）。纯 ASCII 的样本证明
/// 不了转义路径，也证明不了「落盘没做换行转换」。
fn payload(len: usize) -> Vec<u8> {
    (0..len)
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

/// 逐字节比对。先比长度再比内容：`assert_eq!(Vec<u8>, Vec<u8>)` 失败时会把
/// 3000 个数字全打出来，现场会被自己的日志糊掉。
fn assert_same_bytes(what: &str, landed: &[u8], source: &[u8]) {
    assert_eq!(
        landed.len(),
        source.len(),
        "{what}：字节数不同（落盘 {} vs 源 {}）",
        landed.len(),
        source.len()
    );
    assert!(landed == source, "{what}：内容不是逐字节相等");
}

/// FNV-1a 64。只为在 `--nocapture` 日志里留一行可比对的指纹（不引新依赖）。
fn fnv1a64(bytes: &[u8]) -> u64 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in bytes {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash
}

fn digest(bytes: &[u8]) -> String {
    format!("len={} fnv1a64={:016x}", bytes.len(), fnv1a64(bytes))
}

/// 等驱动的终态。`what` 是超时时要说的话。
fn wait_until(what: &str, mut done: impl FnMut() -> bool) {
    let deadline = Instant::now() + TIMEOUT;
    while !done() {
        assert!(Instant::now() < deadline, "{} 秒还没等到：{what}", TIMEOUT.as_secs());
        std::thread::sleep(Duration::from_millis(10));
    }
}

/// 对端 stderr 的内容（失败时贴进断言消息）。
fn tail(path: &Path) -> String {
    match std::fs::read(path) {
        Ok(bytes) => String::from_utf8_lossy(&bytes).into_owned(),
        Err(err) => format!("（读不到对端 stderr：{err}）"),
    }
}

fn touch(path: &Path) {
    std::fs::write(path, b"go").expect("写触发文件");
}

/// 落盘文件的 `modified`（Unix 秒）。mtime 用例只按秒比对，容差 1 秒。
fn modified_secs(path: &Path) -> u64 {
    std::fs::metadata(path)
        .expect("读落盘文件元数据")
        .modified()
        .expect("读 modified 时间")
        .duration_since(UNIX_EPOCH)
        .expect("时间在 epoch 之后")
        .as_secs()
}

// ── 1. 接收：真实 sz → 我们 ────────────────────────────────────────────────

#[test]
fn receive_from_real_sz_lands_the_file_byte_for_byte() {
    let Some(sz) = resolve_prog("sz") else {
        eprintln!("跳过：本机没有 sz（容器里镜像自带 lrzsz，宿主 brew install lrzsz）");
        return;
    };
    eprintln!("· 对端 sz = {sz}");

    let root = unique_dir("rx");
    let peer = root.join("peer");
    let target = root.join("target");
    let trigger = root.join("go");
    std::fs::create_dir_all(&peer).expect("建对端目录");
    std::fs::create_dir_all(&target).expect("建接收目录");

    let data = payload(3000);
    std::fs::write(peer.join("payload.bin"), &data).expect("写源文件");
    eprintln!("· 源文件 payload.bin {}", digest(&data));

    let command = peer_command(&peer, &trigger, "sz", &sz, "--binary payload.bin");
    let (pool, id) = peer_session(&command);

    // 1) 先接管字节（`start` 返回即旁路生效），2) 再放对端开口。
    let handle = XferHandle::start(
        pool.clone(),
        id,
        XferKind::Receive,
        XferProtocol::Zmodem,
        target.clone(),
    )
    .expect("开始接收");
    touch(&trigger);

    wait_until("接收结束", || {
        let progress = handle.progress();
        progress.finished || progress.error.is_some()
    });

    let progress = handle.progress();
    let stderr = tail(&peer.join("sz.err"));
    assert_eq!(progress.error, None, "{progress:?}\nsz stderr：{stderr}");
    assert!(progress.finished, "协议没有正常收尾：{progress:?}\nsz stderr：{stderr}");
    // ZFILE 带长度：接收侧的 `total` 必须是源文件的长度，不是猜的。
    assert_eq!(progress.total, Some(data.len() as u64), "{progress:?}");
    assert_eq!(progress.bytes, data.len() as u64, "{progress:?}");

    // 文件名是**对端给的**，必须原样落成 payload.bin（不是驱动起的默认名）。
    let landed_path = target.join("payload.bin");
    assert!(
        landed_path.is_file(),
        "落盘文件名不对，目录里是：{:?}",
        std::fs::read_dir(&target)
            .expect("读目录")
            .filter_map(|e| e.ok())
            .map(|e| e.file_name())
            .collect::<Vec<_>>()
    );
    let landed = std::fs::read(&landed_path).expect("读落盘文件");
    assert_same_bytes("接收方向", &landed, &data);

    eprintln!(
        "· 落盘 {}{}（源 {}）",
        landed_path.display(),
        digest(&landed),
        digest(&data)
    );

    pool.close(id);
}

// ── 2. 发送：我们 → 真实 rz ────────────────────────────────────────────────

#[test]
fn send_to_real_rz_lands_the_file_byte_for_byte() {
    let Some(rz) = resolve_prog("rz") else {
        eprintln!("跳过：本机没有 rz（容器里镜像自带 lrzsz，宿主 brew install lrzsz）");
        return;
    };
    eprintln!("· 对端 rz = {rz}");

    let root = unique_dir("tx");
    let peer = root.join("peer");
    let local = root.join("local");
    let trigger = root.join("go");
    std::fs::create_dir_all(&peer).expect("建对端目录");
    std::fs::create_dir_all(&local).expect("建本地目录");

    let data = payload(3000);
    let source = local.join("payload.bin");
    std::fs::write(&source, &data).expect("写源文件");
    eprintln!("· 源文件 payload.bin {}", digest(&data));

    let command = peer_command(&peer, &trigger, "rz", &rz, "--binary");
    let (pool, id) = peer_session(&command);

    let handle = XferHandle::start(
        pool.clone(),
        id,
        XferKind::Send,
        XferProtocol::Zmodem,
        source.clone(),
    )
    .expect("开始发送");
    // 发送侧开跑就知道了总长（本地文件已在内存里）。
    assert_eq!(handle.progress().total, Some(data.len() as u64));
    touch(&trigger);

    wait_until("发送结束", || {
        let progress = handle.progress();
        progress.finished || progress.error.is_some()
    });

    let progress = handle.progress();
    let stderr = tail(&peer.join("rz.err"));
    assert_eq!(progress.error, None, "{progress:?}\nrz stderr：{stderr}");
    assert!(progress.finished, "协议没有正常收尾：{progress:?}\nrz stderr：{stderr}");
    assert_eq!(progress.bytes, data.len() as u64, "{progress:?}");

    // **这就是全刀的核心断言**：对端目录里真的多了一个 payload.bin，内容相同。
    let landed_path = peer.join("payload.bin");
    assert!(
        landed_path.is_file(),
        "对端目录里没有 payload.bin：{:?}\nrz stderr：{stderr}",
        std::fs::read_dir(&peer)
            .expect("读目录")
            .filter_map(|e| e.ok())
            .map(|e| e.file_name())
            .collect::<Vec<_>>()
    );
    let landed = std::fs::read(&landed_path).expect("读对端落盘文件");
    assert_same_bytes("发送方向", &landed, &data);

    eprintln!(
        "· 对端落盘 {}{}（源 {}）",
        landed_path.display(),
        digest(&landed),
        digest(&data)
    );

    pool.close(id);
}

// ── 3. W6-C4：四项增量的产品侧验收 ─────────────────────────────────────────
//
// 前两条（开关真值、落盘展开）是**纯函数**用例，不依赖 lrzsz；第三条（mtime）
// 直接拿驱动的 `Sink` 验，同样不需要外部程序。后面两条才是真 PTY + 真 sz/rz
// 的端到端：它们覆盖「多个文件经同一 Sender 发出」与「非零 mtime 一路到落盘」。

/// ① 开关真值：驱动把内核的两项 opt-in 增量都打开了，且没有顺手改别的参数。
#[test]
fn zmodem_config_enables_batch_streaming_and_keeps_retry_policy() {
    let cfg = xfer_run::zmodem_config();
    assert!(cfg.batch, "驱动必须打开内核的多文件批处理（Config::batch）");
    assert!(cfg.streaming, "驱动必须打开 ZCRCG 连发（Config::streaming）");
    assert_eq!(cfg.max_retries, 40, "既定的 40 次重试策略不能被覆盖");

    let defaults = ZConfig::default();
    assert!(!defaults.batch && !defaults.streaming, "内核默认仍是 opt-in 关闭");
    assert_eq!(cfg.header, defaults.header, "这一刀只动三项：帧头不变");
    assert_eq!(cfg.block_size, defaults.block_size, "这一刀只动三项：块长不变");
}

/// ②a 单文件路径：名字、内容、数量都与改动前一致（原来那一次 `add_file`）。
#[test]
fn a_single_file_path_keeps_its_name_content_and_count() {
    let root = unique_dir("collect-single");
    let file = root.join("only.bin");
    let data = payload(321);
    std::fs::write(&file, &data).expect("写源文件");

    let files = xfer_run::collect_send_files(&file).expect("展开单文件");
    assert_eq!(files.len(), 1, "单文件只能排进一个 add_file");
    assert_eq!(files[0].name, "only.bin");
    assert_eq!(files[0].data, data);
    assert!(files[0].mtime.is_some(), "本地 mtime 应当读到（非零）");
}

/// ②b 目录展开：`read_dir` 顺序不稳定，必须排序；子目录明确跳过。
#[test]
fn a_directory_expands_to_a_sorted_batch_and_skips_subdirectories() {
    let root = unique_dir("collect-dir");
    let dir = root.join("batch");
    std::fs::create_dir_all(dir.join("nested")).expect("建子目录");
    // 反序写入：断言的是排序结果，不是 read_dir 的返回顺序。
    std::fs::write(dir.join("c.bin"), b"CCC").expect("写 c");
    std::fs::write(dir.join("a.bin"), b"AAA").expect("写 a");
    std::fs::write(dir.join("nested/inside.bin"), b"nested").expect("写子目录里的文件");
    std::fs::write(dir.join("b.bin"), b"BBB").expect("写 b");

    let files = xfer_run::collect_send_files(&dir).expect("展开目录");
    let names: Vec<&str> = files.iter().map(|f| f.name.as_str()).collect();
    assert_eq!(names, ["a.bin", "b.bin", "c.bin"], "字典序 + 跳过子目录");
    assert_eq!(files[0].data, b"AAA");
    assert_eq!(files[1].data, b"BBB");
    assert_eq!(files[2].data, b"CCC");
}

/// ②c `;` 分隔的多路径；重名必须当场拒绝（否则同批文件会互相覆盖）。
#[test]
fn semicolon_separated_paths_expand_and_duplicate_names_are_refused() {
    let root = unique_dir("collect-multi");
    let a = root.join("a.bin");
    let b = root.join("b.bin");
    std::fs::write(&a, b"AAA").expect("写 a");
    std::fs::write(&b, b"BBB").expect("写 b");

    let joined = PathBuf::from(format!("{};{}", a.display(), b.display()));
    let files = xfer_run::collect_send_files(&joined).expect("展开多路径");
    let names: Vec<&str> = files.iter().map(|f| f.name.as_str()).collect();
    assert_eq!(names, ["a.bin", "b.bin"]);
    assert_eq!(files[0].data, b"AAA");
    assert_eq!(files[1].data, b"BBB");

    let other = root.join("other");
    std::fs::create_dir_all(&other).expect("建目录");
    let dup = other.join("a.bin");
    std::fs::write(&dup, b"DUP").expect("写重名文件");
    let joined = PathBuf::from(format!("{};{}", a.display(), dup.display()));
    let err = xfer_run::collect_send_files(&joined).expect_err("重名必须拒绝");
    assert!(
        err.to_string().contains("重名"),
        "错误里要说清原因：{err}"
    );
}

/// `drive()` 把取样到的元数据挂到唯一的 `FileStart` 上；一次 `feed` 里出现多个
/// `FileStart` 时宁可整批不带 mtime，也不能把文件 B 的时间串到文件 A 上。
#[test]
fn drive_attaches_meta_to_a_single_file_start_and_drops_it_when_ambiguous() {
    use term_xfer::Action;

    let meta = xfer_run::FileMeta {
        mtime: Some(1_700_000_000),
        mode: Some(0o644),
    };
    let one = xfer_run::drive(
        vec![
            Action::FileStart {
                name: "a.bin".to_string(),
                size: Some(5),
            },
            Action::Data(vec![1, 2, 3, 4, 5]),
            Action::FileDone,
        ],
        meta,
    );
    assert_eq!(one.len(), 3);
    match &one[0] {
        xfer_run::DriveAction::FileStart {
            name,
            size,
            meta: got,
        } => {
            assert_eq!(name, "a.bin");
            assert_eq!(*size, Some(5));
            assert_eq!(*got, meta, "唯一的 FileStart 必须带上取样到的元数据");
        }
        other => panic!("第一个动作应是 FileStart，实际 {other:?}"),
    }
    assert!(matches!(one[1], xfer_run::DriveAction::Data(_)));
    assert!(matches!(one[2], xfer_run::DriveAction::FileDone));

    let two = xfer_run::drive(
        vec![
            Action::FileStart {
                name: "a.bin".to_string(),
                size: Some(1),
            },
            Action::FileStart {
                name: "b.bin".to_string(),
                size: Some(1),
            },
        ],
        meta,
    );
    assert_eq!(two.len(), 2);
    for action in &two {
        match action {
            xfer_run::DriveAction::FileStart { meta: got, .. } => {
                assert_eq!(*got, xfer_run::FileMeta::default(), "歧义时不能串味");
            }
            other => panic!("这里只该有 FileStart，实际 {other:?}"),
        }
    }
}

/// ③a 收尾落盘：`Sink::close` 用标准库 `FileTimes` 把对端 mtime 写回文件；
/// `None` / `Some(0)` 都不动时间（0 是「没给时间」的哨兵，不该设成 1970）。
#[test]
fn sink_applies_peer_mtime_but_leaves_time_alone_for_none_or_zero() {
    let root = unique_dir("sink-mtime");
    let pool = SessionPool::new();
    let want = 1_700_000_000u32;
    {
        let mut sink = xfer_run::Sink {
            pool: &pool,
            session: 0,
            dir: Some(root.clone()),
            file: None,
            mtime: None,
            mode: None,
        };
        sink.open(
            "with.bin",
            xfer_run::FileMeta {
                mtime: Some(want),
                mode: Some(0o644),
            },
        )
        .expect("开文件");
        sink.write(b"payload").expect("写内容");
        sink.close().expect("收尾");
    }
    let landed = root.join("with.bin");
    assert_eq!(std::fs::read(&landed).expect("读落盘"), b"payload");
    let got = modified_secs(&landed);
    assert!(
        (got as i64 - i64::from(want)).abs() <= 1,
        "mtime 应落在 {want}（容差 1 秒），实际 {got}"
    );

    for (name, mtime) in [("none.bin", None), ("zero.bin", Some(0))] {
        let mut sink = xfer_run::Sink {
            pool: &pool,
            session: 0,
            dir: Some(root.clone()),
            file: None,
            mtime: None,
            mode: None,
        };
        sink.open(name, xfer_run::FileMeta { mtime, mode: None })
            .expect("开文件");
        sink.write(b"x").expect("写内容");
        let path = root.join(name);
        let before = modified_secs(&path);
        sink.close().expect("收尾");
        let after = modified_secs(&path);
        assert!(
            (after as i64 - before as i64).abs() <= 1,
            "{name} 不该被改时间：{before} → {after}"
        );
        assert!(after > 1_600_000_000, "{name} 不该被设成 1970：{after}");
    }
}

/// ③b 真端到端：本地目录里的两个文件经**同一台 `ZSender`** 发给真 `rz`，
/// 对端必须落盘成两个文件、内容逐字节相同。
#[test]
fn send_a_directory_as_a_zmodem_batch_lands_every_file_byte_for_byte() {
    let Some(rz) = resolve_prog("rz") else {
        eprintln!("跳过：本机没有 rz（等价的内核闭环见 term-zmodem/tests/w6c4_batch_meta.rs）");
        return;
    };
    eprintln!("· 对端 rz = {rz}");

    let root = unique_dir("batch-tx");
    let peer = root.join("peer");
    let local = root.join("local");
    let trigger = root.join("go");
    std::fs::create_dir_all(&peer).expect("建对端目录");
    std::fs::create_dir_all(&local).expect("建本地目录");

    let first = payload(1500);
    let second = payload(2300);
    std::fs::write(local.join("a.bin"), &first).expect("写 a.bin");
    std::fs::write(local.join("b.bin"), &second).expect("写 b.bin");
    eprintln!("· 待发 a.bin {} / b.bin {}", digest(&first), digest(&second));

    let command = peer_command(&peer, &trigger, "rz", &rz, "--binary");
    let (pool, id) = peer_session(&command);

    // `target` 是目录：`XferHandle::start` 展开成两个 `add_file`，排在同一台
    // `ZSender` 上（batch=true），中间由 `ZEOF → ZRINIT` 划出 FileDone 边界。
    let handle = XferHandle::start(
        pool.clone(),
        id,
        XferKind::Send,
        XferProtocol::Zmodem,
        local.clone(),
    )
    .expect("开始批处理发送");
    assert_eq!(
        handle.progress().total,
        Some((first.len() + second.len()) as u64),
        "总长应是两个文件之和"
    );
    touch(&trigger);

    wait_until("批处理发送结束", || {
        let progress = handle.progress();
        progress.finished || progress.error.is_some()
    });
    let progress = handle.progress();
    let stderr = tail(&peer.join("rz.err"));
    assert_eq!(progress.error, None, "{progress:?}\nrz stderr：{stderr}");
    assert!(
        progress.finished,
        "协议没有正常收尾：{progress:?}\nrz stderr：{stderr}"
    );

    for (name, data) in [("a.bin", &first), ("b.bin", &second)] {
        let landed_path = peer.join(name);
        assert!(
            landed_path.is_file(),
            "对端目录里没有 {name}：{:?}\nrz stderr：{stderr}",
            std::fs::read_dir(&peer)
                .expect("读目录")
                .filter_map(|e| e.ok())
                .map(|e| e.file_name())
                .collect::<Vec<_>>()
        );
        let landed = std::fs::read(&landed_path).expect("读对端落盘文件");
        assert_same_bytes(name, &landed, data);
        eprintln!("· 对端落盘 {}{}", landed_path.display(), digest(&landed));
    }

    pool.close(id);
}

/// ③c 真端到端：把源文件的 mtime 设成一个明确的非零值，真 `sz` 把它放进
/// `ZFILE`；驱动从 `Receiver::file_mtime()` 取出来，在 `Sink::close` 落盘。
#[test]
fn receive_from_real_sz_preserves_the_source_mtime() {
    let Some(sz) = resolve_prog("sz") else {
        eprintln!("跳过：本机没有 sz（等价的内核闭环见 term-zmodem/tests/w6c4_batch_meta.rs）");
        return;
    };
    eprintln!("· 对端 sz = {sz}");

    let root = unique_dir("rx-mtime");
    let peer = root.join("peer");
    let target = root.join("target");
    let trigger = root.join("go");
    std::fs::create_dir_all(&peer).expect("建对端目录");
    std::fs::create_dir_all(&target).expect("建接收目录");

    let data = payload(777);
    let source = peer.join("stamped.bin");
    std::fs::write(&source, &data).expect("写源文件");
    let want = 1_700_000_000u32;
    let handle_for_times = OpenOptions::new()
        .write(true)
        .open(&source)
        .expect("打开源文件设时间");
    handle_for_times
        .set_times(FileTimes::new().set_modified(
            UNIX_EPOCH + Duration::from_secs(u64::from(want)),
        ))
        .expect("设置源 mtime");
    drop(handle_for_times);
    assert_eq!(modified_secs(&source), u64::from(want), "源 mtime 要真的设上");

    let command = peer_command(&peer, &trigger, "sz", &sz, "--binary stamped.bin");
    let (pool, id) = peer_session(&command);

    let handle = XferHandle::start(
        pool.clone(),
        id,
        XferKind::Receive,
        XferProtocol::Zmodem,
        target.clone(),
    )
    .expect("开始接收");
    touch(&trigger);

    wait_until("mtime 接收结束", || {
        let progress = handle.progress();
        progress.finished || progress.error.is_some()
    });
    let progress = handle.progress();
    let stderr = tail(&peer.join("sz.err"));
    assert_eq!(progress.error, None, "{progress:?}\nsz stderr：{stderr}");
    assert!(
        progress.finished,
        "协议没有正常收尾：{progress:?}\nsz stderr：{stderr}"
    );

    let landed_path = target.join("stamped.bin");
    assert!(
        landed_path.is_file(),
        "落盘文件名不对，目录里是：{:?}\nsz stderr：{stderr}",
        std::fs::read_dir(&target)
            .expect("读目录")
            .filter_map(|e| e.ok())
            .map(|e| e.file_name())
            .collect::<Vec<_>>()
    );
    let landed = std::fs::read(&landed_path).expect("读落盘文件");
    assert_same_bytes("mtime 用例内容", &landed, &data);
    let got = modified_secs(&landed_path);
    assert!(
        (got as i64 - i64::from(want)).abs() <= 1,
        "落盘 mtime 应与源一致（容差 1 秒）：want={want} got={got}\nsz stderr：{stderr}"
    );
    eprintln!("· 落盘 {} mtime={}（源 {}）", landed_path.display(), got, want);

    pool.close(id);
}

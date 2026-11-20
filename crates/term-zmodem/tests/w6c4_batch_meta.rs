//! W6-C4 的内核侧闭环证据：多文件批处理 + `ZCRCG` 连发 + 非零 mtime。
//!
//! 全在内存字节流里跑，**不依赖 `lrzsz` / PTY**。产品侧（`apps/desktop`）的真端到端
//! 用例在 `apps/desktop/tests/xfer_zmodem.rs`；这里负责钉住「即便没有外部程序，
//! 两项开关打开后内核能不能把两个文件、两个不同的 mtime 完整送过去」。
//!
//! 驱动侧 `zmodem_config()` 的三项策略在这里对齐成 [`driver_config`]：改动驱动
//! 参数却忘了同步这里，下面的形状断言会立刻红。

mod common;

use common::*;
use term_zmodem::{Action, Config, HeaderKind, Receiver, Sender};

/// 与 `apps/desktop/src/xfer_run.rs::zmodem_config()` 同构的协议策略。
fn driver_config() -> Config {
    Config::default()
        .with_max_retries(40)
        .with_batch(true)
        .with_streaming(true)
}

/// 把一组 `(名字, 内容, mtime)` 用**同一台 `Sender`** 发出，闭环到一台 `Receiver`。
///
/// 返回 `(接收侧 sink, 每个 FileStart 时刻取样到的 mtime)`。取样写在动作产生的
/// 那一刻（内核 `on_file_info` 先设 `mtime` 再 push `FileStart`），正是产品侧
/// `Engine::file_meta()` 的语义。
fn loopback_with_mtimes(
    cfg: Config,
    files: &[(&str, Vec<u8>, u32)],
) -> (Sink, Vec<Option<u32>>) {
    let mut tx = Sender::new(cfg);
    for (name, data, mtime) in files {
        tx.add_file_with_time(*name, data.clone(), *mtime)
            .expect("批处理应当接受排进队列的文件");
    }
    let mut rx = Receiver::new(cfg);

    let mut tx_sink = Sink::default();
    let mut rx_sink = Sink::default();
    tx_sink.apply(tx.start());
    rx_sink.apply(rx.start());

    let mut mtimes = Vec::new();
    let mut steps = 0;
    loop {
        steps += 1;
        assert!(steps < 10_000, "闭环未收敛（疑似重传风暴）");
        let mut moved = false;

        let to_rx = tx_sink.take_out();
        if !to_rx.is_empty() {
            let actions = rx.feed(&to_rx);
            for action in &actions {
                if matches!(action, Action::FileStart { .. }) {
                    mtimes.push(rx.file_mtime());
                }
            }
            rx_sink.apply(actions);
            moved = true;
        }
        let to_tx = rx_sink.take_out();
        if !to_tx.is_empty() {
            tx_sink.apply(tx.feed(&to_tx));
            moved = true;
        }
        if !moved {
            break;
        }
    }
    assert!(tx_sink.errors.is_empty(), "发送方报错：{:?}", tx_sink.errors);
    assert!(rx_sink.errors.is_empty(), "接收方报错：{:?}", rx_sink.errors);
    (rx_sink, mtimes)
}

#[test]
fn the_product_config_really_turns_on_both_opt_in_increments() {
    let defaults = Config::default();
    assert!(!defaults.batch, "内核默认必须是 opt-in 关闭");
    assert!(!defaults.streaming, "内核默认必须是 opt-in 关闭");
    assert_eq!(defaults.max_retries, 10);

    let cfg = driver_config();
    assert!(cfg.batch, "批处理开关必须打开");
    assert!(cfg.streaming, "ZCRCG 连发开关必须打开");
    assert_eq!(cfg.max_retries, 40, "驱动侧重试策略保持 40");
    // 只动这三项：帧头与块长仍是内核默认值。
    assert_eq!(cfg.header, defaults.header);
    assert_eq!(cfg.block_size, defaults.block_size);
}

#[test]
fn batch_plus_streaming_delivers_two_files_byte_exact_with_distinct_mtimes() {
    // 块长 300 + 两份都超过一块的载荷：既走「多块连发」，也走「文件间 ZEOF →
    // ZRINIT → 下一个 ZFILE」的批处理器节奏。
    let cfg = driver_config()
        .with_header(HeaderKind::Hex)
        .with_block_size(300);
    let first: Vec<u8> = (0..1400u32).map(|i| (i % 251) as u8).collect();
    let second: Vec<u8> = (0..900u32)
        .map(|i| match i % 7 {
            0 => 0x18,
            1 => 0x0D,
            _ => (i % 239) as u8,
        })
        .collect();
    let mtime_first = 1_700_000_000u32;
    let mtime_second = 1_650_000_123u32;

    let (sink, mtimes) = loopback_with_mtimes(
        cfg,
        &[
            ("a.bin", first.clone(), mtime_first),
            ("b.bin", second.clone(), mtime_second),
        ],
    );

    assert!(sink.done, "ZFIN 之后整体结束");
    assert_eq!(sink.file_done, 2, "每个文件恰好一次 FileDone 边界");
    assert_eq!(sink.files.len(), 2, "两个文件各一次 FileStart");
    assert_eq!(sink.files[0].name, "a.bin");
    assert_eq!(sink.files[0].data, first, "第一个文件逐字节一致");
    assert_eq!(sink.files[1].name, "b.bin");
    assert_eq!(sink.files[1].data, second, "第二个文件逐字节一致");
    assert_eq!(sink.files[0].size, Some(first.len() as u64));
    assert_eq!(sink.files[1].size, Some(second.len() as u64));
    assert_eq!(
        mtimes,
        vec![Some(mtime_first), Some(mtime_second)],
        "每个 FileStart 都应取到**该文件自己的**非零 mtime，不能串味"
    );
}

#[test]
fn batch_off_still_rejects_the_second_file() {
    // 对照：关掉 batch 后同一台 Sender 排不进第二个文件。产品侧之所以打开
    // `with_batch(true)`，正是为了跨过这道门槛。
    let mut tx = Sender::new(driver_config().with_batch(false));
    tx.add_file_with_time("a.bin", b"aaa".to_vec(), 1_700_000_000)
        .unwrap();
    assert!(
        tx.add_file_with_time("b.bin", b"bbb".to_vec(), 1_600_000_000)
            .is_err(),
        "batch=false 时第二个文件必须被拒绝"
    );
}

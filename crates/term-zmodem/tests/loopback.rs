//! 收发闭环：Sender 的输出直接喂给 Receiver，收到的文件必须与原文件逐字节相同。

mod common;

use common::*;
use term_zmodem::{Config, HeaderKind, Phase};

fn assert_loopback_as(tx_cfg: Config, rx_cfg: Config, name: &str, data: &[u8], cut: Cut) {
    let (tx, rx) = loopback_as(tx_cfg, rx_cfg, name, data, cut);
    assert!(tx.done, "发送方没有正常结束（{tx_cfg:?} / {cut:?}）");
    assert!(rx.done, "接收方没有正常结束（{rx_cfg:?} / {cut:?}）");
    assert!(tx.errors.is_empty(), "发送方报错：{:?}", tx.errors);
    assert!(rx.errors.is_empty(), "接收方报错：{:?}", rx.errors);
    assert_eq!(rx.file_done, 1, "FileDone 应当恰好一次");
    let file = rx.single();
    assert_eq!(file.name, name);
    assert_eq!(file.size, Some(data.len() as u64));
    assert_eq!(file.data, data, "内容不一致（{cut:?}）");
}

fn assert_loopback(cfg: Config, name: &str, data: &[u8], cut: Cut) {
    assert_loopback_as(cfg, cfg, name, data, cut);
}

#[test]
fn loopback_small_file() {
    assert_loopback(hex(), "hello.txt", b"hello", Cut::Whole);
}

#[test]
fn loopback_zero_length_file() {
    assert_loopback(hex(), "empty.bin", b"", Cut::Whole);
}

#[test]
fn loopback_zero_length_file_one_byte_cut() {
    assert_loopback(hex(), "empty.bin", b"", Cut::OneByte);
}

#[test]
fn loopback_exactly_one_block() {
    let data = vec![0x33u8; 1024];
    assert_loopback(hex(), "block.bin", &data, Cut::Whole);
}

#[test]
fn loopback_multi_block() {
    let data: Vec<u8> = (0..3000u32).map(|i| (i % 251) as u8).collect();
    assert_loopback(hex(), "big.bin", &data, Cut::Whole);
}

#[test]
fn loopback_multi_block_one_byte_cut() {
    // 600 字节、默认 1024 块长：一次一块，但整条流逐字节喂。
    let data: Vec<u8> = (0..600u32).map(|i| (i % 239) as u8).collect();
    assert_loopback(hex(), "small.bin", &data, Cut::OneByte);
}

#[test]
fn loopback_random_chunks() {
    let data: Vec<u8> = (0..2049u32).map(|i| (i % 249) as u8).collect();
    for seed in [11u64, 0xFEED, 0x0BAD_C0DE, 42] {
        assert_loopback(hex(), "r.bin", &data, Cut::Random(seed));
    }
}

#[test]
fn loopback_utf8_filename() {
    assert_loopback(
        hex(),
        "固件 v1.2/说明.txt",
        "中文内容，需要按 UTF-8 原样透传。".as_bytes(),
        Cut::Whole,
    );
}

#[test]
fn loopback_payload_full_of_escapes() {
    // 0x18/0x10/0x13/0x11/0x0D 全都出现，且连续 0x18 是最刁钻的情况。
    let data: Vec<u8> = vec![
        0x18, 0x18, 0x18, 0x13, 0x11, 0x10, 0x0D, 0x41, 0x18, 0x0D, 0x18, 0x18, 0x13, 0x00, 0xFF,
    ];
    assert_loopback(hex(), "esc.bin", &data, Cut::Whole);
    assert_loopback(hex(), "esc.bin", &data, Cut::OneByte);
    // 同样的载荷在三种帧头下各跑一遍：二进制帧头的类型/头数据转义路径也一起过。
    assert_loopback(bin(), "esc.bin", &data, Cut::Whole);
    assert_loopback(bin32(), "esc.bin", &data, Cut::OneByte);
}

#[test]
fn loopback_tiny_blocks() {
    // 块长 1：每一块都要走一次「ZDATA → ZACK」往返，最容易把偏移算错。
    let data = vec![1u8, 2, 3, 4, 5, 6, 7, 8, 9, 10];
    let cfg = hex().with_block_size(1);
    assert_loopback(cfg, "tiny.bin", &data, Cut::Whole);
}

#[test]
fn loopback_block_size_with_remainder() {
    // 1000 字节 / 300 字节块 = 3 整块 + 100 字节尾块。
    let data: Vec<u8> = (0..1000u32).map(|i| (i % 253) as u8).collect();
    let cfg = hex().with_block_size(300);
    assert_loopback(cfg, "odd.bin", &data, Cut::Random(0xABCD));
}

// ── 三种帧头的闭环 ─────────────────────────────────────────────────────────

fn mixed_payload() -> Vec<u8> {
    (0..2500u32)
        .map(|i| match i % 5 {
            0 => 0x18,
            1 => 0x0D,
            2 => 0x10,
            3 => 0x11,
            _ => (i % 251) as u8,
        })
        .collect()
}

#[test]
fn loopback_bin32_is_the_default() {
    assert_eq!(Config::default().header, HeaderKind::Bin32);
    let data = mixed_payload();
    assert_loopback(bin32(), "b32.bin", &data, Cut::Whole);
    assert_loopback(bin32(), "b32.bin", &data, Cut::OneByte);
}

#[test]
fn loopback_bin16() {
    let data = mixed_payload();
    assert_loopback(bin(), "b16.bin", &data, Cut::Whole);
    assert_loopback(bin(), "b16.bin", &data, Cut::Random(7));
}

#[test]
fn loopback_all_header_kinds() {
    let data = mixed_payload();
    for kind in [HeaderKind::Hex, HeaderKind::Bin, HeaderKind::Bin32] {
        let cfg = Config::default().with_header(kind);
        assert_loopback(cfg, "kinds.bin", &data, Cut::Whole);
    }
}

#[test]
fn loopback_cross_format() {
    // 发送方用一种帧头、接收方用另一种：解析不依赖配置，两边都得通。
    let data = mixed_payload();
    for (tx_kind, rx_kind) in [
        (HeaderKind::Bin32, HeaderKind::Hex),
        (HeaderKind::Hex, HeaderKind::Bin32),
        (HeaderKind::Bin, HeaderKind::Bin32),
        (HeaderKind::Bin32, HeaderKind::Bin),
    ] {
        let tx_cfg = Config::default().with_header(tx_kind);
        let rx_cfg = Config::default().with_header(rx_kind);
        assert_loopback_as(tx_cfg, rx_cfg, "cross.bin", &data, Cut::Whole);
    }
}

#[test]
fn loopback_ends_in_finished_phase() {
    let mut tx = term_zmodem::Sender::new(bin32());
    tx.add_file("a", vec![0u8; 10]).unwrap();
    let mut rx = term_zmodem::Receiver::new(hex());

    let mut tx_sink = Sink::default();
    let mut rx_sink = Sink::default();
    tx_sink.apply(tx.start());
    rx_sink.apply(rx.start());
    let mut steps = 0;
    loop {
        steps += 1;
        assert!(steps < 1000, "闭环未收敛");
        let mut moved = false;
        let to_rx = tx_sink.take_out();
        if !to_rx.is_empty() {
            let actions = rx.feed(&to_rx);
            rx_sink.apply(actions);
            moved = true;
        }
        let to_tx = rx_sink.take_out();
        if !to_tx.is_empty() {
            let actions = tx.feed(&to_tx);
            tx_sink.apply(actions);
            moved = true;
        }
        if !moved {
            break;
        }
    }
    assert_eq!(tx.phase(), Phase::Finished);
    assert_eq!(rx.phase(), Phase::Finished);
}

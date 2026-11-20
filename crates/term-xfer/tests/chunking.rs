//! 切分不变性 + 收发闭环等价。
//!
//! 这是本 crate 最容易写错的两个地方，所以单独一组：
//!
//! 1. **切分不变性**：同一段线上字节，整块喂 / 逐字节喂 / 伪随机切分喂，结果必须
//!    逐字段相等（文件内容、事件序列、应答字节）。
//! 2. **闭环等价**：把 Sender 的输出直接喂给 Receiver，最后收到的字节必须与原文件
//!    逐字节一致——这条同时验证了帧编解码、块号推进、填充裁剪、EOT/头块时序。

mod common;

use common::*;
use term_xfer::{Config, Protocol};

fn ymodem() -> Config {
    Config::for_protocol(Protocol::Ymodem)
}

fn xmodem() -> Config {
    Config::for_protocol(Protocol::Xmodem)
}

fn xmodem1k() -> Config {
    Config::for_protocol(Protocol::Xmodem1k)
}

/// 造一段有代表性的 YMODEM 流：零长文件 + 小数文件 + 跨 1K 边界的大文件。
fn ymodem_stream() -> Vec<u8> {
    let mut stream = Vec::new();
    stream.extend(header_frame("empty.txt", 0));
    stream.push(term_xfer::packet::EOT);

    stream.extend(header_frame("small.txt", 5));
    stream.extend(frame128_crc(1, b"hello"));
    stream.push(term_xfer::packet::EOT);

    stream.extend(header_frame("big.bin", 1500));
    stream.extend(frame1024_crc(1, &vec![0x7Cu8; 1024]));
    stream.extend(frame1024_crc(2, &vec![0x7Cu8; 476]));
    stream.push(term_xfer::packet::EOT);

    stream.extend(empty_header_frame());
    stream
}

#[test]
fn receiver_is_chunking_invariant() {
    let stream = ymodem_stream();
    let whole = run_receiver(ymodem(), &stream, Cut::Whole);
    assert!(whole.done);
    assert_eq!(whole.files.len(), 3);

    for cut in [
        Cut::OneByte,
        Cut::Random(1),
        Cut::Random(0xDEAD_BEEF),
        Cut::Random(987_654_321),
    ] {
        let other = run_receiver(ymodem(), &stream, cut);
        assert_eq!(other.files, whole.files, "切分方式 {cut:?} 改变了文件内容");
        assert_eq!(other.out, whole.out, "切分方式 {cut:?} 改变了应答字节");
        assert_eq!(other.file_done, whole.file_done);
        assert!(other.done);
    }
}

#[test]
fn xmodem_stream_is_chunking_invariant() {
    let mut stream = Vec::new();
    for block in 1..=5u8 {
        stream.extend(frame128_crc(block, &[0x10 * block; 128]));
    }
    stream.push(term_xfer::packet::EOT);

    let whole = run_receiver(xmodem(), &stream, Cut::Whole);
    for cut in [Cut::OneByte, Cut::Random(7), Cut::Random(0x1234_5678)] {
        let other = run_receiver(xmodem(), &stream, cut);
        assert_eq!(other.files, whole.files);
        assert_eq!(other.out, whole.out);
    }
    assert_eq!(whole.single_data().len(), 640);
}

#[test]
fn loopback_xmodem_128() {
    let data: Vec<u8> = (0..1000u32).map(|i| (i % 251) as u8).collect();
    let (tx, rx) = loopback(xmodem(), &[("ignored", data.clone())], Cut::Whole);
    assert!(tx.done && rx.done);
    assert_eq!(rx.files.len(), 1);
    assert_eq!(rx.files[0].name, "", "XMODEM 线上不带文件名");
    assert_eq!(rx.single_data(), &data[..]);
}

#[test]
fn loopback_xmodem_1k() {
    let data: Vec<u8> = (0..5000u32).map(|i| (i % 241) as u8).collect();
    let (tx, rx) = loopback(xmodem1k(), &[("ignored", data.clone())], Cut::Random(0xABCD));
    assert!(tx.done && rx.done);
    assert_eq!(rx.single_data(), &data[..]);
}

#[test]
fn loopback_ymodem_multi_file() {
    let files = vec![
        ("empty.bin", Vec::new()),
        (
            "docs.txt",
            b"the quick brown fox jumps over the lazy dog".to_vec(),
        ),
        ("bin.dat", (0..3000u32).map(|i| (i % 253) as u8).collect()),
    ];
    let (tx, rx) = loopback(ymodem(), &files, Cut::Whole);
    assert!(tx.done && rx.done);
    assert_eq!(rx.files.len(), 3);
    for (idx, (name, data)) in files.iter().enumerate() {
        assert_eq!(&rx.files[idx].name, name);
        assert_eq!(rx.files[idx].size, Some(data.len() as u64));
        assert_eq!(&rx.files[idx].data, data);
    }
    assert_eq!(rx.file_done, 3);
}

#[test]
fn loopback_ymodem_random_chunking() {
    let files = vec![
        ("a.txt", b"alpha".to_vec()),
        ("b.bin", (0..2049u32).map(|i| (i % 249) as u8).collect()),
    ];
    for seed in [11u64, 0xFEED, 0x0BAD_C0DE, 42] {
        let (tx, rx) = loopback(ymodem(), &files, Cut::Random(seed));
        assert!(tx.done && rx.done, "seed {seed} 未收敛");
        assert_eq!(rx.files.len(), 2);
        assert_eq!(&rx.files[0].data, &files[0].1);
        assert_eq!(&rx.files[1].data, &files[1].1);
    }
}

#[test]
fn loopback_large_file_one_byte_at_a_time() {
    let data: Vec<u8> = (0..3000u32).map(|i| (i % 251) as u8).collect();
    let (tx, rx) = loopback(ymodem(), &[("huge.bin", data.clone())], Cut::OneByte);
    assert!(tx.done && rx.done);
    assert_eq!(rx.single_data(), &data[..]);
}

#[test]
fn loopback_single_byte_file() {
    // 边界：1 字节文件在 1K 模式下走 128 字节尾块路径。
    let (tx, rx) = loopback(ymodem(), &[("one", vec![0x42])], Cut::Whole);
    assert!(tx.done && rx.done);
    assert_eq!(rx.single_data(), &[0x42]);
}

#[test]
fn loopback_exactly_one_block() {
    let data = vec![0x33u8; 128];
    let (tx, rx) = loopback(xmodem(), &[("x", data.clone())], Cut::Whole);
    assert!(tx.done && rx.done);
    assert_eq!(rx.single_data(), &data[..]);
}

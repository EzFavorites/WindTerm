//! 切分不变性：同一段线上字节，整块喂 / 逐字节喂 / 伪随机切分喂，结果必须逐字段相等。
//!
//! 这是 ZMODEM 实现里最容易写错的地方：帧头是定长的，但数据子包是变长的，而且
//! 子包里的 `ZDLE` 既可能是转义的开始，也可能是帧尾的开始。只要「攒够再解析」的
//! 边界写错一次，逐字节喂就会把某一帧拆成两半。
//!
//! 三种帧头都要跑：ZHEX 的定长部分是 14 个十六进制字符，ZBIN/ZBIN32 是带转义的
//! 定长二进制段——后者的每个前缀都必须被判成「不完整」，早一字节判完整就会错位。

mod common;

use common::*;
use term_zmodem::{Config, HeaderKind, Sender};

/// 一段有代表性的接收流：一个 5 字节文件 + 收尾。
fn receiver_stream(kind: HeaderKind) -> Vec<u8> {
    let mut stream = Vec::new();
    stream.extend(zfile_frame_kind(kind, "hello.txt", 5, 0));
    stream.extend(zdata_frame_kind(kind, 0, b"hello"));
    stream.extend(zeof_frame_kind(kind, 5));
    stream.extend(zfin_frame_kind(kind));
    stream
}

fn check_single_block(kind: HeaderKind) {
    let stream = receiver_stream(kind);
    let whole = run_receiver(kind, &stream, Cut::Whole);
    assert!(whole.done, "{kind:?} 整块喂应当完成");
    assert_eq!(whole.files.len(), 1);

    for cut in [
        Cut::OneByte,
        Cut::Random(1),
        Cut::Random(0xDEAD_BEEF),
        Cut::Random(987_654_321),
    ] {
        let other = run_receiver(kind, &stream, cut);
        assert_eq!(
            other.files, whole.files,
            "{kind:?} 在切分方式 {cut:?} 下改变了文件内容"
        );
        assert_eq!(
            other.out, whole.out,
            "{kind:?} 在切分方式 {cut:?} 下改变了应答字节"
        );
        assert_eq!(other.file_done, whole.file_done);
        assert!(other.done, "{kind:?} 在切分方式 {cut:?} 下没有完成");
    }
}

#[test]
fn receiver_is_chunking_invariant() {
    check_single_block(HeaderKind::Hex);
}

#[test]
fn receiver_bin_header_is_chunking_invariant() {
    check_single_block(HeaderKind::Bin);
}

#[test]
fn receiver_bin32_header_is_chunking_invariant() {
    check_single_block(HeaderKind::Bin32);
}

fn check_multi_block(kind: HeaderKind) {
    // 三块，其中每块的载荷都塞满必须转义的控制字节。
    let mut stream = Vec::new();
    stream.extend(zfile_frame_kind(kind, "mid.bin", 2600, 1_700_000_000));
    stream.extend(zdata_frame_kind(kind, 0, &vec![0x18; 1000]));
    stream.extend(zdata_frame_kind(kind, 1000, &vec![0x0D; 1000]));
    stream.extend(zdata_frame_kind(kind, 2000, &vec![0x11; 600]));
    stream.extend(zeof_frame_kind(kind, 2600));
    stream.extend(zfin_frame_kind(kind));

    let whole = run_receiver(kind, &stream, Cut::Whole);
    assert!(whole.done, "{kind:?} 整块喂应当完成");
    assert_eq!(whole.single().data.len(), 2600);

    for cut in [Cut::OneByte, Cut::Random(7), Cut::Random(0x1234_5678)] {
        let other = run_receiver(kind, &stream, cut);
        assert_eq!(
            other.files, whole.files,
            "{kind:?} 在切分方式 {cut:?} 下改变了文件内容"
        );
        assert_eq!(
            other.out, whole.out,
            "{kind:?} 在切分方式 {cut:?} 下改变了应答字节"
        );
    }
}

#[test]
fn receiver_multi_block_is_chunking_invariant() {
    check_multi_block(HeaderKind::Hex);
}

#[test]
fn receiver_multi_block_bin32_is_chunking_invariant() {
    check_multi_block(HeaderKind::Bin32);
}

fn check_sender(kind: HeaderKind) {
    // 一段固定的应答流：ZRINIT → ZFILE；ZRPOS(0) → 数据；ZACK(5) → ZEOF；
    // ZRINIT → ZFIN；ZFIN → OO。
    let mut replies = zrinit_frame_kind(kind);
    replies.extend(zrpos_frame_kind(kind, 0));
    replies.extend(zack_frame_kind(kind, 5));
    replies.extend(zrinit_frame_kind(kind));
    replies.extend(zfin_frame_kind(kind));

    let whole = run_sender(kind, &replies, Cut::Whole);
    assert!(whole.done, "{kind:?} 整块喂应当完成");
    assert_eq!(whole.errors.len(), 0);

    for cut in [
        Cut::OneByte,
        Cut::Random(3),
        Cut::Random(0xFEED_FACE),
        Cut::Random(42),
    ] {
        let other = run_sender(kind, &replies, cut);
        assert_eq!(
            other.out, whole.out,
            "{kind:?} 在切分方式 {cut:?} 下改变了发送字节"
        );
        assert!(other.done, "{kind:?} 在切分方式 {cut:?} 下没有完成");
    }
}

#[test]
fn sender_is_chunking_invariant() {
    check_sender(HeaderKind::Hex);
}

#[test]
fn sender_bin_header_is_chunking_invariant() {
    check_sender(HeaderKind::Bin);
}

#[test]
fn sender_bin32_header_is_chunking_invariant() {
    check_sender(HeaderKind::Bin32);
}

fn cfg(kind: HeaderKind) -> Config {
    Config::default().with_header(kind)
}

fn run_receiver(kind: HeaderKind, stream: &[u8], cut: Cut) -> Sink {
    let mut rx = term_zmodem::Receiver::new(cfg(kind));
    let mut sink = Sink::default();
    // start() 的 ZRINIT 也算进 out，这样各切分之间可以直接比较。
    sink.apply(rx.start());
    drive_receiver(&mut rx, &mut sink, stream, cut);
    sink
}

fn run_sender(kind: HeaderKind, replies: &[u8], cut: Cut) -> Sink {
    let mut tx = Sender::new(cfg(kind));
    tx.add_file("a.txt", b"hello".to_vec()).expect("排文件失败");
    let mut sink = Sink::default();
    sink.apply(tx.start());
    for chunk in cut.cuts(replies) {
        let actions = tx.feed(&chunk);
        sink.apply(actions);
    }
    sink
}

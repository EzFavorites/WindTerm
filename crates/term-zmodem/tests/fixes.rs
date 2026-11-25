//! 传输审查修项的接线级回归：超时错误分类、流式分批、ZRPOS(total) 收尾、
//! 以及协议层钳制后的续传偏移。

mod common;

use common::*;
use term_xfer::{Error, Fault};
use term_zmodem::frame::{self, FrameType};
use term_zmodem::{Action, Config, HeaderKind, Receiver, Sender};

/// 把发送方推到 `ZEOF` 之后（`AwaitingZrinitAfterEof`）。
fn sender_after_eof(cfg: Config, data: &[u8]) -> (Sender, Vec<Action>) {
    let mut tx = Sender::new(cfg);
    tx.add_file("s.bin", data.to_vec()).unwrap();
    let mut out = tx.start();
    out.extend(tx.feed(&zrinit_frame_kind(HeaderKind::Hex)));
    out.extend(tx.feed(&zrpos_frame(0)));
    (tx, out)
}

/// 静默超时必须报 `Fault::Timeout`，不能沿用校验错的分类。
#[test]
fn a_receive_timeout_is_reported_as_timeout_not_checksum() {
    let mut rx = Receiver::new(hex().with_max_retries(1));
    let mut sink = Sink::default();
    sink.apply(rx.start());
    sink.take_out();
    sink.apply(rx.feed(&zfile_frame("s.bin", 16, 0)));
    sink.take_out();
    assert_eq!(rx.phase(), term_zmodem::Phase::Receiving);

    // 第一次只是催一次 ZRPOS；第二次越过上限，错误里要写 Timeout。
    sink.apply(rx.on_timeout());
    assert!(sink.errors.is_empty(), "第一次还没到上限");
    sink.take_out();
    sink.apply(rx.on_timeout());
    match sink.errors.first() {
        Some(Error::TooManyRetries { last: Fault::Timeout, .. }) => {}
        other => panic!("超时该分类成 Timeout，实得 {other:?}"),
    }
}

/// 真正的坏块仍然报 Checksum —— 两类的分流不能变成「一律 Timeout」。
#[test]
fn a_bad_subpacket_still_reports_checksum() {
    let mut rx = Receiver::new(hex().with_max_retries(0));
    let mut sink = Sink::default();
    sink.apply(rx.start());
    sink.take_out();
    sink.apply(rx.feed(&zfile_frame("s.bin", 4, 0)));
    sink.take_out();

    let mut bad = subpacket(HeaderKind::Hex, b"abcd", b'k');
    let at = bad.len() - 1;
    bad[at] ^= 0xFF;
    let mut stream = header(HeaderKind::Hex, 10, 0u32.to_le_bytes());
    stream.extend_from_slice(&bad);
    sink.apply(rx.feed(&stream));
    match sink.errors.first() {
        Some(Error::TooManyRetries { last: Fault::Checksum, .. }) => {}
        other => panic!("坏块该分类成 Checksum，实得 {other:?}"),
    }
}

/// 连发按固定批拆成多个 `Action::Send`，峰值内存不再随文件大小线性增长；
/// 子包顺序、偏移与末块帧尾保持原样。
#[test]
fn streaming_splits_into_bounded_action_batches_in_order() {
    let data: Vec<u8> = (0..30u32).flat_map(|i| [i as u8; 64]).collect();
    // ZHEX 会把每个字节写成两个字符，单个批帧看着很大；用二进制帧头量真实载荷。
    let cfg = bin().with_block_size(64).with_streaming(true);
    let (tx, actions) = sender_after_eof(cfg, &data);
    assert_eq!(tx.phase(), term_zmodem::Phase::AwaitingZrinitAfterEof);

    let sends: Vec<&Vec<u8>> = actions
        .iter()
        .filter_map(|a| match a {
            Action::Send(bytes) => Some(bytes),
            _ => None,
        })
        .collect();
    // 30 个子包 / 每批 12 = 3 个 ZDATA 帧（外加 ZFILE、ZEOF、开场帧）。
    let zdata: Vec<&Vec<u8>> = sends
        .iter()
        .copied()
        .filter(|bytes| first_header(bytes).frame == FrameType::ZDATA)
        .collect();
    assert!(zdata.len() >= 3, "30 个子包必须拆成多帧，实得 {}", zdata.len());
    for frame in &zdata {
        assert!(frame.len() < data.len(), "单帧不该装下整文件");
    }

    // 把这一串字节喂给接收方：偏移与顺序必须仍然拼回原文件。
    let mut rx = Receiver::new(hex());
    let mut sink = Sink::default();
    sink.apply(rx.start());
    for bytes in sends {
        sink.apply(rx.feed(bytes));
    }
    assert_eq!(sink.single().data, data, "分批后帧序不能乱");
    assert!(sink.errors.is_empty(), "{:?}", sink.errors);
}

/// `ZACK(total)` 是迟到的确认，`ZEOF` 已发就不再补；`ZRPOS(total)` 是对端
/// 明确要收尾，必须补发一帧 ZEOF。
#[test]
fn zack_total_is_ignored_but_zrpos_total_resends_eof() {
    let cfg = hex().with_block_size(16).with_streaming(true);
    let data = vec![7u8; 32];

    let (mut tx, actions) = sender_after_eof(cfg, &data);
    assert!(actions.iter().any(|a| matches!(
        a,
        Action::Send(bytes) if first_header(bytes).frame == FrameType::ZEOF
    )));
    let acked = tx.feed(&zack_frame(32));
    assert!(
        acked.iter().all(|a| !matches!(
            a,
            Action::Send(bytes) if first_header(bytes).frame == FrameType::ZEOF
        )),
        "ZACK(total) 不该再触发一次 ZEOF"
    );

    let (mut tx2, _) = sender_after_eof(cfg, &data);
    let resent = tx2.feed(&zrpos_frame(32));
    assert!(
        resent.iter().any(|a| matches!(
            a,
            Action::Send(bytes) if first_header(bytes).frame == FrameType::ZEOF
        )),
        "ZRPOS(total) 必须补发 ZEOF"
    );
}

/// 对端声明的文件比本地续传偏移短时，协议层钳到声明长度，并把钳制后的
/// 有效值暴露给落盘侧（避免 `set_len` 越过末尾补零）。
#[test]
fn resume_offset_is_clamped_to_the_declared_size_and_exposed() {
    let mut rx = Receiver::new(hex());
    rx.resume_from(100).unwrap();
    let mut sink = Sink::default();
    sink.apply(rx.start());
    sink.take_out();
    assert_eq!(rx.resume_offset(), 100, "原始声明值仍可读");

    sink.apply(rx.feed(&zfile_frame("s.bin", 10, 0)));
    assert_eq!(rx.file_size(), Some(10));
    assert_eq!(rx.received_bytes(), 10);
    assert_eq!(
        rx.effective_resume_offset(),
        10,
        "落盘侧必须用钳制后的有效偏移"
    );
    let out = sink.take_out();
    let head = frame::parse_header(&out);
    match head {
        term_zmodem::frame::ParsedHeader::Valid(h) => {
            assert_eq!(h.frame, FrameType::ZRPOS);
            assert_eq!(frame::decode_position(h.data), 10);
        }
        other => panic!("期望 ZRPOS，实得 {other:?}"),
    }
}

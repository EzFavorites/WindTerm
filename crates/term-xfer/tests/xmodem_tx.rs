//! 发送方（XMODEM / XMODEM-1K）：握手跟随、重传、超次、块号回绕、中止。

mod common;

use common::*;
use term_xfer::packet::{crc16, sum8, ACK, CAN, EOT, NAK, SOH, STX, SUB};
use term_xfer::{Config, Error, Fault, Phase, Protocol, Sender};

fn cfg() -> Config {
    Config::for_protocol(Protocol::Xmodem)
}

/// 校验一帧 128 字节 CRC 帧的结构，返回载荷。
fn check_crc128(frame: &[u8], block: u8) -> Vec<u8> {
    assert_eq!(frame.len(), 133, "128 字节块 + 帧头 3 + CRC 2");
    assert_eq!(frame[0], SOH);
    assert_eq!(frame[1], block);
    assert_eq!(frame[2], !block);
    let payload = frame[3..131].to_vec();
    let crc = crc16(&payload);
    assert_eq!(&frame[131..133], &[(crc >> 8) as u8, (crc & 0xFF) as u8]);
    payload
}

#[test]
fn waits_for_handshake_then_sends_first_block() {
    let mut tx = Sender::new(cfg());
    tx.add_file("whatever.bin", b"hello".to_vec()).unwrap();
    assert_eq!(tx.phase(), Phase::Idle);

    let mut sink = Sink::default();
    // 没听到对端开口之前，一个字节都不发。
    sink.apply(tx.feed(b""));
    assert!(sink.take_out().is_empty());

    sink.apply(tx.feed(b"C"));
    let frame = sink.take_out();
    let payload = check_crc128(&frame, 1);
    assert_eq!(&payload[..5], b"hello");
    assert!(payload[5..].iter().all(|&b| b == SUB), "块尾必须用 SUB 填充");
    assert_eq!(tx.phase(), Phase::SendingData);
}

#[test]
fn nak_handshake_switches_to_checksum_mode() {
    let mut tx = Sender::new(cfg());
    tx.add_file("x", b"hi".to_vec()).unwrap();

    let mut sink = Sink::default();
    sink.apply(tx.feed(&[NAK]));
    let frame = sink.take_out();
    assert_eq!(frame.len(), 132, "8 位校验和帧少 1 字节尾巴");
    let payload = frame[3..131].to_vec();
    assert_eq!(frame[131], sum8(&payload));
}

#[test]
fn ack_advances_then_eot_then_done() {
    let mut tx = Sender::new(cfg());
    tx.add_file("x", vec![1, 2, 3]).unwrap();

    let mut sink = Sink::default();
    sink.apply(tx.feed(b"C"));
    let frame = sink.take_out();
    check_crc128(&frame, 1);

    sink.apply(tx.feed(&[ACK]));
    assert_eq!(sink.take_out(), vec![EOT]);
    assert_eq!(tx.phase(), Phase::AwaitingEndAck);

    sink.apply(tx.feed(&[ACK]));
    assert!(sink.done);
    assert_eq!(tx.phase(), Phase::Finished);
    assert!(tx.feed(&[ACK]).is_empty(), "终态之后不再有动作");
}

#[test]
fn nak_retransmits_byte_identical_frame() {
    let mut tx = Sender::new(cfg());
    tx.add_file("x", vec![9u8; 200]).unwrap();

    let mut sink = Sink::default();
    sink.apply(tx.feed(b"C"));
    let first = sink.take_out();
    sink.apply(tx.feed(&[NAK]));
    let again = sink.take_out();
    assert_eq!(first, again, "重传必须逐字节一致（含块号与 CRC）");
    assert_eq!(tx.phase(), Phase::SendingData);
}

#[test]
fn data_spans_multiple_blocks() {
    let data: Vec<u8> = (0..300u32).map(|i| (i % 251) as u8).collect();
    let mut tx = Sender::new(cfg());
    tx.add_file("x", data.clone()).unwrap();

    let mut sink = Sink::default();
    sink.apply(tx.feed(b"C"));
    let mut got = Vec::new();
    let mut blocks = Vec::new();
    loop {
        let frame = sink.take_out();
        if frame.len() == 1 {
            assert_eq!(frame, vec![EOT]);
            break;
        }
        let block = block_no_of(&frame);
        blocks.push(block);
        got.extend(check_crc128(&frame, block));
        sink.apply(tx.feed(&[ACK]));
    }
    assert_eq!(blocks, vec![1, 2, 3]);
    assert_eq!(&got[..300], &data[..]);
    assert!(got[300..].iter().all(|&b| b == SUB));
    assert_eq!(tx.sent_bytes(), 300);
}

#[test]
fn too_many_naks_aborts() {
    let mut tx = Sender::new(cfg().with_max_retries(1));
    tx.add_file("x", vec![0u8; 10]).unwrap();

    let mut sink = Sink::default();
    sink.apply(tx.feed(b"C"));
    sink.take_out();

    sink.apply(tx.feed(&[NAK])); // 第 1 次：允许，重传
    assert_eq!(sink.take_out().len(), 133);
    assert!(sink.errors.is_empty());

    sink.apply(tx.feed(&[NAK])); // 第 2 次：超次
    let out = sink.take_out();
    assert!(out.iter().all(|&b| b == CAN));
    assert!(matches!(
        sink.last_error(),
        Error::TooManyRetries {
            failures: 2,
            last: Fault::Checksum
        }
    ));
    assert_eq!(tx.phase(), Phase::Aborted);
}

#[test]
fn timeout_retransmits_then_gives_up() {
    let mut tx = Sender::new(cfg().with_max_retries(1));
    tx.add_file("x", vec![0u8; 10]).unwrap();

    let mut sink = Sink::default();
    sink.apply(tx.feed(b"C"));
    let first = sink.take_out();

    sink.apply(tx.on_timeout());
    assert_eq!(sink.take_out(), first);
    sink.apply(tx.on_timeout());
    assert!(matches!(
        sink.last_error(),
        Error::TooManyRetries {
            last: Fault::Timeout,
            ..
        }
    ));
}

#[test]
fn idle_timeout_does_nothing() {
    let mut tx = Sender::new(cfg());
    tx.add_file("x", vec![0u8; 10]).unwrap();
    assert!(tx.on_timeout().is_empty(), "还没听到对端开口，催也没用");
}

#[test]
fn remote_cancel_aborts() {
    let mut tx = Sender::new(cfg());
    tx.add_file("x", vec![0u8; 10]).unwrap();

    let mut sink = Sink::default();
    sink.apply(tx.feed(b"C"));
    sink.take_out();
    sink.apply(tx.feed(&[CAN, CAN]));
    assert_eq!(sink.last_error(), &Error::Cancelled);
    assert_eq!(tx.phase(), Phase::Aborted);
}

#[test]
fn local_cancel_emits_abort_sequence() {
    let mut tx = Sender::new(cfg());
    tx.add_file("x", vec![0u8; 10]).unwrap();
    let out = tx.cancel();
    assert_eq!(out.len(), 1);
    assert_eq!(tx.phase(), Phase::Aborted);
    assert!(tx.cancel().is_empty());
}

#[test]
fn empty_queue_is_an_error() {
    let mut tx = Sender::new(cfg());
    let mut sink = Sink::default();
    sink.apply(tx.feed(b"C"));
    let out = sink.take_out();
    assert_eq!(out, term_xfer::packet::ABORT_SEQUENCE.to_vec());
    assert_eq!(sink.last_error(), &Error::NoFileToSend);
}

#[test]
fn add_file_after_start_is_rejected() {
    let mut tx = Sender::new(Config::for_protocol(Protocol::Ymodem));
    tx.add_file("a", vec![1]).unwrap();
    tx.feed(b"C");
    assert_eq!(tx.add_file("b", vec![2]), Err(Error::AlreadyStarted));
}

#[test]
fn add_file_rejects_bad_specs() {
    let mut tx = Sender::new(Config::for_protocol(Protocol::Ymodem));
    assert!(matches!(
        tx.add_file("", vec![1]),
        Err(Error::BadFileSpec { .. })
    ));
    assert!(matches!(
        tx.add_file("a\0b", vec![1]),
        Err(Error::BadFileSpec { .. })
    ));
    let long = "x".repeat(126); // 126 + "1" + 两个 NUL = 129 > 128
    assert!(
        matches!(tx.add_file(long, vec![1]), Err(Error::BadFileSpec { .. })),
        "文件名 + 长度放不进 128 字节头块时必须报错而不是静默截断"
    );
    assert_eq!(tx.queued_files(), 0);
}

#[test]
fn empty_file_sends_eot_only() {
    let mut tx = Sender::new(cfg());
    tx.add_file("x", Vec::new()).unwrap();

    let mut sink = Sink::default();
    sink.apply(tx.feed(b"C"));
    assert_eq!(sink.take_out(), vec![EOT]);
    sink.apply(tx.feed(&[ACK]));
    assert!(sink.done);
}

#[test]
fn block_numbers_wrap_at_256() {
    // 300 个满块：块号 1..=255，然后 0（代表第 256 块），再回到 1。
    let data = vec![7u8; 300 * 128];
    let mut tx = Sender::new(cfg());
    tx.add_file("big", data).unwrap();

    let mut sink = Sink::default();
    sink.apply(tx.feed(b"C"));
    let mut blocks = Vec::new();
    let mut steps = 0;
    while !sink.done && steps < 2000 {
        let frame = sink.take_out();
        if frame.len() == 1 {
            assert_eq!(frame, vec![EOT]);
            sink.apply(tx.feed(&[ACK]));
            break;
        }
        blocks.push(block_no_of(&frame));
        sink.apply(tx.feed(&[ACK]));
        steps += 1;
    }
    assert!(sink.done);
    assert_eq!(blocks.len(), 300, "300 个满块");
    assert_eq!(&blocks[..3], &[1, 2, 3]);
    assert_eq!(blocks[254], 255);
    assert_eq!(blocks[255], 0, "8 位回绕：第 256 块号是 0");
    assert_eq!(blocks[256], 1);
}

#[test]
fn one_k_mode_uses_1024_blocks_and_128_tail() {
    let data: Vec<u8> = (0..1152u32).map(|i| (i % 199) as u8).collect();
    let mut tx = Sender::new(Config::for_protocol(Protocol::Xmodem1k));
    tx.add_file("big", data.clone()).unwrap();

    let mut sink = Sink::default();
    sink.apply(tx.feed(b"C"));
    let first = sink.take_out();
    assert_eq!(first[0], STX);
    assert_eq!(first.len(), 1029);
    sink.apply(tx.feed(&[ACK]));

    let second = sink.take_out();
    assert_eq!(second[0], SOH, "剩 128 字节时降级为 128 字节帧");
    assert_eq!(block_no_of(&second), 2);
    sink.apply(tx.feed(&[ACK]));

    assert_eq!(sink.take_out(), vec![EOT]);
}

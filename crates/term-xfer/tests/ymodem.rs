//! YMODEM：文件头块、多文件、批结束空头块、零长文件、坏头块、中止。

mod common;

use common::*;
use term_xfer::packet::{ACK, CAN, CRC_REQUEST, EOT, SOH, STX};
use term_xfer::{Config, Error, Phase, Protocol, Receiver, Sender};

fn cfg() -> Config {
    Config::for_protocol(Protocol::Ymodem)
}

/// 从一帧里解出 `(文件名, 长度)`（测试用的独立实现，不复用被测代码）。
fn header_of(frame: &[u8]) -> (String, Option<u64>) {
    assert_eq!(frame[0], SOH);
    let payload = &frame[3..131];
    let name_end = payload.iter().position(|&b| b == 0).expect("头块必须有 NUL");
    let name = String::from_utf8_lossy(&payload[..name_end]).into_owned();
    let rest = &payload[name_end + 1..];
    let size_end = rest.iter().position(|&b| b == 0).unwrap_or(rest.len());
    let size = String::from_utf8_lossy(&rest[..size_end]).trim().to_string();
    (name, size.parse::<u64>().ok())
}

#[test]
fn send_two_files_end_to_end() {
    let mut tx = Sender::new(cfg());
    tx.add_file("a.txt", b"abc".to_vec()).unwrap();
    tx.add_file("b.bin", vec![0x5Au8; 2000]).unwrap();
    assert_eq!(tx.queued_files(), 2);

    let mut sink = Sink::default();

    // 1. 对端喊 'C' → 发 a.txt 的头块。
    sink.apply(tx.feed(b"C"));
    let head = sink.take_out();
    assert_eq!(block_no_of(&head), 0, "头块是块 0");
    assert_eq!(header_of(&head), ("a.txt".to_string(), Some(3)));
    assert_eq!(tx.phase(), Phase::SendingHeader);

    // 2. 头块被 ACK 之后必须等 'C' 才发数据。
    sink.apply(tx.feed(&[ACK]));
    assert!(sink.take_out().is_empty());
    assert_eq!(tx.phase(), Phase::AwaitingDataRequest);

    // 3. 'C' 到了 → a.txt 只有一个 128 字节块（3 字节内容 + 填充）。
    sink.apply(tx.feed(&[CRC_REQUEST]));
    let block = sink.take_out();
    assert_eq!(block_no_of(&block), 1);
    assert_eq!(block[0], SOH);
    sink.apply(tx.feed(&[ACK]));

    // 4. 文件发完 → EOT；ACK 之后进入「等下一个头块」。
    assert_eq!(sink.take_out(), vec![EOT]);
    sink.apply(tx.feed(&[ACK]));
    assert!(sink.take_out().is_empty());
    assert_eq!(tx.phase(), Phase::AwaitingNextHeader);
    assert_eq!(tx.queued_files(), 1);

    // 5. 'C' → b.bin 的头块。
    sink.apply(tx.feed(&[CRC_REQUEST]));
    let head = sink.take_out();
    assert_eq!(header_of(&head), ("b.bin".to_string(), Some(2000)));

    // 6. 数据：1024 + 976 两块。
    sink.apply(tx.feed(&[ACK]));
    sink.apply(tx.feed(&[CRC_REQUEST]));
    let first = sink.take_out();
    assert_eq!(first[0], STX);
    assert_eq!(block_no_of(&first), 1);
    sink.apply(tx.feed(&[ACK]));
    let second = sink.take_out();
    assert_eq!(second[0], STX);
    assert_eq!(block_no_of(&second), 2);
    sink.apply(tx.feed(&[ACK]));

    // 7. EOT → ACK → 'C' → 批结束空头块 → ACK → Done。
    assert_eq!(sink.take_out(), vec![EOT]);
    sink.apply(tx.feed(&[ACK]));
    sink.apply(tx.feed(&[CRC_REQUEST]));
    let tail = sink.take_out();
    assert_eq!(block_no_of(&tail), 0);
    assert_eq!(header_of(&tail), (String::new(), None), "空头块宣布批次结束");
    sink.apply(tx.feed(&[ACK]));

    assert!(sink.done);
    assert_eq!(tx.phase(), Phase::Finished);
    assert_eq!(tx.queued_files(), 0);
}

#[test]
fn receive_two_files() {
    let mut stream = Vec::new();
    // 文件 1：3 字节。
    stream.extend(header_frame("a.txt", 3));
    stream.extend(frame128_crc(1, b"abc"));
    stream.push(EOT);
    // 文件 2：1500 字节（1024 + 476，两块都用 1K 帧）。
    stream.extend(header_frame("b.bin", 1500));
    stream.extend(frame1024_crc(1, &vec![0x5Au8; 1024]));
    stream.extend(frame1024_crc(2, &vec![0x5Au8; 476]));
    stream.push(EOT);
    // 批结束空头块。
    stream.extend(empty_header_frame());

    let sink = run_receiver(cfg(), &stream, Cut::Whole);

    assert!(sink.done);
    assert_eq!(sink.file_done, 2);
    assert_eq!(sink.files.len(), 2);
    assert_eq!(sink.files[0].name, "a.txt");
    assert_eq!(sink.files[0].size, Some(3));
    assert_eq!(sink.files[0].data, b"abc");
    assert_eq!(sink.files[1].name, "b.bin");
    assert_eq!(sink.files[1].size, Some(1500));
    assert_eq!(sink.files[1].data.len(), 1500, "按声明长度精确裁剪");
    assert!(sink.files[1].data.iter().all(|&b| b == 0x5A));
}

#[test]
fn zero_length_file() {
    let mut stream = Vec::new();
    stream.extend(header_frame("empty.txt", 0));
    stream.push(EOT);
    stream.extend(empty_header_frame());

    let sink = run_receiver(cfg(), &stream, Cut::Whole);
    assert!(sink.done);
    assert_eq!(sink.files.len(), 1);
    assert_eq!(sink.files[0].name, "empty.txt");
    assert_eq!(sink.files[0].size, Some(0));
    assert!(sink.files[0].data.is_empty());
}

#[test]
fn send_zero_length_file() {
    let mut tx = Sender::new(cfg());
    tx.add_file("empty.txt", Vec::new()).unwrap();

    let mut sink = Sink::default();
    sink.apply(tx.feed(b"C"));
    assert_eq!(header_of(&sink.take_out()), ("empty.txt".to_string(), Some(0)));
    sink.apply(tx.feed(&[ACK]));
    sink.apply(tx.feed(&[CRC_REQUEST]));
    assert_eq!(sink.take_out(), vec![EOT], "零长文件没有数据块，直接 EOT");
}

#[test]
fn empty_header_terminates_batch() {
    let mut rx = Receiver::new(cfg());
    let mut sink = Sink::default();
    sink.apply(rx.start());
    sink.take_out();

    sink.apply(rx.feed(&empty_header_frame()));
    assert!(sink.done);
    assert_eq!(sink.take_out(), vec![ACK]);
    assert_eq!(rx.phase(), Phase::Finished);
}

#[test]
fn bad_header_is_fatal() {
    let mut payload = Vec::new();
    payload.extend_from_slice(b"a.txt\0not-a-number\0");
    payload.resize(128, 0);
    let mut frame = vec![SOH, 0, !0u8];
    frame.extend_from_slice(&payload);
    let crc = term_xfer::packet::crc16(&payload);
    frame.push((crc >> 8) as u8);
    frame.push((crc & 0xFF) as u8);

    let mut rx = Receiver::new(cfg());
    let mut sink = Sink::default();
    sink.apply(rx.start());
    sink.take_out();
    sink.apply(rx.feed(&frame));

    assert!(matches!(sink.last_error(), Error::BadHeader { .. }));
    assert!(!sink.done);
    assert_eq!(rx.phase(), Phase::Aborted);
}

/// 头块文件名来自对端：`../` 与控制字符必须整笔拒绝，绝不能变成 `FileStart`
/// 交给调用方拼落盘路径（与 `term-sftp` 的 `is_plain_name` 同一口径）。
#[test]
fn a_dirty_header_name_is_refused_before_file_start() {
    for name in ["../../evil", "a/b", ".", "..", "a\nb"] {
        let mut rx = Receiver::new(cfg());
        let mut sink = Sink::default();
        sink.apply(rx.start());
        sink.take_out();

        sink.apply(rx.feed(&header_frame(name, 3)));
        assert!(sink.files.is_empty(), "{name:?} 不得进 FileStart");
        assert!(matches!(sink.last_error(), Error::BadHeader { .. }));
        assert_eq!(rx.phase(), Phase::Aborted);
    }
}

#[test]
fn cancel_mid_batch() {
    let mut stream = Vec::new();
    stream.extend(header_frame("a.txt", 300));
    stream.extend(frame128_crc(1, &[1u8; 128]));
    stream.extend_from_slice(&[CAN, CAN]);

    let sink = run_receiver(cfg(), &stream, Cut::Whole);
    assert_eq!(sink.last_error(), &Error::Cancelled);
    assert!(!sink.done);
    assert_eq!(sink.files.len(), 1, "已开始的文件仍可交付已到的字节");
}

#[test]
fn replayed_header_does_not_start_file_twice() {
    let mut stream = Vec::new();
    stream.extend(header_frame("a.txt", 4));
    stream.extend(header_frame("a.txt", 4)); // ACK 丢了导致的重传
    stream.extend(frame128_crc(1, b"data"));
    stream.push(EOT);
    stream.extend(empty_header_frame());

    let sink = run_receiver(cfg(), &stream, Cut::Whole);
    assert!(sink.done);
    assert_eq!(sink.files.len(), 1, "重传的头块不得重复 FileStart");
    assert_eq!(sink.files[0].data, b"data");
}

#[test]
fn after_eot_receiver_waits_for_next_header() {
    // 批处理模式下「最后一个 EOT」与「还有下一个文件」在线上长得一样：
    // 收方必须回到握手态等下一个头块，只有头块（或超时）才能判定批次结束。
    let mut stream = Vec::new();
    stream.extend(header_frame("a.txt", 2));
    stream.extend(frame128_crc(1, b"hi"));
    stream.push(EOT);

    let mut rx = Receiver::new(cfg().with_max_retries(1));
    let mut sink = Sink::default();
    sink.apply(rx.start());
    sink.apply(rx.feed(&stream));

    assert!(!sink.done, "最后一个 EOT 之后仍要等下一个头块");
    assert_eq!(sink.files[0].data, b"hi");
    assert_eq!(rx.phase(), Phase::Handshake);

    // 对端再也不说话 → 靠调用方的定时器收敛，不能永远挂着。
    sink.apply(rx.on_timeout());
    sink.apply(rx.on_timeout());
    assert!(matches!(
        sink.last_error(),
        Error::TooManyRetries {
            last: term_xfer::Fault::Timeout,
            ..
        }
    ));
    assert_eq!(rx.phase(), Phase::Aborted);
}

#[test]
fn receiver_requests_next_header_after_eot() {
    let mut rx = Receiver::new(cfg());
    let mut sink = Sink::default();
    sink.apply(rx.start());
    assert_eq!(sink.take_out(), vec![CRC_REQUEST]);

    sink.apply(rx.feed(&header_frame("a.txt", 1)));
    // ACK 头块 + 紧接着请求数据。
    assert_eq!(sink.take_out(), vec![ACK, CRC_REQUEST]);

    sink.apply(rx.feed(&frame128_crc(1, b"x")));
    assert_eq!(sink.take_out(), vec![ACK]);

    sink.apply(rx.feed(&[EOT]));
    assert_eq!(sink.take_out(), vec![ACK, CRC_REQUEST], "EOT 之后要 'C' 请下一个头块");
    assert_eq!(rx.phase(), Phase::Handshake);
}

#[test]
fn xmodem_protocol_has_no_header_block() {
    // 同一段 YMODEM 头块喂给 XMODEM 接收方：块 0 不是期望的块 1 → NAK。
    let mut rx = Receiver::new(Config::for_protocol(Protocol::Xmodem));
    let mut sink = Sink::default();
    sink.apply(rx.start());
    sink.take_out();
    sink.apply(rx.feed(&header_frame("a.txt", 1)));
    assert_eq!(sink.take_out(), vec![term_xfer::packet::NAK]);
    assert!(sink.files.is_empty());
}

//! 接收方（XMODEM / XMODEM-1K）：帧解析、校验、重传、重复帧、填充、中止。

mod common;

use common::*;
use term_xfer::packet::{ACK, CAN, CRC_REQUEST, EOT, NAK, SOH, STX, SUB};
use term_xfer::{Action, Config, Error, Fault, Phase, Protocol, Receiver};

fn cfg() -> Config {
    Config::for_protocol(Protocol::Xmodem)
}

fn cfg1k() -> Config {
    Config::for_protocol(Protocol::Xmodem1k)
}

#[test]
fn single_frame_then_eot() {
    let payload = vec![0x41u8; 128];
    let mut stream = frame128_crc(1, &payload);
    stream.push(EOT);

    let sink = run_receiver(cfg(), &stream, Cut::Whole);

    assert!(sink.done);
    assert_eq!(sink.file_done, 1);
    assert_eq!(sink.files.len(), 1);
    // XMODEM 不带文件名也不带长度。
    assert_eq!(sink.files[0].name, "");
    assert_eq!(sink.files[0].size, None);
    assert_eq!(sink.files[0].data, payload);
    // 握手 'C' + 数据块 ACK + EOT ACK。
    assert_eq!(sink.out, vec![CRC_REQUEST, ACK, ACK]);
}

#[test]
fn multi_frame_accumulates_in_order() {
    let mut stream = Vec::new();
    for block in 1..=3u8 {
        stream.extend(frame128_crc(block, &[block; 128]));
    }
    stream.push(EOT);

    let sink = run_receiver(cfg(), &stream, Cut::Whole);
    assert!(sink.done);
    let mut want = Vec::new();
    for block in 1..=3u8 {
        want.extend(vec![block; 128]);
    }
    assert_eq!(sink.single_data(), &want[..]);
}

#[test]
fn arbitrary_chunking_is_equivalent() {
    let mut stream = Vec::new();
    for block in 1..=4u8 {
        stream.extend(frame128_crc(block, vec![b'a' + block; usize::from(block) * 30].as_slice()));
    }
    stream.push(EOT);

    let whole = run_receiver(cfg(), &stream, Cut::Whole);
    let bytes = run_receiver(cfg(), &stream, Cut::OneByte);
    let random = run_receiver(cfg(), &stream, Cut::Random(0x5EED_1234));

    assert_eq!(whole.files, bytes.files);
    assert_eq!(whole.files, random.files);
    assert_eq!(whole.out, bytes.out);
    assert_eq!(whole.out, random.out);
    assert!(whole.done && bytes.done && random.done);
}

#[test]
fn crc_error_triggers_nak_then_retransmit_works() {
    let payload = vec![0x11u8; 128];
    let mut bad = frame128_crc(1, &payload);
    bad[10] ^= 0xFF; // 载荷被打坏

    let mut rx = Receiver::new(cfg());
    let mut sink = Sink::default();
    sink.apply(rx.start());
    sink.take_out();

    sink.apply(rx.feed(&bad));
    assert_eq!(sink.take_out(), vec![NAK], "校验失败必须回 NAK");
    assert!(sink.files.is_empty(), "坏帧不得产生任何 Data");

    // 对端重传同一块（这次是好的）。
    sink.apply(rx.feed(&frame128_crc(1, &payload)));
    assert_eq!(sink.take_out(), vec![ACK]);
    assert_eq!(sink.single_data(), &payload[..]);

    sink.apply(rx.feed(&[EOT]));
    assert!(sink.done);
}

#[test]
fn corrupted_block_complement_is_nak() {
    let mut frame = frame128_crc(1, &[0x22u8; 128]);
    frame[2] ^= 0x01; // ~blk 不匹配

    let mut rx = Receiver::new(cfg());
    let mut sink = Sink::default();
    sink.apply(rx.start());
    sink.take_out();
    sink.apply(rx.feed(&frame));
    assert_eq!(sink.take_out(), vec![NAK]);
    assert!(sink.files.is_empty());
}

#[test]
fn consecutive_nak_eventually_gives_up() {
    let mut bad = frame128_crc(1, &[0x11u8; 128]);
    bad[5] ^= 0xFF;

    let mut rx = Receiver::new(cfg().with_max_retries(2));
    let mut sink = Sink::default();
    sink.apply(rx.start());
    sink.take_out();

    // 两次坏帧：都只是 NAK，还没放弃。
    sink.apply(rx.feed(&bad));
    sink.apply(rx.feed(&bad));
    assert_eq!(sink.take_out(), vec![NAK, NAK]);
    assert!(sink.errors.is_empty());

    // 第三次：超次放弃，先甩中止序列再报错。
    sink.apply(rx.feed(&bad));
    let out = sink.take_out();
    assert_eq!(out.len(), term_xfer::packet::ABORT_SEQUENCE.len());
    assert!(out.iter().all(|&b| b == CAN));
    assert!(matches!(
        sink.last_error(),
        Error::TooManyRetries {
            failures: 3,
            last: Fault::Checksum
        }
    ));
    assert_eq!(rx.phase(), Phase::Aborted);
    // 终态之后喂什么都当没发生。
    assert!(rx.feed(&frame128_crc(1, &[0u8; 128])).is_empty());
}

#[test]
fn duplicate_block_is_reacked_without_duplicating_data() {
    let payload = vec![0x33u8; 128];
    let frame = frame128_crc(1, &payload);

    let mut rx = Receiver::new(cfg());
    let mut sink = Sink::default();
    sink.apply(rx.start());
    sink.take_out();

    sink.apply(rx.feed(&frame));
    assert_eq!(sink.take_out(), vec![ACK]);
    // 我们的 ACK 丢了，对端重发同一块。
    sink.apply(rx.feed(&frame));
    assert_eq!(sink.take_out(), vec![ACK]);
    assert_eq!(sink.single_data(), &payload[..], "重复帧不得重复落盘");

    sink.apply(rx.feed(&[EOT]));
    assert!(sink.done);
}

#[test]
fn unexpected_block_number_is_nak() {
    let mut rx = Receiver::new(cfg());
    let mut sink = Sink::default();
    sink.apply(rx.start());
    sink.take_out();

    // 期望块 1，来了块 3。
    sink.apply(rx.feed(&frame128_crc(3, &[0x44u8; 128])));
    assert_eq!(sink.take_out(), vec![NAK]);
    assert!(sink.files.is_empty());
    assert_eq!(rx.expected_block(), 1);
}

#[test]
fn checksum_mode_handshake_is_nak() {
    let payload = vec![0x55u8; 128];
    let mut stream = frame128_sum(1, &payload);
    stream.push(EOT);

    let sink = run_receiver(cfg().with_checksum(term_xfer::Checksum::Sum8), &stream, Cut::Whole);
    assert!(sink.done);
    assert_eq!(sink.single_data(), &payload[..]);
    // 握手用 NAK 而不是 'C'。
    assert_eq!(sink.out[0], NAK);
}

#[test]
fn empty_file_is_eot_only() {
    let sink = run_receiver(cfg(), &[EOT], Cut::Whole);
    assert!(sink.done);
    assert_eq!(sink.file_done, 1);
    assert_eq!(sink.files.len(), 1);
    assert_eq!(sink.files[0].name, "");
    assert!(sink.files[0].data.is_empty());
}

#[test]
fn block_padding_is_stripped_at_end_of_file() {
    let mut stream = frame128_crc(1, b"hello");
    stream.push(EOT);
    let sink = run_receiver(cfg(), &stream, Cut::Whole);
    assert_eq!(sink.single_data(), b"hello");
}

#[test]
fn sub_bytes_inside_file_survive_padding_trim() {
    // 第 1 块真实内容以两个 0x1A 结尾（块是满的，不是填充）；第 2 块还有内容。
    let mut first = vec![0x41u8; 126];
    first.push(SUB);
    first.push(SUB);
    let second = vec![0x42u8; 128];

    let mut stream = frame128_crc(1, &first);
    stream.extend(frame128_crc(2, &second));
    stream.push(EOT);

    let sink = run_receiver(cfg(), &stream, Cut::Whole);
    let mut want = first.clone();
    want.extend(second.clone());
    assert_eq!(
        sink.single_data(),
        &want[..],
        "文件中间以 SUB 结尾的块不得被吃掉"
    );
}

#[test]
fn trailing_padding_spanning_blocks_is_dropped() {
    // 第 1 块整块都是填充（不可能出现在文件中间，但协议允许对端这么发）。
    let mut stream = frame128_crc(1, &[SUB; 128]);
    stream.extend(frame128_crc(2, b"tail"));
    stream.push(EOT);
    let sink = run_receiver(cfg(), &stream, Cut::Whole);
    // 第 1 块末尾的 128 个 SUB 先被暂扣；第 2 块证明它不是文件末尾 → 补发。
    let mut want = vec![SUB; 128];
    want.extend_from_slice(b"tail");
    assert_eq!(sink.single_data(), &want[..]);
}

#[test]
fn garbage_before_frame_is_skipped() {
    let mut stream = b"Welcome to the router\r\nlogin: ".to_vec();
    stream.extend(frame128_crc(1, b"payload"));
    stream.push(EOT);
    let sink = run_receiver(cfg(), &stream, Cut::Whole);
    assert!(sink.done);
    assert_eq!(sink.single_data(), b"payload");
}

#[test]
fn remote_cancel_aborts() {
    let mut rx = Receiver::new(cfg());
    let mut sink = Sink::default();
    sink.apply(rx.start());
    sink.apply(rx.feed(&frame128_crc(1, b"abc")));
    sink.take_out();

    sink.apply(rx.feed(&[CAN, CAN]));
    assert_eq!(sink.last_error(), &Error::Cancelled);
    assert!(!sink.done);
    assert_eq!(rx.phase(), Phase::Aborted);
}

#[test]
fn single_can_is_not_a_cancel() {
    let mut rx = Receiver::new(cfg());
    let mut sink = Sink::default();
    sink.apply(rx.start());
    sink.take_out();
    sink.apply(rx.feed(&[CAN]));
    assert!(sink.errors.is_empty());
    assert_eq!(sink.take_out(), Vec::<u8>::new());
    assert_eq!(rx.phase(), Phase::Handshake);
}

#[test]
fn local_cancel_emits_abort_sequence() {
    let mut rx = Receiver::new(cfg());
    rx.start();
    let out = rx.cancel();
    assert_eq!(
        out,
        vec![Action::Send(term_xfer::packet::ABORT_SEQUENCE.to_vec())]
    );
    assert_eq!(rx.phase(), Phase::Aborted);
    assert!(rx.feed(&[EOT]).is_empty());
    assert!(rx.cancel().is_empty(), "重复中止不得重复发 CAN");
}

#[test]
fn mixed_1k_and_128_blocks() {
    let big = vec![0x61u8; 1024];
    let mut stream = frame1024_crc(1, &big);
    stream.extend(frame128_crc(2, b"tail"));
    stream.push(EOT);

    let sink = run_receiver(cfg1k(), &stream, Cut::OneByte);
    let mut want = big;
    want.extend_from_slice(b"tail");
    assert_eq!(sink.single_data(), &want[..]);
    assert!(sink.done);
    // 接收方两种帧长都吃。
    assert_eq!(stream[0], STX);
    assert_eq!(stream[1029], SOH);
}

#[test]
fn handshake_timeout_resends_c_then_gives_up() {
    let mut rx = Receiver::new(cfg().with_max_retries(2));
    let mut sink = Sink::default();
    sink.apply(rx.start());
    assert_eq!(sink.take_out(), vec![CRC_REQUEST]);

    sink.apply(rx.on_timeout());
    assert_eq!(sink.take_out(), vec![CRC_REQUEST], "握手阶段超时应重发 'C'");
    sink.apply(rx.on_timeout());
    assert_eq!(sink.take_out(), vec![CRC_REQUEST]);
    assert!(sink.errors.is_empty());

    sink.apply(rx.on_timeout());
    assert!(matches!(
        sink.last_error(),
        Error::TooManyRetries {
            failures: 3,
            last: Fault::Timeout
        }
    ));
    assert_eq!(rx.phase(), Phase::Aborted);
}

#[test]
fn receiving_timeout_naks_current_block() {
    let mut rx = Receiver::new(cfg());
    let mut sink = Sink::default();
    sink.apply(rx.start());
    sink.apply(rx.feed(&frame128_crc(1, b"abc")));
    sink.take_out();

    sink.apply(rx.on_timeout());
    assert_eq!(sink.take_out(), vec![NAK]);
    assert_eq!(rx.expected_block(), 2);
}

#[test]
fn start_is_idempotent_and_feed_auto_starts() {
    let mut rx = Receiver::new(cfg());
    let first = rx.start();
    assert_eq!(first, vec![Action::Send(vec![CRC_REQUEST])]);
    assert!(rx.start().is_empty());

    // 另一个实例：不显式 start，直接 feed，握手字节应从 feed 里出来。
    let mut rx2 = Receiver::new(cfg());
    let out = rx2.feed(&frame128_crc(1, b"hi"));
    assert_eq!(out[0], Action::Send(vec![CRC_REQUEST]));
    assert_eq!(rx2.phase(), Phase::Receiving);
}

#[test]
fn config_padding_is_honoured() {
    use term_xfer::Checksum;
    let mut config = Config::for_protocol(Protocol::Xmodem);
    config.padding = 0x00;
    config.checksum = Checksum::Crc16;

    // 用 0x00 填充的最后一块：内容 b"hi"，其余是 0。
    let payload = pad_to(b"hi", 128); // pad_to 用 SUB，需要手工换掉
    let mut zero_padded = payload.clone();
    for byte in zero_padded.iter_mut().skip(2) {
        *byte = 0x00;
    }
    let mut stream = frame128_crc(1, &zero_padded);
    stream.push(EOT);

    let sink = run_receiver(config, &stream, Cut::Whole);
    assert_eq!(sink.single_data(), b"hi");
}

//! 错误路径：重试、超次放弃、对端中止、本地取消、坏帧、单文件限制。

mod common;

use common::*;
use term_zmodem::{Action, Error, Fault, Phase, Receiver, Sender};

fn sender_with(data: &[u8], cfg: term_zmodem::Config) -> (Sender, Sink) {
    let mut tx = Sender::new(cfg);
    tx.add_file("a.bin", data.to_vec()).expect("排文件失败");
    let mut sink = Sink::default();
    sink.apply(tx.start());
    (tx, sink)
}

#[test]
fn sender_resends_last_frame_on_timeout() {
    let cfg = hex().with_max_retries(3);
    let (mut tx, mut sink) = sender_with(b"hello", cfg);
    let first = sink.take_out();
    assert!(!first.is_empty());
    assert_eq!(tx.phase(), Phase::AwaitingZrinit);

    sink.apply(tx.on_timeout());
    assert_eq!(sink.take_out(), first, "超时应当原样重发 ZRQINIT");
    assert_eq!(tx.phase(), Phase::AwaitingZrinit);
}

#[test]
fn sender_gives_up_after_retries() {
    let cfg = hex().with_max_retries(2);
    let (mut tx, mut sink) = sender_with(b"hello", cfg);
    sink.take_out();

    sink.apply(tx.on_timeout());
    sink.apply(tx.on_timeout());
    assert_eq!(tx.phase(), Phase::AwaitingZrinit, "两次重试内不该放弃");
    sink.take_out();

    sink.apply(tx.on_timeout());
    assert_eq!(tx.phase(), Phase::Aborted);
    assert_eq!(
        sink.errors.last(),
        Some(&Error::TooManyRetries {
            failures: 3,
            last: Fault::Timeout,
        })
    );
}

#[test]
fn receiver_gives_up_after_retries() {
    let cfg = hex().with_max_retries(2);
    let mut rx = Receiver::new(cfg);
    let mut sink = Sink::default();
    sink.apply(rx.start());
    sink.take_out();

    sink.apply(rx.on_timeout());
    sink.apply(rx.on_timeout());
    assert_eq!(rx.phase(), Phase::AwaitingZfile);

    sink.apply(rx.on_timeout());
    assert_eq!(rx.phase(), Phase::Aborted);
    assert!(matches!(
        sink.errors.last(),
        Some(Error::TooManyRetries {
            failures: 3,
            last: Fault::Timeout
        })
    ));
}

#[test]
fn remote_can_aborts_sender() {
    let (mut tx, mut sink) = sender_with(b"hello", hex());
    sink.take_out();
    sink.apply(tx.feed(&[0x18; 5]));
    assert_eq!(tx.phase(), Phase::Aborted);
    assert_eq!(sink.errors, vec![Error::Cancelled]);
}

#[test]
fn remote_can_aborts_receiver() {
    let mut rx = Receiver::new(hex());
    let mut sink = Sink::default();
    sink.apply(rx.start());
    sink.take_out();
    sink.apply(rx.feed(&[0x18; 5]));
    assert_eq!(rx.phase(), Phase::Aborted);
    assert_eq!(sink.errors, vec![Error::Cancelled]);
    assert_eq!(
        sink.take_out(),
        term_zmodem::frame::ABORT_SEQUENCE.to_vec(),
        "本端放弃前要先甩中止序列"
    );
}

#[test]
fn fewer_than_five_cans_is_not_an_abort() {
    let mut rx = Receiver::new(hex());
    let mut sink = Sink::default();
    sink.apply(rx.start());
    sink.take_out();
    sink.apply(rx.feed(&[0x18; 4]));
    assert!(!rx.phase().is_terminal(), "4 个 CAN 还不算中止");
    assert!(sink.errors.is_empty());
}

#[test]
fn local_cancel_sends_abort_sequence() {
    let (mut tx, _sink) = sender_with(b"hello", hex());
    let actions = tx.cancel();
    assert_eq!(
        actions,
        vec![Action::Send(term_zmodem::frame::ABORT_SEQUENCE.to_vec())]
    );
    assert_eq!(tx.phase(), Phase::Aborted);
    assert!(tx.cancel().is_empty(), "终态下 cancel 返回空");

    let mut rx = Receiver::new(hex());
    let actions = rx.cancel();
    assert_eq!(actions.len(), 1);
    assert_eq!(rx.phase(), Phase::Aborted);
    assert!(rx.feed(b"anything").is_empty(), "终态下 feed 不再做事");
}

#[test]
fn receiver_rejects_bad_zfile() {
    let mut rx = Receiver::new(hex());
    let mut sink = Sink::default();
    sink.apply(rx.start());
    sink.take_out();
    // 名字为空。
    sink.apply(rx.feed(&zfile_frame("", 5, 0)));
    assert_eq!(rx.phase(), Phase::Aborted);
    assert!(matches!(sink.errors.last(), Some(Error::BadHeader { .. })));
}

#[test]
fn receiver_rejects_oversized_declared_length() {
    let mut rx = Receiver::new(hex());
    let mut sink = Sink::default();
    sink.apply(rx.start());
    sink.take_out();
    // 5 GB：超过 32 位偏移上限，后续 ZDATA 的 4 字节偏移一定错位。
    sink.apply(rx.feed(&zfile_frame("huge.bin", 5_000_000_000, 0)));
    assert_eq!(rx.phase(), Phase::Aborted);
    assert!(matches!(sink.errors.last(), Some(Error::BadHeader { .. })));
}

#[test]
fn receiver_retries_corrupt_block_with_zrpos() {
    let cfg = hex().with_max_retries(5);
    let mut rx = Receiver::new(cfg);
    let mut sink = Sink::default();
    sink.apply(rx.start());
    sink.take_out();

    sink.apply(rx.feed(&zfile_frame("a.txt", 5, 0)));
    assert_eq!(sink.take_out(), zrpos_frame(0));
    assert_eq!(rx.phase(), Phase::Receiving);

    // 把载荷首字节改掉：CRC-32 不过。
    let mut corrupt = zdata_frame(0, b"hello");
    corrupt[20] ^= 0xFF;
    sink.apply(rx.feed(&corrupt));
    assert_eq!(sink.take_out(), zrpos_frame(0), "坏块应当请求从当前偏移重发");
    assert_eq!(rx.received_bytes(), 0);
    assert!(!rx.phase().is_terminal());

    // 对端重发好块：继续。
    sink.apply(rx.feed(&zdata_frame(0, b"hello")));
    assert_eq!(sink.take_out(), zack_frame(5));
    assert_eq!(rx.received_bytes(), 5);

    sink.apply(rx.feed(&zeof_frame(5)));
    assert_eq!(sink.file_done, 1);
    assert_eq!(sink.take_out(), zrinit_frame());

    sink.apply(rx.feed(&zfin_frame()));
    assert_eq!(sink.take_out(), zfin_frame());
    assert!(sink.done);
}

#[test]
fn receiver_gives_up_after_too_many_corrupt_blocks() {
    let cfg = hex().with_max_retries(1);
    let mut rx = Receiver::new(cfg);
    let mut sink = Sink::default();
    sink.apply(rx.start());
    sink.take_out();
    sink.apply(rx.feed(&zfile_frame("a.txt", 5, 0)));
    sink.take_out();

    let mut corrupt = zdata_frame(0, b"hello");
    corrupt[20] ^= 0xFF;
    sink.apply(rx.feed(&corrupt));
    sink.apply(rx.feed(&corrupt));
    assert_eq!(rx.phase(), Phase::Aborted);
    assert!(matches!(
        sink.errors.last(),
        Some(Error::TooManyRetries {
            failures: 2,
            last: Fault::Checksum
        })
    ));
}

#[test]
fn receiver_accepts_duplicate_block_without_duplicating_data() {
    let mut rx = Receiver::new(hex());
    let mut sink = Sink::default();
    sink.apply(rx.start());
    sink.take_out();
    sink.apply(rx.feed(&zfile_frame("a.txt", 5, 0)));
    sink.take_out();

    sink.apply(rx.feed(&zdata_frame(0, b"hello")));
    sink.take_out();
    // 同一个块又来一次（我们的 ZACK 丢了）。
    sink.apply(rx.feed(&zdata_frame(0, b"hello")));
    assert_eq!(sink.take_out(), zack_frame(5));
    assert_eq!(sink.single().data, b"hello");
    assert_eq!(rx.received_bytes(), 5);
}

#[test]
fn receiver_asks_for_expected_offset_when_block_jumps_forward() {
    let mut rx = Receiver::new(hex());
    let mut sink = Sink::default();
    sink.apply(rx.start());
    sink.take_out();
    sink.apply(rx.feed(&zfile_frame("a.txt", 100, 0)));
    sink.take_out();

    sink.apply(rx.feed(&zdata_frame(50, &[0x41; 10])));
    assert_eq!(sink.take_out(), zrpos_frame(0));
    assert_eq!(rx.received_bytes(), 0);
}

#[test]
fn receiver_naks_data_before_zfile() {
    let mut rx = Receiver::new(hex());
    let mut sink = Sink::default();
    sink.apply(rx.start());
    sink.take_out();
    sink.apply(rx.feed(&zdata_frame(0, b"hello")));
    assert_eq!(sink.take_out(), znak_frame());
    assert!(sink.files.is_empty());
}

#[test]
fn receiver_repeats_zrinit_on_duplicate_zeof() {
    let mut rx = Receiver::new(hex());
    let mut sink = Sink::default();
    sink.apply(rx.start());
    sink.take_out();
    sink.apply(rx.feed(&zfile_frame("a.txt", 0, 0)));
    sink.take_out();
    sink.apply(rx.feed(&zeof_frame(0)));
    assert_eq!(sink.take_out(), zrinit_frame());

    // 对端没收到我们的 ZRINIT，重发 ZEOF：应当再回一次 ZRINIT，而不是 NAK。
    sink.apply(rx.feed(&zeof_frame(0)));
    assert_eq!(sink.take_out(), zrinit_frame());
    assert_eq!(sink.file_done, 1, "不能重复 FileDone");
}

#[test]
fn receiver_skips_second_file() {
    let mut rx = Receiver::new(hex());
    let mut sink = Sink::default();
    sink.apply(rx.start());
    sink.take_out();
    sink.apply(rx.feed(&zfile_frame("a.txt", 0, 0)));
    sink.take_out();
    sink.apply(rx.feed(&zeof_frame(0)));
    sink.take_out();
    assert_eq!(rx.phase(), Phase::AwaitingZfin);

    sink.apply(rx.feed(&zfile_frame("b.txt", 5, 0)));
    assert_eq!(sink.take_out(), zskip_frame(), "单文件模式应当回 ZSKIP");
    assert_eq!(sink.files.len(), 1);
}

#[test]
fn receiver_skips_zsinit_subpacket() {
    let mut rx = Receiver::new(hex());
    let mut sink = Sink::default();
    sink.apply(rx.start());
    sink.take_out();

    let mut stream = zsinit_frame(&[0x11, 0x13, 0x11, 0x13]);
    stream.extend(zfile_frame("a.txt", 5, 0));
    stream.extend(zdata_frame(0, b"hello"));
    stream.extend(zeof_frame(5));
    stream.extend(zfin_frame());

    drive_receiver(&mut rx, &mut sink, &stream, Cut::Whole);
    assert!(sink.done);
    assert_eq!(sink.single().data, b"hello");
}

#[test]
fn sender_treats_zskip_as_failure() {
    let (mut tx, mut sink) = sender_with(b"hello", hex());
    sink.take_out();
    sink.apply(tx.feed(&zrinit_frame()));
    sink.take_out();
    assert_eq!(tx.phase(), Phase::AwaitingZrpos);
    sink.apply(tx.feed(&zskip_frame()));
    assert_eq!(tx.phase(), Phase::Aborted);
    assert!(matches!(sink.errors.last(), Some(Error::BadHeader { .. })));
}

#[test]
fn sender_honours_zrpos_rewind() {
    let cfg = hex().with_block_size(4).with_max_retries(5);
    let (mut tx, mut sink) = sender_with(b"abcdefgh", cfg);
    sink.take_out();
    sink.apply(tx.feed(&zrinit_frame()));
    sink.take_out();
    sink.apply(tx.feed(&zrpos_frame(0)));
    let first_block = sink.take_out();
    assert!(!first_block.is_empty());

    // 对端说只收到了 4 字节：从 4 继续。
    sink.apply(tx.feed(&zack_frame(4)));
    sink.take_out();
    assert_eq!(tx.sent_bytes(), 4);

    // 对端又要从 0 来：接受（会话内回退），并计入一次失败。
    sink.apply(tx.feed(&zrpos_frame(0)));
    assert_eq!(sink.take_out(), first_block, "回退后应当重发同一块");
    assert_eq!(tx.sent_bytes(), 0);
}

#[test]
fn add_file_validates() {
    let mut tx = Sender::new(hex());
    assert_eq!(
        tx.add_file("", vec![1]),
        Err(Error::BadFileSpec {
            detail: "文件名为空".to_string()
        })
    );
    assert!(matches!(
        tx.add_file("a\0b", vec![1]),
        Err(Error::BadFileSpec { .. })
    ));
    tx.add_file("a.txt", vec![1]).unwrap();
    assert!(
        matches!(tx.add_file("b.txt", vec![2]), Err(Error::BadFileSpec { .. })),
        "本轮只支持单文件"
    );
}

#[test]
fn add_file_after_start_is_rejected() {
    let mut tx = Sender::new(hex());
    tx.add_file("a.txt", vec![1]).unwrap();
    tx.start();
    assert_eq!(tx.add_file("b.txt", vec![2]), Err(Error::AlreadyStarted));
}

#[test]
fn sender_without_file_fails_on_start() {
    let mut tx = Sender::new(hex());
    let actions = tx.start();
    assert_eq!(tx.phase(), Phase::Aborted);
    assert!(matches!(
        actions.last(),
        Some(Action::Error(Error::NoFileToSend))
    ));
}

// ── 外部输入上限 ───────────────────────────────────────────────────────────

#[test]
fn receiver_rejects_zpad_flood() {
    let mut rx = Receiver::new(hex());
    let mut sink = Sink::default();
    sink.apply(rx.start());
    sink.take_out();

    // 10 KB 的 ZPAD：合法发送方最多打两三个，这里必须在吃满上限时就判错，
    // 而不是一直等到缓冲爆掉。
    sink.apply(rx.feed(&vec![0x2A; 10 * 1024]));
    assert_eq!(rx.phase(), Phase::Aborted);
    match sink.errors.last() {
        Some(Error::BadHeader { detail }) => assert!(
            detail.contains("ZPAD"),
            "错误信息应当点明是 ZPAD 洪泛，实际：{detail}"
        ),
        other => panic!("期望 BadHeader，实际：{other:?}"),
    }
    assert_eq!(
        sink.take_out(),
        term_zmodem::frame::ABORT_SEQUENCE.to_vec(),
        "判错前要先甩中止序列"
    );
}

#[test]
fn receiver_rejects_unterminated_subpacket() {
    let mut rx = Receiver::new(hex());
    let mut sink = Sink::default();
    sink.apply(rx.start());
    sink.take_out();

    // 只给 ZFILE 帧头，子包永远不来：接收方进入「等子包」状态。
    sink.apply(rx.feed(&hex_header(4, [0; 4])));
    assert!(!rx.phase().is_terminal());

    // 子包只有载荷、永远不给帧尾和 CRC：缓冲超过上限就必须判错，
    // 不能无限吃内存等一个不存在的终止符。
    sink.apply(rx.feed(&vec![0x41; 20 * 1024]));
    assert_eq!(rx.phase(), Phase::Aborted);
    match sink.errors.last() {
        Some(Error::BadHeader { detail }) => assert!(
            detail.contains("终止符"),
            "错误信息应当点明缺少终止符，实际：{detail}"
        ),
        other => panic!("期望 BadHeader，实际：{other:?}"),
    }
}

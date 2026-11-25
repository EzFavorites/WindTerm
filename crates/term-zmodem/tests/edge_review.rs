//! 第二轮复核的边界用例：连发大块 + 逐字节切分、续传恰好等于声明长度、
//! 空文件的 ZRPOS(total)、部分子包超时后的分类。

mod common;

use common::*;
use term_zmodem::frame::FrameType;
use term_zmodem::{Action, Config, Phase, Receiver, Sender};

#[test]
fn streaming_with_max_block_size_survives_bytewise_delivery() {
    let cfg = Config::default()
        .with_streaming(true)
        .with_block_size(8192);
    let data: Vec<u8> = (0..20_000u32).map(|i| (i % 251) as u8).collect();
    let (_, rx) = loopback(cfg, "big.bin", &data, Cut::OneByte);
    assert!(rx.done, "大块连发 + 逐字节到达必须收敛");
    assert!(rx.errors.is_empty(), "{:?}", rx.errors);
    assert_eq!(rx.single().data, data);
}

#[test]
fn resume_exactly_at_the_declared_size_clamps_to_the_same_offset() {
    let mut rx = Receiver::new(Config::default());
    rx.resume_from(10).unwrap();
    let mut sink = Sink::default();
    sink.apply(rx.start());
    sink.take_out();
    sink.apply(rx.feed(&zfile_frame("s.bin", 10, 0)));
    assert_eq!(rx.effective_resume_offset(), 10);
    assert_eq!(rx.received_bytes(), 10);
    assert!(sink.errors.is_empty(), "{:?}", sink.errors);
}

#[test]
fn an_empty_file_answers_zrpos_total_with_eof_not_a_loop() {
    let cfg = Config::default().with_streaming(true);
    let mut tx = Sender::new(cfg);
    tx.add_file("empty.bin", Vec::new()).unwrap();
    let mut sink = Sink::default();
    sink.apply(tx.start());
    sink.apply(tx.feed(&zrinit_frame()));
    let actions = tx.feed(&zrpos_frame(0));
    assert!(
        actions.iter().any(|a| matches!(
            a,
            Action::Send(bytes) if first_header(bytes).frame == FrameType::ZEOF
        )),
        "空文件仍要发 ZEOF"
    );
    assert_eq!(tx.phase(), Phase::AwaitingZrinitAfterEof);
}

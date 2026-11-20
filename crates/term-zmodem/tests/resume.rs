//! 断点续传：接收方声明「本地已有前 N 字节」，用 `ZRPOS(N)` 请发送方从 N 接着发。
//!
//! 全部用内存里的字节流驱动，不碰真实终端与文件系统——续传偏移由调用方
//! （测试里是测试自己）通过 [`term_zmodem::Receiver::resume_from`] 告知。
//!
//! 另有一条历史兼容断言：发送方仍容忍早期约定里的 `ZSKIP(offset > 0)`。真实对端
//! 不会这么发，主路径是 `ZRPOS`。

mod common;

use common::*;
use term_zmodem::frame::{self, FrameType, ParsedHeader, ParsedSubpacket};
use term_zmodem::{Config, Error, HeaderKind, Phase, Receiver, Sender};

/// 一个带偏移的 `ZRPOS` 帧（线格式由测试手写工厂生成，不复用实现）。
fn zrpos_with_offset(offset: u32) -> Vec<u8> {
    header(HeaderKind::Hex, 9, frame::encode_position(offset))
}

/// 一个带偏移的 `ZSKIP` 帧——只有「跳过文件」的协议原义才该出现。
fn zskip_with_offset(offset: u32) -> Vec<u8> {
    header(HeaderKind::Hex, 5, frame::encode_position(offset))
}

/// 解析一段完整的发送方向线上字节流，返回 `(ZDATA 载荷总字节数, 第一个 ZDATA 的偏移)`。
fn data_payload_sum(wire: &[u8]) -> (usize, Option<u32>) {
    let mut at = 0;
    let mut sum = 0;
    let mut first_offset = None;
    while at < wire.len() {
        let ParsedHeader::Valid(head) = frame::parse_header(&wire[at..]) else {
            break;
        };
        at += head.consumed;
        let carries_subpacket = matches!(
            head.frame,
            FrameType::ZFILE | FrameType::ZDATA | FrameType::ZSINIT
        );
        if !carries_subpacket {
            continue;
        }
        let ParsedSubpacket::Valid(packet) =
            frame::parse_subpacket(&wire[at..], head.kind.subpacket_crc())
        else {
            break;
        };
        at += packet.consumed;
        if head.frame == FrameType::ZDATA {
            sum += packet.data.len();
            first_offset.get_or_insert(frame::decode_position(head.data));
        }
    }
    (sum, first_offset)
}

/// 收发闭环，但接收方先声明「本地已有前 `resume` 字节」。
///
/// 返回 `(发送侧字节流, 接收侧 sink)`；接收侧 sink 里只有**本次新收**的字节。
fn resume_loopback(resume: usize, name: &str, data: &[u8]) -> (Vec<u8>, Sink) {
    let cfg = hex().with_block_size(300);
    let mut tx = Sender::new(cfg);
    tx.add_file(name, data.to_vec()).expect("排文件失败");
    let mut rx = Receiver::new(cfg);
    rx.resume_from(resume as u64).expect("设置续传偏移失败");

    let mut tx_sink = Sink::default();
    let mut rx_sink = Sink::default();
    tx_sink.apply(tx.start());
    rx_sink.apply(rx.start());

    let mut wire = Vec::new();
    let mut steps = 0;
    loop {
        steps += 1;
        assert!(steps < 10_000, "闭环未收敛（疑似重传风暴）");
        let mut moved = false;

        let to_rx = tx_sink.take_out();
        if !to_rx.is_empty() {
            wire.extend_from_slice(&to_rx);
            rx_sink.apply(rx.feed(&to_rx));
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
    (wire, rx_sink)
}

// ── 1. 帧编解码往返 ────────────────────────────────────────────────────────

#[test]
fn zskip_frame_roundtrip_carries_offset() {
    for kind in [HeaderKind::Hex, HeaderKind::Bin, HeaderKind::Bin32] {
        let data = frame::encode_position(12_345);
        let wire = frame::encode_header(kind, FrameType::ZSKIP, data);
        let head = first_header(&wire);
        assert_eq!(head.frame, FrameType::ZSKIP, "帧类型应还原（{kind:?}）");
        assert_eq!(head.kind, kind);
        assert_eq!(
            frame::decode_position(head.data),
            12_345,
            "偏移应逐字节往返（{kind:?}）"
        );
        assert_eq!(head.consumed, wire.len(), "不应残留多余字节（{kind:?}）");
    }
}

// ── 2. 接收端发现已有 N 字节 → 回 ZRPOS(N) ─────────────────────────────────

#[test]
fn receiver_requests_resume_from_existing_prefix() {
    let mut rx = Receiver::new(hex());
    rx.resume_from(700).unwrap();
    assert_eq!(rx.resume_offset(), 700);

    let mut sink = Sink::default();
    sink.apply(rx.start());
    sink.take_out();

    sink.apply(rx.feed(&zfile_frame("big.bin", 3000, 0)));
    let out = sink.take_out();
    let head = first_header(&out);
    assert_eq!(head.frame, FrameType::ZRPOS, "有断点时应当回 ZRPOS");
    assert_eq!(frame::decode_position(head.data), 700, "ZRPOS 应当带续传偏移");
    assert_eq!(rx.phase(), Phase::Receiving);
    assert_eq!(rx.received_bytes(), 700);
    assert_eq!(sink.single().name, "big.bin");
    assert_eq!(sink.single().size, Some(3000));
}

/// 对照：`resume_from(0)`（显式声明「从头收」）与不调用时逐字节相同。
#[test]
fn resume_from_zero_still_requests_zrpos_zero() {
    let mut rx = Receiver::new(hex());
    rx.resume_from(0).unwrap();
    let mut sink = Sink::default();
    sink.apply(rx.start());
    sink.take_out();

    sink.apply(rx.feed(&zfile_frame("a.txt", 5, 0)));
    let head = first_header(&sink.take_out());
    assert_eq!(head.frame, FrameType::ZRPOS);
    assert_eq!(frame::decode_position(head.data), 0);
    assert_eq!(rx.received_bytes(), 0);
}

/// `ZSKIP` 只用于「跳过这个文件」：偏移恒为 0，且只在 `ZEOF` 之后出现。
#[test]
fn zskip_is_only_used_to_skip_a_file() {
    let mut rx = Receiver::new(hex());
    // 续传点必须在握手之前声明。
    rx.resume_from(400).unwrap();
    let mut sink = Sink::default();
    sink.apply(rx.start());
    sink.take_out();

    // 有断点的第一个文件：应答必须是 ZRPOS(400)，不能是 ZSKIP(400)。
    sink.apply(rx.feed(&zfile_frame("a.bin", 401, 0)));
    let head = first_header(&sink.take_out());
    assert_eq!(head.frame, FrameType::ZRPOS);
    assert_eq!(frame::decode_position(head.data), 400);

    // 单文件模式：ZEOF 之后的第二个 ZFILE 才轮到 ZSKIP，偏移必须为 0。
    sink.apply(rx.feed(&zdata_frame(400, b"x")));
    sink.take_out();
    sink.apply(rx.feed(&zeof_frame(401)));
    sink.take_out();
    sink.apply(rx.feed(&zfile_frame("b.bin", 7, 0)));
    let head = first_header(&sink.take_out());
    assert_eq!(head.frame, FrameType::ZSKIP);
    assert_eq!(
        frame::decode_position(head.data),
        0,
        "ZSKIP 是「跳过文件」，不该携带续传偏移"
    );
}

// ── 3. 发送端收到 ZRPOS → 偏移推进 ─────────────────────────────────────────

#[test]
fn sender_advances_offset_on_zrpos() {
    let cfg = hex().with_block_size(4);
    let mut tx = Sender::new(cfg);
    tx.add_file("a.bin", b"0123456789".to_vec()).unwrap();

    let mut sink = Sink::default();
    sink.apply(tx.start());
    sink.take_out();
    sink.apply(tx.feed(&zrinit_frame()));
    sink.take_out();
    assert_eq!(tx.phase(), Phase::AwaitingZrpos);

    sink.apply(tx.feed(&zrpos_with_offset(6)));
    assert_eq!(tx.sent_bytes(), 6, "偏移应当直接推进到 6");
    assert_eq!(tx.phase(), Phase::AwaitingZack);
    assert!(sink.errors.is_empty(), "续传不是错误：{:?}", sink.errors);

    let out = sink.take_out();
    let head = first_header(&out);
    assert_eq!(head.frame, FrameType::ZDATA);
    assert_eq!(frame::decode_position(head.data), 6, "下一块应从 6 开始");
    assert_eq!(
        &head.data,
        &frame::encode_position(6),
        "位置字段是小端"
    );
}

/// 历史兼容：早期约定用 `ZSKIP(offset > 0)` 承载续传偏移，发送方仍容忍。
#[test]
fn sender_still_tolerates_legacy_zskip_offset() {
    let cfg = hex().with_block_size(4);
    let mut tx = Sender::new(cfg);
    tx.add_file("a.bin", b"0123456789".to_vec()).unwrap();

    let mut sink = Sink::default();
    sink.apply(tx.start());
    sink.take_out();
    sink.apply(tx.feed(&zrinit_frame()));
    sink.take_out();

    sink.apply(tx.feed(&zskip_with_offset(6)));
    assert_eq!(tx.sent_bytes(), 6, "旧约定仍应推进偏移");
    assert!(sink.errors.is_empty(), "不该报错：{:?}", sink.errors);
}

// ── 4. 续传拼接后与原文件逐字节一致 ────────────────────────────────────────

#[test]
fn resume_concatenation_matches_original() {
    let data: Vec<u8> = (0..2500u32).map(|i| (i % 251) as u8).collect();
    for resume in [1usize, 300, 1024, 2499] {
        let (wire, sink) = resume_loopback(resume, "big.bin", &data);
        assert!(sink.done, "接收方没有正常结束（resume={resume}）");
        assert_eq!(sink.file_done, 1, "FileDone 应当恰好一次");
        assert_eq!(
            sink.single().data,
            &data[resume..],
            "本次只应收到剩余字节（resume={resume}）"
        );

        let (payload, first_offset) = data_payload_sum(&wire);
        assert_eq!(
            payload,
            data.len() - resume,
            "线上只应传剩余字节（resume={resume}）"
        );
        assert_eq!(
            first_offset,
            Some(resume as u32),
            "第一块应从断点开始（resume={resume}）"
        );

        // 调用方按「追加写」拼起来，必须与原文件逐字节相同。
        let mut joined = data[..resume].to_vec();
        joined.extend_from_slice(&sink.single().data);
        assert_eq!(joined, data, "拼接结果与原文件不一致（resume={resume}）");
    }
}

// ── 5. 非法 / 越界应答不 panic ─────────────────────────────────────────────

#[test]
fn out_of_range_zskip_is_rejected_without_panic() {
    let mut tx = Sender::new(hex());
    tx.add_file("a.bin", b"hello".to_vec()).unwrap();
    let mut sink = Sink::default();
    sink.apply(tx.start());
    sink.take_out();
    sink.apply(tx.feed(&zrinit_frame()));
    sink.take_out();

    // 偏移大于文件长度：明确拒绝，不上抛 panic。
    sink.apply(tx.feed(&zskip_with_offset(1_000_000)));
    assert_eq!(tx.phase(), Phase::Aborted);
    assert!(matches!(sink.errors.last(), Some(Error::BadHeader { .. })));

    // 已经终态：再喂一次同样的帧也不 panic、不再产出动作。
    assert!(tx.feed(&zskip_with_offset(1_000_000)).is_empty());

    // 接收端：超过 32 位偏移上限直接拒绝（4 字节位置字段表达不了）。
    let mut rx = Receiver::new(hex());
    assert!(matches!(
        rx.resume_from(u64::from(u32::MAX) + 1),
        Err(Error::BadFileSpec { .. })
    ));
    // 握手开始之后不允许再改续传点。
    sink.apply(rx.start());
    assert_eq!(rx.resume_from(10), Err(Error::AlreadyStarted));
    assert_eq!(rx.resume_offset(), 0, "被拒绝的调用不应写入状态");
}

#[test]
fn resume_offset_beyond_declared_size_clamps() {
    let mut rx = Receiver::new(hex());
    rx.resume_from(100).unwrap();
    let mut sink = Sink::default();
    sink.apply(rx.start());
    sink.take_out();

    // 声明长度只有 5 字节：偏移钳到 5，不能溢出、不能让对端无限发。
    sink.apply(rx.feed(&zfile_frame("tiny.bin", 5, 0)));
    let head = first_header(&sink.take_out());
    assert_eq!(head.frame, FrameType::ZRPOS);
    assert_eq!(frame::decode_position(head.data), 5);
    assert_eq!(rx.received_bytes(), 5);

    // 直接收 ZEOF(5) 即可收尾，不报错。
    sink.apply(rx.feed(&zeof_frame(5)));
    assert_eq!(sink.file_done, 1);
    assert!(sink.errors.is_empty(), "钳位后不应报错：{:?}", sink.errors);
}

// ── 6. 无历史文件时退化为从头传 ────────────────────────────────────────────

#[test]
fn no_existing_file_starts_from_zero() {
    let mut rx = Receiver::new(hex());
    let mut sink = Sink::default();
    sink.apply(rx.start());
    sink.take_out();

    sink.apply(rx.feed(&zfile_frame("a.txt", 5, 0)));
    let head = first_header(&sink.take_out());
    assert_eq!(head.frame, FrameType::ZRPOS, "没有断点时应回 ZRPOS");
    assert_eq!(frame::decode_position(head.data), 0);
    assert_eq!(rx.received_bytes(), 0);

    sink.apply(rx.feed(&zdata_frame(0, b"hello")));
    sink.take_out();
    sink.apply(rx.feed(&zeof_frame(5)));
    assert_eq!(sink.single().data, b"hello");
    assert_eq!(sink.file_done, 1);
    assert!(sink.errors.is_empty(), "{:?}", sink.errors);
}

// 显式引用一次 `Config`，让「三种帧头的配置入口」在本文件里也被覆盖。
#[test]
fn resume_works_for_all_header_kinds() {
    let data: Vec<u8> = (0..1200u32).map(|i| (i % 241) as u8).collect();
    for kind in [HeaderKind::Hex, HeaderKind::Bin, HeaderKind::Bin32] {
        let cfg_config = Config::default().with_header(kind);
        let mut tx = Sender::new(cfg_config);
        tx.add_file("k.bin", data.clone()).unwrap();
        let mut rx = Receiver::new(cfg_config);
        rx.resume_from(400).unwrap();

        let mut tx_sink = Sink::default();
        let mut rx_sink = Sink::default();
        tx_sink.apply(tx.start());
        rx_sink.apply(rx.start());
        let mut steps = 0;
        loop {
            steps += 1;
            assert!(steps < 10_000, "闭环未收敛（{kind:?}）");
            let mut moved = false;
            let to_rx = tx_sink.take_out();
            if !to_rx.is_empty() {
                rx_sink.apply(rx.feed(&to_rx));
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
        assert!(tx_sink.errors.is_empty(), "{kind:?}: {:?}", tx_sink.errors);
        assert!(rx_sink.errors.is_empty(), "{kind:?}: {:?}", rx_sink.errors);
        assert_eq!(rx_sink.single().data, &data[400..], "{kind:?}");
    }
}

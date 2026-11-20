//! 两项 opt-in 增量：多文件批处理（`Config::batch`）与 `ZCRCG` 连发
//! （`Config::streaming`）。
//!
//! 全部用内存字节流驱动。默认配置（两项都关）必须与改动前逐字节一致——这里用
//! 「默认配置 vs 显式关掉」的对照测试钉住，不靠人眼比对。

mod common;

use common::*;
use term_zmodem::frame::{self, FrameEnd, FrameType, ParsedHeader, ParsedSubpacket};
use term_zmodem::{Action, Config, HeaderKind, Receiver, Sender};

// ── 夹具 ───────────────────────────────────────────────────────────────────

/// 两个文件的完整线上字节流：`ZFILE/ZDATA/ZEOF` ×2 + `ZFIN`。
fn two_file_stream() -> Vec<u8> {
    let mut stream = zfile_frame("a.txt", 5, 0);
    stream.extend(zdata_frame(0, b"hello"));
    stream.extend(zeof_frame(5));
    stream.extend(zfile_frame("b.txt", 3, 0));
    stream.extend(zdata_frame(0, b"abc"));
    stream.extend(zeof_frame(3));
    stream.extend(zfin_frame());
    stream
}

/// 收一个接收方从握手到结束的全部动作（含 `start` 的 `ZRINIT`）。
fn actions_of(rx: &mut Receiver, bytes: &[u8]) -> Vec<Action> {
    let mut out = rx.start();
    out.extend(rx.feed(bytes));
    out
}

/// 走一遍收发闭环，返回**发送侧线上字节**与接收侧 sink。
fn wire_of(cfg: Config, name: &str, data: &[u8]) -> (Vec<u8>, Sink) {
    let mut tx = Sender::new(cfg);
    tx.add_file(name, data.to_vec()).expect("排文件失败");
    let mut rx = Receiver::new(cfg);

    let mut tx_sink = Sink::default();
    let mut rx_sink = Sink::default();
    tx_sink.apply(tx.start());
    rx_sink.apply(rx.start());

    let mut wire = Vec::new();
    for _ in 0..10_000 {
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

/// 线上所有 `ZDATA` 子包的帧尾字节（按顺序）。
fn data_frame_ends(wire: &[u8]) -> Vec<u8> {
    let mut at = 0;
    let mut ends = Vec::new();
    while at < wire.len() {
        let ParsedHeader::Valid(head) = frame::parse_header(&wire[at..]) else {
            break;
        };
        at += head.consumed;
        if !matches!(
            head.frame,
            FrameType::ZFILE | FrameType::ZDATA | FrameType::ZSINIT
        ) {
            continue;
        }
        let ParsedSubpacket::Valid(packet) =
            frame::parse_subpacket(&wire[at..], head.kind.subpacket_crc())
        else {
            break;
        };
        at += packet.consumed;
        if head.frame != FrameType::ZDATA {
            continue;
        }
        // 连发时一个 ZDATA 帧头后面跟一串子包：ZCRCG 表示「后面还有」。
        let mut end = packet.end;
        loop {
            ends.push(end.as_byte());
            if end != FrameEnd::ZCRCG {
                break;
            }
            let ParsedSubpacket::Valid(next) =
                frame::parse_subpacket(&wire[at..], head.kind.subpacket_crc())
            else {
                return ends;
            };
            at += next.consumed;
            end = next.end;
        }
    }
    ends
}

/// 一段 `ZDATA` 帧：一个帧头 + 若干带指定帧尾的子包（模拟连发）。
fn multi_subpacket_frame(offset: u32, blocks: &[(&[u8], u8)]) -> Vec<u8> {
    let mut out = header(HeaderKind::Hex, 10, offset.to_le_bytes());
    for (data, end) in blocks {
        out.extend(subpacket(HeaderKind::Hex, data, *end));
    }
    out
}

// ── 2a 批处理 ──────────────────────────────────────────────────────────────

#[test]
fn batch_accepts_next_file_and_marks_each_boundary() {
    let mut rx = Receiver::new(hex().with_batch(true));
    let mut sink = Sink::default();
    sink.apply(actions_of(&mut rx, &two_file_stream()));

    assert_eq!(sink.files.len(), 2, "批处理应当收下两个文件");
    assert_eq!(sink.files[0].name, "a.txt");
    assert_eq!(sink.files[0].data, b"hello");
    assert_eq!(sink.files[1].name, "b.txt");
    assert_eq!(sink.files[1].data, b"abc");
    assert_eq!(sink.file_done, 2, "每个文件结束一次 FileDone（边界信号）");
    assert!(sink.done, "ZFIN 之后整体结束");
    assert!(sink.errors.is_empty(), "{:?}", sink.errors);
}

#[test]
fn batch_off_skips_the_second_file() {
    let mut rx = Receiver::new(hex());
    let mut sink = Sink::default();
    sink.apply(actions_of(&mut rx, &two_file_stream()));

    assert_eq!(sink.files.len(), 1, "默认单文件：第二个文件被跳过");
    assert_eq!(sink.file_done, 1);
    assert!(sink.done);
}

/// 硬要求：`batch = false` 与默认配置对同一段字节流产出**完全相同的动作序列**。
#[test]
fn batch_off_is_action_identical_to_default() {
    let stream = two_file_stream();
    let mut default_rx = Receiver::new(Config::default());
    let mut explicit_off = Receiver::new(Config::default().with_batch(false));
    assert_eq!(
        actions_of(&mut default_rx, &stream),
        actions_of(&mut explicit_off, &stream),
        "batch=false 必须与默认配置逐动作一致"
    );
}

#[test]
fn batch_sender_queues_second_file() {
    let cfg = hex().with_batch(true).with_block_size(4);
    let mut tx = Sender::new(cfg);
    tx.add_file("a.txt", b"hello".to_vec()).unwrap();
    tx.add_file("b.txt", b"world!".to_vec())
        .expect("批处理应当接受第二个文件");

    let mut rx = Receiver::new(cfg);
    let mut tx_sink = Sink::default();
    let mut rx_sink = Sink::default();
    tx_sink.apply(tx.start());
    rx_sink.apply(rx.start());
    for _ in 0..10_000 {
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
    assert!(tx_sink.done && rx_sink.done, "两个文件应当都走完");
    assert!(tx_sink.errors.is_empty(), "{:?}", tx_sink.errors);
    assert!(rx_sink.errors.is_empty(), "{:?}", rx_sink.errors);
    assert_eq!(rx_sink.files.len(), 2);
    assert_eq!(rx_sink.files[0].data, b"hello");
    assert_eq!(rx_sink.files[1].data, b"world!");
}

#[test]
fn single_file_sender_still_rejects_second_file() {
    let mut tx = Sender::new(hex());
    tx.add_file("a.txt", b"hello".to_vec()).unwrap();
    assert!(tx.add_file("b.txt", b"world".to_vec()).is_err());
}

/// 续传点只作用于第一个文件：批处理里第二个文件必须从 0 收，不能被同一个
/// 偏移「顺带跳过」。
#[test]
fn resume_applies_only_to_the_first_batch_file() {
    let mut rx = Receiver::new(hex().with_batch(true));
    rx.resume_from(3).unwrap();
    let mut sink = Sink::default();
    sink.apply(rx.start());
    sink.take_out();

    let mut stream = zfile_frame("one.bin", 5, 0);
    stream.extend(zdata_frame(3, b"de"));
    stream.extend(zeof_frame(5));
    stream.extend(zfile_frame("two.bin", 3, 0));
    stream.extend(zdata_frame(0, b"abc"));
    stream.extend(zeof_frame(3));
    sink.apply(rx.feed(&stream));

    assert!(sink.errors.is_empty(), "{:?}", sink.errors);
    assert_eq!(sink.files.len(), 2);
    assert_eq!(sink.files[0].data, b"de", "第一个文件从断点 3 续");
    assert_eq!(sink.files[1].data, b"abc", "第二个文件必须从头收");
}

// ── 2b ZCRCG 连发 ──────────────────────────────────────────────────────────

#[test]
fn streaming_sends_zcrcg_blocks_and_still_delivers_every_byte() {
    let data: Vec<u8> = (0..2500u32).map(|i| (i % 251) as u8).collect();
    let (wire, sink) = wire_of(hex().with_block_size(300).with_streaming(true), "s.bin", &data);

    assert!(sink.done);
    assert_eq!(sink.file_done, 1);
    assert_eq!(sink.single().data, data, "连发同样要逐字节一致");

    let ends = data_frame_ends(&wire);
    assert!(ends.len() > 1, "应当有多个数据块");
    assert_eq!(
        *ends.last().unwrap(),
        FrameEnd::ZCRCW.as_byte(),
        "最后一块必须是 ZCRCW（要一个 ZACK）"
    );
    assert!(
        ends[..ends.len() - 1]
            .iter()
            .all(|&end| end == FrameEnd::ZCRCG.as_byte()),
        "中间块应当是 ZCRCG（块间无应答）"
    );
}

#[test]
fn streaming_bad_block_requests_zrpos_of_received_bytes() {
    let mut rx = Receiver::new(hex());
    let mut sink = Sink::default();
    sink.apply(rx.start());
    sink.take_out();
    sink.apply(rx.feed(&zfile_frame("s.bin", 16, 0)));
    sink.take_out();
    assert_eq!(rx.received_bytes(), 0);

    // 两块好的（ZCRCG）+ 一块坏 CRC（ZCRCG）+ 一块好的（不该被采纳）。
    let mut bad = subpacket(HeaderKind::Hex, b"ijkl", b'i');
    let last = bad.len() - 1;
    bad[last] ^= 0xFF;

    let mut stream = header(HeaderKind::Hex, 10, 0u32.to_le_bytes());
    stream.extend(subpacket(HeaderKind::Hex, b"abcd", b'i'));
    stream.extend(subpacket(HeaderKind::Hex, b"efgh", b'i'));
    stream.extend_from_slice(&bad);
    stream.extend(subpacket(HeaderKind::Hex, b"mnop", b'i'));
    sink.apply(rx.feed(&stream));

    assert_eq!(rx.received_bytes(), 8, "只应采纳坏块之前的两块");
    let head = first_header(&sink.take_out());
    assert_eq!(head.frame, FrameType::ZRPOS, "连发里丢一块必须整段回退");
    assert_eq!(
        frame::decode_position(head.data),
        8,
        "ZRPOS 应当等于已收字节数（前两块长度）"
    );
    assert!(sink.errors.is_empty(), "{:?}", sink.errors);
}

#[test]
fn streaming_truncated_block_times_out_to_zrpos() {
    let mut rx = Receiver::new(hex());
    let mut sink = Sink::default();
    sink.apply(rx.start());
    sink.take_out();
    sink.apply(rx.feed(&zfile_frame("s.bin", 8, 0)));
    sink.take_out();

    // 第一块完整（ZCRCG），第二块只发一半就断了。
    let second = subpacket(HeaderKind::Hex, b"efgh", b'i');
    let mut stream = multi_subpacket_frame(0, &[(b"abcd", b'i')]);
    stream.extend_from_slice(&second[..second.len() / 2]);
    sink.apply(rx.feed(&stream));

    assert_eq!(rx.received_bytes(), 4, "半个块不能算收到");
    assert!(sink.take_out().is_empty(), "缺半个块时不该产出应答");

    // 调用方定时器判定静默：丢掉拼不完整的残块，从已收偏移整段回退。
    sink.apply(rx.on_timeout());
    let head = first_header(&sink.take_out());
    assert_eq!(head.frame, FrameType::ZRPOS);
    assert_eq!(frame::decode_position(head.data), 4);
}

/// 硬要求：`streaming = false` 与默认配置产出**逐字节相同**的线上数据。
#[test]
fn streaming_off_is_byte_identical_to_default() {
    let data: Vec<u8> = (0..1200u32).map(|i| (i % 239) as u8).collect();
    let (wire_default, sink_default) = wire_of(Config::default(), "x.bin", &data);
    let (wire_off, sink_off) = wire_of(Config::default().with_streaming(false), "x.bin", &data);
    assert_eq!(wire_default, wire_off, "streaming=false 必须逐字节一致");
    assert_eq!(sink_default.single().data, sink_off.single().data);
    assert!(
        data_frame_ends(&wire_default)
            .iter()
            .all(|&end| end == FrameEnd::ZCRCW.as_byte()),
        "默认路径仍然只用 ZCRCW"
    );
}

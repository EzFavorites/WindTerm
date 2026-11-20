//! 握手与帧头协商：控制帧永远 `ZHEX`，数据帧按对端 `CANFC32` 在 `ZBIN32`/`ZBIN` 之间选。
//!
//! 这是接真实 `sz`/`rz` 的第一道门槛：开场时双方还不知道链路是否 8-bit clean，
//! 可打印帧头是唯一安全的选择；只有对端在 `ZRINIT` 里明说支持 CRC-32，数据段才升级。

mod common;

use common::*;
use term_zmodem::frame::FrameType;
use term_zmodem::{Config, HeaderKind, Phase, Receiver, Sender};

/// ZHEX 帧头的前 4 个字节：`* * ZDLE 'B'`。
const HEX_START: [u8; 4] = [0x2A, 0x2A, 0x18, b'B'];
/// ZBIN32 帧头的前 3 个字节：`* ZDLE 'C'`。
const BIN32_START: [u8; 3] = [0x2A, 0x18, b'C'];
/// ZBIN 帧头的前 3 个字节：`* ZDLE 'A'`。
const BIN_START: [u8; 3] = [0x2A, 0x18, b'A'];

/// 排好文件并发出 `ZRQINIT`，返回发送方与那一帧的线上字节。
fn started(cfg: Config) -> (Sender, Vec<u8>) {
    let mut tx = Sender::new(cfg);
    tx.add_file("a.txt", b"hello".to_vec()).expect("排文件失败");
    let mut sink = Sink::default();
    sink.apply(tx.start());
    (tx, sink.take_out())
}

#[test]
fn sender_handshake_is_hex() {
    let (_tx, out) = started(bin32());
    assert_eq!(out[..4], HEX_START, "ZRQINIT 必须用可打印帧头开场");
    let header = first_header(&out);
    assert_eq!(header.frame, FrameType::ZRQINIT);
    assert_eq!(header.kind, HeaderKind::Hex);
}

#[test]
fn receiver_handshake_is_hex_and_declares_canfc32() {
    let mut rx = Receiver::new(bin32());
    let mut sink = Sink::default();
    sink.apply(rx.start());
    let out = sink.take_out();
    assert_eq!(out[..4], HEX_START, "ZRINIT 必须用可打印帧头");
    let header = first_header(&out);
    assert_eq!(header.frame, FrameType::ZRINIT);
    assert_eq!(header.kind, HeaderKind::Hex);
    // 能力位（`ZF0`）在头数据的**第 4 字节**，不是第 1 字节。
    assert_ne!(header.data[3] & 0x20, 0, "ZRINIT 必须宣告 CANFC32");
}

#[test]
fn sender_upgrades_data_frames_to_bin32() {
    let (mut tx, _) = started(bin32());
    let mut sink = Sink::default();

    // 对端宣告 CANFC32：数据段升级。
    sink.apply(tx.feed(&zrinit_frame_with_flags(HeaderKind::Hex, 0x60)));
    let file = sink.take_out();
    assert_eq!(file[..3], BIN32_START, "ZFILE 应当用 ZBIN32");
    let header = first_header(&file);
    assert_eq!(header.frame, FrameType::ZFILE);
    assert_eq!(header.kind, HeaderKind::Bin32);
    assert_eq!(tx.phase(), Phase::AwaitingZrpos);

    sink.apply(tx.feed(&zrpos_frame(0)));
    let data = sink.take_out();
    assert_eq!(
        first_header(&data).kind,
        HeaderKind::Bin32,
        "ZDATA 应当用 ZBIN32"
    );
    assert_eq!(first_header(&data).frame, FrameType::ZDATA);

    sink.apply(tx.feed(&zack_frame(5)));
    let eof = sink.take_out();
    assert_eq!(
        first_header(&eof).kind,
        HeaderKind::Bin32,
        "ZEOF 也是数据帧"
    );
    assert_eq!(first_header(&eof).frame, FrameType::ZEOF);
}

#[test]
fn sender_falls_back_to_bin_without_canfc32() {
    let (mut tx, _) = started(bin32());
    let mut sink = Sink::default();

    // 对端没宣告 CANFC32（flags 只有 ESCCTL）：只能发 CRC-16 的 ZBIN。
    sink.apply(tx.feed(&zrinit_frame_with_flags(HeaderKind::Hex, 0x40)));
    let file = sink.take_out();
    assert_eq!(file[..3], BIN_START, "对端不支持 CRC-32 时应当退到 ZBIN");
    let header = first_header(&file);
    assert_eq!(header.frame, FrameType::ZFILE);
    assert_eq!(header.kind, HeaderKind::Bin);
    assert_eq!(header.consumed, 10, "无转义时 ZBIN 帧头 10 字节");
}

#[test]
fn bin_config_never_upgrades_even_if_peer_supports_it() {
    let (mut tx, _) = started(bin());
    let mut sink = Sink::default();

    sink.apply(tx.feed(&zrinit_frame_with_flags(HeaderKind::Hex, 0x60)));
    let header = first_header(&sink.take_out());
    assert_eq!(header.kind, HeaderKind::Bin, "配置钉死 16 位就不该升级");
}

#[test]
fn hex_config_keeps_data_frames_printable() {
    let (mut tx, _) = started(hex());
    let mut sink = Sink::default();

    sink.apply(tx.feed(&zrinit_frame_with_flags(HeaderKind::Hex, 0x60)));
    let file = sink.take_out();
    assert_eq!(file[..4], HEX_START, "配置选 Hex 时数据帧也用可打印帧头");
    assert_eq!(first_header(&file).kind, HeaderKind::Hex);
}

#[test]
fn sender_zfin_stays_hex_after_upgrade() {
    let (mut tx, _) = started(bin32());
    let mut sink = Sink::default();

    sink.apply(tx.feed(&zrinit_frame_with_flags(HeaderKind::Hex, 0x60)));
    sink.take_out();
    sink.apply(tx.feed(&zrpos_frame(0)));
    sink.take_out();
    sink.apply(tx.feed(&zack_frame(5)));
    assert_eq!(
        first_header(&sink.take_out()).kind,
        HeaderKind::Bin32,
        "此时数据段已经是 ZBIN32"
    );

    // 对端再发 ZRINIT（表示收完 ZEOF）→ 我们回 ZFIN：控制帧，必须回到 ZHEX。
    sink.apply(tx.feed(&zrinit_frame_with_flags(HeaderKind::Hex, 0x60)));
    let fin = sink.take_out();
    assert_eq!(fin[..4], HEX_START, "ZFIN 是控制帧，必须 ZHEX");
    assert_eq!(first_header(&fin).frame, FrameType::ZFIN);
    assert_eq!(tx.phase(), Phase::AwaitingZfin);
}

#[test]
fn receiver_answers_with_hex_even_for_bin32_data() {
    let mut rx = Receiver::new(bin32());
    let mut sink = Sink::default();
    sink.apply(rx.start());
    sink.take_out();

    // 对端用 ZBIN32 发数据段，我们的应答仍然全是 ZHEX。
    sink.apply(rx.feed(&zfile_frame_kind(HeaderKind::Bin32, "a.txt", 5, 0)));
    assert_eq!(sink.take_out(), zrpos_frame(0), "ZRPOS 必须是 ZHEX");

    sink.apply(rx.feed(&zdata_frame_kind(HeaderKind::Bin32, 0, b"hello")));
    assert_eq!(sink.take_out(), zack_frame(5), "ZACK 必须是 ZHEX");

    sink.apply(rx.feed(&zeof_frame_kind(HeaderKind::Bin32, 5)));
    assert_eq!(sink.take_out(), zrinit_frame(), "ZRINIT 必须是 ZHEX");

    sink.apply(rx.feed(&zfin_frame_kind(HeaderKind::Bin32)));
    assert_eq!(sink.take_out(), zfin_frame(), "ZFIN 必须是 ZHEX");
    assert!(sink.done);
    assert_eq!(sink.single().data, b"hello");
}


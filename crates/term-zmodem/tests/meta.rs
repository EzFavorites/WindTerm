//! `ZFILE` 里的 mtime / 权限位：解析出来必须保真，缺字段或非法时是 `None`。
//!
//! `None` 不是「0」——`0` 会被调用方当成 1970 年 / 权限 `000`，所以这里专门钉住
//! 「不编造默认值」这条。`Action` 是 `term-xfer` 共用的事件形状，放不下这些字段，
//! 调用方在 `FileStart` 时从 [`Receiver::file_mtime`] / [`Receiver::file_mode`] 读。

mod common;

use common::*;
use term_zmodem::{HeaderKind, Receiver};

/// 拼一个 `ZFILE` 帧：文件名 + NUL + 调用方给的文件信息字段。
fn zfile_with_info(name: &str, info: &[u8]) -> Vec<u8> {
    let mut payload = name.as_bytes().to_vec();
    payload.push(0);
    payload.extend_from_slice(info);
    let mut out = header(HeaderKind::Hex, 4, [0; 4]);
    out.extend(subpacket(HeaderKind::Hex, &payload, b'k'));
    out
}

/// 建接收方、握手、吃一个 `ZFILE`，返回接收方（动作丢掉，只看访问器）。
fn after_zfile(info: &[u8]) -> Receiver {
    let mut rx = Receiver::new(hex());
    let mut sink = Sink::default();
    sink.apply(rx.start());
    sink.apply(rx.feed(&zfile_with_info("a.bin", info)));
    assert!(sink.errors.is_empty(), "{:?}", sink.errors);
    rx
}

#[test]
fn mtime_and_mode_are_parsed() {
    // mtime / mode 在线上都是**八进制字符串**。
    let info = format!("5 {:o} 100644 0", 1_700_000_000u32);
    let rx = after_zfile(info.as_bytes());
    assert_eq!(rx.file_size(), Some(5));
    assert_eq!(rx.file_mtime(), Some(1_700_000_000));
    assert_eq!(rx.file_mode(), Some(0o644));
}

#[test]
fn missing_meta_fields_are_none() {
    // 只有长度，其余字段（mtime / mode / 序号）全没给。
    let rx = after_zfile(b"5");
    assert_eq!(rx.file_size(), Some(5));
    assert_eq!(rx.file_mtime(), None, "缺 mtime 必须是 None，不能是 0");
    assert_eq!(rx.file_mode(), None, "缺 mode 必须是 None，不能是 0");

    // 连长度都没有：三个都是 None。
    let rx = after_zfile(b"");
    assert_eq!(rx.file_size(), None);
    assert_eq!(rx.file_mtime(), None);
    assert_eq!(rx.file_mode(), None);
}

#[test]
fn invalid_octal_fields_are_none() {
    let rx = after_zfile(b"5 9z zz 0\0");
    assert_eq!(rx.file_size(), Some(5));
    assert_eq!(rx.file_mtime(), None, "非法八进制不能糊涂解析成 0");
    assert_eq!(rx.file_mode(), None, "非法八进制不能糊涂解析成 0");
}

#[test]
fn mode_is_masked_to_low_12_bits() {
    let rx = after_zfile(b"5 1 100644 0\0");
    assert_eq!(rx.file_mode(), Some(0o644), "100644 & 0o7777 == 0o644");

    let rx = after_zfile(b"5 1 100777 0\0");
    assert_eq!(rx.file_mode(), Some(0o777));

    // 对端只发 3 位权限：原样保留。
    let rx = after_zfile(b"5 1 600 0\0");
    assert_eq!(rx.file_mode(), Some(0o600));
}

#[test]
fn meta_is_reset_for_each_file() {
    use term_zmodem::Phase;

    let mut rx = Receiver::new(hex().with_batch(true));
    let mut sink = Sink::default();
    sink.apply(rx.start());
    let first = format!("0 {:o} 100600 0", 1_700_000_000u32);
    sink.apply(rx.feed(&zfile_with_info("one.bin", first.as_bytes())));
    assert_eq!(rx.file_mtime(), Some(1_700_000_000));
    assert_eq!(rx.file_mode(), Some(0o600));

    sink.apply(rx.feed(&zeof_frame(0)));
    assert_eq!(rx.phase(), Phase::AwaitingZfin);
    sink.take_out();

    // 第二个文件没带这些字段：不能沿用上一个文件的元信息。
    sink.apply(rx.feed(&zfile_with_info("two.bin", b"0")));
    assert_eq!(rx.file_mtime(), None);
    assert_eq!(rx.file_mode(), None);
    assert!(sink.errors.is_empty(), "{:?}", sink.errors);
}

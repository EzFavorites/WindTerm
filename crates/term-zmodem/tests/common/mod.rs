//! 测试公用夹具：动作收集器、任意切分器、手工构造线上字节流的帧工厂。
//!
//! 帧工厂是**手写**的（不复用 `frame::encode_header` / `encode_subpacket`），
//! 这样集成测试才真的在验证线格式，而不是「用实现验证实现」。只借用两个 CRC：
//! [`term_zmodem::frame::crc16`] 与 [`term_zmodem::frame::crc32`]，它们各自有
//! 已知向量测试。
#![allow(dead_code)]

use term_zmodem::{Action, Config, CrcKind, Error, HeaderKind, Receiver, Sender};

/// 六进制帧头的配置（测试里大量手写字节流都是 ZHEX）。
pub fn hex() -> Config {
    Config::default().with_header(HeaderKind::Hex)
}

/// 16 位 CRC 二进制帧头的配置。
pub fn bin() -> Config {
    Config::default().with_header(HeaderKind::Bin)
}

/// 32 位 CRC 二进制帧头的配置（也是默认值）。
pub fn bin32() -> Config {
    Config::default()
}

/// 收到/发出的一个文件。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct GotFile {
    pub name: String,
    pub size: Option<u64>,
    pub data: Vec<u8>,
}

/// 把状态机的 action 序列折叠成可断言的事实。
#[derive(Debug, Default)]
pub struct Sink {
    /// 按 `FileStart` 顺序记录的文件（含内容）。
    pub files: Vec<GotFile>,
    /// 收到过 `Action::Done`。
    pub done: bool,
    /// 收到过的错误。
    pub errors: Vec<Error>,
    /// `FileDone` 出现次数。
    pub file_done: usize,
    /// 要写给对端的字节（按序累积，`take_out` 取走）。
    pub out: Vec<u8>,
}

impl Sink {
    pub fn apply(&mut self, actions: Vec<Action>) {
        for action in actions {
            match action {
                Action::Send(bytes) => self.out.extend_from_slice(&bytes),
                Action::Data(bytes) => {
                    assert!(!bytes.is_empty(), "Action::Data 不得为空");
                    match self.files.last_mut() {
                        Some(file) => file.data.extend_from_slice(&bytes),
                        None => panic!("Action::Data 出现在 FileStart 之前"),
                    }
                }
                Action::FileStart { name, size } => self.files.push(GotFile {
                    name,
                    size,
                    data: Vec::new(),
                }),
                Action::FileDone => self.file_done += 1,
                Action::Done => self.done = true,
                Action::Error(err) => self.errors.push(err),
            }
        }
    }

    /// 取走自上次调用以来累积的「发给对端」的字节。
    pub fn take_out(&mut self) -> Vec<u8> {
        std::mem::take(&mut self.out)
    }

    /// 恰好一个文件时把它取出来。
    pub fn single(&self) -> &GotFile {
        assert_eq!(self.files.len(), 1, "期望恰好一个文件");
        &self.files[0]
    }
}

/// 把一段字节喂给状态机时用的切分方式。
#[derive(Debug, Clone, Copy)]
pub enum Cut {
    /// 整块一次喂。
    Whole,
    /// 每次一个字节。
    OneByte,
    /// 用 xorshift 生成的伪随机长度（1..=7）切。
    Random(u64),
}

impl Cut {
    /// 按当前方式切分；空输入返回空列表。
    pub fn cuts(self, bytes: &[u8]) -> Vec<Vec<u8>> {
        if bytes.is_empty() {
            return Vec::new();
        }
        match self {
            Cut::Whole => vec![bytes.to_vec()],
            Cut::OneByte => bytes.chunks(1).map(|chunk| chunk.to_vec()).collect(),
            Cut::Random(seed) => {
                let mut out = Vec::new();
                let mut state = seed | 1;
                let mut at = 0;
                while at < bytes.len() {
                    state ^= state << 13;
                    state ^= state >> 7;
                    state ^= state << 17;
                    let take = (state % 7 + 1) as usize;
                    let end = (at + take).min(bytes.len());
                    out.push(bytes[at..end].to_vec());
                    at = end;
                }
                out
            }
        }
    }
}

// ── 手写帧工厂 ─────────────────────────────────────────────────────────────

fn push_hex(out: &mut Vec<u8>, byte: u8) {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    out.push(DIGITS[(byte >> 4) as usize]);
    out.push(DIGITS[(byte & 0x0F) as usize]);
}

/// 按 `lrzsz` 的 `zsendline()`（`zm.c:914-954`）转义：`ZDLE`/`XON`/`XOFF`/`DLE` 与
/// 它们的 `|0x80` 变体恒转义，`CR`（真机上只在 `Zctlesc` 或跟在 `'@'` 之后才转义）
/// 这里一律转义——多转义对解码方无害。
fn escape(out: &mut Vec<u8>, byte: u8) {
    if matches!(
        byte,
        0x0D | 0x8D | 0x10 | 0x90 | 0x11 | 0x91 | 0x13 | 0x93 | 0x18
    ) {
        out.push(0x18);
        out.push(byte ^ 0x40);
    } else {
        out.push(byte);
    }
}

/// 十六进制帧头：`* * ZDLE 'B'` + 类型(2) + 数据(8) + CRC-16(4) + `CR 0x8A`(+`XON`)。
///
/// 终止符照 `lrzsz` 的 `zshhdr()`：第二个字节是 `0x8A`（`LF|0x80`），并且除
/// `ZACK`(3) / `ZFIN`(8) 之外再跟一个裸 `XON`。
pub fn hex_header(frame: u8, data: [u8; 4]) -> Vec<u8> {
    let mut out = vec![0x2A, 0x2A, 0x18, b'B'];
    let mut crc_input = vec![frame];
    crc_input.extend_from_slice(&data);
    let crc = term_zmodem::frame::crc16(&crc_input);
    for byte in [
        frame,
        data[0],
        data[1],
        data[2],
        data[3],
        (crc >> 8) as u8,
        (crc & 0x00FF) as u8,
    ] {
        push_hex(&mut out, byte);
    }
    out.extend_from_slice(&[0x0D, 0x8A]);
    if !matches!(frame, 3 | 8) {
        out.push(0x11);
    }
    out
}

/// 二进制帧头：`* ZDLE 'A'|'C'` + 类型 + 4 字节头 + CRC，逐字节转义。
pub fn bin_header(bin32: bool, frame: u8, data: [u8; 4]) -> Vec<u8> {
    let mut out = vec![0x2A, 0x18, if bin32 { b'C' } else { b'A' }];
    let mut covered = vec![frame];
    covered.extend_from_slice(&data);
    for &byte in &covered {
        escape(&mut out, byte);
    }
    if bin32 {
        // 小端：低字节在前（照 lrzsz 的 zsbh32）。
        let crc = term_zmodem::frame::crc32(&covered);
        for shift in [0, 8, 16, 24] {
            escape(&mut out, (crc >> shift) as u8);
        }
    } else {
        let crc = term_zmodem::frame::crc16(&covered);
        escape(&mut out, (crc >> 8) as u8);
        escape(&mut out, (crc & 0x00FF) as u8);
    }
    out
}

/// 按帧头类型编码一个帧头（测试里构造混流用）。
pub fn header(kind: HeaderKind, frame: u8, data: [u8; 4]) -> Vec<u8> {
    match kind {
        HeaderKind::Hex => hex_header(frame, data),
        HeaderKind::Bin => bin_header(false, frame, data),
        HeaderKind::Bin32 => bin_header(true, frame, data),
    }
}

/// 数据子包：转义后的载荷 + `ZDLE` + 帧尾 + CRC。
///
/// 与 `lrzsz` 一致：CRC 覆盖**载荷 + 帧尾字节**；CRC-16 高字节在前、
/// CRC-32 低字节在前。
pub fn subpacket_crc(data: &[u8], end: u8, crc: CrcKind) -> Vec<u8> {
    let mut out = Vec::new();
    for &byte in data {
        escape(&mut out, byte);
    }
    out.push(0x18);
    out.push(end);
    let mut covered = data.to_vec();
    covered.push(end);
    match crc {
        CrcKind::Crc16 => {
            let value = term_zmodem::frame::crc16(&covered);
            escape(&mut out, (value >> 8) as u8);
            escape(&mut out, (value & 0x00FF) as u8);
        }
        CrcKind::Crc32 => {
            let value = term_zmodem::frame::crc32(&covered);
            for shift in [0, 8, 16, 24] {
                escape(&mut out, (value >> shift) as u8);
            }
        }
    }
    out
}

/// 数据子包，CRC 宽度跟帧头类型走（与协议一致）。
pub fn subpacket(kind: HeaderKind, data: &[u8], end: u8) -> Vec<u8> {
    subpacket_crc(data, end, kind.subpacket_crc())
}

/// `ZFILE` 子包的线上内容：`名字 \0 长度 mtime mode serial \0`。
pub fn file_info(name: &str, size: u64, mtime: u32) -> Vec<u8> {
    let mut out = Vec::new();
    out.extend_from_slice(name.as_bytes());
    out.push(0);
    out.extend_from_slice(format!("{size} {mtime:o} 100644 0").as_bytes());
    out.push(0);
    out
}

pub fn zfile_frame_kind(kind: HeaderKind, name: &str, size: u64, mtime: u32) -> Vec<u8> {
    let mut out = header(kind, 4, [0; 4]);
    out.extend(subpacket(kind, &file_info(name, size, mtime), b'k'));
    out
}

pub fn zfile_frame(name: &str, size: u64, mtime: u32) -> Vec<u8> {
    zfile_frame_kind(HeaderKind::Hex, name, size, mtime)
}

pub fn zdata_frame_kind(kind: HeaderKind, offset: u32, payload: &[u8]) -> Vec<u8> {
    // 位置字段是小端（`lrzsz` 的 `stohdr()` / `ZP0 == 0`）。
    let mut out = header(kind, 10, offset.to_le_bytes());
    out.extend(subpacket(kind, payload, b'k'));
    out
}

pub fn zdata_frame(offset: u32, payload: &[u8]) -> Vec<u8> {
    zdata_frame_kind(HeaderKind::Hex, offset, payload)
}

pub fn zeof_frame_kind(kind: HeaderKind, offset: u32) -> Vec<u8> {
    header(kind, 11, offset.to_le_bytes())
}

pub fn zeof_frame(offset: u32) -> Vec<u8> {
    zeof_frame_kind(HeaderKind::Hex, offset)
}

pub fn zfin_frame_kind(kind: HeaderKind) -> Vec<u8> {
    header(kind, 8, [0; 4])
}

pub fn zfin_frame() -> Vec<u8> {
    zfin_frame_kind(HeaderKind::Hex)
}

/// `ZRINIT` 的头数据：`[缓冲低字节, 缓冲高字节, 0, 能力位]`。
///
/// `lrzsz` 的 `tryz()`（`lrz.c:1648-1664`）先 `stohdr(缓冲大小)`（小端铺进
/// `ZP0..ZP2`），再 `Txhdr[ZF0] = flags` 覆盖**第 4 字节**（`zmodem.h` 里 `ZF0 = 3`）。
pub fn zrinit_frame_kind(kind: HeaderKind) -> Vec<u8> {
    header(kind, 1, [0x00, 0x20, 0x00, 0x60])
}

/// 带指定能力位的 `ZRINIT`（`flags` 落在头数据的**第 4 字节**，即 `ZF0`）。
pub fn zrinit_frame_with_flags(kind: HeaderKind, flags: u8) -> Vec<u8> {
    header(kind, 1, [0x00, 0x20, 0x00, flags])
}

pub fn zrinit_frame() -> Vec<u8> {
    zrinit_frame_kind(HeaderKind::Hex)
}

pub fn zrpos_frame_kind(kind: HeaderKind, offset: u32) -> Vec<u8> {
    header(kind, 9, offset.to_le_bytes())
}

pub fn zrpos_frame(offset: u32) -> Vec<u8> {
    zrpos_frame_kind(HeaderKind::Hex, offset)
}

pub fn zack_frame_kind(kind: HeaderKind, offset: u32) -> Vec<u8> {
    header(kind, 3, offset.to_le_bytes())
}

pub fn zack_frame(offset: u32) -> Vec<u8> {
    zack_frame_kind(HeaderKind::Hex, offset)
}

pub fn znak_frame_kind(kind: HeaderKind) -> Vec<u8> {
    header(kind, 6, [0; 4])
}

pub fn znak_frame() -> Vec<u8> {
    znak_frame_kind(HeaderKind::Hex)
}

pub fn zskip_frame_kind(kind: HeaderKind) -> Vec<u8> {
    header(kind, 5, [0; 4])
}

pub fn zskip_frame() -> Vec<u8> {
    zskip_frame_kind(HeaderKind::Hex)
}

/// `ZSINIT` 帧（注意序列在随后的子包里）。
pub fn zsinit_frame(attention: &[u8]) -> Vec<u8> {
    let mut out = hex_header(2, [0; 4]);
    out.extend(subpacket_crc(attention, b'k', CrcKind::Crc16));
    out
}

// ── 驱动 ───────────────────────────────────────────────────────────────────

/// 断言 `bytes` 开头是一个完整帧头，并把它解出来。
pub fn first_header(bytes: &[u8]) -> term_zmodem::frame::Header {
    match term_zmodem::frame::parse_header(bytes) {
        term_zmodem::frame::ParsedHeader::Valid(header) => header,
        other => panic!("期望开头是完整帧头，实际：{other:?}（前 24 字节：{:02x?}）", &bytes[..bytes.len().min(24)]),
    }
}

/// 把 `bytes` 按 `cut` 喂给接收方，把动作收进 `sink`。
pub fn drive_receiver(rx: &mut Receiver, sink: &mut Sink, bytes: &[u8], cut: Cut) {
    for chunk in cut.cuts(bytes) {
        let actions = rx.feed(&chunk);
        sink.apply(actions);
    }
}

/// 收发闭环：把 Sender 的输出直接喂给 Receiver，直到双方都不再说话。
///
/// `tx_cfg` / `rx_cfg` 可以不同——这正是「发送方用 ZBIN32、接收方回应 ZHEX」这类
/// 跨格式场景的测试入口。
///
/// 返回 `(发送侧 sink, 接收侧 sink)`。不做超时注入——健康路径上不应该需要重传；
/// 需要重传的路径在 `tests/errors.rs` 里手工驱动。
pub fn loopback_as(
    tx_cfg: Config,
    rx_cfg: Config,
    name: &str,
    data: &[u8],
    cut: Cut,
) -> (Sink, Sink) {
    let mut tx = Sender::new(tx_cfg);
    tx.add_file(name, data.to_vec()).expect("排文件失败");
    let mut rx = Receiver::new(rx_cfg);

    let mut tx_sink = Sink::default();
    let mut rx_sink = Sink::default();
    tx_sink.apply(tx.start());
    rx_sink.apply(rx.start());

    let mut steps = 0;
    loop {
        steps += 1;
        assert!(steps < 10_000, "闭环未收敛（疑似重传风暴）");

        let mut moved = false;
        let to_rx = tx_sink.take_out();
        if !to_rx.is_empty() {
            for chunk in cut.cuts(&to_rx) {
                let actions = rx.feed(&chunk);
                rx_sink.apply(actions);
            }
            moved = true;
        }
        let to_tx = rx_sink.take_out();
        if !to_tx.is_empty() {
            for chunk in cut.cuts(&to_tx) {
                let actions = tx.feed(&chunk);
                tx_sink.apply(actions);
            }
            moved = true;
        }
        if !moved {
            break;
        }
    }
    (tx_sink, rx_sink)
}

/// 收发闭环（两边同一份配置）。
pub fn loopback(cfg: Config, name: &str, data: &[u8], cut: Cut) -> (Sink, Sink) {
    loopback_as(cfg, cfg, name, data, cut)
}

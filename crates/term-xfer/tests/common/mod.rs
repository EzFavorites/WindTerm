//! 测试公用夹具：动作收集器、任意切分器、手工构造线上字节流的帧工厂。
//!
//! 帧工厂是**手写**的（不复用 `packet::encode_frame`），这样测试才真的在验证线格式，
//! 而不是「用实现验证实现」。只有 CRC 借用 [`term_xfer::packet::crc16`]，它自己有一
//! 条已知向量测试（`"123456789" → 0x31C3`）。
#![allow(dead_code)]

use term_xfer::{Action, Config, Error, Receiver, Sender};

/// 收到/发出的一个文件。
#[derive(Debug, Clone, PartialEq, Eq)]
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

    /// 第一个文件的内容（单文件用例的便捷入口）。
    pub fn single_data(&self) -> &[u8] {
        assert_eq!(self.files.len(), 1, "期望恰好一个文件，实际 {}", self.files.len());
        &self.files[0].data
    }

    /// 最后一个错误。
    pub fn last_error(&self) -> &Error {
        self.errors.last().expect("期望至少一个错误")
    }
}

/// 切分策略。同一段字节流用不同策略喂进去，结果必须一致。
#[derive(Debug, Clone, Copy)]
pub enum Cut {
    /// 整段一次喂完。
    Whole,
    /// 逐字节喂（最坏情况的碎片化）。
    OneByte,
    /// 伪随机块长（1..=9 字节），`seed` 不能为 0。
    Random(u64),
}

impl Cut {
    pub fn splits(&self, data: &[u8]) -> Vec<Vec<u8>> {
        match *self {
            Cut::Whole => vec![data.to_vec()],
            Cut::OneByte => data.iter().map(|b| vec![*b]).collect(),
            Cut::Random(seed) => {
                assert!(seed != 0, "seed 不能为 0（xorshift 会退化成 0）");
                let mut state = seed;
                let mut out = Vec::new();
                let mut at = 0usize;
                while at < data.len() {
                    let len = (xorshift(&mut state) % 9) as usize + 1;
                    let end = (at + len).min(data.len());
                    out.push(data[at..end].to_vec());
                    at = end;
                }
                out
            }
        }
    }
}

fn xorshift(state: &mut u64) -> u64 {
    let mut x = *state;
    x ^= x << 13;
    x ^= x >> 7;
    x ^= x << 17;
    *state = x;
    x
}

/// 接收方跑完一整段远端字节流。
pub fn run_receiver(cfg: Config, stream: &[u8], cut: Cut) -> Sink {
    let mut rx = Receiver::new(cfg);
    let mut sink = Sink::default();
    sink.apply(rx.start());
    for chunk in cut.splits(stream) {
        sink.apply(rx.feed(&chunk));
    }
    sink
}

/// 全双工闭环：Sender ↔ Receiver 直连，把字节在两边来回搬运直到都完成。
pub fn loopback(cfg: Config, files: &[(&str, Vec<u8>)], cut: Cut) -> (Sink, Sink) {
    let mut tx = Sender::new(cfg);
    for (name, data) in files {
        tx.add_file(*name, data.clone()).expect("入队失败");
    }
    let mut rx = Receiver::new(cfg);

    let mut rx_sink = Sink::default();
    let mut tx_sink = Sink::default();

    rx_sink.apply(rx.start());
    let mut to_tx = rx_sink.take_out();
    let mut to_rx: Vec<u8> = Vec::new();

    for _ in 0..500 {
        if !to_tx.is_empty() {
            let bytes = std::mem::take(&mut to_tx);
            for chunk in cut.splits(&bytes) {
                tx_sink.apply(tx.feed(&chunk));
            }
            to_rx.extend(tx_sink.take_out());
        }
        if !to_rx.is_empty() {
            let bytes = std::mem::take(&mut to_rx);
            for chunk in cut.splits(&bytes) {
                rx_sink.apply(rx.feed(&chunk));
            }
            to_tx.extend(rx_sink.take_out());
        }
        if tx_sink.done && rx_sink.done {
            return (tx_sink, rx_sink);
        }
        if to_tx.is_empty() && to_rx.is_empty() {
            break;
        }
    }
    panic!(
        "闭环未收敛：tx.done={} rx.done={} tx.errors={:?} rx.errors={:?}",
        tx_sink.done, rx_sink.done, tx_sink.errors, rx_sink.errors
    );
}

// ── 帧工厂（手工构造线上字节）─────────────────────────────────────────────

/// 128 字节块（CRC 帧）。
pub fn frame128_crc(block: u8, payload: &[u8]) -> Vec<u8> {
    frame_crc(block, &pad_to(payload, 128))
}

/// 1024 字节块（CRC 帧）。
pub fn frame1024_crc(block: u8, payload: &[u8]) -> Vec<u8> {
    frame_crc(block, &pad_to(payload, 1024))
}

/// 128 字节块（8 位校验和帧）。
pub fn frame128_sum(block: u8, payload: &[u8]) -> Vec<u8> {
    let payload = pad_to(payload, 128);
    let mut out = vec![term_xfer::packet::SOH, block, !block];
    out.extend_from_slice(&payload);
    out.push(term_xfer::packet::sum8(&payload));
    out
}

fn frame_crc(block: u8, payload: &[u8]) -> Vec<u8> {
    let header = match payload.len() {
        128 => term_xfer::packet::SOH,
        1024 => term_xfer::packet::STX,
        other => panic!("帧载荷必须 128 或 1024 字节，实际 {other}"),
    };
    let mut out = vec![header, block, !block];
    out.extend_from_slice(payload);
    let crc = term_xfer::packet::crc16(payload);
    out.push((crc >> 8) as u8);
    out.push((crc & 0xFF) as u8);
    out
}

/// 用 `SUB` 填充到指定长度（超长直接 panic）。
pub fn pad_to(payload: &[u8], size: usize) -> Vec<u8> {
    assert!(payload.len() <= size, "载荷 {} 超过块长 {size}", payload.len());
    let mut out = payload.to_vec();
    out.resize(size, term_xfer::packet::SUB);
    out
}

/// YMODEM 文件头块：`name \0 size \0` + 0 填充。
pub fn header_frame(name: &str, size: u64) -> Vec<u8> {
    let mut payload = Vec::new();
    payload.extend_from_slice(name.as_bytes());
    payload.push(0);
    payload.extend_from_slice(size.to_string().as_bytes());
    payload.push(0);
    payload.resize(128, 0);
    frame_crc(0, &payload)
}

/// YMODEM 批结束空头块（全 0 载荷）。
pub fn empty_header_frame() -> Vec<u8> {
    frame_crc(0, &[0u8; 128])
}

/// 从「一帧」字节里取出块号（用于断言发送方的块号序列）。
pub fn block_no_of(frame: &[u8]) -> u8 {
    frame[1]
}

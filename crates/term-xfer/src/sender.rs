//! 发送方状态机：喂远端字节 → 该发什么。

use crate::action::{Action, Phase};
use crate::config::{Checksum, Config};
use crate::error::{Error, Fault};
use crate::packet;

/// 队列里的一个待发文件。
#[derive(Debug, Clone)]
struct Queued {
    name: String,
    data: Vec<u8>,
}

/// 当前正在发的那一帧是「文件头块」还是「批次结束空头块」。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum HeaderKind {
    File,
    EndOfBatch,
}

/// 发送方：把文件编码成帧，按对端的应答推进。
///
/// # 用法
///
/// ```text
/// let mut tx = Sender::new(Config::for_protocol(Protocol::Ymodem));
/// tx.add_file("a.txt", b"hello".to_vec())?;
/// tx.add_file("b.bin", big)?;
/// // 对端先喊 'C'，收到就自动开始发；之后每来一个 ACK/NAK 就推进：
/// for a in tx.feed(&chunk) { /* Send → 写进会话 */ }
/// // 调用方定时器判定对端静默：
/// for a in tx.on_timeout() { /* 重发上一帧，超次则 Error */ }
/// ```
///
/// # 校验方式由对端定
///
/// [`Config::checksum`] 只是「还没听到对端说话时的初值」：一旦在 `Idle` 阶段收到
/// `'C'` 就切 CRC-16，收到 `NAK` 就切 8 位校验和。XMODEM 的规矩是收方开口，
/// 发方跟随。
///
/// # 队列在开始前准备
///
/// [`Sender::add_file`] 只在 `Idle` 阶段可用（传输中改队列会让块号/长度对不上）。
/// 全部字节必须一次性给出（`Vec<u8>`）：本 crate 零 IO，不能替调用方去读盘；将来
/// 要做流式（边读边发）得在这里加一个「按需取下一块」的 trait，接口位置就在
/// `send_data_block` 这一层（`build_data_frame` 是它唯一的数据来源，换成 trait 即可）。
#[derive(Debug)]
pub struct Sender {
    cfg: Config,
    files: Vec<Queued>,
    phase: Phase,
    buf: Vec<u8>,
    /// 实际使用的校验方式（被对端握手字符覆盖）。
    checksum: Checksum,
    /// 当前文件已 ACK 的字节数。
    cursor: usize,
    /// 当前数据块号（1 起，8 位回绕）。
    block_no: u8,
    /// 上一帧的载荷字节数（ACK 之后推进 `cursor` 要用）。
    last_take: usize,
    /// 上一帧的完整字节（NAK / 超时重发用）。
    last_frame: Vec<u8>,
    /// 上一帧是文件头块还是批结束空头块。
    header_kind: HeaderKind,
    /// 连续失败次数。
    failures: u32,
    /// 连续收到的 CAN 数。
    cans: u8,
}

impl Sender {
    /// 建一个发送方（队列为空，需先 [`Sender::add_file`]）。
    pub fn new(cfg: Config) -> Self {
        Self {
            cfg,
            files: Vec::new(),
            phase: Phase::Idle,
            buf: Vec::new(),
            checksum: cfg.checksum,
            cursor: 0,
            block_no: 0,
            last_take: 0,
            last_frame: Vec::new(),
            header_kind: HeaderKind::File,
            failures: 0,
            cans: 0,
        }
    }

    /// 当前参数。
    pub fn config(&self) -> Config {
        self.cfg
    }

    /// 当前阶段。
    pub fn phase(&self) -> Phase {
        self.phase
    }

    /// 还没发完的文件数（含当前正在发的那个）。
    pub fn queued_files(&self) -> usize {
        self.files.len()
    }

    /// 当前文件已 ACK 的字节数。
    pub fn sent_bytes(&self) -> u64 {
        self.cursor as u64
    }

    /// 追加一个待发文件。
    ///
    /// XMODEM 只会发队列里的第一个文件（协议本身单文件、无头块）；YMODEM 会按
    /// 队列顺序逐个发。
    ///
    /// # 错误
    ///
    /// - 传输已开始（[`Error::AlreadyStarted`]）；
    /// - 文件名为空、含 `NUL`，或「文件名 + 长度 + 两个 `NUL`」放不进 128 字节头块
    ///   （[`Error::BadFileSpec`]）——YMODEM 头块只有 128 字节，超长必须由调用方
    ///   自己截断或改名，而不是由本 crate 静默截断。
    pub fn add_file(&mut self, name: impl Into<String>, data: Vec<u8>) -> Result<(), Error> {
        if self.phase != Phase::Idle {
            return Err(Error::AlreadyStarted);
        }
        let name = name.into();
        if name.is_empty() {
            return Err(Error::BadFileSpec {
                detail: "文件名为空".to_string(),
            });
        }
        if name.as_bytes().contains(&0) {
            return Err(Error::BadFileSpec {
                detail: format!("文件名含 NUL：{name:?}"),
            });
        }
        if self.cfg.protocol.is_batch() {
            let needed = name.len() + data.len().to_string().len() + 2;
            if needed > packet::HEADER_BLOCK_SIZE {
                return Err(Error::BadFileSpec {
                    detail: format!(
                        "头块放不下：文件名 + 长度需要 {needed} 字节，头块只有 {} 字节",
                        packet::HEADER_BLOCK_SIZE
                    ),
                });
            }
        }
        self.files.push(Queued { name, data });
        Ok(())
    }

    /// 喂入任意切分的远端应答字节，返回要发给对端的动作序列。
    pub fn feed(&mut self, bytes: &[u8]) -> Vec<Action> {
        let mut out = Vec::new();
        if self.phase.is_terminal() {
            return out;
        }
        self.buf.extend_from_slice(bytes);
        while !self.buf.is_empty() {
            let byte = self.buf.remove(0);
            match byte {
                packet::CAN => {
                    self.cans += 1;
                    if self.cans >= packet::ABORT_CAN_COUNT {
                        self.phase = Phase::Aborted;
                        self.buf.clear();
                        out.push(Action::Error(Error::Cancelled));
                        break;
                    }
                    continue;
                }
                packet::ACK => {
                    self.cans = 0;
                    self.on_ack(&mut out);
                }
                packet::NAK => {
                    self.cans = 0;
                    self.on_nak(&mut out);
                }
                packet::CRC_REQUEST => {
                    self.cans = 0;
                    self.on_request(&mut out);
                }
                _ => {
                    self.cans = 0;
                }
            }
            if self.phase.is_terminal() {
                self.buf.clear();
                break;
            }
        }
        out
    }

    /// 调用方定时器判定「对端静默」时调用：重发上一帧。
    ///
    /// 在等对端主动开口的阶段（`Idle` / `AwaitingDataRequest` /
    /// `AwaitingNextHeader`）什么都不做——那些阶段本来就是等，催也没用。
    pub fn on_timeout(&mut self) -> Vec<Action> {
        let mut out = Vec::new();
        if self.phase.is_terminal() {
            return out;
        }
        match self.phase {
            Phase::SendingHeader | Phase::SendingData | Phase::AwaitingEndAck => {
                self.failures += 1;
                if self.failures > self.cfg.max_retries {
                    self.fail(Error::TooManyRetries {
                        failures: self.failures,
                        last: Fault::Timeout,
                    }, &mut out);
                } else if !self.last_frame.is_empty() {
                    out.push(Action::Send(self.last_frame.clone()));
                }
            }
            _ => {}
        }
        out
    }

    /// 本端主动中止（对应用户点了「取消传输」）。
    pub fn cancel(&mut self) -> Vec<Action> {
        let mut out = Vec::new();
        if self.phase.is_terminal() {
            return out;
        }
        self.phase = Phase::Aborted;
        self.buf.clear();
        out.push(Action::Send(packet::ABORT_SEQUENCE.to_vec()));
        out
    }

    // ── 应答处理 ───────────────────────────────────────────────────────────

    fn on_ack(&mut self, out: &mut Vec<Action>) {
        self.failures = 0;
        match self.phase {
            Phase::SendingHeader => {
                if self.header_kind == HeaderKind::EndOfBatch {
                    self.finish(out);
                } else {
                    // 头块被 ACK；等接收方的 'C' 才开始发数据。
                    self.phase = Phase::AwaitingDataRequest;
                }
            }
            Phase::SendingData => {
                self.cursor += self.last_take;
                let done = match self.files.first() {
                    Some(file) => self.cursor >= file.data.len(),
                    None => true,
                };
                if done {
                    self.send_eot(out);
                } else {
                    self.block_no = self.block_no.wrapping_add(1);
                    self.send_data_block(out);
                }
            }
            Phase::AwaitingEndAck => {
                if self.cfg.protocol.is_batch() {
                    if !self.files.is_empty() {
                        self.files.remove(0);
                    }
                    self.cursor = 0;
                    self.block_no = 0;
                    self.last_take = 0;
                    // 等接收方用 'C' 请下一个头块（或批结束空头块）。
                    self.phase = Phase::AwaitingNextHeader;
                } else {
                    self.finish(out);
                }
            }
            _ => {}
        }
    }

    fn on_nak(&mut self, out: &mut Vec<Action>) {
        if self.phase == Phase::Idle {
            // 收方用 NAK 开口 = 它只会 8 位校验和。跟随。
            self.checksum = Checksum::Sum8;
            self.begin(out);
            return;
        }
        self.failures += 1;
        if self.failures > self.cfg.max_retries {
            self.fail(Error::TooManyRetries {
                failures: self.failures,
                last: Fault::Checksum,
            }, out);
            return;
        }
        if !self.last_frame.is_empty() {
            out.push(Action::Send(self.last_frame.clone()));
        }
    }

    fn on_request(&mut self, out: &mut Vec<Action>) {
        match self.phase {
            Phase::Idle => {
                // 'C' = 请用 CRC-16。
                self.checksum = Checksum::Crc16;
                self.begin(out);
            }
            Phase::AwaitingDataRequest => {
                self.block_no = 1;
                self.cursor = 0;
                self.send_data_block(out);
            }
            Phase::AwaitingNextHeader => self.send_header(out),
            _ => {}
        }
    }

    // ── 帧组装 ─────────────────────────────────────────────────────────────

    /// 对端已开口，开始按协议发第一个东西。
    fn begin(&mut self, out: &mut Vec<Action>) {
        if self.files.is_empty() {
            self.fail(Error::NoFileToSend, out);
            return;
        }
        self.cursor = 0;
        self.failures = 0;
        if self.cfg.protocol.is_batch() {
            self.send_header(out);
        } else {
            self.block_no = 1;
            self.send_data_block(out);
        }
    }

    fn send_header(&mut self, out: &mut Vec<Action>) {
        let next = self.files.first();
        let (payload, kind) = match next {
            Some(file) => (
                packet::build_header_payload(&file.name, file.data.len() as u64),
                HeaderKind::File,
            ),
            None => (packet::empty_header_payload(), HeaderKind::EndOfBatch),
        };
        let frame = packet::encode_frame(0, &payload, self.checksum);
        self.header_kind = kind;
        self.last_frame = frame.clone();
        self.phase = Phase::SendingHeader;
        self.failures = 0;
        out.push(Action::Send(frame));
    }

    fn send_data_block(&mut self, out: &mut Vec<Action>) {
        let Some(frame_spec) = self.build_data_frame() else {
            self.send_eot(out);
            return;
        };
        let (frame, take) = frame_spec;
        self.last_take = take;
        self.last_frame = frame.clone();
        self.phase = Phase::SendingData;
        out.push(Action::Send(frame));
    }

    /// 组装当前块；文件已发完时返回 `None`。
    fn build_data_frame(&self) -> Option<(Vec<u8>, usize)> {
        let file = self.files.first()?;
        let remaining = file.data.len().saturating_sub(self.cursor);
        if remaining == 0 {
            return None;
        }
        let block_size = self.cfg.protocol.block_size_for(remaining);
        let take = remaining.min(block_size);
        let mut payload = vec![self.cfg.padding; block_size];
        payload[..take].copy_from_slice(&file.data[self.cursor..self.cursor + take]);
        let frame = packet::encode_frame(self.block_no, &payload, self.checksum);
        Some((frame, take))
    }

    fn send_eot(&mut self, out: &mut Vec<Action>) {
        let frame = vec![packet::EOT];
        self.last_frame = frame.clone();
        self.phase = Phase::AwaitingEndAck;
        out.push(Action::Send(frame));
    }

    fn finish(&mut self, out: &mut Vec<Action>) {
        self.phase = Phase::Finished;
        self.buf.clear();
        out.push(Action::Done);
    }

    fn fail(&mut self, err: Error, out: &mut Vec<Action>) {
        self.phase = Phase::Aborted;
        self.buf.clear();
        out.push(Action::Send(packet::ABORT_SEQUENCE.to_vec()));
        out.push(Action::Error(err));
    }
}

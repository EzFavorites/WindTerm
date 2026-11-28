//! 接收方状态机：喂远端字节 → 该回什么、该往文件里写什么。

use crate::action::{Action, Phase};
use crate::config::Config;
use crate::error::{Error, Fault};
use crate::packet::{self, Parsed};

/// 对端给的文件名能不能安全地下发为 [`Action::FileStart`]。
///
/// 与 `term-sftp` 的 `walk::is_plain_name` 同一口径（单段、非空、非 `.` / `..`、
/// 无 `/` 与 `\`），再额外挡掉 NUL 与控制字符。名字由对端决定，调用方随后会拿它
/// 拼落盘路径：一条 `../evil` 就能写到目标目录之外，`\r\x1b[2J` 之类的控制字符
/// 则会污染日志与终端。协议层先拒绝，比指望每个调用方都记得清洗可靠。
pub fn is_plain_name(name: &str) -> bool {
    !name.is_empty()
        && name != "."
        && name != ".."
        && !name.contains('/')
        && !name.contains('\\')
        && !name.contains('\0')
        && !name.chars().any(char::is_control)
}

/// 接收方：把对端发来的字节流解析成「文件内容 + 要回的应答」。
///
/// # 用法
///
/// ```text
/// let mut rx = Receiver::new(Config::for_protocol(Protocol::Ymodem));
/// for a in rx.start() { /* 把 Send 写进会话，把 Data 写进文件 */ }
/// // 会话收到字节时：
/// for a in rx.feed(&chunk) { /* 同上 */ }
/// // 调用方自己的定时器判定「静默太久」时：
/// for a in rx.on_timeout() { /* 通常是一个 NAK，重试超次会是 Error */ }
/// ```
///
/// # 任意切分
///
/// [`Receiver::feed`] 内部维护未消费字节缓冲，帧长由帧头字节唯一决定，所以
/// 「一帧分三次到」与「一次到两帧半」都得到同样的结果；不足一帧的尾巴留在缓冲里
/// 等下次。垃圾字节（既不是 `SOH`/`STX`，也不是 `EOT`/`CAN`）被逐字节丢弃以重新
/// 对齐——这是必要的容错：握手前对端可能先吐了一段登录横幅。
///
/// # 填充暂扣（XMODEM 的固有缺陷，本实现在协议允许范围内尽量少丢）
///
/// XMODEM 线上不带文件长度，块尾填充（默认 `SUB` 0x1A）与真实内容在末尾无法区分。
/// 常见实现直接砍掉每块尾部所有 `SUB`——那会把「文件中间恰好以 0x1A 结尾的块」
/// 砍出空洞。本实现改为**暂扣**：每块尾部连续的 `SUB` 先攒着不下发，
///
/// - 若还有下一块到来，说明那些 `SUB` 是真实内容 → 先补发再处理新块；
/// - 若等到 `EOT`，说明那是最后一块的填充 → 丢弃。
///
/// 代价是「文件真实末尾的 `SUB` 会被当成填充吃掉」——这在 XMODEM 里无解，要精确
/// 就得用 YMODEM（有长度）。YMODEM 路径按声明长度精确裁剪，不暂扣、不丢字节。
///
/// # 定时器归调用方
///
/// 本 crate 不碰时钟。对端静默时由调用方按自己的节奏调 [`Receiver::on_timeout`]，
/// 它负责「重发握手字符 / 重发 NAK / 记失败次数 / 超次放弃」这套策略。
#[derive(Debug)]
pub struct Receiver {
    cfg: Config,
    phase: Phase,
    /// 未消费的远端字节。
    buf: Vec<u8>,
    /// 期望的下一个块号（YMODEM 头块是 0，数据块从 1 起，8 位回绕）。
    expected_block: u8,
    /// 当前文件已接受的有效块数（用于区分「重复帧」与「块号乱跳」）。
    accepted_blocks: u32,
    /// YMODEM：下一帧应该是头块（块 0）。
    expecting_header: bool,
    /// 当前文件声明长度；`None` 表示协议没给（XMODEM）。
    file_size: Option<u64>,
    /// 当前文件已下发的字节数。
    received: u64,
    /// 暂扣的块尾填充字节数（仅 XMODEM 路径）。
    held_pad: usize,
    /// 当前文件是否已经发过 [`Action::FileStart`]。
    file_started: bool,
    /// 连续收到的 CAN 数。
    cans: u8,
    /// 当前期望上的连续失败次数。
    failures: u32,
}

impl Receiver {
    /// 建一个接收方。此时还不会发任何字节——握手由 [`Receiver::start`] 触发。
    pub fn new(cfg: Config) -> Self {
        let batch = cfg.protocol.is_batch();
        Self {
            cfg,
            phase: Phase::Idle,
            buf: Vec::new(),
            expected_block: if batch { 0 } else { 1 },
            accepted_blocks: 0,
            expecting_header: batch,
            file_size: None,
            received: 0,
            held_pad: 0,
            file_started: false,
            cans: 0,
            failures: 0,
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

    /// 当前文件已下发的字节数。
    pub fn received_bytes(&self) -> u64 {
        self.received
    }

    /// 当前期望的块号（0 = YMODEM 头块）。诊断/测试用。
    pub fn expected_block(&self) -> u8 {
        self.expected_block
    }

    /// 开始接收：发出握手字符（CRC 模式是 `'C'`，校验和模式是 `NAK`）。
    ///
    /// 幂等：非 [`Phase::Idle`] 时返回空。调用方若忘了先调它也不致命——
    /// [`Receiver::feed`] 在 `Idle` 状态下会自动补一次。
    pub fn start(&mut self) -> Vec<Action> {
        let mut out = Vec::new();
        if self.phase != Phase::Idle {
            return out;
        }
        let batch = self.cfg.protocol.is_batch();
        self.phase = Phase::Handshake;
        self.expecting_header = batch;
        self.expected_block = if batch { 0 } else { 1 };
        self.accepted_blocks = 0;
        self.failures = 0;
        self.cans = 0;
        out.push(Action::Send(vec![self.cfg.checksum.request_byte()]));
        out
    }

    /// 喂入任意切分的远端字节，返回要执行的动作序列。
    ///
    /// 终态（[`Phase::Finished`] / [`Phase::Aborted`]）下直接返回空——调用方可以在
    /// 收尾后继续无脑转发字节而不必自己判状态。
    pub fn feed(&mut self, bytes: &[u8]) -> Vec<Action> {
        let mut out = Vec::new();
        if self.phase.is_terminal() {
            return out;
        }
        if self.phase == Phase::Idle {
            out.extend(self.start());
        }
        if !bytes.is_empty() {
            self.buf.extend_from_slice(bytes);
        }
        self.drain(&mut out);
        out
    }

    /// 调用方定时器判定「对端静默」时调用。
    ///
    /// - 握手阶段：重发握手字符（`rz`/`rx` 就是每 3 秒再喊一次 `C`）；
    /// - 收数据阶段：发 `NAK` 请求重传当前块；
    /// - 其它阶段：什么都不做（应答由对端驱动，不该我们催）。
    ///
    /// 连续失败超 `max_retries` 时返回 `Error` + 中止序列。
    pub fn on_timeout(&mut self) -> Vec<Action> {
        let mut out = Vec::new();
        if self.phase.is_terminal() {
            return out;
        }
        match self.phase {
            Phase::Handshake | Phase::Receiving => {
                self.failures += 1;
                if self.failures > self.cfg.max_retries {
                    self.fail(Error::TooManyRetries {
                        failures: self.failures,
                        last: Fault::Timeout,
                    }, &mut out);
                } else if self.phase == Phase::Handshake {
                    out.push(Action::Send(vec![self.cfg.checksum.request_byte()]));
                } else {
                    out.push(Action::Send(vec![packet::NAK]));
                }
            }
            _ => {}
        }
        out
    }

    /// 本端主动中止（对应用户点了「取消传输」）。
    ///
    /// 返回中止序列，调用方写进会话即可。终态下返回空。
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

    /// 消费缓冲区直到不足一帧或已进终态。
    fn drain(&mut self, out: &mut Vec<Action>) {
        while let Some(&first) = self.buf.first() {
            match first {
                packet::SOH | packet::STX => match packet::parse_frame(&self.buf, self.cfg.checksum)
                {
                    Parsed::Incomplete => break,
                    Parsed::Invalid { consumed } => {
                        self.buf.drain(..consumed);
                        self.on_corrupt(out);
                    }
                    Parsed::Valid {
                        block,
                        payload,
                        consumed,
                    } => {
                        self.buf.drain(..consumed);
                        self.on_frame(block, &payload, out);
                    }
                },
                packet::EOT => {
                    self.buf.drain(..1);
                    self.on_eot(out);
                }
                packet::CAN => {
                    self.buf.drain(..1);
                    self.on_can(out);
                }
                _ => {
                    // 噪声字节：丢掉一个，寄望下一字节是帧头。
                    self.buf.drain(..1);
                    self.cans = 0;
                }
            }
            if self.phase.is_terminal() {
                self.buf.clear();
                break;
            }
        }
    }

    /// 一帧完全有效但校验不过：请求重传当前块。
    fn on_corrupt(&mut self, out: &mut Vec<Action>) {
        self.cans = 0;
        self.failures += 1;
        if self.failures > self.cfg.max_retries {
            self.fail(Error::TooManyRetries {
                failures: self.failures,
                last: Fault::Checksum,
            }, out);
            return;
        }
        out.push(Action::Send(vec![packet::NAK]));
    }

    fn on_can(&mut self, out: &mut Vec<Action>) {
        self.cans += 1;
        if self.cans >= packet::ABORT_CAN_COUNT {
            self.phase = Phase::Aborted;
            out.push(Action::Error(Error::Cancelled));
        }
    }

    fn on_frame(&mut self, block: u8, payload: &[u8], out: &mut Vec<Action>) {
        self.cans = 0;
        let batch = self.cfg.protocol.is_batch();

        if batch {
            if self.expecting_header {
                if block == 0 {
                    self.on_header(payload, out);
                } else {
                    self.on_block_number_jump(block, out);
                }
                return;
            }
            if block == 0 {
                // 头块的重传：我们的 ACK（或随后的 'C'）大概丢了。重发应答，
                // 但**不再发一次 FileStart**——否则调用方会给同一个文件开两个落盘句柄。
                out.push(Action::Send(vec![packet::ACK]));
                out.push(Action::Send(vec![self.cfg.checksum.request_byte()]));
                return;
            }
        }

        if block == self.expected_block {
            self.on_data_block(payload, out);
            return;
        }
        // 上一块的重复帧（典型成因：我们的 ACK 丢了）。再 ACK 一次，内容丢弃。
        if self.accepted_blocks > 0 && block == self.expected_block.wrapping_sub(1) {
            out.push(Action::Send(vec![packet::ACK]));
            return;
        }
        self.on_block_number_jump(block, out);
    }

    fn on_block_number_jump(&mut self, block: u8, out: &mut Vec<Action>) {
        let expected = self.expected_block;
        self.failures += 1;
        if self.failures > self.cfg.max_retries {
            self.fail(Error::TooManyRetries {
                failures: self.failures,
                last: Fault::BlockNumber {
                    expected,
                    got: block,
                },
            }, out);
            return;
        }
        out.push(Action::Send(vec![packet::NAK]));
    }

    fn on_header(&mut self, payload: &[u8], out: &mut Vec<Action>) {
        match packet::parse_header(payload) {
            Ok(None) => {
                // 空头块 = 批次结束。
                out.push(Action::Send(vec![packet::ACK]));
                self.finish(out);
            }
            Ok(Some(header)) => {
                if !is_plain_name(&header.name) {
                    // 名字不干净就整笔拒绝：不能让 `../evil` 之类的头块走到
                    // `FileStart`，调用方会拿它当落盘文件名。
                    self.fail(
                        Error::BadHeader {
                            detail: format!("文件名不是合法的单段名字：{:?}", header.name),
                        },
                        out,
                    );
                    return;
                }
                self.expecting_header = false;
                self.expected_block = 1;
                self.accepted_blocks = 0;
                self.file_size = header.size;
                self.received = 0;
                self.held_pad = 0;
                self.file_started = true;
                self.failures = 0;
                self.phase = Phase::Receiving;
                out.push(Action::FileStart {
                    name: header.name,
                    size: header.size,
                });
                out.push(Action::Send(vec![packet::ACK]));
                // YMODEM 规范：ACK 之后再发一个握手字符，请求第一个数据块。
                out.push(Action::Send(vec![self.cfg.checksum.request_byte()]));
            }
            Err(detail) => {
                self.fail(Error::BadHeader { detail }, out);
            }
        }
    }

    fn on_data_block(&mut self, payload: &[u8], out: &mut Vec<Action>) {
        if !self.file_started {
            // XMODEM：协议不带文件名，也永远不会有头块。
            self.file_started = true;
            self.phase = Phase::Receiving;
            out.push(Action::FileStart {
                name: String::new(),
                size: None,
            });
        }
        self.emit_payload(payload, out);
        self.expected_block = self.expected_block.wrapping_add(1);
        self.accepted_blocks += 1;
        self.failures = 0;
        out.push(Action::Send(vec![packet::ACK]));
    }

    /// 把一块载荷（去掉填充、按声明长度裁剪）变成 [`Action::Data`]。
    fn emit_payload(&mut self, raw: &[u8], out: &mut Vec<Action>) {
        let pad = self.cfg.padding;
        match self.file_size {
            Some(size) => {
                let remaining = size.saturating_sub(self.received);
                let take = (raw.len() as u64).min(remaining) as usize;
                if take > 0 {
                    self.received += take as u64;
                    out.push(Action::Data(raw[..take].to_vec()));
                }
            }
            None => {
                // 先补发上一块暂扣的填充：能走到这里说明那些字节不是文件末尾填充。
                if self.held_pad > 0 {
                    out.push(Action::Data(vec![pad; self.held_pad]));
                    self.received += self.held_pad as u64;
                    self.held_pad = 0;
                }
                let mut end = raw.len();
                while end > 0 && raw[end - 1] == pad {
                    end -= 1;
                }
                self.held_pad = raw.len() - end;
                if end > 0 {
                    self.received += end as u64;
                    out.push(Action::Data(raw[..end].to_vec()));
                }
            }
        }
    }

    fn on_eot(&mut self, out: &mut Vec<Action>) {
        self.cans = 0;
        if self.expecting_header {
            // 等头块却等来 EOT：对端没发结束空头块就撤了。按批次结束处理，
            // 别让调用方永远等下去。
            out.push(Action::Send(vec![packet::ACK]));
            self.finish(out);
            return;
        }
        if !self.file_started {
            // 空文件：XMODEM 一个数据块都不发，只有 EOT。
            self.file_started = true;
            out.push(Action::FileStart {
                name: String::new(),
                size: None,
            });
        }
        // 最后一块的暂扣填充到此确认是填充，丢掉。
        self.held_pad = 0;
        self.failures = 0;
        out.push(Action::FileDone);
        if self.cfg.protocol.is_batch() {
            self.expecting_header = true;
            self.expected_block = 0;
            self.accepted_blocks = 0;
            self.file_size = None;
            self.received = 0;
            self.file_started = false;
            self.phase = Phase::Handshake;
            // ACK 这个 EOT，紧接着用 'C' 请下一个文件头块（或批结束空头块）。
            out.push(Action::Send(vec![
                packet::ACK,
                self.cfg.checksum.request_byte(),
            ]));
        } else {
            out.push(Action::Send(vec![packet::ACK]));
            self.finish(out);
        }
    }

    fn finish(&mut self, out: &mut Vec<Action>) {
        self.phase = Phase::Finished;
        self.buf.clear();
        out.push(Action::Done);
    }

    fn fail(&mut self, err: Error, out: &mut Vec<Action>) {
        self.phase = Phase::Aborted;
        self.buf.clear();
        // 先通知对端别再发了，再上报失败（顺序有意义：调用方按序 flush）。
        out.push(Action::Send(packet::ABORT_SEQUENCE.to_vec()));
        out.push(Action::Error(err));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 与 `term-sftp` 的 `is_plain_name` 同口径，外加 NUL / 控制字符。
    #[test]
    fn only_a_single_plain_segment_is_a_safe_name() {
        for name in ["a.txt", "中文 名", ".hidden"] {
            assert!(is_plain_name(name), "{name} 应当合法");
        }
        for name in [
            "", ".", "..", "../evil", "a/b", "a\\b", "a\0b", "a\nb", "a\u{1b}b", "a\u{7f}b",
        ] {
            assert!(!is_plain_name(name), "{name:?} 必须被拒绝");
        }
    }
}

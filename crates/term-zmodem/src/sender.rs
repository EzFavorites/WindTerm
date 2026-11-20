//! 发送方状态机：喂远端字节 → 该发什么。

use crate::config::{Config, HeaderKind};
use crate::frame::{self, FrameEnd, FrameType};
use crate::phase::Phase;
use crate::scan::{Scan, Scanner};
use std::collections::VecDeque;
use term_xfer::{Action, Error, Fault};

/// 发送方：把文件编码成 ZMODEM 帧，按对端的应答推进。
///
/// # 用法
///
/// ```text
/// let mut tx = Sender::new(Config::default());
/// tx.add_file("a.txt", b"hello".to_vec())?;
/// // 用户点了「发送」：先发 ZRQINIT。
/// for a in tx.start() { /* Send → 写进会话 */ }
/// // 会话收到字节时：
/// for a in tx.feed(&chunk) { /* 同上 */ }
/// // 调用方自己的定时器判定对端静默：
/// for a in tx.on_timeout() { /* 重发上一帧，超次则 Error */ }
/// ```
///
/// # 时序（严格停等）
///
/// ```text
/// 发送方                接收方
/// ZRQINIT ────────────►
///        ◄──────────── ZRINIT
/// ZFILE(名字/长度/mtime) ►
///        ◄──────────── ZRPOS(0)
/// ZDATA(0,   ZCRCW) ───►
///        ◄──────────── ZACK(1024)
/// ZDATA(1024,ZCRCW) ───►
///        ◄──────────── ZACK(2048)
/// ……
/// ZEOF(总长度) ─────────►
///        ◄──────────── ZRINIT
/// ZFIN ─────────────────►
///        ◄──────────── ZFIN
/// OO
/// ```
///
/// 默认每块都等 `ZACK`（`ZCRCW` 帧尾）。[`Config::streaming`] 打开后改为连发：
/// 一次把剩余数据全打成 `ZCRCG` 子包（块间不等应答），只在最后一块用 `ZCRCW`，
/// 然后接 `ZEOF`。流式的错误恢复是「整段回退重来」（对端发 `ZRPOS(已收字节)`），
/// 所以默认仍是停等。
///
/// # 队列
///
/// 默认**只支持单个文件**：[`Sender::add_file`] 第二次调用会被拒绝。
/// [`Config::batch`] 打开后多出来的文件进队列，按 `ZEOF` → `ZRINIT` 的节奏依次
/// 发出，最后仍是 `ZFIN` 收尾。
#[derive(Debug)]
pub struct Sender {
    cfg: Config,
    phase: Phase,
    /// 未消费的远端字节。
    buf: Vec<u8>,
    /// 帧头 / CAN 中止的判别器。
    scanner: Scanner,
    /// 待发文件名（`None` 表示还没排文件）。
    name: Option<String>,
    /// 待发文件内容。
    data: Vec<u8>,
    /// 修改时间（只进 `ZFILE` 头，本 crate 不碰文件系统）。
    mtime: u32,
    /// 批处理模式下排在当前文件后面的文件：`(名字, 内容, mtime)`。
    queue: VecDeque<(String, Vec<u8>, u32)>,
    /// 已确认的字节数，也就是下一块的起始偏移。
    offset: u64,
    /// 上一帧的完整字节（超时 / `ZNAK` 重发用）。
    last_frame: Vec<u8>,
    /// 同一阶段上的连续失败次数。
    failures: u32,
    /// 对端的 `ZRINIT` 是否宣告了 `CANFC32`（能收 CRC-32 的数据帧）。
    ///
    /// 只有这里为 `true` 且 [`Config::header`] 允许时，数据段才升级到 `ZBIN32`。
    peer_canfc32: bool,
}

impl Sender {
    /// 建一个发送方（还没有待发文件，需先 [`Sender::add_file`]）。
    pub fn new(cfg: Config) -> Self {
        Self {
            cfg: cfg.clamped(),
            phase: Phase::Idle,
            buf: Vec::new(),
            scanner: Scanner::new(),
            name: None,
            data: Vec::new(),
            mtime: 0,
            queue: VecDeque::new(),
            offset: 0,
            last_frame: Vec::new(),
            failures: 0,
            peer_canfc32: false,
        }
    }

    /// 实际生效的参数（`block_size` 已被钳到合法范围）。
    pub fn config(&self) -> Config {
        self.cfg
    }

    /// 当前阶段。
    pub fn phase(&self) -> Phase {
        self.phase
    }

    /// 待发文件名。
    pub fn file_name(&self) -> Option<&str> {
        self.name.as_deref()
    }

    /// 待发文件的字节数。
    pub fn total_bytes(&self) -> u64 {
        self.data.len() as u64
    }

    /// 已确认（对端已 `ZACK`）的字节数，也就是下一块的起始偏移。
    pub fn sent_bytes(&self) -> u64 {
        self.offset
    }

    /// 排一个待发文件（mtime 记 `0`，即「不提供修改时间」）。
    ///
    /// # 错误
    ///
    /// - 已经有文件了且 [`Config::batch`] 为 `false`（单文件模式，[`Error::BadFileSpec`]）；
    /// - 传输已开始（[`Error::AlreadyStarted`]）；
    /// - 文件名为空或含 `NUL`，或文件大于 `u32::MAX` 字节（[`Error::BadFileSpec`]）。
    ///   ZMODEM 的偏移是 32 位，超过就表达不了。
    ///
    /// [`Config::batch`] 为 `true` 时后续文件进队列，按 `ZEOF` → `ZRINIT` 的节奏
    /// 依次发送。
    pub fn add_file(&mut self, name: impl Into<String>, data: Vec<u8>) -> Result<(), Error> {
        self.add_file_with_time(name, data, 0)
    }

    /// 排一个待发文件，并指定修改时间（Unix 秒，0 表示不提供）。
    ///
    /// # 错误
    ///
    /// 同 [`Sender::add_file`]。
    pub fn add_file_with_time(
        &mut self,
        name: impl Into<String>,
        data: Vec<u8>,
        mtime: u32,
    ) -> Result<(), Error> {
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
        if data.len() as u64 > u64::from(u32::MAX) {
            return Err(Error::BadFileSpec {
                detail: format!(
                    "文件 {} 字节，超过 ZMODEM 的 32 位偏移上限 {}",
                    data.len(),
                    u32::MAX
                ),
            });
        }
        if self.name.is_some() {
            if self.cfg.batch {
                // 批处理：后续文件进队列，当前文件 `ZEOF` 之后依次排出去。
                self.queue.push_back((name, data, mtime));
                return Ok(());
            }
            return Err(Error::BadFileSpec {
                detail: "单文件模式只支持一个文件（Config::batch 未开启）".to_string(),
            });
        }
        self.name = Some(name);
        self.data = data;
        self.mtime = mtime;
        Ok(())
    }

    /// 开始发送：发出 `ZRQINIT`。
    ///
    /// 幂等：非 [`Phase::Idle`] 时返回空。调用方若忘了先调它也不致命——
    /// [`Sender::feed`] 在 `Idle` 状态下会自动补一次。
    pub fn start(&mut self) -> Vec<Action> {
        let mut out = Vec::new();
        if self.phase != Phase::Idle {
            return out;
        }
        if self.name.is_none() {
            self.fail(Error::NoFileToSend, &mut out);
            return out;
        }
        self.send_header(FrameType::ZRQINIT, [0; 4], Phase::AwaitingZrinit, &mut out);
        out
    }

    /// 喂入任意切分的远端字节，返回要执行的动作序列。
    pub fn feed(&mut self, bytes: &[u8]) -> Vec<Action> {
        let mut out = Vec::new();
        if self.phase.is_terminal() {
            return out;
        }
        if self.phase == Phase::Idle {
            out.extend(self.start());
            if self.phase.is_terminal() {
                return out;
            }
        }
        if !bytes.is_empty() {
            self.buf.extend_from_slice(bytes);
        }
        self.drain(&mut out);
        self.check_buffer_bound(&mut out);
        out
    }

    /// 调用方定时器判定「对端静默」时调用：重发上一帧。
    ///
    /// 连续超时超过 `max_retries` 时返回 `Error`（附中止序列）。
    pub fn on_timeout(&mut self) -> Vec<Action> {
        let mut out = Vec::new();
        if self.phase.is_terminal() || self.phase == Phase::Idle {
            return out;
        }
        if !self.bump_failure(Fault::Timeout, &mut out) {
            return out;
        }
        if !self.last_frame.is_empty() {
            out.push(Action::Send(self.last_frame.clone()));
        }
        out
    }

    /// 本端主动中止（对应用户点了「取消传输」）。终态下返回空。
    pub fn cancel(&mut self) -> Vec<Action> {
        let mut out = Vec::new();
        if self.phase.is_terminal() {
            return out;
        }
        self.phase = Phase::Aborted;
        self.buf.clear();
        out.push(Action::Send(frame::ABORT_SEQUENCE.to_vec()));
        out
    }

    // ── 输入解析 ───────────────────────────────────────────────────────────

    fn drain(&mut self, out: &mut Vec<Action>) {
        loop {
            if self.phase.is_terminal() {
                self.buf.clear();
                return;
            }
            match self.scanner.scan(&self.buf) {
                Scan::Wait => return,
                Scan::Abort => {
                    self.fail(Error::Cancelled, out);
                    return;
                }
                Scan::Flood => {
                    self.fail(
                        Error::BadHeader {
                            detail: format!(
                                "帧头前连续 ZPAD 超过上限 {} 字节",
                                frame::MAX_ZPAD_RUN
                            ),
                        },
                        out,
                    );
                    return;
                }
                Scan::Garbage(count) => self.drop_bytes(count),
                Scan::Header(header) => {
                    self.drop_bytes(header.consumed);
                    self.on_header(header, out);
                }
            }
        }
    }

    fn drop_bytes(&mut self, count: usize) {
        let count = count.min(self.buf.len());
        self.buf.drain(..count);
    }

    /// 缓冲超过「最坏合法帧」还消化不掉，说明对端的帧没有终止符：判错并清空。
    fn check_buffer_bound(&mut self, out: &mut Vec<Action>) {
        if self.phase.is_terminal() || self.buf.len() <= frame::MAX_BUFFER {
            return;
        }
        let len = self.buf.len();
        self.fail(
            Error::BadHeader {
                detail: format!(
                    "接收缓冲 {len} 字节超过上限 {}（对端帧疑似没有终止符）",
                    frame::MAX_BUFFER
                ),
            },
            out,
        );
    }

    fn on_header(&mut self, header: frame::Header, out: &mut Vec<Action>) {
        match header.frame {
            FrameType::ZRINIT => {
                // 对端的 ZRINIT 里能力位在头数据的**第 4 字节**（`ZF0`，`zmodem.h`
                // 的 `#define ZF0 3`）：只有它说能收 CRC-32（`CANFC32`），数据段才
                // 升级到 ZBIN32，否则留在 ZBIN。读第 1 字节会永远读到 0。
                self.peer_canfc32 = frame::rinit_flags(header.data) & frame::RINIT_CANFC32 != 0;
                self.on_zrinit(out);
            }
            FrameType::ZRPOS | FrameType::ZACK => {
                // 位置字段是**小端**（`rclhdr()` / `ZP0 == 0`）。
                let offset = u64::from(frame::decode_position(header.data));
                self.on_position(offset, out);
            }
            FrameType::ZNAK => {
                if self.bump_failure(Fault::Checksum, out) {
                    self.resend_last(out);
                }
            }
            FrameType::ZSKIP => {
                // 兼容兜底：早期本项目的续传约定把偏移塞进 `ZSKIP` 的 4 字节头数据。
                // 真实对端不会这么发（`ZSKIP` 的原义是「跳过这个文件」），真正的续传
                // 路径是 `ZRPOS`。这里保留容忍分支，免得旧调用方 / 旧对端直接断掉。
                let offset = u64::from(frame::decode_position(header.data));
                self.on_zskip(offset, out);
            }
            FrameType::ZFERR => self.fail(
                Error::BadHeader {
                    detail: "对端报告文件错误（ZFERR）".to_string(),
                },
                out,
            ),
            FrameType::ZABORT | FrameType::ZCAN => self.fail(Error::Cancelled, out),
            FrameType::ZFIN => self.on_peer_fin(out),
            // ZRQINIT 是对端在问「你要发什么」——我们已经在等它的 ZRINIT 了，
            // 重复的 ZRQINIT 直接忽略。其余帧类型与发送方无关。
            _ => {}
        }
    }

    fn on_zrinit(&mut self, out: &mut Vec<Action>) {
        match self.phase {
            Phase::AwaitingZrinit => self.send_file(out),
            Phase::AwaitingZrinitAfterEof => {
                if self.cfg.batch {
                    if let Some((name, data, mtime)) = self.queue.pop_front() {
                        // 批处理：上一个文件已确认，接着发下一个。
                        self.name = Some(name);
                        self.data = data;
                        self.mtime = mtime;
                        self.send_file(out);
                        return;
                    }
                }
                self.send_header(FrameType::ZFIN, [0; 4], Phase::AwaitingZfin, out);
            }
            // 其它阶段收到 ZRINIT：多半是我们应答丢失导致的重复帧。忽略，
            // 让定时器去重发——这样不会因为一个迟到的 ZRINIT 把数据打乱。
            _ => {}
        }
    }

    /// 对端给出了一个偏移（`ZRPOS` 要求重发，`ZACK` 表示已收到这段）。
    fn on_position(&mut self, offset: u64, out: &mut Vec<Action>) {
        // 连发模式下 `ZEOF` 之后仍可能收到 `ZRPOS`：接收方在流中间发现某块 CRC 错，
        // 要求整段回退。这时虽然已经发过 `ZEOF`，也必须从该偏移重发。
        let retry_after_stream = self.cfg.streaming && self.phase == Phase::AwaitingZrinitAfterEof;
        if !retry_after_stream && !matches!(self.phase, Phase::AwaitingZrpos | Phase::AwaitingZack)
        {
            return;
        }
        let total = self.data.len() as u64;
        let offset = offset.min(total);
        if retry_after_stream && offset >= total {
            // 连发之后收到的 `ZACK(total)`：最后一个 `ZCRCW` 的确认，`ZEOF` 已发，
            // 不能再发一次（否则对端会收到重复的 `ZEOF`）。
            return;
        }
        if offset < self.offset {
            // 偏移倒退 = 对端没收到后面的数据，算一次失败。
            if !self.bump_failure(Fault::Checksum, out) {
                return;
            }
        } else {
            self.failures = 0;
        }
        self.offset = offset;
        if offset >= total {
            self.send_eof(out);
        } else if self.cfg.streaming {
            self.send_stream(out);
        } else {
            self.send_block(out);
        }
    }

    /// 兼容早期自洽约定的兜底：把 `ZSKIP(offset > 0)` 当作「跳过前 `offset` 字节」。
    ///
    /// **真实对端不会这么发**——`ZSKIP` 在协议原文里的原义是「跳过这个文件」，续传
    /// 请求是 `ZRPOS(N)`（走 [`Sender::on_position`]，那条路径才是主路径）。这里保留
    /// 容忍分支，只是为了让早期按「`ZSKIP(offset)`」实现的调用方不至于一升级就断。
    ///
    /// 偏移 `0` 保留协议原义——对端明确要求跳过这个文件，按失败上报
    /// （[`Config::max_retries`] 之类的重试没有意义，这个文件对方不要了）。越界偏移
    /// （大于文件长度）同样按失败上报，并且**不 panic**：外部输入一律当作不可信。
    ///
    /// 合法偏移落在 `1..=总长度`：
    ///
    /// - `offset < 总长度`：直接发该偏移处的块（`sent_bytes()` 立即推进到 `offset`）；
    /// - `offset == 总长度`：对端只差一个 `ZEOF`，直接进收尾。
    fn on_zskip(&mut self, offset: u64, out: &mut Vec<Action>) {
        if !matches!(self.phase, Phase::AwaitingZrpos | Phase::AwaitingZack) {
            return;
        }
        let total = self.data.len() as u64;
        if offset == 0 {
            self.fail(
                Error::BadHeader {
                    detail: "对端跳过了该文件（ZSKIP）".to_string(),
                },
                out,
            );
            return;
        }
        if offset > total {
            self.fail(
                Error::BadHeader {
                    detail: format!("ZSKIP 的续传偏移 {offset} 超过文件长度 {total}"),
                },
                out,
            );
            return;
        }
        self.offset = offset;
        self.failures = 0;
        if offset >= total {
            self.send_eof(out);
        } else if self.cfg.streaming {
            self.send_stream(out);
        } else {
            self.send_block(out);
        }
    }

    fn on_peer_fin(&mut self, out: &mut Vec<Action>) {
        if self.phase == Phase::AwaitingZfin {
            // 规范的收尾：收到 ZFIN 之后回 `OO`（over and out，不是帧）。
            out.push(Action::Send(vec![b'O', b'O']));
            self.finish(out);
        } else {
            self.send_header(FrameType::ZFIN, [0; 4], Phase::AwaitingZfin, out);
        }
    }

    // ── 帧组装 ─────────────────────────────────────────────────────────────

    /// 数据段（`ZFILE` / `ZDATA` / `ZEOF` / `ZSINIT`）用哪种帧头。
    ///
    /// 控制帧不走这里——它们永远是 `ZHEX`（[`FrameType::header_kind`]）。
    fn data_header(&self) -> HeaderKind {
        match self.cfg.header {
            // 手动退回：数据帧也用可打印帧头，留给只认 ZHEX 的对端。
            HeaderKind::Hex => HeaderKind::Hex,
            // 明确钉死 16 位 CRC：即使对端宣告 CANFC32 也不升级。
            HeaderKind::Bin => HeaderKind::Bin,
            // 默认：对端宣告 CANFC32 才用 ZBIN32，否则退到 ZBIN。
            HeaderKind::Bin32 if self.peer_canfc32 => HeaderKind::Bin32,
            HeaderKind::Bin32 => HeaderKind::Bin,
        }
    }

    /// 发 `ZFILE`：帧头（`ZF0 = ZCBIN`）+ 文件信息子包（`ZCRCW`）。
    ///
    /// 头数据**不是**位置字段：真实 `sz` 发的是 `00 00 00 01`，第 4 个字节是
    /// `ZF0 = ZCBIN`（二进制传输、禁止换行转换），见 [`frame::FILE_HEADER_DATA`]。
    fn send_file(&mut self, out: &mut Vec<Action>) {
        let Some(name) = self.name.clone() else {
            self.fail(Error::NoFileToSend, out);
            return;
        };
        let info = frame::encode_file_info(&name, self.data.len() as u64, self.mtime);
        let header = self.data_header();
        let mut bytes = frame::encode_header(header, FrameType::ZFILE, frame::FILE_HEADER_DATA);
        bytes.extend_from_slice(&frame::encode_subpacket(
            &info,
            FrameEnd::ZCRCW,
            header.subpacket_crc(),
        ));
        self.last_frame = bytes.clone();
        self.offset = 0;
        self.failures = 0;
        self.phase = Phase::AwaitingZrpos;
        out.push(Action::Send(bytes));
    }

    fn send_block(&mut self, out: &mut Vec<Action>) {
        let start = self.offset as usize;
        let total = self.data.len();
        let end = (start + self.cfg.block_size).min(total);
        let payload = &self.data[start..end];
        let bytes = frame::encode_data_frame(
            self.offset as u32,
            payload,
            FrameEnd::ZCRCW,
            self.data_header(),
        );
        self.last_frame = bytes.clone();
        self.phase = Phase::AwaitingZack;
        out.push(Action::Send(bytes));
    }

    /// 连发（`ZCRCG`）：从 `self.offset` 起把剩余数据全部打成子包，一次交给调用方。
    ///
    /// 线上形态是**一个 `ZDATA` 帧头 + 一串子包**：除最后一块用 `ZCRCW`（要求
    /// `ZACK`）之外都是 `ZCRCG`（「后面还有，不用回」）。块间没有帧头、没有应答，
    /// 接收方只能靠每块的 CRC 与长度判界。收尾照常接 `ZEOF`。
    fn send_stream(&mut self, out: &mut Vec<Action>) {
        let total = self.data.len();
        if self.offset as usize >= total {
            self.send_eof(out);
            return;
        }
        let header = self.data_header();
        let mut bytes = frame::encode_header(
            header,
            FrameType::ZDATA,
            frame::encode_position(self.offset as u32),
        );
        let crc = header.subpacket_crc();
        while (self.offset as usize) < total {
            let start = self.offset as usize;
            let end = (start + self.cfg.block_size).min(total);
            let tail = if end == total {
                FrameEnd::ZCRCW
            } else {
                FrameEnd::ZCRCG
            };
            bytes.extend_from_slice(&frame::encode_subpacket(&self.data[start..end], tail, crc));
            self.offset = end as u64;
        }
        self.last_frame = bytes.clone();
        out.push(Action::Send(bytes));
        self.failures = 0;
        self.send_eof(out);
    }

    fn send_eof(&mut self, out: &mut Vec<Action>) {
        let total = self.data.len() as u32;
        // `ZEOF` 的头数据是**文件总长**，与 `ZRPOS`/`ZACK`/`ZDATA` 一样按小端写。
        self.send_header(
            FrameType::ZEOF,
            frame::encode_position(total),
            Phase::AwaitingZrinitAfterEof,
            out,
        );
    }

    fn send_header(
        &mut self,
        frame_type: FrameType,
        data: [u8; 4],
        next: Phase,
        out: &mut Vec<Action>,
    ) {
        // 按帧类别选帧头：控制帧永远 ZHEX，数据帧用协商结果。
        let kind = frame_type.header_kind(self.data_header());
        let bytes = frame::encode_header(kind, frame_type, data);
        self.last_frame = bytes.clone();
        self.phase = next;
        self.failures = 0;
        out.push(Action::Send(bytes));
    }

    fn resend_last(&mut self, out: &mut Vec<Action>) {
        if !self.last_frame.is_empty() {
            out.push(Action::Send(self.last_frame.clone()));
        }
    }

    // ── 收尾 ───────────────────────────────────────────────────────────────

    /// 记一次失败；超过上限时直接判死并返回 `false`。
    fn bump_failure(&mut self, last: Fault, out: &mut Vec<Action>) -> bool {
        self.failures += 1;
        if self.failures > self.cfg.max_retries {
            self.fail(
                Error::TooManyRetries {
                    failures: self.failures,
                    last,
                },
                out,
            );
            false
        } else {
            true
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
        // 先通知对端别再发了，再上报失败（调用方按序 flush，顺序有意义）。
        out.push(Action::Send(frame::ABORT_SEQUENCE.to_vec()));
        out.push(Action::Error(err));
    }
}

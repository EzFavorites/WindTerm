//! 接收方状态机：喂远端字节 → 该回什么、该往文件里写什么。

use crate::config::Config;
use crate::frame::{self, CrcKind, FrameEnd, FrameType, ParsedSubpacket};
use crate::phase::Phase;
use crate::scan::{Scan, Scanner};
use term_xfer::{Action, Error, Fault};

/// 帧头解析完之后，接下来等的是哪种数据子包。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Pending {
    /// `ZDATA` 头之后的载荷，携带块起始偏移。
    Data {
        /// 这一块的起始偏移。
        offset: u32,
        /// 子包校验宽度（取自帧头类型）。
        crc: CrcKind,
    },
    /// `ZFILE` 头之后的「文件名 + 长度 + mtime」。
    File {
        /// 子包校验宽度（取自帧头类型）。
        crc: CrcKind,
    },
    /// `ZSINIT` 等「本轮只做兼容」的帧：子包读出来直接丢。
    Discard {
        /// 子包校验宽度（取自帧头类型）。
        crc: CrcKind,
    },
}

impl Pending {
    fn crc(self) -> CrcKind {
        match self {
            Pending::Data { crc, .. } | Pending::File { crc } | Pending::Discard { crc } => crc,
        }
    }
}

/// 接收方：把对端发来的字节流解析成「文件内容 + 要回的应答」。
///
/// # 用法
///
/// ```text
/// let mut rx = Receiver::new(Config::default());
/// for a in rx.start() { /* 把 Send 写进会话 */ }
/// // 会话收到字节时：
/// for a in rx.feed(&chunk) { /* Send 写进会话，Data 写进文件 */ }
/// // 调用方自己的定时器判定「静默太久」时：
/// for a in rx.on_timeout() { /* 通常是一个 ZRPOS，重试超次会是 Error */ }
/// ```
///
/// # 时序
///
/// ```text
///        ◄──────────── ZRQINIT / 直接 ZFILE
/// ZRINIT ────────────►
///        ◄──────────── ZFILE(名字/长度/mtime)
/// ZRPOS(0) ──────────►
///        ◄──────────── ZDATA(0, 载荷)
/// ZACK(1024) ────────►
/// ……
///        ◄──────────── ZEOF(总长度)
/// ZRINIT ────────────►
///        ◄──────────── ZFIN
/// ZFIN ──────────────►
/// ```
///
/// # 三种帧头都能收
///
/// [`frame::parse_header`] 同时认 `ZHEX` / `ZBIN` / `ZBIN32`，与
/// [`Config::header`] 无关（那个字段只决定本端**发**哪种）。真实对端会在
/// `ZRINIT`/`ZRQINIT` 上用十六进制帧头、在数据帧上切到 `ZBIN32`，所以「三种混着
/// 来」是常态，不是异常。
///
/// 数据子包的校验宽度跟着**收到的帧头**走（`ZHEX`/`ZBIN` → CRC-16，
/// `ZBIN32` → CRC-32），不跟本端配置走。
///
/// # 任意切分
///
/// [`Receiver::feed`] 内部维护未消费字节缓冲：帧头长度由帧头自身决定，数据子包
/// 长度由 `ZDLE` + 帧尾唯一决定，所以「一帧分三次到」与「一次到两帧半」都得到
/// 同样的结果。垃圾字节逐字节丢弃以重新对齐——握手前对端可能先吐一段登录横幅。
///
/// # 连发（`ZCRCG`）
///
/// 一个 `ZDATA` 帧里可以跟多个数据子包：除最后一块用 `ZCRCW`（要求应答），其余
/// 都是 `ZCRCG`（「后面还有，不用回」）。接收方按块的偏移与长度逐块推进，块间
/// 不回应答。
///
/// 校验失败时**不能「跳过继续」**：连发流里丢一块，后面的子包全部错位。所以此时
/// 丢弃缓冲里的残尾，回 `ZRPOS(已收字节)`，让对端从最后一次成功的位置整段重发。
/// 这与停等模式（每块一个帧头，跳过一个坏块后能自行重新对齐）不同。
///
/// # 外部输入的硬上限
///
/// 帧头前连续 `ZPAD` 超过 [`frame::MAX_ZPAD_RUN`]，或未消费缓冲超过
/// [`frame::MAX_BUFFER`]，一律报 [`Error::BadHeader`] 并中止——避免一个永远不
/// 结束的帧把内存吃干。
///
/// # 文件名安全
///
/// 本 crate **不碰文件系统**，所以 [`Action::FileStart::name`] 里是对端给的原始
/// 名字（可能含 `../`、绝对路径、控制字符）。**调用方必须自己清洗**再拼路径，
/// 这是本 crate 唯一一处「必须由上层兜底」的安全边界。
///
/// # 单文件 / 批处理
///
/// 默认单文件：`ZEOF` 之后收到第二个 `ZFILE` 会回 `ZSKIP(0)`，让对端直接走到
/// `ZFIN`，避免它一直等。
///
/// [`Config::batch`] 为 `true` 时改为接受下一个 `ZFILE`：每个文件结束都发一次
/// [`Action::FileDone`]，它就是两个 `FileStart` 之间的边界信号；`ZFIN` 仍然是整个
/// 会话的收尾。默认值 `false`，线上字节与单文件行为逐字节不变。
///
/// # 断点续传（`ZRPOS` 协商）
///
/// 调用方若发现本地已有该文件的前 N 字节，可在 [`Receiver::start`] 之前调用
/// [`Receiver::resume_from`]。之后收到 `ZFILE` 时，接收方不再回 `ZRPOS(0)`，而是回
/// **`ZRPOS(N)`**（头数据是按小端写的位置字段）——这正是 `lrzsz` 的续传请求：
/// 「前 N 字节已有，从这个偏移接着发」。[`Receiver::received_bytes`] 也直接从 N
/// 起算，`on_block` 的「偏移必须等于已收字节数」这条判据天然成立。
///
/// `ZSKIP` 保持协议原义「跳过这个文件」：本实现只在「`ZEOF` 之后又收到一个
/// `ZFILE`」时发它，且偏移恒为 0（见上文「单文件」一节）。
///
/// 历史：早期版本在续传时回 `ZSKIP(N)`（借用它的 4 字节头数据承载偏移）。那是一个
/// 自洽但不符合协议的设计——真实 `sz` 会把 `ZSKIP` 理解成「放弃这个文件」，于是
/// 续传从未真正生效。纠正为 `ZRPOS(N)`；发送方对 `ZSKIP(offset > 0)` 的
/// 容忍分支作为兼容兜底保留（真实对端不会这么发）。
#[derive(Debug)]
pub struct Receiver {
    cfg: Config,
    phase: Phase,
    /// 未消费的远端字节。
    buf: Vec<u8>,
    /// 帧头 / CAN 中止的判别器。
    scanner: Scanner,
    /// 已解析到一个带子包的帧头，正在等（或正在攒）它的子包。
    pending: Option<Pending>,
    /// 当前文件是否已经发过 [`Action::FileStart`]。
    file_open: bool,
    /// 当前文件的声明长度；`None` 表示对端没给。
    size: Option<u64>,
    /// 当前文件的修改时间；`None` 表示对端没给。
    mtime: Option<u32>,
    /// 当前文件的模式（Unix 权限位，低 12 位）；`None` 表示对端没给。
    mode: Option<u32>,
    /// 调用方声明的「本地已有前 N 字节」，收到 `ZFILE` 时用于续传。
    resume: u64,
    /// 当前文件已下发的字节数。
    received: u64,
    /// 是否正在 `ZCRCG` 连发的中间：此时丢一块就整段错位，恢复只能整段回退。
    in_stream: bool,
    /// 同一阶段上的连续失败次数。
    failures: u32,
}

impl Receiver {
    /// 建一个接收方。此时还不会发任何字节——握手由 [`Receiver::start`] 触发。
    pub fn new(cfg: Config) -> Self {
        Self {
            cfg: cfg.clamped(),
            phase: Phase::Idle,
            buf: Vec::new(),
            scanner: Scanner::new(),
            pending: None,
            file_open: false,
            size: None,
            mtime: None,
            mode: None,
            resume: 0,
            received: 0,
            in_stream: false,
            failures: 0,
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

    /// 当前文件已下发的字节数。
    pub fn received_bytes(&self) -> u64 {
        self.received
    }

    /// 当前文件的声明长度（`ZFILE` 里没给时为 `None`）。
    pub fn file_size(&self) -> Option<u64> {
        self.size
    }

    /// 当前文件的修改时间（`ZFILE` 里没给时为 `None`）。
    ///
    /// 调用方应在处理 [`Action::FileStart`] 时读取它——`Action` 里没有 mtime 字段
    /// （复用 `term-xfer` 的事件形状），这个访问器是唯一的出口。
    pub fn file_mtime(&self) -> Option<u32> {
        self.mtime
    }

    /// 当前文件的模式（Unix 权限位，已裁到低 12 位；`ZFILE` 里没给时为 `None`）。
    ///
    /// 与 [`Receiver::file_mtime`] 一样：`Action` 是 X/YMODEM 共用的事件形状，
    /// 放不下 ZMODEM 的附加字段，所以调用方在处理 [`Action::FileStart`] 时从
    /// 这里读。**`None` 表示对端没给**，不能当成权限 `000`。
    pub fn file_mode(&self) -> Option<u32> {
        self.mode
    }

    /// 本地已有该文件的前 `offset` 字节，请对端从该偏移续传（`ZRPOS` 协商）。
    ///
    /// 只对本次会话的**第一个**文件生效：批处理时后续文件从 0 开始（调用方只可能
    /// 知道那一个半截文件）。
    ///
    /// 必须在 [`Receiver::start`] 之前调用——续传偏移是「这次会话的起点」，一旦
    /// 握手开始就固定了。调用方按本地半截文件的长度传值即可；本 crate 不读文件系统，
    /// 也不做 seek / 截断，落盘由调用方按「追加写」处理
    /// （[`Action::Data`] 只包含**本次新收到**的字节）。
    ///
    /// 收到 `ZFILE` 时：
    ///
    /// - `offset` 大于声明长度时**钳到声明长度**（不 panic，最多是多要一个空尾块）；
    /// - `offset > 0` 回 `ZRPOS(offset)`（协议里「从 N 接着发」的标准请求），
    ///   否则回 `ZRPOS(0)`（从头收）。
    ///
    /// # 错误
    ///
    /// - 传输已开始（[`Error::AlreadyStarted`]）；
    /// - `offset` 超过 ZMODEM 的 32 位偏移上限（[`Error::BadFileSpec`]）——`ZDATA` /
    ///   `ZEOF` / `ZRPOS` 的偏移字段都只有 4 字节，超过就表达不了。
    pub fn resume_from(&mut self, offset: u64) -> Result<(), Error> {
        if self.phase != Phase::Idle {
            return Err(Error::AlreadyStarted);
        }
        if offset > u64::from(u32::MAX) {
            return Err(Error::BadFileSpec {
                detail: format!(
                    "续传偏移 {offset} 超过 ZMODEM 的 32 位上限 {}",
                    u32::MAX
                ),
            });
        }
        self.resume = offset;
        Ok(())
    }

    /// 调用方声明的续传起点（默认 `0`，即从头收）。
    pub fn resume_offset(&self) -> u64 {
        self.resume
    }

    /// 开始接收：发出 `ZRINIT`（宣告 `CANFC32` 与 8192 字节缓冲）。
    ///
    /// 幂等：非 [`Phase::Idle`] 时返回空。调用方若忘了先调它也不致命——
    /// [`Receiver::feed`] 在 `Idle` 状态下会自动补一次。
    pub fn start(&mut self) -> Vec<Action> {
        let mut out = Vec::new();
        if self.phase != Phase::Idle {
            return out;
        }
        self.phase = Phase::AwaitingZfile;
        self.failures = 0;
        self.emit_zrinit(&mut out);
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
        }
        if !bytes.is_empty() {
            self.buf.extend_from_slice(bytes);
        }
        self.drain(&mut out);
        self.check_buffer_bound(&mut out);
        out
    }

    /// 调用方定时器判定「对端静默」时调用。
    ///
    /// - 等 `ZFILE`：重发 `ZRINIT`；
    /// - 收数据：丢掉缓冲里的半截子包（它永远也拼不完整了），发 `ZRPOS` 请对端
    ///   从当前期望偏移重发；
    /// - 等 `ZFIN`：重发 `ZRINIT`。
    ///
    /// 连续失败超过 `max_retries` 时返回 `Error`（附中止序列）。
    pub fn on_timeout(&mut self) -> Vec<Action> {
        let mut out = Vec::new();
        if self.phase.is_terminal() || self.phase == Phase::Idle {
            return out;
        }
        match self.phase {
            Phase::Receiving => {
                self.buf.clear();
                self.pending = None;
                self.in_stream = false;
                self.request_retry(&mut out);
            }
            Phase::AwaitingZfile | Phase::AwaitingZfin => {
                if self.bump_failure(Fault::Timeout, &mut out) {
                    self.emit_zrinit(&mut out);
                }
            }
            _ => {}
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
        self.pending = None;
        self.in_stream = false;
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
            if let Some(pending) = self.pending.take() {
                // 子包入口先吸收流控噪声：`ZHEX` 帧头的 `XON` 尾巴（`zshhdr()`
                // 对非 `ZACK`/`ZFIN` 帧恒发）逐字节喂入时会晚一批到达，正好落在
                // 子包的第一个字节位置。`lrzsz` 的 `zdlread2()` 也是这么吞的。
                self.drop_flow_control();
                match frame::parse_subpacket(&self.buf, pending.crc()) {
                    ParsedSubpacket::Incomplete => {
                        self.pending = Some(pending);
                        return;
                    }
                    ParsedSubpacket::Invalid { consumed } => {
                        self.drop_bytes(consumed);
                        self.on_bad_subpacket(pending, out);
                    }
                    ParsedSubpacket::Valid(packet) => {
                        self.drop_bytes(packet.consumed);
                        match pending {
                            Pending::Data { offset, crc } => {
                                self.on_block(offset, &packet.data, packet.end, crc, out);
                            }
                            Pending::File { .. } => self.on_file_info(&packet.data, out),
                            Pending::Discard { .. } => {}
                        }
                    }
                }
                continue;
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

    /// 丢掉缓冲区头部连续的流控字节（`XON` / `XOFF` 及其高位变体）。
    ///
    /// 只影响缓冲区**头部**：子包一旦开始，第一个字节就是载荷，不会被误吞。
    fn drop_flow_control(&mut self) {
        while self
            .buf
            .first()
            .copied()
            .is_some_and(frame::is_flow_control)
        {
            self.drop_bytes(1);
        }
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
        let crc = header.kind.subpacket_crc();
        match header.frame {
            FrameType::ZRQINIT => {
                if self.phase == Phase::AwaitingZfile {
                    // 发送方的开场白：回一次 ZRINIT。对端可能连发几次 ZRQINIT，
                    // 每次回一次是 `lrzsz` 的行为，也让对端拿到任意一个就能开工。
                    self.emit_zrinit(out);
                }
            }
            FrameType::ZFILE => {
                if self.phase == Phase::AwaitingZfin {
                    if self.cfg.batch {
                        // 批处理：上一个文件已经收完，接着收下一个（`FileDone` 已在
                        // `ZEOF` 时发过，它就是文件之间的边界信号）。
                        self.pending = Some(Pending::File { crc });
                    } else {
                        // 单文件：后面的文件直接跳过，让对端走向 ZFIN。
                        self.emit(FrameType::ZSKIP, [0; 4], out);
                    }
                } else if self.file_open {
                    // 我们上一轮的 ZRPOS 大概丢了，对端在重发 ZFILE。
                    // 只补应答，不再发一次 FileStart（否则调用方会开两个句柄）。
                    self.emit_zrpos(out);
                } else {
                    self.pending = Some(Pending::File { crc });
                }
            }
            FrameType::ZDATA => {
                if self.file_open {
                    // 位置字段是**小端**（`lrzsz` 的 `stohdr()` / `zmodem.h` 里
                    // `ZP0 == 0`）。按大端读会把偏移 1024 读成 262144。
                    let offset = frame::decode_position(header.data);
                    self.pending = Some(Pending::Data { offset, crc });
                } else {
                    // 没见过 ZFILE 就来数据：按协议回 ZNAK 让对端重来。
                    self.emit(FrameType::ZNAK, [0; 4], out);
                }
            }
            FrameType::ZEOF => self.on_eof(header.data, out),
            FrameType::ZFIN => {
                self.emit(FrameType::ZFIN, [0; 4], out);
                self.finish(out);
            }
            FrameType::ZSINIT => self.pending = Some(Pending::Discard { crc }),
            FrameType::ZABORT | FrameType::ZCAN => self.fail(Error::Cancelled, out),
            FrameType::ZFERR => self.fail(
                Error::BadHeader {
                    detail: "对端报告文件错误（ZFERR）".to_string(),
                },
                out,
            ),
            FrameType::ZNAK => {
                // 对端没听清我们上一轮的应答。
                if self.file_open {
                    self.emit_zrpos(out);
                } else if self.phase == Phase::AwaitingZfile {
                    self.emit_zrinit(out);
                }
            }
            // ZSKIP / ZCRC / ZCHALLENGE 等本轮不处理：忽略，等对端继续。
            _ => {}
        }
    }

    fn on_bad_subpacket(&mut self, pending: Pending, out: &mut Vec<Action>) {
        match pending {
            Pending::Data { .. } => self.request_retry(out),
            Pending::File { .. } => {
                if self.bump_failure(Fault::Checksum, out) {
                    self.emit(FrameType::ZNAK, [0; 4], out);
                }
            }
            Pending::Discard { .. } => {}
        }
    }

    fn on_file_info(&mut self, data: &[u8], out: &mut Vec<Action>) {
        let info = match frame::parse_file_info(data) {
            Ok(info) => info,
            Err(detail) => {
                self.fail(Error::BadHeader { detail }, out);
                return;
            }
        };
        if info.size.is_some_and(|size| size > u64::from(u32::MAX)) {
            // ZMODEM 的偏移是 32 位：比这更大的文件无法用 `ZDATA` / `ZEOF` 的
            // 4 字节偏移表达，后续一定会错位，不如现在就明确拒绝。
            self.fail(
                Error::BadHeader {
                    detail: format!(
                        "声明长度 {:?} 超过 ZMODEM 的 32 位偏移上限 {}",
                        info.size,
                        u32::MAX
                    ),
                },
                out,
            );
            return;
        }
        // 续传起点：调用方说本地已有前 N 字节。超过声明长度时钳到声明长度——
        // 这样最后只会多要一个空的 `ZEOF`，不会让对端永远发不完。
        let start = match info.size {
            Some(size) => self.resume.min(size),
            None => self.resume,
        };
        self.file_open = true;
        self.size = info.size;
        self.mtime = info.mtime;
        self.mode = info.mode;
        self.received = start;
        self.in_stream = false;
        // 续传点只对本次会话的第一个文件有意义（调用方只知道那一个半截文件）。
        // 批处理时后续文件必须从 0 开始，不能沿用同一个偏移。
        self.resume = 0;
        self.failures = 0;
        self.phase = Phase::Receiving;
        out.push(Action::FileStart {
            name: info.name,
            size: info.size,
        });
        // `ZRPOS` 既是「从头开始」（偏移 0）也是「从断点续传」（偏移 N）的请求：
        // 头数据就是 [`Receiver::received_bytes`] 的当前值，小端。
        self.emit_zrpos(out);
    }

    /// 一个 `ZDATA` 子包到手（已转义还原、CRC 已校验）。
    ///
    /// `end == ZCRCG` 表示「同一帧里还有下一个子包」：这里把 `pending` 重新装上，
    /// 下一轮循环直接按 `offset + len` 继续解析，不需要新帧头——这就是连发（流式）
    /// 的解析方式。`ZCRCW` / `ZCRCE` 则结束这一段。
    fn on_block(
        &mut self,
        offset: u32,
        data: &[u8],
        end: FrameEnd,
        crc: CrcKind,
        out: &mut Vec<Action>,
    ) {
        let offset = u64::from(offset);
        let end_offset = offset + data.len() as u64;
        let continues = end == FrameEnd::ZCRCG;

        if offset > self.received {
            // 跳到了前面：从期望位置重发。
            self.request_retry(out);
            return;
        }
        if end_offset <= self.received {
            // 整块都是重复的（我们的 ZACK 丢了）。内容丢弃；连发时继续等下一块。
            self.failures = 0;
            if continues {
                self.rearm_stream(end_offset, crc);
            } else {
                self.in_stream = false;
                self.emit_zack(out);
            }
            return;
        }
        if offset < self.received {
            // 部分重叠：既不是重传也不是新块，只能请对端从期望位置来。
            self.request_retry(out);
            return;
        }

        // offset == received：新块。按声明长度裁剪（最后一块可能带多余填充/字节）。
        let take = match self.size {
            Some(size) => (data.len() as u64).min(size.saturating_sub(self.received)) as usize,
            None => data.len(),
        };
        if take > 0 {
            self.received += take as u64;
            out.push(Action::Data(data[..take].to_vec()));
        }
        self.failures = 0;
        if continues {
            self.rearm_stream(end_offset, crc);
        } else {
            self.in_stream = false;
            if end.wants_ack() {
                self.emit_zack(out);
            }
        }
    }

    /// 装好「同一 `ZDATA` 帧里的下一个子包」：偏移推进到本块末尾，校验宽度不变。
    ///
    /// 同时把 [`Receiver::in_stream`] 置位——之后若这个子包 CRC 错，就知道不能在
    /// 流中间「跳过继续」，必须丢掉残尾并回 `ZRPOS` 整段回退。
    fn rearm_stream(&mut self, next: u64, crc: CrcKind) {
        self.in_stream = true;
        self.pending = Some(Pending::Data {
            offset: next.min(u64::from(u32::MAX)) as u32,
            crc,
        });
    }

    fn on_eof(&mut self, data: [u8; 4], out: &mut Vec<Action>) {
        if !self.file_open {
            if self.phase == Phase::AwaitingZfin {
                // 文件已经收完，但对端的 `ZEOF` 重发了——我们的 `ZRINIT` 丢了。
                // 再喊一次；用 `ZNAK` 会让对端一直重发 `ZEOF` 直到超次。
                self.emit_zrinit(out);
            } else {
                self.emit(FrameType::ZNAK, [0; 4], out);
            }
            return;
        }
        let offset = u64::from(frame::decode_position(data));
        let mismatch = match self.size {
            Some(size) => offset != size || self.received != size,
            None => offset != self.received,
        };
        if mismatch {
            self.request_retry(out);
            return;
        }
        self.file_open = false;
        self.failures = 0;
        self.in_stream = false;
        out.push(Action::FileDone);
        // ACK 这个 ZEOF，并请下一个文件（单文件模式下接下来就是 ZFIN）。
        self.emit_zrinit(out);
        self.phase = Phase::AwaitingZfin;
    }

    // ── 应答组装 ───────────────────────────────────────────────────────────

    /// 发一个帧头。接收方只会发握手/控制帧（`ZRINIT` / `ZRPOS` / `ZACK` /
    /// `ZNAK` / `ZSKIP` / `ZFIN`），按协议一律用可打印的 `ZHEX`。
    ///
    /// 这一点与 [`Config::header`] 无关：那个配置只影响**发送方**的数据帧。
    fn emit(&self, frame_type: FrameType, data: [u8; 4], out: &mut Vec<Action>) {
        debug_assert!(!frame_type.is_data_frame(), "接收方不发数据帧");
        let kind = frame_type.header_kind(self.cfg.header);
        out.push(Action::Send(frame::encode_header(kind, frame_type, data)));
    }

    /// 发 `ZRINIT`：宣告能力位（`CANFC32|ESCCTL`）与 8192 字节接收缓冲。
    ///
    /// 能力位落在头数据的**第 4 字节**（`ZF0`）、缓冲大小落在**前两字节（小端）**，
    /// 见 [`frame::rinit_header_data`]。放错位置不会立刻报错——对端只是读不到能力位、
    /// 并把缓冲大小算成一个错值——所以这一条特别值得钉住。
    fn emit_zrinit(&self, out: &mut Vec<Action>) {
        let data = frame::rinit_header_data(frame::RINIT_FLAGS, frame::RINIT_BUFFER);
        self.emit(FrameType::ZRINIT, data, out);
    }

    fn emit_zrpos(&self, out: &mut Vec<Action>) {
        self.emit(FrameType::ZRPOS, self.emit_position(), out);
    }

    fn emit_zack(&self, out: &mut Vec<Action>) {
        self.emit(FrameType::ZACK, self.emit_position(), out);
    }

    /// `ZRPOS` / `ZACK` 的位置字段：当前已收到的偏移，**小端**。
    fn emit_position(&self) -> [u8; 4] {
        frame::encode_position(self.received as u32)
    }

    // ── 收尾 ───────────────────────────────────────────────────────────────

    /// 请求对端从当前期望偏移重发。
    ///
    /// 连发（`ZCRCG`）中途失败时格外重要：丢掉缓冲里的**残尾**再回 `ZRPOS`——
    /// 连发流里丢了一块，后面的子包全会错位，继续「跳过继续」只会越错越远。
    fn request_retry(&mut self, out: &mut Vec<Action>) {
        if self.in_stream {
            self.buf.clear();
            self.pending = None;
            self.in_stream = false;
        }
        if !self.bump_failure(Fault::Checksum, out) {
            return;
        }
        self.emit_zrpos(out);
    }

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
        self.pending = None;
        out.push(Action::Done);
    }

    fn fail(&mut self, err: Error, out: &mut Vec<Action>) {
        self.phase = Phase::Aborted;
        self.buf.clear();
        self.pending = None;
        out.push(Action::Send(frame::ABORT_SEQUENCE.to_vec()));
        out.push(Action::Error(err));
    }
}

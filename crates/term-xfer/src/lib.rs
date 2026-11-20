//! `term-xfer` — 文件传输协议层：XMODEM / XMODEM-1K / YMODEM 收发状态机。
//!
//! 目标是把 `Terminal.SendXModem` / `ReceiveXModem` / `SendYModem` / `ReceiveYModem` /
//! `CancelModem` 这几条菜单命令背后的**协议内核**做实：给定「远端字节流」与「本地
//! 文件内容」，产出「该发给远端的字节」与「文件/进度事件」。
//!
//! # 三条纪律
//!
//! 1. **纯逻辑，零 IO。** 不读文件、不开 socket、不 sleep、不起线程、不看时钟。
//!    输入是 `&[u8]`，输出是 [`Action`]。落盘与发送通道由调用方持有。
//! 2. **零依赖。** 连 `term-core` 都不需要——协议层只认字节，不需要会话抽象。
//!    所以本 crate 的 `Cargo.toml` 里 `[dependencies]` 段是空的，也没有任何异步运行时
//!    或 UI 框架进来。这条纪律是**可 grep 验证**的：`src/` 里不出现异步运行时、
//!    SSH 栈、UI 框架、文件系统、线程、网络、单调时钟的任何符号（口径同
//!    `scripts/verify.sh` 的异步笼子那一步）。
//! 3. **定时器归调用方。** 「对端静默 3 秒就重发」是**策略**，本 crate 只提供
//!    [`Receiver::on_timeout`] / [`Sender::on_timeout`] 这个钩子，什么时候调由驱动方
//!    决定（生产里是会话的定时器，测试里是测试自己数步数）。
//!
//! # 数据流
//!
//! ```text
//!            远端字节                 Action::Send(应答/帧)        会话写出
//!   会话 ──────────────► Receiver ──────┬───────────────► 驱动方 ─────────► 远端
//!                        Sender   ◄──────┘
//!                                    └─► Action::Data / FileStart / FileDone ─► 文件
//! ```
//!
//! # 任意切分（这是本 crate 最容易写错的地方，所以专门钉了测试）
//!
//! 会话层拿到的字节块边界是任意的：一条协议帧可能分三次到达，一次到达可能跨两帧
//! 半。两个状态机内部都维护未消费缓冲，帧长由帧头字节唯一决定（`SOH`→133 字节、
//! `STX`→1029 字节，校验和模式各少 1 字节），因此**喂进去的切分方式不影响结果**。
//! `tests/chunking.rs` 用「整块喂 / 逐字节喂 / 伪随机切分喂」三种方式跑同一段字节
//! 流，断言三者等价。
//!
//! # 协议范围（本轮）
//!
//! - **XMODEM**：128 字节块；CRC-16 与 8 位校验和两种帧尾；NAK 重传；重试超次放弃；
//!   `SUB`(0x1A) 填充；`CAN CAN` 中止。
//! - **XMODEM-1K**：主体 1024 字节块，尾块自动降到 128 字节；接收方两种帧长都收。
//! - **YMODEM**：块 0 头块（文件名 + 长度）、连续多文件、批结束空头块、EOT 之后
//!   再发 `'C'` 请求下一个头块。
//!
//! ## 为什么本轮不做 ZMODEM
//!
//! 不是一个量级的东西，硬塞进来只会两边都做不好：
//!
//! - XMODEM 是「停等 + 定长块」，状态机是**线性**的：发一块、等 ACK、发下一块。
//!   YMODEM 只是在它前面加了个头块、在后面加了个空头块，仍然是线性的。
//! - ZMODEM 是**滑动窗口 + 可变长帧 + 帧内十六进制转义 + 会话内子协议协商**
//!   （`ZRQINIT`→`ZRINIT`→`ZSINIT`→`ZFILE`→`ZDATA`→`ZEOF`→`ZFIN`，帧尾分
//!   ZCRCW/ZCRCQ/ZCRCG 三种），还要处理协商后的「从任意偏移重传」（`ZRPOS`）与
//!   可选压缩；它的错误恢复语义是「从偏移续传」，与 X/Y 的「重发当前块」根本不同。
//! - 结论：ZMODEM 应当**单独一个 crate**（`term-xfer-zmodem` 或本 crate 内的
//!   `zmodem/` 子模块），复用本 crate 已有的两样东西——[`packet::crc16`] 与
//!   `Action` 事件形状——但状态机独立实现，不试图与 X/Y 共用同一个 `feed`。
//!
//! 下一步接 ZMODEM 时的入口：
//!
//! 1. 新增 `Protocol::Zmodem` 变体，但**不要让** [`Receiver`]/[`Sender`] 内部出现
//!    `match protocol { ... }` 分叉——正确做法是新类型 `ZmodemSender` /
//!    `ZmodemReceiver`，与现有两个类型实现同一个 trait（届时把
//!    `fn feed(&mut self, &[u8]) -> Vec<Action>` / `fn on_timeout(&mut self)` /
//!    `fn cancel(&mut self)` 抽成 trait `Transfer`，UI 只持有 `Box<dyn Transfer>`）。
//!    现在不抽 trait 是因为只有实现方没有使用方，抽了也是空转。
//! 2. `Action` 可以直接复用：ZMODEM 的「文件开始 / 数据 / 文件结束」语义与 YMODEM
//!    一一对应，只是多了「从偏移重传」——那属于协议内部状态，不需要新事件。
//! 3. CRC-32 加在 [`packet`] 里，与 `crc16` 并列。
//!
//! # 目录同步
//!
//! [`sync`] 模块是 XFER-06 的决策层与执行记账层：两份目录清单（本地 / 远端）进，
//! 「上传 / 下载 / 删除 / 跳过」的计划出（[`sync::plan`]）；随后 [`sync::SyncRun`]
//! 按 [`sync::execution_order`] 一项一项推进并记账——**先建父目录、后删父目录**
//! 这条顺序契约就落在这里。它同样**零 IO**：扫盘与 SFTP 列目录都在调用方，这里只
//! 比较 `path` / `is_dir` / `size` / `mtime` 四个字段，因此规则可以被纯数据单测钉死，
//! 也不需要任何线程或时钟。
//!
//! # 传输队列
//!
//! [`queue`] 是「一次传多个文件」的模型层：入队顺序、并行上限、退避重试、总进度
//! 全在这里算，界面只负责画。同样是零 IO——连时钟都不读，`now` 由宿主注入，
//! 所以每条策略（第 n 次重试等多久、什么时候算彻底失败）都能用纯数据单测钉死。
//!
//! [`rate`] 是「还要多久」的那一块：瞬时速率 → 指数平滑 → 剩余时间。`now_ms` 同样
//! 由宿主注入，所以采样窗口、折叠、回退这些规则也全是纯数据单测。
//!
//! # 本轮没做的事
//!
//! - **不接 UI**：菜单命令的接线是下一轮，本 crate 的公开 API 就是那一轮的接口面。
//! - **不落盘**：[`Action::Data`] 交给谁写文件与本 crate 无关。
//! - **不做 YMODEM-G**（流式、无 ACK、只在大容量链路上有意义）与 XMODEM 的
//!   「单字符校验和」变体（XMODEM-1K 之外的古董）。
//!
//! # 与建议形状的差异（一处新增事件）
//!
//! 建议的 `Action` 只有 `Send` / `FileStart` / `FileDone` / `Done` / `Error`，缺了
//! **「收到的文件内容」**这一路：接收方解出来的载荷必须交给调用方落盘，否则这个
//! crate 无法完成「接收」这件本职。因此加了 [`Action::Data`]。发送方永远不产出它。
//!
//! 另外两处小补充（都是为了让调用方不必偷看内部状态）：
//!
//! - [`Receiver::start`]：接收方必须先发握手字符（`'C'`/`NAK`）才能开始，而构造
//!   函数不该有副作用，所以给一个显式入口；`feed` 在 `Idle` 时也会自动补一次，
//!   忘调不致命。
//! - `phase()` / `received_bytes()` / `sent_bytes()` / `queued_files()`：给 UI 画进度。

#![deny(missing_docs)]
#![deny(rustdoc::broken_intra_doc_links)]

pub mod action;
pub mod config;
pub mod error;
pub mod packet;
pub mod queue;
pub mod rate;
mod receiver;
mod sender;
pub mod sync;

pub use action::{Action, Phase};
pub use config::{Checksum, Config, Protocol};
pub use error::{Error, Fault};
pub use queue::{Direction, FailReason, Job, JobState, Policy, Queue};
pub use rate::{RateMeter, MIN_WINDOW_MS};
pub use receiver::Receiver;
pub use sender::Sender;
pub use sync::{
    execution_order, plan, DeletePolicy, Progress, StepResult, SyncEntry, SyncItem, SyncOp, SyncRun,
};

//! `term-zmodem` — ZMODEM 收发状态机。
//!
//! 目标是把 `Terminal.SendZmodem` / `ReceiveZmodem` 背后的**协议内核**做实：给定
//! 「远端字节流」与「本地文件内容」，产出「该发给远端的字节」与「文件/进度事件」。
//!
//! # 为什么单独一个 crate
//!
//! `term-xfer` 的 `lib.rs` 已经写清了理由，这里只重复结论：ZMODEM 是**滑动窗口 +
//! 可变长帧 + 帧内十六进制转义 + 会话内子协议协商**，错误恢复语义是「从偏移重传」
//! 而不是 X/YMODEM 的「重发当前块」。硬塞进 `term-xfer` 会让两边的 `feed` 都长出
//! 一堆 `match protocol` 分支。所以复用 `term-xfer` 的 `Action` / `Error` 形状与
//! `packet::crc16`，状态机独立实现。
//!
//! # 三条纪律
//!
//! 1. **纯逻辑，零 IO。** 不读文件、不开 socket、不 sleep、不起线程、不看时钟。
//!    输入是 `&[u8]`，输出是 [`Action`]。落盘与发送通道由调用方持有。
//! 2. **只依赖 `term-xfer`。** 没有异步运行时、SSH 栈、UI 框架、文件系统、线程、
//!    网络、单调时钟的任何符号（口径同 `scripts/verify.sh` 的异步笼子那一步）。
//! 3. **定时器归调用方。** 「对端静默 3 秒就重发」是**策略**，本 crate 只提供
//!    [`Receiver::on_timeout`] / [`Sender::on_timeout`] 这个钩子。
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
//! # 从真联调学到的线上事实
//!
//! 这一段是**被真机 `sz` / `rz`（`lrzsz` 0.13.1）打回来之后**才写下的。每一条都
//! 回到 `src/zm.c` / `lsz.c` / `lrz.c` 与真机抓到的字节上复核过；写错任何一条的
//! 症状都不是「报个错」，而是**对端无限重发**或**一个子包都不接受**。
//!
//! 1. **帧头的 4 字节位置字段是小端。** `zmodem.h` 里 `ZP0 = 0` 是最低字节，
//!    `stohdr()`（`zm.c:960`）逐字节发 `pos`、`pos>>8`、`pos>>16`、`pos>>24`。
//!    本 crate 统一走 [`frame::encode_position`] / [`frame::decode_position`]。
//!    偏移 0 时大小端看不出差别，所以这个错只在**第一块之后**才发作——症状是
//!    我们把 `ZACK(1024)` 写成 `00000400`，`sz` 读成 262144 后干脆不理。
//! 2. **`ZRINIT` 的能力位在头数据的第 4 字节。** `#define ZF0 3`（`zshhdr` 里
//!    `Txhdr[ZF0] = flags`），缓冲大小在前两字节（小端，`lsz.c:1621` 就是
//!    `Rxhdr[ZP0] + (Rxhdr[ZP1]<<8)`）。见 [`frame::rinit_header_data`]。
//! 3. **`ZFILE` 的头数据不是位置字段**，第 4 字节是转换选项 `ZF0`；真实 `sz` 发
//!    `00 00 00 01`（`ZCBIN`）。见 [`frame::FILE_HEADER_DATA`]。
//! 4. **`ZHEX` 终止符是 `0D 8A`**（`LF|0x80`），且除 `ZACK`/`ZFIN` 之外再跟一个
//!    裸 `XON` 把可能被假 `XOFF` 掐住的远端放行（`zshhdr()`，`zm.c:409-418`）。
//! 5. **`XON|0x80`（0x91）与 `XOFF|0x80`（0x93）必须转义。** 它们的低位看着像
//!    流控、高位看着「无害」，但 `zdlread2()`（`zm.c:188-247`）对这两个**裸值**
//!    是直接 `goto again` 吃掉的——整帧从此错位，对端只报 `Bad CRC`。`0x8D` 同理：
//!    对端 `Zctlesc` 置位时它同样被 `goto again` 吞掉（`zm.c:212-215`）。这条是
//!    方向二（我们发、`rz` 收）一个子包都不被接受的**根因**。
//! 6. **子包 CRC 覆盖「载荷 + 帧尾字节」**，`ZCRCW` 收尾后立刻补一个 `XON` 并
//!    flush（`zsdata()`，`zm.c:428-447`）。本 crate 的算法本来就对。
//! 7. **帧头 CRC-16 覆盖「类型 + 4 字节头」，不额外加零字节**（`zsbhdr()`，
//!    `zm.c:344-353`；表法本身等价于补 16 个 0 位）。本 crate 的算法也本来就对
//!    ——真机 `sz` 的三处帧头/子包反算是 `0x9927` / `0x46ae` / `0x0b9d`，与我们的
//!    输出逐字节一致。所以最初那两条 CRITICAL 与校验算法无关。
//! 8. **在 `ZRINIT` 里宣告 `ESCCTL`（0x40）会让 `sz` 真的打开 `Zctlesc`**
//!    （`lsz.c:1615-1619`：`Zctlesc |= Rxflags & TESCCTL`，随即重建转义表），
//!    之后它把 `0x00-0x1F` 与 `0x80-0x9F` 统统转义成 `ZDLE + (c ^ 0x40)`。
//!    所以**接收侧两种都得吃**：转义过的，以及裸的（对端不宣告 `ESCCTL` 时
//!    `CR` 就是裸的，`zm.c:936-945` 只有跟在 `'@'` 后面才转义）。
//!
//! ```
//! // 位置字段的钉子：真机 `sz`/`rz` 认的是小端。
//! assert_eq!(term_zmodem::frame::encode_position(1024), [0x00, 0x04, 0x00, 0x00]);
//! assert_eq!(term_zmodem::frame::decode_position([0x00, 0x04, 0x00, 0x00]), 1024);
//! // `ZFILE` 头数据：`ZF0 = ZCBIN = 1` 落在第 4 字节。
//! assert_eq!(term_zmodem::frame::FILE_HEADER_DATA, [0, 0, 0, 1]);
//! // `ZRINIT`：缓冲 8192 小端在前两字节，能力位在最后。
//! assert_eq!(
//!     term_zmodem::frame::rinit_header_data(0x60, 8192),
//!     [0x00, 0x20, 0x00, 0x60]
//! );
//! ```
//!
//! # 实现的协议范围（依据）
//!
//! 依据是 Chuck Forsberg 的《ZMODEM Protocol Description》与 `lrzsz` 的实现约定
//! （帧类型编号、`ZFILE` 字段格式、`OO` 收尾、5 个 `CAN` 判中止、三种帧头的
//! 字节布局，均照 `lrzsz` / 协议原文 §7.3）。
//!
//! - **帧层**：三种帧头**全部能解**——
//!   `ZHEX`（`* * ZDLE 'B'` + 14 个十六进制字符 + `0D 8A`（+ 可选 `XON`））、
//!   `ZBIN`（`* ZDLE 'A'` + 1 字节类型 + 4 字节头 + CRC-16）、
//!   `ZBIN32`（`* ZDLE 'C'` + 1 字节类型 + 4 字节头 + CRC-32）；
//!   帧头前允许任意多个 `ZPAD`（有上限）；类型、头数据、CRC 在二进制帧头里逐字节
//!   转义。数据子包的 ZDLE 转义、四种帧尾、以及**跟帧头类型走的子包 CRC**
//!   （`ZHEX`/`ZBIN` → CRC-16，`ZBIN32` → CRC-32）都已实现。帧头里的 4 字节在
//!   位置语义下是**小端**（[`frame::encode_position`]）。
//! - **发送方**：**按帧类别选帧头**。握手/控制帧（`ZRQINIT`、`ZFIN`）**永远
//!   `ZHEX`**——开场时双方还不知道链路是否 8-bit clean；数据帧（`ZFILE` /
//!   `ZDATA` / `ZEOF` / `ZSINIT`）按协商：对端 `ZRINIT` 置了 `CANFC32`（0x20）
//!   才用 `ZBIN32`，否则 `ZBIN`（[`Config::header`] 可把数据帧钉在 `Bin`/`Hex`）。
//!   时序：`ZRQINIT` →（收 `ZRINIT`）→ `ZFILE`（名字 + 长度 + mtime + 权限）→
//!   `ZDATA` 逐块（默认 `ZCRCW`，每块等 `ZACK`；[`Config::streaming`] 打开后
//!   连发 `ZCRCG`，只在结尾 `ZCRCW`）→ `ZEOF` →（收 `ZRINIT`）→
//!   `ZFIN` →（收 `ZFIN`）→ `OO`。
//! - **接收方**：`ZRINIT`（宣告 `CANFC32`）→（收 `ZFILE`）→ `ZRPOS(0)`
//!   （有断点时回 `ZRPOS(N)`，见 [`Receiver::resume_from`]）→ 逐块
//!   `ZACK`（`ZCRCG` 连发则块间不答）→（收 `ZEOF`）→ `ZRINIT` →（收 `ZFIN`）→
//!   `ZFIN`。应答**永远 `ZHEX`**，
//!   与 [`Config::header`] 无关；解析也不依赖配置，三种帧头混着来照收（真实对端
//!   就是 `ZRINIT` 用 `ZHEX`、数据帧用 `ZBIN32`）。
//! - **错误路径**：帧头/子包 CRC 错 → 接收方 `ZRPOS`（从当前期望偏移重发）、发送方
//!   重发上一帧；偏移倒退计入失败；重试超次放弃并甩中止序列；对端连发 5 个 `CAN`
//!   判中止；本端 [`Sender::cancel`] / [`Receiver::cancel`] 主动中止。
//! - **边界**：零长文件（`ZFILE` 之后直接 `ZEOF(0)`，不产生空 `Action::Data`）、
//!   UTF-8 文件名（按字节透传，接收侧 `from_utf8_lossy`）、单文件（默认）与
//!   多文件批处理（[`Config::batch`]，opt-in）、`ZCRCG` 连发（[`Config::streaming`]，
//!   opt-in；接收侧两种帧尾都能解析，与配置无关）。
//! - **抗外部输入**：帧头前连续 `ZPAD` 上限 [`frame::MAX_ZPAD_RUN`]，未消费缓冲上限
//!   [`frame::MAX_BUFFER`]；超限即 `Error::BadHeader` 并清空，不会无限分配。
//!
//! # 与 `term-xfer` 的复用边界
//!
//! | 东西 | 处理 |
//! | --- | --- |
//! | `Action` / `Error` / `Fault` | 直接复用（`pub use`），UI 只认一套事件 |
//! | `packet::crc16` | 直接复用（帧头 CRC），一条「没偷偷换算法」的测试钉着 |
//! | `Phase` | **自己定义**——X/Y 的变体名是线性时序（`SendingHeader` 等），套到 ZMODEM 上会错位 |
//! | `Config` | 自己定义——X/Y 的「校验方式 / 填充 / 协议变体」在 ZMODEM 里不存在 |
//!
//! `Error` 的变体没有为 ZMODEM 新增：`TooManyRetries`（重试耗尽，`Fault` 记最后一次
//! 原因）、`Cancelled`（对端中止）、`BadHeader`（`ZFILE` 子包解析失败 / 对端
//! `ZFERR` / `ZSKIP`）、`BadFileSpec`、`NoFileToSend`、`AlreadyStarted` 都够用。
//! 唯一语义上略勉强的是把 `BadHeader` 当作「对端明确拒绝」的通用出口，调用方按
//! `detail` 展示即可。
//!
//! # 能力边界：已做与没做
//!
//! - **`ZRPOS` 断点续传。** 会话内按偏移重发是支持的（文件全在内存，代价为零）；
//!   **跨会话续传也支持，但起点由调用方给**：接收侧在 [`Receiver::start`] 之前调用
//!   [`Receiver::resume_from`] 告知「本地已有前 N 字节」，收到 `ZFILE` 时回
//!   `ZRPOS(N)`，发送侧据此把 `offset` 直接推到 N。本 crate 仍然零 IO——它不发现
//!   半截文件、不做 seek / 截断，调用方按「追加写」落盘即可。
//!   早期版本借用 `ZSKIP(N)` 承载偏移（自洽但不符合协议），已由 W6-C4b 纠正；
//!   发送侧仍保留 `ZSKIP(offset > 0)` 的兼容兜底。
//! - **`ZCRCG` 连发与 `ZCRCQ`。** 连发已实现（[`Config::streaming`]），默认仍走
//!   停等 `ZCRCW`：连发的错误恢复是「整段回退重来」，不是所有调用方都要那个代价。
//!   接收方的 `ZRINIT` 仍然**不宣告 `CANFDX`**，所以真实 `sz` 面对我们依旧停等；
//!   连发路径靠我们自己的发送方 + `tests/batch_stream.rs` 覆盖。`ZCRCQ`（要求回
//!   `ZACK` 的中间块）只解析，不主动发。
//! - **压缩（`ZCDATA` / `ZDUMMY`）与 MIME。** 前者需要 deflate 一类的编码器，属于
//!   另一个依赖面；后者只影响 `ZFILE` 的字段解释，先不做。
//! - **多文件批处理**已按 opt-in 实现（[`Config::batch`]，默认关闭）：接收方在
//!   `ZEOF` 之后接受下一个 `ZFILE`，每个文件结束发一次 [`Action::FileDone`] 作边界；
//!   发送方多出来的文件排队。默认关闭时行为与单文件逐字节一致。仍未做的是
//!   「第 K 个文件失败怎么办」的批级策略——失败依旧直接中止整个会话。
//! - **`ZSINIT` 的注意序列。** 只做「收到就跳过它的子包」，不解析、不回显。
//! - **`ZCRC` / `ZCHALLENGE` / `ZFREECNT` / `ZCOMMAND` 等命令帧。** 收到只忽略；
//!   真实 `sz` 的默认配置不发这些（`--zmodem` 不带 `--zcheck` 之类的选项时），
//!   所以不影响主路径。
//! - **十六进制帧头的 `XON` 尾巴。** 除 `ZACK`/`ZFIN` 外，每个 `ZHEX` 帧后都跟
//!   一个裸 `XON` 冲掉线路噪声产生的假 `XOFF`（`zshhdr()` 的行为，`zm.c:409-418`）。
//!   本实现**照发**；解析侧也能吃（[`frame::is_flow_control`]），并且对端少发这个
//!   尾巴同样成立——它是可选的。
//! - **不接 UI、不落盘。** 同 `term-xfer`：[`Action::Send`] 由会话写，[`Action::Data`]
//!   由调用方写文件。
//!
//! # 安全边界
//!
//! 接收侧的 [`Action::FileStart`] 带的是**对端给的原始文件名**。本 crate 不碰文件
//! 系统，无法判断「这个名字拼出来的路径会落到哪」。调用方必须自己拒绝绝对路径、
//! `..`、以及平台相关的特殊名字——这是本 crate 唯一一处必须由上层兜底的边界。
//!
//! # 已知风险（按严重程度排）
//!
//! 1. **只做过两个方向的真机联调，覆盖面就是那两个方向。** `tests/interop.rs` 现在
//!    真的跑通「我们收 / 真 `sz` 发」与「我们发 / 真 `rz` 收」，内容逐字节比对。
//!    没覆盖的是：`ZCRCG` 流式（`sz` 因为我们的 `ZRINIT` 不宣告 `CANFDX` 而自动
//!    退化成停等，`lsz.c:1622-1623` 把 `Txwindow` 置 0）、多文件批处理、断点续传、
//!    压缩。这些路径上没有真机证据，只有内存字节流测试：续传在 `tests/resume.rs`、
//!    批处理与连发在 `tests/batch_stream.rs`。续传按协议走 `ZRPOS(N)`，与真实
//!    `lrzsz` 的续传请求一致，但仍未与真 `sz` 对过；
//! 2. **协商只认 `CANFC32` 一位。** 我们按它决定数据帧用 `ZBIN32` 还是 `ZBIN`，
//!    不处理「对端明确要求某种帧头」的其它信号；也不主动发 `ZCHALLENGE` /
//!    `ZCRC` 之类的探测帧。对端若对二进制帧头有额外讲究，只能靠
//!    [`Config::header`] 手动钉死。
//! 3. **未知帧一律跳过**（`ZCRC`、`ZCHALLENGE` 等）。这会让「对端说了什么我们
//!    没听懂」表现为超时重试而不是明确报错，排查时要看线上抓包。
//! 4. **`ZFIN` 握手的重传**：本端收到 `ZFIN` 回 `ZFIN` 后立即 `Finished`，若这个
//!    回应丢失，对端会重发 `ZFIN` 而我们已不再处理（终态 `feed` 返回空）——文件
//!    本身是完整的，只是对端会报收尾超时。要修需要一个短暂的「收尾居留」状态。
//! 5. **上限是硬拒绝而不是流式剪裁**：帧头前 `ZPAD` 超过
//!    [`frame::MAX_ZPAD_RUN`]、或未消费缓冲超过 [`frame::MAX_BUFFER`]，直接中止。
//!    这两个值远高于任何合法帧（4096 / 16448），正常对端撞不上，但确实是一条
//!    「宁可误杀也不无限分配」的取舍。
//! 6. **`ZCRCG` 连发的恢复粒度是整段。** 连发流里丢一块，后面的子包全部错位，
//!    所以接收方在 `ZCRCG` 中途校验失败时丢掉残尾、回 `ZRPOS(已收字节)`，让对端
//!    从最后一次成功的位置整段重发——代价与文件剩余长度成正比。停等模式没有这个
//!    问题（每块一个帧头，跳过一个坏块后能自行重新对齐）。

#![deny(missing_docs)]
#![deny(rustdoc::broken_intra_doc_links)]

pub mod config;
pub mod frame;
mod phase;
mod receiver;
mod scan;
mod sender;

pub use config::{Config, HeaderKind};
pub use frame::{crc32, CrcKind};
pub use phase::Phase;
pub use receiver::Receiver;
pub use sender::Sender;

pub use term_xfer::{Action, Error, Fault};

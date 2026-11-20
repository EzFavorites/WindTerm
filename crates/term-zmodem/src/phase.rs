//! 状态机阶段。
//!
//! 为什么不用 `term_xfer::Phase`：那一组变体是 X/YMODEM 的**线性时序**命名的
//! （`SendingHeader` 是「已发 YMODEM 头块」、`AwaitingDataRequest` 是「等收方喊 `C`」）。
//! ZMODEM 的四段（`ZRQINIT`→`ZRINIT`、`ZFILE`→`ZRPOS`、`ZDATA`⇄`ZACK`、
//! `ZEOF`→`ZRINIT`→`ZFIN`）套进去会出现「`SendingHeader` 其实在等 ZFILE 的应答」
//! 这类错位命名，UI 上更难读。所以这里按 ZMODEM 的帧名重新起一组。
//!
//! [`Phase::is_terminal`] 的语义与 `term_xfer` 一致：终态下 `feed` 不再做任何事。

/// 状态机当前所处阶段。
///
/// Sender 与 Receiver 共用同一个枚举，每个变体的文档注明归属；另一方永远不会处在
/// 它不拥有的阶段。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Phase {
    /// 尚未开始。发送方还没发 `ZRQINIT`；接收方还没发 `ZRINIT`。
    Idle,

    /// 发送方：已发 `ZRQINIT`，等接收方的 `ZRINIT`。
    AwaitingZrinit,

    /// 接收方：已发 `ZRINIT`，等发送方的 `ZFILE`。
    AwaitingZfile,

    /// 发送方：已发 `ZFILE`（文件名 + 长度 + mtime 的子包），等 `ZRPOS`。
    ///
    /// 对端的 `ZRPOS(N)` 表示「前 N 字节已有，从 N 续传」（N 为 0 就是从头收）；
    /// `ZSKIP(0)` 则是协议原义「跳过该文件」。早期本项目用 `ZSKIP(N)` 承载续传
    /// 偏移，现已改为 `ZRPOS(N)`（见 [`crate::Receiver::resume_from`]）；发送方
    /// 仍保留对旧约定的容忍。
    AwaitingZrpos,

    /// 接收方：正在收数据块（`ZDATA` 头 + 数据子包）。
    Receiving,

    /// 发送方：已发一块 `ZDATA`（帧尾 `ZCRCW`），等 `ZACK` / `ZRPOS`。
    AwaitingZack,

    /// 发送方：已发 `ZEOF`（偏移 = 文件长度），等接收方的 `ZRINIT`。
    AwaitingZrinitAfterEof,

    /// 已发 `ZFIN`，等对端回 `ZFIN`（收发双方的收尾握手）。
    AwaitingZfin,

    /// 成功结束。
    Finished,

    /// 失败或被任一方中止。
    Aborted,
}

impl Phase {
    /// 是否为终态（[`Phase::Finished`] / [`Phase::Aborted`]）。终态下 `feed` 不再做任何事。
    pub fn is_terminal(self) -> bool {
        matches!(self, Phase::Finished | Phase::Aborted)
    }
}

//! 状态机的输出事件。
//!
//! 状态机只产出两种东西：**该发给对端的字节**（[`Action::Send`]）与**给调用方的
//! 事实**（文件开始 / 数据 / 文件结束 / 传输完成 / 失败）。调用方按变体分流：
//! `Send` 写进会话通道，`Data` 写进文件，其余驱动 UI。

use crate::error::Error;

/// 状态机一次推进后要调用方做的事。
///
/// 变体顺序即语义分组：`Send` 面向对端，`FileStart` / `Data` / `FileDone` 面向
/// 落盘，`Done` / `Error` 面向流程控制。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    /// 把这些字节原样写给对端（握手字符、ACK/NAK、帧、CAN 序列）。
    ///
    /// 多条 `Send` 的顺序就是线上顺序，调用方不得重排或合并后改变顺序。
    Send(Vec<u8>),

    /// 一个文件开始了。
    ///
    /// - YMODEM：来自文件头块（块 0），`name` / `size` 都是从线上解析出来的；
    /// - XMODEM：协议本身不带文件名也不带长度，此时 `name` 为空串、`size` 为
    ///   `None`。**空串意味着「协议未携带文件名」**，不是「文件叫空名字」，
    ///   调用方应当自己决定落盘名（或弹窗问用户）。
    ///
    /// 保证 `Action::Data` 一定出现在某个 `FileStart` 之后。
    FileStart {
        /// 文件名。XMODEM 恒为空串（协议不携带文件名）。
        name: String,
        /// 文件字节数。XMODEM 恒为 `None`（协议不携带长度）。
        size: Option<u64>,
    },

    /// 当前文件的载荷字节，已按协议去掉块尾填充。
    ///
    /// - YMODEM：按文件头块声明的长度精确裁剪，末尾填充不会漏进文件；
    /// - XMODEM：无长度信息，块尾填充字节被**暂扣**（见 [`crate::Receiver`] 文档
    ///   的「填充暂扣」一节），最后一块的填充在 EOT 时丢弃。
    ///
    /// 不会出现空 `Data`：没有有效字节时不产事件。
    Data(Vec<u8>),

    /// 当前文件收/发完毕（XMODEM 在 EOT 之后，YMODEM 在 EOT 之后；发送侧在
    /// 该文件的 EOT 被 ACK 之后）。
    FileDone,

    /// 整个传输成功结束，状态机进入 [`Phase::Finished`]，后续 `feed` 一律返回空。
    Done,

    /// 传输失败，状态机进入 [`Phase::Aborted`]，后续 `feed` 一律返回空。
    ///
    /// 若失败是本端判定（例如重试超次），同一批 action 里会先有一个
    /// [`Action::Send`]，内容为 CAN 序列——用于通知对端别再发了。
    Error(Error),
}

/// 状态机当前所处阶段。
///
/// Sender 与 Receiver 共用同一个枚举（UI 只需显示一个阶段名）；每个变体的文档
/// 注明归属，另一方永远不会处在它不拥有的阶段。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Phase {
    /// 尚未开始。接收方还没发握手字符；发送方还没收到对端的 `C` / `NAK`。
    Idle,

    /// 接收方：已发出握手字符（`C` 或 `NAK`），等对端的第一帧。
    ///
    /// YMODEM 在每次 EOT 被 ACK 之后也会回到这里——那一轮的「帧」是下一个文件
    /// 的头块（块 0）。
    Handshake,

    /// 发送方：已发出 YMODEM 文件头块，等 ACK。
    SendingHeader,

    /// 发送方：头块已被 ACK，等接收方的 `C`（YMODEM 规范：ACK 之后再发 `C`
    /// 才开始请求数据）。
    AwaitingDataRequest,

    /// 发送方：已发出一个数据块，等 ACK / NAK。
    SendingData,

    /// 发送方：已发出 EOT，等 ACK（部分接收方先 NAK 再 ACK，本实现两种都吃）。
    AwaitingEndAck,

    /// 发送方：EOT 已被 ACK，等接收方的 `C`——它请求下一个文件头块（或批结束的
    /// 空头块）。只有 YMODEM 会到这里。
    AwaitingNextHeader,

    /// 接收方：正在收数据块。
    Receiving,

    /// 成功结束。
    Finished,

    /// 失败或被任一方中止。
    Aborted,
}

impl Phase {
    /// 是否为终态（`Finished` / `Aborted`）。终态下 `feed` 不再做任何事。
    pub fn is_terminal(self) -> bool {
        matches!(self, Phase::Finished | Phase::Aborted)
    }
}

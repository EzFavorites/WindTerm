//! 回信句柄与界面侧的请求视图。
//!
//! `PendingAsk` / `PendingPick` 活着的是**通道发送端**，归 apps 层的脚本线程
//! 所有；界面不该看见那个类型（B2 门禁第 5 项）。这里把发送端收进不透明句柄，
//! 只露一个 `reply(...)` 方法，再由 `From<PendingAsk>` / `From<PendingPick>` 把
//! 队列里的请求转成界面能拿的视图——ui/ 于是只持有句柄，不持有通道端。
//!
//! 句柄内部仍是有界容量 1 的 `SyncSender`：脚本只等一个答案，界面也只回一个；
//! 第二发会失败，不会悄悄留给下一个问题。

use std::path::PathBuf;
use std::sync::mpsc::SyncSender;

use term_script::{Answer, Ask, AskError, CancelToken, PickFile};

use super::{PendingAsk, PendingPick};

/// 脚本提问的回信句柄（不透明：ui/ 看不到里面的通道类型）。
pub struct AskReply {
    tx: SyncSender<Result<Answer, AskError>>,
}

impl AskReply {
    /// 只供 apps 层把队列请求转成界面视图时调用。
    pub(crate) fn new(tx: SyncSender<Result<Answer, AskError>>) -> Self {
        Self { tx }
    }

    /// 把这一帧的结论送回等着的脚本线程。
    ///
    /// 送不出去只有一种原因：脚本线程已经收摊（取消 / 窗口关闭）。那不是界面
    /// 的错误，调用方可以像以前一样忽略这个结果；返回 `Err(NoInput)` 是为了让
    /// 句柄自己不撒谎说「送达了」。
    pub fn reply(&self, answer: Result<Answer, AskError>) -> Result<(), AskError> {
        self.tx.send(answer).map_err(|_| AskError::NoInput)
    }
}

/// 文件框的回信句柄（与 [`AskReply`] 同理，载荷换成一个可选路径）。
pub struct PickReply {
    tx: SyncSender<Option<PathBuf>>,
}

impl PickReply {
    /// 只供 apps 层把队列请求转成界面视图时调用。
    pub(crate) fn new(tx: SyncSender<Option<PathBuf>>) -> Self {
        Self { tx }
    }

    /// `Some(path)` = 用户手选的路径（授权在 apps 层追加）；`None` = 取消。
    pub fn reply(&self, path: Option<PathBuf>) -> Result<(), AskError> {
        self.tx.send(path).map_err(|_| AskError::NoInput)
    }
}

/// 界面拿到的一条提问：请求原样，回信口换成不透明句柄。
pub struct AskRequest {
    pub cancel: CancelToken,
    pub request: Ask,
    pub reply: AskReply,
}

impl AskRequest {
    /// 已经立着的模态框所属运行被取消时，也要撤掉它。
    pub fn is_stale(&self) -> bool {
        self.cancel.is_cancelled()
    }
}

impl From<PendingAsk> for AskRequest {
    fn from(pending: PendingAsk) -> Self {
        Self {
            cancel: pending.cancel,
            request: pending.request,
            reply: AskReply::new(pending.reply),
        }
    }
}

/// 界面拿到的一条文件框请求：请求原样，回信口换成不透明句柄。
pub struct PickRequest {
    pub cancel: CancelToken,
    pub request: PickFile,
    pub reply: PickReply,
}

impl PickRequest {
    /// 已经被「停止」的请求连弹窗都不该再弹。
    pub fn is_stale(&self) -> bool {
        self.cancel.is_cancelled()
    }
}

impl From<PendingPick> for PickRequest {
    fn from(pending: PendingPick) -> Self {
        Self {
            cancel: pending.cancel,
            request: pending.request,
            reply: PickReply::new(pending.reply),
        }
    }
}

//! 取消原语，以及「停下之后目标文件怎么办」的那张表。
//!
//! # 为什么单开一个模块
//!
//! 取消**不是**一个布尔参数这么简单。`docs/design/05-files-and-transfer.md` 把三种停下
//! 方式分得清清楚楚，它们的差别不在「停不停」，而在**停完要不要删文件**：
//!
//! | 触发 | 目标文件 | 用户意图 |
//! |---|---|---|
//! | 传完（`Completed`） | 留着 | 这就是结果 |
//! | 网络中断 / IO 错误（`Failed`） | **留分片**，标 `paused`，可续传 | 「没传完」 |
//! | 用户主动取消（`Cancelled`），目标原先不存在 | **删掉**分片 | 「不要了」 |
//! | 用户主动取消，目标原先就存在 | **一个字节都不动** | 那是用户既有数据 |
//!
//! 最后一行是最容易写错的一行：把「用户传了一半的文件」当成「我们造的垃圾」删掉，
//! 等于替用户做了删除决定。所以判据是**传输开始前目标是否已存在**，不是传了多少。
//!
//! 表在 [`disposition`] 里，是纯函数，单测不需要连服务端；旗子在
//! [`CancelToken`] 里，可克隆（句柄一份、传输循环一份，看到的是同一个 `AtomicBool`）。

use std::fmt;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

/// 一条可跨线程、可克隆的取消旗子。
///
/// `Clone` 是廉价且**语义上必须**的：句柄持有它来响应「取消」按钮，传输循环
/// 持有一份来轮询；两个克隆指向同一个 `AtomicBool`，否则取消只能在下一帧生效。
///
/// 一条旗子不值得为内存序做推理，统一 `SeqCst`：这里没有性能敏感的热路径
/// （每 64 KiB 才读一次），换来的是「读到的值一定是某次写入的值」这种可以直说的性质。
#[derive(Clone, Default)]
pub struct CancelToken(Arc<AtomicBool>);

impl CancelToken {
    pub fn new() -> Self {
        Self::default()
    }

    /// 置位。重复调用是幂等的——用户连点几下「取消」不该有任何额外后果。
    pub fn cancel(&self) {
        self.0.store(true, Ordering::SeqCst);
    }

    /// 传输循环每个分片问一次。`true` = 立刻收尾。
    pub fn is_cancelled(&self) -> bool {
        self.0.load(Ordering::SeqCst)
    }

    /// 清位。**只由后台线程在每次传输开始前调**：句柄是复用的，一条传输的
    /// 取消不能顺延到下一条。反过来，如果把清位放在收请求之前，用户在
    /// 「点了取消、下一条还没开始」之间点的那一下会被吞掉——所以清位要紧贴着
    /// 传输开始，而不是紧贴着请求入队。
    pub fn reset(&self) {
        self.0.store(false, Ordering::SeqCst);
    }
}

impl fmt::Debug for CancelToken {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_tuple("CancelToken")
            .field(&self.is_cancelled())
            .finish()
    }
}

/// 一次传输是怎么停下的。`Completed` 也算「停下」，这样处置表只有一个入口。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Stop {
    /// 传完了。
    Completed,
    /// 用户按了取消。
    Cancelled,
    /// 网络断了 / 权限不对 / 磁盘满。
    Failed,
}

/// 停下之后对目标文件的处置。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Disposition {
    /// 原地留着，一个字节都不动。
    Keep,
    /// 删掉（这是本次传输造出来的半成品）。
    Delete,
}

/// §2.1 的处置表。`target_preexisted` 是**传输开始前**目标是否已存在。
///
/// 注意 `Failed` 与 `Cancelled { preexisted }` 的分野：同样是「传了一半」，
/// 前者留着等续传，后者在目标原本不存在时要删干净——因为用户的意图不同。
pub fn disposition(stop: Stop, target_preexisted: bool) -> Disposition {
    match stop {
        Stop::Cancelled if !target_preexisted => Disposition::Delete,
        _ => Disposition::Keep,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_fresh_token_is_not_cancelled() {
        assert!(!CancelToken::new().is_cancelled());
    }

    #[test]
    fn clones_share_one_flag() {
        // 句柄与传输循环各持一份，取消必须立刻被看见。
        let handle = CancelToken::new();
        let worker = handle.clone();
        handle.cancel();
        assert!(worker.is_cancelled(), "克隆必须指向同一条旗子");
        worker.cancel();
        assert!(handle.is_cancelled(), "反向也一样");
    }

    #[test]
    fn cancelling_twice_is_idempotent() {
        let token = CancelToken::new();
        token.cancel();
        token.cancel();
        assert!(token.is_cancelled());
    }

    #[test]
    fn reset_lets_the_next_transfer_start_clean() {
        // 句柄复用：上一条传输的取消不许顺延到下一条。
        let token = CancelToken::new();
        token.cancel();
        token.reset();
        assert!(!token.is_cancelled());
    }

    #[test]
    fn a_completed_transfer_always_keeps_the_target() {
        assert_eq!(disposition(Stop::Completed, false), Disposition::Keep);
        assert_eq!(disposition(Stop::Completed, true), Disposition::Keep);
    }

    #[test]
    fn a_failed_transfer_keeps_the_partial_file_for_resume() {
        assert_eq!(
            disposition(Stop::Failed, false),
            Disposition::Keep,
            "网络中断要留分片，否则 `--resume` 是句假话"
        );
        assert_eq!(disposition(Stop::Failed, true), Disposition::Keep);
    }

    #[test]
    fn cancelling_a_brand_new_target_deletes_the_partial() {
        assert_eq!(
            disposition(Stop::Cancelled, false),
            Disposition::Delete,
            "用户说不要了，本次造出来的分片就该消失"
        );
    }

    #[test]
    fn cancelling_over_an_existing_target_deletes_nothing() {
        assert_eq!(
            disposition(Stop::Cancelled, true),
            Disposition::Keep,
            "那是用户既有数据，程序不替用户做删除决定"
        );
    }
}

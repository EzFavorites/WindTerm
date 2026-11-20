//! 取消令牌：脚本线程与「谁能让它停下来」之间的唯一约定。
//!
//! # 为什么一个原子布尔就够
//!
//! 取消只有一个方向（未取消 → 已取消），而且**不需要**计数、不需要重入、
//! 不需要区分「第几次取消」——脚本要么还在跑，要么已经停了。用 channel
//! 或 `Condvar` 会把「谁负责唤醒」变成新的约定，而这里每个阻塞点本来就
//! 按 50ms 切片轮询（`wait_cancellable` / `collect_cancellable`），
//! 一次原子读就是最合适的粒度。
//!
//! # 为什么不是「杀线程」
//!
//! Rust 没有安全的线程终止。脚本线程卡在 `wait` 里时，唯一能打断它的是
//! **它自己**在切片边界看见令牌——所以每个可能长阻塞的原语都必须接这个
//! 令牌（ADR-0027）。漏一个，那个原语就成了「按停止没反应」的黑洞。

use std::fmt;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

/// 一次运行的取消开关。克隆共享同一个标志。
#[derive(Clone, Default)]
pub struct CancelToken {
    flag: Arc<AtomicBool>,
}

impl CancelToken {
    pub fn new() -> Self {
        Self::default()
    }

    /// 请求取消。幂等；可以在任意线程调用（界面线程就是在这里调的）。
    pub fn cancel(&self) {
        self.flag.store(true, Ordering::SeqCst);
    }

    pub fn is_cancelled(&self) -> bool {
        self.flag.load(Ordering::SeqCst)
    }

    /// 切片检查点：取消就返回 [`Cancelled`]，否则 `Ok(())`。
    pub fn check(&self) -> Result<(), Cancelled> {
        if self.is_cancelled() {
            Err(Cancelled)
        } else {
            Ok(())
        }
    }
}

impl fmt::Debug for CancelToken {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("CancelToken")
            .field("cancelled", &self.is_cancelled())
            .finish()
    }
}

/// 让阻塞点（住在 `term-io`）能看见本令牌而不必依赖脚本引擎。
impl term_core::session_runtime::Cancel for CancelToken {
    fn is_cancelled(&self) -> bool {
        CancelToken::is_cancelled(self)
    }
}

/// 「被用户按了停止」。与超时、会话结束分开：三者的处置不同（超时要报告
/// 等不到什么、会话结束要改走离线分支、取消是「人不想让它继续了」）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Cancelled;

impl fmt::Display for Cancelled {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("脚本已被取消")
    }
}

impl std::error::Error for Cancelled {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cancellation_is_shared_between_clones_and_is_one_way() {
        let token = CancelToken::new();
        let other = token.clone();
        assert!(token.check().is_ok());
        other.cancel();
        assert!(token.is_cancelled(), "克隆共享同一个标志");
        assert_eq!(token.check(), Err(Cancelled));
        // 没有「取消回来」：取消是单向的（要重跑就新建一次运行）。
        assert!(other.is_cancelled());
    }
}

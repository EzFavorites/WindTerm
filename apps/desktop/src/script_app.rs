//! 桌面宿主的应用控制面实现（ADR-0027）。
//!
//! 形状与 `script_human.rs` 的问答队列**一模一样**：脚本线程把请求连同一条
//! 容量 1 的回信通道塞进队列，界面每帧取**一条**执行、执行完回信。不发明第二
//! 套机制——`ask` / `pick_file` 已经是这个形状，多一套只会多一处可漂的地方。
//!
//! # 为什么读也要走队列
//!
//! 选区是「每帧抄一份」的槽（`SelectionSlot`），读到的可能晚一帧；那对选区无害，
//! 对应用状态却是**误判**：脚本 `app_panel_set` 之后立刻 `app_state`，拿到上一帧
//! 的快照就会以为「刚才那次没生效」。所以读写统一走往返，回执在界面线程里现算。
//!
//! # 三条纪律
//!
//! 1. **容量 1**：一问一答；界面回两次时第二发失败，而不是把多余答案喂给下一个请求。
//! 2. **每帧只处理一条**：面板置位是 `exclusive` 的，同帧两条会让「第二条基于第一条
//!    之后的状态」这种语义不可解释。
//! 3. **100ms 轮询取消、2 秒封顶**：与 `UiHuman` 同一节拍；超时返回 `Timeout`，
//!    **不自动重试**（重发一次置位是幂等的，但重发一次布局动作会连做两次）。

use std::sync::mpsc::{sync_channel, RecvTimeoutError, SyncSender};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use eframe::egui;
use term_core::app_state::AppSnapshot;
use term_script::{AppControl, AppDelta, AppError, CancelToken, LayoutOp};

/// 一次应用控制面请求，一个变体对应端口上的一个方法。
pub enum AppRequest {
    /// 读一份快照。
    Snapshot,
    /// 置位一块面板。
    PanelSet { panel: String, visible: bool },
    /// 改停靠位（reset / dock / redock）。
    Layout(LayoutOp),
}

/// 界面线程的回答。
pub enum AppReply {
    Snapshot(AppSnapshot),
    Delta(AppDelta),
    /// 界面侧拒绝（目前只有「面板名不认识」）。带完整的 `AppError` 而不是一句
    /// 字符串：端口层要能分清「名字不存在」与「参数形状不对」，压成字符串就只能
    /// 一律报 `BadArgument`，脚本就没法据此换一条路。
    Error(AppError),
}

/// 待答复的一次请求：请求 + 回信通道。
pub struct PendingApp {
    pub request: AppRequest,
    pub reply: SyncSender<AppReply>,
}

/// 界面与脚本线程之间的请求队列。界面每帧 `remove(0)` 一条。
pub type AppQueue = Arc<Mutex<Vec<PendingApp>>>;

/// 桌面宿主的端口实现。
pub struct DeskApp {
    ctx: egui::Context,
    queue: AppQueue,
    cancel: CancelToken,
}

impl DeskApp {
    pub fn new(ctx: egui::Context, queue: AppQueue, cancel: CancelToken) -> Self {
        Self {
            ctx,
            queue,
            cancel,
        }
    }

    /// 把请求交给界面并等回执。
    fn roundtrip(&self, request: AppRequest) -> Result<AppReply, AppError> {
        let (tx, rx) = sync_channel(1);
        match self.queue.lock() {
            // 队列非空 = 上一帧那条还没落地。**不排第二条**：
            // 置位是幂等的、读是本帧的，两条一起等在队里只会让「谁先谁后」
            // 变得不可解释；脚本拿 `Busy` 自己决定要不要重试，比让它盲等 2 秒好。
            Ok(queue) if !queue.is_empty() => return Err(AppError::Busy),
            Ok(mut queue) => queue.push(PendingApp {
                request,
                reply: tx,
            }),
            // 中毒只说明界面线程持锁时崩过；队列本身还能用，但这一次请求不该
            // 假装成功——按 `Busy` 报，脚本可以决定重试。
            Err(_) => return Err(AppError::Busy),
        }
        // 界面可能此刻没在画（窗口最小化 / 刚起来）：主动要一帧。
        self.ctx.request_repaint();
        let deadline = Instant::now() + Duration::from_secs(2);
        loop {
            if self.cancel.is_cancelled() {
                return Err(AppError::Cancelled);
            }
            match rx.recv_timeout(Duration::from_millis(100)) {
                Ok(reply) => return Ok(reply),
                Err(RecvTimeoutError::Timeout) => {
                    if Instant::now() >= deadline {
                        return Err(AppError::Timeout);
                    }
                }
                // 发送端被丢掉 = 界面对着这条请求崩了。报 `Busy` 而不是空成功。
                Err(RecvTimeoutError::Disconnected) => return Err(AppError::Busy),
            }
        }
    }
}

impl AppControl for DeskApp {
    fn snapshot(&self) -> Result<AppSnapshot, AppError> {
        match self.roundtrip(AppRequest::Snapshot)? {
            AppReply::Snapshot(snapshot) => Ok(snapshot),
            // 请求与回答必须配对：串了就说明队列被别人复用，宁可报错也不返回错数据。
            _ => Err(AppError::Busy),
        }
    }

    fn panel_set(&self, panel: &str, visible: bool) -> Result<AppDelta, AppError> {
        match self.roundtrip(AppRequest::PanelSet {
            panel: panel.to_string(),
            visible,
        })? {
            AppReply::Delta(delta) => Ok(delta),
            AppReply::Error(err) => Err(err),
            _ => Err(AppError::Busy),
        }
    }

    fn layout(&self, op: &LayoutOp) -> Result<AppDelta, AppError> {
        match self.roundtrip(AppRequest::Layout(op.clone()))? {
            AppReply::Delta(delta) => Ok(delta),
            AppReply::Error(err) => Err(err),
            _ => Err(AppError::Busy),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 一条界面线程不在画的 `DeskApp`：`queue` 里的请求永远不会被取走，
    /// 正好用来量「阻塞到取消 / 报 Busy」这两条时间与状态语义。
    fn headless() -> (DeskApp, AppQueue, CancelToken) {
        let queue: AppQueue = Default::default();
        let cancel = CancelToken::new();
        let app = DeskApp::new(egui::Context::default(), queue.clone(), cancel.clone());
        (app, queue, cancel)
    }

    #[test]
    fn a_second_request_while_one_is_still_pending_reports_busy() {
        // 队列非空 → Busy（不排第二条）。
        let (app, queue, _cancel) = headless();
        let (tx, _rx) = sync_channel(1);
        queue.lock().expect("队列锁").push(PendingApp {
            request: AppRequest::Snapshot,
            reply: tx,
        });
        assert!(matches!(app.snapshot(), Err(AppError::Busy)));
        assert_eq!(
            queue.lock().expect("队列锁").len(),
            1,
            "被拒的请求不该进队（否则第二条会排在第一条后面，语义又回去了）"
        );
    }

    #[test]
    fn cancelling_releases_a_blocked_call_within_one_poll() {
        // 验收 4 的时限是 200ms；轮询是 100ms，所以最坏落在 100~200ms 之间。
        let (app, _queue, cancel) = headless();
        let trigger = cancel.clone();
        let canceller = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(30));
            trigger.cancel();
        });
        let started = Instant::now();
        let err = app.snapshot().expect_err("取消后不该返回快照");
        let waited = started.elapsed();
        assert!(matches!(err, AppError::Cancelled), "应报 Cancelled，实得 {err:?}");
        assert!(
            waited <= Duration::from_millis(200),
            "取消到返回花了 {waited:?}，超过 200ms"
        );
        canceller.join().expect("取消线程");
    }

    #[test]
    fn a_dropped_reply_channel_reports_busy_instead_of_pretending_success() {
        // 界面线程拿走请求后崩了 = 回信通道被 drop。报 Busy，不返回空快照。
        let (app, queue, _cancel) = headless();
        let drain = queue.clone();
        let grabber = std::thread::spawn(move || {
            for _ in 0..400 {
                {
                    // 取走后立刻丢：`PendingApp` 一 drop，它的 `tx` 跟着 drop。
                    let mut pending = drain.lock().expect("队列锁");
                    if !pending.is_empty() {
                        pending.remove(0);
                        return;
                    }
                }
                std::thread::sleep(Duration::from_millis(5));
            }
        });
        assert!(matches!(app.snapshot(), Err(AppError::Busy)));
        grabber.join().expect("取队列线程");
    }
}
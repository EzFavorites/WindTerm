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

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{sync_channel, RecvTimeoutError, SyncSender};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use eframe::egui;
use term_core::app_state::AppSnapshot;
use term_script::{AppControl, AppDelta, AppError, CancelToken, CatalogEntry, CommandAck, LayoutOp};

/// 一次应用控制面请求，一个变体对应端口上的一个方法。
pub enum AppRequest {
    /// 读一份快照。
    Snapshot,
    /// 置位一块面板。
    PanelSet { panel: String, visible: bool },
    /// 改停靠位（reset / dock / redock）。
    Layout(LayoutOp),
    /// 按命令 ID 派发一条应用命令。必须走界面帧：命令的效果在 reducer 里，
    /// 脚本线程不能自己执行，否则又是一个「回执说成功、屏幕没动」的假派发。
    Command { id: String },
}

/// 界面线程的回答。
pub enum AppReply {
    Snapshot(AppSnapshot),
    Delta(AppDelta),
    /// `app_command` 的回执：命令 ID 是否被识别并真的派发。
    Command(CommandAck),
    /// 界面侧拒绝（目前只有「面板名不认识」）。带完整的 `AppError` 而不是一句
    /// 字符串：端口层要能分清「名字不存在」与「参数形状不对」，压成字符串就只能
    /// 一律报 `BadArgument`，脚本就没法据此换一条路。
    Error(AppError),
}

/// 待答复的一次请求：请求 + 回信通道 + 所属运行的取消令牌。
pub struct PendingApp {
    /// 队列内唯一序号：脚本被取消时据此**只摘自己那一条**，不误删别的脚本。
    pub seq: u64,
    /// 所属运行是否已取消。界面 drain 前先看一眼，取消的请求直接丢。
    pub cancel: CancelToken,
    pub request: AppRequest,
    pub reply: SyncSender<AppReply>,
}

impl PendingApp {
    /// 界面取出来之后先问：这条请求还有人等回信吗？
    pub fn is_stale(&self) -> bool {
        self.cancel.is_cancelled()
    }
}

/// 界面与脚本线程之间的请求队列。界面每帧 `remove(0)` 一条。
pub type AppQueue = Arc<Mutex<Vec<PendingApp>>>;

/// 每条请求一个号。用全局原子而不是队列长度：两条运行交错时长度会撞号。
static NEXT_SEQ: AtomicU64 = AtomicU64::new(1);
fn next_seq() -> u64 {
    NEXT_SEQ.fetch_add(1, Ordering::Relaxed)
}

/// 取消后把**自己的**那条请求从队列里摘掉。
///
/// 不做这一步的话，脚本线程已经报 `Cancelled` 收场，请求却还躺在队列里，
/// 下一帧界面照旧执行 `panel_set` / `layout`——用户点了停止，面板却自己动了。
fn drop_own(queue: &AppQueue, seq: u64) {
    let mut guard = match queue.lock() {
        Ok(guard) => guard,
        Err(poisoned) => poisoned.into_inner(),
    };
    guard.retain(|pending| pending.seq != seq);
}

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
        let seq = next_seq();
        let (tx, rx) = sync_channel(1);
        match self.queue.lock() {
            // 队列非空 = 上一帧那条还没落地。**不排第二条**：
            // 置位是幂等的、读是本帧的，两条一起等在队里只会让「谁先谁后」
            // 变得不可解释；脚本拿 `Busy` 自己决定要不要重试，比让它盲等 2 秒好。
            Ok(queue) if !queue.is_empty() => return Err(AppError::Busy),
            Ok(mut queue) => queue.push(PendingApp {
                seq,
                cancel: self.cancel.clone(),
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
                drop_own(&self.queue, seq);
                return Err(AppError::Cancelled);
            }
            match rx.recv_timeout(Duration::from_millis(100)) {
                Ok(reply) => return Ok(reply),
                Err(RecvTimeoutError::Timeout) => {
                    if Instant::now() >= deadline {
                        drop_own(&self.queue, seq);
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

    fn command(&self, id: &str) -> Result<CommandAck, AppError> {
        // 真派发：请求进界面帧，由 `drain_app_requests` 经 `Action::parse`
        // 后调用 `dispatch_action`。脚本层不抄 ID 表（4e 门禁），也不在
        // 这里自己判「成功」——判据只能是界面真的收到了那条命令。
        match self.roundtrip(AppRequest::Command { id: id.to_string() })? {
            AppReply::Command(ack) => Ok(ack),
            _ => Err(AppError::Busy),
        }
    }

    fn catalog(&self) -> Result<Vec<CatalogEntry>, AppError> {
        Ok(crate::script_host::desk_app_catalog())
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
        let (app, queue, cancel) = headless();
        let (tx, _rx) = sync_channel(1);
        queue.lock().expect("队列锁").push(PendingApp {
            seq: next_seq(),
            cancel: cancel.clone(),
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

    /// `app_command` 必须把命令 ID 送进界面帧，不能再只在脚本侧判「能不能 parse」。
    #[test]
    fn app_command_goes_through_the_ui_request_queue() {
        let (app, queue, _cancel) = headless();
        let drain = queue.clone();
        let ui = std::thread::spawn(move || {
            for _ in 0..400 {
                let pending = {
                    let mut q = drain.lock().expect("队列锁");
                    if q.is_empty() { None } else { Some(q.remove(0)) }
                };
                if let Some(pending) = pending {
                    match pending.request {
                        AppRequest::Command { id } => {
                            assert_eq!(id, "Terminal.ClearScreen");
                            let _ = pending.reply.send(AppReply::Command(
                                CommandAck::dispatched(true),
                            ));
                        }
                        _other => panic!("期望 Command 请求"),
                    }
                    return;
                }
                std::thread::sleep(Duration::from_millis(2));
            }
            panic!("界面线程没取到请求");
        });
        let ack = app.command("Terminal.ClearScreen").expect("回执");
        assert!(ack.dispatched, "界面确认已派发");
        ui.join().expect("界面线程");
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

    #[test]
    fn cancelling_takes_the_queued_request_out_before_the_ui_can_drain_it() {
        // 「停止」之后脚本已经报 Cancelled，但请求若还躺在队列里，下一帧
        // 界面照旧会 panel_set / layout。这里盯住「自己的那条被摘掉」。
        let (app, queue, cancel) = headless();
        let worker = std::thread::spawn(move || app.snapshot());
        let deadline = Instant::now() + Duration::from_secs(2);
        while queue.lock().expect("队列锁").is_empty() {
            assert!(Instant::now() < deadline, "请求没能进队");
            std::thread::sleep(Duration::from_millis(5));
        }
        cancel.cancel();
        assert!(matches!(worker.join().expect("脚本线程"), Err(AppError::Cancelled)));
        assert!(
            queue.lock().expect("队列锁").is_empty(),
            "取消的请求不该留到下一帧被执行"
        );
    }

    #[test]
    fn a_pending_request_reports_staleness_from_its_cancel_token() {
        let cancel = CancelToken::new();
        let (tx, _rx) = sync_channel(1);
        let pending = PendingApp {
            seq: next_seq(),
            cancel: cancel.clone(),
            request: AppRequest::Snapshot,
            reply: tx,
        };
        assert!(!pending.is_stale());
        cancel.cancel();
        assert!(pending.is_stale(), "界面 drain 前据此丢弃");
    }
}
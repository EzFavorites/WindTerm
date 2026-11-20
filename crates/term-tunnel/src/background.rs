//! 后台起隧道：与 `term_ssh::connect_in_background` 同一个形状。
//!
//! 为什么需要：`Tunnel::start()` 是同步的，最长等 `START_TIMEOUT`（15 秒）——
//! 握手、认证、逐个建监听都在里面。桌面端一帧都不能挂，所以要「起了线程、
//! 立刻返回」。线程与队列都收在本 crate，调用方只轮询一个不透明句柄
//! （S2 起的规矩：投影层不接触线程与 `mpsc`，见 verify 第 5 项）。

use std::sync::mpsc::{Receiver, TryRecvError};

use term_ssh::SshOptions;

use crate::{Forward, Tunnel, TunnelError};

/// 一次进行中的「起隧道」。
pub struct TunnelHandle {
    rx: Receiver<Result<Tunnel, TunnelError>>,
}

/// 在后台线程里 `Tunnel::start(...)`，立刻返回句柄。
///
/// `on_wake` 在结果就绪、已塞进队列之后被调用一次：调用方用它叫醒事件循环
/// （egui 是 `ctx.request_repaint()`）。本 crate 不认识 UI，所以唤醒只能是回调。
pub fn start_in_background(
    opts: SshOptions,
    forwards: Vec<Forward>,
    on_wake: impl FnOnce() + Send + 'static,
) -> TunnelHandle {
    let (tx, rx) = std::sync::mpsc::channel();
    let host = opts.host.clone();
    std::thread::Builder::new()
        .name(format!("term-tunnel-start-{host}"))
        .spawn(move || {
            let result = Tunnel::start(opts, forwards);
            let _ = tx.send(result);
            on_wake();
        })
        .expect("无法启动隧道线程");
    TunnelHandle { rx }
}

impl TunnelHandle {
    /// 结果到了没有？`None` = 还在建。
    ///
    /// 队列归本 crate 所有，所以「线程死了没留话」也在这里翻译成
    /// `TunnelError::Local`——调用方不该需要知道 `TryRecvError` 是什么。
    pub fn try_result(&mut self) -> Option<Result<Tunnel, TunnelError>> {
        match self.rx.try_recv() {
            Ok(result) => Some(result),
            Err(TryRecvError::Empty) => None,
            Err(TryRecvError::Disconnected) => Some(Err(TunnelError::Local(
                "隧道线程意外退出（没有返回任何结果）".to_string(),
            ))),
        }
    }
}

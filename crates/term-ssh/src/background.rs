//! 后台连接：线程与结果队列都收在本 crate，调用方只轮询一个不透明句柄。
//!
//! 为什么要有这一层：`connect()` 是同步的（阻塞到握手完成，最多等超时），
//! 而调用它的事件循环（egui 桌面端）一帧都不能挂。于是需要一个「起了线程、
//! 立刻返回」的形状。**注意这条缝的形状是刻意的**：线程和 `mpsc` 都不许
//! 漏到调用方那里——桌面端的边界就是「只拿不透明句柄」（`SupervisorHandle`
//! 是先例），如果这里交出一个裸 `Receiver`，那个边界就破了（verify 第 5 项）。

use crate::{connect, SshChannel, SshError, SshOptions};
use std::sync::mpsc::{Receiver, TryRecvError};

/// 一次进行中的后台连接。
pub struct ConnectHandle {
    rx: Receiver<Result<SshChannel, SshError>>,
}

/// 在后台线程里做一次 `connect()`，立刻返回句柄。
///
/// `on_wake` 在结果就绪、队列已塞好之后被调用一次——调用方用它叫醒自己的
/// 事件循环（egui 是 `ctx.request_repaint()`；空闲不重绘的策略下，不叫醒
/// 结果就永远躺在队列里）。本 crate 不认识 UI，所以唤醒只能是一个回调，
/// 这是「异步不外泄」与「结果不等人」之间唯一的缝。
///
/// 线程起不来直接 panic：和 `connect()` 内部一样，这是「本机资源枯竭」，
/// 不是一条能恢复的连接错误。
pub fn connect_in_background(
    opts: SshOptions,
    on_wake: impl FnOnce() + Send + 'static,
) -> ConnectHandle {
    let (tx, rx) = std::sync::mpsc::channel();
    let name = format!("term-ssh-connect-{}", opts.host);
    std::thread::Builder::new()
        .name(name)
        .spawn(move || {
            let result = connect(opts);
            let _ = tx.send(result);
            on_wake();
        })
        .expect("无法启动 SSH 连接线程");
    ConnectHandle { rx }
}

impl ConnectHandle {
    /// 结果到了没有？`None` = 还在连。
    ///
    /// 队列归本 crate 所有，所以「线程死了没留话」也在这里翻译成
    /// `SshError::Local`——调用方不该需要知道 `TryRecvError` 是什么。
    pub fn try_result(&mut self) -> Option<Result<SshChannel, SshError>> {
        match self.rx.try_recv() {
            Ok(result) => Some(result),
            Err(TryRecvError::Empty) => None,
            Err(TryRecvError::Disconnected) => Some(Err(SshError::Local(
                "连接线程意外退出（没有返回任何结果）".to_string(),
            ))),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::AuthMethod;

    /// 后台形状必须与同步形状给同样的错：连一个没人监听的本地端口，
    /// 最终拿到 `SshError::Connect`（而不是永远 None 或线程悄悄死掉）。
    #[test]
    fn a_background_connect_reports_the_same_error_as_the_sync_one() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind");
        let port = listener.local_addr().expect("addr").port();
        drop(listener);

        let mut opts = SshOptions::new(
            "127.0.0.1",
            "user",
            AuthMethod::Password {
                password: String::new(),
            },
        );
        opts.port = port;
        let mut handle = connect_in_background(opts, || {});
        // 轮询到结果为止（最多几秒：连接被拒是立刻的）。
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(15);
        let result = loop {
            match handle.try_result() {
                Some(result) => break result,
                None if std::time::Instant::now() < deadline => {}
                None => panic!("15 秒内没有拿到任何结果"),
            }
        };
        assert!(matches!(result, Err(SshError::Connect(_))), "实际是 {result:?}");
    }
}

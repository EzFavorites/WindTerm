//! 原始字节 tee 的**接收端**：把「通道」这件事关在 `ui/` 之外。
//!
//! 为什么这个文件在 `apps/desktop/src/` 而不在 `src/ui/`：
//!
//! 1. **`ui/` 不得持有 OS 资源**。`scripts/verify.sh` 第 5 步直接在
//!    `apps/desktop/src/ui/**` 里禁了 `mpsc::Receiver` 这类字样——投影层只该有
//!    「画」与「发动作」。② 那一笔把 `Receiver<Vec<u8>>` 直接塞进了
//!    `ui/terminal/bin_spec.rs::RawLog`，门禁因此红过一次；这里按同目录
//!    `script_run.rs` / `xfer_run.rs` 的先例，把资源留在 app 层，界面拿到的是
//!    一个**不透明句柄**。
//! 2. **搬字节是有时序的副作用**，不是排版。`drain` 每帧把已经到达的块全搬走，
//!    「什么时候搬、搬多少」是会话生命周期的一部分；`ui/` 只该回答「这一帧
//!    要不要重画」。
//!
//! 与 `session.rs::attach_tap` 的分工：那边向控制面登记生产端（`SessionPool::
//! attach_tap`），这边只是收下来的人。**通道本身无界**（见 `attach_tap` 的注释：
//! 监管者线程 `send` 绝不许阻塞），因此「UI 不搬就攒着」——所以 `drain` 必须
//! 每帧调，调用点只有 `Tab::poll` 一处。

use std::sync::mpsc::Receiver;

/// 一条字节 tee 的消费端。丢弃即解绑（生产端随后 `send` 会失败，但那是
/// 控制面自己的事：它在标签关闭时也会 `detach_tap`）。
pub(crate) struct HexTap {
    rx: Receiver<Vec<u8>>,
}

impl HexTap {
    /// 包住 `SessionPool::attach_tap` 给出的接收端。
    pub(crate) fn new(rx: Receiver<Vec<u8>>) -> Self {
        Self { rx }
    }

    /// 把**已经到达**的块全部交给 `sink`，返回搬了多少字节。
    ///
    /// `try_recv` 取到空为止而不是「每帧搬一块」：后者在洪水输出时永远追不上
    /// 生产端，环里看到的历史会持续落后于屏幕。
    ///
    /// 用回调而不是「返回 `Vec<Vec<u8>>`」：hex 环这一路只要求「就地塞进环」，
    /// 攒一个中间 `Vec` 只是白搬一次内存——环是 1 MiB 级的东西，别在它前面
    /// 再放一个同样大的中转站。那条路要把同一批字节喂第二个消费者，
    /// 见下面的 `drain_owned`。
    pub(crate) fn drain<F>(&self, mut sink: F) -> usize
    where
        F: FnMut(&[u8]),
    {
        let mut moved = 0usize;
        while let Ok(chunk) = self.rx.try_recv() {
            moved += chunk.len();
            sink(&chunk);
        }
        moved
    }

    /// 把**已经到达**的字节收成一份 owned 值交回去。
    ///
    /// 与 `drain` 的分工：那条路只喂 hex 环，回调就地搬完就完；这条路要把
    /// 同一批字节再喂 `Tab` 的 tmux 状态机，而 `&mut self.raw` 与 `&mut self.tmux`
    /// 不能同时塞进一个闭包。字节在这里收成一份 `Vec`，借用边界就留在 app 层，
    /// `ui/` 里的 `RawLog::drain_take` 只需「拿一份 → 推一次环 → 原样交回」。
    /// 仍然是 `try_recv` 取空为止、不阻塞；空通道返回零分配的 `Vec::new()`。
    pub(crate) fn drain_owned(&self) -> Vec<u8> {
        let mut taken = Vec::new();
        while let Ok(chunk) = self.rx.try_recv() {
            taken.extend_from_slice(&chunk);
        }
        taken
    }
}

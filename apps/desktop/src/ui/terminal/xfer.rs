//! 传输进度状态行 + 投影层接线（弹层确认 → 驱动 → 画一行）。
//!
//! 画什么：终端底部的一行——标题 / 进度明细 / 失败原因 / 一个取消按钮。
//! 不画什么：进度条像素、独立面板、新的布局。这一行**只在传输时出现**：
//! 没有句柄、没有弹层时本模块不画任何像素，空闲画面的布局与改动前逐字节一致
//! （整窗基线就是这么守的）。
//!
//! 状态与措辞（哪句话该说什么、字节怎么念、单测）在 `xfer_spec.rs`：
//! 450 行上限要求「数据面」与「绘制面」分开，本文件**只画与接线**。
//!
//! # 分工
//!
//! - 驱动（协议、线程、落盘）在 `apps/desktop/src/xfer_run.rs`，**本模块不碰**；
//! - 菜单点的口令（`Effect::Xfer`）在 `ui/effects.rs` 只转成「开弹层」；
//! - 弹层在 `ui/dialogs/xfer.rs`，只收一段路径、不读盘；
//! - 这里负责三件事：把 `XferCmd` 落成一次 `start`、每帧把 `Progress` 映射成
//!   [`XferView`]、画那一行并把按钮的意图回传给句柄。

use super::xfer_spec::{file_name, human_bytes, rate_suffix, run_protocol, XferUi, XferView};
use crate::ui::dialogs::xfer::{protocol_label, XferDirection};
use crate::ui::*;
use crate::xfer_run::{XferHandle, XferKind};
use std::path::PathBuf;
use term_app::action::XferProtocol;

/// 画一行传输状态。返回这一帧的交互意图。
///
/// 取消**没有在这里发生**：本函数不驱动传输、不改任何状态，只把「用户想取消」
/// 交回调用方。按钮文案里没有绑 `Esc`：`Esc` 在确认框 / 面板里有主，
/// 全局抢一个会在两个模态同时开的时候打架——取消只走按钮。
///
/// 失败时没有取消按钮：那次传输已经结束了，能取消的东西不存在；改给一个
/// 「关闭」，让用户把这一行收掉。
fn draw(ui: &mut egui::Ui, view: &XferView<'_>) -> XferUi {
    if view.is_idle() {
        return XferUi::None;
    }
    let mut action = XferUi::None;
    ui.horizontal_centered(|ui| {
        let failed = view.error.is_some();
        if view.running && !view.canceled {
            ui.spinner();
        }
        // 等宽：字节计数每帧都在变，比例字体下整行会左右抖。
        let mut text = egui::RichText::new(view.status()).monospace();
        if failed {
            text = text.color(ui.visuals().error_fg_color);
        } else if view.canceled {
            text = text.weak();
        }
        ui.add(egui::Label::new(text));
        if view.running && !view.canceled {
            if ui.small_button("Cancel").clicked() {
                action = XferUi::Cancel;
            }
        } else if ui.small_button("Close").clicked() {
            action = XferUi::Dismiss;
        }
    });
    action
}

/// 一次在跑的传输：句柄 + 起点信息。
///
/// 句柄的 `Drop` 会取消传输，所以它**绝不能**存在会被每帧重建的地方。
/// 方向与目标是给标题用的：`Progress` 里没有文件名（对端协议不一定给），
/// 而「发的是哪个文件、收到哪个目录」只有 `start` 那一刻知道。
pub(in crate::ui) struct XferJob {
    pub handle: XferHandle,
    pub kind: XferKind,
    /// 存投影层的协议枚举（不是驱动层那个）：显示名由 `protocol_label` 给，
    /// 驱动层的枚举只在 `start` 那一刻用一次（`run_protocol`）。
    pub protocol: XferProtocol,
    /// `Send` 是本地文件路径，`Receive` 是目标目录（都原样存字符串，只用来显示）。
    pub target: String,
    /// 接收完成后是否打开目标目录（来自接收弹层的勾选）。
    pub open_dir: bool,
    /// 已经打开过一次——`done` 会持续多帧，没有这一位会连开好几个窗口。
    pub opened: bool,
    /// 速率估计（XFER-05）。跟着这一次传输走：换一次传输就该重新建基线，
    /// 所以它放在这里而不是 `TerminalWindow` 上。
    pub meter: term_xfer::RateMeter,
}

impl TerminalWindow {
    /// 每帧一次：把 `Progress` 映射成一行，画在状态栏正上方。
    ///
    /// `None` 时整段短路——空闲路径一个像素都不变。
    pub(in crate::ui) fn draw_xfer_status(&mut self, ctx: &egui::Context) {
        let action = self.xfer_action(ctx);
        match action {
            XferUi::None => {}
            XferUi::Cancel => {
                if let Some(job) = self.xfer.as_ref() {
                    job.handle.cancel();
                }
                self.ctx.request_repaint();
            }
            // 结束时丢句柄是安全的：线程已经收尾，`Drop` 里的 `cancel()`
            // 对已结束的传输是空操作。
            XferUi::Dismiss => self.xfer = None,
        }
    }

    /// 画一行并返回意图（不改传输状态，只喂一次速率采样）。
    fn xfer_action(&mut self, ctx: &egui::Context) -> XferUi {
        let Some(job) = self.xfer.as_mut() else {
            return XferUi::None;
        };
        let progress = job.handle.progress();
        let running = !progress.finished && progress.error.is_none();
        // 取消是驱动约定的**精确**值，引用常量而不是重抄字面量——两边各写一份
        // 的话改文案时这里会静默失效（取消被显示成 Failed，而不是编译报错）。
        // 协议失败一律以「…阶段失败：」开头，两者不会混。
        let canceled = !running && progress.error.as_deref() == Some(crate::xfer_run::CANCELLED);
        let error = if canceled || running {
            None
        } else {
            progress.error.clone()
        };
        let done = !running && !canceled && error.is_none();
        // X/Y/ZMODEM 文档：接收后可选自动打开下载目录。只在「刚完成」那一帧
        // 拉起系统默认程序，`done` 持续多帧也不能重复弹窗；打开失败只记一句
        // 提示，不让一次成功的传输变成 UI 报错。
        if done && job.open_dir && !job.opened {
            job.opened = true;
            if let Err(e) = crate::open_with::open_path(std::path::Path::new(&job.target)) {
                self.log_note = Some(format!("打开下载目录失败：{}", e.message()));
            }
        }
        // 只在跑的时候喂采样：结束后再喂，`done` 那一帧的字节数与上一帧相同，
        // 会算出一个 0 B/s 并覆盖掉最后一个真实读数。
        if running {
            let now_ms = (ctx.input(|i| i.time) * 1000.0) as u64;
            job.meter.sample(progress.bytes, now_ms);
        }

        let proto = protocol_label(job.protocol);
        let title = match job.kind {
            XferKind::Send => {
                let name = file_name(&job.target);
                if running {
                    format!("Sending {name}")
                } else if done {
                    format!("Sent {name}")
                } else {
                    // 失败 / 取消：标题回到中性的文件名，前缀由状态行给。
                    name
                }
            }
            XferKind::Receive => {
                if done {
                    format!("Saved to {}", job.target)
                } else {
                    format!("Receiving into {}", job.target)
                }
            }
        };
        let detail = if running || done {
            // XFER-05：速率与剩余时间。时间取自 egui 自己的单调时钟（`ctx` 的
            // `time` 从启动算起），所以 `ui/` 不必去读系统时钟。
            let suffix = rate_suffix(
                job.meter.bytes_per_sec_rounded(),
                progress.total.and_then(|total| job.meter.eta_secs(total, progress.bytes)),
            );
            match progress.total {
                // YMODEM 与发送侧给了总长：写「已走 / 总长」。
                Some(total) => format!(
                    "{} / {}{suffix} ({proto})",
                    human_bytes(progress.bytes),
                    human_bytes(total)
                ),
                // XMODEM 恒为 `None`：只写已走字节，**不**补一个「/ ?」，
                // 更不换算百分比（分母都没有，百分比是编的）。
                None if progress.bytes > 0 => {
                    format!("{}{suffix} ({proto})", human_bytes(progress.bytes))
                }
                // 一个字节都还没动：阶段名（「待开始」/「握手」）才是有效信息。
                None => progress.phase.clone(),
            }
        } else if progress.bytes > 0 {
            // 失败 / 取消：报一下已经走了多少，原因在 `error` 那一路。
            format!("{} ({proto})", human_bytes(progress.bytes))
        } else {
            String::new()
        };

        let view = XferView {
            title: &title,
            detail: &detail,
            error: error.as_deref(),
            running,
            canceled,
        };
        let mut action = XferUi::None;
        // 与 `find_bar` 同一条位置逻辑：声明在 `CentralPanel` 之后，覆盖终端
        // 底部一行，**不切分**中央区（终端视口高度不变）。只在有句柄时创建。
        egui::TopBottomPanel::bottom("xfer_bar")
            .frame(shell::bar_frame())
            .show_separator_line(false)
            .show(ctx, |ui| {
                action = draw(ui, &view);
            });
        action
    }

    /// 在跑的时候按 120ms 重绘：驱动线程不会通知 egui，没有这一手，
    /// 进度会停在第一帧。**结束之后不再请求**，交回空闲 0 CPU 的约定。
    pub(in crate::ui) fn request_xfer_repaint(&self, ctx: &egui::Context) {
        let Some(job) = self.xfer.as_ref() else {
            return;
        };
        let progress = job.handle.progress();
        if !progress.finished && progress.error.is_none() {
            ctx.request_repaint_after(std::time::Duration::from_millis(120));
        }
    }

    /// 弹层确认之后：真的开始一次传输。失败把原因写回弹层（**不关**），
    /// 用户能就地改路径再来一次——关掉再重开会丢掉刚敲的那段。
    pub(in crate::ui) fn start_xfer(&mut self, path: PathBuf) {
        let Some(prompt) = self.xfer_prompt.as_ref() else {
            return;
        };
        let kind = match prompt.direction {
            XferDirection::Send => XferKind::Send,
            XferDirection::Receive => XferKind::Receive,
        };
        let protocol = prompt.protocol;
        // 接收弹层选的同名冲突策略原样交给驱动；发送侧不用，传默认档即可。
        let policy = prompt.policy;
        // 一次只跑一个：起第二个会把旧句柄丢掉，而它的 `Drop` 就是取消——
        // 用户会看到上一次传输莫名其妙停了。挡住，让他自己决定取消还是等。
        if let Some(job) = self.xfer.as_ref() {
            let progress = job.handle.progress();
            if !progress.finished && progress.error.is_none() {
                self.xfer_prompt_error("已有一次传输在跑：先取消，或等它结束".to_string());
                return;
            }
        }
        let Some(session) = self.tabs.get(self.active_slot()).and_then(|t| t.session()) else {
            self.xfer_prompt_error("这个标签没有活着的会话".to_string());
            return;
        };
        match XferHandle::start_with_policy(
            self.pool.clone(),
            session,
            kind,
            run_protocol(protocol),
            path.clone(),
            policy,
        ) {
            Ok(handle) => {
                self.xfer = Some(XferJob {
                    handle,
                    kind,
                    protocol,
                    target: path.display().to_string(),
                    open_dir: self.xfer_prompt.as_ref().is_some_and(|p| p.open_after) && kind == XferKind::Receive,
                    opened: false,
                    meter: term_xfer::RateMeter::new(),
                });
                self.xfer_prompt = None;
                self.ctx.request_repaint();
            }
            Err(message) => self.xfer_prompt_error(message),
        }
    }

    /// 把失败原因挂回弹层。弹层已经关掉（理论上不会）就只留一行状态提示。
    fn xfer_prompt_error(&mut self, message: String) {
        match self.xfer_prompt.as_mut() {
            Some(prompt) => prompt.error = Some(message),
            None => self.log_note = Some(message),
        }
    }
}

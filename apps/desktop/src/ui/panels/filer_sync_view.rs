//! 目录同步的绘制：**审阅页**与**执行进度**。状态机在 [`filer_sync.rs`]。
//!
//! 与 `filer_view.rs` 同一条分工：那个文件画面板，这个文件画同步的窗。分成两份是因为
//! 同步窗有自己的一整套内容（策略三选一、待删项清单、逐项进度），塞回面板绘制会让
//! 「面板怎么画」和「同步怎么画」在一个文件里互相挤。
//!
//! # 审阅页为什么必须把「要删的东西」单列出来
//!
//! 同步的默认策略是 `Keep`（一项都不删），但用户可以把它切到「删本地」或「删远端」。
//! 一旦切过去，`plan()` 出来的删除项就是**真的会消失的文件**。把它们混在那句
//! 「3 项待删远端」的计数里，等于让用户在没有看到具体路径的情况下同意删除。
//!
//! 所以：有删除时**逐条列出路径**（最多 [`MAX_LISTED`] 条，多的用一行计数收尾），
//! 用红字，并且把确认按钮的文案改成「同步并删除 N 项」。文案改掉是最后一道：
//! 一个写着「开始同步」的按钮在删除时也该叫删除。
//!
//! # 进度为什么只有一行
//!
//! 执行阶段同一时刻只飞一步（见 `filer_sync.rs` 的模块头），所以「第几项 / 共几项」
//! 加上一步的名字就是全部信息。不做逐项列表：那个列表要么每帧重排（贵），要么
//! 只在结束时出现（那就不叫进度）。

use super::sync;
use super::sync as filer_sync;
use crate::ui::kit::window::{DialogId, DialogWindow};
use crate::ui::*;
use eframe::egui;
use term_xfer::sync::{DeletePolicy, SyncOp};

/// 待删路径最多单列几条。再多就用「还有 N 项」收尾——一屏放不下的清单
/// 和一句计数一样没用，但前者会让窗口长胖到盖住整个界面。
const MAX_LISTED: usize = 8;

const DANGER: egui::Color32 = egui::Color32::from_rgb(0xE0, 0x6C, 0x6C);

/// 审阅页里能点的事。先收集、后执行——绘制期间已经借了 `self.filer`。
enum Action {
    None,
    Cancel,
    Policy(DeletePolicy),
    Confirm,
}

impl TerminalWindow {
    /// 进入执行阶段：把审阅出来的计划交给执行器。**这是唯一会改文件的入口。**
    ///
    /// 放在这里（而不是状态机的 `poll`）是因为它要拿 `handle`——审阅阶段本身
    /// 不需要句柄，只有这一步要。
    fn start_sync_exec(&mut self) {
        let Some(sync::SyncState::Review(review)) = self.filer.sync.take() else {
            return;
        };
        if self.filer.handle.is_none() {
            self.filer.error = Some("文件面板还没连上，同步没有开始".to_string());
            return;
        }
        let (local, remote) = review.roots();
        let items = review.items().to_vec();
        let count = review.summary().mutations();
        if count == 0 {
            // 一项都不用动：不必进执行阶段，直接说句实话。
            self.filer.progress = Some(filer_sync::summary_line(&review.summary()));
            return;
        }
        self.filer.progress = Some(format!("同步中：0 / {count}"));
        self.filer.error = None;
        self.filer.sync = Some(sync::SyncState::Running(Box::new(
            filer_sync::SyncExec::new(items, std::path::PathBuf::from(local), remote),
        )));
        // 执行靠下一帧的 `poll_sync` 推进；这里显式叫醒它，否则空闲不重绘会让
        // 第一步等到用户随手动一下鼠标才发出去。
        self.ctx.request_repaint();
    }

    /// 画同步的窗。三种状态各画一种内容：扫描中 / 待审阅 / 执行中。
    pub(in crate::ui) fn draw_sync_prompt(&mut self, ctx: &egui::Context) {
        match &self.filer.sync {
            None => {}
            Some(sync::SyncState::Scanning(scan)) => {
                let body = format!(
                    "{} → {}\n正在扫描两侧清单…",
                    scan.local_root().display(),
                    scan.remote_root()
                );
                let mut cancel = false;
                let open = DialogWindow::new(DialogId::FilerSyncScan, "Sync folders")
                .show(ctx, |ui| {
                    ui.label(body.clone());
                    ui.add_space(8.0);
                    if ui.button("Cancel").clicked() {
                        cancel = true;
                    }
                });
                // 右上角 × 与 Cancel 同一条路：都是「这次不做了」。
                cancel |= !open;
                if cancel {
                    // 取消扫描 = 丢弃结果。本地线程的那一份没人收就自己结束了；
                    // 远端清单回来时状态已经不是 `Scanning`，会自然被忽略。
                    self.filer.sync = None;
                    self.filer.progress = None;
                }
            }
            Some(sync::SyncState::Review(_)) => self.draw_sync_review(ctx),
            Some(sync::SyncState::Running(exec)) => {
                let p = exec.progress();
                let done = p.done + p.failed + p.skipped;
                let mut line = format!(
                    "同步中：{done} / {}（成功 {} / 失败 {}）",
                    p.todo, p.done, p.failed
                );
                if let Some(err) = exec.last_error() {
                    line.push('\n');
                    line.push_str(err);
                }
                let stopping = exec.is_stopping();
                let mut stop = false;
                DialogWindow::new(DialogId::FilerSyncRun, "Sync folders")
                .show(ctx, |ui| {
                    ui.label(line.clone());
                    ui.add_space(8.0);
                    // 停止 = 不再发下一步，已做的保留（不回滚）。按钮在手的那一步
                    // 收尾前先写「停止中…」——让用户知道这一下已经收到了。
                    if ui.button(if stopping { "Stopping…" } else { "Stop" }).clicked() {
                        stop = true;
                    }
                });
                if stop && !stopping {
                    self.filer.request_sync_stop();
                    self.ctx.request_repaint();
                }
            }
        }
    }

    /// 审阅页本体。
    fn draw_sync_review(&mut self, ctx: &egui::Context) {
        let Some(sync::SyncState::Review(review)) = &self.filer.sync else {
            return;
        };
        let (local_root, remote_root) = review.roots();
        let summary = review.summary();
        let policy = review.policy();
        // 待删清单：按策略挑出来那一种操作的路径。`Keep` 时为空。
        let doomed: Vec<String> = review
            .items()
            .iter()
            .filter(|i| match policy {
                DeletePolicy::Keep => false,
                DeletePolicy::DeleteLocal => i.op == SyncOp::DeleteLocal,
                DeletePolicy::DeleteRemote => i.op == SyncOp::DeleteRemote,
            })
            .map(|i| i.entry.path.clone())
            .collect();
        let mut action = Action::None;

        DialogWindow::new(DialogId::FilerSyncReview, "Sync folders")
        .show(ctx, |ui| {
            // 两侧根：一行一条，路径可能很长，用 weak 色压住不让它抢戏。
            ui.label(egui::RichText::new(format!("{local_root} → {remote_root}")).weak());
            ui.add_space(6.0);
            ui.label(filer_sync::summary_line(&summary));

            if summary.has_deletes() {
                ui.add_space(4.0);
                ui.colored_label(
                    DANGER,
                    "⚠ 这一趟会删除文件。两边都改过时按修改时间取新的，被覆盖的那一份不会留副本。",
                );
            }

            ui.add_space(10.0);
            ui.label("Delete policy");
            // 三选一。默认 `Keep`，且**不记住上次选择**：删除策略按趟决定，
            // 记住它等于让上一次的胆量变成这一次的默认值。
            ui.horizontal(|ui| {
                for (value, label, hint) in [
                    (DeletePolicy::Keep, "Keep both", "一项都不删，只补齐缺的"),
                    (
                        DeletePolicy::DeleteRemote,
                        "Delete remote extras",
                        "远端有、本地没有的删掉",
                    ),
                    (
                        DeletePolicy::DeleteLocal,
                        "Delete local extras",
                        "本地有、远端没有的删掉",
                    ),
                ] {
                    if ui
                        .radio(policy == value, label)
                        .on_hover_text(hint)
                        .clicked()
                    {
                        action = Action::Policy(value);
                    }
                }
            });

            if !doomed.is_empty() {
                ui.add_space(8.0);
                ui.colored_label(DANGER, format!("将删除 {} 项：", doomed.len()));
                for path in doomed.iter().take(MAX_LISTED) {
                    ui.colored_label(DANGER, format!("  {path}"));
                }
                if doomed.len() > MAX_LISTED {
                    ui.colored_label(
                        DANGER,
                        format!("  …还有 {} 项", doomed.len() - MAX_LISTED),
                    );
                }
            }

            ui.add_space(12.0);
            ui.horizontal(|ui| {
                if ui.button("Cancel").clicked() {
                    action = Action::Cancel;
                }
                // 文案随代价变：一项都不动时说「不用同步」，有删除时说「同步并删除」。
                let label = if summary.mutations() == 0 {
                    "Nothing to do".to_string()
                } else if summary.has_deletes() {
                    format!("Sync and delete {}", doomed.len())
                } else {
                    format!("Sync {} items", summary.mutations())
                };
                let enabled = summary.mutations() > 0;
                if ui
                    .add_enabled(enabled, egui::Button::new(label))
                    .clicked()
                {
                    action = Action::Confirm;
                }
            });
        });

        match action {
            Action::None => {}
            Action::Cancel => {
                self.filer.sync = None;
                self.filer.progress = None;
            }
            Action::Policy(next) => {
                // 只重算计划，**不重扫**：两份清单还在手上（见 `SyncReview::rebuild`）。
                if let Some(sync::SyncState::Review(review)) = &mut self.filer.sync {
                    review.set_policy(next);
                }
                self.ctx.request_repaint();
            }
            Action::Confirm => self.start_sync_exec(),
        }
    }
}

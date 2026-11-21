//! 上传 / 下载：右键入口的判定、请求的发出、队列推进。
//!
//! 三件事放在一起，是因为它们共用**同一条判据**：「这一行现在能不能传」。
//! 判据只要写两遍就一定会漂移，而漂移的表现恰好是最难查的那种——菜单亮着，
//! 点下去什么也没发生（项目铁律）。所以这里给出 [`transfer_blocker`]，
//! `filer_view.rs` 拿它决定菜单项灰不灰、灰的话写哪句话，本文件拿它决定要不要
//! 真的入队。
//!
//! # 为什么落地要等这一帧画完
//!
//! 右键菜单是在**画列表的时候**弹出来的，那一刻 `lines` 与 `self.filer` 都借着。
//! 所以菜单只收集意图（[`XferPick`]），画完再落地——与 `filer_ops.rs` 的
//! `FilerOp` 同一个套路，理由也一样：存**已经算好的路径**而不是行号，等落地时
//! 列表可能已经刷过一轮了。
//!
//! # 目录走树传输
//!
//! 目录行带着 `is_dir` 一路到这里：入队时选 `enqueue_*_tree`（整棵树一行），
//! `next_to_run` 把 `tree` 标记带进 [`XferTask`]，发起时改成
//! `download_tree` / `upload_tree`。后台为树里每个文件发一组 `Progress` + `Done`，
//! 再由树根的 `Done` 收尾；队列那边按前缀把这些事件归到同一行（见 `xfer_queue.rs`）。
//!
//! # 续传与时间戳
//!
//! `resume = true`：两条链路的任意一端都可能已经有半截文件（上次取消 / 断线），
//! 覆盖会白传一遍。`preserve` 取活动会话的
//! `SessionConfig.sftp_preserve_mtime`（New Session ▸ Sftp ▸ Preserve original
//! file modification time，出厂 true）：文件面板不显示 mtime，但「传到对端之后
//! 时间戳还是源端那个」是用户在会话页明确表达的意图，不能由一个写死的 false
//! 替他决定。

use super::ops::join_path;
use crate::ui::panels::xfer_queue::{XferQueue, XferTask};
use crate::ui::*;
use term_xfer::Direction;

/// 右键菜单选中的一次传输。
pub(in crate::ui) enum XferPick {
    /// 远端行 → 本地当前目录。
    Download {
        /// 远端完整路径（事件的 `path` 口径）。目录时是树根。
        remote: String,
        /// 真名：目标文件名要它，不能用屏上那个带 `/` 的标签。
        name: String,
        size: u64,
        /// 目录行走树传输（见模块头）。
        is_dir: bool,
    },
    /// 本地行 → 远端当前目录。
    Upload {
        /// 本地完整路径。目录时是树根。
        local: String,
        name: String,
        size: u64,
        is_dir: bool,
    },
}

/// 这一行现在能不能传；`Err` 是要写在灰项下面的那句话。
///
/// 两条前置条件各给一句原因——都灰着不说话，用户只会以为面板坏了：
///
/// - 没有 SFTP 句柄（本地视图上传同样需要它）；
/// - 远端当前目录还不知道（`filer.path` 为空，等 `Ready`）。
///
/// 第三个参数 `_is_dir` 不再是判据：目录与文件同样放行，由 `start_filer_transfer`
/// 按它选树传输。参数留在签名上，是为了让「这一行能不能传」的调用点与测试
/// 仍能就目录表态（之前这里明确挡掉目录），而不是留一个未用参数警告。
pub(in crate::ui) fn transfer_blocker(
    connected: bool,
    remote_dir: &str,
    _is_dir: bool,
) -> Result<(), &'static str> {
    if !connected {
        return Err("未连接 SFTP");
    }
    if remote_dir.is_empty() {
        return Err("远端目录未知");
    }
    Ok(())
}

impl TerminalWindow {
    /// 队列本身（只读）。Transfer 面板画它，画完再通过下面两个方法改。
    pub(in crate::ui) fn filer_xfer(&self) -> &XferQueue {
        &self.filer.xfer_queue
    }

    /// 右键菜单落地：算目标路径、入队。返回写进 `filer.progress` 的那句话。
    ///
    /// 重复入队也回一句话（「已经在队列里」）而不是沉默：点了就得有反应。
    pub(in crate::ui) fn start_filer_transfer(&mut self, pick: XferPick) -> String {
        let fix_invalid = self
            .tabs
            .get(self.active_slot())
            .is_none_or(|t| t.config.sftp_fix_invalid_path);
        match pick {
            XferPick::Download {
                remote,
                name,
                size,
                is_dir,
            } => {
                // 目标名非法字符按会话配置替换；源端 `remote` 一个字节不动。
                let name = if fix_invalid {
                    term_sftp::sanitize_component(&name)
                } else {
                    name
                };
                // 目标目录用**此刻**的本地目录，之后就固定在任务里（见 enqueue）。
                // 目录时它是一棵树的落点目录（walk::run 会 create_dir_all）。
                let local = self
                    .filer
                    .local_path
                    .join(&name)
                    .to_string_lossy()
                    .into_owned();
                let enqueued = if is_dir {
                    self.filer.xfer_queue.enqueue_download_tree(&remote, &local)
                } else {
                    self.filer.xfer_queue.enqueue_download(&remote, &local, size)
                };
                match enqueued {
                    Some(id) => {
                        if is_dir {
                            format!("#{id} ↓ {name} 目录树已加入队列")
                        } else {
                            format!("#{id} ↓ {name} 已加入队列")
                        }
                    }
                    None => format!("{name} 已经在队列里（或与在传的目录重叠）"),
                }
            }
            XferPick::Upload {
                local,
                name,
                size,
                is_dir,
            } => {
                // 目标名非法字符按会话配置替换；源端 `local` 一个字节不动。
                let name = if fix_invalid {
                    term_sftp::sanitize_component(&name)
                } else {
                    name
                };
                // 远端目标目录由当前 `filer.path` + 真名拼出；目录时是树根。
                let remote = join_path(&self.filer.path, &name);
                let enqueued = if is_dir {
                    self.filer.xfer_queue.enqueue_upload_tree(&local, &remote)
                } else {
                    self.filer.xfer_queue.enqueue_upload(&local, &remote, size)
                };
                match enqueued {
                    Some(id) => {
                        if is_dir {
                            format!("#{id} ↑ {name} 目录树已加入队列")
                        } else {
                            format!("#{id} ↑ {name} 已加入队列")
                        }
                    }
                    None => format!("{name} 已经在队列里（或与在传的目录重叠）"),
                }
            }
        }
    }

    /// 每帧推进队列：退避到点的退回排队，然后**最多**起一项。
    ///
    /// `now` 由 `poll_filer` 从 `ctx.input(|i| i.time)` 取整到秒后传进来——
    /// 队列不读墙钟，界面层读时钟是允许的（只禁 `term-core`）。
    pub(in crate::ui) fn pump_transfer(&mut self, now: u64) {
        self.filer.xfer_queue.retry(now);
        if let Some(task) = self.filer.xfer_queue.next_to_run(now) {
            self.launch_transfer(task, now);
        }
        // 队列里还有活的时候要自己叫醒自己：SFTP 只在**有事件**时叫醒界面，
        // 而「等退避」「等前一项结束」都不产生事件。少了这一行，队列会停在
        // 上一帧的样子，直到用户碰一下鼠标（之后空闲不重绘）。
        if !self.filer.xfer_queue.all_settled() {
            self.ctx
                .request_repaint_after(std::time::Duration::from_millis(200));
        }
    }

    fn launch_transfer(&mut self, task: XferTask, now: u64) {
        // 先取会话配置：`handle` 一借住 `self.filer`，`active_slot()` 就要整份
        // `&self`，两处借不动的是同一个窗口结构体（不是同一个字段）。
        let (preserve, preserve_dir, fix_invalid) = self
            .tabs
            .get(self.active_slot())
            .map(|tab| {
                (
                    tab.config.sftp_preserve_mtime,
                    tab.config.sftp_preserve_dir_mtime,
                    tab.config.sftp_fix_invalid_path,
                )
            })
            .unwrap_or((true, false, true));
        let Some(handle) = self.filer.handle.as_ref() else {
            // 兜底：入口已经按「有没有句柄」灰过一次，走到这里说明句柄是在排队
            // 之后才没有的（会话断了 / 面板重绑）。判一次失败，别让它永远挂着
            // 「传输中」——那比报错更难查。
            self.filer
                .xfer_queue
                .on_failed(&task.remote, "未连接 SFTP", now);
            return;
        };
        // 单文件与目录树的签名同形（两端路径 + resume + preserve），区别只在
        // 方法名与目录 mtime 那一位：`*_tree` 才让后台先展开整棵计划再逐文件传
        // （term-sftp::walk），并在全部文件到齐后写目录时间。
        match (task.direction, task.tree) {
            (Direction::Upload, true) => {
                handle.upload_tree(task.local, task.remote, true, preserve, preserve_dir, fix_invalid)
            }
            (Direction::Upload, false) => handle.upload(task.local, task.remote, true, preserve),
            (Direction::Download, true) => {
                handle.download_tree(task.remote, task.local, true, preserve, preserve_dir, fix_invalid)
            }
            (Direction::Download, false) => {
                handle.download(task.remote, task.local, true, preserve)
            }
        }
    }

    /// Transfer 面板：取消一行。
    ///
    /// 跑着的那一项还要额外叫后台停——模型层只改状态，停传输是持 socket 那一侧
    /// 的事（`queue.rs::cancel` 的注释）。后台是串行的，所以 `cancel()` 取消的
    /// 就是「正在跑的那一次」，不会错杀别项。
    pub(in crate::ui) fn cancel_filer_transfer(&mut self, id: u64) {
        let running = matches!(
            self.filer.xfer_queue.state_of(id),
            Some(term_xfer::JobState::Running { .. })
        );
        if self.filer.xfer_queue.cancel(id) && running {
            if let Some(handle) = self.filer.handle.as_ref() {
                handle.cancel();
            }
        }
        self.ctx.request_repaint();
    }

    /// Transfer 面板：把所有在退避里等着的项提前跑起来。
    pub(in crate::ui) fn retry_filer_transfers(&mut self) {
        self.filer.xfer_queue.retry_now();
        self.ctx.request_repaint();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_blocker_explains_every_reason_out_loud_and_lets_directories_through() {
        // 两条门都有话说：灰着一言不发，用户只会认为面板坏了。
        assert_eq!(
            transfer_blocker(false, "/srv", false),
            Err("未连接 SFTP")
        );
        assert_eq!(transfer_blocker(true, "", false), Err("远端目录未知"));
        // 目录不再被拒——发起方会按 is_dir 选 download_tree / upload_tree，
        // 事件由 xfer_queue 按前缀归到同一行。
        assert_eq!(transfer_blocker(true, "/srv", true), Ok(()));
        assert_eq!(transfer_blocker(true, "/srv", false), Ok(()));
        // 连着目录一起问，两条判据也不能互相遮掉。
        assert_eq!(transfer_blocker(false, "", true), Err("未连接 SFTP"));
        assert_eq!(transfer_blocker(true, "", true), Err("远端目录未知"));
    }

    #[test]
    fn the_connected_check_comes_first() {
        // 顺序是有意的：没连上时「远端目录未知」也成立，但先说根因——
        // 报第二因会把人引去查路径，而真正缺的是一条连接。
        assert_eq!(transfer_blocker(false, "", true), Err("未连接 SFTP"));
    }
}

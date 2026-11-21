//! `term-sftp`：远端文件浏览与传输。
//!
//! 两条边界：
//!
//! 1. **不在 `term-ssh` 里**。连接层管「连上 / 认证 / 主机密钥」，这一层管
//!    「列一个目录」「传一个文件」。失败方式不同：终端连不上是这台机器不可用，
//!    SFTP 列不出来只是这个目录不可读，终端照常能敲命令。
//! 2. **对外同步**。`start_in_background` 返回 `SftpHandle`，调用方发请求、
//!    每帧收事件；`apps/*` 的源码里零 `.await`（ADR-0017 §决策 2 的笼子，
//!    这个笼子有第三处：`term-ssh` / `term-tunnel` / `term-sftp`）。
//!
//! 浏览与传输共用一条会话：列目录、下载、上传都是同一条 `SftpSession` 上的请求，
//! 每次重连要额外付认证与 subsystem 的钱。传输的**规则**（续传偏移、目标路径、
//! 进度格式）在 [`transfer`] 里，都是纯函数；动手读写的两个函数只收
//! `&SftpSession` 与一个进度回调，不认识事件通道。

pub mod blocking;
mod background;
mod cancel;
mod entry;
mod error;
mod mutate;
mod preserve;
mod transfer;
mod walk;

pub use background::{Event, Request, SftpHandle, start_in_background};
pub use blocking::{Client as BlockingClient, RemoteStat};
pub use cancel::{CancelToken, Disposition, Stop, disposition};
pub use entry::{Entry, format_size, join, mtime_secs, parent, sort_entries};
pub use mutate::{MAX_MODE, check_mode, check_path, merge_permissions};
pub use error::SftpError;
pub use preserve::{from_epoch_secs, to_epoch_secs, write_local_dir_mtime, write_local_mtime, write_remote_mtime};
pub use transfer::{
    Direction, ResumeError, TransferError, format_progress, local_base_name, local_destination,
    local_name_for, percent, remote_destination, resume_offset,
};
pub use walk::{
    FileItem, Plan, ScanError, ScannedEntry, local_path_for, plan_local, plan_remote,
    remote_path_for, scan_local_entries, scan_remote_entries,
};

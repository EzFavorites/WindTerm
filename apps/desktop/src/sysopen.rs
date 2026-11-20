//! 用系统默认程序打开文件或目录。
//!
//! 为什么不写在 `ui/` 里：`ui/` 的约定是「只读快照 + 画」，拉起一个外部进程不是画。
//! 这里与 `clipboard.rs` / `fonts.rs` 同级——属于投影层的 OS 集成，且只有一处。
//!
//! **不等子进程**：`open` / `xdg-open` 自己会 fork 出真正的程序就退出，
//! 等它等于等用户关掉编辑器，那会把界面卡死。

use std::path::Path;
use std::process::{Command, Stdio};

/// 用系统默认程序打开 `path`（文件或目录）。
pub fn open(path: &Path) -> std::io::Result<()> {
    let mut cmd = opener();
    cmd.arg(path)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    cmd.spawn()?;
    Ok(())
}

fn opener() -> Command {
    if cfg!(target_os = "macos") {
        Command::new("open")
    } else if cfg!(target_os = "windows") {
        Command::new("explorer")
    } else {
        Command::new("xdg-open")
    }
}

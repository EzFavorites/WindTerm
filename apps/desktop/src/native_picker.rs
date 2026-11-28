//! 系统文件/目录对话框的宿主侧句柄（ADR-0027 的「不透明句柄」纪律）。
//!
//! `ui/` 只拿到这个句柄，不持有线程、channel 或平台对象：对话框在后台线程上
//! 调用 `rfd`，其 macOS 后端把 `NSOpenPanel` 派回主线程跑模态循环。界面每帧
//! `poll()` 一次共享槽；对话框开着时 `poll()` 返回 `None`，调用方请求重画。
//!
//! 截图夹具绝不弹窗：`--screenshot` 下 `new()` 直接返回一个已经填好「取消」的
//! 句柄，自动化不会因为一个看不见的系统对话框挂死。`--headless` 已由 `main.rs`
//! 显式报错退出（ADR-0030），不存在这条路径。

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

/// 选择类型与初始位置。标题用的是纯文案，不含任何命令 ID。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum NativePick {
    File,
    Folder,
}

pub(crate) struct NativePicker {
    slot: Arc<Mutex<Option<Option<PathBuf>>>>,
}

impl NativePicker {
    pub(crate) fn new(kind: NativePick, title: &'static str, current: &str) -> Self {
        let slot = Arc::new(Mutex::new(None));
        if automated_run() {
            // 截屏夹具里「点了浏览」不能真弹系统窗口；按取消处理。
            if let Ok(mut guard) = slot.lock() {
                *guard = Some(None);
            }
            return Self { slot };
        }

        let shared = Arc::clone(&slot);
        let current = current.to_string();
        std::thread::spawn(move || {
            let path = PathBuf::from(&current);
            let start = if path.is_dir() {
                path.clone()
            } else {
                path.parent()
                    .filter(|p| !p.as_os_str().is_empty())
                    .map(PathBuf::from)
                    .unwrap_or_else(term_io::local_dir::home)
            };
            let mut dialog = rfd::FileDialog::new().set_title(title).set_directory(start);
            if kind == NativePick::File {
                if let Some(name) = path.file_name() {
                    dialog = dialog.set_file_name(name.to_string_lossy().into_owned());
                }
            }
            let picked = match kind {
                NativePick::File => dialog.pick_file(),
                NativePick::Folder => dialog.pick_folder(),
            };
            // 线程可能比窗口活得久；槽还在，写进去即可。锁中毒时取回内部值，
            // 绝不 unwrap——生产路径的 panic 基线只许减不许增。
            match shared.lock() {
                Ok(mut guard) => *guard = Some(picked),
                Err(poisoned) => *poisoned.into_inner() = Some(picked),
            }
        });
        Self { slot }
    }

    /// 对话框关闭后取一次结果；`None` = 还在选，`Some(None)` = 用户取消。
    pub(crate) fn poll(&self) -> Option<Option<PathBuf>> {
        match self.slot.lock() {
            Ok(mut guard) => guard.take(),
            Err(poisoned) => poisoned.into_inner().take(),
        }
    }
}

fn automated_run() -> bool {
    std::env::args().any(|arg| arg == "--screenshot")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_cancelled_dialog_reports_none_once() {
        let picker = NativePicker {
            slot: Arc::new(Mutex::new(Some(None))),
        };
        assert_eq!(picker.poll(), Some(None));
        assert_eq!(picker.poll(), None);
    }
}

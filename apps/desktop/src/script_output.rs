//! 脚本面板的只读输出标签：用内存 `TermChannel` 把脚本日志喂给一个普通终端标签。
//!
//! 脚本自己的 `send/write` 仍走真实会话 PTY（屏幕上的字节都是会话事实）；这里只
//! 接 `notify` 与最终返回值，写进一个 `local_mode = true` 的独立标签。只读意味着
//! 键盘/粘贴/触发器都进不去，不会被误当成可输入会话。
//!
//! 不把返回值写进真实会话——那是伪造远端输出（见 `panels/script_panel.rs` 模块头）。

use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use term_core::channel::TermChannel;

/// 可写的输出队列。日志写入端持有它，`TermChannel::drain` 从同一份取字节。
#[derive(Clone, Default)]
pub struct ScriptOutputQueue {
    bytes: Arc<Mutex<VecDeque<u8>>>,
    done: Arc<AtomicBool>,
}

impl ScriptOutputQueue {
    pub fn push(&self, text: &str) {
        let mut bytes = text.as_bytes().to_vec();
        bytes.extend_from_slice(b"\r\n");
        if let Ok(mut q) = self.bytes.lock() {
            q.extend(bytes);
        }
    }

    pub fn finish(&self, status: &str) {
        self.push(status);
        self.done.store(true, Ordering::Release);
    }

    pub fn channel(&self) -> ScriptOutputChannel {
        ScriptOutputChannel {
            bytes: Arc::clone(&self.bytes),
            done: Arc::clone(&self.done),
            reported: false,
        }
    }
}

/// 给 `SessionPool::open_with_channel` 用的只读源通道。
pub struct ScriptOutputChannel {
    bytes: Arc<Mutex<VecDeque<u8>>>,
    done: Arc<AtomicBool>,
    reported: bool,
}

impl TermChannel for ScriptOutputChannel {
    fn drain(&mut self) -> Vec<u8> {
        match self.bytes.lock() {
            Ok(mut q) => q.drain(..).collect(),
            Err(_) => Vec::new(),
        }
    }

    fn write(&mut self, _bytes: &[u8]) {
        // 只读输出：键盘输入不进脚本、也不进真实会话。
    }

    fn resize(&mut self, _cols: usize, _rows: usize) {}

    fn poll(&mut self) -> Option<String> {
        if self.done.load(Ordering::Acquire) && !self.reported {
            self.reported = true;
            Some("script finished".to_string())
        } else {
            None
        }
    }

    fn finished(&self) -> bool {
        self.done.load(Ordering::Acquire)
    }

    fn close(&mut self) {
        self.done.store(true, Ordering::Release);
    }

    fn description(&self) -> &str {
        "script output (read-only)"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn queue_drains_only_once_and_reports_finish_once() {
        let q = ScriptOutputQueue::default();
        let mut ch = q.channel();
        q.push("hello");
        assert_eq!(String::from_utf8_lossy(&ch.drain()), "hello\r\n");
        assert!(ch.drain().is_empty());
        q.finish("done");
        assert_eq!(String::from_utf8_lossy(&ch.drain()), "done\r\n");
        assert_eq!(ch.poll().as_deref(), Some("script finished"));
        assert_eq!(ch.poll(), None);
        assert!(ch.finished());
    }
}

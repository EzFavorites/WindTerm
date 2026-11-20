//! 脚本的**剪贴板**与**窗口**端口：把 `egui` 的能力接成 `Clipboard` / `Ui`。
//!
//! # 为什么这两个端口都只是「转发」
//!
//! 剪贴板与窗口都属于 UI 线程的资产（`egui::Context` 是唯一合法的入口）。
//! 脚本线程不能直接动它们，但它可以**排队**：`Context::copy_text` 与
//! `send_viewport_cmd` 本身就是「把请求塞进下一帧」的形状，跨线程调用是安全的
//! （`Context` 是 `Send + Sync`）。所以这里没有额外的队列。
//!
//! 唯一的例外是**选区**：egui 没有「读当前选区」的 API（选区是我们自己在
//! 终端控件里维护的），所以界面每帧把选区文本抄进一份共享槽
//! （[`SelectionSlot`]），脚本读槽。读到的可能比「此刻」晚一帧——对
//! 「把选中内容存成变量」这个用途足够，而比「脚本自己去猜用户在选什么」好得多。
//!
//! # 授权
//!
//! 剪贴板读、窗口控制都要显式授权（`clipboard` / `ui`）。判定在工作区
//! （`caps.check_clipboard()` / `check_ui()`），这里不重复判——两处都判会让
//! 「到底哪一层拒的」变成谜。

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use eframe::egui;
use term_core::session_runtime::SessionId;
use term_script::{ClipError, Clipboard, Ui, UiError};

/// 剪贴板端口：`copy_text` 写、`output().copied_text` 读。
pub struct DeskClipboard {
    ctx: egui::Context,
}

impl DeskClipboard {
    pub fn new(ctx: egui::Context) -> Self {
        Self { ctx }
    }
}

impl Clipboard for DeskClipboard {
    fn get(&self) -> Result<String, ClipError> {
        // egui 把「本进程最近一次复制的文本」放在输出的 `copied_text` 里。
        // 读不到（冷启动、别的程序复制的）就是空串——不是错误：CRT 的
        // `Clipboard.Text` 在空剪贴板上也返回空串。
        Ok(self.ctx.output(|out| out.copied_text.clone()))
    }

    fn set(&self, text: &str) -> Result<(), ClipError> {
        self.ctx.copy_text(text.to_string());
        Ok(())
    }
}

/// 界面每帧抄进来的「当前选区」。
///
/// `Arc<Mutex<String>>` 而不是 `RwLock`：写入每帧一次、读取偶尔一次，
/// 锁的争用为零，读写锁的复杂度换不来什么。
pub type SelectionSlot = Arc<Mutex<String>>;

/// 窗口端口：显隐、退出、取选区。
pub struct DeskUi {
    ctx: egui::Context,
    selection: SelectionSlot,
    /// `quit` 被叫过没有。界面据此决定「这一帧结束就关窗」——`send_viewport_cmd`
    /// 是异步的，再存一份旗子让界面能立刻回应（也便于单测断言）。
    quit: Arc<AtomicBool>,
}

impl DeskUi {
    pub fn new(ctx: egui::Context, selection: SelectionSlot) -> Self {
        Self {
            ctx,
            selection,
            quit: Arc::new(AtomicBool::new(false)),
        }
    }

    /// 退出旗子的共享句柄（界面轮询它，或者脚本线程之间互相看）。
    pub fn quit_flag(&self) -> Arc<AtomicBool> {
        Arc::clone(&self.quit)
    }

    fn read_selection(&self) -> String {
        match self.selection.lock() {
            Ok(text) => text.clone(),
            // 中毒只说明抄写线程崩过；槽里的内容还能用。
            Err(poisoned) => poisoned.into_inner().clone(),
        }
    }
}

impl Ui for DeskUi {
    fn selection(&self, _session: SessionId) -> Result<String, UiError> {
        // `_session` 现在没用上：选区是**窗口级**的派生值（鼠标选中哪块就
        // 是哪块，不区分标签）。将来若按标签各存一份，这个参数就是入口。
        Ok(self.read_selection())
    }

    fn set_visible(&self, visible: bool) -> Result<(), UiError> {
        self.ctx
            .send_viewport_cmd(egui::ViewportCommand::Visible(visible));
        Ok(())
    }

    fn quit(&self, _save: bool) -> Result<(), UiError> {
        // `save` 不在这里区分：保存布局与会话库是 `on_exit` 的既有职责
        // （它一定会在窗口关闭时跑），脚本再存一遍只会多一次写盘竞态。
        self.quit.store(true, Ordering::SeqCst);
        self.ctx
            .send_viewport_cmd(egui::ViewportCommand::Close);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_selection_slot_round_trips() {
        let slot: SelectionSlot = Arc::new(Mutex::new(String::new()));
        let ui = DeskUi::new(egui::Context::default(), Arc::clone(&slot));
        assert_eq!(ui.selection(1).expect("读选区"), "", "空槽 = 空串，不是错误");
        *slot.lock().expect("槽锁") = "选中的字".to_string();
        assert_eq!(ui.selection(1).expect("读选区"), "选中的字");
    }

    #[test]
    fn quitting_raises_the_flag_the_window_polls() {
        let slot: SelectionSlot = Arc::new(Mutex::new(String::new()));
        let ui = DeskUi::new(egui::Context::default(), slot);
        let flag = ui.quit_flag();
        assert!(!flag.load(Ordering::SeqCst));
        ui.quit(true).expect("退出不留失败");
        assert!(flag.load(Ordering::SeqCst));
    }

    /// 空剪贴板读空串、写不留失败。
    ///
    /// 这里**不**断言「写进去能读回来」：egui 的 `copied_text` 是**本帧**的
    /// 平台输出，只在一次 `begin_pass`/`end_pass` 之内存在；在帧外写、帧外读
    /// 拿到的永远是空串。真正能读回自己刚写的内容只有「同一帧里先写后读」，
    /// 那是 `OSC 52` 会话内自读的场景，不是脚本的（脚本的 `clipboard_get`
    /// 读的是用户上一次复制的东西，由 egui 从系统剪贴板填进输出）。
    #[test]
    fn an_empty_clipboard_reads_as_an_empty_string_and_writes_do_not_fail() {
        let ctx = egui::Context::default();
        let board = DeskClipboard::new(ctx);
        assert_eq!(board.get().expect("空剪贴板"), "");
        board.set("hello").expect("写剪贴板不留失败");
    }
}

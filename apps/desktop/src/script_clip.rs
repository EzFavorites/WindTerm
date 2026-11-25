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
//! # 剪贴板读：只认界面递过来的真值
//!
//! `egui` 的 `output().copied_text` 是**本进程最近一次 `copy_text`** 的影子，
//! 脚本读它会读到自己上一次 `clipboard_set` 写进去的字——跨应用复制的内容
//! 永远读不到。真正的系统剪贴板文本只有一条入口：宿主把 Cmd+V / 右键粘贴
//! 翻成 `egui::Event::Paste(text)` 送进界面。于是在这里装一个 pass 级
//! reactor，把每次 Paste 的文本抄进共享槽；`get()` 读槽。
//!
//! 槽里还没有任何 Paste 事实（冷启动 / 用户从没在本窗口粘过）时显式
//! `Unsupported`，**不**拿空串冒充成功，更不回读自己写出去的内容。
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

/// 从 `Event::Paste` 收割来的**系统剪贴板事实**。`None` = 界面还没递过来过。
#[derive(Default)]
struct ClipboardTruth {
    pasted: Option<String>,
}

type ClipboardTruthSlot = Arc<Mutex<ClipboardTruth>>;

/// 剪贴板端口：`copy_text` 写；读只认 reactor 从 `Event::Paste` 抄进来的真值。
pub struct DeskClipboard {
    ctx: egui::Context,
    truth: ClipboardTruthSlot,
}

/// 把本帧的 Paste 事件抄进槽。只克隆、不摘掉事件——终端的粘贴处理也必须拿到它。
fn harvest_paste(ctx: &egui::Context, truth: &ClipboardTruthSlot) {
    let pasted: Vec<String> = ctx.input(|input| {
        input
            .events
            .iter()
            .filter_map(|event| match event {
                egui::Event::Paste(text) => Some(text.clone()),
                _ => None,
            })
            .collect()
    });
    if let Some(last) = pasted.last() {
        if let Ok(mut slot) = truth.lock() {
            slot.pasted = Some(last.clone());
        }
    }
}

impl DeskClipboard {
    pub fn new(ctx: egui::Context) -> Self {
        let truth: ClipboardTruthSlot = Arc::new(Mutex::new(ClipboardTruth::default()));
        // UI 每帧开始就把 Paste 抄一份；脚本线程只在 get() 时读槽，互不阻塞。
        let reactor_truth = Arc::clone(&truth);
        ctx.on_begin_pass(
            "script-clipboard-paste",
            Arc::new(move |ctx: &egui::Context| harvest_paste(ctx, &reactor_truth)),
        );
        Self { ctx, truth }
    }
}

impl Clipboard for DeskClipboard {
    fn get(&self) -> Result<String, ClipError> {
        // 帧外直接调时也收一次（脚本线程可能在两次 pass 之间读）。
        harvest_paste(&self.ctx, &self.truth);
        let slot = self.truth.lock().map_err(|_| {
            ClipError::Unsupported("剪贴板事实槽锁中毒，无法确认系统剪贴板内容".to_string())
        })?;
        slot.pasted.clone().ok_or_else(|| {
            ClipError::Unsupported(
                "没有来自界面的剪贴板事实：egui 只在用户粘贴时把系统剪贴板送进来；请先在窗口里粘贴一次"
                    .to_string(),
            )
        })
    }

    fn set(&self, text: &str) -> Result<(), ClipError> {
        // 写是「把请求排队给下一帧」，不会回头污染读槽：脚本读到的永远是
        // 用户从别的应用粘进来的那一份，而不是自己刚写出去的。
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

    /// 冷启动：界面还没把任何 Paste 事实递进来时**显式 Unsupported**，
    /// 不拿空串冒充「剪贴板是空的」，更不回读 `set` 自己写出去的字。
    #[test]
    fn a_cold_clipboard_is_unsupported_not_an_empty_string() {
        let board = DeskClipboard::new(egui::Context::default());
        match board.get() {
            Err(ClipError::Unsupported(why)) => assert!(why.contains("界面"), "{why}"),
            other => panic!("冷启动应显式 Unsupported：{other:?}"),
        }
        board.set("我自己写的").expect("写剪贴板不留失败");
        match board.get() {
            Err(ClipError::Unsupported(_)) => {}
            other => panic!("自己的写入不能冒充系统剪贴板：{other:?}"),
        }
    }

    /// 跨应用：别的程序复制的字经界面 `Event::Paste` 进来，脚本读到的就是它。
    /// reactor 在 pass 开始时收割——这里直接跑一次 pass 模拟那一帧。
    #[test]
    fn a_paste_from_another_app_becomes_the_value_the_script_reads() {
        let ctx = egui::Context::default();
        let board = DeskClipboard::new(ctx.clone());
        ctx.input_mut(|input| {
            input.events.push(egui::Event::Paste("别的应用的密码".into()));
        });
        assert_eq!(board.get().expect("读到真值"), "别的应用的密码");
        // 自己随后写一次，不该覆盖用户粘进来的事实。
        board.set("脚本输出").expect("写不留失败");
        assert_eq!(board.get().expect("仍是真值"), "别的应用的密码");
    }

    /// 新的一次粘贴覆盖旧值（reactor 每次 pass 都收割最新一条）。
    #[test]
    fn the_latest_paste_wins() {
        let ctx = egui::Context::default();
        let board = DeskClipboard::new(ctx.clone());
        ctx.input_mut(|input| input.events.push(egui::Event::Paste("old".into())));
        assert_eq!(board.get().expect("旧值"), "old");
        ctx.input_mut(|input| input.events.push(egui::Event::Paste("new".into())));
        assert_eq!(board.get().expect("新值"), "new");
    }
}

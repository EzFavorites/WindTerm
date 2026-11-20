//! `HostEnv`：一次运行的**全部宿主接缝**（ADR-0027）。
//!
//! # 为什么不是一串 `with_*`
//!
//! 端口只有 `Sessions` 一个时，`PooledSessions::new(...).with_opened(...)
//! .with_notes(...).without_input()` 是可读的。六个端口各自要注入实现 +
//! 授权 + 取消令牌 + 上下文之后，这个链会到十项——那时「哪一项没接」
//! 只能靠人眼在十行里找。
//!
//! 所以把「一次运行需要的东西」收成一个结构：缺省值全是**显式降级**
//! （`HeadlessHuman` / `NoFiles` / `NoProcess` / `NoClipboard` / `NoUi`），
//! 接上哪一个由调用方显式 `with_*`。少接一项的后果是「那个动词报
//! Unsupported」，不是「静默用默认值」。
//!
//! # `cols` / `rows` 为什么在这里
//!
//! `session_open()` 不带尺寸时用它们。界面传窗口尺寸、CLI 传 80×24；
//! 这个数属于**宿主**（它知道终端多大），不属于脚本。

use std::sync::{Arc, Mutex};

use term_core::script_caps::Capabilities;

use crate::app_ui::{AppControl, NoApp};
use crate::cancel::CancelToken;
use crate::clip_ui::{Clipboard, NoClipboard, NoUi, Ui};
use crate::human::{HeadlessHuman, HumanChannel};
use crate::localfs::{LocalFs, NoFiles};
use crate::proc::{NoProcess, Process};
use crate::sessions::Sessions;

/// 运行上下文：脚本自己那几件「我是谁、从哪来」的事实。
///
/// 与 [`HostEnv`] 分开是因为生命周期不同：上下文只读（`last_error` 除外），
/// 可以跟着工作区跨线程共享；端口是「能做什么」，由宿主决定接哪些。
#[derive(Debug)]
pub struct ScriptContext {
    /// 脚本文件路径；`-`（stdin）为 `None`。
    pub script_path: Option<String>,
    /// 命令行参数（`desk script run --arg a --arg b`）。
    pub args: Vec<String>,
    /// 版本串（`term-script` 的包版本；与能力面 `version()` 同一个来源）。
    pub version: String,
    /// 最近一次脚本错误的文本（`last_error()`）。
    last_error: Mutex<String>,
}

impl Default for ScriptContext {
    fn default() -> Self {
        Self::new(None, Vec::new())
    }
}

impl ScriptContext {
    pub fn new(script_path: Option<String>, args: Vec<String>) -> Self {
        Self {
            script_path,
            args,
            version: format!("WindTerm {}", env!("CARGO_PKG_VERSION")),
            last_error: Mutex::new(String::new()),
        }
    }

    /// 记下最近一次错误。空串表示「还没有错误」。
    pub fn record_error(&self, message: impl Into<String>) {
        let mut slot = match self.last_error.lock() {
            Ok(slot) => slot,
            Err(poisoned) => poisoned.into_inner(),
        };
        *slot = message.into();
    }

    pub fn last_error(&self) -> String {
        match self.last_error.lock() {
            Ok(slot) => slot.clone(),
            Err(poisoned) => poisoned.into_inner().clone(),
        }
    }
}

/// 一次运行的宿主接缝。缺省值都是显式降级（见模块文档）。
pub struct HostEnv {
    pub sessions: Arc<dyn Sessions>,
    pub human: Arc<dyn HumanChannel>,
    pub files: Arc<dyn LocalFs>,
    pub process: Arc<dyn Process>,
    pub clipboard: Arc<dyn Clipboard>,
    pub ui: Arc<dyn Ui>,
    /// 应用控制面（面板 / 布局 / 偏好，ADR-0027）。缺省 [`AppControl`] 的
    /// `NoApp`：两个方法都显式 `Unsupported`，与 `NoUi` / `NoFiles` 同一条规矩。
    pub app: Arc<dyn AppControl>,
    /// 本次运行授予的能力（缺省全拒）。
    pub caps: Capabilities,
    /// 取消开关（界面「停止」按的就是它）。
    pub cancel: CancelToken,
    pub context: Arc<ScriptContext>,
    pub cols: usize,
    pub rows: usize,
}

impl HostEnv {
    /// 最小宿主：只有会话面，其余全部显式降级。
    pub fn new(sessions: Arc<dyn Sessions>) -> Self {
        Self {
            sessions,
            human: Arc::new(HeadlessHuman),
            files: Arc::new(NoFiles),
            process: Arc::new(NoProcess),
            clipboard: Arc::new(NoClipboard),
            ui: Arc::new(NoUi),
            app: Arc::new(NoApp),
            caps: Capabilities::NONE,
            cancel: CancelToken::new(),
            context: Arc::new(ScriptContext::default()),
            cols: term_core::session::OpenRequest::DEFAULT_COLS,
            rows: term_core::session::OpenRequest::DEFAULT_ROWS,
        }
    }

    pub fn with_human(mut self, human: Arc<dyn HumanChannel>) -> Self {
        self.human = human;
        self
    }

    pub fn with_files(mut self, files: Arc<dyn LocalFs>) -> Self {
        self.files = files;
        self
    }

    pub fn with_process(mut self, process: Arc<dyn Process>) -> Self {
        self.process = process;
        self
    }

    pub fn with_clipboard(mut self, clipboard: Arc<dyn Clipboard>) -> Self {
        self.clipboard = clipboard;
        self
    }

    pub fn with_ui(mut self, ui: Arc<dyn Ui>) -> Self {
        self.ui = ui;
        self
    }

    pub fn with_app(mut self, app: Arc<dyn AppControl>) -> Self {
        self.app = app;
        self
    }

    pub fn with_caps(mut self, caps: Capabilities) -> Self {
        self.caps = caps;
        self
    }

    pub fn with_cancel(mut self, cancel: CancelToken) -> Self {
        self.cancel = cancel;
        self
    }

    pub fn with_context(mut self, context: Arc<ScriptContext>) -> Self {
        self.context = context;
        self
    }

    pub fn with_size(mut self, cols: usize, rows: usize) -> Self {
        self.cols = cols;
        self.rows = rows;
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sessions::FixedSessions;
    use crate::testutil::fake_session;
    use std::time::Duration;

    fn env() -> HostEnv {
        let (session, _manip) = fake_session(1);
        HostEnv::new(Arc::new(FixedSessions::new(session)))
    }

    #[test]
    fn the_default_env_only_knows_about_sessions() {
        let env = env();
        assert_eq!(env.cols, 80);
        assert_eq!(env.rows, 24);
        assert_eq!(env.caps, Capabilities::NONE, "缺省必须全拒");
        assert!(!env.cancel.is_cancelled());
        let _ = Duration::from_secs(1);
    }

    #[test]
    fn context_records_the_last_error_and_starts_empty() {
        let context = ScriptContext::new(Some("/tmp/a.rhai".to_string()), vec!["x".to_string()]);
        assert!(context.last_error().is_empty());
        context.record_error("第 2 行：等待超时");
        assert!(context.last_error().contains("第 2 行"));
        assert!(context.version.starts_with("WindTerm "));
    }
}

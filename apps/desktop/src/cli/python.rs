//! `script run a.py` 的同步包装：复用界面那套 `ScriptRun`（租约 / 取消 /
//! Outcome），只是这边轮询到结果再退出进程。放在单独模块是为了让
//! `cli/script.rs` 守住 500 代码行门禁。

use std::sync::Arc;
use std::time::Duration;

use term_core::session_runtime::SessionId;
use term_script::ScriptBudget;
use term_session::SessionPool;

use crate::script_run::{Outcome, RunOptions, ScriptRun};

/// 跑一份外部 Python 脚本，返回给 CLI 打印的结果文本。
pub fn run(
    pool: Arc<SessionPool>,
    id: SessionId,
    path: &str,
    args: Vec<String>,
    allow: String,
    budget: ScriptBudget,
) -> Result<String, crate::python_run::PythonRunError> {
    let mut options = RunOptions::default();
    options.args = args;
    options.allow = allow;
    options.budget = budget;
    let mut run = ScriptRun::start_python(pool, id, path, options).map_err(|e| e.to_string())?;
    loop {
        if let Some(outcome) = run.try_outcome() {
            return match outcome {
                Outcome::Done(value) => Ok(value),
                Outcome::Failed { message, .. } => Err(message.into()),
            };
        }
        std::thread::sleep(Duration::from_millis(10));
    }
}

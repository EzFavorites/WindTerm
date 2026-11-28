//! 外部 Python 脚本宿主：把 Python 子进程接到本进程的 JSON-RPC 桥。
//!
//! CRT 9.0 是进程内加载 Python DLL；这里刻意走**子进程**：
//! 桥接已经是「一行一个 JSON」的 stdio 协议，子进程天然拿到一条独占管道，
//! 崩溃、第三方库、GIL 都被关在进程外，宿主不需要链接 libpython。
//! 用户看到的是同一件事：写 Python、用原生 `write` / `read_until`、
//! 从脚本菜单 / 面板运行。
//!
//! 起外部进程要 `process` 授权（默认拒绝、只作用于本次运行），与 `proc_run`
//! 同一条纪律。取消或超时会 kill 并回收子进程，不会留下孤儿。

use std::io::BufReader;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use term_script::CancelToken;
use term_session::SessionPool;

use crate::bridge::surface::DeskSurface;

const POLL: Duration = Duration::from_millis(50);

/// 外部 Python 的失败边界。用类型而不是 `String`，避免给全仓的
/// `Result<_, String>` 基线再添一条；kind 与文案分离，UI 分支不再匹配文本。
#[derive(Debug, Clone)]
pub struct PythonRunError {
    message: String,
    kind: crate::script_run::FailureKind,
}

impl PythonRunError {
    fn new(kind: crate::script_run::FailureKind, message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            kind,
        }
    }

    pub fn kind(&self) -> crate::script_run::FailureKind {
        self.kind
    }
}

impl std::fmt::Display for PythonRunError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for PythonRunError {}

impl From<String> for PythonRunError {
    fn from(message: String) -> Self {
        Self::new(crate::script_run::FailureKind::Failed, message)
    }
}

/// 选解释器：允许用 `WINDTERM_PYTHON` 指定绝对路径，否则找 PATH 上的 python3/python。
fn interpreter() -> Result<String, PythonRunError> {
    if let Ok(path) = std::env::var("WINDTERM_PYTHON") {
        if !path.trim().is_empty() {
            return Ok(path);
        }
    }
    let candidates: &[&str] = if cfg!(windows) {
        &["python", "python3"]
    } else {
        &["python3", "python"]
    };
    for name in candidates {
        // 只做存在性判断：真正执行时仍按这个名字找，避免 TOCTOU 式的路径缓存。
        if which(name).is_some() {
            return Ok((*name).to_string());
        }
    }
    Err(format!(
        "找不到 Python 解释器（试了 {}）；安装 Python 3 或用 WINDTERM_PYTHON 指定路径",
        candidates.join(" / ")
    )
    .into())
}

fn which(name: &str) -> Option<std::path::PathBuf> {
    let paths = std::env::var_os("PATH")?;
    for dir in std::env::split_paths(&paths) {
        let candidate = dir.join(name);
        if candidate.is_file() {
            return Some(candidate);
        }
    }
    None
}

/// 启动器：原生 API 层在 import 时接管 stdin/stdout 作为 RPC 管道，
/// 用户的 print 被改道 stderr；这里再把用户脚本按 UTF-8 读进来执行。
const LAUNCHER: &str = r#"
import os, sys
sys.path.insert(0, os.environ["ZTERM_SHIM_DIR"])
import windterm_api as api  # noqa: F401 —— import 即绑定会话 / 改道 stdout
path = sys.argv[1]
with open(path, "r", encoding="utf-8") as handle:
    source = handle.read()
# 原生 API 是一等函数，不是对象：把模块里的函数直接交给用户脚本。
ns = dict(vars(api))
ns["__name__"] = "__main__"
ns["__file__"] = path
exec(compile(source, path, "exec"), ns)
"#;

/// 同步跑一份 Python 脚本，返回给界面显示的结果字符串。
///
/// 源文本由调用方给、这里写到临时文件再执行：授权解析与真正执行的是**同一份
/// 字节**，不存在「检查完又被改掉」的窗口。stderr 落临时文件而不是开线程读，
/// 孙进程持有 fd 也读不完也不会把运行线程卡死。
pub fn run_blocking(
    pool: Arc<SessionPool>,
    session: String,
    script_path: &str,
    source: &str,
    args: &[String],
    wall: Duration,
    cancel: CancelToken,
    caps: term_core::script_caps::Capabilities,
) -> Result<String, PythonRunError> {
    use crate::script_run::FailureKind as K;

    if caps.check_process().is_err() {
        return Err(PythonRunError::new(
            K::Denied,
            "外部 Python 需要 process 授权：在脚本面板勾选「运行程序」，或用 # @cap process / --allow process",
        ));
    }
    if session.trim().is_empty() {
        return Err(PythonRunError::new(
            K::NoSuchSession,
            "这条会话没有活着的目标，Python 脚本无处可跑",
        ));
    }
    let python = interpreter()?;

    let tmp = std::env::temp_dir().join(format!(
        "zterm-python-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos()
    ));
    std::fs::create_dir_all(&tmp)
        .map_err(|e| format!("建不了 Python 临时目录 {}：{e}", tmp.display()))?;
    let shim = tmp.join("windterm_api.py");
    std::fs::write(&shim, term_bridge::python_api_shim())
        .map_err(|e| format!("写不了原生 API 层：{e}"))?;
    let script_file = tmp.join("script.py");
    std::fs::write(&script_file, source)
        .map_err(|e| format!("写不了 Python 临时脚本：{e}"))?;
    let err_file = tmp.join("stderr.txt");
    let err_handle = std::fs::File::create(&err_file)
        .map_err(|e| format!("建不了 Python 错误输出文件：{e}"))?;

    let mut child = Command::new(&python)
        .arg("-c")
        .arg(LAUNCHER)
        .arg(&script_file)
        .env("ZTERM_SHIM_DIR", &tmp)
        .env("ZTERM_SESSION", &session)
        .env("ZTERM_SCRIPT_PATH", script_path)
        .env("ZTERM_ARGS", serde_json::to_string(args).unwrap_or_else(|_| "[]".to_string()))
        .env("ZTERM_VERSION", env!("CARGO_PKG_VERSION"))
        .env("PYTHONUNBUFFERED", "1")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::from(err_handle))
        .spawn()
        .map_err(|e| format!("起不了 Python（{python}）：{e}"))?;

    let mut stdin = child
        .stdin
        .take()
        .ok_or_else(|| PythonRunError::new(K::Internal, "拿不到 Python 标准输入"))?;
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| PythonRunError::new(K::Internal, "拿不到 Python 标准输出"))?;

    let child = Arc::new(Mutex::new(child));
    let done = Arc::new(AtomicBool::new(false));
    let timed_out = Arc::new(AtomicBool::new(false));
    let watchdog_child = Arc::clone(&child);
    let watchdog_done = Arc::clone(&done);
    let watchdog_timeout = Arc::clone(&timed_out);
    let watchdog_cancel = cancel.clone();
    let deadline = Instant::now() + wall;
    let watchdog = std::thread::spawn(move || loop {
        if watchdog_done.load(Ordering::SeqCst) {
            return;
        }
        if Instant::now() >= deadline {
            watchdog_timeout.store(true, Ordering::SeqCst);
        }
        if watchdog_cancel.is_cancelled() || watchdog_timeout.load(Ordering::SeqCst) {
            if let Ok(mut guard) = watchdog_child.lock() {
                let _ = guard.kill();
            }
            return;
        }
        std::thread::sleep(POLL);
    });

    // 一条 stdio 管道是一条串行事实流，桥接不并发。
    let surface = DeskSurface::new(Arc::clone(&pool));
    let bridge = term_bridge::transport::serve_stdio(&surface, BufReader::new(stdout), &mut stdin);
    done.store(true, Ordering::SeqCst);
    let _ = watchdog.join();

    let mut handle = child
        .lock()
        .map_err(|e| format!("Python 进程状态坏了：{e}"))?;
    let status = handle.wait().map_err(|e| format!("收不了 Python 进程：{e}"))?;
    let errors = std::fs::read_to_string(&err_file).unwrap_or_default();
    let _ = std::fs::remove_dir_all(&tmp);

    // 顺序有意义：墙钟到点优先报超时，不能被「进程被 kill 后的桥接 EOF」盖掉。
    if timed_out.load(Ordering::SeqCst) {
        return Err(PythonRunError::new(
            K::Timeout,
            format!("Python 脚本超过墙钟预算{}", suffix(&errors)),
        ));
    }
    if cancel.is_cancelled() {
        return Err(PythonRunError::new(
            K::Cancelled,
            format!("Python 脚本已取消{}", suffix(&errors)),
        ));
    }
    if let Err(e) = bridge {
        return Err(PythonRunError::new(
            K::Internal,
            format!("桥接出错：{e}{}", suffix(&errors)),
        ));
    }
    if status.success() {
        Ok(format!("Python 脚本完成{}", suffix(&errors)))
    } else {
        Err(PythonRunError::new(
            K::Failed,
            format!("Python 退出码 {}{}", status.code().unwrap_or(-1), suffix(&errors)),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generated_shim_is_valid_python_when_an_interpreter_is_available() {
        let Some(python) = which("python3").or_else(|| which("python")) else {
            return; // 没装 Python 的环境跳过；门禁不依赖用户的机器装了 Python。
        };
        let dir = std::env::temp_dir().join(format!(
            "zterm-shim-syntax-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos()
        ));
        let _ = std::fs::create_dir_all(&dir);
        let shim = dir.join("windterm_api.py");
        std::fs::write(&shim, term_bridge::python_api_shim()).expect("写 shim");
        let status = Command::new(python)
            .arg("-c")
            .arg("import ast,sys; ast.parse(open(sys.argv[1], encoding='utf-8').read())")
            .arg(&shim)
            .status()
            .expect("跑 Python 语法检查");
        let _ = std::fs::remove_dir_all(&dir);
        assert!(status.success(), "生成的原生 API shim 必须是合法 Python");
    }

    #[test]
    fn python_requires_process_grant() {
        let pool = Arc::new(SessionPool::new());
        let err = run_blocking(
            pool,
            "session".to_string(),
            "/tmp/does-not-matter.py",
            "# @cap process\n",
            &[],
            Duration::from_secs(1),
            CancelToken::new(),
            term_core::script_caps::Capabilities::NONE,
        )
        .unwrap_err();
        assert!(err.to_string().contains("process"), "必须点名缺的是 process 授权：{err}");
    }
}

fn suffix(errors: &str) -> String {
    let trimmed = errors.trim();
    if trimmed.is_empty() {
        String::new()
    } else {
        format!("：{trimmed}")
    }
}

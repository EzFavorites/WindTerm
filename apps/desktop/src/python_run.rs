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

use std::io::{BufReader, Read};
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use term_script::CancelToken;
use term_session::SessionPool;

use crate::bridge::surface::DeskSurface;

const POLL: Duration = Duration::from_millis(50);

/// 外部 Python 的失败边界。用类型而不是 `String`，避免给全仓的
/// `Result<_, String>` 基线再添一条；文案仍是面向脚本作者的原话。
#[derive(Debug, Clone)]
pub struct PythonRunError(String);

impl std::fmt::Display for PythonRunError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for PythonRunError {}

impl From<String> for PythonRunError {
    fn from(message: String) -> Self {
        Self(message)
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
pub fn run_blocking(
    pool: Arc<SessionPool>,
    session: String,
    script: &Path,
    args: &[String],
    wall: Duration,
    cancel: CancelToken,
    process_allowed: bool,
) -> Result<String, PythonRunError> {
    if !process_allowed {
        return Err(
            "外部 Python 需要 process 授权：在脚本面板勾选「运行程序」，或用 # @cap process / --allow process"
                .to_string().into(),
        );
    }
    if session.trim().is_empty() {
        return Err("这条会话没有活着的目标，Python 脚本无处可跑".to_string().into());
    }
    let python = interpreter()?;

    // 生成的形状层写到临时目录；每次运行一份，避免并发运行互相覆盖。
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

    let mut child = Command::new(&python)
        .arg("-c")
        .arg(LAUNCHER)
        .arg(script)
        .env("ZTERM_SHIM_DIR", &tmp)
        .env("ZTERM_SESSION", &session)
        .env("ZTERM_SCRIPT_PATH", script)
        .env("ZTERM_ARGS", serde_json::to_string(args).unwrap_or_else(|_| "[]".to_string()))
        .env("ZTERM_VERSION", env!("CARGO_PKG_VERSION"))
        .env("PYTHONUNBUFFERED", "1")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("起不了 Python（{python}）：{e}"))?;

    let mut stdin = child.stdin.take().ok_or_else(|| "拿不到 Python 标准输入".to_string())?;
    let stdout = child.stdout.take().ok_or_else(|| "拿不到 Python 标准输出".to_string())?;
    let stderr = child.stderr.take().ok_or_else(|| "拿不到 Python 标准错误".to_string())?;

    // stderr 单独读：用户的 print() 和异常栈都在这里，失败时要带进 Outcome。
    let err_thread = std::thread::spawn(move || {
        let mut buf = String::new();
        let mut reader = BufReader::new(stderr);
        let _ = reader.read_to_string(&mut buf);
        buf
    });

    // 看门狗：取消或墙钟到点就 kill。`Child::kill` 要 &mut，所以用 Mutex。
    let child = Arc::new(Mutex::new(child));
    let done = Arc::new(AtomicBool::new(false));
    let watchdog_child = Arc::clone(&child);
    let watchdog_done = Arc::clone(&done);
    let watchdog_cancel = cancel.clone();
    let deadline = Instant::now() + wall;
    let watchdog = std::thread::spawn(move || {
        loop {
            if watchdog_done.load(Ordering::SeqCst) {
                return;
            }
            let timed_out = Instant::now() >= deadline;
            if watchdog_cancel.is_cancelled() || timed_out {
                if let Ok(mut guard) = watchdog_child.lock() {
                    let _ = guard.kill();
                }
                return;
            }
            std::thread::sleep(POLL);
        }
    });

    // 桥接在主线程跑：一条 stdio 管道是一条串行事实流，不并发。
    let surface = DeskSurface::new(Arc::clone(&pool));
    let bridge = term_bridge::transport::serve_stdio(
        &surface,
        BufReader::new(stdout),
        &mut stdin,
    );
    done.store(true, Ordering::SeqCst);
    let _ = watchdog.join();

    let mut handle = child
        .lock()
        .map_err(|e| format!("Python 进程状态坏了：{e}"))?;
    let status = handle.wait().map_err(|e| format!("收不了 Python 进程：{e}"))?;
    let errors = err_thread.join().unwrap_or_default();
    let _ = std::fs::remove_dir_all(&tmp);

    if cancel.is_cancelled() {
        return Err("Python 脚本已取消".to_string().into());
    }
    if let Err(e) = bridge {
        return Err(format!("桥接出错：{e}{}", suffix(&errors)).into());
    }
    if status.success() {
        Ok(format!("Python 脚本完成{}", suffix(&errors)))
    } else {
        Err(format!(
            "Python 退出码 {}{}",
            status.code().unwrap_or(-1),
            suffix(&errors)
        )
        .into())
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
        let script = std::path::Path::new("/tmp/does-not-matter.py");
        let err = run_blocking(
            pool,
            "session".to_string(),
            script,
            &[],
            Duration::from_secs(1),
            CancelToken::new(),
            false,
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

//! 脚本的**本地进程端口**：把 `std::process::Command` 接到 `Process`。
//!
//! # 授权
//!
//! 起外部进程是「以用户身份动用户的机器」，缺省拒绝，必须显式授
//! `process`（脚本头 `// @cap process` 或命令行 `--allow process`）。
//!
//! # 三条硬约定
//!
//! 1. **不经 shell**：`program` 与 `args` 分开传，脚本写 `proc_run("ls", ["-l"])`
//!    就真的只执行 `ls`。拼一行字符串再交给 `/bin/sh -c` 会让「参数里有空格」
//!    变成一条注入通道——而脚本的作者与执行者是同一个人，出问题的只会是
//!    「变量拼错」这种正常失误，却要以注入的形式表现出来。
//! 2. **标准输入要么给、要么立刻关**：不给 `stdin` 时关掉它（`Stdio::null`），
//!    否则子进程会去读父进程的终端，和用户在终端里的输入抢字节。
//! 3. **超时必杀**：`timeout` 到期就 kill 并报 [`ProcError::Timeout`]。不杀的话
//!    一条挂住的 `ping` 会把脚本线程永远钉住（而沙箱的墙钟中断也救不了
//!    ——它只在语句之间检查）。

use std::io::Write;
use std::process::{Command, Stdio};
use std::time::Duration;

use term_core::script_caps::Capabilities;
use term_script::{CancelToken, ProcError, ProcOutput, ProcRun, Process};

/// `ProcRun.timeout = None` 时的缺省上限（端口文档承诺的 30 秒）。
const DEFAULT_PROC_TIMEOUT: Duration = Duration::from_secs(30);

/// 本地进程端口。`caps` 是每次现问的授权来源（理由同 `script_fs`）。
pub struct LocalProcess<F> {
    caps: F,
}

impl<F> LocalProcess<F>
where
    F: Fn() -> Capabilities + Send + Sync,
{
    pub fn new(caps: F) -> Self {
        Self { caps }
    }
}

impl<F> Process for LocalProcess<F>
where
    F: Fn() -> Capabilities + Send + Sync,
{
    fn run(&self, request: &ProcRun) -> Result<ProcOutput, ProcError> {
        // 兼容直接调 `run` 的宿主：没有本次运行的令牌，用一个不会被取消的。
        self.run_cancellable(request, &CancelToken::new())
    }

    fn run_cancellable(
        &self,
        request: &ProcRun,
        cancel: &CancelToken,
    ) -> Result<ProcOutput, ProcError> {
        cancel.check().map_err(|_| ProcError::Cancelled)?;
        (self.caps)()
            .check_process()
            .map_err(|denied| ProcError::Denied(denied.to_string()))?;
        if request.program.trim().is_empty() {
            return Err(ProcError::Io("proc_run 的程序名不能为空".to_string()));
        }
        let mut command = Command::new(&request.program);
        command.args(&request.args);
        if let Some(cwd) = &request.cwd {
            command.current_dir(cwd);
        }
        command.stdout(Stdio::piped()).stderr(Stdio::piped());
        // 不喂输入就关掉它：见模块文档第 2 条。
        let mut stdin = None;
        if let Some(text) = &request.stdin {
            command.stdin(Stdio::piped());
            stdin = Some(text.clone());
        } else {
            command.stdin(Stdio::null());
        }
        let mut child = command
            .spawn()
            .map_err(|e| ProcError::Io(format!("起不了 {}：{e}", request.program)))?;
        if let Some(text) = stdin {
            // 写完立刻丢句柄 = 给子进程一个 EOF，`cat` 这类命令才不会挂住。
            if let Some(mut pipe) = child.stdin.take() {
                pipe.write_all(text.as_bytes())
                    .map_err(|e| ProcError::Io(format!("往子进程写输入失败：{e}")))?;
            }
        }
        // `None` 也必须有上限：端口文档承诺 30 秒缺省，无限等会让「停止」
        // 彻底失效（沙箱只在语句之间检查，卡在 wait 里救不了）。
        let limit = request.timeout.unwrap_or(DEFAULT_PROC_TIMEOUT);
        let output = wait_with_cancel(child, limit, cancel)?;
        Ok(ProcOutput {
            code: output.status.code(),
            stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
        })
    }
}

/// 等子进程结束：每 20ms 同时检查取消令牌与超时，两者都会 kill + 收尸。
///
/// 为什么轮询而不是「在另一个线程上 `wait` + `recv_timeout`」：`Child::wait` 要
/// `&mut self`，跨线程传一个 `Child` 再拿回它的输出是可行的，但那条路在超时
/// 分支上要处理「线程还攥着 Child」的收尾；轮询 20ms 一次足够（脚本对子进程的
/// 时延要求是「秒级」，不是「微秒级」），而且没有额外的线程要管。
fn wait_with_cancel(
    mut child: std::process::Child,
    limit: Duration,
    cancel: &CancelToken,
) -> Result<std::process::Output, ProcError> {
    let started = std::time::Instant::now();
    loop {
        if cancel.is_cancelled() {
            let _ = child.kill();
            // 收尸：不 wait 会留一个僵尸。
            let _ = child.wait();
            return Err(ProcError::Cancelled);
        }
        match child.try_wait() {
            Ok(Some(_)) => {
                return child
                    .wait_with_output()
                    .map_err(|e| ProcError::Io(format!("收回输出失败：{e}")))
            }
            Ok(None) => {
                if started.elapsed() >= limit {
                    let _ = child.kill();
                    let _ = child.wait();
                    return Err(ProcError::Timeout);
                }
                std::thread::sleep(Duration::from_millis(20));
            }
            Err(e) => return Err(ProcError::Io(format!("等子进程时失败：{e}"))),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn allow() -> Capabilities {
        let mut caps = Capabilities::NONE;
        caps.grant(&term_core::script_caps::CapRequest::Process);
        caps
    }

    fn request(program: &str, args: &[&str]) -> ProcRun {
        ProcRun {
            program: program.to_string(),
            args: args.iter().map(|a| a.to_string()).collect(),
            cwd: None,
            timeout: Some(Duration::from_secs(10)),
            stdin: None,
        }
    }

    #[test]
    fn running_a_command_returns_its_streams_and_exit_code() {
        let caps = allow();
        let process = LocalProcess::new(|| caps.clone());
        let output = process
            .run(&request("/bin/sh", &["-c", "echo out; echo err >&2; exit 3"]))
            .expect("跑得起来");
        assert_eq!(output.code, Some(3), "退出码要原样带回来");
        assert_eq!(output.stdout, "out\n");
        assert_eq!(output.stderr, "err\n");
    }

    #[test]
    fn stdin_is_fed_and_closed_so_cat_terminates() {
        let caps = allow();
        let process = LocalProcess::new(|| caps.clone());
        let mut req = request("/bin/cat", &[]);
        req.stdin = Some("喂进去\n".to_string());
        let output = process.run(&req).expect("跑得起来");
        assert_eq!(output.stdout, "喂进去\n", "写进去的字节要经 cat 回来");
    }

    /// 没有授权时**在起进程之前**就拒。
    #[test]
    fn without_the_capability_it_refuses_before_spawning() {
        let process = LocalProcess::new(|| Capabilities::NONE);
        match process.run(&request("/bin/echo", &["hi"])) {
            Err(ProcError::Denied(why)) => assert!(why.contains("进程"), "{why}"),
            other => panic!("期望被拒，实得 {other:?}"),
        }
    }

    /// Stop 必须打断 `wait`：即使 `timeout = None`（走 30 秒缺省），也不能
    /// 让一条挂住的子进程把脚本线程钉到超时。
    #[test]
    fn a_stop_token_kills_the_child_and_returns_immediately() {
        let caps = allow();
        let process = LocalProcess::new(|| caps.clone());
        let token = CancelToken::new();
        let mut req = request("/bin/sh", &["-c", "sleep 30"]);
        req.timeout = None;
        let stopper = token.clone();
        let thread = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(150));
            stopper.cancel();
        });
        let started = std::time::Instant::now();
        match Process::run_cancellable(&process, &req, &token) {
            Err(ProcError::Cancelled) => {}
            other => panic!("期望被取消，实得 {other:?}"),
        }
        thread.join().unwrap();
        assert!(
            started.elapsed() < Duration::from_secs(3),
            "Stop 后要立刻返回，实耗 {:?}",
            started.elapsed()
        );
    }

    /// 接线级：脚本引擎的 Stop → 进程端口 → 会话租约释放。
    #[test]
    fn stop_returns_through_the_engine_and_releases_the_session_lease() {
        let pool = std::sync::Arc::new(term_session::SessionPool::new());
        let id = pool.open_exec("cat", 80, 24).expect("起会话");
        let registry = std::sync::Arc::new(crate::script_run::ScriptRunRegistry::new());
        let source = "// @cap process\nproc_run(\"/bin/sh\", [\"-c\", \"sleep 30\"]);\n";
        let options = crate::script_run::RunOptions::default()
            .with_registry(registry.clone(), "stop-test");
        let mut run = crate::script_run::ScriptRun::start(
            pool.clone(),
            id,
            source.to_string(),
            options,
        )
        .expect("起脚本");
        assert!(registry.holder(id).is_some(), "跑起来就该占着会话");

        // 让脚本真的进到子进程的 wait 里，再按 Stop。
        std::thread::sleep(Duration::from_millis(250));
        let started = std::time::Instant::now();
        run.cancel();
        let outcome = loop {
            if let Some(outcome) = run.try_outcome() {
                break outcome;
            }
            assert!(
                started.elapsed() < Duration::from_secs(3),
                "按了 Stop 却迟迟不返回"
            );
            std::thread::sleep(Duration::from_millis(10));
        };
        assert!(
            matches!(&outcome, crate::script_run::Outcome::Failed { message, .. } if message.contains("取消")),
            "应当报取消，实得 {outcome:?}"
        );
        drop(run);
        assert!(registry.holder(id).is_none(), "停了以后会话租约必须释放");
        pool.close(id);
    }

    /// 挂住的子进程要被杀掉并报 `Timeout`，不能把脚本线程钉死。
    #[test]
    fn a_hanging_child_is_killed_at_the_deadline() {
        let caps = allow();
        let process = LocalProcess::new(|| caps.clone());
        let mut req = request("/bin/sh", &["-c", "sleep 5"]);
        req.timeout = Some(Duration::from_millis(120));
        let started = std::time::Instant::now();
        match process.run(&req) {
            Err(ProcError::Timeout) => {}
            other => panic!("期望超时，实得 {other:?}"),
        }
        assert!(
            started.elapsed() < Duration::from_secs(3),
            "要在到期后立刻返回，实耗 {:?}",
            started.elapsed()
        );
    }
}

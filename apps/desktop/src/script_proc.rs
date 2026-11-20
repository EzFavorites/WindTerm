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
use term_script::{ProcError, ProcOutput, ProcRun, Process};

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
        let output = match request.timeout {
            Some(limit) => wait_with_timeout(child, limit)?,
            // 不给超时 = 一直等（脚本作者显式要的语义）。
            None => child
                .wait_with_output()
                .map_err(|e| ProcError::Io(format!("等 {} 结束时失败：{e}", request.program)))?,
        };
        Ok(ProcOutput {
            code: output.status.code(),
            stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
        })
    }
}

/// 等子进程结束，最多等 `limit`；到期就 kill 并报超时。
///
/// 为什么轮询而不是「在另一个线程上 `wait` + `recv_timeout`」：`Child::wait` 要
/// `&mut self`，跨线程传一个 `Child` 再拿回它的输出是可行的，但那条路在超时
/// 分支上要处理「线程还攥着 Child」的收尾；轮询 20ms 一次足够（脚本对子进程的
/// 时延要求是「秒级」，不是「微秒级」），而且没有额外的线程要管。
fn wait_with_timeout(
    mut child: std::process::Child,
    limit: Duration,
) -> Result<std::process::Output, ProcError> {
    let started = std::time::Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(_)) => {
                return child
                    .wait_with_output()
                    .map_err(|e| ProcError::Io(format!("收回输出失败：{e}")))
            }
            Ok(None) => {
                if started.elapsed() >= limit {
                    let _ = child.kill();
                    // 收尸：不 wait 会留一个僵尸。
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

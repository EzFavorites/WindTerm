//! 会话字节通道。
//!
//! 把 PTY 的所有权从 `Tab` 搬到这里。UI 不再见到 `MasterPty` / `Child` /
//! `mpsc::Receiver`，只通过 `SupervisorHandle` 发命令、收快照。
//!
//! v0.1 保留既定协议策略：本地 shell 与系统 `ssh`/`nc` 都跑在同一个
//! pty 里，因此两个实现共享 `PtySession`，区别只在启动计划与描述。
//! **串口与 Telnet 是例外**：`Protocol::Serial` 走
//! [`serial_channel::SerialChannel`]，由本进程直接开设备（`cu` 那条外部进程的
//! 路子已经去掉），因为 RX/TX 计数、热插拔与 DTR/RTS 只有在进程内才拿得到；
//! `Protocol::Telnet` 走 [`telnet_channel::TelnetChannel`]，用 `TcpStream` 直连
//! 并把 `term_core::telnet` 的协商语义接上（系统 `telnet` 客户端只在
//! `spawn_plan()` 的 CLI / 无窗口兼容路径里保留）。两者失败都**不回退**系统客户端。
//! 未来接 `russh` + SFTP 时，只需给 `Ssh` 增加一个 `TermChannel` 实现，
//! `Supervisor` 与 UI 均零改动。
//!
//! 2026-11-12 底层收敛：**trait 本体已下沉到 [`term_core::channel`]**，本模块只做
//! 再导出并留下两个本地实现。理由写在 `term_core::channel` 的模块文档里——
//! 一句话是「传输层（`term-ssh`）不该为了实现这个 trait 而依赖带 PTY 与线程的
//! 运行时层」。`impl TermChannel for ...` 的写法和调用路径都没变。

use crate::error::SpawnError;
use crate::pty::PtySession;
use term_core::session::{Protocol, SessionConfig, SpawnPlan};

pub use term_core::channel::TermChannel;

/// 本地 shell：协议 `Shell`，跑 `$SHELL`。
pub struct LocalPty {
    inner: PtySession,
    description: String,
}

/// 外部进程：协议 `Ssh` / `Tcp`，拉系统客户端。
/// （`Serial` / `Telnet` 不在这里：它们各自有原生通道；`spawn_plan()` 中对应
/// 的系统客户端分支仍保留给 CLI / 无窗口路径。）
pub struct ExternalProc {
    inner: PtySession,
    description: String,
}

impl LocalPty {
    fn open(config: SessionConfig, cols: usize, rows: usize, plan: SpawnPlan) -> Result<Self, SpawnError> {
        let description = match &plan {
            SpawnPlan::ShellProgram(path) => format!("{path} (local pty)"),
            _ => "$SHELL (local pty)".to_string(),
        };
        let inner = PtySession::spawn_with(config, cols, rows, plan)?;
        Ok(Self { inner, description })
    }
}

impl ExternalProc {
    fn open(config: SessionConfig, cols: usize, rows: usize, plan: SpawnPlan) -> Result<Self, SpawnError> {
        let description = match &plan {
            SpawnPlan::External(argv) => argv.join(" "),
            SpawnPlan::LocalShell => "$SHELL".to_string(),
            SpawnPlan::ShellProgram(path) => path.clone(),
        };
        let inner = PtySession::spawn_with(config, cols, rows, plan)?;
        Ok(Self { inner, description })
    }
}

impl TermChannel for LocalPty {
    fn drain(&mut self) -> Vec<u8> {
        self.inner.drain()
    }
    fn write(&mut self, bytes: &[u8]) {
        self.inner.send(bytes)
    }
    fn resize(&mut self, cols: usize, rows: usize) {
        self.inner.resize(cols, rows)
    }
    fn poll(&mut self) -> Option<String> {
        self.inner.poll()
    }
    fn finished(&self) -> bool {
        self.inner.finished()
    }
    fn close(&mut self) {
        self.inner.kill()
    }
    fn description(&self) -> &str {
        &self.description
    }
}

impl TermChannel for ExternalProc {
    fn drain(&mut self) -> Vec<u8> {
        self.inner.drain()
    }
    fn write(&mut self, bytes: &[u8]) {
        self.inner.send(bytes)
    }
    fn resize(&mut self, cols: usize, rows: usize) {
        self.inner.resize(cols, rows)
    }
    fn poll(&mut self) -> Option<String> {
        self.inner.poll()
    }
    fn finished(&self) -> bool {
        self.inner.finished()
    }
    fn close(&mut self) {
        self.inner.kill()
    }
    fn description(&self) -> &str {
        &self.description
    }
}

/// 按会话配置打开合适的通道。
pub fn open_channel(
    config: SessionConfig,
    cols: usize,
    rows: usize,
) -> Result<Box<dyn TermChannel>, SpawnError> {
    // `spawn_plan()` 先跑：它给出的 `MissingHost`（设备名为空）是唯一的权威说法，
    // 串口分支也要这份校验；它同时算出来的 `cu` argv 在下面**不会**被用上。
    let plan = config.spawn_plan()?;
    // 串口不拉 `cu`：设备由本进程按 termios 直开，这样才有 RX/TX 计数、热插拔
    // 与 DTR/RTS。打开失败**不退回** `cu`——那会让人以为在用原生串口，实际
    // 还在拉外部进程，这种假绿比报错更糟。
    if config.protocol == Protocol::Serial {
        let params = crate::serial_channel::config_of(&config);
        let ch = crate::serial_channel::SerialChannel::open(&params)
            .map_err(|err| SpawnError::Serial(err.to_string()))?;
        return Ok(Box::new(ch));
    }
    // Telnet 也不拉系统客户端：由本进程直连 TCP，协商在进程内剥离，这样
    // 才拿得到干净的字节流与 RX/TX 计数。连不上就明确报错，不静默回退。
    if config.protocol == Protocol::Telnet {
        let ch = crate::telnet_channel::TelnetChannel::open(&config, cols, rows)
            .map_err(|err| SpawnError::Telnet(err.to_string()))?;
        return Ok(Box::new(ch));
    }
    match plan {
        // 显式 shell 程序仍是本地 PTY，只是启动的命令不同。
        SpawnPlan::LocalShell | SpawnPlan::ShellProgram(_) => {
            Ok(Box::new(LocalPty::open(config, cols, rows, plan)?))
        }
        SpawnPlan::External(_) => Ok(Box::new(ExternalProc::open(config, cols, rows, plan)?)),
    }
}

/// 无头运行器用：执行一次命令后退出，用于金样本之外的端到端冒烟测试。
pub fn open_exec_channel(
    command: &str,
    cols: usize,
    rows: usize,
) -> Result<Box<dyn TermChannel>, SpawnError> {
    let config = SessionConfig::new(Protocol::Shell);
    // 一条命令的解释器按平台选：unix 是 `/bin/sh -c`，Windows 是 `cmd /C`
    // （`cmd` 由 `CommandBuilder` 按 `PATH`/`%ComSpec%` 解析）。
    // 用 `cfg!` 而不是 `#[cfg]`：两种形态都只是一个 `&str`，没必要为此分叉整块代码。
    let (program, flag) = if cfg!(windows) {
        ("cmd", "/C")
    } else {
        ("/bin/sh", "-c")
    };
    let plan = SpawnPlan::External(vec![
        program.to_string(),
        flag.to_string(),
        command.to_string(),
    ]);
    Ok(Box::new(LocalPty::open(config, cols, rows, plan)?))
}

#[cfg(test)]
mod tests {
    use super::*;
    use term_core::session::ConfigError;

    #[test]
    fn shell_session_uses_the_local_plan() {
        let config = SessionConfig::new(Protocol::Shell);
        assert!(matches!(
            config.spawn_plan().unwrap(),
            SpawnPlan::LocalShell
        ));
    }

    #[test]
    fn a_serial_session_never_falls_back_to_cu() {
        // 关键回归：串口设备打不开时必须**报错**，不能悄悄拉起 `cu`。
        let mut config = SessionConfig::new(Protocol::Serial);
        config.host = "/dev/definitely-not-a-serial-port".to_string();
        let err = match open_channel(config, 80, 24) {
            Err(err) => err,
            Ok(_) => panic!("不存在的串口不该打开成功"),
        };
        assert!(matches!(err, SpawnError::Serial(_)), "{err:?}");
        let text = err.to_string();
        assert!(text.contains("/dev/definitely-not-a-serial-port"), "{text}");
    }

    #[test]
    fn a_serial_session_without_a_device_stays_a_config_error() {
        let err = match open_channel(SessionConfig::new(Protocol::Serial), 80, 24) {
            Err(err) => err,
            Ok(_) => panic!("没有设备路径不该打开成功"),
        };
        assert!(
            matches!(err, SpawnError::Config(ConfigError::MissingHost(Protocol::Serial))),
            "{err:?}"
        );
    }
}

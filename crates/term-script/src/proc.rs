//! 外部进程端口（ADR-0027 ④）：脚本起一个子进程并取回 stdout / stderr / 退出码。
//!
//! # 为什么必须带授权
//!
//! 「脚本能起进程」等于「脚本能做这台机器上的一切」——它可以起 `sh -c ...`
//! 绕过所有别的端口。所以这个端口只有一个能力位（`process`），缺省拒绝；
//! 没有授权时报的是 [`ProcError::Denied`]，**不是**「命令不存在」。
//!
//! # 为什么不给一个长期存活的子进程句柄
//!
//! 给句柄就要给「读它 stdout / 写它 stdin / 等它退出 / 杀它」四个动词，
//! 每个都要接取消与超时；而脚本里的真实需求九成是「跑一下、拿结果」。
//! 一次性的 [`Process::run`] 覆盖那九成，且只有一处阻塞点要接取消。

use std::fmt;
use std::time::Duration;

/// 跑什么。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProcRun {
    /// 可执行文件（**不经过 shell**：参数里的空格 / 通配符不会被解释）。
    pub program: String,
    pub args: Vec<String>,
    /// 工作目录；`None` = 继承宿主。
    pub cwd: Option<String>,
    /// 单次运行上限；`None` = 用宿主的缺省（30 秒）。
    pub timeout: Option<Duration>,
    /// 允许写进子进程 stdin 的字节（`None` = 不写、直接关掉）。
    pub stdin: Option<String>,
}

impl ProcRun {
    pub fn new(program: impl Into<String>) -> Self {
        Self {
            program: program.into(),
            args: Vec::new(),
            cwd: None,
            timeout: None,
            stdin: None,
        }
    }
}

/// 跑完的结果。`code = None` 表示被信号杀掉（Unix）或宿主报不出退出码。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProcOutput {
    pub code: Option<i32>,
    pub stdout: String,
    pub stderr: String,
}

/// 起不来的原因。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ProcError {
    /// 没有 `process` 授权。
    Denied(String),
    /// 宿主不提供进程能力（无头 / 未接线）。
    Unsupported(String),
    /// 被取消。
    Cancelled,
    /// 超过 `timeout`：进程已被杀掉，不会留在后台。
    Timeout,
    /// 起不来 / 写 stdin 失败（可执行文件不存在、权限不足……）。
    Io(String),
}

impl fmt::Display for ProcError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ProcError::Denied(why) => write!(f, "{why}"),
            ProcError::Unsupported(why) => write!(f, "当前宿主不能起外部进程：{why}"),
            ProcError::Cancelled => f.write_str("外部进程已被取消"),
            ProcError::Timeout => f.write_str("外部进程超时，已被杀掉"),
            ProcError::Io(why) => write!(f, "起不了外部进程：{why}"),
        }
    }
}

impl std::error::Error for ProcError {}

/// 外部进程端口。
pub trait Process: Send + Sync {
    fn run(&self, request: &ProcRun) -> Result<ProcOutput, ProcError>;
}

/// 未接线的宿主：`Unsupported`（不是「命令不存在」——那会让人去查 PATH）。
#[derive(Clone, Copy, Debug, Default)]
pub struct NoProcess;

impl Process for NoProcess {
    fn run(&self, _: &ProcRun) -> Result<ProcOutput, ProcError> {
        Err(ProcError::Unsupported(
            "没有接入外部进程端口".to_string(),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_unwired_host_reports_unsupported_not_a_missing_command() {
        let err = NoProcess.run(&ProcRun::new("uname")).unwrap_err();
        assert!(matches!(err, ProcError::Unsupported(_)));
        assert!(!err.to_string().contains("不存在"));
    }
}

//! 一次性 `exec` 会话的**同步门面**（W6-C3）。
//!
//! 与 `channel.rs::connect` 是同一个笼子：对外一个阻塞的 `Result`，异步只在
//! 本 crate 内一条专用线程里的 `current_thread` runtime 上跑。第一梯队消费者
//! （远端 Sysmon 的第二 exec 通道）拿到的就是 [`ExecOutput`]，不用碰 tokio。
//!
//! ```text
//! exec_capture(opts, cmd)                [调用方线程，同步]
//!    │  spawn 专用线程 + std::mpsc
//!    ▼
//! run_worker ── tokio::time::timeout(EXEC_TIMEOUT, capture(..))
//!    │
//!    ├── Authenticated::connect ──▶ opener().exec(cmd) ──▶ 裸 Channel
//!    └── channel.wait() 循环 ──▶ Data / ExtendedData / ExitStatus / Eof
//! ```
//!
//! 两个硬上限是这里最要紧的设计，缺一个就能把进程拖死：
//! - [`EXEC_MAX_OUTPUT_BYTES`]：`cat /dev/zero` 会无限吐字节，缓冲区必须有顶。
//!   触顶后**继续 drain 但不再 append**（`truncated = true`）——提前 break 会
//!   丢掉 `ExitStatus`，还会把服务端留在半开的通道上。
//! - [`EXEC_TIMEOUT`]：整次操作（建链 + exec + 收数据）的天花板。内层用
//!   `tokio::time::timeout` 兜住，外层 `recv_timeout` 再兜住「runtime 还没建起来
//!   就卡住」这种内层管不到的窗口。

use std::sync::mpsc::{self, RecvTimeoutError};
use std::thread::JoinHandle;
use std::time::Duration;

use russh::ChannelMsg;

use crate::authenticated::Authenticated;
use crate::error::SshError;
use crate::options::SshOptions;

/// stdout + stderr **合计**最多留多少字节：1 MiB。
///
/// 合起来算而不是各 1 MiB，是因为调用方通常会把两路都留在内存里；分开算等于
/// 把真实上限悄悄放大一倍。触顶后截断并置 `ExecOutput::truncated`，不返回错误——
/// 「命令跑完了但输出太长」和「连接断了」是两件事，不该混成一类。
pub const EXEC_MAX_OUTPUT_BYTES: usize = 1024 * 1024;

/// 整次 exec 的时间上限：30 秒。
///
/// 覆盖建链 + 请求 + 收数据全过程。`SshOptions::connect_timeout` 约束的是**每一段**
/// TCP/握手（默认 15 秒，N 跳代理链按跳数累加），这里是「不管几跳，总共别超过
/// 30 秒」的外层天花板。两者不冲突：先到者生效。
pub const EXEC_TIMEOUT: Duration = Duration::from_secs(30);

/// 外层硬兜底比内层多给的宽限：留给「线程调度 + 建 runtime + 交出结果」。
/// 内层 `tokio::time::timeout` 已经覆盖所有正常路径，这一层只防运行时之外的卡死。
const EXEC_JOIN_GRACE: Duration = Duration::from_secs(5);

/// 一次远端命令的全部可观察结果。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExecOutput {
    /// `ChannelMsg::Data`（stdout）。
    pub stdout: Vec<u8>,
    /// `ChannelMsg::ExtendedData`（RFC 4254 里 `ext = 1` 是 stderr）。
    pub stderr: Vec<u8>,
    /// 服务端报了 `ExitStatus` 才有值；被信号杀掉 / 连接被砍时是 `None`。
    pub exit_code: Option<u32>,
    /// 输出触到 [`EXEC_MAX_OUTPUT_BYTES`] 后被截断。为 `true` 时 `stdout`/`stderr`
    /// 是**前缀**，不是完整输出；调用方要据此决定要不要重跑或换分页命令。
    pub truncated: bool,
}

/// 同步跑一条远端命令，收全 stdout / stderr / 退出码。
///
/// 阻塞到命令结束、超时或出错为止。所有失败都是分类过的 [`SshError`]：
/// 配置不足 → `Config`，认证被拒 → `Auth`，服务端拒绝 exec → `Channel`，
/// 本机起不了线程/运行时 → `Local`。调用方不需要解析字符串。
pub fn exec_capture(opts: &SshOptions, command: &str) -> Result<ExecOutput, SshError> {
    // 先在本地把明显不合法的表单挡掉：不必为了「没填主机名」起一条线程。
    opts.validate()?;

    let owned_opts = opts.clone();
    let owned_command = command.to_string();
    let (tx, rx) = mpsc::channel::<Result<ExecOutput, SshError>>();

    let name = format!("term-ssh-exec-{}", owned_opts.host);
    let worker: JoinHandle<()> = match std::thread::Builder::new().name(name).spawn(move || {
        let result = run_worker(&owned_opts, &owned_command);
        // 外层已经因硬超时先走时，这里的 send 会失败；那是预期内的，忽略。
        let _ = tx.send(result);
    }) {
        Ok(handle) => handle,
        Err(e) => return Err(SshError::Local(format!("无法启动 exec 线程：{e}"))),
    };

    let hard_deadline = EXEC_TIMEOUT + EXEC_JOIN_GRACE;
    match rx.recv_timeout(hard_deadline) {
        Ok(result) => {
            // 结果已经到手，join 只是收掉线程句柄，不会再等网络。
            let _ = worker.join();
            result
        }
        Err(RecvTimeoutError::Disconnected) => {
            let _ = worker.join();
            Err(SshError::Local(
                "exec 线程在交出结果前退出（可能是平台不支持或内存不足）".to_string(),
            ))
        }
        Err(RecvTimeoutError::Timeout) => {
            // 不 join：那会重新变成无限阻塞。线程的 tokio::time::timeout 会自己
            // 到点收摊，rx 随本函数返回被丢掉，worker 的 send 失败后正常退出。
            Err(SshError::Channel(format!(
                "exec 超过硬上限 {hard_deadline:?} 仍未返回（{command}）"
            )))
        }
    }
}

/// 专用线程体：建 current_thread runtime，把整次 exec 包进时间上限。
fn run_worker(opts: &SshOptions, command: &str) -> Result<ExecOutput, SshError> {
    let runtime = match tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    {
        Ok(rt) => rt,
        Err(e) => return Err(SshError::Local(format!("无法创建异步运行时：{e}"))),
    };
    runtime.block_on(async move {
        match tokio::time::timeout(EXEC_TIMEOUT, capture(opts, command)).await {
            Ok(result) => result,
            Err(_) => Err(SshError::Channel(format!(
                "exec 在 {EXEC_TIMEOUT:?} 内没跑完（{command}）—— 已放弃等待"
            ))),
        }
    })
}

/// 一次完整的 exec：握手 → 开通道 → 收数据 → 主动挂断。
async fn capture(opts: &SshOptions, command: &str) -> Result<ExecOutput, SshError> {
    let auth = Authenticated::connect(opts).await?;
    let result = run_exec(&auth, command).await;
    // 不管成功失败都主动挂断：不发这一下，服务端会留一个半死的会话直到超时。
    auth.disconnect().await;
    result
}

/// 在裸 channel 上收 `ChannelMsg`，直到 EOF / Close。
async fn run_exec(auth: &Authenticated, command: &str) -> Result<ExecOutput, SshError> {
    let mut channel = auth.opener().exec(command).await?;

    let mut stdout: Vec<u8> = Vec::new();
    let mut stderr: Vec<u8> = Vec::new();
    let mut exit_code: Option<u32> = None;
    let mut truncated = false;

    loop {
        match channel.wait().await {
            Some(ChannelMsg::Data { data }) => {
                append_capped(&mut stdout, &mut stderr, &data, false, &mut truncated);
            }
            Some(ChannelMsg::ExtendedData { data, .. }) => {
                // ext=1 是 stderr（RFC 4254 §5.2）。其它扩展码也不该混进 stdout，
                // 一律归到 stderr：宁可多留，也不能把字节悄悄丢掉。
                append_capped(&mut stdout, &mut stderr, &data, true, &mut truncated);
            }
            Some(ChannelMsg::ExitStatus { exit_status }) => exit_code = Some(exit_status),
            Some(ChannelMsg::ExitSignal { .. }) => {
                // 被信号杀掉没有数字退出码，保持 None；调用方靠它区分「异常终止」。
            }
            Some(ChannelMsg::Failure) => {
                // want_reply=true 才有这条回执：服务端明确拒了这次 exec。
                return Err(SshError::Channel(format!("服务器拒绝执行 {command}")));
            }
            Some(ChannelMsg::Success) => {
                // want_reply=true 的受理回执，exec 已被接受，继续等数据。
            }
            Some(ChannelMsg::Eof) | Some(ChannelMsg::Close) => break,
            None => {
                // 通道在 EOF 之前就没了 = 连接被砍，不是「命令跑完了没输出」。
                return Err(SshError::Channel(
                    "exec 通道在收到 EOF 之前断开（连接被砍）".to_string(),
                ));
            }
            // ChannelMsg 是 #[non_exhaustive]：Open / XonXoff / WindowAdjusted /
            // OpenFailure 与客户端侧的请求变体都不该出现在收数据这一侧，忽略即可。
            _ => {}
        }
    }

    Ok(ExecOutput {
        stdout,
        stderr,
        exit_code,
        truncated,
    })
}

/// 把一段数据追加到目标流，但绝不越过 stdout+stderr 的合计预算。
///
/// 触顶后**只置标志、不 append，也不打断外层循环**——外层还得继续 drain 到
/// EOF，才能拿到 `ExitStatus` 并让服务端正常关通道。
fn append_capped(
    stdout: &mut Vec<u8>,
    stderr: &mut Vec<u8>,
    data: &[u8],
    to_stderr: bool,
    truncated: &mut bool,
) {
    let used = stdout.len() + stderr.len();
    let room = EXEC_MAX_OUTPUT_BYTES.saturating_sub(used);
    let take = data.len().min(room);
    if take > 0 {
        if to_stderr {
            stderr.extend_from_slice(&data[..take]);
        } else {
            stdout.extend_from_slice(&data[..take]);
        }
    }
    if take < data.len() {
        *truncated = true;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_two_limits_are_documented_values() {
        assert_eq!(EXEC_MAX_OUTPUT_BYTES, 1024 * 1024);
        assert_eq!(EXEC_TIMEOUT, Duration::from_secs(30));
        assert_eq!(EXEC_JOIN_GRACE, Duration::from_secs(5));
    }

    #[test]
    fn a_stream_crossing_the_budget_is_truncated_not_dropped() {
        let mut stdout = vec![b'a'; EXEC_MAX_OUTPUT_BYTES - 4];
        let mut stderr = Vec::new();
        let mut truncated = false;
        append_capped(
            &mut stdout,
            &mut stderr,
            b"0123456789",
            false,
            &mut truncated,
        );
        assert_eq!(stdout.len(), EXEC_MAX_OUTPUT_BYTES);
        assert!(truncated);
        assert_eq!(&stdout[EXEC_MAX_OUTPUT_BYTES - 4..], b"0123");
    }

    #[test]
    fn the_budget_is_shared_between_stdout_and_stderr() {
        let mut stdout = vec![b'a'; EXEC_MAX_OUTPUT_BYTES];
        let mut stderr = Vec::new();
        let mut truncated = false;
        append_capped(&mut stdout, &mut stderr, b"err", true, &mut truncated);
        assert!(truncated);
        assert!(stderr.is_empty(), "预算已用完，stderr 不该再往里塞");
    }

    #[test]
    fn data_fitting_exactly_into_the_budget_is_not_flagged() {
        let mut stdout = vec![b'a'; EXEC_MAX_OUTPUT_BYTES - 3];
        let mut stderr = Vec::new();
        let mut truncated = false;
        append_capped(&mut stdout, &mut stderr, b"end", true, &mut truncated);
        assert!(!truncated);
        assert_eq!(stderr, b"end".to_vec());
        assert_eq!(stdout.len() + stderr.len(), EXEC_MAX_OUTPUT_BYTES);
    }

    #[test]
    fn an_incomplete_option_fails_before_any_thread_starts() {
        let mut opts = SshOptions::new(
            "127.0.0.1",
            "tester",
            crate::options::AuthMethod::Password {
                password: "x".to_string(),
            },
        );
        opts.host.clear();
        match exec_capture(&opts, "echo hi") {
            Err(SshError::Config(_)) => {}
            other => panic!("应当是 Config 错误，实际是 {other:?}"),
        }
    }
}

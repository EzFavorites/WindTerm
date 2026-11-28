//! 触发器意图的**派发落点**（求值与写分离）。
//!
//! `Tab` 只把 `Effect` 翻译成这四种意图并排队；真正的副作用由窗口层消费：
//!
//! - [`IntentKind::Surface`] → `invoke`（会话控制面动词，PTY 写入的唯一入口）；
//! - [`IntentKind::App`] → AppControl / UI 帧队列（标记 / 链接 / 菜单 / 提示 / 命令）；
//! - [`IntentKind::Credential`] → 保险库取值后经 `invoke` 写 PTY；
//! - [`IntentKind::Script`] → 统一脚本入口（`ScriptRun`）。
//!
//! 写意图可带内核算出的 `Release` 门控（`Prompt` / `LineBreak` / `Later`），
//! 由帧末派发层排队到正确时机；其余三类不排队。
//!
//! 每条意图都带 [`DispatchCtx`]：`source` / `priority` / `owner` 与仲裁许可
//! 一并带走。落地前必须用 [`DispatchCtx::may_write_pty`] 复核许可——用户按键
//! 或独占抢占发生在排队之后时，旧许可已经失效。

use std::cell::RefCell;
use std::process::{Child as RunningChild, Command as ProcessCommand, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver as ReceiptRx, TryRecvError};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde_json::{json, Value};
use term_core::automation::{
    Action, AlertKind, AppAction, CredentialField, Effect, Invocation, Priority, Release, RuleId,
    Ticks,
};
use term_core::session_runtime::SessionId;

use term_core::automation::{Arbitrator, Grant};

/// 谁、以什么优先级、在哪个会话上请求写入。`owner` 是请求时刻的会话 owner。
#[derive(Clone, Debug, PartialEq, Eq)]
pub(in crate::ui) struct DispatchCtx {
    pub session: SessionId,
    /// 请求者身份（规则 id / `api:write` / `keyboard`）。
    pub source: String,
    pub priority: Priority,
    /// 请求时刻的 owner（可能不是自己）。仅用于提示，不用于判据。
    pub owner: Option<String>,
    /// 仲裁许可。没有许可的意图根本不产生 PTY 写。
    pub grant: Option<Grant>,
}

impl DispatchCtx {
    pub(in crate::ui) fn for_rule(
        session: SessionId,
        rule: &RuleId,
        owner: Option<String>,
        grant: Option<Grant>,
    ) -> Self {
        Self {
            session,
            source: rule.clone(),
            priority: Priority::P3Rule,
            owner,
            grant,
        }
    }

    /// PTY 写入闸门：许可必须在落地时依然有效（按键 / 独占抢占会让它失效）。
    pub(in crate::ui) fn may_write_pty(&self, arbiter: &Arbitrator) -> bool {
        self.grant
            .is_some_and(|grant| arbiter.is_valid(self.session, &grant))
    }

    /// 同帧排序键：数值越大越优先，同优先级按到达序号（不交错）。
    pub(in crate::ui) fn seq(&self) -> u64 {
        self.grant.map_or(u64::MAX, |grant| grant.seq)
    }
}

/// 会话控制面调用。`params` 形状已由 `Rule::validate` 按 `params_schema` 校验。
#[derive(Clone, Debug, PartialEq)]
pub(in crate::ui) struct SurfaceIntent {
    pub method: String,
    pub params: Value,
}

/// 应用面 / UI 帧队列意图。
#[derive(Clone, Debug, PartialEq)]
pub(in crate::ui) enum AppIntent {
    Mark {
        text_color: String,
        background: String,
        line_style: String,
        line_color: String,
    },
    Link(String),
    /// 菜单动作：**完整 Invocation**（method + params）一起交给命令通道。
    Menu(Vec<Invocation>),
    Tooltip(String),
    Alert(AlertKind),
    Dispatch(String),
}

/// 命令通道的一格：菜单 ID 可以没有参数，会话动词必须把 params 带来。
#[derive(Clone, Debug, PartialEq)]
pub(in crate::ui) struct CommandCall {
    pub method: String,
    pub params: Value,
}

impl CommandCall {
    pub(in crate::ui) fn plain(id: impl Into<String>) -> Self {
        Self {
            method: id.into(),
            params: json!({}),
        }
    }

    pub(in crate::ui) fn from_invocation(inv: Invocation) -> Self {
        Self {
            method: inv.method,
            params: inv.params,
        }
    }
}

impl From<String> for CommandCall {
    fn from(id: String) -> Self {
        Self::plain(id)
    }
}

/// 凭据意图：只带引用与**类型化字段**，明文在窗口层问保险库。
#[derive(Clone, Debug, PartialEq, Eq)]
pub(in crate::ui) struct CredentialIntent {
    pub scheme: String,
    pub field: CredentialField,
}

/// 脚本意图：路径 + 参数，授权按次由统一脚本入口决定。
#[derive(Clone, Debug, PartialEq, Eq)]
pub(in crate::ui) struct ScriptIntent {
    pub path: String,
    pub args: Vec<String>,
}

/// 一条命中产出的、待窗口层派发的意图。
///
/// `release` / `since` 只管会话写意图（见 [`FrameIntent::ready`]）：
/// 事件类与 UI 类动作不排队，命中当帧就走 `trigger_intents`。
#[derive(Clone, Debug, PartialEq)]
pub(in crate::ui) struct FrameIntent {
    pub ctx: DispatchCtx,
    pub kind: IntentKind,
    pub release: Release,
    /// 命中时刻（毫秒刻度），`Release::Later` 从这一刻起算。
    pub since: Ticks,
}

impl FrameIntent {
    /// 只有会话写动词受发送门控与仲裁排序；所有其它动作立即发生。
    pub(in crate::ui) fn is_write(&self) -> bool {
        matches!(&self.kind, IntentKind::Surface(call) if call.method == "write" || call.method == "keys")
    }

    /// 本帧是否该落地。等待提示符 / 换行只看这一帧的输出；
    /// `Later` 只看注入的时钟（与求值器同一时基，不读墙钟）。
    pub(in crate::ui) fn ready(&self, now: u64, output: Option<&str>) -> bool {
        if !self.is_write() {
            return true;
        }
        match self.release {
            Release::Immediate => true,
            Release::Prompt => output.is_some_and(term_app::trigger_builtins::looks_like_prompt),
            Release::LineBreak => output.is_some_and(|text| text.contains('\n')),
            Release::Later(delay) => {
                // `now` 是秒（与求值器同源），`since` 是 Ticks（毫秒）。
                let elapsed_ms = Ticks::from_secs(now).0.saturating_sub(self.since.0);
                Duration::from_millis(elapsed_ms) >= delay
            }
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub(in crate::ui) enum IntentKind {
    Surface(SurfaceIntent),
    App(AppIntent),
    Credential(CredentialIntent),
    Script(ScriptIntent),
}

/// `Effect` → 意图。`matched` 的文本与 span 不参与派发：动作只声明意图。
pub(in crate::ui) fn intent_from_effect(effect: &Effect, ctx: DispatchCtx) -> Option<FrameIntent> {
    let kind = match &effect.action {
        Action::SessionInvocation { method, params } => IntentKind::Surface(SurfaceIntent {
            method: method.clone(),
            params: params.clone(),
        }),
        Action::AppAction(app) => IntentKind::App(match app {
            AppAction::Mark(spec) => AppIntent::Mark {
                text_color: spec.text_color.clone(),
                background: spec.background.clone(),
                line_style: spec.line_style.clone(),
                line_color: spec.line_color.clone(),
            },
            AppAction::Link(spec) => AppIntent::Link(spec.url.clone()),
            AppAction::Menu(spec) => {
                AppIntent::Menu(spec.items.iter().filter_map(|item| item.action.clone()).collect())
            }
            AppAction::Tooltip(spec) => AppIntent::Tooltip(spec.text.clone()),
            AppAction::Alert(kind) => AppIntent::Alert(*kind),
            AppAction::Dispatch(id) => AppIntent::Dispatch(id.clone()),
        }),
        Action::CredentialRef { scheme, field } => IntentKind::Credential(CredentialIntent {
            scheme: scheme.clone(),
            field: *field,
        }),
        Action::RunScript { path, args } => IntentKind::Script(ScriptIntent {
            path: path.clone(),
            args: args.clone(),
        }),
    };
    Some(FrameIntent {
        ctx,
        kind,
        release: effect.release,
        since: Ticks::ZERO,
    })
}

// ── 触发器 Run Command：后台执行 + 每帧回执（appdoc P1-1） ──────────
//
// 旧实现在 UI 线程直接 `Command::output()`：一条卡住的外部命令能把整帧
// 拖死。现在 spawn 只登记一个句柄，真正的输出在后台线程等；UI 每帧只做
// `try_recv` + 超时的 `kill()`，两类动作都不会阻塞。

/// 单次后台命令的墙钟上限。到点由帧泵 kill，不等它自然退出。
pub(in crate::ui) const COMMAND_TIMEOUT: Duration = Duration::from_secs(10);
/// 工作线程检查子进程退出的切片（UI 帧泵可以在这之间拿到锁去 kill）。
const COMMAND_POLL_SLICE: Duration = Duration::from_millis(25);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::ui) enum DispatchTarget {
    /// 菜单 ID：解析成强类型 `Action` 后走 reducer。
    Action,
    /// 自由文本命令行：交给后台执行器。
    Command,
    /// 形状像命令 ID 但没登记（或空串）：显式报错，绝不降级成 shell。
    Unknown,
}

/// 是不是「`Category.Name`」这种命令 ID 的形状（无空格 / 参数）。
pub(in crate::ui) fn is_action_id_shaped(text: &str) -> bool {
    !text.is_empty()
        && text.contains('.')
        && !text.chars().any(char::is_whitespace)
        && text
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'))
}

/// 分发前先分类：**只有自由文本才是命令行**。看起来像 ID 却查不到的
/// 字符串是配置 / 输入错误，调用方必须看见一条显式错误。
pub(in crate::ui) fn classify_target(id: &str) -> DispatchTarget {
    if term_app::action::Action::parse(id).is_some() {
        DispatchTarget::Action
    } else if id.trim().is_empty() || is_action_id_shaped(id) {
        DispatchTarget::Unknown
    } else {
        DispatchTarget::Command
    }
}

#[derive(Debug)]
pub(in crate::ui) enum CommandOutcome {
    Finished {
        stdout: Vec<u8>,
        stderr: String,
        success: bool,
        exit: String,
    },
    SpawnFailed(String),
}

/// 一帧 poll 收回的一次执行结果。
pub(in crate::ui) struct FinishedCommand {
    command: String,
    outcome: CommandOutcome,
    timed_out: bool,
}

impl FinishedCommand {
    /// 成功且有输出时，把 stdout 写回活动会话；其余情况没有可写字节。
    pub(in crate::ui) fn stdout(&self) -> &[u8] {
        match &self.outcome {
            CommandOutcome::Finished { stdout, success: true, .. } if !self.timed_out => stdout,
            _ => &[],
        }
    }

    /// 只有失败 / 超时 / 起不来才有提示；成功且无输出不打扰用户。
    pub(in crate::ui) fn note(&self) -> Option<String> {
        if self.timed_out {
            return Some(format!(
                "{}：执行超过 {} 秒，已 kill",
                self.command, COMMAND_TIMEOUT.as_secs()
            ));
        }
        match &self.outcome {
            CommandOutcome::SpawnFailed(why) => Some(format!("{}：起不来（{why}）", self.command)),
            CommandOutcome::Finished { success: true, .. } => None,
            CommandOutcome::Finished { stderr, exit, .. } => Some(format!(
                "{}：失败（{exit}）{}",
                self.command,
                if stderr.is_empty() { String::new() } else { format!("：{stderr}") }
            )),
        }
    }
}

struct RunningCommand {
    command: String,
    started: Instant,
    timeout: Duration,
    rx: ReceiptRx<CommandOutcome>,
    child: Arc<Mutex<Option<RunningChild>>>,
    killed: Arc<AtomicBool>,
}

// 注册表按**派发线程**隔离：生产里登记（dispatch）与回收（每帧 drain）
// 都在 UI 线程，后台线程只经 Arc 句柄持有子进程。这样既没有 UI 线程上的
// 全局锁争用，测试里各自线程的运行也不会互相污染回执。
std::thread_local! {
    static RUNNING: RefCell<Vec<RunningCommand>> = const { RefCell::new(Vec::new()) };
}

/// 登记一次后台执行。spawn 本身很快；慢的是等子进程，那部分在后台线程。
pub(in crate::ui) fn spawn_command(command: &str) -> Result<(), String> {
    spawn_command_with_timeout(command, COMMAND_TIMEOUT)
}

fn spawn_command_with_timeout(command: &str, timeout: Duration) -> Result<(), String> {
    let command = command.trim().to_owned();
    if command.is_empty() {
        return Err("触发器命令行为空".to_owned());
    }
    let (tx, rx) = mpsc::channel::<CommandOutcome>();
    let child: Arc<Mutex<Option<RunningChild>>> = Arc::new(Mutex::new(None));
    let killed = Arc::new(AtomicBool::new(false));
    let worker_child = Arc::clone(&child);
    let worker_command = command.clone();
    std::thread::spawn(move || {
        #[cfg(windows)]
        let spawned = ProcessCommand::new("cmd")
            .args(["/C", &worker_command])
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn();
        #[cfg(not(windows))]
        let spawned = ProcessCommand::new("/bin/sh")
            .args(["-c", &worker_command])
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn();
        let process = match spawned {
            Ok(process) => process,
            Err(err) => {
                let _ = tx.send(CommandOutcome::SpawnFailed(err.to_string()));
                return;
            }
        };
        {
            let mut slot = worker_child.lock().unwrap_or_else(|e| e.into_inner());
            *slot = Some(process);
        }
        loop {
            std::thread::sleep(COMMAND_POLL_SLICE);
            let mut slot = worker_child.lock().unwrap_or_else(|e| e.into_inner());
            let done = slot.as_mut().is_some_and(|p| p.try_wait().unwrap_or(None).is_some());
            if !done {
                continue;
            }
            let process = match slot.take() {
                Some(process) => process,
                None => return,
            };
            drop(slot);
            let outcome = match process.wait_with_output() {
                Ok(out) => CommandOutcome::Finished {
                    stdout: out.stdout,
                    stderr: String::from_utf8_lossy(&out.stderr).trim().to_owned(),
                    success: out.status.success(),
                    exit: out.status.to_string(),
                },
                Err(err) => CommandOutcome::SpawnFailed(err.to_string()),
            };
            let _ = tx.send(outcome);
            return;
        }
    });
    RUNNING.with(|runs| {
        runs.borrow_mut().push(RunningCommand {
            command,
            started: Instant::now(),
            timeout,
            rx,
            child,
            killed,
        });
    });
    Ok(())
}

/// 每帧调用：收齐已结束的回执；到墙钟上限就 kill 子进程（UI 线程不做 wait）。
pub(in crate::ui) fn poll_commands() -> Vec<FinishedCommand> {
    RUNNING.with(|registry| {
        let mut runs = registry.borrow_mut();
        let mut finished = Vec::new();
        let mut i = 0;
        while i < runs.len() {
            let run = &runs[i];
            let mut timed_out = run.killed.load(Ordering::SeqCst);
            if !timed_out && run.started.elapsed() >= run.timeout {
                timed_out = true;
                if let Some(process) = run.child.lock().unwrap_or_else(|e| e.into_inner()).as_mut() {
                    let _ = process.kill();
                }
                run.killed.store(true, Ordering::SeqCst);
            }
            let outcome = match run.rx.try_recv() {
                Ok(outcome) => Some(outcome),
                // 执行线程没了也要显式报，不静默当作「跑成功了」。
                Err(TryRecvError::Disconnected) => {
                    Some(CommandOutcome::SpawnFailed("执行线程提前退出".to_owned()))
                }
                Err(TryRecvError::Empty) => None,
            };
            if let Some(outcome) = outcome {
                let run = runs.remove(i);
                finished.push(FinishedCommand {
                    command: run.command,
                    outcome,
                    timed_out,
                });
                continue;
            }
            i += 1;
        }
        finished
    })
}

#[cfg(test)]
mod command_tests {
    use super::*;

    fn wait_for_finished(command: &str) -> FinishedCommand {
        for _ in 0..200 {
            if let Some(done) = poll_commands().into_iter().find(|d| d.command == command) {
                return done;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        panic!("后台命令 {command:?} 没有在 2 秒内回执");
    }

    #[test]
    fn an_unknown_action_id_is_never_silently_run_as_shell() {
        let id = term_app::menu::context_ids::LINE_ENDINGS;
        assert_eq!(classify_target(id), DispatchTarget::Action);
        let unknown_id = ["Window", "NoSuchCommand"].join(".");
        assert_eq!(classify_target(&unknown_id), DispatchTarget::Unknown);
        assert_eq!(classify_target(""), DispatchTarget::Unknown);
        assert_eq!(classify_target("printf bg-ok"), DispatchTarget::Command);
        assert!(is_action_id_shaped(&unknown_id));
        assert!(!is_action_id_shaped("printf bg-ok"));
    }

    #[test]
    fn a_background_command_reports_stdout_on_a_later_poll() {
        assert!(spawn_command("printf bg-ok").is_ok());
        let done = wait_for_finished("printf bg-ok");
        assert!(!done.timed_out);
        assert_eq!(done.stdout(), b"bg-ok");
        assert!(done.note().is_none());
    }

    #[test]
    fn a_command_past_its_deadline_is_killed_and_reported() {
        assert!(spawn_command_with_timeout("sleep 30", Duration::from_millis(150)).is_ok());
        let done = wait_for_finished("sleep 30");
        assert!(done.timed_out, "超时必须被帧泵标记并 kill");
        assert!(
            done.note().is_some_and(|n| n.contains("kill")),
            "超时不能静默：{}",
            done.note().unwrap_or_default()
        );
        assert!(done.stdout().is_empty());
    }
}

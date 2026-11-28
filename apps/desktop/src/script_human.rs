//! 脚本的**人机端口**：把 `ask` / `notify` / 文件框接到界面或标准输入。
//!
//! # 两条实现，按入口选
//!
//! - [`StdinHuman`]：夹具 / 单测用的 stdin 端口。提示写 stderr、回答读一行
//!   标准输入。**提示绝不进 stdout**——那里是脚本的返回值。
//! - [`UiHuman`]：界面里的脚本。脚本线程把问题塞进 [`AskQueue`]，界面每帧取
//!   一个显示成模态框，用户点完把答案送回队列；脚本线程在 `recv` 上等。
//!
//! # 为什么界面那条要走队列而不是直接在脚本线程里 `egui` 开窗
//!
//! 脚本跑在**自己的线程**上（`script_run` 的 `desk-script`），而 egui 的
//! `Context` 只能在 UI 线程里推帧。跨线程「直接开一个窗口」不存在；能做的
//! 只是「把请求交出去、等一个答案」。队列 + 一进一出通道正好是这个形状，
//! 而且天然支持「脚本连问三个问题」——界面处理完第一个，第二个就浮上来。
//!
//! # 超时
//!
//! 界面那条没有超时：用户看得到框、想答多久是他的自由。但脚本被取消时
//! （界面「停止」）等在 `recv` 上的线程必须醒——[`UiHuman`] 用 100ms 的
//! `recv_timeout` 轮询 [`term_script::CancelToken`]，醒来就报 `Cancelled`。

use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{Receiver, SyncSender};
use std::sync::{Arc, Mutex};

use term_script::{Answer, Ask, AskError, CancelToken, HumanChannel, PickFile};

use crate::script_env::Envelope;

// 回信句柄与界面侧的请求视图：通道端留在 apps 层，ui/ 只拿到不透明句柄。
#[path = "script_human_reply.rs"]
mod reply;
pub use reply::{AskReply, AskRequest, PickRequest};

/// 脚本交给界面的一个问题：请求 + 回信的通道。
///
/// 回信通道是 `SyncSender`（容量 1）：脚本只会等一个答案，界面也只会给一个。
/// 有界容量让「界面回了两次」（双击按钮之类的 bug）在第二发上失败，而不是
/// 悄悄把第二个答案留在队列里喂给下一个问题。
pub struct PendingAsk {
    /// 队列内唯一序号：取消时据此只摘自己那一条。
    pub seq: u64,
    /// 所属运行的取消令牌：界面取之前先看一眼，取消的直接丢。
    pub cancel: CancelToken,
    pub request: Ask,
    /// 回信通道带 `Result`：真实答案是 `Ok`，取消（Esc / 关窗 / Cancel）是
    /// `Err(NoInput)`。取消绝不能伪装成 `Answer::Bool(false)` 送过去——那样
    /// Confirm 的 Esc 与「用户点了 No」无法区分，Text/Secret/Choose 还会收到
    /// 类型错误的 bool。
    pub reply: SyncSender<Result<Answer, AskError>>,
}

impl PendingAsk {
    pub fn is_stale(&self) -> bool {
        self.cancel.is_cancelled()
    }
}

/// 界面与脚本线程之间的问题队列。
///
/// 界面每帧 `pop()` 一个（已经有一个在显示时就不 pop），显示完把答案经
/// `PendingAsk::reply` 送回。队列为空 = 没有脚本在等人。
pub type AskQueue = Arc<Mutex<Vec<PendingAsk>>>;

/// 脚本交给界面的一个文件框请求。
pub struct PendingPick {
    pub seq: u64,
    pub cancel: CancelToken,
    pub request: PickFile,
    pub reply: SyncSender<Option<PathBuf>>,
}

impl PendingPick {
    pub fn is_stale(&self) -> bool {
        self.cancel.is_cancelled()
    }
}

/// 每条请求一个号（与 `script_app` 各自的计数器即可，队列互不混用）。
static NEXT_SEQ: AtomicU64 = AtomicU64::new(1);
fn next_seq() -> u64 {
    NEXT_SEQ.fetch_add(1, Ordering::Relaxed)
}

fn drop_own_ask(queue: &AskQueue, seq: u64) {
    let mut guard = match queue.lock() {
        Ok(guard) => guard,
        Err(poisoned) => poisoned.into_inner(),
    };
    guard.retain(|pending| pending.seq != seq);
}

fn drop_own_pick(queue: &Arc<Mutex<Vec<PendingPick>>>, seq: u64) {
    let mut guard = match queue.lock() {
        Ok(guard) => guard,
        Err(poisoned) => poisoned.into_inner(),
    };
    guard.retain(|pending| pending.seq != seq);
}

/// 界面那条路的人机端口。
pub struct UiHuman {
    queue: AskQueue,
    /// 文件框的落点（界面同一套队列，多一个变体）。
    picks: Arc<Mutex<Vec<PendingPick>>>,
    /// 授权信封：用户手选的路径要**当场**加进来（ADR-0027）。
    caps: Option<Envelope>,
    /// `notify` 的落点（界面每帧取走显示在状态栏）。`None` = 写 stderr。
    notes: Option<Arc<Mutex<Vec<String>>>>,
    cancel: CancelToken,
}

impl UiHuman {
    pub fn new(
        queue: AskQueue,
        picks: Arc<Mutex<Vec<PendingPick>>>,
        caps: Option<Envelope>,
        notes: Option<Arc<Mutex<Vec<String>>>>,
        cancel: CancelToken,
    ) -> Self {
        Self {
            queue,
            picks,
            caps,
            notes,
            cancel,
        }
    }
}

impl HumanChannel for UiHuman {
    fn notify(&self, message: &str) {
        match &self.notes {
            Some(notes) => push_note(notes, message),
            // 没接线就写 stderr：一条没能显示的提示至少留在日志里。
            None => eprintln!("[script] {message}"),
        }
    }

    fn ask(&self, request: &Ask) -> Result<Answer, AskError> {
        let seq = next_seq();
        let (tx, rx) = std::sync::mpsc::sync_channel(1);
        // 队列中毒只说明别的线程持锁时崩过；问题本身还能排进去。
        match self.queue.lock() {
            Ok(mut queue) => queue.push(PendingAsk {
                seq,
                cancel: self.cancel.clone(),
                request: request.clone(),
                reply: tx,
            }),
            Err(poisoned) => poisoned.into_inner().push(PendingAsk {
                seq,
                cancel: self.cancel.clone(),
                request: request.clone(),
                reply: tx,
            }),
        }
        // 界面回 `Err(NoInput)` = 用户取消；`Ok(answer)` 才是真实回答。
        // 两种错误（取消 / 断线）都按「没人回答」处理，branch 一致。
        let answer = self.wait_for_reply(&rx).and_then(|reply| reply);
        if answer.is_err() {
            // 取消 / 断线时脚本已经走了；问题若不摘掉，下一帧还会弹出来。
            drop_own_ask(&self.queue, seq);
        }
        answer
    }

    fn pick_file(&self, request: &PickFile) -> Result<Option<PathBuf>, AskError> {
        let seq = next_seq();
        let (tx, rx) = std::sync::mpsc::sync_channel(1);
        match self.picks.lock() {
            Ok(mut queue) => queue.push(PendingPick {
                seq,
                cancel: self.cancel.clone(),
                request: request.clone(),
                reply: tx,
            }),
            Err(poisoned) => poisoned.into_inner().push(PendingPick {
                seq,
                cancel: self.cancel.clone(),
                request: request.clone(),
                reply: tx,
            }),
        }
        let answer = self.wait_for_reply(&rx);
        if answer.is_err() {
            drop_own_pick(&self.picks, seq);
        }
        match answer {
            // 用户点了「取消」→ `None`（不是错误：CRT 的文件框取消也是空串）。
            Ok(picked) => {
                // 用户**亲手选了**这个路径 = 他同意脚本碰它。授权在这里追加，
                // 而且要求与拒绝的理由一致：读框授读、存框授写。
                if let (Some(caps), Some(path)) = (&self.caps, &picked) {
                    let request = match request {
                        PickFile::Open { .. } => {
                            term_core::script_caps::CapRequest::FsRead(path.clone())
                        }
                        PickFile::Save { .. } => {
                            term_core::script_caps::CapRequest::FsWrite(path.clone())
                        }
                    };
                    crate::script_env::grant(caps, &request);
                }
                Ok(picked)
            }
            Err(e) => Err(e),
        }
    }
}

impl UiHuman {
    /// 等界面回话，同时盯着取消旗子。
    fn wait_for_reply<T>(&self, rx: &Receiver<T>) -> Result<T, AskError> {
        loop {
            if self.cancel.is_cancelled() {
                return Err(AskError::Cancelled);
            }
            match rx.recv_timeout(std::time::Duration::from_millis(100)) {
                Ok(value) => return Ok(value),
                Err(std::sync::mpsc::RecvTimeoutError::Timeout) => continue,
                // 界面把通道丢了（窗口关了 / 模态被强行收掉）：说「没有输入」，
                // 让脚本走它的「没人回答」分支，而不是永远等下去。
                Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
                    return Err(AskError::NoInput)
                }
            }
        }
    }
}

/// 有 `notes` 队列但没有界面的人在听：`notify` 进队列、`ask` 报 NoInput。
///
/// 用在哪：界面路径没接线（`RunOptions` 只带 `opened`）时，脚本的 `notify`
/// 仍然要进 `ScriptRun::notes`——否则「运行脚本」窗口里的提示会跑到 stderr，
/// 而用户根本看不到 stderr（那是 GUI 进程）。
pub struct NotesHuman {
    notes: Arc<Mutex<Vec<String>>>,
}

impl NotesHuman {
    pub fn new(notes: Arc<Mutex<Vec<String>>>) -> Self {
        Self { notes }
    }
}

impl HumanChannel for NotesHuman {
    fn notify(&self, message: &str) {
        push_note(&self.notes, message);
    }
}

/// 往 `notes` 队列追加一条（中毒了就取出内层继续用）。
fn push_note(notes: &Arc<Mutex<Vec<String>>>, message: &str) {
    match notes.lock() {
        Ok(mut queue) => queue.push(message.to_string()),
        Err(poisoned) => poisoned.into_inner().push(message.to_string()),
    }
}

/// 夹具 / 单测用的 stdin 人机端口：stderr 问、stdin 答。
pub struct StdinHuman {
    /// `false` = 不读标准输入（脚本源码本身就来自 stdin，再读只会拿到 EOF）。
    input: bool,
}

impl StdinHuman {
    pub fn new() -> Self {
        Self { input: true }
    }

    /// 关掉输入来源（脚本源码本身占用 stdin 时）。
    pub fn without_input(mut self) -> Self {
        self.input = false;
        self
    }

    /// 标准输入现在可不可以用来问人：开关开着、而且它真是终端。
    ///
    /// 被重定向（`< /dev/null`、管道）时不能问：读到的会是 EOF 或别人喂的
    /// 数据，而脚本会把「空行」当成「用户按了回车」——「要不要继续」那一支
    /// 就会静默走成「继续」。
    fn interactive(&self) -> bool {
        use std::io::IsTerminal;
        self.input && std::io::stdin().is_terminal()
    }

    /// 提示写 stderr、读一行、去掉行尾换行。
    fn read_line(&self, prompt: &str, secret: bool) -> Result<String, AskError> {
        use std::io::{BufRead, Write};
        if !self.interactive() {
            return Err(AskError::NoInput);
        }
        if secret {
            // 不关回显 = 把密码打进终端历史与肩窥范围。这里直接拒绝做半套：
            // 「看似隐藏其实没隐藏」比明说不支持更糟。
            return Err(AskError::Unsupported(
                "stdin 通道无法隐藏输入，请改在前台界面里运行脚本（或在脚本里用 profile/密钥）"
                    .to_string(),
            ));
        }
        eprint!("{prompt} ");
        std::io::stderr().flush().map_err(|e| AskError::BadRequest(e.to_string()))?;
        let mut line = String::new();
        let read = std::io::stdin()
            .lock()
            .read_line(&mut line)
            .map_err(|e| AskError::BadRequest(e.to_string()))?;
        if read == 0 {
            return Err(AskError::NoInput);
        }
        Ok(line.trim_end_matches(['\r', '\n']).to_string())
    }
}

impl Default for StdinHuman {
    fn default() -> Self {
        Self::new()
    }
}

impl HumanChannel for StdinHuman {
    fn notify(&self, message: &str) {
        eprintln!("[script] {message}");
    }

    fn ask(&self, request: &Ask) -> Result<Answer, AskError> {
        match request {
            Ask::Alert { title, text } => {
                self.read_line(&format!("{title} {text}（回车继续）"), false)?;
                Ok(Answer::Ack)
            }
            Ask::Confirm { title, text } => {
                let answer = self.read_line(&format!("{title} {text} [y/N]"), false)?;
                Ok(Answer::Bool(matches!(
                    answer.trim().to_ascii_lowercase().as_str(),
                    "y" | "yes"
                )))
            }
            Ask::Text {
                title,
                prompt,
                default,
            } => {
                let shown = if default.is_empty() {
                    format!("{title} {prompt}")
                } else {
                    format!("{title} {prompt} [{default}]")
                };
                let answer = self.read_line(&shown, false)?;
                // 空回答 = 采纳缺省值（CRT 的 `Prompt` 同语义）。
                Ok(Answer::Text(if answer.is_empty() {
                    default.clone()
                } else {
                    answer
                }))
            }
            // 密码类输入直接拒绝：stdin 没有不回显的读法，而「看似隐藏其实
            // 没隐藏」比明说不支持更糟。
            Ask::Secret { title, prompt } => {
                self.read_line(&format!("{title} {prompt}"), true).map(Answer::Text)
            }
            Ask::Choose {
                title,
                prompt,
                options,
            } => {
                if options.is_empty() {
                    return Err(AskError::BadRequest("选项列表是空的".to_string()));
                }
                let listing = options
                    .iter()
                    .enumerate()
                    .map(|(i, option)| format!("{}. {option}", i + 1))
                    .collect::<Vec<_>>()
                    .join("\n");
                let answer = self.read_line(&format!("{title} {prompt}\n{listing}\n选择"), false)?;
                // 接受编号（1 起，与列表一致），也接受选项原文。
                let picked = match answer.trim().parse::<usize>() {
                    Ok(n) if (1..=options.len()).contains(&n) => Some(n - 1),
                    _ => options.iter().position(|option| option == answer.trim()),
                };
                picked.map(Answer::Choice).ok_or_else(|| {
                    AskError::BadRequest(format!("{answer:?} 既不是编号也不是选项"))
                })
            }
            Ask::Message {
                title,
                text,
                buttons,
                ..
            } => {
                // stdin 没有按钮：用户输入具体按钮标签就能精确选中（三按钮组的
                // abort/retry/ignore 也能各自映射到 CRT ID）；仍兼容 y/N 简写。
                let hint = if buttons == &term_script::MessageButton::Ok {
                    String::new()
                } else {
                    format!(" [{}]", buttons.labels().join("/"))
                };
                let answer = self.read_line(&format!("{title} {text}{hint}"), false)?;
                if let Some(label) = buttons
                    .labels()
                    .iter()
                    .find(|label| label.eq_ignore_ascii_case(answer.trim()))
                {
                    return Ok(Answer::Text((*label).to_string()));
                }
                if buttons == &term_script::MessageButton::Ok {
                    return Ok(Answer::Ack);
                }
                Ok(Answer::Bool(matches!(
                    answer.trim().to_ascii_lowercase().as_str(),
                    "y" | "yes"
                )))
            }
        }
    }

    fn pick_file(&self, _request: &PickFile) -> Result<Option<PathBuf>, AskError> {
        Err(AskError::Unsupported(
            "当前脚本运行通道没有文件选择框；请在脚本面板 ▸ Permissions 勾选本次 fs 授权后用 fs_*".to_string(),
        ))
    }
}

#[cfg(test)]
#[path = "script_human_tests.rs"]
mod tests;

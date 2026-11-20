//! 脚本的**人机端口**：把 `ask` / `notify` / 文件框接到界面或标准输入。
//!
//! # 两条实现，按入口选
//!
//! - [`StdinHuman`]：进程级（`desk script run`）。提示写 stderr、回答读一行
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
use std::sync::mpsc::{Receiver, SyncSender};
use std::sync::{Arc, Mutex};

use term_script::{Answer, Ask, AskError, CancelToken, HumanChannel, PickFile};

use crate::script_env::Envelope;

/// 脚本交给界面的一个问题：请求 + 回信的通道。
///
/// 回信通道是 `SyncSender`（容量 1）：脚本只会等一个答案，界面也只会给一个。
/// 有界容量让「界面回了两次」（双击按钮之类的 bug）在第二发上失败，而不是
/// 悄悄把第二个答案留在队列里喂给下一个问题。
pub struct PendingAsk {
    pub request: Ask,
    pub reply: SyncSender<Answer>,
}

/// 界面与脚本线程之间的问题队列。
///
/// 界面每帧 `pop()` 一个（已经有一个在显示时就不 pop），显示完把答案经
/// `PendingAsk::reply` 送回。队列为空 = 没有脚本在等人。
pub type AskQueue = Arc<Mutex<Vec<PendingAsk>>>;

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

/// 脚本交给界面的一个文件框请求。
pub struct PendingPick {
    pub request: PickFile,
    pub reply: SyncSender<Option<PathBuf>>,
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
        let (tx, rx) = std::sync::mpsc::sync_channel(1);
        // 队列中毒只说明别的线程持锁时崩过；问题本身还能排进去。
        match self.queue.lock() {
            Ok(mut queue) => queue.push(PendingAsk {
                request: request.clone(),
                reply: tx,
            }),
            Err(poisoned) => poisoned.into_inner().push(PendingAsk {
                request: request.clone(),
                reply: tx,
            }),
        }
        self.wait_for_reply(&rx)
    }

    fn pick_file(&self, request: &PickFile) -> Result<Option<PathBuf>, AskError> {
        let (tx, rx) = std::sync::mpsc::sync_channel(1);
        match self.picks.lock() {
            Ok(mut queue) => queue.push(PendingPick {
                request: request.clone(),
                reply: tx,
            }),
            Err(poisoned) => poisoned.into_inner().push(PendingPick {
                request: request.clone(),
                reply: tx,
            }),
        }
        match self.wait_for_reply(&rx) {
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

/// 进程级（`desk script run`）的人机端口：stderr 问、stdin 答。
pub struct StdinHuman {
    /// `false` = 不读标准输入（脚本源码本身就来自 stdin，再读只会拿到 EOF）。
    input: bool,
}

impl StdinHuman {
    pub fn new() -> Self {
        Self { input: true }
    }

    /// 关掉输入来源（`desk script run -`）。
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
                "进程级运行无法隐藏输入，请改在界面里跑（或在脚本里用 profile/密钥）"
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
            // 密码类输入直接拒绝：进程级没有不回显的读法，而「看似隐藏其实
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
                // 进程级没有按钮，用「回车 = 默认按钮 / y = 是 / n = 否」顶上，
                // 语义与界面里的按钮一致。只有 OK 的形态回车就完了。
                if buttons == &term_script::MessageButton::Ok {
                    self.read_line(&format!("{title} {text}"), false)?;
                    return Ok(Answer::Ack);
                }
                let answer = self.read_line(&format!("{title} {text} [y/N]"), false)?;
                Ok(Answer::Bool(matches!(
                    answer.trim().to_ascii_lowercase().as_str(),
                    "y" | "yes"
                )))
            }
        }
    }

    fn pick_file(&self, _request: &PickFile) -> Result<Option<PathBuf>, AskError> {
        Err(AskError::Unsupported(
            "进程级运行没有文件选择框；请在脚本里写死路径，或加 --allow fs:… 授权后用 fs_*".to_string(),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 进程级没有终端时**不猜答案**：`NoInput` 而不是空串。
    #[test]
    fn the_stdin_channel_refuses_when_it_is_not_a_terminal() {
        use std::io::IsTerminal;
        // 这一条要钉的是「stdin 被重定向」那条分支，而 stdin 到底是不是终端
        // 由**跑测试的环境**决定，不是由这条测试决定。`cargo test` 在终端里
        // （`scripts/verify.sh` 第 103 行就是裸跑，没重定向）会继承那个终端，
        // 于是 `read_line` 一直等人敲字——实测挂死 18 分钟，门禁看起来是「卡住」
        // 而不是「红了」。CI 与 `cargo test < /dev/null` 下 stdin 是管道，正常断言。
        if std::io::stdin().is_terminal() {
            eprintln!("skip: stdin is a terminal; redirect it to exercise this branch");
            return;
        }
        let human = StdinHuman::new();
        assert_eq!(
            human.ask(&Ask::Confirm {
                title: "t".into(),
                text: "继续？".into()
            }),
            Err(AskError::NoInput)
        );
    }

    /// 关掉输入来源之后，即使有终端也不许问。
    #[test]
    fn without_input_closes_the_door_even_on_a_terminal() {
        let human = StdinHuman::new().without_input();
        assert!(!human.interactive());
        assert_eq!(human.pick_file(&PickFile::Open {
            title: "t".into(),
            filter: String::new(),
            default: String::new(),
        }), Err(AskError::Unsupported(
            "进程级运行没有文件选择框；请在脚本里写死路径，或加 --allow fs:… 授权后用 fs_*".to_string()
        )));
    }

    /// 界面那条：问题进队列、答案回到脚本线程；文件框同理。
    #[test]
    fn the_ui_channel_round_trips_through_the_queue() {
        let queue: AskQueue = Default::default();
        let picks = Arc::new(Mutex::new(Vec::new()));
        let human = Arc::new(UiHuman::new(
            Arc::clone(&queue),
            Arc::clone(&picks),
            None,
            None,
            CancelToken::new(),
        ));

        // ① 文本问题。
        let asker = {
            let human = Arc::clone(&human);
            std::thread::spawn(move || {
                human.ask(&Ask::Text {
                    title: String::new(),
                    prompt: "名字？".into(),
                    default: String::new(),
                })
            })
        };
        let pending = pop_until(&queue);
        assert!(matches!(pending.request, Ask::Text { .. }));
        pending.reply.send(Answer::Text("abc".into())).expect("回信");
        assert_eq!(
            asker.join().expect("脚本线程").expect("有答案"),
            Answer::Text("abc".into())
        );

        // ② 文件框：同上一条路，签名多一个 PathBuf。用户手选的路径要**当场**
        // 进授权信封（这里没信封，所以只验证「选中的路径带回去了」）。
        let picker = {
            let human = Arc::clone(&human);
            std::thread::spawn(move || {
                human.pick_file(&PickFile::Open {
                    title: String::new(),
                    filter: String::new(),
                    default: String::new(),
                })
            })
        };
        let pending = pop_pick_until(&picks);
        pending
            .reply
            .send(Some(PathBuf::from("/tmp/picked.txt")))
            .expect("回信");
        assert_eq!(
            picker.join().expect("脚本线程").expect("有答案"),
            Some(PathBuf::from("/tmp/picked.txt"))
        );
    }

    /// 等到队列里有东西再取一个（界面那一侧的动作）。
    fn pop_until(queue: &AskQueue) -> PendingAsk {
        loop {
            if let Some(ask) = queue.lock().expect("队列锁").pop() {
                return ask;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
    }

    fn pop_pick_until(queue: &Arc<Mutex<Vec<PendingPick>>>) -> PendingPick {
        loop {
            if let Some(pick) = queue.lock().expect("队列锁").pop() {
                return pick;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
    }

    /// 取消要能叫醒等在 `recv` 上的脚本线程（界面「停止」按钮那条路）。
    #[test]
    fn cancelling_wakes_a_waiting_ask() {
        let queue: AskQueue = Default::default();
        let cancel = CancelToken::new();
        let human = std::sync::Arc::new(UiHuman::new(
            Arc::clone(&queue),
            Arc::new(Mutex::new(Vec::new())),
            None,
            None,
            cancel.clone(),
        ));
        let worker = {
            let human = Arc::clone(&human);
            std::thread::spawn(move || {
                human.ask(&Ask::Confirm {
                    title: String::new(),
                    text: "继续？".into(),
                })
            })
        };
        // 等脚本排进队列，再叫停。
        while queue.lock().expect("队列锁").is_empty() {
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        cancel.cancel();
        assert_eq!(worker.join().expect("脚本线程"), Err(AskError::Cancelled));
    }
}

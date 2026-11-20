//! 测试用假端口：会话面（本模块）与应用面（[`app`]）各一份，
//! 都只模拟**时序语义**，不依赖 term-runtime / 真 PTY / 真窗口。
//!
//! 拆成目录模块的理由是 `verify.sh` 4j（单文件 500 代码行）：会话面的假实现
//! 一个文件就顶到 499 行，应用面再往里加就没有余量了。对外路径不变——
//! 调用方仍然写 `crate::testutil::FakeSessions` / `::FakeApp`。

mod app;

pub use app::FakeApp;

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use term_core::grid::{Cell, GridSnapshot};
use term_core::session::OpenRequest;
use term_core::session_runtime::{
    FacetKind, OutputReader, Session, SessionId, SessionState, WaitError, WaitMatch,
    WaitRegistration,
};

use crate::human::{Answer, Ask, AskError, HumanChannel, MessageButton, PickFile};
use crate::sessions::{SessionInfo, Sessions, SessionsError};

/// 假会话共享内核：测试经 [`Manip`] 推输出，登记经它读累计文本。
struct FakeCore {
    /// `write` 累计写入的字节。
    written: Mutex<Vec<u8>>,
    /// 累计到达的清洗文本（模拟 TextCleaner 之后的纯文本流）。
    output: Mutex<String>,
    /// 屏幕全文（`screen` 的事实来源）。
    screen: Mutex<String>,
    /// `resize` 声明过的尺寸；`None` = 没 resize 过，按屏幕内容推。
    size: Mutex<Option<(usize, usize)>>,
    /// 光标位置 `(行, 列)`，0 起。只有显式设置才不是 `(0, 0)`。
    cursor: Mutex<(usize, usize)>,
    finished: AtomicBool,
    /// 回显：`write` 的字节原样变成输出（模拟本地 shell / `cat`）。
    /// 同步写（`set_synchronous`）的用例靠它验「写完 read 能读到」。
    echo: AtomicBool,
}

impl FakeCore {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            written: Mutex::new(Vec::new()),
            output: Mutex::new(String::new()),
            screen: Mutex::new(String::new()),
            size: Mutex::new(None),
            cursor: Mutex::new((0, 0)),
            finished: AtomicBool::new(false),
            echo: AtomicBool::new(false),
        })
    }

    /// 记下新尺寸。**必须建模**：`terminal_size()` 的唯一可观察契约就是
    /// 「resize 之后尺寸跟着变」，假会话原先把 `resize` 写成空实现，
    /// 于是这个原语在测试里永远只能返回「按屏幕文本推出来的那个数」。
    fn resize(&self, cols: usize, rows: usize) {
        *self.size.lock().expect("size 锁") = Some((cols, rows));
    }
}

/// 假会话句柄本体（实现 [`Session`]）。
struct FakeSession {
    core: Arc<FakeCore>,
    id: SessionId,
}

/// 测试操纵器：推输出、读写字节、置屏幕、结束会话。
#[derive(Clone)]
pub struct Manip {
    core: Arc<FakeCore>,
}

impl Manip {
    /// 模拟一段会话输出到达（清洗后的纯文本），同步进屏幕。
    pub fn push_output(&self, text: &str) {
        self.core
            .output
            .lock()
            .expect("output 锁")
            .push_str(text);
        self.core.screen.lock().expect("screen 锁").push_str(text);
    }

    /// 覆盖屏幕全文（`screen` 返回它按行切出的快照）。
    pub fn set_screen(&self, text: &str) {
        *self.core.screen.lock().expect("screen 锁") = text.to_string();
    }

    /// 改尺寸（与 [`Session::resize`] 同一条路：两者都只改内核里记的尺寸）。
    pub fn resize(&self, cols: usize, rows: usize) {
        self.core.resize(cols, rows);
    }

    /// 把屏幕光标挪到 `(行, 列)`，0 起。
    pub fn set_cursor(&self, row: usize, col: usize) {
        *self.core.cursor.lock().expect("cursor 锁") = (row, col);
    }

    /// `write` 累计写入的字节。
    pub fn written(&self) -> Vec<u8> {
        self.core.written.lock().expect("written 锁").clone()
    }

    /// 会话进程结束：`state` → Finished，未命中的 wait 报 Closed。
    pub fn finish(&self) {
        self.core.finished.store(true, Ordering::SeqCst);
    }

    /// 打开/关闭回显：写入的字节原样变成输出（模拟 `cat` / 本地 shell）。
    ///
    /// 为什么需要它：同步写（`set_synchronous`）的唯一可观察契约就是「write
    /// 返回时，回显已经在缓冲里」——没有回显的假会话验不了这件事。
    pub fn set_echo(&self, on: bool) {
        self.core.echo.store(on, Ordering::SeqCst);
    }
}

/// 造一对「假会话 + 操纵器」：句柄喂给 [`crate::session::ScriptSession`]。
pub fn fake_session(id: SessionId) -> (Arc<Mutex<dyn Session>>, Manip) {
    let core = FakeCore::new();
    let session: Arc<Mutex<dyn Session>> = Arc::new(Mutex::new(FakeSession {
        core: core.clone(),
        id,
    }));
    (session, Manip { core })
}

impl Session for FakeSession {
    fn id(&self) -> SessionId {
        self.id
    }

    fn write(&self, bytes: &[u8]) {
        self.core
            .written
            .lock()
            .expect("written 锁")
            .extend_from_slice(bytes);
        if self.core.echo.load(Ordering::SeqCst) {
            let text = String::from_utf8_lossy(bytes).into_owned();
            self.core.output.lock().expect("output 锁").push_str(&text);
            self.core.screen.lock().expect("screen 锁").push_str(&text);
        }
    }

    fn resize(&self, cols: usize, rows: usize) {
        self.core.resize(cols, rows);
    }

    fn screen(&self) -> Option<GridSnapshot> {
        // 与真会话对齐：进程结束后仍可取最后一屏（快照直通）。
        let text = self.core.screen.lock().expect("screen 锁").clone();
        let size = *self.core.size.lock().expect("size 锁");
        let cursor = *self.core.cursor.lock().expect("cursor 锁");
        Some(snapshot_from_text(&text, size, cursor))
    }

    fn state(&self) -> SessionState {
        if self.core.finished.load(Ordering::SeqCst) {
            SessionState::Finished(None)
        } else {
            SessionState::Alive
        }
    }

    fn facets(&self) -> Vec<FacetKind> {
        Vec::new()
    }

    fn wait_register(&self, pattern: &str) -> Result<Box<dyn WaitRegistration>, WaitError> {
        if pattern.is_empty() {
            return Err(WaitError::EmptyPattern);
        }
        // 先登记后消费：登记点 = 当前已累计文本末尾，之前的输出不入账。
        let start = self.core.output.lock().expect("output 锁").len();
        Ok(Box::new(FakeWait {
            core: self.core.clone(),
            pattern: pattern.to_string(),
            start,
        }))
    }

    fn reader_register(&self) -> Result<Box<dyn OutputReader>, WaitError> {
        // 先登记后消费：登记点 = 当前已累计文本末尾，之前的输出不入账。
        let start = self.core.output.lock().expect("output 锁").len();
        Ok(Box::new(FakeReader {
            core: self.core.clone(),
            pos: start,
        }))
    }

    fn close(&mut self) {
        self.core.finished.store(true, Ordering::SeqCst);
    }
}

/// 假等待登记：轮询登记点之后的累计文本，直到命中 / 超时 / 会话结束。
struct FakeWait {
    core: Arc<FakeCore>,
    pattern: String,
    /// 登记时的输出累计长度（字节）＝ 匹配窗口起点。
    start: usize,
}

impl WaitRegistration for FakeWait {
    fn wait(self: Box<Self>, timeout: Duration) -> Result<WaitMatch, WaitError> {
        let deadline = Instant::now() + timeout;
        loop {
            {
                let output = self.core.output.lock().expect("output 锁");
                let region = &output[self.start..];
                if let Some(idx) = region.find(&self.pattern) {
                    let end = self.start + idx + self.pattern.len();
                    return Ok(WaitMatch {
                        text_upto: output[self.start..end].to_string(),
                    });
                }
            }
            if self.core.finished.load(Ordering::SeqCst) {
                return Err(WaitError::Closed);
            }
            if Instant::now() >= deadline {
                return Err(WaitError::Timeout);
            }
            thread::sleep(Duration::from_millis(1));
        }
    }
}

/// 假输出读取器：轮询登记点之后的累计文本，取走即推进消费点（同真
/// `OutputReader` 的「取走即清空累计」语义）。
struct FakeReader {
    core: Arc<FakeCore>,
    /// 登记点 = 已消费到的输出累计长度（字节）。
    pos: usize,
}

impl OutputReader for FakeReader {
    fn collect(&mut self, timeout: Duration) -> Result<String, WaitError> {
        let deadline = Instant::now() + timeout;
        loop {
            {
                let output = self.core.output.lock().expect("output 锁");
                if self.pos < output.len() {
                    let text = output[self.pos..].to_string();
                    self.pos = output.len();
                    return Ok(text);
                }
            }
            // 无新数据：会话结束报 Closed（不装作空串），超时返空串。
            if self.core.finished.load(Ordering::SeqCst) {
                return Err(WaitError::Closed);
            }
            if Instant::now() >= deadline {
                return Ok(String::new());
            }
            thread::sleep(Duration::from_millis(1));
        }
    }
}

/// 把纯文本按行灌进网格快照（`screen_text` 能原样读回，行尾空白被裁）。
///
/// `size` 是 `resize` 声明过的尺寸：**行数取「声明的」与「文本实际占的」
/// 两者较大值**——声明的行数更大时多出来的空行如实留着（真终端就是这样），
/// 但绝不因为声明的行数小于文本而把内容截掉，否则夹具会静默吞掉断言对象。
/// 没 `resize` 过时按文本推尺寸，于是既有 `screen_returns_full_text` 的
/// 「逐字相等」断言不受影响。
fn snapshot_from_text(
    text: &str,
    size: Option<(usize, usize)>,
    cursor: (usize, usize),
) -> GridSnapshot {
    let lines: Vec<&str> = text.split('\n').collect();
    let derived_rows = lines.len().max(1);
    let derived_cols = lines
        .iter()
        .map(|l| l.chars().count())
        .max()
        .unwrap_or(1)
        .max(1);
    let (cols, rows) = match size {
        Some((c, r)) if c > 0 && r > 0 => (c, r.max(derived_rows)),
        _ => (derived_cols, derived_rows),
    };
    let mut snap = GridSnapshot::empty(cols, rows);
    for (r, line) in lines.iter().enumerate() {
        for (c, ch) in line.chars().enumerate().take(cols) {
            snap.cells[r * cols + c] = Cell {
                ch,
                ..Cell::default()
            };
        }
    }
    snap.cursor = (cursor.0.min(rows - 1), cursor.1.min(cols - 1));
    snap
}

/// 假宿主：托着若干条假会话，用来验工作区的「池级」行为
/// （`session_list` / `session_open` / `session_use` / `session_close`）。
///
/// 与 `term-session` 的 `SessionPool` 刻意不同：这里不碰线程、不碰 PTY，
/// 只维护一张 `id → 假会话` 的表，所以「关掉之后 `session_list` 少一条」这类
/// 事实可以在微秒级断言。真实宿主的接线由 `apps/desktop` 的 `PooledSessions` 负责，
/// 那条路径靠 e2e 验。
///
/// **名字唯一化**在 [`Sessions::open`] 里照 ADR-0024 §2 对齐一遍（`base` /
/// `base-2` / `base-3`…）：假宿主若允许重名，`resolve` 就退化成「打中第一条」，
/// 而这正是 ADR 要消灭的那类静默。规则只有这一处与 `SessionPool::insert`
/// 两处实现，改动时两边必须一起改。
pub struct FakeSessions {
    tabs: Mutex<Vec<FakeTab>>,
    /// `session_open` 造的会话从这个号往后排，避免与测试预挂的 id 撞车。
    next: AtomicU64,
    /// `notify` 收到的消息（按到达顺序）。
    notes: Mutex<Vec<String>>,
    /// `ask` 的回答队列；用完即空 = 「这里没有可交互输入」（返回 `None`）。
    answers: Mutex<Vec<String>>,
    /// `open` 收到的请求（按到达顺序）：`session_open` / `session_connect`
    /// 到底把哪一变体递到端口上，只有这里能看见。
    opened: Mutex<Vec<OpenRequest>>,
    /// 文件框的回答队列；`Some` = 选中该路径，`None` = 用户取消。
    picks: Mutex<Vec<Option<String>>>,
}

struct FakeTab {
    id: SessionId,
    name: String,
    session: Arc<Mutex<dyn Session>>,
    manip: Manip,
}

impl FakeSessions {
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            tabs: Mutex::new(Vec::new()),
            next: AtomicU64::new(100),
            notes: Mutex::new(Vec::new()),
            answers: Mutex::new(Vec::new()),
            opened: Mutex::new(Vec::new()),
            picks: Mutex::new(Vec::new()),
        })
    }

    /// `open` 收到的请求（按到达顺序）。
    ///
    /// 现在没有调用点（`session_open` 的行为由 `workspace` 的测试从结果侧验），
    /// 留着是因为「端口到底把哪一变体递下去」只有这里看得见，写下一个用例时
    /// 不必再动一次结构体。
    #[allow(dead_code)]
    pub fn opened(&self) -> Vec<OpenRequest> {
        self.opened.lock().expect("opened 锁").clone()
    }

    /// `notify` 收到的消息（按到达顺序）。
    pub fn notes(&self) -> Vec<String> {
        self.notes.lock().expect("notes 锁").clone()
    }

    /// 预置一条 `ask` 的回答。answer 队列空着时 `ask` 返回 `None`
    /// （= 没有可交互输入），于是工作区报 `NoInput`。
    pub fn answer(&self, text: &str) {
        self.answers.lock().expect("answers 锁").push(text.to_string());
    }

    /// 把同一条测试替身装成 [`HostEnv`]：会话端口与人机端口**都指向它**，
    /// 于是 `ws.notify` 进 `notes`、`ws.ask` 读 `answers`。真实实现里这两件事
    /// 分属界面与会话池，测试替身没必要拆成两个对象。
    pub fn env(self: &Arc<Self>) -> crate::env::HostEnv {
        let sessions: Arc<dyn Sessions> = self.clone();
        let human: Arc<dyn HumanChannel> = self.clone();
        crate::env::HostEnv::new(sessions).with_human(human)
    }

    /// 预置一次文件框的结果：`Some(路径)` = 选中，`None` = 取消。
    #[allow(dead_code)]
    pub fn pick(&self, path: Option<&str>) {
        self.picks
            .lock()
            .expect("picks 锁")
            .push(path.map(str::to_string));
    }

    /// 预挂一条会话（模拟「窗口里本来就开着的会话」），返回它的操纵器。
    ///
    /// 叫 `mount` 而不是 `attach`：[`Sessions::attach`] 是「按 id 取句柄」，
    /// 同名会让测试里的 `fake.attach(1, "one")` 与 `sessions.attach(1)` 读起来像同一件事。
    pub fn mount(&self, id: SessionId, name: &str) -> Manip {
        let (session, manip) = fake_session(id);
        self.tabs.lock().expect("tabs 锁").push(FakeTab {
            id,
            name: name.to_string(),
            session,
            manip: manip.clone(),
        });
        manip
    }

    /// 某条会话的操纵器（往它的输出里推文本 / 把它标成结束）。
    pub fn manip(&self, id: SessionId) -> Option<Manip> {
        self.tabs
            .lock()
            .expect("tabs 锁")
            .iter()
            .find(|t| t.id == id)
            .map(|t| t.manip.clone())
    }

    /// 当前在册的 id（按登记顺序）。
    pub fn ids(&self) -> Vec<SessionId> {
        self.tabs.lock().expect("tabs 锁").iter().map(|t| t.id).collect()
    }

    /// 当前在册的名字（按登记顺序）。
    pub fn names(&self) -> Vec<String> {
        self.tabs
            .lock()
            .expect("tabs 锁")
            .iter()
            .map(|t| t.name.clone())
            .collect()
    }

    /// 按 ADR-0024 §2 的唯一化：`base` 被占了就 `base-2`、`base-3`…
    ///
    /// 用 `-2` 而不是 `(2)`：名字要进 JSON 与脚本字符串，带括号/空格每次都要
    /// 转义。与 `SessionPool::insert` 用同一条规则。
    fn unique_name(&self, base: &str) -> String {
        let tabs = self.tabs.lock().expect("tabs 锁");
        let taken = |candidate: &str| tabs.iter().any(|t| t.name == candidate);
        if !taken(base) {
            return base.to_string();
        }
        (2u32..)
            .map(|n| format!("{base}-{n}"))
            .find(|candidate| !taken(candidate))
            .expect("总能找到一个没被占用的后缀")
    }
}

impl Sessions for FakeSessions {
    fn list(&self) -> Vec<SessionInfo> {
        let tabs = self.tabs.lock().expect("tabs 锁");
        let mut rows: Vec<SessionInfo> = tabs
            .iter()
            .map(|t| {
                SessionInfo::from_state(
                    t.id,
                    t.name.clone(),
                    t.session.lock().expect("会话锁").state(),
                )
            })
            .collect();
        rows.sort_by_key(|t| t.id);
        rows
    }

    fn resolve(&self, name: &str) -> Option<SessionId> {
        self.tabs
            .lock()
            .expect("tabs 锁")
            .iter()
            .find(|t| t.name == name)
            .map(|t| t.id)
    }

    fn name(&self, id: SessionId) -> Option<String> {
        self.tabs
            .lock()
            .expect("tabs 锁")
            .iter()
            .find(|t| t.id == id)
            .map(|t| t.name.clone())
    }

    fn open(&self, request: &OpenRequest) -> Result<SessionInfo, SessionsError> {
        let base = match request {
            OpenRequest::Profile { name } => name.clone(),
            OpenRequest::Config(config) => config.name.clone(),
            OpenRequest::Command { command, name, .. } => {
                name.clone().unwrap_or_else(|| command.clone())
            }
            OpenRequest::LocalShell { .. } => "local-shell".to_string(),
        };
        let name = self.unique_name(&base);
        let id = self.next.fetch_add(1, Ordering::Relaxed);
        let (session, manip) = fake_session(id);
        self.tabs.lock().expect("tabs 锁").push(FakeTab {
            id,
            name: name.clone(),
            session,
            manip,
        });
        self.opened.lock().expect("opened 锁").push(request.clone());
        Ok(SessionInfo::from_state(id, name, SessionState::Alive))
    }

    fn close(&self, name: &str) -> Result<(), SessionsError> {
        let mut tabs = self.tabs.lock().expect("tabs 锁");
        let before = tabs.len();
        tabs.retain(|t| t.name != name);
        if tabs.len() == before {
            return Err(SessionsError::NotFound(name.to_string()));
        }
        Ok(())
    }

    fn attach(&self, id: SessionId) -> Option<Arc<Mutex<dyn Session>>> {
        self.tabs
            .lock()
            .expect("tabs 锁")
            .iter()
            .find(|t| t.id == id)
            .map(|t| Arc::clone(&t.session))
    }

    fn index(&self, id: SessionId) -> Option<usize> {
        let tabs = self.tabs.lock().expect("tabs 锁");
        let mut ids: Vec<SessionId> = tabs.iter().map(|t| t.id).collect();
        ids.sort_unstable();
        ids.iter().position(|each| *each == id).map(|i| i + 1)
    }
}

/// `y` / `yes` / `true` / `ok` 视为肯定（大小写不敏感）。
fn affirmative(text: &str) -> bool {
    matches!(
        text.trim().to_ascii_lowercase().as_str(),
        "y" | "yes" | "true" | "ok"
    )
}

/// 测试替身同时充当人机端口：`notes` 收通知、`answers` 供回答、`picks`
/// 供文件框。**端口与集合共用一个对象**是为了让既有测试的
/// `fake.notes()` / `fake.answer("y")` 断言原样成立——真实实现里这两件事
/// 分属两个对象（界面 vs 会话池），测试里不必为此多绕一层。
impl HumanChannel for FakeSessions {
    fn notify(&self, message: &str) {
        self.notes
            .lock()
            .expect("notes 锁")
            .push(message.to_string());
    }

    fn ask(&self, request: &Ask) -> Result<Answer, AskError> {
        let text = {
            let mut answers = self.answers.lock().expect("answers 锁");
            if answers.is_empty() {
                // 与真实宿主同一约定：没有输入就是 NoInput，不是空串。
                return Err(AskError::NoInput);
            }
            answers.remove(0)
        };
        Ok(match request {
            Ask::Alert { .. } => Answer::Ack,
            Ask::Confirm { .. } => Answer::Bool(affirmative(&text)),
            Ask::Text { .. } | Ask::Secret { .. } => Answer::Text(text),
            Ask::Choose { options, .. } => {
                // 回答既可以是下标（0 起），也可以是选项原文——两种写法测试里
                // 都常写，认错会在断言里变成难读的 off-by-one。
                match text.trim().parse::<usize>() {
                    Ok(index) if index < options.len() => Answer::Choice(index),
                    _ => match options.iter().position(|option| option == &text) {
                        Some(index) => Answer::Choice(index),
                        None => return Err(AskError::BadRequest(format!("没有选项 {text:?}"))),
                    },
                }
            }
            Ask::Message { buttons, .. } => match buttons {
                MessageButton::Ok => Answer::Ack,
                _ if affirmative(&text) => Answer::Bool(true),
                _ => Answer::Bool(false),
            },
        })
    }

    fn pick_file(&self, _request: &PickFile) -> Result<Option<PathBuf>, AskError> {
        let mut picks = self.picks.lock().expect("picks 锁");
        if picks.is_empty() {
            return Err(AskError::NoInput);
        }
        Ok(picks.remove(0).map(PathBuf::from))
    }
}


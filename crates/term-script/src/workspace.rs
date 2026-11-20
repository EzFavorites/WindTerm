//! 脚本的**工作区**：把「一条会话」升成「一组会话 + 一个当前绑定 + 一套宿主接缝」。
//!
//! # 分层
//!
//! - [`ScriptSession`] = **单会话原语核心**：`write` / `wait` / `read*` / `log_*`，
//!   每个实例自带一份捕获游标（`read` / `read_until` / `wait_any` 共享）。
//! - `ScriptWorkspace` = **全局层**：持有一个 [`HostEnv`]（六端口 + 授权 + 取消 +
//!   上下文）+ 每会话一个 [`ScriptSession`] + 当前绑定 id，负责全部动词。
//!
//! 为什么不是「一个 ScriptSession 内部换会话」：游标必须**按会话**独立。
//! 换绑之后若共用一份游标，前一条会话没读完的尾巴会被记到新会话名下，
//! `read_until` 就会拿旧数据匹配新会话的哨兵。分层之后这个错误在结构上
//! 不可能发生——`ScriptSession` 自己就是游标的归属单位。
//!
//! # 为什么按名字寻址（ADR-0024）
//!
//! 对外的一切键是**会话名**，不是 `SessionId`：id 是进程内自增，重启即变，
//! 而脚本的用途之一是「存成文件明天再跑」。所以本模块内部的 `map` 仍然
//! 按 id 索引（游标归属、去重都靠它），但导出到脚本与远程的只有名字。
//!
//! # 为什么要「当前绑定」而不是给每个原语加 session 参数
//!
//! 远程前端是**无状态**地址：每次调用都带 `"session"`，因为客户端可以并发、
//! 可以多路复用。脚本是**有状态**的线性文本：`write` / `wait` / `read_until`
//! 连着写十几行才是常见形状，每行都缀一个名字会把脚本读成参数表。
//!
//! # 取消的传播路径
//!
//! `HostEnv.cancel` → 每个 `ScriptSession`（构造时交一份克隆，共享同一个原子
//! 布尔）→ 每次阻塞的 50ms 切片。工作区自己的门面方法（`sleep` / `ask` /
//! `pick_file` / `clipboard_get` / `selection`）在调用前与返回后各看一眼。

use std::collections::HashMap;
use std::fmt;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use term_core::log_opts::LogOptions;
use term_core::session_runtime::LogStatus;
use term_core::script_caps::Denied;
use term_core::session::{OpenRequest, ProfileCatalog};
use term_core::session_runtime::{Session, SessionId, SessionState};

use crate::clip_ui::{ClipError, UiError};
use crate::env::HostEnv;
use crate::error::PrimitiveError;
use crate::filter::LineFilter;
use crate::human::{Answer, Ask, AskError, PickFile};
use crate::localfs::{DirEntry, FileStat, FsError};
use crate::proc::{ProcError, ProcOutput, ProcRun};
use crate::sendkeys;
use crate::sessions::{FixedSessions, SessionInfo, SessionsError};
use crate::session::{ScriptSession, WaitAnyHit};

/// 全局层动词的失败方式：要么是「原语层面的失败」（没有当前绑定、点了不存在
/// 的名字），要么是「集合层面的失败」（会话起不来、集合不支持）。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SessionError {
    /// 原语层面的失败。
    Primitive(PrimitiveError),
    /// 集合层面的失败。
    Sessions(SessionsError),
}

impl fmt::Display for SessionError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            SessionError::Primitive(e) => e.fmt(f),
            SessionError::Sessions(e) => e.fmt(f),
        }
    }
}

impl std::error::Error for SessionError {}

impl From<PrimitiveError> for SessionError {
    fn from(e: PrimitiveError) -> Self {
        SessionError::Primitive(e)
    }
}

impl From<SessionsError> for SessionError {
    fn from(e: SessionsError) -> Self {
        SessionError::Sessions(e)
    }
}

impl From<FsError> for SessionError {
    fn from(e: FsError) -> Self {
        SessionError::Primitive(e.into())
    }
}

impl From<ProcError> for SessionError {
    fn from(e: ProcError) -> Self {
        SessionError::Primitive(e.into())
    }
}

impl From<ClipError> for SessionError {
    fn from(e: ClipError) -> Self {
        SessionError::Primitive(e.into())
    }
}

impl From<UiError> for SessionError {
    fn from(e: UiError) -> Self {
        SessionError::Primitive(e.into())
    }
}

impl From<AskError> for SessionError {
    fn from(e: AskError) -> Self {
        SessionError::Primitive(e.into())
    }
}

impl From<Denied> for SessionError {
    fn from(e: Denied) -> Self {
        SessionError::Primitive(PrimitiveError::Denied(e.to_string()))
    }
}

/// 工作区内部状态。`map` 与 `current` 放同一把锁：两者的一致性
/// （当前绑定必须在表里）不需要额外约定来维护。
struct Tabs {
    map: HashMap<SessionId, ScriptSession>,
    current: SessionId,
}

struct Inner {
    env: HostEnv,
    /// 单次阻塞的上限（沙箱墙钟预算）——新包进来的会话也要继承它。
    max_wait: Duration,
    tabs: Mutex<Tabs>,
    /// 同步写开关（`set_synchronous`）。整个工作区一份。
    synchronous: AtomicBool,
    /// `LogFileName` 设置的模板（`crt` 门面用）：下一次 `log_start` 生效。
    log_template: Mutex<Option<LogOptions>>,
    /// 脚本**启动时**绑定的会话（`script_session()`）：不随 `session_use` 改。
    script_session: SessionId,
}

/// 脚本的工作区句柄。可克隆，所有克隆共享同一份绑定与同一批游标
/// （与 [`ScriptSession`] 一样按 `Arc` 共享，不做写时复制）。
#[derive(Clone)]
pub struct ScriptWorkspace {
    inner: Arc<Inner>,
    /// 这个句柄**只会话**作用在哪一条会话上：`None` = 跟随全局绑定（普通
    /// 脚本句柄），`Some(id)` = 钉死在那一条。
    ///
    /// 为什么需要它：`crt.GetTab(2).Screen.Send(…)` 必须打到 2 号标签上，
    /// 而「改全局绑定再改回来」既不是原子的、也会让并发克隆看到中间的绑定。
    /// 钉死的句柄只是一个多了字段的克隆，共享同一批游标（`inner` 是同一个
    /// `Arc`），因此 `Session` 状态不会分裂。
    scoped: Option<SessionId>,
}

impl ScriptWorkspace {
    /// 池级工作区：调用方给一整套宿主接缝（[`HostEnv`]），`current` 是脚本
    /// 一开始就在用的那条会话。
    pub fn new(env: HostEnv, current: Arc<Mutex<dyn Session>>, max_wait: Duration) -> Self {
        let session = ScriptSession::with_cancel(
            current,
            max_wait,
            Some(env.cancel.clone()),
        );
        let id = session.id();
        let mut map = HashMap::new();
        map.insert(id, session);
        Self {
            inner: Arc::new(Inner {
                env,
                max_wait,
                tabs: Mutex::new(Tabs { map, current: id }),
                synchronous: AtomicBool::new(false),
                log_template: Mutex::new(None),
                script_session: id,
            }),
            scoped: None,
        }
    }

    /// 把句柄钉到某一条会话上（[`CrtTab`](crate::compat::CrtTab) 用）。
    ///
    /// 与 `session_use` 的区别：`session_use` 改的是**全局绑定**，所有克隆
    /// 都跟着变；这里只改这一个句柄的视角，别处不受影响，也不需要把绑定
    /// 改回来。
    ///
    /// 会先把目标会话包进 `tabs`（与 `session_use` 同一套惰性包装），否则
    /// 钉过去的句柄在 `current()` 里找不到游标、只能报 `NoBinding`。
    pub fn scoped_to(&self, id: SessionId) -> Result<Self, PrimitiveError> {
        self.wrap_session(id)?;
        Ok(Self {
            inner: Arc::clone(&self.inner),
            scoped: Some(id),
        })
    }

    /// 确保 `id` 已经进到 `tabs.map`（必要时向集合要句柄包装一次）。
    /// **只准备游标，不改绑定**。
    fn wrap_session(&self, id: SessionId) -> Result<(), PrimitiveError> {
        if self.tabs().map.contains_key(&id) {
            return Ok(());
        }
        let name = || {
            self.inner
                .env
                .sessions
                .name(id)
                .unwrap_or_else(|| format!("#{id}"))
        };
        let Some(session) = self.inner.env.sessions.attach(id) else {
            return Err(PrimitiveError::NoSuchSession(name()));
        };
        let wrapped = ScriptSession::with_cancel(
            session,
            self.inner.max_wait,
            Some(self.inner.env.cancel.clone()),
        );
        // `or_insert` 而不是 `insert`：另一条线程可能刚好抢先包好了同一
        // 条会话，覆盖会把它已经推进的读游标丢掉。
        self.tabs().map.entry(id).or_insert(wrapped);
        Ok(())
    }

    /// 这个句柄实际作用的会话 id（钉死优先，否则全局绑定）。
    fn bound_id(&self) -> SessionId {
        self.scoped.unwrap_or_else(|| self.tabs().current)
    }

    /// 单会话工作区：集合退化成 [`FixedSessions`]、其余端口全部显式降级。
    /// 给 [`crate::ScriptEngine::run`] 与单测用。
    pub fn single(current: Arc<Mutex<dyn Session>>, max_wait: Duration) -> Self {
        let sessions = Arc::new(FixedSessions::new(Arc::clone(&current)));
        Self::new(HostEnv::new(sessions), current, max_wait)
    }

    /// 宿主接缝（端口 / 授权 / 上下文）。
    pub fn env(&self) -> &HostEnv {
        &self.inner.env
    }

    /// 取消令牌。
    pub fn cancel(&self) -> &crate::cancel::CancelToken {
        &self.inner.env.cancel
    }

    /// 本次运行的授权信封。
    pub fn caps(&self) -> &term_core::script_caps::Capabilities {
        &self.inner.env.caps
    }

    /// 取消检查点（长循环 / 大文件前调它）。
    pub fn check_cancel(&self) -> Result<(), PrimitiveError> {
        if self.inner.env.cancel.is_cancelled() {
            Err(PrimitiveError::Cancelled)
        } else {
            Ok(())
        }
    }

    /// 当前绑定的会话名（`session_current()`）。
    pub fn current_name(&self) -> Result<String, PrimitiveError> {
        let id = self.bound_id();
        self.inner
            .env
            .sessions
            .name(id)
            .ok_or(PrimitiveError::NoBinding)
    }

    /// `script_session()`：脚本启动时绑定的会话名（不随 `session_use` 变）。
    pub fn script_session_name(&self) -> Result<String, PrimitiveError> {
        let id = self.inner.script_session;
        self.inner
            .env
            .sessions
            .name(id)
            .ok_or(PrimitiveError::NoBinding)
    }

    /// 当前绑定的 id（界面要拿它取选区）。
    pub fn current_id(&self) -> SessionId {
        self.bound_id()
    }

    /// 取当前绑定的单会话核心。所有单会话原语的唯一入口。
    pub fn current(&self) -> Result<ScriptSession, PrimitiveError> {
        let id = self.bound_id();
        self.tabs()
            .map
            .get(&id)
            .cloned()
            .ok_or(PrimitiveError::NoBinding)
    }

    /// `session_list()`：当前所有会话（含已经不存活、但仍在册的），按**名字**排序。
    pub fn session_list(&self) -> Vec<SessionInfo> {
        let mut rows = self.inner.env.sessions.list();
        rows.sort_by(|a, b| a.name.cmp(&b.name));
        rows
    }

    /// `session_open(…)`：按 [`OpenRequest`] 新开一条会话，**并把它设为当前绑定**。
    ///
    /// 为什么要顺手改绑：脚本写 `session_open("cat")` 之后下一句几乎总是往这条新
    /// 会话发命令。不改绑的话那条命令会发给**原来**的会话——错误命令已经发
    /// 出去了才可能被发现。
    pub fn session_open_request(
        &self,
        request: OpenRequest,
    ) -> Result<SessionInfo, SessionError> {
        let info = self.inner.env.sessions.open(&request)?;
        let Some(session) = self.inner.env.sessions.attach(info.id) else {
            // 集合刚说开成功、转头又不认识它：这是实现自相矛盾。不能把 info
            // 交回去假装成功——脚本会拿到一个根本用不了的名字。
            return Err(SessionsError::Refused(format!(
                "会话集合报告新开了 {:?}，但紧接着取不到它的句柄",
                info.name
            ))
            .into());
        };
        let wrapped = ScriptSession::with_cancel(
            session,
            self.inner.max_wait,
            Some(self.inner.env.cancel.clone()),
        );
        {
            let mut tabs = self.tabs();
            tabs.map.insert(info.id, wrapped);
            tabs.current = info.id;
        }
        self.describe(info.id)
    }

    /// 旧形态：`session_open(command[, cols, rows])`。
    pub fn session_open(
        &self,
        command: Option<&str>,
        cols: usize,
        rows: usize,
    ) -> Result<SessionInfo, SessionError> {
        let request = match command {
            Some(command) => OpenRequest::Command {
                command: command.to_string(),
                name: None,
                cols: inner_or(cols, OpenRequest::DEFAULT_COLS),
                rows: inner_or(rows, OpenRequest::DEFAULT_ROWS),
            },
            None => OpenRequest::LocalShell {
                cols: inner_or(cols, OpenRequest::DEFAULT_COLS),
                rows: inner_or(rows, OpenRequest::DEFAULT_ROWS),
            },
        };
        self.session_open_request(request)
    }

    /// `session_connect(name)`：从会话库打开一条并绑定（CRT 的 `Session.Connect`）。
    pub fn session_connect(&self, name: &str) -> Result<SessionInfo, SessionError> {
        self.session_open_request(OpenRequest::Profile {
            name: name.to_string(),
        })
    }

    /// `session_use(name)`：把当前绑定切到 `name`，返回它的描述。
    pub fn session_use(&self, name: &str) -> Result<SessionInfo, SessionError> {
        let id = self.resolve(name)?;
        // **锁的作用域要小心**：`describe` 自己也要拿这把锁，所以任何
        // `self.tabs()` 的守卫都必须在调它之前释放（`std::sync::Mutex` 不可重入）。
        self.wrap_session(id)?;
        self.tabs().current = id;
        self.describe(id)
    }

    /// `session_close(name)`：关掉并移出清单。
    ///
    /// 关**当前绑定**是错误（[`PrimitiveError::CloseCurrentSession`]）而不是
    /// 「关掉再自动改绑」：自动改绑等于让后面那些 `write` 悄悄写到另一条会话里。
    pub fn session_close(&self, name: &str) -> Result<(), SessionError> {
        let id = self.resolve(name)?;
        if self.tabs().current == id {
            return Err(PrimitiveError::CloseCurrentSession(name.to_string()).into());
        }
        // 先放锁再调集合：关会话要等监管线程收尾，攥着工作区锁会挡住
        // 同一脚本里其它克隆的读取。
        self.inner.env.sessions.close(name)?;
        self.tabs().map.remove(&id);
        Ok(())
    }

    /// `session_clone(name)`：复制一条会话（副本会成为当前绑定）。
    pub fn session_clone(&self, name: &str) -> Result<SessionInfo, SessionError> {
        let id = self.resolve(name)?;
        let info = self.inner.env.sessions.clone_session(id)?;
        let Some(session) = self.inner.env.sessions.attach(info.id) else {
            return Err(SessionsError::Refused(format!(
                "会话集合报告复制出了 {:?}，但紧接着取不到它的句柄",
                info.name
            ))
            .into());
        };
        let wrapped = ScriptSession::with_cancel(
            session,
            self.inner.max_wait,
            Some(self.inner.env.cancel.clone()),
        );
        {
            let mut tabs = self.tabs();
            tabs.map.insert(info.id, wrapped);
            tabs.current = info.id;
        }
        self.describe(info.id)
    }

    /// `session_activate(name)`：请求宿主把它带到前台（脚本的绑定不变）。
    pub fn session_activate(&self, name: &str) -> Result<(), SessionError> {
        let id = self.resolve(name)?;
        self.inner.env.sessions.activate(id)?;
        Ok(())
    }

    /// `session_at(i)`：集合里第 `i` 条（**1 起**，与标签栏顺序一致）。
    pub fn session_at(&self, index: usize) -> Result<SessionInfo, SessionError> {
        if index == 0 {
            return Err(PrimitiveError::BadArgument("session_at 的序号从 1 起").into());
        }
        let rows = self.inner.env.sessions.list();
        rows.get(index - 1).cloned().ok_or_else(|| {
            SessionsError::NotFound(format!(
                "第 {index} 条会话（当前只有 {} 条）",
                rows.len()
            ))
            .into()
        })
    }

    /// `session_index()`：当前绑定在集合里的 1 起序号。
    pub fn session_index(&self) -> Result<usize, SessionError> {
        let id = self.bound_id();
        self.inner
            .env
            .sessions
            .index(id)
            .ok_or_else(|| SessionsError::Unsupported("当前宿主没有会话顺序".to_string()).into())
    }

    /// `session_active()`：界面当前激活的会话；没有这个概念时退回当前绑定。
    pub fn session_active(&self) -> Result<SessionInfo, SessionError> {
        match self.inner.env.sessions.active() {
            Some(id) => self.describe(id),
            None => {
                let id = self.bound_id();
                self.describe(id)
            }
        }
    }

    /// `session_info(name?)` 的完整描述：会话描述 + 相位 + 本端地址 + 主机名。
    pub fn session_info(&self, name: Option<&str>) -> Result<SessionInfo, SessionError> {
        let id = match name {
            Some(name) => self.resolve(name)?,
            None => self.bound_id(),
        };
        let mut info = self.describe(id)?;
        if let Some(addr) = self.inner.env.sessions.addr(id) {
            info.local_addr = Some(addr);
        }
        Ok(info)
    }

    /// 名字 → id。集合里没有这个名字就报 [`SessionsError::NotFound`]（点名那个名字）。
    fn resolve(&self, name: &str) -> Result<SessionId, SessionsError> {
        self.inner
            .env
            .sessions
            .resolve(name)
            .ok_or_else(|| SessionsError::NotFound(name.to_string()))
    }

    /// 一条会话的当前描述：**名字取自集合，状态取自会话本身**。
    fn describe(&self, id: SessionId) -> Result<SessionInfo, SessionError> {
        let Some(name) = self.inner.env.sessions.name(id) else {
            return Err(SessionsError::Refused(format!(
                "集合取不到 id={id} 的会话名，无法按名字寻址"
            ))
            .into());
        };
        let session = self.tabs().map.get(&id).cloned();
        let state = match session {
            Some(session) => session.state(),
            None => SessionState::Alive,
        };
        Ok(SessionInfo::from_state(id, name, state))
    }

    fn tabs(&self) -> MutexGuard<'_, Tabs> {
        match self.inner.tabs.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        }
    }

    // ── 单会话原语的门面 ────────────────────────────────────────────────

    pub fn write(&self, text: &str) -> Result<(), PrimitiveError> {
        let session = self.current()?;
        session.write(text)?;
        self.settle_if_synchronous(&session);
        Ok(())
    }

    /// `keys(names)`：按键名发按键，返回写入的字节数。
    pub fn keys(&self, names: &[&str]) -> Result<usize, PrimitiveError> {
        let session = self.current()?;
        let written = session.keys(names)?;
        self.settle_if_synchronous(&session);
        Ok(written)
    }

    /// `send_keys(expr)`：WSH `SendKeys` 语法（`^C` / `%{F4}` / `+{TAB}` / `~`）。
    pub fn send_keys(&self, expr: &str) -> Result<usize, PrimitiveError> {
        let strokes = sendkeys::parse(expr).map_err(PrimitiveError::BadArgument)?;
        let session = self.current()?;
        let mut written = 0;
        for stroke in strokes {
            match stroke {
                sendkeys::Stroke::Text(text) => {
                    session.write(&text)?;
                    written += text.len();
                }
                sendkeys::Stroke::Key(name) => {
                    written += session.keys(&[name.as_str()])?;
                }
            }
        }
        self.settle_if_synchronous(&session);
        Ok(written)
    }

    /// `set_synchronous(on)`：返回设置后的状态。
    pub fn set_synchronous(&self, on: bool) -> bool {
        self.inner.synchronous.store(on, Ordering::Relaxed);
        on
    }

    /// 同步写开着时，写完等输出安静（不推进游标，收下的文本留给后续 read）。
    fn settle_if_synchronous(&self, session: &ScriptSession) {
        if self.inner.synchronous.load(Ordering::Relaxed) {
            session.settle(crate::session::settle_quiet());
        }
    }

    pub fn wait_idle(
        &self,
        quiet_secs: f64,
        timeout_secs: f64,
    ) -> Result<String, PrimitiveError> {
        self.current()?.wait_idle(quiet_secs, timeout_secs)
    }

    /// `notify(message)`：把一句话送到用户眼前（宿主决定是状态栏还是 stderr）。
    pub fn notify(&self, message: &str) -> Result<(), PrimitiveError> {
        if message.trim().is_empty() {
            return Err(PrimitiveError::BadArgument("notify 的消息不能为空"));
        }
        self.inner.env.human.notify(message);
        Ok(())
    }

    /// `ask(prompt)`：向用户提问并取一行回答（CRT `Prompt` 的文本形态）。
    pub fn ask(&self, prompt: &str) -> Result<String, PrimitiveError> {
        if prompt.trim().is_empty() {
            return Err(PrimitiveError::BadArgument("ask 的提示语不能为空"));
        }
        let answer = self.inner.env.human.ask(&Ask::Text {
            title: String::new(),
            prompt: prompt.to_string(),
            default: String::new(),
        })?;
        match answer {
            Answer::Text(text) => Ok(text),
            other => Err(PrimitiveError::Failed(format!(
                "宿主对文本输入回了 {other:?}（实现与询问形态不一致）"
            ))),
        }
    }

    /// `ask(#{kind: …})` 的统一入口：宿主回什么就原样交出去。
    pub fn ask_request(&self, request: &Ask) -> Result<Answer, PrimitiveError> {
        self.check_cancel()?;
        let answer = self.inner.env.human.ask(request)?;
        self.check_cancel()?;
        Ok(answer)
    }

    /// `pick_file(…)`：用户选中的路径**自动追加**进本次 `fs` 授权。
    ///
    /// 这是 D2.3 的落点：「用户亲手选的就是用户同意的那一个」。追加发生在
    /// 工作区（而不是界面实现里），因为授权信封属于这次运行。
    pub fn pick_file(&self, request: &PickFile) -> Result<Option<PathBuf>, PrimitiveError> {
        self.check_cancel()?;
        let picked = self.inner.env.human.pick_file(request)?;
        // 授权追加需要可变访问；`HostEnv.caps` 是值，工作区持的是共享引用，
        // 所以这里交给宿主实现去追加（它拥有 `Capabilities`）。见
        // `apps/desktop/src/script_host.rs` 的说明。
        Ok(picked)
    }

    /// `sleep(ms)`：可取消的等待；**不消费输出**（与 CRT 同语义）。
    pub fn sleep(&self, millis: u64) -> Result<(), PrimitiveError> {
        let deadline = Instant::now() + Duration::from_millis(millis);
        loop {
            self.check_cancel()?;
            let now = Instant::now();
            if now >= deadline {
                return Ok(());
            }
            std::thread::sleep((deadline - now).min(Duration::from_millis(50)));
        }
    }

    pub fn wait(&self, pattern: &str, timeout_secs: f64) -> Result<String, PrimitiveError> {
        self.current()?.wait(pattern, timeout_secs)
    }

    pub fn read_until(
        &self,
        delimiter: &str,
        timeout_secs: f64,
        filter: Option<&LineFilter>,
    ) -> Result<String, PrimitiveError> {
        self.current()?
            .read_until(delimiter, timeout_secs, filter)
    }

    pub fn read(
        &self,
        timeout_secs: f64,
        filter: Option<&LineFilter>,
    ) -> Result<String, PrimitiveError> {
        self.current()?.read(timeout_secs, filter)
    }

    pub fn wait_any(
        &self,
        patterns: &[String],
        timeout_secs: f64,
        filter: Option<&LineFilter>,
    ) -> Result<WaitAnyHit, PrimitiveError> {
        self.current()?.wait_any(patterns, timeout_secs, filter)
    }

    pub fn screen(&self) -> Result<String, PrimitiveError> {
        self.current()?.screen()
    }

    /// `region(r1, c1, r2, c2)`：屏幕矩形（1 起、含端点、越界裁剪）。
    pub fn region(
        &self,
        row1: usize,
        col1: usize,
        row2: usize,
        col2: usize,
    ) -> Result<String, PrimitiveError> {
        self.current()?.region(row1, col1, row2, col2, false)
    }

    /// `region_lines(…)`：同上，行间用 `\n`。
    pub fn region_lines(
        &self,
        row1: usize,
        col1: usize,
        row2: usize,
        col2: usize,
    ) -> Result<String, PrimitiveError> {
        self.current()?.region(row1, col1, row2, col2, true)
    }

    pub fn cursor_pos(&self) -> Result<(usize, usize), PrimitiveError> {
        self.current()?.cursor_pos()
    }

    pub fn terminal_size(&self) -> Result<(usize, usize), PrimitiveError> {
        self.current()?.terminal_size()
    }

    pub fn state(&self) -> Result<SessionState, PrimitiveError> {
        Ok(self.current()?.state())
    }

    pub fn facets(&self) -> Result<Vec<&'static str>, PrimitiveError> {
        Ok(self.current()?.facets())
    }

    // ── 应用控制面（ADR-0027 §9，Q2 刀1） ────────────────────────────────

    /// `app_state()`：一份应用状态快照（需要 `app` 授权）。
    ///
    /// 与本文件其它原语的差别只有一处：它**不碰会话**——参数里没有会话名，
    /// 结果也与绑定无关。授权判定仍在这里做（与 `selection` / `window` 同一层），
    /// 端口实现不重复判。
    pub fn app_state(&self) -> Result<term_core::app_state::AppSnapshot, PrimitiveError> {
        if let Err(e) = self.inner.env.caps.check_app_view() {
            return Err(PrimitiveError::Denied(e.to_string()));
        }
        self.check_cancel()?;
        let snapshot = self.inner.env.app.snapshot()?;
        // 阻塞往返之后再看一次：界面按「停止」时，脚本不该拿着一份刚回来的
        // 快照继续往下跑（与 `selection` 前后各一次取消检查同一套纪律）。
        self.check_cancel()?;
        Ok(snapshot)
    }

    /// `app_panel_set(panel, visible)`：置位一块面板的显示开关（幂等）。
    ///
    /// 回执里的 `changed` 为空即「确实没变」（设成同值）。这一条让脚本不必
    /// 再取一次快照就能判断「这次调用有没有效果」。
    pub fn app_panel_set(
        &self,
        panel: &str,
        visible: bool,
    ) -> Result<crate::app_ui::AppDelta, PrimitiveError> {
        if let Err(e) = self.inner.env.caps.check_app_view() {
            return Err(PrimitiveError::Denied(e.to_string()));
        }
        self.check_cancel()?;
        let delta = self.inner.env.app.panel_set(panel, visible)?;
        self.check_cancel()?;
        Ok(delta)
    }

    /// `app_layout(op)`：改停靠位（`reset` / `dock` / `redock`，Q2 §4.3）。
    ///
    /// 授权要求 `app=configure`：`apply_layout_action` 末尾**必然**落盘
    /// （`save_layout()` 是既有行为），而 `AppScope::Tune` 的定义是「不落盘」。
    /// 想只改内存态得给动词一个 `persist:false`，本版不做。
    pub fn app_layout(
        &self,
        op: &crate::app_ui::LayoutOp,
    ) -> Result<crate::app_ui::AppDelta, PrimitiveError> {
        if let Err(e) = self.inner.env.caps.check_app_configure() {
            return Err(PrimitiveError::Denied(e.to_string()));
        }
        self.check_cancel()?;
        let delta = self.inner.env.app.layout(op)?;
        self.check_cancel()?;
        Ok(delta)
    }

    // ── 日志（ADR-0027 §1.1：三个参数内核都有） ──────────────────────────

    /// `log_start(dir)`：出厂模板（老签名，行为不变）。
    pub fn log_start(&self, dir: &str) -> Result<(), PrimitiveError> {
        self.log_start_opts(dir, None, None, LogOptions::default())
    }

    /// `log_start(dir, #{name, append, raw, max_mb})`。
    ///
    /// 会话名缺省取**当前绑定名**——修掉一个真 bug：以前写死成 `"script"`，
    /// 一次脚本给两台主机开日志时，同一秒内同名覆盖。
    pub fn log_start_opts(
        &self,
        dir: &str,
        name: Option<&str>,
        host: Option<&str>,
        mut opts: LogOptions,
    ) -> Result<(), PrimitiveError> {
        let session_name = match name {
            Some(name) if !name.trim().is_empty() => name.to_string(),
            _ => self.current_name()?,
        };
        // 门面设置的 `LogFileName` 模板：调用方没显式给名字时用它。
        if opts.file_name == LogOptions::default().file_name {
            if let Ok(slot) = self.inner.log_template.lock() {
                if let Some(template) = slot.as_ref() {
                    opts.file_name = template.file_name.clone();
                }
            }
        }
        let session = self.current()?;
        // 主机名：显式给了用显式的，否则问集合（界面宿主知道 config.host）。
        let host = host.map(str::to_string).or_else(|| {
            let id = self.bound_id();
            self.inner.env.sessions.host(id)
        });
        session.log_start_full(dir, &session_name, host.as_deref().unwrap_or(""), &opts)
    }

    /// 记下 `LogFileName` 模板（`crt` 门面用）。
    pub fn remember_log_template(&self, opts: LogOptions) {
        if let Ok(mut slot) = self.inner.log_template.lock() {
            *slot = Some(opts);
        }
    }

    /// 默认日志目录（宿主给；没有就报 Unsupported）。
    pub fn log_dir(&self) -> Option<PathBuf> {
        self.inner.env.sessions.log_dir()
    }

    pub fn log_stop(&self) -> Result<(), PrimitiveError> {
        self.current()?.log_stop()
    }

    pub fn log_status(&self) -> Result<LogStatus, PrimitiveError> {
        self.current()?.log_status()
    }

    // ── 人机 / 剪贴板 / 窗口 ─────────────────────────────────────────────

    pub fn clipboard_get(&self) -> Result<String, PrimitiveError> {
        self.caps_clipboard()?;
        self.check_cancel()?;
        let text = self.inner.env.clipboard.get()?;
        self.check_cancel()?;
        Ok(text)
    }

    pub fn clipboard_set(&self, text: &str) -> Result<(), PrimitiveError> {
        self.inner.env.clipboard.set(text)?;
        Ok(())
    }

    fn caps_clipboard(&self) -> Result<(), PrimitiveError> {
        self.inner
            .env
            .caps
            .check_clipboard()
            .map_err(|e| PrimitiveError::Denied(e.to_string()))
    }

    /// `selection()`：当前会话的选区文本（无界面 = 显式 Unsupported）。
    pub fn selection(&self) -> Result<String, PrimitiveError> {
        if let Err(e) = self.inner.env.caps.check_ui() {
            return Err(PrimitiveError::Denied(e.to_string()));
        }
        self.check_cancel()?;
        let id = self.bound_id();
        let text = self.inner.env.ui.selection(id)?;
        self.check_cancel()?;
        Ok(text)
    }

    /// `window(#{visible})`。
    pub fn set_window_visible(&self, visible: bool) -> Result<(), PrimitiveError> {
        if let Err(e) = self.inner.env.caps.check_ui() {
            return Err(PrimitiveError::Denied(e.to_string()));
        }
        self.inner.env.ui.set_visible(visible)?;
        Ok(())
    }

    /// `app_quit(#{save})`。
    pub fn app_quit(&self, save: bool) -> Result<(), PrimitiveError> {
        if let Err(e) = self.inner.env.caps.check_ui() {
            return Err(PrimitiveError::Denied(e.to_string()));
        }
        self.inner.env.ui.quit(save)?;
        Ok(())
    }

    // ── 本地文件 / 进程 ─────────────────────────────────────────────────

    pub fn fs_read(&self, path: &str) -> Result<Vec<u8>, PrimitiveError> {
        self.check_cancel()?;
        let data = self.inner.env.files.read(path)?;
        self.check_cancel()?;
        Ok(data)
    }

    pub fn fs_write(&self, path: &str, data: &[u8], append: bool) -> Result<u64, PrimitiveError> {
        self.check_cancel()?;
        let written = self.inner.env.files.write(path, data, append)?;
        Ok(written)
    }

    pub fn fs_list(&self, path: &str) -> Result<Vec<DirEntry>, PrimitiveError> {
        self.check_cancel()?;
        Ok(self.inner.env.files.list(path)?)
    }

    pub fn fs_stat(&self, path: &str) -> Result<FileStat, PrimitiveError> {
        self.check_cancel()?;
        Ok(self.inner.env.files.stat(path)?)
    }

    pub fn fs_mkdir(&self, path: &str) -> Result<(), PrimitiveError> {
        self.check_cancel()?;
        Ok(self.inner.env.files.mkdir(path)?)
    }

    pub fn fs_remove(&self, path: &str) -> Result<(), PrimitiveError> {
        self.check_cancel()?;
        Ok(self.inner.env.files.remove(path)?)
    }

    pub fn proc_run(&self, request: &ProcRun) -> Result<ProcOutput, PrimitiveError> {
        if let Err(e) = self.inner.env.caps.check_process() {
            return Err(PrimitiveError::Denied(e.to_string()));
        }
        self.check_cancel()?;
        let output = self.inner.env.process.run(request)?;
        self.check_cancel()?;
        Ok(output)
    }

    // ── 远端文件（SFTP） ────────────────────────────────────────────────

    fn remote(
        &self,
        name: Option<&str>,
    ) -> Result<Arc<dyn crate::localfs::RemoteFiles>, SessionError> {
        let id = match name {
            Some(name) => self.resolve(name)?,
            None => self.bound_id(),
        };
        Ok(self.inner.env.sessions.remote_files(id)?)
    }

    pub fn sftp_list(
        &self,
        name: Option<&str>,
        path: &str,
    ) -> Result<Vec<DirEntry>, SessionError> {
        Ok(self.remote(name)?.list(path)?)
    }

    pub fn sftp_get(&self, name: Option<&str>, remote: &str, local: &str) -> Result<u64, SessionError> {
        Ok(self.remote(name)?.get(remote, local)?)
    }

    pub fn sftp_put(&self, name: Option<&str>, local: &str, remote: &str) -> Result<u64, SessionError> {
        Ok(self.remote(name)?.put(local, remote)?)
    }

    pub fn sftp_mkdir(&self, name: Option<&str>, path: &str) -> Result<(), SessionError> {
        Ok(self.remote(name)?.mkdir(path)?)
    }

    pub fn sftp_rename(&self, name: Option<&str>, from: &str, to: &str) -> Result<(), SessionError> {
        Ok(self.remote(name)?.rename(from, to)?)
    }

    pub fn sftp_remove(&self, name: Option<&str>, path: &str) -> Result<(), SessionError> {
        Ok(self.remote(name)?.remove(path)?)
    }

    pub fn sftp_stat(&self, name: Option<&str>, path: &str) -> Result<FileStat, SessionError> {
        Ok(self.remote(name)?.stat(path)?)
    }

    // ── 会话库 ──────────────────────────────────────────────────────────

    fn profiles(&self) -> Result<Arc<dyn ProfileCatalog>, SessionError> {
        self.inner
            .env
            .sessions
            .profiles()
            .ok_or_else(|| SessionsError::Unsupported("当前宿主没有会话库".to_string()).into())
    }

    pub fn profile_list(&self) -> Result<Vec<term_core::session::SessionConfig>, SessionError> {
        self.profiles()?
            .sessions()
            .map_err(|e| SessionsError::Refused(e.0).into())
    }

    pub fn profile_get(
        &self,
        name: &str,
    ) -> Result<term_core::session::SessionConfig, SessionError> {
        let all = self.profile_list()?;
        all.into_iter()
            .find(|config| config.name == name)
            .ok_or_else(|| SessionsError::NotFound(name.to_string()).into())
    }

    /// 覆盖保存同名的哪一条（没有就追加）。返回保存后的整表条数。
    pub fn profile_save(
        &self,
        config: term_core::session::SessionConfig,
    ) -> Result<usize, SessionError> {
        let mut all = self.profile_list()?;
        match all.iter_mut().find(|existing| existing.name == config.name) {
            Some(slot) => *slot = config,
            None => all.push(config),
        }
        self.profiles()?
            .save(&all)
            .map_err(|e| SessionError::Sessions(SessionsError::Refused(e.0)))?;
        Ok(all.len())
    }
}

/// `0` 视为「没给」，用缺省值（能力面缺省 80×24）。
fn inner_or(value: usize, fallback: usize) -> usize {
    if value == 0 {
        fallback
    } else {
        value
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app_ui::LayoutOp;
    use crate::sessions::Sessions;
    use crate::testutil::{FakeApp, FakeSessions};
    use term_core::script_caps::{AppScope, Capabilities};

    /// 造一个挂着两条会话（id 1 "one" / id 2 "two"）的工作区，当前绑定是 1。
    fn two_tabs() -> (ScriptWorkspace, Arc<FakeSessions>) {
        let fake = FakeSessions::new();
        fake.mount(1, "one");
        fake.mount(2, "two");
        let sessions: Arc<dyn Sessions> = fake.clone();
        let current = sessions.attach(1).expect("预挂的会话 1");
        let ws = ScriptWorkspace::new(fake.env(), current, Duration::from_secs(2));
        (ws, fake)
    }

    /// 挂上假应用端口的工作区。`scope` 决定 `capabilities.app` 档——
    /// `Deny` 用来验「连读都拒」，`Tune` 是刀1/刀2 的正常档位。
    fn app_workspace(scope: AppScope) -> (ScriptWorkspace, FakeApp) {
        let fake = FakeSessions::new();
        fake.mount(1, "one");
        let sessions: Arc<dyn Sessions> = fake.clone();
        let current = sessions.attach(1).expect("预挂的会话 1");
        let app = FakeApp::new().with_language("zh-CN");
        let env = fake
            .env()
            .with_app(Arc::new(app.clone()))
            .with_caps(Capabilities {
                app: scope,
                ..Capabilities::NONE
            });
        (
            ScriptWorkspace::new(env, current, Duration::from_secs(2)),
            app,
        )
    }

    #[test]
    fn app_panel_set_reports_the_delta_and_the_new_panel_panorama() {
        // Q2 §10 刀2 验收 1 / 2：写后 `changed` 恰为 `["panels.<id>"]` 且 `rev`+1；
        // 同值 → `changed == []` 且 `rev` 不变。
        let (ws, app) = app_workspace(AppScope::Tune);
        let before = ws.app_state().expect("读得到快照");
        assert_eq!(before.panels.len(), 10, "面板恒十条");
        assert_eq!(before.rev, 0);
        assert_eq!(before.prefs_language, "zh-CN", "语言读的是端口给的落盘值");

        let delta = ws.app_panel_set("filer", true).expect("置位成功");
        assert_eq!(delta.changed, vec!["panels.filer".to_string()]);
        assert_eq!(delta.rev, 1, "真的变了才推进版本");
        assert_eq!(delta.panels.len(), 10, "回执带写入之后的全景");
        assert!(
            delta.panels.iter().any(|p| p.id == "filer" && p.visible),
            "回执里的 filer 必须是写入之后的值"
        );

        let same = ws.app_panel_set("filer", true).expect("重发同值");
        assert!(same.changed.is_empty(), "幂等：设成同值不写 changed");
        assert_eq!(same.rev, 1, "幂等：设成同值不推进版本");
        assert_eq!(app.rev(), 1, "端口侧的版本号与回执一致");
        assert_eq!(same.panels.len(), 10, "空操作也带全景");
        // 假端口把两次调用都记下来了（含空操作），到达顺序可查。
        assert_eq!(
            app.sets(),
            vec![
                ("filer".to_string(), true),
                ("filer".to_string(), true)
            ]
        );
    }

    #[test]
    fn app_verbs_tell_denied_apart_from_unknown_names() {
        // 授权不足 → `Denied`（连读都拒：快照会暴露 prefs 值）。
        let (denied, _app) = app_workspace(AppScope::Deny);
        assert!(
            matches!(denied.app_state(), Err(PrimitiveError::Denied(_))),
            "app=Deny 时 app_state 也该拒"
        );
        assert!(matches!(
            denied.app_panel_set("filer", true),
            Err(PrimitiveError::Denied(_))
        ));

        // 名字拼错 → `UnknownName` 带名字，不与「参数形状不对」混为一谈。
        let (ws, _app) = app_workspace(AppScope::Tune);
        match ws.app_panel_set("pane_filer", true) {
            Err(PrimitiveError::UnknownName { kind, name }) => {
                assert_eq!(kind, "停靠面板");
                assert_eq!(name, "pane_filer");
            }
            other => panic!("未知面板名应报 UnknownName，实得 {other:?}"),
        }
    }

    #[test]
    fn session_list_reports_every_session_by_name() {
        let (ws, _fake) = two_tabs();
        let rows = ws.session_list();
        assert_eq!(
            rows.iter().map(|t| t.name.as_str()).collect::<Vec<_>>(),
            vec!["one", "two"],
            "按名字排序：id 跨进程不可复现，名字才排得住"
        );
        assert!(rows.iter().all(|t| t.alive && t.exit.is_none()));
    }

    #[test]
    fn binding_decides_which_session_the_primitives_reach() {
        let (ws, fake) = two_tabs();
        ws.write("to-one\n").expect("发得出去");
        ws.session_use("two").expect("能切到 two");
        ws.write("to-two\n").expect("发得出去");

        let one = fake.manip(1).expect("会话 1").written();
        let two = fake.manip(2).expect("会话 2").written();
        assert_eq!(String::from_utf8_lossy(&one), "to-one\n");
        assert_eq!(String::from_utf8_lossy(&two), "to-two\n");
    }

    #[test]
    fn each_session_keeps_its_own_read_cursor() {
        let (ws, fake) = two_tabs();
        fake.manip(1).expect("会话 1").push_output("AAA");
        assert_eq!(ws.read(0.2, None).expect("读得到"), "AAA");

        ws.session_use("two").expect("能切到 two");
        fake.manip(2).expect("会话 2").push_output("BBB");
        assert_eq!(
            ws.read(0.2, None).expect("读得到"),
            "BBB",
            "切绑后不该看到上一条会话的残留"
        );

        ws.session_use("one").expect("能切回 one");
        assert_eq!(ws.read(0.05, None).expect("读得到"), "");
    }

    #[test]
    fn opening_a_session_makes_it_the_current_binding() {
        let (ws, fake) = two_tabs();
        let opened = ws.session_open(Some("cat"), 80, 24).expect("开得起来");
        assert_eq!(
            ws.current_name().expect("有绑定"),
            opened.name,
            "新开的会话应当成为当前绑定"
        );
        assert_eq!(opened.name, "cat");
        ws.write("hi\n").expect("发得出去");
        assert_eq!(
            String::from_utf8_lossy(&fake.manip(opened.id).expect("新会话").written()),
            "hi\n"
        );
        assert_eq!(ws.session_list().len(), 3);
    }

    #[test]
    fn opening_a_duplicate_name_reports_the_real_name_not_the_request() {
        let (ws, fake) = two_tabs();
        let second = ws.session_open(Some("one"), 80, 24).expect("开得起来");
        assert_eq!(second.name, "one-2", "重名要加后缀，且返回的是真名");
        assert_eq!(ws.current_name().expect("有绑定"), "one-2");
        assert_eq!(fake.names(), vec!["one", "two", "one-2"]);

        ws.session_use("one").expect("base 仍指向第一条");
        assert_eq!(ws.session_list().len(), 3);
    }

    #[test]
    fn closing_the_current_binding_is_refused_then_allowed_after_switching() {
        let (ws, fake) = two_tabs();
        assert_eq!(
            ws.session_close("one").unwrap_err(),
            SessionError::Primitive(PrimitiveError::CloseCurrentSession("one".to_string())),
            "关当前绑定必须显式失败，不能自动改绑"
        );
        assert_eq!(fake.ids(), vec![1, 2], "被拒的关闭不能真的关掉会话");

        ws.session_use("two").expect("能切到 two");
        ws.session_close("one").expect("关非当前绑定应当成功");
        assert_eq!(fake.ids(), vec![2], "集合那边也要真的移除");
    }

    #[test]
    fn use_and_close_report_an_unknown_name_instead_of_picking_another() {
        let (ws, _fake) = two_tabs();
        assert_eq!(
            ws.session_use("ghost").unwrap_err(),
            SessionError::Sessions(SessionsError::NotFound("ghost".to_string()))
        );
        assert_eq!(
            ws.session_close("ghost").unwrap_err(),
            SessionError::Sessions(SessionsError::NotFound("ghost".to_string()))
        );
        assert_eq!(ws.current_name().expect("绑定不该被改动"), "one");
    }

    #[test]
    fn single_host_refuses_pool_level_operations() {
        let (session, _manip) = crate::testutil::fake_session(7);
        let sessions = FixedSessions::new(Arc::clone(&session));
        assert_eq!(sessions.name(7).as_deref(), Some("session-7"));
        assert!(matches!(
            sessions.open(&OpenRequest::Command {
                command: "cat".to_string(),
                name: None,
                cols: 80,
                rows: 24
            }),
            Err(SessionsError::Refused(_))
        ));
        assert!(matches!(sessions.close("session-7"), Err(SessionsError::Refused(_))));

        let ws = ScriptWorkspace::single(session, Duration::from_secs(1));
        assert_eq!(
            ws.session_close("session-7").unwrap_err(),
            SessionError::Primitive(PrimitiveError::CloseCurrentSession(
                "session-7".to_string()
            ))
        );
        assert_eq!(ws.session_list().len(), 1, "被拒之后清单不能少一条");
    }

    #[test]
    fn synchronous_write_waits_for_the_echo_so_a_following_read_sees_it() {
        let fake = FakeSessions::new();
        let manip = fake.mount(1, "echo-shell");
        manip.set_echo(true);
        let sessions: Arc<dyn Sessions> = fake.clone();
        let current = sessions.attach(1).expect("预挂的会话 1");
        let ws = ScriptWorkspace::new(fake.env(), current, Duration::from_secs(2));

        assert!(ws.set_synchronous(true), "开关要回报设置后的状态");
        ws.write("hi\n").expect("发得出去");
        assert_eq!(ws.read(0.0, None).expect("读得到"), "hi\n");
        assert!(!ws.set_synchronous(false));
    }

    #[test]
    fn the_sync_switch_is_workspace_wide_not_per_session() {
        let (ws, _fake) = two_tabs();
        ws.set_synchronous(true);
        ws.session_use("two").expect("能切到 two");
        assert!(ws.set_synchronous(true), "切绑不该重置开关");
    }

    #[test]
    fn notify_reaches_the_host_and_ask_reports_missing_input() {
        let (ws, fake) = two_tabs();
        ws.notify("干活中").expect("notify 不失败");
        assert_eq!(fake.notes(), vec!["干活中".to_string()]);

        assert_eq!(ws.ask("继续？").unwrap_err(), PrimitiveError::NoInput);

        fake.answer("y");
        assert_eq!(ws.ask("继续？").expect("有回答"), "y");
    }

    #[test]
    fn empty_prompts_are_refused_before_touching_the_host() {
        let (ws, fake) = two_tabs();
        assert_eq!(
            ws.notify("  ").unwrap_err(),
            PrimitiveError::BadArgument("notify 的消息不能为空")
        );
        assert_eq!(
            ws.ask("").unwrap_err(),
            PrimitiveError::BadArgument("ask 的提示语不能为空")
        );
        assert!(fake.notes().is_empty(), "被拒的 notify 不该真的送到宿主");
    }

    #[test]
    fn session_at_is_one_based_and_out_of_range_is_an_error() {
        let (ws, _fake) = two_tabs();
        assert_eq!(ws.session_at(1).expect("第一条").name, "one");
        assert_eq!(ws.session_at(2).expect("第二条").name, "two");
        assert!(ws.session_at(3).is_err(), "越界必须报错，不能返回最后一条");
        assert!(ws.session_at(0).is_err(), "序号从 1 起");
    }

    #[test]
    fn sleeping_can_be_cancelled_quickly() {
        let (ws, _fake) = two_tabs();
        let cancel = ws.cancel().clone();
        let started = Instant::now();
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(60));
            cancel.cancel();
        });
        let err = ws.sleep(5_000).unwrap_err();
        assert_eq!(err, PrimitiveError::Cancelled);
        assert!(
            started.elapsed() < Duration::from_millis(600),
            "取消要在切片内生效，实际 {:?}",
            started.elapsed()
        );
    }

    #[test]
    fn send_keys_translates_wsh_syntax_into_bytes() {
        let (ws, fake) = two_tabs();
        ws.send_keys("^C").expect("Ctrl+C");
        assert_eq!(fake.manip(1).expect("会话 1").written(), vec![0x03]);
        ws.send_keys("ab~").expect("文本 + 回车");
        let written = fake.manip(1).expect("会话 1").written();
        assert_eq!(&written[1..], b"ab\r");
    }

    #[test]
    fn app_layout_needs_configure_because_it_always_persists() {
        // `Tune` 的定义是「不落盘」，而布局动作末尾必然 `save_layout()`。
        let (tune, _app) = app_workspace(AppScope::Tune);
        assert!(
            matches!(tune.app_layout(&LayoutOp::Reset), Err(PrimitiveError::Denied(_))),
            "Tune 不该能改布局文件"
        );

        let (ws, app) = app_workspace(AppScope::Configure);
        let delta = ws
            .app_layout(&LayoutOp::Dock {
                panel: "filer".to_string(),
                zone: "left".to_string(),
                index: Some(0),
            })
            .expect("Configure 放行");
        assert_eq!(
            delta.changed,
            vec!["panels.filer".to_string(), "layout".to_string()]
        );
        assert_eq!(delta.rev, 1);
        assert_eq!(delta.panels.len(), 10, "回执照样带十块面板");

        // 名字由端口侧校验（名字表在 `term-app`，这一层拿不到）。
        assert!(matches!(
            ws.app_layout(&LayoutOp::Redock {
                panel: "pane_filer".to_string()
            }),
            Err(PrimitiveError::UnknownName { .. })
        ));
        assert_eq!(app.layouts().len(), 1, "只有过了名字校验的那次到达端口");
    }
}

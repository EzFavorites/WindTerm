//! 脚本能看见的**会话集合**：列出 / 新开 / 关闭 / 绑定（`session_*` 四个动词的落点）。
//!
//! # 为什么要这一层
//!
//! 单会话原语只认 [`Session`]（协议无关，ADR-0019），但全局层
//! （`session_list` / `session_open` / `session_close` / `session_use`）必须
//! 回答两个单会话看不见的问题：「现在有哪些会话」「再开一个」。
//!
//! # 为什么叫 `Sessions` 而不是 `Host`
//!
//! 本仓的 `host` 已经被「远程主机」占死：`SessionConfig::host` 是主机名
//! （`crates/term-core/src/session.rs:164`），`HostKeyPolicy` 是主机密钥策略，
//! 地址里的 `AddressKind::Host` 中文就译作「主机」。而这个端口做的是
//! 「一组会话的登记与启停」，与远程主机无关。
//!
//! 复数是刻意的：单数是 [`Session`]（一条会话，读写/等待），复数是这组会话
//! （列举、开关、绑定）。因此**取句柄的方法叫 `attach` 而不是 `session`**——
//! 让 `dyn Sessions` 与 `dyn Session` 不出现「同一行只差一个 s」的写法。
//!
//! # 寻址：名字，不是 id（ADR-0024）
//!
//! 脚本里一切按**会话名**寻址。名字由实现在开会话时定下、池内唯一
//! （重名自动加后缀 `cat` → `cat-2`），并保证「同一个名字永远指向同一条会话」。
//! `SessionId` 仍然存在，但它退回实现细节：脚本不该看到它，因为它是进程内
//! 自增——存下来的脚本明天再跑就失效了，而「明天再跑」正是脚本的用途。
//!
//! 这两件事的答案属于**应用层**（`term-session` 的 `SessionPool` 是登记表，
//! 内置 SSH 的通道工厂、凭据、保险库都是应用策略）。`term-script` 不能依赖
//! 它——那会把「脚本引擎」和「会话登记表」焊在一起，也不符合单向依赖。
//!
//! 所以这里只**声明** trait，由调用方实现：
//! - `apps/desktop/src/script_host.rs` 的 `PooledSessions`（`SessionPool` 之上）；
//! - [`FixedSessions`]：只托管一条会话的固定集合，给 [`crate::ScriptEngine::run`]
//!   这条老 API 与单测用。
//!
//! # 一条硬规矩
//!
//! 实现**不能假装成功**：`open` 起不来就报 [`SessionsError`]，`close` 关不掉就报，
//! 不能返回一个「看起来像成功」的空值。全局层最坏的一种静默是「新开会话失败，
//! 脚本继续往旧会话里发命令」——那时错误命令已经发出去了。

use std::sync::{Arc, Mutex};

use term_core::session::{OpenRequest, ProfileCatalog};
use term_core::session_runtime::{Session, SessionId, SessionState};

use crate::localfs::RemoteFiles;

/// 会话集合能办、但办不成时的原因。文案直接面向脚本作者。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SessionsError {
    /// 会话起不来（命令不存在、PTY 分配失败、通道注入缺失……）。
    Spawn(String),
    /// 这组会话里没有这个名字（已经关掉、或从来不属于本集合）。
    NotFound(String),
    /// 这组会话本身不支持该操作（例如只托管一条会话的固定集合不能新开）。
    Refused(String),
    /// 宿主有这个概念、但没接上这条实现（例如没有 SFTP 适配器）。
    Unsupported(String),
    /// 能力没有授予（授权信封里没有这一项）。
    Denied(String),
}

impl std::fmt::Display for SessionsError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SessionsError::Spawn(why) => write!(f, "会话起不来：{why}"),
            SessionsError::NotFound(name) => write!(
                f,
                "没有名为 {name:?} 的会话（用 session_list() 看当前有哪些）"
            ),
            SessionsError::Refused(why) => f.write_str(why),
            SessionsError::Unsupported(why) => write!(f, "当前宿主不支持：{why}"),
            SessionsError::Denied(why) => write!(f, "{why}"),
        }
    }
}

impl std::error::Error for SessionsError {}

/// 一个会话的对外描述。字段与能力面 `session_list` 的结果形状一一对应
/// （ADR-0023：同一个动词在两个前端返回同一种形状）。
///
/// `id` 是**内部**的绑定句柄，脚本侧永远看不到它（见 [`Sessions`] 的寻址说明）。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SessionInfo {
    /// 会话 id：只用于本进程内绑定，不是寻址键。
    pub id: SessionId,
    /// 会话名：寻址键，池内唯一。
    pub name: String,
    /// 存活中。
    pub alive: bool,
    /// 结束时的状态文本；存活时为 `None`。
    pub exit: Option<String>,
    /// 连接相位（`alive` / `connecting` / `finished`）。
    ///
    /// 为什么另立一个字段而不复用 `alive`：内置 SSH 的连接是**异步**的，
    /// 「正在连」既不是活着也不是结束；脚本据此决定「再等等」还是「改走
    /// 离线分支」。不支持相位的实现给 `alive`，于是老行为不变。
    pub phase: String,
    /// 连接的本端地址（`127.0.0.1:54321` 这种）；本地 shell / 取不到时为 `None`。
    pub local_addr: Option<String>,
}

/// 相位字面量（与能力面 `session_info` 的 enum 同一套字符串）。
pub const PHASE_ALIVE: &str = "alive";
pub const PHASE_CONNECTING: &str = "connecting";
pub const PHASE_FINISHED: &str = "finished";

impl SessionInfo {
    /// 由「id + 名字 + 状态」造一条描述。实现侧唯一需要知道状态怎么映射的地方。
    pub fn from_state(id: SessionId, name: String, state: SessionState) -> Self {
        match state {
            SessionState::Alive => Self {
                id,
                name,
                alive: true,
                exit: None,
                phase: PHASE_ALIVE.to_string(),
                local_addr: None,
            },
            SessionState::Finished(exit) => Self {
                id,
                name,
                alive: false,
                exit,
                phase: PHASE_FINISHED.to_string(),
                local_addr: None,
            },
        }
    }

    /// 带上本端地址（实现侧知道它时用）。
    pub fn with_local_addr(mut self, addr: Option<String>) -> Self {
        self.local_addr = addr;
        self
    }
}

/// 一组会话。实现方负责「哪些会话算本工作区的」这件事。
pub trait Sessions: Send + Sync {
    /// 当前清单，按 id 升序（稳定顺序让脚本的输出可复现）。
    fn list(&self) -> Vec<SessionInfo>;

    /// 按会话名查 id。查不到返回 `None`（调用方报 [`SessionsError::NotFound`]）。
    ///
    /// 刻意**不**做模糊匹配、不忽略大小写、不「找不到就取第一个」：
    /// 名字打空必须报错，静默打中另一条会话是这类 API 最贵的错。
    fn resolve(&self, name: &str) -> Option<SessionId>;

    /// 会话 id 的寻址名。集合里没有这条会话就返回 `None`。
    ///
    /// 需要它是因为「名字与 id 不是同一个东西」：`open` 返回 id，而重名时
    /// 池可能把 `cat` 改成 `cat-2`——脚本必须拿**真名**去寻址。
    fn name(&self, id: SessionId) -> Option<String>;

    /// 新开一条会话并返回它的描述。`request` 走
    /// [`OpenRequest`](term_core::session::OpenRequest) 的四个变体：
    /// 会话库里的名字 / 完整配置 / 一次性命令 / 本地 shell。
    ///
    /// 尺寸不合法由实现方决定是否钳制，但不许静默改成另一个数之后又报成功
    /// ——那会让脚本的断行位置与预期不符。
    fn open(&self, request: &OpenRequest) -> Result<SessionInfo, SessionsError>;

    /// 关闭并从清单里移除。关不掉（不存在 / 已经关了）报 [`SessionsError::NotFound`]。
    fn close(&self, name: &str) -> Result<(), SessionsError>;

    /// 绑定并借出会话句柄。全局层 `session_use` 靠它把「别的会话」包成原语可用的
    /// [`Session`]；本集合不认识就返回 `None`（调用方报 `NotFound`）。
    fn attach(&self, id: SessionId) -> Option<Arc<Mutex<dyn Session>>>;

    // ── 会话面的扩展（ADR-0027）：默认全部显式拒绝 ──────────────────
    //
    // 这些方法只有界面宿主能真做（把标签带到前台、复制一条会话、问哪个是
    // 当前激活的）。默认实现一律 Unsupported/None，于是 CLI 与单测不必假装
    // 「我也有标签栏」——这正是「六个端口比一个巨型 trait 好」的地方。

    /// 把这条会话带到前台（`session_activate`）。无界面宿主不适用。
    fn activate(&self, _id: SessionId) -> Result<(), SessionsError> {
        Err(SessionsError::Unsupported(
            "当前宿主没有界面，无法把会话带到前台".to_string(),
        ))
    }

    /// 复制一条会话（`session_clone`）。默认拒绝：复制要「怎么连」的全部材料
    /// （配置 + 凭据来源），只有界面宿主握得住。
    fn clone_session(&self, _id: SessionId) -> Result<SessionInfo, SessionsError> {
        Err(SessionsError::Unsupported(
            "当前宿主不能复制会话".to_string(),
        ))
    }

    /// 会话在集合**自己**的登记序里的 **1 起**序号。
    ///
    /// 注意这不是标签栏坐标：`ScriptWorkspace` 的 `GetTab(i)` / `Tab.Index()`
    /// / `GetTabCount()` / `session_index()` 统一按名字排序（见
    /// `ScriptWorkspace::session_list`），不调用本方法。留着它是为了宿主侧的
    /// 其它用途；默认 `None` = 没有稳定顺序。
    fn index(&self, _id: SessionId) -> Option<usize> {
        None
    }

    /// 界面当前激活的会话（`session_active`）。默认 `None` = 没有这个概念，
    /// 调用方退回「脚本自己绑定的那条」。
    fn active(&self) -> Option<SessionId> {
        None
    }

    /// 会话连接的主机名（`%{host}` 用）。默认 `None`。
    ///
    /// 这是**远端**主机名（`SessionConfig.host`），不是本端地址——两者在
    /// `SessionInfo` 里是两个字段，混用会让脚本把「连到哪」当成「从我这儿
    /// 的哪条连接出去」。
    fn host(&self, _id: SessionId) -> Option<String> {
        None
    }

    /// 本端连接地址（`session_info.local_addr`，形如 `127.0.0.1:54321`）。
    /// 默认 `None` = 这个宿主取不到（本地 PTY 本来也没有）。
    fn addr(&self, _id: SessionId) -> Option<String> {
        None
    }

    /// 会话库（`profile_get/set/list`）。默认 `None` = 这个宿主没有库。
    fn profiles(&self) -> Option<Arc<dyn ProfileCatalog>> {
        None
    }

    /// 默认日志目录（`Session.Log` 门面用）。默认 `None` = 不知道。
    ///
    /// 为什么放在端口上而不是在 `term-script` 里读 profile 目录：默认日志目录
    /// 是**应用策略**（`~/.zterm/logs` 这条规则属于 `term-paths`），
    /// 脚本引擎不该为了一个默认值多依赖一个 crate。
    fn log_dir(&self) -> Option<std::path::PathBuf> {
        None
    }

    /// 一条会话的远端文件句柄（`sftp_*`）。默认 Unsupported。
    fn remote_files(&self, _id: SessionId) -> Result<Arc<dyn RemoteFiles>, SessionsError> {
        Err(SessionsError::Unsupported(
            "当前宿主没有接入远端文件传输".to_string(),
        ))
    }

    // ── 面向人的两条已迁走 ──────────────────────────────────────────
    //
    // `notify` / `ask` 曾经在这条 trait 上（默认写 stderr / 返回 None）。
    // ADR-0027 之后，它们属于 [`HumanChannel`](crate::HumanChannel) 这个**独立
    // 端口**：会话集合回答「有哪些会话」，人机端口回答「怎么问人」。两件事
    // 混在一条 trait 上，进程级宿主想换「问人的方式」就得连会话集合一起换。
}

/// 只托管一条会话的固定集合：[`crate::ScriptEngine::run`] 的老签名（直接给一条
/// 会话）落到这里。
///
/// `open` / `close` 一律 [`SessionsError::Refused`]——**不是**静默 no-op。原因：
/// 老 API 的调用方（`desk script run` 之外的嵌入式用法、单测）没有「池」的
/// 概念，新开一条会话没人负责关它，泄漏比报错更糟。
pub struct FixedSessions {
    id: SessionId,
    name: String,
    session: Arc<Mutex<dyn Session>>,
}

impl FixedSessions {
    /// 用一条会话造集合。名字取 `session-<id>`：`Session` trait 不带名字，
    /// 而这个名字只用于展示（脚本要真名字就挂 [`Sessions`] 的真实实现）。
    pub fn new(session: Arc<Mutex<dyn Session>>) -> Self {
        let id = lock(&session).id();
        Self {
            id,
            name: format!("session-{id}"),
            session,
        }
    }

    /// 本集合托管的那条会话的 id。
    pub fn id(&self) -> SessionId {
        self.id
    }
}

impl Sessions for FixedSessions {
    fn list(&self) -> Vec<SessionInfo> {
        let state = lock(&self.session).state();
        vec![SessionInfo::from_state(self.id, self.name.clone(), state)]
    }

    fn resolve(&self, name: &str) -> Option<SessionId> {
        (name == self.name).then_some(self.id)
    }

    fn name(&self, id: SessionId) -> Option<String> {
        (id == self.id).then(|| self.name.clone())
    }

    fn open(&self, request: &OpenRequest) -> Result<SessionInfo, SessionsError> {
        Err(SessionsError::Refused(format!(
            "当前集合只托管一条会话，不能新开 {}（要开多会话请用池化实现）",
            request.describe()
        )))
    }

    fn close(&self, name: &str) -> Result<(), SessionsError> {
        Err(SessionsError::Refused(format!(
            "当前集合只托管一条会话，不能关闭 {name:?}（它的生命周期由调用方负责）"
        )))
    }

    fn attach(&self, id: SessionId) -> Option<Arc<Mutex<dyn Session>>> {
        if id == self.id {
            Some(Arc::clone(&self.session))
        } else {
            None
        }
    }
}

/// 取会话锁；中毒时照常取回数据，不 panic（脚本线程上 panic 会毁掉整段脚本，
/// 而中毒只说明「别的线程持锁时出过事」，数据本身仍然可用）。
fn lock<'a>(session: &'a Arc<Mutex<dyn Session>>) -> std::sync::MutexGuard<'a, dyn Session + 'static> {
    match session.lock() {
        Ok(guard) => guard,
        Err(poisoned) => poisoned.into_inner(),
    }
}

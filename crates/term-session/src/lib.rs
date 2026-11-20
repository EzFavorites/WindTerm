//! `term-session` — 协议无关的**会话控制面**（ADR-0019）。
//!
//! 全仓**唯一**的会话所有者库：按 tab 聚合 `Session`，对外暴露能力面
//! 方法（`open` / `send` / `screen` / `state` / `wait_register` /
//! `close` / `list`）。四个客户端——交互式 UI、脚本引擎（腿 A）、
//! JSON-RPC 桥（腿 B）、AI 面板 agent——全部只经这里触达会话；
//! 任何客户端不得自持远端会话（那就是「静默执行」，本 crate 的存在
//! 正是为了让这类分叉在类型层面不可能）。
//!
//! 协议无关（ADR-0019 最高原则）：本 crate 的公开 API 里**不出现**
//! SSH / SFTP / 隧道等协议专有概念。本地 PTY 会话由 [`SessionPool::open`]
//! 直接拉起；其余协议由应用层建好 `TermChannel` 经 [`SessionPool::open_with_channel`]
//! 注入——协议差异只存在于通道实现里，控制面对此一无所知。
//!
//! 线程模型（ADR-0019）：[`SessionPool`] 是 `&self` 的、可跨线程
//! 共享（UI 线程 / 脚本线程 / 桥接线程各持 `Arc<SessionPool>`）；
//! 每个 `Session` 包在自己的 `Mutex` 里，与 `term-io` 的
//! `Arc<Mutex>` 风格一致。零 `.await`、不声明 tokio（ADR-0017 笼子）。

pub mod sysmon;
pub mod sysmon_parse;

use std::collections::HashMap;
use std::fmt;
use std::path::PathBuf;
use std::sync::mpsc::Sender;
use std::sync::{Arc, Mutex, MutexGuard, OnceLock};

use term_core::channel::TermChannel;
use term_core::coords::ViewRange;
use term_core::grid::{GridSnapshot, ScreenImage};
use term_core::session::{ConfigError, Protocol, SessionConfig};
use term_core::session_runtime::{
    LogStatus, OutputReader, Session, SessionId, SessionState, SessionStatus, WaitError,
    WaitRegistration,
};
use term_io::supervisor::{
    spawn, spawn_exec, spawn_with_channel, RawControl, SupervisedSession, SupervisorHandle,
};

/// 应用层通道工厂的失败分类。
///
/// 控制面**不认识** SSH / SFTP / 隧道，但必须能把「为什么没建出通道」按
/// 类别送回调用方：这三类分别对应「去解锁」「改会话」「查网络」三种动作，
/// 拍平成字符串就会把脚本作者唯一能看到的那句话变成一团浆糊。
/// 这是分类，不是协议知识——三个名字里没有任何协议专有概念。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ChannelFailure {
    /// 凭据在保险库里，而这条路径没有解过锁。
    NeedsVault { session: String },
    /// 会话配置 / 认证方式不被支持，用户要改的是会话本身。
    Unsupported(String),
    /// 通道建立失败（DNS / TCP / 握手 / 认证被拒）。
    Connect(String),
}

impl fmt::Display for ChannelFailure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ChannelFailure::NeedsVault { session } => {
                write!(f, "会话 {session} 需要解锁保险库")
            }
            ChannelFailure::Unsupported(why) => f.write_str(why),
            ChannelFailure::Connect(why) => write!(f, "通道连接失败：{why}"),
        }
    }
}

impl std::error::Error for ChannelFailure {}

/// 通道工厂：把「按这份配置建一条字节通道」这件事交给应用层。
///
/// 之前控制面只会说 `BuiltinChannelNeedsInjection`，于是每个客户端
/// 都得自己 `match` 协议、自己拿凭据、自己调 `open_with_channel`——控制面
/// 号称「唯一会话所有者」，却把最重要的那类会话推给了调用方。
/// 现在工厂由应用层在启动时注册一次；控制面依旧零协议知识（签名里只有
/// `SessionConfig` 与 `TermChannel`），`open()` 成了所有客户端的唯一入口。
pub type ChannelFactory = dyn Fn(
        &SessionConfig,
        usize,
        usize,
    ) -> Result<Box<dyn TermChannel>, ChannelFailure>
    + Send
    + Sync;

/// `open` 的失败方式。每一条都是**给用户看的话**，不是给日志看的。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum OpenError {
    /// 会话配置不足以起会话（缺主机名 / 设备路径）。
    Config(ConfigError),
    /// 通道起不来（没有 `$SHELL`、外部客户端不存在……）。
    Spawn(String),
    /// 需要应用层注入通道、但工厂没装上来。
    ///
    /// 它只在「客户端还没注册工厂」时出现；脚本 / 桥接靠这一臂
    /// 明确报告「装忘了一步」，而不是看到一句含糊的连接失败。
    BuiltinChannelNeedsInjection,
    /// 工厂建通道失败，原因保留分类（见 [`ChannelFailure`]）。
    Channel(ChannelFailure),
}

impl fmt::Display for OpenError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            OpenError::Config(e) => write!(f, "{e}"),
            OpenError::Spawn(msg) => write!(f, "起会话失败：{msg}"),
            OpenError::BuiltinChannelNeedsInjection => f.write_str(
                "该会话需要应用层注入字节通道，但通道工厂尚未注册",
            ),
            OpenError::Channel(failure) => write!(f, "{failure}"),
        }
    }
}

impl std::error::Error for OpenError {}

/// 控制面里的一个条目：配置的名字（展示用）+ 会话本体。
struct Entry {
    name: String,
    session: Arc<Mutex<dyn Session>>,
    /// 原始字节旁路的控制端。会话本体在这里是 `dyn Session`，借不回
    /// `SupervisorHandle`；这一份在 `insert` 时就取好，透传旁路登记用。
    raw: RawControl,
    /// 非破坏性 tee 的控制端（hex 视图的字节来源）。与 `raw` 是同一个
    /// 控制端的两个克隆，分开存只为让「哪条通路归谁」在字段名上就看得出来
    /// ——两者语义相反（改道 vs 复制），混用一个字段最容易在改动时被搞错。
    tap: RawControl,
}

/// 会话控制面：`SessionId → Arc<Mutex<dyn Session>>` 的唯一登记处。
///
/// `&self` 的方法 + 内部 `Mutex`：一个 [`SessionPool`] 可以被 UI 线程、
/// 脚本线程、桥接线程同时持有（各持 `Arc`），无需外部加锁。
pub struct SessionPool {
    sessions: Mutex<HashMap<SessionId, Entry>>,
    /// 应用层注册的通道工厂，只装一次、装完只读（无需表锁）。
    factory: OnceLock<Arc<ChannelFactory>>,
}

/// ADR-0024：让会话名在池内唯一（名字是**寻址键**，歧义必须在插入时消掉）。
///
/// 规则：`base`、`base-2`、`base-3`… 与「同名文件不覆盖」同一条思路——
/// 既不静默顶掉别人的名字，也不报错让调用方自己想新名字（脚本里
/// `session_open("cat")` 连开两条是很自然的事）。
///
/// 后缀用 `-2` 而不是 `(2)`：名字要进 JSON 参数与脚本字符串，
/// 带空格的名字每次都要转义，是这个接口上不必要的摩擦。
///
/// 注意这是「遍历现有名字找第一个空位」，所以用户手工起的 `cat-2`
/// 也会被算作占用：再来一条 `cat` 会拿到 `cat-3`，不会撞车。
fn unique_session_name(map: &HashMap<SessionId, Entry>, base: &str) -> String {
    let taken = |candidate: &str| map.values().any(|e| e.name == candidate);
    if !taken(base) {
        return base.to_string();
    }
    let mut n = 2u32;
    loop {
        let candidate = format!("{base}-{n}");
        if !taken(&candidate) {
            return candidate;
        }
        n += 1;
    }
}

/// 取一把锁；**中毒时照常取回里面的数据**，不 panic。
///
/// 中毒只说明「曾经有线程持锁时 panic 过」，不说明登记表的条目或会话状态
/// 不能读。本 crate 跑在 UI / 脚本 / 桥接三条线程上，为这一条脏事实 abort
/// 整个 GUI，是拿最贵的代价换最小的确定性（与 `Capture::reader_mut`
/// 把「登记器缺失」从 expect 改成显式 `Closed` 同源）。
///
/// 登记表与每条会话的加锁**全部**经这里，不再留「锁中毒就 expect」的第二套
/// 写法：少一条漏网，就少一个「别的线程崩过之后所有按钮都炸」的入口。
fn lock_or_recover<T: ?Sized>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    match m.lock() {
        Ok(guard) => guard,
        Err(poisoned) => poisoned.into_inner(),
    }
}

impl SessionPool {
    pub fn new() -> Self {
        Self {
            sessions: Mutex::new(HashMap::new()),
            factory: OnceLock::new(),
        }
    }

    /// 注册通道工厂。第二次调用会明确失败，而不是悄悄换掉前一个实现——
    /// 两个客户端各装一份工厂，等于「同一个会话名两次跑出两种行为」，
    /// 那正是 ADR-0018 要禁的分叉。
    pub fn set_channel_factory(&self, factory: Arc<ChannelFactory>) -> Result<(), String> {
        self.factory
            .set(factory)
            .map_err(|_| "会话通道工厂已经注册过了".to_string())
    }

    /// 起一条会话的**唯一入口**。
    ///
    /// - PTY 承载的会话（本地 shell、系统 `ssh` / `telnet` / `nc` / `cu`）：
    ///   与这一层出现之前完全一致，直接 `spawn`；
    /// - 需要应用层注入通道的会话（内置 SSH）：调用 [`SessionPool::set_channel_factory`]
    ///   注册的工厂，拿回 `TermChannel` 后走同一个 `open_with_channel` 监管路径。
    ///
    /// **绝不静默退化成系统 `ssh`**（行为分叉），也绝不假装成功。没装工厂时
    /// 仍报 [`OpenError::BuiltinChannelNeedsInjection`]：脚本 / 桥接靠这个分类
    /// 说清「漏装了一步」，既有失败分类单测因此不动。
    pub fn open(&self, config: &SessionConfig, cols: usize, rows: usize) -> Result<SessionId, OpenError> {
        if config.protocol == Protocol::Ssh && config.builtin_ssh {
            let Some(factory) = self.factory.get() else {
                return Err(OpenError::BuiltinChannelNeedsInjection);
            };
            let channel = factory(config, cols, rows).map_err(OpenError::Channel)?;
            return Ok(self.open_with_channel(config, cols, rows, channel));
        }
        let handle = spawn(config.clone(), cols, rows)
            .map_err(|e| OpenError::Spawn(e.to_string()))?;
        Ok(self.insert(config.name.clone(), handle))
    }

    /// 起一个一次性命令会话（`desk local exec` 的落点，也是测试夹具）。
    pub fn open_exec(&self, command: &str, cols: usize, rows: usize) -> Result<SessionId, OpenError> {
        let handle = spawn_exec(command, cols, rows)
            .map_err(|e| OpenError::Spawn(e.to_string()))?;
        Ok(self.insert(command.to_string(), handle))
    }

    /// **协议注入点**：应用层（desk）建好通道递进来。原生 SSH 将来走这里
    /// ——`term-ssh::SshChannel` 实现 `TermChannel`，由应用层建立
    /// （凭据、主机密钥策略都是应用策略），控制面只管监管它。
    /// 与 `term_io::supervisor::spawn_with_channel` 同一接缝。
    pub fn open_with_channel(
        &self,
        config: &SessionConfig,
        cols: usize,
        rows: usize,
        channel: Box<dyn TermChannel>,
    ) -> SessionId {
        let handle = spawn_with_channel(config.clone(), cols, rows, channel);
        self.insert(config.name.clone(), handle)
    }

    fn insert(&self, name: String, handle: SupervisorHandle) -> SessionId {
        // 先取旁路控制端再交出 handle：`session()` 借出去的是 `dyn Session`，
        // 之后没有第二条路能拿回具体句柄。
        let raw = handle.raw_control();
        let tap = handle.raw_control();
        let session: Arc<Mutex<dyn Session>> = Arc::new(Mutex::new(SupervisedSession::new(handle)));
        let id = lock_or_recover(&session).id();
        let mut map = self.entries();
        // ADR-0024：名字是**寻址键**，所以必须池内唯一。冲突时按
        // `base`、`base-2`、`base-3`… 排下去——与「同名文件不覆盖」同一条
        // 思路：不静默顶掉别人的名字，也不报错让调用方自己去想新名字
        // （脚本里 `session_open("cat")` 连开两条是很自然的事）。
        let name = unique_session_name(&map, &name);
        map.insert(
            id,
            Entry {
                name,
                session,
                raw,
                tap,
            },
        );
        id
    }

    fn with<R>(&self, id: SessionId, f: impl FnOnce(&mut dyn Session) -> R) -> Option<R> {
        let map = self.entries();
        let entry = map.get(&id)?;
        let mut guard = lock_or_recover(&entry.session);
        Some(f(&mut *guard))
    }

    /// 往会话写字节。会话不存在返回 `false`（调用方自行决定怎么呈现）。
    pub fn send(&self, id: SessionId, bytes: &[u8]) -> bool {
        self.with(id, |s| s.write(bytes)).is_some()
    }

    /// **原始字节旁路**：把这条会话的输出整块改道到 `sink`（字节级传输协议
    /// X/Y/Z Modem 的入口）。返回 `false` = 会话不在册（不是「登记失败」）。
    ///
    /// 语义全在会话线程里，这里只是透传；三条都不是 bug：
    /// 1. **屏幕冻结是对的**——旁路期间字节不进 VT，快照不更新、`version`
    ///    不涨。传输进度由界面自己画，屏幕本来就不该动。
    /// 2. **结束时恢复**：调 [`detach_raw`](Self::detach_raw) 之后到达的字节
    ///    重新进 VT。
    /// 3. **对话回包不参与**——VT 没吃到字节，自然没有回包，这里也不补写
    ///    任何东西到通道。
    pub fn attach_raw(&self, id: SessionId, sink: Sender<Vec<u8>>) -> bool {
        // 先把控制端克隆出来再放开表锁：`attach` 要等会话线程的 ack，
        // 攥着表锁等会挡住其它控制面调用。
        let raw = {
            let map = self.entries();
            match map.get(&id) {
                Some(e) => e.raw.clone(),
                None => return false,
            }
        };
        raw.attach(sink);
        true
    }

    /// 撤销原始字节旁路：之后的字节重新进 VT（屏幕恢复更新）。返回 `false`
    /// = 会话不在册。
    pub fn detach_raw(&self, id: SessionId) -> bool {
        let raw = {
            let map = self.entries();
            match map.get(&id) {
                Some(e) => e.raw.clone(),
                None => return false,
            }
        };
        raw.detach();
        true
    }

    /// **非破坏性 tee**：把这条会话输出的一份拷贝喂给 `sink`（hex 视图的
    /// 字节来源）。返回 `false` = 会话不在册（不是「登记失败」）。
    ///
    /// 与 [`attach_raw`](Self::attach_raw) 语义**相反**，两句话就能分清：
    /// - raw 是**改道**：VT 不吃，屏幕冻结（字节级协议用）；
    /// - tap 是**复制**：VT 照吃，屏幕正常前进（hex 视图用）。
    ///
    /// 阻塞到会话线程 ack 为止——调用方拿到 `true` 之后才清本地环，
    /// 「挂上之后再清」的顺序才不会丢字节。
    pub fn attach_tap(&self, id: SessionId, sink: Sender<Vec<u8>>) -> bool {
        // 同 attach_raw：先克隆控制端再放开表锁，攥着表锁等 ack 会挡住
        // 其它控制面调用。
        let tap = {
            let map = self.entries();
            match map.get(&id) {
                Some(e) => e.tap.clone(),
                None => return false,
            }
        };
        tap.attach_tap(sink);
        true
    }

    /// 摘掉 tee。VT 从未停过，所以没有生效时刻需要对（幂等）。返回 `false`
    /// = 会话不在册。
    pub fn detach_tap(&self, id: SessionId) -> bool {
        let tap = {
            let map = self.entries();
            match map.get(&id) {
                Some(e) => e.tap.clone(),
                None => return false,
            }
        };
        tap.detach_tap();
        true
    }

    /// 当前屏幕快照；`None` = 会话不存在或已结束到没有事实可取。
    pub fn screen(&self, id: SessionId) -> Option<GridSnapshot> {
        self.with(id, |s| s.screen())?
    }

    pub fn state(&self, id: SessionId) -> Option<SessionState> {
        self.with(id, |s| s.state())
    }

    pub fn facets(&self, id: SessionId) -> Option<Vec<term_core::session_runtime::FacetKind>> {
        self.with(id, |s| s.facets())
    }

    /// 登记一次输出等待（腿 A 脚本的原语；见 `term_core::session_runtime`）。
    pub fn wait_register(
        &self,
        id: SessionId,
        pattern: &str,
    ) -> Result<Box<dyn WaitRegistration>, WaitError> {
        let reg = self
            .with(id, |s| s.wait_register(pattern))
            .ok_or(WaitError::Closed)??;
        Ok(reg)
    }

    /// 登记一个输出读取器（`read` / `read_until` 的捕获源；先登记后消费）。
    pub fn reader_register(&self, id: SessionId) -> Result<Box<dyn OutputReader>, WaitError> {
        let reg = self
            .with(id, |s| s.reader_register())
            .ok_or(WaitError::Closed)??;
        Ok(reg)
    }

    /// **脚本线程接缝**：把会话本体借出去（克隆 `Arc`，不转移所有权）。
    ///
    /// 为什么需要这条缝：`term_script::ScriptEngine::run` 要的是
    /// `Arc<Mutex<dyn Session>>`——脚本原语（`send` / `wait` / `read`）由 Rhai
    /// 的全局函数闭包捕获会话后逐个调用，中间隔着解释器，没法折成
    /// `&self` 的控制面调用。
    ///
    /// 边界（ADR-0019）：拿到的仍是**注册表里那一个**会话对象（同一个
    /// `Arc`），不是副本；关闭、列举、状态查询仍然只有控制面能做。
    /// 换句话说这条缝让脚本线程**用**会话，不让它**拥有**会话。
    /// 会话不存在返回 `None`——不悄悄新建一个来顶替。
    pub fn session(&self, id: SessionId) -> Option<Arc<Mutex<dyn Session>>> {
        let map = self.entries();
        Some(Arc::clone(&map.get(&id)?.session))
    }

    /// 控制面清单：`(id, 会话名, 状态)`。桥接的 `session_list` 直接用它。
    pub fn list(&self) -> Vec<(SessionId, String, SessionState)> {
        let map = self.entries();
        let mut rows: Vec<_> = map
            .iter()
            .map(|(id, e)| {
                let st = lock_or_recover(&e.session).state();
                (*id, e.name.clone(), st)
            })
            .collect();
        rows.sort_by_key(|(id, _, _)| *id);
        rows
    }

    /// 按**会话名**查 id（ADR-0024 的寻址入口）。
    ///
    /// 名字池内唯一（[`SessionPool::insert`] 保证），所以这里不需要处理歧义。
    /// 查不到返回 `None`——不模糊匹配、不忽略大小写：寻址打空必须报错，
    /// 「差一个字符却打中了别的会话」是这类 API 最贵的错。
    pub fn resolve(&self, name: &str) -> Option<SessionId> {
        let map = self.entries();
        map.iter()
            .find(|(_, e)| e.name == name)
            .map(|(id, _)| *id)
    }

    /// 会话的**寻址名**（开会话时定下，唯一，之后不随界面改名而变）。
    ///
    /// 为什么要单独一个查询：`open*` 返回的是 id，而唯一化可能把 `cat` 变成
    /// `cat-2`——调用方必须拿真名回去寻址，否则会打中另一条会话。
    pub fn name(&self, id: SessionId) -> Option<String> {
        self.entries().get(&id).map(|e| e.name.clone())
    }

    /// 取登记表锁；中毒时照常取回数据（见 [`lock_or_recover`]）。
    ///
    /// 2026-11-15 起全表的读 / 写路径统一走这里：此前只有寻址两条
    /// （[`resolve`](Self::resolve) / [`name`](Self::name)）不 panic，其余
    /// 13 处「锁中毒就 expect」会在别的线程崩过之后把 GUI 一起带走。
    fn entries(&self) -> MutexGuard<'_, HashMap<SessionId, Entry>> {
        lock_or_recover(&self.sessions)
    }

    // ── 驱动界面需要的门面（ADR-0019：desk 不再自己持句柄） ─────
    //
    // 这一组是「只有界面要」的方法：脚本与桥接读屏幕事实就够，不需要推事件、
    // 不需要开日志、不需要每帧读缓存快照。它们放在控制面上，是为了让
    // `apps/desktop` 的 tab 只持一个 `SessionId`——句柄只有一个主人。

    /// 事件泵：把本帧到达的事件并进会话，返回**是否有新事实**。
    /// `None` = 会话不在册。
    pub fn poll(&self, id: SessionId) -> Option<bool> {
        self.with(id, |s| s.poll())
    }

    /// 上一次 [`poll`](SessionPool::poll) 之后缓存的快照——**不产生往返**。
    ///
    /// 与 [`screen`](SessionPool::screen) 的区别是代价：那个要去问
    /// 会话线程要一屏（发命令 + 等应答），这个只读已经收下的那一帧。
    /// 界面每帧用它；脚本 / 桥接用 `screen`。
    pub fn snapshot(&self, id: SessionId) -> Option<GridSnapshot> {
        self.with(id, |s| s.snapshot())?
    }

    /// 运行时状态（连接 / 存活 / 退出文本）。`None` = 会话不在册。
    pub fn status(&self, id: SessionId) -> Option<SessionStatus> {
        self.with(id, |s| s.status())
    }

    /// 调整终端尺寸。返回 `false` = 会话不在册（不是「尺寸没变」）。
    pub fn resize(&self, id: SessionId, cols: usize, rows: usize) -> bool {
        self.with(id, |s| s.resize(cols, rows)).is_some()
    }

    /// 请求任意行区间的只读事实窗口（回看 / 选区 / 查找共用）。
    /// 结果从 [`screen_window`](SessionPool::screen_window) 取。
    pub fn request_range(&self, id: SessionId, range: ViewRange) -> bool {
        self.with(id, |s| s.request_range(range)).is_some()
    }

    /// P：请求一份预览用只读事实窗口（悬停滚动条时的历史快照）。第二通道，
    /// 与 [`request_range`](SessionPool::request_range) 物理隔离（ADR-0028）。
    /// 结果从 [`preview_window`](SessionPool::preview_window) 取。
    pub fn request_preview_range(&self, id: SessionId, range: ViewRange) -> bool {
        self.with(id, |s| s.request_preview_range(range)).is_some()
    }

    /// 上一次范围请求的结果。`None` = 会话不在册，或这条会话不提供回看。
    pub fn screen_window(&self, id: SessionId) -> Option<ScreenImage> {
        self.with(id, |s| s.screen_window())?
    }

    /// P：上一次预览范围请求的结果。`None` = 会话不在册，或这条会话不提供预览。
    pub fn preview_window(&self, id: SessionId) -> Option<ScreenImage> {
        self.with(id, |s| s.preview_window())?
    }

    /// 注入唤醒回调：有输出时叫醒界面。返回 `false` = 会话不在册。
    pub fn set_wakeup(&self, id: SessionId, wake: Arc<dyn Fn() + Send + Sync>) -> bool {
        self.with(id, |s| s.set_wakeup(wake)).is_some()
    }

    /// 清屏。返回 `false` = 会话不在册。
    pub fn clear(&self, id: SessionId) -> bool {
        self.with(id, |s| s.clear()).is_some()
    }

    /// `Clear Scrollback`：只丢历史，屏幕留着。
    pub fn clear_scrollback(&self, id: SessionId) -> bool {
        self.with(id, |s| s.clear_scrollback()).is_some()
    }

    /// `Clear All`：清屏 + 清历史。
    pub fn clear_all(&self, id: SessionId) -> bool {
        self.with(id, |s| s.clear_all()).is_some()
    }

    /// 会话日志的当前状态。
    pub fn log_status(&self, id: SessionId) -> Option<LogStatus> {
        self.with(id, |s| s.log_status())
    }

    /// 开始记录到 `dir` 下。文件名 / 行格式 / 覆盖方式 / 大小上限在 `opts` 里
    /// （配置面只有 `term_core::log_opts` 一份）。
    pub fn start_log(
        &self,
        id: SessionId,
        dir: PathBuf,
        session: &str,
        opts: &term_core::log_opts::LogOptions,
    ) -> bool {
        self.with(id, |s| s.start_log(dir, session, opts)).is_some()
    }

    pub fn stop_log(&self, id: SessionId) -> bool {
        self.with(id, |s| s.stop_log()).is_some()
    }

    pub fn pause_log(&self, id: SessionId) -> bool {
        self.with(id, |s| s.pause_log()).is_some()
    }

    pub fn resume_log(&self, id: SessionId) -> bool {
        self.with(id, |s| s.resume_log()).is_some()
    }

    /// 关闭并移除一个会话。返回它之前是否存在。
    pub fn close(&self, id: SessionId) -> bool {
        let mut map = self.entries();
        match map.remove(&id) {
            Some(entry) => {
                lock_or_recover(&entry.session).close();
                true
            }
            None => false,
        }
    }
}

impl Default for SessionPool {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use term_core::coords::ViewRow;
    use std::time::Duration;

    #[test]
    fn a_poisoned_table_still_serves_every_path() {
        // 锁中毒不是「表坏了」：只是曾经有线程持锁时 panic 过，数据仍在。
        // 2026-11-15 前只有 resolve / name 两条能穿过它，其余 13 处会把 GUI
        // 一起带走。这里把表侧 / 会话侧 / 写路径各走一遍，钉死收口后的行为。
        let pool = Arc::new(SessionPool::new());
        let mut cfg = SessionConfig::new(Protocol::Shell);
        cfg.name = "poison-me".to_string();
        let id = pool.open(&cfg, 80, 24).expect("起 shell 会话");

        let p = Arc::clone(&pool);
        let joined = std::thread::spawn(move || {
            let _guard = p.sessions.lock().unwrap();
            panic!("故意毒锁");
        })
        .join();
        assert!(joined.is_err(), "这条线程必须真的 panic 过");
        assert!(pool.sessions.is_poisoned(), "登记表锁确实中毒");

        // 表侧：列举 / 按名寻址 / 按 id 取名字 / 借出会话。
        assert_eq!(pool.list().len(), 1);
        assert_eq!(pool.resolve("poison-me"), Some(id));
        assert_eq!(pool.name(id).as_deref(), Some("poison-me"));
        assert!(pool.session(id).is_some());

        // 会话侧：with() 连着两把锁（表 + 会话本体）都要能穿过。
        assert!(pool.state(id).is_some());
        assert!(pool.send(id, b"echo ok\n"));

        // 写路径：中毒的表也照样能摘条目，不是只读降级。
        assert!(pool.close(id));
        assert!(pool.list().is_empty());
    }

    #[test]
    fn an_exec_session_flows_end_to_end() {
        // 第一刀的验收形状：起会话 → wait 命中 → 屏幕上有事实 → 关闭。
        let pool = SessionPool::new();
        let id = pool
            .open_exec("printf 'hello ctl-plane'", 80, 24)
            .expect("起 exec 会话");

        let w = pool
            .wait_register(id, "hello ctl-plane")
            .expect("登记等待");
        let m = w.wait(Duration::from_secs(5)).expect("命中");
        assert_eq!(m.text_upto, "hello ctl-plane");

        // 命中后屏幕上也应有同样的文本（快照直通，不依赖 UI 的邮箱）。
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        loop {
            if pool
                .screen(id)
                .is_some_and(|s| s.screen_text().contains("hello ctl-plane"))
            {
                break;
            }
            assert!(std::time::Instant::now() < deadline, "屏幕上没等到输出");
            std::thread::sleep(Duration::from_millis(10));
        }
        // 一次性命令在 wait 命中时往往已经退出：Alive / Finished 都合法，
        // 唯独不该「查不到状态」——会话必须还在控制面的登记里。
        assert!(pool.state(id).is_some(), "状态查询应有响应");
        assert!(pool.list().iter().any(|(i, _, _)| *i == id), "会话应在册");
        assert!(pool.close(id), "关闭存在的会话应返回 true");
        assert!(!pool.list().iter().any(|(i, _, _)| *i == id), "关闭后应移出");
    }

    #[test]
    fn a_local_shell_session_lists_with_its_config_name() {
        let pool = SessionPool::new();
        let mut cfg = SessionConfig::new(Protocol::Shell);
        cfg.name = "ctl-shell".to_string();
        let id = pool.open(&cfg, 80, 24).expect("起 shell 会话");

        let rows = pool.list();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].0, id);
        assert_eq!(rows[0].1, "ctl-shell");
        assert_eq!(rows[0].2, SessionState::Alive);
        pool.close(id);
    }

    #[test]
    fn builtin_ssh_is_refused_instead_of_silently_degraded() {
        // 行为分叉是 ADR-0018 修掉的 bug 的根源形态：这里钉住「内置 SSH
        // 不许静默换成系统 ssh」——控制面直接拒绝，等应用层注入通道。
        let pool = SessionPool::new();
        let cfg = SessionConfig::new(Protocol::Ssh); // builtin_ssh = true
        let err = pool.open(&cfg, 80, 24).unwrap_err();
        assert_eq!(err, OpenError::BuiltinChannelNeedsInjection);
        assert!(pool.list().is_empty(), "拒绝后不得留下半个会话");
    }

    #[test]
    fn operations_on_an_unknown_session_are_honest_nones() {
        let pool = SessionPool::new();
        assert!(!pool.send(999, b"x"));
        assert!(pool.screen(999).is_none());
        assert!(pool.state(999).is_none());
        assert!(pool.facets(999).is_none());
        let err = match pool.wait_register(999, "x") {
            Err(e) => e,
            Ok(_) => panic!("未知会话的登记应当失败"),
        };
        assert_eq!(err, WaitError::Closed);
        // 界面门面同一套口径：查不到就是 None / false，不造默认值。
        assert!(pool.poll(999).is_none());
        assert!(pool.snapshot(999).is_none());
        assert!(pool.status(999).is_none());
        assert!(pool.log_status(999).is_none());
        assert!(pool.screen_window(999).is_none());
        assert!(!pool.resize(999, 80, 24));
        assert!(!pool.request_range(999, ViewRange::new(ViewRow::new(0), ViewRow::new(1))));
        assert!(!pool.clear(999));
        assert!(!pool.stop_log(999));
        assert!(!pool.close(999));
        // 原始字节旁路对未知会话同样是诚实的 false（不是「登记失败」）。
        let (sink, _rx) = std::sync::mpsc::channel();
        assert!(!pool.attach_raw(999, sink));
        assert!(!pool.detach_raw(999));
        // tee 同一口径：会话不在册就是诚实的 false。
        let (sink, _rx) = std::sync::mpsc::channel();
        assert!(!pool.attach_tap(999, sink));
        assert!(!pool.detach_tap(999));
    }

    #[test]
    fn the_frame_pump_caches_the_snapshot_without_a_round_trip() {
        // 界面每帧走的是 poll → snapshot：poll 说有新事实才取快照，
        // 取的是已经收下的那一帧（不去问会话线程要）。
        let pool = SessionPool::new();
        let id = pool
            .open_exec("printf 'alpha\\n'", 80, 24)
            .expect("起会话");

        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        loop {
            if pool.poll(id).unwrap_or(false)
                && pool
                    .snapshot(id)
                    .is_some_and(|s| s.screen_text().contains("alpha"))
            {
                break;
            }
            assert!(std::time::Instant::now() < deadline, "屏幕上没等到 alpha");
            std::thread::sleep(Duration::from_millis(10));
        }

        // 泵完之后 snapshot 仍然给得出同一帧：它是缓存读，不是一次性事件。
        assert!(
            pool.snapshot(id).is_some_and(|s| s.screen_text().contains("alpha")),
            "缓存快照不该在 poll 之后失效"
        );
        // 状态也得能报（这条会话是 exec，退出后 connected 仍为真）。
        let status = pool.status(id).expect("状态");
        assert!(status.connected, "起过 PTY 就该是 connected");
        pool.close(id);
    }

    #[test]
    fn the_log_switch_really_opens_a_file_and_reports_it() {
        // 「已接线＝有效果」：开关返回 true 不算，文件得真的在，状态得真的变。
        let pool = SessionPool::new();
        // 用活得住的命令：`printf` 会立刻退出，`LogStart` 就赶不上会话线程了。
        let id = pool.open_exec("cat", 80, 24).expect("起会话");

        let dir = std::env::temp_dir().join(format!("term-session-log-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let opts = term_core::log_opts::LogOptions::default();
        assert!(pool.start_log(id, dir.clone(), "unit", &opts), "开关应当被受理");

        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        loop {
            // 关键：日志状态是**事件**报上来的，必须经 `poll` 才会进缓存——
            // 这正好证明「状态由泵推动」而不是每帧去问会话线程。
            pool.poll(id);
            if pool.log_status(id).is_some_and(|s| {
                s.is_recording() && s.path.as_ref().is_some_and(|p| p.exists())
            }) {
                break;
            }
            assert!(std::time::Instant::now() < deadline, "日志文件没开出来");
            std::thread::sleep(Duration::from_millis(10));
        }

        assert!(pool.stop_log(id));
        pool.close(id);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_local_pty_reports_no_capabilities() {
        // 协议无关原则的钉子：能力是会话报告的 facet，控制面不假设。
        let pool = SessionPool::new();
        let id = pool.open_exec("true", 80, 24).expect("起会话");
        assert!(pool.facets(id).unwrap().is_empty());
        pool.close(id);
    }

    #[test]
    fn the_script_seam_hands_out_the_same_session_not_a_copy() {
        // 脚本线程拿到的是登记表里那一个对象：同 id，且经它写进去的字节
        // 从控制面看得见。若这里换成「拷一份配置新建会话」，第二条断言必挂。
        let pool = SessionPool::new();
        let id = pool
            .open_exec("cat", 80, 24)
            .expect("起 cat 会话（回显便于观察写入）");

        let handle = pool.session(id).expect("借出会话");
        assert_eq!(handle.lock().expect("会话锁").id(), id, "必须是同一个会话");

        handle.lock().expect("会话锁").write(b"seam\n");

        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        loop {
            if pool
                .screen(id)
                .is_some_and(|s| s.screen_text().contains("seam"))
            {
                break;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "经借出的句柄写入的字节，控制面上应看得见"
            );
            std::thread::sleep(Duration::from_millis(10));
        }
        pool.close(id);
        assert!(
            pool.session(id).is_none(),
            "关闭之后这条缝也要跟着关上，不许借出一个孤儿"
        );
    }

    // ── 控制面本体认「需要注入通道」的会话 ──────────────────────

    /// 测试用的假通道：只在第一次 `drain` 给一行字，之后安静。
    /// 它不认识任何协议——控制面就该只见到这种东西。
    struct StubChannel {
        banner: Option<&'static [u8]>,
        closed: bool,
    }

    impl TermChannel for StubChannel {
        fn drain(&mut self) -> Vec<u8> {
            self.banner.take().map(|b| b.to_vec()).unwrap_or_default()
        }
        fn write(&mut self, _bytes: &[u8]) {}
        fn resize(&mut self, _cols: usize, _rows: usize) {}
        fn poll(&mut self) -> Option<String> {
            None
        }
        fn finished(&self) -> bool {
            self.closed
        }
        fn close(&mut self) {
            self.closed = true;
        }
        fn description(&self) -> &str {
            "stub channel"
        }
    }

    fn stub_factory() -> Arc<ChannelFactory> {
        Arc::new(|_cfg, _cols, _rows| {
            Ok(Box::new(StubChannel {
                banner: Some(b"built by factory\r\n"),
                closed: false,
            }) as Box<dyn TermChannel>)
        })
    }

    #[test]
    fn the_pool_opens_a_builtin_session_through_the_registered_factory() {
        // 正题：客户端不必自己 match 协议、不必自己调
        // `open_with_channel`，`open()` 一个入口就能把内置会话开起来。
        let pool = SessionPool::new();
        pool.set_channel_factory(stub_factory()).expect("首次注册");
        let mut cfg = SessionConfig::new(Protocol::Ssh);
        cfg.name = "factory-ssh".to_string();
        let id = pool.open(&cfg, 80, 24).expect("工厂建出的通道应当被监管起来");

        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        loop {
            if pool
                .screen(id)
                .is_some_and(|s| s.screen_text().contains("built by factory"))
            {
                break;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "工厂通道的字节必须真的进 VT"
            );
            std::thread::sleep(Duration::from_millis(10));
        }
        assert_eq!(pool.list().len(), 1);
        assert_eq!(pool.list()[0].1, "factory-ssh");
        pool.close(id);
    }

    #[test]
    fn registering_a_second_factory_is_refused() {
        // 两个客户端各装一份 = 同名会话两种行为（ADR-0018 的分叉）。
        let pool = SessionPool::new();
        assert!(pool.set_channel_factory(stub_factory()).is_ok());
        assert!(pool.set_channel_factory(stub_factory()).is_err());
    }

    #[test]
    fn factory_failures_keep_their_category_and_leave_no_session() {
        let pool = SessionPool::new();
        pool.set_channel_factory(Arc::new(|cfg, _cols, _rows| {
            Err(ChannelFailure::NeedsVault {
                session: cfg.name.clone(),
            })
        }))
        .expect("注册");
        let mut cfg = SessionConfig::new(Protocol::Ssh);
        cfg.name = "prod".to_string();
        let err = pool.open(&cfg, 80, 24).unwrap_err();
        assert_eq!(
            err,
            OpenError::Channel(ChannelFailure::NeedsVault {
                session: "prod".to_string()
            })
        );
        assert!(err.to_string().contains("prod"), "{err}");
        assert!(pool.list().is_empty(), "失败不许留半个会话");
    }

    #[test]
    fn without_a_factory_the_old_failure_category_is_unchanged() {
        // `desk script` / `desk serve` 靠这一臂报告「漏装了一步」；
        // 既有失败分类单测因此一条都不用改。
        let pool = SessionPool::new();
        let cfg = SessionConfig::new(Protocol::Ssh);
        assert_eq!(
            pool.open(&cfg, 80, 24).unwrap_err(),
            OpenError::BuiltinChannelNeedsInjection
        );
    }
}

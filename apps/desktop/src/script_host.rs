//! 脚本的**池级会话集合**：把 `SessionPool` 接成 `term_script::Sessions`。
//!
//! 这一层只在应用层（desk）存在，理由与 `session_open` 相同：`term-script`
//! 声明 `Sessions`，但它不认识 `SessionPool`，也不认识「内置 SSH 的通道工厂装在哪」
//! ——那些是应用策略（ADR-0019 §B.14 / I4 单向依赖）。
//!
//! # 谁在用
//!
//! - `cli/script.rs`：`desk script run` 的进程级会话（自己开池、跑完就退）；
//! - `script_run.rs`：界面里的「运行脚本」（跑在用户已有的池子上，`session_list`
//!   看到的就是窗口里的会话）。
//!
//! 两处共用同一份实现，所以「脚本眼里的会话」与「桥接眼里的会话」是同一张
//! 清单——这正是 ADR-0023 要的：一个动词，两个前端，同一份事实。
//!
//! # 语义：新开的会话归调用方所有
//!
//! `open` 只负责建会话，**不**负责关。脚本结束后由引擎的集合实现不管；
//! 进程级那条路是调用方（`cli/script.rs`）在收尾时把池子里剩下的会话一起清掉，
//! 界面那条路则交给用户——脚本开出来的会话就该留在窗口里，用户能看见、
//! 能继续用。这条规则写在 `Sessions::open` 的文档里，不靠调用方猜。
//!
//! ADR-0025 阶段 5 起，界面那条路的「能看见」是**真的**：新开的会话经
//! [`PooledSessions::with_opened`] 递回窗口，下一帧就变成一条标签页。

use std::collections::hash_map::DefaultHasher;
use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use term_core::session::SessionConfig;
use term_core::session_runtime::{Session, SessionId};
use term_core::session::ProfileCatalog;
use term_script::{RemoteFiles, Sessions, SessionsError, SessionInfo};
use term_session::SessionPool;

use crate::session_open::{open_request, OpenFailure};

/// `SessionPool` 之上的一层薄适配。
pub struct PooledSessions {
    pool: Arc<SessionPool>,
    /// 界面侧「脚本新开了哪些会话」的队列（`None` = 进程级，没人看）。
    opened: Option<OpenedQueue>,
    /// 界面侧「把哪条会话带到前台」的队列（`None` = 没有界面）。
    activated: Option<ActivatedQueue>,
    /// 界面当前激活的会话（`session_active` 的事实来源）。
    current: Option<Arc<Mutex<Option<SessionId>>>>,
    /// 已知的会话配置：`session_open` 时登记，界面启动时用窗口里的标签播种。
    ///
    /// 为什么需要另存一份：`SessionPool` 只登记 `(id, name, state)`，配置在
    /// **建它的那一侧**（界面标签 / 脚本集合）。而 `session_clone`、`%{host}`、
    /// SFTP 都要「怎么连」这份材料，事后从池子拿不回来。
    configs: Mutex<HashMap<SessionId, SessionConfig>>,
}

/// 界面侧收「脚本新开了哪些会话」的队列：`(会话 id, 建它用的配置)`。
///
/// 为什么要把 `SessionConfig` 一起带过去：标签页不是「会话 id 的一行字」，
/// 它要 `config` 来画标题（进程已退出时仍要有名字）、来重连、来喂
/// `Session ▸ Reconnect`。而池子只登记 `(id, name, state)`（`SessionPool::list`），
/// 拿不回建会话时的那份配置——所以由**建会话的那一侧**顺手递出来。
///
/// 队列本身是 `Mutex<Vec<_>>` 而不是 channel：消费者是界面每帧 drain 一次，
/// 生产者是脚本线程（可能开多条）。channel 要多一个「关掉时谁负责收尾」的
/// 约定，Vec 在锁里 push/take 就够了，而且丢了也没人误以为成功。
pub type OpenedQueue = Arc<Mutex<Vec<(SessionId, SessionConfig)>>>;

/// 界面侧收「把这条会话带到前台」的队列。
///
/// 与 [`OpenedQueue`] 同一种形状、同一种理由：`session_activate` 是脚本发的
/// 动作，真正切标签是界面的事，中间需要一条不吃阻塞的交接。
pub type ActivatedQueue = Arc<Mutex<Vec<SessionId>>>;

impl PooledSessions {
    /// 全功能集合：能列、能开、能关。给进程级那条路（`desk script run`）用。
    pub fn new(pool: Arc<SessionPool>) -> Self {
        Self {
            pool,
            opened: None,
            activated: None,
            current: None,
            configs: Mutex::new(HashMap::new()),
        }
    }

    /// 把「新开了哪些会话」回报给界面（界面那条路用）。
    ///
    /// ADR-0025 阶段 5：`session_open` 在界面里**不再被拒**。此前这里有一道
    /// `can_open = false` 的闸（旧 `PooledSessions::list_only`），理由是「标签栏渲染的
    /// 不是池子清单，脚本开出来的会话会变成用户看不见也关不掉的隐形会话」。
    /// 那个理由成立，但解法不该是「少一条能力」——现在标签栏接过了这份队列：
    /// 脚本开一条，界面下一帧就把它变成真标签页（能看、能点、能关），于是
    /// 闸门的**前提**消失了，闸门本身也就不必留。
    ///
    /// 进程级 `desk script run` 不调它：那条路自己开池、跑完就退，没有界面在等。
    pub fn with_opened(mut self, opened: OpenedQueue) -> Self {
        self.opened = Some(opened);
        self
    }

    /// 接上「带到前台」的队列与「当前激活」的事实来源（界面那条路用）。
    pub fn with_window(
        mut self,
        activated: ActivatedQueue,
        current: Arc<Mutex<Option<SessionId>>>,
    ) -> Self {
        self.activated = Some(activated);
        self.current = Some(current);
        self
    }

    /// 登记一条已知配置（界面启动时用窗口里的标签播种）。
    ///
    /// 重复登记以**后者**为准：配置可能被用户改过（改了主机名再重连），
    /// 旧的那份已经没有意义。
    pub fn remember(&self, id: SessionId, config: SessionConfig) {
        self.config_store().insert(id, config);
    }

    fn config_store(&self) -> std::sync::MutexGuard<'_, HashMap<SessionId, SessionConfig>> {
        match self.configs.lock() {
            Ok(guard) => guard,
            // 中毒只说明「某个线程在持锁时崩过」，表本身还能用。
            Err(poisoned) => poisoned.into_inner(),
        }
    }

    /// 一条会话的对外描述。名字与状态都向池子要——池子是唯一登记表，
    /// 不在这里另存一份（另存就要处理两份不一致）。
    fn describe(&self, id: SessionId) -> Option<SessionInfo> {
        let name = self.pool.name(id)?;
        let state = self.pool.state(id)?;
        let mut info = SessionInfo::from_state(id, name, state);
        info.local_addr = self.addr(id);
        Some(info)
    }

    /// 阶段 5：把新开的会话递回界面。没人等（进程级那条路）就什么也不做。
    ///
    /// 排队失败**不让 `open` 失败**：会话已经在池子里活着、`session_list` 看得见，
    /// 脚本完全没有做错什么；丢的只是「界面上多一个标签页」这件事。让脚本因为
    /// 一条 UI 队列而报错，等于把「显示不出来」说成「开不了」。
    fn announce_opened(&self, id: SessionId, config: SessionConfig) {
        let Some(queue) = &self.opened else {
            return;
        };
        if let Ok(mut rows) = queue.lock() {
            rows.push((id, config));
        }
    }
}

impl Sessions for PooledSessions {
    fn list(&self) -> Vec<SessionInfo> {
        self.pool
            .list()
            .into_iter()
            .map(|(id, name, state)| SessionInfo::from_state(id, name, state))
            .collect()
    }

    /// 名字 → id。池子是唯一的登记表，所以这里没有任何附加规则。
    fn resolve(&self, name: &str) -> Option<SessionId> {
        self.pool.resolve(name)
    }

    /// 池里给这条会话定的寻址名。
    fn name(&self, id: SessionId) -> Option<String> {
        self.pool.name(id)
    }

    fn open(&self, request: &term_core::session::OpenRequest) -> Result<SessionInfo, SessionsError> {
        // 尺寸 0 的兜底在 `open_request` 里（它那四条分支各自决定缺省值），
        // 这里只做「开 + 登记 + 递回」。
        let (id, config) = open_request(&self.pool, request).map_err(spawn_failure)?;
        self.remember(id, config.clone());
        self.announce_opened(id, config);
        self.describe(id).ok_or_else(|| {
            SessionsError::Refused(format!(
                "会话 {id} 已经建出来了，但控制面里查不到它（登记表与池子不一致）"
            ))
        })
    }

    fn close(&self, name: &str) -> Result<(), SessionsError> {
        // 先按名字解析：关的是「名字指向的那条会话」，不是「调用方以为的那条」。
        let Some(id) = self.pool.resolve(name) else {
            return Err(SessionsError::NotFound(name.to_string()));
        };
        if self.pool.close(id) {
            self.config_store().remove(&id);
            Ok(())
        } else {
            Err(SessionsError::NotFound(name.to_string()))
        }
    }

    fn attach(&self, id: SessionId) -> Option<Arc<Mutex<dyn Session>>> {
        self.pool.session(id)
    }

    fn activate(&self, id: SessionId) -> Result<(), SessionsError> {
        let Some(queue) = &self.activated else {
            return Err(SessionsError::Unsupported(
                "当前宿主没有界面，无法把会话带到前台".to_string(),
            ));
        };
        if self.pool.name(id).is_none() {
            return Err(SessionsError::NotFound(format!("#{id}")));
        }
        if let Ok(mut rows) = queue.lock() {
            rows.push(id);
        }
        Ok(())
    }

    /// 复制一条会话：拿原配置重开一条（名字由池子唯一化成 `name-2`）。
    ///
    /// 不复制**运行态**（滚动位置 / 游标）：那些属于原来那条会话，
    /// 新会话从干净的屏幕开始——CRT 的 Duplicate Session 也是这个语义。
    fn clone_session(&self, id: SessionId) -> Result<SessionInfo, SessionsError> {
        let Some(config) = self.config_store().get(&id).cloned() else {
            let name = self
                .pool
                .name(id)
                .ok_or_else(|| SessionsError::NotFound(format!("#{id}")))?;
            return Err(SessionsError::Refused(format!(
                "不知道会话 {name:?} 是怎么连的（没有它的配置），无法复制"
            )));
        };
        self.open(&term_core::session::OpenRequest::Config(Box::new(config)))
    }

    fn index(&self, id: SessionId) -> Option<usize> {
        // 顺序取 `list ` 的（按 id 升序）：脚本的 `session_at(i)` 与
        // 界面的标签栏顺序各自独立，但**同一个集合必须自洽**——两边都从
        // 这里拿，才不会出现「脚本说第 2 条、界面显示第 3 个」。
        self.list().iter().position(|row| row.id == id).map(|i| i + 1)
    }

    fn active(&self) -> Option<SessionId> {
        let slot = self.current.as_ref()?;
        let id = *slot.lock().ok()?;
        id.filter(|id| self.pool.name(*id).is_some())
    }

    fn host(&self, id: SessionId) -> Option<String> {
        let host = self.config_store().get(&id).map(|c| c.host.clone())?;
        (!host.is_empty()).then_some(host)
    }

    fn addr(&self, id: SessionId) -> Option<String> {
        // 本端地址只有通道自己知道；池子的 `SessionStatus` 不带它，所以这里
        // 一律 `None`——**不拿远端主机名冒充**本端地址。
        let _ = id;
        None
    }

    fn profiles(&self) -> Option<Arc<dyn ProfileCatalog>> {
        Some(Arc::new(DeskProfiles))
    }

    fn log_dir(&self) -> Option<PathBuf> {
        term_runtime::log::default_dir()
    }

    fn remote_files(&self, id: SessionId) -> Result<Arc<dyn RemoteFiles>, SessionsError> {
        let Some(config) = self.config_store().get(&id).cloned() else {
            let name = self
                .pool
                .name(id)
                .ok_or_else(|| SessionsError::NotFound(format!("#{id}")))?;
            return Err(SessionsError::Refused(format!(
                "不知道会话 {name:?} 是怎么连的（没有它的配置），无法开 SFTP"
            )));
        };
        crate::script_sftp::for_session(&self.pool, &config)
    }
}

/// [`OpenFailure`] → 脚本侧的错误分类。三类失败对应脚本作者**不同的动作**：
/// 去解锁 / 改会话配置 / 看网络。
fn spawn_failure(failure: OpenFailure) -> SessionsError {
    match failure {
        OpenFailure::Unsupported(why) => SessionsError::Refused(why),
        other => SessionsError::Spawn(other.to_string()),
    }
}

/// 整表序列化指纹。只比条数会漏掉「条数相同但内容没写进去」这类事故。
fn fingerprint(sessions: &[SessionConfig]) -> Result<u64, term_core::session::CatalogError> {
    let json = serde_json::to_string(sessions).map_err(|e| {
        term_core::session::CatalogError(format!("会话库写盘前序列化失败：{e}"))
    })?;
    let mut hasher = DefaultHasher::new();
    json.hash(&mut hasher);
    Ok(hasher.finish())
}

/// 会话库（`~/.zterm/sessions.db`）之上的 `ProfileCatalog`。
///
/// 落盘走 `term_runtime::persist`，与侧栏是同一份文件、同样两条规则
/// （读不出来当空库、写失败打到 stderr）——脚本改了会话库之后，侧栏下一次
/// 读到的就是同一份事实。
struct DeskProfiles;

impl ProfileCatalog for DeskProfiles {
    fn sessions(&self) -> Result<Vec<SessionConfig>, term_core::session::CatalogError> {
        Ok(term_runtime::persist::load()
            .map(|(_, sessions)| sessions)
            .unwrap_or_default())
    }

    fn save(&self, sessions: &[SessionConfig]) -> Result<(), term_core::session::CatalogError> {
        // 分组表原样保留：脚本只改会话，不该顺手把用户的树清空。
        let groups = term_runtime::persist::load()
            .map(|(groups, _)| groups)
            .unwrap_or_default();
        term_runtime::persist::save(&groups, sessions);
        // `persist::save` 自己吞掉错误（打 stderr）——那边没把 `Result` 递出来。
        // 这里不假装知道成败，但也不能报成功之后用户发现没落盘，所以回读一次。
        // **只比条数不够**：写失败可能留下旧库，而旧库恰好也有同样多条；那会把
        // 「用户改的主机没落盘」报成成功。比整表序列化指纹。
        let written = self.sessions()?;
        let expected_fp = fingerprint(sessions)?;
        let written_fp = fingerprint(&written)?;
        if expected_fp == written_fp {
            Ok(())
        } else {
            Err(term_core::session::CatalogError(format!(
                "写会话库后回读内容不一致（{} 条 → {} 条，指纹 {expected_fp:016x} → {written_fp:016x}；看 stderr 有没有写盘错误）",
                sessions.len(),
                written.len()
            )))
        }
    }
}

// ── 应用级脚本动词的桌面落点（2026-11-25）──────────────────────────
//
// `app_command` / `app_catalog` 在脚本侧只认端口（见 term-script 的 surface.rs），
// 命令 ID 的解释留在有 `term-app` 的这一侧：ID 字面量因此一处都不用抄到 ui/。
//
// 注意：`AppControl` 的 `DeskApp` 实现在 `script_app.rs`（本批次禁止改动，属另一
// 批次所有权）。这里先把「ID → Action」与「三处清单合一」做成可测的纯函数，
// 供那一处直接调用；合并点见交付报告。

/// 把脚本给的命令 ID 交给唯一入口 `Action::parse`。未知 ID 返回 `None`，
/// 上层据此回 `dispatched: false`，绝不假装执行成功。
pub fn parse_app_command(id: &str) -> Option<term_app::action::Action> {
    term_app::action::Action::parse(id)
}

/// `app_command(id)` 的回执：只看 ID 是否被识别，不在这里碰 UI。
pub fn desk_app_command(id: &str) -> term_script::CommandAck {
    term_script::CommandAck::dispatched(parse_app_command(id).is_some())
}

/// `app_catalog()`：应用命令台账 + 脚本能力面，两处合一。
///
/// 每组按 `source` 标明来源，脚本据此判断该用 `app_command(id)` 还是直接调
/// 同名全局函数。清单是纯数据的拼接，不缓存——能力台账/动词表本来就是静态的。
pub fn desk_app_catalog() -> Vec<term_script::CatalogEntry> {
    let mut out = Vec::new();
    for cap in term_app::capability::inventory() {
        let summary = cap
            .feature
            .map(|f| format!("应用命令（能力：{}）", f.name()))
            .unwrap_or_else(|| "应用命令".to_string());
        out.push(term_script::CatalogEntry {
            id: cap.id.to_string(),
            summary,
            source: "app".to_string(),
        });
    }
    for spec in term_core::surface::script_specs() {
        out.push(term_script::CatalogEntry {
            id: spec.name.to_string(),
            summary: spec.summary.to_string(),
            source: "script".to_string(),
        });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use term_core::session::OpenRequest;

    /// `app_command` 的桌面落点必须与菜单同入口：认得出的 ID 报已派发，
    /// 认不出的显式报未派发（不静默成功）。
    #[test]
    fn desk_app_command_uses_the_one_action_parse_entry() {
        assert!(desk_app_command("Terminal.ClearScreen").dispatched);
        assert!(desk_app_command("Window.TogglePaneTmux").dispatched);
        assert!(!desk_app_command("No.SuchCommand").dispatched);
        assert_eq!(
            parse_app_command("Terminal.ClearScreen"),
            Some(term_app::action::Action::Tab(
                term_app::vocab::TabAction::ClearScreen
            ))
        );
    }

    /// 自省清单两处都在，脚本动词（含 app_command / app_catalog / describe）
    /// 单独占一组，且与菜单命令台账不混。
    #[test]
    fn desk_app_catalog_merges_app_and_script_sources() {
        let rows = desk_app_catalog();
        for source in ["app", "script"] {
            assert!(rows.iter().any(|r| r.source == source), "{source} 组缺失");
        }
        assert!(rows.iter().any(|r| r.id == "Terminal.ClearScreen" && r.source == "app"));
        for name in ["app_command", "app_catalog", "describe"] {
            assert!(
                rows.iter()
                    .any(|r| r.id == name && r.source == "script"),
                "自省清单漏了 {name}"
            );
        }
        assert!(!rows.iter().any(|r| r.source == "crt"));
    }

    /// ADR-0025 阶段 5：界面那条路新开的会话必须**真的**被递回来。
    ///
    /// 这条盯的是「标签栏接管」这句话有没有落成机制：闸门撤掉之后，如果
    /// `open` 只是闷头建会话、什么都不回报，脚本开出来的东西就又变回隐形会话
    /// ——只是这次连报错都没有了，比原来的闸更糟。
    #[test]
    fn opening_a_session_hands_it_back_to_the_window() {
        let pool = Arc::new(SessionPool::new());
        let queue: OpenedQueue = Default::default();
        let sessions = PooledSessions::new(Arc::clone(&pool)).with_opened(Arc::clone(&queue));

        let info = sessions.open(&command("cat")).expect("开一条会话");
        let rows = queue.lock().expect("队列锁");
        assert_eq!(rows.len(), 1, "开一条就递回一条");
        let (id, config) = &rows[0];
        assert_eq!(*id, info.id, "递回的必须是刚建的那条");
        // 名字以池子登记为准：池子会把重名改写成 `cat-2` 之类，标签页要显示的是
        // 这条会话**实际**叫什么，否则标签栏上会出现两条同名。
        assert_eq!(config.name, info.name);
        assert_eq!(
            config.shell_command, "cat",
            "命令要留在配置里：标签页重连时还得照它再来一次"
        );
        drop(rows);
        assert!(pool.close(info.id));
    }

    /// 进程级那条路（`desk script run`）没人看池子：不接线就不该有队列，
    /// 也不该因此少开一条会话。
    #[test]
    fn the_process_level_host_hands_nothing_back() {
        let pool = Arc::new(SessionPool::new());
        let sessions = PooledSessions::new(Arc::clone(&pool));
        assert!(sessions.opened.is_none(), "没接线就没有队列");

        let info = sessions.open(&command("cat")).expect("开一条会话");
        assert!(pool.close(info.id));
    }

    /// 复制会话：按原配置再来一条，名字由池子唯一化。
    #[test]
    fn cloning_reopens_the_same_configuration_under_a_fresh_name() {
        let pool = Arc::new(SessionPool::new());
        let sessions = PooledSessions::new(Arc::clone(&pool));
        let first = sessions.open(&command("cat")).expect("开第一条");
        let clone = sessions.clone_session(first.id).expect("复制");
        assert_ne!(clone.id, first.id, "复制出来的是另一条会话");
        assert_ne!(clone.name, first.name, "名字必须唯一化，否则标签栏两条同名");
        let config = sessions
            .config_store()
            .get(&clone.id)
            .cloned()
            .expect("复制出的配置也登记了");
        assert_eq!(config.shell_command, "cat", "复制的是同一份配置");
        assert!(pool.close(first.id) && pool.close(clone.id));
    }

    /// 认不出配置的会话（例如界面在脚本启动前就开着的）要**明确拒绝**复制，
    /// 而不是复制出一条连不上的空壳。
    #[test]
    fn cloning_an_unknown_configuration_is_refused_with_a_reason() {
        let pool = Arc::new(SessionPool::new());
        let sessions = PooledSessions::new(Arc::clone(&pool));
        let id = pool.open_exec("cat", 80, 24).expect("池子自己开一条");
        match sessions.clone_session(id) {
            Err(SessionsError::Refused(why)) => {
                assert!(why.contains("怎么连"), "要说清缺什么：{why}");
            }
            other => panic!("期望明确拒绝，实得 {other:?}"),
        }
        assert!(pool.close(id));
    }

    /// 序号与 `list ` 同源：`index` 给 1 起位置，查不到给 `None`。
    #[test]
    fn the_index_is_one_based_and_agrees_with_the_listing() {
        let pool = Arc::new(SessionPool::new());
        let sessions = PooledSessions::new(Arc::clone(&pool));
        let first = sessions.open(&command("cat")).expect("开第一条");
        let second = sessions.open(&command("cat")).expect("开第二条");
        assert_eq!(sessions.index(first.id), Some(1));
        assert_eq!(sessions.index(second.id), Some(2));
        assert_eq!(sessions.index(9999), None, "不在册就是 None，不猜");
        assert!(pool.close(first.id) && pool.close(second.id));
    }

    fn command(shell: &str) -> OpenRequest {
        OpenRequest::Command {
            command: shell.to_string(),
            name: None,
            cols: 80,
            rows: 24,
        }
    }

    /// 写盘校验必须比**整表内容**而不只是条数：两条同长度但主机不同的库
    /// 指纹不同；真写一次后回读拿到的就是刚写进去的那份。
    #[test]
    fn saving_the_catalog_compares_full_fingerprints_not_just_counts() {
        use std::time::{SystemTime, UNIX_EPOCH};
        use term_core::session::Protocol;

        struct ProfileDirGuard(Option<String>);
        impl Drop for ProfileDirGuard {
            fn drop(&mut self) {
                match &self.0 {
                    Some(value) => std::env::set_var("WINDTERM_PROFILE_DIR", value),
                    None => std::env::remove_var("WINDTERM_PROFILE_DIR"),
                }
            }
        }

        let _env_lock = crate::PROFILE_ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let _guard = ProfileDirGuard(std::env::var("WINDTERM_PROFILE_DIR").ok());
        let dir = std::env::temp_dir().join(format!(
            "windterm-script-host-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).expect("临时 profile 目录");
        std::env::set_var("WINDTERM_PROFILE_DIR", &dir);

        let old = SessionConfig {
            name: "one".to_string(),
            host: "old-host".to_string(),
            ..SessionConfig::new(Protocol::Ssh)
        };
        let new = SessionConfig {
            host: "new-host".to_string(),
            ..old.clone()
        };
        assert_ne!(
            fingerprint(&[old.clone()]).expect("旧库指纹"),
            fingerprint(&[new]).expect("新库指纹"),
            "条数相同但内容不同，指纹必须不同"
        );

        let catalog = DeskProfiles;
        catalog.save(&[old.clone()]).expect("写盘成功");
        let rows = catalog.sessions().expect("回读");
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].host, "old-host", "回读拿到的就是刚写的那份");
        drop(rows);
        std::fs::remove_dir_all(&dir).ok();
    }
}

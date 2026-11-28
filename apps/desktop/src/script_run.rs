//! 界面里的「运行脚本」：把 `term-script` 的引擎放到后台线程上跑。
//!
//! 为什么这个文件在 `apps/desktop/src/` 而不在 `src/ui/`：
//!
//! 1. **IO 不进投影层**。读脚本文件、起线程、收结果都是副作用；`ui/` 里
//!    只该有「画」与「发动作」。验收还专门在 `ui/` 里禁了 `mpsc::Receiver` 这类
//!    字样——把 Runner 关在这里，界面拿到的是一个**不透明句柄**。
//! 2. **脚本会阻塞**。`ScriptEngine::run` 一直跑到脚本结束或沙箱超时，放在 UI
//!    线程上就是界面假死。所以它是 `start()` 起线程、`try_outcome()` 轮询。
//!
//! 与 `cli/script.rs` 的关系：那边是**进程级**的一条命令（自己开池、自己开会话、
//! 跑完就退）。这里跑在**界面已有的会话**上——脚本对用户当前看的那个终端说话，
//! 结果直接出现在同一个屏幕上。两者共用引擎，但会话来源不同，所以不合并。

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::sync::{Arc, Mutex, MutexGuard};

use term_core::script_caps::{parse_allow, Capabilities};
use term_core::session::SessionConfig;
use term_core::session_runtime::SessionId;
use term_script::{CancelToken, ScriptBudget, ScriptEngine, ScriptWorkspace};
use term_session::SessionPool;

use crate::script_clip::{DeskClipboard, DeskUi, SelectionSlot};
use crate::script_env::{self, RunSpec, WindowPorts};
use crate::script_host::{ActivatedQueue, OpenedQueue, PooledSessions};
use crate::script_human::{AskQueue, PendingPick, UiHuman};

/// 一次脚本执行的结果。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Outcome {
    /// 跑完了；带的是最后一个表达式的字符串形式（可能为空串）。
    Done(String),
    /// 失败了。
    ///
    /// `message` 是引擎的原话（`ScriptError` 的 Display 已写明第几行什么错）；
    /// `at` 是**结构化**的 1 起 `(行, 列)`，供脚本编辑器「点错误跳到那一行」
    /// 使用（来自 `ScriptError::line/column`，引擎没给出位置时为 `None`）。
    Failed {
        message: String,
        at: Option<(usize, usize)>,
    },
}

/// 「跑这个脚本文件」这条命令的 ID 前缀（ADR-0027）。
///
/// 映射键、按钮栏（快速命令）、`Recent Scripts` 三条入口说的是同一件事，所以
/// 共用**同一个**命令 ID：`Script.RunFile:<绝对路径>`。
///
/// 为什么这个常量在这里而不是 `ui/` 里：`scripts/verify.sh` 要求命令 ID
/// 字面量只出现在数据层，`apps/desktop/src/ui` 下出现 `"Xxx.Yyy"` 形状的字符串
/// 一律判红。放这里既满足门禁，也让「谁能发这个 ID、谁负责解释它」只剩一份。
pub const RUN_FILE_PREFIX: &str = "Script.RunFile:";

/// 命令 ID 是不是「跑某个脚本文件」。
///
/// 认不出、或路径部分是空的，都返回 `None`——那说明这不是我们要解释的 ID，
/// 该由 `Action::parse` 去处理。**不要**在这里改成「空路径也当作新建脚本」：
/// 一个空 ID 变成一次「打开了什么东西」是最难解释的行为。
pub fn run_file_target(action_id: &str) -> Option<&str> {
    let path = action_id.strip_prefix(RUN_FILE_PREFIX)?.trim();
    if path.is_empty() {
        None
    } else {
        Some(path)
    }
}

/// 「最近跑过的脚本」插一条：**去重、置顶、截断**（ADR-0027）。
///
/// 三条规矩各自都有理由：
///
/// - **去重**：同一个脚本跑十次不该在列表里出现十次（那正好把别的脚本挤光）。
/// - **置顶**：MRU 的意义就在「最近用过的在最上面」。
/// - **截断**：这是一份备忘，不是历史记录（见 `RECENT_SCRIPTS_MAX`）。
///
/// 纯函数：不读盘、不碰 UI，所以「最近列表」的行为可以脱开窗口单测。
pub fn push_recent(list: &mut Vec<String>, path: &str) {
    let path = path.trim();
    if path.is_empty() {
        return;
    }
    list.retain(|item| item != path);
    list.insert(0, path.to_string());
    list.truncate(term_runtime::prefs::RECENT_SCRIPTS_MAX);
}

/// 取锁；**中毒时照常取回数据**，不 panic。///
/// 登记簿里放的是「哪个脚本占着哪个会话」，某一次运行 panic 不该让此后所有
/// 会话都再也跑不了脚本——那是把一次崩溃放大成永久故障。
fn lock<T: ?Sized>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    match m.lock() {
        Ok(guard) => guard,
        Err(poisoned) => poisoned.into_inner(),
    }
}

/// 同会话互斥的登记簿（ADR-0027）。
///
/// **拒绝，不排队**。排队会造出「我点了运行，三分钟后它自己动了」——用户早已
/// 忘了这回事，屏幕上却突然冒出一串命令。所以第二条当场报错，并说清是谁在跑。
///
/// **不同会话之间不互斥**：批量场景本来就是并行跑多台主机。

/// 后台脚本的并发上限。触发器可以被连着点很多次；没有上限就会把机器拖死。
pub const MAX_BACKGROUND_RUNS: usize = 4;

#[derive(Default)]
pub struct ScriptRunRegistry {
    /// 会话 → 正在跑的那个前台脚本的名字（给人看的那一句话）。
    busy: Mutex<HashMap<SessionId, String>>,
    /// 后台运行自己的名额：`(唯一序号, 名字)`。与 `busy` 分开，
    /// 所以后台脚本既不占会话的前台名额，也不许无限并行。
    background: Mutex<Vec<(u64, String)>>,
    /// 后台租约的序号源（只在本登记簿内唯一）。
    bg_seq: AtomicU64,
}

impl ScriptRunRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    /// 占住 `id` 这条会话。失败时把**谁在跑**带在错误里。
    ///
    /// 收 `&Arc<Self>` 而不是 `&self`：租约要在 `Drop` 里自己摘牌，摘牌得能
    /// 活到脚本结束（那时调用方早已换了别的借用）。
    pub fn claim(self: &Arc<Self>, id: SessionId, who: &str) -> Result<Lease, String> {
        let mut busy = lock(&self.busy);
        if let Some(holder) = busy.get(&id) {
            return Err(format!(
                "这条会话上已经有脚本在跑（{holder}）。等它结束，或者先点「停止」。"
            ));
        }
        busy.insert(id, who.to_string());
        Ok(Lease {
            registry: Arc::clone(self),
            kind: LeaseKind::Foreground(id),
        })
    }

    /// 占一个后台名额。与会话互斥无关，但总数封顶。
    ///
    /// 不返回 `Result<_, String>`：错误文案由调用方 `ScriptRun::start` 用它已有的
    /// String 边界给出，别为「后台满了」再开一条全仓冻结的错误边界。
    pub fn claim_background(self: &Arc<Self>, who: &str) -> Option<Lease> {
        let who = if who.trim().is_empty() {
            "未命名脚本"
        } else {
            who.trim()
        };
        let mut bg = lock(&self.background);
        if bg.len() >= MAX_BACKGROUND_RUNS {
            return None;
        }
        let seq = self.bg_seq.fetch_add(1, Ordering::Relaxed);
        bg.push((seq, who.to_string()));
        Some(Lease {
            registry: Arc::clone(self),
            kind: LeaseKind::Background(seq),
        })
    }

    /// 后台名额满了时给人的那句解释（谁占着、上限多少）。
    pub fn background_full_error(&self) -> String {
        let bg = lock(&self.background);
        let names = bg.iter().map(|(_, n)| n.as_str()).collect::<Vec<_>>().join("、");
        let count = bg.len();
        format!(
            "后台脚本已经有 {count} 条在跑（最多 {MAX_BACKGROUND_RUNS} 条：{names}）。等一条结束再试。"
        )
    }

    /// 谁占着 `id`（`None` = 空闲）。界面据此把「运行」按钮灰掉并显示原因。
    pub fn holder(&self, id: SessionId) -> Option<String> {
        lock(&self.busy).get(&id).cloned()
    }

    /// 此刻有几条会话在跑前台脚本。测试与状态栏用。
    pub fn busy_count(&self) -> usize {
        lock(&self.busy).len()
    }

    /// 此刻有几条后台脚本在跑（界面据此显示「后台脚本在跑」）。
    pub fn background_count(&self) -> usize {
        lock(&self.background).len()
    }

    /// 后台脚本的名字清单（错误提示里要说清是谁占着名额）。
    pub fn background_names(&self) -> Vec<String> {
        lock(&self.background)
            .iter()
            .map(|(_, name)| name.clone())
            .collect()
    }

    fn release(&self, id: SessionId) {
        lock(&self.busy).remove(&id);
    }

    fn release_background(&self, seq: u64) {
        let mut bg = lock(&self.background);
        if let Some(i) = bg.iter().position(|(s, _)| *s == seq) {
            bg.remove(i);
        }
    }
}

/// 一份租约占的是哪个名额。前台按会话，后台按唯一序号。
#[derive(Clone, Copy)]
enum LeaseKind {
    Foreground(SessionId),
    Background(u64),
}

/// 「这条会话 / 这个后台名额被我占着」的凭据。**丢弃即放行**——包括脚本线程
/// panic、窗口被关、用户点停止这些路径，都不需要谁记得手动摘牌。
pub struct Lease {
    registry: Arc<ScriptRunRegistry>,
    kind: LeaseKind,
}

impl Lease {
    /// 提前放行（`script_start` 中途失败时用：租约拿到了，线程却没起来）。
    fn release(self) {
        // 让 `Drop` 去摘牌；这里显式写出来只是为了让调用点读起来是「放行」。
        drop(self);
    }
}

impl Drop for Lease {
    fn drop(&mut self) {
        match self.kind {
            LeaseKind::Foreground(id) => self.registry.release(id),
            LeaseKind::Background(seq) => self.registry.release_background(seq),
        }
    }
}

/// 界面里跑一次脚本时交进去的全部东西。
///
/// 为什么是一个结构体而不是十个参数：这些东西**成组出现**（界面的那几个
/// 端口要么都有、要么都没有），拆成参数会让「漏传一个」变成编译能过、运行时
/// 才发现的「脚本说没有界面」。
pub struct RunOptions {
    /// 脚本新开了哪些会话。`None` = 不递回（进程级 / 不想让会话出现在界面上）。
    pub opened: Option<OpenedQueue>,
    /// 「把某条会话带到前台」的队列。`None` = 没有界面。
    pub activated: Option<ActivatedQueue>,
    /// 界面当前激活的会话。`None` = 没有这个概念（脚本退回自己的绑定）。
    pub current: Option<Arc<Mutex<Option<SessionId>>>>,
    /// egui 上下文：剪贴板与窗口端口的唯一合法入口。
    pub ctx: Option<eframe::egui::Context>,
    /// 界面每帧抄进来的选区文本。
    pub selection: Option<SelectionSlot>,
    /// 脚本提问的队列（界面每帧取一个画成模态框）。
    pub asks: Option<AskQueue>,
    /// 文件框的队列。
    pub picks: Option<Arc<Mutex<Vec<PendingPick>>>>,
    /// 应用控制面的请求队列（面板置位 / 快照）。`None` = 没有窗口。
    pub app: Option<crate::script_app::AppQueue>,
    /// 窗口里已知的会话配置：`session_clone` / `%{host}` / SFTP 都要它。
    pub configs: Vec<(SessionId, SessionConfig)>,
    /// `--allow` 的等价值（界面里通常为空：授权来自脚本头）。
    pub allow: String,
    /// 脚本参数（`args()`）。
    pub args: Vec<String>,
    /// 脚本路径（`script_path()`）。
    pub script_path: Option<String>,
    /// 墙钟 / 操作数预算。界面默认 `Interactive`。
    pub budget: ScriptBudget,
    /// 同会话互斥登记簿。`None` = 不做互斥（CLI 单跑、单元测试）。
    pub registry: Option<Arc<ScriptRunRegistry>>,
    /// 这个脚本叫什么（进了「谁在跑」的提示）。空串时按「未命名脚本」。
    pub who: String,
    /// `true` = 前台运行（面板 / 编辑器 / 运行窗口），同一会话只许一条；
    /// `false` = 触发器后台运行，不占会话的前台名额，但受 `MAX_BACKGROUND_RUNS` 限制。
    pub foreground: bool,
}

impl RunOptions {
    /// 接上应用控制面的队列。与 `with_window` 分开：
    /// 窗口端口是「有没有界面」，这条是「能不能改应用状态」——两者可以各自缺席。
    pub fn with_app(mut self, app: crate::script_app::AppQueue) -> Self {
        self.app = Some(app);
        self
    }
}

impl Default for RunOptions {
    fn default() -> Self {
        Self {
            opened: None,
            activated: None,
            current: None,
            ctx: None,
            selection: None,
            asks: None,
            picks: None,
            app: None,
            configs: Vec::new(),
            allow: String::new(),
            args: Vec::new(),
            script_path: None,
            budget: ScriptBudget::of(term_script::BudgetKind::Interactive),
            registry: None,
            who: String::new(),
            foreground: true,
        }
    }
}

impl RunOptions {
    /// 额外的 `--allow` 条目（界面里手勾的那几项）。与脚本头声明取**并集**。
    pub fn with_allow(mut self, allow: impl Into<String>) -> Self {
        self.allow = allow.into();
        self
    }

    /// 接上「同会话互斥」登记簿，并说明这一跑叫什么名字。
    pub fn with_registry(mut self, registry: Arc<ScriptRunRegistry>, who: impl Into<String>) -> Self {
        self.registry = Some(registry);
        self.who = who.into();
        self
    }

    /// 换预算档位（界面里的「Interactive / Batch」）。
    pub fn with_budget(mut self, budget: ScriptBudget) -> Self {
        self.budget = budget;
        self
    }

    /// 触发器后台运行：不占用会话的前台脚本名额。
    pub fn with_background(mut self) -> Self {
        self.foreground = false;
        self
    }

    /// 只要「递回新会话」这一项（既有调用方最常用的形态）。
    pub fn with_opened(opened: OpenedQueue) -> Self {
        Self {
            opened: Some(opened),
            ..Self::default()
        }
    }

    /// 窗口里已知的会话配置（`session_clone` / `%{host}` / SFTP 都要它）。
    pub fn with_configs(mut self, configs: Vec<(SessionId, SessionConfig)>) -> Self {
        self.configs = configs;
        self
    }

    /// 接上界面的全套端口。
    pub fn with_window(
        mut self,
        ctx: eframe::egui::Context,
        selection: SelectionSlot,
        asks: AskQueue,
        picks: Arc<Mutex<Vec<PendingPick>>>,
        current: Arc<Mutex<Option<SessionId>>>,
        activated: ActivatedQueue,
    ) -> Self {
        self.ctx = Some(ctx);
        self.selection = Some(selection);
        self.asks = Some(asks);
        self.picks = Some(picks);
        self.current = Some(current);
        self.activated = Some(activated);
        self
    }
}

/// 一次正在跑的脚本。丢弃即放弃结果（线程自己会跑完，不泄漏）。
pub struct ScriptRun {
    rx: Receiver<Outcome>,
    /// 脚本 `notify` 送来的话。界面每帧取走显示在状态栏——这条路刻意不走
    /// stderr：界面里的脚本对同一个 PTY 说话，往 stderr 写会被用户看成
    /// 「终端输出」，也会被脚本自己的 `read` 读回去。
    notes: Arc<Mutex<Vec<String>>>,
    /// 「停止」按钮按的就是它。
    cancel: CancelToken,
    /// 「这条会话被我占着」。**字段在，占位就在**：脚本跑完（`try_outcome` 取到
    /// 结果后界面把 `ScriptRun` 置空）、窗口关掉、线程 panic，三条路都是丢弃 →
    /// 摘牌，不需要谁记得手动放行。
    _lease: Option<Lease>,
}

impl ScriptRun {
    /// 在 `id` 指向的会话上跑 `source`。
    ///
    /// 会话**借出**而不是复制（`SessionPool::session` 给的是同一个对象的
    /// `Arc`），所以脚本的输出与界面看到的是同一份事实。
    ///
    /// 失败只在一种情况下立刻返回 `Err`：控制面里查不到这个会话（那是内伤，
    /// 不该被说成「脚本报错」）。脚本自身的错误走 [`Outcome::Failed`]。
    pub fn start(
        pool: Arc<SessionPool>,
        id: SessionId,
        source: String,
        options: RunOptions,
    ) -> Result<Self, String> {
        let Some(session) = pool.session(id) else {
            return Err(format!("控制面里查不到这个会话（id={id:?}）").into());
        };
        // 名额先占，再干别的：读文件、起线程都要花时间，而「谁在跑」必须在
        // 第一帧就能报出来，否则连点两下会真的起来两条。
        // 前台按会话互斥；后台不占会话名额，但自己有并发上限（见 `MAX_BACKGROUND_RUNS`）。
        let who = if options.who.trim().is_empty() {
            "未命名脚本"
        } else {
            options.who.trim()
        };
        let lease = match &options.registry {
            Some(registry) if options.foreground => Some(registry.claim(id, who)?),
            Some(registry) => match registry.claim_background(who) {
                Some(lease) => Some(lease),
                None => return Err(registry.background_full_error()),
            },
            None => None,
        };
        let notes = Arc::new(Mutex::new(Vec::new()));
        let cancel = CancelToken::new();
        let mut host = PooledSessions::new(Arc::clone(&pool));
        if let Some(queue) = options.opened.clone() {
            host = host.with_opened(queue);
        }
        if let (Some(activated), Some(current)) = (options.activated.clone(), options.current.clone()) {
            host = host.with_window(activated, current);
        }
        for (tab, config) in &options.configs {
            host.remember(*tab, config.clone());
        }
        let host = Arc::new(host);
        let spec = RunSpec::new(source)
            .with_allow(options.allow.clone())
            .with_args(options.args.clone())
            .with_path(options.script_path.clone())
            .with_budget(options.budget);
        let caps = script_env::envelope(&spec)?;
        let mut ports = WindowPorts::default();
        let human: Arc<dyn term_script::HumanChannel> = match (
            options.ctx.clone(),
            options.asks.clone(),
            options.picks.clone(),
        ) {
            (Some(ctx), Some(asks), Some(picks)) => {
                // 应用控制面：队列由界面侧持有，端口只负责往返。
                if let Some(queue) = options.app.clone() {
                    ports.app = Some(Arc::new(crate::script_app::DeskApp::new(
                        ctx.clone(),
                        queue,
                        cancel.clone(),
                    )));
                }
                ports.clipboard = Some(Arc::new(DeskClipboard::new(ctx.clone())));
                ports.ui = Some(Arc::new(DeskUi::new(
                    ctx,
                    options.selection.clone().unwrap_or_default(),
                )));
                Arc::new(UiHuman::new(
                    asks,
                    picks,
                    Some(Arc::clone(&caps)),
                    Some(Arc::clone(&notes)),
                    cancel.clone(),
                ))
            }
            // 没有界面（或没接线）：`notify` 仍要进 notes（GUI 进程的 stderr
            // 用户看不见），`ask` 显式「没有可交互输入」而不是编一个答案。
            _ => Arc::new(crate::script_human::NotesHuman::new(Arc::clone(&notes))),
        };
        let env = script_env::build(&spec, host, human, ports, cancel.clone(), caps).env;
        let (tx, rx) = mpsc::channel();
        let spawned = std::thread::Builder::new()
            .name("desk-script".to_string())
            .spawn(move || {
                let sandbox = options.budget.sandbox();
                let workspace = Arc::new(ScriptWorkspace::new(
                    env,
                    session,
                    sandbox.execution_timeout,
                ));
                let outcome = match ScriptEngine::new(sandbox).run_in_workspace(&spec.source, workspace)
                {
                    Ok(value) => Outcome::Done(value),
                    Err(err) => Outcome::Failed {
                        at: err.line().zip(err.column()),
                        message: err.to_string(),
                    },
                };
                // 界面已经关了窗口时这里会失败；不是错误，结果本来就没处放。
                let _ = tx.send(outcome);
            });
        if let Err(e) = spawned {
            // 线程都没起来，占位要立刻还回去——不然这条会话此后永远「有脚本在跑」。
            if let Some(lease) = lease {
                lease.release();
            }
            return Err(format!("起不了脚本线程：{e}"));
        }
        Ok(Self {
            rx,
            notes,
            cancel,
            _lease: lease,
        })
    }

    /// 在 `id` 的会话上跑一份**外部 Python** 脚本。
    ///
    /// 与 [`Self::start`] 共用租约、取消令牌和 `Outcome` 通道，所以面板 / 运行窗口
    /// 的「停止」「结果」逻辑一行都不用改；区别只在线程里跑的不是 Rhai 引擎，
    /// 而是 `python_run` 起的 Python 子进程 + stdio JSON-RPC 桥。
    ///
    /// 授权沿两条路收：`// @cap`（Python 文件头用 `# @cap`）与界面手勾 /
    /// `--allow` 对应的 `options.allow`。起外部进程必须拿到 `process`。
    pub fn start_python(
        pool: Arc<SessionPool>,
        id: SessionId,
        path: &str,
        options: RunOptions,
    ) -> Result<Self, crate::python_run::PythonRunError> {
        let Some(session) = pool.session(id) else {
            return Err(format!("控制面里查不到这个会话（id={id:?}）").into());
        };
        drop(session);
        let name = pool
            .name(id)
            .ok_or_else(|| crate::python_run::PythonRunError::from(format!("控制面里查不到会话名（id={id:?}）")))?;
        let who = if options.who.trim().is_empty() {
            "未命名 Python 脚本"
        } else {
            options.who.trim()
        };
        let lease = match &options.registry {
            Some(registry) if options.foreground => Some(registry.claim(id, who).map_err(crate::python_run::PythonRunError::from)?),
            Some(registry) => match registry.claim_background(who) {
                Some(lease) => Some(lease),
                None => return Err(crate::python_run::PythonRunError::from(registry.background_full_error())),
            },
            None => None,
        };

        let source = std::fs::read_to_string(path)
            .map_err(|e| crate::python_run::PythonRunError::from(format!("读不到 Python 脚本 {path}：{e}")))?;
        let mut caps = Capabilities::NONE;
        for request in parse_allow(&options.allow).map_err(|e| crate::python_run::PythonRunError::from(e.0.to_string()))? {
            caps.grant(&request);
        }
        // Python 的注释头是 `#`，不是 Rhai 的 `//`；只取开头连续的注释块。
        let header = source
            .lines()
            .take_while(|line| line.trim().is_empty() || line.trim_start().starts_with('#'))
            .filter_map(|line| {
                let trimmed = line.trim();
                trimmed
                    .strip_prefix("# @cap")
                    .or_else(|| trimmed.strip_prefix("#@cap"))
            })
            .map(str::trim)
            .collect::<Vec<_>>()
            .join(",");
        for request in parse_allow(&header).map_err(|e| crate::python_run::PythonRunError::from(e.0.to_string()))? {
            caps.grant(&request);
        }
        let process_allowed = caps.check_process().is_ok();

        let notes = Arc::new(Mutex::new(Vec::new()));
        let cancel = CancelToken::new();
        let script_path = PathBuf::from(path);
        let args = options.args.clone();
        let wall = options.budget.wall;
        let (tx, rx) = mpsc::channel();
        let run_cancel = cancel.clone();
        let spawned = std::thread::Builder::new()
            .name("desk-python".to_string())
            .spawn(move || {
                let outcome = match crate::python_run::run_blocking(
                    pool,
                    name,
                    &script_path,
                    &args,
                    wall,
                    run_cancel,
                    process_allowed,
                ) {
                    Ok(value) => Outcome::Done(value),
                    Err(message) => Outcome::Failed {
                        message: message.to_string(),
                        at: None,
                    },
                };
                let _ = tx.send(outcome);
            });
        if let Err(e) = spawned {
            if let Some(lease) = lease {
                lease.release();
            }
            return Err(crate::python_run::PythonRunError::from(format!("起不了 Python 线程：{e}")));
        }
        Ok(Self {
            rx,
            notes,
            cancel,
            _lease: lease,
        })
    }

    /// 「停止」：叫停脚本。下一个取消检查点（`wait` / `sleep` / 每次 `collect`
    /// 的 50ms 切片）会让它报 `Cancelled` 收场。
    pub fn cancel(&self) {
        self.cancel.cancel();
    }

    /// 是不是已经被叫停了（界面据此把按钮从「停止」变回「运行」）。
    pub fn is_cancelled(&self) -> bool {
        self.cancel.is_cancelled()
    }

    /// 取走脚本 `notify` 送来的话（界面每帧调一次；没有就是空表）。
    ///
    /// 用「取走」而不是「读取」：同一条提示不该在状态栏里反复显示，也不该在
    /// 脚本报错时把三行旧话一起倒出来。
    pub fn drain_notes(&self) -> Vec<String> {
        let mut queue = match self.notes.lock() {
            Ok(queue) => queue,
            Err(poisoned) => poisoned.into_inner(),
        };
        std::mem::take(&mut *queue)
    }

    /// 结果到了没有（界面每帧问一次）。
    pub fn try_outcome(&mut self) -> Option<Outcome> {
        match self.rx.try_recv() {
            Ok(outcome) => Some(outcome),
            // 线程没了却没留话：说成「失败」，不要装作还在跑（那会让界面一直转圈）。
            Err(TryRecvError::Disconnected) => Some(Outcome::Failed {
                message: "脚本线程意外结束，没有留下结果".to_string(),
                at: None,
            }),
            Err(TryRecvError::Empty) => None,
        }
    }
}

/// 读脚本文件。`-` 表示标准输入（与 `desk script run` 同一套约定）。
///
/// 错误里必须带**路径**：只说「读不到脚本」等于让用户猜是哪个文件。
pub fn read_source(path: &str) -> Result<String, String> {
    if path == "-" {
        use std::io::Read;
        let mut text = String::new();
        std::io::stdin()
            .read_to_string(&mut text)
            .map_err(|e| format!("读标准输入失败：{e}"))?;
        return Ok(text);
    }
    std::fs::read_to_string(path).map_err(|e| format!("读不到脚本 {path}：{e}"))
}

/// 保存失败的两种：还没有路径 / 写盘 IO 失败。
///
/// 不返回 `String`——错误边界基线只许减不许增，新增边界用独立枚举。
#[derive(Debug)]
pub enum SaveError {
    /// 脚本还没存过（调用方应先走「另存为」）。
    NoPath,
    /// 写盘失败；带上是**哪个路径**，否则用户只能猜。
    Io {
        path: String,
        source: std::io::Error,
    },
}

impl std::fmt::Display for SaveError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SaveError::NoPath => f.write_str("脚本还没有路径，先「另存为」"),
            SaveError::Io { path, source } => write!(f, "写不到脚本 {path}：{source}"),
        }
    }
}

impl std::error::Error for SaveError {}

fn io_error(path: &str, source: std::io::Error) -> SaveError {
    SaveError::Io {
        path: path.to_string(),
        source,
    }
}

/// 把编辑好的脚本写回磁盘。路径为空时拒绝（调用方应先「另存为」）。
///
/// 与 [`read_source`] 对称：错误里必须带**路径**。写文件是 OS 资源，所以它留在
/// `apps/desktop/src/` 而不是 `ui/`（`ui/` 不碰文件系统）。
///
/// **写之前按需创建父目录**：脚本库目录（`profile_dir()/scripts`）在第一次
/// 保存时还不存在；用户在「另存为」里手打一级新目录也是常事。不建目录的话，这两种
/// 情况都会得到一句 `No such file or directory`——而用户想要的只是「保存」。
pub fn save_source(path: &str, text: &str) -> Result<(), SaveError> {
    let path = path.trim();
    if path.is_empty() {
        return Err(SaveError::NoPath);
    }
    // 裸文件名（`a.rhai`）的 `parent()` 是空串：不能用它去建目录。
    if let Some(parent) = std::path::Path::new(path).parent() {
        if !parent.as_os_str().is_empty() {
            std::fs::create_dir_all(parent).map_err(|e| io_error(path, e))?;
        }
    }
    std::fs::write(path, text).map_err(|source| io_error(path, source))
}

/// 新建脚本的默认落点：脚本库目录下的 `untitled.rhai`，重名就加序号。
///
/// 为什么不让「新建」落到工作目录：`cargo run` 起来的进程 CWD 是仓库根，双击启动的
/// 进程 CWD 可能是 `$HOME` 也可能是 `/`。同一句「新建脚本」在三种启动方式下写进
/// 三个不同的地方，用户第二次就找不到自己的脚本了。所以默认落**脚本库**
/// （[`term_runtime::paths::scripts_dir`]，即 `profile_dir()/scripts`），与
/// `sessions.db` 同属一个可整体备份的目录。
///
/// 只回答路径、不建文件：用户可能打开编辑器看完又关掉，那时不该在磁盘上留一个空
/// 脚本。目录与文件都在 [`save_source`] 真正落盘时才创建。
pub fn default_script_path() -> Option<String> {
    let dir = term_runtime::paths::scripts_dir()?;
    Some(free_script_path(&dir)?.to_string_lossy().into_owned())
}

/// 在 `dir` 里挑一个还没被占用的脚本路径：`untitled.rhai` 被占就试 `untitled-2.rhai`。
///
/// 试满 100 个还找不到就返回 `None`——那时继续「随便挑一个」会覆盖别人的脚本，
/// 说「没位置」比偷偷盖掉诚实。
fn free_script_path(dir: &std::path::Path) -> Option<std::path::PathBuf> {
    (1..=100).map(|attempt| {
        let name = if attempt == 1 {
            "untitled.rhai".to_string()
        } else {
            format!("untitled-{attempt}.rhai")
        };
        dir.join(name)
    }).find(|candidate| !candidate.exists())
}

#[cfg(test)]
mod tests {
    use super::*;
    use term_core::session::{Protocol, SessionConfig};

    /// 开一个不会自己退的本地会话（`cat` 读 stdin 等命令，脚本发什么它回什么）。
    fn pool_with_session() -> (Arc<SessionPool>, SessionId) {
        let pool = Arc::new(SessionPool::new());
        let mut config = SessionConfig::new(Protocol::Shell);
        config.name = "script-test".to_string();
        let id = pool.open_exec("cat", 80, 24).expect("起会话");
        (pool, id)
    }

    fn wait(run: &mut ScriptRun) -> Outcome {
        // 最多等 10 秒：脚本是纯算术，正常几毫秒就回来；等不到就是真卡了。
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        loop {
            if let Some(outcome) = run.try_outcome() {
                return outcome;
            }
            assert!(std::time::Instant::now() < deadline, "脚本 10 秒还没结果");
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
    }

    #[test]
    fn a_pure_script_runs_on_a_real_session_and_returns_its_value() {
        // 这条守的是「整条管线通」：起线程 → 借出同一个会话 → 引擎跑 → 通道回来。
        // 脚本本身不碰原语，所以它只验证管线，不验证 PTY 行为（那是 term-script
        // 自己的 e2e）。
        let (pool, id) = pool_with_session();
        let mut run =
            ScriptRun::start(pool.clone(), id, "1 + 1".to_string(), RunOptions::default()).expect("起脚本");
        assert_eq!(wait(&mut run), Outcome::Done("2".to_string()));
        pool.close(id);
    }

    #[test]
    fn a_syntax_error_comes_back_as_a_failure_with_the_line_number() {
        let (pool, id) = pool_with_session();
        let mut run =
            ScriptRun::start(pool.clone(), id, "let = ;".to_string(), RunOptions::default()).expect("起脚本");
        match wait(&mut run) {
            Outcome::Failed { message, at } => {
                // 引擎的措辞里带位置；别在这一层重新编一句「脚本失败」。
                assert!(!message.is_empty(), "失败要说清原因");
                // 之后结构化位置也一路带到这里（编辑器点错误跳行要用）。
                assert!(at.is_some(), "语法错应带结构化 (行, 列)：{at:?}");
            }
            other => panic!("{other:?}"),
        }
        pool.close(id);
    }

    #[test]
    fn the_in_app_runner_can_open_sessions_and_hands_them_back() {
        // ADR-0025 阶段 5：界面这条路**不再**拒绝 `session_open`。
        //
        // 原来的闸（`PooledSessions::list_only`）理由是「标签栏渲染的不是池子清单，
        // 开了用户看不见也关不掉」。那个理由成立，但解法不是砍能力——现在窗口
        // 交进来一个队列，标签栏下一帧把开出来的会话变成真标签页，前提消失。
        //
        // 这条盯两件事：① 脚本真的开出来了（不再是 Refused）；② 开出来的会话
        // **确实被递回**（有人接手画）。少了第②点，撤闸就等于把「显式报错」
        // 换成「静默隐形」，比原来更糟。
        let (pool, id) = pool_with_session();
        let opened: OpenedQueue = Default::default();
        let mut run = ScriptRun::start(
            pool.clone(),
            id,
            "session_open(\"cat\").name".to_string(),
            RunOptions::with_opened(Arc::clone(&opened)),
        )
        .expect("起脚本");
        let Outcome::Done(name) = wait(&mut run) else {
            panic!("界面里现在允许脚本新开会话");
        };
        let rows = opened.lock().expect("队列锁");
        assert_eq!(rows.len(), 1, "开一条就递回一条");
        let (new_id, config) = &rows[0];
        assert_eq!(
            config.name, name,
            "递回的配置名要与脚本 `session_open` 拿到的名字一致"
        );
        let new_id = *new_id;
        drop(rows);
        assert!(pool.close(new_id), "脚本开的那条真在池子里");
        pool.close(id);
    }

    #[test]
    fn the_in_app_runner_still_sees_the_pool_it_runs_on() {
        // 撤闸不该顺手把「看得见已有会话」也弄丢：`session_list` 与
        // `session_current` 必须仍然回答池子的真相。
        // 寻址名是池里那条会话的名字（`open_exec("cat")` → `"cat"`），不是 id。
        let (pool, id) = pool_with_session();
        let mut run = ScriptRun::start(
            pool.clone(),
            id,
            // 用 `to_string()` 拼接而不是直接 `+`：这样两边都是字符串，不必靠
            // Rhai 的隐式转换。这条断言第一版曾写成 `+` 对两个整数相加，
            // 得到一个和（8+1=9），看起来像「只打印了一个数」。
            "session_current().to_string() + \"/\" + session_list().sessions.len().to_string()"
                .to_string(),
            RunOptions::default(),
        )
        .expect("起脚本");
        assert_eq!(
            wait(&mut run),
            Outcome::Done("cat/1".to_string()),
            "清单里应当就是这一条会话，名字是它的寻址名"
        );
        pool.close(id);
    }

    #[test]
    fn notify_from_the_in_app_runner_lands_in_the_note_queue() {
        // 界面里的 notify 不能写 stderr：那会混进会话输出（同一个 PTY），
        // 用户看成「远端说了这句话」，脚本自己 read 回来也会读到。
        let (pool, id) = pool_with_session();
        let mut run = ScriptRun::start(
            pool.clone(),
            id,
            "notify(\"hi there\"); 1".to_string(),
            RunOptions::default(),
        )
        .expect("起脚本");
        assert_eq!(wait(&mut run), Outcome::Done("1".to_string()));
        assert_eq!(run.drain_notes(), vec!["hi there".to_string()]);
        assert!(run.drain_notes().is_empty(), "取走之后不该再出现");
        pool.close(id);
    }

    #[test]
    fn ask_in_the_app_has_no_input_and_says_so() {
        // 界面里没有标准输入：必须显式失败，不能返回空串冒充「用户按了回车」。
        let (pool, id) = pool_with_session();
        let mut run = ScriptRun::start(
            pool.clone(),
            id,
            "ask(\"continue?\")".to_string(),
            RunOptions::default(),
        )
        .expect("起脚本");
        match wait(&mut run) {
            Outcome::Failed { message, .. } => assert!(
                message.contains("没有可交互的输入"),
                "要说清「这里没有输入」而不是笼统失败：{message}"
            ),
            other => panic!("界面里的 ask 不该有回答：{other:?}"),
        }
        pool.close(id);
    }

    #[test]
    fn starting_on_a_session_the_pool_does_not_know_is_an_error_not_a_silent_run() {
        // 内伤要当场说出来：假装跑起来会让界面一直转圈。
        let (pool, id) = pool_with_session();
        pool.close(id);
        match ScriptRun::start(pool, id, "1".to_string(), RunOptions::default()) {
            Ok(_) => panic!("会话已关，不该跑起来"),
            Err(err) => assert!(err.contains("查不到"), "{err}"),
        }
    }

    #[test]
    fn the_registry_keeps_one_run_per_session_and_names_the_holder() {
        // 拒绝不排队：第二条要当场说清「谁在跑」，否则用户只会看到「运行没反应」。
        let registry = Arc::new(ScriptRunRegistry::new());
        let first = registry.claim(7, "nightly.rhai").expect("第一条应当能跑");
        let err = match registry.claim(7, "other.rhai") {
            Ok(_) => panic!("同会话第二条应当被拒"),
            Err(err) => err,
        };
        assert!(err.contains("nightly.rhai"), "要说清谁在跑：{err}");
        assert!(err.contains("停止"), "要告诉人能怎么办：{err}");
        assert_eq!(registry.holder(7).as_deref(), Some("nightly.rhai"));
        drop(first);
        assert!(registry.claim(7, "other.rhai").is_ok(), "让出来之后应当能跑");
    }

    #[test]
    fn different_sessions_do_not_exclude_each_other() {
        // 批量脚本本来就是并行跑多台主机；互斥只按会话，不按「全局一次一个」。
        let registry = Arc::new(ScriptRunRegistry::new());
        let a = registry.claim(1, "a.rhai").expect("会话 1");
        let b = registry.claim(2, "b.rhai").expect("会话 2");
        assert_eq!(registry.busy_count(), 2);
        drop(a);
        assert_eq!(registry.busy_count(), 1, "只该摘掉自己那条");
        assert_eq!(registry.holder(2).as_deref(), Some("b.rhai"));
        drop(b);
        assert_eq!(registry.busy_count(), 0);
    }

    #[test]
    fn background_trigger_scripts_do_not_take_the_foreground_slot() {
        let (pool, id) = pool_with_session();
        let registry = Arc::new(ScriptRunRegistry::new());
        let mut fg = ScriptRun::start(
            pool.clone(),
            id,
            "sleep(80); 1".to_string(),
            RunOptions::default().with_registry(Arc::clone(&registry), "fg.rhai"),
        )
        .expect("前台脚本");
        // 触发器后台：同一会话可以并行多条，也不占前台名额。
        let mut bg1 = ScriptRun::start(
            pool.clone(),
            id,
            "sleep(60); 2".to_string(),
            RunOptions::default()
                .with_registry(Arc::clone(&registry), "bg1.rhai")
                .with_background(),
        )
        .expect("后台脚本 1");
        let mut bg2 = ScriptRun::start(
            pool.clone(),
            id,
            "sleep(60); 3".to_string(),
            RunOptions::default()
                .with_registry(Arc::clone(&registry), "bg2.rhai")
                .with_background(),
        )
        .expect("后台脚本 2");
        assert_eq!(registry.holder(id).as_deref(), Some("fg.rhai"));
        assert_eq!(registry.busy_count(), 1, "只有前台脚本占会话名额");
        assert_eq!(registry.background_count(), 2, "后台另有自己的名额");
        // 前台与后台各占各的名额，互不顶替。
        assert_eq!(wait(&mut bg1), Outcome::Done("2".to_string()));
        assert_eq!(wait(&mut bg2), Outcome::Done("3".to_string()));
        assert_eq!(wait(&mut fg), Outcome::Done("1".to_string()));
        drop(bg1);
        drop(bg2);
        assert_eq!(registry.background_count(), 0, "句柄一丢后台名额就还回来");
        pool.close(id);
    }

    #[test]
    fn a_running_script_holds_the_session_until_its_result_is_taken() {
        // 端到端：真起一条脚本，占位随 `ScriptRun` 的存亡走，而不是随线程结束走。
        // 这样「脚本跑完了但界面还没取走结果」的几十毫秒里也不会被第二条插进来。
        let (pool, id) = pool_with_session();
        let registry = Arc::new(ScriptRunRegistry::new());
        let mut run = ScriptRun::start(
            pool.clone(),
            id,
            "sleep(80); 1".to_string(),
            RunOptions::default().with_registry(Arc::clone(&registry), "slow.rhai"),
        )
        .expect("起脚本");
        assert_eq!(registry.holder(id).as_deref(), Some("slow.rhai"));
        let err = match ScriptRun::start(
            pool.clone(),
            id,
            "1".to_string(),
            RunOptions::default().with_registry(Arc::clone(&registry), "fast.rhai"),
        ) {
            Ok(_) => panic!("同会话第二条应当被拒"),
            Err(err) => err,
        };
        assert!(err.contains("slow.rhai"), "{err}");

        assert_eq!(wait(&mut run), Outcome::Done("1".to_string()));
        // 结果还没取走时仍占着。
        assert_eq!(registry.holder(id).as_deref(), Some("slow.rhai"));
        drop(run);
        assert_eq!(registry.holder(id), None, "句柄一丢就该放行");
        pool.close(id);
    }

    #[test]
    fn background_runs_have_their_own_capped_lease() {
        // 后台不占会话名额，但也不能无限并行：触发器连点十次会拖死机器。
        let registry = Arc::new(ScriptRunRegistry::new());
        let mut leases = Vec::new();
        for i in 0..MAX_BACKGROUND_RUNS {
            leases.push(
                registry
                    .claim_background(&format!("bg{i}.rhai"))
                    .expect("名额"),
            );
        }
        assert_eq!(registry.background_count(), MAX_BACKGROUND_RUNS);
        assert!(registry.background_names().iter().any(|n| n == "bg0.rhai"));
        assert!(
            registry.claim_background("extra.rhai").is_none(),
            "后台并发超额必须被拒"
        );
        let err = registry.background_full_error();
        assert!(err.contains("最多"), "{err}");
        assert!(err.contains("bg0.rhai"), "要说清谁占着：{err}");
        leases.remove(0);
        assert_eq!(registry.background_count(), MAX_BACKGROUND_RUNS - 1);
        assert!(registry.claim_background("extra.rhai").is_some(), "让出来就能跑");
    }

    /// 面板 Stop 的那条链：cancel → `Outcome::Failed(取消)` → 句柄置空 → 租约释放。
    /// 面板把句柄留到 Outcome 才置空；这里盯的就是「Outcome 到了才放行」。
    #[test]
    fn cancelling_a_run_yields_a_failed_outcome_then_frees_the_lease() {
        let (pool, id) = pool_with_session();
        let registry = Arc::new(ScriptRunRegistry::new());
        let mut run = ScriptRun::start(
            pool.clone(),
            id,
            "sleep(80); 1".to_string(),
            RunOptions::default().with_registry(Arc::clone(&registry), "stop.rhai"),
        )
        .expect("起脚本");
        assert_eq!(registry.holder(id).as_deref(), Some("stop.rhai"));

        // 「停止」按钮只调 cancel，句柄还攥着（面板不会提前置 None）。
        run.cancel();
        assert!(run.is_cancelled());
        assert_eq!(registry.holder(id).as_deref(), Some("stop.rhai"));

        match wait(&mut run) {
            Outcome::Failed { message, .. } => {
                assert!(message.contains("取消"), "要说清是被取消的：{message}");
            }
            other => panic!("取消应当收场成 Failed：{other:?}"),
        }
        // 结果已到但句柄还在：租约仍占着，这是「先收 Outcome 再置空」的意义。
        assert_eq!(registry.holder(id).as_deref(), Some("stop.rhai"));
        drop(run);
        assert_eq!(registry.holder(id), None, "句柄置空才释放租约");
        assert!(
            ScriptRun::start(
                pool.clone(),
                id,
                "1".to_string(),
                RunOptions::default().with_registry(Arc::clone(&registry), "next.rhai"),
            )
            .is_ok(),
            "租约释放之后同会话应当能再跑"
        );
        pool.close(id);
    }

    #[test]
    fn the_recent_list_puts_the_latest_first_and_keeps_one_entry_per_script() {
        // 「最近」列表如果会重复，反复跑同一个脚本就会把别的脚本全挤出去——
        // 那正好是它最没用的样子。
        let mut list: Vec<String> = Vec::new();
        push_recent(&mut list, "/a.rhai");
        push_recent(&mut list, "/b.rhai");
        assert_eq!(list, ["/b.rhai", "/a.rhai"], "最新的在最前");

        // 再跑一次 a：置顶，而不是出现第二条。
        push_recent(&mut list, "/a.rhai");
        assert_eq!(list, ["/a.rhai", "/b.rhai"]);

        // 空路径 / 纯空白不算一条（绑定写坏了不该在列表里留个空行）。
        push_recent(&mut list, "   ");
        assert_eq!(list.len(), 2);
    }

    #[test]
    fn the_recent_list_stops_at_ten() {
        let mut list: Vec<String> = Vec::new();
        for i in 0..15 {
            push_recent(&mut list, &format!("/s{i}.rhai"));
        }
        assert_eq!(list.len(), term_runtime::prefs::RECENT_SCRIPTS_MAX);
        assert_eq!(list[0], "/s14.rhai", "最新的在最前");
        assert_eq!(list.last().map(String::as_str), Some("/s5.rhai"), "最旧的被挤掉");
    }

    #[test]
    fn the_run_file_command_id_carries_a_path_and_nothing_else() {
        // 映射键 / 按钮栏 / 最近列表三条入口靠这条解析；多认一个字符就会把
        // 别人的命令 ID 当成「跑脚本」。
        assert_eq!(
            run_file_target("Script.RunFile:/tmp/a.rhai"),
            Some("/tmp/a.rhai")
        );
        // 路径两侧的空格是人手写绑定时的常态。
        assert_eq!(run_file_target("Script.RunFile:  /tmp/b.rhai "), Some("/tmp/b.rhai"));
        assert_eq!(run_file_target("Script.RunFile:"), None, "空路径不算命令");
        assert_eq!(run_file_target("Script.RunFile:   "), None);
        assert_eq!(run_file_target("Script.Run"), None);
        assert_eq!(run_file_target("Window.Close"), None);
        assert_eq!(run_file_target(""), None);
    }

    #[test]
    fn reading_a_missing_script_names_the_path() {
        let err = read_source("/definitely/not/here.rhai").expect_err("应当报错");
        assert!(err.contains("/definitely/not/here.rhai"), "{err}");
    }

    /// 一次性临时目录。不引 `tempfile`：为两条用例多一条依赖不值。
    fn temp_dir(tag: &str) -> std::path::PathBuf {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.subsec_nanos())
            .unwrap_or(0);
        let dir = std::env::temp_dir().join(format!(
            "term-desk-script-{tag}-{}-{nanos}",
            std::process::id()
        ));
        std::fs::create_dir_all(&dir).expect("建临时目录");
        dir
    }

    #[test]
    fn saving_creates_the_missing_parent_directories() {
        // 脚本库目录第一次保存时还不存在；用户在「另存为」里手打一级新目录也一样。
        // 两种都只剩一句 `No such file or directory` 是不合理的，所以写前建父目录。
        let dir = temp_dir("save-parent");
        let path = dir.join("scripts").join("nested").join("a.rhai");
        save_source(path.to_str().expect("UTF-8 路径"), "1 + 1").expect("写盘");
        assert_eq!(std::fs::read_to_string(&path).expect("回读"), "1 + 1");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn the_default_script_name_is_untitled_and_avoids_names_already_taken() {
        // 「新建」不能每次都给同一个 `untitled.rhai`——第二次保存会静默盖掉第一个脚本。
        let dir = temp_dir("free-path");
        let first = free_script_path(&dir).expect("有位置");
        assert_eq!(
            first.file_name().and_then(|n| n.to_str()),
            Some("untitled.rhai")
        );
        assert!(!first.exists(), "只回答路径，不该先建出文件");
        std::fs::write(&first, "").expect("占位");
        let second = free_script_path(&dir).expect("有位置");
        assert_eq!(
            second.file_name().and_then(|n| n.to_str()),
            Some("untitled-2.rhai")
        );
        std::fs::remove_dir_all(&dir).ok();
    }
}

//! B2/B3：每会话一个监管者线程。
//!
//! ```text
//! [Supervisor 线程]                          [主线程 update()]
//!   TermChannel::drain() -> bytes              handle.poll()
//!        -> VtCore::advance(bytes)                 ├─ 从一槽邮箱取走最新快照
//!        -> take_replies() 写回                     └─ 合并 Screen / Finished 事件
//!        -> 有变化且槽空 → 放一份新快照          Tab.snapshot 只读
//!                    └── 槽 + wakeup() ──▶      输入 -> handle.write()
//! `wakeup` 由 UI 注入（`ctx.request_repaint()`）  尺寸 -> handle.resize()
//! ```
//!
//! **为什么不是「每轮都 send 快照」**（R6 修正）：
//! 1. 生产者每 2ms 一轮、消费者每帧一次——`send` 出去的快照绝大多数会被 `poll()`
//!    直接丢弃，等于每 2ms 做一次全网格分配然后扔掉；
//! 2. 更关键的是**没有唤醒源**：`send` 只是往队列里塞东西，界面不会因此重绘，
//!    于是「输出到达 → 不重绘 → 不 poll → 看不到输出」，直到用户动一下鼠标。
//!    这个问题在 S4 把「每帧无条件重绘」改成按需重绘之后才成立。
//!
//! 现在改成**一槽邮箱 + 唤醒回调**：槽里只保留最新一份，消费者不取走就不再生成；
//! 生成后调一次注入的 `wakeup`，界面立刻醒来。空闲时既没有快照分配也没有唤醒。
//!
//! UI 不再持有 `MasterPty` / `Child` / `mpsc::Receiver`，也不再在 `draw()` 里
//! 做字节摄入与 VT 解析。保持 `std::mpsc` + 每会话线程，不上异步运行时。

use crate::channel::{open_channel, open_exec_channel, TermChannel};
use crate::error::SpawnError;
use crate::log::{LogStatus, SessionLog};
use term_core::ansi::TextCleaner;
use term_core::coords::ViewRange;
use term_core::grid::{GridSnapshot, ScreenImage};
use term_core::session_runtime::{
    Cancel, FacetKind, OutputReader, Session, SessionState, WaitError, WaitMatch,
    WaitRegistration,
};
use term_core::vt::Terminal;
use term_core::session::{Protocol, SessionConfig};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

pub type SessionId = u64;
static NEXT_ID: AtomicU64 = AtomicU64::new(1);
/// wait 登记的编号空间（与 `SessionId` 无关，只在取消时对上号）。
static NEXT_WAITER: AtomicU64 = AtomicU64::new(1);

/// 输出读取器（`read` / `read_until`）的登记序号。
static NEXT_READER: AtomicU64 = AtomicU64::new(1);

/// 会话的运行时状态。
///
/// 2026-11-08 类型本体搬到 `term_core::session_runtime`（控制面要给 UI 报
/// `Session::status`，而 `term-core` 不能反过来依赖本 crate）。这里保留同一
/// 类型的**再导出**，`supervisor::SessionStatus` 这个路径继续可用。
pub use term_core::session_runtime::SessionStatus;

enum Command {
    Write(Vec<u8>),
    Resize(usize, usize),
    Clear,
    /// 只丢历史（屏幕留着）。与 `Clear` 分开：参考实现把「清屏 / 清历史 / 两者都清」
    /// 做成三条菜单，语义不同。
    ClearScrollback,
    /// 清屏 + 清历史。
    ClearAll,
    Close,
    /// R1：请求任意行区间的事实窗口（不产生新事实）。
    RequestRange(ViewRange),
    /// P（终端滚动条 / 悬停预览）：请求一份**预览用**的事实窗口。
    ///
    /// 与 `RequestRange` 走物理隔离的第二通道（ADR-0028）：各自的事件落进
    /// 各自的槽（`screen` / `preview_screen`），一帧内两份窗口互不覆盖。
    /// 旧变体签名与语义一字不改。
    RequestPreview(ViewRange),
    /// R14-a：开始记录到 `dir` 下。
    ///
    /// 文件名、每行格式、覆盖 / 追加、大小上限全在 `opts` 里（配置面在
    /// `term_core::log_opts`）。**选项由调用方带进来，不在这里读偏好**：
    /// 这一层不认识 `prefs.json`，它只认识「按这份配置落盘」。
    LogStart {
        dir: PathBuf,
        session: String,
        /// 宿主名（`%{host}` 用；不知道时是空串）。
        host: String,
        opts: term_core::log_opts::LogOptions,
    },
    LogStop,
    LogPause,
    LogResume,
    /// ADR-0019：登记一次输出等待（脚本 / 桥接 / agent 的 `wait` 原语）。
    ///
    /// `ack` 由监管者线程在**把登记放进匹配队列后**发回：调用方拿到
    /// `register_waiter` 的返回值时，登记点之后到达的输出保证会被匹配。
    /// 这是「先登记、后消费」的实现——命令循环本来就在 `drain` 之前。
    RegisterWaiter {
        pattern: String,
        id: u64,
        ack: Sender<()>,
        result: Sender<WaitMatch>,
    },
    /// 取消一次未完成的登记（超时 / 调用方放弃）。没人等的匹配不能留在循环里。
    CancelWaiter(u64),
    /// ADR-0019 增量：登记一个输出读取器（`read` / `read_until` 的捕获源）。
    ///
    /// 与 `RegisterWaiter` 同款「先入队、后 ack」——登记点之后到达的
    /// 输出保证进入累计。`signal` 是数据到达通知（边沿触发：累计从空转
    /// 非空时发一个），消费方据此阻塞等待而不是轮询。
    RegisterReader {
        id: u64,
        ack: Sender<()>,
        signal: Sender<()>,
    },
    /// 取走读取器的累计文本（应答即清空）。消费点语义在客户端侧
    /// （`term-script` 的捕获游标）维护。
    TakeReader { id: u64, resp: Sender<String> },
    /// 取消读取器登记（句柄被 drop 也要清掉，没人取的累计不能留着）。
    CancelReader(u64),
    /// W3：登记一条**原始字节旁路**（字节级传输协议 X/Y/Z Modem 的入口）。
    ///
    /// 与 `RegisterReader` 同款「先入队、后 ack」：ack 到手即生效，
    /// 登记点之后 `drain()` 到的字节保证整块进 `sink`。
    ///
    /// 旁路期间三条语义（都不是 bug）：
    /// 1. **屏幕冻结是对的**：字节不进 VT，快照不更新、`version` 不涨。
    ///    传输进度由界面自己画，屏幕本来就不该动。
    /// 2. **结束时恢复**：`DetachRaw` 之后下一次 `drain()` 的字节重新进 VT。
    /// 3. **对话回包不参与**：VT 没吃到字节，`take_replies` 自然无事可做，
    ///    这里也不补写任何东西到 channel——协议的回包由传输层自己 `Write`。
    AttachRaw {
        sink: Sender<Vec<u8>>,
        ack: Sender<()>,
    },
    /// 撤销旁路，恢复正常 VT 处理（日志 / 清洗 / 快照一起回来）。
    DetachRaw,
    /// **非破坏性 tee**：VT 照吃这一份，另拷一份给 sink。
    ///
    /// 与 [`Command::AttachRaw`] 是**相反**的语义，务必别混：
    /// - `AttachRaw` 是**改道**——VT 不吃、屏幕冻结，字节级协议用；
    /// - `AttachTap` 是**复制**——VT 照吃、屏幕正常前进，hex 视图用。
    ///
    /// 之所以要 tee 而不是让 UI 从 VT 反推：字节进了 VT 只剩字符网格，
    /// 控制序列已经消失了，反推不出原始字节。
    ///
    /// 与 `RegisterReader` 同款「先入队、后 ack」：ack 到手即生效，
    /// 登记点之后的字节保证进 `sink`。UI 靠这个顺序保证「挂上 tap 之后再
    /// 清空本地环」不会丢掉两块之间的字节。
    AttachTap {
        sink: Sender<Vec<u8>>,
        ack: Sender<()>,
    },
    /// 撤销 tee（幂等）。VT 一直没停过，所以撤销不需要等 ack。
    DetachTap,
    /// 当前屏幕快照的直通应答（`Session::screen` 用，不碰 UI 的邮箱）。
    Snapshot { resp: Sender<GridSnapshot> },
    /// 非破坏性的状态查询（不偷 `poll()` 的事件）。
    QueryState { resp: Sender<SessionState> },
}

enum Event {
    /// R1：按范围切出的只读事实窗口。
    Screen(ScreenImage),
    /// P：按范围切出的**预览用**只读事实窗口（悬停滚动条时的历史快照）。
    ///
    /// 落进 `SupervisorHandle.preview_screen` 独立槽，与 `Screen` 物理隔离
    /// （ADR-0028）。旧 `Screen` 变体一字不改。
    PreviewScreen(ScreenImage),
    Finished(String),
    /// R14-a：日志状态变化（开始 / 暂停 / 恢复 / 停止 / 失败）。
    ///
    /// 只在**变化**时发，不跟着每块输出发：字节数由界面直接问文件，
    /// 状态才是这里独有的东西。
    Log(LogStatus),
}

/// 一槽邮箱：监管者线程往里放「最新一份」，UI 取走后才允许放下一份。
///
/// 一个槽就够——屏幕只有一份最新事实，中间的版本没有保留价值。
/// `full` 由 `Mutex<Option<..>>` 本身表达：`Some` 即「还没被取走」。
struct Mailbox {
    slot: Mutex<Option<GridSnapshot>>,
    /// UI 注入的唤醒回调（`ctx.request_repaint()`）。由 UI 在 spawn 之后设置。
    wakeup: Mutex<Option<Arc<dyn Fn() + Send + Sync>>>,
}

impl Mailbox {
    fn new() -> Self {
        Self {
            slot: Mutex::new(None),
            wakeup: Mutex::new(None),
        }
    }

    fn set_wakeup(&self, f: Arc<dyn Fn() + Send + Sync>) {
        if let Ok(mut g) = self.wakeup.lock() {
            *g = Some(f);
        }
        // 启动竞态：线程可能在我们装上回调之前就已经放进了一份快照
        // （`spawn` 到 `set_wakeup` 之间有几微秒）。这时槽是满的、那次唤醒已经错过，
        // 不补一次的话界面会一直停在空屏，直到用户碰一下鼠标。
        if !self.is_empty() {
            self.wake();
        }
    }

    fn wake(&self) {
        let f = self.wakeup.lock().ok().and_then(|g| g.clone());
        if let Some(f) = f {
            f();
        }
    }

    fn is_empty(&self) -> bool {
        self.slot.lock().map(|g| g.is_none()).unwrap_or(true)
    }

    fn publish(&self, snap: GridSnapshot) {
        if let Ok(mut g) = self.slot.lock() {
            *g = Some(snap);
        }
        self.wake();
    }

    fn take(&self) -> Option<GridSnapshot> {
        self.slot.lock().ok().and_then(|mut g| g.take())
    }
}

/// UI 侧唯一的会话句柄。所有 OS 资源都在监管者线程里。
pub struct SupervisorHandle {
    id: SessionId,
    /// 包 `Mutex` 的原因：`PendingWait` 也要持一份引用来发取消命令
    /// （`std::sync::mpsc::Sender` 不是 `Clone` 也不是 `Sync`，裸共享不行）。
    cmd: Arc<Mutex<Sender<Command>>>,
    events: Receiver<Event>,
    /// R6：快照的一槽邮箱（与监管者线程共享）。
    mailbox: Arc<Mailbox>,
    /// 最近一次快照；`poll()` 后由 UI 复制到 `Tab.snapshot`。
    pub snapshot: GridSnapshot,
    /// R1：最近一次范围请求的只读事实窗口；UI 只存不写。
    pub screen: Option<ScreenImage>,
    /// P：最近一次**预览**范围请求的只读事实窗口（悬停滚动条时的历史快照）。
    ///
    /// 与 `screen` **物理隔离**：两个事件落进两个槽，一帧内同时到达也不会
    /// 互相覆盖（ADR-0028 §3.3 的闪白屏失败模式在结构上不可能发生）。
    /// UI 只存不写。
    pub preview_screen: Option<ScreenImage>,
    pub status: SessionStatus,
    /// R14-a：会话日志状态（关闭 / 记录中 / 已暂停 / 失败）。
    pub log: LogStatus,
    last_size: (usize, usize),
    /// 退出原因的最后落点（与监管者线程共享）。
    ///
    /// 为什么不能只用线程里的局部变量：进程退出后监管者线程**会结束**，
    /// 命令循环随之消失，`Command::QueryState` 再也拿不到应答。于是
    /// 「已结束但退出原因是什么」只能靠它留下的这一格——否则
    /// `query_state` 只能报 `Finished(None)`，客户端问不出「正常结束还是崩了」。
    /// 这不是缓存读：它只在退出那一刻被写一次，之后不再变。
    exit_note: Arc<Mutex<Option<String>>>,
}

impl SupervisorHandle {
    /// 合并本帧到达的所有事件，返回是否发生变化（用于决定是否重绘）。
    ///
    /// 快照从一槽邮箱里取：取到就说明有新事实，没取到就保持原样。
    pub fn poll(&mut self) -> bool {
        let mut changed = false;
        if let Some(s) = self.mailbox.take() {
            self.snapshot = s;
            changed = true;
        }
        while let Ok(ev) = self.events.try_recv() {
            match ev {
                Event::Screen(s) => {
                    self.screen = Some(s);
                    changed = true;
                }
                Event::PreviewScreen(s) => {
                    self.preview_screen = Some(s);
                    changed = true;
                }
                Event::Finished(st) => {
                    self.status.exit_status = Some(st);
                    self.status.alive = false;
                    changed = true;
                }
                Event::Log(st) => {
                    self.log = st;
                    changed = true;
                }
            }
        }
        changed
    }

    /// R6：注入唤醒回调。UI 传 `ctx.request_repaint()`，让「有输出」能立刻重绘。
    ///
    /// 放在 handle 上而不是构造参数里，是因为线程先起、`Context` 后到；
    /// 没设置时 `wake()` 是空操作（无头运行器就不需要）。
    pub fn set_wakeup(&self, f: Arc<dyn Fn() + Send + Sync>) {
        self.mailbox.set_wakeup(f);
    }

    /// 所有命令的统一出口。监管者线程死了之后发送失败被静默吞掉——
    /// 与旧行为一致（`let _ = self.cmd.send(...)`），状态查询另有应答路径。
    fn send_cmd(&self, cmd: Command) {
        if let Ok(g) = self.cmd.lock() {
            let _ = g.send(cmd);
        }
    }

    pub fn write(&self, bytes: &[u8]) {
        self.send_cmd(Command::Write(bytes.to_vec()));
    }

    /// R1：请求任意行区间的事实窗口。不产生新事实，`version` 不变。
    pub fn request_range(&self, range: ViewRange) {
        self.send_cmd(Command::RequestRange(range));
    }

    /// P：请求一份**预览用**的事实窗口（悬停滚动条时的历史快照）。
    ///
    /// 走独立的 `Command::RequestPreview` → `Event::PreviewScreen` 通道，与
    /// `request_range` 物理隔离（ADR-0028）。不产生新事实，`version` 不变。
    pub fn request_preview_range(&self, range: ViewRange) {
        self.send_cmd(Command::RequestPreview(range));
    }

    /// 去重后下发：UI 每帧都可以安全调用。
    pub fn resize(&mut self, cols: usize, rows: usize) {
        if self.last_size != (cols, rows) {
            self.last_size = (cols, rows);
            self.send_resize(cols, rows);
        }
    }

    /// 不去重的直发（`Session::resize` 的 `&self` 路径用；去重由调用方做）。
    pub fn send_resize(&self, cols: usize, rows: usize) {
        self.send_cmd(Command::Resize(cols, rows));
    }

    pub fn clear(&self) {
        self.send_cmd(Command::Clear);
    }

    /// 只丢历史，屏幕留着。
    pub fn clear_scrollback(&self) {
        self.send_cmd(Command::ClearScrollback);
    }

    /// 清屏 + 清历史。
    pub fn clear_all(&self) {
        self.send_cmd(Command::ClearAll);
    }

    /// R14-a：开始记录当前会话。
    ///
    /// 目录由调用方给（`log::default_dir()`），文件名 / 行格式 / 覆盖方式 /
    /// 大小上限由 `opts` 给；`self.log` 会在下一次 `poll()` 后变成真实状态
    /// （含失败原因）。
    pub fn start_log(&self, dir: PathBuf, session: &str, opts: term_core::log_opts::LogOptions) {
        self.start_log_with_host(dir, session, "", opts);
    }

    /// 带宿主名的一档（ADR-0027 §S5）：`%{host}` 只在调用方真的知道主机时才有值。
    pub fn start_log_with_host(
        &self,
        dir: PathBuf,
        session: &str,
        host: &str,
        opts: term_core::log_opts::LogOptions,
    ) {
        self.send_cmd(Command::LogStart {
            dir,
            session: session.to_string(),
            host: host.to_string(),
            opts,
        });
    }

    pub fn stop_log(&self) {
        self.send_cmd(Command::LogStop);
    }

    pub fn pause_log(&self) {
        self.send_cmd(Command::LogPause);
    }

    pub fn resume_log(&self) {
        self.send_cmd(Command::LogResume);
    }

    pub fn close(&mut self) {
        self.send_cmd(Command::Close);
        self.status.connected = false;
        self.status.alive = false;
    }

    pub fn id(&self) -> SessionId {
        self.id
    }

    /// 当前屏幕快照的直通应答（`Session::screen`）。线程已死返回 `None`。
    ///
    /// 与 `poll()` 的邮箱互不干扰：那是 UI 的单一消费者通道，这里是
    /// 控制面客户端（脚本 / 桥接 / agent）的按需查询。
    pub fn snapshot_once(&self) -> Option<GridSnapshot> {
        let (tx, rx) = channel();
        self.send_cmd(Command::Snapshot { resp: tx });
        rx.recv_timeout(Duration::from_secs(2)).ok()
    }

    /// 非破坏性的状态查询：不碰 `poll()` 的邮箱与事件（那是 UI 的）。
    pub fn query_state(&self) -> Option<SessionState> {
        // 先看线程留下的退出原因：线程结束之后命令队列的接收端随之 drop，
        // `QueryState` 只会立刻拿到 `Disconnected`，于是「已结束」会丢成
        // `Finished(None)`——退出码就这么没了。这一格正是为那一刻准备的。
        if let Ok(note) = self.exit_note.lock() {
            if let Some(note) = note.clone() {
                return Some(SessionState::Finished(Some(note)));
            }
        }
        let (tx, rx) = channel();
        self.send_cmd(Command::QueryState { resp: tx });
        rx.recv_timeout(Duration::from_secs(2)).ok()
    }

    /// 登记一次输出等待（`Session::wait_register` 的直通）。
    ///
    /// 返回即已生效：登记命令带 ack，监管者线程在下一轮**取输出之前**
    /// 处理它（命令循环本来就在 `drain` 之前），所以登记点之后到达的
    /// 输出保证进入匹配——这是 ADR-0019 §B.13「先登记、后消费」。
    pub fn register_waiter(&self, pattern: &str) -> Result<PendingWait, WaitError> {
        if pattern.is_empty() {
            return Err(WaitError::EmptyPattern);
        }
        let (ack_tx, ack_rx) = channel();
        let (res_tx, res_rx) = channel();
        let id = NEXT_WAITER.fetch_add(1, Ordering::Relaxed);
        self.send_cmd(Command::RegisterWaiter {
            pattern: pattern.to_string(),
            id,
            ack: ack_tx,
            result: res_tx,
        });
        // 等 ack：没等到 = 线程已死，登记没生效，如实报错而不是装作在等。
        ack_rx.recv().map_err(|_| WaitError::Closed)?;
        Ok(PendingWait {
            id,
            rx: res_rx,
            cmd: Arc::clone(&self.cmd),
        })
    }

    /// 登记一个输出读取器（`Session::reader_register` 的直通）。
    ///
    /// 与 [`register_waiter`](Self::register_waiter) 同款「先登记后消费」：
    /// ack 到手即生效，登记点之后到达的输出保证可被 `collect` 取到。
    pub fn register_reader(&self) -> Result<PendingReader, WaitError> {
        let (ack_tx, ack_rx) = channel();
        let (sig_tx, sig_rx) = channel();
        let id = NEXT_READER.fetch_add(1, Ordering::Relaxed);
        self.send_cmd(Command::RegisterReader {
            id,
            ack: ack_tx,
            signal: sig_tx,
        });
        ack_rx.recv().map_err(|_| WaitError::Closed)?;
        Ok(PendingReader {
            id,
            signal: sig_rx,
            cmd: Arc::clone(&self.cmd),
        })
    }

    /// W3：旁路控制端的轻量克隆（`term-session` 存 `dyn Session`，拿不回
    /// 具体句柄时用它）。语义与 [`attach_raw`](Self::attach_raw) 完全一样。
    pub fn raw_control(&self) -> RawControl {
        RawControl {
            cmd: Arc::clone(&self.cmd),
        }
    }

    /// W3：把输出字节改道到 `sink`（字节级传输协议的入口）。
    ///
    /// 阻塞到登记生效为止（同 [`register_reader`](Self::register_reader)）。
    /// 生效之后 `drain()` 到的字节**整块**进 `sink`，不进 VT、不落日志、
    /// 不进 `read` 的累计——三条语义见 [`Command::AttachRaw`]。
    /// 监管者线程已死时静默返回（与其它命令同口径）。
    pub fn attach_raw(&self, sink: Sender<Vec<u8>>) {
        self.raw_control().attach(sink);
    }

    /// W3：撤销旁路。之后 `drain()` 到的字节重新进 VT（屏幕恢复更新）。
    pub fn detach_raw(&self) {
        self.raw_control().detach();
    }

    /// 挂一条**非破坏性**字节 tee（hex 视图的输入源）。
    ///
    /// 阻塞到登记生效为止：返回之后到达的字节保证进 `sink`，而 VT 一直
    /// 在照常吃同一份——这一点与 [`attach_raw`](Self::attach_raw) 正相反。
    pub fn attach_tap(&self, sink: Sender<Vec<u8>>) {
        self.raw_control().attach_tap(sink);
    }

    /// 摘掉 tee。VT 从没停过，所以不需要等生效时刻（幂等）。
    pub fn detach_tap(&self) {
        self.raw_control().detach_tap();
    }
}

/// 输出**旁路**（raw）与**拷贝**（tap）两个控制端合成的一个可克隆句柄。
///
/// 存在的理由只有一个：控制面（`term-session`）把会话存成
/// `Arc<Mutex<dyn Session>>`，借不回 `SupervisorHandle` 的具体类型，
/// 而旁路登记只能走监管者的命令队列。由
/// [`SupervisorHandle::raw_control`] 导出，方法与会话句柄同名方法同语义。
///
/// 两条通路共用一个类型而不是各建一个：它们要的东西完全一样（命令队列的
/// 发送端 + 一条 ack 回路），差别只在监管者线程里落到哪个槽位。
#[derive(Clone)]
pub struct RawControl {
    cmd: Arc<Mutex<Sender<Command>>>,
}

impl RawControl {
    /// 登记旁路并阻塞等 ack：返回即生效（登记点之后的字节归 `sink`）。
    pub fn attach(&self, sink: Sender<Vec<u8>>) {
        let (ack_tx, ack_rx) = channel();
        if let Ok(g) = self.cmd.lock() {
            let _ = g.send(Command::AttachRaw {
                sink,
                ack: ack_tx,
            });
        }
        // 线程已死时 ack 收不到：静默返回，与其它命令的失败口径一致。
        let _ = ack_rx.recv();
    }

    /// 撤销旁路。不等 ack：命令先于取输出被处理，所以「下发撤销之后」到达的
    /// 字节一定重新进 VT；撤销本身是幂等的，调用方不需要知道生效时刻。
    pub fn detach(&self) {
        if let Ok(g) = self.cmd.lock() {
            let _ = g.send(Command::DetachRaw);
        }
    }

    /// 挂一条非破坏性 tee 并阻塞等 ack：返回即生效（登记点之后的字节进 `sink`）。
    ///
    /// 与控制端其它方法共用一条命令队列，所以顺序天然与 `write` / `attach`
    /// 一致——UI 需要的正是「挂上之后再清本地环」这个顺序保证。
    pub fn attach_tap(&self, sink: Sender<Vec<u8>>) {
        let (ack_tx, ack_rx) = channel();
        if let Ok(g) = self.cmd.lock() {
            let _ = g.send(Command::AttachTap {
                sink,
                ack: ack_tx,
            });
        }
        // 线程已死时 ack 收不到：静默返回，与 attach 同口径。
        let _ = ack_rx.recv();
    }

    /// 摘掉 tee。VT 一直在推进，没有「生效时刻」需要对齐，所以不等 ack。
    pub fn detach_tap(&self) {
        if let Ok(g) = self.cmd.lock() {
            let _ = g.send(Command::DetachTap);
        }
    }
}

/// 一次已登记、尚未完成的等待。`wait` 阻塞调用线程直到命中 / 超时 /
/// 会话结束——脚本线程阻塞是设计内的（ZOC / SecureCRT 同款语义）。
#[derive(Debug)]
pub struct PendingWait {
    id: u64,
    rx: Receiver<WaitMatch>,
    cmd: Arc<Mutex<Sender<Command>>>,
}

impl PendingWait {
    fn cancel(&self) {
        if let Ok(g) = self.cmd.lock() {
            let _ = g.send(Command::CancelWaiter(self.id));
        }
    }
}

/// 取消检查的切片长度（ADR-0027 S1）：界面点「停止」到阻塞点返回的上限。
///
/// 50ms 是「人感觉不到延迟」与「每秒 20 次唤醒」的折中；比它更细只多烧 CPU，
/// 比它更粗就开始像「点了没反应」。
const CANCEL_SLICE: Duration = Duration::from_millis(50);

impl WaitRegistration for PendingWait {
    fn wait(self: Box<Self>, timeout: Duration) -> Result<WaitMatch, WaitError> {
        let this = *self;
        match this.rx.recv_timeout(timeout) {
            Ok(m) => Ok(m),
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
                this.cancel();
                Err(WaitError::Timeout)
            }
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => Err(WaitError::Closed),
        }
    }

    /// 切片循环：**不重登记**（重登记会丢掉两次登记之间的输出），只在
    /// `recv_timeout` 的空档看一眼令牌。
    fn wait_cancellable(
        self: Box<Self>,
        timeout: Duration,
        cancel: &dyn Cancel,
    ) -> Result<WaitMatch, WaitError> {
        let this = *self;
        let deadline = Instant::now() + timeout;
        loop {
            if cancel.is_cancelled() {
                this.cancel();
                return Err(WaitError::Cancelled);
            }
            let now = Instant::now();
            if now >= deadline {
                this.cancel();
                return Err(WaitError::Timeout);
            }
            match this.rx.recv_timeout((deadline - now).min(CANCEL_SLICE)) {
                Ok(m) => return Ok(m),
                Err(std::sync::mpsc::RecvTimeoutError::Timeout) => continue,
                Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
                    return Err(WaitError::Closed)
                }
            }
        }
    }
}

impl Drop for PendingWait {
    fn drop(&mut self) {
        // 没等到就丢弃（调用方放弃）也要取消登记：没人等的匹配不能留在循环里。
        self.cancel();
    }
}

/// 一次已登记的输出读取。`collect` 阻塞调用线程直到有新数据 / 超时 /
/// 会话结束（脚本线程阻塞是设计内的）。
pub struct PendingReader {
    id: u64,
    /// 数据到达通知（边沿触发：累计从空转非空时发一个）。
    signal: Receiver<()>,
    cmd: Arc<Mutex<Sender<Command>>>,
}

impl PendingReader {
    /// 取走当前累计（应答即清空）。`Gone` = 监管者线程已死。
    fn take(&self) -> TakeOutcome {
        let (tx, rx) = channel();
        let sent = self
            .cmd
            .lock()
            .map(|g| g.send(Command::TakeReader { id: self.id, resp: tx }).is_ok())
            .unwrap_or(false);
        if !sent {
            return TakeOutcome::Gone;
        }
        match rx.recv_timeout(Duration::from_secs(2)) {
            Ok(text) => TakeOutcome::Text(text),
            Err(_) => TakeOutcome::Gone,
        }
    }

    fn cancel(&self) {
        if let Ok(g) = self.cmd.lock() {
            let _ = g.send(Command::CancelReader(self.id));
        }
    }
}

enum TakeOutcome {
    Text(String),
    Gone,
}

impl OutputReader for PendingReader {
    fn collect_cancellable(
        &mut self,
        timeout: Duration,
        cancel: &dyn Cancel,
    ) -> Result<String, WaitError> {
        let deadline = Instant::now() + timeout;
        loop {
            if cancel.is_cancelled() {
                return Err(WaitError::Cancelled);
            }
            match self.take() {
                TakeOutcome::Text(text) if !text.is_empty() => return Ok(text),
                TakeOutcome::Text(_) => {}
                TakeOutcome::Gone => return Err(WaitError::Closed),
            }
            let now = Instant::now();
            if now >= deadline {
                return Ok(String::new());
            }
            match self.signal.recv_timeout((deadline - now).min(CANCEL_SLICE)) {
                Ok(()) => continue,
                Err(std::sync::mpsc::RecvTimeoutError::Timeout) => return Ok(String::new()),
                Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
                    return match self.take() {
                        TakeOutcome::Text(text) if !text.is_empty() => Ok(text),
                        _ => Err(WaitError::Closed),
                    };
                }
            }
        }
    }

    fn collect(&mut self, timeout: Duration) -> Result<String, WaitError> {
        let deadline = Instant::now() + timeout;
        loop {
            // 先取：有积累立返（消费点之后的新数据一次拿全）。
            match self.take() {
                TakeOutcome::Text(text) if !text.is_empty() => return Ok(text),
                TakeOutcome::Text(_) => {}
                TakeOutcome::Gone => return Err(WaitError::Closed),
            }
            let now = Instant::now();
            if now >= deadline {
                return Ok(String::new());
            }
            // 没有积累 → 等到达通知（绝不轮询）。
            match self.signal.recv_timeout(deadline - now) {
                Ok(()) => continue,
                Err(std::sync::mpsc::RecvTimeoutError::Timeout) => return Ok(String::new()),
                Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
                    // 监管者线程已死：取不到就是 Closed。
                    return match self.take() {
                        TakeOutcome::Text(text) if !text.is_empty() => Ok(text),
                        _ => Err(WaitError::Closed),
                    };
                }
            }
        }
    }
}

impl Drop for PendingReader {
    fn drop(&mut self) {
        // 句柄丢弃也要取消登记：没人取的累计不能在监管者循环里白占内存。
        self.cancel();
    }
}

impl Drop for SupervisorHandle {
    fn drop(&mut self) {
        self.send_cmd(Command::Close);
    }
}

/// 启动一个受监管的会话。失败时（例如没有可执行的 $SHELL）返回错误文本。
pub fn spawn(
    config: SessionConfig,
    cols: usize,
    rows: usize,
) -> Result<SupervisorHandle, SpawnError> {
    let (init_cols, init_rows) = initial_size(&config, cols, rows);
    let ch = open_channel(config.clone(), init_cols, init_rows)?;
    Ok(start(config, init_cols, init_rows, ch))
}

/// 无头运行器用：执行一条一次性命令。
pub fn spawn_exec(
    command: &str,
    cols: usize,
    rows: usize,
) -> Result<SupervisorHandle, SpawnError> {
    let ch = open_exec_channel(command, cols, rows)?;
    Ok(start(
        SessionConfig::new(Protocol::Shell),
        cols,
        rows,
        ch,
    ))
}

/// R13-d2：把一个**已经建好**的通道交给监管者。
///
/// 这是原生 SSH 的接缝。`term-ssh::SshChannel` 实现了 `TermChannel`，但本 crate
/// **不能**依赖 `term-ssh`——那会成环（`term-ssh` 要 `TermChannel` 这个 trait）。
/// 所以由应用层建好通道递进来：`run()` 一行没改，这正是 ADR-0014 当初把通道
/// 从 `Tab` 抽出来的目的。
///
/// 与 `spawn()` 的区别只有「谁负责打开它」：`spawn()` 按 `SpawnPlan` 起 PTY，
/// 这里什么都不起，直接开始监管。失败在调用方那边（连接失败就没人调用这个函数）。
pub fn spawn_with_channel(
    config: SessionConfig,
    cols: usize,
    rows: usize,
    ch: Box<dyn TermChannel>,
) -> SupervisorHandle {
    start(config, cols, rows, ch)
}

/// 初始网格尺寸：会话配置里设了 `cols`/`rows`（两者都 >0）就优先用，否则沿用
/// 调用方传来的视口尺寸。`0` 表示「未设置」（默认行为，零回归）。
fn initial_size(config: &SessionConfig, cols: usize, rows: usize) -> (usize, usize) {
    if config.cols > 0 && config.rows > 0 {
        (config.cols, config.rows)
    } else {
        (cols, rows)
    }
}

fn start(
    config: SessionConfig,
    cols: usize,
    rows: usize,
    ch: Box<dyn TermChannel>,
) -> SupervisorHandle {
    // 会话配置里显式设了 `cols`/`rows`（>0）就作为初始网格尺寸；否则沿用调用方
    // 传来的视口尺寸（默认行为，零回归）。这样 New Session 的 Window▸Selection
    // 页设的列/行能真正决定初始终端大小。
    let (init_cols, init_rows) = initial_size(&config, cols, rows);
    let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
    let (cmd_tx, cmd_rx) = channel();
    let (ev_tx, ev_rx) = channel();
    let mailbox = Arc::new(Mailbox::new());
    let worker_mailbox = Arc::clone(&mailbox);
    let snapshot = GridSnapshot::empty(init_cols, init_rows);
    let scrollback = config.scrollback;
    // 退出原因的最后落点：线程退出时写一次，句柄随时可读。
    let exit_note = Arc::new(Mutex::new(None));
    let worker_exit_note = Arc::clone(&exit_note);

    if let Err(e) = std::thread::Builder::new()
        .name(format!("zterm-supervisor-{id}"))
        .spawn(move || {
            // L1（`docs/design/10-crash-hardening.md`）：**会话级 panic
            // 隔离**。这一层是「10 个会话里 1 个炸了不该全陪葬」的唯一防线：
            // 线程体在 `catch_unwind` 里跑，panic 被吞在本线程，其余会话的监管者
            // 线程与 UI 主循环都不受影响。
            //
            // 为什么 `AssertUnwindSafe` 是可以的：`run` 的所有状态都在**本线程私有**
            // 的局部变量里（`vt` / `mailbox` 引用 / 通道端），panic 之后没有第二个
            // 线程会去读它们——线程结束了，`ev_tx` 随展开一起掉，界面下一次
            // `poll` 看到通道关闭就把这条会话标成断开。也就是说「跨 unwind 边界
            // 带着可能不一致状态的引用」这件事在这里不存在。
            //
            // 崩溃**不是**静默的：`exit_note` 写一句可读的原因，状态栏照常显示，
            // 用户能看懂「这条会话内部出错」而不是「莫名其妙掉线」。
            let note = Arc::clone(&worker_exit_note);
            let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                run(
                    ch,
                    cmd_rx,
                    ev_tx,
                    worker_mailbox,
                    worker_exit_note,
                    Terminal::new(init_cols, init_rows, scrollback),
                )
            }));
            if let Err(payload) = outcome {
                // 注意 `&*payload` 而不是 `&payload`：`payload` 是 `Box<dyn Any + Send>`，
                // 而 `Box<dyn Any + Send>` 自己也是 `'static`（`dyn Any + Send` 隐含
                // `+ 'static`），所以 `&payload` 会**unsize 成「Box 本身」的 `&dyn Any`**，
                // 两个 downcast 全落空——实测过，退化成「无法读取消息」。
                let reason = crate::panic_message(&*payload);
                eprintln!("session supervisor {id} panicked（本会话已隔离）: {reason}");
                if let Ok(mut slot) = note.lock() {
                    *slot = Some(format!("会话内部错误：{reason}"));
                }
            }
        })
    {
        eprintln!("could not start supervisor thread {id}: {e}");
    }

    SupervisorHandle {
        id,
        cmd: Arc::new(Mutex::new(cmd_tx)),
        events: ev_rx,
        mailbox,
        snapshot,
        screen: None,
        preview_screen: None,
        status: SessionStatus {
            connected: true,
            alive: true,
            exit_status: None,
        },
        log: LogStatus::default(),
        last_size: (init_cols, init_rows),
        exit_note,
    }
}

/// 刚有活动时的等待：输出往往是成串来的，短等一下能立刻接上下一批。
const ACTIVE_WAIT: Duration = Duration::from_millis(2);
/// 完全空闲时的等待上限：阻塞等下一条命令，最坏情况下的输出延迟也就是这个数
/// —— 与一帧同量级，肉眼看不出来。
const IDLE_WAIT: Duration = Duration::from_millis(20);

fn run(
    mut ch: Box<dyn TermChannel>,
    cmd_rx: Receiver<Command>,
    ev_tx: Sender<Event>,
    mailbox: Arc<Mailbox>,
    exit_note: Arc<Mutex<Option<String>>>,
    mut vt: Terminal,
) {
    let mut version = 0u64;
    // 有变化但还没能放进邮箱（上一份还没被取走）。下一轮槽空了就补上。
    let mut pending = false;
    // 空闲等待时用 `recv_timeout` 直接取到的那条命令，下一轮开头先处理它。
    let mut queued: Option<Command> = None;
    // R14-a：日志写入端。`None` = 没在记录。
    let mut log: Option<SessionLog> = None;
    // wait 匹配器（ADR-0019）：登记经命令队列生效，匹配在输出流上做。
    // 清洗器跨 chunk 保留解析状态——与 vt 吃同一份字节，语义同源。
    let mut cleaner = TextCleaner::new();
    let mut waiters: Vec<ActiveWaiter> = Vec::new();
    // 输出读取器（read / read_until 的捕获源）：与 waiters 同一轮喂同一份清洗文本。
    let mut readers: Vec<ActiveReader> = Vec::new();
    // W3：原始字节旁路。`Some` = 字节级传输协议在跑，字节只进这个 sink。
    let mut raw_sink: Option<Sender<Vec<u8>>> = None;
    // 非破坏性 tee。`Some` = hex 视图在看着，字节除了 VT 再拷一份过去。
    // 与 `raw_sink` 是两个独立的槽位：raw 期间走早退分支，tee 自然收不到。
    let mut tap_sink: Option<Sender<Vec<u8>>> = None;
    // 会话退出状态的缓存：`Event::Finished` 是发给 UI 的，状态查询不能
    // 把它偷走（偷走了界面的状态栏就再也看不到退出原因）。
    let mut last_exit: Option<String> = None;
    // 退出只报告一次：真实 PTY 的 `poll()` 会反复返回同一份状态文本，
    // 每轮重发既无意义，也让下游的收尾排空等不到「安静的一轮」。
    let mut exit_reported = false;
    loop {
        let mut need_snapshot = false;
        let mut closed = false;
        let mut requested: Vec<ViewRange> = Vec::new();
        // P：预览走物理隔离的第二通道（ADR-0028）。每轮重置，与 `requested` 平行。
        let mut preview_requested: Vec<ViewRange> = Vec::new();
        let mut active = false;

        let mut cmds: Vec<Command> = Vec::new();
        if let Some(c) = queued.take() {
            cmds.push(c);
        }
        while let Ok(cmd) = cmd_rx.try_recv() {
            cmds.push(cmd);
        }
        for cmd in cmds {
            active = true;
            match cmd {
                Command::RequestRange(r) => requested.push(r),
                // P：预览请求进独立队列（ADR-0028）。
                Command::RequestPreview(r) => preview_requested.push(r),
                Command::Write(b) => ch.write(&b),
                Command::Resize(c, r) => {
                    ch.resize(c, r);
                    vt.resize(c, r);
                    need_snapshot = true;
                }
                Command::Clear => {
                    vt.clear();
                    need_snapshot = true;
                }
                Command::ClearScrollback => {
                    vt.clear_scrollback();
                    // 历史一清，行号与视口都要跟着重算：不重发快照，界面会停在
                    // 那个已经不存在的回看位置上。
                    need_snapshot = true;
                }
                Command::ClearAll => {
                    vt.clear_screen_and_scrollback();
                    need_snapshot = true;
                }
                Command::LogStart {
                    dir,
                    session,
                    host,
                    opts,
                } => {
                    // 上一次还在记录就先收尾：一条会话同时只有一个日志文件，
                    // 否则「Open Log File」打开的是哪一份就成了运气。
                    if let Some(mut old) = log.take() {
                        old.finish();
                    }
                    match SessionLog::create_for(&dir, &session, &host, vt.cols, vt.rows, &opts) {
                        Ok(l) => {
                            let _ = ev_tx.send(Event::Log(l.status()));
                            log = Some(l);
                        }
                        Err(e) => {
                            let _ = ev_tx.send(Event::Log(LogStatus {
                                phase: crate::log::LogPhase::Failed(format!(
                                    "打不开日志文件：{e}"
                                )),
                                path: None,
                            }));
                        }
                    }
                }
                Command::LogStop => {
                    if let Some(mut l) = log.take() {
                        l.finish();
                        // 保留路径：停掉之后用户还要「Open Log File」看刚记的那一份。
                        let st = LogStatus {
                            phase: crate::log::LogPhase::Off,
                            path: Some(l.path().to_path_buf()),
                        };
                        let _ = ev_tx.send(Event::Log(st));
                    }
                }
                Command::LogPause => {
                    if let Some(l) = log.as_mut() {
                        l.pause();
                        let _ = ev_tx.send(Event::Log(l.status()));
                    }
                }
                Command::LogResume => {
                    if let Some(l) = log.as_mut() {
                        l.resume();
                        let _ = ev_tx.send(Event::Log(l.status()));
                    }
                }
                Command::RegisterWaiter {
                    pattern,
                    id,
                    ack,
                    result,
                } => {
                    // 先入队、后 ack：调用方拿到返回值时登记已生效，
                    // 之后被 `drain` 的输出保证进入匹配。
                    waiters.push(ActiveWaiter {
                        id,
                        pattern,
                        acc: String::new(),
                        result,
                    });
                    let _ = ack.send(());
                }
                Command::CancelWaiter(id) => {
                    waiters.retain(|w| w.id != id);
                }
                Command::RegisterReader { id, ack, signal } => {
                    // 先入队、后 ack（同 RegisterWaiter）：返回即生效，
                    // 登记点之后的输出保证进入累计。
                    readers.push(ActiveReader {
                        id,
                        acc: String::new(),
                        signal,
                    });
                    let _ = ack.send(());
                }
                Command::TakeReader { id, resp } => {
                    let text = match readers.iter_mut().find(|r| r.id == id) {
                        Some(r) => std::mem::take(&mut r.acc),
                        None => String::new(),
                    };
                    let _ = resp.send(text);
                }
                Command::CancelReader(id) => {
                    readers.retain(|r| r.id != id);
                }
                Command::AttachRaw { sink, ack } => {
                    // 先入队、后 ack（同 RegisterReader）：ack 到手即生效，
                    // 登记点之后 `drain` 到的字节保证整块进 sink。
                    raw_sink = Some(sink);
                    let _ = ack.send(());
                }
                Command::DetachRaw => {
                    raw_sink = None;
                }
                Command::AttachTap { sink, ack } => {
                    // 先入队、后 ack（同 AttachRaw）：ack 到手即生效。
                    // 覆盖式登记：hex 视图挂了第二条 tap 就是第一条的 rx 已丢，
                    // 留着旧的只会每块字节都白白 clone 一次。
                    tap_sink = Some(sink);
                    let _ = ack.send(());
                }
                Command::DetachTap => {
                    tap_sink = None;
                }
                Command::Snapshot { resp } => {
                    let _ = resp.send(vt.snapshot(version));
                }
                Command::QueryState { resp } => {
                    let st = if ch.finished() || last_exit.is_some() {
                        SessionState::Finished(last_exit.clone())
                    } else {
                        SessionState::Alive
                    };
                    let _ = resp.send(st);
                }
                Command::Close => {
                    // 关闭前落盘：否则最后几 KB 停在 BufWriter 里，
                    // 用户拿到一个结尾被截断的日志。
                    if let Some(mut l) = log.take() {
                        l.finish();
                    }
                    ch.close();
                    closed = true;
                    break;
                }
            }
        }
        if closed {
            break;
        }

        let bytes = ch.drain();
        // 先算「有没有字节」：raw 分支要把 `bytes` 整个移走，下面「安静的一轮」
        // 的判据只能用它（被 move 之后再问就编译不过了）。
        let had_bytes = !bytes.is_empty();
        if had_bytes {
            active = true;
            if raw_sink.is_some() {
                // 旁路早退：旁路期间字节是纯二进制（X/Y/Z Modem 帧），整块
                // move 进 sink——不 clone，也**不**落日志 / `stamp_row` /
                // `process` / `take_replies` / 喂 cleaner / 置 `need_snapshot`。
                // 屏幕冻结是设计而非缺陷：传输进度由界面自己画，屏幕本来就
                // 不该动；VT 没吃到字节，自然也没有回包要写回 channel。
                //
                // 先把 sink 拿在手里再决定放不放回去：`bytes` 要 move 进
                // `send`，`as_ref` 的借用在这里过不去。
                let sink = raw_sink.take().expect("上面刚判过 Some");
                match sink.send(bytes) {
                    Ok(()) => raw_sink = Some(sink),
                    Err(_) => {
                        // 接收端已 drop（界面关掉了传输）。`mpsc::send` 只在这
                        // 一种情况下失败，而它是**永久性**的：留在 raw 模式就
                        // 意味着这条会话永远收不到字节、快照 `version` 不涨、
                        // 屏幕永久冻结，能发 `DetachRaw` 的那一侧又已经没了。
                        // 所以这里自己退回 VT——最坏也只是二进制被当文本刷一下
                        // 屏，比「终端看起来死了且无法恢复」好得多。
                        // 这一块字节**有意丢弃**：它是二进制帧，重放给 VT 只会
                        // 更脏。`need_snapshot` 保持原值，屏幕本来就冻着，下一轮
                        // 起恢复正常（本轮的 VT 体不在这条分支里，也不再走到）。
                    }
                }
            } else {
                // 日志先记原始输出，再交给 VT：两者拿到的是同一份字节，
                // 顺序反了也不影响，但「先记再消费」在排查崩溃时更保险。
                if let Some(l) = log.as_mut() {
                    l.write(&bytes);
                }
                // 非破坏性 tee：VT 照吃这一份，另拷一份给 UI（hex 视图）。
                // 顺序钉在 `log.write` 之后、`stamp_row` / `process` 之前：
                // 反了就会出现「tap 收到了字节而 VT 还没吃」的窗口，hex 视图
                // 与屏幕就会短暂对不上同一时刻。失败（UI 侧 rx 已丢）就地摘掉
                // ——hex 视图没了不该连累终端本身，VT 这一份照常往下走。
                // 无界 channel，`send` 不会阻塞监管者线程。
                if let Some(tap) = tap_sink.take() {
                    match tap.send(bytes.clone()) {
                        Ok(()) => tap_sink = Some(tap),
                        Err(_) => { /* 有意丢弃：UI 不再看，槽位已空 */ }
                    }
                }
                // margin 时间戳列的时刻在这里落地：宿主读钟，内核只收下（I3）。
                // 打在 `process` 之前，因为「第几行」只有在喂字节之前才是确定的——
                // 一批字节可能写穿好几行，事后再问光标在哪，问到的已经是最后一行。
                vt.stamp_row(now_secs());
                vt.process(&bytes);
                let replies = vt.take_replies();
                if !replies.is_empty() {
                    ch.write(&replies);
                }
                need_snapshot = true;

                // wait 的流上匹配（ADR-0019 §B.13）：登记命令在本轮**取输出之前**
                // 被 ack，所以这里见到的字节保证包含登记点之后到达的全部输出。
                // 匹配的是清洗后的纯文本——`ESC[31m` 不该挡住 `Password:`。
                let text = cleaner.feed(&bytes);
                if !text.is_empty() && !waiters.is_empty() {
                    waiters.retain_mut(|w| {
                        w.acc.push_str(&text);
                        match w.acc.find(&w.pattern) {
                            Some(idx) => {
                                let end = idx + w.pattern.len();
                                // 发送失败 = 调用方已放弃（超时取消 / 句柄丢弃）；
                                // 无论成败，命中即完成登记，都不能留在循环里。
                                let _ = w
                                    .result
                                    .send(WaitMatch { text_upto: w.acc[..end].to_string() });
                                false
                            }
                            None => {
                                w.trim();
                                true
                            }
                        }
                    });
                }
                // 输出读取器吃同一份清洗文本（read / read_until 的捕获源）。
                // 边沿触发通知：累计从空转非空时发一个信号，消费方阻塞等通知
                // 而不是轮询。洪水下 trim 保尾（与 ActiveWaiter 同级策略）。
                if !text.is_empty() && !readers.is_empty() {
                    for r in readers.iter_mut() {
                        let was_empty = r.acc.is_empty();
                        r.acc.push_str(&text);
                        r.trim();
                        if was_empty {
                            let _ = r.signal.send(());
                        }
                    }
                }
            }
        }

        // 退出检测。只认第一次：真实 PTY 的 `poll()` 缓存状态后会每轮重复返回。
        let exited = if exit_reported { None } else { ch.poll() };
        // 会话结束时把 RX/TX 计数附在状态文本后：状态栏只有一行位置，而这些
        // 数字（串口通道才有）在通道还活着的时候才问得到。
        let exited = exited.map(|st| match ch.status_note() {
            Some(note) => format!("{st} ({note})"),
            None => st,
        });
        if let Some(st) = &exited {
            last_exit = Some(st.clone());
            exit_reported = true;
            // 同一个字符串落到句柄能看到的那一格：线程结束之后，
            // 「已经结束、因为退出码 7」这句话只能从这里问出来。
            if let Ok(mut cell) = exit_note.lock() {
                *cell = Some(st.clone());
            }
        }
        if need_snapshot {
            pending = true;
        }
        // 一槽邮箱：正常运行期间槽里还有没被取走的一份就先不生成（合并中间
        // 版本）。**退出已报告后不再合并**：线程可能就在这一轮末尾 break
        // （`ch.finished() && !had_bytes`），被推迟的那份会永远留在
        // `pending` 里；而 `Event::Finished` 一发出，无头消费者（`desk
        // --headless --exec`）就停，拿到的是旧空屏 —— 曾实测 120 次
        // 里 2 次 stdout 为空。强制覆盖槽，邮箱里始终是到本轮为止的最终屏。
        if pending && (mailbox.is_empty() || exit_reported) {
            version += 1;
            mailbox.publish(vt.snapshot(version));
            pending = false;
        }
        // 先落最终屏、再报退出：顺序反了就仍有「事件已到、屏未到」的窗口。
        if let Some(st) = exited {
            let _ = ev_tx.send(Event::Finished(st));
        }
        // 请求看到的是本轮最终状态；不递增 version（不产生新事实）。
        for r in requested {
            let _ = ev_tx.send(Event::Screen(vt.slice_view(version, r)));
        }
        // P：预览请求同样看到本轮最终状态，落进独立的 `PreviewScreen` 事件
        // （ADR-0028）。事件再落到句柄的 `preview_screen` 槽，与 `Screen` 槽
        // 物理隔离，一帧内两份窗口互不覆盖。
        for r in preview_requested {
            let _ = ev_tx.send(Event::PreviewScreen(vt.slice_view(version, r)));
        }

        if ch.finished() && !had_bytes {
            break;
        }

        // R7：按「有没有活动」分两档等待。
        // 有活动 → 短等一档，输出往往成串；完全空闲 → 阻塞在下一条命令上，
        // 这样每会话从「每 2ms 醒一次、每 2ms 一次 read 系统调用」降到
        // 「每 20ms 一次」，而有按键时因为 `recv_timeout` 会被立刻唤醒，
        // 输入延迟不受影响（这是不能用 `sleep(IDLE)` 的原因）。
        if active {
            std::thread::sleep(ACTIVE_WAIT);
        } else {
            match cmd_rx.recv_timeout(IDLE_WAIT) {
                Ok(cmd) => queued = Some(cmd),
                Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
                Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => break,
            }
        }
    }
    // 会话结束（进程退出 / 通道关闭 / 句柄被 drop）也要收尾：日志文件里得有一行
    // 结束标记，否则「日志停在半行」到底是崩溃还是正常结束就分不出来。
    if let Some(mut l) = log.take() {
        l.finish();
    }
}

/// 一条在等的 `wait`：模式 + 自登记以来的清洗文本累积 + 结果应答端。
struct ActiveWaiter {
    id: u64,
    pattern: String,
    acc: String,
    result: Sender<WaitMatch>,
}

impl ActiveWaiter {
    /// 洪水下限制累积长度：子串匹配只需要「模式长度」的历史，再多就是
    /// 白占内存。上限按字符数算（String 按字节切会切进 UTF-8 中间）。
    fn trim(&mut self) {
        let keep = self.pattern.chars().count().max(4096);
        let len = self.acc.chars().count();
        if len > 4 * keep {
            self.acc = self.acc.chars().skip(len - keep).collect();
        }
    }
}

/// 一个在收的输出读取器：自登记以来的清洗文本累计 + 到达通知。
struct ActiveReader {
    id: u64,
    acc: String,
    /// 边沿触发的数据到达通知（累计从空转非空时发）。
    signal: Sender<()>,
}

impl ActiveReader {
    /// 与 [`ActiveWaiter::trim`] 同级的保尾策略：洪水下限长，丢最旧的。
    fn trim(&mut self) {
        let len = self.acc.chars().count();
        if len > 4 * 4096 {
            self.acc = self.acc.chars().skip(len - 4096).collect();
        }
    }
}

/// [`Session`] 的本地会话实现：包着一个受监管的通道线程。
///
/// 协议无关——PTY 现在承载本地 shell 与系统 `ssh`/`telnet`/`nc`/`cu`，
/// 将来原生 SSH 经 `spawn_with_channel` 注入后，这个实现一行不用改。
/// 这是 `TermChannel` 接缝（R13-d2）的自然延伸：通道对协议一无所知，
/// 会话抽象对协议更一无所知（ADR-0019 §B.14）。
pub struct SupervisedSession {
    handle: SupervisorHandle,
    /// `Session::resize` 是 `&self`，去重在这层做（handle 的去重要 `&mut`）。
    last_size: Mutex<(usize, usize)>,
}

impl SupervisedSession {
    pub fn new(handle: SupervisorHandle) -> Self {
        Self {
            handle,
            last_size: Mutex::new((0, 0)),
        }
    }

    pub fn handle(&self) -> &SupervisorHandle {
        &self.handle
    }
}

impl Session for SupervisedSession {
    fn id(&self) -> SessionId {
        self.handle.id()
    }

    fn write(&self, bytes: &[u8]) {
        self.handle.write(bytes);
    }

    fn resize(&self, cols: usize, rows: usize) {
        let mut last = self.last_size.lock().expect("last_size 锁");
        if *last != (cols, rows) {
            *last = (cols, rows);
            self.handle.send_resize(cols, rows);
        }
    }

    fn screen(&self) -> Option<GridSnapshot> {
        self.handle.snapshot_once()
    }

    fn state(&self) -> SessionState {
        self.handle
            .query_state()
            .unwrap_or(SessionState::Finished(None))
    }

    fn facets(&self) -> Vec<FacetKind> {
        // 本地会话没有可发现的附加能力。文件传输 / 端口转发是 SSH 之上的
        // facet，由它们自己的 `Session` 实现报告——控制面不做协议假设。
        Vec::new()
    }

    fn wait_register(&self, pattern: &str) -> Result<Box<dyn WaitRegistration>, WaitError> {
        self.handle
            .register_waiter(pattern)
            .map(|p| Box::new(p) as Box<dyn WaitRegistration>)
    }

    fn reader_register(&self) -> Result<Box<dyn OutputReader>, WaitError> {
        self.handle
            .register_reader()
            .map(|r| Box::new(r) as Box<dyn OutputReader>)
    }

    fn poll(&mut self) -> bool {
        self.handle.poll()
    }

    fn snapshot(&self) -> Option<GridSnapshot> {
        // 与 `screen()` 的关键区别：不发命令、不等应答，读的是 `poll()` 已经
        // 收下的那一帧。界面每帧都调它，走一次 2 秒超时的往返是不可接受的。
        Some(self.handle.snapshot.clone())
    }

    fn status(&self) -> SessionStatus {
        self.handle.status.clone()
    }

    fn request_range(&self, range: ViewRange) {
        self.handle.request_range(range);
    }

    /// P：预览请求走独立通道（ADR-0028）。
    fn request_preview_range(&self, range: ViewRange) {
        self.handle.request_preview_range(range);
    }

    fn screen_window(&self) -> Option<term_core::grid::ScreenImage> {
        self.handle.screen.clone()
    }

    /// P：预览窗口从独立的 `preview_screen` 槽取（ADR-0028），与 `screen` 互不覆盖。
    fn preview_window(&self) -> Option<term_core::grid::ScreenImage> {
        self.handle.preview_screen.clone()
    }

    fn set_wakeup(&self, wake: Arc<dyn Fn() + Send + Sync>) {
        self.handle.set_wakeup(wake);
    }

    fn clear(&self) {
        self.handle.clear();
    }

    fn clear_scrollback(&self) {
        self.handle.clear_scrollback();
    }

    fn clear_all(&self) {
        self.handle.clear_all();
    }

    fn log_status(&self) -> crate::log::LogStatus {
        self.handle.log.clone()
    }

    fn start_log(&self, dir: PathBuf, session: &str, opts: &term_core::log_opts::LogOptions) {
        // trait 侧借一份配置文件，命令侧要 owned（跨线程）——clone 是这一层
        // 唯一的选择，代价是五个小字段，与本来的 `session.to_string()` 同级。
        self.handle.start_log(dir, session, opts.clone());
    }

    fn start_log_with_host(
        &self,
        dir: PathBuf,
        session: &str,
        host: &str,
        opts: &term_core::log_opts::LogOptions,
    ) {
        self.handle
            .start_log_with_host(dir, session, host, opts.clone());
    }

    fn stop_log(&self) {
        self.handle.stop_log();
    }

    fn pause_log(&self) {
        self.handle.pause_log();
    }

    fn resume_log(&self) {
        self.handle.resume_log();
    }

    fn close(&mut self) {
        self.handle.close();
    }
}

/// 当前 epoch 秒。**只有副作用层可以读钟**（I3）：`term-core` 拿到的是这个值，
/// 不是 `SystemTime` 本身。读不到时返回 0——时间戳列会显示 `[00:00:00]`，
/// 这比让整条会话不渲染要好。
fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// `Finished` 之后把监管者的收尾快照排空。
///
/// `!status.alive` 只说明进程退出了，**不说明最后一屏已经到齐**：监管者可能
/// 还有一槽快照要发（`run()` 的退出收尾），也可能输出字节在退出检测之后的
/// 一轮才被 drain。无头消费者（`desk --headless --exec`）
/// 看到 `Finished` 就停的话会打印旧空屏——实测 120 次里 2 次 stdout 为空。
/// 排空到连续三轮没有新事实为止，有界（默认 100ms），不会把命令超时拖长。
pub fn drain_final_frame(handle: &mut SupervisorHandle, budget: Duration) {
    let deadline = Instant::now() + budget;
    let mut quiet = 0;
    loop {
        let changed = handle.poll();
        quiet = if changed { 0 } else { quiet + 1 };
        if quiet >= 3 || Instant::now() >= deadline {
            break;
        }
        std::thread::sleep(Duration::from_millis(2));
    }
}

/// `--headless --exec '<cmd>'`：跑完一条命令，把最终网格文本打到 stdout。
/// 这是 S2 的端到端验收入口，不需要窗口系统。
/// 无头跑一条命令：返回最终网格文本与退出码。
///
/// 退出码是**必须**的：`desk --headless --exec` 要给脚本透传它。上一版 CLI 的
/// `local exec` 就是这么用的（`verify` 第 8b 项曾断言 `exit 7` 原样透传），
/// 删掉 CLI 之后这条能力不能跟着消失——否则「跑一条命令」对 CI 就只剩
/// 「看输出」，判不了成败。
///
/// `None` 只出现在一种情况：命令没在超时内退出（`top` 之类），此刻它没有
/// 退出码可言。调用方应当把这个 `None` 当「没拿到」而不是「成功」。
///
/// 解析退出码的语法只有一份（[`crate::pty::exit_code_of`]），这里不重复。
pub fn run_headless(command: &str, cols: usize, rows: usize) -> Result<(String, Option<i32>), SpawnError> {
    let mut h = spawn_exec(command, cols, rows)?;
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        h.poll();
        if !h.status.alive || Instant::now() >= deadline {
            break;
        }
        std::thread::sleep(Duration::from_millis(4));
    }
    drain_final_frame(&mut h, Duration::from_millis(100));
    let code = h
        .status
        .exit_status
        .as_deref()
        .and_then(crate::pty::exit_code_of);
    Ok((h.snapshot.screen_text(), code))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::channel::TermChannel;
    use std::sync::atomic::{AtomicBool, AtomicUsize};

    /// 不起真实进程的确定性通道，用来端到端验证「意图上行 → 事实下行」。
    struct MockChannel {
        pending: Vec<u8>,
    }

    impl TermChannel for MockChannel {
        fn drain(&mut self) -> Vec<u8> {
            std::mem::take(&mut self.pending)
        }
        fn write(&mut self, _bytes: &[u8]) {}
        fn resize(&mut self, _cols: usize, _rows: usize) {}
        fn poll(&mut self) -> Option<String> {
            None
        }
        fn finished(&self) -> bool {
            false
        }
        fn close(&mut self) {}
        fn description(&self) -> &str {
            "mock"
        }
    }

    /// 可编程通道：能观察「上行」（写入 / 尺寸 / 关闭），也能模拟退出与 EOF。
    ///
    /// 与 `MockChannel` 的分工：那个是「一次性字节进、什么都不记」，
    /// 这个用来测**上行路径**与**生命周期**——这两块以前完全没有覆盖。
    struct ScriptedChannel {
        pending: Arc<Mutex<Vec<u8>>>,
        writes: Arc<Mutex<Vec<Vec<u8>>>>,
        sizes: Arc<Mutex<Vec<(usize, usize)>>>,
        closed: Arc<AtomicUsize>,
        finished: Arc<AtomicBool>,
        exit: Arc<Mutex<Option<String>>>,
    }

    impl ScriptedChannel {
        fn new() -> (Self, ScriptedProbe) {
            let probe = ScriptedProbe {
                pending: Arc::new(Mutex::new(Vec::new())),
                writes: Arc::new(Mutex::new(Vec::new())),
                sizes: Arc::new(Mutex::new(Vec::new())),
                closed: Arc::new(AtomicUsize::new(0)),
                finished: Arc::new(AtomicBool::new(false)),
                exit: Arc::new(Mutex::new(None)),
            };
            let ch = Self {
                pending: Arc::clone(&probe.pending),
                writes: Arc::clone(&probe.writes),
                sizes: Arc::clone(&probe.sizes),
                closed: Arc::clone(&probe.closed),
                finished: Arc::clone(&probe.finished),
                exit: Arc::clone(&probe.exit),
            };
            (ch, probe)
        }
    }

    /// 测试侧的操作柄：可以往里塞输出、读走了什么、模拟进程退出。
    struct ScriptedProbe {
        pending: Arc<Mutex<Vec<u8>>>,
        writes: Arc<Mutex<Vec<Vec<u8>>>>,
        sizes: Arc<Mutex<Vec<(usize, usize)>>>,
        closed: Arc<AtomicUsize>,
        finished: Arc<AtomicBool>,
        exit: Arc<Mutex<Option<String>>>,
    }

    impl ScriptedProbe {
        fn feed(&self, bytes: &[u8]) {
            self.pending.lock().expect("pending 锁").extend_from_slice(bytes);
        }
        fn writes(&self) -> Vec<Vec<u8>> {
            self.writes.lock().expect("writes 锁").clone()
        }
        fn sizes(&self) -> Vec<(usize, usize)> {
            self.sizes.lock().expect("sizes 锁").clone()
        }
        fn exit_with(&self, status: &str) {
            *self.exit.lock().expect("exit 锁") = Some(status.to_string());
        }
        /// 只给 EOF，不走 `close()`：模拟「进程退出但没人调 Close」的正常收尾，
        /// 监管者会在这一轮的 `ch.finished() && bytes.is_empty()` 处 break。
        fn eof(&self) {
            self.finished.store(true, Ordering::SeqCst);
        }
    }

    impl TermChannel for ScriptedChannel {
        fn drain(&mut self) -> Vec<u8> {
            std::mem::take(&mut *self.pending.lock().expect("pending 锁"))
        }
        fn write(&mut self, bytes: &[u8]) {
            self.writes.lock().expect("writes 锁").push(bytes.to_vec());
        }
        fn resize(&mut self, cols: usize, rows: usize) {
            self.sizes.lock().expect("sizes 锁").push((cols, rows));
        }
        fn poll(&mut self) -> Option<String> {
            self.exit.lock().expect("exit 锁").take()
        }
        fn finished(&self) -> bool {
            self.finished.load(Ordering::SeqCst)
        }
        fn close(&mut self) {
            self.closed.fetch_add(1, Ordering::SeqCst);
            self.finished.store(true, Ordering::SeqCst);
        }
        fn description(&self) -> &str {
            "scripted"
        }
    }

    fn start_scripted() -> (SupervisorHandle, ScriptedProbe) {
        let (ch, probe) = ScriptedChannel::new();
        let h = start(SessionConfig::new(Protocol::Shell), 20, 3, Box::new(ch));
        h.set_wakeup(Arc::new(|| {}));
        (h, probe)
    }

    fn start_mock(bytes: &[u8]) -> SupervisorHandle {
        let ch = MockChannel {
            pending: bytes.to_vec(),
        };
        start(
            SessionConfig::new(Protocol::Shell),
            20,
            3,
            Box::new(ch),
        )
    }

    #[test]
    fn a_panicking_session_thread_is_isolated_and_reported() {
        // L1（O 设计稿 §3b① / 验收 B1）：**一个会话的字节处理炸了，不能带走进程，
        // 也不能带走别的会话**。这条测试同时钉住两件事——
        //   1. 崩溃不是静默的：`exit_note` 报出可读原因，界面显示「内部错误」
        //      而不是莫名其妙的掉线；
        //   2. 邻居照常在跑：隔离的证据是隔壁没受影响，不是崩溃那条报了错。
        struct Bombs;

        impl TermChannel for Bombs {
            fn drain(&mut self) -> Vec<u8> {
                panic!("channel exploded")
            }
            fn write(&mut self, _bytes: &[u8]) {}
            fn resize(&mut self, _cols: usize, _rows: usize) {}
            fn poll(&mut self) -> Option<String> {
                None
            }
            fn finished(&self) -> bool {
                false
            }
            fn close(&mut self) {}
            fn description(&self) -> &str {
                "bombs"
            }
        }

        let bad = start(SessionConfig::new(Protocol::Shell), 20, 3, Box::new(Bombs));
        bad.set_wakeup(Arc::new(|| {}));
        let (mut good, probe) = start_scripted();
        probe.feed(b"neighbour alive\r\n");

        let deadline = Instant::now() + Duration::from_secs(2);
        loop {
            if let Some(SessionState::Finished(note)) = bad.query_state() {
                let note = note.expect("崩溃会话必须留下退出原因");
                assert!(note.contains("会话内部错误"), "退出原因说不清是什么错：{note}");
                assert!(note.contains("channel exploded"), "退出原因丢掉了 panic 文本：{note}");
                break;
            }
            assert!(Instant::now() < deadline, "崩溃会话一直没有留下退出原因");
            std::thread::sleep(Duration::from_millis(2));
        }

        wait_until(&mut good, |h| {
            h.snapshot.screen_text().contains("neighbour alive")
        });
    }

    #[test]
    fn spawn_with_channel_supervises_a_channel_it_did_not_open() {
        // R13-d2 的接缝：原生 SSH 的通道由应用层建好递进来。要验的是两件事——
        // 外部塞进去的字节能变成快照，按键能原样走到通道上（而不是被丢掉）。
        let (ch, probe) = ScriptedChannel::new();
        let mut h = spawn_with_channel(SessionConfig::new(Protocol::Ssh), 20, 3, Box::new(ch));
        h.set_wakeup(Arc::new(|| {}));

        probe.feed(b"remote banner\r\n");
        wait_until(&mut h, |h| h.snapshot.screen_text().contains("remote banner"));

        h.write(b"ls\r");
        let deadline = Instant::now() + Duration::from_secs(2);
        while probe.writes().is_empty() {
            assert!(Instant::now() < deadline, "按键没有到达通道");
            std::thread::sleep(Duration::from_millis(2));
        }
        assert_eq!(probe.writes(), vec![b"ls\r".to_vec()]);
    }

    /// 轮询直到条件成立；超时即失败，避免测试挂死。
    fn wait_until(h: &mut SupervisorHandle, mut done: impl FnMut(&SupervisorHandle) -> bool) {
        let deadline = Instant::now() + Duration::from_secs(2);
        loop {
            h.poll();
            if done(h) {
                return;
            }
            assert!(Instant::now() < deadline, "等待 Supervisor 响应超时");
            std::thread::sleep(Duration::from_millis(2));
        }
    }

    /// R6：可被测试线程继续追加字节的通道，用来模拟「输出一直在来」。
    /// `drains` 记录 `drain()` 被调了几次——它就是「监督者多久醒一次」的代理指标。
    struct SharedChannel {
        pending: Arc<Mutex<Vec<u8>>>,
        drains: Arc<AtomicUsize>,
        /// 写入即回显（模拟 tty 的 echo），用来验证按键路径没有卡在空闲等待上。
        echo: bool,
    }

    impl TermChannel for SharedChannel {
        fn drain(&mut self) -> Vec<u8> {
            self.drains.fetch_add(1, Ordering::SeqCst);
            let mut g = self.pending.lock().expect("pending 锁");
            std::mem::take(&mut *g)
        }
        fn write(&mut self, bytes: &[u8]) {
            if self.echo {
                self.pending.lock().expect("pending 锁").extend_from_slice(bytes);
            }
        }
        fn resize(&mut self, _cols: usize, _rows: usize) {}
        fn poll(&mut self) -> Option<String> {
            None
        }
        fn finished(&self) -> bool {
            false
        }
        fn close(&mut self) {}
        fn description(&self) -> &str {
            "shared"
        }
    }

    /// 空闲 200ms 内 `drain()` 的增量。2ms 固定 sleep 时约 100 次；
    /// 20ms 退避后约 10 次。取 40 作阈值，两边都留足余量。
    fn drains_while_idle(drains: &Arc<AtomicUsize>) -> usize {
        let before = drains.load(Ordering::SeqCst);
        std::thread::sleep(Duration::from_millis(200));
        drains.load(Ordering::SeqCst) - before
    }

    fn wait_for_wakes(wakes: &Arc<AtomicUsize>, want: usize) {
        let deadline = Instant::now() + Duration::from_secs(2);
        while wakes.load(Ordering::SeqCst) < want {
            assert!(Instant::now() < deadline, "等待唤醒超时");
            std::thread::sleep(Duration::from_millis(2));
        }
    }

    #[test]
    fn new_snapshot_wakes_the_ui() {
        // 这个 bug 的形状是「输出到了但界面不知道」：supervisor 只 send 事件，
        // 没人叫醒界面。所以这里断言的是**唤醒回调被调用**，不是「队列里有东西」。
        let pending = Arc::new(Mutex::new(b"hello\r\n".to_vec()));
        let wakes = Arc::new(AtomicUsize::new(0));
        let ch = SharedChannel {
            pending: Arc::clone(&pending),
            drains: Arc::new(AtomicUsize::new(0)),
            echo: false,
        };
        let mut h = start(SessionConfig::new(Protocol::Shell), 20, 3, Box::new(ch));
        let w = Arc::clone(&wakes);
        h.set_wakeup(Arc::new(move || {
            w.fetch_add(1, Ordering::SeqCst);
        }));

        wait_for_wakes(&wakes, 1);
        h.poll();
        assert!(h.snapshot.version >= 1);
        assert!(h.snapshot.screen_text().contains("hello"));
    }

    #[test]
    fn snapshot_is_coalesced_until_the_consumer_takes_it() {
        // 消费者每帧取一次，生产者每 2ms 一轮：槽里没被取走就不该再造新快照，
        // 否则洪水输出时几乎每次分配都直接进垃圾。
        let pending = Arc::new(Mutex::new(b"one\r\n".to_vec()));
        let wakes = Arc::new(AtomicUsize::new(0));
        let ch = SharedChannel {
            pending: Arc::clone(&pending),
            drains: Arc::new(AtomicUsize::new(0)),
            echo: false,
        };
        let mut h = start(SessionConfig::new(Protocol::Shell), 20, 3, Box::new(ch));
        let w = Arc::clone(&wakes);
        h.set_wakeup(Arc::new(move || {
            w.fetch_add(1, Ordering::SeqCst);
        }));

        // 第一份进槽后我们**故意不 poll**，模拟「界面还没到下一帧」。
        wait_for_wakes(&wakes, 1);
        for _ in 0..5 {
            pending.lock().expect("pending 锁").extend_from_slice(b"more\r\n");
            std::thread::sleep(Duration::from_millis(10));
        }
        assert_eq!(
            wakes.load(Ordering::SeqCst),
            1,
            "槽里那份还没被取走时不该继续生成新快照"
        );

        // 取走后必须立刻补上最新一份，不能停在旧画面上。
        h.poll();
        wait_for_wakes(&wakes, 2);
        h.poll();
        assert!(h.snapshot.screen_text().contains("more"), "补上的应是最新内容");
    }

    /// 轮询直到条件成立；超时即失败并给出原因（避免测试挂死）。
    fn wait_for(mut done: impl FnMut() -> bool, msg: &str) {
        let deadline = Instant::now() + Duration::from_secs(2);
        while !done() {
            assert!(Instant::now() < deadline, "{msg}");
            std::thread::sleep(Duration::from_millis(2));
        }
    }

    #[test]
    fn writes_reach_the_channel() {
        // 上行路径：按键 → Command::Write → TermChannel::write。
        // 这条以前完全没有覆盖（`MockChannel::write` 是空实现）。
        let (h, probe) = start_scripted();
        h.write(b"ls\r");
        wait_for(
            || probe.writes().iter().any(|w| w == b"ls\r"),
            "写入没有到达通道",
        );
    }

    #[test]
    fn resize_is_deduplicated_before_reaching_the_channel() {
        // UI 每帧都会调 `resize`；同尺寸重复下发会让 PTY 反复 ioctl。
        let (mut h, probe) = start_scripted();
        h.resize(20, 3); // 与启动时相同
        std::thread::sleep(Duration::from_millis(30));
        assert!(probe.sizes().is_empty(), "同尺寸不该下发 resize");

        h.resize(100, 30);
        wait_for(|| probe.sizes().len() == 1, "新尺寸没有下发");
        h.resize(100, 30);
        std::thread::sleep(Duration::from_millis(30));
        assert_eq!(probe.sizes(), vec![(100, 30)], "重复尺寸只该下发一次");
    }

    #[test]
    fn close_stops_the_supervisor() {
        let (mut h, probe) = start_scripted();
        h.close();
        wait_for(|| probe.closed.load(Ordering::SeqCst) >= 1, "close 没有传到通道");
        assert!(!h.status.alive);
        assert!(!h.status.connected);
    }

    /// 只用来验「结束状态文本带上通道的补充说明」的通道（串口计数走这条路）。
    struct NotedChannel;

    impl TermChannel for NotedChannel {
        fn drain(&mut self) -> Vec<u8> {
            Vec::new()
        }
        fn write(&mut self, _bytes: &[u8]) {}
        fn resize(&mut self, _cols: usize, _rows: usize) {}
        fn poll(&mut self) -> Option<String> {
            Some("serial disconnected: /dev/x".to_string())
        }
        fn finished(&self) -> bool {
            true
        }
        fn close(&mut self) {}
        fn description(&self) -> &str {
            "noted"
        }
        fn status_note(&self) -> Option<String> {
            Some("rx 1.0 KB / tx 4 B, dropped 0".to_string())
        }
    }

    #[test]
    fn the_end_status_carries_the_channels_byte_counters() {
        // 状态栏只有一行位置：串口的 RX/TX 计数在会话结束时附在这句话后面。
        let mut h = start(
            SessionConfig::new(Protocol::Serial),
            20,
            3,
            Box::new(NotedChannel),
        );
        wait_until(&mut h, |h| h.status.exit_status.is_some());
        assert_eq!(
            h.status.exit_status.as_deref(),
            Some("serial disconnected: /dev/x (rx 1.0 KB / tx 4 B, dropped 0)")
        );
    }

    #[test]
    fn natural_exit_reaches_the_status_bar() {
        // 子进程退出时 `poll()` 返回状态文本，轮询要把它变成 UI 可读的字段。
        let (mut h, probe) = start_scripted();
        probe.exit_with("exited 0");
        wait_until(&mut h, |h| h.status.exit_status.is_some());
        assert_eq!(h.status.exit_status.as_deref(), Some("exited 0"));
        assert!(!h.status.alive);
    }

    #[test]
    fn exit_delivers_the_final_frame_even_with_a_full_mailbox_slot() {
        // 复现无头执行（`desk --headless --exec`）偶发空输出（实测 120 次里 2 次）：
        // 监管者检测到进程退出就发 `Event::Finished`，但一槽邮箱里还压着
        // 一份没人取走的旧快照，最终屏被留在 `pending` 里；同一轮末尾线程
        // break，那份屏永远不发。无头消费者看到 Finished 就停，只拿到旧屏。
        let (mut h, probe) = start_scripted();
        probe.feed(b"OLD\r\n");
        wait_for(|| !h.mailbox.is_empty(), "旧快照没有进槽");
        // 新输出 + 退出 + EOF 挤在相邻几轮；EOF 让监管者随后立刻 break。
        probe.feed(b"FINAL\r\n");
        probe.exit_with("exited 0");
        probe.eof();
        // 一次 poll 同时拿快照与事件——正是无头消费者「看到退出就停」的形状。
        wait_until(&mut h, |h| h.status.exit_status.is_some());
        assert!(
            h.snapshot.screen_text().contains("FINAL"),
            "退出时邮箱里必须已是最终屏，实得 {:?}",
            h.snapshot.screen_text()
        );
    }

    #[test]
    fn flood_output_keeps_the_final_state() {
        // 大输出不能丢尾巴：一槽邮箱会合并中间版本，但最后一份必须是最终状态。
        let (mut h, probe) = start_scripted();
        let mut bytes = Vec::new();
        for i in 0..5_000 {
            bytes.extend_from_slice(format!("L{i}\r\n").as_bytes());
        }
        probe.feed(&bytes);
        wait_until(&mut h, |h| h.snapshot.screen_text().contains("L4999"));
    }

    #[test]
    fn clear_empties_the_grid_and_bumps_the_version() {
        let (mut h, probe) = start_scripted();
        probe.feed(b"hello\r\n");
        wait_until(&mut h, |h| h.snapshot.screen_text().contains("hello"));
        let before = h.snapshot.version;

        h.clear();
        wait_until(&mut h, |h| {
            h.snapshot.version > before && h.snapshot.screen_text().trim().is_empty()
        });
    }

    #[test]
    fn poll_is_quiet_when_nothing_happened() {
        // `poll()` 的返回值决定要不要重绘；空转时必须是 false，否则又回到
        // 「每帧无脑重绘」的老路。
        let (mut h, _probe) = start_scripted();
        std::thread::sleep(Duration::from_millis(40));
        h.poll(); // 先吃掉可能残留的一份
        assert!(!h.poll(), "没有新事件时 poll 应返回 false");
    }

    #[test]
    fn eof_stops_accepting_input() {
        // 通道报 finished 且没有残留字节时，监督者线程该退出——之后写入不再生效。
        let (h, probe) = start_scripted();
        probe.finished.store(true, Ordering::SeqCst);
        std::thread::sleep(Duration::from_millis(60));
        h.write(b"x");
        std::thread::sleep(Duration::from_millis(60));
        assert!(probe.writes().is_empty(), "线程退出后写入不该再生效");
    }

    #[test]
    fn idle_loop_stops_hammering_the_channel() {
        // R7：空闲时不再每 2ms 醒一次去 drain。阈值取 40：2ms 档在 200ms 内
        // 会 drain ~100 次，20ms 档约 10 次，两边余量都够。
        let pending = Arc::new(Mutex::new(b"hi\r\n".to_vec()));
        let drains = Arc::new(AtomicUsize::new(0));
        let ch = SharedChannel {
            pending: Arc::clone(&pending),
            drains: Arc::clone(&drains),
            echo: false,
        };
        let mut h = start(SessionConfig::new(Protocol::Shell), 20, 3, Box::new(ch));
        h.set_wakeup(Arc::new(|| {}));
        wait_until(&mut h, |h| h.snapshot.version >= 1);

        let n = drains_while_idle(&drains);
        assert!(
            n < 40,
            "空闲 200ms 内 drain 了 {n} 次，退避没生效（2ms 档约 100 次）"
        );
    }

    #[test]
    fn keystroke_is_handled_while_the_loop_is_idle() {
        // 退避必须“可被唤醒”，否则打字会掉进等待窗口里。这条测的是
        // `recv_timeout` 取到的那条命令（`queued`）没有被丢掉——这是改动
        // 引入的新风险。真正的延迟保证来自 `recv_timeout` 而不是 `sleep`，
        // 上一条测试从 CPU 侧守着同一件事。
        let pending = Arc::new(Mutex::new(Vec::new()));
        let ch = SharedChannel {
            pending: Arc::clone(&pending),
            drains: Arc::new(AtomicUsize::new(0)),
            echo: true,
        };
        let mut h = start(SessionConfig::new(Protocol::Shell), 20, 3, Box::new(ch));
        h.set_wakeup(Arc::new(|| {}));

        // 先让它彻底进入空闲等待
        std::thread::sleep(Duration::from_millis(60));

        let started = Instant::now();
        h.write(b"z");
        loop {
            h.poll();
            if h.snapshot.screen_text().contains('z') {
                break;
            }
            assert!(
                started.elapsed() < Duration::from_millis(120),
                "空闲态下的按键没有被处理（命令丢了？）"
            );
            std::thread::sleep(Duration::from_millis(1));
        }
        assert!(
            started.elapsed() < Duration::from_millis(120),
            "按键到回显耗时过长"
        );
    }

    #[test]
    fn logging_captures_output_through_the_supervisor() {
        // 端到端：命令进队列 → 监管者线程建文件 → 字节落盘。
        // 单测 `log.rs` 只能证明写入端本身对；这条证明它真的接在会话循环上。
        let dir = std::env::temp_dir().join(format!("zterm-sup-log-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);

        let (mut h, probe) = start_scripted();
        // 显式 `%v`：这条判据关心的是「字节有没有真的落盘」，行首时间戳
        // 会随跑测试的墙上时间变化，别掺进来。
        let opts = term_core::log_opts::LogOptions {
            content_pattern: "%v".to_string(),
            ..Default::default()
        };
        h.start_log(dir.clone(), "test session", opts);
        wait_until(&mut h, |h| h.log.is_recording());
        let path = h.log.path.clone().expect("开始后应有路径");

        probe.feed(b"\x1b[32mhello\x1b[0m\r\n");
        wait_until(&mut h, |h| h.snapshot.screen_text().contains("hello"));

        h.stop_log();
        wait_until(&mut h, |h| !h.log.is_recording());
        // 停止后路径仍在：菜单「Open Log File」要能打开刚记的那一份。
        assert_eq!(h.log.path.as_deref(), Some(path.as_path()));

        let text = std::fs::read_to_string(&path).expect("读日志");
        assert!(text.contains("hello\n"), "{text}");
        assert!(!text.contains('\x1b'), "日志里不该有转义字符：{text:?}");
        assert!(text.contains("# session: test session"), "{text}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 把裸行区间包成 `ViewRange`：这些用例关心的是链路本身，所以只在边界处
    /// 构造一次；断言那边仍可与 `ScreenImage.range`（裸字段）逐值比对。
    fn window(start: i64, end: i64) -> ViewRange {
        ViewRange::new(
            term_core::coords::ViewRow::new(start),
            term_core::coords::ViewRow::new(end),
        )
    }

    #[test]
    fn request_range_flows_through_the_channel() {
        let mut h = start_mock(b"L1\r\nL2\r\nL3\r\nL4\r\nL5");
        wait_until(&mut h, |h| h.snapshot.version > 0);
        let expected_version = h.snapshot.version;

        h.request_range(window(-1, 2));
        wait_until(&mut h, |h| {
            h.screen.as_ref().is_some_and(|s| s.range == (-1..2))
        });
        let img = h.screen.as_ref().unwrap();
        assert_eq!(img.lines.len(), 3);
        assert_eq!(img.version, expected_version);
        assert!(!img.text().is_empty());
        assert!(h.snapshot.version == expected_version, "请求不应产生新事实");
    }

    #[test]
    fn preview_range_uses_a_separate_slot_from_the_real_view() {
        // P（ADR-0028）：真实视口与预览走物理隔离的两套事件 / 两个槽。
        // 同一 tick 内同时下发两段**不同**的请求，断言两者各自收对、互不覆盖——
        // 这正是方案 A / A′ 会闪白屏、方案 B 结构性排除的那一关。
        let mut h = start_mock(b"L1\r\nL2\r\nL3\r\nL4\r\nL5");
        wait_until(&mut h, |h| h.snapshot.version > 0);

        // 真实视口要 0..3，预览要 -2..1：两段不相交，错开最容易看出覆盖。
        h.request_range(window(0, 3));
        h.request_preview_range(window(-2, 1));
        wait_until(&mut h, |h| h.screen.is_some() && h.preview_screen.is_some());

        let real = h.screen.as_ref().expect("真实视口应收到");
        let preview = h.preview_screen.as_ref().expect("预览应收到");
        assert_eq!(real.range, (0..3), "真实视口不被预览事件覆盖");
        assert_eq!(preview.range, (-2..1), "预览不被真实视口事件覆盖");
        assert_ne!(real.range, preview.range, "两段必须落在各自槽里");
        // 预览也不产生新事实。
        assert_eq!(preview.version, h.snapshot.version);
    }

    #[test]
    fn preview_slot_is_independent_of_real_view_lifetime() {
        // P：先只发预览、不发真实请求，确认预览槽独立填充且 `screen` 仍空
        // （方案 A 的退化点：贴底时 `tab.screen` 为空，预览若复用它就会失效）。
        let mut h = start_mock(b"L1\r\nL2\r\nL3\r\nL4\r\nL5");
        wait_until(&mut h, |h| h.snapshot.version > 0);

        h.request_preview_range(window(-2, 1));
        wait_until(&mut h, |h| h.preview_screen.is_some());
        assert!(
            h.screen.is_none(),
            "只发预览时真实视口槽保持空，不抢预览的位"
        );
        assert_eq!(h.preview_screen.as_ref().unwrap().range, (-2..1));
    }

    #[test]
    fn repeated_range_requests_are_rebuildable() {
        let mut h = start_mock(b"L1\r\nL2\r\nL3\r\nL4\r\nL5");
        wait_until(&mut h, |h| h.snapshot.version > 0);

        h.request_range(window(-2, 2));
        wait_until(&mut h, |h| h.screen.is_some());
        let first = h.screen.clone().unwrap();

        h.screen = None; // 模拟投影被销毁
        h.request_range(window(-2, 2));
        wait_until(&mut h, |h| h.screen.is_some());
        let rebuilt = h.screen.clone().unwrap();
        assert_eq!(rebuilt, first, "I2：重建后的投影应逐项一致");
    }

    #[test]
    fn scrollback_window_is_reported_and_croppable() {
        // F1 路径：快照报告可回看行数 → 视口请求范围 → 裁剪出一屏 → 内容正确。
        let mut h = start_mock(b"L1\r\nL2\r\nL3\r\nL4\r\nL5");
        wait_until(&mut h, |h| h.snapshot.version > 0);
        let rows = h.snapshot.rows as i64;
        assert_eq!(h.snapshot.history, 2, "20x3 屏内 5 行内容应留 2 行历史");

        let range = -1..(rows - 1);
        h.request_range(window(range.start, range.end));
        wait_until(&mut h, |h| {
            h.screen.as_ref().is_some_and(|s| s.range == range)
        });
        let img = h.screen.as_ref().unwrap();
        assert_eq!(img.history, h.snapshot.history);

        let one_screen = img.crop(range).expect("请求窗口应能裁出正好一屏");
        assert_eq!(one_screen.lines.len() as i64, rows);
        let first = one_screen.text();
        let first_line = first.lines().next().unwrap_or("");
        assert_eq!(first_line, "L2", "上滚一行后首行应是 L2");
    }

    #[test]
    fn range_cost_is_proportional_to_window_not_history() {
        // 宪章 §11 的传输成本验证：构造大历史，只请求一屏窗口，
        // 返回的 Cell 数恒等于窗口大小，不随历史总量增长。
        let mut t = Terminal::new(80, 24, 1_000_000);
        let mut bytes = Vec::new();
        for i in 0..20_000 {
            bytes.extend_from_slice(format!("L{i}\r\n").as_bytes());
        }
        t.process(&bytes);
        assert!(t.scrollback.len() > 1_000, "需要大历史");

        let started = Instant::now();
        let img = t.slice_range(1, -24..0);
        let elapsed = started.elapsed();
        let cells: usize = img.lines.iter().map(|l| l.cells.len()).sum();
        assert_eq!(img.lines.len(), 24);
        assert_eq!(cells, 24 * 80);
        println!(
            "spike: scrollback={} 行, 窗口=24 行, 传输={} Cell, 切片耗时={:?}",
            t.scrollback.len(),
            cells,
            elapsed
        );
    }

    // ---- ADR-0019：wait 登记（先登记、后消费）与 Session 抽象 ----

    #[test]
    fn a_registered_wait_matches_output_that_arrives_after_registration() {
        // 核心保证：`register_waiter` 返回（拿到 ack）之后被 drain 的输出，
        // 保证进入匹配。顺序反了（先输出后登记）就会漏匹配——这正是
        // ADR-0019 §B.13 点名的最可能返工点。
        let (h, probe) = start_scripted();
        let w = h.register_waiter("hello").expect("登记");
        probe.feed(b"hello world\r\n");
        let m = Box::new(w).wait(Duration::from_secs(2)).expect("命中");
        // text_upto 的语义是「自登记以来、到匹配串**结尾**」——输出到
        // "hello world" 为止，但匹配串是 "hello"，所以截到它结尾。
        assert_eq!(m.text_upto, "hello");
    }

    #[test]
    fn a_wait_matches_across_chunks_and_reports_text_up_to_the_match() {
        let (h, probe) = start_scripted();
        let w = h.register_waiter("hello").expect("登记");
        probe.feed(b"say hel");
        std::thread::sleep(Duration::from_millis(30));
        probe.feed(b"lo now\r\n");
        let m = Box::new(w).wait(Duration::from_secs(2)).expect("命中");
        assert_eq!(m.text_upto, "say hello");
    }

    #[test]
    fn escape_sequences_do_not_block_a_match() {
        // 清洗语义与日志同源（同一个 vte 解析器）：`ESC[31m` 不挡 `Password:`。
        let (h, probe) = start_scripted();
        let w = h.register_waiter("Password:").expect("登记");
        probe.feed(b"\x1b[31mPassword:\x1b[0m ");
        let m = Box::new(w).wait(Duration::from_secs(2)).expect("命中");
        assert!(m.text_upto.ends_with("Password:"), "{:?}", m.text_upto);
    }

    #[test]
    fn a_timed_out_wait_cancels_its_registration() {
        let (h, probe) = start_scripted();
        let w = h.register_waiter("never").expect("登记");
        let err = Box::new(w).wait(Duration::from_millis(120)).unwrap_err();
        assert_eq!(err, WaitError::Timeout);
        // 超时已取消旧登记：取消生效后重新登记，同样的模式必须能命中——
        // 否则说明旧登记还赖在循环里吞输出。
        std::thread::sleep(Duration::from_millis(60));
        let w2 = h.register_waiter("never").expect("重新登记");
        probe.feed(b"never!\r\n");
        let m = Box::new(w2).wait(Duration::from_secs(2)).expect("重新登记后命中");
        assert_eq!(m.text_upto, "never");
    }

    #[test]
    fn an_empty_pattern_is_rejected_before_reaching_the_session() {
        let (h, _probe) = start_scripted();
        assert_eq!(h.register_waiter("").unwrap_err(), WaitError::EmptyPattern);
    }

    #[test]
    fn snapshot_once_round_trips_without_touching_the_ui_mailbox() {
        let (mut h, probe) = start_scripted();
        probe.feed(b"abc\r\n");
        wait_until(&mut h, |h| h.snapshot.screen_text().contains("abc"));

        let snap = h.snapshot_once().expect("直通快照");
        assert!(snap.screen_text().contains("abc"));
        // UI 的邮箱与 `poll()` 路径没有被状态查询偷走：再 poll 一次仍拿得到事实。
        assert!(h.poll() || h.snapshot.screen_text().contains("abc"));
    }

    #[test]
    fn query_state_reports_alive_then_the_cached_exit_status() {
        // ch.poll() 上报的退出文本既发给 UI（Event::Finished）也留给状态查询：
        // 两边都要看得到，不能谁把谁偷走。
        let (h, probe) = start_scripted();
        assert_eq!(h.query_state(), Some(SessionState::Alive));
        probe.exit_with("exited 0");
        let deadline = Instant::now() + Duration::from_secs(2);
        loop {
            if h.query_state() == Some(SessionState::Finished(Some("exited 0".into()))) {
                break;
            }
            assert!(Instant::now() < deadline, "状态未变成 Finished");
            std::thread::sleep(Duration::from_millis(4));
        }
    }

    #[test]
    fn session_write_reaches_the_channel_through_the_trait() {
        let (h, probe) = start_scripted();
        let s = SupervisedSession::new(h);
        s.write(b"ls\r");
        wait_for(
            || probe.writes().iter().any(|w| w == b"ls\r"),
            "Session::write 没有到达通道",
        );
        // 同尺寸重复 resize 只该下发一次（去重在 SupervisedSession 这层；
        // 这层不知道会话的启动尺寸，所以首次从 (0,0) 起算会下发一次）。
        s.resize(20, 3);
        wait_for(|| probe.sizes() == vec![(20, 3)], "首次 resize 没有下发");
        s.resize(20, 3);
        std::thread::sleep(Duration::from_millis(30));
        assert_eq!(probe.sizes(), vec![(20, 3)], "同尺寸不该重复下发");
        s.resize(100, 30);
        wait_for(
            || probe.sizes() == vec![(20, 3), (100, 30)],
            "新尺寸没有下发",
        );
    }

    #[test]
    fn session_capabilities_are_empty_for_a_local_pty() {
        // 协议无关原则的钉子：能力是会话自己报告的 facet，控制面不假设。
        let (h, _probe) = start_scripted();
        let s = SupervisedSession::new(h);
        assert!(s.facets().is_empty());
    }

    /// ADR-0020：删掉 CLI 之后，`--headless --exec` 必须接住它的退出码能力。
    /// 判据用 `exit_code_of` 解析过的数字，不在测试里另写一套语法。
    #[test]
    fn headless_reports_the_exit_code() {
        let (text, code) = run_headless("sh -c 'exit 7'", 80, 24).expect("无头会话应该能起来");
        assert!(text.is_empty() || text.trim().is_empty(), "不该有屏幕输出");
        assert_eq!(code, Some(7), "退出码没有透传");
    }

    // ---- W3：原始字节旁路（X/Y/Z Modem 这类字节级协议的入口） ----

    /// 旁路生效：字节整块进 sink，**屏幕一格都不动**（快照与版本都不变）。
    /// 屏幕冻结是设计——传输进度由界面自己画，这一条不钉住就会被当成 bug 改掉。
    #[test]
    fn raw_bypass_keeps_binary_bytes_out_of_the_vt() {
        let (mut h, probe) = start_scripted();
        probe.feed(b"banner\r\n");
        wait_until(&mut h, |h| h.snapshot.screen_text().contains("banner"));
        let frozen_version = h.snapshot.version;
        let frozen_text = h.snapshot.screen_text();

        let (sink, rx) = channel();
        h.attach_raw(sink);

        let payload = b"\x01\x02SOH\xff\xfeEOF\x18".to_vec();
        probe.feed(&payload);
        let got = rx.recv_timeout(Duration::from_secs(2)).expect("旁路应收到字节");
        assert_eq!(got, payload, "旁路必须原样收到整块字节");

        // 多等几轮：若字节漏进 VT，版本或文本必然变。
        std::thread::sleep(Duration::from_millis(60));
        h.poll();
        assert_eq!(h.snapshot.version, frozen_version, "raw 期间不该产生新快照");
        assert_eq!(h.snapshot.screen_text(), frozen_text, "raw 期间屏幕应冻结");
    }

    /// 撤销之后恢复正常：下一次 `drain()` 的字节重新进 VT。
    #[test]
    fn detach_raw_lets_bytes_reach_the_vt_again() {
        let (mut h, probe) = start_scripted();
        let (sink, rx) = channel();
        h.attach_raw(sink);

        probe.feed(b"hidden\r\n");
        // 先确认这一块真被旁路吃掉了，再撤销：否则「隐藏」可能还没被 drain。
        assert_eq!(
            rx.recv_timeout(Duration::from_secs(2)).expect("旁路应收到字节"),
            b"hidden\r\n"
        );
        h.poll();
        assert!(!h.snapshot.screen_text().contains("hidden"));

        h.detach_raw();
        probe.feed(b"visible\r\n");
        wait_until(&mut h, |h| h.snapshot.screen_text().contains("visible"));
    }

    /// 旁路期间的字节不进 `read` / `read_until` 的累计（`RegisterReader` 收不到），
    /// 撤销之后同一份读取器又能正常收——两条路径互不串味。
    #[test]
    fn raw_bytes_do_not_accumulate_in_the_output_reader() {
        let (h, probe) = start_scripted();
        let (sink, rx) = channel();
        h.attach_raw(sink);
        let mut reader = h.register_reader().expect("登记读取器");

        probe.feed(b"PROTO\x01\x02");
        assert_eq!(
            rx.recv_timeout(Duration::from_secs(2)).expect("旁路应收到字节"),
            b"PROTO\x01\x02"
        );
        assert_eq!(
            reader.collect(Duration::from_millis(150)).expect("取累计"),
            "",
            "raw 字节不该污染 read 的累计"
        );

        h.detach_raw();
        probe.feed(b"AFTER\r\n");
        let text = reader.collect(Duration::from_secs(2)).expect("取累计");
        assert!(text.contains("AFTER"), "撤销后读取器应恢复：{text:?}");
    }

    /// 旁路不是一次性开关：两次 attach/detach 循环都要完整生效。
    #[test]
    fn raw_bypass_survives_repeated_attach_detach_cycles() {
        let (mut h, probe) = start_scripted();
        for round in 0..2 {
            let (sink, rx) = channel();
            h.attach_raw(sink);
            let bin = format!("round{round}\x01\x02").into_bytes();
            probe.feed(&bin);
            assert_eq!(
                rx.recv_timeout(Duration::from_secs(2)).expect("旁路应收到字节"),
                bin,
                "第 {round} 轮旁路没生效"
            );

            h.detach_raw();
            let text = format!("round{round}-text\r\n");
            probe.feed(text.as_bytes());
            let needle = format!("round{round}-text");
            wait_until(&mut h, |h| h.snapshot.screen_text().contains(&needle));
        }
    }

    /// 接收端被 drop 时**自己退回 VT**：`mpsc::send` 只在 Receiver 没了时失败，
    /// 是永久性故障；留在 raw 模式等于这条会话永远收不到字节、屏幕永久冻结，
    /// 而能发 `DetachRaw` 的那一侧已经不在了。
    ///
    /// 判据用纯文本：退回生效后 VT 必须重新 `process`（快照里看得见）。
    /// 触发退回的那一块**有意丢弃**（二进制帧，重放只会刷屏），所以这里喂
    /// 两块：第一块负责触发退回并被丢，第二块验证 VT 真的活过来了。
    #[test]
    fn a_dropped_raw_sink_falls_back_to_the_vt() {
        let (mut h, probe) = start_scripted();
        let (sink, rx) = channel();
        h.attach_raw(sink);
        drop(rx);

        probe.feed(b"\x01\x02dropped\x03");
        // 没有「已退回」的回调可等：给几轮时间让退回在 drain 里发生。
        std::thread::sleep(Duration::from_millis(60));

        // 重试喂：启动慢的机器上第一块可能与退回同轮被丢，flaky 要避免。
        let deadline = Instant::now() + Duration::from_secs(2);
        loop {
            probe.feed(b"recovered\r\n");
            h.poll();
            if h.snapshot.screen_text().contains("recovered") {
                break;
            }
            assert!(Instant::now() < deadline, "接收端 drop 之后 VT 没有恢复");
            std::thread::sleep(Duration::from_millis(20));
        }
        assert!(
            !h.snapshot.screen_text().contains("dropped"),
            "触发退回的那块字节是有意丢弃的，不该被重放进 VT"
        );
    }

    // ---- 非破坏性 tee（hex 视图的输入源） ----

    /// 这一条与 `raw_bypass_keeps_binary_bytes_out_of_the_vt` 互为反面：
    /// tee **不是**改道，VT 照吃同一份字节、屏幕照常前进。
    /// 钉住它是因为「复制」与「改道」在实现上只差一个早退分支，
    /// 而两者对屏幕的影响正好相反——混淆一次就会让 hex 视图一开就冻屏。
    #[test]
    fn a_tap_sees_bytes_and_the_vt_still_advances() {
        let (mut h, probe) = start_scripted();
        let (sink, rx) = channel();
        h.attach_tap(sink);

        probe.feed(b"hello\r\n");
        assert_eq!(
            rx.recv_timeout(Duration::from_secs(2)).expect("tap 应收到字节"),
            b"hello\r\n".to_vec(),
            "tap 必须拿到与 VT 同一份字节"
        );
        // 同一条断言的两面：字节到了 tap，屏幕也真的推进了。
        wait_until(&mut h, |h| h.snapshot.screen_text().contains("hello"));
    }

    /// 摘掉 tee 之后 tap 不再收字节，而 VT 一轮都没停过。
    #[test]
    fn detach_tap_stops_the_tap_but_the_vt_keeps_running() {
        let (mut h, probe) = start_scripted();
        let (sink, rx) = channel();
        h.attach_tap(sink);
        probe.feed(b"before\r\n");
        assert_eq!(
            rx.recv_timeout(Duration::from_secs(2)).expect("tap 应收到字节"),
            b"before\r\n".to_vec()
        );

        h.detach_tap();
        probe.feed(b"after\r\n");
        // 屏幕照样前进——detach 摘的是拷贝，不是 VT。
        wait_until(&mut h, |h| h.snapshot.screen_text().contains("after"));
        // 再喂一块并给足轮次：若 tee 没摘干净，这里会额外收到东西。
        probe.feed(b"more\r\n");
        wait_until(&mut h, |h| h.snapshot.screen_text().contains("more"));
        assert!(
            rx.recv_timeout(Duration::from_millis(120)).is_err(),
            "摘掉之后 tap 不该再收到任何字节"
        );
    }

    /// UI 侧 rx 被 drop 时**不能**把会话带走：摘掉 tap，VT 继续跑。
    ///
    /// 与 raw 的处理刻意不同：raw 那里必须自己退回 VT（否则会话永久冻屏），
    /// 这里 VT 从来没停过，所以只需清空槽位。
    #[test]
    fn a_dropped_tap_sink_does_not_stop_the_session() {
        let (mut h, probe) = start_scripted();
        let (sink, rx) = channel();
        h.attach_tap(sink);
        drop(rx);

        probe.feed(b"still-alive\r\n");
        wait_until(&mut h, |h| h.snapshot.screen_text().contains("still-alive"));
        // 摘掉之后继续喂：线程还活着，还能出快照。
        probe.feed(b"and-well\r\n");
        wait_until(&mut h, |h| h.snapshot.screen_text().contains("and-well"));
    }

    /// raw 优先：改道期间 tee 收不到任何字节（走早退分支），撤销后恢复。
    ///
    /// X/Y/Z Modem 跑的时候屏幕本来就冻着，此时给 hex 视图喂二进制帧只会
    /// 让它显示一堆不属于终端对话的内容——所以「raw 赢」是设计。
    #[test]
    fn attach_raw_wins_over_the_tap() {
        let (mut h, probe) = start_scripted();
        let (tap_tx, tap_rx) = channel();
        h.attach_tap(tap_tx);
        let (raw_tx, raw_rx) = channel();
        h.attach_raw(raw_tx);

        probe.feed(b"\x01\x02FRAME\x03");
        assert_eq!(
            raw_rx.recv_timeout(Duration::from_secs(2)).expect("旁路应收到字节"),
            b"\x01\x02FRAME\x03".to_vec()
        );
        assert!(
            tap_rx.recv_timeout(Duration::from_millis(120)).is_err(),
            "raw 期间 tap 不该收到字节"
        );

        h.detach_raw();
        probe.feed(b"back\r\n");
        assert_eq!(
            tap_rx.recv_timeout(Duration::from_secs(2)).expect("撤销 raw 后 tap 应恢复"),
            b"back\r\n".to_vec()
        );
        wait_until(&mut h, |h| h.snapshot.screen_text().contains("back"));
    }
}

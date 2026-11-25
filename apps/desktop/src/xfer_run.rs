//! 界面里的「文件传输」：把 `term-xfer`（XMODEM / YMODEM）与 `term-zmodem`
//! （ZMODEM）两个协议内核放到后台线程上跑。
//!
//! # 两套协议内核，一条驱动
//!
//! 两个 crate 的公开面是照着同一个形状做的（`start` / `feed` / `on_timeout` /
//! `cancel` / `phase` / 字节计数，事件类型共用 `term_xfer::{Action, Error}`），
//! 所以驱动层只需要一个 [`Engine`] 抽象 + 四个薄壳（X/Y 收发、ZMODEM 收发）。
//! 协议差异全部收在壳里，下面的线程、落盘、进度、取消、旁路交还**只有一份实现**
//! ——这正是「换协议不该换驱动」的落点。
//!
//! 为什么这个文件在 `apps/desktop/src/` 而不在 `src/ui/`（与 `script_run.rs` 同一条
//! 理由，见那里的模块注释）：
//!
//! 1. **IO 不进投影层**。读文件、写文件、抢会话的原始字节、起线程都是副作用；
//!    `ui/` 里只该有「画」与「发动作」。界面拿到的是一个不透明句柄 + 一份廉价快照。
//! 2. **传输会阻塞**。两种协议都是「发一块、等一个应答」的停等循环（XMODEM 等
//!    `ACK`，ZMODEM 等 `ZACK`），放在 UI 线程上就是界面假死。所以 `start()` 起
//!    线程、`progress()` 轮询。
//!
//! # 字节走哪条路
//!
//! 传输期间协议字节是纯二进制，绝不能进 VT（会把屏幕刷乱、把日志写满、把
//! `read`/`read_until` 的累计污染掉）。所以线程第一件事是
//! [`SessionPool::attach_raw`] 把这条会话的输出整块改道到本线程的 sink，
//! 收尾时**一定** [`SessionPool::detach_raw`]（[`DetachGuard`] 的 `Drop`，
//! 正常结束 / 取消 / 出错 / panic 四条路都盖住）。漏了就会让那条会话永久收不到
//! 字节——这是本文件最不可原谅的 bug，所以它不靠「记得调」而靠 `Drop`。
//!
//! # 路径穿越
//!
//! 对端给的文件名是**不可信输入**：可能是 `../../etc/passwd`、`/etc/passwd`、
//! `C:\Windows\...`。落盘前一律经 [`resolve_name`] 抽成纯 basename 再拼目标目录；
//! 绝对路径 / 盘符 / 取不出名字（`..`、空串）一律拒绝并中止传输，绝不直接拼接。
//!
//! # 取消
//!
//! [`XferHandle::cancel`] 置一个 `AtomicBool` 并直接调状态机的 `cancel()`
//! （产出中止序列让对端别再发了：X/YMODEM 是 `CAN CAN`，ZMODEM 是 `CAN`×8）。
//! 线程在下一轮循环（最多 200ms）看到旗子就退出，退出时 `Drop` 交还会话。
//!
//! # 四项 opt-in 增量的产品侧接线
//!
//! 内核 `term-zmodem` 已经实现、但默认关闭的四项能力，在这里一次性打开并接上盘：
//!
//! 1. **多文件批处理**（`Config::batch`）：[`zmodem_config`] 打开开关，
//!    [`XferHandle::start`] 把「目录 / `;` 分隔的多路径」展开成同一台 `ZSender`
//!    上的多次 `add_file`，每个文件之间有独立的 `FileDone` 边界。
//! 2. **`ZCRCG` 连发**（`Config::streaming`）：同在 [`zmodem_config`] 打开，
//!    发送方不再逐块等 `ZACK`。
//! 3. **断点续传**（`ZRPOS`）：内核自带，驱动层**不重复实现**——显式续传入口
//!    [`XferHandle::start_resume`] 把「本地已有前 N 字节」交给
//!    `Receiver::resume_from`，落盘侧从该偏移续写。弹层上的 Append / Resume 两档
//!    也走这条冲突策略通道：Append 原地追加（对端从 0 发），Resume 需要握手前就
//!    知道文件名与长度；只有目录时无法安全协商，驱动明确拒绝并说明原因，
//!    绝不静默按覆盖处理（见 [`Sink::open`]）。
//! 4. **mtime / mode 落盘**：`Action` 是 X/Y/Z 共用的事件形状，放不下这两个字段；
//!    内核把它们放在 `ZReceiver::file_mtime()` / `file_mode()` 访问器里。驱动在
//!    动作产生那一刻取样（[`FileMeta`]），随 `FileStart` 一起交给 [`Sink`]，收尾时
//!    用标准库 `FileTimes` 把 mtime 写回文件。**mode 有意不落盘**，见
//!    [`Sink::close`] 的取舍说明。
//!
//! X/YMODEM 的路径不受影响：单文件仍只调一次 `add_file`，线上字节不变。

use std::fs::{File, FileTimes, OpenOptions};
use std::io::{Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, RecvTimeoutError};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

// 会话标识的规范来源是 term-core；term-session 只是 `use` 了它，没有 re-export，
// 所以这里与 `script_run.rs` 取同一个类型（`u64` 别名，天然互通）。
use term_core::session_runtime::SessionId;
use term_session::SessionPool;
use term_xfer::{Action, Config, Phase, Receiver, Sender};
use term_xfer::Protocol as WireProtocol;
// ZMODEM 的协议内核与 X/YMODEM 完全独立（见 `crates/term-zmodem/src/lib.rs`
// 的说明），但事件类型共用 `term_xfer::{Action, Error, Fault}`，所以这里的
// 分派只在**建引擎**那一步，跑起来之后两条路没有区别。
use term_zmodem::{
    Config as ZConfig, Phase as ZPhase, Receiver as ZReceiver, Sender as ZSender,
};

/// 对端静默多久算「该催一下」。XMODEM 的经典节奏是 3 秒；这里取 200ms 的
/// 轮询粒度，由状态机内部的重试上限兜住次数（X/Y 默认 10 次 ≈ 2 秒；ZMODEM
/// 在 [`zmodem_config`] 里抬到 40 次 ≈ 8 秒，理由见那里）。
const POLL: Duration = Duration::from_millis(200);

/// 用户取消时 `Progress::error` 的**精确**内容。
///
/// 这是驱动与界面之间的契约：`ui/terminal/xfer.rs` 按这个值把「中性取消」与
/// 「真失败」分开。**只此一处定义，UI 直接引用这个常量**——若两边各写一份
/// 字面量，改文案时界面会静默退化：取消会被显示成 `Failed:`，而不是编译错误。
pub(crate) const CANCELLED: &str = "传输已取消";

/// 传输协议。**故意不复用** `term_xfer::Protocol`：`term-app` 不依赖 `term-xfer`，
/// 界面层也不该被协议 crate 的版本牵着走。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum XferProtocol {
    /// 经典 XMODEM：128 字节块，单文件，线上不带文件名与长度。
    Xmodem,
    /// XMODEM-1K：主体 1024 字节块，尾块降到 128。
    Xmodem1k,
    /// YMODEM：块 0 带文件名 + 长度，支持多文件。
    Ymodem,
    /// ZMODEM：可变长帧 + 帧内转义 + 按偏移重传；起支持多文件批处理。
    ///
    /// 与上面三种**不是同一个协议家族**：线上时序、错误恢复、帧格式全不同，
    /// 内核在另一个 crate（`term-zmodem`）。所以 [`XferProtocol::wire`] 返回的
    /// 不是 `term_xfer::Protocol`，而是把家族分开的 [`Wire`]。
    Zmodem,
}

impl XferProtocol {
    /// 落到哪个协议内核。
    ///
    /// 为什么需要 [`Wire`] 这一层：`term_xfer::Protocol` 表达不了 ZMODEM——它是
    /// X/Y 的块长与批量语义。往里加一个 `Zmodem` 变体会让那个 crate 里每一个
    /// `match protocol` 都长出一条永远不会走到的分支，而 `term-xfer` 的协议实现
    /// 本轮**明确不许动**。所以家族用驱动层自己的二选一壳分开，`start` 只在这里
    /// 分派一次，之后两条路共用同一个 [`Engine`] 接口。
    fn wire(self) -> Wire {
        match self {
            XferProtocol::Xmodem => Wire::Xfer(WireProtocol::Xmodem),
            XferProtocol::Xmodem1k => Wire::Xfer(WireProtocol::Xmodem1k),
            XferProtocol::Ymodem => Wire::Xfer(WireProtocol::Ymodem),
            XferProtocol::Zmodem => Wire::Zmodem,
        }
    }
}

/// 线上协议家族：谁能处理这次传输。
///
/// 两级而不是平铺四个变体：`Xfer` 里那三种共用 `term-xfer` 的一台状态机
/// （差别只在 `Config`），ZMODEM 是另一台。分派点因此只有两处 `match`。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Wire {
    /// XMODEM / XMODEM-1K / YMODEM，状态机在 `term-xfer`。
    Xfer(WireProtocol),
    /// ZMODEM，状态机在 `term-zmodem`。
    Zmodem,
}

/// 传输方向。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum XferKind {
    /// 本端发送。
    Send,
    /// 本端接收。
    Receive,
}

/// 界面每帧读一次的快照（廉价、可 `Clone`）。
#[derive(Clone, Debug, Default)]
pub struct Progress {
    /// 状态机当前阶段的可读名（「握手」「接收数据」「等对端 ZRINIT」「已完成」……）。
    ///
    /// 由各协议引擎自己给（[`Engine::phase`]）——X/YMODEM 与 ZMODEM 的时序不同名，
    /// 驱动层不做统一翻译。
    pub phase: String,
    /// 已收 / 已发的字节数。单调递增（YMODEM 多文件也不回退）。
    pub bytes: u64,
    /// 已知总大小：YMODEM / ZMODEM 的文件头给得出，发送侧本端文件大小已知；
    /// 经典 XMODEM 线上不带长度，为 `None`。
    pub total: Option<u64>,
    /// 成功结束了。
    pub finished: bool,
    /// 失败原因（人话，带阶段）。取消也算失败，原因写「传输已取消」。
    pub error: Option<String>,
}

/// 一次正在进行的传输。丢弃句柄 = 取消（线程会在 200ms 内收尾并交还会话）。
pub struct XferHandle {
    pool: Arc<SessionPool>,
    session: SessionId,
    engine: SharedEngine,
    progress: Arc<Mutex<Progress>>,
    cancel: Arc<AtomicBool>,
    /// 只为「句柄活着线程就活着」这层直觉保留；`Drop` 不 join（见那里）。
    join: Option<JoinHandle<()>>,
}

/// 状态机被两条路访问：后台线程推进它，`cancel()` 也要能让它吐中止序列。
type SharedEngine = Arc<Mutex<Box<dyn Engine>>>;

impl XferHandle {
    /// 开始一次传输。
    ///
    /// `target`：`Receive` 时是**目标目录**（必须已存在）；`Send` 时是**本地文件
    /// 路径**，也可以是一个目录（展开其中全部普通文件）或 `;` 分隔的多条路径
    /// ——后两种只在 ZMODEM 下作为一次批处理发出，见 [`collect_send_files`]。
    ///
    /// 返回 `Err` 的情况都是「当场就能判定的输入问题」，不是传输失败：
    /// 会话不在册、目标目录不可用、要发的文件读不到、批处理里出现重名文件。
    /// 传输过程中的失败走 [`Progress::error`]。
    pub fn start(
        pool: Arc<SessionPool>,
        session: SessionId,
        kind: XferKind,
        protocol: XferProtocol,
        target: PathBuf,
    ) -> Result<Self, String> {
        Self::start_with_policy(
            pool,
            session,
            kind,
            protocol,
            target,
            Default::default(),
            false,
            None,
        )
    }

    /// 带同名冲突策略的入口。`literal_single` 是 UI 传来的显式事实：`target`
    /// 是否为拖放进来的**一条**本地路径；手动输入的 `a;b` 传 false，按多文件语法判定。
    ///
    /// `explicit_paths` 是拖放拿到的路径表：Some 时逐条展开，绝不按 `;` 拆——
    /// 多文件里文件名含 `;` 是合法的，拼成文本就丢了「这是两条」的事实；手输
    /// 路径传 None，仍按 `send_target_paths` 的碎片判定。`target` 用于展示与
    /// 接收侧目录校验。
    pub fn start_with_policy(
        pool: Arc<SessionPool>,
        session: SessionId,
        kind: XferKind,
        protocol: XferProtocol,
        target: PathBuf,
        policy: crate::xfer_conflict::ConflictPolicy,
        literal_single: bool,
        explicit_paths: Option<Vec<PathBuf>>,
    ) -> Result<Self, String> {
        Self::start_impl(
            pool,
            session,
            kind,
            protocol,
            target,
            policy,
            0,
            literal_single,
            explicit_paths,
        )
    }

    /// 断点续传：本地已有目标文件的前 `offset` 字节，协议层回 ZRPOS(offset)，
    /// 落盘侧从该偏移继续写。`offset=0` 等价于普通接收。
    pub fn start_resume(
        pool: Arc<SessionPool>,
        session: SessionId,
        protocol: XferProtocol,
        target: PathBuf,
        offset: u64,
    ) -> Result<Self, String> {
        Self::start_impl(
            pool,
            session,
            XferKind::Receive,
            protocol,
            target,
            // 显式续传入口：策略也随之落在 Resume 档，让判据与协议层的 ZRPOS
            // 用同一个口径（落盘方式见 `Sink::open`）。
            crate::xfer_conflict::ConflictPolicy::Resume,
            offset,
            false,
            None,
        )
    }

    fn start_impl(
        pool: Arc<SessionPool>,
        session: SessionId,
        kind: XferKind,
        protocol: XferProtocol,
        target: PathBuf,
        policy: crate::xfer_conflict::ConflictPolicy,
        resume_offset: u64,
        literal_single: bool,
        explicit_paths: Option<Vec<PathBuf>>,
    ) -> Result<Self, String> {
        if pool.state(session).is_none() {
            return Err(format!("控制面里查不到这个会话（id={session}）"));
        }
        let mut progress = Progress {
            phase: phase_name(Phase::Idle).to_string(),
            ..Progress::default()
        };
        let wire = protocol.wire();
        let engine: Box<dyn Engine> = match kind {
            XferKind::Receive => {
                let meta = std::fs::metadata(&target)
                    .map_err(|e| format!("接收目录 {} 不可用：{e}", target.display()))?;
                if !meta.is_dir() {
                    return Err(format!("接收目标不是目录：{}", target.display()));
                }
                match wire {
                    Wire::Xfer(protocol) => {
                        if resume_offset > 0 {
                            return Err("X/YMODEM 不支持断点续传，只有 ZMODEM 有 ZRPOS 协商".into());
                        }
                        Box::new(RxEngine {
                            inner: Receiver::new(Config::for_protocol(protocol)),
                        })
                    }
                    Wire::Zmodem => {
                        let mut engine = ZRxEngine {
                            inner: ZReceiver::new(zmodem_config()),
                        };
                        if resume_offset > 0 {
                            engine.resume_from(resume_offset)?;
                        }
                        Box::new(engine)
                    }
                }
            }
            XferKind::Send => {
                // 协议层是零 IO 的，盘只能在这里读；整读进内存是 `add_file` 的要求。
                // 单文件走的就是原来那一次 `add_file`，线上字节逐字节不变。
                let files = match &explicit_paths {
                    // 拖放的路径表是显式事实：逐条展开，不经过 `;` 拼接/拆分。
                    Some(paths) => collect_explicit_send_files(paths),
                    None => collect_send_files(&target, literal_single),
                }
                .map_err(|err| err.to_string())?;
                if files.is_empty() {
                    return Err(format!(
                        "发送目标里没有可发送的普通文件：{}",
                        target.display()
                    ));
                }
                if files.len() > 1 && wire != Wire::Zmodem {
                    return Err(
                        "多文件批处理只支持 ZMODEM；X/YMODEM 请一次发一个文件".to_string(),
                    );
                }
                progress.total = Some(files.iter().map(|f| f.data.len() as u64).sum());
                match wire {
                    Wire::Xfer(protocol) => {
                        let mut tx = Sender::new(Config::for_protocol(protocol));
                        for file in files {
                            tx.add_file(file.name, file.data)
                                .map_err(|e| format!("装不进待发队列：{e}"))?;
                        }
                        Box::new(TxEngine { inner: tx })
                    }
                    Wire::Zmodem => {
                        let mut tx = ZSender::new(zmodem_config());
                        for file in files {
                            // 本地 mtime 一并带过去（`add_file` 恒写 0，须用带时间
                            // 的那个入口）；接收侧会在 `Sink::close` 把它还原。
                            // `None` 用 0 表达「对端没给」，落盘侧会跳过 0。
                            tx.add_file_with_time(
                                file.name,
                                file.data,
                                file.mtime.unwrap_or(0),
                            )
                            .map_err(|e| format!("装不进待发队列：{e}"))?;
                        }
                        Box::new(ZTxEngine { inner: tx })
                    }
                }
            }
        };
        let engine: SharedEngine = Arc::new(Mutex::new(engine));
        let progress = Arc::new(Mutex::new(progress));
        let cancel = Arc::new(AtomicBool::new(false));

        // 先登记旁路再起线程：`start` 返回之后到达的字节必须已经改道，
        // 否则会有一小段字节漏进 VT。`attach_raw` 是等到会话线程 ack 才返回的。
        let (sink_tx, sink_rx) = mpsc::channel::<Vec<u8>>();
        if !pool.attach_raw(session, sink_tx) {
            return Err(format!("会话 {session} 不在册，接管不了字节"));
        }
        let dir = (kind == XferKind::Receive).then_some(target);
        let spawned = {
            let pool = pool.clone();
            let engine = engine.clone();
            let progress = progress.clone();
            let cancel = cancel.clone();
            std::thread::Builder::new()
                .name("desk-xfer".to_string())
                .spawn(move || {
                    // 兜底：线程以任何方式结束（含 panic），会话都要还给 VT。
                    let _detach = DetachGuard {
                        pool: pool.clone(),
                        session,
                    };
                    run(pool, session, engine, progress, cancel, sink_rx, dir, kind, policy, resume_offset);
                })
        };
        let join = match spawned {
            Ok(join) => join,
            Err(e) => {
                pool.detach_raw(session);
                return Err(format!("起不了传输线程：{e}"));
            }
        };
        Ok(Self {
            pool,
            session,
            engine,
            progress,
            cancel,
            join: Some(join),
        })
    }

    /// 读一份快照（界面每帧调一次）。
    pub fn progress(&self) -> Progress {
        self.progress
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
            .clone()
    }

    /// 取消。幂等；已经结束的传输不受影响。
    ///
    /// 用户点取消时 `error` **恒为精确的** `"传输已取消"`——UI 侧按这个字面量
    /// 把「中性取消」和「真失败」分开，所以这里不许换措辞、也不许附诊断前缀。
    pub fn cancel(&self) {
        if self.cancel.swap(true, Ordering::SeqCst) || self.ended() {
            return;
        }
        // 直接让状态机吐中止序列：对端不必等到我们线程下一轮才发现。
        let actions = {
            let mut guard = self.engine.lock().unwrap_or_else(|p| p.into_inner());
            guard.cancel()
        };
        for action in actions {
            if let Action::Send(bytes) = action {
                // 会话已断时这一笔写不出去，但用户的意图仍是「取消」，不是失败。
                let _ = self.pool.send(self.session, &bytes);
            }
        }
        let mut progress = self.progress.lock().unwrap_or_else(|p| p.into_inner());
        progress.phase = phase_name(Phase::Aborted).to_string();
        if !progress.finished && progress.error.is_none() {
            progress.error = Some(CANCELLED.to_string());
        }
    }

    /// 线程是否已经给出终态（成功或失败）。终态之后取消 / 丢弃句柄都是空操作。
    fn ended(&self) -> bool {
        let progress = self.progress.lock().unwrap_or_else(|p| p.into_inner());
        progress.finished || progress.error.is_some()
    }
}

impl Drop for XferHandle {
    fn drop(&mut self) {
        // 句柄没了就别让线程无人值守地继续跑：置旗子，线程 200ms 内收尾并 detach。
        // **不 join**：界面关标签页时不该为一次传输阻塞一帧。
        //
        // 传输已经收尾时什么都不发：UI 在终态上点「关闭」会把句柄丢掉，那时再补
        // 一串 `CAN CAN` 只会让对端收到莫名其妙的噪声（会话早已交还 VT）。
        if !self.cancel.swap(true, Ordering::SeqCst) && !self.ended() {
            let actions = {
                let mut guard = self.engine.lock().unwrap_or_else(|p| p.into_inner());
                guard.cancel()
            };
            for action in actions {
                if let Action::Send(bytes) = action {
                    self.pool.send(self.session, &bytes);
                }
            }
        }
        // 丢掉 JoinHandle 只是「分离」，线程自己跑完；它的 DetachGuard 负责收尾。
        self.join.take();
    }
}

/// 线程无论怎么结束都要交还会话。放在 `run` 的栈上，panic 展开也会执行。
struct DetachGuard {
    pool: Arc<SessionPool>,
    session: SessionId,
}

impl Drop for DetachGuard {
    fn drop(&mut self) {
        self.pool.detach_raw(self.session);
    }
}

/// 状态机的最小接口面：四个引擎（X/Y 收发 + ZMODEM 收发）共用一套驱动循环。
trait Engine: Send {
    /// 开局动作。X/YMODEM 接收方发握手字符；ZMODEM 的**发送方**要主动发
    /// `ZRQINIT`（那一族的开场由发送方开口，与 X/Y 正好相反）；其余情况为空。
    fn begin(&mut self) -> Vec<Action>;
    fn feed(&mut self, bytes: &[u8]) -> Vec<Action>;
    fn on_timeout(&mut self) -> Vec<Action>;
    fn cancel(&mut self) -> Vec<Action>;
    /// 当前阶段的**可读名**，界面原样显示（进 `Progress::phase`）。
    ///
    /// 为什么是 `&'static str` 而不是某个阶段枚举：`Progress::phase` 本来就是
    /// 一句给用户看的话，界面从不按它分支（`ui/terminal/xfer.rs` 只把它当
    /// 明细文本用）。中间再放一个驱动层的枚举等于多一张映射表、多一次 `match`，
    /// 收益为零；而「各协议自己决定这阶段该怎么念」是必须的——X/YMODEM 的
    /// `SendingHeader` 与 ZMODEM 的 `AwaitingZrpos` 不是同一件事（见
    /// `crates/term-zmodem/src/phase.rs` 开头）。所以映射留在各自的壳里，
    /// 驱动层只拿到一句已经能显示的话。
    fn phase(&self) -> &'static str;
    /// 已收 / 已发的字节数（状态机自己的口径）。
    fn bytes(&self) -> u64;
    /// 当前文件的 ZMODEM 元数据。默认全 `None`：X/YMODEM 与 ZMODEM **发送侧**
    /// 都没有这个概念，只有 [`ZRxEngine`] 覆盖它。
    ///
    /// 内核在 `on_file_info` 里先写好 `mtime` / `mode` 再 push `Action::FileStart`，
    /// 所以「动作返回后立刻取样」拿到的就是这个文件的元数据。驱动的握手是
    /// 停等的（每个 `ZFILE` 后面跟着我们回的 `ZRPOS`，对端收到才继续），合规对端
    /// 一次 `feed` 只会给出一个 `FileStart`；`drive()` 按动作逐条取样，因此能
    /// 把取样值安全带下去。
    fn file_meta(&self) -> FileMeta {
        FileMeta::default()
    }
}

struct RxEngine {
    inner: Receiver,
}

impl Engine for RxEngine {
    fn begin(&mut self) -> Vec<Action> {
        self.inner.start()
    }
    fn feed(&mut self, bytes: &[u8]) -> Vec<Action> {
        self.inner.feed(bytes)
    }
    fn on_timeout(&mut self) -> Vec<Action> {
        self.inner.on_timeout()
    }
    fn cancel(&mut self) -> Vec<Action> {
        self.inner.cancel()
    }
    fn phase(&self) -> &'static str {
        phase_name(self.inner.phase())
    }
    fn bytes(&self) -> u64 {
        self.inner.received_bytes()
    }
}

struct TxEngine {
    inner: Sender,
}

impl Engine for TxEngine {
    fn begin(&mut self) -> Vec<Action> {
        // 发送方必须等对端先喊 'C' / NAK，构造时不能有副作用。
        Vec::new()
    }
    fn feed(&mut self, bytes: &[u8]) -> Vec<Action> {
        self.inner.feed(bytes)
    }
    fn on_timeout(&mut self) -> Vec<Action> {
        self.inner.on_timeout()
    }
    fn cancel(&mut self) -> Vec<Action> {
        self.inner.cancel()
    }
    fn phase(&self) -> &'static str {
        phase_name(self.inner.phase())
    }
    fn bytes(&self) -> u64 {
        self.inner.sent_bytes()
    }
}

/// ZMODEM 的参数。**驱动侧的协议策略只在这里调**。
///
/// - `max_retries = 40`：驱动每 200ms 打一次 `on_timeout`，而 `term-zmodem` 的
///   默认上限是 10 次（≈2 秒）。真实 `sz` / `rz` 从 `exec` 到说出第一个字节在忙
///   机器上是几百毫秒，而 `lrzsz` 自己的 `ZRQINIT` 重试间隔是秒级的——2 秒的
///   窗口太窄，会把「对端还在启动」误判成「对端不说话」。抬到 40 次（≈8 秒）。
///   这是**驱动侧的定时策略**，不是协议改动：`term-zmodem` 只提供钩子，重传节奏
///   归调用方（见它的 `lib.rs` 纪律第 3 条）。
/// - `batch = true`：内核的多文件批处理。`ZEOF` 之后接收方接受下一个 `ZFILE`
///   （而不是回 `ZSKIP` 把对端赶去 `ZFIN`），发送方允许 `add_file` 排队；每个文件
///   结束一次 `Action::FileDone`，就是两个 `FileStart` 之间的边界。
/// - `streaming = true`：内核的 `ZCRCG` 连发。发送方一次把剩余数据打成连发子包，
///   只在结尾用 `ZCRCW` 要 `ZACK`；接收方两种帧尾都认，与本字段无关。
///
/// 两项都是 opt-in：关掉时线上字节逐字节不变；打开后单文件场景也不变（`batch`
/// 只在「`ZEOF` 之后又来一个 `ZFILE`」时才多一条分支）。
///
/// 集成测试经 `#[path]` 直接编这份源码，故 `pub(crate)` 暴露给验收断言。
pub(crate) fn zmodem_config() -> ZConfig {
    ZConfig::default()
        .with_max_retries(40)
        .with_batch(true)
        .with_streaming(true)
}

/// ZMODEM 接收方。
struct ZRxEngine {
    inner: ZReceiver,
}

impl ZRxEngine {
    fn resume_from(&mut self, offset: u64) -> Result<(), String> {
        self.inner.resume_from(offset).map_err(|e| e.to_string())
    }
}

impl Engine for ZRxEngine {
    fn begin(&mut self) -> Vec<Action> {
        // 接收方开口：发 `ZRINIT`（宣告 `CANFC32` 与缓冲大小）。
        self.inner.start()
    }
    fn feed(&mut self, bytes: &[u8]) -> Vec<Action> {
        self.inner.feed(bytes)
    }
    fn on_timeout(&mut self) -> Vec<Action> {
        self.inner.on_timeout()
    }
    fn cancel(&mut self) -> Vec<Action> {
        self.inner.cancel()
    }
    fn phase(&self) -> &'static str {
        zmodem_phase_name(self.inner.phase())
    }
    fn bytes(&self) -> u64 {
        self.inner.received_bytes()
    }
    fn file_meta(&self) -> FileMeta {
        FileMeta {
            mtime: self.inner.file_mtime(),
            mode: self.inner.file_mode(),
            size: self.inner.file_size(),
            // 协议层钳制后的续传起点；落盘侧绝不能用原始声明值。
            resume: self.inner.effective_resume_offset(),
        }
    }
}

/// ZMODEM 发送方。**它的 `begin()` 不是空的**：这一族的开场由发送方发
/// `ZRQINIT`，与 X/YMODEM「发送方等对端喊 C」相反。
struct ZTxEngine {
    inner: ZSender,
}

impl Engine for ZTxEngine {
    fn begin(&mut self) -> Vec<Action> {
        self.inner.start()
    }
    fn feed(&mut self, bytes: &[u8]) -> Vec<Action> {
        self.inner.feed(bytes)
    }
    fn on_timeout(&mut self) -> Vec<Action> {
        self.inner.on_timeout()
    }
    fn cancel(&mut self) -> Vec<Action> {
        self.inner.cancel()
    }
    fn phase(&self) -> &'static str {
        zmodem_phase_name(self.inner.phase())
    }
    fn bytes(&self) -> u64 {
        self.inner.sent_bytes()
    }
}

/// 一轮推进的结果。
enum Step {
    Continue,
    Done,
    Cancelled,
    Failed(String),
}

/// ZMODEM 的 `ZFILE` 附加元数据。
///
/// `term_xfer::Action` 是 X/Y/Z 共用的事件形状，放不下 mtime / mode；内核把它们
/// 放在 `ZReceiver::file_mtime()` / `file_mode()` 访问器里。驱动在**引擎产出动作的
/// 那一刻**取样并随 `FileStart` 一起往下传，`pump` 不必回头去问状态机（也就不用
/// 在持有进度锁时再抢状态机锁）。
///
/// 集成测试经 `#[path]` 直接编这份源码，故字段用 `pub(crate)` 暴露给验收断言。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct FileMeta {
    /// 对端给的修改时间（Unix 秒）；`None` = 对端没给。
    pub(crate) mtime: Option<u32>,
    /// 对端给的权限位（低 12 位）；`None` = 对端没给。**有意不落盘**，见
    /// [`Sink::close`]。
    pub(crate) mode: Option<u32>,
    /// 对端声明的文件长度（`ZFILE` 的长度字段）。`NewerAndLonger` 必须拿到它，
    /// 否则「更新且更长」会被当成「无条件覆盖」。
    pub(crate) size: Option<u64>,
    /// 协议层钳制后的续传起点；`0` = 从头收。
    pub(crate) resume: u64,
}

/// 驱动内部动作：把 `term_xfer::Action` 展开，并给 `FileStart` 补上元数据。
///
/// 为什么不直接给 `Action::FileStart` 加字段：那个枚举属于 `term-xfer`，是
/// X/Y/Z 共用的契约，本轮明确不许动。驱动自己这一层薄薄的适配就够了。
#[derive(Debug)]
pub(crate) enum DriveAction {
    Send(Vec<u8>),
    Data(Vec<u8>),
    FileStart {
        name: String,
        size: Option<u64>,
        meta: FileMeta,
    },
    FileDone,
    Done,
    Error(term_xfer::Error),
}

/// 把一批协议动作翻成驱动动作；`sample_meta` 是**逐条**取样的入口。
///
/// 合规对端一次 `feed` 只给出一个 `FileStart`（每个 `ZFILE` 后面跟着我们回的
/// `ZRPOS` / `ZRINIT`，对端要等它才继续），所以采样一次就是全部。多个 `FileStart`
/// 挤进同一帧是非合规对端的行为，但**不能因此把整批的时间戳都丢掉**：每个
/// `FileStart` 都调用一次 `sample_meta()`，由取样方决定该给这一条什么元数据。
/// `Send` / `Data` / `FileDone` 等动作不取样，避免无谓调用。
pub(crate) fn drive(
    actions: Vec<Action>,
    mut sample_meta: impl FnMut() -> FileMeta,
) -> Vec<DriveAction> {
    // 真实 receiver 在一帧多个 ZFILE 时只保留末态快照；逐条取样会把末文件的
    // mtime/size/resume 串给前面的文件（resume 串错会写错位置）。恢复安全门：
    // 同帧多 FileStart 整批默认，宁可丢时间戳也不串数据。
    let file_starts = actions
        .iter()
        .filter(|action| matches!(action, Action::FileStart { .. }))
        .count();
    actions
        .into_iter()
        .map(|action| match action {
            Action::Send(bytes) => DriveAction::Send(bytes),
            Action::Data(bytes) => DriveAction::Data(bytes),
            Action::FileStart { name, size } => DriveAction::FileStart {
                name,
                size,
                meta: if file_starts > 1 { FileMeta::default() } else { sample_meta() },
            },
            Action::FileDone => DriveAction::FileDone,
            Action::Done => DriveAction::Done,
            Action::Error(err) => DriveAction::Error(err),
        })
        .collect()
}

/// 落盘与写会话的那一侧。收尾（`Drop`）自动关文件。
pub(crate) struct Sink<'a> {
    pub(crate) pool: &'a SessionPool,
    pub(crate) session: SessionId,
    /// 接收侧的目标目录；发送侧是 `None`。
    pub(crate) dir: Option<PathBuf>,
    pub(crate) file: Option<(PathBuf, File)>,
    /// 当前文件对端给的 mtime，收尾时落盘。
    pub(crate) mtime: Option<u32>,
    /// 当前文件对端给的权限位。**只记录不落盘**，见 [`Sink::close`]。
    pub(crate) mode: Option<u32>,
    pub(crate) policy: crate::xfer_conflict::ConflictPolicy,
    /// `Skip` 命中：字节照收但丢弃，不建文件。
    pub(crate) skipping: bool,
    /// 断点续传偏移：>0 时文件从该长度继续写。
    pub(crate) resume_offset: u64,
}

impl Sink<'_> {
    /// 开当前文件。`raw` 是**对端给的**名字，必须过 [`resolve_name`]。
    ///
    /// 返回值是「已有内容要计入进度分母的字节数」（只有 Append 档非 0）：
    /// 对端声明的长度不含这段旧字节，直接拿它当分母会显示 > 100%。
    pub(crate) fn open(&mut self, raw: &str, meta: FileMeta) -> Result<Option<u64>, String> {
        let dir = self
            .dir
            .clone()
            .ok_or_else(|| "发送侧不该收到文件头".to_string())?;
        // XMODEM 线上不带文件名（`name` 为空串，见 `term_xfer::Action` 的文档），
        // 这不是「对端给了坏名字」——我们自己起一个不会撞名的默认名。
        let name = if raw.is_empty() {
            format!("xmodem-{}.bin", unix_millis())
        } else {
            resolve_name(raw)?
        };
        self.close()?;
        let path = dir.join(&name);
        let existing = std::fs::metadata(&path).ok().map(|m| {
            crate::xfer_conflict::Existing {
                modified_secs: m
                    .modified()
                    .ok()
                    .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
                    .map(|d| d.as_secs()),
                len: m.len(),
            }
        });
        let existing_len = existing.map(|e| e.len);
        // 每一次开文件都取协议层钳制后的有效偏移；不能用 start_resume 的原始
        // 声明值（对端声明的文件更短时它会越过文件末尾，set_len 会补零）。
        self.resume_offset = meta.resume;
        let (path, mode) = match crate::xfer_conflict::decide(
            &path,
            existing,
            meta.mtime,
            meta.size,
            self.policy,
        ) {
            crate::xfer_conflict::Decision::Write(path, mode) => (path, mode),
            crate::xfer_conflict::Decision::Skip => {
                self.skipping = true;
                self.mtime = meta.mtime;
                self.mode = meta.mode;
                return Ok(None);
            }
            crate::xfer_conflict::Decision::Error(why) => return Err(why),
        };
        // 起始偏移：显式续传入口（`start_resume`）已经把协议层设成 ZRPOS(N)，
        // 它的通道优先，行为与改动前逐字节一致；否则按策略给出的落盘方式决定
        // 「截断 / 追加」，这就是 Append 与 Resume 不只是假选项的落点。
        let (truncate, offset) = if self.resume_offset > 0 {
            (false, self.resume_offset)
        } else {
            match mode {
                crate::xfer_conflict::WriteMode::Overwrite => (true, 0),
                crate::xfer_conflict::WriteMode::Append => (false, existing_len.unwrap_or(0)),
                crate::xfer_conflict::WriteMode::Resume { offset } => {
                    if offset > 0 {
                        // 弹层只给了目标目录：握手前拿不到对端将发的文件名与长度，
                        // 无法请求 ZRPOS(offset)。此时若继续，不是覆盖就是接错位的
                        // 半截文件——两者都是数据损坏。明确拒绝并说清原因。
                        eprintln!(
                            "· 拒绝以 Resume 档接收 {}：目标已有 {offset} 字节，但协议层没有续传起点",
                            path.display()
                        );
                        return Err(format!(
                            "Resume 档无法安全接收 {}：目标已有 {offset} 字节，接收弹层只给了目录，\
                             握手前无法确定文件名与长度、也就无法请求 ZRPOS({offset})；\
                             已拒绝覆盖，请改选 Append / Overwrite，或用显式续传入口。",
                            path.display()
                        ));
                    }
                    // 空文件没有可续的内容，等价于从头写。
                    (true, 0)
                }
            }
        };
        let mut file = OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(truncate)
            .open(&path)
            .map_err(|e| format!("建不了文件 {}：{e}", path.display()))?;
        let extra = if matches!(mode, crate::xfer_conflict::WriteMode::Append)
            && self.resume_offset == 0
        {
            // 显式续传通道不追加旧内容（对端会从 offset 重发），只有 Append 档
            // 的最终长度才是「旧字节 + 对端完整文件」。
            existing_len
        } else {
            None
        };
        if offset > 0 {
            let len = file
                .metadata()
                .map_err(|e| format!("读不了文件长度 {}：{e}", path.display()))?
                .len();
            if len < offset {
                // 本地实际长度不够却按 offset 追加，中间会出现一段补零、
                // 后面的内容也接错位——这是静默的数据损坏，必须报错。
                return Err(format!(
                    "续传偏移 {offset} 超过本地文件 {} 的实际长度 {len}：拒绝补零造成数据错位",
                    path.display()
                ));
            }
            if len > offset {
                file.set_len(offset)
                    .map_err(|e| format!("对齐续传长度失败 {}：{e}", path.display()))?;
            }
            file.seek(SeekFrom::End(0))
                .map_err(|e| format!("定位续传偏移失败 {}：{e}", path.display()))?;
        }
        self.file = Some((path, file));
        self.mtime = meta.mtime;
        self.mode = meta.mode;
        Ok(extra)
    }

    pub(crate) fn write(&mut self, bytes: &[u8]) -> Result<(), String> {
        if self.skipping {
            return Ok(());
        }
        let Some((path, file)) = self.file.as_mut() else {
            return Err("还没收到文件头就收到了数据".to_string());
        };
        file.write_all(bytes)
            .map_err(|e| format!("写 {} 失败：{e}", path.display()))
    }

    /// 收尾：flush，然后把对端给的 mtime 写回文件。
    ///
    /// 用标准库 `std::fs::FileTimes` + `File::set_times()`（1.75 起稳定），不新增
    /// 依赖。`set_times` 失败只记一行诊断、**不改传输结果**：内容已经完整落盘，
    /// 某些文件系统 / 权限组合不允许改时间，不该因此把一次成功的传输判成失败。
    ///
    /// `mode` 的取舍：`frame::encode_file_info` 里发送端恒发 `100644`、序号恒 0，
    /// 这个权限位是「线格式惯例」而不是可信的对端意图；接收端 chmod 还涉及
    /// Unix/Windows 差异，以及「对端能不能让本端落一个可执行文件」的安全面，
    /// 超出本次范围。所以这里只是显式消费它（`take()`），让「读到了但决定不用」
    /// 在代码里可见，而不是悄悄丢掉。
    pub(crate) fn close(&mut self) -> Result<(), String> {
        if self.skipping {
            self.skipping = false;
            self.mtime = None;
            self.mode = None;
            self.resume_offset = 0;
            return Ok(());
        }
        let Some((path, mut file)) = self.file.take() else {
            return Ok(());
        };
        file.flush()
            .map_err(|e| format!("收尾 {} 失败：{e}", path.display()))?;
        // `0` 当成「对端没给有意义的时间」：内核 `add_file`（不带时间）恒写 0，
        // 而 `parse_file_info` 会把 "0" 解析成 `Some(0)`。别把文件时间设成 1970。
        if let Some(secs) = self.mtime.take().filter(|&secs| secs > 0) {
            let modified = UNIX_EPOCH + Duration::from_secs(u64::from(secs));
            if let Err(err) = file.set_times(FileTimes::new().set_modified(modified)) {
                eprintln!(
                    "· 设置 {} 的修改时间失败（内容已落盘）：{err}",
                    path.display()
                );
            }
        }
        // 权限位有意不落盘（取舍见上）。`take()` 显式消费，避免「字段写了没人读」。
        let _ignored_mode = self.mode.take();
        self.resume_offset = 0;
        Ok(())
    }
}

/// 后台线程的主体：推进状态机、分派动作、刷新进度。
#[allow(clippy::too_many_arguments)]
fn run(
    pool: Arc<SessionPool>,
    session: SessionId,
    engine: SharedEngine,
    progress: Arc<Mutex<Progress>>,
    cancel: Arc<AtomicBool>,
    rx: mpsc::Receiver<Vec<u8>>,
    dir: Option<PathBuf>,
    kind: XferKind,
    policy: crate::xfer_conflict::ConflictPolicy,
    resume_offset: u64,
) {
    let mut sink = Sink {
        pool: &pool,
        session,
        dir,
        file: None,
        mtime: None,
        mode: None,
        policy,
        skipping: false,
        resume_offset,
    };
    let mut step = pump(begin(&engine), &mut sink, &progress);
    refresh(&engine, &progress, kind);
    loop {
        match step {
            Step::Continue => {}
            Step::Done => {
                // 成功：`pump` 已经置了 `finished`，这里只把文件收干净。
                let _ = sink.close();
                return;
            }
            Step::Cancelled => {
                let _ = sink.close();
                finish(&progress, Some(CANCELLED.to_string()), true);
                return;
            }
            Step::Failed(message) => {
                // 尽力通知对端别再发了；失败就失败，不改变上报的原因。
                let _ = pump(cancel_step(&engine), &mut sink, &progress);
                let _ = sink.close();
                finish(&progress, Some(message), true);
                return;
            }
        }
        step = if cancel.load(Ordering::SeqCst) {
            pump(cancel_step(&engine), &mut sink, &progress);
            Step::Cancelled
        } else {
            match rx.recv_timeout(POLL) {
                Ok(chunk) => pump(feed(&engine, &chunk), &mut sink, &progress),
                Err(RecvTimeoutError::Timeout) => {
                    pump(timeout_step(&engine), &mut sink, &progress)
                }
                Err(RecvTimeoutError::Disconnected) => {
                    Step::Failed("会话已结束，传输中断".to_string())
                }
            }
        };
        refresh(&engine, &progress, kind);
    }
}

/// 执行一批动作。返回「接下来怎么办」。
fn pump(
    actions: Vec<DriveAction>,
    sink: &mut Sink<'_>,
    progress: &Mutex<Progress>,
) -> Step {
    let mut guard = progress.lock().unwrap_or_else(|p| p.into_inner());
    let during = guard.phase.clone();
    let mut step = Step::Continue;
    for action in actions {
        match action {
            DriveAction::Send(bytes) => {
                if !sink.pool.send(sink.session, &bytes) {
                    step = Step::Failed(format!("{during}阶段失去会话，写不出去"));
                    break;
                }
            }
            DriveAction::Data(bytes) => {
                // 「已收字节」按真正落盘的字节累加：YMODEM 多文件时状态机的
                // `received_bytes` 会在每个文件结束后归零，界面会看到进度倒退。
                if let Err(message) = sink.write(&bytes) {
                    step = Step::Failed(message);
                    break;
                }
                guard.bytes += bytes.len() as u64;
            }
            DriveAction::FileStart { name, size, meta } => {
                match sink.open(&name, meta) {
                    // Append 的最终长度 = 既有字节 + 对端发来的完整文件。
                    Ok(existing) => guard.total = size.map(|n| n + existing.unwrap_or(0)),
                    Err(message) => {
                        step = Step::Failed(message);
                        break;
                    }
                }
            }
            DriveAction::FileDone => {
                if let Err(message) = sink.close() {
                    step = Step::Failed(message);
                    break;
                }
            }
            DriveAction::Done => {
                guard.finished = true;
                step = Step::Done;
            }
            DriveAction::Error(err) => {
                step = Step::Failed(format!("{during}阶段失败：{err}"));
            }
        }
    }
    step
}

fn begin(engine: &SharedEngine) -> Vec<DriveAction> {
    let mut guard = lock(engine);
    let actions = guard.begin();
    drive(actions, || guard.file_meta())
}

fn feed(engine: &SharedEngine, bytes: &[u8]) -> Vec<DriveAction> {
    let mut guard = lock(engine);
    let actions = guard.feed(bytes);
    drive(actions, || guard.file_meta())
}

fn timeout_step(engine: &SharedEngine) -> Vec<DriveAction> {
    let mut guard = lock(engine);
    let actions = guard.on_timeout();
    drive(actions, || guard.file_meta())
}

fn cancel_step(engine: &SharedEngine) -> Vec<DriveAction> {
    let mut guard = lock(engine);
    let actions = guard.cancel();
    drive(actions, || guard.file_meta())
}

fn lock(engine: &SharedEngine) -> std::sync::MutexGuard<'_, Box<dyn Engine>> {
    engine.lock().unwrap_or_else(|poison| poison.into_inner())
}

/// 把状态机的阶段 / 字节数同步进快照。
///
/// 发送侧用状态机的 `sent_bytes`（它是权威口径）；接收侧保留 `pump` 累加的
/// 落盘字节，只在状态机更大时抬高——保证进度单调，不会因为换文件回到 0。
fn refresh(engine: &SharedEngine, progress: &Mutex<Progress>, kind: XferKind) {
    let (phase, bytes) = {
        let guard = lock(engine);
        (guard.phase(), guard.bytes())
    };
    let mut guard = progress.lock().unwrap_or_else(|p| p.into_inner());
    guard.phase = phase.to_string();
    guard.bytes = match kind {
        XferKind::Send => bytes,
        XferKind::Receive => guard.bytes.max(bytes),
    };
}

/// 收尾时补上「已中止」与失败原因（成功路径不改 `error`）。
fn finish(progress: &Mutex<Progress>, error: Option<String>, aborted: bool) {
    let mut guard = progress.lock().unwrap_or_else(|p| p.into_inner());
    if aborted {
        guard.phase = phase_name(Phase::Aborted).to_string();
    }
    if error.is_some() && guard.error.is_none() {
        guard.error = error;
    }
}

/// XMODEM / YMODEM 的阶段 → 界面文案。
///
/// 这几个名字是**线性时序**的说法（`SendingHeader` 是「刚发完 YMODEM 头块」），
/// 只对 X/YMODEM 成立，所以 ZMODEM 有自己的一张表（[`zmodem_phase_name`]）。
fn phase_name(phase: Phase) -> &'static str {
    match phase {
        Phase::Idle => "待开始",
        Phase::Handshake => "握手",
        Phase::SendingHeader => "发送文件头",
        Phase::AwaitingDataRequest => "等待数据请求",
        Phase::SendingData => "发送数据",
        Phase::AwaitingEndAck => "等待结束确认",
        Phase::AwaitingNextHeader => "等待下一个文件头",
        Phase::Receiving => "接收数据",
        Phase::Finished => "已完成",
        Phase::Aborted => "已中止",
    }
}

/// ZMODEM 的阶段 → 界面文案。
///
/// 按**帧名**念，不按 X/YMODEM 的时序念：`AwaitingZrpos` 说的是「已发 `ZFILE`，
/// 等对端告诉我从哪个偏移开始」，硬套成 `SendingHeader` 会让人以为还在发头块。
fn zmodem_phase_name(phase: ZPhase) -> &'static str {
    match phase {
        ZPhase::Idle => "待开始",
        ZPhase::AwaitingZrinit => "等对端 ZRINIT",
        ZPhase::AwaitingZfile => "等对端文件头",
        ZPhase::AwaitingZrpos => "等对端定位",
        ZPhase::Receiving => "接收数据",
        ZPhase::AwaitingZack => "等对端确认",
        ZPhase::AwaitingZrinitAfterEof => "等对端收尾",
        ZPhase::AwaitingZfin => "收尾握手",
        ZPhase::Finished => "已完成",
        ZPhase::Aborted => "已中止",
    }
}

/// 把对端给的名字压成纯 basename。
///
/// 拒收的是「路径」而不是「文件名」：绝对路径、Windows 盘符、`..`、空串。
/// 剩下的 `../../etc/passwd` 这类相对路径**不拒**——它抽出来的 basename 是
/// 一个正常的文件名，落在目标目录下就该叫 `passwd`。
pub(crate) fn resolve_name(raw: &str) -> Result<String, String> {
    if raw.is_empty() {
        return Err("拒绝空文件名".to_string());
    }
    if raw.contains('\0') {
        return Err(format!("拒绝含 NUL 的文件名：{raw:?}"));
    }
    // 对端可能来自 Windows：先统一分隔符再判绝对路径。
    let normalized = raw.replace('\\', "/");
    if normalized.starts_with('/') {
        return Err(format!("拒绝绝对路径：{raw}"));
    }
    if has_drive_prefix(&normalized) {
        return Err(format!("拒绝带盘符的路径：{raw}"));
    }
    match Path::new(&normalized).file_name().and_then(|n| n.to_str()) {
        Some(name) if !name.is_empty() && name != "." && name != ".." => Ok(name.to_string()),
        _ => Err(format!("拒绝取不出文件名的路径：{raw}")),
    }
}

fn has_drive_prefix(s: &str) -> bool {
    let bytes = s.as_bytes();
    bytes.len() >= 2 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':'
}

fn unix_millis() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0)
}

/// 一个待发的本地文件（已经整读进内存，`add_file` 的要求）。
#[derive(Debug, Clone)]
pub(crate) struct SendFile {
    pub(crate) name: String,
    pub(crate) data: Vec<u8>,
    /// 本地文件的修改时间（Unix 秒）；读不到 / 越界 / 为 0 时是 `None`。
    pub(crate) mtime: Option<u32>,
}

/// 展开待发文件时的失败。
///
/// 新加的内部边界**用分类错误**，不把 `String` 当错误类型继续扩散（冻结
/// 的正是 `Result<_, String>` 这种签名）。只有 [`XferHandle::start`] 那个面向 UI
/// 的既有边界才转成一句人话——那一条是基线里本来就有的，不是新增。
#[derive(Debug)]
pub(crate) enum SendFilesError {
    /// `metadata` / `read_dir` / `read` 失败。
    Io {
        path: PathBuf,
        source: std::io::Error,
    },
    /// 既不是普通文件也不是目录。
    NotRegular(PathBuf),
    /// 取不出文件名。
    BadName(PathBuf),
    /// 同批里出现重名（落盘会互相覆盖）。
    Duplicate(String),
}

impl std::fmt::Display for SendFilesError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SendFilesError::Io { path, source } => {
                write!(f, "读不到发送目标 {}：{source}", path.display())
            }
            SendFilesError::NotRegular(path) => {
                write!(f, "发送目标不是普通文件或目录：{}", path.display())
            }
            SendFilesError::BadName(path) => {
                write!(f, "文件名取不到：{}", path.display())
            }
            SendFilesError::Duplicate(name) => {
                write!(f, "待发文件重名：{name}（同批文件会互相覆盖）")
            }
        }
    }
}

/// 把 `Send` 侧的 `target` 展开成一排待发文件。
///
/// 三种写法，单文件路径是**原来那一条**，行为不变：
///
/// 1. 普通文件 → 一个 `add_file` 的入参（名字、内容、total 都与改动前一致）；
/// 2. 目录 → 展开其中所有**普通文件**，按路径字典序排序（`read_dir` 本身不保证
///    顺序，排序让批处理可复现），跳过子目录；
/// 3. `;` 分隔的多条路径 → 逐条按 1 / 2 展开。
///
/// 是否启用「整条路径存在就当单路径」的闸门，由调用方的**显式事实**决定：
/// `literal_single = true` 表示这条路径来自拖放（确实只有一条），才启用；
/// 手动在弹层里输入的 `a;b` 传 `false`，始终按碎片判定。
pub(crate) fn collect_send_files(
    target: &Path,
    literal_single: bool,
) -> Result<Vec<SendFile>, SendFilesError> {
    collect_explicit_send_files(&send_target_paths(target, literal_single))
}

/// 拖放拿到的路径表 → 待发文件。**每一条都是一份**，不经过 `;` 拼接或拆分。
///
/// 多文件拖放里文件名含 `;` 是合法的：`a;b.txt` 与 `c.txt` 两条拖进来，文本形态
/// 是 `a;b.txt;c.txt`，拆不掉哪一段来自哪个文件。调用方把原始 `Vec<PathBuf>`
/// 作为显式事实传到这里，才能逐条保真。展开与重名拒绝规则与手输路径共用。
pub(crate) fn collect_explicit_send_files(
    paths: &[PathBuf],
) -> Result<Vec<SendFile>, SendFilesError> {
    let mut files = Vec::new();
    for path in paths {
        expand_send_path(path, &mut files)?;
    }
    // 同名文件落盘会互相覆盖：批处理里这是数据丢失，不如当场拒绝。
    let mut seen = std::collections::BTreeSet::new();
    for file in &files {
        if !seen.insert(file.name.as_str()) {
            return Err(SendFilesError::Duplicate(file.name.clone()));
        }
    }
    Ok(files)
}

/// 把输入路径拆成一条或多条；单路径时返回的仍是**原样的那条 `PathBuf`**。
///
/// `literal_single` 是「这一条来自拖放、确实只有一条」的显式事实，不是从路径
/// 形状推出来的猜测。它只控制下面那道「整条存在优先」的闸门：
///
/// - `true`（拖放单条）：文件名里带 `;` 是合法的，`a;b` 本身是真文件时绝不拆；
///   否则「碎片都存在」的启发式会在 `a`、`b` 恰好也在同目录时把它误拆成两条。
/// - `false`（用户手动输入）：`;` 就是多文件语法，**只**看碎片是否都存在。
///   哪怕同目录同时有 `a`、`b`、`a;b` 三个文件，输入 `a;b` 也要发 `a` 和 `b`——
///   整条优先在这里会把 typed 的双文件歧义重新引回来，所以不对它开闸门。
fn send_target_paths(target: &Path, literal_single: bool) -> Vec<PathBuf> {
    let single = || vec![target.to_path_buf()];
    let Some(text) = target.to_str() else {
        return single();
    };
    if !text.contains(';') {
        return single();
    }
    if literal_single && target.exists() {
        return single();
    }
    let parts: Vec<PathBuf> = text
        .split(';')
        .map(str::trim)
        .filter(|part| !part.is_empty())
        .map(PathBuf::from)
        .collect();
    if parts.len() > 1 && parts.iter().all(|part| part.exists()) {
        parts
    } else {
        single()
    }
}

/// 展开一条路径：目录按文件名排序取普通文件，普通文件直接读，其余拒绝。
fn expand_send_path(path: &Path, out: &mut Vec<SendFile>) -> Result<(), SendFilesError> {
    let meta = std::fs::metadata(path).map_err(|source| SendFilesError::Io {
        path: path.to_path_buf(),
        source,
    })?;
    if meta.is_dir() {
        let mut entries: Vec<PathBuf> = std::fs::read_dir(path)
            .map_err(|source| SendFilesError::Io {
                path: path.to_path_buf(),
                source,
            })?
            .filter_map(|entry| entry.ok().map(|entry| entry.path()))
            .filter(|entry| entry.is_file())
            .collect();
        entries.sort();
        for entry in &entries {
            out.push(read_send_file(entry)?);
        }
    } else if meta.is_file() {
        out.push(read_send_file(path)?);
    } else {
        return Err(SendFilesError::NotRegular(path.to_path_buf()));
    }
    Ok(())
}

fn read_send_file(path: &Path) -> Result<SendFile, SendFilesError> {
    let data = std::fs::read(path).map_err(|source| SendFilesError::Io {
        path: path.to_path_buf(),
        source,
    })?;
    let name = path
        .file_name()
        .and_then(|n| n.to_str())
        .ok_or_else(|| SendFilesError::BadName(path.to_path_buf()))?
        .to_string();
    let mtime = std::fs::metadata(path)
        .ok()
        .and_then(|meta| meta.modified().ok())
        .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
        .and_then(|elapsed| u32::try_from(elapsed.as_secs()).ok())
        .filter(|&secs| secs > 0);
    Ok(SendFile { name, data, mtime })
}

#[cfg(test)]
mod tests {
    use super::{
        collect_explicit_send_files, collect_send_files, drive, pump, resolve_name, zmodem_config,
        Action, DriveAction, Engine, FileMeta, Progress, Sink, ZRxEngine,
    };
    use crate::xfer_conflict::ConflictPolicy;
    use std::path::{Path, PathBuf};
    use std::sync::Mutex;
    use std::time::{Duration, UNIX_EPOCH};
    use term_zmodem::frame::{self, CrcKind, FrameEnd, FrameType};
    use term_zmodem::HeaderKind;

    /// 每个落盘用例一个独立临时目录，互不干扰。
    fn temp_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "desk-xfer-{tag}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// 拖进来的单条路径带 `literal_single=true`：整条 `a;b` 存在就不拆。
    #[test]
    fn a_dropped_single_path_with_semicolons_is_not_split() {
        let dir = temp_dir("collect-literal");
        let whole = dir.join("a;b");
        std::fs::write(dir.join("a"), b"AAA").unwrap();
        std::fs::write(dir.join("b"), b"BBB").unwrap();
        std::fs::write(&whole, b"WHOLE").unwrap();

        let files = collect_send_files(&whole, true).expect("整条路径优先");
        let names: Vec<&str> = files.iter().map(|f| f.name.as_str()).collect();
        assert_eq!(names, ["a;b"], "拖放单条不许被拆成三份");
        assert_eq!(files[0].data, b"WHOLE");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 多文件拖放：其中一条文件名带 `;`，靠显式路径表保真，不能按拼接文本拆。
    #[test]
    fn explicit_dropped_paths_keep_a_semicolon_in_a_file_name() {
        let dir = temp_dir("collect-dropped-multi");
        let semi = dir.join("a;b.txt");
        let plain = dir.join("c.txt");
        std::fs::write(&semi, b"SEMI").unwrap();
        std::fs::write(&plain, b"PLAIN").unwrap();

        // 若退化成 `a;b.txt;c.txt` 这段文本再拆，会得到三条都不存在的碎片。
        let joined = format!("{};{}", semi.display(), plain.display());
        assert!(collect_send_files(Path::new(&joined), false).is_err());

        let files = collect_explicit_send_files(&[semi.clone(), plain.clone()])
            .expect("显式路径表该逐条保真");
        let names: Vec<&str> = files.iter().map(|f| f.name.as_str()).collect();
        assert_eq!(names, ["a;b.txt", "c.txt"], "分号是文件名的一部分，不是分隔符");
        assert_eq!(files[0].data, b"SEMI");
        assert_eq!(files[1].data, b"PLAIN");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 用户手动输入「两条绝对路径用 `;` 连接」：哪怕拼出来的整条路径本身也是
    /// 真文件（最刁钻的歧义盘面），typed 侧也必须按碎片发两个文件；同一盘面
    /// 带上拖放事实则整条优先。两份结论只由 `literal_single` 一个事实决定。
    #[test]
    fn a_typed_semicolon_input_sends_two_files_even_if_the_whole_exists() {
        let dir = temp_dir("collect-typed");
        let a = dir.join("a");
        let b = dir.join("b");
        std::fs::write(&a, b"AAA").unwrap();
        std::fs::write(&b, b"BBB").unwrap();
        // `a;b` 的歧义盘面：整条 `<dir>/a;<dir>/b` 也是存在的文件（靠一个叫 `a;`
        // 的目录把后半段绝对路径再接回来），碎片 `a` / `b` 同样都存在。
        let whole = PathBuf::from(format!("{};{}", a.display(), b.display()));
        std::fs::create_dir_all(whole.parent().unwrap()).unwrap();
        std::fs::write(&whole, b"WHOLE").unwrap();
        assert!(whole.exists(), "歧义盘面的整条路径要真的存在");

        let files = collect_send_files(&whole, false).expect("typed 多文件");
        let names: Vec<&str> = files.iter().map(|f| f.name.as_str()).collect();
        assert_eq!(names, ["a", "b"], "手动输入的分号就是两条路径");
        assert_eq!(files[0].data, b"AAA");
        assert_eq!(files[1].data, b"BBB");

        let kept = collect_send_files(&whole, true).expect("拖放单条整条优先");
        assert_eq!(kept.len(), 1, "同一盘面带拖放事实就不拆");
        let _ = std::fs::remove_dir_all(&dir);
    }

    fn sink_in<'a>(
        pool: &'a term_session::SessionPool,
        dir: PathBuf,
        policy: ConflictPolicy,
        resume_offset: u64,
    ) -> Sink<'a> {
        Sink {
            pool,
            session: 0,
            dir: Some(dir),
            file: None,
            mtime: None,
            mode: None,
            policy,
            skipping: false,
            resume_offset,
        }
    }

    #[test]
    fn append_policy_appends_the_whole_file_to_existing_content() {
        let dir = temp_dir("append");
        let target = dir.join("a.bin");
        std::fs::write(&target, b"OLD").unwrap();
        let pool = term_session::SessionPool::new();
        {
            let mut sink = sink_in(&pool, dir.clone(), ConflictPolicy::Append, 0);
            sink.open("a.bin", FileMeta::default()).unwrap();
            sink.write(b"NEW").unwrap();
            sink.close().unwrap();
        }
        assert_eq!(std::fs::read(&target).unwrap(), b"OLDNEW");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn append_policy_on_a_missing_target_starts_fresh() {
        let dir = temp_dir("append-new");
        let pool = term_session::SessionPool::new();
        {
            let mut sink = sink_in(&pool, dir.clone(), ConflictPolicy::Append, 0);
            sink.open("fresh.bin", FileMeta::default()).unwrap();
            sink.write(b"NEW").unwrap();
            sink.close().unwrap();
        }
        assert_eq!(std::fs::read(dir.join("fresh.bin")).unwrap(), b"NEW");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn resume_policy_without_a_protocol_offset_refuses_to_clobber() {
        // 弹层只给了目录：握手前拿不到文件名与长度，Resume 档必须明确拒绝，
        // 绝不能静默按覆盖写（那会丢掉目标里已有的 3 字节）。
        let dir = temp_dir("resume-refuse");
        let target = dir.join("a.bin");
        std::fs::write(&target, b"OLD").unwrap();
        let pool = term_session::SessionPool::new();
        {
            let mut sink = sink_in(&pool, dir.clone(), ConflictPolicy::Resume, 0);
            let err = sink.open("a.bin", FileMeta::default()).unwrap_err();
            assert!(err.contains("Resume"), "{err}");
            assert!(err.contains("ZRPOS"), "{err}");
        }
        assert_eq!(std::fs::read(&target).unwrap(), b"OLD", "拒绝时目标必须原样保留");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn resume_policy_with_an_empty_or_missing_target_starts_fresh() {
        // 没有可续的旧内容：空文件与不存在的目标都从头写，不该误报拒绝。
        let dir = temp_dir("resume-empty");
        std::fs::write(dir.join("empty.bin"), b"").unwrap();
        let pool = term_session::SessionPool::new();
        for name in ["empty.bin", "missing.bin"] {
            let mut sink = sink_in(&pool, dir.clone(), ConflictPolicy::Resume, 0);
            sink.open(name, FileMeta::default()).unwrap();
            sink.write(b"NEW").unwrap();
            sink.close().unwrap();
            assert_eq!(std::fs::read(dir.join(name)).unwrap(), b"NEW", "{name}");
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn explicit_resume_channel_appends_at_the_protocol_effective_offset() {
        // 显式通道：协议层回 ZRPOS(5)，且本地确有 5 字节，落盘从末尾接着写。
        let dir = temp_dir("resume-explicit");
        std::fs::write(dir.join("part.bin"), b"abcde").unwrap();
        let pool = term_session::SessionPool::new();
        {
            let mut sink = sink_in(&pool, dir.clone(), ConflictPolicy::Resume, 0);
            let meta = FileMeta { resume: 5, ..FileMeta::default() };
            sink.open("part.bin", meta).unwrap();
            sink.write(b"XY").unwrap();
            sink.close().unwrap();
        }
        assert_eq!(std::fs::read(dir.join("part.bin")).unwrap(), b"abcdeXY");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_resume_offset_past_the_local_file_is_an_error_not_zero_fill() {
        // 本地只有 3 字节却要从 5 开始写：旧实现 set_len(5) 会补两个 0，
        // 数据从此错位。现在必须报错，且原文件一个字节都不动。
        let dir = temp_dir("resume-too-far");
        let target = dir.join("part.bin");
        std::fs::write(&target, b"abc").unwrap();
        let pool = term_session::SessionPool::new();
        {
            let mut sink = sink_in(&pool, dir.clone(), ConflictPolicy::Resume, 0);
            let meta = FileMeta { resume: 5, ..FileMeta::default() };
            let err = sink.open("part.bin", meta).unwrap_err();
            assert!(err.contains("续传偏移"), "{err}");
            assert!(err.contains("实际长度"), "{err}");
        }
        assert_eq!(std::fs::read(&target).unwrap(), b"abc");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_clamped_resume_offset_wins_over_the_stale_initial_value() {
        // 对端声明 10 字节、本地已有 10 字节；原始声明值 100 被协议层钳到 10。
        // 落盘只认 FileMeta 里的有效值，否则又会在末尾补 90 个零。
        let dir = temp_dir("resume-clamped");
        let target = dir.join("part.bin");
        std::fs::write(&target, b"0123456789").unwrap();
        let pool = term_session::SessionPool::new();
        {
            let mut sink = sink_in(&pool, dir.clone(), ConflictPolicy::Resume, 100);
            let meta = FileMeta { resume: 10, size: Some(10), ..FileMeta::default() };
            sink.open("part.bin", meta).unwrap();
            sink.write(b"!").unwrap();
            sink.close().unwrap();
        }
        assert_eq!(std::fs::read(&target).unwrap(), b"0123456789!");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 真走一遍 `ZRxEngine::feed → file_meta → drive → pump → Sink::open`：
    /// 对端声明的长度必须一路带到冲突判据里，不能在中途变成 `None`。
    fn wired_file_start(name: &str, size: u64, mtime: u32) -> Vec<DriveAction> {
        let mut engine = ZRxEngine {
            inner: term_zmodem::Receiver::new(zmodem_config()),
        };
        let _ = engine.begin();
        let info = frame::encode_file_info(name, size, mtime);
        let mut wire = frame::encode_header(HeaderKind::Hex, FrameType::ZFILE, [0, 0, 0, 1]);
        wire.extend_from_slice(&frame::encode_subpacket(
            &info,
            FrameEnd::ZCRCW,
            CrcKind::Crc16,
        ));
        drive(engine.feed(&wire), || engine.file_meta())
    }

    #[test]
    fn each_file_start_in_one_frame_gets_its_own_meta_sample() {
        // 同帧多 ZFILE 时真实 receiver 只剩末态，逐条取样会串味；安全门恢复后
        // 这批元数据整批默认，非 FileStart 动作不取样。
        let first = FileMeta {
            mtime: Some(1_700_000_001),
            mode: Some(0o644),
            size: Some(3),
            resume: 0,
        };
        let second = FileMeta {
            mtime: Some(1_700_000_002),
            mode: Some(0o600),
            size: Some(5),
            resume: 2,
        };
        let mut samples = [first, second].into_iter();
        let mut calls = 0;
        let actions = vec![
            Action::Send(vec![1]),
            Action::FileStart {
                name: "a.bin".to_string(),
                size: Some(3),
            },
            Action::Data(b"abc".to_vec()),
            Action::FileDone,
            Action::FileStart {
                name: "b.bin".to_string(),
                size: Some(5),
            },
            Action::Data(b"de".to_vec()),
        ];
        let out = drive(actions, || {
            calls += 1;
            samples.next().unwrap_or_default()
        });
        assert_eq!(calls, 0, "同帧多文件不取样，避免末态串味");
        let metas: Vec<FileMeta> = out
            .iter()
            .filter_map(|action| match action {
                DriveAction::FileStart { meta, .. } => Some(*meta),
                _ => None,
            })
            .collect();
        assert_eq!(metas, [FileMeta::default(), FileMeta::default()], "同帧多文件不许串味");
    }

    #[test]
    fn a_single_file_start_keeps_the_sampled_meta() {
        // 合规对端仍是一次取样：mtime / size / 续传偏移原样进 FileStart。
        let meta = FileMeta {
            mtime: Some(42),
            mode: Some(0o755),
            size: Some(8),
            resume: 3,
        };
        let actions = vec![Action::FileStart {
            name: "only.bin".to_string(),
            size: Some(8),
        }];
        let out = drive(actions, || meta);
        match out.as_slice() {
            [DriveAction::FileStart { name, size, meta: got }] => {
                assert_eq!(name, "only.bin");
                assert_eq!(*size, Some(8));
                assert_eq!(*got, meta);
            }
            other => panic!("期望一个带元数据的 FileStart，实得 {other:?}"),
        }
    }

    /// 一个非合规对端把**两整个文件**（各含 ZFILE/ZDATA/ZEOF）流水线塞进一次
    /// `feed`：两个 ZFILE 都会走到 `on_file_info`，末态快照只剩第二个文件的值。
    /// 这是「逐条 mtime/resume 取样串味」的真实输入形状。
    fn two_files_in_one_feed_wire() -> Vec<u8> {
        let mut wire = Vec::new();
        for (name, payload, mtime) in [
            ("a.bin", &b"abc"[..], 1_700_000_001u32),
            ("b.bin", &b"defff"[..], 1_700_000_002u32),
        ] {
            let info = frame::encode_file_info(name, payload.len() as u64, mtime);
            wire.extend_from_slice(&frame::encode_header(
                HeaderKind::Hex,
                FrameType::ZFILE,
                frame::FILE_HEADER_DATA,
            ));
            wire.extend_from_slice(&frame::encode_subpacket(
                &info,
                FrameEnd::ZCRCW,
                CrcKind::Crc16,
            ));
            wire.extend_from_slice(&frame::encode_data_frame(
                0,
                payload,
                FrameEnd::ZCRCW,
                HeaderKind::Hex,
            ));
            wire.extend_from_slice(&frame::encode_header(
                HeaderKind::Hex,
                FrameType::ZEOF,
                frame::encode_position(payload.len() as u32),
            ));
        }
        wire
    }

    /// 真接一台 `ZRxEngine`，一次 `feed` 喂入双 ZFILE：安全门必须让两个
    /// `FileStart` 都拿 `FileMeta::default()`，绝不把末态（第二个文件）的 mtime
    /// 或续传偏移串到第一个文件上。
    #[test]
    fn two_file_starts_in_one_feed_never_cross_contaminate_meta() {
        let mut engine = ZRxEngine {
            inner: term_zmodem::Receiver::new(zmodem_config()),
        };
        let _ = engine.begin();

        let actions = engine.feed(&two_files_in_one_feed_wire());
        let sampled = engine.file_meta();
        // 先钉住末态确实是第二个文件的：否则这个用例根本测不出串味。
        assert_eq!(sampled.mtime, Some(1_700_000_002));
        assert_eq!(sampled.size, Some(5));
        assert_eq!(sampled.resume, 0);

        let out = drive(actions, || sampled);
        let starts: Vec<_> = out
            .iter()
            .filter_map(|action| match action {
                DriveAction::FileStart { name, size, meta } => Some((name.as_str(), *size, *meta)),
                _ => None,
            })
            .collect();
        assert_eq!(starts.len(), 2, "一次 feed 里确实出了两个 FileStart");
        assert_eq!(starts[0].0, "a.bin");
        assert_eq!(starts[1].0, "b.bin");
        assert_eq!(starts[0].1, Some(3));
        assert_eq!(starts[1].1, Some(5));
        // 同帧多 FileStart 整批安全门：任一 FileStart 都不许串到别的文件元数据。
        for (name, _, meta) in &starts {
            assert_eq!(
                *meta,
                FileMeta::default(),
                "{name} 拿到了串味的元数据（末态 mtime / 续传偏移）"
            );
        }
    }

    #[test]
    fn newer_and_longer_gets_the_peer_size_through_the_whole_wiring() {
        let dir = temp_dir("wired-newer-longer");
        let target = dir.join("a.bin");
        std::fs::write(&target, vec![b'O'; 100]).unwrap();
        let file = std::fs::File::options().write(true).open(&target).unwrap();
        file.set_times(
            std::fs::FileTimes::new()
                .set_modified(UNIX_EPOCH + Duration::from_secs(100)),
        )
        .unwrap();
        drop(file);

        let actions = wired_file_start("a.bin", 10, 101);
        // 引擎确实把 ZFILE 的长度与 mtime 取到了（不是只靠纯函数判据）。
        assert!(actions.iter().any(|a| matches!(
            a,
            DriveAction::FileStart { size: Some(10), meta, .. } if meta.mtime == Some(101)
        )));

        let pool = term_session::SessionPool::new();
        let mut sink = sink_in(&pool, dir.clone(), ConflictPolicy::NewerAndLonger, 0);
        let progress = Mutex::new(Progress::default());
        let _ = pump(actions, &mut sink, &progress);
        assert!(sink.skipping, "更新但更短：NewerAndLonger 必须跳过，不能覆盖");
        assert_eq!(std::fs::read(&target).unwrap().len(), 100, "旧文件要原样保留");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn append_progress_denominator_includes_the_existing_bytes() {
        let dir = temp_dir("append-total");
        std::fs::write(dir.join("a.bin"), b"OLD").unwrap();
        let pool = term_session::SessionPool::new();
        let mut sink = sink_in(&pool, dir.clone(), ConflictPolicy::Append, 0);
        let progress = Mutex::new(Progress::default());
        let actions = vec![DriveAction::FileStart {
            name: "a.bin".to_string(),
            size: Some(4),
            meta: FileMeta::default(),
        }];
        let _ = pump(actions, &mut sink, &progress);
        assert_eq!(
            progress.lock().unwrap().total,
            Some(7),
            "既有 3 字节 + 对端 4 字节 = 7，不能用 4 当分母"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_relative_traversal_is_flattened_to_its_basename() {
        assert_eq!(resolve_name("../../etc/passwd").unwrap(), "passwd");
        assert_eq!(resolve_name("a/b/c.bin").unwrap(), "c.bin");
        assert_eq!(resolve_name("..\\..\\windows\\x.dll").unwrap(), "x.dll");
    }

    #[test]
    fn paths_that_name_no_file_are_refused() {
        for raw in ["", "..", "/", "/etc/passwd", "/abs/path", "C:\\Windows\\x", "c:x", "."] {
            assert!(resolve_name(raw).is_err(), "{raw:?} 应当被拒");
        }
    }
}

//! 运行时会话的**协议无关抽象**（ADR-0019 最高原则）。
//!
//! 「会话」在这里只有一个形状：**双向字节流 + 终端模型 + 生命周期**。
//! 底下是 SSH、Telnet、Serial、本地 PTY 还是容器 exec，本模块不知道，
//! 也不想知道——协议差异只许存在于各 crate 对 [`Session`] 的实现里，
//! 控制面与所有客户端（UI / 脚本 / 桥接 / agent）只认这个抽象。
//!
//! 纪律：本模块是纯类型层。trait 定义不碰 IO；`Duration` 是纯时长量，
//! 不是墙钟（`Instant` / `SystemTime` 才是）——term-core 的「零墙钟」
//! 指的是后者。
//!
//! [`wait`] 的语义（对标 ZOC `ZocWait` / SecureCRT `WaitForString`）：
//! **先登记、后匹配**。登记必须先于登记点之后到达的输出被消费（实现方
//! 用 ack 保证，见 `term-runtime` 的 Supervisor），匹配对象是清洗后的
//! 纯文本（复用 [`crate::ansi::TextCleaner`] 的语义：剥转义序列、
//! CRLF 归一、跨 chunk 拼 UTF-8）。绝不轮询屏幕——洪水输出下必漏匹配。

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use crate::coords::ViewRange;
use crate::grid::{GridSnapshot, ScreenImage};
use crate::log_opts::LogOptions;

/// 会话的稳定标识。跨客户端（脚本 / 桥接 / agent）可序列化寻址。
///
/// 与 `term_runtime::supervisor::SessionId` 同为 `u64` 别名，互通。
pub type SessionId = u64;

/// 会话生命周期状态。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SessionState {
    /// 正常运行中。
    Alive,
    /// 会话结束。`None` = 被[`Session::close`]或句柄丢弃；`Some` = 进程退出
    /// 时通道上报的状态文本。
    Finished(Option<String>),
}

/// 一个会话**可能**暴露的可发现能力（ADR-0019：能力即 facet，
/// 不是协议假定）。文件传输、端口转发这类东西是否存在于某个会话，
/// 由 [`Session::facets`] 报告，调用方不得按协议硬编码假设。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FacetKind {
    /// 会话能传文件（如 SSH 之上的 SFTP facet）。
    FileTransfer,
    /// 会话能建端口转发（如 SSH 之上的隧道 facet）。
    PortForward,
}

/// [`wait`] 命中的结果：**自登记以来、直到匹配串结尾**的那段清洗后文本。
///
/// 给全这段而不只给匹配串，是脚本的真实需要——「等到 `Password:` 了」
/// 往往还想看它前面的横幅。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WaitMatch {
    pub text_upto: String,
}

/// [`wait`] 的失败方式。`Display` 的文案直接面向脚本作者。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WaitError {
    /// 空模式会立刻匹配任何东西，是脚本 bug，直接拒绝。
    EmptyPattern,
    /// 超时未命中。登记已被取消，不会在后台悄悄留着。
    Timeout,
    /// 会话线程已结束，登记没能生效（或生效前会话已死）。
    Closed,
    /// 脚本被用户取消（`CancelToken` 在切片边界看见了停止请求）。
    ///
    /// 与超时分开：超时是「等的东西没来」，取消是「人不让它等了」——
    /// 前者的处置是报告等什么，后者是安静收尾。
    Cancelled,
}

impl std::fmt::Display for WaitError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            WaitError::EmptyPattern => f.write_str("wait 的匹配串不能为空"),
            WaitError::Timeout => f.write_str("等待超时：会话输出里没出现匹配串"),
            WaitError::Closed => f.write_str("会话已结束，wait 无法登记"),
            WaitError::Cancelled => f.write_str("脚本已被取消"),
        }
    }
}

impl std::error::Error for WaitError {}

/// 取消信号的抽象（ADR-0027）。
///
/// 为什么在 `term-core` 而不是直接收 `term-script` 的 `CancelToken`：
/// 阻塞点住在 `term-io`，它不该依赖脚本引擎。这个 trait 只有一个只读方法，
/// 脚本侧的实现就是一个原子布尔；界面线程调 `cancel()`，阻塞点在切片边界
/// 调 `is_cancelled()`。
pub trait Cancel: Send + Sync {
    fn is_cancelled(&self) -> bool;
}

/// 一次已登记、尚未完成的等待。`wait` 阻塞调用线程直到命中 / 超时 /
/// 会话结束——脚本线程阻塞是**设计内**的（ZOC / SecureCRT 同款语义），
/// 但 UI 线程绝不该调它。
pub trait WaitRegistration: Send {
    fn wait(self: Box<Self>, timeout: Duration) -> Result<WaitMatch, WaitError>;

    /// 可取消的一档。默认实现只做「进来时先看一眼」——够用于微秒级返回的
    /// 实现；真正的长阻塞实现（`term-io` 的监管者句柄）覆写成 50ms 切片循环。
    ///
    /// 关键约束：**登记只做一次**，切片之间不重登记。重登记会丢掉两次登记
    /// 之间到达的输出（等待的匹配已经出现过，脚本却还在等）。
    fn wait_cancellable(
        self: Box<Self>,
        timeout: Duration,
        cancel: &dyn Cancel,
    ) -> Result<WaitMatch, WaitError> {
        if cancel.is_cancelled() {
            return Err(WaitError::Cancelled);
        }
        self.wait(timeout)
    }
}

/// 一次已登记的输出读取（`read` / `read_until` 的捕获源）。
///
/// 与 [`WaitRegistration`] 的区别只在取法：wait 等模式命中（同步事实），
/// reader 按段取走（排水）。两者都遵守**先登记后消费**：`collect` 取到的
/// 文本自登记点起算，登记前的输出不入账。
pub trait OutputReader: Send {
    /// 取走自上一次 `collect` 以来的全部新文本（取走即清空累计）。
    ///
    /// 等到有新数据即返回；`timeout` 内没有新数据返回**空串**——排水
    /// 语义，超时不算失败（`read` 原语据此区分 `read_until` 的超时显式
    /// 失败）。会话线程已结束且没有新数据 → [`WaitError::Closed`]。
    fn collect(&mut self, timeout: Duration) -> Result<String, WaitError>;

    /// 可取消的一档（同 [`WaitRegistration::wait_cancellable`]）。
    ///
    /// 默认只做「进来先看一眼」；长阻塞实现（监管者句柄）覆写成切片循环。
    fn collect_cancellable(
        &mut self,
        timeout: Duration,
        cancel: &dyn Cancel,
    ) -> Result<String, WaitError> {
        if cancel.is_cancelled() {
            return Err(WaitError::Cancelled);
        }
        self.collect(timeout)
    }
}

/// 会话的**运行时状态**：连上了没有、还活着没有、退出时说了什么。
///
/// 2026-11-08 从 `term_runtime::supervisor` 搬进来（那是 `pub use` 出来的
/// 同一个类型，老路径照样能用）。搬的理由不是整洁：控制面要给 UI 提供
/// `Session::status`，而 `term-core` 不能反过来依赖 `term-runtime`——
/// 这个类型是「会话的通用描述」，本来就不该住在副作用层。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SessionStatus {
    /// 连接建立过（PTY 起来了 / 握手过了）。
    pub connected: bool,
    /// 会话仍在正常运行。
    pub alive: bool,
    /// 子进程退出后的状态文本（状态栏那行 "Connected ( exited)" 用的就是它）。
    pub exit_status: Option<String>,
}

/// 会话日志的阶段。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum LogPhase {
    /// 没在记。
    #[default]
    Off,
    Recording,
    Paused,
    /// 打不开文件 / 写失败。**带原因**：日志写不进去时「静默继续跑」比报错
    /// 更糟——用户以为记下来了。
    Failed(String),
}

/// 会话日志的状态。
///
/// 与 [`SessionStatus`] 同批从 `term_runtime::log` 搬进来，理由相同。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct LogStatus {
    pub phase: LogPhase,
    /// 正在写的文件（`Off` / `Failed` 时可能是上一次的路径，界面对此不假设）。
    pub path: Option<PathBuf>,
}

impl LogStatus {
    /// 正在记（含已暂停——暂停时文件还开着，重开会让时间戳断档）。
    pub fn is_recording(&self) -> bool {
        matches!(self.phase, LogPhase::Recording | LogPhase::Paused)
    }
}

/// 协议无关的运行时会话。
///
/// 实现者：本地 PTY（`term-runtime`）、原生 SSH（将来经 `TermChannel`
/// 注入的通道）、以及未来的 Telnet / Serial / 容器 exec。所有方法对
/// 任意协议同形；任何协议专有概念（SSH / SFTP / 隧道）**不得**出现在
/// 这个 trait 的签名里——那正是 ADR-0019 复查触发第 5 条要回滚的东西。
///
/// # 为什么后半段方法有默认实现
///
/// `poll` / `snapshot` / `request_range` / `log_*` 这一组是**驱动界面**才
/// 需要的：脚本与桥接只读屏幕事实，不需要推事件、不需要开日志。给它们
/// 默认实现，是为了让测试替身（`FakeSession` / `DeadSession`）不必假装
/// 自己有一台监管者线程。
///
/// 默认值**不是**「静默失败」，而是「这条会话不提供该能力」，且每一种都
/// 能被调用方看出来：
///
/// - `poll` 恒 `false` → 调用方不会等一个永远不来的「有新事实」，它照常
///   用 `screen()` 取当前屏；
/// - `screen_window` 恒 `None` → 界面显示空帧，而不是显示上一次的陈旧窗口；
/// - `log_status` 恒 `Off` → 界面看到的是「没在记」，不会以为记上了。
/// 远端通过 `OSC 52` 提出的剪贴板请求（协议无关，控制面与 UI 都能认）。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ClipboardRequest {
    /// 想把这段文本写到系统剪贴板。
    Write(String),
    /// 想读取系统剪贴板（UI 批准后用 `Session::answer_clipboard` 回送）。
    Read,
}

pub trait Session: Send {
    fn id(&self) -> SessionId;

    /// 往会话写字节（键盘输入、设备应答）。
    fn write(&self, bytes: &[u8]);

    /// 调整终端尺寸。重复的同尺寸调用由实现方去重。
    fn resize(&self, cols: usize, rows: usize);

    /// 当前屏幕快照；`None` = 会话线程已结束，没有事实可取。
    fn screen(&self) -> Option<GridSnapshot>;

    fn state(&self) -> SessionState;

    /// 本会话暴露的能力。返回空表不是错误——本地 shell 就没有
    /// 文件传输 facet。
    fn facets(&self) -> Vec<FacetKind>;

    /// 登记一次等待：返回后即生效，登记点之后的输出保证被匹配。
    ///
    /// 匹配的是**清洗后的纯文本**（剥转义序列），不是原始字节——
    /// `ESC[31m` 不该挡住 `Password:` 的命中。
    ///
    /// # 避免
    /// 不要用「轮询屏幕文本」实现等待：洪水输出下，两次轮询之间的
    /// 行会被屏幕滚动冲掉，必漏匹配（ADR-0019）。
    fn wait_register(&self, pattern: &str) -> Result<Box<dyn WaitRegistration>, WaitError>;

    /// 登记一个输出读取器：返回后即生效，登记点之后的输出保证可被
    /// [`OutputReader::collect`] 取到（先登记后消费，同 [`wait_register`]）。
    ///
    /// 与 wait 的区别只在取法：wait 等模式命中，reader 按段取走。
    /// 行级过滤（grep）与哨兵匹配在客户端（`term-script`）侧做——
    /// 正则等重逻辑不进本抽象层。
    fn reader_register(&self) -> Result<Box<dyn OutputReader>, WaitError>;

    // ── 事件泵与呈现（驱动界面才需要；脚本 / 桥接用不到） ──────────────

    /// 把本帧到达的事件并入内部快照，返回**是否有新事实**。
    ///
    /// 这是「界面要不要重绘」的唯一判据：`true` 才取 [`snapshot`]，
    /// 否则整帧都不必碰快照（避免每帧克隆一份网格）。
    ///
    /// [`snapshot`]: Session::snapshot
    fn poll(&mut self) -> bool {
        false
    }

    /// 最近一次 [`poll`](Session::poll) 之后缓存的快照——**不产生往返**。
    ///
    /// 与 [`screen`](Session::screen) 的区别是代价与语义：`screen` 是
    /// 「去问监管者要一屏」（要发命令、要等应答，脚本与桥接用它），
    /// 这个只是把已经收下的那一帧读出来（界面每帧用它）。
    fn snapshot(&self) -> Option<GridSnapshot> {
        self.screen()
    }

    /// 取走本帧的 `OSC 52` 请求。默认空表——脚本 / 桥接不接剪贴板。
    fn take_clipboard_requests(&mut self) -> Vec<ClipboardRequest> {
        Vec::new()
    }

    /// 回送用户批准的剪贴板文本（仅对 `ClipboardRequest::Read` 有意义）。
    fn answer_clipboard(&self, _text: String) {}

    /// 运行时状态（连接 / 存活 / 退出文本）。
    fn status(&self) -> SessionStatus {
        let state = self.state();
        SessionStatus {
            connected: true,
            alive: matches!(state, SessionState::Alive),
            exit_status: match state {
                SessionState::Finished(text) => text,
                SessionState::Alive => None,
            },
        }
    }

    /// 请求任意行区间的只读事实窗口（回看 / 选区 / 查找共用）。
    /// 结果从 [`screen_window`](Session::screen_window) 取。
    fn request_range(&self, _range: ViewRange) {}

    /// 上一次 [`request_range`](Session::request_range) 的结果。
    /// `None` = 这条会话不提供回看，或请求还没兑现。
    fn screen_window(&self) -> Option<ScreenImage> {
        None
    }

    /// P（终端滚动条 / 悬停预览）：请求一份**预览用**的只读事实窗口。
    ///
    /// 与 [`request_range`](Session::request_range) 走**物理隔离**的第二通道
    /// （ADR-0028）：内核本来就支持一轮吐多份 `Event::Screen`，瓶颈只在句柄
    /// 只有一个 `screen` 槽；这里把预览塞进独立的槽与变体，避免一帧内两份
    /// 事件只剩一份、真实视口退化成空白帧。结果从
    /// [`preview_window`](Session::preview_window) 取。
    ///
    /// 默认空实现：脚本 / 桥接 / 无头这些不画终端的消费者不需要预览。
    fn request_preview_range(&self, _range: ViewRange) {}

    /// P：上一次 [`request_preview_range`](Session::request_preview_range) 的结果。
    /// `None` = 这条会话不提供预览，或请求还没兑现。
    fn preview_window(&self) -> Option<ScreenImage> {
        None
    }

    /// 注入唤醒回调：有输出时叫一次，让前台自己决定要不要重画。
    /// 控制面不关心前台是谁，也不知道「重画」是什么意思——它只调这个闭包。
    fn set_wakeup(&self, _wake: Arc<dyn Fn() + Send + Sync>) {}

    /// 清屏（等价终端的 `clear`）。历史不回退。
    fn clear(&self) {}

    /// `Clear Scrollback`：丢掉当前屏**以上**的全部历史行。
    ///
    /// 与 [`Self::clear`] 分开是因为屏幕上那几行是「现在」，历史是「过去」：
    /// 兼容终端把这三档拆成 Clear Screen / Clear Scrollback / Clear All 三条菜单，
    /// 不是同一个动作的三种叫法。默认空实现 = 能力尚未通到这一层的会话
    /// （如测试替身）什么都不做，而不是假装清过。
    fn clear_scrollback(&self) {}

    /// `Clear All`：清屏 + 清历史。
    fn clear_all(&self) {}

    /// 会话日志的当前状态。
    fn log_status(&self) -> LogStatus {
        LogStatus::default()
    }

    /// 开始把输出记到 `dir` 下。文件名 / 每行格式 / 覆盖方式 / 大小上限
    /// 全在 `opts` 里（`crate::log_opts` 是配置面的唯一定义）。
    fn start_log(&self, _dir: PathBuf, _session: &str, _opts: &LogOptions) {}

    /// 同上，额外带**宿主名**（ADR-0027 的 `%{host}`）。
    ///
    /// 默认转给 [`start_log`](Session::start_log)：不认识的实现照样能记日志，
    /// 只是文件名模板里的 `%{host}` 展开成空串。实现方（本地 PTY / 内置 SSH 的
    /// 监管者）覆写它，就能让 `%{host}` 有值——**不**从会话名反推主机名，
    /// 那个反推在 `user@host` 之外的命名上全是错的。
    fn start_log_with_host(
        &self,
        dir: PathBuf,
        session: &str,
        _host: &str,
        opts: &LogOptions,
    ) {
        self.start_log(dir, session, opts)
    }

    fn stop_log(&self) {}

    fn pause_log(&self) {}

    fn resume_log(&self) {}

    /// 关闭会话。之后 `state` 应报告 [`SessionState::Finished`]。
    fn close(&mut self);
}

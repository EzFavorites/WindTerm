//! 脚本侧原语：`write` / `await` 族 / `read` 族 / `screen`。
//!
//! 全部走 [`Session`] trait（协议无关），绝不轮询屏幕（§B.13）。
//! `read` / `read_until` / `wait_any` 共享一个 [`ScanCursor`]（双指针窗口，
//! 命中不消费与命中即消费两路互不干扰）；行级 grep 是**结果后处理**，见
//! [`crate::filter`]（2026-11-19 用户裁定：清理不影响匹配 / 消费 / 缓冲）。

use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use crate::cancel::CancelToken;
use term_core::automation::{Pattern, ScanCursor, Syntax};
use term_core::session_runtime::{
    FacetKind, LogStatus, OutputReader, Session, SessionId, SessionState,
};

use crate::error::PrimitiveError;
use crate::filter::LineFilter;

/// 未注入沙箱时的单次等待上限（24h 防呆钳制）。
const DEFAULT_MAX_WAIT: Duration = Duration::from_secs(24 * 60 * 60);

/// 同步写（[`ScriptSession::settle`]）判定「安静」的窗口。
///
/// 120ms 是按「本地 PTY 回显 + 一次局域网往返」量出来的：再小会把一次回显
/// 拆成两段（等于没等），再大就只是在脚本里白等。这个数**不承诺**远端已处理——
/// 要确定性就用 `wait` / `wait_idle`，同步写只是把「写完立刻 read」的那个
/// 漏读窗口从 0 收到 120ms。
const SETTLE_QUIET: Duration = Duration::from_millis(120);

/// 同步写最多等多久（一次 `write` 的收尾上限）。
///
/// 写本身早就成功了，收尾等不到安静不该让脚本卡住：输出洪水的场合
/// 「安静」永远不会来，没有上限的话每次 write 都要吃掉整个沙箱预算。
const SETTLE_CAP: Duration = Duration::from_secs(2);

/// 把 [`ScriptSession::settle`] 的安静窗口暴露给工作区（同步写开关在那边）。
pub(crate) fn settle_quiet() -> Duration {
    SETTLE_QUIET
}

/// 取一把锁；**中毒时照常取回里面的数据**，不 panic。
///
/// 中毒只说明「曾经有线程持锁时 panic 过」，终端的字节流与捕获游标都没有
/// 需要回滚的一致性约束。脚本线程不该因为别人崩过就 abort 整个 GUI。
/// 构造期（还没有 `self`）、会话锁、捕获锁三处共用这一份写法，不留
/// 「锁中毒就 expect」的第二条路径。
fn lock_or_recover<T: ?Sized>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    match m.lock() {
        Ok(guard) => guard,
        Err(poisoned) => poisoned.into_inner(),
    }
}

/// `read` / `read_until` / `wait_any` / `await` 族的共享捕获窗口。
///
/// - `reader`：**构造时**登记（§B.13 先登记后消费；登记前输出不入账）。
/// - `cursor`：[`ScanCursor`] 双指针——`scan` 供 probe（触发器式、不消费），
///   `consume` 供 take（脚本式、命中即消费）；保尾裁剪一处定义，不再有
///   `raw` + `mid_line` 两套并行状态。过滤器是结果后处理，不碰这套指针。
struct Capture {
    reader: Option<Box<dyn OutputReader>>,
    cursor: ScanCursor,
}

/// 跨 chunk 窗口的保尾字节数。有界模式由 `Pattern::window_hint` 精确决定
/// 需要多少字节；无界模式（`a.*b`）只能按这个固定上限截断。
const CAPTURE_CAP: usize = 64 * 1024;

impl Capture {
    /// `reader` 由 [`ScriptSession`] 在**构造时**登记（§B.13 先登记后消费的
    /// 最早时点）；登记失败（会话已死）时留 `None`，由首次读取显式报
    /// `Closed`，不在这里吞掉。
    fn with_reader(reader: Option<Box<dyn OutputReader>>) -> Self {
        Self {
            reader,
            cursor: ScanCursor::new(CAPTURE_CAP),
        }
    }

    /// 追加新到文本并做保尾裁剪。
    fn push_new(&mut self, text: &str) {
        self.cursor.push(text);
    }

    fn unconsumed(&self) -> &str {
        self.cursor.unconsumed()
    }

    /// 取已登记的读取器。调用前应已走过 [`ScriptSession::ensure_reader`]，
    /// 但**不用 `expect` 表达这个前提**：登记器缺失是完全可达的运行时状态
    /// （构造时登记就被 `Closed` 拒过，见 [`Capture::with_reader`]），
    /// 而脚本原语报错能让人改脚本、GUI abort 不能（门禁 4p）。
    fn reader_mut(&mut self) -> Result<&mut Box<dyn OutputReader>, PrimitiveError> {
        self.reader.as_mut().ok_or(PrimitiveError::Closed)
    }
}

/// `wait_any` 的命中结果（对标 CRT `WaitForStrings` / `MatchIndex`）。
///
/// `index` 从 **1** 起（0 保留给超时，但本项目改为显式报错，见
/// [`ScriptSession::wait_any`]）；`text` 是过滤视图里自捕获游标到命中串
/// 结尾的文本（含命中串）。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WaitAnyHit {
    pub index: usize,
    pub text: String,
}

/// 脚本驱动的会话句柄：`Arc<Mutex<dyn Session>>` 的薄包装，可克隆、
/// 跨线程共享（Send + Sync）。
///
/// 协议无关（ADR-0019）：只认 [`Session`] trait，不出现
/// SSH / PTY 等任何协议专有分支。
#[derive(Clone)]
pub struct ScriptSession {
    inner: Arc<Mutex<dyn Session>>,
    /// `read` / `read_until` / `wait_any` 的共享捕获游标（所有克隆共享同一份）。
    capture: Arc<Mutex<Capture>>,
    /// 消费式读原语的**逐会话序列化门**（所有克隆共享同一把）。
    ///
    /// 脚本与 API 两个前端共用一份游标后，两次 `read_until` 若并发推进同一个
    /// `ScanCursor`，两段文本会被谁先醒来谁拿走，调用方无法预测自己读到哪一段。
    /// 这里把「检查未消费段 → 等新数据 → 推进游标」整段串成一次临界区：
    /// 第二个前端排队等第一个取完（或超时）再读。不排队、不抢占，语义确定。
    read_gate: Arc<Mutex<()>>,
    /// **绝对**墙钟截止点（构造时 = 现在 + 预算）。所有阻塞原语都把请求的
    /// 超时收成 `deadline - now`，于是多次等待共享同一份预算——一次等掉多少，
    /// 后面的就少多少（不再是每次都按整份预算重新钳一遍）。
    deadline: Instant,
    /// 取消令牌（ADR-0027）。`None` = 这次运行没接取消（老 API 与单测）；
    /// 有它时每个阻塞点都走 `*_cancellable`，50ms 切片看一次。
    cancel: Option<CancelToken>,
}

impl ScriptSession {
    /// 包装一个会话。预算取 [`DEFAULT_MAX_WAIT`]，截止点从现在起算。
    pub fn new(session: Arc<Mutex<dyn Session>>) -> Self {
        Self::with_max_wait(session, DEFAULT_MAX_WAIT)
    }

    /// 同 [`new`](Self::new)，但显式指定预算。截止点在**构造时**定死，不是
    /// 每次等待重新算：这是独立 API（没有工作区）的入口，工作区走
    /// [`with_deadline`](Self::with_deadline)，让所有会话共享同一个截止点。
    pub fn with_max_wait(session: Arc<Mutex<dyn Session>>, max_wait: Duration) -> Self {
        Self::with_cancel(session, max_wait, None)
    }

    /// 带取消令牌的一档（ADR-0027）：工作区从 `HostEnv` 取出令牌交给每个
    /// `ScriptSession`，于是任何会话上的 `wait` / `read` 都能被「停止」打断。
    pub fn with_cancel(
        session: Arc<Mutex<dyn Session>>,
        max_wait: Duration,
        cancel: Option<CancelToken>,
    ) -> Self {
        let now = Instant::now();
        // `Instant + Duration` 在极端超长预算上会溢出 panic；生产路径不用 panic
        // 表达「预算不合理」，溢出就当作已经到期（立即超时）。
        let deadline = match now.checked_add(max_wait) {
            Some(when) => when,
            None => now,
        };
        Self::with_deadline(session, deadline, cancel)
    }

    /// 工作区入口：把**同一个**绝对截止点交给这条会话（新开 / 切绑的会话也
    /// 继承它）。这样两次 25s 等待在 30s 预算下不会变成 50s——第二次只拿到
    /// 剩下的 5s。
    ///
    /// **构造即登记**捕获器（§B.13「先登记后消费」的最早时点）：脚本在任何
    /// `write` 之前就已挂到输出流上。若延迟到首次 `read` / `read_until` 才登记，
    /// `write("ls\n")` 与首次读取之间到达的输出会落在登记点之前而永久丢失——
    /// 真实脚本会因此漏数据，e2e 也曾因此偶发 10s 超时。登记失败不在这里报错
    /// （会话可能已死），留给首次读取显式报 `Closed`。
    pub(crate) fn with_deadline(
        session: Arc<Mutex<dyn Session>>,
        deadline: Instant,
        cancel: Option<CancelToken>,
    ) -> Self {
        let reader = {
            let guard = lock_or_recover(&session);
            guard.reader_register().ok()
        };
        Self {
            inner: session,
            capture: Arc::new(Mutex::new(Capture::with_reader(reader))),
            read_gate: Arc::new(Mutex::new(())),
            deadline,
            cancel,
        }
    }

    /// 共享同一会话与同一份捕获游标，只把墙钟截止点与取消令牌换成这次运行的。
    ///
    /// 登记处里的那份 `ScriptSession` 长期驻留（远程前端也在用），不能替某一次
    /// 脚本运行背一个会过期的截止点；工作区每次取句柄时用本方法派生一份带
    /// 本次运行策略的视图。`inner` / `capture` / `read_gate` 都是 `Arc` 克隆，
    /// 所以换的只是策略，游标仍是同一份实例。
    pub(crate) fn for_run(&self, deadline: Instant, cancel: Option<CancelToken>) -> Self {
        Self {
            inner: Arc::clone(&self.inner),
            capture: Arc::clone(&self.capture),
            read_gate: Arc::clone(&self.read_gate),
            deadline,
            cancel,
        }
    }

    /// 等一次已登记的等待（有令牌就切片，没有就直接等）。
    fn await_registration(
        &self,
        reg: Box<dyn term_core::session_runtime::WaitRegistration>,
        timeout: Duration,
    ) -> Result<term_core::session_runtime::WaitMatch, PrimitiveError> {
        match &self.cancel {
            Some(cancel) => reg.wait_cancellable(timeout, cancel).map_err(Into::into),
            None => reg.wait(timeout).map_err(Into::into),
        }
    }

    /// 从捕获器取一段（有令牌就切片）。
    fn collect_reader(
        &self,
        reader: &mut Box<dyn term_core::session_runtime::OutputReader>,
        timeout: Duration,
    ) -> Result<String, PrimitiveError> {
        match &self.cancel {
            Some(cancel) => reader.collect_cancellable(timeout, cancel).map_err(Into::into),
            None => reader.collect(timeout).map_err(Into::into),
        }
    }

    /// `write(text)`：把 `text` 的字节**原样**写入会话，不自动追加换行
    /// （需要换行由脚本自己拼 `\n`，ZOC / CRT 同款语义）。
    ///
    /// 会话已结束时显式报错——脚本对着死会话发字节是 bug，不静默吞掉。
    pub fn write(&self, text: &str) -> Result<(), PrimitiveError> {
        let guard = self.guard();
        if matches!(guard.state(), SessionState::Finished(_)) {
            return Err(PrimitiveError::Closed);
        }
        guard.write(text.as_bytes());
        Ok(())
    }

    /// `wait(pattern, timeout_secs)`：等待清洗文本里出现 `pattern`。
    ///
    /// 命中返回「自登记点到模式结尾」的清洗文本（含模式本身，
    /// [`term_core::session_runtime::WaitMatch::text_upto`]）；超时 / 会话
    /// 结束显式失败。实现走 [`Session::wait_register`]（先登记后消费，
    /// 登记 ack 先于后续输出被匹配），绝不轮询屏幕。
    ///
    /// `wait` 不加过滤器——它是同步原语；需要过滤时用 [`read_until`]。
    pub fn wait(&self, pattern: &str, timeout_secs: f64) -> Result<String, PrimitiveError> {
        if pattern.is_empty() {
            return Err(PrimitiveError::EmptyPattern);
        }
        let timeout = self.effective_timeout(timeout_secs);
        // 先登记后消费：wait_register 返回（ack 到手）即生效。阻塞等待前
        // 放锁，不挡住 UI / 桥接等其他客户端并发使用会话。
        let reg = {
            let guard = self.guard();
            guard.wait_register(pattern)?
        };
        let m = self.await_registration(reg, timeout)?;
        Ok(m.text_upto)
    }

    /// `read_until(delimiter, timeout_secs[, filter])`：从捕获游标往后读到
    /// `delimiter` 首次出现（**含 delimiter**），游标推进到模式结尾——
    /// 模式之后的尾部文本不丢，留给后续 [`read`](Self::read) /
    /// `read_until` 续读（与 [`wait`](Self::wait) 的区别：wait 是一次性
    /// 同步（自登记点），read_until 是游标续读取数）。
    ///
    /// `filter` 是行级类 grep 过滤器（[`LineFilter`]）：**结果后处理**——匹配、
    /// 消费、缓冲与无过滤完全同一条路，过滤只清理本次返回文本的行（2026-11-19
    /// 用户裁定）。所以 p 可以命中在将被滤掉的行里，返回文本可能不含命中串。
    /// **超时显式失败**（[`PrimitiveError::Timeout`]），游标不动、文本不丢。
    /// 空 delimiter 是脚本 bug，直接拒绝。
    pub fn read_until(
        &self,
        delimiter: &str,
        timeout_secs: f64,
        filter: Option<&LineFilter>,
    ) -> Result<String, PrimitiveError> {
        self.read_until_with(delimiter, timeout_secs, filter).map(|(text, _)| text)
    }

    /// 同 [`read_until`](Self::read_until)，但连命中区间一起返回（供 `await` 用）。
    pub(crate) fn read_until_with(
        &self,
        delimiter: &str,
        timeout_secs: f64,
        filter: Option<&LineFilter>,
    ) -> Result<(String, term_core::automation::Span), PrimitiveError> {
        // 两个前端共用同一份游标：整段读要串行，否则「谁先拿到哪段文本」
        // 取决于线程调度。门在整个等待期间持有，第二个调用自然排队。
        let _gate = lock_or_recover(&self.read_gate);
        if delimiter.is_empty() {
            return Err(PrimitiveError::EmptyPattern);
        }
        // 分隔符按字面量编译：旧实现是 `str::find`，元字符本就不生效；
        // 现在统一走 `Pattern`，与 `re()` / 触发器同一编译器（收尾）。
        let pat = Pattern::compile(delimiter, Syntax::Literal, true)
            .map_err(|_| PrimitiveError::EmptyPattern)?;
        let timeout = self.effective_timeout(timeout_secs);
        let deadline = Instant::now() + timeout;
        loop {
            let mut cap = self.capture_guard();
            self.ensure_reader(&mut cap)?;
            // ① 现有未消费文本先试匹配（含上次留下的尾部）。find 不消费。
            if let Some((hit, span)) = cap.cursor.take_text(&pat) {
                // take_text 已推进 consume 到命中结尾；过滤只作用于返回文本。
                return Ok((filter.map(|f| f.apply(&hit)).unwrap_or(hit), span));
            }
            // ② 没命中 → 等新数据（阻塞至多 remaining；事件通知，不轮询）。
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                // 超时显式失败；游标不动，已到文本下次调用还在。
                return Err(PrimitiveError::Timeout);
            }
            let new = self.collect_reader(cap.reader_mut()?, remaining)?;
            if !new.is_empty() {
                cap.push_new(&new);
            } else {
                // collect 空返 = 它的超时到期 ≈ 我们的 deadline。
                return Err(PrimitiveError::Timeout);
            }
        }
    }

    /// `wait_any([patterns], timeout_secs[, filter])`：等任意模式出现，
    /// 返回**最先出现**的那个的索引（1 起，对标 CRT
    /// `WaitForStrings` / `MatchIndex`）。同一位置多个模式命中时取数组里
    /// 靠前的（先登记优先，ZOC `ZocWaitMux` 同款）。
    ///
    /// 与 [`read_until`](Self::read_until) 共用捕获游标：命中后游标推进到
    /// 命中串结尾，之后的文本不丢，留给 [`read`](Self::read) /
    /// `read_until` 续读。`filter` 是**结果后处理**，不参与匹配。
    ///
    /// **超时显式失败**（[`PrimitiveError::Timeout`]），游标不动、文本不丢。
    /// 这里有意偏离 CRT 的「索引 0 = 超时」：0 太容易被脚本忘判而
    /// 静默继续（ADR-0019「禁止静默」），显式抛错强制脚本处理超时分支。
    /// 空模式列表、或列表中含空串，都是脚本 bug，直接拒绝。
    pub fn wait_any(
        &self,
        patterns: &[String],
        timeout_secs: f64,
        filter: Option<&LineFilter>,
    ) -> Result<WaitAnyHit, PrimitiveError> {
        self.wait_any_with(patterns, timeout_secs, filter)
            .map(|(hit, _)| hit)
    }

    /// 同 [`wait_any`](Self::wait_any)，但连命中区间一起返回（供 `await_any` 用）。
    pub(crate) fn wait_any_with(
        &self,
        patterns: &[String],
        timeout_secs: f64,
        filter: Option<&LineFilter>,
    ) -> Result<(WaitAnyHit, term_core::automation::Span), PrimitiveError> {
        // 与 read_until 同一把门：两个前端不会同时推进同一个游标。
        let _gate = lock_or_recover(&self.read_gate);
        if patterns.is_empty() || patterns.iter().any(|p| p.is_empty()) {
            return Err(PrimitiveError::EmptyPattern);
        }
        let compiled: Vec<Pattern> = patterns
            .iter()
            .map(|p| Pattern::compile(p, Syntax::Literal, true).map_err(|_| PrimitiveError::EmptyPattern))
            .collect::<Result<_, _>>()?;
        let timeout = self.effective_timeout(timeout_secs);
        let deadline = Instant::now() + timeout;
        loop {
            let mut cap = self.capture_guard();
            self.ensure_reader(&mut cap)?;
            // probe 不消费：各模式在「未消费窗口」里独立 find，取最早命中；
            // 同位置保留先登记的（严格 `<` on start，原语义不变）。
            let mut best: Option<(usize, usize, term_core::automation::Span)> = None;
            for (i, p) in compiled.iter().enumerate() {
                if let Some(span) = p.find(cap.unconsumed(), 0) {
                    let earlier = match &best {
                        None => true,
                        Some((bs, _, _)) => span.start < *bs,
                    };
                    if earlier {
                        best = Some((span.start, i, span));
                    }
                }
            }
            if let Some((_start, idx, span)) = best {
                let end = span.end;
                let hit_text = cap.unconsumed()[..end].to_owned();
                // 直接推进 consume 到命中结尾（相对未消费窗口的偏移），
                // 不用 take 重新查找——probe 与推进同一基准。
                let base = cap.cursor.consume_offset();
                cap.cursor.consume_to(base + end);
                let hit = WaitAnyHit {
                    index: idx + 1,
                    text: filter.map(|f| f.apply(&hit_text)).unwrap_or(hit_text),
                };
                return Ok((hit, span));
            }
            // 没命中 → 等新数据（事件通知，不轮询）。
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return Err(PrimitiveError::Timeout);
            }
            let new = self.collect_reader(cap.reader_mut()?, remaining)?;
            if !new.is_empty() {
                cap.push_new(&new);
            } else {
                return Err(PrimitiveError::Timeout);
            }
        }
    }

    /// `read([timeout_secs[, filter]])`：**排水**——取走自上一次取走点
    /// （捕获游标）以来的全部输出，游标推进到末尾。
    ///
    /// 有积累（含上次 read_until 留下的尾部）立即返回；没有积累时等到
    /// 有新数据或 `timeout_secs` 到期。**超时无数据返回空串**（排水不是
    /// 等待事实，超时不算失败——与 [`read_until`](Self::read_until) 的
    /// 超时显式失败是两种承诺）；会话死透显式报 [`PrimitiveError::Closed`]。
    ///
    /// `filter` 语义同 `read_until`：行级、作用于返回之前；不完整尾行
    /// 与已判行的延续段按原样透传。被过滤掉的行视为已排（游标照常推进，
    /// 不会在下次 read 里复活）。
    pub fn read(
        &self,
        timeout_secs: f64,
        filter: Option<&LineFilter>,
    ) -> Result<String, PrimitiveError> {
        let _gate = lock_or_recover(&self.read_gate);
        let timeout = self.effective_timeout(timeout_secs);
        let mut cap = self.capture_guard();
        self.ensure_reader(&mut cap)?;
        // 有存量就不等（排水）；没存量才等新数据。
        let wait = if cap.unconsumed().is_empty() {
            timeout
        } else {
            Duration::ZERO
        };
        let new = self.collect_reader(cap.reader_mut()?, wait)?;
        if !new.is_empty() {
            cap.push_new(&new);
        }
        // drain 一次取走全部未消费文本并推进游标；过滤只清理返回文本，
        // 不碰缓冲与游标（被滤掉的行已随 drain 正常离开，不会复活）。
        let text = cap.cursor.drain();
        Ok(filter.map(|f| f.apply(&text)).unwrap_or(text))
    }

    /// `screen()`：屏幕全文（[`term_core::grid::GridSnapshot::screen_text`]）。
    /// 会话结束到没有事实可取时显式报错。
    pub fn screen(&self) -> Result<String, PrimitiveError> {
        let guard = self.guard();
        match guard.screen() {
            Some(snap) => Ok(snap.screen_text()),
            None => Err(PrimitiveError::Closed),
        }
    }

    /// `region(r1, c1, r2, c2[, lines])`：屏幕矩形（1 起、含端点、越界裁剪）。
    ///
    /// `lines = true` 时行间用 `\n`（`region_lines`），否则 `\r\n`
    /// （CRT `Screen.Get` 的形状）。纯读取：不消费输出、不推进游标。
    pub fn region(
        &self,
        row1: usize,
        col1: usize,
        row2: usize,
        col2: usize,
        lines: bool,
    ) -> Result<String, PrimitiveError> {
        let guard = self.guard();
        let Some(snap) = guard.screen() else {
            return Err(PrimitiveError::Closed);
        };
        Ok(if lines {
            snap.region_lines(row1, col1, row2, col2)
        } else {
            snap.region_text(row1, col1, row2, col2)
        })
    }

    /// `keys(names)`：按**键名**发送按键，返回写入的字节数。
    ///
    /// 与 [`write`](Self::write) 的分工是语义而不是风格：`write("ls\n")` 送的是
    /// 一串字节，`keys(["ls", "enter"])` 送的是「按下这几个键」——后者要知道
    /// `enter` 是 CR、`F5` 是 `ESC[15~`、方向键要不要按 `DECSET 1` 发 `ESC OA`。
    /// 映射表在 [`term_core::keys`]（与界面那条路是同一份）。
    ///
    /// **整批翻译完才写**：半个按键列表发出去之后才报「第 3 个键不认识」，
    /// 会让程序收到一串缺了尾巴的键——比整批不发更糟。
    pub fn keys(&self, names: &[&str]) -> Result<usize, PrimitiveError> {
        if names.is_empty() {
            return Err(PrimitiveError::BadArgument("keys 至少要给一个键名"));
        }
        // 只有方向键受 `DECSET 1` 影响；没有方向键就不去问屏幕（少一次往返）。
        let app_cursor = if names.iter().any(|name| term_core::keys::is_arrow(name)) {
            self.guard()
                .screen()
                .is_some_and(|snap| snap.application_cursor_keys)
        } else {
            false
        };
        let mut bytes = String::new();
        for name in names {
            match term_core::keys::key_bytes(name, app_cursor) {
                Some(text) => bytes.push_str(&text),
                None => return Err(PrimitiveError::UnknownKey((*name).to_string())),
            }
        }
        let guard = self.guard();
        if matches!(guard.state(), SessionState::Finished(_)) {
            return Err(PrimitiveError::Closed);
        }
        guard.write(bytes.as_bytes());
        Ok(bytes.len())
    }

    /// `wait_idle(quiet_secs, timeout_secs)`：读到**输出停下来**为止。
    ///
    /// 判据是「连续 `quiet` 时间没有新数据」（事件通知，不是轮询屏幕）。返回
    /// 自捕获游标到安静时刻的全部文本，游标推进到结尾——与 [`read`](Self::read)
    /// 一样是**消耗式**的流式动词，差别只在什么时候停。
    ///
    /// 与 [`wait`](Self::wait) 是两种范式：wait 等一个**已知的**事实（模式），
    /// wait_idle 等「没事了」。命令提示符不确定、只知道命令会结束的场合用它。
    ///
    /// `timeout` 内始终没安静下来 → [`PrimitiveError::Timeout`]，游标不动。
    pub fn wait_idle(
        &self,
        quiet_secs: f64,
        timeout_secs: f64,
    ) -> Result<String, PrimitiveError> {
        let _gate = lock_or_recover(&self.read_gate);
        let quiet = self.effective_timeout(quiet_secs);
        let timeout = self.effective_timeout(timeout_secs);
        let deadline = Instant::now() + timeout;
        let mut cap = self.capture_guard();
        self.ensure_reader(&mut cap)?;
        loop {
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                // 输出一直没停：游标不动，已到的文本下次还在。
                return Err(PrimitiveError::Timeout);
            }
            // 单次最多等一个安静窗口：等到空串 = 安静了。
            let slice = quiet.min(remaining);
            let new = self.collect_reader(cap.reader_mut()?, slice)?;
            if new.is_empty() {
                // 空串有两种含义，必须分开：窗口走完 = 真安静；窗口被总上限
                // 截短 = 还没安静就先到期了。混成一种会把「一直在输出」报成
                // 「已经停了」——比超时更难查。
                if slice < quiet {
                    return Err(PrimitiveError::Timeout);
                }
                break;
            }
            cap.push_new(&new);
        }
        let view = cap.cursor.drain();
        Ok(view)
    }

    /// 同步写的收尾：写到「安静」为止，但**不推进游标**。
    ///
    /// 与 [`wait_idle`](Self::wait_idle) 的唯一区别就是这一条：这里收下的文本
    /// 留在未消费段里，脚本紧接着的 `read` 还能取到。语义上是「把已经在路上的
    /// 输出先收进来，顺手等它停」，不是一次读取。
    ///
    /// 尽力而为：安静窗口等不到就返回（写已经成功了，等不到安静不是写的失败），
    /// 会话结束也直接返回。上限 [`SETTLE_CAP`] 保证不会把脚本卡在这里。
    pub(crate) fn settle(&self, quiet: Duration) {
        // 收下的文本留在未消费段里；若此时另一前端正在读，先把门让给它。
        let _gate = lock_or_recover(&self.read_gate);
        let mut cap = self.capture_guard();
        // 收尾也不能把脚本推过总截止点：取「固定收尾上限」与「剩余预算」的较小者。
        let cap_secs = self.deadline.saturating_duration_since(Instant::now());
        let deadline = Instant::now() + cap_secs.min(SETTLE_CAP);
        loop {
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return;
            }
            // 未登记 / 会话已死：写出去的已经是最后一句，没什么可等的。
            let Ok(reader) = cap.reader_mut() else {
                return;
            };
            match self.collect_reader(reader, quiet.min(remaining)) {
                Ok(text) if text.is_empty() => return,
                Ok(text) => cap.push_new(&text),
                Err(_) => return,
            }
        }
    }

    /// 把挂起的事件并进会话，再返回日志状态。
    ///
    /// # 为什么必须 poll 一次
    ///
    /// `Session::log_status` 在实现层是**缓存读**：监管者线程把
    /// `Event::Log` 塞进队列，只有 `poll()` 抽事件时才会写进那个字段。
    /// 界面每帧都在 poll，所以窗口里看到的是准的；脚本与 JSON-RPC 客户端
    /// 从不 poll——它们直接读缓存就会**永远停在构造时的 `Off`**。
    ///
    /// 实测（2026-11-14）：脚本 `log_start` 之后文件已经建出来了，
    /// `log_status()` 仍报 `off`，`log_stop` 于是误报「没在记录」。
    /// 修法就是这里：日志相关原语在判定前先抽一次事件。
    ///
    /// 副作用是可接受的：`poll()` 只是把字节/事件并进本会话的缓存，
    /// 与界面调用它完全一样，不消费输出、不推进任何游标。
    fn refresh(&self) {
        let mut guard = self.guard();
        guard.poll();
    }

    /// 取会话锁；**中毒时照常取回数据**，不 panic。理由见 [`lock_or_recover`]。
    fn guard<'a>(&'a self) -> MutexGuard<'a, dyn Session + 'static> {
        lock_or_recover(&self.inner)
    }

    /// 取捕获锁；与 [`guard`](Self::guard) 同一口径。
    ///
    /// 2026-11-15 前 `read` / `read_until` / `wait_any` / `wait_idle` /
    /// `settle` 五处各自对捕获锁 expect：任何别的线程在持有捕获锁时
    /// panic 过，之后每一条读原语都会把 GUI 一起带走。
    fn capture_guard(&self) -> MutexGuard<'_, Capture> {
        lock_or_recover(&self.capture)
    }

    /// `cursor_pos()`：光标所在 `(行, 列)`，均 **0 起**（与
    /// [`term_core::grid::GridSnapshot::cursor`] 同一口径）。
    ///
    /// 与 `screen` 的区别是取什么：那个给整屏文本，这个只给位置。
    /// 脚本用它在「屏幕上的某个位置」定位后续写操作的参考系；远程客户端
    /// 从 `screen` 的结果字段就能拿到同样的两个数，所以本动词只对脚本
    /// 可见（ADR-0023 §决策 1）。
    pub fn cursor_pos(&self) -> Result<(usize, usize), PrimitiveError> {
        let guard = self.guard();
        match guard.screen() {
            Some(snap) => Ok(snap.cursor),
            None => Err(PrimitiveError::Closed),
        }
    }

    /// `terminal_size()`：终端 `(列, 行)`。
    ///
    /// 脚本用它判断会话是否被 resize 过——换行位置与截断都随之变，
    /// 硬编码的列宽推算会静默错位。
    pub fn terminal_size(&self) -> Result<(usize, usize), PrimitiveError> {
        let guard = self.guard();
        match guard.screen() {
            Some(snap) => Ok((snap.cols, snap.rows)),
            None => Err(PrimitiveError::Closed),
        }
    }

    /// `log_start(dir, session)`：开始把会话输出记到 `dir` 下。
    ///
    /// 文件名由会话侧决定（时间戳 + 重名序号），脚本**不**指定文件名——
    /// 那样两条并发日志会互相覆盖。已经在记时显式报
    /// [`PrimitiveError::AlreadyRecording`] 而不是静默换文件：调用方以为
    /// 「换了个新文件」，实际旧文件被截断，是最坏的一种静默。
    pub fn log_start(&self, dir: &str, session: &str) -> Result<(), PrimitiveError> {
        // 脚本面没有「设置页」：用出厂模板（文件名 / 行格式都按视觉基线）。
        // 要自定义就走 `log_start_with`——不在这里偷偷读 `prefs.json`，
        // 那会让同一句脚本在两台机器上写出不同格式的日志。
        let opts = term_core::log_opts::LogOptions::default();
        self.log_start_with(dir, session, &opts)
    }

    /// 带选项的入口（桥接 / agent 用）。背后是同一个用户在设置页里配过的
    /// 那份 `LogOptions`——无头调用不该写出另一种格式的日志。
    /// 带宿主名的一档（ADR-0027）：`%{host}` 只在真的知道主机时才有值。
    pub fn log_start_full(
        &self,
        dir: &str,
        session: &str,
        host: &str,
        opts: &term_core::log_opts::LogOptions,
    ) -> Result<(), PrimitiveError> {
        self.refresh();
        let guard = self.guard();
        if matches!(guard.state(), SessionState::Finished(_)) {
            return Err(PrimitiveError::Closed);
        }
        if guard.log_status().is_recording() {
            return Err(PrimitiveError::AlreadyRecording);
        }
        guard.start_log_with_host(std::path::PathBuf::from(dir), session, host, opts);
        Ok(())
    }

    pub fn log_start_with(
        &self,
        dir: &str,
        session: &str,
        opts: &term_core::log_opts::LogOptions,
    ) -> Result<(), PrimitiveError> {
        self.refresh();
        let guard = self.guard();
        if matches!(guard.state(), SessionState::Finished(_)) {
            return Err(PrimitiveError::Closed);
        }
        if guard.log_status().is_recording() {
            return Err(PrimitiveError::AlreadyRecording);
        }
        guard.start_log(std::path::PathBuf::from(dir), session, opts);
        Ok(())
    }

    /// `log_stop()`：停止记录并关闭文件。
    ///
    /// 没在记时显式报 [`PrimitiveError::NotRecording`]——空成功会让调用方
    /// 以为文件已经收尾，而它可能根本没开过。
    pub fn log_stop(&self) -> Result<(), PrimitiveError> {
        self.refresh();
        let guard = self.guard();
        if !guard.log_status().is_recording() {
            return Err(PrimitiveError::NotRecording);
        }
        guard.stop_log();
        Ok(())
    }

    /// `log_status()`：日志阶段与文件路径。
    pub fn log_status(&self) -> Result<LogStatus, PrimitiveError> {
        self.refresh();
        let guard = self.guard();
        if matches!(guard.state(), SessionState::Finished(_)) && !guard.log_status().is_recording() {
            // 会话已结束且没在记：没有状态可报，但也不该假装「off 就是正常」——
            // 调用方问的是「这条会话现在记不记」，会话都没了就没有答案。
            return Err(PrimitiveError::Closed);
        }
        Ok(guard.log_status())
    }

    /// `state()`：生命周期状态。
    ///
    /// 与「跑原语炸 `Closed`」的区别是用处：那个是**事后**发现会话死了，
    /// 这个是**事前**判断——脚本想在断线时改走离线分支，就得先问一句。
    pub fn state(&self) -> SessionState {
        self.guard().state()
    }

    /// 这条会话的 id。工作区用它在建表时给当前绑定定 key（不额外传 id，
    /// 免得出现「传进来的 id 和会话本身对不上」）。
    pub fn id(&self) -> SessionId {
        self.guard().id()
    }

    /// `facets()`：该会话暴露的能力 facet。
    ///
    /// 空表不是错误（本地 shell 就没有文件传输）；返回的是
    /// [`FacetKind`] 的线上名字，与能力面 `facets` 结果的
    /// enum 一字不差，脚本与远程判同一组字符串。
    pub fn facets(&self) -> Vec<&'static str> {
        self.guard()
            .facets()
            .into_iter()
            .map(|kind| match kind {
                FacetKind::FileTransfer => "file_transfer",
                FacetKind::PortForward => "port_forward",
            })
            .collect()
    }

    /// 兜底登记：正常路径下 [`with_deadline`](Self::with_deadline) 已在构造时
    /// 登记成功，只有构造时会话已死（`reader = None`）才会走到这里，此时
    /// 显式报 `Closed`。锁顺序：capture → inner（唯一顺序，无反向路径）。
    fn ensure_reader(&self, cap: &mut Capture) -> Result<(), PrimitiveError> {
        if cap.reader.is_none() {
            let guard = self.guard();
            cap.reader = Some(guard.reader_register()?);
        }
        Ok(())
    }

    /// 把脚本传的超时收成合法 [`Duration`]：NaN / 负数 → 0，超过**剩余墙钟**
    /// → 剩余量。上限来自构造时定死的绝对截止点（每次现算 `deadline - now`），
    /// 因此整段脚本的所有等待共享一份预算；截止点已过 → 0（显式超时，不是
    /// 拿一份新的整预算继续等）。
    fn effective_timeout(&self, timeout_secs: f64) -> Duration {
        let capped = self
            .deadline
            .saturating_duration_since(Instant::now())
            .as_secs_f64();
        let secs = if timeout_secs.is_nan() {
            0.0
        } else {
            timeout_secs.clamp(0.0, capped)
        };
        Duration::from_secs_f64(secs)
    }
}

/// 原语单测：假会话（`testutil`）驱动，覆盖命中 / 超时 / 时序 / 过滤 /
/// 游标衔接 / 显式失败。
#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::fake_session;
    use term_core::grid::GridSnapshot;
use term_core::session_runtime::{FacetKind, SessionId, WaitError, WaitRegistration};

    /// 构造即已登记捕获器；这里做一次空 `read` 断言初始为空，顺带覆盖
    /// `read` 在无新数据时返回空串的契约。
    fn prime(s: &ScriptSession) {
        assert_eq!(s.read(0.0, None).unwrap(), "");
    }

    #[test]
    fn lock_or_recover_reads_through_a_poisoned_mutex() {
        // 直接钉住 helper：中毒后仍能取回里面的数据，而不是把 GUI 带走。
        let m = std::sync::Arc::new(std::sync::Mutex::new(41));
        let p = std::sync::Arc::clone(&m);
        let joined = std::thread::spawn(move || {
            let mut guard = p.lock().unwrap();
            *guard += 1;
            panic!("故意毒锁");
        })
        .join();
        assert!(joined.is_err(), "这条线程必须真的 panic 过");
        assert!(m.is_poisoned());
        assert_eq!(*lock_or_recover(&m), 42, "中毒后数据仍在、仍可读");
    }

    #[test]
    fn write_sends_bytes_verbatim_without_newline() {
        let (session, manip) = fake_session(1);
        let s = ScriptSession::new(session);
        s.write("echo hi").expect("write 不应失败");
        assert_eq!(manip.written(), b"echo hi".to_vec(), "不自动加换行");
        s.write("\n").expect("write 不应失败");
        assert_eq!(manip.written(), b"echo hi\n".to_vec(), "换行由脚本自己拼");
    }

    #[test]
    fn wait_hit_returns_text_upto_including_pattern_tail() {
        // 输出必须在登记**之后**到达（先登记后消费）：生产者线程延迟推。
        let (session, manip) = fake_session(1);
        let s = ScriptSession::new(session);
        let producer = {
            let manip = manip.clone();
            std::thread::spawn(move || {
                std::thread::sleep(std::time::Duration::from_millis(30));
                manip.push_output("banner\nPassword:");
            })
        };
        let got = s.wait("Password:", 5.0).expect("应命中");
        producer.join().expect("生产者");
        assert_eq!(got, "banner\nPassword:", "text_upto 含模式结尾");
    }

    #[test]
    fn wait_timeout_fails_explicitly() {
        let (session, _manip) = fake_session(1);
        let s = ScriptSession::new(session);
        let err = s.wait("never", 0.05).expect_err("超时必须显式失败");
        assert_eq!(err, PrimitiveError::Timeout);
    }

    #[test]
    fn wait_rejects_empty_pattern() {
        let (session, _manip) = fake_session(1);
        let s = ScriptSession::new(session);
        assert_eq!(s.wait("", 1.0).unwrap_err(), PrimitiveError::EmptyPattern);
    }

    #[test]
    fn read_until_returns_captured_text_including_delimiter() {
        let (session, manip) = fake_session(1);
        let s = ScriptSession::new(session);
        prime(&s);
        manip.push_output("line1\r\nEND###tail-after");
        let got = s.read_until("###", 5.0, None).expect("应捕获");
        assert_eq!(got, "line1\r\nEND###");
        assert!(got.contains("###"), "返回文本含 delimiter");
    }

    #[test]
    fn read_until_then_read_covers_the_tail_seamlessly() {
        // 共享游标：read_until 命中后游标停在模式结尾，尾部文本不丢，
        // read 无缝续读（此前 tail-after 会丢，现在不丢）。
        let (session, manip) = fake_session(1);
        let s = ScriptSession::new(session);
        prime(&s);
        manip.push_output("line1\nEND###tail-after");
        assert_eq!(s.read_until("###", 5.0, None).unwrap(), "line1\nEND###");
        assert_eq!(s.read(0.05, None).unwrap(), "tail-after");
    }

    #[test]
    fn read_until_timeout_fails_explicitly() {
        let (session, _manip) = fake_session(1);
        let s = ScriptSession::new(session);
        assert_eq!(
            s.read_until("###", 0.05, None).unwrap_err(),
            PrimitiveError::Timeout,
            "read_until 超时显式失败（与 read 的空串承诺不同）"
        );
    }

    #[test]
    fn read_until_rejects_empty_delimiter() {
        let (session, _manip) = fake_session(1);
        let s = ScriptSession::new(session);
        assert_eq!(
            s.read_until("", 1.0, None).unwrap_err(),
            PrimitiveError::EmptyPattern
        );
    }

    #[test]
    fn read_returns_new_data_when_it_arrives() {
        let (session, manip) = fake_session(1);
        let s = ScriptSession::new(session);
        prime(&s);
        manip.push_output("hello\n");
        assert_eq!(s.read(0.05, None).unwrap(), "hello\n");
    }

    #[test]
    fn output_before_the_first_read_is_captured_because_registration_is_eager() {
        // 根因回归：reader 在**构造时**就登记，`write` 之后、首次 `read_until`
        // 之前到达的输出不会丢。惰性登记会让这段输出落在登记点之前永久消失
        // ——QA 实测既有 e2e 因此 15 次里偶发 1 次 read_until 等满 10s 超时。
        // 这里构造后直接推输出，一次读取都还没调过。
        let (session, manip) = fake_session(1);
        let s = ScriptSession::new(session);
        manip.push_output("early BEFORE_FIRST_READ");
        let got = s
            .read_until("BEFORE_FIRST_READ", 1.0, None)
            .expect("构造后、首次读取前的输出必须被捕获");
        assert_eq!(got, "early BEFORE_FIRST_READ");
    }

    #[test]
    fn read_timeout_returns_empty_string_instead_of_failing() {
        // read 是排水不是等待事实：超时无数据返回空串，不报错。
        let (session, _manip) = fake_session(1);
        let s = ScriptSession::new(session);
        assert_eq!(s.read(0.05, None).unwrap(), "");
    }

    #[test]
    fn read_reports_closed_when_the_session_dies() {
        let (session, manip) = fake_session(1);
        let s = ScriptSession::new(session);
        prime(&s);
        manip.finish();
        assert_eq!(s.read(0.05, None).unwrap_err(), PrimitiveError::Closed);
    }

    #[test]
    fn grep_keeps_only_matching_lines() {
        let (session, manip) = fake_session(1);
        let s = ScriptSession::new(session);
        prime(&s);
        manip.push_output("keep1\ndrop\nkeep2\n");
        let f = LineFilter::grep("keep").unwrap();
        assert_eq!(s.read(0.05, Some(&f)).unwrap(), "keep1\nkeep2\n");
    }

    #[test]
    fn grep_v_drops_matching_lines() {
        let (session, manip) = fake_session(1);
        let s = ScriptSession::new(session);
        prime(&s);
        manip.push_output("noise\ndata\nnoise\n");
        let f = LineFilter::grep_v("^noise").unwrap();
        assert_eq!(s.read(0.05, Some(&f)).unwrap(), "data\n");
    }

    #[test]
    fn filtering_after_matching_hits_sentinels_in_noise_lines() {
        // 2026-11-19 用户裁定：过滤是结果后处理。哨兵在噪声行里**照样命中**，
        // 消费照常推进——这是本测试钉住的核心。返回文本止于命中串（无换行），
        // 按约定「最后不完整行透传不判」保留；grep 只清理完整行。
        let (session, manip) = fake_session(1);
        let s = ScriptSession::new(session);
        prime(&s);
        manip.push_output("noise SENTINEL here\n");
        let f = LineFilter::grep_v("^noise").unwrap();
        let got = s
            .read_until("SENTINEL", 0.5, Some(&f))
            .expect("噪声行里的哨兵也命中");
        assert_eq!(got, "noise SENTINEL", "命中和消费不受 grep 影响");
    }

    #[test]
    fn incomplete_tail_line_passes_the_filter_unfiltered() {
        // 整行未结束无法判定：不完整尾行按原样透传。
        let (session, manip) = fake_session(1);
        let s = ScriptSession::new(session);
        prime(&s);
        manip.push_output("noise\nnois");
        let f = LineFilter::grep_v("^noise").unwrap();
        assert_eq!(s.read(0.05, Some(&f)).unwrap(), "nois");
    }

    #[test]
    fn mid_line_continuation_is_not_refiltered() {
        // 游标停在行中：已判行的延续段透传、不回判（整行一次判定）。
        let (session, manip) = fake_session(1);
        let s = ScriptSession::new(session);
        prime(&s);
        manip.push_output("A###rest\nnoise\n");
        let f = LineFilter::grep_v("^noise").unwrap();
        assert_eq!(s.read_until("###", 0.05, Some(&f)).unwrap(), "A###");
        assert_eq!(s.read(0.05, Some(&f)).unwrap(), "rest\n");
    }

    #[test]
    fn filter_dropping_all_sentinel_lines_still_hits_because_filter_is_after_matching() {
        // 新语义：grep_v 只清理返回文本，不影响匹配。哨兵在将被丢的行里
        // 照样命中，返回文本被滤空。
        let (session, manip) = fake_session(1);
        let s = ScriptSession::new(session);
        prime(&s);
        manip.push_output("has END inside\nanother END here\n");
        let f = LineFilter::grep_v(".*END.*").unwrap();
        let got = s
            .read_until("END", 0.5, Some(&f))
            .expect("过滤在后，哨兵照常命中");
        assert_eq!(got, "has END", "命中在噪声行照常返回，grep 不参与匹配");
    }

    #[test]
    fn matching_is_on_raw_text_not_the_filtered_view() {
        // 新语义：匹配在原始文本上。命中串不能跨被丢的行拼接——"keepA" 与
        // "keepB END" 中间的 "noise\n" 真实存在，所以 "keepAkeepB" 不相邻。
        // 这里钉住：p 命中在原始行内，返回文本按 grep_v 清掉噪声行。
        let (session, manip) = fake_session(1);
        let s = ScriptSession::new(session);
        prime(&s);
        manip.push_output("keepA\nnoise\nkeepB END\nrest\n");
        let f = LineFilter::grep_v("^noise").unwrap();
        let got = s.read_until("END", 0.5, Some(&f)).expect("应命中");
        assert_eq!(
            got,
            "keepA\nkeepB END",
            "匹配原始文本、消费到命中结尾；返回文本滤掉 noise 行"
        );
        let tail = s.read(0.05, Some(&f)).expect("read 应取到剩余");
        assert_eq!(tail, "\nrest\n", "游标在 END 之后，不丢不重复");
    }

    #[test]
    fn all_lines_filtered_read_until_times_out_and_read_returns_empty() {
        // C：哨兵本身不在输出里 → read_until 超时；全行被滤 → read 返空串。
        let (session, manip) = fake_session(1);
        let s = ScriptSession::new(session);
        prime(&s);
        manip.push_output("noise1\nnoise2\nnoise3\n");
        let f = LineFilter::grep_v("^noise").unwrap();
        // read_until 找不到任何东西（视图为空）→ 超时。
        let err = s
            .read_until("anything", 0.05, Some(&f))
            .expect_err("全过滤光下不该命中");
        assert_eq!(err, PrimitiveError::Timeout);
        // read 有积累但全被过滤 → 返回空串（排水语义，不报错）。
        assert_eq!(s.read(0.05, Some(&f)).unwrap(), "", "全过滤光 → 空串");
    }

    #[test]
    fn flood_output_with_filter_does_not_defeat_matching() {
        // E：洪水 + 过滤——Capture 保尾裁剪与 filter_view 组合不误伤命中。
        let (session, manip) = fake_session(1);
        let s = ScriptSession::new(session);
        prime(&s);
        // 先推大量噪声（触发保尾裁剪），再推目标行。
        let noise: String = (0..2000).map(|i| format!("noise line {i}\n")).collect();
        manip.push_output(&noise);
        manip.push_output("keep TARGET_HERE done\n");
        let f = LineFilter::grep_v("^noise").unwrap();
        let got = s
            .read_until("TARGET_HERE", 5.0, Some(&f))
            .expect("洪水中过滤后仍须命中");
        assert!(
            got.contains("TARGET_HERE"),
            "裁剪+过滤不误伤命中：{got:?}"
        );
    }

    #[test]
    fn wait_and_read_coexist_without_interference() {
        // F：wait 登记（wait_register）与 read 游标（reader_register）并存
        // 互不干扰——wait 等同步模式，read 排水取数。
        let (session, manip) = fake_session(1);
        let s = ScriptSession::new(session);
        prime(&s);
        manip.push_output("data-line\n");
        // read 先取走 data-line。
        assert_eq!(s.read(0.05, None).unwrap(), "data-line\n");
        // wait 独立于 read 游标：从 wait_register 登记点开始匹配。
        let producer = {
            let manip = manip.clone();
            std::thread::spawn(move || {
                std::thread::sleep(std::time::Duration::from_millis(30));
                manip.push_output("PROMPT>");
            })
        };
        let got = s.wait("PROMPT>", 5.0).expect("wait 应独立命中");
        producer.join().expect("生产者");
        assert_eq!(got, "PROMPT>", "wait 不受 read 游标影响");
    }

    #[test]
    fn screen_returns_full_text() {
        let (session, manip) = fake_session(1);
        let s = ScriptSession::new(session);
        manip.set_screen("top line\nsecond line\nlast");
        assert_eq!(
            s.screen().expect("有屏"),
            "top line\nsecond line\nlast"
        );
    }

    // ── cursor_pos / terminal_size：省一次往返的那两个（`Exposure::Script`）──
    //
    // 这两个原语长期没有任何行为断言：全仓只有实现与能力面表提到它们，
    // 于是改 resize / 滚屏 / 重放历史时不会有任何测试变红。它们存在的**理由**
    // （远程从 `screen` 的结果字段就能拿到同样的两个数，单列只是多一次往返）
    // 也要求它们必须真的等于快照里的那两个数，否则省下来的往返是错的往返。

    #[test]
    fn terminal_size_follows_the_session_resize() {
        let (session, manip) = fake_session(1);
        let s = ScriptSession::new(session);
        // 没 resize 过：按屏幕内容推（"hi\nwide" → 4 列 2 行）。
        manip.set_screen("hi\nwide");
        assert_eq!(s.terminal_size().expect("有屏"), (4, 2));
        // resize 之后必须报新尺寸，而不是继续推文本宽度。
        manip.resize(120, 40);
        assert_eq!(s.terminal_size().expect("有屏"), (120, 40));
        // 缩回去也要跟着。
        manip.resize(80, 24);
        assert_eq!(s.terminal_size().expect("有屏"), (80, 24));
    }

    #[test]
    fn cursor_pos_reports_where_the_screen_cursor_is() {
        let (session, manip) = fake_session(1);
        let s = ScriptSession::new(session);
        manip.set_screen("abc\nde");
        // 默认光标在原点（假会话的初值，与 `GridSnapshot::empty` 一致）。
        assert_eq!(s.cursor_pos().expect("有屏"), (0, 0));
        manip.set_cursor(1, 2);
        assert_eq!(s.cursor_pos().expect("有屏"), (1, 2), "光标要如实报行与列");
    }

    // ── keys：按键名（与 write 分家的那一半） ────────────────────────────
    #[test]
    fn keys_translates_names_to_vt_bytes() {
        let (session, manip) = fake_session(1);
        let s = ScriptSession::new(session);
        // 一次三个键：enter 是 CR、tab 是 HT、up 在 DECSET 1 关闭时是 ESC[A。
        let written = s.keys(&["enter", "tab", "up"]).expect("按键");
        assert_eq!(manip.written(), b"\r\t\x1b[A".to_vec());
        assert_eq!(written, 5, "返回值就是写入的字节数");
    }

    #[test]
    fn keys_refuses_the_whole_batch_when_one_name_is_unknown() {
        // 半个按键列表发出去之后才报错，会让程序收到一串缺了尾巴的键——
        // 比整批不发更糟，所以先全部翻译再写。
        let (session, manip) = fake_session(1);
        let s = ScriptSession::new(session);
        let err = s
            .keys(&["enter", "ctrl+enter"])
            .expect_err("ctrl+enter 没有统一定义");
        assert_eq!(err, PrimitiveError::UnknownKey("ctrl+enter".to_string()));
        assert!(manip.written().is_empty(), "认不出来的批次一个字节都不该写出去");
    }

    #[test]
    fn keys_reports_a_finished_session_instead_of_writing() {
        let (session, manip) = fake_session(1);
        let s = ScriptSession::new(session);
        manip.finish();
        assert_eq!(s.keys(&["enter"]).unwrap_err(), PrimitiveError::Closed);
    }

    // ── wait_idle：等「安静」而不是等一个事实 ───────────────────────────

    #[test]
    fn wait_idle_returns_what_arrived_and_advances_the_cursor() {
        let (session, manip) = fake_session(1);
        let s = ScriptSession::new(session);
        prime(&s);
        // 生产者分两段推：中间隔 10ms（远小于 100ms 的安静窗口），
        // 于是两段都算「同一次输出」，不该被拆成两次返回。
        let producer = {
            let manip = manip.clone();
            std::thread::spawn(move || {
                std::thread::sleep(std::time::Duration::from_millis(20));
                manip.push_output("line one\n");
                std::thread::sleep(std::time::Duration::from_millis(10));
                manip.push_output("line two\n");
            })
        };
        let got = s.wait_idle(0.1, 5.0).expect("应等到安静");
        producer.join().expect("生产者");
        assert_eq!(got, "line one\nline two\n");
        // 消耗式：游标已推到末尾，紧接着的 read 拿不到重复内容。
        assert_eq!(s.read(0.0, None).unwrap(), "");
    }

    #[test]
    fn wait_idle_times_out_when_the_quiet_window_never_fits_the_budget() {
        // 安静窗口 1s > 总上限 0.1s：输出其实早就停了，但「停够 1 秒」这件事
        // 在预算内无法确认——必须报超时。把它报成「安静了」会把「一直在输出」
        // 和「已经停了」混成一种，是最难查的一类静默。
        let (session, _manip) = fake_session(1);
        let s = ScriptSession::new(session);
        let err = s.wait_idle(1.0, 0.1).expect_err("窗口走不完就是超时");
        assert_eq!(err, PrimitiveError::Timeout);
    }

    #[test]
    fn settle_keeps_the_text_for_the_next_read() {
        // 同步写的收尾**不推进游标**：收下的文本要留给脚本紧接着的 read，
        // 否则同步写就成了「偷偷吃掉一段输出」。
        let (session, manip) = fake_session(1);
        let s = ScriptSession::new(session);
        prime(&s);
        manip.push_output("echoed\r\n");
        s.settle(std::time::Duration::from_millis(20));        assert_eq!(
            s.read(0.0, None).unwrap(),
            "echoed\r\n",
            "收尾收下的文本必须还在未消费段里"
        );
    }

    #[test]
    fn output_before_registration_is_not_counted() {
        // 先登记后消费（§B.13）：模式只在登记前出现过 → 不入账，超时。
        let (session, manip) = fake_session(1);
        let s = ScriptSession::new(session);
        manip.push_output("X appeared before registration\n");
        let err = s
            .wait("X appeared", 0.05)
            .expect_err("登记前的输出不该被匹配");
        assert_eq!(err, PrimitiveError::Timeout);
    }

    #[test]
    fn output_after_registration_is_matched_and_anchored_at_the_register_point() {
        // 登记后输出必命中；text_upto 自登记点起算（不含之前的文本）。
        let (session, manip) = fake_session(1);
        let s = ScriptSession::new(session);
        manip.push_output("stale\n");
        let producer = {
            let manip = manip.clone();
            std::thread::spawn(move || {
                std::thread::sleep(std::time::Duration::from_millis(30));
                manip.push_output("fresh MARK_7 done");
            })
        };
        let got = s.wait("MARK_7", 5.0).expect("登记后的输出必须命中");
        producer.join().expect("生产者");
        assert_eq!(got, "fresh MARK_7", "锚在登记点、含模式结尾");
    }

    #[test]
    fn write_to_a_finished_session_fails_explicitly() {
        let (session, manip) = fake_session(1);
        let s = ScriptSession::new(session);
        manip.finish();
        assert_eq!(s.write("x").unwrap_err(), PrimitiveError::Closed);
    }

    #[test]
    fn wait_on_a_finished_session_reports_closed() {
        let (session, manip) = fake_session(1);
        let s = ScriptSession::new(session);
        manip.finish();
        assert_eq!(s.wait("x", 1.0).unwrap_err(), PrimitiveError::Closed);
    }

    #[test]
    fn flood_output_does_not_defeat_wait_matching() {
        // 洪水输出下 wait 不丢命中（§B.13：wait_register 累计全文，
        // 不依赖屏幕可见范围——屏幕滚动冲掉的行仍可匹配）。
        let (session, manip) = fake_session(1);
        let s = ScriptSession::new(session);
        // 先推 200 行噪声（远超任何终端可见行数），再推目标模式。
        let noise: String = (0..200).map(|i| format!("noise line {i}\n")).collect();
        let producer = {
            let manip = manip.clone();
            std::thread::spawn(move || {
                std::thread::sleep(std::time::Duration::from_millis(30));
                manip.push_output(&noise);
                manip.push_output("NEEDLE_IN_HAYSTACK\n");
                manip.push_output(&noise);
            })
        };
        let got = s.wait("NEEDLE_IN_HAYSTACK", 5.0).expect("洪水中必须命中");
        producer.join().expect("生产者");
        assert!(
            got.ends_with("NEEDLE_IN_HAYSTACK"),
            "text_upto 应以模式结尾：{got:?}"
        );
    }

    #[test]
    fn wait_timeout_is_clamped_to_max_wait() {
        // 超时钳制：脚本传的超时超过沙箱墙钟预算时，单次等待上限 = 预算。
        let (session, _manip) = fake_session(1);
        let s = ScriptSession::with_max_wait(session, std::time::Duration::from_millis(100));
        // 传 999 秒但 max_wait = 100ms → 实际 ~100ms 后超时，不是 999 秒。
        let start = std::time::Instant::now();
        let err = s.wait("never", 999.0).expect_err("应被钳制后超时");
        assert_eq!(err, PrimitiveError::Timeout);
        let elapsed = start.elapsed();
        assert!(
            elapsed < std::time::Duration::from_secs(5),
            "钳制后不应等 999 秒：实际等了 {elapsed:?}"
        );
    }

    #[test]
    fn screen_reports_closed_when_screen_returns_none() {
        // 会话彻底死透（screen() = None，没有事实可取）→ 显式报 Closed。
        // FakeSession 的 screen() 刻意始终返回 Some（与真会话对齐：
        // 进程结束后仍可取最后一屏），所以这里用最小桩会话覆盖 None 路径。
        struct DeadSession;
        impl Session for DeadSession {
            fn id(&self) -> SessionId {
                1
            }
            fn write(&self, _: &[u8]) {}
            fn resize(&self, _: usize, _: usize) {}
            fn screen(&self) -> Option<GridSnapshot> {
                None
            }
            fn state(&self) -> SessionState {
                SessionState::Finished(None)
            }
            fn facets(&self) -> Vec<FacetKind> {
                Vec::new()
            }
            fn wait_register(
                &self,
                _: &str,
            ) -> Result<Box<dyn WaitRegistration>, WaitError> {
                Err(WaitError::Closed)
            }
            fn reader_register(&self) -> Result<Box<dyn OutputReader>, WaitError> {
                Err(WaitError::Closed)
            }
            fn close(&mut self) {}
        }
        let session: Arc<Mutex<dyn Session>> = Arc::new(Mutex::new(DeadSession));
        let s = ScriptSession::new(session);
        assert_eq!(s.screen().unwrap_err(), PrimitiveError::Closed);
    }

    #[test]
    fn read_on_a_session_that_cannot_register_reports_closed() {
        // 登记都登记不上（会话死透）→ read 显式报 Closed，不装作空串。
        struct DeadSession;
        impl Session for DeadSession {
            fn id(&self) -> SessionId {
                2
            }
            fn write(&self, _: &[u8]) {}
            fn resize(&self, _: usize, _: usize) {}
            fn screen(&self) -> Option<GridSnapshot> {
                None
            }
            fn state(&self) -> SessionState {
                SessionState::Finished(None)
            }
            fn facets(&self) -> Vec<FacetKind> {
                Vec::new()
            }
            fn wait_register(
                &self,
                _: &str,
            ) -> Result<Box<dyn WaitRegistration>, WaitError> {
                Err(WaitError::Closed)
            }
            fn reader_register(&self) -> Result<Box<dyn OutputReader>, WaitError> {
                Err(WaitError::Closed)
            }
            fn close(&mut self) {}
        }
        let session: Arc<Mutex<dyn Session>> = Arc::new(Mutex::new(DeadSession));
        let s = ScriptSession::new(session);
        assert_eq!(s.read(0.05, None).unwrap_err(), PrimitiveError::Closed);
    }

    /// wait_any 用的生产者：登记之后才推输出（§B.13 先登记后消费）。
    fn push_after_registration(
        manip: crate::testutil::Manip,
        text: &str,
    ) -> std::thread::JoinHandle<()> {
        let t = text.to_string();
        std::thread::spawn(move || {
            std::thread::sleep(std::time::Duration::from_millis(30));
            manip.push_output(&t);
        })
    }

    #[test]
    fn wait_any_returns_index_of_the_earliest_match() {
        let (session, manip) = fake_session(1);
        let s = ScriptSession::new(session);
        prime(&s);
        // "SECOND" 出现在 "FIRST" 之前（"comes first" 是小写，不匹配大写模式）。
        let _producer = push_after_registration(manip, "banner\nSECOND comes first\nFIRST later\n");
        let hit = s
            .wait_any(&["FIRST".into(), "SECOND".into()], 5.0, None)
            .expect("应命中");
        assert_eq!(hit.index, 2, "最先出现的模式是数组第 2 个");
        assert!(hit.text.ends_with("SECOND"), "命中文本以命中串结尾：{:?}", hit.text);
    }

    #[test]
    fn wait_any_same_position_prefers_first_registered() {
        let (session, manip) = fake_session(1);
        let s = ScriptSession::new(session);
        prime(&s);
        let _producer = push_after_registration(manip, "ABCD\n");
        let hit = s
            .wait_any(&["AB".into(), "ABC".into()], 5.0, None)
            .expect("应命中");
        assert_eq!(hit.index, 1, "同位命中取数组靠前的（ZocWaitMux 同款）");
        assert!(hit.text.ends_with("AB"));
    }

    #[test]
    fn wait_any_match_text_includes_pattern_tail_and_advances_cursor() {
        let (session, manip) = fake_session(1);
        let s = ScriptSession::new(session);
        prime(&s);
        let _producer = push_after_registration(manip, "keep\nHIT_MARK tail-after\n");
        let hit = s
            .wait_any(&["MISS".into(), "HIT_MARK".into()], 5.0, None)
            .expect("应命中");
        assert_eq!(hit.index, 2);
        assert!(hit.text.ends_with("HIT_MARK"), "text 含命中串结尾：{:?}", hit.text);
        let tail = s.read(0.0, None).expect("排水");
        assert!(
            tail.contains("tail-after"),
            "命中后的尾部文本不丢，留给 read：{tail:?}"
        );
    }

    #[test]
    fn wait_any_timeout_fails_explicitly_and_keeps_the_text() {
        let (session, manip) = fake_session(1);
        let s = ScriptSession::new(session);
        prime(&s);
        let _producer = push_after_registration(manip, "arrived but no sentinel\n");
        let err = s
            .wait_any(&["NEVER".into()], 0.35, None)
            .expect_err("超时必须显式失败，不返回索引 0");
        assert_eq!(err, PrimitiveError::Timeout);
        // 游标不动、文本不丢：放一个更宽的模式再等，立刻能拿回全部文本。
        let hit = s
            .wait_any(&["arrived".into()], 0.1, None)
            .expect("超时不该吞掉已到文本");
        assert!(hit.text.contains("arrived"));
    }

    #[test]
    fn wait_any_rejects_empty_pattern_list_and_empty_pattern() {
        let (session, _manip) = fake_session(1);
        let s = ScriptSession::new(session);
        assert_eq!(
            s.wait_any(&[], 1.0, None).unwrap_err(),
            PrimitiveError::EmptyPattern
        );
        assert_eq!(
            s.wait_any(&["ok".into(), "".into()], 1.0, None)
                .unwrap_err(),
            PrimitiveError::EmptyPattern,
            "空串会立刻匹配任何东西，是脚本 bug"
        );
    }

    #[test]
    fn wait_any_filter_hits_noise_line_without_affecting_matching() {
        let (session, manip) = fake_session(1);
        let s = ScriptSession::new(session);
        prime(&s);
        let f = LineFilter::grep_v("^noise").expect("正则");
        // 新语义：过滤在结果之后。噪声行里的 "HIT" 照常命中（index 1），
        // 消费到它结尾——grep 不参与匹配。
        let _producer = push_after_registration(manip, "noise: HIT decoy\nkeep\nREAL_HIT done\n");
        let hit = s
            .wait_any(&["HIT".into(), "REAL_HIT".into()], 5.0, Some(&f))
            .expect("噪声行里的 HIT 也命中");
        assert_eq!(hit.index, 1, "最早出现的 HIT 命中");
        assert_eq!(hit.text, "noise: HIT", "命中与消费不受 grep 影响");
        // 游标停在 "HIT" 结尾（行未完，透传段交给后续 read）。
        let tail = s.read(0.05, Some(&f)).unwrap();
        assert!(tail.contains("keep"), "剩余文本不丢：{tail:?}");
    }
}

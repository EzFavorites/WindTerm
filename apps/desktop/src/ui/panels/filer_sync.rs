//! Filer 面板的「目录同步」（XFER-06）：**扫描 → 审阅 → 执行**。
//!
//! 挂在 `filer` 模块下（同 `view` / `ops` / `xfer`），因为它要读写 `Filer` 的
//! `handle` / `local_path` / `progress`——把那些字段逐个提成 `pub(super)` 只为
//! 让一个兄弟模块看一眼，比放在子树里贵得多。绘制在 [`filer_sync_view.rs`]。
//!
//! # 三个阶段，三条各自的失败方式
//!
//! | 阶段 | 做什么 | 会改文件吗 | 失败时 |
//! |---|---|---|---|
//! | [`SyncState::Scanning`] | 两趟采集（本地线程 + 远端 `ScanTree`） | 不会 | 进 `error`，状态清空 |
//! | [`SyncState::Review`] | 纯展示 + 删除策略三选一 | 不会 | 不存在失败（只重算计划） |
//! | [`SyncState::Running`] | 按 `execution_order` 逐步发请求 | 会 | 逐项记账，不中止 |
//!
//! 分成三段而不是「点一下就跑完」，是因为中间那一段是**用户唯一能看见代价的地方**：
//! 同步会覆盖文件、可能删东西，而「我按了之后发生了什么」必须在他按之前就摆在眼前。
//!
//! # 执行的两条硬约束
//!
//! 1. **同一时刻只飞一步。** `SyncRun::step()` 给出一步，发出去，等它的事件回来
//!    `record()`，才发下一步。并发会让 `term_xfer::sync` 的两条契约同时失效：
//!    「目录失败连坐同操作子项」依赖于知道哪个目录先失败、「删除排在最后」依赖于
//!    写操作全部收尾。所以 [`SyncExec::inflight`] 非空时 pump 直接返回。
//! 2. **上传下载一律 `preserve = true`。** 不保留 mtime 的话，传完目标端的 mtime
//!    变成「现在」，下一趟扫描会判「对端更新了」，同一份内容每趟都来回传一次
//!    —— 计划永远收敛不到全 `Skip`。设计稿 §7 的「二次全 Skip」判据抓的就是它。
//!
//! 续传（`resume`）在这里一律 `false`：计划已经判出「该传」，而续传是「接着半截
//! 往下写」——若那半截不是同一份内容（远端被改过、上次是别的方向的残留），续出来
//! 的文件既不是旧的也不是新的。同步宁可整份重传。

use std::path::PathBuf;

use term_sftp::ScannedEntry;
use term_xfer::sync::{
    DeletePolicy, Progress, SyncItem, SyncOp, SyncRun, StepResult,
};

use crate::sync_job::{LocalScan, Summary, build_plan};

/// 目录同步的三个阶段。`Filer::sync` 是 `Option<Self>`，`None` = 空闲。
pub(in crate::ui) enum SyncState {
    /// 两趟采集在跑。
    Scanning(Box<SyncScan>),
    /// 采集完了，等用户看一眼再决定。**这一步不改任何文件。**
    Review(Box<SyncReview>),
    /// 正在按计划执行。
    Running(Box<SyncExec>),
}

/// 一次同步的两侧根 + 两份清单。审阅阶段改删除策略时要用原始清单重算计划，
/// 所以扫完不能只留计划。
pub(in crate::ui) struct Scanned {
    pub local: Vec<ScannedEntry>,
    pub remote: Vec<ScannedEntry>,
}

/// 两趟都到齐之后的结论。取走即结束这次扫描。
pub(in crate::ui) enum Outcome {
    /// 两份清单都在，交给审阅阶段。
    Ready(Scanned),
    /// 某一侧扫不动。文案直接给用户看。
    Failed(String),
}

/// 扫描阶段：本地一条线程、远端一条 `ScanTree` 请求，谁先回来都行。
pub(in crate::ui) struct SyncScan {
    local_root: PathBuf,
    /// 远端根，也是「这份远端清单是不是我的」的判据。
    remote_root: String,
    local_scan: LocalScan,
    /// 本地那一趟的结果（`None` = 还在扫）。
    local: Option<Result<Vec<ScannedEntry>, String>>,
    /// 远端那一趟的结果（`None` = 还在扫）。
    remote: Option<Result<Vec<ScannedEntry>, String>>,
}

impl SyncScan {
    /// 起一次扫描。本地起线程、远端请求由调用方发（它才借得到 `handle`）。
    pub(in crate::ui) fn start(local_root: PathBuf, remote_root: String) -> Self {
        SyncScan {
            local_scan: LocalScan::start(local_root.clone()),
            local_root,
            remote_root,
            local: None,
            remote: None,
        }
    }

    /// 远端根——调用方拿它发 `scan_tree`，不在本模块里发：发请求要 `&SftpHandle`，
    /// 而句柄在 `poll_filer` 里是「取出来用完再放回去」的，不该有第二处借它。
    pub(in crate::ui) fn remote_root(&self) -> &str {
        &self.remote_root
    }

    /// 本地根（给状态行文案用）。
    pub(in crate::ui) fn local_root(&self) -> &std::path::Path {
        &self.local_root
    }

    /// 收到一棵远端的清单。只认自己等的那棵树，别的一律忽略。
    pub(in crate::ui) fn note_remote(&mut self, path: &str, entries: Vec<ScannedEntry>) {
        if path == self.remote_root && self.remote.is_none() {
            self.remote = Some(Ok(entries));
        }
    }

    /// 远端那棵树扫失败了。同样只认自己等的那棵。
    pub(in crate::ui) fn fail_remote(&mut self, path: &str, message: String) {
        if path == self.remote_root && self.remote.is_none() {
            self.remote = Some(Err(message));
        }
    }

    /// 这一帧有没有结论。`None` = 继续等。
    pub(in crate::ui) fn poll(&mut self) -> Option<Outcome> {
        // 只在还没拿到时问：线程发过结果之后 `try_recv` 会变成 `Disconnected`，
        // 无条件重问会把一份好结果覆盖成「线程意外退出」。
        if self.local.is_none() {
            self.local = self.local_scan.poll();
        }
        // 本地失败就不用等远端了——两份清单缺一份算不出计划，早点报错早点能重试。
        if let Some(Err(why)) = &self.local {
            return Some(Outcome::Failed(format!(
                "{}: {why}",
                self.local_root.display()
            )));
        }
        if let Some(Err(why)) = &self.remote {
            return Some(Outcome::Failed(format!("{}: {why}", self.remote_root)));
        }
        match (&self.local, &self.remote) {
            (Some(Ok(_)), Some(Ok(_))) => {
                // 两侧都到手才取走（`take` 而不是 clone：清单可能有几万条）。
                let Some(Ok(local)) = self.local.take() else {
                    return None;
                };
                let Some(Ok(remote)) = self.remote.take() else {
                    return None;
                };
                Some(Outcome::Ready(Scanned { local, remote }))
            }
            _ => None,
        }
    }
}

/// 审阅阶段：一份计划 + 三选一的删除策略。**不改任何文件。**
pub(in crate::ui) struct SyncReview {
    local_root: PathBuf,
    remote_root: String,
    scanned: Scanned,
    /// 按当前策略算出来的计划（含 `Skip` 项——界面要显示「其中 M 项已一致」）。
    items: Vec<SyncItem>,
    summary: Summary,
    policy: DeletePolicy,
}

impl SyncReview {
    fn new(local_root: PathBuf, remote_root: String, scanned: Scanned) -> Self {
        let mut review = SyncReview {
            local_root,
            remote_root,
            scanned,
            items: Vec::new(),
            summary: Summary::default(),
            // 默认 `Keep`：同步工具的默认行为绝不能是删用户文件。
            policy: DeletePolicy::Keep,
        };
        review.rebuild();
        review
    }

    /// 按当前策略重算计划。改策略时调它——**不重新扫盘**：两份清单还在手上，
    /// 重扫一遍既慢又可能扫到不同的树（用户在这中间动了文件）。
    fn rebuild(&mut self) {
        self.items = build_plan(&self.scanned.local, &self.scanned.remote, self.policy);
        self.summary = Summary::of(&self.items);
    }

    /// 审阅页要画的全部内容——只读，除了 [`SyncReview::set_policy`]。
    pub(in crate::ui) fn items(&self) -> &[SyncItem] {
        &self.items
    }

    pub(in crate::ui) fn summary(&self) -> Summary {
        self.summary
    }

    pub(in crate::ui) fn policy(&self) -> DeletePolicy {
        self.policy
    }

    /// 两侧根的 `显示` 形式（`a → b` 那一行用）。
    pub(in crate::ui) fn roots(&self) -> (String, String) {
        (
            self.local_root.display().to_string(),
            self.remote_root.clone(),
        )
    }

    pub(in crate::ui) fn set_policy(&mut self, policy: DeletePolicy) {
        if self.policy != policy {
            self.policy = policy;
            self.rebuild();
        }
    }

}

/// 执行阶段：一份计划 + 正在等的那一步。
pub(in crate::ui) struct SyncExec {
    run: SyncRun,
    local_root: PathBuf,
    remote_root: String,
    /// 已经发出去、还在等事件的那一步。非空时不许再发下一步。
    inflight: Option<Inflight>,
    /// 逐项失败的原话（服务端说的那句，不经我们转述）。只留前若干条：
    /// 一次全盘失败留下的几万条相同文案没有额外的解释力，只会把内存吃满。
    errors: Vec<String>,
    /// 用户点了「停止」：不再发下一步，但**已经发出去的那一步要等它回来**。
    ///
    /// 为什么不是当场清状态：在途那一步已经发到服务端了，它的 `Done` / `Failed`
    /// 迟早会到。当场走人的话，那个结果回来时没人认领，界面会显示「停止时 3/10」
    /// 而实际可能已经传完第 4 个——数字对不上比慢一步糟得多。
    ///
    /// 停止**不回滚**：已经传过去的文件留在那里。同步的语义是「补齐差异」，
    /// 回滚等于把「补了一半」变成「一点没补」，再跑一趟还是从同一处开始。
    stop_requested: bool,
    /// 最近一步的**相对**路径（面板「当前文件」那一格）。绝对路径在这里没用：
    /// 两侧根已经写在别处，用户要看的是「这一趟动到哪了」。
    current: Option<String>,
    /// 计划里所有**文件**的字节总和。目录项与删除项不计——它们不搬字节。
    /// `0` = 这一趟没有字节可搬（全是目录 / 删除），界面字节那一段整段省掉。
    total_bytes: u64,
    /// 已经落地的字节（按 `Done` 的 `bytes` 累加）。
    moved: u64,
    /// 在途那个文件已经走了多少（`Progress` 的 `transferred`）。`Done` 一到就
    /// 归零——那一步的完整字节已经进了 `moved`，再加一次是重复计数。
    inflight_bytes: u64,
    /// 字节速率。与传输那条路径**各持一个**：`RateMeter` 的口径是单向，
    /// 两条路混在一起算没有意义（见 `term_xfer::rate` 模块头最后一条）。
    meter: term_xfer::RateMeter,
}

/// 正在等的那一步。`path` 是**远端**路径——`Done` / `Failed` 事件带的就是它。
///
/// 本地删除没有在途请求（`std::fs` 是同步的），所以那一步不设 inflight，
/// 当场 `record`。
struct Inflight {
    path: String,
    op: SyncOp,
}

/// 失败原话最多留几条。
const MAX_KEPT_ERRORS: usize = 5;

impl SyncExec {
    /// 接一份计划开始跑。由审阅页确认时调用——**这是唯一会改文件的入口。**
    pub(in crate::ui) fn new(
        items: Vec<SyncItem>,
        local_root: PathBuf,
        remote_root: String,
    ) -> Self {
        // 先算总字节再交出去：`SyncRun::new` 会把 `items` 拿走。目录项要排掉——
        // `sync::plan` 给单侧独有的目录也发 `Upload` / `Download`，但它们只建目录，
        // 没有字节可搬，算进分母会让百分比永远到不了 100%。
        let total_bytes = items
            .iter()
            .filter(|i| !i.entry.is_dir && matches!(i.op, SyncOp::Upload | SyncOp::Download))
            .map(|i| i.entry.size)
            .sum();
        SyncExec {
            run: SyncRun::new(items),
            local_root,
            remote_root,
            inflight: None,
            errors: Vec::new(),
            stop_requested: false,
            current: None,
            total_bytes,
            moved: 0,
            inflight_bytes: 0,
            meter: term_xfer::RateMeter::new(),
        }
    }

    /// 进度快照（界面直接画）。
    pub(in crate::ui) fn progress(&self) -> Progress {
        self.run.progress()
    }

    /// 这一趟要搬的总字节（`0` = 没有字节要搬）。
    pub(in crate::ui) fn total_bytes(&self) -> u64 {
        self.total_bytes
    }

    /// 已落地的字节数。含在途那个文件的进度——否则一个 200 MB 的文件在传完
    /// 之前数字一直是 0，看起来像卡住了。
    pub(in crate::ui) fn moved_bytes(&self) -> u64 {
        self.moved.saturating_add(self.inflight_bytes)
    }

    /// 速率（字节/秒）。还没有过一个采样窗口时 `None`——界面写「估算中」而不是
    /// 「0 B/s」，后者会被读成「卡住了」。
    pub(in crate::ui) fn bytes_per_sec(&self) -> Option<u64> {
        self.meter.bytes_per_sec_rounded()
    }

    /// 最近一步的相对路径（「当前文件」那一格）。
    pub(in crate::ui) fn current(&self) -> Option<&str> {
        self.current.as_deref()
    }

    /// 用户已经按过停止、还在等在手那一步收尾。界面据此把按钮改成「Stopping…」。
    pub(in crate::ui) fn is_stopping(&self) -> bool {
        self.stop_requested
    }

    /// 一条失败原话（没有就是 `None`）。界面在 Running 阶段显示最近一条。
    pub(in crate::ui) fn last_error(&self) -> Option<&str> {
        self.errors.last().map(String::as_str)
    }

    fn note_error(&mut self, message: String) {
        if self.errors.len() < MAX_KEPT_ERRORS {
            self.errors.push(message);
        }
    }
}

/// 汇总 → 状态行那一句话。
///
/// 中文硬编码，与传输入口那两句同一笔豁免：翻译表在 `crates/term-app/src/i18n.rs`，
/// 本轮分工不动那个 crate。带计数的模板进 i18n 也不是加一句 `tr()` 能解决的
/// （要带参数），留到 i18n 第二阶段统一处理。
pub(in crate::ui) fn summary_line(s: &Summary) -> String {
    if s.mutations() == 0 {
        return format!("同步：两侧已一致（{} 项）", s.skip);
    }
    let mut parts = Vec::new();
    if s.upload > 0 {
        parts.push(format!("{} 项待上传", s.upload));
    }
    if s.download > 0 {
        parts.push(format!("{} 项待下载", s.download));
    }
    if s.delete_local > 0 {
        parts.push(format!("{} 项待删本地", s.delete_local));
    }
    if s.delete_remote > 0 {
        parts.push(format!("{} 项待删远端", s.delete_remote));
    }
    format!("同步：{}（{} 项已一致）", parts.join(" / "), s.skip)
}

impl super::Filer {
    /// 起一次「目录同步」的扫描。已在同步就忽略——重入会把上一趟的结果连同那条
    /// 线程一起丢掉，而用户看不出区别。
    ///
    /// 两侧根都必须有：本地是 `local_path`（面板在远端视图时它也一直在，跨帧保留），
    /// 远端是当前 `path`。缺一个就没有「同步」可言——这种情况下不发请求也不置状态，
    /// 菜单项的可用性判定在画的时候已经拦了一道。
    pub(in crate::ui) fn begin_sync(&mut self) {
        if self.sync.is_some() {
            return;
        }
        let remote = self.path.clone();
        let local = self.local_path.clone();
        if remote.is_empty() || local.as_os_str().is_empty() {
            return;
        }
        {
            let Some(handle) = &self.handle else { return };
            handle.scan_tree(remote.as_str());
        }
        // 顺序要紧：请求先发（借 `handle` 的那一小段到此结束），再落状态。反过来
        // 会让「扫描中」的界面出现一帧却没有请求在飞——那一帧之后永远等不到结果。
        self.sync = Some(SyncState::Scanning(Box::new(SyncScan::start(local, remote))));
        self.progress = None;
        self.error = None;
    }

    /// 远端清单到了 / 远端扫失败了。只在 `Scanning` 阶段有读者。
    pub(in crate::ui) fn note_scan_remote(&mut self, path: &str, entries: Vec<ScannedEntry>) {
        if let Some(SyncState::Scanning(scan)) = &mut self.sync {
            scan.note_remote(path, entries);
        }
    }

    /// 见 [`Filer::note_scan_remote`]。
    pub(in crate::ui) fn fail_scan_remote(&mut self, path: &str, message: String) {
        if let Some(SyncState::Scanning(scan)) = &mut self.sync {
            scan.fail_remote(path, message);
        }
    }

    /// 执行阶段：某一步成功了。只认在途的那一步（远端路径对上才算）。
    ///
    /// `bytes` 是这一步落地的字节。写操作（mkdir / rename / 删除）也发同一个
    /// 事件但 `bytes` 为 `0`，正好不影响字节账。
    pub(in crate::ui) fn note_sync_done(&mut self, path: &str, bytes: u64) {
        let Some(SyncState::Running(exec)) = &mut self.sync else { return };
        let Some(flight) = &exec.inflight else { return };
        if flight.path == path {
            exec.run.record(StepResult::Done);
            exec.moved = exec.moved.saturating_add(bytes);
            // 在途那份的进度已经并进 `moved` 了，留着会重复计数。
            exec.inflight_bytes = 0;
            exec.inflight = None;
        }
    }

    /// 执行阶段：在途那个文件走到了哪。只认在途的那一步。
    ///
    /// `Event::Progress` 已按「每 1 MB 或每 200 ms」节流（见 `term_sftp` 的事件
    /// 文档），所以这里不必再自己限流；写操作不发这个事件，认不上就该交给别的
    /// 消费者（普通传输也在听同一批事件）。
    pub(in crate::ui) fn note_sync_progress(&mut self, path: &str, transferred: u64) {
        let Some(SyncState::Running(exec)) = &mut self.sync else { return };
        let Some(flight) = &exec.inflight else { return };
        if flight.path == path {
            exec.inflight_bytes = transferred;
        }
    }

    /// 执行阶段：某一步失败了。服务端原话进 `errors`，界面原样显示。
    ///
    /// 同样只认在途那一步：写操作（mkdir / rename / chmod）也会发 `Failed`，
    /// 它们的路径不在同步计划里，认不上就该交给别的消费者。
    pub(in crate::ui) fn note_sync_failed(&mut self, path: &str, message: &str) {
        let Some(SyncState::Running(exec)) = &mut self.sync else { return };
        let Some(flight) = &exec.inflight else { return };
        if flight.path == path {
            exec.run.record(StepResult::Failed);
            exec.note_error(format!("{path}: {message}"));
            exec.inflight_bytes = 0;
            exec.inflight = None;
        }
    }

    /// 用户点了「停止」：不再发下一步，等在手的那一步收尾后清状态。
    pub(in crate::ui) fn request_sync_stop(&mut self) {
        if let Some(SyncState::Running(exec)) = &mut self.sync {
            exec.stop_requested = true;
        }
    }

    /// 会话断了：执行阶段没法继续，也不该假装还在跑。
    pub(in crate::ui) fn abort_sync(&mut self, why: String) {
        if matches!(self.sync, Some(SyncState::Running(_))) {
            self.sync = None;
            self.error = Some(format!("同步中断：{why}"));
        }
    }

    /// 每帧推进同步：扫描阶段看线程，执行阶段发下一步。
    ///
    /// 返回 `true` 表示「还没停在等用户」，调用方据此请求下一帧。这条返回值不是
    /// 优化而是必需：S4 之后空闲不重绘，pump 只在有输入的那几帧跑的话，同步会停在
    /// 第二步上直到用户随手动一下鼠标。
    ///
    /// `handle` 只在执行阶段用得上——扫描阶段的远端请求已经在 `begin_sync` 里
    /// 发出去了。传 `None` 意味着会话没了，执行阶段会当场中止而不是空转。
    ///
    /// `now_ms` 只喂速率估计器（[`term_xfer::RateMeter`] 的窗口下限是 100 ms）。
    /// 调用方给的是**从启动算起的单调毫秒**，不是墙钟——本仓的界面层统一用
    /// egui 的 `time`，这样 `ui/` 不必去读系统时钟。
    pub(in crate::ui) fn poll_sync(
        &mut self,
        handle: Option<&term_sftp::SftpHandle>,
        now_ms: u64,
    ) -> bool {
        match &mut self.sync {
            None => false,
            // 审阅阶段在等用户，不需要重绘推动。
            Some(SyncState::Review(_)) => false,
            Some(SyncState::Scanning(_)) => {
                self.advance_scan();
                true
            }
            Some(SyncState::Running(exec)) => {
                // 每帧喂一次，与面板开没开无关：只挂到绘制上会让「面板一直是关的，
                // 中途打开第一眼看到的是没有速率的估算中」。
                exec.meter.sample(exec.moved_bytes(), now_ms);
                self.pump_exec(handle);
                true
            }
        }
    }

    /// 扫描阶段的一格：两趟到齐就把状态换成审阅。
    fn advance_scan(&mut self) {
        let Some(SyncState::Scanning(scan)) = &mut self.sync else { return };
        let outcome = scan.poll();
        let pair = format!(
            "{} → {}",
            scan.local_root().display(),
            scan.remote_root()
        );
        match outcome {
            None => {}
            Some(Outcome::Ready(scanned)) => {
                // 两侧根在 `scan` 被丢掉之后还要用，先取出来。
                let local_root = scan.local_root().to_path_buf();
                let remote_root = scan.remote_root().to_string();
                self.progress = Some(format!("{pair} ｜ 扫描完成，待确认"));
                self.sync = Some(SyncState::Review(Box::new(SyncReview::new(
                    local_root,
                    remote_root,
                    scanned,
                ))));
            }
            Some(Outcome::Failed(why)) => {
                self.error = Some(why);
                self.progress = None;
                self.sync = None;
            }
        }
    }

    /// 执行阶段的一格：发下一步，或宣告跑完。
    ///
    /// 每一步发出去都设 [`SyncExec::inflight`]，直到事件回来清掉它——这就是
    /// 「同一时刻只飞一步」的全部实现。本地删除是同步的，当场记账、不设在途。
    fn pump_exec(&mut self, handle: Option<&term_sftp::SftpHandle>) {
        let Some(SyncState::Running(exec)) = &mut self.sync else { return };
        if exec.inflight.is_some() {
            return; // 等事件，这一帧不动
        }
        // 在途那一步已经收尾（或者本来就没有），停止请求现在可以兑现了。
        if exec.stop_requested {
            let p = exec.run.progress();
            let line = format!(
                "同步已停止：{} 项已完成 / {} 项失败（剩下 {} 项没动）",
                p.done,
                p.failed,
                p.remaining()
            );
            self.sync = None;
            self.progress = Some(line);
            return;
        }
        let Some(item) = exec.run.step().cloned() else {
            let p = exec.run.progress();
            let line = if p.failed + p.skipped == 0 {
                format!("同步完成：{} 项成功", p.done)
            } else {
                format!(
                    "同步完成：{} 项成功 / {} 项失败 / {} 项跳过",
                    p.done, p.failed, p.skipped
                )
            };
            self.sync = None;
            self.progress = Some(line);
            return;
        };
        let Some(handle) = handle else {
            self.abort_sync("会话已断开".to_string());
            return;
        };
        let local = term_sftp::local_path_for(
            &exec.local_root.to_string_lossy(),
            &item.entry.path,
        );
        let remote = term_sftp::remote_path_for(&exec.remote_root, &item.entry.path);
        let is_dir = item.entry.is_dir;
        // 当前这一步落到哪个文件，面板「当前文件」那一格画的就是它。放在发请求
        // 之前设：用户看到的应该是「正在动的那个」，不是「上一秒动完的那个」。
        exec.current = Some(item.entry.path.clone());
        match item.op {
            // 目录的「上传 / 下载」就是建目录：`sync::plan` 对单侧独有的目录也给
            // `Upload`，但它没有内容要搬。`execution_order` 保证父目录排在前，
            // 所以文件落地时它已经在了。
            SyncOp::Upload if is_dir => {
                handle.mkdir(remote.clone());
                exec.inflight = Some(Inflight {
                    path: remote,
                    op: SyncOp::Upload,
                });
            }
            SyncOp::Download if is_dir => {
                // 本地建目录是同步的，不设 inflight。
                match std::fs::create_dir_all(&local) {
                    Ok(()) => exec.run.record(StepResult::Done),
                    Err(e) => {
                        exec.run.record(StepResult::Failed);
                        exec.note_error(format!("{}: {e}", local.display()));
                    }
                }
            }
            SyncOp::Upload => {
                handle.upload(
                    local.to_string_lossy().to_string(),
                    remote.clone(),
                    false,
                    true,
                );
                exec.inflight = Some(Inflight {
                    path: remote,
                    op: SyncOp::Upload,
                });
            }
            SyncOp::Download => {
                handle.download(
                    remote.clone(),
                    local.to_string_lossy().to_string(),
                    false,
                    true,
                );
                exec.inflight = Some(Inflight {
                    path: remote,
                    op: SyncOp::Download,
                });
            }
            SyncOp::DeleteRemote => {
                if is_dir {
                    handle.remove_dir(remote.clone());
                } else {
                    handle.remove_file(remote.clone());
                }
                exec.inflight = Some(Inflight {
                    path: remote,
                    op: SyncOp::DeleteRemote,
                });
            }
            SyncOp::DeleteLocal => {
                // 本地删除是同步的。`execution_order` 让子项先于父目录，所以删到
                // 目录时它已经空了——`remove_dir` 非空会失败，那正是我们要的信号。
                let result = if is_dir {
                    std::fs::remove_dir(&local)
                } else {
                    std::fs::remove_file(&local)
                };
                match result {
                    Ok(()) => exec.run.record(StepResult::Done),
                    Err(e) => {
                        exec.run.record(StepResult::Failed);
                        exec.note_error(format!("{}: {e}", local.display()));
                    }
                }
            }
            SyncOp::Skip => {} // `execution_order` 不含 `Skip`，走不到
        }
    }
}

// 测试单独一个文件（同 `filer_wheel_tests.rs` / `bridge/lock_tests.rs`）：这一层的
// 判据比实现长——执行顺序、连坐、在途一步、事件认领各要一条，塞在本文件里会把
// 4j 的 500 行上限顶破，而它们是「验」不是「实现」，读者也不一样。

/// Transfer 面板顶部那一条 Sync 行（XFER-06 执行中才有）。
pub(in crate::ui) struct SyncLine {
    /// 第一行：走了多少步、成几失败几。
    pub headline: String,
    /// 第二行：字节 / 速率 / 当前文件。全都没有时是空串。
    pub detail: String,
    /// 用户已经按过停止、还在等在手那一步收尾。
    pub stopping: bool,
}

impl SyncLine {
    /// 由执行器算一行。纯读，不改任何状态——所以判据可以直接喂一个 `SyncExec`，
    /// 不必造一个窗口。
    ///
    /// 字节那一段**只在真有字节要搬时出现**：全是目录 / 删除的一趟里，
    /// 「0 B / 0 B」既没有信息又占满一整段（`total_bytes == 0`）。
    /// 速率还没有过一个窗口时写「估算中」而不是「0 B/s」——后者是「卡住了」。
    pub(in crate::ui) fn of(exec: &SyncExec) -> Self {
        let p = exec.progress();
        let touched = p.done + p.failed + p.skipped;
        let headline = format!(
            "目录同步 {touched}/{} · 成功 {} / 失败 {}",
            p.todo, p.done, p.failed
        );
        let mut parts = Vec::new();
        if exec.total_bytes() > 0 {
            parts.push(format!(
                "{} / {}",
                term_sftp::format_size(exec.moved_bytes()),
                term_sftp::format_size(exec.total_bytes())
            ));
        }
        match exec.bytes_per_sec() {
            Some(bps) => parts.push(format!("{}/s", term_sftp::format_size(bps))),
            // 只有真在搬字节时才说「估算中」：不搬字节的一趟里没有速率可言，
            // 而已经喊停的一趟也不该继续承诺一个估计。
            None if exec.total_bytes() > 0 && !exec.is_stopping() => {
                parts.push("估算中".to_string())
            }
            None => {}
        }
        if let Some(rel) = exec.current() {
            parts.push(rel.to_string());
        }
        SyncLine {
            headline,
            detail: parts.join(" · "),
            stopping: exec.is_stopping(),
        }
    }
}

impl crate::ui::TerminalWindow {
    /// Transfer 面板顶部那一行（`None` = 没在同步）。
    ///
    /// 为什么给 Transfer 面板单独一行：停靠面板是「这次运行里在搬什么」的唯一
    /// 常驻视图，同步在执行时搬的字节不比右键传一个文件少，只画在文件面板的
    /// 状态行上就等于「只有盯着那个面板才看得见」。
    pub(in crate::ui) fn filer_sync_progress(&self) -> Option<SyncLine> {
        match &self.filer.sync {
            Some(SyncState::Running(exec)) => Some(SyncLine::of(exec)),
            _ => None,
        }
    }

    /// 面板上的停止入口。与执行窗那个 `Stop` 走同一条路（`request_sync_stop`），
    /// 两个入口一个语义：不再发下一步，在途那一步照常收尾。
    pub(in crate::ui) fn stop_filer_sync(&mut self) {
        self.filer.request_sync_stop();
    }
}

#[cfg(test)]
#[path = "filer_sync_tests.rs"]
mod tests;

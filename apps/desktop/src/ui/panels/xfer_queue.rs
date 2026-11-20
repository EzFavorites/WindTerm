//! 上传 / 下载队列的界面壳：把纯模型 [`term_xfer::Queue`] 与 SFTP 事件接起来。
//!
//! # 为什么还要这一层
//!
//! 模型层（`crates/term-xfer/src/queue.rs`）只认 `id` 与 `size`，而 SFTP 的事件只给
//! **远端路径**（`Event::Progress { path, .. }`）。两者之间需要一张「路径 → id」的表；
//! 这张表还必须跟发起请求时记下的两端路径绑在一起——排队期间用户可以在文件面板里
//! 换目录，等轮到它跑的时候再按「当前目录」拼路径，就会传到别的地方去。
//!
//! # 三条纪律
//!
//! 1. **不持有 `SftpHandle`。** 这一层只排程，发请求的是 `filer_xfer.rs`。所以单测
//!    可以喂一串假事件，把「入队 → 跑 → 进度 → 完成 / 失败 → 退避重试 → 取消」
//!    全走完，不需要一条真的 SSH 会话。
//! 2. **不读时钟。** `now` 由调用方注入，与 `queue.rs` 同一口径。界面层读
//!    `ctx.input(|i| i.time)` 是允许的，`term-core` 才禁止。
//! 3. **并行上限 1。** 后台是**一条线程串行消费**请求的（见 `term-sftp` 的
//!    `background.rs` 模块头：「一次传输期间后面排着的请求要等它传完」）。填 2 只会
//!    让第二项在界面上显示「传输中」、实际却堵在通道里等——那是假进度。
//!
//! # 事件口径（这一条最容易写错）
//!
//! 上传与下载的 `Event::{Progress,Done,Failed,Cancelled}` 里的 `path` **都是远端
//! 路径**：`background.rs` 的 `transfer_and_report` 里只有一句
//! `let path = remote.to_string();`，两个方向共用它（下载时是源、上传时是目标）。
//! 所以下面这张表的键一律用远端路径，用本地路径做键会让上传的进度永远对不上。
//!
//! # 目录树：一行对一棵树
//!
//! `SftpHandle::download_tree` / `upload_tree` 的事件流是「每个文件一组
//! `Progress` + `Done`（`path` 是该文件的远端路径），最后由**树根**的 `Done`
//! 收尾；任一文件失败 / 取消则 `Failed` / `Cancelled` 挂在树根上」。整棵树在队列
//! 里只占**一行**，因此：
//!
//! - 树根另存一份 [`XferQueue::tree_roots`]，文件事件按**前缀**归到这一行；
//! - 文件级 `Done` 只记账不结算，否则第一个文件就把整棵树标成 `Done`，后面的
//!   事件再也找不到 id；
//! - 只有树根那条 `Done` / `Failed` / `Cancelled` 才 `finish`，并一次摘干净两张表。

use std::collections::BTreeMap;

use term_xfer::{Direction, FailReason, Job, JobState, Policy, Queue};

/// 队列策略：并行 1（见模块头）、自动重试 3 次、退避 2 / 4 / 8 秒。
///
/// 3 次是权衡：SFTP 的失败多数是「权限 / 磁盘满 / 路径不存在」，重试也不会变好；
/// 但瞬时断流、服务端忙属于真会自愈的那一类。8 秒之后还没有结果，把「重试」交给
/// 用户按比让界面一直自己转更诚实。
const POLICY: Policy = Policy {
    max_parallel: 1,
    max_retries: 3,
    retry_backoff: 2,
};

/// 树任务入队时的 `size` 哨兵：整棵树的总字节要等树根 `Done` 才知道，而模型层
/// [`Queue::progress`] 会把 `transferred` 钳到 `size`，写 0 会让整行永远停在 0。
/// 用一个「上不封顶」的值放行累计进度；**真实分母**由 [`Task::tree_total`] 提供，
/// [`XferQueue::overall`] 与 [`XferQueue::rows`] 都按它算，哨兵绝不透给界面。
const TREE_SIZE_UNKNOWN: u64 = u64::MAX;

/// 一次传输的两端与最近一次失败原话。
struct Task {
    direction: Direction,
    /// 远端路径。**必须与事件里的 `path` 同口径**（见模块头）。
    remote: String,
    /// 另一端：下载是落盘位置，上传是本地源。
    local: String,
    /// 服务端原话。`JobState::Failed` 只存类别，原话在这里——排查全靠它。
    /// 进度重新走起来时清掉：那一句描述的是上一次尝试，不是现在。
    error: Option<String>,
    /// 目录树任务：整棵树一行，只有树根收尾事件才结算（见模块头）。
    tree: bool,
    /// 树里已经收尾（`Done`）的文件字节总和。
    tree_done: u64,
    /// 当前在传文件的最近一次进度。每个文件从 0 重来，所以它是**增量**，
    /// 累计值 = `tree_done + tree_partial`。
    tree_partial: u64,
    /// 整棵树的确切总字节：树根 `Done` 的 `bytes`；之前是 0（分母未知，不编）。
    tree_total: u64,
}

/// 该跑的一项。两端路径都给齐，调用方拿到就能直接发请求。
pub(in crate::ui) struct XferTask {
    pub id: u64,
    pub direction: Direction,
    /// 远端路径（事件口径）。
    pub remote: String,
    /// 另一端：下载是落盘位置，上传是本地源。
    pub local: String,
    /// 目录树任务：发起时选 `download_tree` / `upload_tree` 而不是单文件那两条。
    pub tree: bool,
}

/// 事件路径反查的结果。**树根的收尾事件与文件事件必须分开**，否则第一个文件的
/// `Done` 就会把整棵树结算掉。
enum PathMatch {
    /// 与登记的远端路径完全相同：单文件传输，或树传输的树根收尾事件。
    Exact(u64),
    /// 落在某棵树根之下的文件事件（只记账，不结算）。
    TreeFile(u64),
}

impl PathMatch {
    fn id(self) -> u64 {
        match self {
            PathMatch::Exact(id) | PathMatch::TreeFile(id) => id,
        }
    }
}

/// 队列的一行（快照）。
///
/// 绘制拿快照而不是借队列本身：面板里的按钮要改队列，借着一份 `&XferQueue` 画
/// 再回头改，借用检查器会拦，绕开的写法（先 clone 再回填）只会更难读。
pub(in crate::ui) struct XferRow {
    pub id: u64,
    pub direction: Direction,
    /// 远端路径。
    pub remote: String,
    /// 只给人看的短名：路径最后一段（面板宽度放不下整条路径）。
    pub name: String,
    pub size: u64,
    pub state: JobState,
    pub attempts: u32,
    /// 失败原话；没失败过就是 `None`。
    pub error: Option<String>,
}

/// 传输队列。
///
/// `tasks` / `by_path` 与模型层的 `jobs` 是**同一份事实的两种索引**：`id` 是模型层
/// 分配的，路径是事件带来的。两者都不删（模型层也没有删除入口），所以面板会一直
/// 留着已经结束的行——那是本次运行的历史，不是第二个真相。
pub(in crate::ui) struct XferQueue {
    queue: Queue,
    tasks: BTreeMap<u64, Task>,
    /// 远端路径 → id。事件里只有路径，没有方向也没有 id，这是唯一的反查表。
    by_path: BTreeMap<String, u64>,
    /// 树根远端路径 → id。只有树任务登记：文件事件靠它按前缀归行。
    tree_roots: BTreeMap<String, u64>,
}

impl Default for XferQueue {
    fn default() -> Self {
        Self::new()
    }
}

impl XferQueue {
    pub(in crate::ui) fn new() -> Self {
        Self {
            queue: Queue::new(POLICY),
            tasks: BTreeMap::new(),
            by_path: BTreeMap::new(),
            tree_roots: BTreeMap::new(),
        }
    }

    /// 远端文件 → 本地文件。返回分配到的 id；`None` 表示同一个远端路径已经在
    /// 队列里（还没结束），界面据此说一句「已经在队列里」而不是装没听见。
    ///
    /// 两端路径都在**入队时**定下来：排队期间用户可能换了目录，等轮到它跑时再按
    /// 「当前目录」拼路径就会落到别处。
    pub(in crate::ui) fn enqueue_download(
        &mut self,
        remote: &str,
        local: &str,
        size: u64,
    ) -> Option<u64> {
        self.push(Direction::Download, remote, local, size, false)
    }

    /// 本地文件 → 远端文件。目标文件名由调用方拼好（面板块里已经有 `join_path`）。
    pub(in crate::ui) fn enqueue_upload(
        &mut self,
        local: &str,
        remote: &str,
        size: u64,
    ) -> Option<u64> {
        self.push(Direction::Upload, remote, local, size, false)
    }

    /// 远端目录树 → 本地目录。整棵树一行；两端路径都按目录理解，不拼文件名。
    pub(in crate::ui) fn enqueue_download_tree(
        &mut self,
        remote: &str,
        local: &str,
    ) -> Option<u64> {
        self.push(Direction::Download, remote, local, 0, true)
    }

    /// 本地目录树 → 远端目录。整棵树一行。
    pub(in crate::ui) fn enqueue_upload_tree(&mut self, local: &str, remote: &str) -> Option<u64> {
        self.push(Direction::Upload, remote, local, 0, true)
    }

    fn push(
        &mut self,
        direction: Direction,
        remote: &str,
        local: &str,
        size: u64,
        tree: bool,
    ) -> Option<u64> {
        // 同一个远端路径在队列里只允许一条。树还要多挡两种重叠：新路径落在
        // 一棵活树里、或新树把一条活路径罩进去——事件只带路径，重叠了必然归错行。
        // 比模型层的「同名 + 同方向 + 未结束」更严一档，理由很硬：事件不带方向，
        // 同路径的两条传输会把进度记到彼此身上。
        if self.path_in_use(remote, tree) {
            return None;
        }
        // 模型层还会按「同名 + 同方向」再挡一次；名字就是远端路径，两道闸同向。
        let model_size = if tree { TREE_SIZE_UNKNOWN } else { size };
        let id = self.queue.enqueue(remote, model_size, direction)?;
        self.tasks.insert(
            id,
            Task {
                direction,
                remote: remote.to_string(),
                local: local.to_string(),
                error: None,
                tree,
                tree_done: 0,
                tree_partial: 0,
                tree_total: 0,
            },
        );
        self.by_path.insert(remote.to_string(), id);
        if tree {
            self.tree_roots.insert(remote.to_string(), id);
        }
        Some(id)
    }

    /// 路径是否已经被占着（或与一棵树重叠）。
    ///
    /// `by_path` / `tree_roots` 里的映射本身就是「还占着」的定义：终态事件一到
    /// [`XferQueue::forget_path`] 就摘掉，跑着时点取消则**特意留着**等迟到的
    /// `Cancelled` 事件（`cancel` 的注释）。所以这里不再看 `JobState`——照着表
    /// 判就行，另按状态放宽反而会把「事件还没回来」的窗口放行出去。
    fn path_in_use(&self, remote: &str, tree: bool) -> bool {
        for (path, id) in &self.by_path {
            let Some(task) = self.tasks.get(id) else {
                continue;
            };
            if path == remote {
                return true;
            }
            // 新路径落在一棵树里：树的文件事件走前缀匹配，另起一行会抢走它们。
            if task.tree && is_under(remote, path) {
                return true;
            }
            // 新树把一条已有路径罩进去：反查表会同时命中精确与前缀两条路。
            if tree && is_under(path, remote) {
                return true;
            }
        }
        false
    }

    /// 进度。`path` 是远端路径（见模块头）。
    pub(in crate::ui) fn on_progress(&mut self, path: &str, transferred: u64) {
        let Some(id) = self.match_path(path).map(PathMatch::id) else {
            return;
        };
        // 树行按「已收尾文件字节 + 当前文件进度」累计：每个文件的进度都从 0
        // 重来，直接上报单文件值会让累计值往回走（模型层会把它当迟到事件丢掉）。
        let value = match self.tasks.get_mut(&id) {
            Some(task) if task.tree => {
                task.tree_partial = transferred;
                task.tree_done.saturating_add(task.tree_partial)
            }
            _ => transferred,
        };
        if self.queue.progress(id, value) {
            if let Some(task) = self.tasks.get_mut(&id) {
                task.error = None;
            }
        }
    }

    /// 一次传输成功收尾。`bytes` 是落盘大小（树根那条是整棵树的总字节）。
    ///
    /// 写操作（mkdir / rename / remove / chmod）也会发 `Done { bytes: 0 }`，但它们
    /// 不在这张表里，`match_path` 会先返回 `None`——队列只认自己发出去的请求。
    pub(in crate::ui) fn on_done(&mut self, path: &str, bytes: u64) {
        let Some(found) = self.match_path(path) else {
            return;
        };
        match found {
            PathMatch::TreeFile(id) => {
                // 文件级 Done 只记账、不结算：整棵树要等树根那条 Done。
                // 第一个文件就 finish 的话，后面文件的事件再也找不到 id。
                let done = {
                    let Some(task) = self.tasks.get_mut(&id) else {
                        return;
                    };
                    // 进度被节流过，最后一次 Progress 可能小于落盘大小；取较大者
                    // 保证累计单调不减。
                    task.tree_done = task.tree_done.saturating_add(bytes.max(task.tree_partial));
                    task.tree_partial = 0;
                    task.tree_done
                };
                let _ = self.queue.progress(id, done);
            }
            PathMatch::Exact(id) => {
                // 树根收尾：`bytes` 是整棵树的总字节，分母到这里才确定。
                // 单文件的 `bytes` 不用——模型层 `Done` 按入队时的 size 全额计入。
                // 只有真正在跑的树才认领它：失败 / 取消之后迟到的重复 Done
                // 不许改写总字节（`finish` 会因状态不是 Running 自动空转）。
                let is_live_tree = self.tasks.get(&id).is_some_and(|task| task.tree)
                    && self
                        .state_of(id)
                        .is_some_and(|state| matches!(state, JobState::Running { .. }));
                if is_live_tree {
                    if let Some(task) = self.tasks.get_mut(&id) {
                        task.tree_total = bytes;
                    }
                }
                // `Done` 不读 `now`（它不排重试），传 0 是「这个分支不参与时间
                // 判定」的显式声明，不是漏了时钟。
                self.finish(id, Ok(()), 0);
            }
        }
    }

    /// 一次传输失败。`message` 是服务端的原话——转述成「传输失败」等于把用户
    /// 唯一能拿到的线索擦掉。树的文件级 Failed 也走这里（整棵树一起失败）。
    pub(in crate::ui) fn on_failed(&mut self, path: &str, message: &str, now: u64) {
        let Some(id) = self.match_path(path).map(PathMatch::id) else {
            return;
        };
        if let Some(task) = self.tasks.get_mut(&id) {
            task.error = Some(message.to_string());
        }
        // 这一层不替服务端给失败分类：`Event::Failed` 只带一句原话，`FailReason`
        // 的五档在这里分不出真假。`Rejected`（对端不答应）是最不撒谎的一档，
        // 真正的信息在 `message` 里，面板原样显示。
        self.finish(id, Err(FailReason::Rejected), now);
    }

    /// 用户取消之后后台报的那一下（树传输挂在树根上）。
    pub(in crate::ui) fn on_cancelled(&mut self, path: &str) {
        let Some(id) = self.match_path(path).map(PathMatch::id) else {
            return;
        };
        // 取消不进退避（`queue.rs::finish` 收到 `Cancelled` 直接终态），`now` 不参与。
        self.finish(id, Err(FailReason::Cancelled), 0);
    }

    /// 收尾 + 摘表。返回值那张表要不要摘，取决于模型层把它判成了什么：
    /// 还在退避窗口里的失败**仍然占着这个路径**（它早晚要再传一次），
    /// 只有 `Done` / `Cancelled` / 终态 `Failed` 才放开，让同一个文件能重新入队。
    fn finish(&mut self, id: u64, result: Result<(), FailReason>, now: u64) {
        // 不看 `finish` 的返回值：用户在跑着的时候点过取消，模型层早已是
        // `Cancelled`，这里会返回 `false`——但那一下同样意味着「这一项结束了」，
        // 表还是要摘，否则这个路径从此再也传不了。
        self.queue.finish(id, result, now);
        if self.state_of(id).is_some_and(|state| state.is_settled()) {
            // 从任务登记的**根路径**摘表，而不是事件带来的 path：树的文件级
            // Failed 也走这里，那种 path 是子文件路径，用它会把根映射留在表里。
            if let Some(task) = self.tasks.get(&id) {
                let path = task.remote.clone();
                self.forget_path(id, &path);
            }
        }
    }

    /// 摘掉「路径 → id」，但只摘指向这一条的那份：同一个路径可能已经重新入队，
    /// 那条新的映射不能被旧事件的收尾连坐。树任务还要摘 `tree_roots`。
    fn forget_path(&mut self, id: u64, path: &str) {
        if self.by_path.get(path) == Some(&id) {
            self.by_path.remove(path);
        }
        if self.tree_roots.get(path) == Some(&id) {
            self.tree_roots.remove(path);
        }
    }

    /// 事件路径 → 行。精确命中优先（树根的收尾事件与单文件都走这里），
    /// 没命中再按前缀找**正在跑**的树。
    fn match_path(&self, path: &str) -> Option<PathMatch> {
        if let Some(id) = self
            .by_path
            .get(path)
            .copied()
            .filter(|id| self.tasks.contains_key(id))
        {
            return Some(PathMatch::Exact(id));
        }
        // 前缀只认「正在跑的树」：后台串行、并行上限 1，同一时刻只有一棵树在发
        // 事件；排队中 / 退避中的树不该被迟到的事件认领。逆序遍历让更深的树根
        // 优先（`/srv/a` 先于 `/srv`），嵌套时不会归错行。
        for (root, id) in self.tree_roots.iter().rev() {
            let running = self
                .state_of(*id)
                .is_some_and(|state| matches!(state, JobState::Running { .. }));
            if running && self.tasks.contains_key(id) && is_under(path, root) {
                return Some(PathMatch::TreeFile(*id));
            }
        }
        None
    }

    fn job(&self, id: u64) -> Option<&Job> {
        self.queue.jobs().into_iter().find(|job| job.id == id)
    }

    /// 某一项现在的状态（面板据此决定给「取消」还是「重试」）。
    pub(in crate::ui) fn state_of(&self, id: u64) -> Option<JobState> {
        self.job(id).map(|job| job.state.clone())
    }

    /// 取下一项该跑的。`None` = 没有待跑的，或者前一项还没结束（并行上限 1）。
    pub(in crate::ui) fn next_to_run(&mut self, now: u64) -> Option<XferTask> {
        let started = self.queue.start_next(now);
        let id = *started.first()?;
        debug_assert_eq!(started.len(), 1, "并行上限 1 时不该一次启动多项");
        // 重跑（退避重试）时把上一次的树进度清零：模型层已经把 transferred
        // 归 0，这一层记的「已收尾文件字节 / 总字节」也要跟着归零，否则新一
        // 轮的进度从旧值起跳。
        if let Some(task) = self.tasks.get_mut(&id) {
            if task.tree {
                task.tree_done = 0;
                task.tree_partial = 0;
                task.tree_total = 0;
            }
        }
        let task = self.tasks.get(&id)?;
        Some(XferTask {
            id,
            direction: task.direction,
            remote: task.remote.clone(),
            local: task.local.clone(),
            tree: task.tree,
        })
    }

    /// 取消一项。返回 `true` 表示模型层真的把它标成了取消——**跑着的那一项还要
    /// 调用方另外去叫后台停**：模型层只改状态，停传输是持 socket 那一侧的事
    /// （见 `queue.rs::cancel` 的注释）。
    pub(in crate::ui) fn cancel(&mut self, id: u64) -> bool {
        let Some(state) = self.state_of(id) else {
            return false;
        };
        if !self.queue.cancel(id) {
            return false;
        }
        // 跑着的那一项还有一条 `Cancelled` 事件在路上：映射要留到事件回来再摘，
        // 否则同一个路径重新入队后，那条迟到的事件会把新任务当成自己的（串台）。
        // 还没开跑的项不会再有事件，当场摘掉。
        if !matches!(state, JobState::Running { .. }) {
            if let Some(task) = self.tasks.get(&id) {
                let path = task.remote.clone();
                self.forget_path(id, &path);
            }
        }
        true
    }

    /// 把退避到点的失败项退回 `Pending`。**每帧调一次**，不是按钮。
    pub(in crate::ui) fn retry(&mut self, now: u64) -> Vec<u64> {
        self.queue.retry_due(now)
    }

    /// 用户点了「重试」：不等退避窗口。返回被提前的项数。
    ///
    /// 手法是 `retry_due(u64::MAX)`——模型层只给了「按 `now` 批量到期」这一个入口，
    /// 没有「把某一项立刻到期」（这层不许改模型层）。所以按一颗按钮会把**所有**
    /// 待退避项一起提前。代价可接受：并行上限是 1、退避只有 2/4/8 秒，那些项本来
    /// 也就在这几秒里会自己到期；反过来（为了只提前一条而给一颗点了没反应的按钮）
    /// 是明令禁止的。
    pub(in crate::ui) fn retry_now(&mut self) -> usize {
        self.queue.retry_due(u64::MAX).len()
    }

    /// 总进度：`(已完成字节, 总字节)`。分母不含已取消项，所以进度能到 100%。
    ///
    /// 不直接调 `self.queue.overall()`：树行入队时写的是
    /// [`TREE_SIZE_UNKNOWN`] 哨兵（只为不钳累计进度），把它算进分母会得到
    /// 「16 EB」这种假数。树行的分母用 [`Task::tree_total`]——树根 `Done` 之前
    /// 是 0（总字节无从得知，不编），之后才是确切总字节。
    pub(in crate::ui) fn overall(&self) -> (u64, u64) {
        let mut done = 0u64;
        let mut total = 0u64;
        for job in self.queue.jobs() {
            if matches!(job.state, JobState::Cancelled) {
                continue;
            }
            let size = match self.tasks.get(&job.id) {
                Some(task) if task.tree => task.tree_total,
                _ => job.size,
            };
            total = total.saturating_add(size);
            let bytes = match &job.state {
                JobState::Done => size,
                JobState::Running { transferred } => (*transferred).min(size),
                _ => 0,
            };
            done = done.saturating_add(bytes);
        }
        (done, total)
    }

    /// 正在跑几项（等待退避的不算——它没在传）。
    pub(in crate::ui) fn running(&self) -> usize {
        self.queue.running()
    }

    pub(in crate::ui) fn len(&self) -> usize {
        self.queue.len()
    }

    pub(in crate::ui) fn is_empty(&self) -> bool {
        self.queue.is_empty()
    }

    /// 全部结束？推进逻辑据此决定还要不要每帧叫醒自己。
    pub(in crate::ui) fn all_settled(&self) -> bool {
        self.queue.all_settled()
    }

    /// 全部行的快照，按入队顺序（模型层的顺序就是这么定的）。
    pub(in crate::ui) fn rows(&self) -> Vec<XferRow> {
        self.queue
            .jobs()
            .into_iter()
            .filter_map(|job| {
                let task = self.tasks.get(&job.id)?;
                Some(XferRow {
                    id: job.id,
                    direction: job.direction,
                    remote: task.remote.clone(),
                    name: file_name(&task.remote),
                    // 树行不把哨兵透给界面：分母就是逐文件学到 / 收尾时确定的
                    // 总字节，还没收尾时是 0（面板因此只写「传输中」，不编百分比）。
                    size: if task.tree { task.tree_total } else { job.size },
                    state: job.state.clone(),
                    attempts: job.attempts,
                    error: task.error.clone(),
                })
            })
            .collect()
    }
}

/// 路径的最后一段。传进来的可能是 `/srv/a/b.txt`、`C:\x\b.txt` 或一个裸名字。
///
/// 不区分分隔符是有意的：远端路径来自服务端（`/`），本地路径来自 `PathBuf`
/// （Windows 下是 `\`），而这里只为一个用途服务——把一行名字画窄。
fn file_name(path: &str) -> String {
    path.rsplit(['/', '\\'])
        .find(|segment| !segment.is_empty())
        .unwrap_or(path)
        .to_string()
}

/// `child` 是不是 `parent` 的**真后代**（必须隔一个 `/`）。
///
/// 只比 `starts_with` 会把 `/srv2/a` 认成 `/srv` 的孩子；树根末尾带 `/` 时先去掉，
/// 根是 `/` 时所有绝对路径都是它的后代。
fn is_under(child: &str, parent: &str) -> bool {
    let base = parent.trim_end_matches('/');
    if base.is_empty() {
        return child.starts_with('/');
    }
    child.len() > base.len()
        && child.starts_with(base)
        && child.as_bytes().get(base.len()) == Some(&b'/')
}

#[cfg(test)]
#[path = "xfer_queue_tests.rs"]
mod tests;

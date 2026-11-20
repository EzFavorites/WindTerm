//! 目录同步：两张清单进，一份计划出；计划再交给执行器按顺序推进。
//!
//! 这是 XFER-06（终端目录 ↔ 文件管理器同步）的**纯决策层 + 执行记账层**：调用方把
//! 本地与远端的目录清单各扫一遍，塞进来，拿到「每一条该上传 / 下载 / 删除 / 跳过」
//! （[`plan`]）；再用 [`SyncRun`] 按 [`execution_order`] 一项一项往下走，边走边记
//! 进度。真正的读写盘、SFTP 请求、进度条都不在这里——所以本模块依旧是**零 IO**，
//! 可以在任何线程上跑，也可以在测试里用纯数据把每条规则钉死。
//!
//! # 判定表
//!
//! 记 `L` 为本地项、`R` 为远端项：
//!
//! | 情形 | 结果 |
//! |---|---|
//! | 只有 `L` | 策略为 [`DeletePolicy::DeleteLocal`] 时 `DeleteLocal`，否则 `Upload` |
//! | 只有 `R` | 策略为 [`DeletePolicy::DeleteRemote`] 时 `DeleteRemote`，否则 `Download` |
//! | 两边都有，都是目录 | `Skip`（目录本身只表达结构，子项各自成条） |
//! | 两边都有，类型不同 | `Upload`（以本地为准，由调用方决定如何替换对端） |
//! | 两边都有，`size` 与 `mtime` 都相同 | `Skip` |
//! | 两边都有，`L.mtime > R.mtime` | `Upload` |
//! | 两边都有，`R.mtime > L.mtime` | `Download` |
//! | 两边都有，`mtime` 相同但 `size` 不同 | `Upload`（mtime 分不出先后，以本地为准） |
//!
//! # 三条刻意的取舍
//!
//! 1. **`mtime` 精确比较，不做容差。** [`SyncEntry::mtime`] 是**秒**级 Unix 时间戳，
//!    两份清单必须用同一口径采集（同一时钟源、同一次精度归一化）。把「2 秒容差」
//!    这类策略塞进纯函数，会让「为什么这条没同步」变得无法解释；需要容差时在采集
//!    阶段就把时间戳归一化掉。
//! 2. **删除是显式策略，默认不删。** 默认 [`DeletePolicy::Keep`]——同步工具的默认
//!    行为绝不能是删用户文件，尤其是「扫描时远端恰好没列出来」这种误判的情形。
//! 3. **输出按路径升序，且两份清单的并集全部出现在计划里。** 顺序稳定是为了让
//!    UI 列表与测试断言都不用再排一次；`Skip` 也保留是为了让界面能显示「共 N 项，
//!    其中 M 项已一致」，而不是只剩待办项让人以为文件丢了。

use std::collections::BTreeMap;

/// 清单里的一项：本模块只关心这四个字段。
///
/// 不借用调用方的结构体，是为了让这一层零耦合——调用方扫盘/扫 SFTP 得到的任何
/// 类型，填成这个 struct 即可。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SyncEntry {
    /// 相对同步根的路径，用 `/` 分隔（两端统一，本模块不解析也不比较父子关系）。
    pub path: String,
    /// 是否目录。目录不比较 `size` / `mtime`。
    pub is_dir: bool,
    /// 文件字节数；目录恒为 `0`。
    pub size: u64,
    /// 修改时间，Unix 纪元起的**秒**数。允许为负（1970 年之前的文件）。
    pub mtime: i64,
}

impl SyncEntry {
    /// 造一个文件条目。
    pub fn file(path: impl Into<String>, size: u64, mtime: i64) -> Self {
        Self {
            path: path.into(),
            is_dir: false,
            size,
            mtime,
        }
    }

    /// 造一个目录条目（`size` 恒为 0，`mtime` 不参与比较）。
    pub fn dir(path: impl Into<String>, mtime: i64) -> Self {
        Self {
            path: path.into(),
            is_dir: true,
            size: 0,
            mtime,
        }
    }
}

/// 单选题：某一侧独有的条目怎么处理。
///
/// 默认 [`DeletePolicy::Keep`]：「只在一侧存在」一律视为需要补齐到另一侧，永不删除。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum DeletePolicy {
    /// 不删任何东西：本地独有 → 上传，远端独有 → 下载。
    #[default]
    Keep,
    /// 删除本地多余项（远端没有的本地文件），即让本地镜像远端。
    ///
    /// 与 [`DeletePolicy::DeleteRemote`] 互斥：想两边都删就不该用「同步」，
    /// 而是先确认哪边是权威副本。
    DeleteLocal,
    /// 删除远端多余项（本地没有的远端文件），即让远端镜像本地。
    DeleteRemote,
}

/// 计划里一条要做的事。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SyncOp {
    /// 本地 → 远端：新增或覆盖。
    Upload,
    /// 远端 → 本地：新增或覆盖。
    Download,
    /// 删除本地这份（远端没有它）。
    DeleteLocal,
    /// 删除远端这份（本地没有它）。
    DeleteRemote,
    /// 已一致，什么都不做。
    Skip,
}

impl SyncOp {
    /// 是否真的会产生写操作（`Upload` / `Download` / 两种 `Delete`）。
    ///
    /// `Skip` 返回 `false`，方便调用方算「待办 N 项」。
    pub fn is_mutation(self) -> bool {
        !matches!(self, SyncOp::Skip)
    }
}

/// 计划里的一项：条目本身 + 对它做什么。
///
/// [`SyncItem::entry`] 取的是**被操作的那一份**：`Upload` / `DeleteLocal` / `Skip`
/// 用本地条目，`Download` / `DeleteRemote` 用远端条目。`Skip` 取本地是因为跳过时
/// 两份等价（目录除外，而目录两边都没有可比较的字段）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SyncItem {
    /// 该条目。
    pub entry: SyncEntry,
    /// 对该条目做什么。
    pub op: SyncOp,
}

/// 出计划：输入两份清单与删除策略，输出按路径升序的完整计划。
///
/// 两份清单的**并集**都会出现在结果里（包括 [`SyncOp::Skip`] 的项）；两份都为空时
/// 返回空 `Vec`。同一路径在入参里重复出现时，**后出现者覆盖先出现者**——这既让
/// 上游的「扫描中途补了一条」自然生效，也让结果与遍历顺序无关（`BTreeMap` 只按
/// 路径排序）。
///
/// 本函数不读文件系统、不看时钟、不分配后台任务，纯 `&[SyncEntry]` → `Vec<SyncItem>`。
pub fn plan(local: &[SyncEntry], remote: &[SyncEntry], deletes: DeletePolicy) -> Vec<SyncItem> {
    // 路径 → (本地项, 远端项)。用 BTreeMap 顺带把输出排好序。
    let mut merged: BTreeMap<&str, (Option<&SyncEntry>, Option<&SyncEntry>)> = BTreeMap::new();
    for entry in local {
        merged.entry(entry.path.as_str()).or_default().0 = Some(entry);
    }
    for entry in remote {
        merged.entry(entry.path.as_str()).or_default().1 = Some(entry);
    }

    merged
        .into_values()
        .map(|(local, remote)| {
            let (entry, op) = match (local, remote) {
                (Some(l), Some(r)) => {
                    let op = compare(l, r);
                    // 下载携带远端条目（它的 size/mtime 才是要被复制过来的那份）。
                    let entry = if op == SyncOp::Download { r } else { l };
                    (entry, op)
                }
                (Some(l), None) => (
                    l,
                    match deletes {
                        DeletePolicy::DeleteLocal => SyncOp::DeleteLocal,
                        DeletePolicy::Keep | DeletePolicy::DeleteRemote => SyncOp::Upload,
                    },
                ),
                (None, Some(r)) => (
                    r,
                    match deletes {
                        DeletePolicy::DeleteRemote => SyncOp::DeleteRemote,
                        DeletePolicy::Keep | DeletePolicy::DeleteLocal => SyncOp::Download,
                    },
                ),
                // `merged` 的键来自某一侧，不可能两侧都空。
                (None, None) => unreachable!("plan() 的键必来自本地或远端清单"),
            };
            SyncItem {
                entry: entry.clone(),
                op,
            }
        })
        .collect()
}

/// 两侧都有的条目怎么定。
fn compare(local: &SyncEntry, remote: &SyncEntry) -> SyncOp {
    if local.is_dir && remote.is_dir {
        // 目录的存在性由子项体现；目录 mtime 会因子项增删而变，拿来比较只会误判。
        return SyncOp::Skip;
    }
    if local.is_dir != remote.is_dir {
        // 一边文件一边目录：类型冲突，以本地为准（调用方负责替换对端）。
        return SyncOp::Upload;
    }
    if local.size == remote.size && local.mtime == remote.mtime {
        return SyncOp::Skip;
    }
    if local.mtime > remote.mtime {
        return SyncOp::Upload;
    }
    if remote.mtime > local.mtime {
        return SyncOp::Download;
    }
    // mtime 相同但大小不同：时间戳分不出先后，按约定以本地为准。
    SyncOp::Upload
}

// ── 执行层 ──────────────────────────────────────────────────────────────
//
// `plan()` 只回答「做什么」，不回答「按什么顺序做」。顺序错了会真的出错，而且报的
// 是对端的一句话，看不出根因是排序：
//
// - **建**必须先有父目录：上传 `a/b.txt` 之前 `a` 必须已经在，否则只拿到一句
//   「No such file or directory」——看起来像路径写错了。
// - **删**必须先删子项：先删 `a` 会失败（目录非空），于是整棵子树留在原地，而调用
//   方只看到「删除失败」。
//
// 所以顺序不是「顺手排一下」，而是这一层要担保的契约；失败连坐同理（见 [`SyncRun`]）。

/// 一步的结局。
///
/// 只分「成 / 不成」：**失败原因由调用方归类**（对端拒绝、权限不足、磁盘满、连接断），
/// 这一层没有 socket 也没有文件系统，猜出来的原因只会误导人。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum StepResult {
    /// 做成了。
    Done,
    /// 没做成。
    Failed,
}

/// 执行顺序：给出 [`SyncItem`] 下标序列。
///
/// 两条规则：
///
/// 1. 需要写的项（[`SyncOp::is_mutation`]）按**路径升序**。路径是前缀式编码，父目录
///    恒排在子项之前，所以「先建目录再放文件」自动成立。
/// 2. 删除项排在**所有写操作之后**，彼此按**路径降序**——子项先于父目录，否则父目录
///    非空、删不掉。
///
/// [`SyncOp::Skip`] 不进序列：它们不需要执行，但仍留在计划里给界面显示「共 N 项，
/// 其中 M 项已一致」。删除排在最后还有一层意思：先把该补的补上，最后才收拾多余的；
/// 中途失败时用户看到的是「多了一点」而不是「少了一点」——前者可恢复，后者会让人
/// 以为文件丢了。
pub fn execution_order(items: &[SyncItem]) -> Vec<usize> {
    let mut writes: Vec<usize> = Vec::new();
    let mut deletes: Vec<usize> = Vec::new();
    for (index, item) in items.iter().enumerate() {
        match item.op {
            SyncOp::Skip => {}
            SyncOp::DeleteLocal | SyncOp::DeleteRemote => deletes.push(index),
            SyncOp::Upload | SyncOp::Download => writes.push(index),
        }
    }
    // 重新排序而不是「信 `plan()` 已经排好」：调用方可能自己拼计划、只取其中几项、
    // 或者在后头又追加了几条。
    writes.sort_by(|a, b| items[*a].entry.path.cmp(&items[*b].entry.path));
    deletes.sort_by(|a, b| items[*b].entry.path.cmp(&items[*a].entry.path));
    writes.extend(deletes);
    writes
}

/// 进度快照。界面直接画，不必自己再数一遍。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Progress {
    /// 计划总项数（含本就一致的项）。
    pub total: usize,
    /// 本来就一致、不需要动的项数。
    pub already: usize,
    /// 需要执行的项数。
    pub todo: usize,
    /// 已做成。
    pub done: usize,
    /// 已失败。
    pub failed: usize,
    /// 因父目录那一步失败而没被尝试的项数。
    pub skipped: usize,
}

impl Progress {
    /// 还剩几项要动。
    pub fn remaining(&self) -> usize {
        self.todo.saturating_sub(self.done + self.failed + self.skipped)
    }
}

/// 同步执行器：按 [`execution_order`] 一项一项往外发并记账，**不碰 IO**。
///
/// 调用方的循环长这样：
///
/// ```text
/// while let Some(item) = run.step() {
///     let result = do_the_io(item);   // 上传 / 下载 / 删除，都在调用方手里
///     run.record(result);
/// }
/// let progress = run.progress();
/// ```
///
/// # 为什么失败不中止
///
/// 一个文件传不上去（权限、磁盘满、对端拒绝）不该让其余九百个文件停在原地——那才是
/// 更糟的结果。要不要中止是**调用方**的决定：它知道这一趟是「同步整个项目」还是
/// 「补一个文件」。
///
/// 但这一层做了一件保守动作：**目录那一步失败时，跳过它下面同操作的子项**。那些步骤
/// 注定失败，挨个撞一遍只会用 N 条一模一样的错误盖住真正的原因。连坐限定「同操作」，
/// 因为上传失败不影响下载。
pub struct SyncRun {
    items: Vec<SyncItem>,
    order: Vec<usize>,
    cursor: usize,
    done: usize,
    failed: usize,
    skipped: usize,
    /// 失败过的目录：`(路径, 操作)`。只连坐**同一操作**的子项。
    broken: Vec<(String, SyncOp)>,
}

impl SyncRun {
    /// 接一份计划。计划原文保留，可用 [`SyncRun::items`] 取回给界面画列表。
    pub fn new(items: Vec<SyncItem>) -> Self {
        let order = execution_order(&items);
        Self {
            items,
            order,
            cursor: 0,
            done: 0,
            failed: 0,
            skipped: 0,
            broken: Vec::new(),
        }
    }

    /// 当前该做的一步；跑完了给 `None`。
    ///
    /// 连坐的子项在这里就地跳过（并计入 [`Progress::skipped`]），调用方不必知道这条
    /// 规则存在。
    pub fn step(&mut self) -> Option<&SyncItem> {
        loop {
            let index = *self.order.get(self.cursor)?;
            let blocked = {
                let item = &self.items[index];
                self.blocked_by_broken_parent(item)
            };
            if blocked {
                self.skipped += 1;
                self.cursor += 1;
                continue;
            }
            return Some(&self.items[index]);
        }
    }

    /// 记下当前一步的结局并前进。
    ///
    /// 已经没有当前步骤时什么都不做——调用方多记一次不该 panic（那会把一个记账疏漏
    /// 变成一场崩溃）。
    pub fn record(&mut self, result: StepResult) {
        let Some(index) = self.order.get(self.cursor).copied() else {
            return;
        };
        let item = &self.items[index];
        match result {
            StepResult::Done => self.done += 1,
            StepResult::Failed => {
                self.failed += 1;
                if item.entry.is_dir {
                    self.broken.push((item.entry.path.clone(), item.op));
                }
            }
        }
        self.cursor += 1;
    }

    /// 是否已经跑完。
    pub fn is_finished(&self) -> bool {
        self.cursor >= self.order.len()
    }

    /// 进度快照。
    pub fn progress(&self) -> Progress {
        Progress {
            total: self.items.len(),
            already: self.items.iter().filter(|i| i.op == SyncOp::Skip).count(),
            todo: self.order.len(),
            done: self.done,
            failed: self.failed,
            skipped: self.skipped,
        }
    }

    /// 计划原文（含 `Skip` 项）。
    pub fn items(&self) -> &[SyncItem] {
        &self.items
    }

    fn blocked_by_broken_parent(&self, item: &SyncItem) -> bool {
        self.broken
            .iter()
            .any(|(dir, op)| *op == item.op && is_under(&item.entry.path, dir))
    }
}

/// `path` 是否在目录 `dir` 底下（不含 `dir` 自己）。
///
/// 只认 `/` 分隔：这是 [`SyncEntry::path`] 的约定。`a/b` 在 `a` 下，`ab/c` 不在。
fn is_under(path: &str, dir: &str) -> bool {
    if dir.is_empty() {
        return false;
    }
    path.len() > dir.len()
        && path.starts_with(dir)
        && path.as_bytes().get(dir.len()) == Some(&b'/')
}

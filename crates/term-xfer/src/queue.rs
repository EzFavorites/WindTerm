//! 多文件传输队列：谁先传、失败重试几次、同时传几个、总进度多少。
//!
//! 这是「一次传 20 个文件」的**纯模型层**。UI 只做两件事：把用户选的条目丢进
//! [`Queue::enqueue`]，以及按固定的节拍（比如每 200ms）把一批事件报进来
//! （[`Queue::progress`] / [`Queue::finish`] / [`Queue::cancel`]）。「排队顺序、
//! 并行上限、退避重试、总进度」全部在这里算，界面只负责画——把这些规则写在 UI 里
//! 是历来的错误来源：列表刷新的时机、用户点「重试」的时机、宿主时钟的跳变，都会
//! 让同一份策略在不同界面上表现不一致。
//!
//! # 纪律（与 [`crate::sync`] 同一口径）
//!
//! **零 IO、零线程、零时钟。** 所有与时间有关的判定都靠调用方注入的 `now`
//! （Unix 秒，任意单调口径均可，本模块只做整数比较与加法）。所以本模块可以被
//! 纯数据单测钉死，也可以在任意线程上跑。
//!
//! # 状态机
//!
//! ```text
//!                 start_next                 finish(Ok)
//!   Pending ─────────────────► Running{0} ─────────────────► Done
//!      │  ▲                        │
//!      │  │ retry_due(now>=retry_at)│ finish(Err) 且还有重试额度
//!      │  └──────── Failed{retry_at:Some(t)}
//!      │                           │ finish(Err) 且 attempts >= max_retries
//!      │                           ▼
//!      │                     Failed{retry_at:None}  （终态，永不回 Pending）
//!      ├── cancel ──────────────► Cancelled
//!      └────────── cancel ──────► Cancelled
//! ```
//!
//! # 判定表
//!
//! | 问题 | 判定 |
//! |---|---|
//! | 一次能跑几个 | `min(Running 数量 <= max_parallel.max(1))`；`0` 按 `1` 处理 |
//! | 先跑哪个 | 按入队顺序（即 id 升序）；只有 `Pending` 能被启动 |
//! | 第 n 次重试等多久 | `min(retry_backoff * 2^(n-1), 300)` 秒；`retry_backoff = 0` 即刻可重试 |
//! | 什么时候能重试 | `retry_at = now + 等待时长`，绝对时刻；`retry_due(now)` 只看 `retry_at <= now` |
//! | 什么时候彻底失败 | `attempts >= max_retries` 时 `finish(Err)` 直接终态，`attempts` 不再增长 |
//! | 什么算重复 | 同名 + 同方向 + **尚未结束**；`Done` / `Cancelled` / 终态 `Failed` 不参与 |
//! | 总进度分母 | 未取消项的大小之和；取消项既不计入分子也不计入分母 |
//!
//! # 两处需要解释的取舍
//!
//! 1. **`Failed{retry_at: Some}` 不占并行位。** 只有 `Running` 计入
//!    [`Queue::running`]：等待退避的项没有在传输，占着名额只会让并行度凭空跌掉。
//!    它在重复判定里仍算「未结束」，因为同一文件早晚要再传一次。
//! 2. **`finish(Err(Cancelled))` 落到 `Cancelled` 而不是 `Failed`。** 用户主动取消
//!    不是故障，不该进入退避重试队列；否则「取消」这个动作会在几秒后自己复活。

/// 传输方向。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Direction {
    /// 本地 → 远端。
    Upload,
    /// 远端 → 本地。
    Download,
}

/// 失败原因。由传输层归类后上报，本模块只按「要不要重试」区分。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FailReason {
    /// 协议错误：校验失败、对端连续 NAK、帧格式不符。
    Protocol,
    /// 本地 IO 错误：打开 / 读写 / 落盘失败。
    Io,
    /// 对端拒绝：YMODEM 头块被拒、权限不足、远端磁盘满。
    Rejected,
    /// 对端静默超时（握手或等 ACK 超时）。
    Timeout,
    /// 用户取消。[`Queue::finish`] 收到它时直接置为 [`JobState::Cancelled`]，不重试。
    Cancelled,
}

/// 单项的状态。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum JobState {
    /// 在队列里等着被启动。
    Pending,
    /// 正在传输。
    Running {
        /// 已传字节数。单调不减，且不超过 [`Job::size`]。
        transferred: u64,
    },
    /// 传完了。
    Done,
    /// 失败了。
    Failed {
        /// 失败原因。
        reason: FailReason,
        /// 允许重试的绝对时刻（与 `now` 同口径）。`None` 表示不再重试，即终态。
        retry_at: Option<u64>,
    },
    /// 被取消：用户主动取消，或 [`Queue::finish`] 收到 [`FailReason::Cancelled`]。
    Cancelled,
}

impl JobState {
    /// 是否是终态：`Done` / `Cancelled` / 不再重试的 `Failed`。
    ///
    /// 只有终态的项才允许「同名同方向再入队一次」。
    pub fn is_settled(&self) -> bool {
        match self {
            JobState::Done | JobState::Cancelled => true,
            JobState::Failed { retry_at, .. } => retry_at.is_none(),
            JobState::Pending | JobState::Running { .. } => false,
        }
    }
}

/// 队列里的一项。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Job {
    /// 入队时分配的编号：从 `1` 开始单调递增，永不复用。
    pub id: u64,
    /// 文件名。只用于展示与重复判定，本模块不解析路径，也不比较大小写。
    pub name: String,
    /// 文件字节数。
    pub size: u64,
    /// 传输方向。
    pub direction: Direction,
    /// 当前状态。
    pub state: JobState,
    /// 已经安排过几次重试（不是失败次数：终态失败那次不计入）。
    pub attempts: u32,
}

/// 队列策略。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Policy {
    /// 同时处于 `Running` 的上限。`0` 视作 `1`。
    pub max_parallel: usize,
    /// 单项最多重试几次。`0` 表示首次失败即终态。
    pub max_retries: u32,
    /// 退避基准，单位秒：第 n 次重试等 `min(retry_backoff * 2^(n-1), 300)`；
    /// `0` 表示失败后立刻可重试。
    pub retry_backoff: u64,
}

/// 多文件传输队列。
///
/// 内部只有一张按 id 升序的 `Vec<Job>`：只追加、不删除、不重排，所以
/// [`Queue::jobs`] 的快照顺序天然稳定（这是能让 UI 基线与测试断言不抖的前提）。
pub struct Queue {
    policy: Policy,
    /// 只 push，不删不重排 —— 因此恒按 id 升序。
    jobs: Vec<Job>,
    /// 下一个要分配的 id。
    next_id: u64,
}

impl Queue {
    /// 建一个空队列。`policy.max_parallel = 0` 在这里不归一化，等到启动时按 `1` 处理。
    pub fn new(policy: Policy) -> Self {
        Self {
            policy,
            jobs: Vec::new(),
            next_id: 1,
        }
    }

    /// 入队，返回分配到的 id；被判为重复时返回 `None`。
    ///
    /// 重复的判据是「同名 + 同方向 + 尚未结束」（`Pending` / `Running` / 可重试的
    /// `Failed`）。已结束的项——`Done`、`Cancelled`、终态 `Failed`——不参与，因此
    /// 同一个文件传完之后可以再入队一次（重传、覆盖远端）。`size` 不参与判定：
    /// 同名同方向但大小变了，仍然视为同一个传输目标。
    pub fn enqueue(&mut self, name: impl Into<String>, size: u64, direction: Direction) -> Option<u64> {
        let name = name.into();
        let duplicated = self.jobs.iter().any(|job| {
            !job.state.is_settled() && job.direction == direction && job.name == name
        });
        if duplicated {
            return None;
        }
        let id = self.next_id;
        self.next_id += 1;
        self.jobs.push(Job {
            id,
            name,
            size,
            direction,
            state: JobState::Pending,
            attempts: 0,
        });
        Some(id)
    }

    /// 取消一项。`Pending` 与 `Running`（以及等待退避的 `Failed`）都会变成
    /// [`JobState::Cancelled`] 并返回 `true`；已结束的项返回 `false`。
    ///
    /// 已经 `Running` 的项，本模块只改状态，**真正停掉传输是调用方的事**——它手里
    /// 才有 socket 与定时器。
    pub fn cancel(&mut self, id: u64) -> bool {
        let Some(index) = self.index_of(id) else {
            return false;
        };
        let job = &mut self.jobs[index];
        if job.state.is_settled() {
            return false;
        }
        job.state = JobState::Cancelled;
        true
    }

    /// 启动下一批：按入队顺序把 `Pending` 项转为 `Running { transferred: 0 }`，
    /// 返回被启动的 id（按入队顺序）。
    ///
    /// 硬约束：启动后 `Running` 数量不超过 `max_parallel.max(1)`；名额已满时返回空
    /// 列表。`now` 不参与判定（启动本身不需要时钟），保留它只是为了与
    /// [`Queue::finish`] / [`Queue::retry_due`] 同形，宿主可以在同一个时间快照上
    /// 顺序调用三者。
    pub fn start_next(&mut self, _now: u64) -> Vec<u64> {
        let limit = self.policy.max_parallel.max(1);
        let mut running = self.running();
        let mut started = Vec::new();
        for job in &mut self.jobs {
            if running >= limit {
                break;
            }
            if job.state == JobState::Pending {
                job.state = JobState::Running { transferred: 0 };
                started.push(job.id);
                running += 1;
            }
        }
        started
    }

    /// 上报进度，返回是否被接受。
    ///
    /// 只有 `Running` 项接受上报；`transferred` 单调不减（比已记录的小就忽略并
    /// 返回 `false`，否则进度条会来回抖），超过 [`Job::size`] 时钳到 `size`
    /// （协议层的填充块、`EOT` 重传都可能让计数略微超前）。
    pub fn progress(&mut self, id: u64, transferred: u64) -> bool {
        let Some(index) = self.index_of(id) else {
            return false;
        };
        let size = self.jobs[index].size;
        let job = &mut self.jobs[index];
        let JobState::Running {
            transferred: current,
        } = &mut job.state
        else {
            return false;
        };
        let clamped = transferred.min(size);
        if clamped < *current {
            return false;
        }
        *current = clamped;
        true
    }

    /// 结束一项，返回是否被接受（只有 `Running` 项接受，其余返回 `false`）。
    ///
    /// - `Ok(())` → [`JobState::Done`]。
    /// - `Err(Cancelled)` → [`JobState::Cancelled`]，不重试。
    /// - 其它 `Err` → [`JobState::Failed`]：还有额度就安排一次重试，
    ///   `attempts` 加一并按 [`Policy::retry_backoff`] 算出绝对时刻 `retry_at`；
    ///   `attempts >= max_retries` 时是终态（`retry_at: None`），永不回 `Pending`。
    pub fn finish(&mut self, id: u64, result: Result<(), FailReason>, now: u64) -> bool {
        let policy = self.policy;
        let Some(index) = self.index_of(id) else {
            return false;
        };
        let job = &mut self.jobs[index];
        if !matches!(job.state, JobState::Running { .. }) {
            return false;
        }
        match result {
            Ok(()) => job.state = JobState::Done,
            Err(FailReason::Cancelled) => job.state = JobState::Cancelled,
            Err(reason) => {
                job.state = if job.attempts >= policy.max_retries {
                    JobState::Failed {
                        reason,
                        retry_at: None,
                    }
                } else {
                    let attempt = job.attempts.saturating_add(1);
                    job.attempts = attempt;
                    let delay = backoff(policy, attempt);
                    JobState::Failed {
                        reason,
                        retry_at: Some(now.saturating_add(delay)),
                    }
                };
            }
        }
        true
    }

    /// 把到点的可重试失败项退回 `Pending`，返回到点的 id（按 id 升序）。
    ///
    /// 只看 `retry_at <= now`；`attempts` 保留（它记的是「已经重试过几次」）。
    /// 退回后仍是 `Pending`，要跑还得下次 [`Queue::start_next`] 按并行上限捞。
    pub fn retry_due(&mut self, now: u64) -> Vec<u64> {
        let mut due = Vec::new();
        for job in &mut self.jobs {
            let ready = match &job.state {
                JobState::Failed {
                    retry_at: Some(at), ..
                } => *at <= now,
                _ => false,
            };
            if ready {
                job.state = JobState::Pending;
                due.push(job.id);
            }
        }
        due
    }

    /// 全部项的快照，按 id 升序（入队顺序）。顺序稳定是 UI 基线与测试断言的前提。
    pub fn jobs(&self) -> Vec<&Job> {
        self.jobs.iter().collect()
    }

    /// 当前 `Running` 的数量（不包含等待退避的失败项）。
    pub fn running(&self) -> usize {
        self.jobs
            .iter()
            .filter(|job| matches!(job.state, JobState::Running { .. }))
            .count()
    }

    /// 队列里的总项数（含已结束的项）。
    pub fn len(&self) -> usize {
        self.jobs.len()
    }

    /// 队列是否为空。
    pub fn is_empty(&self) -> bool {
        self.jobs.is_empty()
    }

    /// 是否全部结束（`Done` + `Cancelled` + 终态 `Failed`）。空队列视为全部结束。
    pub fn all_settled(&self) -> bool {
        self.jobs.iter().all(|job| job.state.is_settled())
    }

    /// 总进度：`(已完成字节, 总字节)`。
    ///
    /// 只统计**未取消**的项：取消项既不计入分子也不计入分母，否则总进度永远到不了
    /// 100%。`Done` 项按 `size` 全额计入（不必等最后一次 `progress` 上报），
    /// `Running` 项按已传字节（已钳到 `size`）计入，其余为 0。总字节为 0 时返回
    /// `(0, 0)`——不会除零，也不会给 UI 一个 `NaN`。
    pub fn overall(&self) -> (u64, u64) {
        let mut done = 0u64;
        let mut total = 0u64;
        for job in &self.jobs {
            if matches!(job.state, JobState::Cancelled) {
                continue;
            }
            total = total.saturating_add(job.size);
            let bytes = match &job.state {
                JobState::Done => job.size,
                JobState::Running { transferred } => (*transferred).min(job.size),
                _ => 0,
            };
            done = done.saturating_add(bytes);
        }
        (done, total)
    }

    /// 是否有可重试的失败项——UI 用它决定要不要显示「重试全部」。
    pub fn has_retryable(&self) -> bool {
        self.jobs.iter().any(|job| {
            matches!(
                job.state,
                JobState::Failed {
                    retry_at: Some(_),
                    ..
                }
            )
        })
    }

    /// 按 id 找下标。`jobs` 恒按 id 升序，所以二分即可。
    fn index_of(&self, id: u64) -> Option<usize> {
        self.jobs.binary_search_by_key(&id, |job| job.id).ok()
    }
}

/// 第 `attempt` 次重试的等待秒数：`min(retry_backoff * 2^(attempt-1), 300)`。
///
/// 乘法与移位都饱和处理：`max_retries` 很大时也不会溢出，最终由 300 秒封顶。
fn backoff(policy: Policy, attempt: u32) -> u64 {
    /// 退避上限，秒。
    const CAP: u64 = 300;
    if policy.retry_backoff == 0 {
        return 0;
    }
    let shift = attempt.saturating_sub(1).min(63);
    policy
        .retry_backoff
        .saturating_mul(1u64 << shift)
        .min(CAP)
}

//! 传输速率与剩余时间估计（XFER-05）。
//!
//! 进度界面现在只能说「已传 1.2 MB / 3.4 MB」——用户真正想知道的是「还要多久」。
//! 那个数字不能由界面自己猜：它会随每一帧抖动，而一抖就会让人觉得传输卡住了。
//! 所以把「瞬时速率 → 平滑估计 → 剩余时间」这一套放在**纯函数层**，界面只负责显示
//! 最后一次 [`RateMeter::bytes_per_sec`] 的结果。
//!
//! # 为什么时间由宿主喂
//!
//! 本 crate 的纪律是零 IO、零墙钟（见 crate 文档第 3 条）。读时钟是一个副作用，
//! 而「多久算一次采样」是策略——两者都归调用方。这里只接受
//! `sample(bytes, now_ms)`，于是所有规则都能用纯数据钉死。
//!
//! # 三条刻意的取舍
//!
//! 1. **用毫秒而不是秒。** 一个 40 MB 的文件在千兆链路上几秒就完事；按秒粒度算出来
//!    的速率是「0 或 40」，没有意义。
//! 2. **几百毫秒内的重复采样被折叠进下一个窗口**，而不是丢弃。丢掉字节等于让速率
//!    偏低，而偏低会被用户读成「速度慢」；折叠之后字节还在，只是窗口更长。
//! 3. **指数平滑的初值取第一次有效瞬时速率**，不从 0 往上爬。否则刚开头那几秒
//!    界面会显示一个很小的速度，看起来像在卡。
//!
//! # 已知边界
//!
//! 只估计**单向**速率。同时收发的会话（例如 SFTP 面板既在看目录又在传文件）需要
//! 各持一个 `RateMeter`，混在一起算没有意义。

/// 采样窗口的下限（毫秒）。比这更密的重复采样被折叠进下一个窗口。
pub const MIN_WINDOW_MS: u64 = 100;

/// 指数平滑系数：新样本占 30%，历史占 70%。
///
/// 再大一点会跟着每一帧跳，再小一点则「传输已经提速了」要好几秒才反映出来。
const SMOOTHING: f64 = 0.3;

/// 速率估计器。
///
/// ```text
/// meter.sample(progress.bytes, host_now_ms());   // 每帧或每 200ms 调一次
/// if let Some(bps) = meter.bytes_per_sec() {
///     label = format!("{}/s", human_bytes(bps as u64));
/// }
/// ```
#[derive(Debug, Clone, Default)]
pub struct RateMeter {
    /// 上一次采样的字节数与时刻；`None` = 还没建立基线。
    baseline: Option<(u64, u64)>,
    /// 平滑后的速率（字节/秒）。`None` = 还没有过有效窗口。
    smoothed: Option<f64>,
}

impl RateMeter {
    /// 建一个空的估计器：第一次 [`RateMeter::sample`] 只记基线。
    pub fn new() -> Self {
        Self::default()
    }

    /// 清空所有状态（开始新文件、重试、续传重新计时都该调）。
    pub fn reset(&mut self) {
        *self = Self::default();
    }

    /// 喂一次采样。`now_ms` 必须单调递增（调用方的时钟），单位毫秒。
    ///
    /// 三种情况会**不更新速率**、只把基线摆正：
    ///
    /// - 第一次采样（还没有窗口）；
    /// - 距上次不足 [`MIN_WINDOW_MS`]（窗口太短，算出来是噪声，字节留给下一个窗口）；
    /// - 时间没有前进（同一毫秒重复调用）。
    ///
    /// 字节数**回退**（换了一个文件、续传从头开始）视为重新开始：估计清空，这次采样
    /// 成为新基线。
    pub fn sample(&mut self, bytes: u64, now_ms: u64) {
        let Some((last_bytes, last_ms)) = self.baseline else {
            self.baseline = Some((bytes, now_ms));
            return;
        };
        if bytes < last_bytes {
            // 换文件 / 重新开始：旧速率与新传输无关，留着只会误导。
            self.smoothed = None;
            self.baseline = Some((bytes, now_ms));
            return;
        }
        let elapsed = now_ms.saturating_sub(last_ms);
        if elapsed < MIN_WINDOW_MS {
            // 不推进基线：这些字节会在下一个窗口里被算进去。
            return;
        }
        self.baseline = Some((bytes, now_ms));
        let instant = (bytes - last_bytes) as f64 * 1000.0 / elapsed as f64;
        self.smoothed = Some(match self.smoothed {
            None => instant,
            Some(previous) => previous * (1.0 - SMOOTHING) + instant * SMOOTHING,
        });
    }

    /// 当前速率估计（字节/秒）。还没有足够信息时 `None`。
    ///
    /// 注意返回值可能小于 `1.0`（几百字节每秒的串口链路），调用方别先取整再判断。
    pub fn bytes_per_sec(&self) -> Option<f64> {
        self.smoothed.filter(|bps| *bps > 0.0)
    }

    /// 速率的整数字节表示，直接丢给界面的「人类可读字节」格式化。
    pub fn bytes_per_sec_rounded(&self) -> Option<u64> {
        self.bytes_per_sec().map(|bps| bps.round() as u64)
    }

    /// 剩余时间估计（秒，向上取整）。
    ///
    /// - 已经传完（`done >= total`）→ `Some(0)`；
    /// - 还没有速率、或速率恰好为 0 → `None`（界面应显示「估算中」而不是「剩余 ∞」）。
    ///
    /// 取整向上：报「还剩 1 秒」而实际 0.2 秒，比报「还剩 0 秒」而实际还在跑好。
    pub fn eta_secs(&self, total: u64, done: u64) -> Option<u64> {
        let remaining = total.saturating_sub(done);
        if remaining == 0 {
            return Some(0);
        }
        let bps = self.bytes_per_sec()?;
        // 极慢但非零的链路（比如 8 字节/秒）算出来可能是天文数字；钳到一天，
        // 免得界面显示「剩余 812 天」。
        let secs = (remaining as f64 / bps).ceil();
        Some(secs.min(86_400.0) as u64)
    }
}

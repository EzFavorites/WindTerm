//! Sysmon 采样的**模型 + 纯解析层**（roadmap C3 语义层）。
//!
//! # 为什么要在独立 exec 通道里采样
//!
//! 状态栏里的 CPU / 内存 / 进程本该由程序自己去问系统。若退而求其次在
//! **用户的 shell 上**反复敲 `top` / `free`，代价是三条硬伤：
//!
//! 1. 采样命令和用户正在输入的那一行抢同一个 stdin，回车就会把半条命令
//!    送出去；
//! 2. 每次采样的原始输出都进 VT 和 scrollback，翻历史时全是采样垃圾；
//! 3. 采样频率一高，用户看不见自己的输出。
//!
//! 正确做法是经控制面的 [`open_exec`] 另开一条 one-shot 通道，输出只汇总
//! 进模型。**本模块只负责后半段**：拿到那坨文本之后怎么变成数据，以及
//! 「该跑哪条命令」。真正开通道、收字节、喂进这里的是上层
//! （`term-io` 的 supervisor / `desk`），本模块**零 IO、零线程、零墙钟**：所有函数
//! 都是纯函数，同一个字符串永远解析出同一份 [`Sample`]。
//!
//! [`open_exec`]: crate::SessionPool::open_exec
//!
//! # 采样命令的无副作用约定
//!
//! [`SAMPLE_CPU`] / [`SAMPLE_MEM`] / [`SAMPLE_PS`] 三条命令都必须在**不
//! 改动系统状态**的前提下跑完：不落文件、不改终端属性、不读用户配置、
//! 不因为缺 root 而报错退出（缺权限时输出残缺，由解析层降级）。因此：
//!
//! - 只用 `-b` / `-l1` 这类批处理开关，不跑交互式 `top`；
//! - 不拼 `sh -c '... > /tmp/x'`，也不重定向——输出直接进 exec 通道；
//! - `ps` 用 `-eo` 显式列名，不依赖 `$PS_FORMAT` 之类的环境差异。
//!
//! # 平台差异
//!
//! | 用途 | Linux | macOS |
//! |---|---|---|
//! | CPU / 负载 | `top -bn1` | `top -l1 -n0` |
//! | 内存 | `free -k` | `vm_stat` |
//! | 进程 | `ps -eo pid,pcpu,pmem,comm` | 同左（BSD `ps` 列名一致） |
//!
//! 解析层对两套格式各有一个入口，互不猜测：Linux 的 `top` 首行是
//! `load average:`，macOS 是 `Load Avg:`；内存一个是 `MiB Mem :` 行，
//! 一个是 `PhysMem:` 行。缺哪一项就留 0，**不返回 `Err`、不 panic**。
//!
//! # 数值口径
//!
//! [`Sample::cpu_pct`] 是**整机平均**占用（0..=100），不是把每个核的百分比
//! 加起来：`top` 的 `%Cpu(s)` 一行本身就是按核平均后的值（`100 - idle`），
//! 16 核满载也只报 100，不会报 1600。界面画趋势图时可以直接当百分比用。

pub use crate::sysmon_parse::{
    parse_free, parse_ps, parse_top, parse_top_darwin, parse_vm_stat,
};

/// Linux：一次 `top` 批处理采样，含负载 / CPU / 内存摘要。
///
/// `-b` 是批处理（无终端控制序列），`-n1` 只要一帧。输出里没有 `ESC`，
/// 所以即使被误喂给 VT 也不会染色——这是无副作用的另一半。
pub const SAMPLE_CPU: &str = "top -bn1";

/// macOS：`top -l1` 的等价物。`-n0` 不列进程（进程走 [`SAMPLE_PS`]），
/// 因为 macOS `top` 的进程列宽随终端宽度变化，逐行对齐不可靠。
pub const SAMPLE_CPU_DARWIN: &str = "top -l1 -n0";

/// Linux：以 KiB 为单位的内存表。`-k` 钉住单位，免得被 `free` 的
/// 自动缩放（`-h`）或本地化配置改掉列宽。
pub const SAMPLE_MEM: &str = "free -k";

/// macOS：虚拟内存分页统计。页大小在首行，换算成 KiB 由解析层做。
pub const SAMPLE_MEM_DARWIN: &str = "vm_stat";

/// 进程列表：只取四列，按 CPU 降序，由远端 `ps` 排好再送回来。
///
/// 显式列名是关键：`ps aux` 的列随 BSD / procps 变体不同（`%CPU` 与
/// `pcpu`），`-eo` 的名字两边一致，解析层只有一套。
pub const SAMPLE_PS: &str = "ps -eo pid,pcpu,pmem,comm --sort=-pcpu";

/// 进程表里的一行。`cpu` / `mem` 是百分比（0..=100），不是绝对值。
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Proc {
    pub pid: u32,
    pub cpu: f32,
    pub mem: f32,
    pub name: String,
}

/// 一次采样的完整结果。缺项一律留 0（不是 `Option`）：界面画图时 0 就是
/// 「没数据」，而采样本身成功与否由 exec 通道的退出状态表示，不该混进
/// 数据里。
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Sample {
    /// 整机平均 CPU 占用（0..=100，多核是平均不是合计）。
    pub cpu_pct: f32,
    pub mem_used_kb: u64,
    pub mem_total_kb: u64,
    /// 1 / 5 / 15 分钟平均负载。
    pub load: [f32; 3],
    /// 按 CPU 降序，最多 20 条。
    pub procs: Vec<Proc>,
}

/// 采样历史环形缓冲：只留最近 `capacity` 个样本，供状态栏画迷你趋势图。
///
/// 容量很小（默认 60），所以用 `Vec` + 丢头而不是 `VecDeque`：
/// [`samples`](History::samples) 要返回连续切片，`VecDeque` 在环绕后还得
/// `make_contiguous`，而这里每次 `push` 的移位最多几十个元素。
pub struct History {
    buf: Vec<Sample>,
    capacity: usize,
}

impl History {
    /// `capacity` 为 0 时按 1 处理：一个连最近一个样本都留不住的历史没有
    /// 意义，静默变成「永不记录」会让调用方以为采到了。
    pub fn new(capacity: usize) -> Self {
        let capacity = capacity.max(1);
        Self {
            buf: Vec::with_capacity(capacity),
            capacity,
        }
    }

    /// 推入一个样本；满了就丢最旧的那个。
    pub fn push(&mut self, s: Sample) {
        if self.buf.len() == self.capacity {
            self.buf.remove(0);
        }
        self.buf.push(s);
    }

    /// 时间序（旧 → 新）。
    pub fn samples(&self) -> &[Sample] {
        &self.buf
    }

    pub fn len(&self) -> usize {
        self.buf.len()
    }

    pub fn is_empty(&self) -> bool {
        self.buf.is_empty()
    }

    pub fn clear(&mut self) {
        self.buf.clear();
    }

    /// 最近 `k` 个样本的 CPU 均值；不足 `k` 个就全取。
    ///
    /// 空集合返回 `None`——0.0 会被界面画成「0% 占用」，而实际是「还没采到」，
    /// 这两种状态必须分得开。
    pub fn avg_cpu(&self, k: usize) -> Option<f32> {
        if k == 0 || self.buf.is_empty() {
            return None;
        }
        let take = k.min(self.buf.len());
        let sum: f32 = self.buf.iter().rev().take(take).map(|s| s.cpu_pct).sum();
        Some(sum / take as f32)
    }

    /// 历史里最高的内存占用百分比。总量为 0 的样本（解析残缺）跳过，
    /// 一个有效样本都没有时返回 `None`（不除零、不硬凑 0）。
    pub fn max_mem_pct(&self) -> Option<f32> {
        self.buf
            .iter()
            .filter(|s| s.mem_total_kb > 0)
            .map(|s| (s.mem_used_kb as f64 * 100.0 / s.mem_total_kb as f64) as f32)
            .fold(None, |acc: Option<f32>, v| {
                Some(acc.map_or(v, |a: f32| a.max(v.min(100.0))))
            })
    }
}

impl Default for History {
    fn default() -> Self {
        Self::new(60)
    }
}

/// `"37.5%"`。非有限值（解析层不该产出，但别让 NaN 漏到界面上）给 `"—"`。
pub fn fmt_pct(v: f32) -> String {
    if !v.is_finite() {
        return "—".to_string();
    }
    format!("{v:.1}%")
}

/// `"1.2 GiB (37%)"`。`total_kb == 0` 时返回 `"—"`：这是「还没采到内存」，
/// 不是「占用 0%」，所以既不做除法也不给 0%。
pub fn fmt_mem(used_kb: u64, total_kb: u64) -> String {
    if total_kb == 0 {
        return "—".to_string();
    }
    let pct = (used_kb as f64 * 100.0 / total_kb as f64).round().min(100.0) as u32;
    format!("{} ({pct}%)", fmt_kb(used_kb))
}

/// KiB 数量转给人看的单位：≥1 GiB 用 GiB，≥1 MiB 用 MiB，否则 KiB。
/// 保留一位小数，和 [`fmt_pct`] 的精度一致。
fn fmt_kb(kb: u64) -> String {
    const MIB: u64 = 1024;
    const GIB: u64 = 1024 * 1024;
    if kb >= GIB {
        format!("{:.1} GiB", kb as f64 / GIB as f64)
    } else if kb >= MIB {
        format!("{:.1} MiB", kb as f64 / MIB as f64)
    } else {
        format!("{kb} KiB")
    }
}

/// 进程名缩短：超过 `n` 个字符时**保留尾部**。
///
/// 保留尾部是因为路径的尾部才是程序名（`/usr/lib/firefox/firefox` 的
/// `firefox`），截头会只剩一串没意义的目录名。按 **char** 计数而不是字节：
/// 中文 / emoji 一个字符占多个字节，按字节切会切出半个码点。
pub fn short_name(name: &str, n: usize) -> String {
    let count = name.chars().count();
    if count <= n {
        return name.to_string();
    }
    name.chars().skip(count - n).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(cpu: f32, used: u64, total: u64) -> Sample {
        Sample {
            cpu_pct: cpu,
            mem_used_kb: used,
            mem_total_kb: total,
            ..Sample::default()
        }
    }

    #[test]
    fn history_keeps_only_the_newest_samples_in_time_order() {
        let mut h = History::new(3);
        for i in 1..=5 {
            h.push(s(i as f32, 0, 0));
        }
        assert_eq!(h.len(), 3);
        let seen: Vec<f32> = h.samples().iter().map(|x| x.cpu_pct).collect();
        assert_eq!(seen, vec![3.0, 4.0, 5.0], "旧样本先被挤掉，顺序是旧→新");
        h.clear();
        assert!(h.is_empty());
    }

    #[test]
    fn a_zero_capacity_history_still_records_the_last_sample() {
        let mut h = History::new(0);
        h.push(s(1.0, 0, 0));
        assert_eq!(h.len(), 1);
    }

    #[test]
    fn avg_cpu_is_none_when_there_is_no_data_and_never_divides_by_zero() {
        let mut h = History::default();
        assert_eq!(h.avg_cpu(10), None, "空集合必须是 None，不能是 0.0");
        h.push(s(10.0, 0, 0));
        h.push(s(30.0, 0, 0));
        assert_eq!(h.avg_cpu(1), Some(30.0), "不足 k 个时取最近的若干个");
        assert_eq!(h.avg_cpu(5), Some(20.0));
        assert_eq!(h.avg_cpu(0), None, "取 0 个样本没有均值");
    }

    #[test]
    fn max_mem_pct_skips_samples_without_a_total() {
        let mut h = History::default();
        assert_eq!(h.max_mem_pct(), None);
        h.push(s(0.0, 0, 0));
        assert_eq!(h.max_mem_pct(), None, "总量 0 的样本不参与计算");
        h.push(s(0.0, 37, 100));
        h.push(s(0.0, 12, 100));
        assert_eq!(h.max_mem_pct(), Some(37.0));
    }

    #[test]
    fn formatting_is_stable_and_never_divides_by_a_zero_total() {
        assert_eq!(fmt_pct(37.5), "37.5%");
        assert_eq!(fmt_pct(f32::NAN), "—");
        assert_eq!(fmt_mem(0, 0), "—", "没采到内存不是 0%");
        assert_eq!(fmt_mem(1_258_291, 3_400_000), "1.2 GiB (37%)");
        assert_eq!(fmt_mem(512, 4_096), "512 KiB (13%)");
        assert_eq!(fmt_mem(5_000_000, 4_000_000), "4.8 GiB (100%)", "不报超过 100%");
    }

    #[test]
    fn short_name_keeps_the_tail_and_counts_chars_not_bytes() {
        assert_eq!(short_name("/usr/lib/firefox/firefox", 7), "firefox");
        assert_eq!(short_name("bash", 7), "bash", "够短就原样返回");
        assert_eq!(short_name("abcdef", 0), "");
        // 按 char 截断：中文与 emoji 都不能切出半个码点。
        assert_eq!(short_name("终端监控程序", 3), "控程序");
        assert_eq!(short_name("a🦀b🦀c", 3), "b🦀c");
    }
}

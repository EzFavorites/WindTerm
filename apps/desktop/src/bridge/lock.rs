//! 锁屏探测：人不在机器前时，`desk serve` 不该继续接受新连接。
//!
//! 这是**纵深防御**，不是主闸门——主闸门是 token（[`super::token`]）。锁屏探测
//! 只解决一个具体场景：用户锁屏离开，同机上的另一个进程（或另一个本地用户
//! 拿到的会话）不该还能连上来驱动终端。
//!
//! 探测做成 trait 而不是一个函数，是因为「锁屏」在本机**没法自动化验证**：
//! 真正锁一次屏要人工介入。于是把「怎么问系统」和「问了之后怎么办」拆开，
//! 后者（拒绝服务）可以用一个假的探针在单测与 e2e 里钉死。
//!
//! # 判据的三态，以及为什么「不知道」要放行
//!
//! `locked()` 返回 `Option<bool>`：
//!
//! - `Some(true)`：锁屏中 → 关连接，不服务。
//! - `Some(false)`：没锁 → 正常服务。
//! - `None`：问不出来（非 macOS、`ioreg` 挂了、输出不认识）→ **放行**。
//!
//! 放行而不是拒绝：`desk serve` 的主要用途是给本机的脚本 / Agent 用，探测
//! 失败就整体不可用属于本末倒置；而 token 仍然要求，安全性没有归零。
//! **这是有意的产品决策，P0-4 不翻转它**：把 `None` 改成拒绝服务会让一个
//! 探测不到的平台整体不可用，不符合 1.0 的定位。
//!
//! # 1.0 平台声明（P0-4）
//!
//! 锁屏探测**仅 macOS 有实现**（`ioreg`）。其他平台 [`Ioreg::locked`] 恒返回
//! `None`，即放行；这些平台上 `desk serve` 的纵深防御**只有 token 主闸门**
//! （回环监听 + 一次性握手）。因此 1.0 **不得宣称跨平台锁屏防护**：若要宣称，
//! 必须为对应平台补实现，并另行决策「探测失败是否仍放行」。
//!
//! 为了让「本平台没做锁屏探测」在运行时可见、可测，fail-open 不再是无声的：
//! 生产路径 `Cached<Ioreg>`（见 `bridge::serve`）每次把三态过一遍 [`permits`]，
//! `None` 会累加 `Cached::unknown_hits`（含缓存命中，即「因探测失败而放行的
//! 判定次数」），并写一条带 [`UNKNOWN_MARK`] 的一次性 stderr 审计行。审计行
//! 里的次数是写这一行时的快照，后续按 `unknown_hits()` 查实时累计。
//! 注意审计点在这个包装层：直接持有裸 [`Ioreg`] 不会有日志，生产路径恒用
//! `Cached<Ioreg>`。审计行与计数都能在单测里钉死，见 `lock_tests.rs`。

use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// `desk serve` stderr 上 fail-open 审计行的固定前缀，运维按它 grep。
pub const UNKNOWN_MARK: &str = "[desk-serve][lock-probe] UNKNOWN";

/// 1.0 的平台事实：锁屏探测只在 macOS 有实现。
pub fn platform_has_probe() -> bool {
    cfg!(target_os = "macos")
}

/// 把三态翻译成「是否放行」的判据，与 `bridge/mod.rs` 里 `handle` /
/// `LockedGate` 现行的 `== Some(true)` 等价（那两处仍直接比较，本函数供
/// `Cached::observe` 与单测共用同一口径）。
///
/// `None` **放行**（见文件头产品决策）；留痕不在这里做，而在 [`Cached`] 的
/// `observe` 里——它才是生产路径上每次判定都会经过的地方。
pub fn permits(value: Option<bool>) -> bool {
    !matches!(value, Some(true))
}

/// 审计行的纯构造函数：日志内容可单测，不必捕获 stderr。
fn audit_line(supported: bool, hits: u64) -> String {
    let reason = if supported {
        "macOS/ioreg 探测失败"
    } else {
        "本平台未实现锁屏探测（仅 macOS 有实现）"
    };
    format!(
        "{UNKNOWN_MARK} {reason}；按 1.0 产品决策放行，主闸门仍是 token；fail-open 判定次数={hits}"
    )
}

/// 探针结论的纯函数形态：让「非 macOS 恒 None」不必真的在 Linux 上跑才能证伪。
///
/// `supported=false` 时即便喂一份「锁屏中」的 `ioreg` 文本也返回 `None`——
/// 没实现的平台不许假装知道，更不许假装锁着。`output=None` 代表命令没跑起来。
fn decide(supported: bool, output: Option<&str>) -> Option<bool> {
    if !supported {
        return None;
    }
    judge(output.unwrap_or(""))
}

/// 谁来判断「现在锁屏了吗」。
pub trait LockProbe: Send + Sync {
    fn locked(&self) -> Option<bool>;
}

/// 真实探针：macOS 的 `ioreg`。其他平台一律 `None`（放行）。
pub struct Ioreg;

impl LockProbe for Ioreg {
    fn locked(&self) -> Option<bool> {
        let supported = platform_has_probe();
        // 非 macOS 不执行 `ioreg`：探测没实现就是没实现，别拿一份假输出去猜。
        let output: Option<String> = if supported {
            Command::new("ioreg")
                .args(["-n", "Root", "-d1"])
                .stdin(Stdio::null())
                .stderr(Stdio::null())
                .output()
                .ok()
                .map(|out| String::from_utf8_lossy(&out.stdout).into_owned())
        } else {
            None
        };
        decide(supported, output.as_deref())
    }
}

/// 带缓存的探针：同一次「问系统」结果的复用窗口。
///
/// 为什么需要它：`LockedGate` 是**每次能力面调用**都问一次锁屏，而
/// [`Ioreg`] 的每次询问都是一次 `fork + exec`（十几毫秒）。不缓存的话，
/// 一段连续 `send` 的脚本会被 `ioreg` 拖成逐条十几毫秒——安全检查变成性能税，
/// 而性能税最终会被人用「关掉它」来偿还。
///
/// 缓存窗口取 2 秒：锁屏是个**人**的动作，没人能在 2 秒内锁屏并连上来。
pub struct Cached<P: LockProbe> {
    inner: P,
    ttl: Duration,
    last: Mutex<Option<(Instant, Option<bool>)>>,
    /// 本实例因 `None` 而放行的判定次数（含缓存命中）。运行期可查、单测可断言。
    unknown_hits: AtomicU64,
    /// 审计行只写一次，避免每次能力面调用刷一行；计数仍每次累加。
    unknown_logged: AtomicBool,
}

impl<P: LockProbe> Cached<P> {
    pub fn new(inner: P, ttl: Duration) -> Self {
        Self {
            inner,
            ttl,
            last: Mutex::new(None),
            unknown_hits: AtomicU64::new(0),
            unknown_logged: AtomicBool::new(false),
        }
    }

    /// 本实例累计的 fail-open 判定次数（生产上一个 `serve` 进程只有一个
    /// `Cached<Ioreg>`，因此就是进程口径）。运维诊断 / 单测按它核对「本平台
    /// 到底有没有做锁屏探测」——为 0 表示探测每次都有明确结论。
    pub fn unknown_hits(&self) -> u64 {
        self.unknown_hits.load(Ordering::Relaxed)
    }

    /// 三态判定 + fail-open 审计的唯一收口。返回是否放行。
    ///
    /// `bridge/mod.rs` 的 `LockedGate` 自己用 `== Some(true)` 判拒，所以这里
    /// **不要**再让调用方重复调 `permits`；否则计数会翻倍。
    fn observe(&self, value: Option<bool>) -> bool {
        let allowed = permits(value);
        if value.is_none() {
            let hits = self.unknown_hits.fetch_add(1, Ordering::Relaxed) + 1;
            if !self.unknown_logged.swap(true, Ordering::Relaxed) {
                eprintln!("{}", audit_line(platform_has_probe(), hits));
            }
        }
        allowed
    }
}

impl<P: LockProbe> LockProbe for Cached<P> {
    fn locked(&self) -> Option<bool> {
        // 锁中毒就当作没缓存（`into_inner`）：这里没有「必须保持的不变式」，
        // 一次 panic 让锁屏门永久失效才是真正的问题。
        let mut slot = match self.last.lock() {
            Ok(slot) => slot,
            Err(poisoned) => poisoned.into_inner(),
        };
        if let Some((at, value)) = *slot {
            if at.elapsed() < self.ttl {
                // 缓存命中同样是一次「放行判定」：也过 observe，否则 2 秒窗口里
                // 放了 N 次却只留 1 条痕，运行时的数字与实际放行次数不符。
                self.observe(value);
                return value;
            }
        }
        let value = self.inner.locked();
        self.observe(value);
        *slot = Some((Instant::now(), value));
        value
    }
}

/// 从 `ioreg` 的文本输出里读锁屏状态。
///
/// 未锁屏时该键**不存在**（本机实测：`ioreg -n Root -d1` 里没有
/// `CGSSessionScreenIsLocked` 这一行），所以「有输出但没有这个键」判为
/// `Some(false)`；「完全没有输出」判为 `None`（命令没跑起来，不是「没锁」）。
pub fn judge(text: &str) -> Option<bool> {
    const KEY: &str = "CGSSessionScreenIsLocked";
    if text.trim().is_empty() {
        return None;
    }
    for line in text.lines() {
        // 从**键的位置**往后找 `=`，而不是取整行的第一个 `=`：
        // `ioreg` 既可能把每个键单独一行，也可能把整段 `IOConsoleUsers`
        // 压在一行里（`"IOConsoleUsers" = ({"CGSSessionScreenIsLocked" = Yes,...`）。
        // 取整行第一个 `=` 会把 `IOConsoleUsers` 的值当成锁屏状态。
        let Some(at) = line.find(KEY) else { continue };
        let rest = &line[at + KEY.len()..];
        // 键找到了却没有 `=`：输出形态不认识，判「不知道」而不是猜。
        let (_, value) = rest.split_once('=')?;
        let token: String = value
            .trim()
            .trim_start_matches(['"', '<'])
            .chars()
            .take_while(|c| c.is_ascii_alphanumeric())
            .collect::<String>()
            .to_ascii_lowercase();
        return match token.as_str() {
            "yes" | "true" | "1" => Some(true),
            "no" | "false" | "0" => Some(false),
            _ => None,
        };
    }
    Some(false)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    #[test]
    fn a_locked_session_is_recognized() {
        let sample = r#"+-o Root  <class IORegistryEntry>
  {
    "IOConsoleUsers" = ({"CGSSessionScreenIsLocked" = Yes,"kCGSSessionUserNameKey" = "unique"})
  }"#;
        assert_eq!(judge(sample), Some(true));
    }

    #[test]
    fn an_unlocked_session_is_recognized() {
        let sample = r#"+-o Root
  {
    "IOConsoleUsers" = ({"CGSSessionScreenIsLocked" = No})
  }"#;
        assert_eq!(judge(sample), Some(false));
    }

    #[test]
    fn a_missing_key_means_unlocked_not_unknown() {
        // 本机未锁屏时的真实形态：整段输出里没有这个键。
        let sample = r#"+-o Root  <class IORegistryEntry>
  {
    "IOKitBuildVersion" = "..."
  }"#;
        assert_eq!(judge(sample), Some(false));
    }

    #[test]
    fn no_output_at_all_is_unknown_so_the_caller_can_decide() {
        assert_eq!(judge(""), None);
        assert_eq!(judge("   \n  "), None);
    }

    #[test]
    fn an_unrecognized_value_is_unknown_rather_than_optimistic() {
        assert_eq!(judge(r#""CGSSessionScreenIsLocked" = Whatever"#), None);
    }

    #[test]
    fn the_probe_is_injectable_so_refusal_can_be_tested() {
        struct AlwaysLocked;
        impl LockProbe for AlwaysLocked {
            fn locked(&self) -> Option<bool> {
                Some(true)
            }
        }
        let probe: Arc<dyn LockProbe> = Arc::new(AlwaysLocked);
        assert_eq!(probe.locked(), Some(true));
    }

    #[test]
    fn the_cache_keeps_a_burst_of_calls_from_forking_ioreg_each_time() {
        struct Counting(std::sync::atomic::AtomicUsize);
        impl LockProbe for Counting {
            fn locked(&self) -> Option<bool> {
                self.0.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                Some(false)
            }
        }
        let probe = Cached::new(
            Counting(std::sync::atomic::AtomicUsize::new(0)),
            std::time::Duration::from_secs(60),
        );
        for _ in 0..50 {
            assert_eq!(probe.locked(), Some(false));
        }
        assert_eq!(
            probe.inner.0.load(std::sync::atomic::Ordering::Relaxed),
            1,
            "60s 的窗口里 50 次调用只该问一次系统"
        );
    }

    #[test]
    fn a_zero_window_means_always_ask() {
        struct Counting(std::sync::atomic::AtomicUsize);
        impl LockProbe for Counting {
            fn locked(&self) -> Option<bool> {
                self.0.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                Some(true)
            }
        }
        let probe = Cached::new(
            Counting(std::sync::atomic::AtomicUsize::new(0)),
            std::time::Duration::from_secs(0),
        );
        probe.locked();
        probe.locked();
        assert_eq!(
            probe.inner.0.load(std::sync::atomic::Ordering::Relaxed),
            2,
            "窗口为 0 时必须每次都问（这条同时说明缓存不是永久记忆）"
        );
    }
}

// P0-4 的三态判据 / fail-open 审计回归单独放一个文件：夹具与断言较多，
// 且 `#[path]` 引入的纯测试模块不会污染上层门禁对生产代码的扫描。
#[cfg(test)]
#[path = "lock_tests.rs"]
mod lock_tests;

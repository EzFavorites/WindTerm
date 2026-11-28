//! 锁屏探测：人不在机器前时，前台控制端点不该继续接受新连接。
//!
//! 这是**纵深防御**，不是准入本身——准入靠 loopback 对端校验 + `0600` key
//! 文件 + HMAC-SHA256 挑战证明（ADR-0030）。锁屏探测只解决一个具体场景：
//! 用户锁屏离开，同机上的另一个进程不该还能连上来驱动终端。
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
//! - `None`：问不出来（没有桌面环境、OS 调用失败、输出不认识）→ **放行**。
//!
//! 放行而不是拒绝：控制端点的主要用途是给本机的 CLI / MCP 客户端用，探测
//! 失败就整体不可用属于本末倒置；loopback + key 文件 + HMAC 证明的准入仍然
//! 要求，安全性没有归零。
//! **这是有意的产品决策，不翻转它**：把 `None` 改成拒绝服务会让一个
//! 探测不到的机器整体不可用，不符合 1.0 的定位。
//!
//! # 平台声明
//!
//! macOS（`ioreg`）、Windows（`OpenInputDesktop` + `GetUserObjectInformation`）
//! 与 Linux（freedesktop `ScreenSaver.GetActive`，经 gdbus / dbus-send）都有实现，
//! [`Ioreg::locked`] 按当前平台分派（名字沿用生产接线的历史命名）。其余平台恒返回
//! `None`，即放行；那些平台没有锁屏探测这一层，准入只剩 loopback + key + HMAC 证明。
//! Linux 没有桌面环境 / 桌面不暴露该接口时同样返回 `None`——这是「问不出来」，
//! 不是「没锁」，绝不伪装成 `Some(false)`。
//!
//! 为了让「本平台没做锁屏探测」在运行时可见、可测，fail-open 不再是无声的：
//! 生产路径 `Cached<Ioreg>`（见 `bridge::serve`）每次把三态过一遍 [`permits`]，
//! `None` 会累加 `Cached::unknown_hits`（含缓存命中，即「因探测失败而放行的
//! 判定次数」），并写一条带 [`UNKNOWN_MARK`] 的一次性 stderr 审计行。审计行
//! 里的次数是写这一行时的快照，后续按 `unknown_hits()` 查实时累计。
//! 注意审计点在这个包装层：直接持有裸 [`Ioreg`] 不会有日志，生产路径恒用
//! `Cached<Ioreg>`。审计行与计数都能在单测里钉死，见 `lock_tests.rs`。

#[cfg(target_os = "macos")]
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// 前台控制端点 stderr 上 fail-open 审计行的固定前缀，运维按它 grep。
pub const UNKNOWN_MARK: &str = "[desk][lock-probe] UNKNOWN";

/// 当前平台是否有锁屏探测实现（macOS / Windows / Linux 为真）。
///
/// 这是运行时可见的平台事实：审计行用它区分「探测失败」与「平台没做探测」，
/// 单测用它断言三个平台都真正接了实现。
pub fn platform_has_probe() -> bool {
    cfg!(any(target_os = "macos", target_os = "windows", target_os = "linux"))
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
        "OS 调用没给出明确结论（无桌面环境 / 命令失败 / 输出不认识）"
    } else {
        "本平台未实现锁屏探测（仅 macOS / Windows / Linux 有实现）"
    };
    format!(
        "{UNKNOWN_MARK} {reason}；按产品决策放行，主闸门是 loopback 对端 + 0600 key 文件 + HMAC-SHA256 挑战证明，锁屏探测只是纵深防御；fail-open 判定次数={hits}"
    )
}

/// macOS 探针结论的纯函数形态：让「没输出就是不知道」不必真跑 `ioreg` 才能证伪。
///
/// `supported=false` 时即便喂一份「锁屏中」的 `ioreg` 文本也返回 `None`——
/// 没实现的平台不许假装知道，更不许假装锁着。`output=None` 代表命令没跑起来。
#[cfg(any(test, target_os = "macos"))]
fn decide(supported: bool, output: Option<&str>) -> Option<bool> {
    if !supported {
        return None;
    }
    judge(output.unwrap_or(""))
}

/// 谁来判断「现在锁屏了吗」。三态见文件头。
pub trait LockProbe: Send + Sync {
    fn locked(&self) -> Option<bool>;
}

/// 系统探针。名字沿用生产接线（`bridge::serve` 里的 `lock::Ioreg`），
/// 实际按平台分派：macOS 问 `ioreg`，Windows 问输入桌面，Linux 问
/// freedesktop ScreenSaver；未实现的平台返回 `None`（放行 + 审计）。
pub struct Ioreg;

impl LockProbe for Ioreg {
    #[cfg(target_os = "macos")]
    fn locked(&self) -> Option<bool> {
        let output = Command::new("ioreg")
            .args(["-n", "Root", "-d1"])
            .stdin(Stdio::null())
            .stderr(Stdio::null())
            .output()
            .ok()
            .map(|out| String::from_utf8_lossy(&out.stdout).into_owned());
        decide(true, output.as_deref())
    }

    #[cfg(target_os = "windows")]
    fn locked(&self) -> Option<bool> {
        win32::input_desktop()
    }

    #[cfg(target_os = "linux")]
    fn locked(&self) -> Option<bool> {
        freedesktop::screensaver_active()
    }

    #[cfg(not(any(target_os = "macos", target_os = "windows", target_os = "linux")))]
    fn locked(&self) -> Option<bool> {
        None
    }
}

/// Windows 纯判据：输入桌面名 → 三态。与 OS 调用隔离，单测可在任意平台跑。
#[cfg(any(test, target_os = "windows"))]
fn judge_windows_desktop(name: &str) -> Option<bool> {
    match name.trim().to_ascii_lowercase().as_str() {
        // 锁屏时输入桌面切到安全的 Winlogon 桌面；屏保桌面同理。
        "winlogon" | "screen-saver" => Some(true),
        // 普通交互桌面：明确没锁。
        "default" => Some(false),
        // 其它名字不认识：判「问不出来」，不猜。
        _ => None,
    }
}

/// Windows 纯判据：`OpenInputDesktop` 失败时的 `GetLastError` → 三态。
///
/// 锁屏时普通进程无权打开 Winlogon 输入桌面，会得到 ERROR_ACCESS_DENIED(5)；
/// 其它错误（权限策略、会话已结束等）不代表锁屏，判「问不出来」。
#[cfg(any(test, target_os = "windows"))]
fn judge_windows_error(code: u32) -> Option<bool> {
    const ERROR_ACCESS_DENIED: u32 = 5;
    (code == ERROR_ACCESS_DENIED).then_some(true)
}

/// Linux 纯判据：ScreenSaver.GetActive 的返回文本 → 三态。
///
/// `gdbus` 打 `(<true>)`、`dbus-send` 打 `(true)`，也可能带 method-return 头；
/// 只要文本里出现且只出现一个布尔字面量就采用，出现零个或两个（自相矛盾）
/// 都判「问不出来」。
#[cfg(any(test, target_os = "linux"))]
fn judge_screensaver(text: &str) -> Option<bool> {
    let lower = text.to_ascii_lowercase();
    let mut seen: Option<bool> = None;
    for token in lower.split(|c: char| !c.is_ascii_alphanumeric()) {
        let value = match token {
            "true" => true,
            "false" => false,
            _ => continue,
        };
        match seen {
            None => seen = Some(value),
            Some(prev) if prev == value => {}
            Some(_) => return None,
        }
    }
    seen
}

/// Windows 平台 API：问当前输入桌面的名字。整段 `#[cfg]` 隔离，
/// 本机（macOS）不编译、不链接 User32，只有 Windows 构建才进编译单元。
#[cfg(target_os = "windows")]
mod win32 {
    use super::{judge_windows_desktop, judge_windows_error};
    use std::ffi::c_void;

    type Handle = *mut c_void;
    const FALSE: i32 = 0;
    const DESKTOP_READOBJECTS: u32 = 0x0001;
    const UOI_NAME: i32 = 2;

    #[link(name = "user32")]
    extern "system" {
        fn OpenInputDesktop(flags: u32, inherit: i32, access: u32) -> Handle;
        fn GetUserObjectInformationW(
            handle: Handle,
            index: i32,
            info: *mut c_void,
            length: u32,
            needed: *mut u32,
        ) -> i32;
        fn CloseDesktop(handle: Handle) -> i32;
    }
    extern "system" {
        fn GetLastError() -> u32;
    }

    pub(super) fn input_desktop() -> Option<bool> {
        unsafe {
            let desktop = OpenInputDesktop(0, FALSE, DESKTOP_READOBJECTS);
            if desktop.is_null() {
                return judge_windows_error(GetLastError());
            }
            // 256 个 UTF-16 单元足够容纳 "Winlogon" / "Screen-saver" / "Default"；
            // 名字再长也只需前缀判三态，不认识的名字本来就返回 None。
            let mut buf = [0u16; 256];
            let mut needed = 0u32;
            let ok = GetUserObjectInformationW(
                desktop,
                UOI_NAME,
                buf.as_mut_ptr() as *mut c_void,
                (buf.len() * 2) as u32,
                &mut needed,
            );
            let _ = CloseDesktop(desktop);
            if ok == 0 {
                return None;
            }
            let end = buf.iter().position(|&unit| unit == 0).unwrap_or(buf.len());
            judge_windows_desktop(&String::from_utf16_lossy(&buf[..end]))
        }
    }
}

/// Linux 平台 API：freedesktop ScreenSaver.GetActive。整段 `#[cfg]` 隔离，
/// 本机（macOS）不编译；命令不存在 / 没有会话总线 / 输出不认识一律返回 None
/// （「问不出来」），不把「没桌面」伪装成「没锁」。
#[cfg(target_os = "linux")]
mod freedesktop {
    use super::judge_screensaver;
    use std::process::{Command, Stdio};

    pub(super) fn screensaver_active() -> Option<bool> {
        // gdbus 的输出更规整，优先；dbus-send 是很多最小化发行版唯一自带的工具。
        let attempts: [(&str, &[&str]); 2] = [
            (
                "gdbus",
                &[
                    "call",
                    "--session",
                    "--dest",
                    "org.freedesktop.ScreenSaver",
                    "--object-path",
                    "/org/freedesktop/ScreenSaver",
                    "--method",
                    "org.freedesktop.ScreenSaver.GetActive",
                ],
            ),
            (
                "dbus-send",
                &[
                    "--session",
                    "--print-reply",
                    "--dest=org.freedesktop.ScreenSaver",
                    "/org/freedesktop/ScreenSaver",
                    "org.freedesktop.ScreenSaver.GetActive",
                ],
            ),
        ];
        for (program, args) in attempts {
            let Ok(out) = Command::new(program)
                .args(args)
                .stdin(Stdio::null())
                .stderr(Stdio::null())
                .output()
            else {
                continue;
            };
            if out.status.success() {
                if let Some(value) = judge_screensaver(&String::from_utf8_lossy(&out.stdout)) {
                    return Some(value);
                }
            }
        }
        None
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

// 三态判据 / fail-open 审计回归单独放一个文件：夹具与断言较多，
// 且 `#[path]` 引入的纯测试模块不会污染上层门禁对生产代码的扫描。
#[cfg(test)]
#[path = "lock_tests.rs"]
mod lock_tests;

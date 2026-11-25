//! 远端 Sysmon：经**第二条非交互 exec 通道**去活动会话那台机器上取文本，
//! 再用 term-session 已有的纯解析器变成 [`Reading`](crate::sysmon_sample::Reading)。
//!
//! # 为什么不是用户那条 PTY
//!
//! 在用户 shell 上敲 `top` 会抢 stdin、把采样灌进 scrollback、还抢焦点。这里
//! 用 [`term_ssh::exec_capture`]：它自己开一条 SSH exec 通道，stdout 收在内存里，
//! 不与任何 tab 的 VT / scrollback 相交，也不创建窗口。调用方只在每帧问一次快照；
//! 真正阻塞的握手 / 收字节发生在专用工作线程上，UI 一帧都不会等网络。
//!
//! # 哪些平台真接，哪些降级
//!
//! | 会话 `system` | 命令 | 结果 |
//! |---|---|---|
//! | Linux | `top -bn1; free -k; /proc/uptime; /proc/net/dev` | 真接（走现有解析器） |
//! | macOS | `top -l2 -n0; vm_stat; kern.boottime; netstat -ibn` | 真接（走现有解析器） |
//! | Windows / WSL | `wsl -e sh -lc '<Linux 那条>'` | WSL 在时真接；原生 Cmd/PowerShell 没有对应纯解析器 → `—` |
//! | 其它 / 空 | 不发命令 | `—` |
//!
//! 另外，只有**内置 SSH + 内存里还握着凭据**的 tab 能建这条通道；系统 `ssh`
//! 子进程没有凭据副本，本地 shell 本来就是宿主。两者都诚实地画 `—`，不编数字。

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use crate::sysmon_sample::Reading;
use term_ssh::{exec_capture, ExecOutput, SshError, SshOptions};

/// 四段文本的分隔标记。用 `echo` 而不是注释或空行：`top` / `vm_stat` 自己会吐空行，
/// 靠空行切段必然错位。
const SPLIT: &str = "__SYSMON_SPLIT__";

/// 两次采样之间至少隔这么久；远端 exec 要重新握手，比本机 top 贵得多。
const INTERVAL: Duration = Duration::from_secs(5);

static SNAP: OnceLock<Mutex<Option<Reading>>> = OnceLock::new();
static BUSY: AtomicBool = AtomicBool::new(false);
static LAST: OnceLock<Mutex<Option<Instant>>> = OnceLock::new();
/// 目标代数：每次 `clear()` 自增。在途采样捕获请求时的代数，落地前比对；
/// 代数变了就把结果丢掉，旧机器的数据不能盖掉新目标的快照。
static GENERATION: AtomicU64 = AtomicU64::new(0);

/// 目标系统（来自 `SessionConfig::system`）。大小写与 WindTerm 历史写法都在这里收口。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Target {
    Linux,
    MacOs,
    /// Windows：只有 WSL 这条路能产出现有解析器认识的文本。
    Windows,
}

/// 会话里的 `system` 字符串 → 目标系统。`None` = 不认识，不猜、不发命令。
pub fn target_for(system: &str) -> Option<Target> {
    match system.trim().to_ascii_lowercase().as_str() {
        "linux" | "unix" | "gnu" => Some(Target::Linux),
        "macos" | "darwin" | "mac" | "osx" => Some(Target::MacOs),
        "windows" | "win" | "win32" | "wsl" => Some(Target::Windows),
        _ => None,
    }
}

/// 一次采样跑的完整远端命令。都是批处理开关，不进交互、不写文件、不重定向。
///
/// 四段依次是 CPU/负载、内存、开机时长、网络字节；每段各自交给对应的纯解析器，
/// 缺段就留 0（见 [`parse_output`]）。Windows 的 WSL 分支复用 Linux 那段命令，
/// 因此**不新增第二套解析逻辑**；未装 WSL 时 exec 失败，界面画 `—`。
pub fn sample_command(target: Target) -> &'static str {
    match target {
        Target::Linux => "top -bn1; echo __SYSMON_SPLIT__; free -k; echo __SYSMON_SPLIT__; \
                          cat /proc/uptime; echo __SYSMON_SPLIT__; cat /proc/net/dev",
        Target::MacOs => "top -l2 -n0; echo __SYSMON_SPLIT__; vm_stat; echo __SYSMON_SPLIT__; \
                          sysctl -n kern.boottime; echo __SYSMON_SPLIT__; netstat -ibn",
        Target::Windows => "wsl -e sh -lc 'top -bn1; echo __SYSMON_SPLIT__; free -k; \
                            echo __SYSMON_SPLIT__; cat /proc/uptime; echo __SYSMON_SPLIT__; \
                            cat /proc/net/dev'",
    }
}

/// 把 exec 输出切回四段。标记之后的部分才归下一段，避免 `free` 的 `Mem:` 行
/// 被 `parse_top` 当成 `top` 的摘要（后者只认 `MiB Mem :`，但两边版本一多就不保险）。
fn sections(text: &str) -> [&str; 4] {
    let mut it = text.split(SPLIT);
    // `echo MARK` 自己在标记后留下换行，紧随其后的命令输出又从行首开始；
    // 不 trim 的话 `/proc/net/dev` 的第一行会被空行顶掉，解析器会跳过真正的表头。
    [
        it.next().unwrap_or("").trim(),
        it.next().unwrap_or("").trim(),
        it.next().unwrap_or("").trim(),
        it.next().unwrap_or("").trim(),
    ]
}

/// 远端文本 → 纯读数。这是**唯一**的解析接入点：本地与远端共用 term-session
/// 的 `parse_top` / `parse_free` / `parse_vm_stat` / uptime / net 解析器。
///
/// `now_secs` 由宿主给（macOS 只报开机 epoch，要当场算差值），本函数本身不读墙钟。
/// 所有字段读不出就留 0，界面据此画 `—`。
pub fn parse_output(target: Target, text: &str, now_secs: u64) -> Reading {
    let [cpu, mem, up, net] = sections(text);
    let mut out = Reading::default();
    match target {
        Target::Linux | Target::Windows => {
            let sample = term_session::sysmon::parse_top(cpu);
            out.cpu_pct = sample.cpu_pct;
            let (used, total) = term_session::sysmon::parse_free(mem);
            out.mem_used_kb = used;
            out.mem_total_kb = total;
            out.uptime_secs = term_session::sysmon_parse::parse_uptime_linux(up).unwrap_or(0);
            if let Some((rx, tx)) = term_session::sysmon_parse::parse_net_linux(net) {
                out.net_rx_bytes = rx;
                out.net_tx_bytes = tx;
            }
        }
        Target::MacOs => {
            let sample = term_session::sysmon::parse_top_darwin(cpu);
            let (used, total) = term_session::sysmon::parse_vm_stat(mem);
            out.cpu_pct = term_session::sysmon_parse::parse_top_darwin_cpu(cpu);
            if total > 0 {
                out.mem_used_kb = used;
                out.mem_total_kb = total;
            } else {
                out.mem_used_kb = sample.mem_used_kb;
                out.mem_total_kb = sample.mem_total_kb;
            }
            if let Some(boot) = term_session::sysmon_parse::parse_boottime_darwin(up) {
                out.uptime_secs = now_secs.saturating_sub(boot);
            }
            if let Some((rx, tx)) = term_session::sysmon_parse::parse_net_darwin(net) {
                out.net_rx_bytes = rx;
                out.net_tx_bytes = tx;
            }
        }
    }
    out
}

/// exec 的结果 → 快照。任何失败都退化成「没有读数」（`None`），绝不返回半截假数字：
/// 非零退出、输出触顶截断、空 stdout 都不算一次成功采样。
///
/// 这里必须区分「采到了 0.0%」与「根本没采到」：返回值是 `Option`，失败是 `None`，
/// 界面据此画 `—`（而不是把上一次失败伪装成 0.0% 的真读数）。
pub fn from_exec(
    target: Target,
    now_secs: u64,
    result: Result<ExecOutput, SshError>,
) -> Option<Reading> {
    match result {
        Ok(out) if !out.truncated && out.exit_code.unwrap_or(0) == 0 && !out.stdout.is_empty() => {
            Some(parse_output(target, &String::from_utf8_lossy(&out.stdout), now_secs))
        }
        _ => None,
    }
}

fn unix_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

fn snap() -> &'static Mutex<Option<Reading>> {
    SNAP.get_or_init(|| Mutex::new(None))
}

/// 当前远端读数。`None` = 没开 / 还在采 / 上次失败：界面必须画 `—`。
pub fn reading() -> Option<Reading> {
    snap().lock().ok().and_then(|g| g.clone())
}

/// 只在单测里播种快照：切换远端目标的断言要能造出「上一台机器的读数」，
/// 这条路径不发命令、不碰网络（`#[cfg(test)]`，不进生产二进制）。
#[cfg(test)]
pub(crate) fn seed_reading_for_test(reading: Option<Reading>) {
    if let Ok(mut g) = snap().lock() {
        *g = reading;
    }
}

/// 工作线程落地一次采样：只有请求时的代数仍是当前代数才写 SNAP。
/// 过期结果连 `None` 都不写（新目标可能已经有读数）；调用方仍照常释放 BUSY、叫醒 UI。
fn publish(captured: u64, next: Option<Reading>) {
    let Ok(mut g) = snap().lock() else {
        return;
    };
    if GENERATION.load(Ordering::SeqCst) != captured {
        return;
    }
    *g = next;
}

/// 关掉开关 / 切换远端目标时清掉旧读数，避免下次打开先闪一帧上一台机器的数字。
/// 自增代数让已经飞出去的旧结果作废（见 [`publish`]）。
pub fn clear() {
    // 先自增再持 SNAP 锁，与 publish 的「持锁比对 + 写入」配对：旧 worker 要么
    // 先写完被这里清掉，要么看到新代数被丢弃，两种情况都不留旧机器的数字。
    GENERATION.fetch_add(1, Ordering::SeqCst);
    if let Ok(mut g) = snap().lock() {
        *g = None;
    }
    if let Some(cell) = LAST.get() {
        if let Ok(mut g) = cell.lock() {
            *g = None;
        }
    }
}

/// 请求一次采样。**不阻塞**：节流 / 忙检查之后把握手交给工作线程。
///
/// 重复请求（每帧都调）在 5 秒窗口内是 no-op；上一次还没回来时也不会再开一条。
/// 工作线程结束时 `request_repaint` 叫醒界面来取新快照。
pub fn request(
    opts: SshOptions,
    system: &str,
    wake: Arc<dyn Fn() + Send + Sync>,
) -> bool {
    let Some(target) = target_for(system) else {
        return false;
    };
    if BUSY.compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst).is_err() {
        return false;
    }
    let last = LAST.get_or_init(|| Mutex::new(None));
    let recent = match last.lock() {
        Ok(g) => g.map_or(false, |t| t.elapsed() < INTERVAL),
        Err(_) => false,
    };
    if recent {
        BUSY.store(false, Ordering::SeqCst);
        return false;
    }
    if let Ok(mut g) = last.lock() {
        *g = Some(Instant::now());
    }
    // 捕获本次请求的代数；期间 clear() 一换目标，回来时就不许再写 SNAP。
    let generation = GENERATION.load(Ordering::SeqCst);
    let command = sample_command(target);
    let spawned = std::thread::Builder::new()
        .name("sysmon-remote-exec".to_string())
        .spawn(move || {
            let result = match exec_capture(&opts, command) {
                Ok(out) => Ok(out),
                Err(e) => Err(e),
            };
            let next = from_exec(target, unix_secs(), result);
            // 同代内失败写 None（界面画 `—`）；目标已换则整个结果作废，
            // 只释放 BUSY + 叫醒界面，不碰新目标的快照。
            publish(generation, next);
            BUSY.store(false, Ordering::SeqCst);
            wake();
        });
    match spawned {
        Ok(_) => true,
        Err(_) => {
            BUSY.store(false, Ordering::SeqCst);
            false
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const LINUX: &str = concat!(
        "top - 21:14:03 up 12 days, load average: 0.52, 0.58, 0.59\n",
        "%Cpu(s):  6.2 us,  1.5 sy,  0.0 ni, 91.8 id,  0.3 wa,  0.0 hi,  0.2 si,  0.0 st\n",
        "__SYSMON_SPLIT__\n",
        "              total        used        free\n",
        "Mem:       16307848     8042136     1234500\n",
        "__SYSMON_SPLIT__\n12345.67 998.10\n__SYSMON_SPLIT__\n",
        "Inter-| Receive | Transmit\n face |bytes|bytes\n lo: 999 1 0 0 0 0 0 0 999 1 0 0 0 0 0 0\n",
        " en0: 100 2 0 0 0 0 0 0 200 3 0 0 0 0 0 0\n"
    );

    const DARWIN: &str = concat!(
        "Load Avg: 1.52, 1.58, 1.69  CPU usage: 6.25% user, 12.50% sys, 81.25% idle\n",
        "CPU usage: 10.0% user, 15.0% sys, 75.0% idle\n",
        "__SYSMON_SPLIT__\n",
        "Mach Virtual Memory Statistics: (page size of 4096 bytes)\n",
        "Pages free:  100.\nPages active:  200.\n",
        "__SYSMON_SPLIT__\n{ sec = 1600000000, usec = 0 }\n__SYSMON_SPLIT__\n",
        "Name Mtu Network Address Ipkts Ierrs Ibytes Opkts Oerrs Obytes Coll\n",
        "lo0 16384 <Link#1> 127.0.0.1 1 0 999 1 0 999 0\n",
        "en0 1500 <Link#2> aa:bb 10 0 100 12 0 200 0\n"
    );

    #[test]
    fn commands_are_selected_by_the_target_system() {
        assert_eq!(target_for("linux"), Some(Target::Linux));
        assert_eq!(target_for(" macOS "), Some(Target::MacOs));
        assert_eq!(target_for("Windows"), Some(Target::Windows));
        assert_eq!(target_for("wsl"), Some(Target::Windows));
        assert_eq!(target_for("serial"), None);
        assert!(sample_command(Target::Linux).starts_with("top -bn1"));
        assert!(sample_command(Target::MacOs).starts_with("top -l2 -n0"));
        assert!(sample_command(Target::Windows).contains("wsl -e sh -lc"));
        // 三条命令都无重定向、无交互标志。
        for t in [Target::Linux, Target::MacOs, Target::Windows] {
            let c = sample_command(t);
            assert!(!c.contains('>'));
            assert!(!c.contains('<'));
        }
    }

    #[test]
    fn linux_output_goes_through_the_existing_parsers() {
        let r = parse_output(Target::Linux, LINUX, 1_600_001_000);
        assert!((r.cpu_pct - 8.2).abs() < 0.01);
        assert_eq!(r.mem_used_kb, 8_042_136);
        assert_eq!(r.mem_total_kb, 16_307_848);
        assert_eq!(r.uptime_secs, 12_345);
        assert_eq!((r.net_rx_bytes, r.net_tx_bytes), (100, 200));
    }

    #[test]
    fn darwin_output_uses_virtual_memory_stats_and_the_last_cpu_frame() {
        let r = parse_output(Target::MacOs, DARWIN, 1_600_003_600);
        assert!((r.cpu_pct - 25.0).abs() < 0.01, "只认最后一帧的瞬时值");
        assert_eq!((r.mem_used_kb, r.mem_total_kb), (800, 1200));
        assert_eq!(r.uptime_secs, 3600);
        assert_eq!((r.net_rx_bytes, r.net_tx_bytes), (100, 200));
    }

    #[test]
    fn every_failure_mode_degrades_to_no_reading() {
        // 失败必须是 None（→ 界面 `—`），不能是 `Reading::default()`（→ 0.0%）。
        let err = from_exec(Target::Linux, 0, Err(SshError::Config("缺主机".to_string())));
        assert!(err.is_none());
        let bad_exit = ExecOutput {
            stdout: LINUX.as_bytes().to_vec(),
            stderr: Vec::new(),
            exit_code: Some(127),
            truncated: false,
        };
        assert!(from_exec(Target::Linux, 0, Ok(bad_exit)).is_none());
        let empty = ExecOutput {
            stdout: Vec::new(),
            stderr: b"sh: top: not found".to_vec(),
            exit_code: Some(0),
            truncated: false,
        };
        assert!(from_exec(Target::Linux, 0, Ok(empty)).is_none());
        let capped = ExecOutput {
            stdout: LINUX.as_bytes().to_vec(),
            stderr: Vec::new(),
            exit_code: Some(0),
            truncated: true,
        };
        assert!(from_exec(Target::Linux, 0, Ok(capped)).is_none());
        let good = ExecOutput {
            stdout: LINUX.as_bytes().to_vec(),
            stderr: Vec::new(),
            exit_code: Some(0),
            truncated: false,
        };
        let ok = from_exec(Target::Linux, 0, Ok(good));
        assert!(ok.is_some_and(|r| (r.cpu_pct - 8.2).abs() < 0.01), "真采到的 0 才是 0");
    }

    #[test]
    fn a_failed_sample_clears_the_successful_snapshot() {
        // 模拟工作线程落地：成功写 Some，失败写 None（`request()` 现在就这么做）。
        clear();
        let good = ExecOutput {
            stdout: LINUX.as_bytes().to_vec(),
            stderr: Vec::new(),
            exit_code: Some(0),
            truncated: false,
        };
        *snap().lock().unwrap() = from_exec(Target::Linux, 0, Ok(good));
        assert!(reading().is_some());
        *snap().lock().unwrap() = from_exec(
            Target::Linux,
            0,
            Err(SshError::Config("采样失败".to_string())),
        );
        assert!(reading().is_none(), "失败后不能再留上一轮的读数");
        clear();
    }

    #[test]
    fn missing_sections_keep_what_was_parsed_without_inventing_numbers() {
        let r = parse_output(Target::Linux, "top - x\n%Cpu(s): 90.0%id\n__SYSMON_SPLIT__\n", 0);
        assert!((r.cpu_pct - 10.0).abs() < 0.01);
        assert_eq!(r.mem_total_kb, 0, "没采到内存不能写成 0%");
        assert_eq!(r.uptime_secs, 0);
    }

    // 只测纯逻辑：真握手路径由 term-ssh 的 exec 端到端测试守（`tests/exec.rs`）。
    // `reading()` / `clear()` 在进程级快照上做短锁，夹具模式下不启动采样线程。
    #[test]
    fn the_snapshot_starts_empty_and_can_be_cleared() {
        clear();
        assert!(reading().is_none());
    }

    /// 切目标后，旧机器在途的采样回来时不许覆盖新目标的快照——成功值和失败
    /// 的 `None` 都不许写。同代结果照常落地。
    #[test]
    fn switching_target_drops_the_stale_in_flight_result() {
        clear();
        let stale_gen = GENERATION.load(Ordering::SeqCst);
        clear();
        let fresh_gen = GENERATION.load(Ordering::SeqCst);
        assert_ne!(stale_gen, fresh_gen, "clear() 必须让在途结果过期");
        seed_reading_for_test(Some(Reading {
            cpu_pct: 12.5,
            ..Default::default()
        }));
        publish(
            stale_gen,
            Some(Reading {
                cpu_pct: 99.9,
                ..Default::default()
            }),
        );
        assert_eq!(reading().map(|r| r.cpu_pct), Some(12.5), "旧成功值不许盖新快照");
        publish(stale_gen, None);
        assert_eq!(
            reading().map(|r| r.cpu_pct),
            Some(12.5),
            "旧失败也不许把新快照清成 None"
        );
        publish(
            fresh_gen,
            Some(Reading {
                cpu_pct: 30.0,
                ..Default::default()
            }),
        );
        assert_eq!(reading().map(|r| r.cpu_pct), Some(30.0));
        publish(fresh_gen, None);
        assert!(reading().is_none(), "同代失败仍要写 None（界面画 —）");
        clear();
    }
}

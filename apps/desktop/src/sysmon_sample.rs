//! Sysmon 面板的采样宿主（OS 资源留在 `ui/` 之外）。
//!
//! 面板只读 [`Reading`] 这份纯值。真正的采样由一条后台线程每 2 秒跑一次系统
//! 命令，经 `term_session::sysmon_parse` 的纯函数解析后写进全局快照；界面侧
//! [`reading`] 只做一次短锁，不持有 `Child` / channel / 进程句柄。
//!
//! 为什么不开控制面的 `open_exec`：这一路监控的是**宿主进程所在的这台机器**，
//! 与任何一条 SSH 会话无关。借用户的会话通道采样会把 `top` 输出灌进别人的
//! scrollback（见 `term-session/src/sysmon.rs` 模块头）。这里在宿主进程另起
//! 短命命令，输出只进模型，也不碰 PTY。
//!
//! 远端那一路由 `sysmon_remote` 负责：它走 `term_ssh::exec_capture` 的第二条
//! 非交互 exec 通道，字节进内存快照、不进用户 PTY / scrollback，最后复用本文件的
//! `Reading` 与 term-session 的纯解析器。两路互不覆盖，界面按开关选一路读。
//!
//! 三列的显示开关也放这里（原子位掩码）：它是进程级监控器的列配置，不是某个
//! 窗口的投影态。界面切换只是一次 `fetch_xor`，没有第二份可失同步的状态。

use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// 界面消费的纯读数。
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Reading {
    /// `HH:MM`（UTC，与状态栏 / margin 时间戳同一口径）。
    pub hhmm: String,
    /// 整机平均 CPU 占用（0..=100，多核是平均不是合计）。
    pub cpu_pct: f32,
    /// 已用物理内存（KiB）。
    pub mem_used_kb: u64,
    /// 物理内存总量（KiB）。
    pub mem_total_kb: u64,
    /// 开机至今秒数（0 = 还没采到）。
    pub uptime_secs: u64,
    /// 网卡累计接收 / 发送字节（跳过 loopback）。
    pub net_rx_bytes: u64,
    pub net_tx_bytes: u64,
}

/// 采样间隔。兼容终端读数条是「越几秒跳一次」而不是逐帧刷新，2 秒既跟得上
/// 肉眼又不会让 `top` 常驻。
const SAMPLE_EVERY: Duration = Duration::from_secs(2);

static SNAPSHOT: OnceLock<Mutex<Reading>> = OnceLock::new();
static STARTED: AtomicU8 = AtomicU8::new(0);

/// T / C / M 三列显示位。bit0 = T(time)，bit1 = C(system)，bit2 = M(mem)。
static COLUMNS: AtomicU8 = AtomicU8::new(0b111);

pub const COL_TIME: u8 = 0b001;
pub const COL_SYSTEM: u8 = 0b010;
pub const COL_MEM: u8 = 0b100;

/// 这一列当前开不开。
pub fn column_visible(bit: u8) -> bool {
    COLUMNS.load(Ordering::Relaxed) & bit != 0
}

/// 点 T / C / M 头上的方块：翻转对应列的显示。
pub fn toggle_column(bit: u8) {
    COLUMNS.fetch_xor(bit, Ordering::Relaxed);
}

/// 宿主在每帧 `update()` 里调一次；线程只起一次，重复调用是 no-op。
pub fn ensure_started() {
    if STARTED.swap(1, Ordering::Relaxed) != 0 {
        return;
    }
    let _ = SNAPSHOT.set(Mutex::new(Reading::default()));
    let _ = std::thread::Builder::new()
        .name("sysmon-sample".to_string())
        .spawn(|| loop {
            std::thread::sleep(SAMPLE_EVERY);
            let sample = sample_once();
            if let Some(cell) = SNAPSHOT.get() {
                if let Ok(mut guard) = cell.lock() {
                    *guard = sample;
                }
            }
        });
}

/// 取当前快照。线程还没起 / 采样失败时返回全 0，界面画 `—` 而不是假数。
pub fn reading() -> Reading {
    SNAPSHOT
        .get()
        .and_then(|cell| cell.lock().ok().map(|guard| guard.clone()))
        .unwrap_or_default()
}

/// UTC `HH:MM`，与 `status_bar::format_utc` / `margin::stamp_text` 同源。
fn hhmm() -> String {
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() % 86_400)
        .unwrap_or(0);
    format!("{:02}:{:02}", secs / 3600, secs % 3600 / 60)
}

#[cfg(target_os = "macos")]
fn sample_once() -> Reading {
    use std::process::Command;

    // `top -l2`：第一帧的 CPU 是开机累计平均，第二帧才是瞬时值。
    let cpu = Command::new("top")
        .args(["-l", "2", "-n", "0"])
        .output()
        .ok()
        .map(|out| {
            term_session::sysmon_parse::parse_top_darwin_cpu(&String::from_utf8_lossy(&out.stdout))
        })
        .unwrap_or(0.0);
    let (used, total) = Command::new("vm_stat")
        .output()
        .ok()
        .map(|out| {
            term_session::sysmon_parse::parse_vm_stat(&String::from_utf8_lossy(&out.stdout))
        })
        .unwrap_or((0, 0));
    let uptime_secs = Command::new("sysctl")
        .args(["-n", "kern.boottime"])
        .output()
        .ok()
        .and_then(|out| term_session::sysmon_parse::parse_boottime_darwin(&String::from_utf8_lossy(&out.stdout)))
        .and_then(|boot| SystemTime::now().duration_since(UNIX_EPOCH).ok().map(|now| now.as_secs().saturating_sub(boot)))
        .unwrap_or(0);
    let (net_rx_bytes, net_tx_bytes) = Command::new("netstat")
        .args(["-ibn"])
        .output()
        .ok()
        .and_then(|out| term_session::sysmon_parse::parse_net_darwin(&String::from_utf8_lossy(&out.stdout)))
        .unwrap_or((0, 0));
    Reading {
        hhmm: hhmm(),
        cpu_pct: cpu,
        mem_used_kb: used,
        mem_total_kb: total,
        uptime_secs,
        net_rx_bytes,
        net_tx_bytes,
    }
}

#[cfg(not(target_os = "macos"))]
fn sample_once() -> Reading {
    use std::process::Command;

    let text = Command::new("top")
        .args(["-bn1"])
        .output()
        .ok()
        .map(|out| String::from_utf8_lossy(&out.stdout).into_owned())
        .unwrap_or_default();
    let sample = term_session::sysmon_parse::parse_top(&text);
    let text = Command::new("free")
        .arg("-k")
        .output()
        .ok()
        .map(|out| String::from_utf8_lossy(&out.stdout).into_owned())
        .unwrap_or_default();
    let (used, total) = term_session::sysmon_parse::parse_free(&text);
    let uptime_secs = Command::new("cat")
        .arg("/proc/uptime")
        .output()
        .ok()
        .and_then(|out| term_session::sysmon_parse::parse_uptime_linux(&String::from_utf8_lossy(&out.stdout)))
        .unwrap_or(0);
    let (net_rx_bytes, net_tx_bytes) = Command::new("cat")
        .arg("/proc/net/dev")
        .output()
        .ok()
        .and_then(|out| term_session::sysmon_parse::parse_net_linux(&String::from_utf8_lossy(&out.stdout)))
        .unwrap_or((0, 0));
    Reading {
        hhmm: hhmm(),
        cpu_pct: sample.cpu_pct,
        mem_used_kb: used,
        mem_total_kb: total,
        uptime_secs,
        net_rx_bytes,
        net_tx_bytes,
    }
}

// macOS 瞬时 CPU 的解析在 term-session 的纯解析层
// （`parse_top_darwin_cpu`）：本地 top 与远端 exec 两路共用一份，
// 这里不再留第二套 `split(',')`。

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn columns_start_all_on_and_toggle_independently() {
        // 每个测试进程一开始都是出厂态；注意别的测试若也翻转过，这里只看相对值。
        let before = COLUMNS.load(Ordering::Relaxed);
        toggle_column(COL_TIME);
        assert_ne!(COLUMNS.load(Ordering::Relaxed) & COL_TIME, before & COL_TIME);
        assert_eq!(COLUMNS.load(Ordering::Relaxed) & COL_SYSTEM, before & COL_SYSTEM);
        toggle_column(COL_TIME);
        assert_eq!(COLUMNS.load(Ordering::Relaxed), before);
    }

    #[test]
    fn the_shared_darwin_cpu_parser_is_used_by_the_host_path() {
        // 本地与远端共用 term-session 那一处解析；这里只钉住「拿得到值」。
        // 边界用例（最后一帧 / 缺 idle）在纯解析层的单测里。
        let text = "CPU usage: 6.25% user, 12.50% sys, 81.25% idle\n";
        assert!((term_session::sysmon_parse::parse_top_darwin_cpu(text) - 18.75).abs() < 0.01);
    }
}

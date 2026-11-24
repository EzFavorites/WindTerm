//! 采样文本 → [`Sample`] 的纯解析（roadmap）。
//!
//! 每个解析器都面对三类现实输入：不同版本的列名、被截断/加宽的输出、以及
//! 权限不足或命令不存在时的空串或半截报错。约定只有一条：**能读出多少算
//! 多少，读不出的字段留 0，永远不 `Err` 也不 panic**。
//!
//! 不要在这里 spawn 进程或读环境变量——这一层只认字符串。命令由
//! [`crate::sysmon`] 的常量给出，通道由上层开。

// uptime / network 解析器拆出以守住 500 代码行门禁；公共 API 从本模块再导出。
#[path = "sysmon_parse_extra.rs"]
mod extra;
pub use extra::*;

use crate::sysmon::{Proc, Sample};

/// 结果上限：进程表只留 CPU 最高的这些条，再多界面也画不下。
pub const PROC_LIMIT: usize = 20;

/// `top -bn1`（Linux）。负载 / CPU / 内存三项各自独立解析，缺哪项留 0。
///
/// **进程表不在这里解**：`top` 的进程列数随终端宽度变化，`-bn1` 在窄终端
/// 下会把 `COMMAND+` 截断、在宽终端下多出 `S`/`TIME+` 等列，行内对齐不可
/// 靠。进程一律走 [`parse_ps`]，那里的列名由 `-eo` 钉死。
pub fn parse_top(text: &str) -> Sample {
    let mut s = Sample::default();
    for line in text.lines() {
        let l = line.trim();
        // Linux 的 `load average:` 在首行中段（前面还有 `top - 21:14:03`），
        // 所以先定位再切片，否则会把时间里的冒号当成分隔符。
        if let Some(i) = l.find("load average:") {
            s.load = parse_load(&l[i..]);
            continue;
        }
        let cpu = l
            .strip_prefix("%Cpu(s):")
            .or_else(|| l.strip_prefix("Cpu(s):"))
            .and_then(cpu_busy);
        if let Some(busy) = cpu {
            s.cpu_pct = busy;
            continue;
        }
        if !is_top_mem_line(l) {
            continue;
        }
        if let Some((used, total)) = top_mem(l) {
            s.mem_used_kb = used;
            s.mem_total_kb = total;
        }
    }
    s
}

/// `top -l1`（macOS）。
///
/// 只解负载与物理内存：`top -l1` 的 `CPU usage:` 是自开机以来的**累计**
/// 平均（macOS 要 `-l2` 才有瞬时值），拿来当实时占用会长期糊在一个数上；
/// 与其报一个错数，不如留 0 让上层用两次采样算差值。进程同样走 [`parse_ps`]。
pub fn parse_top_darwin(text: &str) -> Sample {
    let mut s = Sample::default();
    for line in text.lines() {
        let l = line.trim();
        if let Some(i) = l.find("Load Avg:") {
            s.load = parse_load(&l[i..]);
            continue;
        }
        let mem = l.strip_prefix("PhysMem:").and_then(darwin_phys_mem);
        if let Some((used, total)) = mem {
            s.mem_used_kb = used;
            s.mem_total_kb = total;
        }
    }
    s
}

/// `free -k`（Linux）。返回 `(used_kb, total_kb)`——顺序与
/// [`Sample`] 的字段顺序一致，别在调用处靠猜。
///
/// 只认开头是 `Mem` 的行：老版本 `free` 的 `-/+ buffers/cache:` 行是**派生**
/// 值（去掉缓存后的真实占用），单位与主表不同、列的含义也不同，混进来会把
/// 内存读成两倍。它天然不匹配 `Mem` 前缀，因此直接跳过。
pub fn parse_free(text: &str) -> (u64, u64) {
    for line in text.lines() {
        let l = line.trim_start();
        if !l.starts_with("Mem") {
            continue;
        }
        let Some((_, body)) = l.split_once(':') else {
            continue;
        };
        // 位置取值而不是「过滤出所有数字」：少一列时不能把后面的数字顶上
        // 来当 used。
        let mut it = body.split_whitespace();
        let total = it.next().and_then(|t| t.parse::<u64>().ok()).unwrap_or(0);
        let used = it.next().and_then(|t| t.parse::<u64>().ok()).unwrap_or(0);
        return (used, total);
    }
    (0, 0)
}

/// `ps -eo pid,pcpu,pmem,comm` 的解析。
///
/// - 表头（`PID %CPU ...`）因为 pid 不是数字被丢掉；
/// - pid 非数字、`pcpu` / `pmem` 缺失或非数字 → **整行丢弃**：列一少就无法
///   判断剩下的字段属于哪一列，猜着填会把内存当 CPU；
/// - `comm` 里的空格原样保留（`emacs -nw` 是合法进程名）；
/// - 结果按 CPU 降序，同值保持输入顺序（稳定排序），最多 [`PROC_LIMIT`] 条。
pub fn parse_ps(text: &str) -> Vec<Proc> {
    let mut out = Vec::new();
    for line in text.lines() {
        let Some((pid, cpu, mem, name)) = ps_row(line) else {
            continue;
        };
        if name.is_empty() {
            continue;
        }
        out.push(Proc {
            pid,
            cpu,
            mem,
            name: name.to_string(),
        });
    }
    out.sort_by(|a, b| b.cpu.partial_cmp(&a.cpu).unwrap_or(std::cmp::Ordering::Equal));
    out.truncate(PROC_LIMIT);
    out
}

/// `vm_stat`（macOS）。返回 `(used_kb, total_kb)`。
///
/// 口径：`total = (free + active + inactive + speculative + wired +
/// compressor) × 页大小`，`used = total − free`。页大小取首行括号里的值
/// （Apple Silicon 上是 16384，Intel 上是 4096）；读不到就按 4096 算。
///
/// 只用物理页口径，不用 `Pages stored in compressor`——那是压缩前的逻辑
/// 页数，乘页大小会把占用算大。
pub fn parse_vm_stat(text: &str) -> (u64, u64) {
    let page_bytes = page_size_bytes(text).unwrap_or(4096);
    let Some(free) = page_count(text, "Pages free:") else {
        return (0, 0);
    };
    let pages = free
        + page_count(text, "Pages active:").unwrap_or(0)
        + page_count(text, "Pages inactive:").unwrap_or(0)
        + page_count(text, "Pages speculative:").unwrap_or(0)
        + page_count(text, "Pages wired down:").unwrap_or(0)
        + page_count(text, "Pages occupied by compressor:").unwrap_or(0);
    let to_kb = |p: u64| p.saturating_mul(page_bytes) / 1024;
    (to_kb(pages - free), to_kb(pages))
}

// ── 行级小工具 ─────────────────────────────────────────────────────────

/// 取字符串开头的数字（`12G` → 12，`45678.` → 45678，`1.69  CPU` → 1.69）。
///
/// 比 `str::parse` 宽松是必要的：`top` 的 `load average` 后面紧跟着别的
/// 字段、`vm_stat` 的数字带句点、老版 `top` 写成 `8175544k`。
fn leading_num(s: &str) -> Option<f64> {
    let t = s.trim_start();
    let end = t
        .find(|c: char| !(c.is_ascii_digit() || c == '.' || c == '-' || c == '+'))
        .unwrap_or(t.len());
    t[..end].trim_end_matches('.').parse().ok()
}

/// `load average: 0.52, 0.58, 0.59` / `Load Avg: 1.52, 1.58, 1.69  CPU usage: …`
fn parse_load(line: &str) -> [f32; 3] {
    let body = line.split_once(':').map(|(_, r)| r).unwrap_or("");
    let mut out = [0.0f32; 3];
    let mut it = body.split(',');
    for slot in out.iter_mut() {
        *slot = it.next().and_then(leading_num).unwrap_or(0.0) as f32;
    }
    out
}

/// `"%Cpu(s)"` 行的身体 → 整机占用百分比。
///
/// 优先 `100 − idle`（`top` 自己的定义，多核已平均）；没有 `id` 列时退化为
/// `us + sy + ni + hi + si` 求和。`st`（steal）不计：它已经从 guest 时间里
/// 扣过，再加会和 `us` 重复。
fn cpu_busy(body: &str) -> Option<f32> {
    let mut idle = None;
    let mut busy = 0.0f32;
    let mut saw_busy = false;
    for piece in body.split(',') {
        let Some((v, unit)) = cpu_piece(piece) else {
            continue;
        };
        match unit {
            "id" => idle = Some(v),
            "us" | "sy" | "ni" | "hi" | "si" => {
                busy += v;
                saw_busy = true;
            }
            _ => {}
        }
    }
    if let Some(idle) = idle {
        Some((100.0 - idle).clamp(0.0, 100.0))
    } else if saw_busy {
        Some(busy.clamp(0.0, 100.0))
    } else {
        None
    }
}

/// 一个 CPU 分片：`"6.2 us"`（procps-ng）或 `"12.5%us"`（老版本）。
fn cpu_piece(piece: &str) -> Option<(f32, &str)> {
    let p = piece.trim();
    if let Some((v, unit)) = p.split_once('%') {
        return Some((v.trim().parse().ok()?, unit.trim()));
    }
    let mut it = p.split_whitespace();
    let v = it.next()?.parse().ok()?;
    Some((v, it.next().unwrap_or("")))
}

/// 是不是 `top` 的内存摘要行。`MiB Mem :` / `KiB Mem :` / 老版 `Mem:`。
///
/// 必须排掉 `Swap:` 行：它的末尾也有 `avail Mem` 字样，只看 `Mem` 会误判，
/// 把 swap 总量当成物理内存。
fn is_top_mem_line(l: &str) -> bool {
    let header = l.split(':').next().unwrap_or("");
    header.contains("Mem") && !header.contains("Swap")
}

/// `MiB Mem : 15926.6 total, 1234.5 free, 8042.1 used, …` → `(used, total)` 的 KiB。
fn top_mem(line: &str) -> Option<(u64, u64)> {
    let (header, body) = line.split_once(':')?;
    let unit = header
        .split_whitespace()
        .find(|t| kb_factor(t).is_some())
        .unwrap_or("KiB");
    let mut used = None;
    let mut total = None;
    for piece in body.split(',') {
        let mut it = piece.split_whitespace();
        let value = it.next().and_then(leading_num);
        match (value, it.next()) {
            (Some(v), Some("total")) => total = Some(to_kb(v, unit)),
            (Some(v), Some("used")) => used = Some(to_kb(v, unit)),
            _ => {}
        }
    }
    Some((used?, total?))
}

/// `PhysMem: 12G used (1234M wired), 3456M unused.` → `(used, total)` 的 KiB。
///
/// macOS 不给「总量」，只给 used / unused，总量由两者相加得到。
fn darwin_phys_mem(body: &str) -> Option<(u64, u64)> {
    let toks: Vec<&str> = body.split_whitespace().collect();
    let mut used = None;
    let mut unused = None;
    for pair in toks.windows(2) {
        let (Some(v), Some(unit)) = (leading_num(pair[0]), unit_of(pair[0])) else {
            continue;
        };
        // `unused.` 带句点，去掉再比；`(1234M wired),` 这类括号项不匹配。
        match pair[1].trim_end_matches(['.', ',']) {
            "used" => used = Some(to_kb(v, unit)),
            "unused" => unused = Some(to_kb(v, unit)),
            _ => {}
        }
    }
    Some((used?, used?.saturating_add(unused?)))
}

/// `"12G"` → `"G"`；纯数字 / 以 `(` 开头的分片返回 `None`。
fn unit_of(tok: &str) -> Option<&str> {
    let t = tok.trim_start_matches('(');
    let idx = t.find(|c: char| c.is_ascii_alphabetic())?;
    let unit = &t[idx..];
    kb_factor(unit).map(|_| unit)
}

/// `vm_stat` 首行：`(page size of 16384 bytes)`。
fn page_size_bytes(text: &str) -> Option<u64> {
    let start = text.find("page size of ")? + "page size of ".len();
    let rest = &text[start..];
    let end = rest.find(|c: char| !c.is_ascii_digit())?;
    rest[..end].parse().ok()
}

/// `Pages free:  45678.` → 45678。
fn page_count(text: &str, key: &str) -> Option<u64> {
    for line in text.lines() {
        let l = line.trim();
        let Some(rest) = l.strip_prefix(key) else {
            continue;
        };
        let v = rest.trim().trim_end_matches('.');
        return v.parse().ok();
    }
    None
}

/// 单位 → KiB 倍数。认大小写两种写法（`MiB` / `M` / `MB`），
/// 不认得的单位返回 `None`，由调用方回退到默认值。
fn kb_factor(unit: &str) -> Option<f64> {
    match unit {
        "KiB" | "kB" | "KB" | "K" => Some(1.0),
        "MiB" | "MB" | "M" => Some(1024.0),
        "GiB" | "GB" | "G" => Some(1024.0 * 1024.0),
        "TiB" | "TB" | "T" => Some(1024.0 * 1024.0 * 1024.0),
        "B" => Some(1.0 / 1024.0),
        _ => None,
    }
}

/// 带单位的值 → KiB。负数 / NaN / Inf 一律 0（命令输出里不该有，但别让它
/// 溢出成天文数字）。
fn to_kb(v: f64, unit: &str) -> u64 {
    let factor = kb_factor(unit).unwrap_or(1.0);
    if !v.is_finite() || v <= 0.0 {
        return 0;
    }
    (v * factor) as u64
}

/// 一行 `ps`：前三个空白分隔的字段 + 其余部分当名字。
fn ps_row(line: &str) -> Option<(u32, f32, f32, &str)> {
    let mut rest = line.trim_start();
    let mut parts = [""; 3];
    for slot in parts.iter_mut() {
        // 每轮先跳过上一轮留下的分隔空白，否则第二个字段永远是空串。
        rest = rest.trim_start();
        if rest.is_empty() {
            return None;
        }
        let end = rest.find(char::is_whitespace).unwrap_or(rest.len());
        *slot = &rest[..end];
        rest = &rest[end..];
    }
    let pid: u32 = parts[0].parse().ok()?;
    let cpu: f32 = parts[1].parse().ok()?;
    let mem: f32 = parts[2].parse().ok()?;
    Some((pid, sane_pct(cpu), sane_pct(mem), rest.trim()))
}

/// `ps` 的 `%CPU` 偶尔会给出 `-1.0` 或极值；夹到 0..=100 并清掉 NaN。
fn sane_pct(v: f32) -> f32 {
    if v.is_finite() {
        v.clamp(0.0, 100.0)
    } else {
        0.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// procps-ng 3.3 的真实形状（`MiB Mem`、`%Cpu(s)` 带空格分隔）。
    const TOP_LINUX: &str = r#"top - 21:14:03 up 12 days,  3:41,  2 users,  load average: 0.52, 0.58, 0.59
Tasks: 231 total,   1 running, 229 sleeping,   0 stopped,   1 zombie
%Cpu(s):  6.2 us,  1.5 sy,  0.0 ni, 91.8 id,  0.3 wa,  0.0 hi,  0.2 si,  0.0 st
MiB Mem :  15926.6 total,   1234.5 free,   8042.1 used,   6649.9 buff/cache
MiB Swap:   8192.0 total,   8191.2 free,      0.8 used.   7102.3 avail Mem

    PID USER      PR  NI    VIRT    RES    SHR S  %CPU  %MEM     TIME+ COMMAND
   2893 alice     20   0 4123456 512344  98760 R  12.3   3.2   1:23.45 firefox
"#;

    /// 老版 `top`（`Cpu(s): 12.5%us`、`Mem:` 无单位、数字带 `k`）。
    const TOP_LINUX_OLD: &str = r#"top - 10:02:11 up 1 day,  4 users,  load average: 1.20, 1.10, 0.90
Tasks:  95 total,   2 running,  93 sleeping,   0 stopped,   0 zombie
Cpu(s): 12.5%us,  3.1%sy,  0.0%ni, 83.9%id,  0.4%wa,  0.0%hi,  0.1%si,  0.0%st
Mem:   8175544k total,  2054320k used,  6121224k free,   123456k buffers
Swap:  2097148k total,        0k used,  2097148k free,   456780k cached
"#;

    const TOP_DARWIN: &str = r#"Processes: 412 total, 2 running, 410 sleeping, 1234 threads
2026-11-09 21:14:03.123456+0800
Load Avg: 1.52, 1.58, 1.69  CPU usage: 6.25% user, 12.50% sys, 81.25% idle
SharedLibs: 158M resident, 20M data, 0B linkedit.
MemRegions: 41234 total, 1234M resident, 0 private, 0 shared.
PhysMem: 12G used (1234M wired), 3456M unused.
VM: 1234G vsize, 1234M framework vsize, 0(0) swapins, 0(0) swapouts.
Networks: packets: 12345/1234 in, 2345/234 out.
Disks: 1234/4567 read, 890/1234 written.
"#;

    const FREE_LINUX: &str = r#"              total        used        free      shared  buff/cache   available
Mem:       16307848     8042136     1234500      123456     7031212     7102344
Swap:       8388604           0     8388604
"#;

    const FREE_LINUX_OLD: &str = r#"             total       used       free     shared    buffers     cached
Mem:       8175544    2054320    6121224          0     123456     456780
-/+ buffers/cache:    1474084    6701460
Swap:      2097148          0    2097148
"#;

    const PS_LINUX: &str = r#"    PID %CPU %MEM COMMAND
      1  0.0  0.1 /sbin/init
    412  0.0  0.3 /lib/systemd/systemd-journald
   2893 12.3  3.2 /usr/lib/firefox/firefox
   3011  5.7  1.1 /usr/lib/firefox/firefox -contentproc
    777  2.0  1.0 emacs -nw
   3456  0.0  0.0 [kworker/0:1]
"#;

    const VM_STAT: &str = r#"Mach Virtual Memory Statistics: (page size of 16384 bytes)
Pages free:                               45678.
Pages active:                            123456.
Pages inactive:                          123000.
Pages speculative:                         4567.
Pages throttled:                              0.
Pages wired down:                         98765.
Pages purgeable:                           1234.
"Translation faults":                  12345678.
Pages copy-on-write:                     234567.
Pages zero filled:                      8765432.
Pages reactivated:                        12345.
File-backed pages:                        56789.
Anonymous pages:                         234567.
Pages stored in compressor:              123456.
Pages occupied by compressor:             67890.
Decompressions:                          234567.
Swapins:                                      0.
Swapouts:                                     0.
"#;

    fn close(a: f32, b: f32) -> bool {
        (a - b).abs() < 0.01
    }

    #[test]
    fn linux_top_yields_load_cpu_and_memory() {
        let s = parse_top(TOP_LINUX);
        assert_eq!(s.load, [0.52, 0.58, 0.59]);
        assert!(close(s.cpu_pct, 8.2), "100 - 91.8 id，得到 {}", s.cpu_pct);
        assert_eq!(s.mem_total_kb, 16_308_838, "15926.6 MiB → KiB");
        assert_eq!(s.mem_used_kb, 8_235_110, "8042.1 MiB → KiB");
        assert!(s.procs.is_empty(), "进程表只从 ps 来");
    }

    #[test]
    fn the_old_linux_top_spelling_still_parses() {
        let s = parse_top(TOP_LINUX_OLD);
        assert_eq!(s.load, [1.20, 1.10, 0.90]);
        assert!(close(s.cpu_pct, 16.1), "100 - 83.9，得到 {}", s.cpu_pct);
        assert_eq!(s.mem_total_kb, 8_175_544, "老格式的单位是 KiB");
        assert_eq!(s.mem_used_kb, 2_054_320);
    }

    #[test]
    fn linux_top_without_the_summary_lines_degrades_to_zeros() {
        let s = parse_top("Tasks: 1 total, 1 running\n\n");
        assert_eq!(s, Sample::default());
    }

    #[test]
    fn free_reports_used_then_total_from_the_mem_row() {
        assert_eq!(parse_free(FREE_LINUX), (8_042_136, 16_307_848));
    }

    #[test]
    fn free_ignores_the_derived_buffers_cache_row() {
        // 老格式里 `-/+ buffers/cache` 也是两个数字，但它不是 Mem 行；
        // 若被读进来，used 会变成缓存扣减后的 1474084。
        assert_eq!(parse_free(FREE_LINUX_OLD), (2_054_320, 8_175_544));
        let only_derived = "-/+ buffers/cache:  1474084  6701460\n";
        assert_eq!(parse_free(only_derived), (0, 0));
    }

    #[test]
    fn free_with_missing_columns_keeps_what_it_has() {
        assert_eq!(parse_free("Mem:  8175544\n"), (0, 8_175_544));
        assert_eq!(parse_free(""), (0, 0));
        assert_eq!(parse_free("              total        used\n"), (0, 0));
    }

    #[test]
    fn ps_sorts_by_cpu_descending_and_caps_the_list() {
        let procs = parse_ps(PS_LINUX);
        let pids: Vec<u32> = procs.iter().map(|p| p.pid).collect();
        assert_eq!(pids, vec![2893, 3011, 777, 1, 412, 3456]);
        assert!(close(procs[0].cpu, 12.3));
        assert!(close(procs[0].mem, 3.2));
        assert_eq!(procs[0].name, "/usr/lib/firefox/firefox");
        // comm 里带空格的部分原样保留（`-contentproc` 不是列）。
        assert_eq!(procs[1].name, "/usr/lib/firefox/firefox -contentproc");
        assert_eq!(procs[2].name, "emacs -nw");
        assert_eq!(procs[5].name, "[kworker/0:1]");
    }

    #[test]
    fn ps_drops_malformed_rows_instead_of_guessing_columns() {
        let text = r#"    PID %CPU %MEM COMMAND
   abc  1.0  2.0 bad-pid
   222 0.0
   333   9.9 half-row
"#;
        // `222` 只有两列；`333` 缺 pmem，`half-row` 落在第三列上又解析不成
        // 数字——两种情况都无法判断列归属，只能丢。
        assert!(parse_ps(text).is_empty());
    }

    #[test]
    fn ps_keeps_zero_cpu_processes_after_the_busy_ones() {
        let text = "  PID %CPU %MEM COMMAND\n 11 0.0 0.0 idle\n 22 0.0 0.0 idle2\n";
        let procs = parse_ps(text);
        assert_eq!(procs.len(), 2);
        assert_eq!((procs[0].pid, procs[0].cpu), (11, 0.0));
        assert!(parse_ps("").is_empty());
        assert!(parse_ps("  PID %CPU %MEM COMMAND\n").is_empty(), "只有表头");
    }

    #[test]
    fn darwin_top_yields_load_and_physical_memory_only() {
        let s = parse_top_darwin(TOP_DARWIN);
        assert_eq!(s.load, [1.52, 1.58, 1.69]);
        assert_eq!(s.mem_used_kb, 12 * 1024 * 1024, "12G → KiB");
        assert_eq!(
            s.mem_total_kb,
            12 * 1024 * 1024 + 3456 * 1024,
            "used + unused"
        );
        assert_eq!(s.cpu_pct, 0.0, "首帧 CPU 是累计均值，不冒充实时值");
        assert!(s.procs.is_empty());
    }

    #[test]
    fn darwin_top_without_phys_mem_keeps_only_the_load() {
        let s = parse_top_darwin("Load Avg: 0.10, 0.20, 0.30\n");
        assert_eq!(s.load, [0.10, 0.20, 0.30]);
        assert_eq!((s.mem_used_kb, s.mem_total_kb), (0, 0));
        assert_eq!(parse_top_darwin(""), Sample::default());
    }

    #[test]
    fn vm_stat_uses_the_reported_page_size() {
        // total 页 = 45678+123456+123000+4567+98765+67890 = 463356
        // used 页 = 417678；× 16384 / 1024 = ×16
        assert_eq!(parse_vm_stat(VM_STAT), (6_682_848, 7_413_696));
    }

    #[test]
    fn vm_stat_falls_back_to_4k_pages_and_survives_truncation() {
        let text = "Mach Virtual Memory Statistics: (page size of 4096 bytes)\nPages free:  100.\nPages active:  200.\n";
        assert_eq!(parse_vm_stat(text), (800, 1200), "used = (300-100)×4 KiB");
        let no_page_size = "Pages free: 100\nPages wired down: 100\n";
        assert_eq!(parse_vm_stat(no_page_size), (400, 800), "缺页大小按 4 KiB");
        assert_eq!(parse_vm_stat(""), (0, 0));
    }

    #[test]
    fn top_and_free_compose_into_one_linux_sample() {
        // 上层的用法：cpu/load 从 top，内存以 free -k 为准（top 的单位随
        // 版本变，free 的 -k 是钉死的）。
        let mut s = parse_top(TOP_LINUX);
        let (used, total) = parse_free(FREE_LINUX);
        s.mem_used_kb = used;
        s.mem_total_kb = total;
        s.procs = parse_ps(PS_LINUX);
        assert_eq!(s.mem_total_kb, 16_307_848);
        assert_eq!(s.procs.len(), 6);
    }

    #[test]
    fn non_finite_and_negative_numbers_never_leak_into_the_model() {
        let text = "  PID %CPU %MEM COMMAND\n  1 nan inf weird\n  2 -3.0 -4.0 neg\n";
        let procs = parse_ps(text);
        assert_eq!(procs.len(), 2);
        assert_eq!(procs[0].pid, 1);
        assert_eq!(procs[0].cpu, 0.0, "NaN 归 0");
        assert_eq!(procs[1].cpu, 0.0, "负数夹到 0");
        assert_eq!(procs[1].mem, 0.0);
    }

    #[test]
    fn a_cpu_line_with_only_busy_columns_uses_the_sum() {
        let text = "Cpu(s): 10.0%us,  5.0%sy,  0.0%ni\n";
        let s = parse_top(text);
        assert!(close(s.cpu_pct, 15.0), "没有 id 列时退化为求和");
    }

}

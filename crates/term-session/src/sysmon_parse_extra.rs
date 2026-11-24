//! Sysmon 的 uptime / network 纯解析器。从 `sysmon_parse.rs` 拆出只为守住
//! 单文件 500 代码行门禁；公共 API 经 `pub use` 不变。
//!
//! 所有函数都只读字符串、返回 `Option`，零 IO、零墙钟（采样与当前时刻由宿主给）。

/// Linux `/proc/uptime` 的首个浮点秒数。
pub fn parse_uptime_linux(text: &str) -> Option<u64> {
    let first = text.split_whitespace().next()?;
    let secs = first.parse::<f64>().ok()?;
    if !secs.is_finite() || secs < 0.0 {
        return None;
    }
    Some(secs as u64)
}

/// macOS `sysctl -n kern.boottime` 的 `sec = <epoch>`。
pub fn parse_boottime_darwin(text: &str) -> Option<u64> {
    let start = text.find("sec = ")? + 6;
    let digits: String = text[start..]
        .chars()
        .take_while(|c| c.is_ascii_digit())
        .collect();
    digits.parse().ok()
}

/// `/proc/net/dev`：累加除 `lo` 外各网卡的接收/发送字节。
pub fn parse_net_linux(text: &str) -> Option<(u64, u64)> {
    let mut rx = 0u64;
    let mut tx = 0u64;
    let mut seen = false;
    for line in text.lines().skip(2) {
        let mut fields = line.split_whitespace();
        let name = fields.next()?;
        if name.starts_with("lo:") {
            continue;
        }
        let r = fields.next()?.parse::<u64>().ok()?;
        for _ in 0..7 {
            fields.next()?;
        }
        let t = fields.next()?.parse::<u64>().ok()?;
        rx = rx.saturating_add(r);
        tx = tx.saturating_add(t);
        seen = true;
    }
    seen.then_some((rx, tx))
}

/// macOS `netstat -ibn`：列序 `[Name Mtu Network Address Ipkts Ierrs Ibytes Opkts Oerrs Obytes Coll]`。
pub fn parse_net_darwin(text: &str) -> Option<(u64, u64)> {
    let mut rx = 0u64;
    let mut tx = 0u64;
    let mut seen = false;
    for line in text.lines().skip(1) {
        let cols: Vec<&str> = line.split_whitespace().collect();
        if cols.len() < 10 || cols[0] == "lo0" {
            continue;
        }
        let r = cols[6].parse::<u64>().ok()?;
        let t = cols[9].parse::<u64>().ok()?;
        rx = rx.saturating_add(r);
        tx = tx.saturating_add(t);
        seen = true;
    }
    seen.then_some((rx, tx))
}


#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn uptime_and_boottime_parsers_ignore_garbage() {
        assert_eq!(parse_uptime_linux("12345.67 998.10"), Some(12345));
        assert_eq!(parse_uptime_linux("nan"), None);
        assert_eq!(parse_boottime_darwin("{ sec = 1600000000, usec = 0 }"), Some(1600000000));
        assert_eq!(parse_boottime_darwin("no time"), None);
    }

    #[test]
    fn network_parsers_sum_interfaces_but_skip_loopback() {
        let linux = "Inter-|   Receive                                                |  Transmit\n\
            face |bytes    packets errs drop fifo frame compressed multicast|bytes    packets errs drop fifo colls carrier compressed\n\
              lo: 999999       1    0    0    0     0          0         0   999999       1    0    0    0     0       0          0\n\
              en0:     100       2    0    0    0     0          0         0      200       3    0    0    0     0       0          0\n";
        assert_eq!(parse_net_linux(linux), Some((100, 200)));
        let darwin = "Name       Mtu Network       Address              Ipkts Ierrs     Ibytes    Opkts Oerrs     Obytes  Coll\n\
            lo0        16384 <Link#1>      127.0.0.1          12345     0    999999    12345     0    999999     0\n\
            en0         1500 <Link#2>      aa:bb:cc:dd         10     0       100        12     0       200     0\n";
        assert_eq!(parse_net_darwin(darwin), Some((100, 200)));
    }

}

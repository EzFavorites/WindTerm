//! 传输状态行的**状态与措辞**：一帧要画什么、这帧该说哪句话、字节数怎么念。
//!
//! 与 `xfer.rs` 分开是为了 450 行上限——状态 + 措辞 + 单测和绘制面
//! 加起来 507 行会一条门禁红掉整个阶段。分工照 `dialogs/log_settings_spec.rs`
//! 的先例：**本文件不含 `egui`**，只有纯数据与纯字符串；`xfer.rs` 只画。
//!
//! # 四种（五种）状态互不混淆
//!
//! | 状态 | 长什么样 |
//! |---|---|
//! | 在跑 | `Receiving into /tmp — 12.4 KB / 48.0 KB (XMODEM)…` |
//! | 跑完 | `Done: Saved to /tmp — 48.0 KB (XMODEM)` |
//! | 跑完没值 | `Done (no detail): Sent report.pdf` |
//! | 已取消 | `Canceled: Receiving into /tmp — 12.0 KB (XMODEM)`（中性色） |
//! | 失败 | `Failed: report.pdf — 握手阶段失败：…`（错误色） |
//! | 空闲 | 整行**不画**（状态行消失，不常驻一个「Ready」） |
//!
//! 三条容易踩的线：
//!
//! 1. **「在跑」不许长成「跑完」。** 两者的标题可能一模一样，只差明细停在
//!    最后一个值。判据是显式的 [`XferView::running`]（来自 `Progress.finished`
//!    与 `error`），不是「明细非空」——收尾那一帧驱动回报的明细仍是满的。
//! 2. **「跑完没值」不许写成 `Done: xxx — `。** 空尾巴像数据丢了，与
//!    `script.rs` 的 `Done (no value)` 同一个理由。
//! 3. **「已取消」不是「失败」。** 取消是用户主动做的，驱动会把它记成
//!    `error = "传输已取消"`；映射层把它翻成 [`XferView::canceled`]，状态行
//!    因此不解析任何字符串（判「是不是取消」只看这一个布尔）。

use crate::xfer_run::XferProtocol as RunProtocol;
use std::path::Path;
use term_app::action::XferProtocol;

/// 一帧要画的东西。由 `TerminalWindow::xfer_action` 从 `xfer_run::Progress` 映射而来。
pub(super) struct XferView<'a> {
    /// 例如 `Receiving into /tmp` / `Sent report.pdf`。空闲时留空。
    pub title: &'a str,
    /// 进度明细，例如 `12.4 KB / 48.0 KB (XMODEM)`；也可以放阶段名
    /// （`待开始` / `握手`），或者跑完时为空。**不解析**，原样显示。
    pub detail: &'a str,
    /// 失败原因。`Some` 即「这次传输失败了」，此时不看其它字段。
    pub error: Option<&'a str>,
    /// 是否还在跑。
    ///
    /// 这一位**不能**从 `detail` 推：明细非空既可能是跑到一半，也可能是刚跑完
    /// 的最后一帧。它是「在跑」与「跑完」的唯一分界线。
    pub running: bool,
    /// 用户取消。与 `error` 分开是因为取消不是故障——用户按的按钮，不是设备坏了。
    pub canceled: bool,
}

/// 状态行这一帧的交互结果。调用方据此改句柄，**本函数不碰传输**。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum XferUi {
    /// 没有操作（或者这一帧根本没画）。
    None,
    /// 点了「取消」：调用方把 `cancel()` 交给句柄。
    Cancel,
    /// 点了「关闭」：调用方丢掉句柄（结束时丢掉是安全的，见 `XferHandle::Drop`）。
    Dismiss,
}

impl XferView<'_> {
    /// 空闲判定：三个文本字段全空、没在跑、没被取消。
    ///
    /// 注意「跑完没值」**不是**空闲——它有标题，三者合起来才是那条
    /// 「跑完了但没数」的说法。
    pub(super) fn is_idle(&self) -> bool {
        self.title.is_empty()
            && self.detail.is_empty()
            && self.error.is_none()
            && !self.running
            && !self.canceled
    }

    /// 状态行要写的整句话。空闲返回空串（调用方据此不画）。
    pub(super) fn status(&self) -> String {
        if self.is_idle() {
            return String::new();
        }
        if let Some(err) = self.error {
            return if self.title.is_empty() {
                format!("Failed: {err}")
            } else {
                format!("Failed: {} — {err}", self.title)
            };
        }
        // 取消优先于 running：`cancel()` 置了旗子之后线程还要 200ms 才收尾，
        // 这期间不该还在转圈。
        if self.canceled {
            return if self.title.is_empty() {
                "Canceled".to_string()
            } else {
                format!("Canceled: {}", self.title)
            };
        }
        if self.running {
            // 省略号是「还没结束」的唯一标记；明细空的时候也得留着它，
            // 否则一行光秃秃的标题会被读成「跑完了」。
            return if self.detail.is_empty() {
                format!("{}…", self.title)
            } else {
                format!("{} — {}…", self.title, self.detail)
            };
        }
        // 到这儿是「跑完」：明细空要说成「没值」，不能留空尾巴。
        if self.detail.is_empty() {
            return if self.title.is_empty() {
                "Done (no detail)".to_string()
            } else {
                format!("Done (no detail): {}", self.title)
            };
        }
        format!("Done: {} — {}", self.title, self.detail)
    }
}

/// 路径取最后一段做标题。取不到（极端路径）就原样显示整条。
pub(super) fn file_name(path: &str) -> String {
    Path::new(path)
        .file_name()
        .and_then(|name| name.to_str())
        .map(str::to_string)
        .unwrap_or_else(|| path.to_string())
}

/// 字节数 → 人话。1 位小数足够：这一行每 120ms 刷一次，位数越多越抖。
pub(super) fn human_bytes(bytes: u64) -> String {
    const UNITS: [&str; 4] = ["B", "KB", "MB", "GB"];
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit + 1 < UNITS.len() {
        value /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{bytes} B")
    } else {
        format!("{value:.1} {}", UNITS[unit])
    }
}

/// `term-app` 的协议词表 → 驱动层的协议词表。**全仓唯一一处映射**：
/// 两边各自定义枚举是为了不让 UI 依赖协议实现，缝在这里。
pub(super) fn run_protocol(protocol: XferProtocol) -> RunProtocol {
    match protocol {
        XferProtocol::Xmodem => RunProtocol::Xmodem,
        XferProtocol::Xmodem1k => RunProtocol::Xmodem1k,
        XferProtocol::Ymodem => RunProtocol::Ymodem,
        XferProtocol::Zmodem => RunProtocol::Zmodem,
    }
}

/// 剩余时间 → 人话。
///
/// 秒级只在两分钟以内有意义（`~47s` 一眼就懂）；再往上换成 `~3m 20s` / `~2h 05m`，
/// 否则一个 3 GB 的传输会显示 `~7200s`，用户得自己换算。
pub(super) fn human_eta(secs: u64) -> String {
    if secs < 60 {
        return format!("~{secs}s");
    }
    if secs < 3_600 {
        let (m, s) = (secs / 60, secs % 60);
        return if s == 0 {
            format!("~{m}m")
        } else {
            format!("~{m}m {s}s")
        };
    }
    let (h, m) = (secs / 3_600, (secs % 3_600) / 60);
    if m == 0 {
        format!("~{h}h")
    } else {
        format!("~{h}h {m:02}m")
    }
}

/// 进度明细后面的「速率 · 剩余」尾巴。拿不到速率时给**空串**——首帧本来就没有速率，
/// 写个「0 B/s」会让人以为传输没动。
///
/// 有速率没有总长（XMODEM 就是这种）时只给速率、不给剩余时间：分母是编的。
pub(super) fn rate_suffix(bytes_per_sec: Option<u64>, eta_secs: Option<u64>) -> String {
    let Some(bps) = bytes_per_sec else {
        return String::new();
    };
    let mut text = format!(" · {}/s", human_bytes(bps));
    if let Some(secs) = eta_secs {
        text.push_str(" · ");
        text.push_str(&human_eta(secs));
    }
    text
}


#[cfg(test)]
mod tests {
    use super::*;

    fn view<'a>(title: &'a str, detail: &'a str, running: bool) -> XferView<'a> {
        XferView {
            title,
            detail,
            error: None,
            running,
            canceled: false,
        }
    }

    /// 全空的一帧：空闲必须是「什么都不说」。
    fn blank() -> XferView<'static> {
        view("", "", false)
    }

    #[test]
    fn the_status_line_tells_apart_idle_running_done_and_failed() {
        // 空闲：没有传输就什么都不占着——不常驻 "Ready"。
        let idle = blank();
        assert!(idle.is_idle());
        assert_eq!(idle.status(), "");

        // 在跑：收在省略号上，且绝不以 Done 开头。
        let running = view("Receiving into /tmp", "12.4 KB / 48.0 KB (XMODEM)", true);
        assert!(running.status().starts_with("Receiving into /tmp"));
        assert!(
            running.status().ends_with('…'),
            "在跑必须收在省略号上：{}",
            running.status()
        );
        assert!(!running.status().contains("Done"), "{}", running.status());

        // 跑完（有值）：以 Done 开头，且不带省略号。
        let done = view("Saved to /tmp", "48.0 KB (XMODEM)", false);
        assert!(done.status().starts_with("Done"), "{}", done.status());
        assert!(!done.status().ends_with('…'), "{}", done.status());

        // 跑完但没值：说清「没值」，不留空尾巴。
        let done_without_detail = view("Sent report.pdf", "", false);
        assert_eq!(done_without_detail.status(), "Done (no detail): Sent report.pdf");
        assert!(
            !done_without_detail.is_idle(),
            "有标题就是有话说，不能当成空闲"
        );

        // 失败：以 Failed 开头，带原因。
        let failed = XferView {
            error: Some("握手阶段失败：对端无响应"),
            ..view("report.pdf", "", false)
        };
        assert!(failed.status().starts_with("Failed"), "{}", failed.status());
        assert!(failed.status().contains("握手阶段失败"));
    }

    #[test]
    fn a_finished_transfer_never_reads_as_running() {
        // 同一份标题、同一份明细，只差 `running` 这一位——两条状态必须不一样。
        let running = view("Sending report.pdf", "48.0 KB / 48.0 KB (YMODEM)", true);
        let done = view("Sent report.pdf", "48.0 KB / 48.0 KB (YMODEM)", false);
        assert_ne!(running.status(), done.status());
        assert!(!done.status().ends_with('…'));

        // 反向：跑完没值也不能退化成「在跑」。
        let running_without_detail = view("report.pdf", "", true);
        let done_without_detail = view("report.pdf", "", false);
        assert_ne!(running_without_detail.status(), done_without_detail.status());
        assert!(running_without_detail.status().ends_with('…'));
    }

    #[test]
    fn a_canceled_transfer_reads_as_canceled_not_failed() {
        // 取消是用户按的，不是故障：中性文案、不带 Failed。
        let canceled = XferView {
            canceled: true,
            ..view("Receiving into /tmp", "12.0 KB (XMODEM)", false)
        };
        assert!(canceled.status().starts_with("Canceled"), "{}", canceled.status());
        assert!(!canceled.status().contains("Failed"));
        assert!(!canceled.status().contains("Done"));
        // 即便驱动把原因写成「传输已取消」，只要映射层不塞进 error，就仍是取消。
        assert!(!canceled.is_idle());
    }

    #[test]
    fn only_a_fully_empty_view_counts_as_idle() {
        // 只要有一项在说话，就不是空闲——空闲是「整行不画」，
        // 误判成空闲会让用户完全看不到进度。
        assert!(!view("report.pdf", "", false).is_idle(), "有标题");
        assert!(!view("", "48.0 KB", false).is_idle(), "有明细");
        assert!(!view("", "", true).is_idle(), "在跑");
        let failed = XferView {
            error: Some("超时"),
            ..blank()
        };
        assert!(!failed.is_idle(), "有错误");
        assert!(failed.status().starts_with("Failed"));
        let canceled = XferView {
            canceled: true,
            ..blank()
        };
        assert!(!canceled.is_idle(), "已取消");
    }

    #[test]
    fn byte_counts_are_human_readable_and_never_lie_about_units() {
        assert_eq!(human_bytes(0), "0 B");
        assert_eq!(human_bytes(999), "999 B");
        assert_eq!(human_bytes(1024), "1.0 KB");
        assert_eq!(human_bytes(12_697), "12.4 KB");
        assert_eq!(human_bytes(48 * 1024), "48.0 KB");
        assert_eq!(human_bytes(3 * 1024 * 1024), "3.0 MB");
        assert_eq!(human_bytes(2 * 1024 * 1024 * 1024), "2.0 GB");
        // GB 是最后一档，再大也停在这里（这一行只需要读到「几个 G」）。
        assert_eq!(human_bytes(5 * 1024 * 1024 * 1024), "5.0 GB");
    }

    #[test]
    fn a_send_title_uses_the_last_path_component() {
        assert_eq!(file_name("/tmp/report.pdf"), "report.pdf");
        assert_eq!(file_name("report.pdf"), "report.pdf");
        // 末尾带斜杠的目录：`Path::file_name` 给出最后一段（`/tmp/` → `tmp`）。
        // 取不到任何一段（`/`、空串）才退回原样。
        assert_eq!(file_name("/tmp/"), "tmp");
        assert_eq!(file_name("/"), "/", "根目录没有末段，原样显示");
        assert_eq!(file_name(""), "", "空路径不 panic");
    }

    #[test]
    fn a_remaining_time_reads_as_seconds_minutes_or_hours_by_scale() {
        // 秒级只在两分钟内有意义；再往上换分钟 / 小时，否则「~7200s」得让人自己算。
        assert_eq!(human_eta(0), "~0s");
        assert_eq!(human_eta(47), "~47s");
        assert_eq!(human_eta(59), "~59s");
        assert_eq!(human_eta(60), "~1m");
        assert_eq!(human_eta(200), "~3m 20s");
        assert_eq!(human_eta(3_599), "~59m 59s");
        assert_eq!(human_eta(3_600), "~1h");
        assert_eq!(human_eta(7_200), "~2h");
        // 分钟补零：`~2h 05m` 读起来是一个整体，`~2h 5m` 会像两个数。
        assert_eq!(human_eta(7_500), "~2h 05m");
    }

    #[test]
    fn the_rate_suffix_says_nothing_until_there_is_a_rate() {
        // 首帧没有速率：什么都不加。写「0 B/s」会被读成「传不动了」。
        assert_eq!(rate_suffix(None, None), "");
        assert_eq!(rate_suffix(None, Some(30)), "", "没速率就没有剩余时间");
    }

    #[test]
    fn the_rate_suffix_gives_speed_always_and_remaining_time_only_with_a_total() {
        // 有总长：速率 + 剩余都给。XMODEM 没有总长，只剩速率——分母是编的就不给。
        assert_eq!(
            rate_suffix(Some(1_048_576), Some(41)),
            " · 1.0 MB/s · ~41s"
        );
        assert_eq!(rate_suffix(Some(512), None), " · 512 B/s");
    }
}

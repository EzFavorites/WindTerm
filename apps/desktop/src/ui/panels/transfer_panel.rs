//! Transfer 面板：这次运行里传过 / 正在传的东西各到哪一步。
//!
//! 数据只有一份——`Filer` 里的 `xfer_queue`（`xfer_queue.rs`）。本文件只画，
//! 按钮只收集意图，改队列的动作在 `filer_xfer.rs` 的 `cancel_filer_transfer` /
//! `retry_filer_transfers` 里，与 `create-then-draw` 的老规矩一致。
//!
//! # 隧道不在这里
//!
//! 旧实现把「在跑的隧道规则与端口 + 文件面板开关状态」画在这块面板上，那是替身
//! 内容（模块头原话）。隧道有自己的一块界面：`Terminal.ManageTunnels` →
//! `tunnel_panel.rs`，停 / 重启 / 加规则都在那边。一块面板挂两件互不相干的事，
//! 结果是两件都做不深。
//!
//! # 空态不编数据
//!
//! 没有任务时只写一行「去哪能发起传输」。摆一个永远 0% 的进度条、列几条假任务，
//! 是这类面板最常见也最难被发现的说谎——用户会去点那些行，然后发现点不动。
//!
//! # 每行只给「现在真能做」的按钮
//!
//! 取消给排队与传输中的行；重试只给**还在退避窗口里**的行。已完成的没有取消，
//! 终态失败（重试额度用完）的没有重试——那种情况重新右键那个文件即可，入口本来
//! 就在文件面板里。灰按钮占位是另一种假入口。

use crate::ui::panels::xfer_queue::XferRow;
use crate::ui::*;
use term_xfer::{Direction, JobState};

/// 方向箭头：上传朝外、下载朝内。与文件面板的 `↑ / ↓` 是同一套记号。
fn glyph(direction: Direction) -> &'static str {
    match direction {
        Direction::Upload => "↑",
        Direction::Download => "↓",
    }
}

/// 一行的状态文案。`now` 只用于「还有几秒重试」——队列里的时刻是绝对秒。
fn state_text(row: &XferRow, now: u64) -> String {
    match &row.state {
        JobState::Pending => "排队".to_string(),
        // 分母是入队时记下的 size。0 字节（空文件）时不算百分比——除不出东西，
        // 写一个 100% 反倒像「已经传完了」。
        JobState::Running { transferred } => {
            if row.size > 0 {
                format!(
                    "传输中 {:.0}%",
                    *transferred as f64 / row.size as f64 * 100.0
                )
            } else {
                "传输中".to_string()
            }
        }
        JobState::Done => "完成".to_string(),
        JobState::Cancelled => "已取消".to_string(),
        JobState::Failed { retry_at, .. } => {
            // 服务端原话优先：转述成「失败」等于把用户唯一能拿到的线索擦掉。
            let why = row.error.clone().unwrap_or_else(|| "原因未知".to_string());
            match retry_at {
                Some(at) => format!(
                    "失败：{why}（{} 秒后第 {} 次重试）",
                    at.saturating_sub(now),
                    row.attempts
                ),
                None => format!("失败：{why}"),
            }
        }
    }
}

/// 这一行能不能取消 / 能不能重试。
fn can_cancel(row: &XferRow) -> bool {
    matches!(row.state, JobState::Pending | JobState::Running { .. })
}

fn can_retry(row: &XferRow) -> bool {
    matches!(
        row.state,
        JobState::Failed {
            retry_at: Some(_),
            ..
        }
    )
}

/// 画面板。`PanelId::Transfer` 默认关（`state.rs`），打开才画。
pub(in crate::ui) fn draw_transfer(win: &mut TerminalWindow, ui: &mut egui::Ui) {
    let now = ui.input(|i| i.time).max(0.0) as u64;
    let (done, total) = win.filer_xfer().overall();
    let running = win.filer_xfer().running();
    let len = win.filer_xfer().len();
    let rows = win.filer_xfer().rows();
    // 目录同步是「另一件事」，与队列不共用一张表（设计稿明确不复用
    // `xfer_queue`），但它搬的字节同样是「这次运行在动什么」，所以画在同一块
    // 面板的最上面。取一次、两处用（空态判定与绘制），避免算两遍。
    let sync = win.filer_sync_progress();

    if rows.is_empty() && sync.is_none() {
        // 两行短句而不是一句长话：右带的面板宽度只有 220 点，长句会折行，
        // 折出来的第二行贴着面板下沿被切掉（实测）。`truncate()` 让它绝不折行——
        // 宁可截断，也不要画一半。
        for line in ["暂无传输任务", "右键文件即可上传 / 下载"] {
            ui.add(
                egui::Label::new(egui::RichText::new(line).weak().small())
                    .truncate(),
            );
        }
        return;
    }
    let mut stop_sync = false;
    if let Some(sync) = &sync {
        // 第一行带停止按钮：面板是常驻视图，用户看着这里发现「传错了目录」时
        // 不该先去找文件面板。与执行窗那个 `Stop` 是同一条路（见 `SyncLine`）。
        ui.horizontal(|ui| {
            ui.add(
                egui::Label::new(egui::RichText::new(&sync.headline).small())
                    .wrap_mode(egui::TextWrapMode::Truncate),
            )
            .on_hover_text(&sync.headline);
            if sync.stopping {
                ui.add(egui::Label::new(egui::RichText::new("停止中…").weak().small()));
            } else if ui.small_button("停止").clicked() {
                stop_sync = true;
            }
        });
        // 第二行：字节 / 速率 / 当前文件。三者都可能缺（全是目录的一趟没有字节、
        // 刚开头没有速率），缺就整段不画，不补「0 B/s」这种会读成卡住的占位。
        if !sync.detail.is_empty() {
            ui.add(
                egui::Label::new(egui::RichText::new(&sync.detail).weak().small())
                    .wrap_mode(egui::TextWrapMode::Truncate),
            )
            .on_hover_text(&sync.detail);
        }
    }
    if stop_sync {
        win.stop_filer_sync();
    }
    if rows.is_empty() {
        // 只有同步在跑、队列空着：上面那两行已经把话说完了，不必再补一行空态。
        return;
    }

    // ── 顶部一行总进度 ──
    //
    // 只有文字，不用 `ProgressBar` 控件：这块面板在右带里宽 220 点，控件画出来
    // 是一个裹着「1%」的小药丸，既看不出进度也白占十来个像素（实测）。百分比
    // 与字节数写在一行里，读数比一条细线准。
    //
    // 也不在这里加「有失败项在退避」之类的小结：那一行本来就说清了，多加一句
    // 会把这一行撑到截断（实测「条」字被切掉）。谁在等重试，看下面那一行。
    let summary = if total > 0 {
        // 220 点宽放不下「58.6 KB / 4.7 MB · 1 传输中 / 3 条」这种带空格的写法
        // （实测末尾的「条」被截掉）。压掉两侧空格，一行能放全。
        format!(
            "{:.0}% · {}/{} · {}/{} 传输中",
            done as f64 / total as f64 * 100.0,
            term_sftp::format_size(done),
            term_sftp::format_size(total),
            running,
            len
        )
    } else {
        // 全是 0 字节文件：分母是 0，百分比无从算起，编一个出来就是撒谎。
        format!("{len} 条 · {running} 传输中")
    };
    ui.add(
        egui::Label::new(egui::RichText::new(summary).small())
            .wrap_mode(egui::TextWrapMode::Truncate),
    );

    // ── 任务列表 ──
    //
    // **这里不再自己包一层 `ScrollArea`**（2026-11-15）。停靠容器已经替每个工具
    // 面板包了一层（`PanelId::scrolls_in_dock`，salt 用面板名，见
    // `layout/dock_paint.rs` / `layout/dock_bottom.rs`），这里再包一层就是两套
    // 滚动状态叠着：
    //
    // - 内层的 `auto_shrink([false, false])` 会把高度撑满外层视口，于是**外层永远
    //   滚不到任何东西**——两层都「在」，真正生效的只有内层；
    // - 结果是「改哪一层的手感都像在改另一层」：调外层 `auto_shrink` 没反应，
    //   调内层又只影响任务列表，总进度那一行永远固定在顶上，看起来像设计如此。
    //
    // 留容器那一层，是因为其余工具面板（Outline / Session / Sysmon…）都是容器
    // 统一滚的：同一列里七个面板一种手感、这一个另一种，用户没法形成预期。
    // 代价是总进度那一行会跟着滚走——面板在右带通常只有两百来点高，滚走一行不算
    // 损失；真要固定它，正确做法是把这一行画进面板 chrome（与 `Sidebar` 的标题带
    // 同一条路），而不是在内容区再套一层滚动。
    ui.spacing_mut().item_spacing.y = 2.0;
    let mut cancel: Option<u64> = None;
    let mut retry = false;
    for row in &rows {
        // 每个任务两行：箭头 + 名字 / 状态 + 按钮。面板只有 220 宽，一行塞不下。
        ui.horizontal(|ui| {
            ui.label(egui::RichText::new(glyph(row.direction)).monospace());
            ui.add(
                egui::Label::new(egui::RichText::new(truncate(&row.name, 18)).monospace())
                    .wrap_mode(egui::TextWrapMode::Truncate),
            )
            .on_hover_text(row.remote.as_str());
        });
        let state = state_text(row, now);
        ui.horizontal(|ui| {
            // 逐字符截断而不是交给 `truncate()`：同一行里还要放按钮，
            // 会吃满剩余宽度的标签会把按钮顶出面板（实测）。
            ui.add(
                egui::Label::new(egui::RichText::new(truncate(&state, 16)).weak().small())
                    .wrap_mode(egui::TextWrapMode::Truncate),
            )
            .on_hover_text(&state);
            if can_cancel(row) && ui.small_button("取消").clicked() {
                cancel = Some(row.id);
            }
            if can_retry(row) && ui.small_button("重试").clicked() {
                retry = true;
            }
        });
    }

    // ── 动作落地 ──
    if let Some(id) = cancel {
        win.cancel_filer_transfer(id);
    }
    if retry {
        win.retry_filer_transfers();
    }
}

/// 按字符截断，超长补省略号。与 `tools.rs::snippet` 同一套做法，只是这里
/// 还需要「不切坏中文」——`chars()` 天然做到。
fn truncate(text: &str, max: usize) -> String {
    let mut chars = text.chars();
    let mut out: String = chars.by_ref().take(max).collect();
    if chars.next().is_some() {
        out.push('…');
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(state: JobState, size: u64) -> XferRow {
        XferRow {
            id: 1,
            direction: Direction::Download,
            remote: "/srv/www/a.txt".to_string(),
            name: "a.txt".to_string(),
            size,
            state,
            attempts: 0,
            error: None,
        }
    }

    #[test]
    fn zero_byte_rows_do_not_invent_a_percentage() {
        // 空文件：分母是 0，写 100% 会被读成「已经传完了」。
        let text = state_text(&row(JobState::Running { transferred: 0 }, 0), 0);
        assert_eq!(text, "传输中");
        let text = state_text(&row(JobState::Running { transferred: 512 }, 1024), 0);
        assert_eq!(text, "传输中 50%");
    }

    #[test]
    fn a_retryable_failure_says_how_long_and_which_attempt() {
        let mut r = row(
            JobState::Failed {
                reason: term_xfer::FailReason::Rejected,
                retry_at: Some(110),
            },
            10,
        );
        r.attempts = 2;
        r.error = Some("Permission denied".to_string());
        let text = state_text(&r, 100);
        assert!(text.contains("Permission denied"), "原话要留着：{text}");
        assert!(text.contains("10 秒后"), "还剩几秒要说清：{text}");
        assert!(text.contains("第 2 次"), "第几次重试要说清：{text}");
    }

    #[test]
    fn a_terminal_failure_shows_no_countdown() {
        let mut r = row(
            JobState::Failed {
                reason: term_xfer::FailReason::Rejected,
                retry_at: None,
            },
            10,
        );
        r.error = Some("No space left".to_string());
        assert_eq!(state_text(&r, 100), "失败：No space left");
        // 终态失败没有「重试」按钮：额度用完了，再传请重新右键那个文件。
        assert!(!can_retry(&r));
        assert!(!can_cancel(&r));
    }

    #[test]
    fn only_live_rows_can_be_cancelled() {
        assert!(can_cancel(&row(JobState::Pending, 1)));
        assert!(can_cancel(&row(JobState::Running { transferred: 0 }, 1)));
        assert!(!can_cancel(&row(JobState::Done, 1)));
        assert!(!can_cancel(&row(JobState::Cancelled, 1)));
    }

    #[test]
    fn truncation_counts_characters_not_bytes() {
        assert_eq!(truncate("中文名字", 2), "中文…");
        assert_eq!(truncate("short", 8), "short");
    }
}

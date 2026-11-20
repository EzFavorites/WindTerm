//! 六个工具面板 + 2026-11-08 新增的 Sysmon。
//!
//! 教训是「面板开关不能是空转 bool」——Filer 之所以能接，是因为它背后
//! 真有一条 SFTP 连接。这六个也一样：每个开关开着就画一个**有真实数据**的面板，
//! 数据全部每帧从事实状态现算，不在投影层存第二份。
//!
//! - Explorer：会话分组树（与 sidebar 同一份 `sessions` / `groups`）
//! - Outline：当前事实窗口的非空行摘要（与渲染、查找同一个 `tab.view`）
//! - Sender：**不在这里**——工具行 + 多行输入区自成一份，见 `tools_sender.rs`
//! - Session：会话列表，可选中并连接（复用 `connect_selected_session`）
//! - Shell：**一块完整终端**——一条特殊的 session（默认系统本地 shell），
//!   渲染与交互与主区标签页完全同一份代码（见 [`draw_shell`]）
//! - Transfer：**不在这里**——真队列在 `transfer_panel.rs`（2026-11-10 换掉替身）
//! - Sysmon：兼容终端形态的 `T/C/M` 切换头 + `time / system / mem` 读数条（默认关；
//!   采样在 `ui/` 之外的 `sysmon_sample`，这里只读纯值）
//!
//! 默认全关，且专注模式压掉全部面板；新版默认布局里 Explorer / Outline /
//! Sender / Shell 四块由 `ViewState::default()` 打开。

use crate::ui::*;
use eframe::egui;

/// 摘要截断按字符数走，不切坏 UTF-8；超长补一个省略号。
fn snippet(s: &str, max: usize) -> String {
    let mut chars = s.chars();
    let mut out: String = chars.by_ref().take(max).collect();
    if chars.next().is_some() {
        out.push('…');
    }
    out
}

impl TerminalWindow {
    /// 把一行命令经终端编码写给 PTY（工作台的地址栏 / 历史补全也复用它）。
    pub(in crate::ui) fn send_snippet(&mut self, cmd: &str) {
        let Some(tab) = self.tabs.get(self.active_slot()) else {
            return;
        };
        let bytes = paste_bytes_for(&format!("{cmd}\n"), tab.snapshot.bracketed_paste);
        tab.write(bytes.as_bytes());
    }
}

/// Explorer：当前打开的**标签列表**。
///
/// 视觉基线里这一块只有一行 `# zsh`（选中态深灰条），不是分组统计也不是会话树；
/// 会话树在右带的 `Session` 面板，左带这一块列的是「开着的窗格」。点一行切活动
/// 标签，Filter 按名字过滤——两者都是真效果，不是摆设。
pub(in crate::ui) fn draw_explorer(win: &mut TerminalWindow, ui: &mut egui::Ui) {
    let s = &term_core::theme::CHROME_SCALE;
    let text_col = crate::theme::chrome(s.text);
    let dim = text_col.gamma_multiply(0.62);
    let filter = win.panel_filter(term_app::dock::PanelId::Explorer).to_string();
    let names: Vec<(usize, String)> = win
        .tabs
        .iter()
        .enumerate()
        .filter(|(_, t)| {
            filter.is_empty() || t.config.name.to_lowercase().contains(&filter.to_lowercase())
        })
        .map(|(i, t)| (i, t.config.name.clone()))
        .collect();
    let mut activate: Option<usize> = None;
    ui.spacing_mut().item_spacing.y = 0.0;
    for (i, name) in names {
        let (rect, resp) = ui.allocate_exact_size(
            egui::vec2(ui.available_width(), 24.0),
            egui::Sense::click(),
        );
        let p = ui.painter();
        if i == win.active_slot() {
            p.rect_filled(rect, 0.0, crate::theme::chrome(s.selected));
        } else if resp.hovered() {
            p.rect_filled(rect, 0.0, crate::theme::chrome(s.hover));
        }
        p.text(
            egui::pos2(rect.left() + 10.0, rect.center().y),
            egui::Align2::LEFT_CENTER,
            "#",
            crate::ui::type_scale::body(),
            dim,
        );
        p.text(
            egui::pos2(rect.left() + 24.0, rect.center().y),
            egui::Align2::LEFT_CENTER,
            name,
            crate::ui::type_scale::body(),
            text_col,
        );
        if resp.clicked() {
            activate = Some(i);
        }
    }
    if let Some(i) = activate {
        win.set_active_slot(i);
    }
}

/// Outline：只列「看起来像标题」的行（`# ` 开头）。
///
/// 旧实现把终端里每一条非空行都塞进这块面板，右带下半于是变成一片终端文本
/// ——视觉基线里它是**空的**（只有 Filter 与 ⋮）。大纲不是终端正文的副本；没有
/// 结构条目就留空。Filter 有词却一条都匹配不上的时候给一行弱提示，免得被读成
/// 「面板坏了」。
pub(in crate::ui) fn draw_outline(win: &mut TerminalWindow, ui: &mut egui::Ui) {
    let filter = win.panel_filter(term_app::dock::PanelId::Outline).to_string();
    let Some(tab) = win.tabs.get(win.active_slot()) else {
        return;
    };
    let items: Vec<(i64, String)> = tab
        .view
        .lines
        .iter()
        .filter_map(|line| {
            let trimmed = line.text().trim().to_string();
            if !trimmed.starts_with("# ") {
                return None;
            }
            if !filter.is_empty()
                && !trimmed.to_lowercase().contains(&filter.to_lowercase())
            {
                return None;
            }
            Some((line.row, snippet(&trimmed, 48)))
        })
        .collect();
    if items.is_empty() {
        if !filter.is_empty() {
            ui.label(
                egui::RichText::new(t(win.view.lang, "No outline items"))
                    .weak()
                    .small(),
            );
        }
        return;
    }
    for (row, text) in items {
        ui.label(format!("{row}  {text}"));
    }
}

pub(in crate::ui) fn draw_session(win: &mut TerminalWindow, ui: &mut egui::Ui) {
    ui.label("选中会话后可连接到新标签");
    ui.separator();
    let mut selected = None;
    let mut connect = false;
    for (i, s) in win.sessions.iter().enumerate() {
        ui.horizontal(|ui| {
            if ui
                .selectable_label(i == win.selected_session, &s.name)
                .clicked()
            {
                selected = Some(i);
            }
            if ui.small_button("连接").clicked() {
                selected = Some(i);
                connect = true;
            }
        });
    }
    if let Some(i) = selected {
        win.selected_session = i;
    }
    if connect {
        win.connect_selected_session();
    }
}

// Shell 面板**不在这里**：它已经变成「面板内多标签 + 与主区同一份终端」，
// 见 `panels/shell_panel.rs`。原先这块是 `draw_shell`——一份独立实现：自己
// `pool.open` 起会话、`layout_no_wrap` 把整屏文本画一遍、手画一个光标块、
// 键盘只认六个键。于是「面板里能用什么」与「标签页里能用什么」有两份答案
// （面板里没有选区、`Cmd+C`、滚轮回看、鼠标报告、hex、折叠、查找高亮、粘贴
// 确认、margin、右键菜单）。2026-11-15 用户定案：面板保留，内容与主区 `term`
// 一致，且**面板支持多标签**——那些代码全部搬走，这里不再有第二套终端。

/// Sysmon：视觉基线里终端区右下角那枚折叠标签展开后的内容。
///
/// 兼容终端是两行紧凑条（用户比对 2026-11-19）：上行 `T - C - M -` 切换头，下行
/// `[T] HH:MM  [C] NN.N%  [M] A.AA/B.BB GiB` 读数。颜色逐像素取自兼容终端截图：
/// 条底 `#333333`（= `CHROME_SCALE.tab`）、时间 `#BBBBBB`（= `CHROME_SCALE.text`）、
/// system 绿 `#009E00`、mem 红 `#FF0000`。
///
/// 三个字母方块是**真开关**（点一下藏/显对应读数，`sysmon_sample` 的原子位），
/// 中间的 `-` 只是静态分隔符。读数来自宿主层的后台采样（ui/ 不碰 OS 资源），
/// 采样失败时画 `—`，不编假数字。
const SYSMON_ROW_H: f32 = 25.0;
const SYSMON_CHIP: f32 = 16.0;
const SYSMON_PAD_L: f32 = 11.0;
const SYSMON_GAP: f32 = 10.0;
/// 读数条在 12pt 字号下的自然宽度（视觉基线逐像素：条带 531px @2x ≈ 265pt，
/// 取 320 留出三段文字 + chip + 间隙的余量）。面板窄于此值时整条等比缩小，
/// 保证 `23.00/23.63 GiB` 永不被裁；宽于此值就是视觉基线的 1:1 尺寸。
const SYSMON_DESIGN_W: f32 = 320.0;

/// 视觉基线的绿 / 红是兼容终端读数条状态色，不是主题色——主题里没有这两个语义槽，
/// 故只在这一处固化（逐像素 `#009E00` / `#FF0000`）。
fn sysmon_system_colour() -> egui::Color32 {
    egui::Color32::from_rgb(0x00, 0x9E, 0x00)
}
fn sysmon_mem_colour() -> egui::Color32 {
    egui::Color32::from_rgb(0xFF, 0x00, 0x00)
}

/// `used/total GiB`，两位小数，与视觉基线 `23.00/23.63 GiB` 同口径。
fn sysmon_mem_text(used_kb: u64, total_kb: u64) -> String {
    if total_kb == 0 {
        return "—/— GiB".to_string();
    }
    const KIB_PER_GIB: f64 = 1024.0 * 1024.0;
    format!(
        "{:.2}/{:.2} GiB",
        used_kb as f64 / KIB_PER_GIB,
        total_kb as f64 / KIB_PER_GIB
    )
}

/// 一枚字母方块（浅底深字 / 绿底深字 / 红底深字）。
fn sysmon_chip(
    painter: &egui::Painter,
    centre: egui::Pos2,
    label: &str,
    bg: egui::Color32,
    fg: egui::Color32,
    font: &egui::FontId,
    size: f32,
) {
    let rect = egui::Rect::from_center_size(centre, egui::vec2(size, size));
    painter.rect_filled(rect, 2.0, bg);
    painter.text(rect.center(), egui::Align2::CENTER_CENTER, label, font.clone(), fg);
}

pub(in crate::ui) fn draw_sysmon(_win: &TerminalWindow, ui: &mut egui::Ui) {
    let scale = &term_core::theme::CHROME_SCALE;
    let strip = crate::theme::chrome(scale.tab); // #333333
    let light = crate::theme::chrome(scale.text); // #BBBBBB
    let dark = crate::theme::chrome(scale.line); // 方块里的深色字
    let button = crate::theme::chrome(scale.button); // 关掉的那列退到底色
    // 夹具出图钉死视觉基线读数（与 `status_bar::FIXTURE_CLOCK` / `demo::DEMO_CLOCK_BASE`
    // 同一惯例）：墙钟与整机占用每台机器每条采样都不同，逐字节基线会永远红。
    // 生产路径始终是宿主后台线程的真实采样，这一支只在 `--screenshot` 下走。
    let reading = if crate::ui::harness::fixture_mode() {
        crate::sysmon_sample::Reading {
            hhmm: "21:49".to_string(),
            cpu_pct: 11.9,
            // 23.00 / 23.63 GiB（视觉基线逐字值）。
            mem_used_kb: 24_117_248,
            mem_total_kb: 24_776_704,
        }
    } else {
        crate::sysmon_sample::reading()
    };
    // 右带是「一条 SidePanel 管多个面板」，宽度按锚点（Sidebar 200pt）算，
    // 不给 Sysmon 单独加宽。整条按面板宽度等比缩放：宽面板 = 视觉基线 1:1，
    // 窄面板整体缩小（字号 / chip / 间隙一起缩），`23.00/23.63 GiB` 永不裁。
    let width = ui.available_width().max(160.0);
    let k = (width / SYSMON_DESIGN_W).min(1.0);
    let row_h = SYSMON_ROW_H * k;
    let chip = SYSMON_CHIP * k;
    let pad = SYSMON_PAD_L * k;
    let gap = SYSMON_GAP * k;
    let font = egui::FontId::monospace(12.0 * k);

    // 上行：`T - C - M -`。每个字母方块是真开关，`-` 是静态分隔符。
    let head = ui
        .allocate_exact_size(egui::vec2(width, row_h), egui::Sense::hover())
        .0;
    let p = ui.painter();
    p.rect_filled(head, 2.0, strip);
    let mut x = head.left() + pad;
    let cy = head.center().y;
    for (bit, label) in [
        (crate::sysmon_sample::COL_TIME, "T"),
        (crate::sysmon_sample::COL_SYSTEM, "C"),
        (crate::sysmon_sample::COL_MEM, "M"),
    ] {
        let on = crate::sysmon_sample::column_visible(bit);
        let centre = egui::pos2(x + chip * 0.5, cy);
        sysmon_chip(p, centre, label, if on { light } else { button }, if on { dark } else { light }, &font, chip);
        let chip_rect = egui::Rect::from_center_size(centre, egui::vec2(chip, chip));
        if ui
            .interact(chip_rect, egui::Id::new(("sysmon_col", bit)), egui::Sense::click())
            .clicked()
        {
            crate::sysmon_sample::toggle_column(bit);
        }
        x += chip + 6.0 * k;
        p.hline(
            egui::Rangef::new(x, x + 8.0 * k),
            cy,
            egui::Stroke::new(1.5, light),
        );
        x += 8.0 * k + gap;
    }

    // 下行：`[T] 21:49  [C] 11.9%  [M] 23.00/23.63 GiB`。
    ui.add_space(2.0 * k);
    let body = ui
        .allocate_exact_size(egui::vec2(width, row_h), egui::Sense::hover())
        .0;
    // `ui` 刚被上面的 `add_space` / `allocate` 可变借过，painter 必须重新取，
    // 不能续用上行那只（E0502）。
    let p = ui.painter();
    p.rect_filled(body, 2.0, strip);
    let mut x = body.left() + pad;
    let cy = body.center().y;
    let mut draw_seg = |bit: u8, label: &str, text: String, colour: egui::Color32| {
        if !crate::sysmon_sample::column_visible(bit) {
            return;
        }
        let centre = egui::pos2(x + chip * 0.5, cy);
        sysmon_chip(p, centre, label, colour, dark, &font, chip);
        x += chip + 4.0 * k;
        p.text(egui::pos2(x, cy), egui::Align2::LEFT_CENTER, &text, font.clone(), colour);
        x += p.layout_no_wrap(text.clone(), font.clone(), colour).size().x + gap;
    };
    let time = if reading.hhmm.is_empty() {
        "—".to_string()
    } else {
        reading.hhmm
    };
    draw_seg(crate::sysmon_sample::COL_TIME, "T", time, light);
    draw_seg(
        crate::sysmon_sample::COL_SYSTEM,
        "C",
        term_session::sysmon::fmt_pct(reading.cpu_pct),
        sysmon_system_colour(),
    );
    draw_seg(
        crate::sysmon_sample::COL_MEM,
        "M",
        sysmon_mem_text(reading.mem_used_kb, reading.mem_total_kb),
        sysmon_mem_colour(),
    );
}

// Transfer 面板**不在这里**：它画的是传输队列（`transfer_panel.rs`）。
//
// 这块面板曾经是本文件的 `draw_transfer`，内容却是「在跑的隧道规则 + 文件面板
// 开关状态」——隧道在 `Terminal.ManageTunnels`（`tunnel_panel.rs`）里有自己
// 的界面，同一份事实画两遍只会两遍都做不深。2026-11-10 换成真队列后搬走。

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn snippet_keeps_short_text_untouched() {
        assert_eq!(snippet("ls -la", 48), "ls -la");
    }

    #[test]
    fn snippet_truncates_by_chars_and_marks_the_cut() {
        let out = snippet("中文abcdef", 4);
        assert_eq!(out, "中文ab…");
    }

    #[test]
    fn snippet_never_splits_a_multibyte_char() {
        // 按字符截断，第三个汉字不会留下半个 UTF-8 字节。
        assert_eq!(snippet("中文汉", 2), "中文…");
    }
}

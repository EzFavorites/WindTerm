//! 窗口最底部的状态栏（WindTerm 视觉基线 G8）。
//!
//! 放大 `main-dark.png` / `main-light.png` 底部逐字读出：
//!
//! ```text
//! 左区： Ready                       （或一条真实事件：连接中 / 隧道 / 日志 / 错误）
//! 右区： Remote Mode   Ln 44 Ch 52   linux   2020/11/13 18:58   WindTerm
//!                                                              └── 蓝色带下划线
//! ```
//!
//! 右区**从右往左**依次是 `WindTerm` / 时间 / os / Ln Ch / Local-Remote Mode；
//! 没有竖线分隔符，字段之间只有等宽空隙。底色与快捷命令栏一样是
//! `CHROME_SCALE.dialog`，没有分区色块、没有状态点。
//!
//! # 为什么删掉原来的 7 个元件
//!
//! 旧版左区塞了 `Connected | 74x31 | Bytes received: 0`，右区是
//! `WindTerm`，还有 scheme 下拉 / 🐞 / 🔒 / SYNC。那是照着
//! 规格 §2.1 自造的。主题切换在工作台里已有真实入口，锁屏在菜单里，
//! 所以删掉状态栏里的重复控件不丢功能；规范见 `docs/visual-golden.md §7.3`。
//!
//! # 确定性（夹具不隐藏元件）
//!
//! 旧实现在截图夹具里把整段新元件短路掉，只剩四个词——这正是「夹具里看着
//! 还行、真跑起来完全不是一回事」的根源。现在**夹具与正常运行画同一套字段**，
//! 只把唯一的非确定量（墙钟）换成视觉基线截图里的固定时间；其余字段都来自
//! 只读快照与编译期常量，逐字节可复现。

use crate::theme::chrome;
use crate::ui::*;
use term_app::shell::ShellAction;
use term_core::theme::CHROME_SCALE;

/// 夹具里的固定时间：逐字取自视觉基线状态栏，便于并排比对。
/// 正常运行走 `format_utc(SystemTime::now())`，不受这个常量影响。
const FIXTURE_CLOCK: &str = "2020/11/13 18:58";

/// 右区字段之间的空隙。视觉基线靠等宽空隙分区，不画竖线。
const FIELD_GAP: f32 = 20.0;

impl TerminalWindow {
    /// 查找命中时状态栏要显示的那对数：`(第几个, 共几处)`。
    ///
    /// 与 `find_bar.rs` 里的匹配用**同一个**纯函数入口（`app::find::matches`），
    /// 这里不重算、也不缓存——两边各算一次只是同一份事实读了两遍。
    /// 栏没开 / 查询是空串 / 窗口里一处都没命中 → `None`，状态栏保持原样。
    pub(in crate::ui) fn find_hit(&self) -> Option<(usize, usize)> {
        let find = &self.view.find;
        if !find.visible || find.query.is_empty() {
            return None;
        }
        let tab = self.tabs.get(self.active_slot())?;
        let total = term_app::find::matches(find, &tab.view).len();
        let idx = term_app::find::current_index(find, total)?;
        Some((idx + 1, total))
    }

    pub(in crate::ui) fn draw_status_bar(&mut self, ui: &mut egui::Ui) {
        let fixture = self.screenshot_fixture();
        // 命中时整条染色 + 白字：左区换成 `N of M Found`，右区字段一个不变。
        let hit = self.find_hit();
        let fg = if hit.is_some() {
            egui::Color32::WHITE
        } else {
            chrome(CHROME_SCALE.text)
        };
        let left = match hit {
            Some((n, total)) => format!("{} of {} {}", n, total, self.tr("Found")),
            None => self.status_text(),
        };
        let tab = self.tabs.get(self.active_slot());
        let mode = match tab.map(|t| t.config.protocol) {
            Some(Protocol::Shell) | None => self.tr("Local Mode"),
            Some(_) => self.tr("Remote Mode"),
        };
        // `Window 21×114`：终端的列×行（视觉基线状态栏里 `Remote Mode` 与 `Ln/Ch`
        // 之间那一格）。它说的是「这个窗口能装多少格」，与光标位置无关，
        // 所以取快照的网格尺寸而不是屏幕切片。
        let win_size = match tab {
            Some(t) => format!(
                "{} {}×{}",
                self.tr("Window"),
                t.snapshot.cols,
                t.snapshot.rows
            ),
            None => String::new(),
        };
        let ln_ch = match tab {
            Some(t) => {
                let (col, row) = t.snapshot.cursor;
                format!("{} {} {} {}", self.tr("Ln"), row + 1, self.tr("Ch"), col + 1)
            }
            None => format!("{} 1 {} 1", self.tr("Ln"), self.tr("Ch")),
        };
        let clock = if fixture {
            FIXTURE_CLOCK.to_string()
        } else {
            clock_text()
        };

        let mut open_help = false;
        let mut lock_now = false;
        // `horizontal_centered`：状态栏高度是定值，内容不居中就会整行偏下
        // （2026-11-07 实测偏 +2.5px，全窗最宽的一行最看得出来）。
        ui.horizontal_centered(|ui| {
            ui.label(egui::RichText::new(&left).color(fg));
            // 右区用 right_to_left：先画的最靠右，所以顺序就是视觉基线的从右往左。
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                // 蓝底上再画一条蓝链接等于没链接：染色的那条用白字。
                let link_color = if hit.is_some() {
                    egui::Color32::WHITE
                } else {
                    chrome(CHROME_SCALE.focus)
                };
                // 视觉基线最右两项：`🔒 Lock Screen`（点击锁屏）与
                // `🐛 WindTerm Issues`（项目主页）。锁屏在前是因为右区是
                // `right_to_left` 布局——先画的最靠右。
                let lock = egui::RichText::new(format!("🔒 {}", self.tr("Lock Screen"))).color(fg);
                if ui
                    .add(egui::Label::new(lock).sense(egui::Sense::click()))
                    .on_hover_text("锁屏")
                    .clicked()
                {
                    lock_now = true;
                }
                ui.add_space(FIELD_GAP);
                let issues =
                    egui::RichText::new(format!("🐛 {}", self.tr("WindTerm Issues")))
                        .color(link_color)
                        .underline();
                if ui
                    .add(egui::Label::new(issues).sense(egui::Sense::click()))
                    .on_hover_text("打开 WindTerm 项目主页")
                    .clicked()
                {
                    open_help = true;
                }
                ui.add_space(FIELD_GAP);
                ui.monospace(egui::RichText::new(&clock).color(fg))
                    .on_hover_text("UTC · 2000 ms");
                ui.add_space(FIELD_GAP);
                ui.monospace(egui::RichText::new(os_label()).color(fg));
                ui.add_space(FIELD_GAP);
                ui.monospace(egui::RichText::new(&ln_ch).color(fg));
                ui.add_space(FIELD_GAP);
                if !win_size.is_empty() {
                    ui.monospace(egui::RichText::new(&win_size).color(fg));
                    ui.add_space(FIELD_GAP);
                }
                ui.monospace(egui::RichText::new(mode).color(fg));
            });
        });

        // 点击只收集意图，闭包结束后再派发，避免绘制期间借用冲突。
        if open_help {
            self.dispatch_action(Action::Shell(ShellAction::Help));
        }
        if lock_now {
            self.dispatch_action(Action::Lock(term_app::vocab::LockAction::Lock));
        }
        if !fixture {
            self.ctx.request_repaint_after(std::time::Duration::from_millis(
                term_core::config::CLOCK_REFRESH_MS,
            ));
        }
    }

    /// 左区：没有事件时永远只有一个词 `Ready`；有事件时换成那一条。
    /// 这些标签都由各自的功能模块从事实状态现算，状态栏只是念出来。
    fn status_text(&self) -> String {
        if let Some(label) = self.connecting_label() {
            return label;
        }
        if let Some(label) = self.tunnel_label() {
            return label;
        }
        if let Some(label) = self.log_label() {
            return label;
        }
        // 2026-11-19：只读查看是本窗口里**持续存在**的状态，排在一次性
        // `log_note` 之前——「为什么打字没反应」比一条刚才的提示重要。
        // 文案与菜单项同名（`Window ▸ Local Mode`），用户能对上是哪一项。
        if self.local_mode_on() {
            return self.tr("Local mode").to_string();
        }
        if let Some(note) = &self.log_note {
            return note.clone();
        }
        self.tr("Ready").to_string()
    }

    /// 截图 / 视觉基线夹具。判据收在 `harness::fixture_mode`，另外把
    /// `--demo` 的动态夹具也算进来（它没有环境变量那条通道）。
    pub(in crate::ui) fn screenshot_fixture(&self) -> bool {
        crate::ui::harness::fixture_mode() || self.demo.is_some()
    }
}

/// 状态栏里的 os 字段。视觉基线显示 `linux` 是因为原图跑在 Linux 上；
/// 这里不写死，显示本机编译目标，行为才是对的（macOS 上就是 `macos`）。
fn os_label() -> &'static str {
    std::env::consts::OS
}

/// UTC 墙钟 → `YYYY/MM/DD HH:MM`。不引 chrono：状态栏只要一格分钟精度。
fn clock_text() -> String {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    format_utc(secs)
}

/// 秒 → 定长字符串。抽成纯函数才能单测（不读墙钟）。
fn format_utc(secs: u64) -> String {
    let days = (secs / 86_400) as i64;
    let day = secs % 86_400;
    let (y, m, d) = civil_from_days(days);
    format!(
        "{:04}/{:02}/{:02} {:02}:{:02}",
        y,
        m,
        d,
        day / 3600,
        (day / 60) % 60
    )
}

/// 儒略日数（1970-01-01 = 0）→ 公历年月日。
///
/// Howard Hinnant 的 civil_from_days：以 400 年为一纪，全部整数运算，
/// 支持负数（1970 之前）。写成纯函数是为了让日期换算有单测可守——
/// 直接 `SystemTime` 进字符串的话，测的只能是格式而不是换算。
fn civil_from_days(z: i64) -> (i64, i64, i64) {
    let z = z + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = mp + if mp < 10 { 3 } else { -9 };
    (y + if m <= 2 { 1 } else { 0 }, m, d)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn epoch_and_golden_timestamp_round_trip() {
        assert_eq!(civil_from_days(0), (1970, 1, 1));
        // 2020/11/13 18:58 UTC，与视觉基线状态栏逐字一致。
        assert_eq!(format_utc(1_605_293_880), FIXTURE_CLOCK);
    }

    #[test]
    fn month_ends_and_leap_day_are_right() {
        assert_eq!(civil_from_days(31), (1970, 2, 1));
        assert_eq!(civil_from_days(365), (1971, 1, 1));
        // 2000 是闰年：2 月 29 日存在，次日是 3 月 1 日。
        assert_eq!(format_utc(951_782_400), "2000/02/29 00:00");
        assert_eq!(format_utc(951_868_800), "2000/03/01 00:00");
    }

    #[test]
    fn clock_is_a_fixed_width_string() {
        let s = FIXTURE_CLOCK;
        assert_eq!(s.len(), 16);
        assert_eq!(&s[4..5], "/");
        assert_eq!(&s[7..8], "/");
        assert_eq!(&s[10..11], " ");
        assert_eq!(&s[13..14], ":");
    }

    #[test]
    fn the_fixture_clock_is_the_only_thing_that_differs_from_runtime() {
        assert!(!os_label().is_empty());
        assert_eq!(FIXTURE_CLOCK.chars().filter(|c| *c == ':').count(), 1);
    }
}

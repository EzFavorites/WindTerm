//! 每条标签自己的底部工具条（**tab toolbar**）：`Sysmon` 读数与**行过滤器**都在
//! 这同一条上。
//!
//! # 它不是侧边面板，也不是第二条栏
//!
//! `Sysmon`：收起时右端只有一枚**无边框**的文字按钮 `Sysmon`；点它**原地**展开成
//! 一条 T/C/M 读数 `[T] HH:MM [C] NN.N% [M] X.XX/Y.YY GiB`，再点读数条收回按钮。
//! 没有齿轮 / `×` 的面板标题，也没有停靠带里的那块内容——从按钮到读数都是同一条
//! 带上的两种形态，切换只翻一个状态位（`show_pane_sysmon`），所以夹具 `--demo sysmon`
//! 派发一次 `TogglePanel` 就正好定格在展开读数。
//!
//! 行过滤器：左端 `Filter` 按钮把同一条工具条在「T/C/M 读数」与「紧凑过滤行」之间
//! 切换。过滤状态**按会话持有**（`Tab.filter`），不是全局、不在面板里、也不在
//! `ViewState`；`Filter` 按钮与过滤行都只操作本工具条归属的那一条 tab。
//!
//! # 高度恒定
//!
//! 这条工具条**永远是 [`TAB_TOOLBAR_H`]（25pt）**，过滤激活也不抬高。用户
//! 2026-11-27 裁定「过滤行融入 tab，不要贴一块 Find 栏上来」：没有 43pt 档、
//! 没有整框 `TextEdit`，`Filter:` 标签 / 内联输入 / 三个图标 / 计数 / `×` 都在
//! 同一条 25pt 细带里垂直居中。终端区高度因此与过滤开关无关（原来的 18pt
//! 差值已撤销），行号与时间戳仍按 `LineSlice.row` 原样保留。
//!
//! # 位置
//!
//! 只占中央终端区的宽度，声明在左右竖带之后、中央区之前（见 `ui/mod.rs`）。分屏时
//! 每片 pane 自己画一条（`layout/pane_chrome.rs`），并按**自己那条 tab** 的过滤开关
//! 取高度；`ui.interact` 的 id 带 pane 身份，避免多格同名撞 ID。

use crate::ui::*;
use eframe::egui;
use std::sync::{Arc, Mutex, OnceLock};
use term_app::action::Action;
use term_app::dock::PanelId;
use term_app::shell::ShellAction;
use term_core::session::{Protocol, SessionConfig};
use term_core::theme::CHROME_SCALE;
use term_session::sysmon::{fmt_bytes, fmt_uptime};

/// 细带高度（视觉基线 2× 图约 50px → 25pt）。过滤激活也用这一个高度。
pub(in crate::ui) const TAB_TOOLBAR_H: f32 = 25.0;

/// 读数条设计宽度（视觉基线逐像素：条带 531px @2x ≈ 265pt）。
const READOUT_W: f32 = 265.0;
/// 读数条与右窗沿的距离。
pub(in crate::ui) const EDGE_PAD: f32 = 4.0;
/// 字母方块边长。
const CHIP: f32 = 16.0;
/// 条内左内边距。
pub(in crate::ui) const PAD_L: f32 = 9.0;
/// chip → 文字 与 段 → 段 的间隙。
pub(in crate::ui) const GAP: f32 = 8.0;
/// 文字 / chip 用的等宽字号。
const FONT_PT: f32 = 12.0;

/// 零内边距：细带的背景要贴住窗沿，不能被 `Frame` 的全局 margin 推走。
pub(in crate::ui) fn tab_toolbar_frame() -> egui::Frame {
    egui::Frame::none().fill(crate::theme::chrome(CHROME_SCALE.dialog))
}

/// 读数条的绿 / 红是状态色，不是主题色——主题里没有这两个语义槽，
/// 故只在这一处固化（逐像素 `#009E00` / `#FF0000`）。
fn system_colour() -> egui::Color32 {
    egui::Color32::from_rgb(0x00, 0x9E, 0x00)
}
fn mem_colour() -> egui::Color32 {
    egui::Color32::from_rgb(0xFF, 0x00, 0x00)
}

/// `used/total GiB`，两位小数，与视觉基线 `23.00/23.11 GiB` 同口径。
fn mem_text(used_kb: u64, total_kb: u64) -> String {
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

/// 一段读数：`(字母 chip, 文字, 状态色)`。
type Segment = (&'static str, String, egui::Color32);

/// 本帧读数：夹具下钉死，生产走后台采样快照。
fn current_reading() -> crate::sysmon_sample::Reading {
    if crate::ui::harness::fixture_mode() {
        crate::sysmon_sample::Reading {
            hhmm: "21:49".to_string(),
            cpu_pct: 11.9,
            mem_used_kb: 24_117_248,
            mem_total_kb: 24_776_704,
            uptime_secs: 12_345,
            net_rx_bytes: 1_234_567_890,
            net_tx_bytes: 98_765_432,
        }
    } else {
        crate::sysmon_sample::reading()
    }
}

/// 远端读数：夹具里没有真会话，宁可画 `—`，不编假数字。
fn remote_reading() -> Option<crate::sysmon_sample::Reading> {
    if crate::ui::harness::fixture_mode() {
        None
    } else {
        crate::sysmon_remote::reading()
    }
}

/// R 方块的文字。`off` 时用 `—`：这是「没开远端」，不是「远端占用 0%」。
fn remote_text(on: bool, reading: Option<&crate::sysmon_sample::Reading>) -> String {
    if on {
        reading.map_or_else(
            || "—".to_string(),
            |r| term_session::sysmon::fmt_pct(r.cpu_pct),
        )
    } else {
        "—".to_string()
    }
}

/// 三段文字一次性算好：命中区宽度与绘制都读它，避免两帧采样不一致。
fn readout_segments(reading: &crate::sysmon_sample::Reading) -> [Segment; 3] {
    let time = if reading.hhmm.is_empty() {
        "—".to_string()
    } else {
        reading.hhmm.clone()
    };
    [
        ("T", time, crate::theme::chrome(CHROME_SCALE.text)),
        (
            "C",
            term_session::sysmon::fmt_pct(reading.cpu_pct),
            system_colour(),
        ),
        (
            "M",
            mem_text(reading.mem_used_kb, reading.mem_total_kb),
            mem_colour(),
        ),
    ]
}

/// 三段文字实际占位。固定 265pt 只够夹具那组值；CPU 到 `100.0%`、内存到
/// 三位 GiB 时会溢出右窗沿，所以条宽按内容托底（下限仍是 265pt）。
fn segments_width(ui: &egui::Ui, segs: &[Segment]) -> f32 {
    let font = egui::FontId::monospace(FONT_PT);
    let mut w = PAD_L;
    for (_, text, colour) in segs {
        w += CHIP
            + 4.0
            + ui.painter()
                .layout_no_wrap(text.clone(), font.clone(), *colour)
                .size()
                .x
            + GAP;
    }
    w
}

/// 一枚字母方块（底色 + 深字）。
fn chip(
    p: &egui::Painter,
    centre: egui::Pos2,
    label: &str,
    bg: egui::Color32,
    fg: egui::Color32,
    font: &egui::FontId,
) {
    let rect = egui::Rect::from_center_size(centre, egui::vec2(CHIP, CHIP));
    p.rect_filled(rect, 2.0, bg);
    p.text(
        rect.center(),
        egui::Align2::CENTER_CENTER,
        label,
        font.clone(),
        fg,
    );
}

/// 远端源的稳定键：会话身份每次重连都会变，但「同一台机器」不该因此清读数；
/// 反过来 host / user / port 任一变了就是另一台机器，上一台的读数必须丢弃。
fn remote_target_key(config: &SessionConfig) -> String {
    format!("{}@{}:{}", config.username, config.host, config.port)
}

/// 上次采样的远端目标。`None` = 开关关着 / 没有合法目标。进程级状态与
/// `sysmon_remote` 的快照同源：两者都由这条路径统一作废，不留半个真相。
static LAST_TARGET: OnceLock<Mutex<Option<String>>> = OnceLock::new();

fn last_target() -> &'static Mutex<Option<String>> {
    LAST_TARGET.get_or_init(|| Mutex::new(None))
}

/// 目标变化时清掉快照与节流时戳（后者让新目标能立刻采，不用等 5 秒）。
/// 返回是否真的变了；同一台机器每帧调用也不会反复清。
fn set_remote_target(key: Option<&str>) -> bool {
    let changed = match last_target().lock() {
        Ok(mut last) if last.as_deref() != key => {
            *last = key.map(str::to_owned);
            true
        }
        _ => false,
    };
    if changed {
        crate::sysmon_remote::clear();
    }
    changed
}

impl TerminalWindow {
    /// 每帧调一次：开关开着且活动 tab 是**握着凭据的内置 SSH** 时，请求一次
    /// 非阻塞远端采样。节流与「上一轮还在飞」由 `sysmon_remote::request` 自己管，
    /// 这里只负责把 `SessionConfig` + 内存凭据拼成连接参数。
    ///
    /// 系统 `ssh` 子进程没有凭据副本、本地 shell 就是宿主，两者都不发命令，
    /// 远端读数保持 `—`。夹具模式也绝不连网，截图才能逐字节稳定。
    pub(in crate::ui) fn sync_remote_sysmon(&self, ctx: &egui::Context) {
        if !self.view.sysmon_remote {
            set_remote_target(None);
            return;
        }
        let Some(tab) = self.tabs.get(self.active_slot()) else {
            set_remote_target(None);
            return;
        };
        if tab.config.protocol != Protocol::Ssh || !tab.config.builtin_ssh || tab.session.is_none()
        {
            // 换了数据源就不能留上一台机器的数字——那是「看起来有数据」的假读数。
            set_remote_target(None);
            return;
        }
        let Some(auth) = tab.auth.clone() else {
            set_remote_target(None);
            return;
        };
        // 目标一换就清快照 + 节流时戳：否则新机器的头几帧还在显示上一台的数字，
        // 而且 5 秒节流会拖到新目标第一次采样。会话身份会随重连变，故用 host/user/port。
        set_remote_target(Some(&remote_target_key(&tab.config)));
        if crate::ui::harness::fixture_mode() {
            return;
        }
        if crate::sysmon_remote::target_for(&tab.config.system).is_none() {
            set_remote_target(None);
            return;
        }
        // 与连接时同一份参数拼装（ProxyJump / 代理链 / 主机密钥），不另写一套；
        // 会话已经连上，TOFU 允许沿用那次已接受的主机。
        let opts = match crate::ui::dialogs::ssh_ask::options_for(
            &tab.config,
            auth,
            term_ssh::HostKeyPolicy::Tofu,
        ) {
            Ok(opts) => opts,
            Err(_) => {
                set_remote_target(None);
                return;
            }
        };
        let ctx = ctx.clone();
        crate::sysmon_remote::request(
            opts,
            &tab.config.system,
            Arc::new(move || ctx.request_repaint()),
        );
    }

    /// 本条工具条的高度：过滤激活也不变，恒为 [`TAB_TOOLBAR_H`]（25pt）。
    /// 保留这个方法是为了让调用方不必知道「高度恒定」这件事；分屏每片都得到同一个值。
    pub(in crate::ui) fn tab_toolbar_height(
        &self,
        _target_session: Option<term_core::session_runtime::SessionId>,
    ) -> f32 {
        TAB_TOOLBAR_H
    }

    /// 左端 `Filter` 按钮的自然宽（文字 + 左右留白），供过滤行布局共用。
    pub(in crate::ui) fn filter_button_w(&self, ui: &egui::Ui) -> f32 {
        let font = crate::ui::type_scale::body();
        label_width(ui, &self.tr("Filter"), &font) + PAD_L * 2.0
    }

    /// 画这条工具条。`pane_salt`：分屏时每片 pane 一份 `egui::Id`，全窗传 `None`。
    ///
    /// `target_session`：本条工具条归属的会话。**行过滤按会话生效**——分屏时每个
    /// pane 传自己那条会话，左端 `Filter` 按钮与过滤行只操作它；单窗格传当前活动
    /// 会话。
    ///
    /// 过滤行与 Sysmon 读数**共用同一条** `TopBottomPanel`（不新增第二条）；高度恒为
    /// [`TAB_TOOLBAR_H`]（`tab_toolbar_height`），`ui.max_rect()` 就是本帧真实高度。
    ///
    /// `target_session`：本细带归属的会话。**行过滤按会话生效**——分屏时每个 pane
    /// 传自己那条会话，左端 `Filter` 按钮与过滤行只操作它；单窗格传当前活动会话。
    /// 这条细带不另起 `TopBottomPanel`，过滤行与 Sysmon 读数共用同一条 25pt 细带。
    pub(in crate::ui) fn draw_tab_toolbar(
        &mut self,
        ui: &mut egui::Ui,
        pane_salt: Option<u64>,
        target_session: Option<term_core::session_runtime::SessionId>,
    ) {
        let rect = ui.max_rect();
        let expanded = self.panel_visible(PanelId::Sysmon);
        let id = match pane_salt {
            Some(p) => egui::Id::new(("tab_toolbar", p)),
            None => egui::Id::new("tab_toolbar"),
        };

        // 本细带归属的会话：分屏每个 pane 自己那条；单窗格取活动会话。
        let filter_session = target_session.or(self.active);
        let filter_slot =
            filter_session.and_then(|sid| self.tabs.iter().position(|t| t.identity == sid));
        let filter_visible = filter_slot.is_some_and(|slot| self.tabs[slot].filter.visible);

        // ── 左端 `Filter` 按钮：把细带在「T/C/M 读数」与「紧凑过滤行」之间切换 ──
        // id 带 pane_salt：分屏时每个 pane 只翻自己那条会话的过滤位，不撞 ID。
        let filter_label = self.tr("Filter");
        let filter_font = crate::ui::type_scale::body();
        let filter_w = label_width(ui, &filter_label, &filter_font) + PAD_L * 2.0;
        // 左端 `Filter` 与右端 Sysmon 读数**共用同一条**，谁也不能越到对方身上：
        // 读数条右对齐后往左撑开，在窄条里会一路撑到条左沿、正好压住 `Filter`。
        let filter_rect = if filter_visible {
            // 激活态：这枚 25pt 快速按钮就是 Find 式的下划线 `Filter:` 标签本身，
            // 命中区收在输入框左沿（x≈52）之前，与标签同处一行。
            super::filter_label_button_rect(rect)
        } else {
            filter_button_rect(rect, filter_w)
        };
        // Sysmon 那一侧的左边界：永远从 `Filter` 按钮右边起步。
        let sysmon_left = filter_rect.right() + GAP;
        let filter_id = match pane_salt {
            Some(p) => egui::Id::new(("sysmon_filter", p)),
            None => egui::Id::new("sysmon_filter"),
        };
        let filter_resp = ui.interact(filter_rect, filter_id, egui::Sense::click());

        // ── `Filter` 按钮绘制 + 点击翻转本会话的过滤位 ──
        // 先画按钮底色：激活态的字由 `draw_filter_row` 按 Find 的 galley + 下划线
        // 画法出，必须在底色之后落笔，不能被 hover 底盖住。收起态文字这里画。
        let hovered = filter_resp.hovered();
        let p = ui.painter();
        if hovered {
            p.rect_filled(filter_rect, 2.0, crate::theme::chrome(CHROME_SCALE.hover));
        }
        if !filter_visible {
            p.text(
                filter_rect.center(),
                egui::Align2::CENTER_CENTER,
                filter_label,
                filter_font,
                crate::theme::chrome(if hovered {
                    CHROME_SCALE.text_strong
                } else {
                    CHROME_SCALE.text
                }),
            );
        }
        let filter_clicked = filter_resp.clicked();
        if hovered || filter_clicked {
            ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
        }

        // Sysmon 读数仍要能切回去：过滤态下藏起 T/C/M，但 `Filter` 按钮一直在，
        // 点它回到 Sysmon 读数；Sysmon 那个命中区在过滤态下不登记。
        if filter_visible {
            // ── 紧凑过滤行：占用同一条工具条的剩余宽度，不新增任何面板/底栏 ──
            self.draw_filter_row(ui, rect, filter_session, filter_slot, pane_salt);
        } else {
            // 一次采样、一次文本测量：命中区与绘制共用同一份读数与同一条宽。
            // 同 id 一帧只能登记一次 ui.interact（第二次 egui 报 ID 撞车）。
            let reading = current_reading();
            let segs = expanded.then(|| readout_segments(&reading));
            let hit = match &segs {
                Some(segs) => readout_rect_for(ui, rect, sysmon_left, segs),
                None => button_rect(ui, rect, sysmon_left, self.tr("Sysmon")),
            };
            let resp = ui.interact(hit, id, egui::Sense::click());
            // 官网 page/11 的 uptime / network 不挤占视觉基线的三段读数，放悬停详情。
            let resp = if expanded {
                resp.on_hover_text(format!(
                    "Uptime: {}\nNetwork down: {}\nNetwork up: {}",
                    fmt_uptime(reading.uptime_secs),
                    fmt_bytes(reading.net_rx_bytes),
                    fmt_bytes(reading.net_tx_bytes),
                ))
            } else {
                resp
            };

            // 展开态左端多一枚 R 方块：它是监控源开关，有独立的命中区与 ID，
            // 与「点读数条收起细带」互不抢。收起态不画，基线一个像素都不动。
            let remote = expanded.then(remote_reading).flatten();
            let r_text = remote_text(self.view.sysmon_remote, remote.as_ref());
            let r_rect = if expanded {
                Some(remote_chip_rect(ui, sysmon_left, hit, &r_text))
            } else {
                None
            };
            let r_id = match pane_salt {
                Some(p) => egui::Id::new(("sysmon_remote", p)),
                None => egui::Id::new("sysmon_remote"),
            };
            let r_resp = r_rect.map(|r| ui.interact(r, r_id, egui::Sense::click()));
            let r_detail = remote.map_or_else(String::new, |r| {
                format!(
                    "\nRemote uptime: {}\nRemote net: {}/{}",
                    fmt_uptime(r.uptime_secs),
                    fmt_bytes(r.net_rx_bytes),
                    fmt_bytes(r.net_tx_bytes)
                )
            });

            if let Some(segs) = segs {
                self.draw_sysmon_readout(ui, rect, &segs, resp.hovered());
            } else {
                self.draw_sysmon_button(ui, rect, resp.hovered(), self.tr("Sysmon"));
            }
            if let (Some(rr), Some(hover)) = (r_rect, r_resp.as_ref().map(|x| x.hovered())) {
                self.draw_remote_chip(ui, rr, self.view.sysmon_remote, &r_text, hover);
            }

            if resp.hovered() || r_resp.as_ref().is_some_and(|x| x.hovered()) {
                ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
            }
            if resp.clicked() {
                self.dispatch_action(Action::Shell(ShellAction::TogglePanel(PanelId::Sysmon)));
            }
            if r_resp.as_ref().is_some_and(|x| x.clicked()) {
                self.dispatch_action(Action::Shell(ShellAction::ToggleSysmonRemote));
            }
            if let Some(resp) = r_resp {
                resp.on_hover_text(format!(
                    "Remote Sysmon: {}{}",
                    if self.view.sysmon_remote { "on" } else { "off" },
                    r_detail
                ));
            }
        }

        // 点击最后落地（绘制时仍读本帧旧状态，关栏不会先闪一帧空查询）。
        // `Action::Filter` 不写 `ViewState`，对本会话那条 tab 落地（分屏互不串味）。
        if filter_clicked {
            if let Some(session) = filter_session {
                self.apply_filter_on(session, term_app::vocab::FilterAction::Toggle);
            }
        }
    }

    /// 一枚 R 方块 + 远端 CPU 读数，贴在三段读数条左侧。
    fn draw_remote_chip(
        &self,
        ui: &mut egui::Ui,
        rect: egui::Rect,
        on: bool,
        text: &str,
        hovered: bool,
    ) {
        let p = ui.painter();
        p.rect_filled(
            rect,
            2.0,
            crate::theme::chrome(if hovered {
                CHROME_SCALE.hover
            } else {
                CHROME_SCALE.tab
            }),
        );
        let font = egui::FontId::monospace(FONT_PT);
        let dark = crate::theme::chrome(CHROME_SCALE.line);
        let colour = if on {
            system_colour()
        } else {
            crate::theme::chrome(CHROME_SCALE.text)
        };
        chip(
            p,
            egui::pos2(rect.left() + CHIP * 0.5, rect.center().y),
            "R",
            colour,
            dark,
            &font,
        );
        p.text(
            egui::pos2(rect.left() + CHIP + 4.0, rect.center().y),
            egui::Align2::LEFT_CENTER,
            text,
            font,
            colour,
        );
    }

    /// 收起态：右端一枚**无边框**的 `Sysmon` 文字按钮。hover 只出底色
    /// （与全项目一致：蓝底是指向色，不是固有底色）。
    fn draw_sysmon_button(&self, ui: &mut egui::Ui, rect: egui::Rect, hovered: bool, label: &str) {
        let r = button_rect(ui, rect, rect.left(), label);
        let p = ui.painter();
        if hovered {
            p.rect_filled(r, 2.0, crate::theme::chrome(CHROME_SCALE.hover));
        }
        let text = label;
        let font = crate::ui::type_scale::body();
        let col = crate::theme::chrome(if hovered {
            CHROME_SCALE.text_strong
        } else {
            CHROME_SCALE.text
        });
        p.text(r.center(), egui::Align2::CENTER_CENTER, text, font, col);
    }

    /// 展开态：右端一条 `#333333` 读数条，一段段画 `[T] HH:MM [C] NN.N% [M] X.XX/Y.YY GiB`。
    fn draw_sysmon_readout(
        &self,
        ui: &mut egui::Ui,
        rect: egui::Rect,
        segs: &[Segment],
        hovered: bool,
    ) {
        let strip = readout_rect_for(ui, rect, rect.left(), segs);
        let p = ui.painter();
        p.rect_filled(
            strip,
            2.0,
            crate::theme::chrome(if hovered {
                CHROME_SCALE.hover
            } else {
                CHROME_SCALE.tab
            }),
        );

        let dark = crate::theme::chrome(CHROME_SCALE.line);
        let font = egui::FontId::monospace(FONT_PT);
        let mut x = strip.left() + PAD_L;
        let cy = strip.center().y;
        for (label, text, colour) in segs {
            let centre = egui::pos2(x + CHIP * 0.5, cy);
            chip(p, centre, label, *colour, dark, &font);
            x += CHIP + 4.0;
            p.text(
                egui::pos2(x, cy),
                egui::Align2::LEFT_CENTER,
                text,
                font.clone(),
                *colour,
            );
            x += p
                .layout_no_wrap(text.clone(), font.clone(), *colour)
                .size()
                .x
                + GAP;
        }
    }
}

/// `Filter` 按钮的最小命中宽：再窄就不是一枚按钮了（点击都落不进去）。
const MIN_FILTER_W: f32 = 48.0;

/// `Filter` 按钮的自然宽夹到 [MIN_FILTER_W, 条宽一半]。纯函数（不收 `&Ui`），
/// 所以窄条下的命中几何能进单测。
pub(in crate::ui) fn filter_button_width(rect: egui::Rect, filter_w: f32) -> f32 {
    filter_w.min(rect.width() * 0.5).max(MIN_FILTER_W)
}

/// 快速按钮的矩形：**高度恒为 25pt**（与 Sysmon 按钮同高），在工具条里垂直居中。
///
/// 收起态条高正好 25，视觉基线的文字中线不动；过滤态条高 43，按钮不跟着拉长——
/// 它化作 Find 式的下划线 `Filter:` 标签，仍是一枚 25pt 的点击区。
pub(in crate::ui) fn quick_button_rect(rect: egui::Rect, left: f32, width: f32) -> egui::Rect {
    let h = TAB_TOOLBAR_H.min(rect.height());
    let cy = rect.center().y;
    egui::Rect::from_min_max(
        egui::pos2(left, cy - h / 2.0),
        egui::pos2(left + width, cy + h / 2.0),
    )
}

/// `Filter` 按钮的命中矩形：条左端，左内边距 `EDGE_PAD`。
pub(in crate::ui) fn filter_button_rect(rect: egui::Rect, filter_w: f32) -> egui::Rect {
    let w = filter_button_width(rect, filter_w);
    quick_button_rect(rect, rect.left() + EDGE_PAD, w)
}

/// 一段文字的实际宽度（用于排显式命中矩形，不依赖 egui 的居中布局）。
pub(in crate::ui) fn label_width(ui: &egui::Ui, text: &str, font: &egui::FontId) -> f32 {
    ui.painter()
        .layout_no_wrap(text.to_string(), font.clone(), egui::Color32::WHITE)
        .size()
        .x
}

/// 展开态读数条的矩形（右对齐）。`min_left` 是**不许越过**的左边界——通常是
/// `Filter` 按钮的右沿：`Filter` 与读数条在**同一条**工具条上，窄条里读数条往左
/// 撑开会一路压到 `Filter` 上，于是同一块像素有两个命中区（谁都能点到）。夹在
/// 这里之后两者永不重叠；代价是极窄条里读数文字会被裁，而不是被抢点击。
fn readout_rect(rect: egui::Rect, min_left: f32, width: f32) -> egui::Rect {
    let available = (rect.right() - EDGE_PAD - min_left).max(0.0);
    let w = width.min(available).max(8.0);
    egui::Rect::from_min_max(
        egui::pos2(rect.right() - EDGE_PAD - w, rect.top() + 1.0),
        egui::pos2(rect.right() - EDGE_PAD, rect.bottom() - 1.0),
    )
}

/// 内容宽度托底 265pt：夹具那一组值仍画成基线宽度，长读数才把条撑宽。
fn readout_rect_for(
    ui: &egui::Ui,
    rect: egui::Rect,
    min_left: f32,
    segs: &[Segment],
) -> egui::Rect {
    readout_rect(rect, min_left, segments_width(ui, segs).max(READOUT_W))
}

/// R 方块的矩形：贴在读数条左边，宽度随读数文字走；窄条时停在 `min_left`，
/// 绝不越过 `Filter` 按钮。
fn remote_chip_rect(
    ui: &egui::Ui,
    min_left: f32,
    strip: egui::Rect,
    text: &str,
) -> egui::Rect {
    let text_w = label_width(ui, text, &egui::FontId::monospace(FONT_PT));
    let w = CHIP + 4.0 + text_w + GAP;
    let left = (strip.left() - w - 2.0).max(min_left);
    egui::Rect::from_min_max(
        egui::pos2(left, strip.top()),
        egui::pos2(left + w, strip.bottom()),
    )
}

/// 收起态按钮的矩形：按 `Sysmon` 文字自然宽度 + 左右留白，右对齐。
/// `min_left` 同 [`readout_rect`]：不许压到 `Filter` 按钮上。
fn button_rect(ui: &egui::Ui, rect: egui::Rect, min_left: f32, label: &str) -> egui::Rect {
    let text_w = label_width(ui, label, &crate::ui::type_scale::body());
    let w = (text_w + PAD_L * 2.0)
        .min(rect.right() - EDGE_PAD - min_left)
        .max(8.0);
    // 与 `Filter` 快速按钮同一份 25pt 高度 / 垂直居中（见 `quick_button_rect`）。
    quick_button_rect(rect, rect.right() - EDGE_PAD - w, w)
}


#[cfg(test)]
#[path = "tab_toolbar_tests.rs"]
mod tests;

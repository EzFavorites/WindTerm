//! 终端区下沿的 `Sysmon` 细带。
//!
//! # 它不是侧边面板
//!
//! 兼容终端里 `Sysmon` 就在终端区底部这条细带上：收起时右端只有一枚**无边框**
//! 的文字按钮 `Sysmon`；点它**原地**展开成一条 T/C/M 读数
//! `[T] HH:MM  [C] NN.N%  [M] X.XX/Y.YY GiB`，再点读数条收回按钮。
//! 没有齿轮 / `×` 的面板标题，也没有停靠带里的那块内容——从按钮到读数
//! 都是同一条带上的两种形态，切换只翻一个状态位。
//!
//! 之前的实现把按钮做成了带边框的折叠标签、点击去翻 `TogglePanel(PanelId::Sysmon)`
//! 打开右带面板（见旧版注释），与视觉基线不符，改成本文件这套。
//! 状态位仍复用 `show_pane_sysmon`（`ViewState` 里已有的那一位），这样夹具
//! `--demo sysmon` 派发一次 `TogglePanel` 就正好定格在展开读数，无需新动作。
//!
//! # 位置
//!
//! 这条细带只占中央终端区的宽度，声明在左右竖带之后、中央区之前（见
//! `ui/mod.rs`）。分屏时每片 pane 自己画一条（`layout/pane_chrome.rs`），
//! `ui.interact` 的 id 带 pane 身份，避免多格同名撞 ID。

use crate::ui::*;
use eframe::egui;
use std::sync::{Arc, Mutex, OnceLock};
use term_app::action::Action;
use term_app::dock::PanelId;
use term_app::shell::ShellAction;
use term_core::session::{Protocol, SessionConfig};
use term_core::theme::CHROME_SCALE;
use term_session::sysmon::{fmt_bytes, fmt_uptime};

/// 细带高度（视觉基线 2× 图约 50px → 25pt）。
pub(in crate::ui) const SYSMON_BAR_H: f32 = 25.0;

/// 读数条设计宽度（视觉基线逐像素：条带 531px @2x ≈ 265pt）。
const READOUT_W: f32 = 265.0;
/// 读数条与右窗沿的距离。
const EDGE_PAD: f32 = 4.0;
/// 字母方块边长。
const CHIP: f32 = 16.0;
/// 条内左内边距。
const PAD_L: f32 = 9.0;
/// chip → 文字 与 段 → 段 的间隙。
const GAP: f32 = 8.0;
/// 文字 / chip 用的等宽字号。
const FONT_PT: f32 = 12.0;

/// 零内边距：细带的背景要贴住窗沿，不能被 `Frame` 的全局 margin 推走。
pub(in crate::ui) fn sysmon_bar_frame() -> egui::Frame {
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

    /// 画这条细带。`pane_salt`：分屏时每片 pane 一份 `egui::Id`，全窗传 `None`
    /// 保持原来的 `"sysmon_bar"` 一字不变。
    ///
    /// `target_session`：本细带归属的会话。**行过滤按会话生效**——分屏时每个 pane
    /// 传自己那条会话，左端 `Filter` 按钮与过滤行只操作它；单窗格传当前活动会话。
    /// 这条细带不另起 `TopBottomPanel`，过滤行与 Sysmon 读数共用同一条 25pt 细带。
    pub(in crate::ui) fn draw_sysmon_bar(
        &mut self,
        ui: &mut egui::Ui,
        pane_salt: Option<u64>,
        target_session: Option<term_core::session_runtime::SessionId>,
    ) {
        let rect = ui.max_rect();
        let expanded = self.panel_visible(PanelId::Sysmon);
        let id = match pane_salt {
            Some(p) => egui::Id::new(("sysmon_bar", p)),
            None => egui::Id::new("sysmon_bar"),
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
        let filter_w = (ui
            .painter()
            .layout_no_wrap(
                filter_label.to_string(),
                filter_font.clone(),
                egui::Color32::WHITE,
            )
            .size()
            .x
            + PAD_L * 2.0)
            .min(rect.width() * 0.5);
        let filter_rect = egui::Rect::from_min_max(
            egui::pos2(rect.left() + EDGE_PAD, rect.top() + 1.0),
            egui::pos2(rect.left() + EDGE_PAD + filter_w, rect.bottom() - 1.0),
        );
        let filter_id = match pane_salt {
            Some(p) => egui::Id::new(("sysmon_filter", p)),
            None => egui::Id::new("sysmon_filter"),
        };
        let filter_resp = ui.interact(filter_rect, filter_id, egui::Sense::click());

        // Sysmon 读数仍要能切回去：过滤态下藏起 T/C/M，但 `Filter` 按钮一直在，
        // 点它回到 Sysmon 读数；Sysmon 那个命中区在过滤态下不登记。
        if filter_visible {
            // ── 紧凑过滤行：占用细带剩余宽度，不新增任何面板/底栏 ──
            let row_left = filter_rect.right() + GAP;
            let row_rect = egui::Rect::from_min_max(
                egui::pos2(row_left, rect.top() + 1.0),
                egui::pos2(rect.right() - EDGE_PAD, rect.bottom() - 1.0),
            );
            let mut child = ui.new_child(egui::UiBuilder::new().max_rect(row_rect));
            self.draw_filter_row(&mut child, filter_session, filter_slot, pane_salt);
        } else {
            // 一次采样、一次文本测量：命中区与绘制共用同一份读数与同一条宽。
            // 同 id 一帧只能登记一次 ui.interact（第二次 egui 报 ID 撞车）。
            let reading = current_reading();
            let segs = expanded.then(|| readout_segments(&reading));
            let hit = match &segs {
                Some(segs) => readout_rect_for(ui, rect, segs),
                None => button_rect(ui, rect, self.tr("Sysmon")),
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
                Some(remote_chip_rect(ui, rect, hit, &r_text))
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

        // ── `Filter` 按钮绘制 + 点击翻转本会话的过滤位 ──
        let hovered = filter_resp.hovered();
        let p = ui.painter();
        if hovered || filter_visible {
            p.rect_filled(filter_rect, 2.0, crate::theme::chrome(CHROME_SCALE.hover));
        }
        p.text(
            filter_rect.center(),
            egui::Align2::CENTER_CENTER,
            filter_label,
            filter_font,
            crate::theme::chrome(if hovered || filter_visible {
                CHROME_SCALE.text_strong
            } else {
                CHROME_SCALE.text
            }),
        );
        if hovered || filter_resp.clicked() {
            ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
        }
        if filter_resp.clicked() {
            if let Some(session) = filter_session {
                // `Action::Filter` 不写 `ViewState`，对本会话那条 tab 落地
                // （与 `Effect::Filter` 同一个 `filter::reduce`，这里只是 pane 自选会话）。
                self.apply_filter_on(session, term_app::vocab::FilterAction::Toggle);
            }
        }
    }

    /// 紧凑过滤行：`TextEdit` + grep / grep -v + 大小写 + 正则 + 关闭。
    ///
    /// 全部 egui 控件 id 带 `pane_salt`（`ui.push_id` + 显式 id）：分屏时每个 pane
    /// 只读写自己那条会话的 `Tab.filter` / `Tab.filter_input`。受控输入——本函数
    /// 只收集 `FilterAction`，先收集完再统一落地，避免绘制期借用冲突。
    fn draw_filter_row(
        &mut self,
        ui: &mut egui::Ui,
        session: Option<term_core::session_runtime::SessionId>,
        slot: Option<usize>,
        pane_salt: Option<u64>,
    ) {
        let Some(session) = session else { return };
        let Some(slot) = slot else { return };
        let lang = self.view.lang;

        // 过滤状态快照 + 每帧现算的 kept/hidden（不缓存进状态）。
        let (mode, case_sensitive, regex, regex_error) = {
            let tab = &self.tabs[slot];
            let err = if tab.filter.regex && !tab.filter.query.is_empty() {
                term_app::filter::compile(&tab.filter).err()
            } else {
                None
            };
            (
                tab.filter.mode,
                tab.filter.case_sensitive,
                tab.filter.regex,
                err,
            )
        };
        let (kept, hidden) = {
            let tab = &self.tabs[slot];
            term_app::filter::counts(&tab.filter, &tab.view)
        };

        let mut pending: Vec<term_app::vocab::FilterAction> = Vec::new();
        let salt = pane_salt;

        ui.push_id(("sysmon_filter_row", salt), |ui| {
            ui.horizontal_centered(|ui| {
                ui.spacing_mut().item_spacing.x = 4.0;

                // `TextEdit` 受控输入，缓冲归本会话（`Tab.filter_input`）。
                let response = {
                    let tab = &mut self.tabs[slot];
                    let border = if regex_error.is_some() {
                        egui::Color32::from_rgb(0xE0, 0x6C, 0x6C)
                    } else {
                        crate::theme::chrome(CHROME_SCALE.border)
                    };
                    let resp = ui.add(
                        egui::TextEdit::singleline(&mut tab.filter_input)
                            .id_source(("sysmon_filter_query", salt))
                            .desired_width(160.0)
                            .hint_text(t(lang, "Filter hint")),
                    );
                    ui.painter().rect_stroke(
                        resp.rect.expand(1.0),
                        0.0,
                        egui::Stroke::new(1.0, border),
                    );
                    resp
                };
                if response.changed() {
                    let q = self.tabs[slot].filter_input.clone();
                    pending.push(term_app::vocab::FilterAction::SetQuery(q));
                }
                if response.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Escape)) {
                    pending.push(term_app::vocab::FilterAction::Close);
                }

                // grep / grep -v 切换。
                let mode_label = match mode {
                    term_app::state::FilterMode::KeepMatching => t(lang, "grep"),
                    term_app::state::FilterMode::DropMatching => t(lang, "grep -v"),
                };
                if ui
                    .selectable_label(
                        matches!(mode, term_app::state::FilterMode::DropMatching),
                        mode_label,
                    )
                    .on_hover_text(t(lang, "grep -v"))
                    .clicked()
                {
                    pending.push(term_app::filter::ui_command(
                        term_app::filter::FilterUiCommand::ToggleMode,
                    ));
                }
                // 大小写。
                if ui.selectable_label(case_sensitive, t(lang, "Aa")).clicked() {
                    pending.push(term_app::vocab::FilterAction::ToggleCase);
                }
                // 正则。
                if ui
                    .selectable_label(regex, t(lang, "Regular expression"))
                    .clicked()
                {
                    pending.push(term_app::vocab::FilterAction::ToggleRegex);
                }
                // 计数（每帧现算）。
                ui.label(format!("{kept}/{hidden}"));
                // 关闭。
                if ui.button(t(lang, "Close")).clicked() {
                    pending.push(term_app::vocab::FilterAction::Close);
                }
                if let Some(err) = regex_error.as_deref() {
                    ui.label(
                        egui::RichText::new(err).color(egui::Color32::from_rgb(0xE0, 0x6C, 0x6C)),
                    );
                }
            });
        });

        for action in pending {
            // `Action::Filter` 不写 `ViewState`；对本会话那条 tab 落地（分屏互不串味）。
            self.apply_filter_on(session, action);
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
        let r = button_rect(ui, rect, label);
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
        let strip = readout_rect_for(ui, rect, segs);
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

/// 展开态读数条的矩形（右对齐）。窄窗时不超过细带本身。
fn readout_rect(rect: egui::Rect, width: f32) -> egui::Rect {
    let w = width.min(rect.width() - EDGE_PAD * 2.0).max(24.0);
    egui::Rect::from_min_max(
        egui::pos2(rect.right() - EDGE_PAD - w, rect.top() + 1.0),
        egui::pos2(rect.right() - EDGE_PAD, rect.bottom() - 1.0),
    )
}

/// 内容宽度托底 265pt：夹具那一组值仍画成基线宽度，长读数才把条撑宽。
fn readout_rect_for(ui: &egui::Ui, rect: egui::Rect, segs: &[Segment]) -> egui::Rect {
    readout_rect(rect, segments_width(ui, segs).max(READOUT_W))
}

/// R 方块的矩形：贴在读数条左边，宽度随读数文字走；窄窗时仍留在细带内。
fn remote_chip_rect(ui: &egui::Ui, bar: egui::Rect, strip: egui::Rect, text: &str) -> egui::Rect {
    let font = egui::FontId::monospace(FONT_PT);
    let text_w = ui
        .painter()
        .layout_no_wrap(text.to_string(), font, egui::Color32::WHITE)
        .size()
        .x;
    let w = CHIP + 4.0 + text_w + GAP;
    let left = (strip.left() - w - 2.0).max(bar.left());
    egui::Rect::from_min_max(
        egui::pos2(left, strip.top()),
        egui::pos2(left + w, strip.bottom()),
    )
}

/// 收起态按钮的矩形：按 `Sysmon` 文字自然宽度 + 左右留白，右对齐。
fn button_rect(ui: &egui::Ui, rect: egui::Rect, label: &str) -> egui::Rect {
    let font = crate::ui::type_scale::body();
    let text_w = ui
        .painter()
        .layout_no_wrap(label.to_string(), font, egui::Color32::WHITE)
        .size()
        .x;
    let w = (text_w + PAD_L * 2.0).min(rect.width());
    egui::Rect::from_min_max(
        egui::pos2(rect.right() - EDGE_PAD - w, rect.top() + 1.0),
        egui::pos2(rect.right() - EDGE_PAD, rect.bottom() - 1.0),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn readout_and_button_share_the_same_right_edge() {
        let rect = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(640.0, SYSMON_BAR_H));
        let strip = readout_rect(rect, READOUT_W);
        // 按钮宽度由字体决定，这里只要求读数条右沿贴住 EDGE_PAD。
        assert!((strip.right() - (rect.right() - EDGE_PAD)).abs() < 0.01);
        assert!(strip.width() <= READOUT_W);
    }

    #[test]
    fn longer_readouts_grow_left_ward_without_moving_the_right_edge() {
        let rect = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(640.0, SYSMON_BAR_H));
        let fixture = readout_rect(rect, READOUT_W);
        let long = readout_rect(rect, READOUT_W + 40.0);
        assert!(long.left() < fixture.left(), "长读数要往左撑开");
        assert_eq!(long.right(), fixture.right(), "右沿必须钉死");
        assert!(long.width() <= rect.width());
    }

    #[test]
    fn remote_text_never_invents_a_reading() {
        let none = crate::sysmon_sample::Reading::default();
        assert_eq!(remote_text(false, Some(&none)), "—", "关着就不报数");
        assert_eq!(remote_text(true, None), "—", "采不到就是没有");
        assert_eq!(remote_text(true, Some(&none)), "0.0%", "真采到的 0 才是 0");
        let live = crate::sysmon_sample::Reading {
            cpu_pct: 37.5,
            ..none
        };
        assert_eq!(remote_text(true, Some(&live)), "37.5%");
    }

    #[test]
    fn mem_text_matches_the_reference_format() {
        let used = 24_117_248u64;
        let total = 24_776_704u64;
        assert_eq!(mem_text(used, total), "23.00/23.63 GiB");
        assert_eq!(mem_text(0, 0), "—/— GiB");
    }

    #[test]
    fn a_failed_remote_sample_shows_a_dash_not_zero_percent() {
        let failed = crate::sysmon_remote::from_exec(
            crate::sysmon_remote::Target::Linux,
            0,
            Err(term_ssh::SshError::Config("采样失败".to_string())),
        );
        assert!(failed.is_none());
        assert_eq!(remote_text(true, failed.as_ref()), "—");
    }

    #[test]
    fn remote_target_key_ignores_session_identity_but_separates_machines() {
        let a = SessionConfig::new(Protocol::Ssh);
        let mut renamed = a.clone();
        renamed.name = "另一条会话".to_string();
        assert_eq!(remote_target_key(&a), remote_target_key(&renamed));
        let mut other_host = a.clone();
        other_host.host = "10.0.0.2".to_string();
        assert_ne!(remote_target_key(&a), remote_target_key(&other_host));
        let mut other_user = a.clone();
        other_user.username = "root".to_string();
        assert_ne!(remote_target_key(&a), remote_target_key(&other_user));
        let mut other_port = a.clone();
        other_port.port = 2222;
        assert_ne!(remote_target_key(&a), remote_target_key(&other_port));
    }

    /// 切换远端源时，上一台机器的读数不能在新机器上继续显示。
    /// 同一台机器重复同步也不许清——否则每帧都归零，读数永远出不来。
    #[test]
    fn switching_remote_target_clears_the_previous_reading() {
        let stale = crate::sysmon_sample::Reading {
            cpu_pct: 77.7,
            ..Default::default()
        };
        set_remote_target(None);
        crate::sysmon_remote::clear();
        assert!(set_remote_target(Some("alice@10.0.0.1:22")));
        crate::sysmon_remote::seed_reading_for_test(Some(stale.clone()));
        assert_eq!(
            crate::sysmon_remote::reading().map(|r| r.cpu_pct),
            Some(77.7),
            "上一台机器的读数确实在快照里"
        );
        assert!(
            !set_remote_target(Some("alice@10.0.0.1:22")),
            "同一台不重置"
        );
        assert!(crate::sysmon_remote::reading().is_some());
        assert!(
            set_remote_target(Some("bob@10.0.0.2:2222")),
            "换机器必须重置"
        );
        assert!(
            crate::sysmon_remote::reading().is_none(),
            "上一台机器的读数不能留到新机器上"
        );
        crate::sysmon_remote::seed_reading_for_test(Some(stale));
        assert!(set_remote_target(None), "关掉开关也要重置");
        assert!(crate::sysmon_remote::reading().is_none());
        set_remote_target(None);
        crate::sysmon_remote::clear();
    }

    /// 过滤行与 Sysmon 读数共用同一条细带、不产生第二个 `TopBottomPanel` 声明。
    ///
    /// 源码/绘制形状断言（仓库已有 `include_str!` 风格）：`draw_sysmon_bar` 里同时
    /// 含 Sysmon 读数与过滤行；`ui/mod.rs` 里没有 `filter_bar` 独立面板；过滤输入
    /// 与按钮 id 带 `pane_salt`；没有 `filter_bar.rs` / `draw_filter_bar`。
    #[test]
    fn filter_row_shares_the_sysmon_band_no_second_panel() {
        let src = include_str!("sysmon_bar.rs");
        let mod_src = include_str!("../mod.rs");
        let shell_src = include_str!("mod.rs");

        // 不产生第二个 TopBottomPanel 声明（这条细带只声明一次 sysmon_bar）。
        assert!(
            !mod_src.contains("TopBottomPanel::bottom(\"filter_bar\")"),
            "不许有独立过滤底栏 TopBottomPanel::bottom(filter_bar)"
        );
        assert!(
            !mod_src.contains("filter_bar"),
            "ui/mod.rs 不许再有 filter_bar 独立面板"
        );
        assert!(
            !shell_src.contains("mod filter_bar;"),
            "filter_bar.rs 已删除"
        );
        assert!(
            !shell_src.contains("draw_filter_bar"),
            "不再有 draw_filter_bar"
        );

        // 过滤行与 Sysmon 读数共用同一个 `draw_sysmon_bar` 这一条 25pt 细带。
        assert!(src.contains("pub(in crate::ui) const SYSMON_BAR_H: f32 = 25.0;"));
        assert!(
            src.contains("fn draw_filter_row"),
            "过滤行画在 sysmon 细带内"
        );
        assert!(
            src.contains("draw_sysmon_readout"),
            "Sysmon 读数仍在同一细带"
        );
        // `Filter` 按钮 + 过滤行在同一个 `draw_sysmon_bar` 里。
        let bar_body = &src[src.find("pub(in crate::ui) fn draw_sysmon_bar").unwrap()
            ..src.find("/// 紧凑过滤行").unwrap()];
        assert!(
            bar_body.contains("draw_filter_row"),
            "过滤行由 draw_sysmon_bar 派发"
        );
        assert!(
            bar_body.contains("draw_sysmon_readout"),
            "Sysmon 读数由 draw_sysmon_bar 派发"
        );
        assert!(
            bar_body.contains("filter_visible"),
            "Filter 按钮在两态之间切换"
        );
        // 过滤行内含 TextEdit + grep/grep-v + 大小写 + 正则 + 关闭。
        assert!(
            src.contains("egui::TextEdit::singleline"),
            "过滤行有 TextEdit"
        );
        assert!(
            src.contains("FilterUiCommand::ToggleMode"),
            "grep / grep -v 切换"
        );
        assert!(src.contains("FilterAction::ToggleCase"), "大小写");
        assert!(src.contains("FilterAction::ToggleRegex"), "正则");
        assert!(src.contains("FilterAction::Close"), "关闭");
        // 输入与按钮 id 带 pane_salt。
        assert!(
            src.contains("(\"sysmon_filter\", p)"),
            "Filter 按钮 id 带 pane_salt"
        );
        assert!(
            src.contains("(\"sysmon_filter_query\", salt)"),
            "输入 id 带 pane_salt"
        );
        assert!(
            src.contains("ui.push_id((\"sysmon_filter_row\", salt)"),
            "过滤行 id 带 pane_salt"
        );
    }
}

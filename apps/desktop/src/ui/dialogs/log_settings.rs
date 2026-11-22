//! Session Settings ▸ Logging 配置页的**绘制**（2026-11-08 视觉基线
//! `clipboard-…e44425d2.png`，1280×1140 @2x ⇒ 640×570 逻辑点）。
//!
//! 规格 / 状态 / 单测（含视觉基线逐项与取色表）在 `log_settings_spec.rs` 要求
//! `ui/**/*.rs` ≤ 450 行，两边合起来 738 行会一条门禁红掉整个阶段。本文件只画，
//! 绝不定义第二份状态。
//!
//! # 诚实接线
//! - 协议条 / 左树 / Log types / 三个输入框 / 步进 / `...` / Cancel / Save 都是真状态；
//! - `Log options` 两行的禁用判据是 `LogType::Nothing`，出厂态与视觉基线一致；
//! - 其余叶子明确显示「该配置页尚未接线」，不做点了没反应的开关；
//! - `Edit Default Settings...` 打开全局 Preferences，不伪造还不存在的默认会话表。
//!
//! # 这五个字段从 2026-11-14 起真的生效
//! 表单 → `term_core::log_opts::LogOptions` → `SessionLog`：文件名模板展开、
//! 每行内容模板、覆盖 / 追加、大小上限轮转全部由写入端消费。`prefs.json` 那
//! 五个字段的下游只有一个（`Prefs::log_options`），界面与写入端不再各解释一遍。
//!
//! `Log types` 同时是**自动日志**的开关：非 `Nothing` 时会话刚连上就自动开一份
//! （`poll_auto_log`，只认 `Tab::poll` 的 Connect 边沿，重连会重新开）。
//! `Nothing` 只表示「不自动记」，手动 `Start...` 仍可点——视觉基线里 `Log nothing`
//! 选中时它就是亮的。
//!
//! 状态放在 `TerminalWindow.log_settings`（投影层控件态，不进 `ViewState`）。

use super::log_settings_spec::{
    logging_row, nav_rows, LogType, BODY_H, BTN_H, CONTENT_MIN, FIELD_FILL,
    FIELD_W, FOOTER_DY, LABEL_W, NAV_W, RADIO_H, ROW_H,
    TAB_ACTIVE, TAB_BG, TAB_H, TABS, TEXT, TEXT_DISABLED, TEXT_STRONG, TREE_HOVER,
    TREE_SELECTED,
};
use crate::ui::kit::window::{DialogId, DialogWindow};
use crate::ui::*;
use term_app::i18n::{self, Lang};

use crate::ui::kit::spec::col;

impl TerminalWindow {
    /// `Session ▸ Log ▸ Settings...` 打开的窗口。外框（尺寸 / 淡入 / ×）
    /// 一律走 `StableWindow`，内容全在本文件。
    pub(in crate::ui) fn draw_log_settings_dialog(&mut self, ctx: &egui::Context) {
        let lang = self.lang();
        let title = i18n::t(lang, "Session Settings");
        let note = self.log_note.clone();
        let status = self
            .tabs
            .get(self.active_slot())
            .map(|t| t.log_status())
            .unwrap_or_default();
        let size = status
            .path
            .as_ref()
            .and_then(|p| std::fs::metadata(p).ok())
            .map(|m| m.len());
        // 一次性提示 / 当前会话状态只在「有话要说」时画，出厂态右栏下方保持
        // 视觉基线那片空白，不为了显示状态而改版式。
        let show_status = note.is_some() || status.phase != term_runtime::log::LogPhase::Off;

        // 闭包会以 &mut self 捕获窗口，所有取词先按 Lang 算好（Copy，不借 self）。
        let mut picked_protocol = None;
        let mut picked_page = None;
        let mut toggled_group = None;
        let mut want_close = false;
        let mut want_save = false;
        let mut want_defaults = false;
        // Esc=Cancel；Enter=Save，但输入框聚焦时不抢回车（打字不能误保存）。
        let keys = DialogId::SessionLogging.keys().poll(ctx);
        if keys.cancel {
            want_close = true;
        }
        if keys.confirm && ctx.memory(|m| m.focused().is_none()) {
            want_save = true;
        }

        let open = DialogWindow::new(DialogId::SessionLogging, title)
            .show_with_rect(ctx, |ui, win| {
                // ── 顶部协议标签条 ──
                ui.horizontal(|ui| {
                    ui.set_min_height(TAB_H);
                    ui.spacing_mut().item_spacing.x = 2.0;
                    let w = (CONTENT_MIN.x - 8.0) / TABS.len() as f32;
                    for (p, label) in TABS {
                        let active = p == self.log_settings.protocol;
                        if protocol_tab(ui, w, i18n::t(lang, label), active).clicked() {
                            picked_protocol = Some(p);
                        }
                    }
                });
                ui.add_space(8.0);

                // ── 左树 + 右栏 ──
                // 页脚走绝对定位（见下），这里只留够放内容的高度，不撑满窗口。
                let rows = nav_rows(self.log_settings.protocol);
                ui.horizontal(|ui| {
                    ui.set_min_height(BODY_H);
                    ui.vertical(|ui| {
                        ui.set_width(NAV_W);
                        ui.spacing_mut().item_spacing.y = 0.0;
                        for (i, row) in rows.iter().enumerate() {
                            if let Some(g) = row.group {
                                if !row.header && !self.log_settings.groups_open[g] {
                                    continue;
                                }
                            }
                            let label = if row.header {
                                let mark = match row.group {
                                    Some(g) if self.log_settings.groups_open[g] => "▼ ",
                                    _ => "▸ ",
                                };
                                format!("{}{}", mark, i18n::t(lang, row.label))
                            } else {
                                i18n::t(lang, row.label).to_string()
                            };
                            let selected = !row.header && self.log_settings.page == i;
                            if nav_row(ui, NAV_W - 8.0, row.indent, &label, selected).clicked() {
                                if row.header {
                                    toggled_group = row.group;
                                } else {
                                    picked_page = Some(i);
                                }
                            }
                        }
                    });
                    ui.separator();
                    ui.vertical(|ui| {
                        ui.set_width(CONTENT_MIN.x - NAV_W - 24.0);
                        let is_logging = rows
                            .get(self.log_settings.page)
                            .map(|r| r.label == "Logging")
                            .unwrap_or(false);
                        if is_logging {
                            self.draw_logging_form(ui, lang);
                        } else {
                            let name = rows
                                .get(self.log_settings.page)
                                .map(|r| i18n::t(lang, r.label))
                                .unwrap_or("");
                            ui.add_space(10.0);
                            ui.label(name);
                            ui.add_space(8.0);
                            ui.label(
                                egui::RichText::new(i18n::t(lang, "Not wired (placeholder)"))
                                    .color(col(TEXT_DISABLED)),
                            );
                        }
                        if show_status {
                            ui.add_space(12.0);
                            if let Some(n) = &note {
                                ui.label(egui::RichText::new(n).color(egui::Color32::LIGHT_RED));
                            }
                            ui.label(format!(
                                "{} {}",
                                i18n::t(lang, "Status:"),
                                super::log::phase_text(&status.phase)
                            ));
                            if let (Some(p), Some(n)) = (&status.path, size) {
                                ui.label(format!(
                                    "{} {}",
                                    i18n::t(lang, "File:"),
                                    p.display()
                                ));
                                ui.label(format!(
                                    "{} {}",
                                    i18n::t(lang, "Size:"),
                                    super::log::format_bytes(n)
                                ));
                            }
                        }
                    });
                });

                // ── 页脚：绝对定位 ──
                // `place` 按窗口矩形把每颗按钮钉在金标准坐标上；按钮本身的
                // 尺寸/配色来自 `kit::spec::FooterButton`，不再用 `ui.put` +
                // 三份手写 `egui::Button`。与 Preferences / 会话弹窗同一原语。
                use crate::ui::kit::spec::FooterButton as FB;
                let fy = win.min.y + FOOTER_DY;
                let r_defaults = egui::Rect::from_min_size(
                    egui::pos2(win.min.x + 11.0, fy),
                    egui::vec2(186.0, BTN_H),
                );
                let save_rect = egui::Rect::from_min_size(
                    egui::pos2(win.max.x - 88.0, fy),
                    egui::vec2(77.0, BTN_H),
                );
                let cancel_rect = egui::Rect::from_min_size(
                    egui::pos2(win.max.x - 174.0, fy),
                    egui::vec2(77.0, BTN_H),
                );
                if super::dialog_chrome::place(ui, r_defaults, |ui| {
                    crate::ui::kit::footer::widget_button(
                        ui, &i18n::t(lang, "Edit Default Settings"), FB::LogDefault.spec(), true,
                    )
                })
                .on_hover_text(i18n::t(lang, "Open Preferences"))
                .clicked()
                {
                    want_defaults = true;
                }
                if super::dialog_chrome::place(ui, save_rect, |ui| {
                    crate::ui::kit::footer::widget_button(ui, &i18n::t(lang, "Save"), FB::LogSave.spec(), true)
                })
                .clicked()
                {
                    want_save = true;
                }
                if super::dialog_chrome::place(ui, cancel_rect, |ui| {
                    crate::ui::kit::footer::widget_button(ui, &i18n::t(lang, "Cancel"), FB::LogCancel.spec(), true)
                })
                .clicked()
                {
                    want_close = true;
                }
            });

        if let Some(p) = picked_protocol {
            self.log_settings.protocol = p;
            self.log_settings.page = logging_row(p);
        }
        if let Some(i) = picked_page {
            self.log_settings.page = i;
        }
        if let Some(g) = toggled_group {
            self.log_settings.groups_open[g] = !self.log_settings.groups_open[g];
        }
        if want_save {
            let saved = self.log_settings.save();
            self.log_note = Some(if saved {
                i18n::t(lang, "Logging settings saved").to_string()
            } else {
                i18n::t(lang, "Could not save logging settings").to_string()
            });
            want_close = true;
        }
        if want_defaults {
            // 本版本「默认设置」的唯一真实入口就是全局 Preferences；不伪造
            // 一张还不存在的「默认会话配置」表。
            self.view.show_settings = true;
            want_close = true;
        }
        if want_close || !open {
            self.view.show_log_settings = false;
        }
    }

    /// 右栏 Logging 表单。所有控件都读 / 写 `log_settings`，没有本地临时变量，
    /// 所以输入法预编辑不会逐帧丢（受控输入先例）。
    fn draw_logging_form(&mut self, ui: &mut egui::Ui, lang: Lang) {
        ui.spacing_mut().item_spacing.y = 4.0;
        ui.label(underlined(ui, i18n::t(lang, "Log types:"), "Log types:"));
        ui.add_space(6.0);
        let type_labels = [
            ("Log nothing", LogType::Nothing),
            ("Log printable output", LogType::Printable),
            ("Log all session output", LogType::All),
        ];
        for (key, value) in type_labels {
            let row = option_row(
                ui,
                i18n::t(lang, key),
                self.log_settings.log_type == value,
                true,
            );
            if row.clicked() {
                self.log_settings.log_type = value;
            }
        }
        ui.add_space(8.0);
        ui.label(underlined(ui, i18n::t(lang, "Log options:"), "Log options:"));
        ui.add_space(6.0);
        let enabled = self.log_settings.log_type.enabled();
        for (key, selected) in [
            ("Overwrite log file", self.log_settings.overwrite == Some(true)),
            ("Append log file", self.log_settings.overwrite == Some(false)),
        ] {
            let row = option_row(ui, i18n::t(lang, key), selected, enabled);
            // 手绘单选没有 `add_enabled(false)` 的 disabled 标记，用普通悬停
            // 说明为什么这两行是灰的；颜色仍由 option_row 烤成 #555555。
            let row = if enabled {
                row
            } else {
                row.on_hover_text(i18n::t(lang, "Not available in the current log state"))
            };
            if enabled && row.clicked() {
                self.log_settings.overwrite = Some(key.starts_with("Overwrite"));
            }
        }
        ui.add_space(10.0);
        ui.label(underlined(ui, i18n::t(lang, "Log file name:"), "Log file name:"));
        // 与 Modem 目录框同一原语：通栏字段 + 右端内嵌 `| ...` 浏览槽。
        // 保留这一格独有的 #333333 实心底（视觉基线），其余交付给 kit。
        ui.scope(|ui| {
            ui.visuals_mut().extreme_bg_color = col(FIELD_FILL);
            let (_, browse) = crate::ui::kit::field::browse_field(
                ui,
                &mut self.log_settings.file_name,
                "",
                "...",
                24.0,
                28.0,
            );
            if browse.on_hover_text(i18n::t(lang, "Open log folder")).clicked() {
                self.open_log_folder();
            }
        });
        ui.add_space(8.0);
        // 两行「标签 + 控件」走 Grid：列宽由最长的 `Log content pattern:`
        // 决定，短标签也左对齐（`add_sized` 的 Label 会被居中，首版实测缩进）。
        let mut mb = self.log_settings.max_size_mb as f64;
        egui::Grid::new("log_settings_form_grid")
            .num_columns(2)
            .spacing(egui::vec2(8.0, 8.0))
            .min_col_width(LABEL_W)
            .show(ui, |ui| {
                ui.label(underlined(
                    ui,
                    i18n::t(lang, "Log maximum size:"),
                    "Log maximum size:",
                ));
                let dv = egui::DragValue::new(&mut mb)
                    .range(0.0..=1024.0)
                    .speed(1.0)
                    .custom_formatter(|n, _| {
                        if n == 0.0 { "∞".to_string() } else { format!("{n:.0} MB") }
                    });
                let (drag, stepped) = crate::ui::kit::form::spin_field_delta(ui, |ui| { ui.add(dv); });
                let _ = drag;
                if let Some(d) = stepped {
                    mb = (mb + d as f64).clamp(0.0, 1024.0);
                }
                ui.end_row();
                ui.label(underlined(
                    ui,
                    i18n::t(lang, "Log content pattern:"),
                    "Log content pattern:",
                ));
                ui.add_sized(
                    egui::vec2(FIELD_W, 24.0),
                    egui::TextEdit::singleline(&mut self.log_settings.pattern),
                );
                ui.end_row();
            });
        self.log_settings.max_size_mb = mb as u32;
    }

    /// `...` 按钮：打开日志目录（目录不存在先建出来），与菜单
    /// `Open Log Folder` 走同一条路径，不另造一个「点了没反应」的按钮。
    fn open_log_folder(&mut self) {
        let Some(dir) = term_runtime::log::default_dir() else { return };
        if std::fs::create_dir_all(&dir).is_err() {
            return;
        }
        let _ = crate::sysopen::open(&dir);
    }
}

/// 协议标签：整块填充 + 首字母下划线。选中块与弹窗同色（视觉基线反而压暗）。
fn protocol_tab(ui: &mut egui::Ui, width: f32, label: &str, active: bool) -> egui::Response {
    let (rect, resp) = ui.allocate_exact_size(egui::vec2(width, TAB_H), egui::Sense::click());
    let painter = ui.painter_at(rect);
    painter.rect_filled(
        rect,
        egui::Rounding::ZERO,
        col(if active { TAB_ACTIVE } else { TAB_BG }),
    );
    let font = egui::TextStyle::Button.resolve(ui.style());
    painter.text(
        rect.center(),
        egui::Align2::CENTER_CENTER,
        label,
        font,
        col(if active { TEXT_STRONG } else { TEXT }),
    );
    // 访问字母下划线只画一个字符宽，位置按居中文本估算（标签文字等宽）。
    let cw = 7.0;
    let start = rect.center().x - label.len() as f32 * cw / 2.0;
    painter.line_segment(
        [
            egui::pos2(start, rect.center().y + 8.0),
            egui::pos2(start + cw, rect.center().y + 8.0),
        ],
        egui::Stroke::new(1.0, col(TEXT)),
    );
    resp
}

/// 左树一行：整行灰块选中 / 悬停，手绘而不是 `SelectableLabel`
/// （后者只按文字宽度画底，做不出视觉基线的整行 #282828）。
fn nav_row(ui: &mut egui::Ui, width: f32, indent: f32, label: &str, selected: bool) -> egui::Response {
    let (rect, resp) = ui.allocate_exact_size(egui::vec2(width, ROW_H), egui::Sense::click());
    let painter = ui.painter_at(rect);
    if selected {
        painter.rect_filled(rect, 3.0, col(TREE_SELECTED));
    } else if resp.hovered() {
        painter.rect_filled(rect, 3.0, col(TREE_HOVER));
    }
    let font = egui::TextStyle::Body.resolve(ui.style());
    painter.text(
        rect.left_center() + egui::vec2(6.0 + indent, 0.0),
        egui::Align2::LEFT_CENTER,
        label,
        font,
        col(TEXT),
    );
    resp
}

/// 单选项：实心圈 + 内点 + 文字。禁用时用 `#555555` 且不响应点击
/// （`add_enabled(false)` 会让 egui 按主题置灰，色值不可控，见菜单三态取色）。
fn option_row(ui: &mut egui::Ui, label: &str, selected: bool, enabled: bool) -> egui::Response {
    let width = ui.available_width();
    let sense = if enabled { egui::Sense::click() } else { egui::Sense::hover() };
    let (rect, resp) = ui.allocate_exact_size(egui::vec2(width, RADIO_H), sense);
    let color = col(if enabled { TEXT } else { TEXT_DISABLED });
    let center = egui::pos2(rect.min.x + 12.0, rect.center().y);
    let painter = ui.painter_at(rect);
    painter.circle_stroke(center, 7.0, egui::Stroke::new(1.2, color));
    if selected {
        painter.circle_filled(center, 3.5, color);
    }
    let font = egui::TextStyle::Body.resolve(ui.style());
    painter.text(
        egui::pos2(rect.min.x + 30.0, rect.center().y),
        egui::Align2::LEFT_CENTER,
        label,
        font,
        color,
    );
    resp
}

/// 字段标签的访问字母下划线：英文原文里找 `acc_word` 的首字母，中文退化到首字。
fn underlined(ui: &egui::Ui, shown: &str, key: &str) -> egui::WidgetText {
    let font = egui::TextStyle::Body.resolve(ui.style());
    let plain = egui::TextFormat::simple(font, egui::Color32::PLACEHOLDER);
    let mut marked = plain.clone();
    marked.underline = egui::Stroke::new(1.0, col(TEXT));
    let target = key.chars().nth(4).unwrap_or('L');
    let pos = shown.find(target).unwrap_or(0);
    let pos = if shown.is_char_boundary(pos) { pos } else { 0 };
    let tail = &shown[pos..];
    let n = tail.chars().next().map(char::len_utf8).unwrap_or(0);
    let mut job = egui::text::LayoutJob::default();
    job.append(&shown[..pos], 0.0, plain.clone());
    job.append(&tail[..n], 0.0, marked);
    job.append(&tail[n..], 0.0, plain);
    egui::WidgetText::from(job)
}

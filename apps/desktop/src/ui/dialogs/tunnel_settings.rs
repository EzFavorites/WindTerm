//! Tunnels 设置对话框（视觉基线 `clipboard-…-b2ab71d2.png`，2026-11-17 用户提供）。
//!
//! # 版式（2× 截图逐像素量，除 2 得逻辑点）
//!
//! ```text
//! ┌──────────────┐  Name:              [________________]
//! │              │  Type:              [             ▾ ]
//! │   (列表)      │  Session:           [             ▾ ]
//! │              │  Listen Port:       [_____________ ⇅ ]
//! │              │  Destination Host:  [________________]
//! │              │  Destination Port:  [             0 ⇅]
//! │              │  Status:            (●)              ← 胶囊开关
//! ├──────────────┤
//! │ +  -         │  ☐ Accept local connections only
//! └──────────────┘  ☐ Automatically open at startup
//!  Tunnels will be refreshed…              [Cancel] [Ok]
//! ```
//!
//! | 元素 | 物理 px | 逻辑点 |
//! |---|---|---|
//! | 对话框 | 1408×912 | 704×456 |
//! | 左列表框 | x 22..538，y 22..789 | x 11..269，y 11..394.5 |
//! | 左工具行 | y 789..829，高 40 | y 394.5..414，高 19.5 |
//! | 标签列起点 | x 563 | 281.5 |
//! | 控件列 | x 820..1385（宽 566） | x 410..693（宽 283） |
//! | 控件高 | 48 | 24 |
//! | 行距 | 60 | 30 |
//! | 勾选框 | 30×30 | 15×15，y 731 / 791 |
//! | 页脚按钮 | 161×48 | 80.5×24，y 842..889 |
//! | 橙字提示 | `#FF8400` | 基线盒 y 431..445 |
//!
//! 配色：底 `#1C1C1C`、外框 `#333333`、控件底 `#333333`、列表框框线 `#444444`、
//! 标签 `#BBBBBB`、控件内文字 `#B7B7B7`、关态开关轨道 `#444444` + 白滑块。
//!
//! # 语义
//!
//! 左列表是 `Prefs.tunnels` 的一份**草稿**：`+` 新建、`−` 删除，`Ok` 才写回
//! `prefs` 并 `reload_tunnels()`。`Cancel` 丢掉草稿。右栏编辑当前选中的那条。
//!
//! `Ok` 之后那句橙字说的「Tunnels will be refreshed after closing the dialog」
//! 是真的：写回 prefs 时一并按新的启用状态重起隧道（`apply_tunnels_after_edit`）。

use crate::ui::*;
use crate::ui::kit::window::DialogWindow;
use super::dialog_chrome::{place, rgb};
use term_core::tunnel::{TunnelKind, TunnelProfile};

// ── 实测几何（逻辑点） ──
pub(in crate::ui) const DIALOG_W: f32 = 704.0;
pub(in crate::ui) const DIALOG_H: f32 = 456.0;
const LIST_X: f32 = 11.0;
const LIST_Y: f32 = 11.0;
const LIST_W: f32 = 258.0;
const LIST_H: f32 = 383.5;
const TOOL_Y: f32 = 394.5;
const TOOL_H: f32 = 19.5;
const LABEL_X: f32 = 281.5;
const FIELD_X: f32 = 410.0;
const FIELD_W: f32 = 283.0;
/// 控件高：视觉基线 46..48 px，按 23 走（Type/Session 那两行是 46）。
const FIELD_H: f32 = 23.0;
/// 前六行的 y（`Name` / `Type` / `Session` / `Listen Port` / `Dest Host` / `Dest Port`）。
const ROW_Y: [f32; 6] = [11.0, 41.0, 70.0, 99.0, 129.0, 159.0];
/// `Status` 那一行（开关不与控件列对齐，它在标签列右侧）。
/// `Status` 那一行的开关顶边（物理 378 → 189）。
const STATUS_Y: f32 = 189.0;
const CHECK_Y: [f32; 2] = [365.5, 395.5];
const CHECK_BOX: f32 = 15.0;
const FOOT_Y: f32 = 421.0;
const HINT_Y: f32 = 426.5;
/// 列表行高：与 Triggers 左栏同一档（视觉基线里这两张对话框是同一个列表）。
const ROW_H: f32 = 19.5;

const C_BG: u32 = 0x1C1C1C;
const C_LINE: u32 = 0x444444;
const C_FIELD: u32 = 0x333333;
const C_LABEL: u32 = 0xBBBBBB;
const C_TEXT: u32 = 0xB7B7B7;
const C_HINT: u32 = 0xFF8400;
const C_SEL: u32 = 0x094771;

/// 对话框的编辑草稿。**不落盘**——`Ok` 才写回 `prefs`。
pub(in crate::ui) struct Editor {
    /// 草稿列表（副本，`Cancel` 直接丢掉）。
    pub(in crate::ui) rows: Vec<TunnelProfile>,
    pub(in crate::ui) sel: Option<usize>,
    /// 最近一次校验失败的原因（显示在橙字那一行下方）。
    pub(in crate::ui) error: Option<String>,
}

impl Editor {
    pub(in crate::ui) fn new(rows: Vec<TunnelProfile>) -> Self {
        let sel = if rows.is_empty() { None } else { Some(0) };
        Self {
            rows,
            sel,
            error: None,
        }
    }

    pub(in crate::ui) fn current(&self) -> Option<&TunnelProfile> {
        self.sel.and_then(|i| self.rows.get(i))
    }

    pub(in crate::ui) fn current_mut(&mut self) -> Option<&mut TunnelProfile> {
        let i = self.sel?;
        self.rows.get_mut(i)
    }
}

/// 把总线开关写回 `prefs.json`（`Effect::ToggleTunnels` 的第二步）。
///
/// 放在这里而不是 `construct.rs`：`settings_live::save_prefs` 是 `dialogs`
/// 内的私有口子，`ui` 层拿不到；本文件与它同父，正好能转一道手。
pub(in crate::ui) fn save_enabled(on: bool) {
    super::settings_live::save_prefs(|p| p.tunnels_enabled = on);
}

/// 画对话框。返回 `Some(Ok)` = 用户按了 `Ok`（草稿已写回 `prefs`），
/// `Some(Err)` = 校验失败停在原地，`None` = 还开着。
pub(in crate::ui) fn draw(
    win: &mut TerminalWindow,
    ctx: &egui::Context,
) -> Option<Result<(), String>> {
    let mut ed = win.tunnel_editor.take()?;
    let mut close = false;
    let mut commit = false;

    DialogWindow::new_sized("tunnel_settings", "", egui::vec2(DIALOG_W, DIALOG_H))
        .without_title_bar()
        .tight()
        .show(ctx, |ui| {
            ui.set_min_size(egui::vec2(DIALOG_W, DIALOG_H));
            let o = ui.cursor().min;
            ui.painter()
                .rect_filled(egui::Rect::from_min_size(o, egui::vec2(DIALOG_W, DIALOG_H)), 0.0, rgb(C_BG));
            list(ui, o, &mut ed);
            form(ui, o, &mut ed);
            footer(ui, o, &ed, &mut close, &mut commit);
        });

    if commit {
        if ed.rows.iter().any(|r| r.name.trim().is_empty()) {
            // 校验失败：**停在原地**，草稿原样留给用户改。
            ed.error = Some("有一条隧道没有名字。".into());
        } else {
            let snapshot = ed.rows.clone();
            super::settings_live::save_prefs(|p| p.tunnels = snapshot.clone());
            // 橙字那句「closing the dialog 之后会 refresh」就是这一步：
            // 按新配置把隧道重起一遍（总开关关着就全停）。
            win.apply_tunnels(snapshot);
            return Some(Ok(()));
        }
    }
    if !close {
        win.tunnel_editor = Some(ed);
        return None;
    }
    Some(Err("cancelled".into()))
}

/// 左栏：列表 + 底部工具行。
fn list(ui: &mut egui::Ui, o: egui::Pos2, ed: &mut Editor) {
    let rect = egui::Rect::from_min_size(
        o + egui::vec2(LIST_X, LIST_Y),
        egui::vec2(LIST_W, LIST_H),
    );
    let p = ui.painter();
    p.rect_filled(rect, 0.0, rgb(C_BG));
    p.rect_stroke(rect, 0.0, egui::Stroke::new(1.0, rgb(C_LINE)));
    let mut pick = None;
    for (i, row) in ed.rows.iter().enumerate() {
        let r = egui::Rect::from_min_size(
            rect.min + egui::vec2(0.0, i as f32 * ROW_H),
            egui::vec2(rect.width(), ROW_H),
        );
        if r.bottom() > rect.bottom() {
            break; // 放不下的不再画（滚动留给下一轮）
        }
        let resp = ui.interact(r, egui::Id::new(("tunnel_row", i)), egui::Sense::click());
        if ed.sel == Some(i) {
            ui.painter().rect_filled(r, 0.0, rgb(C_SEL));
        } else if resp.hovered() {
            ui.painter().rect_filled(r, 0.0, rgb(0x2A2A2A));
        }
        ui.painter().text(
            r.left_center() + egui::vec2(6.0, 0.0),
            egui::Align2::LEFT_CENTER,
            &row.name,
            egui::TextStyle::Button.resolve(ui.style()),
            rgb(C_LABEL),
        );
        if resp.clicked() {
            pick = Some(i);
        }
    }
    if let Some(i) = pick {
        ed.sel = Some(i);
    }

    // 工具行：只有 `+` 与 `−`（视觉基线里就这两个）。
    let strip = egui::Rect::from_min_size(
        o + egui::vec2(LIST_X, TOOL_Y),
        egui::vec2(LIST_W, TOOL_H),
    );
    ui.painter().rect_stroke(strip, 0.0, egui::Stroke::new(1.0, rgb(C_LINE)));
    place(ui, strip.shrink2(egui::vec2(8.0, 0.0)), |ui| {
        ui.horizontal_centered(|ui| {
            ui.spacing_mut().item_spacing.x = 10.0;
            if super::dialog_chrome::tiny_button(ui, "+").on_hover_text("New tunnel").clicked() {
                let name = TunnelProfile::fresh_name(&ed.rows);
                ed.rows.push(TunnelProfile::new(name));
                ed.sel = Some(ed.rows.len() - 1);
            }
            let has = ed.sel.is_some();
            if ui.add_enabled(has, egui::Button::new("-").frame(false)).clicked() {
                if let Some(i) = ed.sel {
                    ed.rows.remove(i);
                    ed.sel = if ed.rows.is_empty() {
                        None
                    } else {
                        Some(i.min(ed.rows.len() - 1))
                    };
                }
            }
        });
    });
}

/// 右栏：六行字段 + `Status` 开关 + 两个勾选框。
fn form(ui: &mut egui::Ui, o: egui::Pos2, ed: &mut Editor) {
    // 金标准 `tunnel-settings-empty.png`：列表为空时右栏仍显示一份出厂占位
    // （Name/Type/Session 空、Listen Port = Dynamic port allocation、Dest Port 0）。
    let mut next = ed.current().cloned().unwrap_or_default();
    if ed.current().is_none() {
        next.name.clear();
        next.session.clear();
        next.dest_host.clear();
        next.listen_port = 0;
        next.dest_port = 0;
    }

    text_row(ui, o, ROW_Y[0], "Name:", &mut next.name);
    kind_row(ui, o, ROW_Y[1], &mut next.kind);
    session_row(ui, o, ROW_Y[2], &mut next.session, ed);
    port_row(ui, o, ROW_Y[3], "Listen Port:", &mut next.listen_port, true);
    text_row(ui, o, ROW_Y[4], "Destination Host:", &mut next.dest_host);
    port_row(ui, o, ROW_Y[5], "Destination Port:", &mut next.dest_port, false);

    label(ui, o, STATUS_Y, "Status:");
    place(
        ui,
        egui::Rect::from_min_size(
            o + egui::vec2(FIELD_X, STATUS_Y),
            egui::vec2(FIELD_W, FIELD_H),
        ),
        |ui| {
            // 31.5×15.5 是兼容终端胶囊的实测尺寸（与 Triggers 那颗同一份量测）。
            super::dialog_chrome::toggle(ui, egui::vec2(31.5, 15.5), &mut next.status, true);
        },
    );

    check_row(ui, o, CHECK_Y[0], "Accept local connections only", &mut next.local_only);
    check_row(ui, o, CHECK_Y[1], "Automatically open at startup", &mut next.auto_open);

    if let Some(i) = ed.sel {
        if let Some(slot) = ed.rows.get_mut(i) {
            if *slot != next {
                *slot = next;
            }
        }
    }
}

fn label(ui: &mut egui::Ui, o: egui::Pos2, y: f32, text: &str) {
    place(
        ui,
        egui::Rect::from_min_size(
            o + egui::vec2(LABEL_X, y + 5.0),
            egui::vec2(FIELD_X - LABEL_X - 4.0, 14.0),
        ),
        |ui| {
            ui.label(egui::RichText::new(text).color(rgb(C_LABEL)));
        },
    );
}

fn field_rect(o: egui::Pos2, y: f32) -> egui::Rect {
    egui::Rect::from_min_size(
        o + egui::vec2(FIELD_X, y),
        egui::vec2(FIELD_W, FIELD_H),
    )
}

fn text_row(ui: &mut egui::Ui, o: egui::Pos2, y: f32, name: &str, value: &mut String) {
    label(ui, o, y, name);
    let rect = field_rect(o, y);
    ui.painter().rect_filled(rect, 0.0, rgb(C_FIELD));
    place(ui, rect.shrink2(egui::vec2(6.0, 3.0)), |ui| {
        ui.add(
            egui::TextEdit::singleline(value)
                .desired_width(f32::INFINITY)
                .frame(false)
                .text_color(rgb(C_TEXT)),
        );
    });
}

/// 数字栏：输入框 + 右端上下两个小箭头（视觉基线那对 ⇅）。
///
/// `dynamic_hint` 为真且值为 0 时，显示灰字 `Dynamic port allocation`
/// ——**只是占位**，不是真值（见 `term_core::tunnel` 模块头的「已知边界」）。
fn port_row(
    ui: &mut egui::Ui,
    o: egui::Pos2,
    y: f32,
    name: &str,
    value: &mut u16,
    dynamic_hint: bool,
) {
    label(ui, o, y, name);
    let rect = field_rect(o, y);
    ui.painter().rect_filled(rect, 0.0, rgb(C_FIELD));
    let text_area = egui::Rect::from_min_max(rect.min, egui::pos2(rect.right() - 18.0, rect.bottom()));
    let mut buf = if *value == 0 { String::new() } else { value.to_string() };
    place(ui, text_area.shrink2(egui::vec2(6.0, 3.0)), |ui| {
        let resp = ui.add(
            egui::TextEdit::singleline(&mut buf)
                .desired_width(f32::INFINITY)
                .frame(false)
                .hint_text(if dynamic_hint { "Dynamic port allocation" } else { "0" })
                .text_color(rgb(C_TEXT)),
        );
        if resp.changed() {
            *value = buf.trim().parse().unwrap_or(0);
        }
    });
    // ⇅：两枚小三角，点一下 ±1（视觉基线是 spinner，不是滚轮）。
    for (k, d) in [(0.0_f32, 1i32), (FIELD_H / 2.0, -1i32)] {
        let r = egui::Rect::from_min_size(
            egui::pos2(rect.right() - 16.0, rect.top() + k),
            egui::vec2(16.0, FIELD_H / 2.0),
        );
        let resp = ui.interact(r, egui::Id::new(("spin", name, k as i32)), egui::Sense::click());
        if resp.clicked() {
            *value = value.saturating_add_signed(d as i16);
        }
        let c = r.center();
        let col = if resp.hovered() { rgb(0xE0E0E0) } else { rgb(0x9A9A9A) };
        // 上箭头往上收、下箭头往下收：两点在基线、一点在尖上。
        let pts = if d > 0 {
            vec![
                egui::pos2(c.x - 3.0, c.y + 1.5),
                egui::pos2(c.x + 3.0, c.y + 1.5),
                egui::pos2(c.x, c.y - 1.5),
            ]
        } else {
            vec![
                egui::pos2(c.x - 3.0, c.y - 1.5),
                egui::pos2(c.x + 3.0, c.y - 1.5),
                egui::pos2(c.x, c.y + 1.5),
            ]
        };
        ui.painter()
            .add(egui::Shape::convex_polygon(pts, col, egui::Stroke::NONE));
    }
}

/// `Type` 下拉：三项，与 `TunnelKind::ALL` 同序。
fn kind_row(ui: &mut egui::Ui, o: egui::Pos2, y: f32, value: &mut TunnelKind) {
    label(ui, o, y, "Type:");
    let rect = field_rect(o, y);
    combo(ui, rect, value.name(), |ui| {
        for k in TunnelKind::ALL {
            if ui.selectable_label(*value == k, k.name()).clicked() {
                *value = k;
                ui.close_menu();
            }
        }
    });
}

/// `Session` 下拉：列出**当前打开着**的会话（隧道借用它们的 SSH 连接）。
fn session_row(ui: &mut egui::Ui, o: egui::Pos2, y: f32, value: &mut String, ed: &Editor) {
    let _ = ed;
    label(ui, o, y, "Session:");
    let rect = field_rect(o, y);
    let shown = value.clone();
    let mut picked: Option<String> = None;
    combo(ui, rect, &shown, |ui| {
        for name in session_names(ui) {
            if ui.selectable_label(shown == name, &name).clicked() {
                picked = Some(name);
                ui.close_menu();
            }
        }
    });
    if let Some(name) = picked {
        *value = name;
    }
}

/// 当前打开着的会话名（下拉候选）。
///
/// 从 `TerminalWindow` 拿不到（这里是自由函数、没有 `win`），所以走
/// egui 的临时态：`draw()` 每帧开头把会话名存进去一次。比把 `win` 传进
/// 每个小函数更省事，也避免借用冲突。
fn session_names(ui: &egui::Ui) -> Vec<String> {
    ui.ctx()
        .data(|d| d.get_temp::<Vec<String>>(egui::Id::new("tunnel_session_names")))
        .unwrap_or_default()
}

/// 一个下拉框：底 `#333333`、右端 `▾`，点开在正下方浮一层。
///
/// 不用 `egui::ComboBox`：它的外框、内边距与箭头都来自主题，与视觉基线这套
/// （无圆角、`#333333` 底、9 点实心三角）对不上；`menu_custom_button` 又要求
/// 自己是一枚按钮、位置由它接管。这里用 `Area` 手工摆，位置就是控件矩形。
fn combo(
    ui: &mut egui::Ui,
    rect: egui::Rect,
    shown: &str,
    items: impl FnOnce(&mut egui::Ui),
) {
    ui.painter().rect_filled(rect, 0.0, rgb(C_FIELD));
    let id = egui::Id::new(("tunnel_combo", rect.left() as i32, rect.top() as i32));
    let resp = ui.interact(rect, id, egui::Sense::click());
    ui.painter().text(
        egui::pos2(rect.left() + 6.0, rect.center().y),
        egui::Align2::LEFT_CENTER,
        shown,
        egui::TextStyle::Button.resolve(ui.style()),
        rgb(C_TEXT),
    );
    let c = egui::pos2(rect.right() - 10.0, rect.center().y);
    ui.painter().add(egui::Shape::convex_polygon(
        vec![
            egui::pos2(c.x - 4.0, c.y - 2.0),
            egui::pos2(c.x + 4.0, c.y - 2.0),
            egui::pos2(c.x, c.y + 2.5),
        ],
        rgb(0x9A9A9A),
        egui::Stroke::NONE,
    ));
    let popup = id.with("popup");
    if resp.clicked() {
        ui.memory_mut(|m| m.toggle_popup(popup));
    }
    if !ui.memory(|m| m.is_popup_open(popup)) {
        return;
    }
    egui::Area::new(popup)
        .order(egui::Order::Foreground)
        .fixed_pos(rect.left_bottom())
        .fade_in(false)
        .show(ui.ctx(), |ui| {
            egui::Frame::menu(ui.style()).show(ui, |ui| {
                ui.set_min_width(rect.width());
                items(ui);
            });
        });
}

fn check_row(ui: &mut egui::Ui, o: egui::Pos2, y: f32, text: &str, on: &mut bool) {
    let box_r = egui::Rect::from_min_size(
        o + egui::vec2(LABEL_X, y),
        egui::vec2(CHECK_BOX, CHECK_BOX),
    );
    let resp = ui.interact(
        box_r.expand(2.0),
        egui::Id::new(("chk", text)),
        egui::Sense::click(),
    );
    if resp.clicked() {
        *on = !*on;
    }
    let p = ui.painter();
    p.rect_stroke(box_r, 0.0, egui::Stroke::new(1.0, rgb(0x555555)));
    if *on {
        p.line_segment(
            [box_r.left_center() + egui::vec2(3.0, 0.0), box_r.center() + egui::vec2(1.0, 3.0)],
            egui::Stroke::new(1.5, rgb(0xCCCCCC)),
        );
        p.line_segment(
            [box_r.center() + egui::vec2(1.0, 3.0), egui::pos2(box_r.right() - 3.0, box_r.top() + 4.0)],
            egui::Stroke::new(1.5, rgb(0xCCCCCC)),
        );
    }
    p.text(
        egui::pos2(box_r.right() + 8.0, box_r.center().y),
        egui::Align2::LEFT_CENTER,
        text,
        egui::TextStyle::Button.resolve(ui.style()),
        rgb(if *on { C_LABEL } else { 0x777777 }),
    );
}

/// 页脚：左侧橙字提示，右侧 `Cancel` / `Ok`。
fn footer(
    ui: &mut egui::Ui,
    o: egui::Pos2,
    ed: &Editor,
    close: &mut bool,
    commit: &mut bool,
) {
    let hint = ed
        .error
        .clone()
        .unwrap_or_else(|| "Tunnels will be refreshed after closing the dialog".to_string());
    place(
        ui,
        egui::Rect::from_min_size(o + egui::vec2(10.0, HINT_Y), egui::vec2(480.0, 14.0)),
        |ui| {
            ui.label(egui::RichText::new(hint).color(rgb(C_HINT)));
        },
    );
    // 两个按钮右对齐：`Ok` 贴右边距（693 = 704 - 11），`Cancel` 在它左边 4.5 处。
    // 尺寸从规格表取，不再本地存一份 BTN_W/BTN_H——改规格时两处不会各走各的。
    let cancel_spec = crate::ui::kit::spec::FooterButton::TunnelsCancel.spec();
    let ok_spec = crate::ui::kit::spec::FooterButton::TunnelsOk.spec();
    let ok_r = egui::Rect::from_min_size(
        o + egui::vec2(DIALOG_W - 11.0 - ok_spec.size.x, FOOT_Y),
        ok_spec.size,
    );
    let cancel_r = egui::Rect::from_min_size(
        egui::pos2(ok_r.left() - 4.5 - cancel_spec.size.x, ok_r.top()),
        cancel_spec.size,
    );
    // 兼容终端这两个按钮与 Triggers 页脚不是同一套：宽 80.5（不是 72/70）、高 24
    // （不是 22），**`Cancel` 那圈描边是亮蓝 `#3EA0EC`**、`Ok` 是 `#424242`，
    // 内部都带一层自上而下的渐变。这些差异全部记在规格表的 Tunnels 变体里，
    // 绘制与另外三套页脚共用 `kit::footer`。
    let cancel_resp = ui.interact(
        cancel_r,
        egui::Id::new(("tunnel_btn", "Cancel")),
        egui::Sense::click(),
    );
    let ok_resp = ui.interact(
        ok_r,
        egui::Id::new(("tunnel_btn", "Ok")),
        egui::Sense::click(),
    );
    crate::ui::kit::footer::draw(
        ui, cancel_r, cancel_r, "Cancel", cancel_spec, cancel_resp.hovered(), true,
    );
    crate::ui::kit::footer::draw(ui, ok_r, ok_r, "Ok", ok_spec, ok_resp.hovered(), true);
    if cancel_resp.clicked() {
        *close = true;
    }
    if ok_resp.clicked() {
        *commit = true;
    }
}

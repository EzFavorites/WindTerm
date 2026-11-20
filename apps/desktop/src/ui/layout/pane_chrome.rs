//! ④-1：分屏时**每片 pane 自己的一套 chrome**（标签条 / 地址栏 / Sysmon）。
//!
//! 单窗格路径完全不走这里——`ui/mod.rs` 的三条全窗 `TopBottomPanel` 原样保留，
//! 所以 12 张整窗基线一个像素都不动。只有 `draw_node` 的 `Layout::Leaf` 分支
//! 在分屏时调 [`draw_pane_leaf`]。
//!
//! # 与 `shell/tab_bar::draw_tabs` 的关系
//!
//! 那边遍历**全局** `self.tabs`；这里遍历 `pane_groups[pane].order`（组内会话），
//! 标签条右端画的是**本 group 的名字**（`A/B/C/D`，见 [`pane_group_name`]）。
//! 绘图原语（会话图标 / `×` / `＋` / `≡` / 活动蓝线）仍从 `shell::tab_bar` 复用，
//! 颜色与线宽只有一个真相源。
//!
//! # 本刀范围
//!
//! 只做「渲染 + 组内点选」：`＋` / `×` / `≡` 的**新建与关闭语义**属 ④-3
//! （ADR-0026 迁移计划），这里只画出来，不派发命令，避免出现半个假按钮。

use crate::ui::layout::{pane_group_name, PaneGroup, PaneId};
use crate::ui::shell::{
    paint_cross, paint_right_tool, paint_tab_icon, ACTIVE_LINE, LINE_H,
    PANE_TOOLS_H, SYSMON_BAR_H, TAB_BAR_H,
};
use crate::ui::*;
use term_app::menu::{TabTool, TAB_TOOLS};
use term_core::session_runtime::SessionId;
use term_core::theme::CHROME_SCALE;

// 与 `shell/tab_bar.rs` 同款几何：两处标签条长在不同地方，但说的是同一句话。
const TAB_PAD: f32 = 10.0;
const ICON_W: f32 = 11.0;
const ICON_GAP: f32 = 6.0;
const CLOSE_GAP: f32 = 14.0;
const CLOSE_W: f32 = 11.0;
const RIGHT_W: f32 = 18.0;
const RIGHT_GAP: f32 = 3.0;
const RIGHT_EDGE: f32 = 2.0;

/// 分屏时画一片 Leaf：自上而下切四段，再把终端内容交给原来的
/// [`TerminalWindow::draw_terminal_area_for`]（同一份渲染实现，不重写终端）。
pub(in crate::ui) fn draw_pane_leaf(
    win: &mut TerminalWindow,
    ui: &mut egui::Ui,
    pane: PaneId,
    group: &PaneGroup,
    rect: egui::Rect,
) {
    // 矮窗格保底：三段 chrome 先按可用高度收缩，终端至少留一行高。
    let chrome = TAB_BAR_H + PANE_TOOLS_H + SYSMON_BAR_H;
    let term_h = (rect.height() - chrome).max(16.0);
    let scale = if rect.height() < chrome + 16.0 {
        (rect.height() - 16.0).max(0.0) / chrome
    } else {
        1.0
    };
    let tab_h = TAB_BAR_H * scale;
    let tools_h = PANE_TOOLS_H * scale;
    let sysmon_h = SYSMON_BAR_H * scale;
    let tab_rect = egui::Rect::from_min_size(rect.min, egui::vec2(rect.width(), tab_h));
    let tools_rect = egui::Rect::from_min_size(
        egui::pos2(rect.left(), rect.min.y + tab_h),
        egui::vec2(rect.width(), tools_h),
    );
    let term_rect = egui::Rect::from_min_size(
        egui::pos2(rect.left(), rect.min.y + tab_h + tools_h),
        egui::vec2(rect.width(), term_h),
    );
    let sysmon_rect = egui::Rect::from_min_size(
        egui::pos2(rect.left(), rect.max.y - sysmon_h),
        egui::vec2(rect.width(), sysmon_h),
    );

    {
        let mut top = ui.new_child(egui::UiBuilder::new().max_rect(tab_rect));
        draw_pane_tab_strip(win, &mut top, pane, group);
    }
    {
        let mut tools = ui.new_child(egui::UiBuilder::new().max_rect(tools_rect));
        draw_pane_address_bar(win, &mut tools, pane, group.active);
    }
    {
        let mut term = ui.new_child(egui::UiBuilder::new().max_rect(term_rect));
        win.draw_terminal_area_for(&mut term, group.active);
    }
    {
        let mut sysmon = ui.new_child(egui::UiBuilder::new().max_rect(sysmon_rect));
        win.draw_sysmon_bar(&mut sysmon, Some(pane.0));
    }
    // 父 Ui 必须知道整片被占掉了，否则后续内容会叠上来（与 Split 分支同款）。
    ui.allocate_rect(rect, egui::Sense::hover());
}

/// 一片 pane 的标签条：遍历本组的 `order`，活动会话画顶部蓝线，右端画
/// **本 group 的名字** + ＋ + ≡。
fn draw_pane_tab_strip(
    win: &mut TerminalWindow,
    ui: &mut egui::Ui,
    pane: PaneId,
    group: &PaneGroup,
) {
    let band = ui.max_rect();
    let font = egui::TextStyle::Button.resolve(ui.style());
    let text_c = crate::theme::chrome(CHROME_SCALE.text);
    let strong_c = crate::theme::chrome(CHROME_SCALE.text_strong);
    let hover_c = crate::theme::chrome(CHROME_SCALE.hover);

    // 先收成拥有所有权的值：循环里还要改 `win.pane_groups` / `win.active`（E0502）。
    let labels: Vec<(SessionId, String)> = group
        .order
        .iter()
        .filter_map(|sid| {
            let tab = win.tabs.iter().find(|t| t.identity == *sid)?;
            let title = if tab.is_live() {
                if let Some(status) = tab.status().exit_status {
                    format!("{} [{}]", tab.config.name, status)
                } else if !tab.snapshot.title.is_empty() {
                    tab.snapshot.title.clone()
                } else {
                    tab.config.name.clone()
                }
            } else {
                format!("{}{}", tab.config.name, win.tr(" (disconnected)"))
            };
            Some((*sid, title))
        })
        .collect();

    // 交互攒到画完再落地：绘制期间 `win` 与 `ui` 的借用要分开（E0502）。
    let mut close_in_pane: Option<SessionId> = None;
    let mut new_in_pane = false;
    let mut pick_in_pane: Option<SessionId> = None;
    let mut x = band.left();
    for (sid, title) in &labels {
        let is_active = *sid == group.active;
        let g = ui.painter().layout_no_wrap(title.clone(), font.clone(), strong_c);
        let w = TAB_PAD + ICON_W + ICON_GAP + g.size().x + CLOSE_GAP + CLOSE_W + TAB_PAD;
        let rect = egui::Rect::from_min_size(
            egui::pos2(x, band.top()),
            egui::vec2(w, band.height()),
        );
        let cy = rect.center().y;
        {
            let p = ui.painter();
            if is_active {
                p.rect_filled(
                    egui::Rect::from_min_size(rect.min, egui::vec2(w, LINE_H)),
                    0.0,
                    ACTIVE_LINE,
                );
            }
            paint_tab_icon(p, egui::pos2(rect.left() + TAB_PAD + ICON_W / 2.0, cy));
            p.galley(
                egui::pos2(rect.left() + TAB_PAD + ICON_W + ICON_GAP, cy - g.size().y / 2.0),
                g,
                if is_active { strong_c } else { text_c },
            );
            let close_cx = rect.right() - TAB_PAD - CLOSE_W / 2.0;
            paint_cross(p, egui::pos2(close_cx, cy), text_c);
        }
        // 点组内 tab：只换**这一片 pane** 的活动会话，并同步全局焦点。
        let resp = ui.interact(rect, egui::Id::new(("pane_tab", pane.0, sid)), egui::Sense::click());
        if resp.clicked() {
            if let Some(g) = win.pane_groups.get_mut(&pane) {
                g.active = *sid;
            }
            win.active = Some(*sid);
        }
        // `×`：关掉**本组内**这一条（不是整片窗格）。先注册整块再注册 ×，
        // 后注册的命中优先，与全窗标签条同一套规则。
        let close_cx = rect.right() - TAB_PAD - CLOSE_W / 2.0;
        let close_rect = egui::Rect::from_center_size(
            egui::pos2(close_cx, cy),
            egui::vec2(CLOSE_W + 6.0, rect.height()),
        );
        let close_resp = ui.interact(
            close_rect,
            egui::Id::new(("pane_tab_close", pane.0, sid)),
            egui::Sense::click(),
        );
        if close_resp.clicked() {
            close_in_pane = Some(*sid);
        }
        x += w;
    }
    if let Some(sid) = close_in_pane {
        win.close_tab_in_pane(pane, sid);
    }

    // 右端三件：第一件是 **group 名字**（逐 pane 不同），后两件复用绘图原语。
    // 视觉顺序从左到右，右对齐摆放故 `.rev()`。
    // `＋` 在本组开新会话、`≡` 只列本组会话（用户拍板 ④：≡ 不列全局）。
    let group_name = pane_group_name(pane);
    let mut right = band.right() - RIGHT_EDGE;
    for tool in TAB_TOOLS.iter().rev() {
        let rect = egui::Rect::from_min_size(
            egui::pos2(right - RIGHT_W, band.top()),
            egui::vec2(RIGHT_W, band.height()),
        );
        let clickable = *tool != TabTool::GroupName;
        let sense = if clickable {
            egui::Sense::click()
        } else {
            egui::Sense::hover()
        };
        let resp = ui.interact(
            rect,
            egui::Id::new(("pane_tool", pane.0, *tool)),
            sense,
        );
        {
            let p = ui.painter();
            if resp.hovered() {
                p.rect_filled(rect.shrink2(egui::vec2(0.0, 3.0)), 2.0, hover_c);
            }
            if *tool == TabTool::GroupName {
                p.text(
                    rect.center(),
                    egui::Align2::CENTER_CENTER,
                    &group_name,
                    font.clone(),
                    text_c,
                );
            } else {
                paint_right_tool(p, rect, *tool, text_c);
            }
        }
        if *tool == TabTool::NewTab && resp.clicked() {
            new_in_pane = true;
        }
        if *tool == TabTool::TabList {
            let mut picked: Option<SessionId> = None;
            ui.scope_builder(egui::UiBuilder::new().max_rect(rect), |ui| {
                egui::menu::menu_custom_button(
                    ui,
                    egui::Button::new("").frame(false).min_size(rect.size()),
                    |ui| {
                        for (sid, title) in &labels {
                            if ui
                                .selectable_label(*sid == group.active, title.as_str())
                                .clicked()
                            {
                                picked = Some(*sid);
                                ui.close_menu();
                            }
                        }
                    },
                );
            });
            if picked.is_some() {
                pick_in_pane = picked;
            }
        }
        right -= RIGHT_W + RIGHT_GAP;
    }
    if new_in_pane {
        win.new_tab_in_pane(pane);
    }
    if let Some(sid) = pick_in_pane {
        if let Some(g) = win.pane_groups.get_mut(&pane) {
            g.active = sid;
        }
        win.active = Some(sid);
    }
}

/// 一片 pane 的地址栏：**复用整条工具栏**（抓手 + 图标组 + 面包屑 + 右端控件），
/// 只把面包屑的会话来源换成本 pane 的活动会话、`egui` id 带上 pane 身份。
///
/// 2026-11-15 修正：上一版这里只手写了面包屑文字，导致分屏时「工具栏和地址栏
/// 都不见了」——那是把 `tool_bar::draw` 的整套绘制漏掉了，不是方案如此。
fn draw_pane_address_bar(win: &mut TerminalWindow, ui: &mut egui::Ui, pane: PaneId, session: SessionId) {
    crate::ui::shell::draw_tool_bar(
        win,
        ui,
        crate::ui::shell::BarScope {
            session,
            salt: Some(pane.0),
            float: false,
        },
    );
}

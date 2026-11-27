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
    paint_cross, paint_right_tool, paint_tab_icon, tab_number_prefix, ACTIVE_LINE, LINE_H,
    PANE_TOOLS_H, TAB_BAR_H, TAB_TOOLBAR_FILTER_H, TAB_TOOLBAR_H,
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

/// 一片 Leaf 的四段矩形。纯函数，便于断言专注模式确实压掉了全部 chrome。
///
/// `focus = true` 时终端吃满整片窗格，tab 条 / 工具行 / Sysmon 三段全是 `None`：
/// 专注模式对**每一片**分屏窗格都生效，不是只压单窗格那条全窗顶栏。
struct LeafRects {
    tab: Option<egui::Rect>,
    tools: Option<egui::Rect>,
    term: egui::Rect,
    toolbar: Option<egui::Rect>,
}

fn leaf_rects(rect: egui::Rect, focus: bool, toolbar_h_in: f32) -> LeafRects {
    if focus {
        return LeafRects {
            tab: None,
            tools: None,
            term: rect,
            toolbar: None,
        };
    }
    // 工具条高度只允许两档：收起 [`TAB_TOOLBAR_H`]、过滤激活 [`TAB_TOOLBAR_FILTER_H`]。
    // 夹在这里，调用方多传一个别的值也不会把这条工具条画成第三种高度。
    let toolbar_h_in = toolbar_h_in.clamp(TAB_TOOLBAR_H, TAB_TOOLBAR_FILTER_H);
    // 矮窗格保底：三段 chrome 先按可用高度收缩，终端至少留一行高。
    let chrome = TAB_BAR_H + PANE_TOOLS_H + toolbar_h_in;
    let term_h = (rect.height() - chrome).max(16.0);
    let scale = if rect.height() < chrome + 16.0 {
        (rect.height() - 16.0).max(0.0) / chrome
    } else {
        1.0
    };
    let tab_h = TAB_BAR_H * scale;
    let tools_h = PANE_TOOLS_H * scale;
    let toolbar_h = toolbar_h_in * scale;
    let tab_rect = egui::Rect::from_min_size(rect.min, egui::vec2(rect.width(), tab_h));
    let tools_rect = egui::Rect::from_min_size(
        egui::pos2(rect.left(), rect.min.y + tab_h),
        egui::vec2(rect.width(), tools_h),
    );
    let term_rect = egui::Rect::from_min_size(
        egui::pos2(rect.left(), rect.min.y + tab_h + tools_h),
        egui::vec2(rect.width(), term_h),
    );
    let toolbar_rect = egui::Rect::from_min_size(
        egui::pos2(rect.left(), rect.max.y - toolbar_h),
        egui::vec2(rect.width(), toolbar_h),
    );
    LeafRects {
        tab: Some(tab_rect),
        tools: Some(tools_rect),
        term: term_rect,
        toolbar: Some(toolbar_rect),
    }
}

/// 分屏时画一片 Leaf：自上而下切四段，再把终端内容交给原来的
/// [`TerminalWindow::draw_terminal_area_for`]（同一份渲染实现，不重写终端）。
///
/// 专注模式下只画终端段：分屏窗格里不再出现 tab 条 / 工具行 / Sysmon。
pub(in crate::ui) fn draw_pane_leaf(
    win: &mut TerminalWindow,
    ui: &mut egui::Ui,
    pane: PaneId,
    group: &PaneGroup,
    rect: egui::Rect,
) {
    let parts = leaf_rects(
        rect,
        win.view.focus_mode,
        win.tab_toolbar_height(Some(group.active)),
    );
    if let Some(tab_rect) = parts.tab {
        let mut top = ui.new_child(egui::UiBuilder::new().max_rect(tab_rect));
        draw_pane_tab_strip(win, &mut top, pane, group);
    }
    if let Some(tools_rect) = parts.tools {
        let mut tools = ui.new_child(egui::UiBuilder::new().max_rect(tools_rect));
        draw_pane_address_bar(win, &mut tools, pane, group.active);
    }
    {
        let mut term = ui.new_child(egui::UiBuilder::new().max_rect(parts.term));
        win.draw_terminal_area_for(&mut term, group.active);
    }
    if let Some(toolbar_rect) = parts.toolbar {
        let mut toolbar = ui.new_child(egui::UiBuilder::new().max_rect(toolbar_rect));
        win.draw_tab_toolbar(&mut toolbar, Some(pane.0), Some(group.active));
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
        .enumerate()
        .filter_map(|(i, sid)| {
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
            // 编号按**本组内**的位置算：分屏时每片窗格的标签条各数各的，
            // 与全窗标签条按全局位置编号不冲突。
            Some((
                *sid,
                format!("{}{}", tab_number_prefix(win.tab_numbers, i), title),
            ))
        })
        .collect();

    // 交互攒到画完再落地：绘制期间 `win` 与 `ui` 的借用要分开（E0502）。
    let mut close_in_pane: Option<SessionId> = None;
    let mut new_in_pane = false;
    let mut pick_in_pane: Option<SessionId> = None;
    let mut x = band.left();
    for (sid, title) in &labels {
        let is_active = *sid == group.active;
        let g = ui
            .painter()
            .layout_no_wrap(title.clone(), font.clone(), strong_c);
        let w = TAB_PAD + ICON_W + ICON_GAP + g.size().x + CLOSE_GAP + CLOSE_W + TAB_PAD;
        let rect =
            egui::Rect::from_min_size(egui::pos2(x, band.top()), egui::vec2(w, band.height()));
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
                egui::pos2(
                    rect.left() + TAB_PAD + ICON_W + ICON_GAP,
                    cy - g.size().y / 2.0,
                ),
                g,
                if is_active { strong_c } else { text_c },
            );
            let close_cx = rect.right() - TAB_PAD - CLOSE_W / 2.0;
            paint_cross(p, egui::pos2(close_cx, cy), text_c);
        }
        // 点组内 tab：只换**这一片 pane** 的活动会话，并同步全局焦点。
        let resp = ui.interact(
            rect,
            egui::Id::new(("pane_tab", pane.0, sid)),
            egui::Sense::click(),
        );
        if resp.clicked() {
            if let Some(g) = win.pane_groups.get_mut(&pane) {
                g.active = *sid;
            }
            win.active = Some(*sid);
        }
        // 中键 / 右键的偏好与全窗标签条同一份。分屏标签条没有右键菜单，
        // 所以「标签菜单」这一档在这里等价于没有动作（原本就是这样）。
        if win.tab_middle_click == 1 && resp.middle_clicked() {
            close_in_pane = Some(*sid);
        } else if win.tab_right_click == 1 && resp.secondary_clicked() {
            close_in_pane = Some(*sid);
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
        let resp = ui.interact(rect, egui::Id::new(("pane_tool", pane.0, *tool)), sense);
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
fn draw_pane_address_bar(
    win: &mut TerminalWindow,
    ui: &mut egui::Ui,
    pane: PaneId,
    session: SessionId,
) {
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

#[cfg(test)]
mod tests {
    use super::*;

    fn leaf_rect() -> egui::Rect {
        egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(640.0, 400.0))
    }

    #[test]
    fn non_focus_leaf_keeps_all_three_chrome_segments() {
        let r = leaf_rect();
        let parts = leaf_rects(r, false, TAB_TOOLBAR_H);
        assert!(parts.tab.is_some(), "tab 条不能丢");
        assert!(parts.tools.is_some(), "工具行 / 地址栏不能丢");
        assert!(parts.toolbar.is_some(), "tab 工具条不能丢");
        assert_eq!(parts.term.top(), r.top() + TAB_BAR_H + PANE_TOOLS_H);
        assert_eq!(parts.toolbar.unwrap().bottom(), r.bottom());
    }

    /// 分屏每片按**自己那条会话**的工具条高度切段：过滤激活的那片终端更矮，
    /// 收起的那片一个像素都不变。
    #[test]
    fn split_leaf_sizes_its_terminal_by_its_own_toolbar_height() {
        let r = leaf_rect();
        let plain = leaf_rects(r, false, TAB_TOOLBAR_H);
        let filtered = leaf_rects(r, false, TAB_TOOLBAR_FILTER_H);
        assert_eq!(
            plain.term.width(),
            filtered.term.width(),
            "宽度不受影响"
        );
        assert!(
            filtered.term.height() + TAB_TOOLBAR_FILTER_H
                == plain.term.height() + TAB_TOOLBAR_H,
            "过滤态吃掉的正是多出来的 18pt"
        );
        assert_eq!(plain.toolbar.unwrap().height(), TAB_TOOLBAR_H);
        assert_eq!(filtered.toolbar.unwrap().height(), TAB_TOOLBAR_FILTER_H);
        assert_eq!(filtered.toolbar.unwrap().bottom(), r.bottom());
    }

    /// 专注模式压掉的不只是单窗格那条全窗顶栏——**每一片分屏窗格**也只剩终端。
    /// 这段按 `win.view.focus_mode` 收口，几何层先断言，绘制层再断言一次。
    #[test]
    fn focus_leaf_hides_every_chrome_segment() {
        let r = leaf_rect();
        let parts = leaf_rects(r, true, TAB_TOOLBAR_H);
        assert!(parts.tab.is_none());
        assert!(parts.tools.is_none());
        assert!(parts.toolbar.is_none());
        assert_eq!(parts.term, r, "终端吃满整片窗格");
    }

    /// 真跑两帧绘制（首帧 `Area` 是 sizing pass，形状是 `Noop`，见 `address_tests`）。
    /// 同一片窗格：专注态形状数必须严格少于常态，否则 chrome 只是「没画在预期位置」
    /// 而不是真的被压掉。
    fn pane_shapes(focus: bool) -> usize {
        let ctx = egui::Context::default();
        let pool = std::sync::Arc::new(term_session::SessionPool::new());
        let mut win = crate::ui::lifecycle_test_support::window_with(pool, &ctx, Vec::new());
        win.view.focus_mode = focus;
        let pane = crate::ui::layout::mint_pane_identity();
        let group = PaneGroup {
            order: Vec::new(),
            active: 0,
        };
        let mut shapes = 0;
        for _ in 0..2 {
            let input = egui::RawInput {
                screen_rect: Some(leaf_rect()),
                ..Default::default()
            };
            let out = ctx.run(input, |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    draw_pane_leaf(&mut win, ui, pane, &group, ui.available_rect_before_wrap());
                });
            });
            shapes = out.shapes.len();
        }
        shapes
    }

    #[test]
    fn focus_mode_split_pane_renders_without_chrome() {
        let normal = pane_shapes(false);
        let focused = pane_shapes(true);
        assert!(
            focused < normal,
            "专注态 {focused} 形状应少于常态 {normal}：tab 条 / 工具行 / Sysmon 都不该画"
        );
    }
}

//! 统一 Dock 容器：**五个位置的壳只在这一处定义**。
//!
//! （2026-11-16）：把三套容器 chrome 逐字搬进本模块——
//! 左右四块的标题/Filter（`dock_panel_chrome`）、右 Sidebar 的标题/Filter
//! （`sidebar_view`）、底带的标签条（`dock_bottom`）。原调用点通过 `impl
//! TerminalWindow` 方法名直接命中这里，签名与画法一字未改，**像素零变化**。
//!
//! 为什么要搬：三套实现里同一块几何各写一遍，× 的位置因此出现 `right-12`
//! 与 `right-10` 两种、笔法出现文字与手绘两种（见 `2026-11-16-dock-containers-audit.md`）。
//! 搬进同一文件是让这些差异**并排可见**，不是顺手统一——统一会改像素，要按
//! 用户逐页提供的视觉基线定。
//!
//! 遵守 ADR-0021：函数式原语、`Props` 只读、交互以返回值交回调用方。
//! 本阶段先落契约类型与 chrome 平移；`draw_container` 接上五个位置。

use crate::ui::*;
use eframe::egui;
use term_app::action::Action;
use term_app::dock::{DockZone, PanelId};
use term_app::shell::ShellAction;
use term_app::i18n::Lang;
use term_core::theme::CHROME_SCALE;

use super::dock_bottom::{dot_color, Cmd, BTN_W, DOT_GAP, DOT_W, EDGE_PAD, TAB_H, TAB_PAD_L};
use super::dock_bottom_menu::{
    btn_center, dock_menu, icon_button, menu_id, paint_chevron, GearMenu,
};

/// 标题带高度：`dock_panel_chrome` 与 `sidebar_view` 的同一个实测值。
pub(in crate::ui) const TITLE_H: f32 = 21.0;
/// 筛选行高度（含下沿 1px 分隔线）。
pub(in crate::ui) const FILTER_H: f32 = 29.0;

// 2026-11-17：这里的 `×` 原先**排字体**（`FontId::proportional(13.0)` 画 `×`
// 字形，还配了一段按 `mesh_bounds` 反推墨迹中心的兜底）。它和标签条的几何画法
// 量出来差半个点，「齿轮与叉同心」那件事也只能靠反推凑。现在统一走
// `shell::glyph::paint_cross`：几何中心就是盒子中心，那段兜底一起删掉。
use crate::ui::shell::{paint_cross, paint_gear};

/// 哪些带内面板有 Filter 行。Filer 用路径栏、Session 自带一条，都不在这里。
pub(in crate::ui) fn has_filter(id: PanelId) -> bool {
    matches!(id, PanelId::Explorer | PanelId::Outline)
}

/// 这块面板的 chrome 占多高。父 `Ui` 要用这个矩形推进光标，否则
/// `new_child` 不占空间、内容会与标题重叠（2026-11-08 已踩过一次）。
pub(in crate::ui) fn chrome_height(id: PanelId) -> f32 {
    if has_filter(id) {
        TITLE_H + FILTER_H
    } else {
        TITLE_H
    }
}

/// 两个筛选框的跨帧缓冲。放在一个结构里，`TerminalWindow` 只多一个字段。
#[derive(Default)]
pub(in crate::ui) struct PanelFilters {
    explorer: String,
    outline: String,
}

impl PanelFilters {
    fn for_id(&mut self, id: PanelId) -> Option<&mut String> {
        match id {
            PanelId::Explorer => Some(&mut self.explorer),
            PanelId::Outline => Some(&mut self.outline),
            _ => None,
        }
    }

    /// 只读版：绘制内容时要拿当前过滤词。
    pub(in crate::ui) fn get(&self, id: PanelId) -> &str {
        match id {
            PanelId::Explorer => &self.explorer,
            PanelId::Outline => &self.outline,
            _ => "",
        }
    }
}

/// 顶部 chrome 的三种形态。它把五个位置收敛到一个绘制入口。
pub(in crate::ui) enum Header<'a> {
    /// 停靠单块：▪ 标题 … ⚙ ×
    Title { accent: egui::Color32, text: &'a str },
    /// 横向 tab（底带停靠态 / 浮动窗内）
    TabStrip { members: &'a [PanelId], active: PanelId, with_buttons: bool, maximized: bool },
    /// 空容器：只留 ⚙ × 作为「往这里放东西」的入口
    Compact,
}

/// Filter 行的文案与归属。
pub(in crate::ui) struct FilterSpec<'a> {
    pub id: PanelId,
    pub hint: &'a str,
}

/// 一个停靠位置的完整外观。只描述「长什么样」，不持有跨帧状态。
pub(in crate::ui) struct DockProps<'a> {
    pub rect: egui::Rect,
    pub header: Header<'a>,
    pub filter: Option<FilterSpec<'a>>,
    pub draggable: bool,
    pub zone: DockZone,
}

/// 交互结果交回调用方落地，组件不碰 `DockLayout` / `ViewState`。
#[derive(Default)]
pub(in crate::ui) struct ContainerOutcome {
    pub commands: Vec<Cmd>,
    pub drag_started: bool,
    pub tab_picked: Option<PanelId>,
    pub close_clicked: bool,
}

impl TerminalWindow {
    pub(in crate::ui) fn dock_panel_title(
        &mut self,
        ui: &mut egui::Ui,
        id: PanelId,
        zone: DockZone,
        band: &[PanelId],
        cmds: &mut Vec<super::dock_bottom::Cmd>,
    ) {
        let s = &CHROME_SCALE;
        let text_col = crate::theme::chrome(s.text);
        let band_floating = self.dock.is_band_floating(zone);
        let (rect, _) = ui.allocate_exact_size(
            egui::vec2(ui.available_width(), TITLE_H),
            egui::Sense::hover(),
        );
        // 拿走一份 painter（`ui.painter()` 借的是 `&Ui`，后面弹菜单要 `&mut Ui`）。
        let p = ui.painter().clone();
        let p = &p;
        let cy = rect.center().y;
        p.rect_filled(
            egui::Rect::from_center_size(
                egui::pos2(rect.left() + 12.0, cy),
                egui::vec2(6.0, 6.0),
            ),
            1.0,
            super::dock_bottom::dot_color(id),
        );
        p.text(
            egui::pos2(rect.left() + 22.5, cy),
            egui::Align2::LEFT_CENTER,
            self.tr(id.title()),
            crate::ui::type_scale::body(),
            text_col,
        );

        // 齿轮：与 Session 面板同一条动作，位置也取同一个实测值（距右沿 29.5）。
        let gear = egui::Rect::from_center_size(
            egui::pos2(rect.right() - 29.5, cy),
            egui::vec2(18.0, 18.0),
        );
        // id 里必须带 `zone`：同一个面板可能同时出现在两条带上（一条带是它的
        // 停靠位，另一条只剩它的空壳），只按面板名派生就会同帧重复登记同一 id，
        // 两处的 ⚙ 共享 hover / click 状态。
        let gresp = ui.interact(
            gear,
            egui::Id::new((id.name(), zone.name(), "panel_gear")),
            egui::Sense::click(),
        );
        // 齿轮与 `×` 现在**只有一份几何**：`shell::glyph`（⚙ 外径 10、× 边长 6.5、
        // 线宽 1.0，与标签条 / Find 栏 / 快捷栏同一套）。2026-11-15 曾试图把这里并到
        // `dock_bottom_menu::paint_gear`（当时按 `BTN_ICON` 比例算：环 3.0、齿宽 1.0、
        // 齿顶 5.0），结果齿宽从 1.7 掉到 1.0、环从 1.2 掉到 1.0，视觉上细了一圈——
        // 那是**观感改变**，不该以「删掉重复实现」的名义顺带发生。这次先把视觉基线的
        // 尺寸量出来（面板标题带 ⚙ 列 363..382 @2× = 10.0 逻辑点）再改，两处才都是
        // 「按视觉基线」而不是「按另一处实现」。
        paint_gear(
            p,
            gear.center(),
            if gresp.hovered() {
                crate::theme::chrome(s.text_strong)
            } else {
                crate::theme::chrome(s.icon)
            },
        );
        let gear_menu = super::dock_bottom_menu::GearMenu {
            zone,
            band,
            active: id,
            band_floating,
            in_float_window: false,
        };
        if gresp.clicked() {
            ui.memory_mut(|m| m.toggle_popup(super::dock_bottom_menu::menu_id(&gear_menu)));
        }
        let _ = gresp.clone().on_hover_text(self.tr("Panel menu"));
        super::dock_bottom_menu::dock_menu(ui, &gresp, gear_menu, cmds, self.lang());

        let close = egui::Rect::from_center_size(
            egui::pos2(rect.right() - 12.0, cy),
            egui::vec2(18.0, 18.0),
        );
        let resp = ui.interact(
            close,
            egui::Id::new((id.name(), zone.name(), "dock_close")),
            egui::Sense::click(),
        );
        if resp.hovered() {
            p.rect_filled(close, 3.0, crate::theme::chrome(s.hover));
        }
        paint_cross(p, close.center(), text_col);
        if resp.clicked() {
            self.dispatch_action(Action::Shell(ShellAction::TogglePanel(id)));
        }
    }

    /// 空容器的壳：只画 ⚙ 与 ×，没有色块 / 标题 / Filter。
    /// 兼容终端「空容器 + 全不勾菜单」那张图就是这个形态——它保留一个入口让用户
    /// 把面板勾回来，而不是整条带消失、再也找不到入口。
    pub(in crate::ui) fn draw_compact_chrome(
        &mut self,
        ui: &mut egui::Ui,
        id: PanelId,
        zone: DockZone,
        band: &[PanelId],
        cmds: &mut Vec<super::dock_bottom::Cmd>,
    ) {
        let s = &CHROME_SCALE;
        let band_floating = self.dock.is_band_floating(zone);
        let (rect, _) = ui.allocate_exact_size(
            egui::vec2(ui.available_width(), TITLE_H),
            egui::Sense::hover(),
        );
        let p = ui.painter().clone();
        let cy = rect.center().y;
        let gear = egui::Rect::from_center_size(
            egui::pos2(rect.right() - 29.5, cy),
            egui::vec2(18.0, 18.0),
        );
        let gresp = ui.interact(
            gear,
            egui::Id::new((id.name(), zone.name(), "panel_gear")),
            egui::Sense::click(),
        );
        paint_gear(
            &p,
            gear.center(),
            if gresp.hovered() {
                crate::theme::chrome(s.text_strong)
            } else {
                crate::theme::chrome(s.icon)
            },
        );
        let menu = super::dock_bottom_menu::GearMenu { zone, band, active: id, band_floating, in_float_window: false };
        if gresp.clicked() {
            ui.memory_mut(|m| m.toggle_popup(super::dock_bottom_menu::menu_id(&menu)));
        }
        let _ = gresp.clone().on_hover_text(self.tr("Panel menu"));
        super::dock_bottom_menu::dock_menu(ui, &gresp, menu, cmds, self.lang());

        let close = egui::Rect::from_center_size(
            egui::pos2(rect.right() - 12.0, cy),
            egui::vec2(18.0, 18.0),
        );
        let resp = ui.interact(
            close,
            egui::Id::new((id.name(), zone.name(), "dock_close")),
            egui::Sense::click(),
        );
        if resp.hovered() {
            p.rect_filled(close, 3.0, crate::theme::chrome(s.hover));
        }
        let text_col = crate::theme::chrome(s.text);
        paint_cross(&p, close.center(), text_col);
        if resp.clicked() {
            self.dispatch_action(Action::Shell(ShellAction::TogglePanel(id)));
        }
    }

    pub(in crate::ui) fn draw_panel_filter(&mut self, ui: &mut egui::Ui, id: PanelId) {
        if !has_filter(id) {
            return;
        }
        let Some(buf) = self.panel_filters.for_id(id) else {
            return;
        };
        draw_filter(ui, id, 12.0, self.view.lang, buf);
    }

    pub(in crate::ui) fn panel_filter(&self, id: PanelId) -> &str {
        self.panel_filters.get(id)
    }

    #[allow(clippy::too_many_arguments)]
    pub(in crate::ui) fn tab_strip(
        &mut self,
        ui: &mut egui::Ui,
        col: egui::Rect,
        group: &[PanelId],
        band: &[PanelId],
        active: PanelId,
        with_buttons: bool,
        maximized: bool,
        cmds: &mut Vec<Cmd>,
        zone: DockZone,
        band_floating: bool,
        show_maximize: bool,
        in_float_window: bool,
    ) -> Option<PanelId> {
        let strip = egui::Rect::from_min_size(col.min, egui::vec2(col.width(), TAB_H));
        let p = ui.painter_at(strip);
        p.rect_filled(strip, 0.0, crate::theme::chrome(CHROME_SCALE.dialog));

        // 右端三颗：只有最右那组画（它们是整条带的控制，不是某一组的）。
        let btns_left = if with_buttons {
            strip.right() - EDGE_PAD - BTN_W * 3.0
        } else {
            strip.right()
        };
        let font = egui::TextStyle::Button.resolve(ui.style());
        let mut x = strip.left();
        let mut picked = None;

        for id in group {
            let w = self.tab_width(ui, *id);
            if x + w > btns_left {
                break;
            }
            let r = egui::Rect::from_min_size(egui::pos2(x, strip.top()), egui::vec2(w, TAB_H));
            let resp = ui.interact(r, egui::Id::new(("dock_tab", id.name())), egui::Sense::click());
            let on = *id == active;
            if on {
                p.rect_filled(r, 0.0, crate::theme::chrome(CHROME_SCALE.tab));
            } else if resp.hovered() {
                p.rect_filled(r, 0.0, crate::theme::chrome(CHROME_SCALE.hover));
            }
            let dot = egui::Rect::from_center_size(
                egui::pos2(r.left() + TAB_PAD_L + DOT_W / 2.0, r.center().y),
                egui::vec2(DOT_W, DOT_W),
            );
            p.rect_filled(dot, 0.0, dot_color(*id));
            let text_c = crate::theme::chrome(if on {
                CHROME_SCALE.text_strong
            } else {
                CHROME_SCALE.text
            });
            let g = p.layout_no_wrap(self.tr(id.title()).to_string(), font.clone(), text_c);
            p.galley(
                egui::pos2(
                    r.left() + TAB_PAD_L + DOT_W + DOT_GAP,
                    r.center().y - g.size().y / 2.0,
                ),
                g,
                text_c,
            );
            if resp.clicked() {
                picked = Some(*id);
            }
            x = r.right();
        }

        if with_buttons {
            let icon = crate::theme::chrome(CHROME_SCALE.icon);
            let mut bx = btns_left + EDGE_PAD;

            // ^ / v：向上展开占满 / 还原（兼容终端方向：点 ^ 是展开占满，不是收起）。
            // 浮动窗里没有这颗：窗口自己就是「展开态」，原图浮窗右端只有 ⚙ ×。
            if show_maximize {
                if icon_button(ui, strip.top(), bx, egui::Id::new((zone.name(), "dock_collapse"))).clicked() {
                    cmds.push(Cmd::Maximize);
                }
                paint_chevron(&p, btn_center(bx, strip.top()), !maximized, icon);
                bx += BTN_W;
            }

            // ⚙：菜单。每颗齿轮独立 popup id（见 `menu_id`），避免同帧互关。
            let gear = icon_button(ui, strip.top(), bx, egui::Id::new((zone.name(), "dock_gear")));
            paint_gear(&p, btn_center(bx, strip.top()), icon);
            let gear_menu = GearMenu {
                zone,
                band,
                active,
                band_floating,
                in_float_window,
            };
            if gear.clicked() {
                ui.memory_mut(|m| m.toggle_popup(menu_id(&gear_menu)));
            }
            dock_menu(ui, &gear, gear_menu, cmds, self.lang());
            bx += BTN_W;

            // ×：只关**当前标签**，不是整条带——底带是多功能 tab 切换，关一个 tab
            // 不该把同带的其它 tab 一起带走。最后一个也被关掉时，下一帧
            // `visible_members` 为空、`draw_bottom_dock` 提前 return，整条带自然消失
            // （即「除非最后一个也关闭了，才关闭面板」）。
            if icon_button(ui, strip.top(), bx, egui::Id::new((zone.name(), "dock_close"))).clicked() {
                cmds.push(Cmd::Toggle(active));
            }
            paint_cross(&p, btn_center(bx, strip.top()), icon);
        }
        picked
    }

}

/// Filter 行的共用画法。跨帧缓冲由调用方传入：Explorer / Outline 用
/// `PanelFilters`，Sidebar 用 `session_filter`；几何与配色只在这里定义一次。
pub(in crate::ui) fn draw_filter(
    ui: &mut egui::Ui,
    id: PanelId,
    font_size: f32,
    lang: Lang,
    buf: &mut String,
) {
    let s = &CHROME_SCALE;
    let text_col = crate::theme::chrome(s.text);
    let hint = egui::RichText::new(t(lang, "Filter")).color(crate::theme::chrome(s.hint));
    let (rect, _) = ui.allocate_exact_size(
        egui::vec2(ui.available_width(), FILTER_H),
        egui::Sense::hover(),
    );
    let edit = egui::Rect::from_min_max(
        egui::pos2(rect.left() + 6.0, rect.top() + 5.2),
        egui::pos2(rect.right() - 6.0, rect.bottom()),
    );
    ui.put(
        edit,
        egui::TextEdit::singleline(buf)
            .id(egui::Id::new((id.name(), "panel_filter")))
            .hint_text(hint)
            .frame(false)
            .margin(egui::Margin::symmetric(0.0, 5.0))
            .font(crate::ui::type_scale::font(font_size))
            .text_color(text_col)
            .desired_width(edit.width()),
    );
    ui.painter().line_segment(
        [
            egui::pos2(rect.left() + 2.0, rect.bottom() - 0.5),
            egui::pos2(rect.right() - 4.0, rect.bottom() - 0.5),
        ],
        egui::Stroke::new(1.0, crate::theme::chrome(s.border)),
    );
}

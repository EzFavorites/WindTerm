//! 整条带浮起来之后，**窗内**画什么。
//!
//! 窗口本身（系统外框、关闭归位、坐标换算）在 `dock_native.rs`；这里只管
//! 「一块给定尺寸的画布里，怎么把一条带的成员摆出来」——横向 tab 切成员、
//! 正文、底带的横向滚动兜底。内嵌回退与原生视口共用这一个函数，两条路径
//! 因此不可能画出两种样子。
//!
//! 成员在 `DockLayout` 里仍是 `Dock { zone, order }`，`zone()` 对浮动带返回空；
//! 这条分工让空壳与原位次恢复都不需要额外状态。

use crate::ui::*;
use eframe::egui;
use term_app::dock::{DockZone, PanelId};

/// Sender 工具行所需的最小内容宽度。按 `tools_sender_row` 的逐项量测：
/// 5 颗图标 ≈ 136 + 两组单选 ≈ 190 + Count/Interval ≈ 250 + Targets ≈ 285，
/// 再加间隙共约 880pt。浮窗较窄时给正文一个横向滚动兜底，控件不被裁掉。
const BOTTOM_MIN_CONTENT_W: f32 = 920.0;

impl TerminalWindow {
    /// 画一条带在浮窗里的全部内容（tab 条 + Filter + 正文）。
    ///
    /// 成员表由调用方给：调用方已经按可见性筛过一遍，且要用它取窗口标题。
    pub(super) fn draw_band_content(
        &mut self,
        ui: &mut egui::Ui,
        zone: DockZone,
        members: &[PanelId],
    ) {
        let Some(active) = self.band_active(zone, members) else {
            return;
        };
        let mut cmds: Vec<super::dock_bottom::Cmd> = Vec::new();
        let full = ui.available_rect_before_wrap();
        // 与底带同一条 tab 原语；区别只在右端没有 ^、齿轮说的是这条带自己的
        // 归属。两条带的菜单文案一样，都是 "Float"。
        let picked = self.tab_strip(
            ui,
            full,
            members,
            members,
            active,
            true,
            false,
            &mut cmds,
            zone,
            true,
            false,
            true,
        );
        if let Some(id) = picked {
            self.set_band_tab(zone, id);
        }
        let body = egui::Rect::from_min_max(
            egui::pos2(full.left(), full.top() + super::dock_bottom::TAB_H),
            full.max,
        );
        let mut child = ui.new_child(
            egui::UiBuilder::new()
                .id_salt((zone.name(), "float_band_body"))
                .max_rect(body)
                .layout(egui::Layout::top_down(egui::Align::Min)),
        );
        child.set_clip_rect(body);
        // Sidebar 自己的 Filter 在正文里画；这里只补 Explorer / Outline。
        let mut paint_body = |ui: &mut egui::Ui| {
            self.draw_panel_filter(ui, active);
            if active.scrolls_in_dock() {
                egui::ScrollArea::vertical()
                    .id_salt((zone.name(), active.name()))
                    .show(ui, |ui| self.draw_panel_body(active, ui));
            } else {
                self.draw_panel_body(active, ui);
            }
        };
        if zone == DockZone::Bottom {
            // 底带的 Sender 是一排固定宽的控件：窗口比原带窄时用横向滚动
            // 兜底，而不是把 Targets 那一截悄悄裁掉。
            egui::ScrollArea::horizontal()
                .id_salt((zone.name(), "float_band_hscroll"))
                .show(&mut child, |ui| {
                    ui.set_min_width(BOTTOM_MIN_CONTENT_W);
                    paint_body(ui);
                });
        } else {
            paint_body(&mut child);
        }
        ui.advance_cursor_after_rect(full);
        for cmd in cmds {
            self.apply_dock_cmd(cmd, zone, members);
        }
    }

    /// 这条带里当前看得到（未被关掉）的成员。
    pub(super) fn band_visible_members(&self, zone: DockZone) -> Vec<PanelId> {
        self.dock
            .band_members(zone)
            .into_iter()
            .filter(|id| self.panel_visible(*id))
            .collect()
    }

    /// 活动标签优先取本带的记忆值；成员被关 / 被挪走就退回第一颗。
    pub(super) fn band_active(&self, zone: DockZone, members: &[PanelId]) -> Option<PanelId> {
        members
            .iter()
            .copied()
            .find(|p| Some(*p) == self.band_tab(zone))
            .or_else(|| members.first().copied())
    }

    /// 整带浮窗的初始矩形：**从原停靠带的真实矩形推尺寸**。
    ///
    /// 左右带沿用宽度并夹到 60%；底带**例外**——Sender 那一排控件是定宽的，
    /// 缩窄就会显示不全（用户反馈），所以直接沿用原带整宽（只留一点窗边距），
    /// 宁可窗口大一些。高度竖带夹到 70% 屏高，底带夹到 50%。
    pub(super) fn float_band_rect(&self, zone: DockZone) -> egui::Rect {
        let members = self.dock.band_members(zone);
        let source = self
            .dock_rects
            .iter()
            .filter(|(id, z, _)| *z == zone && members.contains(id))
            .map(|(_, _, r)| *r)
            .reduce(|a, b| a.union(b));
        let bounds = self.ctx.available_rect();
        let raw = source.unwrap_or_else(|| {
            egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(440.0, super::dock_paint::FLOAT_MIN_HEIGHT),
            )
        });
        let size = if zone == DockZone::Bottom {
            egui::vec2(
                (bounds.width() - 24.0).max(BOTTOM_MIN_CONTENT_W),
                raw.height()
                    .clamp(super::dock_paint::FLOAT_MIN_HEIGHT, bounds.height() * 0.5),
            )
        } else {
            egui::vec2(
                raw.width().clamp(280.0, bounds.width() * 0.6),
                raw.height().clamp(
                    super::dock_paint::FLOAT_MIN_HEIGHT * 0.8,
                    bounds.height() * 0.7,
                ),
            )
        };
        egui::Rect::from_min_size(self.float_origin(size), size)
    }

    /// 带 → `band_tabs` 下标。三条带的顺序只在这里写死一次。
    pub(in crate::ui) fn band_slot(zone: DockZone) -> usize {
        match zone {
            DockZone::Left => 0,
            DockZone::Right => 1,
            DockZone::Bottom => 2,
        }
    }

    pub(in crate::ui) fn band_tab(&self, zone: DockZone) -> Option<PanelId> {
        self.band_tabs[Self::band_slot(zone)]
    }

    pub(in crate::ui) fn set_band_tab(&mut self, zone: DockZone, id: PanelId) {
        self.band_tabs[Self::band_slot(zone)] = Some(id);
    }

    /// 面板离开任何带时清掉它的活动标签，免得下次 `Split` / 浮窗用错面板。
    pub(in crate::ui) fn forget_band_tab(&mut self, id: PanelId) {
        for tab in &mut self.band_tabs {
            if *tab == Some(id) {
                *tab = None;
            }
        }
    }
}

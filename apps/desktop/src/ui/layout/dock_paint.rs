//! P4：把 `DockLayout` 画出来——两条带、浮动窗、以及布局动作的落地。
//!
//! 这是 `layout/` 里唯一碰 `Ui` / `Context` 的文件。上面两层分别是纯数据
//! （`dock.rs`）与纯判定（`dock_drag.rs`），所以这里剩下的活只有「按顺序问
//! 模型要成员、按带选面板类型、把松手翻成动作」。
//!
//! # 抓手是隐形的
//!
//! 每块面板先注册一个**盖住自己整块**的 `Sense::drag()` 交互，**不画任何像素、
//! 不占位**。它先于内容注册，而 `egui` 的命中判定里后注册的控件在上，所以按钮 /
//! 文本框 / 滚动条照常拿到按下事件，只有「按在空白处拖动」才落到抓手。
//! 于是默认布局一个像素都不动（六张整窗基线不变），拖拽却能用。

use crate::ui::layout::dock_drag::{drag_far_enough, resolve_drop, DropTarget, DOCK_EDGE};
use crate::ui::*;
use eframe::egui;
use term_app::action::Action;
use term_app::dock::{DockZone, LayoutAction, PanelId};

/// 底带面板的默认高度。`SidePanel` 的 `default_width` 在底带上没有意义。
///
/// 值就取 `dock_bottom::DEFAULT_H`：底带的实际高度现在归那条带自己管，
/// 这里只用来推浮动窗的最小高度，两处不该各写一个 220。
const BOTTOM_DEFAULT_HEIGHT: f32 = super::dock_bottom::DEFAULT_H;

/// 浮动窗的最小高度：比它再矮就装不下工具面板的标题 + 两三行内容。
pub(super) const FLOAT_MIN_HEIGHT: f32 = BOTTOM_DEFAULT_HEIGHT + 140.0;

/// 隐形抓手。见模块注释：不画像素，先注册，让内容优先。
fn grip(ui: &egui::Ui, id: PanelId) -> egui::Response {
    ui.interact(
        ui.max_rect(),
        egui::Id::new((id.name(), "grip")),
        egui::Sense::drag(),
    )
    .on_hover_cursor(egui::CursorIcon::Grab)
}

/// 落点提示的几何：目标带的矩形 + 「插在第几位」的那条线。
///
/// 纯函数（不碰 `Ui`），所以能单测：带矩形按边带厚度给（左右竖带是整高的，
/// 底带是整宽的），插入线贴在目标带里第 `index` 块面板的外沿——`index` 等于
/// 成员数时贴最后一块的下沿 / 右沿，也就是「追加到末尾」。
fn hint_geometry(
    bounds: egui::Rect,
    zone: DockZone,
    index: usize,
    docked: &[(PanelId, DockZone, egui::Rect)],
) -> (egui::Rect, egui::Rect) {
    let band = match zone {
        DockZone::Left => egui::Rect::from_min_max(
            bounds.min,
            egui::pos2(bounds.left() + DOCK_EDGE, bounds.bottom()),
        ),
        DockZone::Right => egui::Rect::from_min_max(
            egui::pos2(bounds.right() - DOCK_EDGE, bounds.top()),
            bounds.max,
        ),
        DockZone::Bottom => egui::Rect::from_min_max(
            egui::pos2(bounds.left(), bounds.bottom() - DOCK_EDGE),
            bounds.max,
        ),
    };
    let members: Vec<egui::Rect> = docked
        .iter()
        .filter(|(_, z, _)| *z == zone)
        .map(|(_, _, rect)| *rect)
        .collect();
    let line = match zone {
        DockZone::Bottom => {
            let x = match members.get(index) {
                Some(rect) => rect.left(),
                None => members.last().map(|r| r.right()).unwrap_or(band.left()),
            };
            egui::Rect::from_min_max(
                egui::pos2(x - 1.0, band.top()),
                egui::pos2(x + 1.0, band.bottom()),
            )
        }
        _ => {
            let y = match members.get(index) {
                Some(rect) => rect.top(),
                None => members.last().map(|r| r.bottom()).unwrap_or(band.top()),
            };
            egui::Rect::from_min_max(
                egui::pos2(band.left(), y - 1.0),
                egui::pos2(band.right(), y + 1.0),
            )
        }
    };
    (band, line)
}

/// 停靠面板的框：**零内边距**，底色等于面板底。
///
/// `egui` 默认的 `Frame::side_top_panel` 会把内容整体内缩 `menu_margin`
/// （本项目 `menu_pad_x = 11`）。兼容终端的侧栏没有这 11px：标题方块距左沿 12、
/// 分组箭头距左沿 8，各面板自己知道要留多少。所以统一去掉，让「距左沿多少」
/// 只有面板自己那一处常量说了算，而不是加上一个全局偏移再去对图。
/// 停靠带的外壳：只填底色，**不画边框**——边框改由 [`paint_panel_border`]
/// 逐面板绘制。原因是「一条带一个 SidePanel」：带内多个面板共用外壳，
/// 外壳的 stroke 只会给整条带描一圈，带内各面板自己并没有完整边框（用户反馈
/// 「部分面板依然没有边框」）。统一改成每个面板自己画一圈，才真正一致。
pub(super) fn panel_frame() -> egui::Frame {
    egui::Frame::none().fill(crate::theme::chrome(term_core::theme::CHROME_SCALE.dialog))
}

/// 给单个面板画一圈 1px 外框。所有面板（左右带的每一块、底带每个分组、
/// 浮动窗）都走这一个函数，保证边框粗细 / 颜色 / 内缩完全一致。
/// `shrink(0.5)` 让 1px 线落在像素中心，边缘不发虚。
pub(super) fn paint_panel_border(p: &egui::Painter, rect: egui::Rect) {
    p.rect_stroke(
        rect.shrink(0.5),
        0.0,
        egui::Stroke::new(
            1.0,
            crate::theme::chrome(term_core::theme::CHROME_SCALE.border),
        ),
    );
}

impl TerminalWindow {
    /// 画一条停靠带。**发射点在 `draw()` 里固定**，这里只管带内成员与位次。
    ///
    /// 底带是例外：它是**一条带 + 标签条**（`dock_bottom`），不是「每个成员一条
    /// 面板」。所以 `Bottom` 整支交给 `draw_bottom_dock`，本函数不再逐成员循环。
    pub(in crate::ui) fn draw_dock_zone(&mut self, ctx: &egui::Context, zone: DockZone) {
        if zone == DockZone::Bottom {
            self.draw_bottom_dock(ctx);
            return;
        }
        let ids: Vec<PanelId> = self
            .dock
            .zone(zone)
            .into_iter()
            .filter(|id| self.panel_visible(*id))
            .collect();
        // K3：空带**不再早退**。兼容终端保留一个空壳与齿轮入口（截图 2），
        // 用户可以从「全不勾」的菜单里把面板勾回来；整条带消失等于把入口藏了。
        let band_anchor = PanelId::ALL
            .into_iter()
            .find(|p| p.default_zone() == zone)
            .unwrap_or(PanelId::Explorer);
        let empty_band = ids.is_empty();

        // 一条带 = 一条 SidePanel。之前是「每个成员各声明一条」，多个 left
        // SidePanel 会**并排**成好几条竖带；视觉基线里左带是 Explorer + Filer
        // 上下堆叠、右带是 Session + Outline 上下堆叠，同一条带只有一个宽度。
        // 带内按成员数垂直均分，每块各自记一个真实矩形：`resolve_drop` 用
        // `rect.center().y` 数插入位次，堆叠之后这个判定才真正对得上画面。
        let width = if empty_band {
            TerminalWindow::panel_width(band_anchor)
        } else {
            ids.iter()
                .map(|id| TerminalWindow::panel_width(*id))
                .fold(0.0_f32, f32::max)
        };
        let mut rects: Vec<(PanelId, egui::Rect)> = Vec::new();
        let mut dragged: Option<PanelId> = None;
        // 齿轮菜单攒下的命令：等这条带画完再落地，否则本帧的成员表与矩形对不上。
        let mut cmds: Vec<super::dock_bottom::Cmd> = Vec::new();
        let panel = if zone == DockZone::Left {
            egui::SidePanel::left(egui::Id::new(zone.name()))
        } else {
            egui::SidePanel::right(egui::Id::new(zone.name()))
        };
        panel
            .default_width(width)
            .resizable(true)
            .frame(panel_frame())
            .show(ctx, |ui| {
                let avail = ui.available_rect_before_wrap();
                if empty_band {
                    let mut child = ui.new_child(
                        egui::UiBuilder::new()
                            .id_salt((band_anchor.name(), "empty_chrome"))
                            .max_rect(avail)
                            .layout(egui::Layout::top_down(egui::Align::Min)),
                    );
                    child.set_clip_rect(avail);
                    self.draw_compact_chrome(&mut child, band_anchor, zone, &[], &mut cmds);
                    ui.advance_cursor_after_rect(avail);
                    return;
                }
                let h = avail.height() / ids.len() as f32;
                for (i, id) in ids.iter().copied().enumerate() {
                    let top = avail.top() + i as f32 * h;
                    let bottom = if i + 1 == ids.len() {
                        avail.bottom()
                    } else {
                        top + h
                    };
                    let rect = egui::Rect::from_min_max(
                        egui::pos2(avail.left(), top),
                        egui::pos2(avail.right(), bottom),
                    );
                    // 独立 id：两块面板里的 ScrollArea / TextEdit 自动 id 不能撞。
                    let mut child = ui.new_child(
                        egui::UiBuilder::new()
                            .id_salt(id.name())
                            .max_rect(rect)
                            .layout(egui::Layout::top_down(egui::Align::Min)),
                    );
                    child.set_clip_rect(rect);
                    if self.dock_panel_ui(&mut child, id, zone, &ids, &mut cmds) {
                        dragged = Some(id);
                    }
                    // 每个面板各自一圈 1px 外框（不再只靠带外壳 + 中间一条 hline）。
                    paint_panel_border(ui.painter(), rect);
                    rects.push((id, rect));
                }
                // `new_child` 不分配父 Ui 的空间。少了这一句，父 `panel_ui` 的
                // min_rect 只剩 `set_min_width(96)` 那点宽度，SidePanel 会塌到
                // egui 的 96pt 下限（2026-11-08 实测 260 → 96）。必须显式把
                // 整块带占住，宽度才是 `default_width` 给的 260 / 220。
                ui.advance_cursor_after_rect(avail);
            });
        for (id, rect) in rects {
            self.dock_rects.push((id, zone, rect));
        }
        for cmd in cmds {
            self.apply_dock_cmd(cmd, zone, &ids);
        }
        if let Some(id) = dragged {
            let press = ctx
                .input(|i| i.pointer.press_origin().or(i.pointer.interact_pos()))
                .unwrap_or_default();
            self.dock_drag = Some((id, press));
        }
    }

    /// 面板内容 + 抓手。返回「这一帧开始拖了没有」。
    ///
    /// `zone` / `band` / `cmds` 只为一件事：标题栏那颗齿轮弹的菜单要说清楚
    /// 「这是哪条带、带里现在有谁」，并把点击攒成命令丢回给带自己落地。
    fn dock_panel_ui(
        &mut self,
        ui: &mut egui::Ui,
        id: PanelId,
        zone: DockZone,
        band: &[PanelId],
        cmds: &mut Vec<super::dock_bottom::Cmd>,
    ) -> bool {
        let started = grip(ui, id).drag_started();
        // 侧栏自带标题带与 Filter（`draw_sidebar_title` / `draw_sidebar_filter`），
        // 再叠一层就是双标题。其余面板由容器补公共 chrome：
        // 「▪ 标题 … ⚙ ×」+ Explorer / Outline 的一条 Filter。
        //
        // chrome 单独切一个 child Ui，只为把它内部的 `item_spacing.y` 清零：
        // 标题与筛选行的高度都是逐像素实测常量，掺进全局 3pt 间距就会整体
        // 下移；而面板正文仍需要那份间距，所以不能在整个面板上清零。
        // `new_child` 不占父 Ui 空间，末尾必须显式推进光标。
        // 五个位置统一由容器画 chrome：色块 + 标题 + 齿轮 + 文字 ×。
        // 这里不再对 Sidebar 开特例——它原先自绘手绘 ×（right-10），
        // 与其余四块（文字 ×，right-12）不一致，见 K2。
        let avail = ui.available_rect_before_wrap();
        let chrome_rect = egui::Rect::from_min_size(
            avail.min,
            egui::vec2(avail.width(), super::dock_panel_chrome::chrome_height(id)),
        );
        let mut chrome = ui.new_child(
            egui::UiBuilder::new()
                .id_salt((id.name(), "chrome"))
                .max_rect(chrome_rect)
                .layout(egui::Layout::top_down(egui::Align::Min)),
        );
        chrome.set_clip_rect(chrome_rect);
        chrome.spacing_mut().item_spacing.y = 0.0;
        self.dock_panel_title(&mut chrome, id, zone, band, cmds);
        self.draw_panel_filter(&mut chrome, id);
        ui.advance_cursor_after_rect(chrome_rect);
        // `PanelId::Shell` 例外：那块面板装的是**终端**，滚轮归终端自己的回看
        // （`pane.rs` 的 `viewport.scroll_up/down`），而且 `ScrollArea` 把内容
        // 方向摊成无穷高，`available_rect_before_wrap` 会给出 `f32::INFINITY`——
        // 终端算不出行列数。与 `layout/dock_bottom.rs` 同一条判据（都在
        // `PanelId::scrolls_in_dock` 里）。
        if id.scrolls_in_dock() {
            egui::ScrollArea::vertical()
                .id_salt(id.name())
                .show(ui, |ui| self.draw_panel_body(id, ui));
        } else {
            self.draw_panel_body(id, ui);
        }
        started
    }

    // 标题带（含齿轮）与 Filter 行在 `dock_panel_chrome.rs`，这里只管装配。

    /// 松手时把落点翻成布局动作。
    ///
    /// 必须在三条带都画完之后调用：落点要用**本帧**各面板的真实矩形。
    pub(in crate::ui) fn finish_dock_drag(&mut self, ctx: &egui::Context, bounds: egui::Rect) {
        let Some((panel, press)) = self.dock_drag else {
            return;
        };
        if !ctx.input(|i| i.pointer.any_released()) {
            return;
        }
        self.dock_drag = None;
        let Some(release) = ctx.input(|i| i.pointer.interact_pos()) else {
            return;
        };
        if !drag_far_enough(press, release) {
            // 当成一次点击：按在空白处点一下不该把面板换个位置。
            return;
        }
        let action = match resolve_drop(release, bounds, &self.dock_rects, DOCK_EDGE) {
            DropTarget::Dock { zone, index } => LayoutAction::Dock { panel, zone, index },
            DropTarget::Float => LayoutAction::Float(panel),
        };
        self.dispatch_action(Action::Layout(action));
    }

    /// 浮窗的发射点。**窗口本体不在这里**：系统外框 / 关闭归位 / 坐标换算
    /// 都在 `dock_native.rs`，这里只负责「什么时候落盘」。
    pub(in crate::ui) fn draw_floating_panels(&mut self, ctx: &egui::Context) {
        let moved = self.draw_native_floats(ctx);
        // 只在拖动手已经松开时落盘：拖动过程中每帧写一次布局文件既没必要，
        // 也会让「写盘失败」的提示刷满 stderr。
        if moved && ctx.input(|i| !i.pointer.any_down()) {
            self.save_layout();
        }
    }

    /// 布局动作的落地。**唯一**改 `DockLayout` 的地方（I1 的布局版）。
    pub(in crate::ui) fn apply_layout_action(&mut self, action: LayoutAction) {
        match action {
            LayoutAction::Dock { panel, zone, index } => self.dock.insert(panel, zone, index),
            LayoutAction::Float(panel) => {
                let rect = self.float_rect(panel);
                self.dock.float(panel, rect);
            }
            LayoutAction::FloatBand(zone) => {
                let rect = self.float_band_rect(zone);
                self.dock.band_float(zone, rect);
            }
            LayoutAction::Redock(panel) => self.dock.redock(panel),
            LayoutAction::RedockBand(zone) => self.dock.band_redock(zone),
            LayoutAction::Reset => self.dock.reset(),
        }
        self.save_layout();
        self.ctx.request_repaint();
    }

    /// 拖拽中的落点提示。只在**按住拖动时**画，平时一个像素都没有。
    ///
    /// 没有它的话「拖拽移位」是盲操作：松手才知道停到哪了，而这个动作会改变
    /// 整窗布局——盲操作 + 难撤销（只能 Reset Layout）是不能接受的组合。
    pub(in crate::ui) fn paint_drop_hint(&self, ctx: &egui::Context, bounds: egui::Rect) {
        if self.dock_drag.is_none() {
            return;
        }
        let Some(pointer) = ctx.input(|i| i.pointer.interact_pos()) else {
            return;
        };
        let DropTarget::Dock { zone, index } =
            resolve_drop(pointer, bounds, &self.dock_rects, DOCK_EDGE)
        else {
            // 落在中间 = 要浮起来。浮起来的位置就是鼠标处，画个框反而会挡住
            // 那块本来能看见的终端。
            return;
        };
        // 落点提示是「瞬时焦点」，用灰阶里唯一允许的饱和蓝。
        // 以前读 `selection.bg_fill`（亮蓝），ChromeScale 把全局选中底统一成
        // 中性 #262626 之后，这条提示会一起变得几乎看不见——拖拽是交互功能，
        // 不能跟着弹窗皮肤一起降对比。
        let accent = crate::theme::chrome(term_core::theme::CHROME_SCALE.focus);
        let painter = ctx.layer_painter(egui::LayerId::new(
            egui::Order::Foreground,
            egui::Id::new("dock_drop_hint"),
        ));
        let (band, line) = hint_geometry(bounds, zone, index, &self.dock_rects);
        painter.rect_filled(band, 4.0, accent.gamma_multiply(0.16));
        painter.rect_stroke(band, 4.0, egui::Stroke::new(1.0, accent));
        painter.rect_filled(line, 1.0, accent);
    }

    /// 刚浮起来的面板放哪：落在松开鼠标的地方。
    ///
    /// reducer 只知道「要浮起来」，不知道鼠标在哪——坐标只能在这里取。
    fn float_rect(&self, panel: PanelId) -> egui::Rect {
        let size = egui::vec2(
            TerminalWindow::panel_width(panel).max(240.0),
            FLOAT_MIN_HEIGHT,
        );
        egui::Rect::from_min_size(self.float_origin(size), size)
    }

    /// 让指针落在窗口标题栏上，而不是窗口左上角：松手后标题栏正好在鼠标
    /// 下面，想再拖一次不用先去找它。单面板与整带共用。
    pub(super) fn float_origin(&self, size: egui::Vec2) -> egui::Pos2 {
        let centre = self
            .ctx
            .input(|i| i.pointer.interact_pos().or(i.pointer.hover_pos()))
            .unwrap_or_else(|| self.ctx.available_rect().center());
        let min = centre - egui::vec2(40.0, 12.0);
        let bounds = self.ctx.available_rect();
        egui::pos2(
            min.x
                .clamp(bounds.left(), (bounds.right() - size.x).max(bounds.left())),
            min.y
                .clamp(bounds.top(), (bounds.bottom() - size.y).max(bounds.top())),
        )
    }


}

#[cfg(test)]
mod tests {
    use super::*;

    fn rect(x: f32, y: f32, w: f32, h: f32) -> egui::Rect {
        egui::Rect::from_min_size(egui::pos2(x, y), egui::vec2(w, h))
    }

    #[test]
    fn the_hint_band_is_edge_thick_and_spans_the_whole_side() {
        let bounds = rect(0.0, 0.0, 1200.0, 800.0);
        let (band, _) = hint_geometry(bounds, DockZone::Left, 0, &[]);
        assert_eq!(band.width(), DOCK_EDGE);
        assert_eq!(band.height(), 800.0, "左带是整高的");
        let (band, _) = hint_geometry(bounds, DockZone::Bottom, 0, &[]);
        assert_eq!(band.height(), DOCK_EDGE);
        assert_eq!(band.width(), 1200.0, "底带是整宽的");
    }

    #[test]
    fn the_insertion_line_follows_the_slot_not_the_pointer() {
        let bounds = rect(0.0, 0.0, 1200.0, 800.0);
        let docked = vec![
            (PanelId::Sidebar, DockZone::Left, rect(0.0, 24.0, 200.0, 300.0)),
            (PanelId::Filer, DockZone::Left, rect(0.0, 324.0, 200.0, 400.0)),
        ];
        // 插在第 0 位：线在第一块的上沿；插在第 1 位：第二块的上沿；
        // 追加（index == 2）：最后一块的下沿。
        let (_, line) = hint_geometry(bounds, DockZone::Left, 0, &docked);
        assert_eq!(line.center().y, 24.0);
        let (_, line) = hint_geometry(bounds, DockZone::Left, 1, &docked);
        assert_eq!(line.center().y, 324.0);
        let (_, line) = hint_geometry(bounds, DockZone::Left, 2, &docked);
        assert_eq!(line.center().y, 724.0);
    }

    #[test]
    fn an_empty_band_puts_the_line_at_its_leading_edge() {
        let bounds = rect(0.0, 0.0, 1200.0, 800.0);
        let (band, line) = hint_geometry(bounds, DockZone::Bottom, 0, &[]);
        assert_eq!(line.center().x, band.left());
    }

    /// 浮动带要真的画得出来（窗内 tab + Filter + 正文），而且切 tab 不把带
    /// 的归属改掉；收回后原位次照旧。这条是 K3-b 的渲染冒烟测试。
    #[test]
    fn a_floating_band_renders_its_tabs_and_redocks_without_losing_its_members() {
        let ctx = egui::Context::default();
        let pool = std::sync::Arc::new(term_session::SessionPool::new());
        let mut win = crate::ui::lifecycle_test_support::window_with(pool, &ctx, Vec::new());
        let before = win.dock.zone(DockZone::Right);
        win.dock
            .band_float(DockZone::Right, rect(120.0, 90.0, 440.0, 400.0));
        let input = egui::RawInput {
            screen_rect: Some(rect(0.0, 0.0, 1280.0, 800.0)),
            ..Default::default()
        };
        // 两帧：第一帧 egui 才登记窗口 id / tab 命中区，第二帧才稳定。
        for _ in 0..2 {
            let _ = ctx.run(input.clone(), |ctx| win.draw_floating_panels(ctx));
        }
        assert!(win.dock.is_band_floating(DockZone::Right));
        win.set_band_tab(DockZone::Right, PanelId::Outline);
        assert_eq!(win.band_tab(DockZone::Right), Some(PanelId::Outline));
        assert_eq!(win.dock.band_members(DockZone::Right), before);

        win.apply_layout_action(LayoutAction::RedockBand(DockZone::Right));
        assert!(!win.dock.is_band_floating(DockZone::Right));
        assert_eq!(win.dock.zone(DockZone::Right), before);
    }
}

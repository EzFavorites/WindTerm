//! 浮窗走**系统原生外框**：外框、标题、最小化 / 最大化 / 关闭都交给窗管。
//!
//! # 为什么不用自绘 chrome
//!
//! 视觉基线里的浮窗是独立窗口，红黄灰三颗在**系统**那一侧。用 `egui::Window`
//! 只能画个像的：拖动、缩放、最小化、贴边全得自己实现，还要跟真实窗管抢事件。
//! egui 的多视口把这层交给了 winit，我们只声明「这块内容要一扇窗」。
//!
//! # 关闭 = 归位
//!
//! 系统关闭按钮不「关掉面板」也不退进程：`close_requested` 在读到的这一帧就被
//! 翻成 [`LayoutAction::Redock`] / [`LayoutAction::RedockBand`]，面板回到原来的带
//! 与位次。面板的显示开关仍然只有菜单里那一条，于是标题条上的 × 与系统红点
//! 语义一致——「收回停靠位」，不是两个都叫「关」却各干一件事。
//!
//! # 两级回退
//!
//! 1. **夹具**（`--screenshot`）：直接走内嵌 `egui::Window`。多视口是真的开
//!    第二扇系统窗口，而截图用的是 `ViewportCommand::Screenshot`——只拍 root，
//!    浮窗内容会整块从图里消失。截图进程本来也不该弹窗。
//! 2. **后端不支持多视口**：`show_viewport_immediate` 会以
//!    `ViewportClass::Embedded` 回调，同一条内嵌路径接管（单测走的就是这条）。
//!
//! # 坐标
//!
//! 模型里的矩形一直是**主窗口内容坐标**（`float_band_rect` / `float_rect` 都由
//! `ctx.available_rect()` 推出来）。`ViewportBuilder::with_position` 要屏幕坐标，
//! 差一个「主窗口外框相对内容区」的偏移；只在这一层换算，模型里不混两套坐标。

use crate::ui::*;
use eframe::egui;
use term_app::action::Action;
use term_app::dock::{DockZone, LayoutAction, PanelId};

/// 新浮窗最多替它要几帧前台。见 `draw_one_float` 里那段注释：要少了埋在后头，
/// 一直要就会跟用户抢焦点。
const FOCUS_TRIES: u8 = 3;

/// 一块要浮起来的东西：单块面板，或整条带。
///
/// 两者共用一套窗口管线，但成员归属不同——单面板搬走的是一个 `PanelId`，
/// 整带搬走的是一条带的位置，带内成员还在 `DockLayout` 里。
#[derive(Clone, Copy, PartialEq, Eq)]
pub(in crate::ui) enum FloatTarget {
    Panel(PanelId),
    Band(DockZone),
}

impl FloatTarget {
    /// 视口 / 内嵌窗的 id 种子。**两类必须不同名**：面板与带可以同时浮着
    /// （浮窗里那条 `Float` 就是「把当前 tab 拆出去」），同种子会抢同一扇窗。
    fn key(self) -> (&'static str, &'static str) {
        match self {
            FloatTarget::Panel(id) => ("dock_float_panel", id.name()),
            FloatTarget::Band(zone) => ("dock_float_band", zone.name()),
        }
    }
}

/// 一次浮窗绘制的产物。
#[derive(Default)]
struct FloatPass {
    /// 被拖 / 被缩放后的新矩形（内容坐标）。没变就是 `None`。
    rect: Option<egui::Rect>,
    /// 用户点了系统关闭按钮。
    close: bool,
    /// 这一帧刚替它要过一次前台（一次性，见 `float_focus_sent`）。
    focused: bool,
}

impl TerminalWindow {
    /// 画所有浮窗（单面板 + 整带）。返回这一帧有没有窗口被拖动 / 缩放。
    pub(in crate::ui) fn draw_native_floats(&mut self, ctx: &egui::Context) -> bool {
        let mut moved = false;
        let mut seen: Vec<egui::ViewportId> = Vec::new();
        for (id, rect) in self.dock.floating() {
            if !self.panel_visible(id) {
                continue;
            }
            let pass = self.draw_one_float(ctx, FloatTarget::Panel(id), rect);
            if let Some(now) = pass.rect {
                self.dock.float(id, now);
                moved = true;
            }
            if pass.close {
                self.dispatch_action(Action::Layout(LayoutAction::Redock(id)));
            }
        }
        for (zone, rect) in self.dock.floating_bands() {
            // 成员全被关掉的带没有可画的内容，也就没有窗口。
            if self.band_visible_members(zone).is_empty() {
                continue;
            }
            let pass = self.draw_one_float(ctx, FloatTarget::Band(zone), rect);
            if let Some(now) = pass.rect {
                self.dock.band_float(zone, now);
                moved = true;
            }
            if pass.close {
                self.dispatch_action(Action::Layout(LayoutAction::RedockBand(zone)));
            }
        }
        // 只留这一帧还在浮的视口：收回之后再浮起来，前台那一枪要重新打。
        seen.extend(
            self.dock
                .floating()
                .into_iter()
                .map(|(id, _)| Self::float_viewport_id(FloatTarget::Panel(id))),
        );
        seen.extend(
            self.dock
                .floating_bands()
                .into_iter()
                .map(|(zone, _)| Self::float_viewport_id(FloatTarget::Band(zone))),
        );
        self.float_focus_tries.retain(|(id, _)| seen.contains(id));
        moved
    }

    fn float_viewport_id(target: FloatTarget) -> egui::ViewportId {
        egui::ViewportId::from_hash_of(target.key())
    }

    fn draw_one_float(
        &mut self,
        ctx: &egui::Context,
        target: FloatTarget,
        rect: egui::Rect,
    ) -> FloatPass {
        if harness::fixture_mode() {
            return self.draw_float_embedded(ctx, target, rect);
        }
        let builder = egui::ViewportBuilder::default()
            .with_title(self.tr(self.float_title_key(target)))
            .with_inner_size(rect.size())
            .with_min_inner_size(self.float_min_size(target))
            .with_position(self.builder_position(rect))
            .with_resizable(true);
        let id = Self::float_viewport_id(target);
        let tries = self
            .float_focus_tries
            .iter()
            .find(|(v, _)| *v == id)
            .map_or(0, |(_, n)| *n);
        let mut pass = FloatPass::default();
        // 立即视口的回调在**本帧内**跑完（后端把它当父窗口的一次额外绘制），
        // 所以这里直接对 `pass` 赋值再读，不需要跨帧状态。
        ctx.show_viewport_immediate(id, builder, |ctx, class| {
            if matches!(class, egui::ViewportClass::Embedded) {
                // 后端不支持多视口：同一条内嵌路径接管，行为与夹具一致。
                pass = self.draw_float_embedded(ctx, target, rect);
                return;
            }
            if tries < FOCUS_TRIES {
                // 窗口刚建出来时**可能是埋在主窗口后面的**（实测 macOS：创建它的
                // 那一刻应用不在前台，就一直排在后头）。用户刚点了 Float，这扇窗
                // 就该浮到前面来，所以显式要一次前台。
                //
                // 只打前几帧：不打的话窗口埋着；一直打的话用户点回主窗口又会被
                // 抢回去——那是更难受的毛病。
                ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
                pass.focused = true;
            }
            pass.close = ctx.input(|i| i.viewport().close_requested());
            // 读 `inner_rect` 而**不是** `outer_rect`：模型里的矩形是内容区
            // （`with_inner_size` 也是内容区），外层高度多一个标题栏。用外层
            // 读回来会每帧把标题栏高度算进尺寸里，窗口一路长大。
            if let Some(inner) = ctx.input(|i| i.viewport().inner_rect) {
                // 最小化会把矩形压成 0 尺寸，那种值不能写回模型——
                // 否则还原之后窗口会变成一条线。
                if inner.width() > 1.0 && inner.height() > 1.0 {
                    let now = self.to_content(inner);
                    if (now.min - rect.min).length() > 0.5
                        || (now.size() - rect.size()).length() > 0.5
                    {
                        if std::env::var_os("WINDTERM_DEBUG_FLOAT").is_some() {
                            eprintln!("float {:?}: {:?} → {:?}", target.key(), rect, now);
                        }
                        pass.rect = Some(now);
                    }
                }
            }
            self.draw_float_body(ctx, target);
        });
        if pass.focused {
            match self.float_focus_tries.iter_mut().find(|(v, _)| *v == id) {
                Some((_, n)) => *n += 1,
                None => self.float_focus_tries.push((id, 1)),
            }
        }
        pass
    }

    /// 原生视口里的正文。**没有**我们的标题条：标题归系统窗管。
    fn draw_float_body(&mut self, ctx: &egui::Context, target: FloatTarget) {
        let frame =
            egui::Frame::none().fill(crate::theme::chrome(term_core::theme::CHROME_SCALE.dialog));
        egui::CentralPanel::default().frame(frame).show(ctx, |ui| {
            match target {
                FloatTarget::Panel(id) => self.draw_float_panel(ui, id),
                FloatTarget::Band(zone) => {
                    let members = self.band_visible_members(zone);
                    self.draw_band_content(ui, zone, &members);
                }
            }
        });
    }

    /// 内嵌回退：夹具出图与「后端没有多视口」共用。
    ///
    /// 结构刻意与原生路径一一对应，只是把系统外框换成 `egui::Window` 自己的
    /// 标题条，位置 / 尺寸仍走同一份模型。
    fn draw_float_embedded(
        &mut self,
        ctx: &egui::Context,
        target: FloatTarget,
        rect: egui::Rect,
    ) -> FloatPass {
        let mut pass = FloatPass::default();
        let mut open = true;
        let min = self.float_min_size(target);
        let shown = egui::Window::new(self.tr(self.float_title_key(target)))
            .id(egui::Id::new(("dock_float", target.key())))
            .default_pos(rect.min)
            .default_size(rect.size())
            .min_width(min.x)
            .min_height(min.y)
            .resizable(true)
            .frame(
                egui::Frame::window(&ctx.style())
                    .fill(crate::theme::chrome(term_core::theme::CHROME_SCALE.dialog))
                    .stroke(egui::Stroke::new(
                        1.0,
                        crate::theme::chrome(term_core::theme::CHROME_SCALE.border),
                    )),
            )
            // 与弹窗同理：夹具把时钟钉在 0，淡入会停在第一帧。
            .fade_in(false)
            .open(&mut open)
            .show(ctx, |ui| match target {
                FloatTarget::Panel(id) => self.draw_float_panel(ui, id),
                FloatTarget::Band(zone) => {
                    let members = self.band_visible_members(zone);
                    self.draw_band_content(ui, zone, &members);
                }
            });
        if let Some(shown) = shown {
            let now = shown.response.rect;
            if (now.min - rect.min).length() > 0.5 || (now.size() - rect.size()).length() > 0.5 {
                pass.rect = Some(now);
            }
        }
        pass.close = !open;
        pass
    }

    /// 单块面板的正文。可滚动的面板补一层竖向滚动——原生窗口尺寸固定，
    /// 内容超出就得能滚，不然底部控件会被静默裁掉。
    fn draw_float_panel(&mut self, ui: &mut egui::Ui, id: PanelId) {
        if id.scrolls_in_dock() {
            egui::ScrollArea::vertical()
                .id_salt((id.name(), "float_scroll"))
                .show(ui, |ui| self.draw_panel_body(id, ui));
        } else {
            self.draw_panel_body(id, ui);
        }
    }

    /// 窗口标题取「当前看得到的那块面板」：整带浮着时是它的活动 tab。
    fn float_title_key(&self, target: FloatTarget) -> &'static str {
        match target {
            FloatTarget::Panel(id) => id.title(),
            FloatTarget::Band(zone) => {
                let members = self.band_visible_members(zone);
                match self.band_active(zone, &members) {
                    Some(id) => id.title(),
                    None => "Panel",
                }
            }
        }
    }

    fn float_min_size(&self, target: FloatTarget) -> egui::Vec2 {
        match target {
            // 高度不给下限：0 在 winit 那边等于「不约束」。
            FloatTarget::Panel(id) => egui::vec2(TerminalWindow::panel_width(id), 0.0),
            FloatTarget::Band(_) => {
                egui::vec2(280.0, super::dock_bottom::TAB_H + 80.0)
            }
        }
    }

    /// 主窗口内容坐标 → 屏幕坐标。
    fn to_screen(&self, rect: egui::Rect) -> egui::Rect {
        rect.translate(self.root_frame().0.to_vec2())
    }

    /// 屏幕坐标 → 主窗口内容坐标。
    fn to_content(&self, rect: egui::Rect) -> egui::Rect {
        rect.translate(-self.root_frame().0.to_vec2())
    }

    /// 交给 `ViewportBuilder::with_position` 的值。
    ///
    /// 它要的是**外框**位置，而模型里的是内容区：两者差一个标题栏高度。
    /// 少这一个减法，每次读回来都会把标题栏算进坐标，窗口逐帧往下爬。
    fn builder_position(&self, rect: egui::Rect) -> egui::Pos2 {
        self.to_screen(rect).min + self.root_frame().1
    }

    /// root 视口的两条基准：内容区左上角在屏幕上的位置，以及外框相对内容区
    /// 的偏移（macOS 上就是标题栏高度，y 为负）。
    ///
    /// 必须读 **root** 视口：在子视口回调里 `i.viewport()` 给的是子窗口自己，
    /// 拿它的基准去换算父窗口的坐标会每层都错一次。
    fn root_frame(&self) -> (egui::Pos2, egui::Vec2) {
        self.ctx
            .input(|i| {
                let vp = i.raw.viewports.get(&egui::ViewportId::ROOT)?;
                Some((vp.inner_rect?.min, vp.outer_rect?.min - vp.inner_rect?.min))
            })
            .unwrap_or((egui::Pos2::ZERO, egui::Vec2::ZERO))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 面板与整带**不能**共用视口种子：两者可以同时浮着（浮窗里的 `Float`
    /// 就是拆出当前 tab），撞 id 会让后画的那扇窗抢走前一个的位置与标题。
    #[test]
    fn a_panel_and_a_band_never_share_a_viewport_id() {
        let panel = egui::ViewportId::from_hash_of(FloatTarget::Panel(PanelId::Sender).key());
        let band = egui::ViewportId::from_hash_of(FloatTarget::Band(DockZone::Bottom).key());
        assert_ne!(panel, band);
        // 同一条带的两次请求必须落到同一扇窗，否则每帧都会新开一个。
        let again = egui::ViewportId::from_hash_of(FloatTarget::Band(DockZone::Bottom).key());
        assert_eq!(band, again);
    }

    /// 坐标换算是可逆的：拖完窗口读回来再写进 builder，位置不能因为换算漂移。
    #[test]
    fn the_screen_and_content_spaces_round_trip() {
        let ctx = egui::Context::default();
        let pool = std::sync::Arc::new(term_session::SessionPool::new());
        let win = crate::ui::lifecycle_test_support::window_with(pool, &ctx, Vec::new());
        let rect = egui::Rect::from_min_size(egui::pos2(120.0, 90.0), egui::vec2(440.0, 400.0));
        // 没有 root 视口信息时偏移是 0，换算必须是恒等。
        let back = win.to_content(win.to_screen(rect));
        assert!((back.min - rect.min).length() < 0.01, "{back:?}");
        assert!((back.size() - rect.size()).length() < 0.01);
    }
}

//! 对话框容器：`Area` + 自绘 chrome，**位置由屏幕中心推算**。
//!
//! # 为什么不用 `egui::Window`
//!
//! 仓库里有 23 个对话框，它们全部 `resizable(false)` + `collapsible(false)` +
//! `fixed_rect(...)`——本质是**不可拖动的固定浮层**，却要为 Window 的
//! 拖放 / 缩放 / 折叠 / 标题栏四套机制付全部代价：
//!
//! - `Resize` 交互层包裹整个内容（egui 0.29.1 `window.rs:549`）；
//! - `paint_resize_corner` / `paint_frame_interaction` 每窗算一遍角落与拖动区；
//! - `where_to_put_header_background` 留一个 `Shape::Noop` 占位；
//! - 内容一旦 exceed `min_size` 就会被撑开，于是不得不再加
//!   `.max_size(vec2(rect.width , min_size.y + 28.0))` —— 那个 `+ 28.0`
//!   是 `font_height + 2×window_margin` 的硬编码快照，与 `TITLE_H` 无关。
//!
//! # 为什么位置不写在规格里
//!
//! 原先 23 个弹窗各自钉死一对绝对坐标，窗口一大一小的机器上它们会跑到
//! 边角甚至屏外。这里改成**只存尺寸**，位置统一由 `screen_rect .center `
//! 推算：屏幕怎么变，弹窗永远在正中。
//!
//! # 为什么「零像素替换」做不到
//!
//! `egui::containers::window::TitleBar` 是私有类型，`Area` 无法复刻它画的
//! 标题栏。所以从 `Window` 迁到自绘**必然改变像素**，没有第三条路。这也正是
//! 本仓此前「风格不统一」的物理来源：23 个弹窗是 egui 原生深色标题条，
//! 而 `confirm` / `confirm_paste` 是自绘的 macOS 浅色条 + 三色圆点。
//!
//! 于是本模块的目标不是「像素不变的抽象」，而是**一处定义、全部一致**。

use eframe::egui;

// 从本模块再导出：调用方只需 import 一处，不会有人从别的路径摸到规格表。
pub(in crate::ui) use crate::ui::kit::spec::DialogId;

use crate::ui::to_color32;

/// 标题条高（逻辑点，量自视觉基线）。
pub(in crate::ui) const TITLE_H: f32 = 26.0;
/// 窗口圆角。
pub(in crate::ui) const ROUNDING: f32 = 8.0;
/// 内容默认内边距。
pub(in crate::ui) const PAD: f32 = 12.0;

/// chrome 配色：颜色只在这里出现一次，不再散落到各弹窗。
#[derive(Clone, Copy)]
pub(in crate::ui) struct Chrome {
    /// 标题条底色。
    pub title_bar: u32,
    /// 三色圆点（左起）。
    pub traffic: [u32; 3],
    /// 标题文字色。`None` = 不画标题（确认弹窗族）。
    pub title_text: Option<u32>,
    /// 关闭图标色。`None` = 不画右上角 ×（确认弹窗族）。
    pub close: Option<u32>,
}

impl Chrome {
    /// macOS 三色圆点（红 / 灰 / 灰），带标题文字与关闭 ×。
    pub(in crate::ui) const TRAFFIC: Self = Self {
        title_bar: 0xECECEC,
        traffic: [0xFF5F57, 0xC5C5C5, 0xC5C5C5],
        title_text: Some(0x2A2A2A),
        close: Some(0x6E6E6E),
    };

    /// 关闭确认 / 粘贴确认：同样的浅色条与红绿灯，但**没有**标题文字与 ×。
    /// 这是金标准的样子；它让这两个弹窗也能走 `DialogWindow`，不再单独用
    /// 原生 `egui::Window` 画一套壳。
    pub(in crate::ui) const LIGHT_BARE: Self = Self {
        title_bar: 0xECECEC,
        traffic: [0xFF5F57, 0xC5C5C5, 0xC5C5C5],
        title_text: None,
        close: None,
    };
}

/// 对话框的**统一表面**：底色 + 圆角 + 1pt 描边 + 投影。
///
/// 三个弹窗族（`DialogWindow` / `confirm` / `confirm_paste`）共用这一处。
/// 此前三份 `Frame::none .fill(..).rounding(8).shadow(..)` 是各抄一遍的，
/// 于是「给弹窗加描边」这种改动只加到一处，另两处仍是同色无边界——正是
/// 用户反复说的「弹窗和主体一个颜色，区分不出来」。
///
/// # 为什么必须有描边
///
/// 深色主题里投影是黑的、主体被遮罩压得更黑，投影落在上面等于没有。弹窗边界
/// 的**唯一**稳定信号是这条比弹窗本体更亮的线：`CHROME_SCALE.border`（#444444）
/// 比弹窗底（#1C1C1C）亮，也比被压暗后的主体（#090909）亮得多。
///
/// 描边不参与布局：egui 的 `Frame::total_margin` 只算内外边距，`stroke` 以边线
/// 为中心向两侧各画一半，所以加这一条不会让任何弹窗的尺寸或内容位置移动。
pub(in crate::ui) fn dialog_frame() -> egui::Frame {
    frame_with_border(true)
}

/// 与 [`dialog_frame`] 逐像素相同、但没有那圈描边。
/// 给确认弹窗族用：它们的视觉基线无描边，加一圈会立刻改基线。
pub(in crate::ui) fn dialog_frame_plain() -> egui::Frame {
    frame_with_border(false)
}

fn frame_with_border(border: bool) -> egui::Frame {
    let frame = egui::Frame::none()
        .fill(dialog_fill())
        .rounding(ROUNDING)
        .shadow(egui::Shadow {
            offset: egui::vec2(0.0, 6.0),
            blur: 18.0,
            spread: 0.0,
            color: egui::Color32::from_black_alpha(110),
        });
    if border {
        frame.stroke(egui::Stroke::new(
            1.0,
            to_color32(term_core::theme::rgb(
                term_core::theme::DESIGN_TOKENS.border,
            )),
        ))
    } else {
        frame
    }
}

/// 弹窗**底面**（只有底色，没有圆角 / 描边 / 投影）。
///
/// 给「系统外框的独立窗口」用：那种窗口的边界由窗管画，里面再套一圈 1pt 描边
/// 与 8pt 圆角就成了「窗里还有一扇窗」。底色必须是同一个，否则新建会话与
/// 属性弹窗并排就是两种黑。
pub(in crate::ui) fn dialog_surface() -> egui::Frame {
    egui::Frame::none().fill(dialog_fill())
}

/// 弹窗底色。三处（自绘外框 / 系统外框正文 / 遮罩对比度测试）共用一个值。
pub(in crate::ui) fn dialog_fill() -> egui::Color32 {
    to_color32(term_core::theme::rgb(
        term_core::theme::DESIGN_TOKENS.panel_bg,
    ))
}

/// 「系统外框」独立窗口的**外框位置**（屏幕坐标）：让 `size` 的内容区正好落在
/// 主窗口正中。
///
/// `ViewportBuilder::with_position` 收的是外框左上角，而模型里所有矩形都是
/// 内容区；macOS 上两者差一个标题栏高度（y 为负）。少这一个减法，窗口会整体
/// 上移一条标题栏。与浮窗 `layout::dock_native` 里的 `builder_position` 同一
/// 口径，区别只是这里只需要 `ctx`、不需要 `TerminalWindow`。
pub(in crate::ui) fn centered_outer_position(
    ctx: &egui::Context,
    size: egui::Vec2,
) -> egui::Pos2 {
    let frame = ctx.input(|i| {
        let vp = i.raw.viewports.get(&egui::ViewportId::ROOT)?;
        Some((vp.inner_rect?, vp.outer_rect?))
    });
    let Some((inner, outer)) = frame else {
        // 还没有 root 视口信息（单测 / 首帧）：交给窗管自己摆，别写一个假坐标。
        return egui::Pos2::ZERO;
    };
    inner.center() - size / 2.0 + (outer.min - inner.min)
}

/// 把弹窗内**所有控件的常态底**拉回输入框底。
///
/// # 为什么必须在弹窗这一层做
///
/// egui 里两种输入控件读的是两个不同的色号：
///
/// - `TextEdit` / `DragValue` 读 `Visuals::extreme_bg_color`（= 输入框底 `#1C1C1C`）；
/// - `ComboBox` 读 `WidgetVisuals::weak_bg_fill`（= 按钮底 `#262626`）。
///
/// 于是同一张表单里下拉框比文本框**亮一档**，看起来像两种控件混排。23 个弹窗
/// 各有各的写法，逐个改调用点既改不完也守不住；而它们全部经过这里，所以把
/// 「弹窗内的控件表面」收在这一处：`weak_bg_fill` → 输入框底，边框不变。
///
/// 按钮会跟着变平（底 = 弹窗底 + 1pt 描边），这正是兼容终端的样子——
/// `Cancel` / `Connect` 都是深底加一圈描边，没有另起一档填充色。
///
/// 悬停 / 按下 / 展开四态**不改**：那是指向反馈，用户 明确过
/// 「蓝底/hover 是指向色，不是固有底色」，这里只统一静止态的表面。
fn unify_control_surface(ui: &mut egui::Ui) {
    let field = ui.visuals().extreme_bg_color;
    // 描边与圆角取 `kit::form` 的那一份：`TextEdit` / `ComboBox` 自己画框，
    // 而设置页的字段框由 `field_box` 画。两边若不是同一圈线、同一个圆角，
    // 同一张弹窗里就会出现「有的框圆有的框方」——用户说的「各不统一」。
    let stroke = super::form::field_stroke();
    let rounding = egui::Rounding::same(super::form::FIELD_ROUNDING);
    let v = ui.visuals_mut();
    for w in [
        &mut v.widgets.noninteractive,
        &mut v.widgets.inactive,
        &mut v.widgets.active,
        &mut v.widgets.open,
    ] {
        w.weak_bg_fill = field;
        w.bg_stroke = stroke;
        w.rounding = rounding;
    }
    // 「按下」与「展开」也只是同一个输入框的两个瞬间，不该跳成另一档按钮灰；
    // 悬停保留 `hover`（#2A2A2A）——那是指向反馈，用户认它。悬停面只补描边与
    // 圆角，填充色留着。
    v.widgets.hovered.bg_stroke = stroke;
    v.widgets.hovered.rounding = rounding;
}

/// 一个固定尺寸、居中、不可拖动的对话框。
///
/// 用法：
///
/// ```ignore
/// let open = DialogWindow::new(DialogId::About, title)
///     .show(ctx, |ui| { /* 业务内容*/ });
/// ```
pub(in crate::ui) struct DialogWindow<'a> {
    id: &'a str,
    title: &'a str,
    /// **只存尺寸**：位置由屏幕中心推算，窗口怎么变都在正中。
    size: egui::Vec2,
    chrome: Chrome,
    /// 要不要画标题条。默认画。
    title_bar: bool,
    /// 内容是否贴边（去掉内边距）。
    tight: bool,
    /// 内容区顶部额外留白（无标题栏弹窗用来校准视觉基线的垂直基准）。
    /// 默认 0；只给 Settings 这类逐像素对齐视觉基线的弹窗设非 0。
    top_pad: f32,
    /// 要不要画描边。确认弹窗族基线无描边，用 `without_border()` 关掉。
    border: bool,
    /// 要不要统一控件表面。确认弹窗的滚动条等控件走原生样式。
    raw_controls: bool,
}

impl<'a> DialogWindow<'a> {
    /// 按 [`DialogId`] 查规格表建窗。几何的唯一出处是 `kit::spec`。
    pub(in crate::ui) fn new(id: DialogId, title: &'a str) -> Self {
        Self::new_sized(id.key(), title, id.size())
    }

    /// 尺寸由调用上下文决定（文件名字数、批量条数）的弹窗走这条。
    pub(in crate::ui) fn new_sized(
        id: &'a str,
        title: &'a str,
        size: egui::Vec2,
    ) -> Self {
        Self {
            id,
            title,
            size,
            chrome: Chrome::TRAFFIC,
            title_bar: true,
            tight: false,
            top_pad: 0.0,
            border: true,
            raw_controls: false,
        }
    }

    /// 换一套 chrome 皮肤。见 [`Chrome::LIGHT_BARE`]。
    pub(in crate::ui) fn with_chrome(mut self, chrome: Chrome) -> Self {
        self.chrome = chrome;
        self
    }

    /// 去掉描边（确认弹窗族的基线是无描边的）。
    pub(in crate::ui) fn without_border(mut self) -> Self {
        self.border = false;
        self
    }

    /// 保留弹窗内控件的原生表面，不跑 `unify_control_surface`。
    pub(in crate::ui) fn raw_controls(mut self) -> Self {
        self.raw_controls = true;
        self
    }

    /// 设置内容区顶部留白（逻辑像素）。用于无标题栏、需对齐视觉基线垂直基准的弹窗。
    pub(in crate::ui) fn with_top_pad(mut self, pad: f32) -> Self {
        self.top_pad = pad;
        self
    }

    /// 去掉标题条与右上角关闭（关闭由页内按钮负责）。
    pub(in crate::ui) fn without_title_bar(mut self) -> Self {
        self.title_bar = false;
        self
    }

    /// 去掉内容内边距：内容可从标题条正下方贴边开始排。
    pub(in crate::ui) fn tight(mut self) -> Self {
        self.tight = true;
        self
    }

    /// 画窗口。返回 `false` 表示点了标题条右上角的关闭。
    pub(in crate::ui) fn show(
        self,
        ctx: &egui::Context,
        add: impl FnOnce(&mut egui::Ui),
    ) -> bool {
        self.show_with_rect(ctx, |ui, _| add(ui))
    }

    /// 同上，但把窗口矩形交给调用方。
    ///
    /// 给少数**用绝对坐标排内容**的弹窗（页脚按钮按视觉基线坐标摆的那类）用：
    /// 位置由中心推算后，原先写死的屏幕坐标全部失效，这些弹窗必须改写成
    /// `win.min + 偏移`，否则按钮会留在原地、窗口却跑到了屏幕中间。
    pub(in crate::ui) fn show_with_rect(
        self,
        ctx: &egui::Context,
        add: impl FnOnce(&mut egui::Ui, egui::Rect),
    ) -> bool {
        let size = self.size;
        let mut open = true;

        egui::Area::new(egui::Id::new(self.id))
            // **中心锚定**而不是 fixed_pos：内容比预期高时 Area 会向下生长，
            // 固定左上角就变成「顶边居中、底边溢出」。锚定中心才能保证
            // 「无论多大，窗口始终在屏幕正中」。
            .anchor(egui::Align2::CENTER_CENTER, egui::Vec2::ZERO)
            .movable(false)
            .fade_in(false)
            .order(egui::Order::Middle)
            .show(ctx, |ui| {
                ui.set_width(size.x);
                // Area 内容起点即窗口左上角；尺寸由规格给定，高度不足时
                // 内容自己撑高，锚定中心会自动把它重新摆正。
                let win = egui::Rect::from_min_size(ui.min_rect().min, size);
                let frame = if self.border {
                    dialog_frame()
                } else {
                    dialog_frame_plain()
                };
                frame.show(ui, |ui| {
                        ui.set_width(size.x);
                        // 设计高度只当**下限**：内容更多时允许变高，锚定中心
                        // 会把增加的高度平摊到上下两侧，视觉仍居中。
                        ui.set_min_height(size.y);
                        if self.title_bar {
                            self.paint_chrome(ui, win, &mut open);
                        }
                        let pad = if self.tight { 0.0 } else { PAD };
                        // 内容区**上**内边距：贴边模式给 0；显式 `with_top_pad`
                        // 用它；其余给 `PAD`（与左/右/下同一个 12）。
                        //
                        // 这里原先恒等于 `top_pad`（默认 0），于是所有带标题栏的
                        // 弹窗正文都紧贴标题条。以前看不出来，是因为
                        // `paint_chrome` 里的关闭按钮用 `allocate_rect` 多推了
                        // 31pt 光标、正好当成留白；改成 `interact` 之后那 31pt
                        // 没了，正文就顶到了标题条上（About 弹窗实测）。
                        let top_pad = if self.tight {
                            0.0
                        } else if self.top_pad > 0.0 {
                            self.top_pad
                        } else {
                            PAD
                        };
                        egui::Frame::none()
                            .inner_margin(egui::Margin {
                                left: pad,
                                right: pad,
                                top: top_pad,
                                bottom: pad,
                            })
                            .show(ui, |ui| {
                                ui.set_width(size.x - pad * 2.0);
                                // 只裁**水平**方向：宽内容（横排复选框、长句）
                                // 不换行时会画到窗口外，把弹窗的视觉中心拉偏。
                                // 垂直方向不能裁——内容比设计尺寸高时窗口要自己
                                // 长高（锚定中心会把多出来的高度平摊到上下），
                                // 裁掉就成了「按钮行消失」。
                                let top = if self.title_bar { TITLE_H } else { 0.0 };
                                let bottom = ui.ctx().screen_rect().bottom();
                                ui.set_clip_rect(egui::Rect::from_min_max(
                                    egui::pos2(win.min.x + pad, win.min.y + top),
                                    egui::pos2(win.max.x - pad, bottom),
                                ));
                                if !self.raw_controls {
                                    unify_control_surface(ui);
                                }
                                add(ui, win);
                            });
                    });
            });

        open
    }

    /// 标题条 + 圆点 + 标题文字 + 右上角关闭。画在内容之前、背景之上。
    fn paint_chrome(&self, ui: &mut egui::Ui, rect: egui::Rect, open: &mut bool) {
        let painter = ui.painter().clone();
        // 标题条：只有上两角是圆的。
        painter.rect_filled(
            egui::Rect::from_min_size(rect.min, egui::vec2(rect.width(), TITLE_H)),
            egui::Rounding {
                nw: ROUNDING,
                ne: ROUNDING,
                sw: 0.0,
                se: 0.0,
            },
            to_color32(term_core::theme::rgb(self.chrome.title_bar)),
        );
        for (i, x) in [16.0, 34.0, 52.0].iter().enumerate() {
            painter.circle_filled(
                rect.min + egui::vec2(*x, TITLE_H / 2.0),
                5.0,
                to_color32(term_core::theme::rgb(self.chrome.traffic[i])),
            );
        }
        if let Some(title_text) = self.chrome.title_text {
            painter.text(
                rect.center_top() + egui::vec2(0.0, TITLE_H / 2.0),
                egui::Align2::CENTER_CENTER,
                self.title,
                crate::ui::type_scale::heading(),
                to_color32(term_core::theme::rgb(title_text)),
            );
        }

        // 右上角关闭：命中区 28×TITLE_H，图标用两条线画。
        // `Chrome::LIGHT_BARE` 两处都是 None：确认弹窗没有标题文字与 ×。
        let close_rect = egui::Rect::from_min_size(
            egui::pos2(rect.right() - 28.0, rect.top()),
            egui::vec2(28.0, TITLE_H),
        );
        // 用 `interact` 而不是 `allocate_rect`：后者会推进布局光标（rect + 行距），
        // 让标题条实际吃掉 26+行距 的高度，内容再 `add_space(TITLE_H)` 就多出
        // 一整条空白带（会话弹窗实测：标题条下 29pt 空带，整窗比规格高 29）。
        // 关闭按钮只是标题条上的一个命中区，不参与内容流。
        if let Some(close_color) = self.chrome.close {
            let resp = ui.interact(
                close_rect,
                ui.id().with("dialog_close"),
                egui::Sense::click(),
            );
            if resp.hovered() {
                painter.rect_filled(
                    close_rect,
                    egui::Rounding {
                        nw: 0.0,
                        ne: ROUNDING,
                        sw: 0.0,
                        se: 0.0,
                    },
                    egui::Color32::from_black_alpha(20),
                );
            }
            let c = close_rect.center();
            let s = 4.5;
            let stroke = egui::Stroke::new(1.2, to_color32(term_core::theme::rgb(close_color)));
            painter.line_segment([c + egui::vec2(-s, -s), c + egui::vec2(s, s)], stroke);
            painter.line_segment([c + egui::vec2(s, -s), c + egui::vec2(-s, s)], stroke);
            if resp.clicked() {
                *open = false;
            }
        }

        // 内容让出标题条；左右与底部内边距由 `show` 的 Frame 负责。
        // `LIGHT_BARE` 的正文是**绝对定位**的（确认弹窗），若这里再推进
        // 26pt 光标，Area 会比规格高 26、锚定中心后整窗上移 13pt。
        if self.chrome.title_text.is_some() || self.chrome.close.is_some() {
            ui.add_space(TITLE_H);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn size_is_carried_verbatim() {
        // 规格只描述尺寸、不描述位置：位置由 `Align2::CENTER_CENTER` 锚定，
        // 任何把坐标写回规格的改动都应该在这里被看见。
        let w = DialogWindow::new_sized("x", "T", egui::vec2(350.0, 220.0));
        assert_eq!(w.size, egui::vec2(350.0, 220.0));
    }

    #[test]
    fn light_bare_chrome_has_dots_but_no_text_or_close() {
        // 确认弹窗族复用 DialogWindow 的窗口壳，但基线里没有标题文字与 ×。
        assert_eq!(Chrome::LIGHT_BARE.title_bar, Chrome::TRAFFIC.title_bar);
        assert_eq!(Chrome::LIGHT_BARE.traffic, Chrome::TRAFFIC.traffic);
        assert!(Chrome::LIGHT_BARE.title_text.is_none());
        assert!(Chrome::LIGHT_BARE.close.is_none());
        assert!(Chrome::TRAFFIC.title_text.is_some());
        assert!(Chrome::TRAFFIC.close.is_some());
    }

    #[test]
    fn confirm_shell_keeps_its_plain_frame() {
        let w = DialogWindow::new_sized("confirm_close", "", egui::vec2(400.0, 176.0))
            .with_chrome(Chrome::LIGHT_BARE)
            .without_border()
            .raw_controls()
            .tight();
        assert!(!w.border);
        assert!(w.raw_controls);
        assert!(w.tight);
        assert!(w.chrome.title_text.is_none());
    }

    #[test]
    fn defaults_draw_title_bar_and_pad() {
        // 默认形态是「带标题条 + 有内边距」；`without_title_bar` / `tight`
        // 必须是显式 opt-in，否则一次改动会把 Preferences 那类界面带歪。
        let w = DialogWindow::new_sized("x", "T", egui::vec2(100.0, 100.0));
        assert!(w.title_bar);
        assert!(!w.tight);
        let bare = w.without_title_bar().tight();
        assert!(!bare.title_bar);
        assert!(bare.tight);
    }
}

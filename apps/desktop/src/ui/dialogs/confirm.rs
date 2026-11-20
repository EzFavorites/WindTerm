//! 关闭确认弹窗（关闭软件 / 关闭会话）。
//!
//! 视觉基线（2026-11-08 用户提供两张截图）：macOS 风格小窗——浅色标题条 +
//! 红绿灯（红 + 两颗灰）、深色正文（与 `panel_bg` 同源）、黄色警告三角、
//! 等宽文案、`Don't show this message again` 复选框、右下 `No`（蓝描边
//! 默认，首字母下划线）/ `Yes`（灰描边，首字母下划线）。关窗口的文案
//! 两行，关会话的一行、窗口也矮一截——两种高度都按视觉基线量出。
//!
//! # 拦截点（R5：每个入口都拦，Yes 真关、No 真回程）
//!
//! | 入口 | 位置 | 弹窗 |
//! |---|---|---|
//! | 标签 × / 菜单 Close | `tab_menu::close_tab_with_history` | 会话 |
//! | 菜单 Close View | `effects::close_active_view` | 会话 |
//! | 菜单 Close All Views | `effects::close_all_views` | 会话 |
//! | Exit / 系统关闭钮 / Cmd+Q | `draw()` 顶部 `close_requested` | 软件 |
//!
//! 窗口关闭那条走 `ViewportCommand::CancelClose`（eframe 在 `update` 返回后
//! 检查该命令，见 eframe 0.29 `epi_integration.rs`）：拦下来就地弹窗，Yes
//! 时置 `close_confirmed` 再发 Close，下一次 `close_requested` 不再拦。
//!
//! # 状态归属
//!
//! [`ConfirmState`] 挂在 `TerminalWindow` 上，与 `connector` 同类：它是
//! 「一次待执行的关闭副作用」，不进 `ViewState`（那会让 reducer 认识
//! `usize` 标签下标与视口命令）。两个「不再提示」抑制位与关多标签的确认开关
//! 是 prefs 缓存，勾选即刻 `prefs::save`（读回在 `construct.rs`），与
//! `layout.json` 分开。

use crate::ui::harness;
use crate::ui::*;

/// 要关的东西：决定文案、窗口高度与 Yes 的落点。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::ui) enum ConfirmClose {
    /// 关闭整个窗口（软件）：两行文案，含「有会话正在打开」。
    Window,
    /// 关闭下标 `usize` 的那条会话。
    Tab(usize),
    /// 关闭全部会话（Close All Views）。
    AllTabs,
}

impl ConfirmClose {
    /// 视觉基线文案。英文原文就是 key（`tr` 的约定），返回值进 `self.tr()`。
    /// 关窗口两行；关会话（单条或全部）一行。
    fn message_lines(self) -> (&'static str, Option<&'static str>) {
        match self {
            ConfirmClose::Window => (
                "One or more sessions are opening,",
                Some("Do you want to close the current window?"),
            ),
            ConfirmClose::Tab(_) | ConfirmClose::AllTabs => {
                ("Do you want to close the current session?", None)
            }
        }
    }

    /// 两种窗口高（逻辑点，量自视觉基线）：两行的高 14，一行矮一截。
    fn height(self) -> f32 {
        match self {
            ConfirmClose::Window => 176.0,
            ConfirmClose::Tab(_) | ConfirmClose::AllTabs => 162.0,
        }
    }
}

/// 确认弹窗的跨帧状态（投影层，与 `connector` 同类，不进 `ViewState`）。
#[derive(Debug)]
pub(in crate::ui) struct ConfirmState {
    /// 待确认的关闭（`None` = 没有弹窗）。
    pub(in crate::ui) pending: Option<ConfirmClose>,
    /// 复选框的跨帧缓冲：弹窗打开期间的勾选态。
    pub(in crate::ui) check: bool,
    /// 窗口关闭已被确认：放行下一次 `close_requested`，不再拦。
    pub(in crate::ui) close_confirmed: bool,
    /// 「关闭窗口不再提示」（prefs.json 持久化）。
    pub(in crate::ui) suppress_window: bool,
    /// 「关闭会话不再提示」（prefs.json 持久化）。
    pub(in crate::ui) suppress_session: bool,
    /// `Behaviour ▸ Confirm before closing multiple tabs`（prefs.json 持久化）。
    ///
    /// 门控（`should_confirm_close`）读的是这一份缓存而不是每帧读盘：关标签是
    /// 一条同步路径，不能为了一个 bool 去碰文件。设置页改完必须**同一帧**写回
    /// 这里，否则就是「勾了没反应」的假开关（I5 / R5）。
    pub(in crate::ui) multi_tabs: bool,
}

impl Default for ConfirmState {
    /// 手写而不是 `derive(Default)`：结构体默认的 `false` 会让「忘了从 prefs
    /// 读回」变成「关多标签不再确认」——一个漏赋值就悄悄丢会话。除了这个开关，
    /// 其余字段的默认值都是「安静、不抑制」。
    fn default() -> Self {
        Self {
            pending: None,
            check: false,
            close_confirmed: false,
            suppress_window: false,
            suppress_session: false,
            multi_tabs: true,
        }
    }
}

impl ConfirmState {
    /// 启动时从 prefs 读回两个抑制位与关多标签的确认开关（读不到就全默认）。
    pub(in crate::ui) fn from_prefs(prefs: &term_runtime::prefs::Prefs) -> Self {
        Self {
            suppress_window: prefs.suppress_close_window,
            suppress_session: prefs.suppress_close_session,
            multi_tabs: prefs.confirm_close_multi_tabs,
            ..Self::default()
        }
    }
}

/// 「这次关标签要不要先问一句」——三条关闭路径（标签 × / 菜单 Close View /
/// Close All Views）共用的唯一判据。
///
/// 抽成不依赖 `TerminalWindow` 的自由函数是为了能逐项钉真值表：这条规则错了
/// 的症状是「会话没了」，而原因（少看/多看一个 bool）离现场很远。
///
/// **只剩一条标签时不看 `multi`**：字段名说的是 *multiple*，拿它去管最后一条
/// 等于偷偷减掉现有的确认网。那一格仍归 `suppress`（用户在弹窗里亲手勾的
/// 「不再提示」）管。
pub(in crate::ui) fn should_confirm_close(open_tabs: usize, suppress: bool, multi: bool) -> bool {
    if suppress {
        return false;
    }
    if open_tabs > 1 {
        multi
    } else {
        true
    }
}

// ── 视觉常量（逻辑点，量自两张视觉基线）────────────────────────────────

/// 弹窗宽。两种高度共用一个宽。
const WIN_W: f32 = 400.0;
/// 标题条高。
const TITLE_H: f32 = 26.0;
/// 正文行高（两行文案的中心距）。
const LINE_STEP: f32 = 17.0;
/// 等宽正文字号。视觉基线比例：最长一行 41 字符要在 86..400 的宽度里
/// 放下（14 号等宽一行约 344 点，会溢出窗口右沿——实测过）。
const TEXT_SIZE: f32 = crate::ui::type_scale::SIZE_BODY;

const TITLE_BAR: u32 = 0xECECEC;
const TRAFFIC_RED: u32 = 0xFF5F57;
const TRAFFIC_GRAY: u32 = 0xC5C5C5;
const TEXT: u32 = 0xF0F0F0;
const WARN_YELLOW: u32 = 0xF3BE25;
const CHECK_BORDER: u32 = 0xCFCFCF;

/// 十六进制 → 颜色，与 `harness::canvas_bg` 同一条换算路径。
fn col(hex: u32) -> egui::Color32 {
    crate::theme::to_color32(term_core::theme::rgb(hex))
}

/// 弹窗的固定矩形：在视觉基线主窗口（1280×760）里居中。夹具出图与交互路径
/// 用同一个位置——逐字节可复现，不依赖屏幕实际尺寸（About 的先例）。
fn win_rect(kind: ConfirmClose) -> egui::Rect {
    let h = kind.height();
    egui::Rect::from_min_size(
        egui::pos2((1280.0 - WIN_W) / 2.0, (760.0 - h) / 2.0),
        egui::vec2(WIN_W, h),
    )
}

impl TerminalWindow {
    /// 每帧最先跑：键盘模态 + 系统关闭请求的拦截。放在 `draw()` 顶部是
    /// 因为模态要赶在终端吃到键事件**之前**把键拿走。
    pub(in crate::ui) fn confirm_pre_update(&mut self, ctx: &egui::Context) {
        if self.confirm.pending.is_some() {
            // 视觉基线把 N / Y 首字母加了下划线（键盘加速器）；No 同时是
            // 默认键（蓝描边），所以 Enter 也归 No，Esc 同。
            let (yes_k, no_k) = ctx.input(|i| {
                (
                    i.key_pressed(egui::Key::Y),
                    i.key_pressed(egui::Key::N)
                        || i.key_pressed(egui::Key::Enter)
                        || i.key_pressed(egui::Key::Escape),
                )
            });
            // 真模态：剩下的键事件不再喂给终端与输入框（弹窗没有 TextEdit，
            // 清掉 Key/Text/Ime 不影响自己的交互）。
            ctx.input_mut(|i| {
                i.events
                    .retain(|e| !matches!(e, egui::Event::Key { .. } | egui::Event::Text(_)))
            });
            if yes_k || no_k {
                self.confirm_resolve(yes_k);
                return;
            }
        }
        // 系统关闭请求：确认过 / 已抑制 → 放行（不发 CancelClose，eframe
        // 就会关窗口）；否则拦成本弹窗。Cmd+Q、标题栏红钮、菜单 Exit 全走这。
        if self.confirm.close_confirmed {
            return;
        }
        if ctx.input(|i| i.viewport().close_requested()) {
            if self.confirm.suppress_window {
                self.confirm.close_confirmed = true;
                self.save_layout();
            } else {
                ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
                if !matches!(self.confirm.pending, Some(ConfirmClose::Window)) {
                    self.confirm.pending = Some(ConfirmClose::Window);
                    self.confirm.check = false;
                }
            }
        }
    }

    /// Yes / No 共用的落点。`No` 是「真回程」：收掉弹窗，什么都不做。
    pub(in crate::ui) fn confirm_resolve(&mut self, yes: bool) {
        let Some(kind) = self.confirm.pending.take() else {
            return;
        };
        if !yes {
            return;
        }
        match kind {
            ConfirmClose::Window => self.exit_application(),
            ConfirmClose::Tab(i) => self.confirm_close_tab(i),
            ConfirmClose::AllTabs => self.confirm_close_all_views(),
        }
    }

    /// `close_tab_with_history` 的无拦截版：Yes 之后的真关闭。
    pub(in crate::ui) fn confirm_close_tab(&mut self, index: usize) {
        if let Some(tab) = self.tabs.get(index) {
            self.closed_sessions.push(tab.config.name.clone());
        }
        self.close_tab(index);
        self.save_layout();
    }

    /// `close_all_views` 的无拦截版：Yes 之后的真关闭。
    pub(in crate::ui) fn confirm_close_all_views(&mut self) {
        self.close_all_tabs();
        self.save_layout();
    }

    /// 勾选「不再提示」即刻落盘（load-modify-save，别把语言清掉）。
    fn save_confirm_prefs(&self) {
        // load-modify-save：整份重建会把语言与 Session Settings ▸ Logging
        // 那五项一起清掉（prefs 字段只增不减，这里必须跟着改）。
        let mut prefs = term_runtime::prefs::load().unwrap_or_default();
        prefs.language = self.view.lang.tag().to_string();
        prefs.suppress_close_window = self.confirm.suppress_window;
        prefs.suppress_close_session = self.confirm.suppress_session;
        // 三个位一起写：这条弹窗的复选框与设置页的勾选共用同一份缓存，
        // 谁落盘都带上另外两个的当前值，免得一次保存把刚改的那个冲回去。
        prefs.confirm_close_multi_tabs = self.confirm.multi_tabs;
        term_runtime::prefs::save(&prefs);
    }

    /// 弹窗本体。没有 `pending` 时一行都不画。
    pub(in crate::ui) fn draw_confirm_close_dialog(&mut self, ctx: &egui::Context) {
        let Some(kind) = self.confirm.pending else {
            return;
        };
        // 文案先行：闭包里不能再借 `self.tr()`（about.rs 的教训）。
        let (line1, line2) = kind.message_lines();
        let line1 = self.tr(line1);
        let line2 = line2.map(|s| self.tr(s));
        let check_label = self.tr("Don't show this message again");
        let yes_text = self.tr("Yes");
        let no_text = self.tr("No");

        let h = kind.height();
        let mut yes = false;
        let mut no = false;
        let id = egui::Id::new("confirm_close");

        if harness::fixture_mode() {
            // 夹具热身：与 `harness::StableWindow` 同一条结论——首帧是
            // sizing pass，形状全是 Noop，必须先播种 AreaState（见其注释）。
            egui::Window::new("confirm_close")
                .id(id)
                .collapsible(false)
                .resizable(false)
                .fade_in(false)
                .fixed_rect(win_rect(kind))
                .title_bar(false)
                .frame(egui::Frame::none())
                .show(ctx, |ui| ui.set_min_size(egui::vec2(WIN_W, h)));
        }

        egui::Window::new("confirm_close")
            .id(id)
            .collapsible(false)
            .resizable(false)
            .fade_in(false)
            .fixed_rect(win_rect(kind))
            .title_bar(false)
            .frame(
                egui::Frame::none()
                    .fill(col(term_core::theme::DESIGN_TOKENS.panel_bg))
                    .rounding(8.0)
                    .shadow(egui::Shadow {
                        offset: egui::vec2(0.0, 6.0),
                        blur: 18.0,
                        spread: 0.0,
                        color: egui::Color32::from_black_alpha(110),
                    }),
            )
            .show(ctx, |ui| {
                ui.set_min_size(egui::vec2(WIN_W, h));
                let o = ui.min_rect().min;
                // 全部绘制裁剪在窗口矩形内（painter.text 不自动裁剪，
                // 长文案溢出右沿的教训）。Painter 本体廉价克隆。
                let p = ui.painter_at(egui::Rect::from_min_size(o, egui::vec2(WIN_W, h)));

                // 标题条 + 红绿灯（红 + 两颗灰，量自视觉基线）。
                p.rect_filled(
                    egui::Rect::from_min_size(o, egui::vec2(WIN_W, TITLE_H)),
                    egui::Rounding {
                        nw: 8.0,
                        ne: 8.0,
                        sw: 0.0,
                        se: 0.0,
                    },
                    col(TITLE_BAR),
                );
                for (i, x) in [16.0, 34.0, 52.0].iter().enumerate() {
                    let c = o + egui::vec2(*x, TITLE_H / 2.0);
                    let hex = if i == 0 { TRAFFIC_RED } else { TRAFFIC_GRAY };
                    p.circle_filled(c, 5.0, col(hex));
                }

                // 警告三角：实心多边形 + 顶点圆盖出圆角（`⋮` 的教训——
                // 别指望字形，用绘图原语）。
                let top = o + egui::vec2(47.0, 44.0);
                let left = o + egui::vec2(24.0, 84.0);
                let right = o + egui::vec2(70.0, 84.0);
                p.add(egui::Shape::convex_polygon(
                    vec![top, right, left],
                    col(WARN_YELLOW),
                    egui::Stroke::NONE,
                ));
                for v in [top, left, right] {
                    p.circle_filled(v, 4.0, col(WARN_YELLOW));
                }
                // 白色感叹号：短棒 + 圆点。
                let bar = egui::Rect::from_center_size(
                    o + egui::vec2(47.0, 66.0),
                    egui::vec2(4.5, 14.0),
                );
                p.rect_filled(bar, 2.25, egui::Color32::WHITE);
                p.circle_filled(o + egui::vec2(47.0, 78.0), 2.5, egui::Color32::WHITE);

                // 正文（等宽，左对齐）。
                p.text(
                    o + egui::vec2(86.0, 50.0),
                    egui::Align2::LEFT_CENTER,
                    line1,
                    crate::ui::type_scale::font(TEXT_SIZE),
                    col(TEXT),
                );
                if let Some(line2) = line2 {
                    p.text(
                        o + egui::vec2(86.0, 50.0 + LINE_STEP),
                        egui::Align2::LEFT_CENTER,
                        line2,
                        crate::ui::type_scale::font(TEXT_SIZE),
                        col(TEXT),
                    );
                }

                // 复选框（视觉基线里默认勾上的是用户的选择；交互初值不勾）。
                let check_cy = if line2.is_some() { 90.0 } else { 77.0 };
                let box_rect = egui::Rect::from_center_size(
                    o + egui::vec2(99.5, check_cy),
                    egui::vec2(15.0, 15.0),
                );
                let check_resp = ui.allocate_rect(box_rect, egui::Sense::click());
                // 视觉基线：深底、白框、白勾（不是白底深勾）。
                p.rect_stroke(
                    box_rect,
                    3.0,
                    egui::Stroke::new(1.2, col(CHECK_BORDER)),
                );
                if self.confirm.check {
                    let a = box_rect.left_top() + egui::vec2(2.5, 8.0);
                    let b = box_rect.left_top() + egui::vec2(6.0, 11.5);
                    let c = box_rect.left_top() + egui::vec2(12.5, 3.5);
                    p.add(egui::Shape::line(
                        vec![a, b, c],
                        egui::Stroke::new(1.8, egui::Color32::WHITE),
                    ));
                }
                if check_resp.clicked() {
                    self.confirm.check = !self.confirm.check;
                    self.save_confirm_prefs();
                }
                p.text(
                    o + egui::vec2(114.0, check_cy),
                    egui::Align2::LEFT_CENTER,
                    check_label,
                    crate::ui::type_scale::font(TEXT_SIZE),
                    col(TEXT),
                );

                // 右下两颗按钮：No（蓝描边默认）/ Yes（灰描边），
                // 首字母下划线（视觉基线）。尺寸/配色/字体全查规格表，绘制
                // 走 kit::footer 原语；这里只算坐标与收点击。
                let by = h - 48.0;
                let no_spec = crate::ui::kit::spec::FooterButton::ConfirmNo.spec();
                let yes_spec = crate::ui::kit::spec::FooterButton::ConfirmYes.spec();
                let no_rect =
                    egui::Rect::from_min_size(o + egui::vec2(208.0, by), no_spec.size);
                let yes_rect =
                    egui::Rect::from_min_size(o + egui::vec2(296.0, by), yes_spec.size);
                let no_resp = ui.allocate_rect(no_rect, egui::Sense::click());
                let yes_resp = ui.allocate_rect(yes_rect, egui::Sense::click());
                let clip = egui::Rect::from_min_size(o, egui::vec2(WIN_W, h));
                crate::ui::kit::footer::draw(ui, clip, no_rect, no_text, no_spec, no_resp.hovered(), true);
                crate::ui::kit::footer::draw(ui, clip, yes_rect, yes_text, yes_spec, yes_resp.hovered(), true);
                if no_resp.clicked() {
                    no = true;
                }
                if yes_resp.clicked() {
                    yes = true;
                }
            });

        // R5：按钮的真效果只在闭包外落地。
        if yes {
            self.confirm_resolve(true);
        } else if no {
            self.confirm_resolve(false);
        }
    }
}

#[cfg(test)]
#[path = "confirm_tests.rs"]
mod tests;

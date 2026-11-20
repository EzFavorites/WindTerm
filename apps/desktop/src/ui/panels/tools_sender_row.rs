//! Sender 的工具行：五颗图标 + 两组单选 + 两个数字框 + 目标下拉（+ `Selected
//! Sessions` 时的多选一格）+ 那五行菜单。
//!
//! 全部手绘（与 `tool_bar` / `dock_bottom` 同一条路）：**不赌字体**。运行时有个
//! 图标画不出来，按钮就成了一块空白；线段与多边形没有这个问题。
//!
//! 逐像素对照见 `tools_sender.rs` 的模块文档（那张表是总账，这里只放实现）。

use eframe::egui;
use term_core::theme::CHROME_SCALE;

use crate::ui::TerminalWindow;

use super::tools_sender_state::{picked_title, Target};

/// 工具行高（视觉基线 24）。
pub(super) const ROW_H: f32 = 24.0;
/// 五颗图标按钮的点按区宽与间距（视觉基线中心距 28）。
const ICON_W: f32 = 24.0;
pub(super) const ICON_GAP: f32 = 4.0;
/// 图标本身的绘制半宽。
const GLYPH: f32 = 5.0;
/// 分隔线前后的留白。
pub(super) const SEP_PAD: f32 = 5.0;
/// 单选组之间的留白。
pub(super) const GROUP_GAP: f32 = 13.0;
/// 单选圆直径（视觉基线 13）。
pub(super) const RADIO_D: f32 = 13.0;
/// 圆 → 文字。
const RADIO_GAP: f32 = 6.0;
/// 标签 → 它的输入框。
pub(super) const LABEL_GAP: f32 = 8.0;
/// 输入框高 / 内左 padding / 上下箭头列宽。
pub(super) const FIELD_H: f32 = 18.0;
const FIELD_PAD: f32 = 6.0;
const STEP_W: f32 = 12.0;
/// `Count` 框宽（视觉基线 519..594）。
const COUNT_W: f32 = 75.0;
/// `Interval` 框宽（视觉基线 685..784）。
const INTERVAL_W: f32 = 99.0;
/// `Targets` 下拉宽（视觉基线展开的列表 868..1097）。
const TARGET_W: f32 = 229.0;
/// `Targets ▸ Selected Sessions` 右边那一格多选下拉的宽（与 `Targets` 同宽，
/// 两格并排看着是一对）。
pub(super) const PICK_W: f32 = 229.0;
/// 多选下拉的最小可读宽。窄面板下宁可收窄到这个宽，也不许画到裁剪区外——
/// 那一格一旦看不见，「指定会话」就成了一个点了没反应的选项。
const PICK_MIN_W: f32 = 72.0;
/// 运行键的绿。视觉基线 `#008200`，整张图唯一的绿。
const RUN_GREEN: u32 = 0x008200;
/// 停止键的灰。视觉基线 `#646464`——比图标暗一档，表示「现在没在跑」。
const STOP_GREY: u32 = 0x646464;

/// 工具行上的意图。绘制只产出它，落地统一在 `TerminalWindow::apply_cmd`。
pub(super) enum Cmd {
    Run,
    Stop,
    /// `＋`：多开一页。
    NewTab,
    /// `−`：关掉当前页（只剩一页时不生效）。
    CloseTab,
    Clear,
    Encoding(bool),
    By(bool),
    Count(i32),
    Interval(i32),
    Target(Target),
    OpenTargets,
    /// 展开 / 收起「指定会话」那一格的多选弹层。
    OpenPicked,
    /// 勾 / 取消勾弹层里的第 `usize` 行（本帧标签序号）。
    PickToggle(usize),
}

/// 工具行的绘制结果：下拉框的锚点矩形 + 本帧的意图。
pub(super) struct RowOut {
    pub anchor: egui::Rect,
    /// `Target::Selected` 时右侧多选下拉的矩形（`None` = 本帧没画它）。
    pub picked: Option<egui::Rect>,
    pub cmd: Option<Cmd>,
}

/// 工具行。只读 `win`——本帧所有状态改动都攒成 `Cmd`，由调用方一次性落地。
pub(super) fn tool_row(win: &TerminalWindow, ui: &mut egui::Ui, rect: egui::Rect) -> RowOut {
    let mut row = Row::new(ui, rect);
    let mut cmd = None;
    let s = win.sender_state();

    if row.icon("sender_run", Glyph::Run, crate::theme::chrome(RUN_GREEN)) {
        cmd = Some(Cmd::Run);
    }
    row.gap(ICON_GAP);
    let stop_c = crate::theme::chrome(STOP_GREY);
    if row.icon("sender_stop", Glyph::Stop, stop_c) {
        cmd = Some(Cmd::Stop);
    }
    row.gap(ICON_GAP);
    let icon_c = crate::theme::chrome(CHROME_SCALE.icon);
    if row.icon("sender_add", Glyph::Plus, icon_c) {
        cmd = Some(Cmd::NewTab);
    }
    row.gap(ICON_GAP);
    if row.icon("sender_del", Glyph::Minus, icon_c) {
        cmd = Some(Cmd::CloseTab);
    }
    row.gap(ICON_GAP);
    if row.icon("sender_clear", Glyph::Cross, icon_c) {
        cmd = Some(Cmd::Clear);
    }
    row.gap(SEP_PAD);
    row.separator();
    row.gap(SEP_PAD);

    if row.radio(win.tr("Text"), !s.hex) {
        cmd = Some(Cmd::Encoding(false));
    }
    row.gap(GROUP_GAP);
    if row.radio(win.tr("Hex"), s.hex) {
        cmd = Some(Cmd::Encoding(true));
    }
    row.gap(GROUP_GAP);
    let text_c = row.text_c;
    row.label(win.tr("By:"), text_c);
    row.gap(LABEL_GAP);
    if row.radio(win.tr("Line"), !s.by_char) {
        cmd = Some(Cmd::By(false));
    }
    row.gap(GROUP_GAP);
    if row.radio(win.tr("Char"), s.by_char) {
        cmd = Some(Cmd::By(true));
    }
    row.gap(GROUP_GAP);

    row.label(win.tr("Count:"), text_c);
    row.gap(LABEL_GAP);
    let count_text = format!("{}", s.count);
    let (up, down) = row.field(&count_text, COUNT_W, "sender_count");
    if up {
        cmd = Some(Cmd::Count(1));
    } else if down {
        cmd = Some(Cmd::Count(-1));
    }
    row.gap(GROUP_GAP);

    row.label(win.tr("Interval:"), text_c);
    row.gap(LABEL_GAP);
    let iv_text = format!("{:.2}s", s.interval);
    let (up, down) = row.field(&iv_text, INTERVAL_W, "sender_interval");
    if up {
        cmd = Some(Cmd::Interval(1));
    } else if down {
        cmd = Some(Cmd::Interval(-1));
    }
    row.gap(GROUP_GAP);

    row.label(win.tr("Targets:"), text_c);
    row.gap(LABEL_GAP);
    let (resp, anchor) = row.dropdown(win.tr(s.target.title()), TARGET_W, "sender_targets");
    if resp.clicked() {
        cmd = Some(Cmd::OpenTargets);
    }

    // 「指定会话」多出来那一格：勾了谁只能看得见才有意义，所以它紧跟在
    // `Targets` 右边、只在 `Selected Sessions` 这一档出现。
    let mut picked = None;
    if s.target == Target::Selected {
        row.gap(GROUP_GAP);
        let names = win.sender_picked_names();
        let w = (rect.right() - 8.0 - row.x).min(PICK_W);
        // 剩余宽度不够一格可读的下拉时宁可不画（窄面板），也不画到裁剪区外。
        if w >= PICK_MIN_W {
            let title = picked_title(&names, win.tr("None"));
            let (resp, r) = row.dropdown(&title, w, "sender_picked");
            picked = Some(r);
            if resp.clicked() {
                cmd = Some(Cmd::OpenPicked);
            }
        }
    }

    // 一次性提示贴在行尾：工具行左侧已经排满，右边还剩一截空白。
    if let Some(note) = &s.note {
        let c = egui::Color32::from_rgb(0xE0, 0x60, 0x60);
        let g = row.p.layout_no_wrap(note.clone(), row.font.clone(), c);
        if row.x + g.size().x + 8.0 <= rect.right() {
            row.p.galley(
                egui::pos2(rect.right() - 8.0 - g.size().x, row.cy - g.size().y / 2.0),
                g,
                c,
            );
        }
    }
    RowOut {
        anchor,
        picked,
        cmd,
    }
}

/// 工具行上的五颗图标。`pub(super)`：脚本面板的工具行也「不赌字体」地画同一套。
#[derive(Clone, Copy)]
pub(super) enum Glyph {
    Run,
    Stop,
    Plus,
    Minus,
    Cross,
}

/// 工具行的游标 + 绘制原语。
///
/// 2026-11-17：`pub(super)` 放开——Sender 与 Script 两个面板的工具行共用这一套
/// 原语（同样的图标尺寸、同样的下拉框与竖分隔线），各画各的会长出两种行。
pub(super) struct Row<'a> {
    ui: &'a mut egui::Ui,
    p: egui::Painter,
    font: egui::FontId,
    pub(super) text_c: egui::Color32,
    cy: f32,
    x: f32,
}

impl<'a> Row<'a> {
    pub(super) fn new(ui: &'a mut egui::Ui, rect: egui::Rect) -> Self {
        Self {
            p: ui.painter_at(rect),
            font: egui::TextStyle::Button.resolve(ui.style()),
            text_c: crate::theme::chrome(CHROME_SCALE.text),
            cy: rect.center().y,
            x: rect.left(),
            ui,
        }
    }

    /// 当前游标（下一个控件从这里起笔）。两个面板的行要按剩余宽分配下拉，
    /// 得先问它。
    pub(super) fn cursor(&self) -> f32 {
        self.x
    }

    /// 一段文字按本行的字体有多宽。排布前预留空间用，不画。
    pub(super) fn text_width(&self, s: &str) -> f32 {
        self.p
            .layout_no_wrap(s.to_string(), self.font.clone(), self.text_c)
            .size()
            .x
    }

    pub(super) fn gap(&mut self, w: f32) {
        self.x += w;
    }

    pub(super) fn hit(&mut self, w: f32, id: impl std::hash::Hash) -> egui::Response {
        let r = egui::Rect::from_min_size(
            egui::pos2(self.x, self.cy - ROW_H / 2.0),
            egui::vec2(w, ROW_H),
        );
        self.x += w;
        self.ui
            .interact(r, egui::Id::new(id), egui::Sense::click())
            .on_hover_cursor(egui::CursorIcon::PointingHand)
    }

    /// 一颗图标按钮。返回是否被点。
    pub(super) fn icon(&mut self, id: &'static str, g: Glyph, c: egui::Color32) -> bool {
        let resp = self.hit(ICON_W, id);
        let k = egui::pos2(self.x - ICON_W / 2.0, self.cy);
        if resp.hovered() {
            self.p
                .rect_filled(resp.rect, 0.0, crate::theme::chrome(CHROME_SCALE.hover));
        }
        paint_glyph(&self.p, k, g, c);
        resp.clicked()
    }

    /// 一根整行高的竖线。
    pub(super) fn separator(&mut self) {
        let x = self.x + 0.5;
        self.p.line_segment(
            [
                egui::pos2(x, self.cy - ROW_H / 2.0),
                egui::pos2(x, self.cy + ROW_H / 2.0),
            ],
            egui::Stroke::new(1.0, crate::theme::chrome(CHROME_SCALE.border)),
        );
        self.x += 1.0;
    }

    /// 一行标签。
    pub(super) fn label(&mut self, s: &str, c: egui::Color32) {
        let g = self.p.layout_no_wrap(s.to_string(), self.font.clone(), c);
        let x = self.x;
        self.x += g.size().x;
        self.p
            .galley(egui::pos2(x, self.cy - g.size().y / 2.0), g, c);
    }

    /// 一颗单选。返回是否被点。
    pub(super) fn radio(&mut self, text: &str, on: bool) -> bool {
        let g = self
            .p
            .layout_no_wrap(text.to_string(), self.font.clone(), self.text_c);
        let w = RADIO_D + RADIO_GAP + g.size().x;
        let resp = self.hit(w, ("sender_radio", text));
        let left = self.x - w;
        let k = egui::pos2(left + RADIO_D / 2.0, self.cy);
        self.p.circle_stroke(
            k,
            RADIO_D / 2.0 - 0.5,
            egui::Stroke::new(1.0, crate::theme::chrome(CHROME_SCALE.border)),
        );
        if on {
            self.p.circle_filled(
                k,
                RADIO_D * 0.28,
                crate::theme::chrome(CHROME_SCALE.text_strong),
            );
        }
        self.p.galley(
            egui::pos2(left + RADIO_D + RADIO_GAP, self.cy - g.size().y / 2.0),
            g,
            self.text_c,
        );
        resp.clicked()
    }

    /// 数字框：边框 + 左侧值 + 右侧上下箭头。返回 (加、减) 是否被点。
    pub(super) fn field(&mut self, value: &str, w: f32, id: &'static str) -> (bool, bool) {
        let r = egui::Rect::from_min_size(
            egui::pos2(self.x, self.cy - FIELD_H / 2.0),
            egui::vec2(w, FIELD_H),
        );
        self.x += w;
        self.p.rect_stroke(
            r,
            0.0,
            egui::Stroke::new(1.0, crate::theme::chrome(CHROME_SCALE.border)),
        );
        let g = self
            .p
            .layout_no_wrap(value.to_string(), self.font.clone(), self.text_c);
        self.p.galley(
            egui::pos2(r.left() + FIELD_PAD, self.cy - g.size().y / 2.0),
            g,
            self.text_c,
        );
        let up_r = egui::Rect::from_min_max(
            egui::pos2(r.right() - STEP_W, r.top()),
            egui::pos2(r.right(), self.cy),
        );
        let dn_r = egui::Rect::from_min_max(
            egui::pos2(r.right() - STEP_W, self.cy),
            egui::pos2(r.right(), r.bottom()),
        );
        let up = self
            .ui
            .interact(up_r, egui::Id::new((id, "up")), egui::Sense::click())
            .on_hover_cursor(egui::CursorIcon::PointingHand);
        let dn = self
            .ui
            .interact(dn_r, egui::Id::new((id, "down")), egui::Sense::click())
            .on_hover_cursor(egui::CursorIcon::PointingHand);
        let c = crate::theme::chrome(CHROME_SCALE.icon);
        paint_tri(&self.p, up_r.center(), true, c);
        paint_tri(&self.p, dn_r.center(), false, c);
        (up.clicked(), dn.clicked())
    }

    /// 下拉框（未展开的样子）。返回 (响应, 矩形)——矩形给弹层当锚点。
    pub(super) fn dropdown(&mut self, text: &str, w: f32, id: &'static str) -> (egui::Response, egui::Rect) {
        let r = egui::Rect::from_min_size(
            egui::pos2(self.x, self.cy - FIELD_H / 2.0),
            egui::vec2(w, FIELD_H),
        );
        self.x += w;
        self.p.rect_stroke(
            r,
            0.0,
            egui::Stroke::new(1.0, crate::theme::chrome(CHROME_SCALE.border)),
        );
        let g = self
            .p
            .layout_no_wrap(text.to_string(), self.font.clone(), self.text_c);
        self.p.galley(
            egui::pos2(r.left() + FIELD_PAD, self.cy - g.size().y / 2.0),
            g,
            self.text_c,
        );
        paint_tri(
            &self.p,
            egui::pos2(r.right() - 10.0, self.cy),
            false,
            crate::theme::chrome(CHROME_SCALE.icon),
        );
        let resp = self
            .ui
            .interact(r, egui::Id::new(id), egui::Sense::click())
            .on_hover_cursor(egui::CursorIcon::PointingHand);
        (resp, r)
    }
}

/// 工具行的图标：全部用线段 / 多边形，不排字。
fn paint_glyph(p: &egui::Painter, k: egui::Pos2, g: Glyph, c: egui::Color32) {
    let st = egui::Stroke::new(1.0, c);
    match g {
        Glyph::Run => {
            // 实心三角：视觉基线里唯一饱和的绿。
            p.add(egui::Shape::convex_polygon(
                vec![
                    egui::pos2(k.x - GLYPH * 0.5, k.y - GLYPH),
                    egui::pos2(k.x + GLYPH, k.y),
                    egui::pos2(k.x - GLYPH * 0.5, k.y + GLYPH),
                ],
                c,
                egui::Stroke::NONE,
            ));
        }
        Glyph::Stop => {
            p.rect_filled(
                egui::Rect::from_center_size(k, egui::vec2(GLYPH * 1.7, GLYPH * 1.7)),
                0.0,
                c,
            );
        }
        Glyph::Plus => {
            p.line_segment(
                [egui::pos2(k.x - GLYPH, k.y), egui::pos2(k.x + GLYPH, k.y)],
                st,
            );
            p.line_segment(
                [egui::pos2(k.x, k.y - GLYPH), egui::pos2(k.x, k.y + GLYPH)],
                st,
            );
        }
        Glyph::Minus => {
            p.line_segment(
                [egui::pos2(k.x - GLYPH, k.y), egui::pos2(k.x + GLYPH, k.y)],
                st,
            );
        }
        Glyph::Cross => {
            let a = GLYPH * 0.85;
            p.line_segment(
                [egui::pos2(k.x - a, k.y - a), egui::pos2(k.x + a, k.y + a)],
                st,
            );
            p.line_segment(
                [egui::pos2(k.x - a, k.y + a), egui::pos2(k.x + a, k.y - a)],
                st,
            );
        }
    }
}

/// 小三角（上下箭头 / 下拉箭头）。
fn paint_tri(p: &egui::Painter, k: egui::Pos2, up: bool, c: egui::Color32) {
    let a = 3.0;
    let dy = if up { -a } else { a };
    p.add(egui::Shape::convex_polygon(
        vec![
            egui::pos2(k.x - a, k.y - dy * 0.6),
            egui::pos2(k.x + a, k.y - dy * 0.6),
            egui::pos2(k.x, k.y + dy),
        ],
        c,
        egui::Stroke::NONE,
    ));
}

//! 面板工具行的通用绘制原语：手绘图标、游标、单选、下拉、输入框。
//!
//! 这里只有「怎么画」，没有任何面板的业务语义。Sender 与 Script 各自定义
//! 自己的行与命令集合，共同使用这一层——底部只是停靠区，不是把两个面板
//! 的语义揉在一起的地方。

use eframe::egui;
use term_core::theme::CHROME_SCALE;

pub(in crate::ui) const ROW_H: f32 = 24.0;
pub(in crate::ui) const ICON_W: f32 = 24.0;
pub(in crate::ui) const ICON_GAP: f32 = 4.0;
const GLYPH: f32 = 5.0;
pub(in crate::ui) const SEP_PAD: f32 = 5.0;
pub(in crate::ui) const GROUP_GAP: f32 = 13.0;
pub(in crate::ui) const RADIO_D: f32 = 13.0;
const RADIO_GAP: f32 = 6.0;
pub(in crate::ui) const LABEL_GAP: f32 = 8.0;
pub(in crate::ui) const FIELD_H: f32 = 18.0;
const FIELD_PAD: f32 = 6.0;
const STEP_W: f32 = 12.0;

#[derive(Clone, Copy)]
pub(in crate::ui) enum Glyph {
    Run,
    Stop,
    Plus,
    Minus,
    Cross,
    /// 向下箭头：从脚本库选。
    ChevronDown,
    /// 省略号：打开文件选择器 / 更多动作。
    Ellipsis,
    /// 齿轮：本次运行授权。
    Gear,
}

/// 工具行的游标 + 绘制原语。
///
/// 2026-11-17：`pub(super)` 放开——Sender 与 Script 两个面板的工具行共用这一套
/// 原语（同样的图标尺寸、同样的下拉框与竖分隔线），各画各的会长出两种行。
pub(in crate::ui) struct Row<'a> {
    ui: &'a mut egui::Ui,
    pub(in crate::ui) p: egui::Painter,
    pub(in crate::ui) font: egui::FontId,
    pub(in crate::ui) text_c: egui::Color32,
    pub(in crate::ui) cy: f32,
    pub(in crate::ui) x: f32,
}

impl<'a> Row<'a> {
    pub(in crate::ui) fn new(ui: &'a mut egui::Ui, rect: egui::Rect) -> Self {
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
    pub(in crate::ui) fn cursor(&self) -> f32 {
        self.x
    }

    /// 一段文字按本行的字体有多宽。排布前预留空间用，不画。
    pub(in crate::ui) fn text_width(&self, s: &str) -> f32 {
        self.p
            .layout_no_wrap(s.to_string(), self.font.clone(), self.text_c)
            .size()
            .x
    }

    pub(in crate::ui) fn gap(&mut self, w: f32) {
        self.x += w;
    }

    pub(in crate::ui) fn hit(&mut self, w: f32, id: impl std::hash::Hash) -> egui::Response {
        let r = egui::Rect::from_min_size(
            egui::pos2(self.x, self.cy - ROW_H / 2.0),
            egui::vec2(w, ROW_H),
        );
        self.x += w;
        self.ui
            .interact(r, egui::Id::new(id), egui::Sense::click())
            .on_hover_cursor(egui::CursorIcon::PointingHand)
    }

    /// 一颗图标按钮。返回 `Response`，调用方取 `.clicked()`，也能挂 tooltip。
    ///
    /// 返回 `bool` 时谁都加不上悬停说明——脚本面板那颗 `×` 与 Sender 的
    /// `×` 同形却不同义，只能靠 tooltip 区分，所以这里给出完整响应。
    pub(in crate::ui) fn icon(&mut self, id: &'static str, g: Glyph, c: egui::Color32) -> egui::Response {
        let resp = self.hit(ICON_W, id);
        let k = egui::pos2(self.x - ICON_W / 2.0, self.cy);
        if resp.hovered() {
            self.p
                .rect_filled(resp.rect, 0.0, crate::theme::chrome(CHROME_SCALE.hover));
        }
        paint_glyph(&self.p, k, g, c);
        resp
    }

    /// 一根整行高的竖线。
    pub(in crate::ui) fn separator(&mut self) {
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
    pub(in crate::ui) fn label(&mut self, s: &str, c: egui::Color32) {
        let g = self.p.layout_no_wrap(s.to_string(), self.font.clone(), c);
        let x = self.x;
        self.x += g.size().x;
        self.p
            .galley(egui::pos2(x, self.cy - g.size().y / 2.0), g, c);
    }

    /// 一颗单选。返回是否被点。
    pub(in crate::ui) fn radio(&mut self, text: &str, on: bool) -> bool {
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

    /// 单行输入框：用 egui 原生 TextEdit 画在游标位置，和手绘图标同一行。
    ///
    /// 脚本面板的「脚本来源」用它承载**唯一**的路径真相：库选择 / 文件浏览
    /// 都只是把结果写回这个字符串，不再另起一行、另存一份状态。
    pub(in crate::ui) fn text_field(
        &mut self,
        text: &mut String,
        w: f32,
        id: egui::Id,
        hint: &str,
    ) -> egui::Response {
        let r = egui::Rect::from_min_size(
            egui::pos2(self.x, self.cy - FIELD_H / 2.0),
            egui::vec2(w, FIELD_H),
        );
        self.x += w;
        self.ui
            .put(r, egui::TextEdit::singleline(text).id(id).hint_text(hint.to_string()))
    }

    /// 数字框：边框 + 左侧值 + 右侧上下箭头。返回 (加、减) 是否被点。
    pub(in crate::ui) fn field(&mut self, value: &str, w: f32, id: &'static str) -> (bool, bool) {
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
    pub(in crate::ui) fn dropdown(&mut self, text: &str, w: f32, id: &'static str) -> (egui::Response, egui::Rect) {
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
pub(in crate::ui) fn paint_glyph(p: &egui::Painter, k: egui::Pos2, g: Glyph, c: egui::Color32) {
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
        Glyph::ChevronDown => {
            let a = GLYPH * 0.8;
            p.add(egui::Shape::convex_polygon(
                vec![
                    egui::pos2(k.x - a, k.y - a * 0.4),
                    egui::pos2(k.x + a, k.y - a * 0.4),
                    egui::pos2(k.x, k.y + a * 0.8),
                ],
                c,
                egui::Stroke::NONE,
            ));
        }
        Glyph::Ellipsis => {
            for dx in [-GLYPH, 0.0, GLYPH] {
                p.circle_filled(egui::pos2(k.x + dx, k.y), 1.6, c);
            }
        }
        Glyph::Gear => {
            let r = GLYPH * 0.9;
            p.circle_stroke(k, r, st);
            p.circle_stroke(k, GLYPH * 0.3, st);
            for i in 0..4 {
                let a = i as f32 * std::f32::consts::FRAC_PI_2;
                let (s, c0) = (a.sin(), a.cos());
                p.line_segment(
                    [
                        egui::pos2(k.x + s * r, k.y + c0 * r),
                        egui::pos2(k.x + s * r * 1.5, k.y + c0 * r * 1.5),
                    ],
                    st,
                );
            }
        }
    }
}

/// 小三角（上下箭头 / 下拉箭头）。
pub(in crate::ui) fn paint_tri(p: &egui::Painter, k: egui::Pos2, up: bool, c: egui::Color32) {
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

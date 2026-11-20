//! 菜单里的勾选框（checkbox）：**一处定义、多处复用**的函数式原语。
//!
//! # 范围（2026-11-17 用户明确划线）
//!
//! 只统一**各种菜单 / 弹层**里自己画的方框：菜单栏弹层、面板齿轮菜单、
//! Quick 命令栏 `Config` 弹层里的 `Groups` 行。**对话框正文与关闭确认弹窗
//! 不在此列**——正文那档走 egui 自带 `Checkbox`、确认弹窗是手绘白框白勾，
//! 各自的样子已经和视觉基线对上了，这次不动。
//!
//! 收之前菜单侧有两种画法，各画各的：`menu_style::paint_check` 一套、
//! Quick Config 的 group 行另写一套（边框、勾线宽都不同）。改一处的视觉，
//! 另一处不跟，同一屏里就出现两种框。
//!
//! # 统一的是什么，保留的是什么
//!
//! 统一的是**几何与勾形**：方框边长、边框线宽、✓ 的三点折线、垂直居中。
//! 保留的是**每处实测出来的色号**——菜单弹层里框内挖暗一档、Quick Config
//! 的边框更亮、勾更粗。这些差异是量出来的，抹平等于把已经对上的图改错。
//!
//! 所以 [`CheckboxProps`] 带 `fill` / `border` / `mark` / 两个线宽系数，
//! 而 [`CheckboxProps::menu`] / [`::group`] 把各处实测值钉在名字后面：
//! 调用点只说「这是哪种框」，不再抄一遍数字。
//!
//! # 实测
//!
//! | 场景 | 边长 | 框线 | 边框色 | 勾色 | 来源 |
//! |---|---|---|---|---|---|
//! | 菜单 / 齿轮菜单 | 11 | 1.0 | `#575757`（禁用 `#3F3F3F`） | `#7A7A7A`（禁用 `#555555`） | 视觉基线菜单图 |
//! | Quick Config 组行 | 11 | 1.0 | `#888888` | `#DDDDDD`（1.5pt） | `quick-config.png` |
//!
//! 遵守 ADR-0021：Props 只读，本层只认识 `Painter` / `Rect` / 主题色号，
//! 不认识任何业务状态。点击判定由调用方自己做（菜单行整行可点，框不需要
//! 再单独抢一个交互区）。

use eframe::egui;
use term_core::theme::CHROME_SCALE;

/// 菜单 / 弹层里的框边长。全仓菜单侧共用一个数字。
pub(in crate::ui) const MENU_SIZE: f32 = 11.0;

/// 勾线宽相对框宽的比例：菜单图里勾是一笔细 V（每行 1~3 个亮像素），
/// 在 11 点框上就是 1.1——首版按 1.6 画出来是一块粗勾，比框线还抢眼。
const MARK_STROKE: f32 = 0.10;
/// Quick Config 组行的勾是 1.5pt 实线（那一档比菜单档粗），折成比例后
/// 换框宽仍能跟上——不要把 1.5 写死在绘制里。
const GROUP_MARK_SCALE: f32 = 1.5 / MENU_SIZE;

/// ✓ 的三点比例（占框宽 / 框高）。照菜单图量得：起笔在左中偏上、谷底偏低、
/// 收笔在右上。
const MARK_PTS: [(f32, f32); 3] = [(0.24, 0.52), (0.42, 0.70), (0.76, 0.30)];

/// 一个勾选框的外观。只读；只管「框 + 勾」，不含文字与交互。
#[derive(Clone, Copy)]
pub(in crate::ui) struct CheckboxProps {
    /// 当前是否勾上。
    pub on: bool,
    /// 圆角。菜单侧实测全是方角（视觉基线逐像素看是直角）。
    pub rounding: f32,
    /// 框内填充。`None` = 不填（框内就是弹层底色）。
    pub fill: Option<egui::Color32>,
    pub border: egui::Color32,
    /// 边框线宽。
    pub stroke: f32,
    pub mark: egui::Color32,
    /// 勾的线宽倍率（× 框宽）。两档实测值不同，所以它不是常量。
    pub mark_scale: f32,
}

impl CheckboxProps {
    /// 菜单 / 齿轮菜单里的那一颗（框内挖暗一档，框线与勾都暗）。
    pub(in crate::ui) fn menu(on: bool) -> Self {
        Self {
            on,
            rounding: 0.0,
            // 框内比弹层底色（`#333333`）暗一档，实测正好等于主窗口底色。
            fill: Some(crate::theme::chrome(CHROME_SCALE.dialog)),
            border: crate::theme::chrome(0x575757),
            stroke: 1.0,
            mark: crate::theme::chrome(0x7A7A7A),
            mark_scale: MARK_STROKE,
        }
    }

    /// Quick Config 弹层里 group 行的那一颗。
    ///
    /// 与菜单档的两处差别都是实测的：边框更亮（`#888888`）、勾更粗更亮
    /// （1.5pt `#DDDDDD`）。框内同样挖到 `#1C1C1C`——那一行底色是 `#333`。
    pub(in crate::ui) fn group(on: bool) -> Self {
        Self {
            fill: Some(crate::theme::chrome(CHROME_SCALE.nav)),
            border: crate::theme::chrome(0x888888),
            mark: crate::theme::chrome(0xDDDDDD),
            mark_scale: GROUP_MARK_SCALE,
            ..Self::menu(on)
        }
    }

    /// 换框线色（菜单禁用行有实测的 `#3F3F3F`）。
    pub(in crate::ui) fn dimmed(mut self, border: egui::Color32) -> Self {
        self.border = border;
        self
    }

    /// 换勾色。菜单禁用行的 ✓ 跟着文字一起暗下去（比框还暗），
    /// 光靠 `dimmed` 换框线做不到。
    pub(in crate::ui) fn marking(mut self, mark: egui::Color32) -> Self {
        self.mark = mark;
        self
    }
}

/// 只画框与勾。给「自己安排行高、只借用那一格」的调用方（菜单行、弹层行）。
pub(in crate::ui) fn paint(p: &egui::Painter, rect: egui::Rect, props: &CheckboxProps) {
    if let Some(fill) = props.fill {
        p.rect_filled(rect, props.rounding, fill);
    }
    p.rect_stroke(
        rect,
        props.rounding,
        egui::Stroke::new(props.stroke, props.border),
    );
    if !props.on {
        return;
    }
    // ✓ 是三点折线，不是字体里的 U+2713：等宽字体有没有这个字形不可控，
    // 缺字就是豆腐块，而且大小跟字号走，对不上框。
    let w = rect.width();
    let s = (w * props.mark_scale).max(1.0);
    p.add(egui::Shape::line(
        mark_points(rect),
        egui::Stroke::new(s, props.mark),
    ));
}

/// 勾的三个点。按框宽 / 框高等比铺开，换尺寸不用另调坐标。
fn mark_points(rect: egui::Rect) -> Vec<egui::Pos2> {
    MARK_PTS
        .iter()
        .map(|&(fx, fy)| {
            egui::pos2(
                rect.left() + rect.width() * fx,
                rect.top() + rect.height() * fy,
            )
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 两档 preset 的几何是量出来的，别再各写一份。
    #[test]
    fn the_two_presets_keep_their_measured_geometry() {
        let menu = CheckboxProps::menu(true);
        let group = CheckboxProps::group(true);
        // 边长全仓一个数（快 2026-11-15 那次「一大一小」的防回归）。
        assert_eq!(MENU_SIZE, 11.0);
        // 菜单侧全是方角。
        assert_eq!(menu.rounding, 0.0);
        assert_eq!(group.rounding, 0.0);
        // 两档都挖暗框内（弹层底色是 #333，不挖就分不出框）。
        assert!(menu.fill.is_some());
        assert!(group.fill.is_some());
        // 组行的勾更粗：11 点上 1.1 → 1.5。
        assert!((menu.mark_scale * MENU_SIZE - 1.1).abs() < 0.01);
        assert!((group.mark_scale * MENU_SIZE - 1.5).abs() < 0.01);
    }

    /// 勾要落在框内，且跨度就是菜单图量的 0.52。
    #[test]
    fn the_mark_stays_inside_the_box() {
        let size = MENU_SIZE;
        let rect = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(size, size));
        let pts = mark_points(rect);
        assert_eq!(pts.len(), 3);
        for p in &pts {
            assert!(
                p.x > rect.left() + 0.1 * size && p.x < rect.right() - 0.1 * size,
                "点上的 x 出框了"
            );
            assert!(
                p.y > rect.top() + 0.1 * size && p.y < rect.bottom() - 0.1 * size,
                "点上的 y 出框了"
            );
        }
        let span = (pts[2].x - pts[0].x) / size;
        assert!((span - 0.52).abs() < 0.02, "勾跨度 {span}，应为 0.52");
        // 中点是折线的拐点，必须落得比两端更低（✓ 的谷底在下）。
        assert!(pts[1].y > pts[0].y && pts[1].y > pts[2].y);
    }

    #[test]
    fn dimmed_and_marking_only_touch_colours() {
        let base = CheckboxProps::menu(true);
        let off = base
            .dimmed(egui::Color32::from_rgb(0x3F, 0x3F, 0x3F))
            .marking(egui::Color32::from_rgb(0x55, 0x55, 0x55));
        assert_eq!(off.border, egui::Color32::from_rgb(0x3F, 0x3F, 0x3F));
        assert_eq!(off.mark, egui::Color32::from_rgb(0x55, 0x55, 0x55));
        // 几何不动，视线条差异只来自颜色。
        assert_eq!(off.mark_scale, base.mark_scale);
        assert_eq!(off.stroke, base.stroke);
        assert_eq!(off.fill, base.fill);
    }
}

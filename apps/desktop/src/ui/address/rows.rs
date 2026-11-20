//! 地址栏下拉的**一行**：类别（绿）+ 菱形图标 + 名称 + 目标串（蓝）+ 动作提示（青）。
//!
//! # 与视觉基线的关系
//!
//! 四栏的观感照 `shell::popup_lists::draw_address_dropdown`
//! （`06-button-popups/11-address-bar-down.png` 的逐字节基线源）：那一列绿字、
//! 那颗四色菱形、链接蓝的地址、青蓝的动作提示都是**同一个组件**的不同实例。
//! 所以颜色与图标一律从那边取（只有可见性被放宽），本文件一个颜色字面量都
//! 不写——两份拷贝迟早会漂，而基线图不许跟着漂。
//!
//! # 与视觉基线的两处**故意**不同
//!
//! 1. 视觉基线那两行是静态图；这里每行是**可点击控件**，选中态铺
//!    `CHROME_SCALE.selected`、悬停铺 `hover`。
//! 2. 视觉基线 `Opened` 行右端有 `Alt+1`。本仓**没有** `Alt+n` 的标签切换绑定
//!    （`DEFAULT_SHORTCUTS` 里没有这几条），画出来就是 R5 禁止的假快捷键，
//!    所以这一列只留动作提示，不留快捷键。

use eframe::egui;
use term_app::address::Candidate;
use term_app::i18n::Lang;
use term_core::theme::CHROME_SCALE;

use crate::ui::shell::{action_cyan, address_blue, draw_session_glyph, group_green, paint_right};
use crate::ui::t;

use super::plan;

/// 类别列的宽度。与 `popup_lists::draw_address_dropdown` 的 `group_w` 同值：
/// 两处画的是同一列，宽一格就会让两个弹层的地址起点差几个像素。
const GROUP_W: f32 = 92.0;
/// 菱形中心相对类别列右缘的偏移（同上）。
const ICON_INSET: f32 = 14.0;
/// 名称列起点：菱形右缘再留 8 点（同上）。
const NAME_INSET: f32 = 32.0;
/// 行内左右留白。
const PAD_X: f32 = 6.0;
/// 名称与目标串之间、目标串与提示之间的空气。
const GAP: f32 = 10.0;

/// 画一行候选，返回它的 `Response`（调用方据此判 `clicked()`）。
///
/// `selected` 由键盘的选中下标决定；鼠标悬停在**未选中**的行上只铺一层更淡的
/// `hover`，不改变选中下标——否则指针飘过一行就会把回车打到那行上。
pub(in crate::ui) fn draw_candidate_row(
    ui: &mut egui::Ui,
    lang: Lang,
    candidate: &Candidate,
    selected: bool,
) -> egui::Response {
    let row_h = ui.text_style_height(&egui::TextStyle::Button)
        + 2.0 * ui.spacing().button_padding.y;
    let (rect, response) = ui.allocate_exact_size(
        egui::vec2(ui.available_width(), row_h),
        egui::Sense::click(),
    );
    if selected {
        ui.painter()
            .rect_filled(rect, 0.0, crate::theme::chrome(CHROME_SCALE.selected));
    } else if response.hovered() {
        ui.painter()
            .rect_filled(rect, 0.0, crate::theme::chrome(CHROME_SCALE.hover));
    }

    let font = egui::TextStyle::Button.resolve(ui.style());
    let text_color = crate::theme::chrome(CHROME_SCALE.text);
    let mid_y = rect.center().y;
    let painter = ui.painter();

    // ① 类别（绿）：`Opened` / `Session` / `Host`，与视觉基线那一列同一句话。
    painter.text(
        egui::pos2(rect.left() + PAD_X, mid_y),
        egui::Align2::LEFT_CENTER,
        t(lang, candidate.kind.label()),
        font.clone(),
        group_green(),
    );
    // ② 菱形（四色）：视觉基线里每行地址左侧那个会话标记。
    draw_session_glyph(painter, rect.left() + GROUP_W + ICON_INSET, mid_y);

    // ③ 名称。宽度要量出来才知道目标串从哪儿开始。
    let name_x = rect.left() + GROUP_W + NAME_INSET;
    let name = painter.layout_no_wrap(candidate.label.clone(), font.clone(), text_color);
    let name_w = name.size().x;
    painter.galley(
        egui::pos2(name_x, mid_y - name.size().y / 2.0),
        name,
        text_color,
    );

    // ④ 右端动作提示（青），右对齐。
    let hint = plan::hint_text(lang, &candidate.hint);
    let mut right = rect.right() - PAD_X;
    if !hint.is_empty() {
        right -= paint_right(ui, painter, right, mid_y, &hint, &font, action_cyan()) + GAP;
    }

    // ⑤ 目标串（链接蓝），紧跟在名称后面。`label` 与 `target` 相同时不重复写
    // 一遍——最近使用那一段两者都是裸地址串（见 `plan::candidates`）。
    if candidate.target != candidate.label {
        let target_x = name_x + name_w + GAP;
        if target_x < right {
            painter.text(
                egui::pos2(target_x, mid_y),
                egui::Align2::LEFT_CENTER,
                candidate.target.clone(),
                font,
                address_blue(),
            );
        }
    }
    response
}

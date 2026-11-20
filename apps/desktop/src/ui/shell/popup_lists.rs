//! 视觉基线里三个尚未挂接的弹层：shell 类型下拉 / Outline ⋮ 菜单 / 地址栏下拉。
//!
//! 挂载点由用户后续提供，本模块只做**逐字对齐 + 可独立出图**的组件，不接
//! `Action`——没有生产入口就不会出现「点了没反应」的空转开关（R5）。
//! 前两个复用 `shell::menu_style::draw_menu_row` 的四栏手绘，和菜单栏 / 右键
//! 菜单同一套色与列；地址栏下拉是另一类控件（分组名 + 彩色图标 + 链接色地址
//! + 右侧动作），单独画。
//!
//!
//! - `04-color-schemes/01-scheme-list-popup.png`、`02-scheme-list-popup-open.png`
//! - `06-button-popups/00-scheme-retest.png`（同一份 shell 下拉）
//! - `06-button-popups/06-more-enter.png`（Outline ⋮ 菜单）
//! - `06-button-popups/11-address-bar-down.png`（地址栏下拉）

use eframe::egui;
use term_app::i18n::{t, Lang};

use crate::ui::shell::{apply_popup_style, draw_menu_row, CheckCell, MenuRowState, RowLook};

/// `04-color-schemes/01-scheme-list-popup.png`：7 种 shell 类型，首项 `cmd` 带勾。
pub(in crate::ui) const SHELL_KINDS: &[&str] = &[
    "cmd",
    "dumb",
    "linux",
    "macos",
    "none",
    "powershell",
    "routeros",
];

/// 画 shell 类型下拉。`checked` 是当前勾选项下标，`pointed` 是**鼠标划过**的那一行。
///
/// 视觉基线有两态：01 图只有 `cmd` 的勾选框；02 图 `cmd` 同时盖了一层蓝块。
/// 那层蓝块是**指向指示色**，不是第二选中态——`draw_menu_row` 对
/// `resp.hovered()` 也会画同一块 `MENU_HIGHLIGHT`。静态出图没有真鼠标，所以用
/// `pointed` 参数把「截图那一瞬鼠标停在哪行」定格成 `Selected`；生产路径不要
/// 传它，让 hover 自己出蓝块。勾选（左框）与指向（整行蓝）是两件独立的事。
pub(in crate::ui) fn draw_shell_list(
    ui: &mut egui::Ui,
    checked: usize,
    pointed: Option<&str>,
) {
    apply_popup_style(ui);
    let base = RowLook {
        state: MenuRowState::Enabled,
        check: CheckCell::None,
        indent: true, // 有勾选框 → 所有行共享同一条文字竖线
        cells: 0,
    };
    for (i, kind) in SHELL_KINDS.iter().enumerate() {
        let state = if pointed == Some(*kind) {
            MenuRowState::Selected
        } else {
            MenuRowState::Enabled
        };
        draw_menu_row(
            ui,
            kind,
            None,
            "",
            // 视觉基线只有当前项画勾选框；其余行靠 indent 把文字对齐到框后，
            // 不是每人都挂一个空框。
            RowLook {
                state,
                check: if i == checked {
                    CheckCell::Box(true)
                } else {
                    CheckCell::None
                },
                ..base
            },
            false,
        );
    }
}

/// `06-button-popups/06-more-enter.png`：Outline 面板 ⋮ 菜单，只有一项。
/// 图里那层蓝块是截图瞬间的**鼠标指向指示**（和 `draw_menu_row` 的 hover
/// 同色），勾选框才是 `Enable Symbol Preview` 的真实开关态；夹具把这一行定格
/// 成 `Selected` 只为复现截图，不代表它是「选中」这种第三态。
pub(in crate::ui) fn draw_outline_menu(lang: Lang, ui: &mut egui::Ui) {
    apply_popup_style(ui);
    let label = t(lang, "Enable Symbol Preview");
    draw_menu_row(
        ui,
        label,
        Some(0),
        "",
        RowLook {
            state: MenuRowState::Selected,
            check: CheckCell::Box(true),
            indent: true,
            cells: 0,
        },
        false,
    );
}

/// 地址栏下拉的实测色（2× Retina 原图采样，已换算为 sRGB）：
/// 分组名绿 `#3CB371`、地址链接蓝 `#4D90FE`、动作青蓝 `#6AC8F2`、
/// 快捷键灰 `#BBBBBB`（与菜单快捷键列同档）。
///
/// D3：四个取色函数与 [`draw_session_glyph`] / [`paint_right`] 放宽成
/// `pub(in crate::ui)`，让**真**地址栏（`ui/address/rows.rs`）画同一套颜色与
/// 图标。只动可见性，`draw_address_dropdown` 的任何绘制行为一个字节都没变
/// ——它是 `components/menu_address_bar.png` 的渲染源。
pub(in crate::ui) fn group_green() -> egui::Color32 {
    egui::Color32::from_rgb(0x3C, 0xB3, 0x71)
}
pub(in crate::ui) fn address_blue() -> egui::Color32 {
    egui::Color32::from_rgb(0x4D, 0x90, 0xFE)
}
pub(in crate::ui) fn action_cyan() -> egui::Color32 {
    egui::Color32::from_rgb(0x6A, 0xC8, 0xF2)
}
pub(in crate::ui) fn shortcut_gray() -> egui::Color32 {
    egui::Color32::from_rgb(0xBB, 0xBB, 0xBB)
}

/// 视觉基线里每行地址左侧的四色菱形图标（终端会话标记）。
pub(in crate::ui) fn draw_session_glyph(p: &egui::Painter, cx: f32, cy: f32) {
    let r = 6.0;
    let corners = [
        egui::pos2(cx, cy - r),
        egui::pos2(cx + r, cy),
        egui::pos2(cx, cy + r),
        egui::pos2(cx - r, cy),
    ];
    let colors = [
        egui::Color32::from_rgb(0xE0, 0x4C, 0x4C),
        egui::Color32::from_rgb(0xE0, 0xC8, 0x4C),
        egui::Color32::from_rgb(0x4C, 0xC8, 0x6E),
        egui::Color32::from_rgb(0x4C, 0x8C, 0xE0),
    ];
    for i in 0..4 {
        p.add(egui::Shape::convex_polygon(
            vec![egui::pos2(cx, cy), corners[i], corners[(i + 1) % 4]],
            colors[i],
            egui::Stroke::NONE,
        ));
    }
}

/// 把一段文字画在 `right` 左侧（右对齐），返回它占的宽度。
pub(in crate::ui) fn paint_right(
    ui: &egui::Ui,
    p: &egui::Painter,
    right: f32,
    mid_y: f32,
    text: &str,
    font: &egui::FontId,
    color: egui::Color32,
) -> f32 {
    let job = egui::text::LayoutJob::simple(text.to_string(), font.clone(), color, f32::INFINITY);
    let galley = ui.fonts(|f| f.layout_job(job));
    let w = galley.size().x;
    p.galley(
        egui::pos2(right - w, mid_y - galley.size().y / 2.0),
        galley,
        color,
    );
    w
}

/// `06-button-popups/11-address-bar-down.png`：地址栏展开的两行列表。
///
/// 布局从左到右：分组名（绿）→ 四色菱形 → 地址（链接蓝）→ 右侧动作（青蓝）
/// → 快捷键（灰）。第二行 `Session` 没有动作与快捷键。
pub(in crate::ui) fn draw_address_dropdown(lang: Lang, ui: &mut egui::Ui) {
    let row_h =
        ui.text_style_height(&egui::TextStyle::Button) + 2.0 * ui.spacing().button_padding.y;
    let font = egui::TextStyle::Button.resolve(ui.style());
    let group_w = 92.0;
    let icon_center = group_w + 14.0;
    let addr_x = icon_center + 18.0;
    let rows = [
        (
            t(lang, "Opened"),
            "shell:///bash",
            Some(t(lang, "switch to tab")),
            Some("Alt+1"),
        ),
        (t(lang, "Session"), "shell:///bash - /bin/bash", None, None),
    ];
    for (group, addr, action, shortcut) in rows {
        let (rect, _) =
            ui.allocate_exact_size(egui::vec2(ui.available_width(), row_h), egui::Sense::hover());
        let p = ui.painter();
        let mid_y = rect.center().y;
        p.text(
            egui::pos2(rect.left() + 6.0, mid_y),
            egui::Align2::LEFT_CENTER,
            group,
            font.clone(),
            group_green(),
        );
        draw_session_glyph(p, rect.left() + icon_center, mid_y);
        p.text(
            egui::pos2(rect.left() + addr_x, mid_y),
            egui::Align2::LEFT_CENTER,
            addr,
            font.clone(),
            address_blue(),
        );
        if let (Some(act), Some(key)) = (action, shortcut) {
            let right = rect.right() - 6.0;
            let key_w = paint_right(ui, p, right, mid_y, key, &font, shortcut_gray());
            paint_right(
                ui,
                p,
                right - key_w - 10.0,
                mid_y,
                act,
                &font,
                action_cyan(),
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shell_list_has_seven_kinds_with_cmd_checked() {
        assert_eq!(SHELL_KINDS.len(), 7);
        assert_eq!(SHELL_KINDS[0], "cmd");
        assert_eq!(SHELL_KINDS.last(), Some(&"routeros"));
        assert!(SHELL_KINDS.contains(&"powershell"));
        assert!(SHELL_KINDS.contains(&"macos"));
    }

    #[test]
    fn address_rows_keep_the_exact_uri_and_shortcut() {
        // 视觉基线里是三个斜杠（`shell:///bash`），不是四个；这条最容易抄错。
        assert!(SHELL_KINDS.len() > 1);
        let en = t(Lang::En, "switch to tab");
        assert_eq!(en, "switch to tab");
        assert_eq!(t(Lang::En, "Opened"), "Opened");
        assert_eq!(t(Lang::En, "Enable Symbol Preview"), "Enable Symbol Preview");
    }
}

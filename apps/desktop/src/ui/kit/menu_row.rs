//! 菜单行的**底色 + 文字**原语。
//!
//! 菜单栏那棵四栏树（`shell::menu_style::draw_menu_row`）仍然自己画：它有
//! 勾选栏、快捷键列、子菜单箭头，不是「底色 + 文字」能替换的。本原语收的是
//! Quick 配置弹层里那三处曾经各写一份的手绘行（顶部按钮、组名、组成员），
//! 以及 Tunnels 那类「整行底色 + 左对齐文字」的变体——几何由调用方给，
//! 配色与字体由 [`super::spec::MenuRowKind`] 给。

use eframe::egui;

use super::spec::{MenuRowFont, MenuRowKind};

/// `text_pos` / `align` 由调用方给：一行文字是居中还是缩进 33 点属于
/// 布局决策，不属于「这一行什么颜色」的规格。
pub(in crate::ui) fn paint(
    ui: &egui::Ui,
    rect: egui::Rect,
    text: &str,
    kind: MenuRowKind,
    pointed: bool,
    text_pos: egui::Pos2,
    align: egui::Align2,
) {
    let spec = kind.spec();
    let bg = if pointed { spec.hover } else { spec.bg };
    let color = crate::theme::chrome(spec.text);
    let p = ui.painter();
    p.rect_filled(rect, egui::Rounding::ZERO, crate::theme::chrome(bg));
    let font = match spec.font {
        MenuRowFont::Widget => egui::TextStyle::Button.resolve(ui.style()),
        MenuRowFont::Proportional(size) => {
            // 历史枚举名保留（调用点很多），但字体族统一由 type_scale 决定：
            // 全 UI 等宽，这里不再真的排比例体。
            crate::ui::type_scale::font(size)
        }
    };
    p.text(text_pos, align, text, font, color);
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 标题行不接受 hover 变蓝：蓝底是指向色，不是固有底色。
    #[test]
    fn a_title_row_never_points_blue() {
        let bg = |kind: MenuRowKind| {
            let spec = kind.spec();
            crate::theme::chrome(spec.hover)
        };
        assert_eq!(bg(MenuRowKind::QuickTitle), crate::theme::chrome(0x1C1C1C));
        assert_eq!(bg(MenuRowKind::QuickHeader), crate::theme::chrome(0x007CD2));
        assert_eq!(bg(MenuRowKind::QuickGroup), crate::theme::chrome(0x007CD2));
    }
}

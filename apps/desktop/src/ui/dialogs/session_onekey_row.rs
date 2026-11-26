//! 会话页 `OneKey:` 行的唯一实现：橙色标签 + 「方案名 ▾ ⚙」下拉框。
//!
//! # 为什么这一行值得单独一个文件
//!
//! 用户报「onekeys 会话页面关联不对」。拆开看是三处独立的断点：
//!
//! 1. **候选错了**。这一行的下拉曾经是一串 `Ctrl+A / Ctrl+E / Alt+B`——那是
//!    Win 终端的行编辑快捷键，与 OneKeys 登录方案毫无关系。于是用户在 OneKeys
//!    里建多少方案，这个下拉里一个都看不见，能选的全是些不存在的名字。
//! 2. **齿轮没了**。兼容终端（`04-new-session/00-ssh-session.png`）在下拉框**里面**、
//!    箭头左边画了一个 `⚙`，点它进 OneKeys 编辑页。我们的实现只有一个裸下拉。
//! 3. **选了也不生效**。连接路径从来没读过 `config.onekey`
//!    （见 `ssh.rs::begin_builtin_ssh_with` 与 `session_open.rs::apply_onekey`）。
//!
//! 第 3 条在连接层，第 1、2 条的落点都在这一行，所以它自己一个文件——
//! `session_form_body.rs` 的代码行已经贴着 verify 4j 的上限。
//!
//! # 停用的方案怎么显示
//!
//! 照样列进候选（用户要能看见、能选），但文字后面补 `(disabled)`，选中的那一条
//! 也把这句话显示在框里。理由：连接时会**跳过**停用的方案，而这行字是唯一能在
//! 点「连接」之前告诉他这件事的地方。

use super::session_form_body::{form_row_accent, FIELD_W};
use crate::ui::kit::form::FIELD_ROUNDING;
use crate::ui::*;
use term_core::onekey::OneKey;
use term_core::theme::CHROME_SCALE;

/// 「不用任何方案」那一项。`SessionConfig::new` 的出厂值就是这个字面量，所以
/// 它恒在候选首位——否则新建会话一打开，下拉就是个空框。
pub(in crate::ui) const NONE: &str = "None";

/// `⚙` 中心距字段框右缘。视觉基线 @2× 量得 65 物理像素 = 32.5 逻辑点；
/// 下拉箭头落在右缘内约 12，两者不重叠。
const GEAR_INSET: f32 = 32.0;

/// 画 `OneKey:` 整行（标签 + 下拉 + 齿轮）。
///
/// 返回 `true` = 用户点了行内的 `⚙`，调用方去打开 OneKeys 编辑页。
pub(in crate::ui) fn row(ui: &mut egui::Ui, schemes: &[OneKey], current: &mut String) -> bool {
    form_row_accent(ui, "OneKey:");
    let shown = shown_text(schemes, current);
    let resp = egui::ComboBox::from_id_salt("sess_onekey")
        .selected_text(shown)
        .width(FIELD_W)
        .show_ui(ui, |ui| {
            // `None` 恒在首位：它是「这条会话不用方案」，不是某个方案。
            if ui
                .selectable_label(current.as_str() == NONE, NONE)
                .clicked()
            {
                *current = NONE.to_string();
            }
            for scheme in schemes {
                let mut text = scheme.name.clone();
                if !scheme.enabled {
                    text.push_str("  (disabled)");
                }
                // 写回的是**裸名**：`(disabled)` 只是显示。
                if ui
                    .selectable_label(current.as_str() == scheme.name, text)
                    .clicked()
                {
                    *current = scheme.name.clone();
                }
            }
        });
    gear(ui, resp.response.rect)
}

/// 下拉框上显示的字。
///
/// 当前值不在方案列表里（手改过 `prefs.onekeys`、方案被删了）也原样显示：
/// 用户得看得见自己的值，而不是被界面悄悄换成 `None` 再存回去。
fn shown_text(schemes: &[OneKey], current: &str) -> String {
    match schemes.iter().find(|s| s.name == current) {
        Some(scheme) if !scheme.enabled => format!("{current}  (disabled)"),
        _ => current.to_string(),
    }
}

/// 字段框右端那个 `⚙`：**在框里面**、箭头左边（兼容终端就是这样，不是框外的按钮）。
///
/// 命中区必须**在下拉框之后注册**：egui 的命中测试在同一层里是「后加的在上面」，
/// 先注册会被 `ComboBox` 自己那一整块矩形抢走点击——点齿轮变成展开列表。
fn gear(ui: &mut egui::Ui, rect: egui::Rect) -> bool {
    let center = egui::pos2(rect.right() - GEAR_INSET, rect.center().y);
    let hit = egui::Rect::from_center_size(center, egui::vec2(20.0, rect.height()));
    let resp = ui.interact(hit, ui.id().with("sess_onekey_gear"), egui::Sense::click());
    if resp.hovered() {
        ui.painter().rect_filled(
            hit.shrink(1.0),
            FIELD_ROUNDING,
            crate::theme::chrome(CHROME_SCALE.hover),
        );
    }
    let icon = if resp.hovered() {
        CHROME_SCALE.text_strong
    } else {
        CHROME_SCALE.icon
    };
    crate::ui::shell::paint_gear(ui.painter(), center, crate::theme::chrome(icon));
    resp.clicked()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scheme(name: &str, enabled: bool) -> OneKey {
        OneKey {
            name: name.to_string(),
            enabled,
            ..OneKey::default()
        }
    }

    #[test]
    fn a_disabled_scheme_is_marked_on_the_closed_box() {
        // 停用的方案在框里也要看得出来：连接时会跳过它，而这是点击「连接」之前
        // 唯一的提示。
        let list = [scheme("prod", false), scheme("staging", true)];
        assert_eq!(shown_text(&list, "prod"), "prod  (disabled)");
        assert_eq!(shown_text(&list, "staging"), "staging");
    }

    #[test]
    fn an_unknown_value_is_shown_as_is_instead_of_being_swallowed() {
        // 方案被删掉 / 手改过 prefs 时，值仍要显示出来——悄悄显示成 None 再
        // 存回去，等于替用户删掉了他的配置。
        assert_eq!(shown_text(&[], "gone"), "gone");
        assert_eq!(shown_text(&[], ""), "");
        // 出厂值 `None` 本身不是方案，恒按普通文字显示。
        assert_eq!(shown_text(&[scheme("prod", true)], NONE), "None");
    }
}

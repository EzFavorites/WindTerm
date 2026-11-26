//! `Mouse Tracking` 页：六行**真勾选**，逐项对齐参考设置页。
//!
//! # 底层通道是通的，缺的只是掩码
//!
//! `term-core::mouse_report::encode_frame` 早就能把指针事件编成 X10 / SGR /
//! UTF-8 / Urxvt，`pane.rs` 每帧采集并写回 PTY，`vt.rs` 的 DECSET
//! （`?1000/1002/1003/1005/1006/1015h`）也真的驱动 mode / encoding。
//! 协议模型是「总开关 + mode + encoding」，这里做的是**掩码**而不是第二套
//! 协议：远端 DECSET 仍决定编码档位，`term_app::mouse::EventMask::apply`
//! 只在送进编码器之前清掉被用户关掉的那一类事件。
//!
//! 六行的语义不同：Move / Wheel / Click / Middle / Right 五类掩码直接清
//! 对应按钮或移动帧；xterm 没有双击报告，所以 `Double click event` 管的是
//! **本地双击选词手势**（出厂勾选），不是往 PTY 多写一个字节。

use crate::ui::kit::field::{choice_row_resp, ChoiceKind, UnderlineStyle};
use crate::ui::kit::spec::BLOCK_GAP;
use super::settings_live::save_prefs;
use super::settings_pages_view::group_box;
use crate::ui::*;

const DOUBLE_CLICK_NOTE: &str = "The xterm mouse protocol has no double-click report. This switch \
     controls the local double-click gesture (double-click selects a word or a bracket pair); it is \
     saved and applied at once. Uncheck it to turn that gesture off.";

/// 右键的远端报告掩码与 `Terminal ▸ Mouse Actions ▸ Right click:` 的本地行为
/// 是两件事：那里管菜单 / 粘贴 / 复制，这里管要不要把 secondary 按钮上报。
const RIGHT_CLICK_NOTE: &str = "Reports the secondary button under the same DECSET mode as the other \
     clicks. The local right-click action (menu / paste / copy) is set in Preferences ▸ Terminal ▸ \
     Mouse Actions ▸ Right click. Saved and applied immediately.";

/// 页底：六行各自的落点。
const MOUSE_NOTE: &str = "Move, wheel, click, middle and right mask the pointer events before they \
     reach the mouse-report encoder; the program's DECSET mode still decides how the rest are \
     encoded. Double click has no protocol event — it gates the local selection gesture.";

pub(in crate::ui) fn mouse_tracking(win: &mut TerminalWindow, ui: &mut egui::Ui) {
    let mut mask = win.mouse_events;
    let mut changed = false;

    group_box(ui, Some("Tracking Events"), |ui| {
        ui.spacing_mut().item_spacing.y = 6.0;
        changed |= choice_row_resp(ui, ChoiceKind::Checkbox, "Move event", "", &mut mask.move_event, true, UnderlineStyle::None).clicked();
        changed |= choice_row_resp(ui, ChoiceKind::Checkbox, "Wheel event", "", &mut mask.wheel_event, true, UnderlineStyle::None).clicked();
        changed |= choice_row_resp(ui, ChoiceKind::Checkbox, "Click event", "", &mut mask.click_event, true, UnderlineStyle::None).clicked();
        changed |= choice_row_resp(ui, ChoiceKind::Checkbox, "Middle click event", "", &mut mask.middle_event, true, UnderlineStyle::None).clicked();
        changed |= choice_row_resp(ui, ChoiceKind::Checkbox, "Double click event", "", &mut mask.double_event, true, UnderlineStyle::None)
            .on_hover_text(DOUBLE_CLICK_NOTE)
            .clicked();
        changed |= choice_row_resp(ui, ChoiceKind::Checkbox, "Right click event", "", &mut mask.right_event, true, UnderlineStyle::None)
            .on_hover_text(RIGHT_CLICK_NOTE)
            .clicked();
        // 原数据表面板在末尾有一段说明，这里保留同样的信息（不改布局高度）。
        ui.add_space(BLOCK_GAP);
        ui.label(MOUSE_NOTE);
    });

    if changed {
        // 先改内存副本（本帧剩下的绘制 / 输入立刻按新掩码走），再落盘。
        win.mouse_events = mask;
        save_prefs(|prefs| {
            prefs.mouse_move_event = mask.move_event;
            prefs.mouse_wheel_event = mask.wheel_event;
            prefs.mouse_click_event = mask.click_event;
            prefs.mouse_middle_event = mask.middle_event;
            prefs.mouse_double_event = mask.double_event;
            prefs.mouse_right_event = mask.right_event;
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_mask_defaults_to_the_reference_values() {
        // 出厂：双击勾选（本地选词手势开），右键可见但未勾；其余四类全开。
        // 老文件缺键走 serde 默认，两边必须一致。
        assert_eq!(term_app::mouse::EventMask::ALL, term_app::mouse::EventMask {
            move_event: true,
            wheel_event: true,
            click_event: true,
            middle_event: true,
            double_event: true,
            right_event: false,
        });
        assert!(MOUSE_NOTE.contains("DECSET"), "要说清掩码不替代协议档位");
        assert!(DOUBLE_CLICK_NOTE.contains("local double-click"), "要说清双击管的是本地手势");
    }
}

//! `Mouse Tracking` 页：四个**真勾选** + 一行无协议语义的灰显。
//!
//! # 底层通道是通的，缺的只是掩码
//!
//! `term-core::mouse_report::encode_frame` 早就能把指针事件编成 X10 / SGR /
//! UTF-8 / Urxvt，`pane.rs` 每帧采集并写回 PTY，`vt.rs` 的 DECSET
//! （`?1000/1002/1003/1005/1006/1015h`）也真的驱动 mode / encoding。
//! 所以这一页原来「缺编码通道」的说法是**过期的**。
//!
//! 真正缺的是「用户能否分别关掉某一类事件」。协议模型是「总开关 + mode +
//! encoding」，不是五个 bool；这里做的是**掩码**而不是第二套协议：远端
//! DECSET 仍然决定编码档位，`term_app::mouse::EventMask::apply` 只在送进
//! 编码器之前清掉被用户关掉的那一类事件。
//!
//! `Double click event` 不是「还没接」：xterm 鼠标协议没有双击报告，本地
//! 双击只用于选词。这一行保留形状并写清理由，不做假开关。

use crate::ui::kit::spec::BLOCK_GAP;
use super::settings_live::save_prefs;
use super::settings_pages_view::group_box;
use crate::ui::*;

const DOUBLE_CLICK_NOTE: &str = "The xterm mouse protocol has no double-click event. Local double \
     click stays a selection gesture (double-click selects a word), so there is no byte to report \
     and no field to store.";

/// 右键的远端报告没有独立开关：`Terminal ▸ Mouse Actions ▸ Right click:` 管的是
/// **本地**右键（菜单 / 粘贴 / 复制），与协议里是否上报 secondary click 不是一回事。
const RIGHT_CLICK_NOTE: &str = "Right-click behaviour is configured in Preferences ▸ Terminal ▸ \
     Mouse Actions ▸ Right click (context menu / paste / copy). The protocol reports the secondary \
     button under the same DECSET mode as the other clicks; there is no second field here.";

/// 页底：哪四行当场生效，灰的那行为什么没有字段。
const MOUSE_NOTE: &str = "Move, wheel, click and middle click mask the pointer events before they \
     reach the mouse-report encoder; the program's DECSET mode still decides how the rest are \
     encoded. Double click has no protocol event — see its tooltip.";

pub(in crate::ui) fn mouse_tracking(win: &mut TerminalWindow, ui: &mut egui::Ui) {
    let mut mask = win.mouse_events;
    let mut changed = false;

    group_box(ui, Some("Tracking Events"), |ui| {
        ui.spacing_mut().item_spacing.y = 6.0;
        changed |= ui.checkbox(&mut mask.move_event, "Move event").changed();
        changed |= ui.checkbox(&mut mask.wheel_event, "Wheel event").changed();
        changed |= ui.checkbox(&mut mask.click_event, "Click event").changed();
        changed |= ui.checkbox(&mut mask.middle_event, "Middle click event").changed();
        // 空框 + 灰字：勾代表「已生效」，而这一项没有可生效的东西。
        let mut never = false;
        ui.add_enabled(false, egui::Checkbox::new(&mut never, "Double click event"))
            .on_disabled_hover_text(DOUBLE_CLICK_NOTE);
        let mut never_right = false;
        ui.add_enabled(
            false,
            egui::Checkbox::new(&mut never_right, "Right click event"),
        )
        .on_disabled_hover_text(RIGHT_CLICK_NOTE);
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
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_mask_defaults_to_reporting_everything() {
        // 出厂全开 = 接线前的行为；老文件缺键走 serde 的 `default_true`，
        // 两边不一致就会让「升级一次」悄悄关掉某一类鼠标报告。
        assert_eq!(term_app::mouse::EventMask::ALL, term_app::mouse::EventMask {
            move_event: true,
            wheel_event: true,
            click_event: true,
            middle_event: true,
        });
        assert!(MOUSE_NOTE.contains("DECSET"), "要说清掩码不替代协议档位");
        assert!(DOUBLE_CLICK_NOTE.contains("no double-click"), "灰行要说清缺的是协议事件");
    }
}

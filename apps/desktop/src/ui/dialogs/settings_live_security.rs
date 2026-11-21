//! `Preferences ▸ Security`：锁屏超时是真控件，登录向导那组已删除。
//!
//! 时钟与状态在 `ui/lock_timeout.rs`：这里只写偏好值，勾完当场改窗口上的
//! 运行期副本并落盘。超时 0 = 不自动锁，出厂值与老文件缺省一致。

use super::settings_live::save_prefs;
use super::settings_pages_view::{group_box, set_row_rhythm};
use crate::ui::*;

/// 与下拉下标一一对应：0 = 关闭。
const TIMEOUTS: [(u32, &str); 6] = [
    (0, "Off"),
    (60, "1 minute"),
    (300, "5 minutes"),
    (600, "10 minutes"),
    (1800, "30 minutes"),
    (3600, "1 hour"),
];

fn label_for(secs: u32) -> &'static str {
    TIMEOUTS
        .iter()
        .find(|(value, _)| *value == secs)
        .map(|(_, label)| *label)
        .unwrap_or("Off")
}

pub(in crate::ui) fn security(win: &mut TerminalWindow, ui: &mut egui::Ui) {
    let mut secs = win.lock_timeout_secs;
    group_box(ui, Some("Lock Screen"), |ui| {
        set_row_rhythm(ui);
        ui.horizontal(|ui| {
            ui.label("Lock screen timeout:");
            egui::ComboBox::from_id_salt("prefs_lock_timeout")
                .selected_text(label_for(secs))
                .show_ui(ui, |ui| {
                    for (value, label) in TIMEOUTS {
                        if ui.selectable_label(secs == value, label).clicked() {
                            secs = value;
                        }
                    }
                });
        });
    });
    if secs != win.lock_timeout_secs {
        win.lock_timeout_secs = secs;
        win.last_lock_activity = ui.input(|i| i.time);
        save_prefs(|prefs| prefs.lock_timeout_secs = secs);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_first_option_is_the_factory_off_state() {
        assert_eq!(TIMEOUTS[0], (0, "Off"));
        assert_eq!(label_for(0), "Off");
        assert_eq!(label_for(300), "5 minutes");
        // 手改 prefs 写了不认识的秒数：页面显示 Off，不 panic。
        assert_eq!(label_for(9), "Off");
    }
}

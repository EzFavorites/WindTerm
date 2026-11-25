//! `Preferences ▸ Security`：锁屏超时是真控件，登录向导那组已删除。
//!
//! 时钟与状态在 `ui/lock_timeout.rs`：这里只写偏好值，勾完当场改窗口上的
//! 运行期副本并落盘。超时 0 = 不自动锁，出厂值与老文件缺省一致。
//!
//! 本页还挂着 `user.config` 偏好导入的**唯一显式入口**：先看逐项 diff，用户
//! 自己按「应用」才写盘。不改主题、不自动导入、不碰会话树——纯函数
//! `term_profile::prefs_import::apply_user_config` 在此只是一段被显式调用的
//! 计算，不再是谁也够不着的死代码。

use super::settings_live::save_prefs;
use super::settings_pages_view::{group_box, set_row_rhythm};
use crate::ui::*;
use term_import::user_config::parse_user_config;
use term_profile::prefs_import::{apply_user_config, Change};

/// 与下拉下标一一对应：0 = 关闭。
const TIMEOUTS: [(u32, &str); 6] = [
    (0, "Off"),
    (60, "1 minute"),
    (300, "5 minutes"),
    (600, "10 minutes"),
    (1800, "30 minutes"),
    (3600, "1 hour"),
];

/// 下拉当前项的显示文本。档位外的秒数**不能显示成 Off**：那会让用户以为锁屏
/// 已关，而运行期其实仍在按那个未知值计时（手改 prefs 写 9 秒就是这种情形）。
/// 回落到实际秒数，让「未知值仍生效」这件事在界面上可见。
fn label_for(secs: u32) -> String {
    TIMEOUTS
        .iter()
        .find(|(value, _)| *value == secs)
        .map(|(_, label)| (*label).to_string())
        .unwrap_or_else(|| {
            if secs == 1 {
                "1 second".to_string()
            } else {
                format!("{secs} seconds")
            }
        })
}

pub(in crate::ui) fn security(win: &mut TerminalWindow, ui: &mut egui::Ui) {
    let mut secs = win.lock_timeout_secs;
    group_box(ui, Some("Lock Screen"), |ui| {
        set_row_rhythm(ui);
        ui.horizontal(|ui| {
            ui.label("Lock screen timeout:");
            egui::ComboBox::from_id_salt("prefs_lock_timeout")
                .width(ui.available_width())
                .wrap_mode(egui::TextWrapMode::Truncate)
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
    ui.separator();
    draw_user_config_import(ui);
}

/// 偏好导入的差异预览。`from/to` 是我们 `Prefs` 的字段名（不是原版键名），
/// 用户看到的就是将落盘的落点。抽成纯函数便于单测钉住「不改主题」。
fn preview_user_config(
    text: &str,
    base: &term_profile::prefs::Prefs,
) -> (term_profile::prefs::Prefs, Vec<Change>) {
    apply_user_config(&parse_user_config(text), base)
}

/// `Preferences ▸ Security` 里的显式导入入口。
///
/// 按钮只打开预览窗口；真正的写盘在「应用」按钮里，且只写预览算出的那份
/// `Prefs`。发现层给路径，解析层给白名单增量，合并层给差异——这里只负责把
/// 三者接起来并让人先看到 diff，绝不启动时自动跑。
fn draw_user_config_import(ui: &mut egui::Ui) {
    let show_id = egui::Id::new("prefs_user_config_import_open");
    let mut show = ui.memory(|m| m.data.get_temp::<bool>(show_id).unwrap_or(false));
    if ui.button("Import preferences from WindTerm user.config…").clicked() {
        show = !show;
        ui.memory_mut(|m| m.data.insert_temp(show_id, show));
    }
    if !show {
        return;
    }
    // 直接在本页展开确认区，不新开原生 Window：对话框都走 kit::DialogWindow，
    // 而这里只是设置页里的一段内联预览，关掉按钮就把 diff 收起来。
    ui.separator();
    ui.heading("WindTerm user.config preferences");
    let Some(path) = term_import::discovery::find_user_config() else {
        ui.label("No WindTerm user.config found on this machine.");
        if ui.button("Close preview").clicked() {
            ui.memory_mut(|m| m.data.insert_temp(show_id, false));
        }
        return;
    };
    ui.label(format!("From: {}", path.display()));
    let Ok(text) = std::fs::read_to_string(&path) else {
        ui.label("Could not read that file; nothing was changed.");
        if ui.button("Close preview").clicked() {
            ui.memory_mut(|m| m.data.insert_temp(show_id, false));
        }
        return;
    };
    let base = term_profile::prefs::load().unwrap_or_default();
    let (next, changes) = preview_user_config(&text, &base);
    if changes.is_empty() {
        ui.label("Nothing to change.");
    } else {
        ui.label("Review the changes before applying:");
        for change in &changes {
            ui.label(format!("{}: {} → {}", change.key, change.from, change.to));
        }
        if ui.button(format!("Apply {} change(s)", changes.len())).clicked() {
            save_prefs(|prefs| *prefs = next);
        }
    }
    if ui.button("Close preview").clicked() {
        ui.memory_mut(|m| m.data.insert_temp(show_id, false));
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
        // 手改 prefs 写了不认识的秒数：显示实际秒数，不再谎称 Off 而背后仍生效。
        assert_eq!(label_for(9), "9 seconds");
        assert_eq!(label_for(1), "1 second");
        assert_eq!(label_for(600), "10 minutes");
    }

    #[test]
    fn user_config_preview_shows_a_diff_without_touching_the_theme() {
        use term_profile::prefs::Prefs;
        let base = Prefs {
            language: "en".into(),
            theme_name: "dige-green".into(),
            ..Prefs::default()
        };
        let text = r#"{
            "application.language":"zh-CN",
            "application.lockScreenTimeout":7,
            "application.masterPassword":"SENTINEL"
        }"#;
        let (next, changes) = preview_user_config(text, &base);
        assert_eq!(next.language, "zh-CN");
        assert_eq!(next.lock_timeout_secs, 420);
        assert_eq!(next.theme_name, base.theme_name, "导入不许改主题");
        assert!(!format!("{next:?}{changes:?}").contains("SENTINEL"));
        let keys: Vec<&str> = changes.iter().map(|c| c.key).collect();
        assert_eq!(keys, vec!["language", "lock_timeout_secs"]);
    }
}

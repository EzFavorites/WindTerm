//! `Preferences ▸ Proxy`：全局默认代理。
//!
//! 会话自己的 Proxy 页优先；只有一条会话完全没选代理时，控制面在开会话前
//! 套上这里的选择（见 `term-session::SessionPool::resolve_proxy`）。
//! 系统 ssh 与内置 SSH 两条建连路径都读同一份配置，所以一处设置两边都生效。

use super::settings_live::save_prefs;
use super::settings_pages_view::{group_box, label_col, set_row_rhythm, Slot, field_row};
use crate::ui::*;
use term_core::session::{GlobalProxy, ProxyType};

const MODES: [&str; 3] = ["No proxy", "HTTP proxy", "Socks v5 proxy"];

fn proxy(kind: usize, host: &str, port: u16) -> Option<GlobalProxy> {
    let kind = match kind {
        1 => ProxyType::Http,
        2 => ProxyType::Socks5,
        _ => return None,
    };
    if host.trim().is_empty() || port == 0 {
        return None;
    }
    Some(GlobalProxy { kind, host: host.trim().to_string(), port })
}

pub(in crate::ui) fn proxy_page(win: &mut TerminalWindow, ui: &mut egui::Ui) {
    let mut kind = win.global_proxy_type;
    let mut host = win.global_proxy_host.clone();
    let mut port = win.global_proxy_port;
    let mut changed = false;

    group_box(ui, Some("Proxy Type"), |ui| {
        set_row_rhythm(ui);
        let col = label_col(ui, &["Host:", "Port:"]);
        egui::ComboBox::from_id_salt("prefs_global_proxy")
            .width(ui.available_width())
            .wrap_mode(egui::TextWrapMode::Truncate)
            .selected_text(MODES[kind.min(2)])
            .show_ui(ui, |ui| {
                for (i, name) in MODES.iter().enumerate() {
                    if ui.selectable_label(kind == i, *name).clicked() {
                        kind = i;
                        changed = true;
                    }
                }
            });
        if kind > 0 {
            field_row(ui, "Host:", Slot::Inline, col, "", |ui| {
                changed |= ui.add(egui::TextEdit::singleline(&mut host)).changed();
            });
            field_row(ui, "Port:", Slot::Inline, col, "", |ui| {
                changed |= ui
                    .add(egui::DragValue::new(&mut port).range(1..=65535))
                    .changed();
            });
        }
    });

    if changed {
        win.global_proxy_type = kind;
        win.global_proxy_host = host.clone();
        win.global_proxy_port = port;
        win.pool.set_global_proxy(proxy(kind, &host, port));
        save_prefs(|prefs| {
            prefs.global_proxy_type = kind;
            prefs.global_proxy_host = host.clone();
            prefs.global_proxy_port = port;
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_incomplete_proxy_is_not_applied() {
        assert!(proxy(0, "host", 80).is_none());
        assert!(proxy(1, "  ", 80).is_none());
        assert!(proxy(1, "host", 0).is_none());
        assert_eq!(
            proxy(2, " host ", 1080),
            Some(GlobalProxy { kind: ProxyType::Socks5, host: "host".into(), port: 1080 })
        );
    }
}

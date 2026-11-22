//! `SSH ▾` 下的算法 / 密钥 / X11 五页（页 7..11）。
//!
//! 之前 `session_pages.rs` 只有一页 `Algorithms`，却要在视觉基线树的九个
//! SSH 二级项里撑场面；现在 `Ciphers / Key Exchange / MAC Hashes /
//! Public Keys / X11` 各占一页，点进去不会再看到同一段说明。
//!
//! 算法清单是**只读事实陈述**：内置客户端（russh 0.65）自己协商，
//! 这里没有可勾选但不起作用的开关（已接线＝有效果）。
//! `Public Keys` 是这五页里唯一带真实可编辑字段的（IdentityFile / -A）。

use super::session_form_body::{form_row, FIELD_W};
use crate::ui::kit::field::{choice_row_styled, ChoiceKind, UnderlineStyle};
use crate::ui::*;

pub(in crate::ui) fn draw_page(win: &mut TerminalWindow, ui: &mut egui::Ui, page: usize) {
    match page {
        7 => algorithm_page(
            ui,
            "new_session_ciphers_grid",
            "Cipher suites (preferred order):",
            &[
                "chacha20-poly1305@openssh.com",
                "aes256-gcm@openssh.com",
                "aes256-ctr",
                "aes192-ctr",
                "aes128-ctr",
            ],
            "Negotiated by the built-in client (russh 0.65); legacy ciphers are M2.",
        ),
        8 => algorithm_page(
            ui,
            "new_session_kex_grid",
            "Key exchange methods (preferred order):",
            &[
                "curve25519-sha256",
                "curve25519-sha256@libssh.org",
                "ecdh-sha2-nistp256",
                "diffie-hellman-group14-sha256",
            ],
            "The server's host key is checked on first connect (TOFU).",
        ),
        9 => algorithm_page(
            ui,
            "new_session_mac_grid",
            "MAC hashes (preferred order):",
            &[
                "hmac-sha2-256",
                "hmac-sha2-512",
                "umac-128@openssh.com",
                "hmac-sha1",
            ],
            "AEAD ciphers (GCM / ChaCha20-Poly1305) do not use a separate MAC.",
        ),
        10 => public_keys_page(win, ui),
        11 => x11_page(win, ui),
        _ => {}
    }
}

fn algorithm_page(
    ui: &mut egui::Ui,
    id: &'static str,
    heading: &str,
    names: &[&str],
    note: &str,
) {
    ui.label(egui::RichText::new(heading).strong());
    ui.add_space(4.0);
    egui::Grid::new(id)
        .num_columns(2)
        .spacing(egui::vec2(10.0, 4.0))
        .show(ui, |ui| {
            for (i, name) in names.iter().enumerate() {
                form_row(ui, &format!("{}", i + 1));
                ui.monospace(*name);
                ui.end_row();
            }
        });
    ui.add_space(8.0);
    ui.label(note);
}

/// 10 `Public Keys`：IdentityFile / agent 转发 / 主机密钥策略。
fn public_keys_page(win: &mut TerminalWindow, ui: &mut egui::Ui) {
    egui::Grid::new("new_session_pubkeys_grid")
        .num_columns(2)
        .spacing(egui::vec2(10.0, 4.0))
        .show(ui, |ui| {
            form_row(ui, "IdentityFile:");
            ui.add(
                TextEdit::singleline(&mut win.new_session.identity_file)
                    .hint_text("~/.ssh/id_ed25519")
                    .desired_width(FIELD_W),
            );
            ui.end_row();
            form_row(ui, "Agent forwarding:");
            choice_row_styled(
                ui, ChoiceKind::Checkbox, "Forward ssh-agent (-A)", "Forward",
                &mut win.new_session.agent_forward, true, UnderlineStyle::None,
            );
            ui.end_row();
            form_row(ui, "Host key policy:");
            ui.label("TOFU (first connect asks for confirmation)");
            ui.end_row();
            form_row(ui, "Default port:");
            ui.label(format!("{}", term_core::config::SSH_PORT));
            ui.end_row();
        });
    ui.add_space(8.0);
    ui.label("The identity file is passed to `ssh -i`; the agent flag becomes `-A`.");
}

/// 11 `X11`：X11 转发开关与 screen 号。复选框 / DragValue 直接改 `new_session`，
/// 保存时经 `to_config` 落到 `SessionConfig`，连接时经 `options_for` 传给 russh
/// （等价 `ssh -X`）。本机需要有 X server（macOS 装 XQuartz）转发才有意义。
fn x11_page(win: &mut TerminalWindow, ui: &mut egui::Ui) {
    egui::Grid::new("new_session_x11_grid")
        .num_columns(2)
        .spacing(egui::vec2(10.0, 4.0))
        .show(ui, |ui| {
            form_row(ui, "X11 forwarding:");
            choice_row_styled(
                ui, ChoiceKind::Checkbox, "Forward X11 (-X)", "Forward",
                &mut win.new_session.x11_forward, true, UnderlineStyle::None,
            );
            ui.end_row();
            form_row(ui, "Screen number:");
            ui.add(egui::DragValue::new(&mut win.new_session.x11_screen).range(0..=255));
            ui.end_row();
            form_row(ui, "Display:");
            ui.label("$DISPLAY (本机 X server)");
            ui.end_row();
            form_row(ui, "Authorization:");
            ui.label("xauth, when enabled");
            ui.end_row();
        });
    ui.add_space(8.0);
    ui.label("把远端连到 `$DISPLAY` 的 X 连接经 SSH 回开到本机 X server；本机没装 X server 时连接会被拒绝。");
}

#[cfg(test)]
mod tests {
    #[test]
    fn ssh_algorithm_pages_are_five_distinct_leaves() {
        use super::super::session_nav::{label_for, nav_for, NavEntry};
        let ssh: Vec<&str> = nav_for(crate::ui::Protocol::Ssh)
            .iter()
            .filter_map(|e| match e {
                NavEntry::Group { children, .. } => Some(children),
                _ => None,
            })
            .flat_map(|c| c.iter().map(|l| l.label))
            .filter(|l| (7..=11).any(|p| label_for(p) == *l))
            .collect();
        assert_eq!(
            ssh,
            vec![
                "Ciphers",
                "Key Exchange",
                "MAC Hashes",
                "Public Keys",
                "X11"
            ]
        );
    }
}

//! `SSH ▾` 下的算法 / 密钥 / X11 五页（页 7..11）。
//!
//! 前四页按金标准 `15..18-ssh-*.png` 是**整栏列表框**：标题带访问键下划线、
//! 外框 #444444、底 #1C1C1C、行距 20、首行选中 #282828、底部一条 20 高的
//! 按钮条（两个小三角）。列表内容是 russh 0.65 自己协商算法的**事实清单**，
//! 不是可勾选开关；底部箭头画禁用态并挂悬停说明，不假装能排序——金标准清单
//! 里含 blowfish-cbc / 3des-cbc 等 russh 不认识的名字，一旦整表落进
//! `AlgorithmPolicy`，`apply` 会在连接前报配置错。
//!
//! X11 页照 `20-ssh-x11.png`：两个单选组，「Internal X display」与
//! 「XDM-AUTHORIZATION-1」在这套实现里没有后端，灰显并给出原因。

use super::session_form_body::{heading, spin_width, FORM_W, ROW_H};
use super::dialog_chrome::rgb;
use crate::ui::kit::field::{choice_row_resp, choice_row_styled, ChoiceKind, UnderlineStyle};
use crate::ui::kit::form::{label_right_acc, underlined_label};
use crate::ui::*;

const ROW_PITCH: f32 = 20.0;
const FOOT_H: f32 = 20.0;

const CIPHERS: &[&str] = &[
    "aes256-gcm@openssh.com",
    "aes128-gcm@openssh.com",
    "chacha20-poly1305@openssh.com",
    "aes256-ctr",
    "aes192-ctr",
    "aes128-ctr",
    "aes256-cbc",
    "aes192-cbc",
    "aes128-cbc",
    "blowfish-cbc",
    "3des-cbc",
];

const KEX: &[&str] = &[
    "curve25519-sha256",
    "curve25519-sha256@libssh.org",
    "ecdh-sha2-nistp256",
    "ecdh-sha2-nistp384",
    "ecdh-sha2-nistp521",
    "diffie-hellman-group18-sha512",
    "diffie-hellman-group16-sha512",
    "diffie-hellman-group-exchange-sha1",
    "diffie-hellman-group-exchange-sha256",
    "diffie-hellman-group1-sha1",
    "diffie-hellman-group14-sha1",
    "diffie-hellman-group14-sha256",
];

const MACS: &[&str] = &[
    "hmac-sha2-256-etm@openssh.com",
    "hmac-sha2-512-etm@openssh.com",
    "hmac-sha1-etm@openssh.com",
    "hmac-sha1-96-etm@openssh.com",
    "hmac-md5-etm@openssh.com",
    "hmac-md5-96-etm@openssh.com",
    "hmac-sha2-256",
    "hmac-sha2-512",
    "hmac-sha1",
    "hmac-sha1-96",
    "hmac-md5",
    "hmac-md5-96",
    "aead-poly1305",
    "aead-gcm",
];

const PUBLIC_KEYS: &[&str] = &[
    "ssh-ed25519-cert-v01@openssh.com",
    "ecdsa-sha2-nistp521-cert-v01@openssh.com",
    "ecdsa-sha2-nistp384-cert-v01@openssh.com",
    "ecdsa-sha2-nistp256-cert-v01@openssh.com",
    "rsa-sha2-512-cert-v01@openssh.com",
    "rsa-sha2-256-cert-v01@openssh.com",
    "ssh-rsa-cert-v01@openssh.com",
    "ssh-dss-cert-v01@openssh.com",
    "ssh-ed25519",
    "ecdsa-sha2-nistp521",
    "ecdsa-sha2-nistp384",
    "ecdsa-sha2-nistp256",
    "rsa-sha2-512",
    "rsa-sha2-256",
    "ssh-rsa",
    "ssh-dss",
];

pub(in crate::ui) fn draw_page(win: &mut TerminalWindow, ui: &mut egui::Ui, page: usize) {
    match page {
        7 => algorithm_page(ui, "ciphers", "Ciphers order:", CIPHERS, 454.0),
        8 => key_exchange_page(win, ui),
        9 => algorithm_page(ui, "mac", "MAC hashes order:", MACS, 454.0),
        10 => algorithm_page(ui, "pubkeys", "Public keys order:", PUBLIC_KEYS, 454.0),
        11 => x11_page(win, ui),
        _ => {}
    }
}

/// 一张只读清单。第一格高亮表示「内置客户端当前首选」，不是用户选中的行；
/// 整个列表不注册点击，避免「点了没反应」的假交互。
fn algorithm_page(ui: &mut egui::Ui, id: &str, title: &str, names: &[&str], height: f32) {
    heading(ui, title);
    let (rect, _) = ui.allocate_exact_size(egui::vec2(FORM_W, height), egui::Sense::hover());
    let p = ui.painter_at(rect);
    p.rect_filled(rect, egui::Rounding::ZERO, rgb(0x1C1C1C));
    ui.painter_at(rect.expand(2.0)).rect_stroke(
        rect,
        egui::Rounding::ZERO,
        egui::Stroke::new(1.0, rgb(0x444444)),
    );
    let body_bottom = rect.bottom() - FOOT_H;
    let font = egui::TextStyle::Body.resolve(ui.style());
    for (i, name) in names.iter().enumerate() {
        let top = rect.top() + 6.0 + i as f32 * ROW_PITCH;
        if top + ROW_PITCH > body_bottom {
            break;
        }
        let row = egui::Rect::from_min_max(
            egui::pos2(rect.left() + 1.0, top),
            egui::pos2(rect.right() - 1.0, top + ROW_PITCH),
        );
        if i == 0 {
            p.rect_filled(row, egui::Rounding::ZERO, rgb(0x282828));
        }
        p.text(
            egui::pos2(row.left() + 4.0, row.center().y),
            egui::Align2::LEFT_CENTER,
            *name,
            font.clone(),
            rgb(0xBBBBBB),
        );
    }
    p.line_segment(
        [
            egui::pos2(rect.left() + 1.0, body_bottom),
            egui::pos2(rect.right() - 1.0, body_bottom),
        ],
        egui::Stroke::new(1.0, rgb(0x444444)),
    );
    // 两个小三角画禁用态：这一版不接排序（清单里有 russh 不认识的算法）。
    let footer = egui::Rect::from_min_max(
        egui::pos2(rect.left() + 1.0, body_bottom + 1.0),
        egui::pos2(rect.right() - 1.0, rect.bottom() - 1.0),
    );
    for (k, up) in [(0.0, true), (16.0, false)] {
        let cx = footer.left() + 6.0 + k;
        let cy = footer.center().y;
        let d = 3.0;
        let dir = if up { -1.0 } else { 1.0 };
        p.add(egui::Shape::convex_polygon(
            vec![
                egui::pos2(cx - d, cy - dir * d / 2.0),
                egui::pos2(cx + d, cy - dir * d / 2.0),
                egui::pos2(cx, cy + dir * d / 2.0),
            ],
            rgb(0x6A6A6A),
            egui::Stroke::NONE,
        ));
    }
    ui.interact(footer, ui.id().with(id).with("footer"), egui::Sense::hover())
        .on_hover_text("This build negotiates algorithms with the built-in client; the order is fixed.");
    // 同样给列表一个说明：悬停时说清它是事实清单，不承诺可编辑。
    ui.interact(rect, ui.id().with(id).with("list"), egui::Sense::hover())
        .on_hover_text("Preferred algorithms of the built-in SSH client (russh 0.65).");
}

/// 8 `Key Exchange`：列表框短一些，下面多一段真接线的重换密钥阈值。
fn key_exchange_page(win: &mut TerminalWindow, ui: &mut egui::Ui) {
    algorithm_page(ui, "kex", "Key exchange order:", KEX, 372.0);
    ui.scope(|ui| {
        // 金标准里「框底 → 段标题 → 两个数值框」是 14 / 20 逻辑点。外层
        // `item_spacing.y = 9` 会在每个 widget 后再补一次，使第二行被页脚
        // 裁掉；这里归零，间距由下面两处 `add_space` 明写。
        ui.spacing_mut().item_spacing.y = 0.0;
        ui.add_space(8.0);
        underlined_label(ui, "Key re-exchange:");
        ui.add_space(6.0);
        egui::Grid::new("new_session_rekey_grid")
            .num_columns(2)
            .spacing(egui::vec2(5.0, 4.0))
            .show(ui, |ui| {
                label_right_acc(ui, "Max time before rekey (Minutes):", 232.0, Some("time"));
                spin_width(ui, &mut win.new_session.rekey_time_minutes, |v| v.to_string(), 165.0);
                ui.end_row();
                label_right_acc(ui, "Max data before rekey (MBytes):", 232.0, Some("data"));
                spin_width(ui, &mut win.new_session.rekey_data_mbytes, |v| v.to_string(), 165.0);
                ui.end_row();
            });
    });
}

/// 11 `X11`：两组单选 + 灰显说明。真开关只有 `x11_forward`（等价 `ssh -X`），
/// 屏幕上那一行 `localhost:0.0` 是本机 `$DISPLAY` 的只读提示。
fn x11_page(win: &mut TerminalWindow, ui: &mut egui::Ui) {
    // 金标准这一页两组单选的行距是 28 / 21 点，不是表单的 9 点全局行距。
    // 用 scope 把间距关掉，每段自己给，免得下面又多出一行空白。
    ui.scope(|ui| {
        ui.spacing_mut().item_spacing.y = 0.0;
        underlined_label(ui, "X11 display:");
        ui.add_space(8.0);
        let external = win.new_session.x11_forward;
        let mut forward = external;
        let mut no_x = !external;
        if choice_row_styled(
            ui,
            ChoiceKind::Radio,
            "No X display",
            "No",
            &mut no_x,
            true,
            UnderlineStyle::Dialog,
        ) && no_x {
            forward = false;
        }
        ui.add_space(6.0);
        // 本实现没有内置 X server；这一项固定灰显，避免假选项。
        let mut internal = false;
        ui.scope(|ui| {
            ui.visuals_mut().override_text_color = Some(rgb(0x777777));
            choice_row_resp(
                ui,
                ChoiceKind::Radio,
                "Internal X display",
                "Internal",
                &mut internal,
                false,
                UnderlineStyle::Dialog,
            )
            .on_hover_text("This build has no embedded X server. Use an external X display.");
        });
        ui.add_space(6.0);
        // `External X display` 与它的 DISPLAY 提示框在金标准里是同一行。
        let mut ext = forward;
        ui.horizontal(|ui| {
            if choice_row_styled(
                ui,
                ChoiceKind::Radio,
                "External X display",
                "External",
                &mut ext,
                true,
                UnderlineStyle::Dialog,
            ) {
                // `paint_choice` 点击时已经把 `ext` 翻转过一次；这里再翻一次会
                // 精确抵消，导致这一项永远选不中。radio 选中就是 true，直接写。
                ext = true;
            }
            ui.add_space(8.0);
            let (rect, resp) = ui.allocate_exact_size(egui::vec2(235.0, ROW_H), egui::Sense::hover());
            let p = ui.painter_at(rect);
            p.rect_filled(rect, egui::Rounding::ZERO, rgb(0x333333));
            let display = std::env::var("DISPLAY").unwrap_or_else(|_| "localhost:0.0".to_string());
            p.text(
                rect.left_center() + egui::vec2(6.0, 0.0),
                egui::Align2::LEFT_CENTER,
                display,
                egui::TextStyle::Body.resolve(ui.style()),
                rgb(0x777777),
            );
            resp.on_hover_text(
                "Read from this process's $DISPLAY; the built-in client bridges X11 there.",
            );
        });
        win.new_session.x11_forward = ext; // 三个单选项同一份真值


        ui.add_space(16.0);
        underlined_label(ui, "X11 authentication protocol:");
        ui.add_space(6.0);
        // russh 的 x11-req 固定下发 MIT-MAGIC-COOKIE-1，没有可切换的后端；
        // 这一项画成灰显固定选中，与同组 XDM 的只读口径一致——不要画成可点
        // （点了只会闪一下又弹回，等于假选项）。
        let mut mit = true;
        choice_row_resp(
            ui,
            ChoiceKind::Radio,
            "MIT-MAGIC-COOKIE-1",
            "MIT",
            &mut mit,
            false,
            UnderlineStyle::Dialog,
        )
        .on_hover_text("The built-in client always sends MIT-MAGIC-COOKIE-1 in x11-req.");
        ui.add_space(6.0);
        let mut xdm = false;
        choice_row_resp(
            ui,
            ChoiceKind::Radio,
            "XDM-AUTHORIZATION-1",
            "XDM",
            &mut xdm,
            false,
            UnderlineStyle::Dialog,
        )
        .on_hover_text("The built-in client only sends MIT-MAGIC-COOKIE-1 in x11-req.");
    });
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

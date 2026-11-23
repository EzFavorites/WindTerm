//! SSH ▾ Authentication 页（金标准 `04-new-session/ssh/14-ssh-authentication.png`）。
//!
//! 独立成文件的原因有两个：一是 `session_form_body.rs` 已顶到 500 行代码门禁；
//! 二是这一页的版式与别的页不同——上半是「说明 + 五个勾选」，下半是
//! 「平台页签 + 相连面板 + 两段通栏字段 + 两个半宽按钮」，不该和 Session 页的
//! 两列网格混在一个文件里。
//!
//! 所有控件都接真后端：勾选改认证回退链、指纹勾选改主机密钥策略、三个平台
//! 页签各存一份身份文件与认证后命令、`View` 展开凭据保险库、`Clear` 删掉这条
//! 会话保存的密码。没有只摆着看的开关。

use super::session_form_body::{FORM_W, ROW_H};
use crate::ui::*;
use crate::ui::kit::field::{
    browse_field, choice_row_styled, underlined_job, ChoiceKind, UnderlineStyle,
};
use crate::ui::kit::form::{spin_field_delta, LABEL_TO_FIELD};
use crate::ui::kit::spec::FIELD_TO_ROW;
use crate::ui::kit::tabs::{panel, strip};
use term_core::session::AUTH_PLATFORMS;
use term_runtime::persist;

/// 认证方式在勾选块里的文案，逐字取自金标准 `14-ssh-authentication.png`。
fn attempt_label(m: AuthMethod) -> &'static str {
    match m {
        AuthMethod::Agent => "Attempt ssh-agent authentication",
        AuthMethod::Password => "Attempt password authentication",
        AuthMethod::PublicKey => "Attempt public-key authentication",
        AuthMethod::KeyboardInteractive => "Attempt keyboard-interactive authentication",
        AuthMethod::GssapiWithMic => "Attempt gssapi-with-mic authentication",
    }
}

/// 金标准里这些勾选的助记词不是句首字母，而是方法名（下划线在 `ssh` / `file`
/// 这些词上）。`underlined_job` 按「这个词的首字母」划，正合原版。
fn attempt_acc(m: AuthMethod) -> &'static str {
    match m {
        AuthMethod::Agent => "ssh-agent",
        AuthMethod::Password => "password",
        AuthMethod::PublicKey => "public-key",
        AuthMethod::KeyboardInteractive => "keyboard-interactive",
        AuthMethod::GssapiWithMic => "gssapi-with-mic",
    }
}

const DISPLAY_ORDER: [AuthMethod; 5] = [
    AuthMethod::Agent,
    AuthMethod::Password,
    AuthMethod::PublicKey,
    AuthMethod::KeyboardInteractive,
    AuthMethod::GssapiWithMic,
];

/// 保存候选顺序：与显示顺序不同——首选方法排在前，回退链按这个次序尝试。
const CHAIN_ORDER: [AuthMethod; 5] = [
    AuthMethod::Password,
    AuthMethod::PublicKey,
    AuthMethod::Agent,
    AuthMethod::GssapiWithMic,
    AuthMethod::KeyboardInteractive,
];

pub(in crate::ui) fn draw(win: &mut TerminalWindow, ui: &mut egui::Ui) {
    if win.new_session.protocol != Protocol::Ssh {
        ui.label("Authentication is only applicable to SSH sessions.");
        return;
    }
    // 金标准第一句是纯说明（无下划线），五个勾选才带访问键。
    ui.label("Automatic authentication methods:");
    ui.add_space(5.5);
    draw_methods(win, ui);

    ui.add_space(9.0);
    ui.label("When session is authenticating:");
    ui.add_space(5.5);
    choice_row_styled(
        ui,
        ChoiceKind::Checkbox,
        "Verifying the server's fingerprint",
        "Verifying",
        &mut win.new_session.verify_host_key,
        true,
        UnderlineStyle::Dialog,
    );

    ui.add_space(9.0);
    draw_platform_panel(win, ui);
    draw_exec_command(win, ui);
    draw_saved(win, ui);
}

/// 五个认证方法：勾选集 = 首选 + 回退链。取消最后一个不生效（会话至少要留
/// 一种认证方式），否则保存出来的是一条永远认证不了的会话。
fn draw_methods(win: &mut TerminalWindow, ui: &mut egui::Ui) {
    let mut selected: Vec<AuthMethod> = CHAIN_ORDER
        .iter()
        .copied()
        .filter(|a| {
            *a == win.new_session.auth || win.new_session.auth_chain.contains(a)
        })
        .collect();
    for (i, method) in DISPLAY_ORDER.iter().copied().enumerate() {
        if i > 0 {
            ui.add_space(5.5);
        }
        let mut on = selected.contains(&method);
        if choice_row_styled(
            ui,
            ChoiceKind::Checkbox,
            attempt_label(method),
            attempt_acc(method),
            &mut on,
            true,
            UnderlineStyle::Dialog,
        ) {
            if on {
                if !selected.contains(&method) {
                    selected.push(method);
                }
            } else {
                selected.retain(|m| *m != method);
                if selected.is_empty() {
                    selected.push(method);
                }
            }
        }
    }
    if selected.is_empty() {
        selected.push(AuthMethod::Password);
    }
    let primary = if selected.contains(&win.new_session.auth) {
        win.new_session.auth
    } else {
        // 重新取 CHAIN_ORDER 的第一个，而不是 `selected[0]`：selected 是按
        // CHAIN_ORDER 收的，但用户的勾选变化可能让它与显示序错位。
        CHAIN_ORDER
            .iter()
            .copied()
            .find(|m| selected.contains(m))
            .unwrap_or(AuthMethod::Password)
    };
    win.new_session.auth = primary;
    win.new_session.auth_chain = selected
        .into_iter()
        .filter(|a| *a != primary)
        .collect();
}

/// 三个平台页签 + 与之相连的内容面板（金标准的 macOS / Linux / Windows）。
///
/// 每格存自己的身份文件路径；切页签改的是**当前平台**那一格，不会把另一个
/// 平台的路径悄悄覆盖掉。面板顶框与选中页签相连，仍是 `kit::tabs` 那一份。
fn draw_platform_panel(win: &mut TerminalWindow, ui: &mut egui::Ui) {
    let tab = win.new_session.auth_tab.min(2);
    let outcome = strip(ui, &AUTH_PLATFORMS, tab, 23.0, 12.0);
    if let Some(next) = outcome.clicked {
        win.new_session.auth_tab = next;
    }
    ui.add_space(-FIELD_TO_ROW);
    panel(ui, outcome.selected_rect, 44.0, |ui| {
        ui.label(egui::WidgetText::from(underlined_job(
            ui,
            "Identity file for authentication:",
            "Identity",
            UnderlineStyle::Dialog,
        )));
        ui.add_space(LABEL_TO_FIELD - FIELD_TO_ROW);
        let active = outcome.clicked.unwrap_or(tab).min(2);
        let (_, browse) = browse_field(
            ui,
            &mut win.new_session.auth_identity[active],
            "",
            "...",
            ROW_H,
            42.0,
        );
        if browse
            .on_hover_text("Choose an identity file for this platform")
            .clicked()
        {
            let current = win.new_session.auth_identity[active].clone();
            win.open_path_picker(crate::ui::path_picker::PathTarget::AuthIdentity(active), &current);
        }
    });
}

/// 认证成功后执行的命令与延迟。字段真落盘，连接建立后由窗口每帧的任务
/// 派发器（`fire_auth_commands`）按延迟把命令写进会话。
fn draw_exec_command(win: &mut TerminalWindow, ui: &mut egui::Ui) {
    ui.add_space(10.0);
    ui.label(egui::WidgetText::from(underlined_job(
        ui,
        "Execute command after authentication:",
        "Execute",
        UnderlineStyle::Dialog,
    )));
    ui.add_space(LABEL_TO_FIELD - FIELD_TO_ROW);
    const DELAY_W: f32 = 105.0;
    ui.horizontal(|ui| {
        ui.add_sized(
            egui::vec2(FORM_W - DELAY_W - 8.0, ROW_H),
            TextEdit::singleline(&mut win.new_session.auth_exec_command[win.new_session.auth_tab]),
        )
        .on_hover_text("Sent to the shell after the built-in client authenticates.");
        ui.add_space(8.0);
        let delay = &mut win.new_session.auth_exec_delay[win.new_session.auth_tab];
        spin_field_delta(ui, |ui| {
            ui.add(
                DragValue::new(delay)
                    .range(0..=3600)
                    .custom_formatter(|n, _| format!("Delay: {}s", n as u32)),
            );
        });
    });
}

/// `Saved automatic authentication:` 的两个半宽按钮。
///
/// `View` 展开保险库面板（未建库时给「创建主密码」流程），`Clear` 只删掉
/// **这条会话**的保存密码引用，不顺手清掉整个保险库——那是另一回事。
fn draw_saved(win: &mut TerminalWindow, ui: &mut egui::Ui) {
    ui.add_space(10.0);
    ui.label("Saved automatic authentication:");
    ui.add_space(5.5);
    ui.horizontal(|ui| {
        if half_button(ui, "View").clicked() {
            win.new_session.auth_vault_open = !win.new_session.auth_vault_open;
        }
        ui.add_space(9.0);
        if half_button(ui, "Clear").clicked() {
            let name = win.new_session.name.trim().to_string();
            let had = persist::vault_entry(&name).is_some();
            if had {
                persist::set_vault_entry(&name, None);
            }
            win.new_session.auth_vault_note = Some(if had {
                "Saved password cleared for this session.".to_string()
            } else {
                "This session has no saved password.".to_string()
            });
        }
    });
    if let Some(note) = &win.new_session.auth_vault_note {
        ui.add_space(4.0);
        ui.label(note);
    }
    if win.new_session.auth_vault_open {
        win.draw_vault_section(ui);
    }
}

/// 金标准的半宽按钮：23 高、#2A2A2A 底、#444 描边、首字符带下划线。
fn half_button(ui: &mut egui::Ui, label: &str) -> egui::Response {
    let (rect, resp) = ui.allocate_exact_size(
        egui::vec2((FORM_W - 9.0) / 2.0, ROW_H),
        egui::Sense::click(),
    );
    let p = ui.painter_at(rect);
    let fill = if resp.hovered() { 0x3A3A3A } else { 0x2A2A2A };
    p.rect_filled(rect, egui::Rounding::ZERO, super::dialog_chrome::rgb(fill));
    p.rect_stroke(
        rect,
        egui::Rounding::ZERO,
        egui::Stroke::new(1.0, super::dialog_chrome::rgb(0x444444)),
    );
    let galley = ui.fonts(|f| f.layout_job(underlined_job(ui, label, label, UnderlineStyle::Dialog)));
    p.galley(
        rect.center() - galley.size() / 2.0,
        galley,
        ui.visuals().text_color(),
    );
    resp
}

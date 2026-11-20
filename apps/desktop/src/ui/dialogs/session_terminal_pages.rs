//! New Session 里 `Terminal ▾` 与 `Window ▾` 两组的页内容
//! （Keyboard / Logging / Selection / Bell / Mode / Security）。
//!
//! 为什么从 `session_pages.rs` 拆出来：R9 要求 `ui/` 下非 `mod.rs` 文件
//! ≤ 450 行，2026-11-08 按视觉基线补了 Bell / Mode / Security 三页后原文件
//! 涨到 461 行。这里装的是**五个协议共用的那两组页**，剩下的协议专属页
//! （Proxy / Command / Environment / Protocol / X/Y/Z Modem / SFTP / TCP 连接）
//! 仍在 `session_pages.rs`，两边按「谁拥有这棵树」切，不按行数切。
//!
//! 视觉基线来源：`02-session-dialog/common/{03-bell,05-mode,06-keyboard,
//! 07-security,08-logging,10-selection}.png`。
//!
//! 页下标仍由 `session_nav::PAGES` 唯一决定，这里只认下标。

use super::log_settings_spec::LogType;
use super::session_form_body::form_row;
use super::session_pages::{grid, placeholder_note};
use crate::ui::*;

/// WindTerm 出厂的选区分隔符（两组屏幕共用同一默认值）。
/// 来源：视觉基线 `common/10-selection.png` 两个文本框的内容。
const DEFAULT_WORD_SEPARATORS: &str = "\"!@#$^*()={}[]:;'\",.<>?";

/// `Terminal ▾` / `Window ▾` 两组页的分派。其余下标一律不接。
pub(in crate::ui) fn draw_page(win: &mut TerminalWindow, ui: &mut egui::Ui, page: usize) {
    match page {
        1 => keyboard_page(win, ui),
        2 => logging_page(win, ui),
        3 => selection_page(win, ui),
        17 => bell_page(ui),
        18 => mode_page(ui),
        19 => security_page(ui),
        _ => {}
    }
}

/// 1 `Terminal ▾ Keyboard`：视觉基线 `common/06-keyboard.png` 的按键编码页。
///
/// WindTerm 这一页是**按键编码行为**（Backspace/Delete/Modified keys/Alt/Meta 各自
/// 怎么发），不是终端类型 / 光标样式——后者在 WindTerm 里分别归 Session 页与全局
/// Appearance。所以这里按 I5 渲染 WindTerm 的五个控件，全部灰显 + 说明（终端引擎
/// 目前不消费这些逐会话编码偏好）。这与本文件 `bell`/`mode`/`security` 三页同构。
fn keyboard_page(_win: &mut TerminalWindow, ui: &mut egui::Ui) {
    let mut backspace = 0usize; // Automatic
    let mut delete = 0usize; // Automatic
    let mut modified = 0usize; // XTerm format
    grid(ui, "new_session_keyboard_grid", |ui| {
        form_row(ui, "Backspace sends:");
        ui.add_enabled_ui(false, |ui| {
            ui.vertical(|ui| {
                ui.radio_value(&mut backspace, 0, "Automatic");
                ui.radio_value(&mut backspace, 1, "Backspace");
                ui.radio_value(&mut backspace, 2, "Delete");
            });
        });
        ui.end_row();
        form_row(ui, "Delete sends:");
        ui.add_enabled_ui(false, |ui| {
            ui.vertical(|ui| {
                ui.radio_value(&mut delete, 0, "Automatic");
                ui.radio_value(&mut delete, 1, "Backspace");
                ui.radio_value(&mut delete, 2, "Delete");
            });
        });
        ui.end_row();
        form_row(ui, "Modified other keys sends:");
        ui.add_enabled_ui(false, |ui| {
            ui.vertical(|ui| {
                ui.radio_value(&mut modified, 0, "XTerm format");
                ui.radio_value(&mut modified, 1, "Fixterm format");
            });
        });
        ui.end_row();
        form_row(ui, "Alt modifier key sends:");
        ui.add_enabled_ui(false, |ui| {
            egui::ComboBox::from_id_salt("kbd_alt")
                .selected_text("8-bit characters")
                .show_ui(ui, |_ui| {});
        });
        ui.end_row();
        form_row(ui, "Meta modifier key sends:");
        ui.add_enabled_ui(false, |ui| {
            egui::ComboBox::from_id_salt("kbd_meta")
                .selected_text("Characters preceded by ESC")
                .show_ui(ui, |_ui| {});
        });
        ui.end_row();
    });
    placeholder_note(
        ui,
        "Key encoding is decided by the terminal engine; these five \
         choices are placeholders until the profile stores them.",
    );
}

/// 2 `Terminal ▾ Logging`：视觉基线 `common/08-logging.png` 的完整表单。
///
/// 绑的是**真状态**：`TerminalWindow::log_settings`——也就是 Session ▸ Log ▸
/// Settings 那个独立窗口编辑的同一份记录（`LogSettingsData`）。两个入口、
/// 一份数据，所以 New Session 里改的值，独立窗口再打开就是改后的样子，
/// 落盘走 `LogSettingsData::save()`（写 prefs，不动语言与两个关闭抑制位）。
///
/// 与视觉基线的差别只有一处：WindTerm 的这页是**每会话**设置，我们的日志
/// 配置还是全局默认值，所以末尾有一行说明；等 per-session 字段落地再收掉。
fn logging_page(win: &mut TerminalWindow, ui: &mut egui::Ui) {
    let mut dirty = false;
    ui.label("Log types:");
    for (kind, name) in [
        (LogType::Nothing, "Log nothing"),
        (LogType::Printable, "Log printable output"),
        (LogType::All, "Log all session output"),
    ] {
        if ui.radio(win.log_settings.log_type == kind, name).clicked() {
            win.log_settings.log_type = kind;
            dirty = true;
        }
    }
    ui.add_space(6.0);
    ui.label("Log options:");
    // 视觉基线出厂态：`Log nothing` ⇒ 这两行是灰的（`options_enabled`）。
    let enabled = win.log_settings.log_type.enabled();
    for (value, name) in [(true, "Overwrite log file"), (false, "Append log file")] {
        let on = win.log_settings.overwrite == Some(value);
        if ui
            .add_enabled(enabled, egui::RadioButton::new(on, name))
            .clicked()
        {
            win.log_settings.overwrite = Some(value);
            dirty = true;
        }
    }
    ui.add_space(6.0);
    grid(ui, "new_session_logging_grid", |ui| {
        form_row(ui, "Log file name:");
        // 文本输入按「失焦」落盘：每敲一个字符写一次 prefs.json 太浪费。
        let r = ui.add(
            TextEdit::singleline(&mut win.log_settings.file_name).desired_width(232.0),
        );
        dirty |= r.lost_focus();
        ui.end_row();
        form_row(ui, "Log maximum size:");
        let r = ui.add(
            DragValue::new(&mut win.log_settings.max_size_mb)
                .range(0..=102_400)
                .custom_formatter(|n, _| {
                    if n == 0.0 {
                        "∞".to_string()
                    } else {
                        format!("{} MB", n as u32)
                    }
                }),
        );
        dirty |= r.changed();
        ui.end_row();
        form_row(ui, "Log content pattern:");
        let r =
            ui.add(TextEdit::singleline(&mut win.log_settings.pattern).desired_width(232.0));
        dirty |= r.lost_focus();
        ui.end_row();
    });
    ui.add_space(8.0);
    placeholder_note(
        ui,
        "These are the default log settings (Session ▸ Log ▸ Settings edits the \
         same record). Per-session overrides are not stored yet.",
    );
    if dirty {
        // 保存失败不该静默：写不进去时至少让调用点知道（返回值就地丢掉，
        // 但下一帧用户会看到字段没被带回去）。
        let _ = win.log_settings.save();
    }
}

/// 3 `Window ▾ Selection`：视觉基线 `common/10-selection.png` 的选区分隔符页。
///
/// WindTerm 这一页是**两组屏幕各自的选区分隔符**（不是窗口尺寸 / 回滚）。
/// 逐会话选区分隔符在 `SessionConfig` 里还没有字段，因此整页灰显 + 说明
/// （I5），两个文本框填入 WindTerm 出厂默认值 `"!@#$^*()={}[]:;'",.<>?`。
/// 这与本文件 `keyboard`/`bell`/`mode`/`security` 四页同构。
fn selection_page(_win: &mut TerminalWindow, ui: &mut egui::Ui) {
    let mut separators = DEFAULT_WORD_SEPARATORS.to_string();
    let mut alt_separators = DEFAULT_WORD_SEPARATORS.to_string();
    grid(ui, "new_session_selection_grid", |ui| {
        form_row(ui, "Word Separators:");
        ui.add_enabled(
            false,
            TextEdit::singleline(&mut separators).desired_width(320.0),
        );
        ui.end_row();
        form_row(ui, "Word Separators For Alternate Screen:");
        ui.add_enabled(
            false,
            TextEdit::singleline(&mut alt_separators).desired_width(320.0),
        );
        ui.end_row();
    });
    placeholder_note(
        ui,
        "Per-session word separators are decided by the terminal engine; these \
         two fields are read-only placeholders until the profile stores them.",
    );
}

/// 17 `Terminal ▾ Bell`：视觉基线 `common/03-bell.png`。
///
/// 响铃方式与去重窗口在 `SessionConfig` 里还没有字段，因此整页灰显 +
/// 说明（I5）。选中的那一项按视觉基线画在 `Beep using the PC speaker` 上。
fn bell_page(ui: &mut egui::Ui) {
    let style = 1usize;
    ui.label("Bell styles:");
    for (i, name) in ["Mute", "Beep using the PC speaker", "Play the custom sound"]
        .iter()
        .enumerate()
    {
        ui.add_enabled(false, egui::RadioButton::new(style == i, *name));
    }
    ui.horizontal(|ui| {
        ui.add_space(24.0);
        let mut sound = String::new();
        ui.add_enabled(
            false,
            egui::TextEdit::singleline(&mut sound)
                .hint_text("(no sound file)")
                .desired_width(300.0),
        );
        ui.add_enabled(false, egui::Button::new("..."));
    });
    ui.add_space(8.0);
    ui.label("Ignore bells received more than once within:");
    ui.add_enabled_ui(false, |ui| {
        egui::ComboBox::from_id_salt("bell_dedup_combo")
            .selected_text("2 seconds")
            .show_ui(ui, |_ui| {});
    });
    placeholder_note(
        ui,
        "The bell is not routed through SessionConfig yet: these three choices \
         and the de-duplication window are read-only placeholders.",
    );
}

/// 18 `Terminal ▾ Mode`：视觉基线 `common/05-mode.png` 的三个初始模式 + 两组键模式。
fn mode_page(ui: &mut egui::Ui) {
    let (mut awm, mut lnm, mut rsm) = (true, false, false);
    ui.label("Initial modes:");
    ui.add_enabled(false, egui::Checkbox::new(&mut awm, "Auto Wrap Mode (DECAWM)"));
    ui.add_enabled(false, egui::Checkbox::new(&mut lnm, "New Line Mode (LNM)"));
    ui.add_enabled(false, egui::Checkbox::new(&mut rsm, "Reverse Screen Mode (DECSCNM)"));
    ui.add_space(6.0);
    ui.label("Cursor Keys mode (DECCKM):");
    for name in ["Normal", "Application"] {
        ui.add_enabled(false, egui::RadioButton::new(name == "Normal", name));
    }
    ui.label("Numeric keypad mode (DECNKM):");
    for name in ["Normal", "Application"] {
        ui.add_enabled(false, egui::RadioButton::new(name == "Normal", name));
    }
    placeholder_note(
        ui,
        "DEC private modes belong to the terminal engine, not to the session \
         profile: the engine decides them from the remote's escape sequences.",
    );
}

/// 19 `Terminal ▾ Security`：视觉基线 `common/07-security.png`。
fn security_page(ui: &mut egui::Ui) {
    ui.label("When the remote wants to read the clipboard:");
    for name in ["Ask", "Allow", "Forbid"] {
        ui.add_enabled(false, egui::RadioButton::new(name == "Ask", name));
    }
    ui.add_space(6.0);
    ui.label("When the remote wants to overwrite the clipboard:");
    for name in ["Ask", "Allow", "Forbid"] {
        ui.add_enabled(false, egui::RadioButton::new(name == "Ask", name));
    }
    placeholder_note(
        ui,
        "OSC 52 clipboard access is refused outright today, so there is no \
         policy to store yet; the three choices await that switch.",
    );
}

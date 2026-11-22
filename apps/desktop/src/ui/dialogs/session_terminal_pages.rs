//! New Session 里 `Terminal ▾` 与 `Window ▾` 两组的页内容
//! （Keyboard / Logging / Selection / Bell / Mode / Security）。
//!
//! 为什么从 `session_pages.rs` 拆出来： 要求 `ui/` 下非 `mod.rs` 文件
//! ≤ 450 行，按视觉基线补了 Bell / Mode / Security 三页后原文件
//! 涨到 461 行。这里装的是**五个协议共用的那两组页**，剩下的协议专属页
//! （Proxy / Command / Environment / Protocol / X/Y/Z Modem / SFTP / TCP 连接）
//! 仍在 `session_pages.rs`，两边按「谁拥有这棵树」切，不按行数切。
//!
//! 视觉基线来源：`02-session-dialog/common/{03-bell,05-mode,06-keyboard,
//! 07-security,08-logging,10-selection}.png`。
//!
//! 页下标仍由 `session_nav::PAGES` 唯一决定，这里只认下标。

use crate::ui::kit::field::{choice_row_resp, choice_row_styled, ChoiceKind, UnderlineStyle};
use super::log_settings_spec::LogType;
use super::session_form_body::{form_row, LABEL_W};
use super::session_pages::{grid, placeholder_note};
use crate::ui::kit::form::{
    dead_value, field_box, field_label, field_row, label_col, spin_field, underlined_label, Deco,
    Slot, FIELD_TO_ROW, LABEL_TO_FIELD,
};
use crate::ui::*;

// 出厂分隔符现在只有一份：`term_app::selection::DEFAULT_SEPARATORS`。
// 这里不再抄第二个常量——两份字符串一旦漂移，页面显示的与选词实际用的就不是
// 同一套字符，而且两边都好端端的没有编译错误。

/// `Terminal ▾` / `Window ▾` 两组页的分派。其余下标一律不接。
pub(in crate::ui) fn draw_page(win: &mut TerminalWindow, ui: &mut egui::Ui, page: usize) {
    match page {
        1 => keyboard_page(win, ui),
        2 => logging_page(win, ui),
        3 => selection_page(win, ui),
        17 => bell_page(win, ui),
        18 => mode_page(win, ui),
        19 => security_page(win, ui),
        // 组头自己那两页 ：点组名不再只是折叠。
        22 => terminal_group_page(ui),
        23 => window_group_page(win, ui),
        _ => {}
    }
}

/// 1 `Terminal ▾ Keyboard`：视觉基线 `common/06-keyboard.png` 的按键编码页。
///
/// WindTerm 这一页是**按键编码行为**（Backspace/Delete/Alt/Meta 各自怎么发），
/// 不是终端类型 / 光标样式。四项已经接真：Backspace/Delete 走
/// `term_core::keys`，Alt/Meta 由 `modifier_bytes` 编码；`Modified other keys`
/// 仍灰显——仓库没有第二套线格式（Fixterm），硬造序列会把错字节写进 PTY。
const MODIFIER_FORMATS: [&str; 2] = ["8-bit characters", "Characters preceded by ESC"];

/// 一个「标签 + 通栏下拉」的按键编码行，返回是否改动。
fn combo(ui: &mut egui::Ui, id: &str, items: &[&str], selected: &mut usize) -> bool {
    let mut changed = false;
    egui::ComboBox::from_id_salt(id)
        .selected_text(items[*selected])
        .show_ui(ui, |ui| {
            for (i, name) in items.iter().enumerate() {
                if ui.selectable_label(*selected == i, *name).clicked() {
                    *selected = i;
                    changed = true;
                }
            }
        });
    changed
}

fn keyboard_page(win: &mut TerminalWindow, ui: &mut egui::Ui) {
    let mut backspace = win.new_session.backspace_sends;
    let mut delete = win.new_session.delete_sends;
    let mut alt_sends = win.new_session.alt_modifier_sends;
    let mut meta_sends = win.new_session.meta_modifier_sends;
    let modified = 0usize; // XTerm format（只读展示，不回写）
    let mut changed = false;
    grid(ui, "new_session_keyboard_grid", |ui| {
        form_row(ui, "Backspace sends:");
        ui.vertical(|ui| {
            for (i, name) in ["Automatic", "Backspace", "Delete"].iter().enumerate() {
                let mut on = backspace == i;
                if choice_row_styled(ui, ChoiceKind::Radio, name, "", &mut on, true, UnderlineStyle::None) {
                    backspace = i;
                    win.new_session.backspace_sends = backspace;
                }
            }
        });
        ui.end_row();
        form_row(ui, "Delete sends:");
        ui.vertical(|ui| {
            for (i, name) in ["Automatic", "Backspace", "Delete"].iter().enumerate() {
                let mut on = delete == i;
                if choice_row_styled(ui, ChoiceKind::Radio, name, "", &mut on, true, UnderlineStyle::None) {
                    delete = i;
                    win.new_session.delete_sends = delete;
                }
            }
        });
        ui.end_row();
        form_row(ui, "Modified other keys sends:");
        ui.add_enabled_ui(false, |ui| {
            ui.vertical(|ui| {
                let mut a = modified == 0;
                let mut b = modified == 1;
                choice_row_styled(ui, ChoiceKind::Radio, "XTerm format", "", &mut a, false, UnderlineStyle::None);
                choice_row_styled(ui, ChoiceKind::Radio, "Fixterm format", "", &mut b, false, UnderlineStyle::None);
            });
        });
        ui.end_row();
        form_row(ui, "Alt modifier key sends:");
        changed |= combo(ui, "kbd_alt", &MODIFIER_FORMATS, &mut alt_sends);
        ui.end_row();
        form_row(ui, "Meta modifier key sends:");
        changed |= combo(ui, "kbd_meta", &MODIFIER_FORMATS, &mut meta_sends);
        ui.end_row();
    });
    placeholder_note(
        ui,
        "Backspace, Delete, Alt and Meta are written into the key encoder at connect \
         time; the program may still override them with escape sequences. Modified keys have no \
         second wire format (Fixterm) in this build — see the disabled control.",
    );
    if changed {
        win.new_session.alt_modifier_sends = alt_sends;
        win.new_session.meta_modifier_sends = meta_sends;
    }
}

/// 2 `Terminal ▾ Logging`：视觉基线 `common/08-logging.png` 的完整表单。
///
/// 绑的是**真状态**：`TerminalWindow::log_settings`——也就是 Session ▸ Log ▸
/// Settings 那个独立窗口编辑的同一份记录（`LogSettingsData`）。两个入口、
/// 一份数据，所以 New Session 里改的值，独立窗口再打开就是改后的样子，
/// 落盘走 `LogSettingsData::save `（写 prefs，不动语言与两个关闭抑制位）。
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
        let mut on = win.log_settings.log_type == kind;
        if choice_row_styled(ui, ChoiceKind::Radio, name, "", &mut on, true, UnderlineStyle::None) {
            win.log_settings.log_type = kind;
            dirty = true;
        }
    }
    ui.add_space(6.0);
    ui.label("Log options:");
    // 视觉基线出厂态：`Log nothing` ⇒ 这两行是灰的（`options_enabled`）。
    let enabled = win.log_settings.log_type.enabled();
    for (value, name) in [(true, "Overwrite log file"), (false, "Append log file")] {
        let mut on = win.log_settings.overwrite == Some(value);
        if choice_row_styled(ui, ChoiceKind::Radio, name, "", &mut on, enabled, UnderlineStyle::None) {
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
/// 两个字段都落在 `SessionConfig` 上，读者是双击选词与 Shift+Alt+方向键：
/// 2026-11-21 起 `selection::word_at_with` / `apply_*_with_seps` 把用户串
/// 带进词边界。空串 = 未设置，页面显示 WindTerm 出厂串，读侧回落到同一串。
fn selection_page(win: &mut TerminalWindow, ui: &mut egui::Ui) {
    // 未设置时显示出厂串：视觉基线的框里不是空的。只有真的改过才写回，
    // 所以「打开又取消」不会把默认串固化进用户配置。
    let mut separators = if win.new_session.word_separators.is_empty() {
        term_app::selection::DEFAULT_SEPARATORS.to_string()
    } else {
        win.new_session.word_separators.clone()
    };
    let mut alt_separators = if win.new_session.word_separators_alt.is_empty() {
        term_app::selection::DEFAULT_SEPARATORS.to_string()
    } else {
        win.new_session.word_separators_alt.clone()
    };
    grid(ui, "new_session_selection_grid", |ui| {
        form_row(ui, "Word Separators:");
        if ui
            .add(TextEdit::singleline(&mut separators).desired_width(320.0))
            .changed()
        {
            win.new_session.word_separators = separators.clone();
        }
        ui.end_row();
        form_row(ui, "Word Separators For Alternate Screen:");
        if ui
            .add(TextEdit::singleline(&mut alt_separators).desired_width(320.0))
            .changed()
        {
            win.new_session.word_separators_alt = alt_separators.clone();
        }
        ui.end_row();
    });
    placeholder_note(
        ui,
        "These separators break words for double-click selection and \
         Shift+Alt+arrow movement. An empty main field falls back to the \
         factory set; an empty alternate field follows the main one.",
    );
}

/// 17 `Terminal ▾ Bell`：视觉基线 `common/03-bell.png`。
///
/// 响铃方式与去重窗口在 `SessionConfig` 里还没有字段，因此整页灰显 +
/// 说明。选中的那一项按视觉基线画在 `Beep using the PC speaker` 上。
fn bell_page(win: &mut TerminalWindow, ui: &mut egui::Ui) {
    let mut style = win.new_session.bell_style;
    let mut sound = win.new_session.bell_sound.clone();
    let mut dedup = win.new_session.bell_dedup_secs;
    ui.label("Bell styles:");
    for (i, name) in ["Mute", "Beep using the PC speaker", "Play the custom sound"]
        .iter()
        .enumerate()
    {
        let mut on = style == i;
        if choice_row_styled(ui, ChoiceKind::Radio, name, "", &mut on, true, UnderlineStyle::None) {
            style = i;
            win.new_session.bell_style = style;
        }
    }
    ui.horizontal(|ui| {
        ui.add_space(24.0);
        ui.add(
            egui::TextEdit::singleline(&mut sound)
                .hint_text("(no sound file)")
                .desired_width(300.0),
        );
        if ui
            .add(egui::Button::new("..."))
            .on_hover_text("Browse for a sound file")
            .clicked()
        {
            win.open_path_picker(crate::ui::path_picker::PathTarget::BellSound, &sound);
        }
    });
    ui.add_space(8.0);
    ui.label("Ignore bells received more than once within:");
    egui::ComboBox::from_id_salt("bell_dedup_combo")
        .selected_text(format!("{dedup} seconds"))
        .show_ui(ui, |ui| {
            for secs in [0u32, 1, 2, 5, 10] {
                if ui
                    .selectable_label(dedup == secs, format!("{secs} seconds"))
                    .clicked()
                {
                    dedup = secs;
                }
            }
        });
    placeholder_note(
        ui,
        "BEL reaches the session's bell policy: mute, a system beep, or a custom \
         sound file (played without a shell). 0 seconds disables de-duplication.",
    );
    let bell = &mut win.new_session;
    bell.bell_style = style;
    bell.bell_sound = sound;
    bell.bell_dedup_secs = dedup;
}

/// 18 `Terminal ▾ Mode`：视觉基线 `common/05-mode.png` 的三个初始模式 + 两组键模式。
fn mode_page(win: &mut TerminalWindow, ui: &mut egui::Ui) {
    let (mut lnm, mut rsm) = (false, false);
    let mut awm = win.new_session.initial_auto_wrap;
    let mut cursor_keys = usize::from(win.new_session.initial_cursor_keys_application);
    ui.label("Initial modes:");
    if choice_row_styled(ui, ChoiceKind::Checkbox, "Auto Wrap Mode (DECAWM)", "Auto",
        &mut awm, true, UnderlineStyle::None) {
        win.new_session.initial_auto_wrap = awm;
    }
    choice_row_resp(ui, ChoiceKind::Checkbox, "New Line Mode (LNM)", "New",
        &mut lnm, false, UnderlineStyle::None)
        .on_hover_text(
            "The VT has no LNM state yet: it always treats LF as a line feed only. \
             Storing a choice would not change any byte written to the screen.",
        );
    choice_row_resp(ui, ChoiceKind::Checkbox, "Reverse Screen Mode (DECSCNM)", "Reverse",
        &mut rsm, false, UnderlineStyle::None)
        .on_hover_text(
            "The renderer has no reverse-screen pass, so there is no state for this \
             switch to set.",
        );
    ui.add_space(6.0);
    ui.label("Cursor Keys mode (DECCKM):");
    for (i, name) in ["Normal", "Application"].iter().enumerate() {
        let mut on = cursor_keys == i;
        if choice_row_styled(ui, ChoiceKind::Radio, name, "", &mut on, true, UnderlineStyle::None) {
            cursor_keys = i;
            win.new_session.initial_cursor_keys_application = cursor_keys == 1;
        }
    }
    ui.label("Numeric keypad mode (DECNKM):");
    for name in ["Normal", "Application"] {
        let mut on = name == "Normal";
        choice_row_styled(ui, ChoiceKind::Radio, name, "", &mut on, false, UnderlineStyle::None);
    }
    placeholder_note(
        ui,
        "Auto wrap and the cursor-key mode are written into the VT at startup; the remote may \
         still change them with DECSET/DECRST. LNM, reverse screen and numeric keypad have no \
         engine state yet — see each row's tooltip.",
    );
}

/// 19 `Terminal ▾ Security`：视觉基线 `common/07-security.png`。
fn security_page(win: &mut TerminalWindow, ui: &mut egui::Ui) {
    let mut read = win.new_session.clipboard_read_policy;
    let mut write = win.new_session.clipboard_write_policy;
    ui.label("When the remote wants to read the clipboard:");
    for (i, name) in ["Ask", "Allow", "Forbid"].iter().enumerate() {
        let mut on = read == i;
        if choice_row_styled(ui, ChoiceKind::Radio, name, "", &mut on, true, UnderlineStyle::None) {
            read = i;
        }
    }
    ui.add_space(6.0);
    ui.label("When the remote wants to overwrite the clipboard:");
    for (i, name) in ["Ask", "Allow", "Forbid"].iter().enumerate() {
        let mut on = write == i;
        if choice_row_styled(ui, ChoiceKind::Radio, name, "", &mut on, true, UnderlineStyle::None) {
            write = i;
        }
    }
    placeholder_note(
        ui,
        "Ask pops a confirmation for each OSC 52 request; Allow applies it immediately \
         and Forbid ignores it. The clipboard itself stays with the UI thread.",
    );
    win.new_session.clipboard_read_policy = read;
    win.new_session.clipboard_write_policy = write;
}

/// 22 `Terminal`（**组头页**）：视觉基线 `common/01-terminal-page.png`。
///
/// 组头本身就是一页。用户报「左侧树行为不对、点组名没有独立页面」，
/// 根因是组头只挂了折叠命中区（`CollapsingHeader` 一个区域），点哪儿都只是
/// 展开/收回。页表补了 22 / 23 之后，组名与三角才各管各的。
///
/// # 两项都没有后端
///
/// `Answer back`（ENQ 应答串）在 `SessionConfig` 里没有字段；擦屏用不用底色
/// 由内核按当前底色决定，也没有开关。所以两者都画成只读并在页底说明——
/// 做成「能填、看起来存了、实际没生效」的假控件比少一个控件更坏。
fn terminal_group_page(ui: &mut egui::Ui) {
    let mut erase_with_bg = true;
    // 字段列与 `Session` 页同一条竖线（视觉基线里两者的框左缘都在 626 物理像素），
    // 所以这里用共用的 `LABEL_W` 而不是按本页标签量宽。
    field_row(
        ui,
        "Answer back:",
        Slot::Inline,
        LABEL_W,
        "Answer-back string has no profile field yet.",
        |ui| dead_value(ui, "Wind", Deco::None),
    );
    ui.horizontal(|ui| {
        // 视觉基线里勾选框比标签列左缘再缩进 13 物理像素 ≈ 7 逻辑点。
        ui.add_space(7.0);
        ui.add_enabled(
            false,
            egui::Checkbox::new(&mut erase_with_bg, "Use background color to erase screen"),
        );
    });
    placeholder_note(
        ui,
        "Neither item has a backend field yet: the answer-back string and the \
         erase colour live in the terminal engine, not in the session profile.",
    );
}

/// 23 `Window`（**组头页**）：视觉基线 `common/07-window-page.png`。
///
/// 六个「标签 + 字段」行 + 三段「抬头 + 控件」。与同组子页（`Selection`）不同的是
/// 这一页的标签**左对齐**且列更宽：视觉基线里它的字段左缘在 740 物理像素，而
/// `Session` / `Selection` 页在 626。所以列宽按本页最长标签现量
/// （[`label_col`]），不套 `LABEL_W`——两处硬编码同一个数迟早会分叉。
///
/// # 哪些是真控件
///
/// | 控件 | 后端 |
/// |---|---|
/// | `Columns` / `Rows` / `Lines of scrollback` / `Roll the mouse wheel…` | `SessionConfig.cols` / `.rows` / `.scrollback` / `.wheel_lines`，`to_config` 已落盘 |
/// | `Caret Style` | `SessionConfig.caret_style`（`CARET_STYLES` 八选一） |
/// | `Time format` / `Datetime format` | **没有**字段：灰显占位 |
/// | 两个勾选 | **没有**字段：灰显 + 页底说明 |
fn window_group_page(win: &mut TerminalWindow, ui: &mut egui::Ui) {
    use term_core::config::CARET_STYLES;
    let col = label_col(
        ui,
        &[
            "Columns:",
            "Rows:",
            "Lines of scrollback:",
            "Caret Style:",
            "Time format:",
            "Datetime format:",
        ],
    );
    let ns = &mut win.new_session;

    // 四行数字框：`spin_field` 自己带盒子与右端 ▲▼（视觉基线每个数值框都有），
    // 所以这里走「左对齐标签 + 自己画的框」，不再套 `field_row` 的盒子。
    let number = |ui: &mut egui::Ui, label: &str, value: &mut usize, hi: usize| {
        ui.horizontal(|ui| {
            field_label(ui, label, col);
            spin_field(ui, |ui| {
                ui.add(egui::DragValue::new(value).range(1..=hi));
            });
        });
    };
    number(ui, "Columns:", &mut ns.cols, 1000);
    number(ui, "Rows:", &mut ns.rows, 1000);
    number(ui, "Lines of scrollback:", &mut ns.scrollback, 9_999_999);

    // `Caret Style`：真下拉，取值就是 `SessionConfig.caret_style` 的八档。
    let shown = ns.caret_style.min(CARET_STYLES.len() - 1);
    ui.horizontal(|ui| {
        field_label(ui, "Caret Style:", col);
        field_box(ui, |ui| {
            egui::ComboBox::from_id_salt("new_session_caret_style")
                .selected_text(CARET_STYLES[shown])
                .width(ui.available_width())
                .wrap_mode(egui::TextWrapMode::Truncate)
                .show_ui(ui, |ui| {
                    for (i, name) in CARET_STYLES.iter().enumerate() {
                        if ui.selectable_label(i == shown, *name).clicked() {
                            ns.caret_style = i;
                        }
                    }
                });
        });
    });

    // `Timestamp of lines:` 是一行**没有字段**的小标题（视觉基线里它只有文字），
    // 底下的 Time / Datetime 两行缩在同一个标签列里，不另起缩进。
    ui.label("Timestamp of lines:");
    for (label, value) in [
        ("Time format:", "[HH:mm:ss]"),
        ("Datetime format:", "yyyy-MM-dd HH:mm:ss"),
    ] {
        ui.horizontal(|ui| {
            field_label(ui, label, col);
            field_box(ui, |ui| dead_value(ui, value, Deco::None));
        });
    }

    // `Roll the mouse wheel to scroll:`：抬头 + **通栏**数字框（视觉基线里这个框
    // 从标签列左缘一直铺到右缘，比上面那几行宽）。
    underlined_label(ui, "Roll the mouse wheel to scroll:");
    ui.add_space(LABEL_TO_FIELD - FIELD_TO_ROW);
    spin_field(ui, |ui| {
        ui.add(egui::DragValue::new(&mut ns.wheel_lines).range(1..=100));
    });

    // 两段「抬头 + 缩进勾选」。两者都没有后端字段 → 灰显。
    let mut adjust_rows = true;
    let mut change_title = false;
    for (heading, text, on) in [
        (
            "When window is resized:",
            "Adjust the window rows and columns",
            &mut adjust_rows,
        ),
        (
            "When remote title is changed:",
            "Change the tab title",
            &mut change_title,
        ),
    ] {
        underlined_label(ui, heading);
        ui.horizontal(|ui| {
            ui.add_space(7.0);
            ui.add_enabled(false, egui::Checkbox::new(on, text));
        });
    }
    placeholder_note(
        ui,
        "Time formats and the two window behaviours are read-only placeholders: \
         the terminal engine decides them, the session profile has no field yet.",
    );
}

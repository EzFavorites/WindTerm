//! `Terminal` 页：粘贴确认两勾、`Mouse Actions` 的右键三档与「选中即复制」、
//! 以及 `Enable local mode` 都是**真控件**——这一页没有灰行了。
//!
//! # 四组真控件的落点
//!
//! - `Show paste dialog on the main screen` / `on the alternative screen` →
//!   `Prefs.paste_confirm_main` / `paste_confirm_alt`（出厂都 `false` = 现状）。
//!   判据是 `dialogs::confirm_paste::paste_needs_confirm`；三条粘贴路径
//!   （Cmd+V / 右键 / 中键）最后都汇到 `terminal/pane.rs` 的那一个 `Event::Paste`。
//! - `Right click:` 三档 → `Prefs.right_click_action`，语义在
//!   `term_app::mouse::decide`：`Show menu` / `Paste text` /
//!   `Copy text if selected, otherwise paste text`。落地在
//!   `terminal/pane.rs` 的 `handle_right_click`：菜单档挂 `resp.context_menu`，
//!   另两档在同一位置判 `Secondary` 按下，粘贴走 `request_paste`、
//!   复制走 `ctx.copy_text`——三条路径都是既有实现，没有第二条。
//! - `When I select text, copy it to clipboard` → `Prefs.copy_on_select`，
//!   判据 `term_app::mouse::should_copy_on_select`，读者只有 `pane.rs` 里
//!   选区文本每帧刷新那一处（开关 + 非空 + 指针没按着 + 是新选区）。
//! - `Enable local mode` → 活动标签的 `Tab.local_mode`
//!   （`Window ▸ Local Mode` 同一位）。**它是这一页唯一不落盘的控件**：
//!   勾的是「当前这条会话现在只读」，不是一条全局偏好——四个消费者
//!   （`Tab::write` / `Tab::poll` / `terminal/pane.rs` / `terminal/view.rs`）
//!   全读那一位。点了走 `Action::Shell(ShellAction::ToggleLocalMode)`，
//!   与菜单同一条命令通道，不在本页直接翻字段。
//!
//! 三组落盘的改完**当场生效**：`win.paste_confirm_*` / `win.right_click` /
//! `win.copy_on_select` 是同帧写回的内存副本，下一次手势读的就是它。

use crate::ui::kit::field::{choice_row_resp, ChoiceKind, UnderlineStyle};
use crate::ui::kit::spec::BLOCK_GAP;
use super::settings_live::save_prefs;
use super::settings_pages_view::{field_row, group_box, set_row_rhythm, Slot};
use crate::ui::*;
use term_app::mouse::RightClickAction;
use term_app::shell::ShellAction;

/// 只读查看（`Window ▸ Local Mode`）这一行到底做了什么。
///
/// 它不再是灰行：勾的是**活动标签**的 `Tab.local_mode`，与菜单
/// `Window ▸ Local Mode` 是同一位——不是一条全局偏好，所以**不落盘**。
const LOCAL_MODE_NOTE: &str = "Read-only view of this session: no input, no auto-scroll, triggers \
     and scripts paused. Scrolling, selecting and copying still work. Per session, not saved.";

/// 右键三档的文案，下标与 `RightClickAction::index ` 一一对应。
///
/// 第二、三档按参考页原文（`Paste text` / `Copy text if selected, otherwise
/// paste text`）：第三档的文案就是它的判据，写短了（比如 `Copy or paste`）
/// 用户得靠试才知道「or」的条件是什么。
fn right_click_label(action: RightClickAction) -> &'static str {
    match action {
        RightClickAction::Menu => "Show menu",
        RightClickAction::Paste => "Paste text",
        RightClickAction::CopyOrPaste => "Copy text if selected, otherwise paste text",
    }
}

/// 选中即复制：说清它会**覆盖剪贴板**，以及什么时候不写。
const COPY_ON_SELECT_NOTE: &str = "Every new selection is written to the clipboard once the pointer is \
     released — this replaces whatever you copied last. Empty selections and mid-drag frames are skipped.";

/// 页底：粘贴确认的粒度说明。
///
/// 参考页把这两行放在 `When pasting multiple lines:` 底下，而这里的判据不看
/// 行数——**任何**一次粘贴都会按屏弹窗。不写这一句，用户会以为单行粘贴直通。
const TERMINAL_NOTE: &str = "Both paste confirmations apply immediately to every paste — not only \
     multi-line ones. Local mode is a read-only view of one session, not a global switch.";

/// `Terminal` 页。三组控件改完立刻写回窗口上的副本（下一次手势按新值走）并落盘。
pub(in crate::ui) fn terminal(win: &mut TerminalWindow, ui: &mut egui::Ui) {
    let mut main = win.paste_confirm_main;
    let mut alt = win.paste_confirm_alt;
    let mut right_click = win.right_click;
    let mut copy_on_select = win.copy_on_select;
    let mut mouse_changed = false;

    // 视觉基线 `07-terminal.png`：内容区第一个字是组标题 `Features`，没有页级灰标题。
    group_box(ui, Some("Features"), |ui| {
        set_row_rhythm(ui);
        // ：`Enable local mode` 从灰行接成真勾选。读的是**活动标签**的
        // 只读态（与菜单 `Window ▸ Local Mode` 同一位），点了走同一条命令通道——
        // 不在这一页里直接翻 `Tab.local_mode`，否则「设置页改的」与「菜单改的」
        // 会变成两条写路径，迟早一处漏了取消脚本、一处漏了提示。
        let mut local = win.local_mode_on();
        if choice_row_resp(ui, ChoiceKind::Checkbox, "Enable local mode", "", &mut local, true, UnderlineStyle::None)
            .on_hover_text(LOCAL_MODE_NOTE)
            .clicked()
        {
            win.dispatch_action(Action::Shell(ShellAction::ToggleLocalMode));
        }
    });

    ui.add_space(BLOCK_GAP);
    group_box(ui, Some("Mouse Actions"), |ui| {
        set_row_rhythm(ui);
        // 视觉基线 07-terminal.png：`Right click:` 自己一行，组合框在它**下方**
        // 占满整行——即 `Slot::Above`。三档文案见 `right_click_label`。
        let current = RightClickAction::from_index(right_click);
        field_row(ui, "Right click:", Slot::Above, 0.0, "", |ui| {
            egui::ComboBox::from_id_salt("terminal_right_click")
                .selected_text(right_click_label(current))
                .width(ui.available_width())
                .wrap_mode(egui::TextWrapMode::Truncate)
                .show_ui(ui, |ui| {
                    for action in RightClickAction::ALL {
                        if ui
                            .selectable_label(action == current, right_click_label(action))
                            .clicked()
                        {
                            right_click = action.index();
                            mouse_changed = true;
                        }
                    }
                });
        });
        mouse_changed |= choice_row_resp(
            ui, ChoiceKind::Checkbox, "When I select text, copy it to clipboard", "",
            &mut copy_on_select, true, UnderlineStyle::None,
        )
        .on_hover_text(COPY_ON_SELECT_NOTE)
        .clicked();
    });

    if mouse_changed {
        // 与粘贴两勾同一套：先改内存副本（本帧剩下的绘制按新值走），再落盘。
        win.right_click = right_click;
        win.copy_on_select = copy_on_select;
        save_prefs(|prefs| {
            prefs.right_click_action = right_click;
            prefs.copy_on_select = copy_on_select;
        });
    }

    ui.add_space(BLOCK_GAP);
    let mut changed = false;
    group_box(ui, Some("Paste Dialog"), |ui| {
        set_row_rhythm(ui);
        ui.label("When pasting multiple lines:");
        changed |= choice_row_resp(ui, ChoiceKind::Checkbox, "Show paste dialog on the main screen", "", &mut main, true, UnderlineStyle::None)
            .on_hover_text(TERMINAL_NOTE)
            .clicked();
        changed |= choice_row_resp(ui, ChoiceKind::Checkbox, "Show paste dialog on the alternative screen", "", &mut alt, true, UnderlineStyle::None)
            .on_hover_text(TERMINAL_NOTE)
            .clicked();
    });

    if changed {
        // 先改内存里的副本（同帧的粘贴门控读的就是它），再 load-modify-save。
        // 只写这两个位：语言 / 字号 / 标签行为 / 关闭确认都原样保留。
        win.paste_confirm_main = main;
        win.paste_confirm_alt = alt;
        save_prefs(|prefs| {
            prefs.paste_confirm_main = main;
            prefs.paste_confirm_alt = alt;
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_page_note_keeps_the_paste_granularity_and_the_local_mode_scope() {
        // 底色：粘贴弹窗不看行数，这句不能丢（否则用户以为单行仍直通）。
        assert!(TERMINAL_NOTE.contains("not only"), "要写明不是只对多行生效");
        assert!(TERMINAL_NOTE.contains("apply immediately"));
        // 接真之后，这一页没有灰行了——页底与行内说明都不许再说
        // 「没有开关可翻 / 没接线」，那会是反向的谎（用户会以为勾了没用）。
        for note in [TERMINAL_NOTE, LOCAL_MODE_NOTE] {
            assert!(!note.contains("Not wired"), "接真后不能说没接线：{note}");
            assert!(!note.contains("Protocol::Shell"), "旧理由已失效：{note}");
        }
        // 只读是**每条会话**的一位，不落盘——这句必须留着，否则用户会以为
        // 勾了以后每次启动都只读（那时终端看起来就是「坏了」）。
        assert!(LOCAL_MODE_NOTE.contains("Per session, not saved"));
        assert!(LOCAL_MODE_NOTE.contains("no auto-scroll"));
        // 右键已经接线：页面说明里不该再出现「缺枚举」那套旧理由。
        assert!(!TERMINAL_NOTE.contains("behaviour enum"));
    }

    #[test]
    fn the_three_right_click_labels_match_the_reference_wording() {
        // 第三档的文案就是它的判据，不能缩写。
        assert_eq!(right_click_label(RightClickAction::Menu), "Show menu");
        assert_eq!(right_click_label(RightClickAction::Paste), "Paste text");
        assert_eq!(
            right_click_label(RightClickAction::CopyOrPaste),
            "Copy text if selected, otherwise paste text"
        );
        // 文案与下标同序：设置页按下标取档，取错档就等于改错行为。
        for action in RightClickAction::ALL {
            assert_eq!(RightClickAction::from_index(action.index()), action);
        }
    }
}

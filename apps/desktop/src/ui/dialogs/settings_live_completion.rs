//! `Auto Completion` 页：十行**真控件**（I-AC）。
//!
//! # 与视觉基线的关系
//!
//! `08-terminal-auto-completion.png` 是这一页的视觉基线。出厂值与截图逐项一致：
//! 六个勾全空、`1 characters`、`7 rows`、`Nowhere`。以前整页灰显是因为没有
//! 引擎；`term-core::completion` 落地后，十个参数都有真实读者：
//!
//! - 六个勾分别开关词库三类 + 历史 + QuickBar 五个候选源；
//! - 触发字符 / 列表高度 / 预选策略进 [`term_core::completion::complete`]。
//!
//! 落盘仍是 `settings_live::save_prefs` 那一个入口，运行期副本挂在窗口上，
//! 键盘漏斗每帧读它——不读盘、不造第二份真相。

use crate::ui::kit::field::{choice_row_resp, ChoiceKind, UnderlineStyle};
use crate::ui::kit::spec::BLOCK_GAP;
use super::settings_live::save_prefs;
use super::settings_pages_view::{
    field_height, field_row, group_box, label_col, set_row_rhythm, Slot,
};
use crate::ui::*;
use term_core::completion::Preselect;

/// 触发字符数档位：与视觉基线的 `1 characters` 同格式。
const MIN_CHARS_MAX: usize = 9;
/// 列表高度的可用范围；参考出厂 7 行。
const LIST_HEIGHT_MAX: usize = 40;

/// `▲▼` 的单击 / Shift 单击：在 `1..=LIST_HEIGHT_MAX` 里循环。
///
/// 先归一化 `current`：它是从 prefs 反序列化来的，手改过的文件可能是 0，
/// 而 `0 - 1` 在 `usize` 上会下溢（debug 构建直接 panic，release 回绕成天文数字）。
/// 提成纯函数是为了让「0 起步也不炸」这条能被单测钉住。
fn cycle_list_height(current: usize, step: usize) -> usize {
    let base = current.clamp(1, LIST_HEIGHT_MAX);
    (base - 1 + step) % LIST_HEIGHT_MAX + 1
}

/// 「Preselect the best match」三档文案，与视觉基线下拉一致。
const PRESELECT_ITEMS: [(Preselect, &str); 3] = [
    (Preselect::Nowhere, "Nowhere"),
    (Preselect::First, "First"),
    (Preselect::Best, "Best"),
];

/// 画整页。控件变化时一次性同步窗口副本与磁盘。
pub(in crate::ui) fn auto_completion(win: &mut TerminalWindow, ui: &mut egui::Ui) {
    let mut s = win.completion.clone();
    let before = s.clone();
    // 标签列按本页最长的 `Preselect the best match:` 量，两个框的控件左缘因此对齐。
    let col = label_col(ui, &["Completion list height:", "Preselect the best match:"]);

    // 视觉基线 `08-terminal-auto-completion.png`：内容区第一个字是组标题
    // `Completion`，没有页级灰标题。
    set_row_rhythm(ui);

    group_box(ui, Some("Completion"), |ui| {
        choice_row_resp(ui, ChoiceKind::Checkbox, "Enable auto completion", "", &mut s.enabled, true, UnderlineStyle::None)
            .on_hover_text("Turn the whole completion popup on or off.");
        ui.label("Automatically complete commands with:");
        choice_row_resp(ui, ChoiceKind::Checkbox, "Command Name", "", &mut s.commands, true, UnderlineStyle::None)
            .on_hover_text("Common command names and their subcommands.");
        choice_row_resp(ui, ChoiceKind::Checkbox, "Command Option", "", &mut s.options, true, UnderlineStyle::None)
            .on_hover_text("Options such as --all or -l for the command being typed.");
        choice_row_resp(ui, ChoiceKind::Checkbox, "Command Parameter", "", &mut s.parameters, true, UnderlineStyle::None)
            .on_hover_text("Frequent parameter values of the command being typed.");
        choice_row_resp(ui, ChoiceKind::Checkbox, "History Commands", "", &mut s.history, true, UnderlineStyle::None)
            .on_hover_text("Whole lines from command history, ranked by frequency and recency.");
        choice_row_resp(ui, ChoiceKind::Checkbox, "Quick Commands", "", &mut s.quick, true, UnderlineStyle::None)
            .on_hover_text("Commands from the Quick Bar configuration.");
        field_row(
            ui,
            "Start completion after entering:",
            Slot::Above,
            0.0,
            "",
            |ui| {
                egui::ComboBox::from_id_salt("completion_min_chars")
                    .selected_text(format!("{} characters", s.min_chars))
                    .width(ui.available_width())
                    .wrap_mode(egui::TextWrapMode::Truncate)
                    .show_ui(ui, |ui| {
                        for n in 1..=MIN_CHARS_MAX {
                            let text = format!("{n} characters");
                            if ui.selectable_label(s.min_chars == n, text).clicked() {
                                s.min_chars = n;
                            }
                        }
                    });
            },
        );
    });
    ui.add_space(BLOCK_GAP);

    group_box(ui, Some("Appearance"), |ui| {
        // 行内标签 + 通栏字段框（框里 `7 rows` 靠左、▲▼ 贴框右内缘），
        // 与 `Command History` / `Caret` 那几页的数字框同一套画法。
        field_row(ui, "Completion list height:", Slot::Inline, col, "", |ui| {
            let button_w = 40.0;
            let field_w =
                (ui.available_width() - button_w - ui.spacing().item_spacing.x).max(1.0);
            let mut text = format!("{} rows", s.list_height);
            if ui
                .add(egui::TextEdit::singleline(&mut text).desired_width(field_w))
                .changed()
            {
                // 只认前导数字：框里带着 ` rows` 后缀（视觉基线同形），
                // 用户改完还是「7 rows」这一种写法。
                let digits: String = text.chars().take_while(|c| c.is_ascii_digit()).collect();
                if let Ok(n) = digits.parse::<usize>() {
                    s.list_height = n.clamp(1, LIST_HEIGHT_MAX);
                }
            }
            // ▲▼ 按钮：单击 +1，按住 Shift 单击 -1，到边界回绕。
            // 绝不画一个按下去没反应的箭头。
            // 高度取 `field_height`：写死 18 会让按钮比左边那个文本行矮一截，
            // 框里一高一矮两个盒子（实测）。
            if ui
                .add_sized(
                    egui::vec2(button_w, field_height(ui)),
                    egui::Button::new("▲▼"),
                )
                .on_hover_text("Click: +1 row. Shift-click: -1 row. Wraps at bounds.")
                .clicked()
            {
                let step = if ui.input(|i| i.modifiers.shift) {
                    LIST_HEIGHT_MAX - 1
                } else {
                    1
                };
                s.list_height = cycle_list_height(s.list_height, step);
            }
        });

        field_row(ui, "Preselect the best match:", Slot::Inline, col, "", |ui| {
            let selected = PRESELECT_ITEMS
                .iter()
                .position(|(p, _)| *p == s.preselect)
                .unwrap_or(0);
            let mut next = selected;
            egui::ComboBox::from_id_salt("completion_preselect")
                .selected_text(PRESELECT_ITEMS[selected].1)
                .width(ui.available_width())
                .wrap_mode(egui::TextWrapMode::Truncate)
                .show_ui(ui, |ui| {
                    for (i, (_, label)) in PRESELECT_ITEMS.iter().enumerate() {
                        if ui.selectable_label(i == next, *label).clicked() {
                            next = i;
                        }
                    }
                });
            s.preselect = PRESELECT_ITEMS[next].0;
        });
    });

    if s == before {
        return;
    }
    win.completion = s.clone();
    save_prefs(|prefs| prefs.completion = s);
}

#[cfg(test)]
mod tests {
    use super::{cycle_list_height, LIST_HEIGHT_MAX};
    use term_core::completion::Settings as CompletionSettings;
    use term_profile::prefs::Prefs;

    #[test]
    fn factory_prefs_match_the_reference_screenshot() {
        let c = Prefs::default().completion;
        // 参考参考页六项全勾选，默认值与之一比一。
        assert!(c.enabled && c.commands && c.options);
        assert!(c.parameters && c.history && c.quick);
        assert_eq!(c.min_chars, 1);
        assert_eq!(c.list_height, 7);
        assert_eq!(c.preselect, term_core::completion::Preselect::Nowhere);
    }

    #[test]
    fn round_trip_through_prefs_keeps_every_switch() {
        let mut p = Prefs::default();
        p.completion.enabled = true;
        p.completion.history = true;
        p.completion.list_height = 12;
        let json = serde_json::to_string(&p).expect("prefs 可序列化");
        let back: Prefs = serde_json::from_str(&json).expect("prefs 可反序列化");
        assert_eq!(back.completion, p.completion);
    }

    #[test]
    fn old_prefs_without_the_key_get_factory_defaults() {
        let back: Prefs = serde_json::from_str("{}").expect("空对象应可反序列化");
        assert_eq!(back.completion, CompletionSettings::default());
    }

    #[test]
    fn height_wraps_within_bounds() {
        let mut n = cycle_list_height(LIST_HEIGHT_MAX, 1);
        assert_eq!(n, 1, "到上限后再点回到 1，不会越界");
        n = cycle_list_height(n, LIST_HEIGHT_MAX - 1);
        assert_eq!(n, LIST_HEIGHT_MAX, "下限再 Shift 点击回到上限");
    }

    /// 手改过的 prefs 可能把 `list_height` 写成 0（或一个超大值）。
    /// `0 - 1` 在 usize 上下溢，所以入口必须先夹取；夹完要能正常循环。
    #[test]
    fn a_zero_or_oversized_height_still_cycles_safely() {
        // 0 按 1 处理：+1 得到 2，Shift（回绕）得到上限。
        assert_eq!(cycle_list_height(0, 1), 2, "0 应先夹到 1，再 +1");
        assert_eq!(cycle_list_height(0, LIST_HEIGHT_MAX - 1), LIST_HEIGHT_MAX);
        // 超大值先夹到上限：再 +1 回到 1。
        assert_eq!(cycle_list_height(usize::MAX, 1), 1, "超大值先夹到上限再 +1");
    }
}

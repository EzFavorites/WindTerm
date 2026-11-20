//! `Auto Completion` 页：十行**真控件**（I-AC）。
//!
//! # 与视觉基线的关系
//!
//! `视觉基线` 是这一页的视觉基线。出厂值与截图逐项一致：
//! 六个勾全空、`1 characters`、`7 rows`、`Nowhere`。以前整页灰显是因为没有
//! 引擎；`term-core::completion` 落地后，十个参数都有真实读者：
//!
//! - 六个勾分别开关词库三类 + 历史 + QuickBar 五个候选源；
//! - 触发字符 / 列表高度 / 预选策略进 [`term_core::completion::complete`]。
//!
//! 落盘仍是 `settings_live::save_prefs` 那一个入口，运行期副本挂在窗口上，
//! 键盘漏斗每帧读它——不读盘、不造第二份真相。

use super::settings_live::save_prefs;
use super::settings_pages_view::{field_box, field_label, group_box, label_col};
use crate::ui::*;
use term_core::completion::Preselect;

/// 触发字符数档位：与视觉基线的 `1 characters` 同格式。
const MIN_CHARS_MAX: usize = 9;
/// 列表高度的可用范围；参考出厂 7 行。
const LIST_HEIGHT_MAX: usize = 40;

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

    // 视觉基线 `视觉基线`：内容区第一个字是组标题
    // `Completion`，没有页级灰标题。
    ui.spacing_mut().item_spacing.y = 6.0;

    group_box(ui, Some("Completion"), |ui| {
        ui.checkbox(&mut s.enabled, "Enable auto completion")
            .on_hover_text("Turn the whole completion popup on or off.");
        ui.label("Automatically complete commands with:");
        ui.checkbox(&mut s.commands, "Command Name")
            .on_hover_text("Common command names and their subcommands.");
        ui.checkbox(&mut s.options, "Command Option")
            .on_hover_text("Options such as --all or -l for the command being typed.");
        ui.checkbox(&mut s.parameters, "Command Parameter")
            .on_hover_text("Frequent parameter values of the command being typed.");
        ui.checkbox(&mut s.history, "History Commands")
            .on_hover_text("Whole lines from command history, ranked by frequency and recency.");
        ui.checkbox(&mut s.quick, "Quick Commands")
            .on_hover_text("Commands from the Quick Bar configuration.");
        ui.label("Start completion after entering:");
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
    });
    ui.add_space(6.0);

    group_box(ui, Some("Appearance"), |ui| {
        // 行内标签 + 通栏字段框（框里 `7 rows` 靠左、▲▼ 贴框右内缘），
        // 与 `Command History` / `Caret` 那几页的数字框同一套画法。
        ui.horizontal(|ui| {
            field_label(ui, "Completion list height:", col);
            field_box(ui, |ui| {
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
                if ui
                    .add_sized(egui::vec2(button_w, 18.0), egui::Button::new("▲▼"))
                    .on_hover_text("Click: +1 row. Shift-click: -1 row. Wraps at bounds.")
                    .clicked()
                {
                    let step = if ui.input(|i| i.modifiers.shift) {
                        LIST_HEIGHT_MAX - 1
                    } else {
                        1
                    };
                    s.list_height = (s.list_height - 1 + step) % LIST_HEIGHT_MAX + 1;
                }
            });
        });

        ui.horizontal(|ui| {
            field_label(ui, "Preselect the best match:", col);
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
    use super::LIST_HEIGHT_MAX;
    use term_core::completion::Settings as CompletionSettings;
    use term_profile::prefs::Prefs;

    #[test]
    fn factory_prefs_match_the_reference_screenshot() {
        let c = Prefs::default().completion;
        assert!(!c.enabled && !c.commands && !c.options);
        assert!(!c.parameters && !c.history && !c.quick);
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
        let mut n = LIST_HEIGHT_MAX;
        n = (n - 1 + 1) % LIST_HEIGHT_MAX + 1;
        assert_eq!(n, 1, "到上限后再点回到 1，不会越界");
        n = (n - 1 + LIST_HEIGHT_MAX - 1) % LIST_HEIGHT_MAX + 1;
        assert_eq!(n, LIST_HEIGHT_MAX, "下限再 Shift 点击回到上限");
    }
}

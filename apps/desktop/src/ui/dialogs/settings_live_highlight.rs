//! `Preferences ▸ Highlight`：五行里**两行**有真实消费者，其余三行按  保持
//! 灰显空框 + 悬停说明。
//!
//! # 为什么前两行能接真
//!
//! `Highlight the caret line` 的完整链路在 那条「接线 10 条栅格
//! 可视化命令」时就建好了，只是设置页一直误判成灰行：
//!
//! - 字段：`Prefs.text_highlight_caret_line`（新标签的出厂态）；
//! - 命令：`Text.HighlightCaretLine` → `ShellAction::Marks(CaretLine)`（`parse.rs`）；
//! - 运行态：活动标签 `Tab.marks.caret_line`，菜单勾由 `marks_knob_on` 现读；
//! - 渲染：`terminal/marks.rs` 的 `caret_band_row` → `view.rs` 在字形底下铺淡底。
//!
//! 接线口径与 `settings_live_wrap` 完全一致：勾的是**活动标签**当前值
//! （`marks_knob_on`），改完 `set_marks` 当场生效 + 落 `prefs`（新标签默认）。
//!
//! `Highlight the incremental search`（接）：它的情况**正好相反**——
//! 效果本来就一直在画（`pane_overlays.rs` 的  那段，命中铺底色），
//! 缺的只是「关掉它」的入口。所以这一行不是「补渲染」，是**补字段 + 补门控**：
//! `Prefs.text_highlight_search`（出厂 `true`）→ `TerminalWindow.highlight_search`
//! → `paint_overlays(...)` 那个 `if`。出厂开 = 接线前的行为，一张基线都不动。
//!
//! 五行的**作用域不同**，别顺手统一：四行高亮落在 `Tab.marks`（每条标签各一份），
//! search 落在窗口上（改完对全部标签立刻生效）。兼容终端也确实一层是会话级、
//! 一层是全局级。
//!
//! # 后三行的渲染口径
//!
//! caret word 用 `selection::word_at` 在事实窗口上取整词，current pair 用
//! `selection::pair_start/pair_end` 取两端，current fold 用 OSC 133 命令块的
//! 起始行。三者都只在字形下铺淡色，不进选区、不影响复制。

use crate::ui::kit::field::{choice_row_resp, ChoiceKind, UnderlineStyle};
use super::settings_live::save_prefs;
use super::settings_pages_view::{group_box, set_row_rhythm};
use crate::ui::terminal::MarksParams;
use crate::ui::*;
use term_app::shell::MarksKnob;

/// 页底说明：五项都已接真，各自的数据源不同。
const HIGHLIGHT_NOTE: &str = "Caret line, word, fold and pair tints are per-tab; search matching is \
     window-wide. All five apply immediately and are saved for new tabs. \
     To highlight arbitrary text patterns, use Tools ▸ Triggers (Highlight Text rules).";

/// 查找命中那一行的悬停：它没有菜单项，说明里得交代「为什么只有设置页能改」。
const SEARCH_HOVER: &str = "Tints every search match in every tab; the current match uses the \
     foreground colour and the rest are gold. Applies at once and is saved.";

pub(in crate::ui) fn highlight(win: &mut TerminalWindow, ui: &mut egui::Ui) {
    let mut on = win.marks_knob_on(MarksKnob::CaretLine);
    let mut search = win.highlight_search;

    // 视觉基线里页首第一个字就是分组框标题 `Highlight`，不再另画页级标题。
    group_box(ui, Some("Highlight"), |ui| {
        set_row_rhythm(ui);
        // 第一行真勾选：读活动标签当前值，勾完当场铺底色 + 落 prefs。
        if choice_row_resp(ui, ChoiceKind::Checkbox, "Highlight the caret line", "", &mut on, true, UnderlineStyle::None)
            .on_hover_text("Tints the line under the cursor in the active tab; also the default for new tabs.")
            .clicked()
        {
            win.set_marks(|m| MarksParams { caret_line: on, ..m });
            save_prefs(|prefs| prefs.text_highlight_caret_line = on);
        }
        // 中间三行接真：三项都是「每条标签一份」的 MarksParams，
        // 改完当场在活动标签上看见，同时落 prefs 作为新标签默认值。
        let marks = win.active_marks();
        let mut word = marks.caret_word;
        let mut fold = marks.current_fold;
        let mut pair = marks.current_pair;
        let word_changed = choice_row_resp(ui, ChoiceKind::Checkbox, "Highlight the caret word", "", &mut word, true, UnderlineStyle::None)
            .on_hover_text("Tints every cell of the word under the cursor in the active tab.")
            .clicked();
        let fold_changed = choice_row_resp(ui, ChoiceKind::Checkbox, "Highlight the current fold", "", &mut fold, true, UnderlineStyle::None)
            .on_hover_text("Tints the command-block header containing the cursor (OSC 133 blocks).")
            .clicked();
        let pair_changed = choice_row_resp(ui, ChoiceKind::Checkbox, "Highlight the current pair", "", &mut pair, true, UnderlineStyle::None)
            .on_hover_text("Tints both ends of the bracket pair next to the cursor.")
            .clicked();
        if word_changed || fold_changed || pair_changed {
            // 三项一次写齐：逐个 set_marks 会用旧副本覆盖前一项。
            win.set_marks(|m| MarksParams {
                caret_word: word,
                current_fold: fold,
                current_pair: pair,
                ..m
            });
            save_prefs(|prefs| {
                prefs.text_highlight_caret_word = word;
                prefs.text_highlight_current_fold = fold;
                prefs.text_highlight_current_pair = pair;
            });
        }
        // 第四行真勾选 ：窗口级副本当帧生效，同时落 prefs。
        // 顺序照视觉基线，不因为「能点的要放前面」而重排——那是改设计稿。
        if choice_row_resp(ui, ChoiceKind::Checkbox, "Highlight the incremental search", "", &mut search, true, UnderlineStyle::None)
            .on_hover_text(SEARCH_HOVER)
            .clicked()
        {
            win.highlight_search = search;
            save_prefs(|prefs| prefs.text_highlight_search = search);
        }
        ui.add_space(4.0);
        ui.label(HIGHLIGHT_NOTE);
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_row_is_live_and_the_note_names_all_sources() {
        assert!(HIGHLIGHT_NOTE.contains("Caret line"));
        for source in ["word", "fold", "pair"] {
            assert!(HIGHLIGHT_NOTE.contains(source), "{source} 要在说明里点名");
        }
        assert!(!HIGHLIGHT_NOTE.contains("Not wired"), "不能再挂过期的灰显理由");
        assert!(HIGHLIGHT_NOTE.contains("Triggers"), "任意文本高亮规则在触发器里，给读者指条路");
    }

    #[test]
    fn the_search_row_is_no_longer_described_as_missing() {
        // ：这一行从「没有渲染器」变成真勾选。说明里若还留着
        // "incremental search" 作缺项，就是反着的谎，而且是**我们刚
        // 亲手确认过渲染早就在画**的那一项。
        assert!(
            !HIGHLIGHT_NOTE.contains("incremental search"),
            "已接线的行不能再列在灰显理由里"
        );
        assert!(
            HIGHLIGHT_NOTE.contains("search matching"),
            "要正面写出查找命中是真控件，读者才知道那行为什么能勾"
        );
        // 悬停文案要交代「在哪生效、是否落盘」——没有菜单项可查的人只能靠这句。
        assert!(SEARCH_HOVER.contains("every tab"));
        assert!(SEARCH_HOVER.contains("saved"));
    }
}

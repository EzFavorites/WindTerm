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
//! 两行的**作用域不同**，别顺手统一：caret line 落在 `Tab.marks`（每条标签各一份），
//! search 落在窗口上（改完对全部标签立刻生效）。兼容终端这两项也确实一层是会话级、
//! 一层是全局级。
//!
//! # 其余三行为什么仍灰
//!
//! caret word / current fold / current pair 都没有状态位，也没有渲染读者
//! （终端只画「常驻词表」与上面的 Find 命中，那是另外两件事）。
//! 摆成能点的勾就是假开关，所以画空框 + hover 理由。

use super::settings_live::save_prefs;
use super::settings_pages::Row;
use super::settings_pages_view::{draw_row, group_box, label_col, set_row_rhythm};
use crate::ui::terminal::MarksParams;
use crate::ui::*;
use term_app::shell::MarksKnob;

/// 三行灰显的共同理由：点名缺的是「编辑器侧的高亮状态」，
/// 不说「尚未实现」这种对下一个人没有信息量的话。
const HIGHLIGHT_NOTE: &str = "Not wired: the caret word, current fold and current pair rules have no \
     state to store and no renderer. Caret line and search matching are live (search matching is \
     the same pass that paints find hits).";

/// 查找命中那一行的悬停：它没有菜单项，说明里得交代「为什么只有设置页能改」。
const SEARCH_HOVER: &str = "Tints every search match in every tab; the current match uses the \
     foreground colour and the rest are gold. Applies at once and is saved.";

pub(in crate::ui) fn highlight(win: &mut TerminalWindow, ui: &mut egui::Ui) {
    let mut on = win.marks_knob_on(MarksKnob::CaretLine);
    let mut search = win.highlight_search;

    // 这一页只有勾选行、没有标签列（`col` 用兜底值）；视觉基线里页首第一个字
    // 就是分组框标题 `Highlight`，所以不再另画页级标题。
    let col = label_col(ui, &[]);
    group_box(ui, Some("Highlight"), |ui| {
        set_row_rhythm(ui);
        // 第一行真勾选：读活动标签当前值，勾完当场铺底色 + 落 prefs。
        if ui
            .checkbox(&mut on, "Highlight the caret line")
            .on_hover_text("Tints the line under the cursor in the active tab; also the default for new tabs.")
            .changed()
        {
            win.set_marks(|m| MarksParams { caret_line: on, ..m });
            save_prefs(|prefs| prefs.text_highlight_caret_line = on);
        }
        // 中间三行仍灰：空框 + 逐行悬停理由（它们各自没有读者）。
        for label in [
            "Highlight the caret word",
            "Highlight the current fold",
            "Highlight the current pair",
        ] {
            draw_row(ui, &Row::DisabledCheck(label), HIGHLIGHT_NOTE, col);
        }
        // 第四行真勾选 ：窗口级副本当帧生效，同时落 prefs。
        // 顺序照视觉基线，不因为「能点的要放前面」而重排——那是改设计稿。
        if ui
            .checkbox(&mut search, "Highlight the incremental search")
            .on_hover_text(SEARCH_HOVER)
            .changed()
        {
            win.highlight_search = search;
            save_prefs(|prefs| prefs.text_highlight_search = search);
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_the_caret_line_is_live_and_the_note_names_the_rest() {
        // 第一行有读者（Prefs + MarksKnob + 渲染器）；页底必须逐行点名
        // 其余三行缺的是什么，不能退化成「尚未实现」。
        assert!(HIGHLIGHT_NOTE.contains("Caret line"), "要点明第一行是真控件");
        for dead in ["caret word", "current fold", "current pair"] {
            assert!(HIGHLIGHT_NOTE.contains(dead), "{dead} 要在说明里点名");
        }
        assert!(
            !HIGHLIGHT_NOTE.contains("parses to no action"),
            "CaretLine 早已接线，不能再挂过期的灰显理由"
        );
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

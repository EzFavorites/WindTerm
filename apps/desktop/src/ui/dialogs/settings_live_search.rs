//! `Preferences ▸ Search`：真接线的一页——引擎下拉 + 可编辑的查询 URL 模板。
//!
//! # 这一页真在哪
//!
//! - 下拉里六个引擎来自 `term_runtime::search::SEARCH_ENGINES`（唯一一份数据，
//!   菜单里的 `Text ▸ Search Online` 也读它）；
//! - URL 行是真的 `TextEdit`，改动即落 `prefs.json` 的 `search_url`；
//! - 选中某个引擎＝把它那条模板填进 URL 行并落盘；`Text ▸ Search Online`
//!   下一次就按这条模板搜（`effects.rs::search_online`）。
//!
//! # 为什么每帧读一次 prefs
//!
//! 与 `CommandHistory` 同一做法：页面显示的必须是**生效值**（空串 → 出厂模板），
//! 而窗口上没有任何 `Prefs` 缓存可读。`prefs.json` 只有几百字节，读一次比在
//! `TerminalWindow` 上再养一份同名字段（第二个真相）便宜。
//!
//! # 为什么只在「下拉真的被改动」时才套模板
//!
//! 用户可能已经把 URL 行改成了自己的模板（带 `&hl=en`、换成公司内网搜索页）。
//! 如果每帧都按「当前引擎」回填，打开这一页就会把手写的那条冲掉——那是一次
//! 没人要求的写盘。所以只有点了**另一个**引擎才套用它的模板：点击是明确意图，
//! 「看了一眼下拉」不是。
//!
//! # 下半截为什么仍是灰的
//!
//! `+ / − / ▲ ▼` 是「自定义引擎列表」的编辑按钮，而这一轮只存**一条** URL
//! （`prefs.search_url`），没有存列表的字段；按下它们无处可写，所以灰显
//! 并在页底写清缺的是哪一段。视觉基线那个 `Modify` 按钮的功能被上面这条可编辑
//! URL 行取代，直接删掉——留一个点了没反应的按钮比少一个按钮更糟。

use super::settings_pages_view::{field_label, group_box, label_col};
use crate::ui::*;
use term_runtime::prefs;
use term_runtime::search::{self, SEARCH_ENGINES};

/// URL 行显示不下引擎表里的名字时（用户手写模板）的标签。
const CUSTOM_ENGINE: &str = "Custom";

/// 页底说明。必须点出「缺的是存储 + 列表编辑」，不能退化成「尚未实现」。
const SEARCH_NOTE: &str = "The query URL is stored and used by Text ▸ Search Online; {} is \
     replaced by the URL-encoded query text. The engine list itself is built in — adding, \
     removing or reordering engines would need a stored list, which does not exist yet.";

pub(in crate::ui) fn search(ui: &mut egui::Ui) {
    let mut prefs = prefs::load().unwrap_or_default();
    // 空串 → 出厂模板：输入框里显示的永远是一条能直接用的 URL。
    let mut url = prefs.search_url_template().to_string();
    let matched = search::engine_index_for(&url);
    let shown = matched.map(|i| SEARCH_ENGINES[i].name).unwrap_or(CUSTOM_ENGINE);

    ui.add_space(6.0);

    let mut picked: Option<usize> = None;
    let mut edited = false;
    let col = label_col(ui, &["Search engine:"]);
    // 视觉基线 `视觉基线` 顶端那行字是这一块的标题（不是页级灰标题）：
    // 它下面直接就是引擎表。这里保留同一行字，框内是简化版的一行控件。
    group_box(ui, Some("Search Engines"), |ui| {
        ui.horizontal(|ui| {
            field_label(ui, "Search engine:", col);
            // 组合框吃掉标签列之后的全部余量：视觉基线这一行是「标签 + 表头行」，
            // 右缘一路齐到框右边，不给自己留 230 的固定上限。
            egui::ComboBox::from_id_salt("prefs_search_engine")
                .selected_text(shown)
                .width(ui.available_width())
                // `width` 是下限；长 URL 模板名会按 Extend 模式把框撑开，
                // 进而顶宽固定尺寸的对话框、裁掉页脚的 `Close`。
                .wrap_mode(egui::TextWrapMode::Truncate)
                .show_ui(ui, |ui| {
                    for (i, engine) in SEARCH_ENGINES.iter().enumerate() {
                        if ui.selectable_label(matched == Some(i), engine.name).clicked() {
                            picked = Some(i);
                        }
                    }
                });
        });
        ui.add_space(4.0);
        // URL 行不跟标签挤一行：模板普遍 30 个字符以上（Stack Overflow 那条 36），
        // 140 标签之后只剩 ~190 像素，会被截掉一半——「显示的是哪条 URL」正是
        // 这一页最重要的信息。整行给它，换行不影响可读性。
        ui.label("Query URL:");
        ui.add_space(2.0);
        edited = ui
            .add(
                egui::TextEdit::singleline(&mut url)
                    .desired_width(ui.available_width())
                    .hint_text(search::default_url()),
            )
            .changed();
    });

    // 下拉被真的改动 → 套用它的模板（理由见模块文档）。
    if let Some(i) = picked {
        url = SEARCH_ENGINES[i].url.to_string();
        edited = true;
    }
    if edited {
        // load-modify-save：只动这一个字段，语言 / 历史 / 光标样式原样保留。
        // 清空输入框也算「改」——空串的语义是「回到出厂模板」，不是「不许搜」。
        prefs.search_url = url;
        prefs::save(&prefs);
    }

    ui.add_space(8.0);
    ui.horizontal(|ui| {
        for glyph in ["+", "−", "▲", "▼"] {
            ui.add_enabled(false, egui::Button::new(glyph).min_size(egui::vec2(20.0, 18.0)))
                .on_disabled_hover_text(SEARCH_NOTE);
        }
    });
    // 视觉基线没有页脚说明；自定义引擎列表的缺口挂在按钮 hover 上。
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_page_note_says_what_is_still_missing() {
        // 页底那句必须点出「列表没有存储」——只说「未接线」等于没说。
        assert!(SEARCH_NOTE.contains("stored list"), "要写清自定义列表缺存储");
        assert!(SEARCH_NOTE.contains("Search Online"), "要指出 URL 的真实消费者");
    }

    #[test]
    fn an_engine_template_is_recognized_but_a_hand_edit_is_custom() {
        // 下拉的选中态来自这条判定：认错就会显示一个不属于当前 URL 的引擎名。
        assert_eq!(search::engine_index_for(SEARCH_ENGINES[1].url), Some(1));
        assert_eq!(
            search::engine_index_for(&format!("{}&hl=en", SEARCH_ENGINES[1].url)),
            None
        );
    }
}

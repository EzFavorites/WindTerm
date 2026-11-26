//! `Preferences ▸ Search`：与参考逐项一致的两列引擎表 + Query URL + `Modify`。
//!
//! # 这一页真在哪
//!
//! - 表格六行来自 `term_runtime::prefs::SEARCH_ENGINES`（唯一一份数据，菜单里的
//!   `Text ▸ Search Online` 也读它），顺序 / 文案照参考：Google / Bing / Github /
//!   Stackoverflow / Wikipedia / DuckDuckGo；
//! - 点一行＝把它那条模板填进 Query URL 行；`Modify` 按钮把当前 URL 落盘；
//! - Query URL 行是真的 `TextEdit`，改动即落 `prefs.json` 的 `search_url`。
//!
//! # 为什么每帧读一次 prefs
//!
//! 页面显示的必须是**生效值**（空串 → 出厂模板），而窗口上没有 `Prefs` 缓存。
//! `prefs.json` 只有几百字节，读一次比再养一份同名字段便宜。
//!
//! # 为什么只在「真的选了引擎」时才套模板
//!
//! 用户可能已经把 URL 行改成自己的模板。每帧回填会把手写的那条冲掉——那是
//! 没人要求的写盘。所以只有点了**另一行**才套用它的模板。

use crate::ui::kit::spec::BLOCK_GAP;
use super::settings_pages_view::{field_row, group_box, label_col, Slot};
use crate::ui::*;
use term_runtime::prefs;
use term_runtime::search::{self, SEARCH_ENGINES};

/// URL 行显示不下引擎表里的名字时（用户手写模板）的标签。
const CUSTOM_ENGINE: &str = "Custom";

pub(in crate::ui) fn search(ui: &mut egui::Ui) {
    let mut prefs = prefs::load().unwrap_or_default();
    // 空串 → 出厂模板：输入框里显示的永远是一条能直接用的 URL。
    let mut url = prefs.search_url_template().to_string();
    let matched = search::engine_index_for(&url);
    let shown = matched.map(|i| SEARCH_ENGINES[i].name).unwrap_or(CUSTOM_ENGINE);

    ui.add_space(BLOCK_GAP);

    let mut picked: Option<usize> = None;
    let mut edited = false;
    let col = label_col(ui, &["Search engine:"]);

    // 参考 `Search Engines` 分组框：上面是两列表（Engine / Value），
    // 下面是 Query URL + Modify。这里把表做成真可选行，不是只读标签。
    group_box(ui, Some("Search Engines"), |ui| {
        egui::Frame::none()
            .stroke(egui::Stroke::new(
                1.0,
                egui::Color32::from_rgb(0x2E, 0x2E, 0x2E),
            ))
            .inner_margin(egui::Margin::same(4.0))
            .show(ui, |ui| {
                egui::Grid::new("prefs_search_engine_table")
                    .num_columns(2)
                    .spacing(egui::vec2(18.0, 4.0))
                    .show(ui, |ui| {
                        ui.label(egui::RichText::new("Engine").strong());
                        ui.label(egui::RichText::new("Value").strong());
                        ui.end_row();
                        for (i, engine) in SEARCH_ENGINES.iter().enumerate() {
                            if ui
                                .selectable_label(matched == Some(i), engine.name)
                                .clicked()
                            {
                                picked = Some(i);
                            }
                            ui.label(engine.url);
                            ui.end_row();
                        }
                    });
            });

        ui.add_space(6.0);
        field_row(ui, "Search engine:", Slot::Inline, col, "", |ui| {
            egui::ComboBox::from_id_salt("prefs_search_engine")
                .selected_text(shown)
                .width(ui.available_width())
                .wrap_mode(egui::TextWrapMode::Truncate)
                .show_ui(ui, |ui| {
                    for (i, engine) in SEARCH_ENGINES.iter().enumerate() {
                        if ui.selectable_label(matched == Some(i), engine.name).clicked() {
                            picked = Some(i);
                        }
                    }
                });
        });
        // URL 行整行给它：模板普遍 30 个字符以上，140 标签之后只剩很窄一列，
        // 「显示的是哪条 URL」正是这一页最重要的信息。
        field_row(ui, "Query URL:", Slot::Above, 0.0, "", |ui| {
            edited |= ui
                .add(
                    egui::TextEdit::singleline(&mut url)
                        .desired_width(ui.available_width())
                        .hint_text(search::default_url()),
                )
                .changed();
        });
        ui.add_space(4.0);
        if ui.button("Modify").clicked() {
            // 显式提交：把当前（可能是手写的）模板落盘。
            edited = true;
        }
    });

    // 选了一行 → 套用它的模板（理由见模块文档）。
    if let Some(i) = picked {
        url = SEARCH_ENGINES[i].url.to_string();
        edited = true;
    }
    if edited {
        // load-modify-save：只动这一个字段，语言 / 历史 / 光标样式原样保留。
        prefs.search_url = url;
        prefs::save(&prefs);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_engine_table_matches_the_reference_order_and_names() {
        let names: Vec<&str> = SEARCH_ENGINES.iter().map(|e| e.name).collect();
        assert_eq!(
            names,
            ["Google", "Bing", "Github", "Stackoverflow", "Wikipedia", "DuckDuckGo"]
        );
        assert_eq!(SEARCH_ENGINES.len(), 6);
    }

    #[test]
    fn an_engine_template_is_recognized_but_a_hand_edit_is_custom() {
        assert_eq!(search::engine_index_for(SEARCH_ENGINES[1].url), Some(1));
        assert_eq!(
            search::engine_index_for(&format!("{}&hl=en", SEARCH_ENGINES[1].url)),
            None
        );
    }
}

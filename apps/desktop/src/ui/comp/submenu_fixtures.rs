//! 32 张子菜单展开态里的 31 张（`session/03 Log` 仍由 `menu_log` 守）。
//!
//! 与顶层菜单夹具一样走生产同一份 `draw_menu_items_static`：父菜单高亮当前
//! 路径上的子菜单行，子菜单按参考图位置紧贴父菜单右侧、顶边对齐被高亮的行。
//! 路径可以是三级（`View ▸ Bars ▸ Toolbar`），每一级都画一层弹层。
//!
//! 路径表同时是组件名表：`--demo comp:<name>` 与 `CompId::name()` 都读它，
//! 不会出现「图有了但脚本叫不出名字」的死角。

use eframe::egui;

use crate::ui::{harness, shell, TerminalWindow};

/// `(组件后缀, 从顶级开始的标签路径)`。
pub(in crate::ui) const PATHS: &[(&str, &[&str])] = &[
    ("menu_submenu_edit_intellisense", &["Edit", "IntelliSense"]),
    ("menu_submenu_goto_word", &["Goto", "Word"]),
    ("menu_submenu_goto_page", &["Goto", "Page"]),
    ("menu_submenu_goto_paragraph", &["Goto", "Paragraph"]),
    ("menu_submenu_goto_command", &["Goto", "Command"]),
    ("menu_submenu_goto_fold", &["Goto", "Fold"]),
    ("menu_submenu_goto_scope", &["Goto", "Scope"]),
    ("menu_submenu_goto_indentation", &["Goto", "Indentation"]),
    ("menu_submenu_goto_pair", &["Goto", "Pair"]),
    ("menu_submenu_goto_line", &["Goto", "Line"]),
    ("menu_submenu_goto_document", &["Goto", "Document"]),
    ("menu_submenu_search_online", &["Search", "Search Online"]),
    ("menu_submenu_selection_select", &["Selection", "Select"]),
    ("menu_submenu_selection_select_to", &["Selection", "Select To"]),
    (
        "menu_submenu_selection_select_column_to",
        &["Selection", "Select Column To"],
    ),
    ("menu_submenu_session_open_recent", &["Session", "Open Recent"]),
    ("menu_submenu_session_log", &["Session", "Log"]),
    ("menu_submenu_tool_transfer_binary", &["Tool", "Transfer Binary"]),
    ("menu_submenu_view_hex_base", &["View", "Hex Base"]),
    ("menu_submenu_view_hex_columns", &["View", "Hex Columns"]),
    (
        "menu_submenu_view_hex_group_chars",
        &["View", "Hex Group Chars"],
    ),
    ("menu_submenu_view_bars", &["View", "Bars"]),
    ("menu_submenu_view_toolbar", &["View", "Bars", "Toolbar"]),
    ("menu_submenu_view_panes", &["View", "Panes"]),
    (
        "menu_submenu_view_syntax_schemes",
        &["View", "Syntax Schemes"],
    ),
    ("menu_submenu_view_symbols", &["View", "Symbols"]),
    ("menu_submenu_view_white_spaces", &["View", "White Spaces"]),
    ("menu_submenu_view_folds", &["View", "Folds"]),
    ("menu_submenu_view_zoom", &["View", "Zoom"]),
    (
        "menu_submenu_view_window_opacity",
        &["View", "Window Opacity"],
    ),
    ("menu_submenu_window_merge_to_group", &["Window", "Merge To Group"]),
];

pub(in crate::ui) fn name(idx: u8) -> &'static str {
    PATHS
        .get(idx as usize)
        .map(|(name, _)| *name)
        .unwrap_or(PATHS[0].0)
}

/// 取路径上第 `depth` 层的菜单项（顶级 = 0）。
fn items_at(root: &term_app::menu::Menu, path: &[&str], depth: usize) -> Vec<term_app::menu::Item> {
    use term_app::menu::Item;
    if depth == 0 {
        return root.items.clone();
    }
    let mut current = root.items.clone();
    for label in &path[1..=depth] {
        let next = current.into_iter().find_map(|item| match item {
            Item::Submenu { label: l, items } if l == *label => Some(items),
            _ => None,
        });
        current = match next {
            Some(items) => items,
            None => return Vec::new(),
        };
    }
    current
}

pub(in crate::ui) fn draw(win: &mut TerminalWindow, ctx: &egui::Context, idx: u8) {
    let Some((_, path)) = PATHS.get(idx as usize) else {
        return;
    };
    let root = term_app::menu::menus()
        .into_iter()
        .find(|m| m.label == path[0])
        .unwrap_or_else(|| panic!("顶级菜单 {} 不存在", path[0]));
    harness::show_canvas(ctx, |_ui| {});

    let mut left = 4.0_f32;
    let mut top = 26.0_f32;
    for depth in 0..path.len() {
        let items = items_at(&root, path, depth);
        let highlight = path.get(depth + 1).copied();
        let one = [highlight.unwrap_or("")];
        let highlighted: &[&str] = if highlight.is_some() { &one } else { &[] };
        let mut width = 0.0_f32;
        let mut row_top = None;
        egui::Area::new(egui::Id::new(format!("comp_submenu_{idx}_{depth}")))
            .fixed_pos(egui::pos2(left, top))
            .fade_in(false)
            .order(egui::Order::Foreground)
            .show(ctx, |ui| {
                width = shell::menu_popup_width(win, &items, ui);
                ui.set_width(width);
                egui::Frame::none()
                    .fill(shell::popup_bg())
                    .inner_margin(egui::Margin::symmetric(4.0, 6.0))
                    .rounding(4.0)
                    .show(ui, |ui| {
                        row_top = shell::draw_menu_items_static(win, ui, &items, highlighted)
                            .map(|r| r.top());
                    });
            });
        if depth + 1 == path.len() {
            break;
        }
        top = row_top.unwrap_or(top);
        left += width + 4.0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_submenu_component_has_a_unique_name() {
        let mut names = std::collections::BTreeSet::new();
        for (name, path) in PATHS {
            assert!(names.insert(*name), "子菜单组件名重复：{name}");
            assert!(path.len() >= 2, "{name} 的路径至少要两段");
        }
    }
}

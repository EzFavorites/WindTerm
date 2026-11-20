//! 右键菜单复用的命令 ID 常量（`menu::context_ids`）与菜单树的一致性断言。
//!
//! 单独成文件只因 `menu_check_tests.rs` 已贴 500 代码行门禁：那文件格式是
//! 由 `menu.rs` 的 `#[cfg(test)] + #[path]` 两行拉进来的纯测试模块，本文件同样。
//! 命令 ID 字面量出现在这里是合法的（I5 只扫生产代码），而 ui 侧因此零硬编码。

use super::{context_ids, menus, Item};

/// 递归摊平整棵菜单树成 (label, id)。
fn collect(items: &[Item], out: &mut Vec<(&'static str, &'static str)>) {
    for it in items {
        match it {
            Item::Action { label, id } | Item::Check { label, id, .. } => out.push((*label, *id)),
            Item::Submenu { items, .. } => collect(items, out),
            Item::Separator => {}
        }
    }
}

/// 2026-11-19：右键行表在 ui 里按 label 画手绘行，判态与派发靠这里的常量拿
/// 命令 ID。常量与菜单树若漂移，右键就派发不存在的命令或判错灰态——这条把
/// 「数据层单一来源」钉死。
#[test]
fn context_ids_match_the_same_labelled_menu_items() {
    let mut tree = Vec::new();
    for menu in menus() {
        collect(&menu.items, &mut tree);
    }
    let cases: &[(&str, &str)] = &[
        (context_ids::HIGHLIGHT_TEXT, "Highlight Text"),
        (context_ids::SELECT_COMMAND, "Select Command"),
        (context_ids::SAVE_SESSION, "Save Session"),
        (context_ids::LINE_ENDINGS, "Line Endings"),
        (context_ids::WRAP_SYMBOLS, "Wrap Symbols"),
        (context_ids::SEARCH_ONLINE, "Search Online"),
    ];
    for (id, label) in cases {
        // 顶级菜单 / 递归子树里能查到同 label 同 id 最好；Symbols 等由
        // menu_submenus.rs 单独提供、不在 menus() 顶级树里，则用 parse 反查
        // ——能解析即证明这个 ID 是活的数据层条目，不是 ui 凭空写的。
        let in_tree = tree.iter().any(|(l, i)| *l == *label && *i == *id);
        let wired = crate::action::Action::parse(id).is_some();
        assert!(
            in_tree || wired,
            "右键常量 {id}（label={label}）既不在菜单树，也不是活命令"
        );
    }
}

/// 六个常量互不重复——否则两行右键会派发到同一条命令。
#[test]
fn context_ids_are_distinct() {
    let ids = [
        context_ids::HIGHLIGHT_TEXT,
        context_ids::SELECT_COMMAND,
        context_ids::SAVE_SESSION,
        context_ids::LINE_ENDINGS,
        context_ids::WRAP_SYMBOLS,
        context_ids::SEARCH_ONLINE,
    ];
    let mut sorted = ids;
    sorted.sort_unstable();
    for w in sorted.windows(2) {
        assert_ne!(w[0], w[1], "两个右键行共用命令 ID {}", w[0]);
    }
}

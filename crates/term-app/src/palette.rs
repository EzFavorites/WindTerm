//! R14-c：命令面板能力——「所有可执行项 + 搜索 + 选中」的纯函数注册表。
//!
//! 设计要点（B4 / I5 / R5）：
//! - **注册表不另建一份清单**：条目递归取自 `menu::menus()`，再并上
//!   `tab::SHADOW_ENTRIES`（W2-b：标签右键 10 个真实入口虽不在菜单栏里，
//!   也应能被搜到、被执行）；是否可执行仍由 `capability::is_implemented` 判定。
//!   命令 ID 只活在菜单数据与各能力的 `parse`，这里一个字面量都不存——
//!   任何一边加一条命令，面板自动多一条，台账也自动对齐。
//! - **状态只有四件事**：开没开、哪种范围、搜什么、选中第几个。过滤结果、
//!   匹配数、当前条目全是 `(mode, query)` 的纯函数值，不缓存、不会过期（F2 教训）。
//! - **执行不经过字符串派发的旁路**：`Confirm` 在顶层 `reduce` 里被翻译成
//!   「选中命令的 `Action`」，再走一次同一条纯函数 reducer（见 `action.rs`）。
//!   投影层只负责把键鼠事件派发进来，不碰领域状态（R4）。
//!
//! 两种范围有真实差别，不是两个同义开关：
//! - `Command`：只列已接线命令，选中即可执行；
//! - `Anything`：列**全部**菜单项，未接线的也展示（标成不可执行），
//!   让用户一屏看清「有什么 / 还缺什么」——这正是 `docs/design/12-capability-surface-and-menus.md`
//!   里说「114 项菜单不必靠翻菜单」的那件事。

use crate::capability;
use crate::menu::{self, Item};

pub use crate::vocab::PaletteMode;

/// 面板状态：领域状态，只由 `reduce` 写。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PaletteState {
    pub visible: bool,
    pub mode: PaletteMode,
    pub query: String,
    pub selected: usize,
}

impl Default for PaletteState {
    fn default() -> Self {
        Self {
            visible: false,
            mode: PaletteMode::Command,
            query: String::new(),
            selected: 0,
        }
    }
}

pub use crate::vocab::PaletteAction;

// 留在这里会让 `palette → capability → action → palette` 成环。

/// 一条面板条目。全是菜单树里的 `&'static str`，不拥有任何数据。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Entry {
    pub label: &'static str,
    pub id: &'static str,
    /// 顶级菜单名：既用于显示分组，也参与搜索。
    pub menu: &'static str,
    pub implemented: bool,
}

fn collect(items: &[Item], menu: &'static str, out: &mut Vec<Entry>) {
    for item in items {
        match item {
            // 可勾选行也是命令（`Item::Check`）：搜索里照样能搜到，
            // `implemented` 照旧由能力台账决定，勾选态与本结构无关。
            Item::Action { label, id } | Item::Check { label, id, .. } => out.push(Entry {
                label,
                id,
                menu,
                implemented: capability::is_implemented(id),
            }),
            Item::Separator => {}
            Item::Submenu { items, .. } => collect(items, menu, out),
        }
    }
}

/// 按菜单顺序收集条目；`Command` 只留可执行的。
pub fn entries(mode: PaletteMode) -> Vec<Entry> {
    let mut out = Vec::new();
    for menu in menu::menus() {
        collect(&menu.items, menu.label, &mut out);
    }
    // W2-b：影子入口不画在菜单栏里，但它们是真实可点命令。
    // 分组名单独叫 `Tab`，面板显示时才知道这一组来自标签右键。
    for entry in crate::tab::SHADOW_ENTRIES {
        out.push(Entry {
            label: entry.label,
            id: entry.id,
            menu: "Tab",
            implemented: capability::is_implemented(entry.id),
        });
    }
    if mode == PaletteMode::Command {
        // `CONTEXT_ONLY` 里的命令 ID 路径是构造上的空转（缺 target 就没有意义），
        // 列出来只会让用户点了没反应。它们仍在 `Anything` 模式里可见。
        out.retain(|e| e.implemented && !crate::tab::CONTEXT_ONLY.contains(&e.id));
    }
    out
}

/// 过滤结果：大小写不敏感的子串，命中显示名 / 菜单名 / 命令 ID 任一即可。
///
/// 这是纯函数值，不是状态：搜索框每改一个字就重算一次，窗口换了也不会过期。
pub fn filtered(state: &PaletteState) -> Vec<Entry> {
    let q = state.query.trim().to_lowercase();
    entries(state.mode)
        .into_iter()
        .filter(|e| {
            q.is_empty()
                || e.label.to_lowercase().contains(&q)
                || e.menu.to_lowercase().contains(&q)
                || e.id.to_lowercase().contains(&q)
        })
        .collect()
}

/// 当前选中的条目（对过滤结果取模，结果集变短也不会越界）。
pub fn selected_entry(state: &PaletteState) -> Option<Entry> {
    if !state.visible {
        return None;
    }
    let list = filtered(state);
    if list.is_empty() {
        return None;
    }
    Some(list[state.selected % list.len()])
}

/// `Confirm` 时要执行的命令 ID；选中占位项时为 `None`（点了不会假装成功）。
pub fn confirm_id(state: &PaletteState) -> Option<&'static str> {
    selected_entry(state)
        .filter(|e| e.implemented)
        .map(|e| e.id)
}

/// 纯函数：只改 `PaletteState`。执行本身由顶层 `reduce` 递归派发。
/// 命令 ID → 本能力的动作。未接线的 ID 返回 `None`。
///
/// 2026-11-12 R2：实现只有一份，在 [`crate::parse`]（入口层）。这里保留同名入口，
/// 一是因为「每个能力自带字符串入口」是这个 crate 对外的约定，二是因为
/// `apps/desktop` 有直接调 `term_app::log::parse` 的地方——把路径改掉没有架构价值。
pub fn parse(id: &str) -> Option<PaletteAction> {
    crate::parse::palette_parse(id)
}

pub fn reduce(state: &mut PaletteState, action: PaletteAction) {
    match action {
        PaletteAction::Open(mode) => {
            state.visible = true;
            state.mode = mode;
            state.query.clear();
            state.selected = 0;
        }
        PaletteAction::SetQuery(q) => {
            state.query = q;
            // 换了查询回到第一条，否则「第 9 个」会残留到新结果集上。
            state.selected = 0;
        }
        PaletteAction::Move(delta) => {
            let n = filtered(state).len();
            state.selected = if n == 0 {
                0
            } else {
                ((state.selected % n) as i64 + delta as i64).rem_euclid(n as i64) as usize
            };
        }
        PaletteAction::Select(idx) => {
            // 结果集可能比点击时更短（查询刚改过），钳一下就不会指到空处。
            let n = filtered(state).len();
            state.selected = idx.min(n.saturating_sub(1));
        }
        PaletteAction::Confirm | PaletteAction::Close => {
            state.visible = false;
            state.query.clear();
            state.selected = 0;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn open(mode: PaletteMode) -> PaletteState {
        let mut s = PaletteState::default();
        reduce(&mut s, PaletteAction::Open(mode));
        s
    }

    #[test]
    fn parse_opens_the_two_ranges() {
        assert_eq!(
            parse("Window.ShowPaletteCommand"),
            Some(PaletteAction::Open(PaletteMode::Command))
        );
        assert_eq!(
            parse("Window.ShowPaletteAnything"),
            Some(PaletteAction::Open(PaletteMode::Anything))
        );
        assert_eq!(parse("Window.GotoLine"), None);
    }

    #[test]
    fn registry_comes_from_the_menu_tree_without_a_second_list() {
        // 两种范围的条目数之差，正好等于当前占位命令数。
        //
        // 注意 `Command` 范围是「已接线 **且** 非上下文命令」：`CONTEXT_ONLY` 里的
        // 命令虽然已接线（能力台账算它一条），但 ID 路径是构造上的自赋值，
        // 列进可执行列表只会让用户点了没反应。所以这里比的是
        // 「已接线数 − 已接线的上下文命令数」，而不是全部已接线数。
        let all = entries(PaletteMode::Anything);
        let wired = entries(PaletteMode::Command);
        let context_only_wired = crate::tab::CONTEXT_ONLY
            .iter()
            .filter(|id| capability::is_implemented(id))
            .count();
        assert_eq!(all.len(), capability::inventory().len());
        assert_eq!(wired.len() + context_only_wired, capability::counts().0);
        assert!(all.iter().any(|e| !e.implemented));
        assert!(wired.iter().all(|e| e.implemented));
        // 顶级菜单名随条目一起带来，面板才有分组可显示。
        assert!(wired.iter().any(|e| e.menu == "Session"));
    }

    #[test]
    fn anything_shows_planned_items_but_they_are_not_executable() {
        let mut s = open(PaletteMode::Anything);
        // 锁定一条确定未接线的项：Goto Symbol 仍是占位（R14-d 只接了 Line / Session）。
        let idx = entries(PaletteMode::Anything)
            .iter()
            .position(|e| e.id == "Window.GotoSymbol")
            .expect("菜单里应有该占位项");
        s.selected = idx;
        let e = selected_entry(&s).unwrap();
        assert!(!e.implemented);
        assert_eq!(confirm_id(&s), None, "占位项不许被当成执行成功");
    }

    #[test]
    fn filtering_is_case_insensitive_and_covers_label_menu_and_id() {
        let mut s = open(PaletteMode::Command);
        reduce(&mut s, PaletteAction::SetQuery("ZOOM".into()));
        let f = filtered(&s);
        assert!(!f.is_empty());
        assert!(f.iter().all(|e| {
            e.label.to_lowercase().contains("zoom")
                || e.menu.to_lowercase().contains("zoom")
                || e.id.to_lowercase().contains("zoom")
        }));
        // 命令 ID 也能搜到：用户未必记得菜单名。
        reduce(&mut s, PaletteAction::SetQuery("CopySessionName".into()));
        assert_eq!(filtered(&s).len(), 1);
    }

    #[test]
    fn changing_the_query_resets_the_selection() {
        let mut s = open(PaletteMode::Command);
        reduce(&mut s, PaletteAction::Move(7));
        assert_eq!(s.selected, 7);
        reduce(&mut s, PaletteAction::SetQuery("Find".into()));
        assert_eq!(s.selected, 0);
    }

    #[test]
    fn moving_wraps_in_both_directions() {
        let mut s = open(PaletteMode::Command);
        let n = filtered(&s).len();
        reduce(&mut s, PaletteAction::Move(-1));
        assert_eq!(s.selected, n - 1);
        reduce(&mut s, PaletteAction::Move(1));
        assert_eq!(s.selected, 0);
    }

    #[test]
    fn move_on_empty_result_set_cannot_overflow() {
        let mut s = open(PaletteMode::Command);
        reduce(&mut s, PaletteAction::SetQuery("这个字符串不会命中任何命令".into()));
        assert!(filtered(&s).is_empty());
        reduce(&mut s, PaletteAction::Move(3));
        assert_eq!(s.selected, 0);
        assert_eq!(selected_entry(&s), None);
    }

    #[test]
    fn confirm_closes_and_clears_the_palette() {
        let mut s = open(PaletteMode::Command);
        reduce(&mut s, PaletteAction::SetQuery("Zoom In".into()));
        reduce(&mut s, PaletteAction::Confirm);
        assert!(!s.visible);
        assert!(s.query.is_empty());
        assert_eq!(s.selected, 0);
        assert_eq!(selected_entry(&s), None, "隐藏的面板不该再报选中项");
    }

    #[test]
    fn default_state_is_closed() {
        let s = PaletteState::default();
        assert!(!s.visible);
        assert_eq!(s.mode, PaletteMode::Command);
    }

    #[test]
    fn mouse_selection_is_clamped_to_the_current_result_set() {
        let mut s = open(PaletteMode::Command);
        reduce(&mut s, PaletteAction::Select(999));
        assert_eq!(s.selected, filtered(&s).len() - 1);
        reduce(
            &mut s,
            PaletteAction::SetQuery("绝不会命中的查询串".into()),
        );
        reduce(&mut s, PaletteAction::Select(3));
        assert_eq!(s.selected, 0);
    }
}

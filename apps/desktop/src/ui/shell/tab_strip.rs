//! 标签栏与快捷按钮：会话的唯一入口，以及它们的点击动作。
//!
//! 从 `ui/mod.rs` 拆出（2026-11-05）。**只搬代码，不改行为**——
//! 五张视觉基线逐字节一致是这次拆分唯一的判据。
//!
//! 快捷按钮的画法搬到了 [`super::tool_bar`]（视觉基线里那一行从「8 个
//! emoji 按钮」变成了「抓手 + 三颗几何图标 + 工具栏面板」），原来的
//! `draw_tools` 因此删除。2026-11-08 标签条的绘制也搬到了 [`super::tab_bar`]
//! （无框标签 + 顶部蓝线 + 右端三件）。这个文件仍持有**按钮的定义**
//! （`ToolButton` / `ToolAction`）、**标签的关闭语义**（`close_tab` 系列）与
//! **动作的落地**（`handle_tool_action`）——按钮长什么样可以改，
//! 点下去发生什么必须只有一处。

use crate::ui::*;
// 关标签 / 关窗格要按「窗格身份」操作，而不是会话身份。
use crate::ui::layout::{remove_from_group, GroupRemoval, PaneGroup};
use crate::ui::layout::mint_pane_identity;
use term_core::session_runtime::SessionId;

/// A tool button.
pub(in crate::ui) struct ToolButton {
    pub(in crate::ui) icon: &'static str,
    pub(in crate::ui) tooltip: &'static str,
    pub(in crate::ui) action: ToolAction,
}

/// What a tool button does.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::ui) enum ToolAction {
    NewSession,
    Connect,
    Disconnect,
    Refresh,
    Settings,
    Terminal,
    Sftp,
    Serial,
}

impl TerminalWindow {
    /// 关掉一条标签，并维护 `active_tab` 与「至少留一条可用 shell」的约定。
    ///
    /// 关完之后激活哪一条由 `Tabs ▸ Activate the tab after closing:` 决定
    /// （`prefs.tab_activate_after_close`，0 = 本文件接线之前的「夹到末位」）。
    /// 判定全部收在 [`tab_after_close`] 这个纯函数里：三条关闭路径（标签 ×、
    /// 菜单 Close、确认弹窗 Yes）走的是这一个地方，规则只许有一份。
    pub(in crate::ui) fn close_tab(&mut self, index: usize) {
        if index >= self.tabs.len() {
            return;
        }
        let active_before = self.active_slot();
        let closed_id = self.tabs[index].id;
        let closed_identity = self.tabs[index].identity;
        let mut closed = self.tabs.remove(index);
        term_app::sync_input::leave_all(&mut self.view, closed_identity);
        closed.close(); // 先摘 tee 再关会话；Tab::close 已定义好这条顺序
        // ADR-0025：摘掉一条标签后，恒在身份顺序要立刻跟着重算。
        self.sync_open_order();
        // 按「组里还有没有会话」决定剪不剪叶子。
        // 一个 `SessionId` 只属于一个 group（ADR-0026），按会话反查窗格精确处理：
        // · 组里还有别的 tab → 只更新该组 order/active，**不剪叶子**（pane 保留）；
        // · 组已空           → 剪掉这片窗格并摘掉 group（与旧行为一致）。
        // 当前每组只有一条会话，所以现有生命周期断言走的全是第二条分支，行为不变。
        let mut kept_in_pane: Option<SessionId> = None;
        if let Some(pane) = self.pane_of_session(closed_identity) {
            let snapshot = self.pane_groups.get(&pane).cloned();
            if let Some(g) = snapshot {
                let (order, outcome) = remove_from_group(&g.order, g.active, closed_identity);
                match outcome {
                    GroupRemoval::Remaining { next_active } => {
                        if let Some(live) = self.pane_groups.get_mut(&pane) {
                            live.order = order;
                            live.active = next_active;
                        }
                        kept_in_pane = Some(next_active);
                    }
                    GroupRemoval::Empty => {
                        self.layout.close_leaf(pane);
                        self.pane_groups.remove(&pane);
                    }
                }
            }
        }
        // 标签颜色同样按下标存：少一条就要跟着挪，不然关掉中间那条之后，
        // 后面每条标签都会显示**前一条**的颜色。
        super::tab_menu::drop_color_at(&mut self.tab_colors, index);
        // 激活历史存的是 id，但被关的那条要摘掉：不摘的话「上一次激活」
        // 会指回一个已经不存在（或已经被别人占住）的槽位。
        self.tab_focus.retain(|id| *id != closed_id);
        if let Some(next_active) = kept_in_pane {
            // 组内关闭：焦点留在**同一片 pane** 的下一条，不跳到别的窗格，
            // 也不吃 `tab_after_close` / 激活历史那套全局标签策略。
            self.active = Some(next_active);
        } else {
            self.set_active_slot(tab_after_close(
                self.tab_activate_after_close,
                index,
                active_before,
                self.tabs.len(),
            ));
            // 「上一次激活」档：历史里有还开着的标签就用它，没有则沿用
            // `tab_after_close` 给的退路（前一条）。只有关的是当前标签时才需要问
            // 历史——关背景标签时用户还在看的那一条不该跳走。
            if self.tab_activate_after_close == 2 && active_before == index {
                let open: Vec<egui::Id> = self.tabs.iter().map(|t| t.id).collect();
                if let Some(slot) = last_active_slot(&self.tab_focus, &open) {
                    self.set_active_slot(slot);
                }
            }
        }
        self.ensure_tab();
        // 单叶子坍缩场景：`close_leaf` 在「整树只有一个叶子」时不会删叶子（窗口
        // 始终要留一块终端），于是叶子仍指着刚关掉的死身份对应的窗格。ensure_tab
        // 已保证至少一条存活标签，把那片唯一窗格指回存活会话，恢复「叶子身份唯一且
        // 有效」不变量（窗格身份不变，只换它持有的会话）。
        if let Some(live) = self.tabs.first().map(|t| t.identity) {
            if self.layout.is_single() {
                if let Layout::Leaf(p) = &self.layout {
                    let p = *p;
                    if let Some(g) = self.pane_groups.get_mut(&p) {
                        g.order = vec![live];
                        g.active = live;
                    } else {
                        self.pane_groups
                            .insert(p, PaneGroup { order: vec![live], active: live });
                    }
                }
            }
        }
    }

    /// 每帧一次：把当前激活的标签 id 记进历史（最近激活的在末尾）。
    ///
    /// 走统一钩子而不是给十几处 `active_tab = …` 各加一行——与
    /// `poll_activation_connect` 同一条经验：漏一处就是「从某种方式切过去的
    /// 标签不算上一次激活」，而这种 bug 只在特定顺序下才现形。
    pub(in crate::ui) fn poll_tab_focus(&mut self) {
        let Some(id) = self.tabs.get(self.active_slot()).map(|t| t.id) else {
            return;
        };
        if self.tab_focus.last() == Some(&id) {
            return;
        }
        // 同一条标签不重复入栈：A→B→A 之后历史是 [B, A]，不是 [A, B, A]。
        self.tab_focus.retain(|old| *old != id);
        self.tab_focus.push(id);
        // 只回答「上一次」这件事，留 16 条足够，也不会让它无限长。
        if self.tab_focus.len() > 16 {
            self.tab_focus.remove(0);
        }
    }

    /// 摘掉全部标签并关掉它们的会话；**不**补新标签。
    ///
    /// 与 [`Self::close_all_tabs`] 的唯一区别就是末尾那句 `ensure_tab`。退出路径
    /// （`main.rs::on_exit`）走这一条：`ensure_tab` 会现起一条新的本地 shell，
    /// 在退出时等于「刚 fork 一个进程就为它送终」——多一次 PTY 分配、多一个监管者
    /// 线程，还要多走一遍 `attach_tap` 的 ack。
    /// `pub(crate)`：唯一的调用点是宿主 `main.rs::on_exit`（`ui` 之外），
    /// 与 `construct.rs` 里那几个宿主接口同一档可见性，但没必要更宽。
    pub(crate) fn close_all_sessions(&mut self) {
        for mut tab in self.tabs.drain(..) {
            tab.close();
        }
        // ADR-0025：标签全清后，恒在身份顺序置空。
        self.sync_open_order();
        // 关掉全部标签后布局回到单窗格，不能留着指向旧下标的树。用新窗格身份、
        // 清空 group 映射（退出路径不渲染，不给空 group 留悬空条目）。
        self.layout = Layout::single(mint_pane_identity());
        self.pane_groups.clear();
        // 颜色表也跟着清空：留下 12 条旧颜色，下一条新标签会凭空继承一个色。
        self.tab_colors.clear();
        self.set_active_slot(0);
    }

    /// 关掉全部标签（菜单「Close All Views」）。
    ///
    /// 「关掉视图」在界面上要留一个能继续敲的窗格，所以关空之后补一条本地 shell；
    /// 退出路径不需要这个补位，见 [`Self::close_all_sessions`]。
    pub(in crate::ui) fn close_all_tabs(&mut self) {
        for id in self.tabs.iter().map(|t| t.identity).collect::<Vec<_>>() {
            term_app::sync_input::leave_all(&mut self.view, id);
        }
        self.close_all_sessions();
        self.ensure_tab();
    }

    /// 窗口里始终留一条能用的本地 shell——标签栏、菜单关闭都遵守同一约定。
    fn ensure_tab(&mut self) {
        if self.tabs.is_empty() {
            // 新标签是**新会话**，光标样式取 Caret 页的偏好（不是 `SessionConfig::new` 的写死 0）。
            let shell_cfg = self.fresh_session(Protocol::Shell);
            self.add_tab(make_tab(
                &self.pool,
                shell_cfg,
                egui::Id::new("tab_fallback"),
                &self.ctx,
            ));
            self.set_active_slot(0);
        }
    }

    pub(in crate::ui) fn handle_tool_action(&mut self, action: ToolAction) {
        match action {
            ToolAction::NewSession => {
                self.open_new_session(None);
            }
            ToolAction::Connect => {
                self.connect_selected_session();
            }
            ToolAction::Disconnect => {
                self.disconnect_active_tab();
            }
            ToolAction::Terminal => {
                if let Some(tab) = self.tabs.get(self.active_slot()) {
                    tab.clear();
                }
            }
            ToolAction::Settings => {
                self.view.show_settings = true;
            }
            _ => {}
        }
    }
}

/// 关掉 `closed` 之后该激活哪一格。
///
/// # 三档只在「关的就是当前标签」时有分歧
///
/// 关一条背景标签时，用户正在看的那条不该跳走：它只是可能因为左侧少了一条
/// 而把下标左移一位。三档各自只在「关的正是当前标签」时说话——这也是为什么
/// 这个判定不能写成「关完再说」：那时候已经分不清关的是谁了。
///
/// `kind` 是 `Prefs.tab_activate_after_close` 的生效值：
/// 0 = 下一个（接线前的行为）/ 1 = 上一个 / 2 = 上一次激活（退路 = 上一个）。
/// 越界值按 0 处理——与 `Prefs::tab_activate_after_close_kind` 同一条兜底。
pub(in crate::ui) fn tab_after_close(
    kind: usize,
    closed: usize,
    active: usize,
    remaining: usize,
) -> usize {
    if remaining == 0 {
        return 0;
    }
    if closed != active {
        let shift = if active > closed { 1 } else { 0 };
        return active.saturating_sub(shift).min(remaining - 1);
    }
    match kind {
        1 => closed.saturating_sub(1),
        // 2 的**真正**落点由 `last_active_slot` 从激活历史里给；走到这里说明
        // 历史里没有还开着的标签（刚启动 / 恢复出来还没切过），退到前一条。
        2 => closed.saturating_sub(1),
        // 0 / 越界 = 接线前的行为：原地下标（就是「下一个」），到底了夹到末位。
        _ => closed.min(remaining - 1),
    }
}

/// 从激活历史（末尾最近）里找第一条还开着的标签，返回它此刻的下标。
///
/// 参数刻意是两份 id 列表而不是 `&[Tab]`：标签 id 在删除 / 复制 / 重排之后
/// 仍然指向同一条会话，下标会错位，而纯函数能在单测里用 `egui::Id::new("a")`
/// 直接钉住这些情形。
pub(in crate::ui) fn last_active_slot(history: &[egui::Id], open: &[egui::Id]) -> Option<usize> {
    history
        .iter()
        .rev()
        .find_map(|id| open.iter().position(|other| other == id))
}

#[cfg(test)]
mod tests {
    use super::*;

    // 三档的编号与 `Prefs` 的字段一一对应：0 = 下一个 / 1 = 上一个 / 2 = 上次激活。
    const NEXT: usize = 0;
    const PREV: usize = 1;
    const LAST: usize = 2;

    #[test]
    fn closing_the_active_middle_tab_lands_on_the_next_one() {
        // 5 条里关掉第 2 条（当前）：第 3 条左移到 2，也就是「下一个」。
        assert_eq!(tab_after_close(NEXT, 2, 2, 4), 2);
    }

    #[test]
    fn closing_the_active_last_tab_clamps_to_the_end() {
        // 关最后一条：没有「下一个」可去，夹到末位——这正是接线前的行为。
        assert_eq!(tab_after_close(NEXT, 4, 4, 4), 3);
        // 只剩一条时关它：下标 0（随后 `ensure_tab` 补一条新的）。
        assert_eq!(tab_after_close(NEXT, 0, 0, 0), 0);
    }

    #[test]
    fn previous_goes_left_and_saturates_at_zero() {
        assert_eq!(tab_after_close(PREV, 3, 3, 4), 2);
        // 关的是第一条：左边没有，停在 0 而不是越界。
        assert_eq!(tab_after_close(PREV, 0, 0, 4), 0);
    }

    #[test]
    fn last_active_falls_back_to_previous_without_history() {
        // 纯函数这一层的 2 只是退路；真正的落点由 `last_active_slot` 覆盖。
        // 钉住退路，是为了让「历史空了」这种情况也有确定行为。
        assert_eq!(tab_after_close(LAST, 2, 2, 4), 1);
        assert_eq!(tab_after_close(LAST, 0, 0, 4), 0);
    }

    #[test]
    fn closing_a_background_tab_keeps_the_active_one_in_view() {
        // 关左侧的背景标签（1），当前在 3：下标左移成 2，但看的还是同一条。
        for kind in [NEXT, PREV, LAST] {
            assert_eq!(tab_after_close(kind, 1, 3, 4), 2, "kind={kind}");
        }
        // 关右侧的背景标签（3），当前在 1：下标不动。
        for kind in [NEXT, PREV, LAST] {
            assert_eq!(tab_after_close(kind, 3, 1, 4), 1, "kind={kind}");
        }
    }

    #[test]
    fn an_out_of_range_kind_behaves_like_next() {
        // 手改过的 prefs 写了 9：按「下一个」处理，不 panic、不跳档。
        assert_eq!(tab_after_close(9, 2, 2, 4), tab_after_close(NEXT, 2, 2, 4));
    }

    #[test]
    fn last_active_picks_the_most_recent_tab_that_is_still_open() {
        let a = egui::Id::new("a");
        let b = egui::Id::new("b");
        let c = egui::Id::new("c");
        // 历史 [a, b]（末尾最近），b 刚被关掉、只剩 a 与 c：落回 a。
        assert_eq!(last_active_slot(&[a, b], &[c, a]), Some(1));
        // 历史里最近的 c 还开着：落 c，不是更早的 a。
        assert_eq!(last_active_slot(&[a, c, b], &[a, c]), Some(1));
        // 历史里没有任何还开着的标签：给 `None`，由调用方沿用向前一条的退路。
        assert_eq!(last_active_slot(&[b], &[a, c]), None);
        assert_eq!(last_active_slot(&[], &[a]), None);
    }
}

// 生命周期回归单测单独放一个文件：夹具要手写一份完整的 TerminalWindow
// 结构体字面量，塞进本文件会顶到门禁的 500 行上限。
#[cfg(test)]
#[path = "tab_strip_lifecycle_tests.rs"]
mod lifecycle_tests;

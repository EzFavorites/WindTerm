//! ADR-0026 ④-0/④-1：窗格（组）级别的会话操作——开新窗格 / 关窗格 / 并入下一窗格 /
//! 拖拽落子 / **组内增删与切换**。从 `split.rs` 拆出来，避免单文件代码行超限
//! （门禁）。
//!
//! 这些方法是 `TerminalWindow` 的 `impl`，但只动 `self.layout`（布局树）与
//! `self.pane_groups`（每窗格的会话集合），不碰终端绘制的细节——绘制在
//! `split.rs::draw_node` / `pane_chrome.rs` 里。两者经 `crate::ui::layout`
//! 再导出互相可见。

use crate::ui::*;
use crate::ui::layout::draw_node;
use crate::ui::layout::mint_pane_identity;
use eframe::egui;
use term_core::session::Protocol;
use term_core::session_runtime::SessionId;

/// 从一个 group 里移除一条会话后的形态（④-1 纯函数，可单测）。
///
/// 这是「每 pane 一组 tab」的关闭语义：组里还有别的会话时**只换该 pane 显示的
/// 那一条、不剪叶子**；只有组空了才让调用方剪掉这片窗格（用户拍板 ⑥
/// 「最后一个关闭的时候窗格关闭」）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::ui) enum GroupRemoval {
    /// 组里还有会话，给出该 pane 新的活动会话。
    Remaining { next_active: SessionId },
    /// 组已空，应剪掉这片叶子（最后一组特例由 `ensure_tab` 保活兜住）。
    Empty,
}

/// 从 `order` 里摘掉 `closing`，返回新的 order 与该 pane 的下一步。
///
/// - 关的不是当前显示的那条 → 原 `active` 不动，其它 pane 完全不受影响。
/// - 关的是当前显示的那条 → 落到**被关那条的左邻居**（没有就取第一条），
///   与标签栏「关掉当前 tab 看前一条」的直觉一致。
/// - `closing` 不在组里 → 原样返回（调用方据此当作空操作）。
pub(in crate::ui) fn remove_from_group(
    order: &[SessionId],
    active: SessionId,
    closing: SessionId,
) -> (Vec<SessionId>, GroupRemoval) {
    let Some(pos) = order.iter().position(|s| *s == closing) else {
        return (order.to_vec(), GroupRemoval::Remaining { next_active: active });
    };
    let mut next = order.to_vec();
    next.remove(pos);
    if next.is_empty() {
        return (next, GroupRemoval::Empty);
    }
    let next_active = if active == closing {
        next[pos.saturating_sub(1).min(next.len() - 1)]
    } else {
        active
    };
    (next, GroupRemoval::Remaining { next_active })
}

impl TerminalWindow {
    /// 给定一条会话，找出它属于哪个窗格（PaneId）。
    ///
    /// 一个 `SessionId` 只属于一个 group（ADR-0026），所以至多返回一个。
    /// 拖拽 / 关窗格 / 分屏时都需要把「当前活动会话」翻译成「它所在的窗格」。
    pub(in crate::ui) fn pane_of_session(&self, session: SessionId) -> Option<PaneId> {
        self.pane_groups
            .iter()
            .find_map(|(pid, g)| g.order.contains(&session).then_some(*pid))
    }

    /// 递归切分 CentralPanel。只在布局真的分了屏时才被调用。
    pub(in crate::ui) fn draw_split_layout(&mut self, ui: &mut egui::Ui) {
        let tree = self.layout.clone();
        let mut path = Vec::new();
        draw_node(self, ui, &tree, 1, &mut path);
    }

    /// Split View：新开一条本地 shell 并排；Split Group：复制当前会话配置再开一条。
    ///
    /// 两条命令的差别是**真实**的（不同配置），不是同一效果的别名；方向由
    /// `horizontal` 决定（左右 / 上下）。
    ///
    /// ADR-0026：分屏 = 给当前窗格复制一份配置、但开**新独立会话**（不是共享
    /// `SessionId`），所以新窗格是一个新的 `PaneId` + 一个只含新会话的 group。
    pub(in crate::ui) fn split_view(&mut self, horizontal: bool, copy_config: bool) {
        // ④-0：确保单窗格时那唯一一片窗格的 group 指向当前 active，
        // 这样下面的 `pane_of_session` 锚点才找得到（即便 active 是被直接赋值改的）。
        self.sync_single_pane_to_active();
        let cfg = if copy_config {
            match self.tabs.get(self.active_slot()) {
                Some(tab) => {
                    let mut c = tab.config.clone();
                    c.name = format!("{} · 2", c.name);
                    c
                }
                None => return,
            }
        } else {
            // 分屏新开的是**新会话**，光标样式取 Caret 页的偏好。
            self.fresh_session(Protocol::Shell)
        };
        // 复制一个内置 SSH 会话时，新窗格要等握手完成才存在，所以
        // 「切哪一格、往哪个方向切」要交给连接一起带去落地（`PendingOpen::Split`）。
        // 以前这里直接 `make_tab`：窗格立刻出现，但内容是「(disconnected)」。
        if copy_config && panels::sidebar::uses_builtin_ssh(&cfg) {
            // 先把「当前活动会话」翻译成它所在的窗格身份，再交给落地。
            let anchor = self
                .pane_of_session(self.active.unwrap_or(0))
                .unwrap_or_else(|| {
                    self.layout
                        .leaf_panes()
                        .first()
                        .copied()
                        .unwrap_or(PaneId(0))
                });
            self.begin_builtin_ssh_with(
                cfg,
                None,
                crate::ui::dialogs::ssh_connect::PendingOpen::Split { anchor, horizontal },
            );
            return;
        }
        let new_tab = self.tabs.len();
        let id = egui::Id::new(("tab_split", new_tab));
        let tab = crate::ui::terminal::make_tab(&self.pool, cfg, id, &self.ctx);
        let new_id = tab.identity;
        self.tabs.push(tab);
        // ④-0：新窗格 = 新 PaneId + 只含新会话的 group。
        let new_pane = mint_pane_identity();
        let active_pane = self
            .pane_of_session(self.active.unwrap_or(0))
            .unwrap_or_else(|| {
                self.layout
                    .leaf_panes()
                    .first()
                    .copied()
                    .unwrap_or(PaneId(0))
            });
        self.pane_groups.insert(
            new_pane,
            PaneGroup {
                order: vec![new_id],
                active: new_id,
            },
        );
        // ADR-0025 阶段 3：布局叶子按身份寻址，直接用新窗格的身份，不再传下标。
        self.layout.split(active_pane, horizontal, new_pane);
        // 结构变了，本帧末即时落盘（经 `Action::SaveLayout` 死路径复活）。
        self.pending_save_layout = true;
        // 新窗格获得焦点，敲键盘立刻进新会话——除非 `Behaviour ▸ Focus the new
        // tab` 关着（那时焦点留在原来的窗格，新标签照样建出来了）。
        self.activate_new_tab(new_tab);
        self.ctx.request_repaint();
    }

    /// 关掉当前窗格（标签本身留在标签栏，仍可从那里切回来）。
    pub(in crate::ui) fn close_active_group(&mut self) {
        let active = self.active.unwrap_or(0);
        let Some(active_pane) = self.pane_of_session(active) else {
            return;
        };
        if self.layout.close_leaf(active_pane) {
            // 关掉后活动身份落到树上还剩的第一个叶子（可能为空 = 无活动标签）。
            self.active = self.layout.tabs(&self.pane_groups).first().copied();
            self.ctx.request_repaint();
            // 结构变了，本帧末即时落盘。
            self.pending_save_layout = true;
        }
    }

    /// 把当前会话并入下一个窗格，当前窗格收起。
    pub(in crate::ui) fn move_active_to_next_group(&mut self) {
        let active = self.active.unwrap_or(0);
        let Some(active_pane) = self.pane_of_session(active) else {
            return;
        };
        if self.layout.merge_into_next(active_pane) {
            self.ctx.request_repaint();
            // 结构变了，本帧末即时落盘。
            self.pending_save_layout = true;
        }
    }

    /// 把正在拖拽的会话落进 `target` 这片窗格——交换两格内容并清掉拖拽态。
    ///
    /// 纯逻辑，不读 egui 输入，便于单测直接驱动（落点命中判定在 `draw_node` 里读
    /// 指针位置完成）。返回是否真的发生了落子：`drag_session` 为空（没在拖）或
    /// 拖回自己那一片（同身份）都返回 `false`，调用方据此决定要不要高亮/取消。
    pub(in crate::ui) fn drop_drag_on_pane(&mut self, target: PaneId) -> bool {
        let Some(drag) = self.drag_session else {
            return false;
        };
        // 把被拖的会话翻译成它所在的窗格，再与落点窗格比较。
        let Some(from) = self.pane_of_session(drag) else {
            return false;
        };
        if from == target {
            // 拖回自己那片：本帧末 flush 会清掉拖拽态，这里只当没发生。
            return false;
        }
        self.layout.swap_leaves(from, target);
        self.drag_session = None;
        // 命中落点：拖拽已结束，别等帧末 flush 再清。
        self.drag_released_this_frame = false;
        self.pending_save_layout = true;
        true
    }

    /// ④-1：在**指定的 pane 组内**开一条新会话（不新建窗格）。
    ///
    /// 用户拍板 ①「分屏 / 新增等，全部是开一个新会话」——这里 `fresh_session`
    /// 起一条新的本地 shell（新 `SessionId` / 新 PTY / 独立 viewport），
    /// 追加进 `groups[pane].order` 并设为该组活动会话；布局树一个字节不动。
    /// 同步把全局 `active` 指过去，键盘输入立即落到这条新会话。
    pub(in crate::ui) fn new_tab_in_pane(&mut self, pane: PaneId) {
        let cfg = self.fresh_session(Protocol::Shell);
        let id = egui::Id::new(("pane_tab_new", pane.0, self.tabs.len()));
        let tab = crate::ui::terminal::make_tab(&self.pool, cfg, id, &self.ctx);
        let new_id = tab.identity;
        self.add_tab(tab);
        let Some(group) = self.pane_groups.get_mut(&pane) else {
            return;
        };
        group.order.push(new_id);
        group.active = new_id;
        self.active = Some(new_id);
        // 组内成员变了，但布局树形状没变；仍要落盘，否则重启丢掉这条组关系
        // （④-2 升 v3 后 `SavedNode::Leaf{sessions}` 才能真正记全，此处先置标志）。
        self.pending_save_layout = true;
        self.ctx.request_repaint();
    }

    /// ④-1：关掉**指定 pane 组内**的一条会话。
    ///
    /// 这里只做两件事：确认这条会话确实属于该 pane、把它翻译成 `tabs` 下标。
    /// **真正的组语义全在 `close_tab` 里**（`shell/tab_strip.rs`）——它按
    /// `remove_from_group` 的结果决定「组不空只换 active / 组空才剪叶」，
    /// 并复用确认弹窗 / Reopen Closed / 颜色 / 焦点历史那一整套。
    ///
    /// 不要在这里预先改 `group.order`：`close_tab` 靠 `pane_of_session` 反查窗格，
    /// 提前清空会让它找不到 pane、剪叶逻辑被整段跳过（曾触发单测红灯）。
    pub(in crate::ui) fn close_tab_in_pane(&mut self, pane: PaneId, session: SessionId) {
        let belongs = self
            .pane_groups
            .get(&pane)
            .is_some_and(|g| g.order.contains(&session));
        if !belongs {
            return;
        }
        let Some(index) = self.tabs.iter().position(|t| t.identity == session) else {
            return;
        };
        self.close_tab_with_history(index);
        self.pending_save_layout = true;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui::layout::{PaneGroup, PaneId};
    use std::collections::HashMap;

    #[test]
    fn removing_a_background_session_keeps_the_group_active() {
        let order = [10u64, 11, 12];
        let (next, outcome) = remove_from_group(&order, 11, 10);
        assert_eq!(next, vec![11, 12]);
        // 关的不是当前显示的那条：活动态不动。
        assert_eq!(outcome, GroupRemoval::Remaining { next_active: 11 });
    }

    #[test]
    fn removing_the_active_session_falls_back_to_its_left_neighbour() {
        let (next, outcome) = remove_from_group(&[10u64, 11, 12], 11, 11);
        assert_eq!(next, vec![10, 12]);
        assert_eq!(outcome, GroupRemoval::Remaining { next_active: 10 });
        // 关第一条：左边没有邻居，落到新的第一条。
        let (_, outcome) = remove_from_group(&[10u64, 11], 10, 10);
        assert_eq!(outcome, GroupRemoval::Remaining { next_active: 11 });
    }

    #[test]
    fn removing_the_last_session_reports_an_empty_group() {
        let (next, outcome) = remove_from_group(&[10u64], 10, 10);
        assert!(next.is_empty());
        assert_eq!(outcome, GroupRemoval::Empty);
    }

    #[test]
    fn removing_a_session_not_in_the_group_is_a_no_op() {
        let (next, outcome) = remove_from_group(&[10u64, 11], 11, 99);
        assert_eq!(next, vec![10, 11]);
        assert_eq!(outcome, GroupRemoval::Remaining { next_active: 11 });
    }

    /// ④-1 端到端：`＋` 只往本组加、`×` 组不空时不剪叶、组空时剪叶，
    /// 且另一个 pane 的 group 始终不受影响（用户拍板「每 pane 一组 tab」）。
    #[test]
    fn per_pane_add_and_close_never_touch_the_other_group() {
        let pool = std::sync::Arc::new(term_session::SessionPool::new());
        let ctx = egui::Context::default();
        let t0 = crate::ui::lifecycle_test_support::real_shell_tab(&pool, &ctx, 0);
        let t1 = crate::ui::lifecycle_test_support::real_shell_tab(&pool, &ctx, 1);
        let id0 = t0.identity;
        let id1 = t1.identity;
        let mut win = crate::ui::lifecycle_test_support::window_with(pool, &ctx, vec![t0, t1]);
        let p0 = PaneId(9001);
        let p1 = PaneId(9002);
        let mut groups: HashMap<PaneId, PaneGroup> = HashMap::new();
        groups.insert(p0, PaneGroup { order: vec![id0], active: id0 });
        groups.insert(p1, PaneGroup { order: vec![id1], active: id1 });
        win.pane_groups = groups;
        win.layout = Layout::Split {
            horizontal: true,
            ratio: 500,
            first: Box::new(Layout::Leaf(p0)),
            second: Box::new(Layout::Leaf(p1)),
        };
        win.active = Some(id0);
        // 关掉确认弹窗：测试直接走 `close_tab_in_pane`，不该被弹窗拦下。
        win.confirm.suppress_session = true;

        // ＋：本组多一条，另一组一字不动，布局树形状不变。
        let before = win.tabs.len();
        win.new_tab_in_pane(p0);
        assert_eq!(win.tabs.len(), before + 1, "＋ 要开一条新会话");
        let g0 = win.pane_groups.get(&p0).unwrap();
        assert_eq!(g0.order.len(), 2);
        assert_eq!(win.pane_groups.get(&p1).unwrap().order, vec![id1]);
        assert_eq!(win.layout.leaf_count(), 2, "＋ 不新建窗格");
        let new_id = g0.active;

        // × 关掉新加的那条：组里还有 id0，pane 不能消失。
        win.close_tab_in_pane(p0, new_id);
        assert_eq!(win.layout.leaf_count(), 2, "组不空 → 不剪叶");
        assert_eq!(win.pane_groups.get(&p0).unwrap().order, vec![id0]);
        assert_eq!(win.pane_groups.get(&p1).unwrap().order, vec![id1]);

        // 再关掉本组最后一条：这片窗格收起，另一片仍在。
        win.close_tab_in_pane(p0, id0);
        assert_eq!(win.layout.leaf_count(), 1);
        assert!(!win.pane_groups.contains_key(&p0));
        assert_eq!(win.pane_groups.get(&p1).unwrap().order, vec![id1]);
    }
}

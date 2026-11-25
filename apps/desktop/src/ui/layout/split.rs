//! 分屏布局树——把「一个 CentralPanel 一个活动标签」变成可二分的窗格。
//!
//! 这是布局树的**最小可用形态**：内部节点只有左右 / 上下二等分，叶子挂一个
//! 窗格身份（[`PaneId`]）。兼容终端的 Split View / Split Group / Move Group 是多组
//! 嵌套的完整布局系统，本批不假装做到那个程度（逐条语义见
//! `docs/design/04-tabs-split-layout.md`）——但六条命令都真的改变可见窗格，不是空转
//! 开关。
//!
//! **单叶子时走原来的渲染路径**：`draw()` 只有 `layout.is_single()` 为 false
//! 才进这里的递归切分，所以五张视觉基线一字节都不变。
//!
//! 布局不是 `ViewState` 字段：它和 `tabs` / `active` 同类，是投影层的视图
//! 结构，由 Effect 落地修改；reducer 只声明意图，不认识布局树。
//!
//! ## ADR-0026 ④-0：每 pane 一组 tab
//!
//! 叶子从「恒在身份（`SessionId`）」升级为「窗格身份（`PaneId`）」。每片窗格
//! （一个 `PaneId`）对应 `TerminalWindow.pane_groups` 里的一个 [`PaneGroup`]，
//! 记录该窗格里挂着哪些会话、当前显示哪一个。`Layout` 树只管「窗格怎么切分」，
//! 会话集合与活动态由 `pane_groups` 持有——这是「每 pane 一组 tab」的物理承载
//! （详见 `docs/design/04-tabs-split-layout.md`）。

use crate::ui::*;
use eframe::egui;
use std::collections::HashMap;
use term_core::session_runtime::SessionId;
use term_core::theme::ACCENT_BLUE;

/// 窗格（一个组）的恒在身份。
///
/// 与 `SessionId` 同级的身份：每个 `Layout::Leaf` 挂一个 `PaneId`，运行时
/// 单调递增、进程内唯一（见 `mint_pane_identity`）。不落盘——恢复时由树形
/// 重建（`restore.rs` 的 `from_saved` / `restore_tree`）。
///
/// 与 `SessionId` 是**不同的 newtype**，刻意不混用：把 PaneId 误当 SessionId
/// 传进 `draw_terminal_area_for`、或反过来，都会在编译期被挡下。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct PaneId(pub u64);

/// 一个窗格（组）里挂着的会话，以及当前显示的那一条。
///
/// `order` 是该窗格所有会话（标签）；`active` 是当前可见的那一条。`Layout` 树
/// 只存 `PaneId`，渲染前经 `pane_groups[pane].active` 解析出 `SessionId` 再交给
/// `draw_terminal_area_for`——窗格里切标签只改 `active`，不动布局树。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PaneGroup {
    pub order: Vec<SessionId>,
    pub active: SessionId,
}

/// ADR-0026：每 pane 一组 tab——窗格（组）的恒在身份铸造源。
///
/// 与 `TAB_IDENTITY_SRC` 同款，只是命名空间不同：PaneId 标识「一片窗格（一个组）」，
/// 而非「一条会话」。运行期单调递增、进程内唯一；不落盘，恢复时由树形重建
/// （见 `restore.rs` 的 `from_saved` / `restore_tree`）。
static PANE_IDENTITY_SRC: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);

/// 给一片新窗格铸造恒在身份（见 [`PANE_IDENTITY_SRC`]，ADR-0026 ④-0）。
pub(in crate::ui) fn mint_pane_identity() -> PaneId {
    PaneId(PANE_IDENTITY_SRC.fetch_add(1, std::sync::atomic::Ordering::Relaxed))
}

/// ADR-0026：group 的名字——按 `PaneId` 恒在身份派生，`1→A` … `26→Z`、`27→AA`
/// （Excel 列名式）。
///
/// 用户裁定「A/B/C/D 只是 group 的名字」：兼容终端 `Merge To Group ▸` 子菜单按名字列出
/// `A/B/C/D`，名字**跟着 group 走、不随屏幕位置重排**（四格截图里 `A` 跑到右上、
/// 后建的 `B`/`C`/`D` 在别处）。`PaneId` 本就是运行期按创建序递增的恒在身份，
/// 直接派生即可，不需要再存一份 `name` 字段、也不需要额外的分配器状态。
///
/// 纯函数、无 IO、可单测；不落盘，恢复时 `PaneId` 由树形重建、名字随之重建
/// （④-2 升 v3 后再决定是否把名字写进盘）。字母是否可重命名 / 不连续时怎么补位
/// 未取证，暂按创建序连续派生。
pub(in crate::ui) fn pane_group_name(pane: PaneId) -> String {
    let mut n = pane.0;
    let mut out = String::new();
    while n > 0 {
        let rem = (n - 1) % 26;
        out.insert(0, (b'A' + rem as u8) as char);
        n = (n - 1) / 26;
    }
    if out.is_empty() {
        // `PaneId(0)` 只作兜底（见 `unwrap_or(PaneId(0))`），不能画成空串。
        out.push('A');
    }
    out
}

/// 一棵二分布局树。`horizontal = true` 表示左右并排，`false` 表示上下。
///
/// ADR-0025 阶段 3：叶子挂的是**恒在身份**。ADR-0026 ④-0：身份从 `SessionId`
/// 升级为 `PaneId`——标签增删 / 重排不会让叶子指错窗格，`reindex_after_remove`
/// 这类「删一条就全体挪号」的补丁随之消失；一个 `SessionId` 只属于一个 group。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Layout {
    Leaf(PaneId),
    Split {
        horizontal: bool,
        ratio: u16,
        first: Box<Layout>,
        second: Box<Layout>,
    },
}

/// 分屏比例以「千分比」存储：`500` = 50%，`100..=900` 夹住不让某一格消失。
/// 用整数而非 `f32`：`Layout`/`SavedNode` 都派生 `Eq`，`f32` 不可 `Eq`。
const SPLIT_RATIO_SCALE: u16 = 1000;
const SPLIT_RATIO_DEFAULT: u16 = 500;
const SPLIT_RATIO_MIN: u16 = 100;
const SPLIT_RATIO_MAX: u16 = 900;

impl Layout {
    pub fn single(pane: PaneId) -> Self {
        Layout::Leaf(pane)
    }

    pub fn is_single(&self) -> bool {
        matches!(self, Layout::Leaf(_))
    }

    /// 叶子数（至少 1）。
    pub fn leaf_count(&self) -> usize {
        self.leaf_panes().len()
    }

    /// 按遍历顺序列出所有叶子挂的窗格身份（PaneId）。
    pub fn leaf_panes(&self) -> Vec<PaneId> {
        let mut out = Vec::new();
        collect_panes(self, &mut out);
        out
    }

    /// 按遍历顺序列出所有叶子当前展示的会话（各 group 的 active）。
    ///
    /// 需要 `pane_groups` 才能把 `PaneId` 解析回 `SessionId`：一个 `Layout` 树
    /// 本身不知道每片窗格里显示的是哪条会话（那是 `pane_groups` 的职责）。
    pub fn tabs(&self, groups: &HashMap<PaneId, PaneGroup>) -> Vec<SessionId> {
        self.leaf_panes()
            .iter()
            .map(|p| groups.get(p).map(|g| g.active).unwrap_or(0))
            .collect()
    }

    /// 把 `active_pane` 所在的第一个叶子拆成两个，新叶子挂 `new_pane`。
    ///
    /// 活动窗格不在树里（例如刚被关过）时，退化为「整棵树 + 新叶子」并排，
    /// 不会丢掉已有窗格。
    pub fn split(&mut self, active_pane: PaneId, horizontal: bool, new_pane: PaneId) {
        if split_at(self, active_pane, horizontal, new_pane) {
            return;
        }
        let old = std::mem::replace(self, Layout::Leaf(PaneId(0)));
        *self = Layout::Split {
            horizontal,
            ratio: SPLIT_RATIO_DEFAULT,
            first: Box::new(old),
            second: Box::new(Layout::Leaf(new_pane)),
        };
    }

    /// 关掉 `active_pane` 所在的**第一个**叶子，兄弟顶上来。
    /// 已经是单叶子时返回 false（窗口里始终留一块终端）。
    pub fn close_leaf(&mut self, active_pane: PaneId) -> bool {
        if self.is_single() {
            return false;
        }
        let mut removed = false;
        let pruned = prune_one(self.clone(), active_pane, &mut removed);
        match pruned {
            Some(tree) => {
                *self = tree;
                true
            }
            None => {
                *self = Layout::Leaf(active_pane);
                false
            }
        }
    }

    /// 把 `active_pane` 并入遍历顺序里的下一个叶子，并关掉它自己那块窗格。
    /// 返回被覆盖的旧窗格身份（调用方据此摘掉它的 `pane_groups`），无动作返回 None。
    pub fn merge_into_next(&mut self, active_pane: PaneId) -> Option<PaneId> {
        let Some((next, old_next)) = super::merge_group::merge_target(&self.leaf_panes(), active_pane) else {
            return None;
        };
        let mut cursor = 0usize;
        set_nth_pane(self, next, active_pane, &mut cursor);
        self.close_leaf(active_pane);
        Some(old_next)
    }

    /// 整树为单叶子且恰好指着 `dead` 时，改成 `live`。用于「关掉窗口里最后一条
    /// 会话」那种不能让窗格消失的例外：窗格保留，内容换成 `ensure_tab` 补出的活
    /// 标签。只动单叶子情形——多叶子结构的每个叶子都在 `close_leaf` 时被正确剪除。
    pub fn repoint_single_if_dead(&mut self, dead: PaneId, live: PaneId) {
        if let Layout::Leaf(d) = self {
            if *d == dead {
                *d = live;
            }
        }
    }

    // `swap_leaves`（拖拽交换 pane 的纯函数）见同目录 `swap.rs`，单测也随它走。

    /// 沿着 `path`（`0` = 第一个孩子，`1` = 第二个孩子，空 = 当前节点）定位分屏节点
    /// 并改写比例。比例夹在 [`SPLIT_RATIO_MIN`]…[`SPLIT_RATIO_MAX`] 之内，不让任一格消失。
    /// 路径落到的不是分屏（叶子 / 越界）时静默无操作。
    pub fn set_ratio_at(&mut self, path: &[usize], ratio: u16) {
        match self {
            Layout::Leaf(_) => {}
            Layout::Split {
                first,
                second,
                ratio: r,
                ..
            } => match path.split_first() {
                None => *r = ratio.clamp(SPLIT_RATIO_MIN, SPLIT_RATIO_MAX),
                Some((&head, tail)) => match head {
                    0 => first.set_ratio_at(tail, ratio),
                    1 => second.set_ratio_at(tail, ratio),
                    _ => {}
                },
            },
        }
    }

    /// 读某个分屏节点的当前比例；不是分屏或路径越界返回 `None`。
    pub fn ratio_at(&self, path: &[usize]) -> Option<u16> {
        match self {
            Layout::Leaf(_) => None,
            Layout::Split { first, second, ratio, .. } => {
                match path.split_first() {
                    None => Some(*ratio), // 空路径 = 当前节点
                    Some((&head, tail)) => match head {
                        0 => first.ratio_at(tail),
                        1 => second.ratio_at(tail),
                        _ => None,
                    },
                }
            }
        }
    }
}

fn collect_panes(node: &Layout, out: &mut Vec<PaneId>) {
    match node {
        Layout::Leaf(pane) => out.push(*pane),
        Layout::Split { first, second, .. } => {
            collect_panes(first, out);
            collect_panes(second, out);
        }
    }
}

fn split_at(node: &mut Layout, target: PaneId, horizontal: bool, new_pane: PaneId) -> bool {
    match node {
        Layout::Leaf(pane) if *pane == target => {
            *node = Layout::Split {
                horizontal,
                ratio: SPLIT_RATIO_DEFAULT,
                first: Box::new(Layout::Leaf(target)),
                second: Box::new(Layout::Leaf(new_pane)),
            };
            true
        }
        Layout::Leaf(_) => false,
        Layout::Split { first, second, .. } => {
            split_at(first, target, horizontal, new_pane)
                || split_at(second, target, horizontal, new_pane)
        }
    }
}

/// 递归剪掉第一个匹配的叶子；`None` 表示这棵子树整个被剪空。
fn prune_one(node: Layout, target: PaneId, removed: &mut bool) -> Option<Layout> {
    if *removed {
        return Some(node);
    }
    match node {
        Layout::Leaf(pane) => {
            if pane == target {
                *removed = true;
                None
            } else {
                Some(Layout::Leaf(pane))
            }
        }
        Layout::Split {
            horizontal,
            ratio,
            first,
            second,
        } => {
            let a = prune_one(*first, target, removed);
            let b = prune_one(*second, target, removed);
            match (a, b) {
                (Some(a), Some(b)) => Some(Layout::Split {
                    horizontal,
                    ratio,
                    first: Box::new(a),
                    second: Box::new(b),
                }),
                (Some(x), None) | (None, Some(x)) => Some(x),
                (None, None) => None,
            }
        }
    }
}

fn set_nth_pane(node: &mut Layout, nth: usize, pane: PaneId, cursor: &mut usize) {
    match node {
        Layout::Leaf(p) => {
            if *cursor == nth {
                *p = pane;
            }
            *cursor += 1;
        }
        Layout::Split { first, second, .. } => {
            set_nth_pane(first, nth, pane, cursor);
            set_nth_pane(second, nth, pane, cursor);
        }
    }
}


pub(in crate::ui) fn draw_node(
    win: &mut TerminalWindow,
    ui: &mut egui::Ui,
    node: &Layout,
    salt: usize,
    path: &mut Vec<usize>,
) {
    match node {
        Layout::Leaf(pane) => {
            // ④-0：叶子是窗格身份，经 `pane_groups` 解析出当前显示的会话再画。
            // 找不到（退出态等极少数情况）只画空白，不 panic。
            let Some(session) = win.pane_groups.get(pane).map(|g| g.active) else {
                return;
            };
            let rect = ui.available_rect_before_wrap();
            // 标签栏拖拽中的会话悬停在这片窗格时给放置高亮；松手即交换两格内容。
            // 只在「确实在拖」且不是同一片（拖回自己）时才处理。
            if let Some(drag) = win.drag_session {
                if drag != session {
                    let over = ui
                        .ctx()
                        .pointer_hover_pos()
                        .is_some_and(|p| rect.contains(p));
                    let released = ui.ctx().input(|i| {
                        i.pointer.button_released(egui::PointerButton::Primary)
                    });
                    if over {
                        // 放置提示：整片描一道主题蓝边（与标签活动线同色 #007CD2）。
                        let blue = egui::Color32::from_rgb(
                            (ACCENT_BLUE >> 16) as u8,
                            (ACCENT_BLUE >> 8) as u8,
                            ACCENT_BLUE as u8,
                        );
                        ui.painter().rect_stroke(
                            rect.shrink2(egui::vec2(1.5, 1.5)),
                            0.0,
                            egui::Stroke::new(2.0, blue),
                        );
                    }
                    if over && released {
                        win.drop_drag_on_pane(*pane);
                    }
                }
            }
            // ④-1：分屏时每片 Leaf 画自己的一套 chrome（标签条 + 地址栏 +
            // 终端 + Sysmon）；单窗格不走 draw_node，全窗顶栏原样保留。
            // 专注模式由 `pane_chrome::draw_pane_leaf` 按 `win.view.focus_mode`
            // 压掉前三段，只留终端——分屏格子里也不许漏出 chrome。
            let group = win.pane_groups.get(pane).cloned();
            if let Some(group) = group {
                super::pane_chrome::draw_pane_leaf(win, ui, *pane, &group, rect);
            } else {
                win.draw_terminal_area_for(ui, session);
            }
        }
        Layout::Split {
            horizontal,
            ratio,
            first,
            second,
        } => {
            let rect = ui.available_rect_before_wrap();
            let gap = 2.0;
            // 比例取运行态真实值（拖动后当帧生效，无帧延迟）；缺省退回 50%。
            let ratio = win
                .layout
                .ratio_at(path)
                .unwrap_or(*ratio)
                .clamp(SPLIT_RATIO_MIN, SPLIT_RATIO_MAX);
            let frac = ratio as f32 / SPLIT_RATIO_SCALE as f32;
            let (r1, r2, line, sash) = if *horizontal {
                let mid = rect.min.x + rect.width() * frac;
                (
                    Rect::from_min_max(rect.min, egui::pos2(mid - gap, rect.max.y)),
                    Rect::from_min_max(egui::pos2(mid + gap, rect.min.y), rect.max),
                    [
                        egui::pos2(mid, rect.min.y),
                        egui::pos2(mid, rect.max.y),
                    ],
                    Rect::from_min_max(
                        egui::pos2(mid - 4.0, rect.min.y),
                        egui::pos2(mid + 4.0, rect.max.y),
                    ),
                )
            } else {
                let mid = rect.min.y + rect.height() * frac;
                (
                    Rect::from_min_max(rect.min, egui::pos2(rect.max.x, mid - gap)),
                    Rect::from_min_max(egui::pos2(rect.min.x, mid + gap), rect.max),
                    [
                        egui::pos2(rect.min.x, mid),
                        egui::pos2(rect.max.x, mid),
                    ],
                    Rect::from_min_max(
                        egui::pos2(rect.min.x, mid - 4.0),
                        egui::pos2(rect.max.x, mid + 4.0),
                    ),
                )
            };
            // 分隔条：可拖拽调整两格比例。用稳定 id（`salt`）而非 rect 派生 id，
            // 否则拖动时 rect 一变 id 跟着变，拖拽状态会丢。
            let sash_resp = ui.interact(
                sash,
                egui::Id::new(("split_sash", salt)),
                egui::Sense::drag(),
            );
            if sash_resp.dragged() {
                let pointer = sash_resp
                    .interact_pointer_pos()
                    .unwrap_or_else(|| ui.cursor().min);
                let raw = if *horizontal {
                    ((pointer.x - rect.min.x) / rect.width() * SPLIT_RATIO_SCALE as f32) as i32
                } else {
                    ((pointer.y - rect.min.y) / rect.height() * SPLIT_RATIO_SCALE as f32) as i32
                };
                let new_ratio = raw.clamp(SPLIT_RATIO_MIN as i32, SPLIT_RATIO_MAX as i32) as u16;
                // 只有比例确实变了才置待存标志，避免拖动中无变化帧反复写盘；
                // sash 松手那帧由最后一次变化帧覆盖，同样会得到即时落盘。
                let changed = win.layout.ratio_at(path) != Some(new_ratio);
                win.layout.set_ratio_at(path, new_ratio);
                if changed {
                    win.pending_save_layout = true;
                }
                ui.ctx().request_repaint();
            }
            let mut a = ui.new_child(
                egui::UiBuilder::new()
                    .id_salt(("split_first", salt))
                    .max_rect(r1),
            );
            let mut b = ui.new_child(
                egui::UiBuilder::new()
                    .id_salt(("split_second", salt))
                    .max_rect(r2),
            );
            path.push(0);
            draw_node(win, &mut a, first, salt * 2 + 1, path);
            path.pop();
            path.push(1);
            draw_node(win, &mut b, second, salt * 2 + 2, path);
            path.pop();
            // 父 Ui 必须知道这块被占掉了，否则后续内容会叠上来。
            ui.allocate_rect(rect, egui::Sense::hover());
            ui.painter().line_segment(
                line,
                egui::Stroke::new(1.0, egui::Color32::from_gray(80)),
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_single_leaf_is_the_default_and_is_never_closed() {
        let mut l = Layout::single(PaneId(0));
        assert!(l.is_single());
        assert_eq!(l.leaf_panes(), vec![PaneId(0)]);
        assert!(!l.close_leaf(PaneId(0)));
        assert_eq!(l, Layout::single(PaneId(0)));
    }

    #[test]
    fn splitting_adds_a_second_leaf_in_the_requested_direction() {
        let mut l = Layout::single(PaneId(0));
        l.split(PaneId(0), true, PaneId(1));
        assert_eq!(l.leaf_count(), 2);
        assert_eq!(l.leaf_panes(), vec![PaneId(0), PaneId(1)]);
        assert!(matches!(l, Layout::Split { horizontal: true, .. }));
    }

    #[test]
    fn splitting_an_unknown_pane_keeps_the_existing_panes() {
        let mut l = Layout::single(PaneId(0));
        l.split(PaneId(0), false, PaneId(1));
        l.split(PaneId(99), true, PaneId(2));
        assert_eq!(l.leaf_count(), 3);
        let panes = l.leaf_panes();
        assert!(panes.contains(&PaneId(0)) && panes.contains(&PaneId(1)));
    }

    #[test]
    fn closing_a_leaf_lifts_its_sibling() {
        let mut l = Layout::single(PaneId(0));
        l.split(PaneId(0), true, PaneId(1));
        assert!(l.close_leaf(PaneId(0)));
        assert_eq!(l, Layout::single(PaneId(1)));
    }

    #[test]
    fn merging_moves_the_active_session_into_the_next_pane() {
        let mut l = Layout::single(PaneId(0));
        l.split(PaneId(0), true, PaneId(1));
        assert!(l.merge_into_next(PaneId(0)).is_some());
        assert_eq!(l, Layout::single(PaneId(0)), "另一个窗格换成当前会话后只剩一块");
        let mut l2 = Layout::single(PaneId(0));
        l2.split(PaneId(0), true, PaneId(1));
        l2.split(PaneId(1), false, PaneId(2));
        assert!(l2.merge_into_next(PaneId(0)).is_some());
        assert_eq!(l2.leaf_count(), 2);
        assert!(l2.leaf_panes().contains(&PaneId(0)));
    }

    /// ADR-0026：group 名字由 `PaneId` 恒在身份派生，跟着 group 走、
    /// 不随屏幕位置重排（用户裁定「A/B/C/D 只是 group 的名字」）。
    #[test]
    fn group_names_follow_pane_identity() {
        assert_eq!(pane_group_name(PaneId(1)), "A");
        assert_eq!(pane_group_name(PaneId(2)), "B");
        assert_eq!(pane_group_name(PaneId(3)), "C");
        assert_eq!(pane_group_name(PaneId(4)), "D");
        // 兼容终端四格截图就是 A/B/C/D；名字不随 pane 在树里的位置变化。
        assert_eq!(pane_group_name(PaneId(26)), "Z");
        assert_eq!(pane_group_name(PaneId(27)), "AA");
        assert_eq!(pane_group_name(PaneId(28)), "AB");
        // 身份不同名字必不同；`PaneId(0)` 兜底不能是空串。
        assert_ne!(pane_group_name(PaneId(3)), pane_group_name(PaneId(4)));
        assert_eq!(pane_group_name(PaneId(0)), "A");
    }

    /// 用户 2026-11-15 定的规则：**每分裂一格就领一个新组名，按大写字母递增**。
    ///
    /// 三条约束一起锁住，防止以后有人改成「按屏幕位置编号」或「关闭后回收字母」：
    /// 1. 每次 `mint_pane_identity` 拿到的身份严格更大（分裂 = 领新号）；
    /// 2. 若干次分裂得到的名字**两两不同**（不复用）；
    /// 3. 名字恒为纯大写字母（`A`…`Z`、`AA`…，Excel 列名式）。
    ///
    /// 不断言具体字母：`PANE_IDENTITY_SRC` 是进程级 `static`，并行测试里
    /// 别的用例会先领走若干号，断言 `"A"/"B"` 会 flaky。只断言相对性质。
    #[test]
    fn every_split_gets_a_fresh_uppercase_name() {
        let ids: Vec<PaneId> = (0..8).map(|_| mint_pane_identity()).collect();
        for w in ids.windows(2) {
            assert!(w[0].0 < w[1].0, "每次分裂必须领到更大的身份");
        }
        let names: Vec<String> = ids.iter().map(|p| pane_group_name(*p)).collect();
        for i in 0..names.len() {
            assert!(
                !names[i].is_empty() && names[i].chars().all(|c| c.is_ascii_uppercase()),
                "组名必须是纯大写字母，实际 {:?}",
                names[i]
            );
            for j in (i + 1)..names.len() {
                assert_ne!(names[i], names[j], "不同 group 不能重名");
            }
        }
    }

    #[test]
    fn merging_a_single_pane_is_a_no_op() {
        let mut l = Layout::single(PaneId(0));
        assert!(l.merge_into_next(PaneId(0)).is_none());
        assert_eq!(l, Layout::single(PaneId(0)));
    }

    /// 分屏入口的渲染断言：两片窗格在专注态都不画 tab 条 / 工具行 / Sysmon。
    /// 与 `pane_chrome` 的单叶断言互补——这里守的是递归入口确实走了那道门控。
    #[test]
    fn focus_mode_split_draws_only_terminals() {
        fn shapes(focus: bool) -> usize {
            let ctx = egui::Context::default();
            let pool = std::sync::Arc::new(term_session::SessionPool::new());
            let mut win = crate::ui::lifecycle_test_support::window_with(pool, &ctx, Vec::new());
            win.view.focus_mode = focus;
            let p0 = mint_pane_identity();
            let p1 = mint_pane_identity();
            win.pane_groups.clear();
            win.pane_groups.insert(p0, PaneGroup { order: Vec::new(), active: 0 });
            win.pane_groups.insert(p1, PaneGroup { order: Vec::new(), active: 0 });
            let tree = Layout::Split {
                horizontal: true,
                ratio: 500,
                first: Box::new(Layout::Leaf(p0)),
                second: Box::new(Layout::Leaf(p1)),
            };
            let mut count = 0;
            for _ in 0..2 {
                let input = egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(1280.0, 800.0),
                    )),
                    ..Default::default()
                };
                let out = ctx.run(input, |ctx| {
                    egui::CentralPanel::default().show(ctx, |ui| {
                        draw_node(&mut win, ui, &tree, 1, &mut Vec::new());
                    });
                });
                count = out.shapes.len();
            }
            count
        }
        assert!(
            shapes(true) < shapes(false),
            "专注态分屏形状数必须严格少于常态：每片窗格的 chrome 都要被压掉"
        );
    }

    /// 关掉一个窗格里显示的会话，应当**剪掉那片叶子**而不是用别的会话冒充它。
    /// 关键不变量：整条布局树上**身份唯一**——不能出现重复叶子，否则后续 split /
    /// close 只认第一个匹配，深嵌套后第二个窗格就失灵（这正是修掉的旧 bug）。
    #[test]
    fn closing_a_session_prunes_exactly_its_leaf_without_duplicating() {
        let mut l = Layout::single(PaneId(10));
        l.split(PaneId(10), true, PaneId(11));
        l.split(PaneId(11), false, PaneId(12));
        // 关掉 11：它的叶子被剪、兄弟 12 顶上来；10 与 12 身份各自唯一。
        assert!(l.close_leaf(PaneId(11)));
        assert_eq!(l.leaf_panes(), vec![PaneId(10), PaneId(12)]);
        assert_eq!(l.leaf_count(), 2);
        // 再关 10：剩单叶子 12，且身份仍唯一。
        assert!(l.close_leaf(PaneId(10)));
        assert_eq!(l, Layout::single(PaneId(12)));
        // 反复关最后一个叶子必须无操作（窗口始终留一块终端）。
        assert!(!l.close_leaf(PaneId(12)));
        assert_eq!(l, Layout::single(PaneId(12)));
    }
}

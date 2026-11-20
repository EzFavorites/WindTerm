//! V1-1 / W1-a：布局**结构**的恢复、保存，以及「激活即连」。
//!
//! `05-feature-scope.md §2.2` 的边界：恢复标签 / 分屏 / 会话引用，不恢复
//! 进程内状态（回滚内容、运行中的进程、选区）。`layout_state` 里存的是会话
//! 名字与一棵二分树，所以重启后按名字去会话库里重新查配置——用户后来改过
//! 的密码 / 分组不会被旧布局覆盖回去。
//!
//! # W1-a：摆回结构，但只在视图被激活时连
//!
//! 官方公开文档 `windterm-docs-md/2020/01/22/restore-sessions/index.md:27`：
//! 「All last sessions will be restored after restart, and the session only
//! connects to target when its view is actived」。启动时把 N 条 SSH 全连上会
//! 连着弹指纹窗与密码窗；参考实现把它们摆成断开态，切到哪条才连哪条。
//! 所以这里**不改掉断开态**，只做两件补充：
//!
//! 1. 按 `prefs.json` 的 Startup 三档决定摆回哪些标签（纯判定在
//!    `term_app::startup`，会话库过滤在这里）；
//! 2. 断开态标签登记进 `pending_activation`，每帧由
//!    [`TerminalWindow::poll_activation_connect`] 判一次，只连激活的那个。
//!
//! 保存时机：每次标签或分屏结构变化时（`close_tab` / `split_view` / 退出前）。
//! 截图基线用一次性 profile 目录（`WINDTERM_PROFILE_DIR`），那里没有
//! `layout.json`，所以恢复路径与激活连接都不会碰视觉基线。

use crate::ui::*;
use crate::ui::layout::mint_pane_identity;
use std::collections::HashMap;
use term_app::startup::{RestoreTab, StartupMode};
use term_app::startup::slot_needing_connect;
use term_core::session_runtime::SessionId;
use term_runtime::layout_state::{self, SavedLayout, SavedNode, SavedTab};
// ④-0：恢复时把「会话下标」树翻译成「窗格身份」树，并配套建 `pane_groups`。
use super::{PaneGroup, PaneId};

impl TerminalWindow {
    /// 启动时调用：按 Startup 策略把上次的标签 / 分屏结构摆回来。
    /// 会话名在库里找不到（用户删了）就跳过那一条，不凭空造会话。
    pub(in crate::ui) fn restore_layout(&mut self) {
        let Some(saved) = layout_state::load() else {
            return;
        };
        let saved_names: Vec<String> = saved.tabs.iter().map(|t| t.name.clone()).collect();
        let library: Vec<String> = self.sessions.iter().map(|s| s.name.clone()).collect();
        let wanted = resolve_restore(&self.startup.tabs_to_restore(&saved_names), &library);
        // 面板位置属于窗口结构，与「要不要恢复会话」无关：即使 NoSession，
        // 工具面板昨天停在哪儿今天还在哪儿。老文件里 dock 为空 = 保持默认。
        self.dock = DockLayout::from_saved(&saved.dock);
        if wanted.is_empty() {
            return;
        }
        // 注意：这里清空但下面可能早退（tabs 为空时不替换 self.tabs）。启动时该字段初值为空、
        // restore_layout 只调一次，因此无实际影响；移动它属另一刀。
        self.pending_activation.clear();

        let mut tabs = Vec::new();
        // 建立 name -> SavedTab 的映射，用于提取运行态字段
        let saved_by_name: std::collections::HashMap<String, &SavedTab> = saved
            .tabs
            .iter()
            .map(|t| (t.name.clone(), t))
            .collect();

        for name in &wanted {
            let Some(mut cfg) = self.sessions.iter().find(|s| &s.name == name).cloned() else {
                continue;
            };
            // N：若保存的布局里有该 tab 的 cwd，覆盖会话配置的 working_dir
            if let Some(st) = saved_by_name.get(name) {
                if let Some(cwd) = &st.cwd {
                    if !cwd.trim().is_empty() {
                        cfg.working_dir = cwd.clone();
                    }
                }
            }
            let mut tab = make_tab(
                &self.pool,
                cfg,
                egui::Id::new(format!("tab_restored_{}", tabs.len())),
                &self.ctx,
            );
            // N：写入恢复运行态
            if let Some(st) = saved_by_name.get(name) {
                tab.restore_scroll_top = st.scroll_top;
                tab.restore_history_len = st.history_len;
                // 存活会话（本地 shell / 系统 ssh）立即应用：
                // make_tab 里的 maybe_apply_restore 是 None（刚构造），这里才是真值。
                if tab.session.is_some() {
                    tab.maybe_apply_restore();
                }
                // 断开会话（内置 SSH）：字段留在 tab 上，
                // 等 reconnect 时由 reconnect_active_tab 读取并应用到新 tab。
            }
            // 内置 SSH 经 `pool.open` 起不来，恢复出来就是断开态。只登记这些：
            // 本地 shell / 系统 ssh 已经活着，再连一次是重复起进程。
            if tab.session.is_none() {
                self.pending_activation.push(tab.identity);
            }
            tabs.push(tab);
        }
        if tabs.is_empty() {
            return;
        }
        let n = tabs.len();
        self.replace_tabs(tabs);
        // ADR-0025 阶段 3：持久化存的是下标（`SavedNode::Leaf` 是「第几条」），
        // 这里换成恢复出来的标签身份（与 `tabs` 同序），让内存布局按身份寻址。
        let ids: Vec<SessionId> = self.tabs.iter().map(|t| t.identity).collect();
        // ④-1：构造期那片单叶子（`init_pane`）就是窗口的第一个 group，恢复时
        // 让第一片叶子**复用**它——否则它被丢弃，新铸的 pane 从 `B` 起，
        // group 名字就凭空少一个 `A`（参考实现四格是 A/B/C/D）。
        let init_pane = self.layout.leaf_panes().into_iter().next();
        // ④-0：恢复出「窗格身份」树，并配套建 `pane_groups`（每片窗格一个只含一条会话的 group）。
        let (tree, groups) = restore_tree_reusing(self.startup.mode, &saved.tree, &ids, init_pane);
        self.layout = tree;
        self.pane_groups = groups;
        self.set_active_slot(restore_active(self.startup.mode, saved.active, n));
        // ADR-0025 阶段 0/2：重建标签后同步恒在身份顺序；活动身份已由上行
        // `set_active_slot` 落到 `active`（`restore_active` 恒落在 `0..n`），
        // 不再从这里二次推导——那会经 `active_slot()` 读回刚写的 `active`，成自指。
        self.open_order = self.tabs.iter().map(|t| t.identity).collect();
        // 结构变了，下一次窗口属性比较会自然生效；这里只请求一帧重绘。
        self.ctx.request_repaint();
    }

    /// 整份替换标签数组：旧标签必须先经控制面关掉，再被丢掉。
    ///
    /// 构造窗口时（`ui/construct.rs:41-48`）已经起了一条活本地 shell，恢复布局是
    /// 「换一批标签」而不是「追加」——直接赋值会让旧 PTY 连同监管者线程一起泄漏。
    fn replace_tabs(&mut self, tabs: Vec<Tab>) {
        for mut old in self.tabs.drain(..) {
            old.close();
        }
        self.tabs = tabs;
    }

    /// 每帧一次：当前激活的标签若是「恢复出来、还没连过、仍是断开态」，
    /// 就在**原位**连回来。走 `reconnect_active_tab`，与用户点 Reconnect
    /// 完全同一条路径（内置 SSH 那条会保留保险库密码注入）。
    ///
    /// 放在统一的每帧钩子里，而不是给 15 处 `active_tab = …` 各自加一行：
    /// 漏一处就会出现「从某种方式切过去的标签不连」。
    pub(in crate::ui) fn poll_activation_connect(&mut self) {
        if self.pending_activation.is_empty() {
            return;
        }
        let states: Vec<RestoreTab> = self
            .tabs
            .iter()
            .map(|t| RestoreTab {
                pending: self.pending_activation.contains(&t.identity),
                disconnected: t.session.is_none(),
            })
            .collect();
        let Some(slot) = slot_needing_connect(&states, self.active_slot()) else {
            return;
        };
        // **先摘牌再连**：连接失败也不重试。否则用户来回切标签会无限弹
        // 连接对话框，比「要手动点一下 Reconnect」更糟。
        let id = self.tabs[slot].identity;
        self.pending_activation.retain(|candidate| *candidate != id);
        self.reconnect_active_tab();
    }

    /// 把当前结构写进布局库。失败不崩（可用性资产），由 `layout_state` 报 stderr。
    ///
    /// W1-a：**只有 LastSessions 档写 tabs/tree/active**。其余两档启动时并没有
    /// 读那份布局，退出时却把「此刻只有一条默认 shell」写回去，等于把上次的
    /// 会话记录抹掉——临时切到 `No session` 再切回 `Last sessions`，昨天的
    /// 标签就全没了。那两档只更新 `dock`（面板位置与会话恢复无关，仍要记住）。
    pub(in crate::ui) fn save_layout(&self) {
        let saved = if keep_tab_snapshot(self.startup.mode) {
            // ADR-0025 阶段 3：布局树在内存里按身份寻址，持久化仍存下标（身份每次
            // 运行重新铸造、不可跨运行）。给 `to_saved` 一份当前 `tabs` 的身份顺序，
            // 用它把 identity 翻回「第几条」。
            let ids: Vec<SessionId> = self.tabs.iter().map(|t| t.identity).collect();
            SavedLayout {
                tabs: self
                    .tabs
                    .iter()
                    .map(|t| SavedTab {
                        name: t.config.name.clone(),
                        // N：会话运行态 —— cwd 取自配置，scroll_top/history_len 取自视口与快照
                        cwd: (!t.config.working_dir.trim().is_empty())
                            .then(|| t.config.working_dir.clone()),
                        scroll_top: Some(t.viewport.scroll_top(t.snapshot.history as u64)),
                        history_len: Some(t.snapshot.history as u64),
                    })
                    .collect(),
                active: self.active_slot(),
                tree: to_saved(&self.layout, &ids, &self.pane_groups),
                dock: self.dock.to_saved(),
            }
        } else {
            let fallback = self
                .tabs
                .first()
                .map(|t| t.config.name.as_str())
                .unwrap_or("Local Shell");
            let mut previous = layout_state::load().unwrap_or_else(|| SavedLayout::single(fallback));
            previous.dock = self.dock.to_saved();
            previous
        };
        // L3（O 设计稿 §3d）：运行期落盘一律**盖时间戳**。下次启动靠它判断
        // 「上次的留存点是不是刚发生的」——没有这一格，三小时前的一次崩溃与
        // 三秒前的崩溃长得一模一样。
        layout_state::save_run(&saved, crate::now_secs());
    }

    /// L3（O 设计稿 §3e）：上次没走正常退出的，启动时出一行提示。
    ///
    /// 与 [`Self::restore_layout`] 分开调用：恢复标签为空（会话库被清、Startup
    /// 档位是 No session）时布局没有恢复，但「上次是崩的」这条事实仍然成立。
    ///
    /// 无论提不提示，这条事实都在这里被**消费掉**——它只对「紧接着的这一次启动」
    /// 有意义，留着会污染下一次判断。
    pub(in crate::ui) fn announce_recovery(&mut self) {
        // 夹具模式（截图 / 组件出图）**不出**这条提示。理由不是「不好看」，而是
        // 它的文案里带「N 秒前」，N = now − last_saved_at：同一套截图脚本里第一张
        // 图会写一次留存点，于是第二张起都会弹这条，而 N 取决于两张之间隔了多久
        // （实测间隔 1s 与 10s 得到**不同哈希**）。带着它就永远做不出可复现的
        // 视觉基线——2026-11-18 重冻前就是这么发现的。
        if super::super::harness::fixture_mode() {
            return;
        }
        let Some(meta) = layout_state::load_meta() else {
            return;
        };
        let now = crate::now_secs();
        let announce = meta.should_announce(now);
        layout_state::clear_exit();
        if announce {
            self.notice = Some(super::super::notice::Notice::recovered(meta.saved_at, now));
        }
    }

    /// 正常退出：给这次运行盖一个 `clean` 的戳。
    ///
    /// 只改元信息，**不重写布局**。调用点在 `on_exit` 的 `close_all_sessions()`
    /// 之后，此刻 `SavedLayout` 代表的是一份空布局——在那里图省事 `save_layout()`
    /// 等于把用户明天要恢复的标签全抹掉。
    pub(crate) fn mark_clean_exit(&self) {
        layout_state::mark_exit(layout_state::EXIT_CLEAN);
    }

    /// N：30s 周期留存（设计稿 §3c 路径 B）。
    ///
    /// 在 `draw()` 里每帧调用。夹具模式（截图 / 组件出图）下 `i.time` 被钉在
    /// 0，且 `save_layout` 会污染一次性 profile 目录——两条都该跳过，所以
    /// 夹具模式直接返回。`save_layout` 只在 `LastSessions` 档写 tabs/tree，
    /// 其它两档只更新 dock，周期留存同样沿用这条语义（不破坏另一档的布局）。
    pub(in crate::ui) fn poll_periodic_save(&mut self, ctx: &egui::Context) {
        if harness::fixture_mode() {
            return;
        }
        let now = ctx.input(|i| i.time).max(0.0);
        // 首帧 `i.time` 可能从 0 开始，这里用「距上次 ≥ 30s」判定防抖。
        if now - self.last_periodic_save >= PERIODIC_SAVE_INTERVAL_SECS {
            self.last_periodic_save = now;
            self.save_layout();
        }
    }
}

/// N：周期留存间隔（秒）。30s 对「恢复方位」足够，也远低于 egui 帧率，
/// 不构成写盘压力。设计稿 §5 开放问题 1：30s 是默认，将来可做成可配置。
const PERIODIC_SAVE_INTERVAL_SECS: f64 = 30.0;

/// 这一档要不要把当前标签结构写回布局库（纯判定，可单测）。
fn keep_tab_snapshot(mode: StartupMode) -> bool {
    mode == StartupMode::LastSessions
}

/// 策略给出的会话名 → 会话库里**真的还找得到**的那些。
///
/// 用户在设置页选完之后删掉过会话，或 `layout.json` 来自另一台机器，都会
/// 出现「点名的会话不存在」。这里静默跳过而不是凭空造一个配置（与 V1-1
/// 原行为一致），也**不改顺序、不去重**：用户真的开过两条同名会话时，
/// 恢复出来就该是两条。
fn resolve_restore(wanted: &[String], library: &[String]) -> Vec<String> {
    wanted
        .iter()
        .filter(|name| library.iter().any(|s| s == *name))
        .cloned()
        .collect()
}

/// 这次恢复用哪棵分屏树（及配套的窗格分组）。
///
/// `SpecificSessions` 必须丢掉旧树：它叶子里的下标指向**昨天**那批标签，
/// 套到今天这份会话列表上会把窗格挂到错的机器上。`from_saved` 只会把
/// 越界下标夹到 `n-1`，夹完仍然可能是错的那一台。单叶子就够——标签都在
/// 标签栏里，分屏结构由用户恢复后自己再摆。
///
/// ④-0：返回的 `Layout` 树叶子挂的是**窗格身份**（`PaneId`），`groups` 是每片
/// 窗格一个只含一条会话的 `PaneGroup`——它们一起构成「每 pane 一组 tab」的物理承载。
fn restore_tree(
    mode: StartupMode,
    saved: &SavedNode,
    ids: &[SessionId],
) -> (Layout, HashMap<PaneId, PaneGroup>) {
    restore_tree_reusing(mode, saved, ids, None)
}

/// ④-1：`reuse` 是构造期已有的第一片窗格身份；给了它，第一片叶子就不再新铸，
/// group 名字从 `A` 起连续。纯函数（只由参数决定是否复用），单测可直接驱动。
fn restore_tree_reusing(
    mode: StartupMode,
    saved: &SavedNode,
    ids: &[SessionId],
    reuse: Option<PaneId>,
) -> (Layout, HashMap<PaneId, PaneGroup>) {
    match mode {
        StartupMode::LastSessions => from_saved_reusing(saved, ids, reuse),
        StartupMode::SpecificSessions | StartupMode::NoSession => {
            // 单叶子：有现成的就复用，否则铸造一个窗格身份，配一个只含第一条会话的 group。
            let first = ids.first().copied().unwrap_or(0);
            let pane = reuse.unwrap_or_else(mint_pane_identity);
            let mut groups = HashMap::new();
            groups.insert(pane, PaneGroup { order: vec![first], active: first });
            (Layout::single(pane), groups)
        }
    }
}

/// 这次恢复默认激活哪条标签。`SpecificSessions` 从第一条开始，
/// 不去读昨天记下的 `active`（那个下标属于昨天的列表）。
fn restore_active(mode: StartupMode, saved_active: usize, n: usize) -> usize {
    match mode {
        StartupMode::LastSessions => saved_active.min(n.saturating_sub(1)),
        StartupMode::SpecificSessions | StartupMode::NoSession => 0,
    }
}

pub(in crate::ui) fn to_saved(
    layout: &Layout,
    ids: &[SessionId],
    groups: &HashMap<PaneId, PaneGroup>,
) -> SavedNode {
    match layout {
        Layout::Leaf(pane) => {
            // ④-0：叶子是窗格身份，先经 `pane_groups` 翻回会话，再定位它在 `ids` 里的下标。
            let session = groups.get(pane).map(|g| g.active).unwrap_or(0);
            SavedNode::Leaf(ids.iter().position(|x| x == &session).unwrap_or(0))
        }
        Layout::Split {
            horizontal,
            ratio,
            first,
            second,
        } => SavedNode::Split {
            horizontal: *horizontal,
            ratio: *ratio,
            first: Box::new(to_saved(first, ids, groups)),
            second: Box::new(to_saved(second, ids, groups)),
        },
    }
}

pub(in crate::ui) fn from_saved(
    node: &SavedNode,
    ids: &[SessionId],
) -> (Layout, HashMap<PaneId, PaneGroup>) {
    from_saved_reusing(node, ids, None)
}

/// ④-1：`reuse` 只在**第一片**叶子上消费一次（递归时第一子树带下去、第二子树
/// 传 `None`），保证「复用一个 + 其余新铸」，且新铸身份仍唯一不重复。
fn from_saved_reusing(
    node: &SavedNode,
    ids: &[SessionId],
    reuse: Option<PaneId>,
) -> (Layout, HashMap<PaneId, PaneGroup>) {
    match node {
        SavedNode::Leaf(i) => {
            // 下标夹进 `ids` 范围（与旧 `(*i).min(n-1)` 同口径）；空列表退化成 0。
            let idx = (*i).min(ids.len().saturating_sub(1));
            let session = ids.get(idx).copied().unwrap_or(0);
            // ④-0：每个叶子一个窗格身份，配一个只含该会话的 group。
            let pane = reuse.unwrap_or_else(mint_pane_identity);
            let mut groups = HashMap::new();
            groups.insert(pane, PaneGroup { order: vec![session], active: session });
            (Layout::Leaf(pane), groups)
        }
        SavedNode::Split {
            horizontal,
            ratio,
            first,
            second,
        } => {
            let (f, mut fg) = from_saved_reusing(first, ids, reuse);
            let (s, sg) = from_saved_reusing(second, ids, None);
            fg.extend(sg);
            (
                Layout::Split {
                    horizontal: *horizontal,
                    ratio: *ratio,
                    first: Box::new(f),
                    second: Box::new(s),
                },
                fg,
            )
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use crate::ui::layout::{pane_group_name, PaneGroup, PaneId};

    /// 造一个「窗格身份 + 只含一条会话的 group」（④-0 的落盘前形态）。
    fn group_for(id: u64) -> (PaneId, PaneGroup) {
        let p = PaneId(id);
        (p, PaneGroup { order: vec![id], active: id })
    }

    #[test]
    fn a_split_tree_round_trips_without_losing_direction() {
        let ids = [10u64, 11, 12];
        let (p10, g10) = group_for(10);
        let (p11, g11) = group_for(11);
        let (p12, g12) = group_for(12);
        let mut groups = HashMap::new();
        groups.insert(p10, g10);
        groups.insert(p11, g11);
        groups.insert(p12, g12);
        let tree = Layout::Split {
            horizontal: true,
            ratio: 500,
            first: Box::new(Layout::Leaf(p10)),
            second: Box::new(Layout::Split {
                horizontal: false,
                ratio: 500,
                first: Box::new(Layout::Leaf(p11)),
                second: Box::new(Layout::Leaf(p12)),
            }),
        };
        let saved = to_saved(&tree, &ids, &groups);
        // 方向 / 比例 / 叶子数跨保存-读取守恒（PaneId 由 from_saved 重新铸造，不比较具体值）。
        assert_eq!(
            saved,
            SavedNode::Split {
                horizontal: true,
                ratio: 500,
                first: Box::new(SavedNode::Leaf(0)),
                second: Box::new(SavedNode::Split {
                    horizontal: false,
                    ratio: 500,
                    first: Box::new(SavedNode::Leaf(1)),
                    second: Box::new(SavedNode::Leaf(2)),
                }),
            }
        );
        let (restored, g2) = from_saved(&saved, &ids);
        assert_eq!(restored.leaf_count(), 3);
        assert!(matches!(
            restored,
            Layout::Split { horizontal: true, ratio: 500, .. }
        ));
        // 再次存盘应与上次一致（盘上形态稳定）。
        assert_eq!(to_saved(&restored, &ids, &g2), saved);
    }

    #[test]
    fn out_of_range_leaf_is_clamped_not_kept() {
        let ids = [10u64, 11, 12];
        let (restored, _) = from_saved(&SavedNode::Leaf(9), &ids);
        assert!(restored.is_single());
        let (empty, _) = from_saved(&SavedNode::Leaf(9), &[]);
        assert!(empty.is_single());
    }

    #[test]
    fn a_single_leaf_is_the_default_shape() {
        let ids = [10u64];
        let (p, g) = group_for(10);
        let mut groups = HashMap::new();
        groups.insert(p, g);
        let tree = Layout::single(p);
        assert!(tree.is_single());
        let saved = to_saved(&tree, &ids, &groups);
        let (restored, g2) = from_saved(&saved, &ids);
        assert!(restored.is_single());
        assert_eq!(to_saved(&restored, &ids, &g2), saved);
    }

    /// ④-1：恢复时第一片叶子**复用**构造期的 `init_pane`，否则它凭空少一个
    /// `A`（group 名字会变成 B/C/D）。且复用的 pane 不能被新铸身份重复占掉。
    #[test]
    fn restoring_reuses_the_first_pane_identity_and_keeps_all_panes_unique() {
        let ids = [10u64, 11, 12];
        let init_pane = PaneId(1);
        let saved = SavedNode::Split {
            horizontal: false,
            ratio: 500,
            first: Box::new(SavedNode::Leaf(0)),
            second: Box::new(SavedNode::Split {
                horizontal: true,
                ratio: 500,
                first: Box::new(SavedNode::Leaf(1)),
                second: Box::new(SavedNode::Leaf(2)),
            }),
        };
        let (tree, groups) =
            restore_tree_reusing(StartupMode::LastSessions, &saved, &ids, Some(init_pane));
        let panes = tree.leaf_panes();
        assert_eq!(panes.len(), 3, "三片叶子");
        assert!(panes.contains(&init_pane), "构造期那片窗格必须被复用");
        // 每片窗格身份唯一，且 groups 的键集合与树上的叶子一一对应。
        let mut sorted = panes.clone();
        sorted.sort_by_key(|p| p.0);
        sorted.dedup();
        assert_eq!(sorted.len(), 3, "PaneId 不许重复");
        assert_eq!(groups.len(), 3);
        for p in &panes {
            assert!(groups.contains_key(p), "每片叶子都要有配套 group");
        }
        // 名字从 A 起（复用的是第一个铸造的 PaneId）。
        assert_eq!(pane_group_name(init_pane), "A");
    }

    #[test]
    fn specific_sessions_never_inherits_yesterdays_split_tree() {
        let ids = [10u64, 11, 12, 13];
        let yesterday = SavedNode::Split {
            horizontal: true,
            ratio: 500,
            first: Box::new(SavedNode::Leaf(7)),
            second: Box::new(SavedNode::Leaf(3)),
        };
        let (specific, _) = restore_tree(StartupMode::SpecificSessions, &yesterday, &ids);
        assert!(specific.is_single());
        assert_eq!(restore_active(StartupMode::SpecificSessions, 3, 4), 0);
        // 但 LastSessions 要照旧还原（夹下标，不丢结构）。
        let (last1, g1) = restore_tree(StartupMode::LastSessions, &yesterday, &ids);
        let (last2, g2) = from_saved(&yesterday, &ids);
        // 两次还原都从同一棵 saved 树出发，盘上形态必须一致。
        assert_eq!(to_saved(&last1, &ids, &g1), to_saved(&last2, &ids, &g2));
    }

    #[test]
    fn last_sessions_clamps_the_saved_active_tab() {
        let ids = [10u64, 11, 12];
        let leaf = SavedNode::Leaf(0);
        assert_eq!(restore_active(StartupMode::LastSessions, 9, 3), 2);
        assert_eq!(restore_active(StartupMode::LastSessions, 1, 3), 1);
        assert_eq!(restore_active(StartupMode::LastSessions, 1, 0), 0);
        let (no, g) = restore_tree(StartupMode::NoSession, &leaf, &ids);
        assert!(no.is_single());
        // 单窗格的活跃会话是第一条 ids（10）。
        let pane = no.leaf_panes()[0];
        assert_eq!(g[&pane].active, 10);
    }

    #[test]
    fn deleted_sessions_are_skipped_without_inventing_a_config() {
        let wanted = vec!["db".into(), "gone".into(), "web".into(), "db".into()];
        let library = vec!["shell".into(), "db".into(), "web".into()];
        assert_eq!(
            resolve_restore(&wanted, &library),
            vec!["db".to_string(), "web".to_string(), "db".to_string()],
            "删掉的名字跳过，其余保序；同名两条要照旧恢复成两条"
        );
    }

    #[test]
    fn only_last_sessions_writes_the_tab_snapshot_back() {
        // 另两档没有读那份布局，就不该覆写它：否则「临时切到 No session」会把
        // 昨天的会话记录抹掉（本轮实机验证踩到的：layout.json 被冲成 1 条）。
        assert!(keep_tab_snapshot(StartupMode::LastSessions));
        assert!(!keep_tab_snapshot(StartupMode::NoSession));
        assert!(!keep_tab_snapshot(StartupMode::SpecificSessions));
    }

    #[test]
    fn the_three_policies_reach_the_library_filter_with_the_right_names() {
        use term_app::startup::StartupPolicy;

        let saved = vec!["db".into(), "web".into()];
        let library = vec!["db".into(), "web".into(), "shell".into()];
        // No session：一条都不摆回（启动时那个默认本地 shell 不受影响）。
        let no = StartupPolicy::from_tags("none", &[]);
        assert!(resolve_restore(&no.tabs_to_restore(&saved), &library).is_empty());
        // Last sessions：与保存的顺序逐字相同。
        let last = StartupPolicy::from_tags("", &[]);
        assert_eq!(
            resolve_restore(&last.tabs_to_restore(&saved), &library),
            saved
        );
        // A specific session：只开点名的那条，与上次布局无关。
        let specific = StartupPolicy::from_tags("specific", &["gone".into(), "web".into()]);
        assert_eq!(
            resolve_restore(&specific.tabs_to_restore(&saved), &library),
            vec!["web".to_string()]
        );
    }
}

// 生命周期回归单测（整份替换标签数组前先关掉旧会话）。
#[cfg(test)]
#[path = "restore_tabs_tests.rs"]
mod replace_tabs_tests;

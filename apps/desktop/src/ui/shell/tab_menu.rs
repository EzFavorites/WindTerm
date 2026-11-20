//! ViewTab 右键菜单全套、12 色标签、Reopen Closed、会话树 12 行 tooltip。
//!
//! `closed_sessions` 之前是**预留但无读者的死字段**；这里把它接成
//! 「关掉的会话名」队列，右键菜单的 `Reopen Closed` 真的能按名字把会话开回来。
//! 颜色是投影层的控件状态（`tab_colors`，按标签下标），不是领域状态——
//! 与 `find_input` 同一类，不进 `ViewState`。

use crate::ui::*;
// Reopen 重建的是一个独立窗格（PaneId + group）。
use crate::ui::layout::PaneGroup;
use crate::ui::layout::mint_pane_identity;
// 标签右键的语义现在住在纯层，这里只发强类型 Action。
use term_app::tab::TabAction;
// 内置 SSH 是异步连接的，「连上之后落在哪儿」要跟着连接一起存着。
use crate::ui::dialogs::ssh_connect::PendingOpen;

/// 标签少了一条时，颜色表跟着挪一格。
///
/// `tab_colors` 与 `tabs` 用同一个下标（见模块文档「按标签下标」）。以前它只
/// 增不减：关掉中间那条标签后，后面每一条的颜色都**整体前移一格**——
/// 红的变成蓝的，看起来像「关一个标签把别人的颜色改了」。
///
/// 抽成不依赖 `TerminalWindow` 的自由函数，是为了能单测：这条规则只有三行，
/// 但错了以后症状离原因很远（颜色乱跳），值得钉住。
pub(in crate::ui) fn drop_color_at(colors: &mut Vec<Option<usize>>, index: usize) {
    if index < colors.len() {
        colors.remove(index);
    }
}

impl TerminalWindow {
    /// 标签上的右键菜单。收集动作，循环画完再落地，避免借着 `self.tabs` 改它。
    pub(in crate::ui) fn draw_tab_context_menu(&mut self, resp: &egui::Response, index: usize, is_active: bool) {
        // 私有 `Work` 枚举已提升为 `term_app::tab::TabAction`。
        // 右键菜单收集的是**意图**，画完统一派发；落地由 `reduce` →
        // `Effect::Tab` → `ui/effects.rs::apply_tab_action` 完成，
        // 不再在闭包外直接改 `TerminalWindow` 的字段。
        let mut work: Vec<Action> = Vec::new();
        // 文案在闭包外取好：闭包里还要借 `self.closed_sessions`，别把 `self`
        // 的另一份只读借用混进去（借用纪律）。
        let lang = self.lang();
        let tr = |en: &'static str| t(lang, en);
        let activate = tr("Activate");
        let close = tr("Close");
        let close_others = tr("Close Others");
        let close_all = tr("Close All Views");
        let reopen = tr("Reopen Closed");
        let duplicate = tr("Duplicate Session");
        let split_h = tr("Split Horizontally");
        let split_v = tr("Split Vertically");
        let copy_name = tr("Copy Session Name");
        let copy_url = tr("Copy Session URL");
        let tab_color = tr("Tab Color");
        let default_color = tr("Default");
        let color_names: Vec<&'static str> = term_core::config::TAB_COLORS
            .iter()
            .skip(1)
            .map(|(name, ..)| tr(name))
            .collect();
        let closed = self.closed_sessions.last().cloned();
        let has_closed = !self.closed_sessions.is_empty();
        // 挂在标签 Response 上：右键才出现，标签栏不会多一个可见按钮
        // （五张视觉基线靠这一点守住）。
        resp.context_menu(|ui| {
            if ui.button(activate).clicked() {
                work.push(Action::Tab(TabAction::Activate { target: Some(index) }));
                ui.close_menu();
            }
            ui.separator();
            if ui.button(close).clicked() {
                work.push(Action::Tab(TabAction::Close { target: Some(index) }));
                ui.close_menu();
            }
            if ui.button(close_others).clicked() {
                work.push(Action::Tab(TabAction::CloseOthers { target: Some(index) }));
                ui.close_menu();
            }
            if ui.button(close_all).clicked() {
                work.push(Action::Tab(TabAction::CloseAll));
                ui.close_menu();
            }
            ui.separator();
            let reopen_label = match &closed {
                Some(name) => format!("{reopen} ({name})"),
                None => reopen.to_string(),
            };
            if ui.add_enabled(has_closed, egui::Button::new(reopen_label)).clicked() {
                work.push(Action::Tab(TabAction::Reopen));
                ui.close_menu();
            }
            if ui.button(duplicate).clicked() {
                work.push(Action::Tab(TabAction::Duplicate { target: Some(index) }));
                ui.close_menu();
            }
            ui.separator();
            if ui.button(split_h).clicked() {
                work.push(Action::Tab(TabAction::Split { horizontal: true }));
                ui.close_menu();
            }
            if ui.button(split_v).clicked() {
                work.push(Action::Tab(TabAction::Split { horizontal: false }));
                ui.close_menu();
            }
            ui.separator();
            if ui.button(copy_name).clicked() {
                work.push(Action::Tab(TabAction::CopyName));
                ui.close_menu();
            }
            if ui.button(copy_url).clicked() {
                work.push(Action::Tab(TabAction::CopyUrl));
                ui.close_menu();
            }
            ui.separator();
            ui.menu_button(tab_color, |ui| {
                if ui.button(default_color).clicked() {
                    work.push(Action::Tab(TabAction::Color {
                        target: Some(index),
                        slot: None,
                    }));
                    ui.close_menu();
                }
                for (i, (_, r, g, b)) in term_core::config::TAB_COLORS.iter().enumerate().skip(1) {
                    let color = egui::Color32::from_rgb(*r, *g, *b);
                    if ui
                        .add(egui::Button::new(egui::RichText::new(color_names[i - 1]).color(color)))
                        .clicked()
                    {
                        work.push(Action::Tab(TabAction::Color {
                            target: Some(index),
                            slot: Some(i),
                        }));
                        ui.close_menu();
                    }
                }
            });
            let _ = is_active;
        });
        // 每一个 `Action` 都走 `reduce` → `Effect::Tab` → `apply_tab_action`。
        // 菜单这里一个字段都不写：改标签列表的只有投影层的落地方法。
        for action in work {
            self.dispatch_action(action);
        }
        self.save_layout();
    }

    /// 记进「最近关闭」再关。`close_tab` 是标签栏 × 与菜单共用的那条路径。
    ///
    /// 2026-11-08：先经关闭确认弹窗（标签 × / 菜单 Close 都从这走）；
    /// Yes 之后落到 `confirm_close_tab`（无拦截版）。
    /// 2026-11-10：判据收进 `confirm::should_confirm_close`——三处关闭共用一份，
    /// 「只剩一条标签仍要问」这条写在那个函数里，不在这里各写一遍。
    pub(in crate::ui) fn close_tab_with_history(&mut self, index: usize) {
        if crate::ui::dialogs::confirm::should_confirm_close(
            self.tabs.len(),
            self.confirm.suppress_session,
            self.confirm.multi_tabs,
        ) {
            self.confirm.pending = Some(crate::ui::dialogs::confirm::ConfirmClose::Tab(index));
            self.confirm.check = false;
            return;
        }
        self.confirm_close_tab(index);
    }

    /// 按名字从会话库里重新查配置——不恢复进程态（05 §2.2）。
    pub(in crate::ui) fn reopen_closed(&mut self) {
        let Some(name) = self.closed_sessions.pop() else { return };
        let Some(cfg) = self.sessions.iter().find(|s| s.name == name).cloned() else {
            return;
        };
        if let Some(i) = self.tabs.iter().position(|t| t.config.name == name) {
            self.set_active_slot(i);
            return;
        }
        // 内置 SSH 要握手，tab 只能连上之后才存在，所以布局（单窗格）
        // 也交给落地那一刻做——在这里先改了布局，失败时会留下一棵指向
        // 不存在标签的树。
        if panels::sidebar::uses_builtin_ssh(&cfg) {
            self.begin_builtin_ssh_with(cfg, None, PendingOpen::AppendSingle);
            return;
        }
        let idx = self.tabs.len();
        let tab = make_tab(
            &self.pool,
            cfg,
            egui::Id::new(format!("tab_reopen_{idx}")),
            &self.ctx,
        );
        let idx = self.add_tab(tab);
        self.set_active_slot(idx);
        // Reopen 是独立视图——清掉旧窗格，只留这一片新窗格。
        self.pane_groups.clear();
        let new_id = self.tabs[idx].identity;
        let pane = mint_pane_identity();
        self.pane_groups
            .insert(pane, PaneGroup { order: vec![new_id], active: new_id });
        self.layout = Layout::single(pane);
    }

    /// 落地方法从 `ui/effects.rs::apply_tab_action` 调用，
    /// 所以可见性要覆盖整个 `ui` 模块，而不只是本文件。
    pub(in crate::ui) fn duplicate_tab(&mut self, index: usize) {
        let Some(cfg) = self.tabs.get(index).map(|t| t.config.clone()) else { return };
        let mut copy = cfg;
        copy.name = format!("{} (copy)", copy.name);
        self.sessions.push(copy.clone());
        // 内置 SSH 的副本同样走异步连接。以前这里直接 `make_tab`，
        // 拿到的是一个「(disconnected)」空标签——点了有反应，但什么也没连上。
        if panels::sidebar::uses_builtin_ssh(&copy) {
            self.begin_builtin_ssh_with(copy, None, PendingOpen::Append);
            self.save_sessions();
            return;
        }
        let idx = self.tabs.len();
        let tab = make_tab(
            &self.pool,
            copy,
            egui::Id::new(format!("tab_dup_{idx}")),
            &self.ctx,
        );
        let idx = self.add_tab(tab);
        // 复制出来的是**新标签**，同样受 `Behaviour ▸ Focus the new tab` 管。
        self.activate_new_tab(idx);
        self.save_sessions();
    }

    pub(in crate::ui) fn set_tab_color(&mut self, index: usize, color: Option<usize>) {
        if self.tab_colors.len() <= index {
            self.tab_colors.resize(index + 1, None);
        }
        self.tab_colors[index] = color;
    }

    /// 标签标题的颜色（Default 返回 None，由 egui 用默认文本色）。
    pub(in crate::ui) fn tab_color(&self, index: usize) -> Option<egui::Color32> {
        let i = self.tab_colors.get(index).copied().flatten()?;
        let (_, r, g, b) = term_core::config::TAB_COLORS.get(i).copied()?;
        Some(egui::Color32::from_rgb(r, g, b))
    }

    /// 会话树 12 行 tooltip（13 §2.2）。数据来自 `SessionConfig::tooltip_rows`，
    /// 空值保留空白行，表格形状不随会话变。
    pub(in crate::ui) fn session_tooltip(&self, ui: &mut egui::Ui, config: &SessionConfig) {
        ui.set_min_width(240.0);
        egui::Grid::new("session_tooltip_grid").num_columns(2).show(ui, |ui| {
            for (key, value) in config.tooltip_rows() {
                ui.colored_label(egui::Color32::from_gray(150), key);
                ui.label(value);
                ui.end_row();
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn twelve_tab_colors_include_a_default() {
        // 第 0 项是「不着色」(Default)，其后 12 项取自 DESIGN_TOKENS.tab_colors 的
        // 实测色（Dark Cyan … Teal），共 13 项。
        assert_eq!(term_core::config::TAB_COLORS.len(), 13);
        assert_eq!(term_core::config::TAB_COLORS[0].0, "Default");
        // 末项必须是实测的第 12 色 Teal（0x8CB4C8）。
        assert_eq!(term_core::config::TAB_COLORS[12].0, "Teal");
        assert_eq!(term_core::config::TAB_COLORS[12].1, 0x8C);
        assert_eq!(term_core::config::TAB_COLORS[12].2, 0xB4);
        assert_eq!(term_core::config::TAB_COLORS[12].3, 0xC8);
    }

    #[test]
    fn tooltip_is_always_twelve_rows() {
        let cfg = SessionConfig::new(Protocol::Ssh);
        assert_eq!(cfg.tooltip_rows().len(), 12);
    }

    #[test]
    fn closing_a_middle_tab_shifts_the_colors_with_the_tabs() {
        // 三条标签，颜色分别是 红 / 蓝 / 绿。关掉中间的蓝，剩下两条必须还是
        // 红 绿——而不是「红 蓝」（整条前移一格，位置对不上名字）。
        let mut colors = vec![Some(1), Some(2), Some(3)];
        drop_color_at(&mut colors, 1);
        assert_eq!(colors, vec![Some(1), Some(3)]);
    }

    #[test]
    fn dropping_a_color_beyond_the_table_is_a_no_op() {
        // 关掉一条从来没上过色的标签（颜色表比标签表短）不该 panic，
        // 也不该把别人的颜色挤掉。
        let mut colors = vec![Some(1)];
        drop_color_at(&mut colors, 7);
        assert_eq!(colors, vec![Some(1)]);
        let mut empty: Vec<Option<usize>> = Vec::new();
        drop_color_at(&mut empty, 0);
        assert!(empty.is_empty());
    }
}

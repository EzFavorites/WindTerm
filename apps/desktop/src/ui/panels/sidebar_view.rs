//! 会话树的绘制（视觉层）：标题带 / 筛选 / 分组头 / 会话行 / 图标。
//!
//! 从 `sidebar.rs` 拆出（2026-11-07）。`sidebar.rs` 只留数据与落盘，
//! 这里只画——与 `quick_command_bar.rs` / `quick_layout.rs` 同一分工。
//!
//! # 对齐 WindTerm 3.x 会话管理截图
//!
//! 目标是 `Clipboard_Screenshot` 那张会话管理放大图（2× 截图，逐像素采样后
//! 换算成逻辑点）。四条**结构**结论，与 2.6.1 的差异都记在这里：
//!
//! 1. 标题带只有一行：蓝方块 + 单数 `Session` + 齿轮 + ×，**下方没有分隔线**，
//!    也没有 `＋ New Session / ＋ Group` 那一行按钮（2.6.1 才没有工具行，
//!    3.x 同样没有——入口在右键菜单、菜单栏与 `Cmd+N` 三处，不缺回程）。
//! 2. `Filter` 是**无边框**输入框，占位文字 `#444444`（比正文暗得多），
//!    它**下面**才有一条 1px `#444444` 分隔线，左右各内缩 2px。
//! 3. 分组行 = 实心三角箭头 + 青色渐变文件夹 + 等宽字体组名。
//! 4. 整块底 = `#1C1C1C`，与菜单栏 / 终端 / 状态栏同色（`CHROME_SCALE.nav`）。
//!
//! 形状一律用绘图原语（`session_icons`），**不依赖字体里的字形**：夹具字体一旦
//! 缺字，逐字节基线就会在不同机器上给出不同的图。颜色全部走 `CHROME_SCALE`
//! （灰阶表面）与 `SESSION_TREE`（图标点缀色）两处调色板，本文件不抄任何色值。

use super::sidebar_rows::{group_is_open, paint_group_header, paint_session_row, set_group_open, TREE_FONT};
use crate::ui::*;
use term_app::dock::PanelId;
use term_core::theme::CHROME_SCALE;

/// 标题带高度。实测 2× 图 42px。
const TITLE_H: f32 = 21.0;
/// 筛选行高度（含它下面那条分隔线）。实测标题带下沿到分隔线 29px。
const FILTER_H: f32 = 29.0;
/// 树行高（分组头与会话行同高）。实测视觉基线相邻两行中心距 23.5px。
const ROW_H: f32 = 24.0;
/// 分隔线到第一行之间的留白。
const TREE_GAP: f32 = 4.0;

impl TerminalWindow {
    pub(in crate::ui) fn draw_sidebar(&mut self, ui: &mut egui::Ui) {
        let s = &CHROME_SCALE;
        let text_col = crate::theme::chrome(s.text);
        let dim = text_col.gamma_multiply(0.62);

        // 底色：与菜单栏 / 终端 / 状态栏同一个 #1C1C1C。
        ui.painter()
            .rect_filled(ui.max_rect(), 0.0, crate::theme::chrome(s.nav));

        // **先把纵向间距清零**：标题带 / 筛选行 / 树行三条高度都是实测常量，
        // 中间只要掺进一次 `item_spacing.y`（全局 3），后面每一行都会整体下移，
        // 而 `allocate_exact_size` 的返回值不会告诉你这件事。
        ui.spacing_mut().item_spacing.y = 0.0;

        // 标题带由统一容器 `dock_container::dock_panel_title` 在停靠态画；
        // 浮动态的 `egui::Window` 自带标题栏，容器不会画第二条。
        // Filter 仍在本面板里：跨帧缓冲是 `session_filter`，但画法走共用函数。
        let lang = self.view.lang;
        crate::ui::layout::dock_container::draw_filter(
            ui,
            PanelId::Sidebar,
            TREE_FONT,
            lang,
            &mut self.session_filter,
        );

        ui.add_space(TREE_GAP);

        // Build the tree: every group that has a session, plus declared groups,
        // in creation order.
        let mut group_order: Vec<String> = self.groups.clone();
        for cfg in &self.sessions {
            if !group_order.contains(&cfg.group) {
                group_order.push(cfg.group.clone());
            }
        }

        let mut connect_idx = None;
        let mut edit_idx: Option<usize> = None;
        let mut delete_idx: Option<usize> = None;
        let mut duplicate_idx: Option<usize> = None;
        let mut new_group: Option<String> = None;
        let mut move_to: Option<(usize, String)> = None;
        let mut import_idx = None;
        let selected = self.selected_session;
        let next_group_no = self.groups.len() + 1;
        let filter = self.session_filter.clone();
        let groups = self.groups.clone();
        // 右键菜单的闭包里还会改 `self`（新建会话），文案一律在外面取好。
        let lang = self.lang();
        let tr = |en: &'static str| t(lang, en);
        let lbl_open = tr("Open");
        let lbl_duplicate = tr("Duplicate");
        let lbl_move = tr("Move to");
        let lbl_remove = tr("Remove");
        let lbl_new_session = tr("New Session");
        let lbl_import = tr("Import");
        let lbl_shell_sessions = tr("Shell Sessions");
        let lbl_windterm_sessions = tr("WindTerm Sessions");
        let lbl_properties = tr("Properties");
        let lbl_new_group = tr("New Group");
        let lbl_empty = tr("No session available");

        // ── 树本体：单独一层滚动 ──
        //
        // 标题带与 Filter 已经画在固定位置，它们不该跟着滚；会话多了以后的
        // 溢出交给这一层。`id_salt` 意味着这是**这一块面板自己的**滚动状态，
        // 与 `Filer` 手绘列表的那个 `scroll` 字段、以及停靠容器发给工具面板的
        // `ScrollArea` 三者互不相干。
        //
        // 滚动条藏掉有两个理由：视觉基线里这一列没有条；而且 `VisibleWhenNeeded`
        // 会在溢出时占掉 8 点宽度，`available_width` 一变，每一行的名字列都会
        // 跟着窄——那是「一多就变形」，不是滚动该有的副作用。
        egui::ScrollArea::vertical()
            .id_salt("sidebar_tree")
            .auto_shrink([false, false])
            .scroll_bar_visibility(
                egui::containers::scroll_area::ScrollBarVisibility::AlwaysHidden,
            )
            .show(ui, |ui| {
                ui.spacing_mut().item_spacing.y = 0.0;
            let mut shown = 0usize;
            for group in &group_order {
                let members: Vec<usize> = self
                    .sessions
                    .iter()
                    .enumerate()
                    .filter(|(_, cfg)| &cfg.group == group)
                    .filter(|(_, cfg)| {
                        filter.is_empty() || cfg.name.to_lowercase().contains(&filter.to_lowercase())
                    })
                    .map(|(i, _)| i)
                    .collect();
                if members.is_empty() && group != term_core::config::DEFAULT_GROUP {
                    continue;
                }
                shown += members.len();

                // 分组头：三角箭头 + 青色文件夹 + 组名（可点击折叠）。
                let open = group_is_open(ui, group);
                let (hrect, hresp) = ui.allocate_exact_size(
                    egui::vec2(ui.available_width(), ROW_H),
                    egui::Sense::click(),
                );
                paint_group_header(ui, hrect, group, open, hresp.hovered());
                if hresp.clicked() {
                    set_group_open(ui, group, !open);
                }
                if !group_is_open(ui, group) {
                    continue;
                }

                for &i in &members {
                    let (name, protocol, color, running) = {
                        let cfg = &self.sessions[i];
                        let running = self.tabs.iter().any(|t| t.config.name == cfg.name);
                        (cfg.name.clone(), cfg.protocol, cfg.color, running)
                    };
                    let (rect, resp) = ui.allocate_exact_size(
                        egui::vec2(ui.available_width(), ROW_H),
                        egui::Sense::click(),
                    );
                    paint_session_row(ui, rect, &name, protocol, color, running, selected == i, resp.hovered());

                    if resp.clicked() {
                        self.selected_session = i;
                    }
                    if resp.double_clicked() {
                        connect_idx = Some(i);
                    }
                    // SessionTreeModel::data builds the item's tooltip as an HTML
                    // table; the rows are in the order below.
                    let tip = self.sessions[i].tooltip_rows();
                    let resp = resp.on_hover_ui(|ui| {
                        egui::Grid::new(("tip", i)).num_columns(2).show(ui, |ui| {
                            for (key, value) in &tip {
                                ui.weak(*key);
                                ui.label(value);
                                ui.end_row();
                            }
                        });
                    });
                    // `menu.config` -> "Session Popup"
                    let _ = resp.context_menu(|ui| {
                        if ui.button(lbl_open).clicked() {
                            connect_idx = Some(i);
                            ui.close_menu();
                        }
                        if ui.button(lbl_duplicate).clicked() {
                            duplicate_idx = Some(i);
                            ui.close_menu();
                        }
                        ui.menu_button(lbl_move, |ui| {
                            for g in &groups {
                                if ui.button(g).clicked() {
                                    move_to = Some((i, g.clone()));
                                    ui.close_menu();
                                }
                            }
                        });
                        if ui.button(lbl_remove).clicked() {
                            delete_idx = Some(i);
                            ui.close_menu();
                        }
                        ui.separator();
                        if ui.button(lbl_new_session).clicked() {
                            self.open_new_session(Some(group.clone()));
                            ui.close_menu();
                        }
                        ui.menu_button(lbl_import, |ui| {
                            if ui.button("OpenSSH").clicked() {
                                import_idx = Some(0);
                                ui.close_menu();
                            }
                            if ui.button("PuTTY").clicked() {
                                import_idx = Some(1);
                                ui.close_menu();
                            }
                            if ui.button(lbl_shell_sessions).clicked() {
                                import_idx = Some(2);
                                ui.close_menu();
                            }
                            // WindTerm profiles/ 是迁移路径的主入口
                            //（00-summary 判断五：缺它冷启动就断）。
                            if ui.button(lbl_windterm_sessions).clicked() {
                                import_idx = Some(3);
                                ui.close_menu();
                            }
                        });
                        // `Properties` 从「就地改名」升级成
                        // 真正的属性表单——否则老会话没有入口切到
                        // 内置 SSH 客户端（改名只是它的一个字段）。
                        if ui.button(lbl_properties).clicked() {
                            edit_idx = Some(i);
                            ui.close_menu();
                        }
                        ui.separator();
                        if ui.button(lbl_new_group).clicked() {
                            new_group = Some(format!("Group {}", next_group_no));
                            ui.close_menu();
                        }
                    });
                }
            }
            if shown == 0 {
                // SessionPane::createUi sets this as the tree's
                // empty placeholder: `tr("No session available")`.
                ui.centered_and_justified(|ui| {
                    ui.label(egui::RichText::new(lbl_empty).color(dim));
                });
            }
            });

        if let Some(i) = connect_idx {
            self.selected_session = i;
            self.connect_selected_session();
        }
        if let Some(i) = edit_idx {
            // 属性表单：反向填进去，保存时覆盖 `i`（而不是新增一条）。
            self.open_session_properties(i);
        }
        if let Some(i) = duplicate_idx {
            // SessionPane::duplicate: copy the entry, re-add the
            // tree item, then `sortByColumn(0, Ascending)` and save.
            let mut copy = self.sessions[i].clone();
            copy.name = unique_name(&self.sessions, &copy.name);
            self.sessions.push(copy);
            self.sort_sessions();
        }
        if let Some((i, group)) = move_to {
            if let Some(s) = self.sessions.get_mut(i) {
                s.group = group;
            }
            self.sort_sessions();
        }
        if let Some(source) = import_idx {
            let from = ["OpenSSH", "PuTTY", "Shell Sessions", "WindTerm"][source];
            self.import_sessions(from);
        }
        if let Some(i) = delete_idx {
            self.delete_session_config(i);
        }
        if let Some(name) = new_group {
            if !self.groups.contains(&name) {
                self.groups.push(name);
            }
        }
        // `SessionPane::duplicate` and friends all finish with `saveUserSessions`.
        self.save_sessions();
    }

    /// 删除第 `i` 条会话配置，并先收口同名标签上的活会话。
    ///
    /// 标签与配置是两张表：配置删了，挂在它名下的标签必须经 `Tab::close()` 走控制面，
    /// 否则 `SessionPool` 里的 Entry 不会被 remove，PTY 与监管者线程一直留着。
    ///
    /// 取不到第 `i` 条就直接返回：旧写法 `unwrap_or_default()` 会得到一个空名字，
    /// 再用它 `retain` 会把所有空名标签一并丢掉。UI 上不可达，属防御性修正。
    pub(in crate::ui) fn delete_session_config(&mut self, i: usize) {
        let Some(name) = self.sessions.get(i).map(|s| s.name.clone()) else {
            return;
        };
        // 先 close 再丢：顺序由 `Tab::close` 内部保证（摘 tee → take → pool.close）。
        for t in self.tabs.iter_mut().filter(|t| t.config.name == name) {
            t.close();
        }
        self.tabs.retain(|t| t.config.name != name);
        // ADR-0025：摘掉同名标签后，恒在身份顺序要立刻跟着重算。
        self.sync_open_order();
        self.sessions.remove(i);
        // 属性对话框可能正指着被删掉的那一条（或它后面的某一条）：
        // 序号已经不可信，直接退化成「这是一张新会话表单」。
        self.session_dialog = SessionDialogKind::New { group: None };
        if self.selected_session >= self.sessions.len() {
            self.selected_session = self.sessions.len().saturating_sub(1);
        }
        self.set_active_slot(self.active_slot().min(self.tabs.len().saturating_sub(1)));
    }
}

// 生命周期回归单测（删除会话配置时先收口同名标签的活会话）。
#[cfg(test)]
#[path = "sidebar_view_tests.rs"]
mod lifecycle_tests;


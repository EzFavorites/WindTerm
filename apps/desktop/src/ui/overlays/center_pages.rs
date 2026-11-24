//! 工作台各页面的绘制。从 `center.rs` 拆出：那边的投影副作用与窗口编排已贴
//! 500 代码行门禁，页面清单还在长；这里只负责「这一页画什么、点了派哪个
//! Action」。所有状态仍从 `View` 只读快照取，写回一律走 `actions` / `work`。

use super::{set_input, View, Work};
use crate::ui::*;
use term_app::center::{CenterAction, CenterPage, CenterState};
use term_app::shell::ShellAction;
use term_core::theme::BUILTIN_SCHEME_NAMES;

/// 专注模式页要回显的位，全部来自 `ViewState` 已存在的字段。
/// 抽成纯值（不持借用）是为了让「回显 + 派发」能脱离 egui 单测。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct FocusFlags {
    pub(super) focus_mode: bool,
    pub(super) full_screen: bool,
    pub(super) always_on_top: bool,
    pub(super) show_status_bar: bool,
    pub(super) show_quick_bar: bool,
    pub(super) show_toolbar: bool,
    pub(super) show_sidebar: bool,
}

impl From<&View<'_>> for FocusFlags {
    fn from(view: &View<'_>) -> Self {
        Self {
            focus_mode: view.focus_mode,
            full_screen: view.full_screen,
            always_on_top: view.always_on_top,
            show_status_bar: view.show_status_bar,
            show_quick_bar: view.show_quick_bar,
            show_toolbar: view.show_toolbar,
            show_sidebar: view.show_sidebar,
        }
    }
}

/// 一行真开关：标签、当前态、点击后派发的既有命令。
#[derive(Debug)]
pub(super) struct FocusRow {
    pub(super) label: &'static str,
    pub(super) on: bool,
    pub(super) action: Action,
}

/// 窗口模式三行：`focus_mode` 压 UI，`full_screen` / `always_on_top` 落视口。
pub(super) fn window_mode_rows(f: FocusFlags) -> Vec<FocusRow> {
    vec![
        FocusRow {
            label: "专注模式",
            on: f.focus_mode,
            action: Action::Shell(ShellAction::ToggleFocusMode),
        },
        FocusRow {
            label: "全屏",
            on: f.full_screen,
            action: Action::Shell(ShellAction::ToggleFullScreen),
        },
        FocusRow {
            label: "窗口置顶",
            on: f.always_on_top,
            action: Action::Shell(ShellAction::ToggleAlwaysOnTop),
        },
    ]
}

/// 专注模式压掉的 chrome 四行。每行对应一条既有 `Window.Toggle*` 命令，
/// 且回显的是 `ViewState` 里那个字段——专注模式本身不改变这些字段，
/// 所以退出专注模式后这些开关是什么样，界面就回到什么样。
pub(super) fn chrome_rows(f: FocusFlags) -> Vec<FocusRow> {
    vec![
        FocusRow {
            label: "状态栏",
            on: f.show_status_bar,
            action: Action::Shell(ShellAction::ToggleStatusBar),
        },
        FocusRow {
            label: "快捷命令栏",
            on: f.show_quick_bar,
            action: Action::Shell(ShellAction::ToggleQuickBar),
        },
        FocusRow {
            label: "侧边会话树",
            on: f.show_sidebar,
            action: Action::Shell(ShellAction::ToggleSidebar),
        },
        FocusRow {
            label: "窗格工具行",
            on: f.show_toolbar,
            action: Action::Shell(ShellAction::ToggleToolbar),
        },
    ]
}

#[cfg(test)]
#[path = "center_focus_tests.rs"]
mod focus_tests;

fn draw_focus_rows(ui: &mut egui::Ui, rows: Vec<FocusRow>, actions: &mut Vec<Action>) {
    for row in rows {
        let text = format!("{}：{}", row.label, if row.on { "开" } else { "关" });
        if ui.selectable_label(row.on, text).clicked() {
            actions.push(row.action);
        }
    }
}

pub(super) fn draw_page(
    view: &View<'_>,
    ui: &mut egui::Ui,
    state: &CenterState,
    input: &mut String,
    actions: &mut Vec<Action>,
    work: &mut Vec<Work>,
) {
    match state.page {
        CenterPage::OpenSession => {
            ui.label(format!("会话库共 {} 条", view.sessions.len()));
            if ui.text_edit_singleline(input).changed() {
                set_input(actions, input);
            }
            for (i, session) in term_app::center::filtered_sessions(&view.sessions, input) {
                if ui
                    .selectable_label(i == state.selected, &session.name)
                    .clicked()
                {
                    actions.push(Action::Center(CenterAction::Select(i)));
                }
            }
            if ui.button("连接选中会话").clicked() {
                work.push(Work::Connect(state.selected));
            }
            if state.confirm_delete {
                ui.separator();
                let name = view
                    .sessions
                    .get(state.selected)
                    .map(|s| s.name.clone())
                    .unwrap_or_default();
                ui.label(format!("确认删除「{name}」？此操作不可撤销"));
                if ui.button("确认删除").clicked() {
                    work.push(Work::DeleteSession(state.selected));
                }
                if ui.button("取消").clicked() {
                    actions.push(Action::Center(CenterAction::CancelDeleteSession));
                }
            } else if ui.button("删除选中会话").clicked() {
                actions.push(Action::Center(CenterAction::ConfirmDeleteSession));
            }

            // 会话管理器：编辑 / 复制 / 移动分组 / 新建，对齐 WindTerm 会话树右键。
            ui.separator();
            ui.label("会话操作");
            let has_selection = view.sessions.get(state.selected).is_some();
            if ui
                .add_enabled(has_selection, egui::Button::new("编辑选中会话"))
                .clicked()
            {
                work.push(Work::EditSession(state.selected));
            }
            if ui
                .add_enabled(has_selection, egui::Button::new("复制选中会话"))
                .clicked()
            {
                work.push(Work::DuplicateSession(state.selected));
            }
            // 移动到分组：下拉选目标 + 应用按钮（目标为空时禁用）。
            ui.horizontal(|ui| {
                let mut target = state.move_group.clone();
                let preview = if target.is_empty() {
                    "选择分组".to_string()
                } else {
                    target.clone()
                };
                egui::ComboBox::from_id_salt("session_move_group")
                    .selected_text(preview)
                    .show_ui(ui, |ui| {
                        for g in view.groups {
                            if ui
                                .selectable_label(*g == target, g)
                                .clicked()
                            {
                                target = g.to_string();
                                actions.push(Action::Center(CenterAction::SetMoveGroup(g.to_string())));
                            }
                        }
                    });
                if ui
                    .add_enabled(has_selection && !target.is_empty(), egui::Button::new("应用"))
                    .clicked()
                {
                    work.push(Work::MoveToGroup(state.selected, target));
                }
            });
            if ui.button("＋ 新建会话").clicked() {
                work.push(Work::NewSession);
            }
            if ui.button("＋ 新建分组").clicked() {
                work.push(Work::NewGroup);
            }
        }
        CenterPage::Save | CenterPage::Profile => {
            ui.label(format!(
                "{} 个分组 / {} 条会话",
                view.groups.len(),
                view.sessions.len()
            ));
            if let Some(session) = view.sessions.get(state.selected) {
                ui.label(format!("选中：{} · {:?}", session.name, session.protocol));
                if state.page == CenterPage::Profile {
                    ui.label(format!("描述：{}", session.description));
                }
            }
            if ui.button("保存到会话库").clicked() {
                work.push(Work::SaveSessions);
            }
        }
        CenterPage::SaveSelectionAs => {
            ui.label(format!("当前选区：{} 字符", view.selection_text.len()));
            let response = ui.add(
                egui::TextEdit::singleline(input).hint_text("目标文件路径"),
            );
            if response.changed() {
                set_input(actions, input);
            }
            if ui.button("写入文件").clicked() {
                work.push(Work::SaveSelection(input.clone()));
            }
        }
        CenterPage::SessionSettings => {
            if let Some(tab) = view.tabs.get(view.active_tab) {
                ui.label(format!("名称：{}", tab.config.name));
                ui.label(format!("协议：{:?}", tab.config.protocol));
                ui.label(format!("主机：{}:{}", tab.config.host, tab.config.port));
                ui.label(format!("TERM：{}", tab.config.term));
                let response = ui.add(egui::TextEdit::singleline(input).hint_text("新会话名"));
                if response.changed() {
                    set_input(actions, input);
                }
                if ui.button("应用改名").clicked() {
                    work.push(Work::Rename(input.clone()));
                }
            } else {
                ui.label("没有活动会话");
            }
        }
        CenterPage::DefaultSettings => {
            ui.label(format!(
                "默认 TERM：{}",
                term_core::config::TERM_TYPES.get(view.default_term).copied().unwrap_or("?")
            ));
            ui.label(format!("默认字号：{} pt", view.default_font_size));
            if ui.button("恢复出厂 TERM").clicked() {
                work.push(Work::ResetDefaults);
            }
        }
        CenterPage::ToolbarList => {
            for (i, tool) in view.tools.iter().enumerate() {
                let hidden = state.hidden_tools.get(i).copied().unwrap_or(false);
                ui.label(format!(
                    "{} {} {}{}",
                    i + 1,
                    tool.icon,
                    tool.tooltip,
                    if hidden { "（已隐藏）" } else { "" }
                ));
            }
        }
        CenterPage::ToolbarCustomize => {
            ui.label("点击切换某个快捷按钮的显示");
            for (i, tool) in view.tools.iter().enumerate() {
                let hidden = state.hidden_tools.get(i).copied().unwrap_or(false);
                let label = format!("{} {} · {}", tool.icon, tool.tooltip, if hidden { "隐藏" } else { "显示" });
                if ui.button(label).clicked() {
                    actions.push(Action::Center(CenterAction::ToggleTool(i)));
                }
            }
        }
        // 这一页画的是「把一行字当 shell 命令发给当前终端」，与新的主机
        // 地址栏（`ui/address/`，输入 `user@host:22` 连过去）是两件事，所以
        // 页面改叫 `Command Line`（`Window.GotoCommandLine`）。这里只改名字，
        // 命令输入框本身一行不动。
        CenterPage::GotoCommandLine => {
            let response = ui.add(
                egui::TextEdit::singleline(input).hint_text("输入命令行后回车发送"),
            );
            if response.changed() {
                set_input(actions, input);
            }
            let enter = ui.input(|i| i.key_pressed(egui::Key::Enter));
            if ui.button("发送").clicked() || (response.has_focus() && enter) {
                let cmd = input.trim().to_string();
                if !cmd.is_empty() {
                    work.push(Work::Send(cmd));
                }
            }
            ui.separator();
            ui.label("历史（点击重发）");
            for cmd in &state.history {
                if ui.button(cmd).clicked() {
                    work.push(Work::Send(cmd.clone()));
                }
            }
        }
        CenterPage::CompleteHistory => {
            if state.history.is_empty() {
                ui.label("还没有发过命令（命令行页会记录）");
            }
            for cmd in &state.history {
                if ui.button(cmd).clicked() {
                    work.push(Work::Send(cmd.clone()));
                }
            }
        }
        CenterPage::SchemeList => {
            for (i, name) in BUILTIN_SCHEME_NAMES.iter().enumerate() {
                let active = i == view.scheme_idx;
                let label = if active {
                    format!("✓ {name}")
                } else {
                    (*name).to_string()
                };
                if ui.selectable_label(active, label).clicked() {
                    work.push(Work::SetScheme(i));
                }
            }
            ui.label(format!(
                "当前配色方案：{}",
                BUILTIN_SCHEME_NAMES[view.scheme_idx]
            ));
        }
        CenterPage::WindowList => {
            for (i, tab) in view.tabs.iter().enumerate() {
                let mark = if i == view.active_tab { "· 活动" } else { "" };
                if ui
                    .selectable_label(i == view.active_tab, format!("{} {}", tab.config.name, mark))
                    .clicked()
                {
                    work.push(Work::Activate(i));
                }
            }
        }
        CenterPage::WindowManager => {
            for (i, tab) in view.tabs.iter().enumerate() {
                ui.horizontal(|ui| {
                    ui.label(&tab.config.name);
                    if ui.small_button("切到").clicked() {
                        work.push(Work::Activate(i));
                    }
                    if ui.small_button("关闭").clicked() {
                        work.push(Work::Close(i));
                    }
                });
            }
        }
        CenterPage::FocusModeSettings => {
            let flags = FocusFlags::from(view);
            ui.label("窗口模式（点击切换，当前状态实时回显）");
            draw_focus_rows(ui, window_mode_rows(flags), actions);
            ui.separator();
            ui.label("专注模式会压掉下面这些部件；开关决定退出专注模式后是否显示");
            draw_focus_rows(ui, chrome_rows(flags), actions);
            ui.separator();
            // 标签栏没有 show_tab_bar 字段，也就没有独立命令：单窗格时由专注
            // 模式整条压掉，分屏时每格各自画。不能为它造一个没人读的假开关。
            ui.label(format!(
                "标签栏：{}（无独立开关，由专注模式压掉）",
                if flags.focus_mode { "隐藏" } else { "显示" }
            ));
            ui.label(
                "左右 / 底部面板同样由专注模式整组压掉；平时另受 \
                 Window.ToggleLeftDocks / ToggleRightDocks / ToggleBottomDocks 控制。",
            );
        }
        CenterPage::HighlightText => {
            ui.label(format!("当前选区：{} 字符", view.selection_text.len()));
            if ui.button("把选区加入常驻高亮").clicked() {
                actions.push(Action::Center(CenterAction::AddHighlight(
                    view.selection_text.to_string(),
                )));
            }
            ui.separator();
            for (i, word) in state.highlights.iter().enumerate() {
                ui.horizontal(|ui| {
                    ui.label(word);
                    if ui.small_button("×").clicked() {
                        actions.push(Action::Center(CenterAction::RemoveHighlight(i)));
                    }
                });
            }
        }
    }
}

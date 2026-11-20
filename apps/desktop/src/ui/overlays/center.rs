//! R14-f：v0.1 工作台——15 条命令共用一个居中对话框，每个页面有真实内容/动作。
//!
//! 与命令面板、Goto 面板同款受控契约（R4）：`ViewState.center` 只读，正在敲的字
//! 落在投影层的 `center_input` 缓冲，改动以 `SetInput` 派发回 reducer。
//!
//! 副作用不在这里直接做：绘制时只把意图收进 `Vec<Work>`，面板画完后由
//! `apply_work` 统一执行（连接 / 保存 / 切标签 / 切配色 / 写文件 / 发命令）。
//! 这样绘制期间只持有 `&TerminalWindow`，不会和状态借用打架；唯一需要改
//! `ViewState` 的专注模式走 `Action::Shell(ToggleFocusMode)` 回 reducer。
//!
//! 页面与 v0.1 范围的对应关系见 `docs/design/13-ui-shell.md`。

use crate::ui::harness::AnchoredWindow;
use crate::ui::*;
use term_app::center::{CenterAction, CenterPage, CenterState};
use term_app::shell::ShellAction;
use term_core::theme::{builtin_scheme, BUILTIN_SCHEME_NAMES, Palette};

/// 绘制用的只读快照：按字段名把需要的数据借出来，避免「整个 self 不可变借用」
/// 和 `center_input` 的可变借用冲突。
struct View<'a> {
    sessions: &'a [SessionConfig],
    groups: &'a [String],
    tabs: &'a [Tab],
    tools: &'a [ToolButton],
    selection_text: &'a str,
    theme_name: &'a str,
    scheme_idx: usize,
    focus_mode: bool,
    default_term: usize,
    default_font_size: f32,
    active_tab: usize,
}

/// 一次投影层副作用意图。绘制时收集，画完统一落地。
enum Work {
    Connect(usize),
    SaveSessions,
    SaveSelection(String),
    Rename(String),
    ResetDefaults,
    Send(String),
    Activate(usize),
    Close(usize),
    SetScheme(usize),
    DeleteSession(usize),
    /// 打开选中会话的属性表单（复用 New Session 对话框）。
    EditSession(usize),
    /// 复制选中会话（自动去重命名）。
    DuplicateSession(usize),
    /// 把选中会话移动到某个分组。
    MoveToGroup(usize, String),
    /// 打开新建会话表单。
    NewSession,
    /// 在会话库里新建一个分组。
    NewGroup,
}

impl TerminalWindow {
    pub(in crate::ui) fn draw_center(&mut self, ctx: &egui::Context) {
        if !self.view.center.visible {
            return;
        }
        let mut actions: Vec<Action> = Vec::new();
        let mut work: Vec<Work> = Vec::new();
        {
            let view = View {
                sessions: &self.sessions,
                groups: &self.groups,
                tabs: &self.tabs,
                tools: &self.tools,
                selection_text: &self.selection_text,
                theme_name: &self.palette.theme_name,
                scheme_idx: self.view.center.scheme_idx,
                focus_mode: self.view.focus_mode,
                default_term: self.new_session.term,
                default_font_size: self.settings.font_size,
                active_tab: self.active_slot(),
            };
            // 状态只读、缓冲可变：都是不相交字段，借用互不干扰。
            let state = &self.view.center;
            let input = &mut self.center_input;
            draw_panel(&view, ctx, state, input, &mut actions, &mut work);
        }
        for action in actions {
            self.dispatch_action(action);
        }
        for item in work {
            self.apply_work(item);
        }
    }

    fn apply_work(&mut self, work: Work) {
        match work {
            Work::Connect(i) => {
                if i < self.sessions.len() {
                    self.selected_session = i;
                    self.connect_selected_session();
                    self.note("已发起连接");
                }
            }
            Work::SaveSessions => {
                term_runtime::persist::save(&self.groups, &self.sessions);
                self.note("会话库已保存");
            }
            Work::DeleteSession(i) => {
                if i < self.sessions.len() {
                    let name = self.sessions[i].name.clone();
                    self.sessions.remove(i);
                    term_runtime::persist::save(&self.groups, &self.sessions);
                    self.view.center.confirm_delete = false;
                    let max = self.sessions.len().saturating_sub(1);
                    self.view.center.selected = self.view.center.selected.min(max);
                    self.selected_session = self.selected_session.min(max);
                    self.note(&format!("已删除会话：{name}"));
                }
            }
            Work::EditSession(i) => {
                if i < self.sessions.len() {
                    // 复用 New Session 对话框的属性模式：填进去、保存时覆盖原条目。
                    // 该对话框在 center 之前绘制，关掉 center 它才浮到最上层。
                    self.open_session_properties(i);
                    self.view.center.visible = false;
                }
            }
            Work::DuplicateSession(i) => {
                if i < self.sessions.len() {
                    let name = self.sessions[i].name.clone();
                    let mut copy = self.sessions[i].clone();
                    copy.name = unique_name(&self.sessions, &copy.name);
                    self.sessions.push(copy);
                    self.sort_sessions();
                    self.save_sessions();
                    let max = self.sessions.len().saturating_sub(1);
                    self.view.center.selected = self.view.center.selected.min(max);
                    self.note(&format!("已复制「{name}」"));
                }
            }
            Work::MoveToGroup(i, group) => {
                if i < self.sessions.len() && !group.is_empty() {
                    let name = self.sessions[i].name.clone();
                    self.sessions[i].group = group.clone();
                    self.sort_sessions();
                    self.save_sessions();
                    self.note(&format!("已将「{name}」移动到「{group}」"));
                }
            }
            Work::NewSession => {
                self.open_new_session(None);
                self.view.center.visible = false;
            }
            Work::NewGroup => {
                let name = format!("Group {}", self.groups.len() + 1);
                if !self.groups.contains(&name) {
                    self.groups.push(name.clone());
                    self.save_sessions();
                }
                self.note(&format!("已新建分组：{name}"));
            }
            Work::SaveSelection(path) => {
                let result = std::fs::write(path.trim(), &self.selection_text)
                    .map(|_| "选区已写入文件".to_string())
                    .unwrap_or_else(|e| format!("写入失败：{e}"));
                self.note(&result);
            }
            Work::Rename(name) => {
                let name = name.trim().to_string();
                if !name.is_empty() {
                    let active = self.active_slot();
                    if let Some(tab) = self.tabs.get_mut(active) {
                        tab.config.name = name.clone();
                    }
                    if let Some(session) = self.sessions.get_mut(self.selected_session) {
                        session.name = name;
                    }
                    self.note("会话名已更新（点保存写入会话库）");
                }
            }
            Work::ResetDefaults => {
                self.new_session.term = term_core::config::TERM_TYPES
                    .iter()
                    .position(|t| *t == term_core::config::DEFAULT_TERM)
                    .unwrap_or(0);
                self.note("默认 TERM 已恢复出厂值");
            }
            Work::Send(cmd) => {
                self.send_snippet(&cmd);
                self.dispatch_action(Action::Center(CenterAction::Record(cmd)));
            }
            Work::Activate(i) => {
                if i < self.tabs.len() {
                    self.set_active_slot(i);
                }
            }
            Work::Close(i) => self.close_tab(i),
            Work::SetScheme(i) => {
                if let Some(palette) = scheme(i) {
                    self.view.center.scheme_idx = i;
                    self.palette = palette;
                    self.note("配色已切换");
                }
            }
        }
    }

    fn note(&mut self, text: &str) {
        self.dispatch_action(Action::Center(CenterAction::SetNote(text.to_string())));
    }
}

/// WindTerm 内置 7 套终端配色：cmd / dumb / linux / macos / none / powershell / routeros。
/// 索引对应 `term_core::theme::BUILTIN_SCHEME_NAMES`。
fn scheme(idx: usize) -> Option<Palette> {
    let name = *BUILTIN_SCHEME_NAMES.get(idx)?;
    builtin_scheme(name)
}

fn set_input(actions: &mut Vec<Action>, input: &str) {
    actions.push(Action::Center(CenterAction::SetInput(input.to_string())));
}

fn draw_panel(
    view: &View<'_>,
    ctx: &egui::Context,
    state: &CenterState,
    input: &mut String,
    actions: &mut Vec<Action>,
    work: &mut Vec<Work>,
) {
    if input != &state.input {
        input.clear();
        input.push_str(&state.input);
    }
    if ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
        actions.push(Action::Center(CenterAction::Close));
        return;
    }

    // 稳定 id `center`：十几个页面共用这一个浮层，标题换了不该换窗口状态。
    // 限高 460：命令中心的部分页面（窗口列表 / 工具栏定制）列长了会把窗口
    // 顶出屏幕，超出的部分进滚动区。
    AnchoredWindow::new("center", state.page.title(), 460.0)
        .with_max_height(460.0)
        .show(ctx, |ui| {
        ui.set_min_width(420.0);
        if let Some(note) = &state.note {
            ui.label(note);
            ui.separator();
        }
        draw_page(view, ui, state, input, actions, work);
        ui.separator();
        if ui.button("关闭").clicked() {
            actions.push(Action::Center(CenterAction::Close));
        }
    
    });
}

fn draw_page(
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
            for (i, session) in view.sessions.iter().enumerate() {
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
        // D3：这一页画的是「把一行字当 shell 命令发给当前终端」，与新的主机
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
            ui.label(if view.focus_mode {
                "专注模式：开（压掉侧栏 / 状态栏 / 标签栏）"
            } else {
                "专注模式：关"
            });
            if ui.button("切换专注模式").clicked() {
                actions.push(Action::Shell(ShellAction::ToggleFocusMode));
            }
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

#[cfg(test)]
#[path = "center_tests.rs"]
mod tests;

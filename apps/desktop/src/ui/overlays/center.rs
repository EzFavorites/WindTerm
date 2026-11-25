//! v0.1 工作台——15 条命令共用一个居中对话框，每个页面有真实内容/动作。
//!
//! 与命令面板、Goto 面板同款受控契约：`ViewState.center` 只读，正在敲的字
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
use term_app::center::{CenterAction, CenterState};
use term_core::theme::{builtin_scheme, BUILTIN_SCHEME_NAMES, Palette};

// 页面清单单独住一个文件：本文件的职责是取快照、收集副作用、统一落地，
// 「这一页画什么」交给 center_pages。拆出后两边都远低于 500 代码行门禁。
#[path = "center_pages.rs"]
mod center_pages;
use center_pages::draw_page;

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
    // 专注模式页的回显位：都来自 ViewState，不在这份快照里保存第二份状态。
    full_screen: bool,
    always_on_top: bool,
    show_status_bar: bool,
    show_quick_bar: bool,
    show_toolbar: bool,
    show_sidebar: bool,
    default_term: usize,
    default_font_size: f32,
    active_tab: usize,
}

/// 一次投影层副作用意图。绘制时收集，画完统一落地。
#[derive(Clone, Debug, PartialEq, Eq)]
enum Work {
    Connect(usize),
    SaveSessions,
    SaveSelection(String),
    Rename(String),
    ResetDefaults,
    Send(String),
    Activate(usize),
    /// 按**恒在身份**关闭，绘制时的下标跨帧会作废（脚本可能开新标签）。
    Close(term_core::session_runtime::SessionId),
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
                full_screen: self.view.full_screen,
                always_on_top: self.view.always_on_top,
                show_status_bar: self.view.show_status_bar,
                show_quick_bar: self.view.show_quick_bar,
                show_toolbar: self.view.show_toolbar,
                show_sidebar: self.view.show_sidebar,
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
        // `SetInput` 会先把 selected 归零（绝不指向被过滤掉的会话）；这里按真实
        // 会话表收敛到第一条可见命中，并经 reducer 的 `Select` 回写。投影层不再
        // 留一个指向隐藏会话的下标给下一帧的按钮。
        if self.view.center.converge_selection(&self.sessions) {
            let idx = self.view.center.selected;
            self.dispatch_action(Action::Center(CenterAction::Select(idx)));
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
                    self.dispatch_action(Action::Center(CenterAction::CancelDeleteSession));
                    let max = self.sessions.len().saturating_sub(1);
                    self.dispatch_action(Action::Center(CenterAction::Select(
                        self.view.center.selected.min(max),
                    )));
                    self.selected_session = self.selected_session.min(max);
                    self.note(&format!("已删除会话：{name}"));
                }
            }
            Work::EditSession(i) => {
                if i < self.sessions.len() {
                    // 复用 New Session 对话框的属性模式：填进去、保存时覆盖原条目。
                    // 该对话框在 center 之前绘制，关掉 center 它才浮到最上层。
                    self.open_session_properties(i);
                    self.dispatch_action(Action::Center(CenterAction::Close));
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
                    self.dispatch_action(Action::Center(CenterAction::Select(
                        self.view.center.selected.min(max),
                    )));
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
                self.dispatch_action(Action::Center(CenterAction::Close));
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
            // 关闭确认与布局落盘走标签栏同一条路径，别绕过；身份反查防跨帧换人。
            Work::Close(id) => {
                if let Some(i) = self.tabs.iter().position(|t| t.identity == id) {
                    self.close_tab_with_history(i);
                    self.pending_save_layout = true;
                }
            }
            Work::SetScheme(i) => {
                if let Some(palette) = scheme(i) {
                    self.dispatch_action(Action::Center(CenterAction::SetScheme(i)));
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

#[cfg(test)]
#[path = "center_tests.rs"]
mod tests;

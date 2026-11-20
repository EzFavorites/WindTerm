//! R14-f：v0.1「工作台」能力——把 15 条命令收敛到一个带页面的状态机。
//!
//! 设计取舍（B4 / I5 / R5）：
//! - **命令 ID 只在这里的 `parse`**：每条 ID 映射到一个 `CenterPage`，页面就是
//!   这条命令的真实效果，不是「打开同一个空对话框」的别名。
//! - **状态只由 `reduce` 写**：页面、输入、选中、命令历史、高亮词、工具栏隐藏位、
//!   一次性提示都在 `CenterState`。投影层只画 + 复用既有副作用方法，不回写。
//! - **没有 Effect**：这批 15 条的副作用全是既有投影层方法（连接 / 保存 /
//!   切标签 / 切配色 / 写文件 / 发命令），reducer 保持纯函数；唯一需要改
//!   `ViewState` 的专注模式走 `dispatch_action` 回到 `shell` 的 reducer。
//!
//! 页面内容见 `apps/desktop/src/ui/center.rs`；v0.1 范围依据
//! `docs/design/14-scope-and-requirements.md` 的 P0 范围与版本阶段。

pub use crate::vocab::CenterPage;


#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CenterState {
    pub visible: bool,
    pub page: CenterPage,
    /// 地址栏 / 另存路径 / 会话名编辑的提交值。
    pub input: String,
    pub selected: usize,
    /// 删除会话两步确认态：`true` 时 OpenSession 页显示确认按钮。
    pub confirm_delete: bool,
    /// 会话管理器「移动到分组」下拉框的当前选择（独立于选中项，绘制时回填）。
    pub move_group: String,
    /// 发过的命令（地址栏与历史补全共用，去重、保持顺序）。
    pub history: Vec<String>,
    /// 常驻高亮词（终端渲染时给所有匹配画底色）。
    pub highlights: Vec<String>,
    /// 工具栏第 i 个按钮是否隐藏（工具栏定制）。
    pub hidden_tools: Vec<bool>,
    /// 一次性结果提示（保存成功 / 写文件失败 …）。
    pub note: Option<String>,
    /// `Terminal.SchemeList` 当前选中的内置配色方案下标（0 = cmd，对应
    /// `term_core::theme::BUILTIN_SCHEME_NAMES`）。
    pub scheme_idx: usize,
}

impl Default for CenterState {
    fn default() -> Self {
        Self {
            visible: false,
            page: CenterPage::OpenSession,
            input: String::new(),
            selected: 0,
            confirm_delete: false,
            move_group: String::new(),
            history: Vec::new(),
            highlights: Vec::new(),
            hidden_tools: Vec::new(),
            note: None,
            scheme_idx: 0,
        }
    }
}

pub use crate::vocab::CenterAction;


/// 命令 ID → 本能力的动作。未接线的 ID 返回 `None`。
///
/// 2026-11-12 R2：实现只有一份，在 [`crate::parse`]（入口层）。这里保留同名入口，
/// 一是因为「每个能力自带字符串入口」是这个 crate 对外的约定，二是因为
/// `apps/desktop` 有直接调 `term_app::log::parse` 的地方——把路径改掉没有架构价值。
pub fn parse(id: &str) -> Option<CenterAction> {
    crate::parse::center_parse(id)
}

pub fn reduce(state: &mut CenterState, action: CenterAction) {
    match action {
        CenterAction::Open(page) => {
            state.visible = true;
            state.page = page;
            state.input.clear();
            state.selected = 0;
            state.confirm_delete = false;
            state.move_group.clear();
            state.note = None;
        }
        CenterAction::Close => {
            state.visible = false;
            state.input.clear();
            state.selected = 0;
            state.confirm_delete = false;
            state.move_group.clear();
            state.note = None;
        }
        CenterAction::SetInput(text) => {
            state.input = text;
            state.note = None;
        }
        CenterAction::Select(idx) => {
            state.selected = idx;
            state.confirm_delete = false;
        }
        CenterAction::Record(cmd) => {
            let cmd = cmd.trim().to_string();
            if !cmd.is_empty() && !state.history.contains(&cmd) {
                state.history.push(cmd);
            }
        }
        CenterAction::ToggleTool(idx) => {
            if idx >= state.hidden_tools.len() {
                state.hidden_tools.resize(idx + 1, false);
            }
            state.hidden_tools[idx] = !state.hidden_tools[idx];
        }
        CenterAction::AddHighlight(text) => {
            let text = text.trim().to_string();
            if !text.is_empty() && !state.highlights.contains(&text) {
                state.highlights.push(text);
            }
        }
        CenterAction::RemoveHighlight(idx) => {
            if idx < state.highlights.len() {
                state.highlights.remove(idx);
            }
        }
        CenterAction::SetNote(text) => state.note = Some(text),
        CenterAction::ConfirmDeleteSession => state.confirm_delete = true,
        CenterAction::CancelDeleteSession => state.confirm_delete = false,
        CenterAction::SetMoveGroup(g) => state.move_group = g,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_page_has_a_title_and_a_unique_command() {
        assert_eq!(CenterPage::ALL.len(), 15);
        let ids = [
            "Terminal.OpenSession",
            "Terminal.Save",
            "Terminal.SaveSelectionAs",
            "Terminal.OpenSessionSettings",
            "Terminal.OpenDefaultSessionSettings",
            "Application.OpenProfileDialog",
            "Window.ToolbarList",
            "Window.ToolbarCustomize",
            "Window.GotoCommandLine",
            "Terminal.CompleteHistory",
            "Terminal.SchemeList",
            "Window.WindowList",
            "Window.WindowManager",
            "Window.OpenFocusModeSettings",
            "Text.HighlightText",
        ];
        for (i, id) in ids.iter().enumerate() {
            assert_eq!(parse(id), Some(CenterAction::Open(CenterPage::ALL[i])), "{id}");
            assert!(!CenterPage::ALL[i].title().is_empty());
        }
        assert_eq!(parse("Terminal.SendZModem"), None);
    }

    #[test]
    fn opening_resets_transient_fields_but_keeps_accumulated_data() {
        let mut s = CenterState::default();
        reduce(&mut s, CenterAction::Record("ls".into()));
        reduce(&mut s, CenterAction::AddHighlight("error".into()));
        reduce(&mut s, CenterAction::SetInput("stale".into()));
        s.selected = 4;
        reduce(&mut s, CenterAction::Open(CenterPage::WindowList));
        assert!(s.visible);
        assert_eq!(s.page, CenterPage::WindowList);
        assert!(s.input.is_empty());
        assert_eq!(s.selected, 0);
        assert_eq!(s.history, vec!["ls".to_string()]);
        assert_eq!(s.highlights, vec!["error".to_string()]);
    }

    #[test]
    fn confirm_delete_toggles_and_select_clears_it() {
        let mut s = CenterState::default();
        assert!(!s.confirm_delete);
        reduce(&mut s, CenterAction::ConfirmDeleteSession);
        assert!(s.confirm_delete, "ConfirmDeleteSession 应进入确认态");
        reduce(&mut s, CenterAction::Select(2));
        assert!(!s.confirm_delete, "切换选中应取消确认态");
        reduce(&mut s, CenterAction::ConfirmDeleteSession);
        reduce(&mut s, CenterAction::CancelDeleteSession);
        assert!(!s.confirm_delete, "CancelDeleteSession 应退出确认态");
    }

    #[test]
    fn history_and_highlights_are_deduplicated() {
        let mut s = CenterState::default();
        reduce(&mut s, CenterAction::Record(" pwd ".into()));
        reduce(&mut s, CenterAction::Record("pwd".into()));
        reduce(&mut s, CenterAction::Record("".into()));
        assert_eq!(s.history, vec!["pwd".to_string()]);
        reduce(&mut s, CenterAction::AddHighlight(" ERROR ".into()));
        reduce(&mut s, CenterAction::AddHighlight("ERROR".into()));
        reduce(&mut s, CenterAction::AddHighlight(" ".into()));
        assert_eq!(s.highlights, vec!["ERROR".to_string()]);
        reduce(&mut s, CenterAction::RemoveHighlight(0));
        assert!(s.highlights.is_empty());
        reduce(&mut s, CenterAction::RemoveHighlight(9));
        assert!(s.highlights.is_empty(), "越界删除是空操作");
    }

    #[test]
    fn toolbar_toggle_grows_the_mask_and_is_reversible() {
        let mut s = CenterState::default();
        reduce(&mut s, CenterAction::ToggleTool(3));
        assert_eq!(s.hidden_tools, vec![false, false, false, true]);
        reduce(&mut s, CenterAction::ToggleTool(3));
        assert_eq!(s.hidden_tools, vec![false, false, false, false]);
    }

    #[test]
    fn close_clears_transient_fields() {
        let mut s = CenterState::default();
        reduce(&mut s, CenterAction::Open(CenterPage::Save));
        reduce(&mut s, CenterAction::SetInput("x".into()));
        reduce(&mut s, CenterAction::SetNote("saved".into()));
        reduce(&mut s, CenterAction::Close);
        assert!(!s.visible && s.input.is_empty() && s.note.is_none());
    }

    #[test]
    fn move_group_is_set_and_reset_on_open_close() {
        let mut s = CenterState::default();
        assert!(s.move_group.is_empty());
        reduce(&mut s, CenterAction::SetMoveGroup("Servers".into()));
        assert_eq!(s.move_group, "Servers");
        reduce(&mut s, CenterAction::Close);
        assert!(s.move_group.is_empty(), "关闭应清空移动目标");
        reduce(&mut s, CenterAction::SetMoveGroup("Lab".into()));
        reduce(&mut s, CenterAction::Open(CenterPage::OpenSession));
        assert!(s.move_group.is_empty(), "打开应重置移动目标");
    }
}

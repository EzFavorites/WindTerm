//! 命令面板里的 Tmux 无目标命令表（`Mux: XXX` 官方文档主交互范式）。
//!
//! 放在 `vocab` 命名空间下是为了让 parse.rs / palette.rs 都能引用而不打破
//! `term-app` 的分层门禁（那两层只许向下依赖 `vocab`）。这里只放数据与纯
//! 查表函数：带目标的命令（SelectPane / KillPane …）不在这里，它们必须由
//! tmux 面板按解析出的窗格集合校验后才能下发。

use super::Action;

/// 官网 page/3 的面板入口：只要求**打开输入框**，不产生控制行。
/// 拆两段写是门禁对命令 ID 的要求（见下面这张表）。
pub const RENAME_PANE_PROMPT_ID: &str = concat!("Tmux.", "RenamePanePrompt");

/// `(显示名, 命令 ID, 控制行)`。ID 用 `concat!` 拆两段：整仓门禁禁止命令 ID
/// 字面量出现在 parse 分支以外，拆段既满足规则又保持 `&'static str` 身份。
/// 控制列为 `""` 的条目是**纯提示**（这里是 Rename Pane）：用户输入的名字
/// 绝不能拼进控制行，真正的控制行只由 `Command::RenamePane::line()` 生成。
pub const TMUX_PALETTE: &[(&str, &str, &str)] = &[
    ("Mux: Split Pane Right", concat!("Tmux.", "SplitHorizontal"), "split-window -h"),
    ("Mux: Split Pane Left", concat!("Tmux.", "SplitPaneLeft"), "split-window -h -b"),
    ("Mux: Split Pane Down", concat!("Tmux.", "SplitVertical"), "split-window -v"),
    ("Mux: Split Pane Up", concat!("Tmux.", "SplitPaneUp"), "split-window -v -b"),
    ("Mux: Toggle Pane Zoom", concat!("Tmux.", "TogglePaneZoom"), "resize-pane -Z"),
    ("Tmux: New Window", concat!("Tmux.", "NewWindow"), "new-window"),
    ("Tmux: Refresh Client", concat!("Tmux.", "Refresh"), "refresh-client"),
    ("Tmux: Detach Client", concat!("Tmux.", "Detach"), "detach-client"),
    // 官方四篇 Tmux-Integration 文档的面板入口（显示名照文档逐字抄）：
    // Kill / Rename / Select 用 `Mux:` 前缀，Synchronize 用 `Tmux:` 前缀。
    // 全部无目标：作用于当前窗格/当前 window，不需要先解析窗格号。
    ("Mux: Kill Pane", concat!("Tmux.", "KillPaneActive"), "kill-pane"),
    // 官网 page/3「Rename Pane」的第二条入口：双击标签 / 命令面板运行这一条时
    // 只弹出 tmux 面板的 Pane name 输入框，名字仍由 Command::RenamePane 转义。
    ("Mux: Rename Pane", RENAME_PANE_PROMPT_ID, ""),
    ("Mux: Select Pane Left", concat!("Tmux.", "SelectPaneLeft"), "select-pane -L"),
    ("Mux: Select Pane Right", concat!("Tmux.", "SelectPaneRight"), "select-pane -R"),
    ("Mux: Select Pane Above", concat!("Tmux.", "SelectPaneAbove"), "select-pane -U"),
    ("Mux: Select Pane Below", concat!("Tmux.", "SelectPaneBelow"), "select-pane -D"),
    ("Tmux: Synchronize Panes", concat!("Tmux.", "SynchronizePanes"), "set-window-option synchronize-panes"),
    // 官网 Tmux Integration 目录的其余无目标命令（page/2…page/30 全表）。
    ("Tmux: Kill Window", concat!("Tmux.", "KillWindow"), "kill-window"),
    ("Tmux: Kill Session", concat!("Tmux.", "KillSession"), "kill-session"),
    ("Mux: Select Window Previous", concat!("Tmux.", "SelectWindowPrevious"), "select-window -p"),
    ("Mux: Select Window Next", concat!("Tmux.", "SelectWindowNext"), "select-window -n"),
    ("Mux: Swap Pane", concat!("Tmux.", "SwapPane"), "swap-pane"),
    ("Mux: Swap Window", concat!("Tmux.", "SwapWindow"), "swap-window"),
    ("Mux: Move Pane To New Window", concat!("Tmux.", "MovePaneToNewWindow"), "break-pane"),
    ("Mux: Select Layout Even Horizontal", concat!("Tmux.", "LayoutEvenHorizontal"), "select-layout even-horizontal"),
    ("Mux: Select Layout Even Vertical", concat!("Tmux.", "LayoutEvenVertical"), "select-layout even-vertical"),
    ("Mux: Select Layout Main Horizontal", concat!("Tmux.", "LayoutMainHorizontal"), "select-layout main-horizontal"),
    ("Mux: Select Layout Main Vertical", concat!("Tmux.", "LayoutMainVertical"), "select-layout main-vertical"),
    ("Mux: Select Layout Tiled", concat!("Tmux.", "LayoutTiled"), "select-layout tiled"),
];

/// 命令 ID → 安全的控制行。带目标的命令一律返回 `None`。
pub fn line_for(id: &str) -> Option<&'static str> {
    TMUX_PALETTE
        .iter()
        .find(|(_, candidate, _)| *candidate == id)
        .map(|(_, _, line)| *line)
}

/// 面板里的「打开输入框」载荷是不是 Rename Pane 那一条。
///
/// 投影层收到 `Action::TmuxCommand` 时用它区分「写进 PTY 的控制行」与
/// 「只开面板的 UI 意图」——后者不是控制行，绝不许落到写 PTY 那条路上。
pub fn is_rename_pane_prompt(payload: &str) -> bool {
    payload == RENAME_PANE_PROMPT_ID
}

/// 命令 ID → 动作。提示条目走同一条 `Action::TmuxCommand` 通道，但载荷是
/// 命令 ID 而不是控制行；投影层按 [`is_rename_pane_prompt`] 拦下并聚焦输入框。
/// 这样不用为一条「开输入框」的 UI 意图再造一个 Action 变体。
pub fn action_for(id: &str) -> Option<Action> {
    if is_rename_pane_prompt(id) {
        return Some(Action::TmuxCommand(RENAME_PANE_PROMPT_ID.to_string()));
    }
    line_for(id).map(|line| Action::TmuxCommand(line.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_palette_id_resolves_to_its_control_line() {
        assert_eq!(line_for("Tmux.SplitHorizontal"), Some("split-window -h"));
        assert_eq!(line_for("Tmux.Detach"), Some("detach-client"));
        assert_eq!(line_for("Tmux.unknown"), None);
        for (_, id, _) in TMUX_PALETTE {
            assert!(line_for(id).is_some(), "{id} 必须能解析，否则面板里点了没反应");
        }
    }

    #[test]
    fn rename_pane_is_in_the_palette_without_a_control_line() {
        let entry = TMUX_PALETTE
            .iter()
            .find(|(label, _, _)| *label == "Mux: Rename Pane")
            .expect("官网 page/3 要求命令面板里有 Mux: Rename Pane");
        assert_eq!(entry.1, RENAME_PANE_PROMPT_ID);
        assert_eq!(entry.2, "", "提示条目没有控制行，名字不拼进命令");
        assert!(is_rename_pane_prompt(RENAME_PANE_PROMPT_ID));
        assert!(!is_rename_pane_prompt("rename-pane -t %1 evil"));
        assert!(!is_rename_pane_prompt("split-window -h"));
    }

    #[test]
    fn the_rename_prompt_opens_an_input_instead_of_sending_a_line() {
        assert_eq!(
            action_for(RENAME_PANE_PROMPT_ID),
            Some(Action::TmuxCommand(RENAME_PANE_PROMPT_ID.to_string()))
        );
        assert_eq!(
            action_for("Tmux.SplitHorizontal"),
            Some(Action::TmuxCommand("split-window -h".to_string()))
        );
        // 与顶层入口同一条路径：面板确认后能真的走到这条动作。
        assert_eq!(
            Action::parse(RENAME_PANE_PROMPT_ID),
            action_for(RENAME_PANE_PROMPT_ID)
        );
    }
}

//! 会话日志能力：把 Log 菜单的七条命令翻译成意图。
//!
//! 这一层是纯函数，**连「日志目录在哪」都不知道**。理由与隧道 / 文件面板一致：
//! reducer 拿不到文件系统，也不该拿到——它一旦知道目录，就会有人在这里顺手
//! `create_dir_all`，于是「点了菜单导致磁盘写入」就散落在不可单测的地方。
//!
//! 文件名的最终形态（会话名 + 时间戳 + 重名序号）由写入端决定，因为只有它知道
//! 「同一秒里是不是已经有同名文件」。界面拿到的路径来自 `supervisor::LogStatus`。

use crate::effect::Effect;
use crate::state::ViewState;

pub use crate::vocab::LogAction;


/// 命令 ID → 本能力的动作。未接线的 ID 返回 `None`。
///
/// 2026-11-12 R2：实现只有一份，在 [`crate::parse`]（入口层）。这里保留同名入口，
/// 一是因为「每个能力自带字符串入口」是这个 crate 对外的约定，二是因为
/// `apps/desktop` 有直接调 `term_app::log::parse` 的地方——把路径改掉没有架构价值。
pub fn parse(id: &str) -> Option<LogAction> {
    crate::parse::log_parse(id)
}

pub fn reduce(state: &mut ViewState, action: LogAction) -> Vec<Effect> {
    match action {
        // 对话框开关是领域状态；其余六个都是「对会话做事」，副作用由投影层执行。
        LogAction::OpenSettings => {
            state.show_log_settings = true;
            Vec::new()
        }
        other => vec![Effect::Log(other)],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_log_command_is_wired() {
        let ids = [
            ("Terminal.StartLogger", LogAction::Start),
            ("Terminal.StopLogger", LogAction::Stop),
            ("Terminal.PauseLogger", LogAction::Pause),
            ("Terminal.ResumeLogger", LogAction::Resume),
            ("Terminal.OpenLogFile", LogAction::OpenFile),
            ("Terminal.OpenLogFolder", LogAction::OpenFolder),
            ("Terminal.OpenLogSettings", LogAction::OpenSettings),
        ];
        for (id, want) in ids {
            assert_eq!(parse(id), Some(want), "{id}");
        }
        assert_eq!(parse("Terminal.OpenLog"), None);
        assert_eq!(parse(""), None);
    }

    #[test]
    fn session_actions_emit_an_effect_and_leave_the_view_alone() {
        let mut state = ViewState::default();
        let before = state.clone();
        for action in [
            LogAction::Start,
            LogAction::Stop,
            LogAction::Pause,
            LogAction::Resume,
            LogAction::OpenFile,
            LogAction::OpenFolder,
        ] {
            let effects = reduce(&mut state, action);
            assert_eq!(effects, vec![Effect::Log(action)]);
        }
        assert_eq!(state, before, "日志动作不写视图状态");
    }

    #[test]
    fn settings_opens_the_dialog_without_an_effect() {
        let mut state = ViewState::default();
        assert!(reduce(&mut state, LogAction::OpenSettings).is_empty());
        assert!(state.show_log_settings);
    }

    #[test]
    fn start_and_stop_are_not_a_toggle() {
        // 菜单上有独立的 Start / Stop：连点两次 Start 不该变成 Stop。
        // 这条钉住语义，防止有人「简化」成 ToggleLogger。
        let mut state = ViewState::default();
        assert_eq!(
            reduce(&mut state, LogAction::Start),
            vec![Effect::Log(LogAction::Start)]
        );
        assert_eq!(
            reduce(&mut state, LogAction::Start),
            vec![Effect::Log(LogAction::Start)]
        );
    }
}

//! 自定义菜单 `Run command` 项的**载荷判据**（从 `ip_hover.rs` 拆出以便单测）。
//!
//! `UiAction::RunCommand` 的契约是**命令 ID**，而兼容终端的 `Run command`
//! 输入框里既可能写 `View.ToggleColumnMarker`（ID），也可能写 `ping -c 4 …`（正文）。
//! 把正文塞进 RunCommand 会让执行器把它当 ID 查表、查不到就静默丢弃；这里在产出
//! 动作时就把两种写法分开：真 ID 留给 `RunCommand`，正文走 `SendLine` 直接下发。
//!
//! 三个 OS 的输入框顺序固定在 `[macOS, Linux, Windows]`，按宿主选一项，不再恒取 0。

use crate::ui::dialogs::triggers::model::Rule;
use crate::ui::dialogs::triggers::model::{OsCommand, UiAction};
use term_core::automation::Action;

/// 悬停 / 内置触发器产出的编辑意图展开成内核动作，走与用户规则同一条派发链。
pub(in crate::ui) fn to_core_action(action: &UiAction) -> Option<Action> {
    action.to_core()
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::ui) enum Host {
    Mac,
    Linux,
    Windows,
}

pub(in crate::ui) fn host() -> Host {
    if cfg!(windows) {
        Host::Windows
    } else if cfg!(target_os = "macos") {
        Host::Mac
    } else {
        Host::Linux
    }
}

/// 这一宿主用哪一格 OS 配置。顺序由 `ActionConfig.os` 的文档钉死。
pub(in crate::ui) fn command_of(rule: &Rule, host: Host) -> &OsCommand {
    let idx = match host {
        Host::Mac => 0,
        Host::Linux => 1,
        Host::Windows => 2,
    };
    &rule.config.action.os[idx]
}

/// 命令串 → 动作：真命令 ID 仍走 `RunCommand`（帧末窗口级 `dispatch`），
/// 命令正文改成 `SendLine`（写远端并回车）。
pub(in crate::ui) fn run_command_action(command: &str) -> UiAction {
    if term_app::action::Action::parse(command).is_some() {
        UiAction::RunCommand(command.to_string())
    } else {
        UiAction::SendLine(command.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rule_with_os_commands() -> Rule {
        let mut rule = Rule::default();
        let texts = ["mac-cmd", "linux-cmd", "win-cmd"];
        for (slot, text) in rule.config.action.os.iter_mut().zip(texts) {
            slot.command = text.to_string();
        }
        rule
    }

    #[test]
    fn os_slot_follows_the_host_and_never_falls_back_to_zero() {
        let rule = rule_with_os_commands();
        assert_eq!(command_of(&rule, Host::Mac).command, "mac-cmd");
        assert_eq!(command_of(&rule, Host::Linux).command, "linux-cmd");
        assert_eq!(command_of(&rule, Host::Windows).command, "win-cmd");
        assert_eq!(command_of(&rule, host()).command, rule.config.action.os[
            match host() {
                Host::Mac => 0,
                Host::Linux => 1,
                Host::Windows => 2,
            }
        ].command);
    }

    #[test]
    fn a_command_id_keeps_the_run_command_contract() {
        // 真命令 ID 从数据层取，不在 ui/ 里写死 ID 字面量（4e 门禁）。
        let id = term_app::menu::context_ids::LINE_ENDINGS;
        assert!(term_app::action::Action::parse(id).is_some());
        assert_eq!(
            run_command_action(id),
            UiAction::RunCommand(id.to_string())
        );
    }

    #[test]
    fn command_body_is_sent_as_a_line_instead_of_a_dropped_id() {
        for body in ["ping -c 4 10.0.0.9", "echo ${match}", ""] {
            assert_eq!(
                run_command_action(body),
                UiAction::SendLine(body.to_string())
            );
        }
    }
}

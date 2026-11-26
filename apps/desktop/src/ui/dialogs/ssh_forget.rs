//! `known_hosts` 的 forget 生产入口：Tofu 冲突对话框里的「忘记这台主机」。
//!
//! [`crate::ui::dialogs::ssh_connect::Connector`] 的判定 / 状态机仍住在
//! `ssh_connect.rs`，本文件只放三件事：
//!
//! 1. **纯判据**：二次确认之后走哪条路（`confirm_decision`）、结果怎么变成
//!    给用户看的话（`feedback` / `missing_path_feedback`）。它们不碰 IO，
//!    所以「取消不调用 forget」「失败不谎报成功」是单测问题，不是靠肉眼看按钮。
//! 2. **派发**：[`Connector`] 上的 `begin_forget` / `forget_decision` /
//!    `reconnect_after_forget`。按钮只把「按了什么」送进纯判据，再派发到
//!    取消或真正改账本的两条路。路径统一经
//!    `Connector::known_hosts_path`（内部就是 `SshOptions::resolved_known_hosts`），
//!    拿不到就降级成文案，不 panic、不自己拼路径。
//! 3. **三个窗口的绘制**：从 `ssh.rs` 搬来，那里已贴 500 代码行上限；
//!    三个步骤共用 `DialogId::SshReplaceKey` 一个 id，不新增弹窗规格，
//!    也不新增命令 ID（入口只是既有连接流程上的一个按钮）。
//!
//! 删除的粒度完全交给 [`term_ssh::known_hosts_admin::forget`]：`host_token`
//! 只把精确 `host:port` 写进匹配键，`filter_out` 只丢命中的那一行，
//! `*.example.com` / 另一个端口 / `@revoked` 行都原样留着。

use crate::ui::dialogs::ssh::action_button;
use crate::ui::dialogs::ssh_connect::{
    Connector, SshAction, Step, NO_PATH_PREFIX,
};
use crate::ui::kit::window::{DialogId, DialogWindow};
use crate::ui::*;

/// 路径解析失败时的降级文案。
///
/// 与 `ssh_connect::accept_host_key` 的 `known_hosts_path()` 失败分支同口径：
/// 共用 [`NO_PATH_PREFIX`]，只在尾部说明这次做不了的是「忘掉」。
pub(in crate::ui) const NO_PATH_MESSAGE: &str =
    "找不到 known_hosts 的位置，无法忘掉这台主机";

/// 二次确认之后该做什么。纯函数——「取消绝不调用 forget」由它和
/// [`Connector::forget_decision`] 一起钉住。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::ui) enum ConfirmDecision {
    Cancel,
    RunForget,
}

/// 把用户是否按了确认翻译成动作。UI 只做派发，不在这里碰账本。
pub(in crate::ui) fn confirm_decision(confirmed: bool) -> ConfirmDecision {
    if confirmed {
        ConfirmDecision::RunForget
    } else {
        ConfirmDecision::Cancel
    }
}

/// 给用户看的结果。`ok` 与 `message` 必须一致：失败的那句话里
/// 绝不能出现「已忘掉」这类成功字样。
#[derive(Clone, Debug, PartialEq, Eq)]
pub(in crate::ui) struct ForgetFeedback {
    pub(in crate::ui) ok: bool,
    pub(in crate::ui) message: String,
}

/// `forget()` 的结果 → 可见反馈。纯函数，UI 只搬字符串。
///
/// `Ok(0)` 也算调用成功，但账本里本来就没有那条记录，要照实说，
/// 不能让用户以为删掉了什么。
pub(in crate::ui) fn feedback(
    host: &str,
    port: u16,
    outcome: Result<usize, String>,
) -> ForgetFeedback {
    match outcome {
        Ok(0) => ForgetFeedback {
            ok: true,
            message: format!("{host}:{port} 没有已记录的钥匙，无需忘掉。"),
        },
        Ok(n) => ForgetFeedback {
            ok: true,
            message: format!(
                "已忘掉 {host}:{port} 的 {n} 条 known_hosts 记录；下次连接会重新核对指纹。"
            ),
        },
        Err(e) => ForgetFeedback {
            ok: false,
            message: format!("忘掉 {host}:{port} 失败：{e}"),
        },
    }
}

/// 路径缺失时的反馈。与 `feedback` 的失败态同形状，UI 不必分支。
pub(in crate::ui) fn missing_path_feedback() -> ForgetFeedback {
    ForgetFeedback {
        ok: false,
        message: NO_PATH_MESSAGE.to_string(),
    }
}

impl Connector {
    /// Tofu 冲突对话框里点了「Forget this host」：只进二次确认，**不碰账本**。
    pub(in crate::ui) fn begin_forget(&mut self) {
        let Step::ReplaceKey { changed } = &self.step else {
            return;
        };
        self.step = Step::ConfirmForget {
            changed: changed.clone(),
        };
    }

    /// 二次确认界面唯一的入口：把「按了什么」交给纯判据，UI 只派发。
    ///
    /// 取消走 [`Self::cancel_forget`]（不碰账本），确认才走 [`Self::run_forget`]。
    pub(in crate::ui) fn forget_decision(&mut self, confirmed: bool) {
        match confirm_decision(confirmed) {
            ConfirmDecision::Cancel => self.cancel_forget(),
            ConfirmDecision::RunForget => self.run_forget(),
        }
    }

    /// 二次确认里点了「Cancel」：回到冲突对话框，没有结果态 = 没有调过 forget。
    fn cancel_forget(&mut self) {
        if let Step::ConfirmForget { changed } = &self.step {
            self.step = Step::ReplaceKey {
                changed: changed.clone(),
            };
        }
    }

    /// 二次确认里点了「Forget」：真正改账本。
    ///
    /// `known_hosts_path()` 是 `SshOptions::resolved_known_hosts` 的唯一入口，
    /// 与校验 / 换钥读的是同一份账本；拿不到时给降级文案而不是 panic。
    fn run_forget(&mut self) {
        let Step::ConfirmForget { changed } = &self.step else {
            return;
        };
        let changed = changed.clone();
        let Some(path) = self.known_hosts_path() else {
            self.step = Step::ForgetResult {
                changed,
                feedback: missing_path_feedback(),
            };
            return;
        };
        let outcome = term_ssh::known_hosts_admin::forget(&path, &changed.host, changed.port)
            .map_err(|e| e.to_string());
        self.step = Step::ForgetResult {
            feedback: feedback(&changed.host, changed.port, outcome),
            changed,
        };
    }

    /// 结果页点「Reconnect」：账本已改，按 Strict 重连一次重新核对指纹。
    pub(in crate::ui) fn reconnect_after_forget(&mut self, ctx: &egui::Context) {
        if let Some(auth) = self.auth.clone() {
            self.launch(auth, ctx);
        }
    }
}

/// 画「Tofu 冲突 / 二次确认 / 结果」三个互斥步骤。
///
/// 三个都复用 `DialogId::SshReplaceKey` 的窗口与键盘契约（Esc 取消、
/// Enter 不确认），既不新增弹窗规格，也避免误触安全确认。
pub(in crate::ui) fn draw(
    connector: &Connector,
    title: &str,
    ctx: &egui::Context,
    action: &mut SshAction,
) {
    match &connector.step {
        Step::ReplaceKey { changed } => {
            DialogWindow::new(DialogId::SshReplaceKey, title).show(ctx, |ui| {
                ui.heading("Host key changed");
                ui.label(format!(
                    "The key recorded for {}:{} (known_hosts line {}) is not the one \
                     this server just offered. Either the machine was reinstalled, \
                     or this connection is not going where you think it is.",
                    changed.host, changed.port, changed.line
                ));
                ui.add_space(4.0);
                ui.monospace(changed.fingerprint.clone());
                ui.add_space(6.0);
                // 这一段是这个窗口存在的理由：说清「现在什么都没写」和
                // 「什么情况下才该替换」。不写「是否继续」那种问句。
                ui.label(
                    "Nothing has been written yet. Replace the record only if you \
                     got this fingerprint from the server's administrator, or you \
                     know the machine was reinstalled. The old line is deleted.",
                );
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    if action_button(ui, "Cancel") {
                        *action = SshAction::Cancel;
                    }
                    if action_button(ui, "Replace stored key") {
                        *action = SshAction::ReplaceKey;
                    }
                    if action_button(ui, "Forget this host") {
                        *action = SshAction::Forget;
                    }
                });
            });
        }
        Step::ConfirmForget { changed } => {
            DialogWindow::new(DialogId::SshReplaceKey, title).show(ctx, |ui| {
                ui.heading("Forget this host?");
                ui.label(format!(
                    "This deletes the recorded key for {}:{} (known_hosts line {}) from the ledger.",
                    changed.host, changed.port, changed.line
                ));
                ui.add_space(6.0);
                ui.label(
                    "This is a security-sensitive operation: the pinned fingerprint will be \
                     gone, and the next connection will ask you to verify the server's key \
                     again. Only continue if you are sure this is the right machine.",
                );
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    if action_button(ui, "Cancel") {
                        *action = SshAction::CancelForget;
                    }
                    if action_button(ui, "Forget this host") {
                        *action = SshAction::ConfirmForget;
                    }
                });
            });
        }
        Step::ForgetResult { feedback, .. } => {
            DialogWindow::new(DialogId::SshReplaceKey, title).show(ctx, |ui| {
                ui.heading(if feedback.ok {
                    "Host forgotten"
                } else {
                    "Could not forget host"
                });
                ui.label(&feedback.message);
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    if feedback.ok && action_button(ui, "Reconnect") {
                        *action = SshAction::ReconnectAfterForget;
                    }
                    if action_button(ui, "Close") {
                        *action = SshAction::Cancel;
                    }
                });
            });
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use term_core::session::Protocol;

    fn changed() -> crate::ui::dialogs::ssh_connect::Replacement {
        crate::ui::dialogs::ssh_connect::Replacement {
            host: "example.com".to_string(),
            port: 2222,
            line: 12,
            key_line: "ssh-ed25519 AAAA".to_string(),
            fingerprint: "SHA256:old".to_string(),
        }
    }

    /// 二次确认的判据是纯函数：取消绝不进 forget。
    #[test]
    fn cancelling_never_runs_forget() {
        assert_eq!(confirm_decision(false), ConfirmDecision::Cancel);
        assert_eq!(confirm_decision(true), ConfirmDecision::RunForget);
    }

    /// 按「取消」只退回冲突对话框，不产生结果态——有结果态就等于调过 forget。
    #[test]
    fn cancelling_the_second_confirmation_does_not_touch_the_ledger() {
        let c = changed();
        let mut connector = Connector::new(
            SessionConfig::new(Protocol::Ssh),
            Step::ConfirmForget { changed: c.clone() },
        );
        connector.forget_decision(false);
        assert_eq!(connector.step, Step::ReplaceKey { changed: c });
        assert!(
            !matches!(connector.step, Step::ForgetResult { .. }),
            "取消不该产生结果反馈——那等于调用了 forget"
        );
    }

    /// 确认后一定走到结果态（与取消形成对照）：这条路径里才会调 `forget()`。
    #[test]
    fn confirming_runs_forget_and_lands_on_a_result_state() {
        let mut connector = Connector::new(
            SessionConfig::new(Protocol::Ssh),
            Step::ConfirmForget { changed: changed() },
        );
        connector.forget_decision(true);
        assert!(
            matches!(connector.step, Step::ForgetResult { .. }),
            "确认后必须调用 forget 并落结果态"
        );
    }

    /// 确认后从冲突步骤进入二次确认，此时仍未改账本。
    #[test]
    fn the_first_button_only_opens_the_second_confirmation() {
        let mut connector = Connector::new(
            SessionConfig::new(Protocol::Ssh),
            Step::ReplaceKey { changed: changed() },
        );
        connector.begin_forget();
        assert!(matches!(connector.step, Step::ConfirmForget { .. }));
        assert!(!matches!(connector.step, Step::ForgetResult { .. }));
    }

    #[test]
    fn a_successful_forget_shows_a_success_message() {
        let f = feedback("example.com", 2222, Ok(1));
        assert!(f.ok, "{f:?}");
        assert!(f.message.contains("example.com:2222"), "{}", f.message);
        assert!(f.message.contains("已忘掉"), "{}", f.message);
    }

    /// Err 的文案必须是失败态，且绝不能混进成功字样。
    #[test]
    fn an_error_is_shown_as_a_failure_and_not_as_a_success() {
        let f = feedback("example.com", 22, Err("写不了临时文件".to_string()));
        assert!(!f.ok, "{f:?}");
        assert!(f.message.contains("写不了临时文件"), "{}", f.message);
        assert!(
            !f.message.contains("已忘掉"),
            "失败不能谎报成功：{}",
            f.message
        );
    }

    /// 没命中任何记录时调用仍成功，但文案要照实说，不能假装删了东西。
    #[test]
    fn no_match_is_reported_honestly() {
        let f = feedback("example.com", 22, Ok(0));
        assert!(f.ok, "{f:?}");
        assert!(f.message.contains("没有已记录"), "{}", f.message);
        assert!(!f.message.contains("已忘掉 1 条"), "{}", f.message);
    }

    /// 路径缺失走降级文案，与 `accept_host_key` 同口径（共用前缀）。
    #[test]
    fn a_missing_path_degrades_with_the_same_wording_as_host_key_acceptance() {
        let f = missing_path_feedback();
        assert!(!f.ok, "{f:?}");
        assert!(f.message.starts_with(NO_PATH_PREFIX), "{}", f.message);
        assert!(
            f.message.ends_with("无法忘掉这台主机"),
            "{}",
            f.message
        );
    }
}

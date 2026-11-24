//! 弹窗**行为层**原语：生命周期 + 键盘语义。
//!
//! 绘制原语（`window` / `footer`）只回答「长什么样」；这里回答「Esc / Enter
//! 意味着什么」。此前每个弹窗各写一遍 `ctx.input(key_pressed(...))`，于是同一个
//! Esc 在 A 弹窗是「放弃草稿」、在 B 弹窗被终端当成字节发到了远端。把语义
//! 收在 [`DialogLifecycle`] + [`DialogKeys`] 一处，调用点只声明自己是哪一种。

use eframe::egui;

pub(in crate::ui) use super::spec::DialogLifecycle;

/// 一次按键轮询的结果。调用方决定真正怎么改状态；这里只回答「按了什么」。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(in crate::ui) struct KeyOutcome {
    /// 次按钮（取消）被要求执行。
    pub cancel: bool,
    /// 默认按钮（确认）被要求执行。
    pub confirm: bool,
    /// 快捷键 `Y`（关闭确认弹窗的 Yes）被按下。
    pub yes: bool,
}

/// 弹窗的键盘契约。
#[derive(Clone, Copy, Debug)]
pub(in crate::ui) struct DialogKeys {
    pub lifecycle: DialogLifecycle,
    /// Enter 触发**取消**而非确认（粘贴确认：默认动作是「不贴」）。
    pub enter_cancels: bool,
    /// Enter 触发确认。与 `enter_cancels` 互斥，一般只开一个。
    pub enter_confirms: bool,
    /// 额外的 Yes 快捷键（关闭确认：Y）。只影响 `KeyOutcome::yes`。
    pub yes_key: Option<egui::Key>,
    /// 额外的 No 快捷键（关闭确认：N），与 Esc/Enter 同为 cancel。
    pub extra_cancel_key: Option<egui::Key>,
}

impl DialogKeys {
    /// 草稿/快照弹窗的常用形态：Esc = Cancel，Enter = 确认。
    pub(in crate::ui) const DRAFT: Self = Self {
        lifecycle: DialogLifecycle::Draft,
        enter_cancels: false,
        enter_confirms: true,
        yes_key: None,
        extra_cancel_key: None,
    };

    /// 默认键是「取消」的形态（关闭确认 / 粘贴确认）。
    pub(in crate::ui) const CANCEL_DEFAULT: Self = Self {
        lifecycle: DialogLifecycle::Draft,
        enter_cancels: true,
        enter_confirms: false,
        yes_key: None,
        extra_cancel_key: None,
    };

    /// 只有 Esc = 取消，Enter 什么都不做（SSH 主机密钥确认等安全场景）。
    pub(in crate::ui) const ESC_ONLY: Self = Self {
        enter_cancels: false,
        enter_confirms: false,
        ..Self::DRAFT
    };

    /// 加上「Y = Yes」（关闭确认弹窗的视觉基线加速器）。
    pub(in crate::ui) const fn with_yes_key(mut self, key: egui::Key) -> Self {
        self.yes_key = Some(key);
        self
    }

    /// 加上「N = No」（关闭确认弹窗的视觉基线加速器）。
    pub(in crate::ui) const fn with_no_key(mut self, key: egui::Key) -> Self {
        self.extra_cancel_key = Some(key);
        self
    }

    pub(in crate::ui) fn poll(self, ctx: &egui::Context) -> KeyOutcome {
        ctx.input(|i| {
            let esc = i.key_pressed(egui::Key::Escape);
            let enter = i.key_pressed(egui::Key::Enter);
            KeyOutcome {
                cancel: (self.lifecycle.esc_cancels() && esc)
                    || (self.enter_cancels && enter)
                    || self.extra_cancel_key.is_some_and(|k| i.key_pressed(k)),
                confirm: self.enter_confirms && enter,
                yes: self.yes_key.is_some_and(|k| i.key_pressed(k)),
            }
        })
    }
}

// ── DialogId → 键盘契约的**唯一表** ──────────────────────────────
//
// 规则来自各弹窗的语义与金标准：即时生效的没有可取消的草稿；安全确认
//（主机密钥 / 替换密钥 / 同步删除）绝不接 Enter；带 TextEdit 的弹窗
// 只接 Esc，Enter 仍走控件自己的 `lost_focus()` 路径，避免打字时误提交。
impl super::spec::DialogId {
    pub(in crate::ui) fn keys(self) -> DialogKeys {
        use super::spec::DialogId as D;
        match self {
            // About / Preview 没有可丢失的状态，Esc 与 Enter 都直接关窗。
            D::About | D::Preview => DialogKeys::DRAFT,
            D::Settings | D::Shortcuts | D::ScriptEditor | D::SshFailed | D::SshPassword
            | D::ScriptRun | D::FilerPrompt | D::FilerSyncRun => DialogKeys {
                lifecycle: DialogLifecycle::Immediate,
                ..DialogKeys::ESC_ONLY
            },
            // Log Settings 的 Enter=Save 由调用点做焦点守卫（有 TextEdit 时不抢）。
            D::SessionLogging => DialogKeys::DRAFT,
            D::NewSession | D::QuickCommands | D::OneKeys | D::Triggers
            | D::ScriptManager | D::XferPrompt => DialogKeys {
                // TextEdit 密集：Enter 留给输入框，只消费 Esc。
                lifecycle: DialogLifecycle::Draft,
                ..DialogKeys::ESC_ONLY
            },
            D::ProfileChoice | D::SshConnecting | D::SshHostkey | D::SshReplaceKey | D::FilerSyncScan
            | D::FilerSyncReview | D::Onboarding => DialogKeys::ESC_ONLY,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn immediate_dialogs_never_cancel_on_escape() {
        // Preferences 改动即时生效，没有可放弃的草稿。
        let keys = DialogKeys {
            lifecycle: DialogLifecycle::Immediate,
            ..DialogKeys::ESC_ONLY
        };
        assert!(!keys.lifecycle.esc_cancels());
    }

    #[test]
    fn cancel_default_enter_is_not_a_confirm() {
        assert!(DialogKeys::CANCEL_DEFAULT.enter_cancels);
        assert!(!DialogKeys::CANCEL_DEFAULT.enter_confirms);
    }

    #[test]
    fn every_dialog_has_a_keyboard_contract() {
        // 穷举表：新加一个 DialogId 却忘了选键盘档位时，这里红。
        for id in crate::ui::kit::spec::DialogId::ALL {
            let keys = id.keys();
            if keys.lifecycle == DialogLifecycle::Immediate {
                assert!(!keys.lifecycle.esc_cancels(), "{}", id.key());
            }
            assert!(
                !(keys.enter_cancels && keys.enter_confirms),
                "{} 的 Enter 不能既确认又取消",
                id.key()
            );
        }
    }

    #[test]
    fn safety_dialogs_never_confirm_on_enter() {
        // 主机密钥 / 替换密钥 / 同步删除：Enter 一律不是确认，避免误触。
        use crate::ui::kit::spec::DialogId as D;
        for id in [
            D::SshHostkey,
            D::SshReplaceKey,
            D::FilerSyncReview,
            D::FilerSyncRun,
        ] {
            assert!(!id.keys().enter_confirms, "{}", id.key());
        }
    }

    #[test]
    fn text_heavy_dialogs_leave_enter_to_the_input() {
        //  these弹窗输入框密集，全局 Enter 会把打字中的回车当成提交。
        use crate::ui::kit::spec::DialogId as D;
        for id in [
            D::NewSession,
            D::Settings,
            D::QuickCommands,
            D::OneKeys,
            D::Triggers,
            D::XferPrompt,
        ] {
            assert!(!id.keys().enter_confirms, "{}", id.key());
        }
    }

    #[test]
    fn close_confirmation_keeps_y_and_n_accelerators() {
        let keys = DialogKeys::CANCEL_DEFAULT
            .with_yes_key(egui::Key::Y)
            .with_no_key(egui::Key::N);
        assert_eq!(keys.yes_key, Some(egui::Key::Y));
        assert_eq!(keys.extra_cancel_key, Some(egui::Key::N));
        assert!(keys.enter_cancels);
    }
}

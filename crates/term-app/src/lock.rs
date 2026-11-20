//! 锁屏状态机（V1-3，E-LOCK）。
//!
//! `docs/gap-audit.md §6` 的结论：R5 反对的是「点了没反应」的空转开关，不是
//! 反对做锁屏。所以 `Window.LockScreen` 从 F 类置灰接成真实功能：
//! 界面被一层遮罩盖住，解锁要重输 **Vault 主密码**（复用 `term-vault`，
//! 没有第二套口令体系）。
//!
//! 这里只有状态：锁没锁、上一次密码对不对。密码本身**不进 ViewState**
//! （投影层的受控输入缓冲，与 `find_input` 同一类），校验在投影层用
//! `term_vault::Vault::unlock` 做——纯函数 reducer 拿不到加密文件。

use crate::effect::Effect;
use crate::state::ViewState;

pub use crate::state::LockState;

pub use crate::vocab::LockAction;


/// 命令 ID → 本能力的动作。未接线的 ID 返回 `None`。
///
/// 2026-11-12 R2：实现只有一份，在 [`crate::parse`]（入口层）。这里保留同名入口，
/// 一是因为「每个能力自带字符串入口」是这个 crate 对外的约定，二是因为
/// `apps/desktop` 有直接调 `term_app::log::parse` 的地方——把路径改掉没有架构价值。
pub fn parse(id: &str) -> Option<LockAction> {
    crate::parse::lock_parse(id)
}

pub fn reduce(state: &mut ViewState, action: LockAction) -> Vec<Effect> {
    let lock = &mut state.lock;
    match action {
        LockAction::Lock => {
            lock.locked = true;
            lock.error = false;
        }
        LockAction::Unlock => {
            lock.locked = false;
            lock.error = false;
        }
        LockAction::Failed => lock.error = true,
        LockAction::ClearError => lock.error = false,
        LockAction::Cancel => {
            // 没有主密码时不能留一个出不去的遮罩：Cancel 是逃生口，
            // 但只有「尚未创建保险库」时才允许（投影层据此判断）。
            lock.locked = false;
            lock.error = false;
        }
    }
    vec![]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_the_lock_screen_command_is_wired() {
        assert_eq!(parse("Window.LockScreen"), Some(LockAction::Lock));
        assert_eq!(parse("Window.Close"), None);
    }

    #[test]
    fn lock_unlock_are_involutive_and_clear_the_error() {
        let mut s = ViewState::default();
        assert!(!s.lock.locked);
        assert!(reduce(&mut s, LockAction::Lock).is_empty());
        assert!(s.lock.locked);
        assert!(reduce(&mut s, LockAction::Failed).is_empty());
        assert!(s.lock.error);
        assert!(reduce(&mut s, LockAction::Unlock).is_empty());
        assert!(!s.lock.locked && !s.lock.error);
    }

    #[test]
    fn cancel_is_the_escape_hatch_but_still_clears_state() {
        let mut s = ViewState::default();
        reduce(&mut s, LockAction::Lock);
        reduce(&mut s, LockAction::Failed);
        reduce(&mut s, LockAction::Cancel);
        assert!(!s.lock.locked && !s.lock.error);
    }

    #[test]
    fn a_wrong_password_keeps_the_screen_locked() {
        let mut s = ViewState::default();
        reduce(&mut s, LockAction::Lock);
        reduce(&mut s, LockAction::Failed);
        assert!(s.lock.locked && s.lock.error);
        assert_eq!(s, {
            let mut want = ViewState::default();
            want.lock.locked = true;
            want.lock.error = true;
            want
        });
    }
}

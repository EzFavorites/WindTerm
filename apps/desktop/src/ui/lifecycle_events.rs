//! 控制面 Before* 事件的触发器求值：队列 → 规则 → 当前标签的动作落地。
//!
//! `SessionPool` 在开/关会话时只把事件塞进一个线程安全队列；真正的规则表
//! 在窗口上，所以求值放在每帧的 draw 里。这样构造期不必拿到规则，
//! 也不会在会话线程上碰投影层状态。

use crate::ui::*;
use term_core::trigger::TriggerOn;

/// 取走队列并按活动标签的目标上下文求值。`now` 与 Matcher 同源（秒）。
pub(in crate::ui) fn poll(win: &mut TerminalWindow, now: u64, run_commands: &mut Vec<String>) {
    let events: Vec<TriggerOn> = match win.lifecycle_events.lock() {
        Ok(mut q) => q.drain(..).collect(),
        Err(_) => return,
    };
    if events.is_empty() {
        return;
    }
    let rules = win.triggers.clone();
    let slot = win.active_slot();
    if let Some(tab) = win.tabs.get_mut(slot) {
        let system = tab.config.system.clone();
        let session = tab.config.name.clone();
        let target = term_core::trigger_target::Target {
            system: &system,
            session: &session,
        };
        for on in events {
            let hits = tab.triggers.event_for(&rules, on, now, Some(target));
            for hit in hits {
                tab.apply_trigger(hit, run_commands);
            }
        }
    }
}

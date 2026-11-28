//! 控制面 Before* 事件的触发器求值：队列 → `Signal` → 当前标签的意图队列。
//!
//! `SessionPool` 在开/关会话时只把拥有型的 `Lifecycle` 塞进线程安全队列；
//! 规则表在窗口上，求值放在每帧 draw 里。`Tab` 的求值器已在 `poll` 中装载规则，
//! 这里只喂事件并让 `dispatch_effect` 把四类 `Action` 翻译成待派发意图。

use crate::ui::*;
use term_core::automation::{Signal, Target, Ticks};

/// 取走队列并按活动标签的目标上下文求值。`now` 与 Evaluator 同源（秒）。
pub(in crate::ui) fn poll(
    win: &mut TerminalWindow,
    now: u64,
    run_commands: &mut Vec<String>,
    note: &mut Option<String>,
) {
    let events: Vec<term_core::automation::Lifecycle> = match win.lifecycle_events.lock() {
        Ok(mut q) => q.drain(..).collect(),
        Err(_) => return,
    };
    if events.is_empty() {
        return;
    }
    let slot = win.active_slot();
    if let Some(tab) = win.tabs.get_mut(slot) {
        let target = Target {
            system: tab.config.system.clone(),
            session: tab.config.name.clone(),
        };
        for event in events {
            let stream = tab.config.name.clone();
            let at = Ticks::from_secs(now);
            let verdict = match event {
                term_core::automation::Lifecycle::BeforeConnect => {
                    Signal::BeforeConnect { stream: &stream }
                }
                term_core::automation::Lifecycle::Connected => {
                    Signal::Connected { stream: &stream, at }
                }
                term_core::automation::Lifecycle::BeforeDisconnect => {
                    Signal::BeforeDisconnect { stream: &stream }
                }
                term_core::automation::Lifecycle::Disconnected => {
                    Signal::Disconnected {
                        stream: &stream,
                        at,
                        reason: term_core::automation::CloseReason::Normal,
                    }
                }
            };
            for effect in tab.triggers.eval(&verdict, &target) {
                tab.dispatch_effect(&effect, run_commands, note);
            }
        }
    }
}

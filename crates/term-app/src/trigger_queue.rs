//! 触发器命中队列：Send Condition 与 Send Later 的运行时半边。
//!
//! `term-core::trigger_schedule` 只算「该等什么」，这里负责排队与放行。
//! 不入 UI、不读墙钟：时间由调用方以 `now` 传入（与 Matcher 同源）。
//! 只有带 `SendText` / `SendLine` 动作的命中才排队——高亮、响铃、命令
//! 应当立刻发生，等一个永远不会来的提示符只会让用户以为功能坏了。

use term_core::trigger::{Hit, Rule, TriggerAction};
use term_core::trigger_schedule::{ready, wait_for, Gate, Wait};

#[derive(Clone, Debug)]
struct Pending {
    hit: Hit,
    wait: Wait,
    since: u64,
}

#[derive(Clone, Debug, Default)]
pub struct TriggerQueue {
    pending: Vec<Pending>,
}

impl TriggerQueue {
    /// 接收本轮命中：该等的排进队列，其余立即返回。
    pub fn schedule(&mut self, hits: Vec<Hit>, rules: &[Rule], now: u64) -> Vec<Hit> {
        let mut immediate = Vec::new();
        for hit in hits {
            let is_send = hit.actions.iter().any(|a| {
                matches!(a, TriggerAction::SendText(_) | TriggerAction::SendLine(_))
            });
            let Some(rule) = rules.iter().find(|r| r.id == hit.rule_id) else {
                immediate.push(hit);
                continue;
            };
            let wait = wait_for(rule);
            if is_send && wait != Wait::Ready(Gate::Match) {
                self.pending.push(Pending { hit, wait, since: now });
            } else {
                immediate.push(hit);
            }
        }
        immediate
    }

    /// 到期的定时命中。
    pub fn due(&mut self, now: u64) -> Vec<Hit> {
        let mut ready_hits = Vec::new();
        self.pending.retain(|p| {
            let fire = match p.wait {
                // `now` 与 Matcher 同源：秒。小数秒由 Duration 自己保留。
                Wait::Later(delay) => {
                    !delay.is_zero()
                        && (now.saturating_sub(p.since) as f64) >= delay.as_secs_f64()
                }
                Wait::Ready(_) => false,
            };
            if fire {
                ready_hits.push(p.hit.clone());
            }
            !fire
        });
        ready_hits
    }

    /// 提示符 / 换行事件到达时放行对应 gate 的命中。
    pub fn on_event(&mut self, event: Gate) -> Vec<Hit> {
        let mut ready_hits = Vec::new();
        self.pending.retain(|p| {
            let fire = match p.wait {
                Wait::Ready(_) => ready(&p.wait, event),
                Wait::Later(_) => false,
            };
            if fire {
                ready_hits.push(p.hit.clone());
            }
            !fire
        });
        ready_hits
    }

    pub fn len(&self) -> usize {
        self.pending.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rule(id: &str, condition: term_core::trigger::SendCondition, later: &str) -> Rule {
        let mut rule = Rule::default();
        rule.id = id.into();
        rule.kind = term_core::trigger::TriggerKind::SendText;
        rule.config.send.condition = condition;
        rule.config.send.later_secs = later.into();
        rule.actions = vec![TriggerAction::SendText("go".into())];
        rule
    }

    fn hit(id: &str) -> Hit {
        Hit {
            rule_id: id.into(),
            actions: vec![TriggerAction::SendText("go".into())],
            matched: String::new(),
        }
    }

    #[test]
    fn a_send_text_hit_waits_for_its_prompt_then_fires_once() {
        use term_core::trigger::SendCondition;
        let rules = vec![rule("r", SendCondition::AfterMatchAndPrompt, "0")];
        let mut q = TriggerQueue::default();
        assert!(q.schedule(vec![hit("r")], &rules, 0).is_empty());
        assert_eq!(q.len(), 1);
        assert!(q.on_event(Gate::LineBreak).is_empty());
        assert_eq!(q.on_event(Gate::Prompt).len(), 1);
        assert_eq!(q.len(), 0);
    }

    #[test]
    fn send_later_uses_the_frame_clock_and_ignores_unrelated_events() {
        use term_core::trigger::SendCondition;
        let rules = vec![rule("r", SendCondition::AfterMatch, "0.25")];
        let mut q = TriggerQueue::default();
        assert!(q.schedule(vec![hit("r")], &rules, 0).is_empty());
        assert!(q.on_event(Gate::Prompt).is_empty());
        assert!(q.due(0).is_empty());
        assert_eq!(q.due(1).len(), 1);
    }

    #[test]
    fn non_send_actions_and_unknown_rules_are_never_delayed() {
        use term_core::trigger::SendCondition;
        let rules = vec![rule("r", SendCondition::AfterMatchAndPrompt, "0")];
        let mut q = TriggerQueue::default();
        let mut beep = hit("r");
        beep.actions = vec![TriggerAction::Beep];
        assert_eq!(q.schedule(vec![beep], &rules, 0).len(), 1);
        assert_eq!(q.schedule(vec![hit("missing")], &rules, 0).len(), 1);
        assert_eq!(q.len(), 0);
    }
}

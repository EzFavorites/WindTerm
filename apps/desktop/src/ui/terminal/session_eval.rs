//! 触发器规则装载校验 + Effect 收集/派发（从 `session.rs` 拆出）。
//!
//! 这一块与事件泵的区别：事件泵负责「这一帧发生了什么」，这里负责「命中的规则
//! 能不能进求值器、命中之后往哪里去」。规则校验必须在 reload 之前做，Effect
//! 的写意图必须先过仲裁器再进帧末队列——两件事都属于「求值的结果怎么落地」，
//! 放在一起，事件泵就只剩信号构造与快照搬运。
//!
//! 拆出时代码与注释逐字未改（两处可见性/路径适配除外），行为与公开接口不变。

use term_core::automation::{validate_rules, Effect, Priority, Release, Rule as CoreRule, Ticks};
use term_core::session::ReconnectPolicy;

use crate::ui::*;
use crate::ui::dialogs::triggers::model::{self, Rule};
use crate::bridge::surface::{arbiter_session, shared_arbiter};
use crate::ui::terminal::trigger_dispatch::{
    AppIntent, CommandCall, DispatchCtx, IntentKind, intent_from_effect,
};

use super::now_secs;

/// 编辑投影 → 内核规则，再过装载期校验。抽成独立函数是为了让「坏规则被拒」
/// 不必造一个完整窗口夹具就能测（见本文件末尾的金样本）。
pub(super) fn load_core_rules(rules: &[Rule], note: &mut Option<String>) -> Vec<CoreRule> {
    let mut core_rules = Vec::with_capacity(rules.len());
    for rule in rules {
        match model::ViewRule::to_core(rule) {
            Ok(core) => core_rules.push(core),
            Err(msg) if note.is_none() => *note = Some(msg),
            Err(_) => {}
        }
    }
    validate_core_rules(core_rules, note)
}

/// `Evaluator::reload` 只换规则本体、按 id 留运行态，不校验 method / params /
/// 可见性。这里在 reload 之前调用内核的 `validate_rules`，把不合格的整条剔掉。
/// 坏规则留到命中那一刻才变 NoEffect，是用户最难察觉的失败方式。
fn validate_core_rules(
    mut core_rules: Vec<CoreRule>,
    note: &mut Option<String>,
) -> Vec<CoreRule> {
    loop {
        match validate_rules(&core_rules) {
            Ok(()) => return core_rules,
            Err((id, fault)) => {
                if note.is_none() {
                    *note = Some(format!("规则 {id} 装载期被拒：{fault}"));
                }
                let before = core_rules.len();
                core_rules.retain(|r| r.id != id);
                if core_rules.len() == before {
                    // 防御性出口：validate_rules 报的 id 一定来自这批规则，
                    // 真出现不一致也不能让事件泵卡在死循环里。
                    return core_rules;
                }
            }
        }
    }
}

impl Tab {
    /// 把一次命中翻译成待派发意图。**这里不写 PTY、不改视图状态**：四类
    /// `Action` 各自落到 `FrameIntent`，由帧末 `trigger_intents` 统一消费。
    ///
    /// `gated` 只对输出信号的会话写意图为真：事件类规则（Connect / Disconnect /
    /// Idle）与 Mark / Alert / Dispatch 不排队，命中当帧就发生。
    pub(in crate::ui) fn dispatch_effect(
        &mut self,
        effect: &Effect,
        run_commands: &mut Vec<CommandCall>,
        note: &mut Option<String>,
        gated: bool,
    ) {
        let session = self.session.unwrap_or(self.identity);
        let key = arbiter_session(&self.pool, session);
        let Some(mut intent) = intent_from_effect(
            effect,
            DispatchCtx::for_rule(key, &effect.rule, None, None),
        ) else {
            return;
        };
        // 只有写意图（含凭据，它落地后就是一次写）才申请许可。被压制的
        // 写意图根本不进队列——不是产生后再丢弃；留痕写进本帧 note。
        if intent.is_write() || matches!(intent.kind, IntentKind::Credential(_)) {
            let (result, owner) = {
                let mut arbiter = shared_arbiter().lock().unwrap_or_else(|e| e.into_inner());
                let result = arbiter.request(key, &effect.rule, Priority::P3Rule, false);
                let owner = arbiter.owner(key).map(|o| o.source);
                (result, owner)
            };
            match result {
                Ok(grant) => {
                    intent.ctx.owner = owner;
                    intent.ctx.grant = Some(grant);
                }
                Err(denied) => {
                    if note.is_none() {
                        *note = Some(format!(
                            "规则 {} 的写入被压制：{}",
                            effect.rule,
                            denied.as_message()
                        ));
                    }
                    return;
                }
            }
        }
        if gated && intent.is_write() {
            intent.since = Ticks::from_secs(now_secs());
        } else {
            intent.release = Release::Immediate;
        }
        // 命令 ID / 菜单调用走既有窗口级分发器，其余意图进队列等帧末消费。
        if let IntentKind::App(AppIntent::Dispatch(id)) = &intent.kind {
            run_commands.push(CommandCall::plain(id.clone()));
        }
        self.pending_intents.push(intent);
    }

    /// 断连边沿处理排队的发送意图；只在确实送不出去时丢并报一条提示。
    ///
    /// `Evaluator::reset` 只清尾巴 / 冷却 / 一次性表，不清 trigger queue，所以
    /// 队列本身是跨边沿活着的。关键在**不能无条件抽干**：
    ///
    /// - 自动重连开着且不是 local shell：短暂掉线后还会回来，未到期的
    ///   `Wait::Later` 与等提示符 / 换行的 `Wait::Ready` 都要原样留着，否则
    ///   1 秒后重连，那条 Send Later 已经不在队列里，永远发不出去；
    /// - 其它边沿（手动断开 / shell / 关了自动重连）：只取走**此刻已到期却
    ///   无会话可发**的项，不碰还没到期的计划——它们留给可能的手动重连。
    ///
    /// 连上边沿的 `Evaluator::reset` 不清意图队列，所以留下
    /// 的命中能在新会话上正常落地。
    pub(in crate::ui) fn discard_pending_sends(
        &mut self,
        now: u64,
        note: &mut Option<String>,
        auto_reconnect: bool,
    ) {
        if self.config.allows_reconnect(&ReconnectPolicy::from_enabled(auto_reconnect)) {
            return;
        }
        // 只丢「已到期却发不出去」的写意图：未到期的 Send Later 留给可能的
        // 手动重连（见 `a_not_yet_due_send_later_is_kept_on_a_non_auto_reconnect_disconnect`）。
        let mut dropped = 0;
        self.pending_intents.retain(|intent| {
            if intent.is_write() && !intent.release.is_immediate() && intent.ready(now, None) {
                dropped += 1;
                false
            } else {
                true
            }
        });
        if dropped > 0 && note.is_none() {
            *note = Some(format!("会话已断开，{dropped} 条待发送触发器已丢弃（未送达）"));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bad_rules_are_rejected_at_load_time_not_on_hit() {
        use term_core::automation::{Action, Evaluator, Filter, Guard, When};

        fn core(id: &str, method: &str, params: serde_json::Value) -> CoreRule {
            CoreRule {
                id: id.into(),
                name: id.into(),
                enabled: true,
                when: When::Connected,
                filter: Filter::default(),
                guard: Guard::default(),
                release: Release::default(),
                actions: vec![Action::SessionInvocation { method: method.into(), params }],
            }
        }

        let bad_method = core("bad-method", "not_a_verb", serde_json::json!({}));
        let bad_params = core(
            "bad-params",
            "write",
            serde_json::json!({ "session": "s", "text": 42 }),
        );
        let good = core(
            "good",
            "write",
            serde_json::json!({ "session": "s", "text": "hi" }),
        );

        // 没有这道闸门时 reload 会照单全收：先钉住问题确实存在。
        let mut probe = Evaluator::new();
        assert!(probe.reload(&[bad_method.clone(), bad_params.clone(), good.clone()]).is_empty());
        assert_eq!(probe.rules().len(), 3);

        let mut note = None;
        let loaded = validate_core_rules(vec![bad_method, bad_params, good], &mut note);
        assert_eq!(loaded.len(), 1, "两条坏规则都不得进求值器");
        assert_eq!(loaded[0].id, "good");
        let note = note.expect("坏规则须把原因写进状态栏");
        assert!(note.contains("bad-method") && note.contains("能力面"), "{note}");
    }
}

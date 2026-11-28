//! 前台控制面的**仲裁器**（ADR-0030 决策 6）：每会话一个 owner + 优先级。
//!
//! 只做状态转移，零 IO、零墙钟、零线程：调用方把「谁、什么级别、是否独占」
//! 交进来，这里回答「可以写 / 被压制」，并给每一次放行一个单调 `seq`。
//! 同一帧的多个写意图由调用方按 `(priority, seq)` 排序后一次落地；
//! 用户按键是抢占**信号**（不排队），连一次 owner 都不留——键完下一条规则
//! 仍可正常申请 owner。
//!
//! 优先级按 ADR 表从低到高排列，派生 `Ord` 即数值序，数值越大越优先：
//! `P4Passive < P3Rule < P2Explicit < P1Auth < P0Keyboard`。
//! P4 只读，`request` 会给 `writable = false` 的许可，但绝不产生 PTY 写意图。

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use crate::session_runtime::SessionId;

/// 请求来源的优先级。`Ord` 由声明顺序派生，**数值越大越优先**，
/// 与 UI 侧 `DispatchCtx.priority: u8` 的排序口径一致。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum Priority {
    /// P4：内置被动检测（高亮 / URL / IP）。只读，不抢 owner。
    #[default]
    P4Passive,
    /// P3：用户规则。默认 owner。
    P3Rule,
    /// P2：显式调用（脚本 / API）。只有 `exclusive` 才抢 owner。
    P2Explicit,
    /// P1：连接期认证（auth 链 + vault 凭据）。阶段独占。
    P1Auth,
    /// P0：用户键盘。抢占信号，不进队列，也不留 owner。
    P0Keyboard,
}

impl Priority {
    /// 排序用的数值（越大越优先）。与 `as u8` 不同：P4 = 1，P0 = 5。
    pub fn rank(self) -> u8 {
        self as u8 + 1
    }

    /// 这个级别的许可能不能落向 PTY。只有 P4 只读。
    pub fn is_write(self) -> bool {
        self != Self::P4Passive
    }
}

/// 会话的仲裁阶段。连接期只有 P1 能写；`Live` 才开放规则与显式调用。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Phase {
    /// 连接期（认证链进行中）。
    Connecting,
    /// 已建立、可供自动化写。
    #[default]
    Live,
}

/// 一次放行的凭据。调用方必须把它随写意图一起带到落地时刻，
/// 由 [`Arbitrator::is_valid`] 复核——按键 / 独占抢占发生在排队之后时，
/// 旧许可的 `epoch` 对不上，写就不会发生。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Grant {
    pub seq: u64,
    pub epoch: u64,
    pub priority: Priority,
    /// 本次请求是否成为 owner。
    pub owner: bool,
    /// P4 为 false：可以观察，不可以写 PTY。
    pub writable: bool,
}

impl Grant {
    /// 同帧排序键：`(优先级降序, 到达序号升序)`。
    pub fn order_key(&self) -> (u8, u64) {
        (self.priority.rank(), self.seq)
    }
}

/// 显式失败。两者都要让调用方看得见，不能变成「成功但什么也没做」。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Denied {
    /// 被更高优先级占着；显式调用要拿到 `Busy` 错误。
    Busy { owner: String, phase: Phase },
    /// 非 owner 的写意图被压制；规则只留痕，不报错到用户面前。
    Suppressed { by: String },
}

impl Denied {
    pub fn is_busy(&self) -> bool {
        matches!(self, Self::Busy { .. })
    }

    /// 面向用户的错误文案；`Busy` / `Suppressed` 是机器可辨的固定前缀。
    pub fn as_message(&self) -> String {
        match self {
            Self::Busy { owner, phase } => format!(
                "Busy: 会话正由 {owner} 占用（阶段 {phase:?}）；\
                 用户按键或连接期认证优先，稍后再试"
            ),
            Self::Suppressed { by } => {
                format!("Suppressed: 本次写入被 {by} 压制（非 owner 不产生写意图）")
            }
        }
    }
}

/// 压制留痕原因。计数是硬数字，`notes` 只留最近若干条供状态行 / 排障。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SuppressionReason {
    Connecting,
    ExclusiveOwner(String),
    KeyboardPreempt,
    ReadOnly,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Suppression {
    pub source: String,
    pub priority: Priority,
    pub reason: SuppressionReason,
    pub seq: u64,
}

/// 当前 owner。`exclusive` 只在 P2 显式独占时为 true。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Owner {
    pub source: String,
    pub priority: Priority,
    pub exclusive: bool,
}

const NOTE_LIMIT: usize = 32;

#[derive(Clone, Debug, Default)]
struct SessionState {
    phase: Phase,
    owner: Option<Owner>,
    /// 每次抢占 / 相位切换都会前进；许可的 epoch 落后即失效。
    epoch: u64,
    seq: u64,
    /// 已放行、尚未落地的写意图数。抢占时整批计入压制。
    pending: u64,
    suppressed: u64,
    notes: Vec<Suppression>,
}

impl SessionState {
    /// 记一条留痕并计一次压制（用于「本次请求被拒」这种一对一场景）。
    fn note(&mut self, source: &str, priority: Priority, reason: SuppressionReason, seq: u64) {
        self.push_note(source, priority, reason, seq);
        self.suppressed += 1;
    }

    /// 只写留痕、不计数。批量作废时计数按作废的许可数算，
    /// 一条日志不能把同一个数字再加一遍。
    fn push_note(&mut self, source: &str, priority: Priority, reason: SuppressionReason, seq: u64) {
        if self.notes.len() >= NOTE_LIMIT {
            self.notes.remove(0);
        }
        self.notes.push(Suppression {
            source: source.to_owned(),
            priority,
            reason,
            seq,
        });
    }

    /// 所有者被换掉 / 按键抢占时，所有已排队许可一次性失效并留痕。
    /// 压制数就是作废的许可条数——日志只有一条，不能重复计数。
    fn invalidate_pending(&mut self, reason: SuppressionReason) -> u64 {
        let count = self.pending;
        self.pending = 0;
        self.epoch = self.epoch.wrapping_add(1);
        if count > 0 {
            self.suppressed = self.suppressed.saturating_add(count);
            self.push_note("pending", Priority::P3Rule, reason, self.seq);
        }
        count
    }
}

/// 用户按键抢占的结果，供 UI 记一条状态行。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Preemption {
    pub released: Option<String>,
    pub suppressed: u64,
}

/// 纯状态仲裁器。一个进程一份（宿主用共享容器持有），按会话隔离状态。
#[derive(Clone, Debug, Default)]
pub struct Arbitrator {
    sessions: HashMap<SessionId, SessionState>,
}

impl Arbitrator {
    pub fn new() -> Self {
        Self::default()
    }

    /// 申请一次写许可。`exclusive` 只对 P2 有意义：P2 独占才抢 owner。
    pub fn request(
        &mut self,
        session: SessionId,
        source: &str,
        priority: Priority,
        exclusive: bool,
    ) -> Result<Grant, Denied> {
        // 键盘是信号，不排队；先把旧 owner 与队列一起清掉。
        if priority == Priority::P0Keyboard {
            let preemption = self.user_key_pressed(session);
            let state = self.sessions.entry(session).or_default();
            state.seq += 1;
            let _ = preemption;
            return Ok(Grant {
                seq: state.seq,
                epoch: state.epoch,
                priority,
                owner: false,
                writable: true,
            });
        }

        let state = self.sessions.entry(session).or_default();
        state.seq += 1;
        let seq = state.seq;
        let grant = |state: &SessionState, owner: bool| Grant {
            seq,
            epoch: state.epoch,
            priority,
            owner,
            writable: priority.is_write(),
        };

        // P4 只读：给许可供观察使用，但不进写队列、不抢 owner。
        if priority == Priority::P4Passive {
            return Ok(grant(state, false));
        }

        if state.phase == Phase::Connecting && priority != Priority::P1Auth {
            let owner = state.owner.as_ref().map_or_else(|| "auth".to_owned(), |o| o.source.clone());
            state.note(source, priority, SuppressionReason::Connecting, seq);
            return Err(Denied::Busy { owner, phase: Phase::Connecting });
        }

        let take_owner = match (&state.owner, priority) {
            (None, Priority::P2Explicit) if !exclusive => false,
            (None, _) => true,
            (Some(current), Priority::P1Auth) => current.priority < Priority::P1Auth,
            (Some(current), Priority::P2Explicit) => {
                exclusive && current.priority < Priority::P2Explicit
            }
            (Some(current), Priority::P3Rule) => current.priority < Priority::P3Rule,
            (Some(_), _) => false,
        };

        if take_owner {
            if state.owner.is_some() {
                state.invalidate_pending(SuppressionReason::ExclusiveOwner(source.to_owned()));
            }
            state.owner = Some(Owner {
                source: source.to_owned(),
                priority,
                exclusive: exclusive && priority == Priority::P2Explicit,
            });
            if priority.is_write() {
                state.pending += 1;
            }
            return Ok(grant(state, true));
        }

        // 非 owner：P2 默认只进帧内排序；P3 被更强 owner 压制。
        let by = state.owner.as_ref().map_or_else(|| "auth".to_owned(), |o| o.source.clone());
        let busy = priority == Priority::P2Explicit && exclusive;
        if priority == Priority::P2Explicit && !busy {
            // 默认非独占：与 P3 同帧写，靠排序而非硬隔离（ADR 已接受的后果），
            // 不算压制（压制只记真正没产生写意图的那些请求）。
            state.pending += 1;
            return Ok(grant(state, false));
        }
        state.note(
            source,
            priority,
            SuppressionReason::ExclusiveOwner(by.clone()),
            seq,
        );
        if busy {
            return Err(Denied::Busy { owner: by, phase: state.phase });
        }
        Err(Denied::Suppressed { by })
    }

    /// 用户按键：抢占并释放现有 owner，已排队的写意图一起作废并计数。
    pub fn user_key_pressed(&mut self, session: SessionId) -> Preemption {
        let state = self.sessions.entry(session).or_default();
        let released = state.owner.take().map(|owner| owner.source);
        let suppressed = state.invalidate_pending(SuppressionReason::KeyboardPreempt);
        Preemption { released, suppressed }
    }

    /// 释放 owner（只有持有者本人能释放）。显式调用写完就调它，防泄漏。
    pub fn release(&mut self, session: SessionId, source: &str) -> bool {
        let Some(state) = self.sessions.get_mut(&session) else {
            return false;
        };
        if state.owner.as_ref().is_some_and(|owner| owner.source == source) {
            state.owner = None;
            true
        } else {
            false
        }
    }

    /// 切换阶段。进入 Connecting 时清空 owner 并作废排队许可；
    /// 回到 Live 时释放连接期认证 owner（P1）。
    pub fn set_phase(&mut self, session: SessionId, phase: Phase) {
        let state = self.sessions.entry(session).or_default();
        if phase == state.phase {
            return;
        }
        state.phase = phase;
        match phase {
            Phase::Connecting => {
                state.owner = None;
                state.invalidate_pending(SuppressionReason::Connecting);
            }
            Phase::Live => {
                if state.owner.as_ref().is_some_and(|o| o.priority == Priority::P1Auth) {
                    state.owner = None;
                }
            }
        }
    }

    pub fn phase(&self, session: SessionId) -> Phase {
        self.sessions.get(&session).map_or(Phase::Live, |s| s.phase)
    }

    pub fn owner(&self, session: SessionId) -> Option<Owner> {
        self.sessions.get(&session).and_then(|s| s.owner.clone())
    }

    /// 许可到落地那一刻还有效吗？epoch 落后说明期间发生了抢占 / 相位切换。
    pub fn is_valid(&self, session: SessionId, grant: &Grant) -> bool {
        grant.writable
            && self
                .sessions
                .get(&session)
                .is_some_and(|state| state.epoch == grant.epoch)
    }

    /// 落地成功：把这条待写从计数里减掉，供下一次抢占报准确的压制数。
    pub fn consume(&mut self, session: SessionId, grant: &Grant) {
        if !self.is_valid(session, grant) || !grant.writable {
            return;
        }
        if let Some(state) = self.sessions.get_mut(&session) {
            state.pending = state.pending.saturating_sub(1);
        }
    }

    pub fn suppressed_count(&self, session: SessionId) -> u64 {
        self.sessions.get(&session).map_or(0, |s| s.suppressed)
    }

    pub fn notes(&self, session: SessionId) -> &[Suppression] {
        self.sessions.get(&session).map_or(&[], |s| s.notes.as_slice())
    }

    /// 仅供夹具 / 测试把某条会话的状态清干净。
    pub fn clear(&mut self, session: SessionId) {
        self.sessions.remove(&session);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn priorities_follow_the_adr_table() {
        assert!(Priority::P0Keyboard > Priority::P1Auth);
        assert!(Priority::P1Auth > Priority::P2Explicit);
        assert!(Priority::P2Explicit > Priority::P3Rule);
        assert!(Priority::P3Rule > Priority::P4Passive);
        assert!(!Priority::P4Passive.is_write());
        // rank 只是同帧排序用的内部刻度（discriminant + 1），不是 ADR 表里的
        // 级别号：只钉它的相对顺序，不硬编码某个具体数字。
        assert!(Priority::P4Passive.rank() < Priority::P3Rule.rank());
        assert!(Priority::P3Rule.rank() < Priority::P2Explicit.rank());
    }

    #[test]
    fn a_rule_is_the_default_owner_and_only_one_owner_exists() {
        let mut arbiter = Arbitrator::new();
        let first = arbiter.request(7, "rule-a", Priority::P3Rule, false).unwrap();
        assert!(first.owner);
        assert_eq!(arbiter.owner(7).map(|o| o.source).as_deref(), Some("rule-a"));
        // 同一提示的第二条规则被压制：没有第二个写许可。
        assert_eq!(
            arbiter.request(7, "rule-b", Priority::P3Rule, false),
            Err(Denied::Suppressed { by: "rule-a".into() })
        );
        assert_eq!(arbiter.suppressed_count(7), 1);
        assert_eq!(arbiter.notes(7).len(), 1);
    }

    #[test]
    fn explicit_calls_are_non_exclusive_unless_they_ask_for_it() {
        let mut arbiter = Arbitrator::new();
        let rule = arbiter.request(1, "rule", Priority::P3Rule, false).unwrap();
        let plain = arbiter.request(1, "api", Priority::P2Explicit, false).unwrap();
        assert!(!plain.owner, "显式调用默认不抢 owner（ADR 决策 6）");
        assert_eq!(arbiter.owner(1).unwrap().source, "rule", "owner 仍是默认持有者");
        assert!(arbiter.is_valid(1, &rule), "非独占不打断同帧规则写入");

        let mut other = Arbitrator::new();
        let rule = other.request(1, "rule", Priority::P3Rule, false).unwrap();
        // 一旦显式声明 exclusive，P2 高于 P3：抢 owner 并作废此前排队的规则写。
        let claimed = other.request(1, "api", Priority::P2Explicit, true).unwrap();
        assert!(claimed.owner);
        assert_eq!(other.owner(1).unwrap().source, "api");
        assert!(!other.is_valid(1, &rule), "独占抢占让旧许可失效");
    }

    #[test]
    fn keyboard_preempts_an_exclusive_owner_and_suppresses_queued_writes() {
        let mut arbiter = Arbitrator::new();
        let exclusive = arbiter.request(2, "api", Priority::P2Explicit, true).unwrap();
        assert!(exclusive.owner);
        let queued = arbiter.request(2, "rule", Priority::P3Rule, false).unwrap_err();
        assert!(matches!(queued, Denied::Suppressed { .. }));

        let preempted = arbiter.user_key_pressed(2);
        assert_eq!(preempted.released.as_deref(), Some("api"));
        assert!(arbiter.owner(2).is_none(), "按键不留 owner，下一条规则可再申请");
        let again = arbiter.request(2, "rule", Priority::P3Rule, false).unwrap();
        assert!(again.owner);
        assert!(arbiter.release(2, "rule"));
        assert!(arbiter.owner(2).is_none());
        assert!(!arbiter.release(2, "rule"), "重复释放不能假装成功");
    }

    #[test]
    fn connecting_phase_admits_only_auth_and_live_releases_it() {
        let mut arbiter = Arbitrator::new();
        arbiter.set_phase(3, Phase::Connecting);
        assert_eq!(arbiter.phase(3), Phase::Connecting);
        assert!(
            matches!(
                arbiter.request(3, "rule", Priority::P3Rule, false),
                Err(Denied::Busy { phase: Phase::Connecting, .. })
            )
        );
        let auth = arbiter.request(3, "auth", Priority::P1Auth, true).unwrap();
        assert!(auth.owner);
        arbiter.set_phase(3, Phase::Live);
        assert!(arbiter.owner(3).is_none(), "连接期认证 owner 在 Live 释放");
        assert!(arbiter.request(3, "rule", Priority::P3Rule, false).is_ok());
    }

    #[test]
    fn passive_sources_never_write_or_take_the_owner() {
        let mut arbiter = Arbitrator::new();
        arbiter.request(4, "rule", Priority::P3Rule, false).unwrap();
        let read = arbiter.request(4, "hover", Priority::P4Passive, false).unwrap();
        assert!(!read.writable);
        assert!(!read.owner);
        assert_eq!(arbiter.owner(4).unwrap().source, "rule");
        assert_eq!(arbiter.suppressed_count(4), 0, "观察不进仲裁，不算压制");
    }

    #[test]
    fn a_preemption_invalidates_already_queued_grants() {
        let mut arbiter = Arbitrator::new();
        let grant = arbiter.request(5, "rule", Priority::P3Rule, false).unwrap();
        assert!(arbiter.is_valid(5, &grant));
        let preemption = arbiter.user_key_pressed(5);
        assert_eq!(preemption.suppressed, 1, "作废一条排队写就报一条");
        assert!(!arbiter.is_valid(5, &grant));
        assert_eq!(
            arbiter.suppressed_count(5), 1,
            "累计口径与本次返回值一致：留痕日志不重复计数"
        );
        assert_eq!(arbiter.notes(5).len(), 1);
    }
}

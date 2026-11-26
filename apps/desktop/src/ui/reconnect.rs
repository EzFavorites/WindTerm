//! 自动重连的**窗口侧运行时**：退避策略的消费、等待窗口、并发代数。
//!
//! 纯参数与判据住在 `term_core::session::reconnect`（纯函数、可单测）；这里只做
//! 三件带状态的事：
//!
//! 1. 每帧把到点的断开标签在原位重连（不改活动标签）；
//! 2. 记住每条断开链**首次断开**的时刻——180s 窗口从那一刻起算，每次尝试
//!    不重算，连上清零；
//! 3. 给每次尝试分配一个 epoch，异步 SSH 回调落地前核对 slot / 会话名 / epoch，
//!    避免 `PendingOpen::Replace` 与「到期重连」竞态时把新会话顶掉。
//!
//! 运行时放在窗口而不是 `Tab`：三个测试夹具用完整 `Tab { … }` 字面量构造，
//! 加字段会连坐不可编辑的文件；按稳定的 `tab.identity` 索引，槽位漂移不影响。

use crate::ui::*;
use term_core::session::{backoff, effective, Protocol, ReconnectPolicy};
use term_core::session_runtime::{ExitKind, SessionId};

/// 一条断开链的运行态。
#[derive(Clone, Copy, Default)]
struct Slot {
    /// 首次断开时刻（`now_secs()` 口径）。
    window_start: Option<u64>,
    /// 本次断开已排到第几次（仅用于诊断，次数本体在 Tab 上）。
    gen: u64,
}

/// 窗口级的重连运行时。
pub(in crate::ui) struct ReconnectRuntime {
    /// 全局策略（由 `Prefs::effective_reconnect` 构造）。
    pub(in crate::ui) global: ReconnectPolicy,
    /// 全局递增的尝试代数。同时只允许一个 `Connector` 在飞，所以一个计数器
    /// 就够挡住所有过期回调。
    gen: u64,
    slots: std::collections::HashMap<u64, Slot>,
}

impl From<ReconnectPolicy> for ReconnectRuntime {
    fn from(global: ReconnectPolicy) -> Self {
        Self {
            global,
            gen: 0,
            slots: std::collections::HashMap::new(),
        }
    }
}

impl ReconnectRuntime {
    /// 新排一次尝试：递增代数，返回它给 `Connector` 当凭据。
    pub(in crate::ui) fn next_gen(&mut self) -> u64 {
        self.gen = self.gen.wrapping_add(1);
        self.gen
    }

    /// 开一条新的断开链（或续上旧的）：窗口起点只在第一次记，之后每次
    /// 尝试都不重算。`first_delay` 用来从「首次到期时刻」反推出断开时刻。
    pub(in crate::ui) fn begin(&mut self, identity: u64, now: u64, first_delay: u64) -> u64 {
        let gen = self.next_gen();
        let slot = self.slots.entry(identity).or_default();
        if slot.window_start.is_none() {
            slot.window_start = Some(now.saturating_sub(first_delay));
        }
        slot.gen = gen;
        gen
    }

    /// 这条断开链已经跑了多久（秒）。没有起点时返回 0（还没开始）。
    pub(in crate::ui) fn elapsed(&self, identity: u64, now: u64) -> u64 {
        self.slots
            .get(&identity)
            .and_then(|s| s.window_start)
            .map(|start| now.saturating_sub(start))
            .unwrap_or(0)
    }

    /// 把全局代数对齐到一次已经分配好的尝试。自动重连在 `begin()` 里领号，
    /// 通用的连接发起口又会领一次；这里把多领的那一次退回去，否则回调会被
    /// 自己的过期检查丢掉。
    pub(in crate::ui) fn set_gen(&mut self, gen: u64) {
        self.gen = gen;
    }

    /// 连上（或用户明确断开）：窗口与代数一起作废。
    pub(in crate::ui) fn clear(&mut self, identity: u64) {
        self.slots.remove(&identity);
    }

    /// 回调带着的代数已经不是最新的一次尝试了 → 丢弃。
    pub(in crate::ui) fn stale(&self, gen: u64) -> bool {
        gen != self.gen
    }
}

/// `term_ssh` 的错误 → 本协议无关的退出分类。
///
/// 映射放在 apps 层：`term-core` 不许认识 `SshError`，`term-io` 也不许依赖
/// `term-ssh`。终止性失败立即停，网络 Connect / Channel 可重试。新增错误
/// 变体时默认按「停」处理——宁可少连一次，不可对着错误凭据反复发包。
pub(in crate::ui) fn exit_kind_for_ssh(err: &term_ssh::SshError) -> ExitKind {
    match err {
        term_ssh::SshError::Connect(_) | term_ssh::SshError::Channel(_) => ExitKind::Retryable,
        _ => ExitKind::Terminal,
    }
}

impl Tab {
    /// 串口的通道在 `poll()` 里不会变成 `None`（设备仍在册），此前因此
    /// **没有断开边沿**，自动重连永远等不到。判活取控制面的 `status().alive`
    /// （检查放在 `poll() == Some` 分支；`None` 分支已经 return 了）。
    /// 返回 `true` = 已经 `close()` 造出边沿，调用方应当把这一帧标成有变化。
    pub(in crate::ui) fn note_serial_hangup(&mut self, id: SessionId) -> bool {
        if self.config.protocol != Protocol::Serial {
            return false;
        }
        let Some(st) = self.pool.status(id) else { return false };
        if st.alive {
            return false;
        }
        // hangup / 设备暂缺是可重试失败，不置 blocked；只有终止性分类才停。
        self.auto_reconnect_blocked |= term_core::session::is_terminal(st.exit_kind);
        self.close();
        true
    }
}

impl TerminalWindow {
    /// 一次**自动**重连失败后显式排下一次。新 tab 的 `was_live=false`、
    /// session=None，不会产生「断开边沿」；不在这里排就会把 180s 窗口空耗掉。
    pub(in crate::ui) fn schedule_failed_attempt(&mut self, slot: usize, name: &str) {
        let Some(tab) = self.tabs.get_mut(slot) else { return };
        if tab.config.name != name {
            return;
        }
        let policy = effective(&self.reconnect.global, &tab.config.reconnect);
        let attempts = tab.auto_reconnect_attempts;
        if let Some(delay) = backoff(&policy, u32::from(attempts)) {
            tab.auto_reconnect_at = Some(crate::now_secs() + delay);
            tab.auto_reconnect_attempts = attempts.saturating_add(1);
        }
    }

    /// 每帧一次：连上就清窗口；到点的断开标签在原位重连，不改活动标签。
    ///
    /// 放在所有 `Tab::poll` 之后调用：连上边沿会在那里清零退避。几行代码留在
    /// 这里而不是 `ui/mod.rs` 是为了守住那个文件的单文件行数门禁。
    pub(in crate::ui) fn poll_auto_reconnect(&mut self) {
        let now = crate::now_secs();
        // 连上边沿：`Tab::poll` 已经把 `auto_reconnect_at` / `attempts` 清零，
        // 这里把窗口侧的镜像一起作废。只认「有会话」的那条——断开后等待
        // 下一次尝试的标签 session=None，不能在这里被清掉。
        for tab in &mut self.tabs {
            if tab.session.is_some() && self.reconnect.elapsed(tab.identity, now) > 0 {
                tab.auto_reconnect_at = None;
                tab.auto_reconnect_attempts = 0;
                self.reconnect.clear(tab.identity);
            }
        }
        let due: Vec<usize> = (0..self.tabs.len())
            .filter(|i| self.tabs[*i].auto_reconnect_at.is_some_and(|at| now >= at))
            .collect();
        // 每帧只处理一条：连接层只有一个全局 Connector 槽位，同一帧给两条
        // 会话发起重连会让后一条覆盖前一条，前者既没连上也不会再排下一次。
        // 其余到点的标签留在原队列里，下一帧自然轮到（at 仍到期）。
        for i in due.into_iter().take(1) {
            let identity = self.tabs[i].identity;
            let policy = effective(&self.reconnect.global, &self.tabs[i].config.reconnect);
            let gen = self.reconnect.begin(identity, now, policy.initial_secs);
            let elapsed = self.reconnect.elapsed(identity, now);
            self.tabs[i].auto_reconnect_at = None;
            if elapsed >= policy.retry_window_secs {
                // 窗口耗尽：这是终止条件，不是「暂时歇一下」。清掉运行态，
                // 下次真断开会重新起一个窗口。
                self.reconnect.clear(identity);
                continue;
            }
            self.reconnect_tab_at(i, false, gen);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn disabled_policy_never_schedules() {
        assert_eq!(term_core::session::backoff(&ReconnectPolicy::default(), 0), None);
    }

    #[test]
    fn the_first_four_attempts_back_off_then_keep_going_until_capped() {
        let p = ReconnectPolicy::from_enabled(true);
        let delays: Vec<u64> = (0..6)
            .map(|a| term_core::session::backoff(&p, a).expect("在溢出保护内"))
            .collect();
        assert_eq!(delays, vec![1, 2, 4, 8, 16, 30]);
    }

    #[test]
    fn window_starts_at_the_first_disconnect_and_is_not_recomputed() {
        let mut rt = ReconnectRuntime::from(ReconnectPolicy::from_enabled(true));
        rt.begin(7, 100, 1);
        // 起点 = 100 - 1 = 99，所以到 101 已经跑了 2 秒。
        assert_eq!(rt.elapsed(7, 101), 2);
        // 第二次尝试不重算窗口：起点仍是 99。
        rt.begin(7, 103, 1);
        assert_eq!(rt.elapsed(7, 103), 4);
        rt.clear(7);
        assert_eq!(rt.elapsed(7, 200), 0);
    }

    #[test]
    fn a_wrong_password_schedules_no_reconnect() {
        let err = term_ssh::SshError::Auth(term_ssh::AuthProblem {
            user: "u".into(),
            method: "password",
            partial_success: false,
        });
        let kind = exit_kind_for_ssh(&err);
        assert!(
            term_core::session::is_terminal(kind),
            "密码错是终止性失败，零重连"
        );
        assert!(!kind.allows_reconnect());
    }

    #[test]
    fn attempts_continue_inside_the_retry_window_and_stop_outside_it() {
        let p = ReconnectPolicy::from_enabled(true);
        let mut rt = ReconnectRuntime::from(p);
        rt.begin(11, 100, 1); // 窗口起点 99
        // 窗口内仍要排下一次：首次失败不把链断掉。
        assert!(rt.elapsed(11, 100 + 179 - 1) < p.retry_window_secs);
        assert!(term_core::session::backoff(&p, 9).is_some());
        // 越过 180s 就停；`max_attempts` 只是溢出保护，不是这里的判据。
        assert!(rt.elapsed(11, 99 + p.retry_window_secs) >= p.retry_window_secs);
    }

    #[test]
    fn stale_async_callbacks_are_rejected() {
        let mut rt = ReconnectRuntime::from(ReconnectPolicy::default());
        let old = rt.next_gen();
        let new = rt.next_gen();
        assert!(rt.stale(old));
        assert!(!rt.stale(new));
    }

    #[test]
    fn reusing_the_allocated_epoch_keeps_the_auto_reconnect_callback_alive() {
        let mut rt = ReconnectRuntime::from(ReconnectPolicy::default());
        let allocated = rt.next_gen();
        let _from_generic_launch = rt.next_gen();
        rt.set_gen(allocated);
        assert!(!rt.stale(allocated), "自动重连不能被自己多领的号判成过期");
    }

    #[test]
    fn ssh_auth_and_host_key_failures_are_terminal_but_connect_is_retryable() {
        use term_ssh::{AuthProblem, HostKeyProblem, SshError};
        assert!(matches!(
            exit_kind_for_ssh(&SshError::Auth(AuthProblem {
                user: "u".into(),
                method: "password",
                partial_success: false,
            })),
            ExitKind::Terminal
        ));
        assert!(matches!(
            exit_kind_for_ssh(&SshError::HostKey(HostKeyProblem::Unreadable("x".into()))),
            ExitKind::Terminal
        ));
        assert!(matches!(
            exit_kind_for_ssh(&SshError::Connect("refused".into())),
            ExitKind::Retryable
        ));
    }
}

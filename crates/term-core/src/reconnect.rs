//! 自动重连的**纯策略层**：类型与函数，无 IO、无墙钟、无协议依赖。
//!
//! 冻结的契约（用户 2026-11-26 定死，不在本文件里「顺手优化」）：
//!
//! - 默认关闭：策略 `enabled = false`，老 `Prefs.auto_reconnect` 也是 `false`；
//! - 全协议统一默认：`initial = 1s`、`backoff = ×2`、`max_delay = 30s`、
//!   `retry_window = 180s`、`max_attempts = 12`；
//! - `max_attempts` **只是溢出保护**，不是业务上限；真正的终止条件是
//!   `retry_window` 用尽。窗口从**首次断开**起算，每次尝试不重算，连上清零；
//! - 终止性失败（认证 / 主机密钥 / 私钥读取 / 配置 / 解析 / 本机环境 /
//!   本地 shell 正常退出 / 用户显式 Disconnect）立即停，不重连；
//! - 可重试：网络 Connect / Channel / 对端断开 / 链路重置，串口 hangup /
//!   `serial disconnected` / 设备暂缺。
//!
//! 本文件被 `session.rs` 以 `#[path = "reconnect.rs"]` 挂在
//! `term_core::session::reconnect` 下——`lib.rs` 不在这个改动的领地里，不新增
//! 顶层模块声明。所有类型都从 `session` 再导出一次，调用方写
//! `term_core::session::ReconnectPolicy` 即可。

use serde::{Deserialize, Serialize};

use crate::session_runtime::ExitKind;

/// 一条会话最终生效的重连策略。全部是纯值，秒数用 `u64` 而不是
/// `Duration`——`term-core` 不读墙钟，秒就够了，落盘也少一层嵌套。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReconnectPolicy {
    /// 总开关。出厂 `false`：重连会往远端重新发包，不能替用户默认打开。
    pub enabled: bool,
    /// 首次断开后的等待秒数（默认 1）。
    pub initial_secs: u64,
    /// 每次重试的倍率（默认 2）。`0` / `1` 在 [`effective`] 里被夹到 1。
    pub backoff: u32,
    /// 单次等待的上限秒数（默认 30）。
    pub max_delay_secs: u64,
    /// 重试窗口秒数，从首次断开起算（默认 180）。
    pub retry_window_secs: u64,
    /// 溢出保护上限（默认 12）。不是「第 12 次就不连了」的业务语义。
    pub max_attempts: u32,
}

impl Default for ReconnectPolicy {
    fn default() -> Self {
        Self {
            enabled: false,
            initial_secs: 1,
            backoff: 2,
            max_delay_secs: 30,
            retry_window_secs: 180,
            max_attempts: 12,
        }
    }
}

impl ReconnectPolicy {
    /// 只把总开关翻成 `enabled`，其余保持出厂值。
    ///
    /// 给旧调用方（`Tab::poll(..., bool, ...)`）用：它们只知道「开 / 关」，
    /// 参数仍走全局默认 + 会话覆盖，不会出现第二套退避口径。
    pub fn from_enabled(enabled: bool) -> Self {
        Self {
            enabled,
            ..Self::default()
        }
    }

    /// 老 `Prefs.auto_reconnect` 与新策略字段合并后的**全局**策略。
    ///
    /// 两个开关出厂都关，所以默认值仍是关；老文件里只有 `auto_reconnect=true`
    /// 时也不会因为新字段缺省而静默丢掉用户原来的选择。
    pub fn with_legacy(mut self, legacy_auto_reconnect: bool) -> Self {
        self.enabled |= legacy_auto_reconnect;
        self
    }
}

/// 每条会话对全局策略的覆盖。**每个字段都是 `Option`**：`None` = 沿用全局。
///
/// `#[serde(flatten)]` 进 `SessionConfig` 时，每个字段各自带 `#[serde(default)]`
/// ——老会话库缺这些键时读出来全是 `None`，行为与升级前一致，不会连坐
/// 同文件里的其它字段。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReconnectOverride {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub enabled: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub initial_secs: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub backoff: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_delay_secs: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retry_window_secs: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_attempts: Option<u32>,
}

/// 全局策略 + 会话覆盖 → 生效策略。`None` 的字段逐项回落到全局值。
pub fn effective(global: &ReconnectPolicy, over: &ReconnectOverride) -> ReconnectPolicy {
    ReconnectPolicy {
        enabled: over.enabled.unwrap_or(global.enabled),
        initial_secs: over.initial_secs.unwrap_or(global.initial_secs),
        // 倍率 0 / 1 都退化成「不加速」；避免手改 JSON 写一个 0 让延迟归零、
        // 在一帧里把同一个会话重连几十次。
        backoff: over.backoff.unwrap_or(global.backoff).max(1),
        max_delay_secs: over.max_delay_secs.unwrap_or(global.max_delay_secs),
        retry_window_secs: over.retry_window_secs.unwrap_or(global.retry_window_secs),
        max_attempts: over.max_attempts.unwrap_or(global.max_attempts),
    }
}

/// 第 `attempt` 次尝试（**从 0 起算**）该等多少秒。
///
/// `None` = 不再排：总开关关着，或已经到了溢出保护上限。窗口判定由调用方
/// 另做——`backoff` 只回答「这一次等多久」，不知道「第一次断开是什么时候」。
/// 延迟按 `initial × backoff^attempt` 增长并夹进 `max_delay`，用饱和乘避免
/// 手改一个巨大倍率后在 `u64` 上溢出。
pub fn backoff(policy: &ReconnectPolicy, attempt: u32) -> Option<u64> {
    if !policy.enabled || attempt >= policy.max_attempts {
        return None;
    }
    let factor = policy.backoff.max(1) as u64;
    let mut delay = policy.initial_secs.min(policy.max_delay_secs);
    for _ in 0..attempt {
        delay = delay.saturating_mul(factor).min(policy.max_delay_secs);
    }
    Some(delay)
}

/// 这个退出原因是否**终止性**的：终止性失败立即停，不进退避。
///
/// `Unknown` 不算终止：外部关掉会话时拿不到退出原因，按旧行为给一次重连
/// 机会比什么都不做更符合用户预期。
pub fn is_terminal(kind: ExitKind) -> bool {
    matches!(kind, ExitKind::Normal | ExitKind::Terminal)
}

/// 这个退出原因是否允许进入退避链。
pub fn allows_exit(kind: ExitKind) -> bool {
    !is_terminal(kind)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frozen_defaults_match_the_spec() {
        let p = ReconnectPolicy::default();
        assert!(!p.enabled);
        assert_eq!(p.initial_secs, 1);
        assert_eq!(p.backoff, 2);
        assert_eq!(p.max_delay_secs, 30);
        assert_eq!(p.retry_window_secs, 180);
        assert_eq!(p.max_attempts, 12);
    }

    #[test]
    fn disabled_policy_never_schedules() {
        assert_eq!(backoff(&ReconnectPolicy::default(), 0), None);
    }

    #[test]
    fn backoff_grows_then_caps_and_stops_at_the_overflow_guard() {
        let p = ReconnectPolicy::from_enabled(true);
        let delays: Vec<u64> = (0..p.max_attempts)
            .map(|a| backoff(&p, a).expect("未到上限就该有下一次"))
            .collect();
        assert_eq!(&delays[..6], &[1, 2, 4, 8, 16, 30]);
        assert!(delays.iter().all(|d| *d <= 30));
        assert_eq!(backoff(&p, p.max_attempts), None);
        assert_eq!(backoff(&p, u32::MAX), None);
    }

    #[test]
    fn effective_merges_only_the_fields_that_are_set() {
        let global = ReconnectPolicy {
            enabled: true,
            max_delay_secs: 9,
            ..ReconnectPolicy::default()
        };
        let over = ReconnectOverride {
            initial_secs: Some(3),
            backoff: None,
            ..ReconnectOverride::default()
        };
        let got = effective(&global, &over);
        assert!(got.enabled);
        assert_eq!(got.initial_secs, 3);
        assert_eq!(got.backoff, global.backoff);
        assert_eq!(got.max_delay_secs, 9);
    }

    #[test]
    fn a_zero_backoff_from_a_hand_edited_json_cannot_busy_loop() {
        let p = effective(
            &ReconnectPolicy::from_enabled(true),
            &ReconnectOverride { backoff: Some(0), ..ReconnectOverride::default() },
        );
        assert_eq!(p.backoff, 1);
        assert_eq!(backoff(&p, 3), Some(1));
    }

    #[test]
    fn terminal_exits_are_not_retried() {
        assert!(is_terminal(ExitKind::Terminal));
        assert!(is_terminal(ExitKind::Normal));
        assert!(!is_terminal(ExitKind::Retryable));
        assert!(!is_terminal(ExitKind::SerialLost));
        assert!(!is_terminal(ExitKind::Unknown));
        assert!(!allows_exit(ExitKind::Terminal));
    }
}

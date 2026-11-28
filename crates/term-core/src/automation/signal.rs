//! 世界的输入：信号、时钟刻度、规则关心的触发条件。
//!
//! 本模块不读时钟——`Ticks` 由宿主注入。`Duration` 是纯时长量，
//! 与 `session_runtime.rs` 的等待原语同一性质，不破"零副作用"。
//!
//! 旧设计把六种触发通道塞进 `TriggerOn` 并与 11 种 `TriggerKind` 做映射，
//! 其中三条对产品不可达。这里把「信号」与「关心哪种信号」各收成一个类型，
//! 六条通道一一对应，没有不可达项。

use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::automation::cursor::ScanCursor;
use crate::automation::pattern::{Pattern, Span};

/// 单调时钟刻度，单位**毫秒**。用 newtype 而不是裸 `u64`，避免秒/毫秒混用。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct Ticks(pub u64);

impl Ticks {
    pub const ZERO: Self = Self(0);

    pub const fn from_millis(v: u64) -> Self {
        Self(v)
    }
    pub fn from_secs(v: u64) -> Self {
        Self(v.saturating_mul(1000))
    }
    pub fn from_duration(d: Duration) -> Self {
        Self(d.as_millis() as u64)
    }
    /// 两刻度之差，以 `Duration` 表达（冷却 / 空闲判定共用）。
    pub fn elapsed_since(self, earlier: Self) -> Duration {
        Duration::from_millis(self.0.saturating_sub(earlier.0))
    }
}

/// 会话结束的原因。规则可以用它区分正常退出与异常断开。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum CloseReason {
    Normal,
    Error(String),
}

/// 会话生命周期事件，`Signal` 那四条事件通道的**拥有型**投影。
///
/// 宿主钩子（例如 `SessionPool` 的连接事件接收端）跨线程传递的是它，不是带
/// 借用的 `Signal`；四类事件与 `When` 的四个事件分支一一对应。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Lifecycle {
    BeforeConnect,
    Connected,
    BeforeDisconnect,
    Disconnected,
}

/// 一次外部刺激。求值器只认这个类型，不认"帧" / "轮询" / "事件"这些来源。
#[derive(Clone, Debug, PartialEq)]
pub enum Signal<'a> {
    /// 一段已清洗的输出（转义序列已剥掉）。
    Output {
        stream: &'a str,
        text: &'a str,
        at: Ticks,
    },
    Connected { stream: &'a str, at: Ticks },
    Disconnected {
        stream: &'a str,
        at: Ticks,
        reason: CloseReason,
    },
    BeforeConnect { stream: &'a str },
    BeforeDisconnect { stream: &'a str },
    /// 连续 `quiet` 时间没有新输出。
    Idle {
        stream: &'a str,
        quiet: Duration,
        at: Ticks,
    },
}

impl<'a> Signal<'a> {
    pub fn stream(&self) -> &'a str {
        match self {
            Self::Output { stream, .. }
            | Self::Connected { stream, .. }
            | Self::Disconnected { stream, .. }
            | Self::BeforeConnect { stream }
            | Self::BeforeDisconnect { stream }
            | Self::Idle { stream, .. } => stream,
        }
    }

    pub fn at(&self) -> Option<Ticks> {
        match self {
            Self::Output { at, .. }
            | Self::Connected { at, .. }
            | Self::Disconnected { at, .. }
            | Self::Idle { at, .. } => Some(*at),
            Self::BeforeConnect { .. } | Self::BeforeDisconnect { .. } => None,
        }
    }
}

/// 规则作用的目标（会话所在系统 + 会话名）。名字跨进程稳定（ADR-0024）。
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Target {
    pub system: String,
    pub session: String,
}

/// 规则关心哪一种信号。`Pattern` 在这里已是编译好的。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum When {
    Output(Pattern),
    Connected,
    Disconnected,
    BeforeConnect,
    BeforeDisconnect,
    Idle { after: Duration },
}

/// `When` 对一条信号的判定结果。文本类命中带 `Span`，事件类不带。
#[derive(Clone, Debug, PartialEq)]
pub enum Verdict {
    No,
    Yes(Option<Span>),
}

impl When {
    /// 对一条信号判定。`Output` 分支必须经 [`ScanCursor::probe`]：规则求值发生在
    /// 每帧的清洗文本上，而一个模式完全可以跨两帧（`"pass"` + `"word:"`）。
    /// `cursor` 由求值器按会话持有并在每次 Output 前 `push(text)`；`probe` 的
    /// `scan` 指针同时负责跨帧拼接与「同一命中不重复报」。
    pub fn evaluate(&self, signal: &Signal, cursor: &mut ScanCursor) -> Verdict {
        self.evaluate_keyed(signal, cursor, "")
    }

    /// 求值器路径：用规则 id 作 key，让多条规则在同一份 cursor 上各有自己的
    /// probe 指针。两条规则同时盯 "password:" 时都要命中，由仲裁器决定压制。
    pub fn evaluate_keyed(
        &self,
        signal: &Signal,
        cursor: &mut ScanCursor,
        key: &str,
    ) -> Verdict {
        match (self, signal) {
            (Self::Output(p), Signal::Output { .. }) => {
                match cursor.probe_keyed(key, p) {
                    Some(span) => Verdict::Yes(Some(span)),
                    None => Verdict::No,
                }
            }
            (Self::Connected, Signal::Connected { .. }) => Verdict::Yes(None),
            (Self::Disconnected, Signal::Disconnected { .. }) => Verdict::Yes(None),
            (Self::BeforeConnect, Signal::BeforeConnect { .. }) => Verdict::Yes(None),
            (Self::BeforeDisconnect, Signal::BeforeDisconnect { .. }) => Verdict::Yes(None),
            (Self::Idle { after }, Signal::Idle { quiet, .. }) => {
                if quiet >= after {
                    Verdict::Yes(None)
                } else {
                    Verdict::No
                }
            }
            _ => Verdict::No,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::automation::pattern::Syntax;

    #[test]
    fn ticks_units_do_not_get_mixed() {
        assert_eq!(Ticks::from_secs(2), Ticks::from_millis(2000));
        assert_eq!(Ticks::from_duration(Duration::from_millis(7)), Ticks::from_millis(7));
        assert_eq!(
            Ticks::from_millis(110).elapsed_since(Ticks::from_millis(100)),
            Duration::from_millis(10)
        );
        assert_eq!(Ticks::from_millis(3).elapsed_since(Ticks::from_millis(9)), Duration::ZERO);
    }

    #[test]
    fn every_channel_accepts_only_its_own_signal() {
        let stream = "s1";
        let at = Ticks::from_millis(42);
        let cases: Vec<(When, Signal<'_>)> = vec![
            (When::Connected, Signal::Connected { stream, at }),
            (When::Disconnected, Signal::Disconnected {
                stream, at, reason: CloseReason::Normal,
            }),
            (When::BeforeConnect, Signal::BeforeConnect { stream }),
            (When::BeforeDisconnect, Signal::BeforeDisconnect { stream }),
            (When::Idle { after: Duration::from_secs(1) }, Signal::Idle {
                stream, quiet: Duration::from_secs(2), at,
            }),
        ];
        let mut cursor = ScanCursor::new(64);
        for (when, signal) in &cases {
            assert_eq!(when.evaluate(signal, &mut cursor), Verdict::Yes(None), "{when:?}");
        }
        // 串台：Connected 规则不该被 Disconnected 信号触发。
        assert_eq!(
            When::Connected.evaluate(
                &Signal::Disconnected {
                    stream, at, reason: CloseReason::Normal,
                },
                &mut cursor
            ),
            Verdict::No
        );
    }

    #[test]
    fn output_rule_returns_the_span() {
        let when = When::Output(Pattern::compile("panic", Syntax::Literal, true).unwrap());
        let signal = Signal::Output {
            stream: "s",
            text: "kernel panic: x",
            at: Ticks::ZERO,
        };
        let mut cursor = ScanCursor::new(64);
        cursor.push("kernel panic: x");
        assert_eq!(
            when.evaluate(&signal, &mut cursor),
            Verdict::Yes(Some(Span { start: 7, end: 12 }))
        );
    }

    #[test]
    fn idle_respects_the_threshold() {
        let when = When::Idle { after: Duration::from_secs(30) };
        let short = Signal::Idle {
            stream: "s", quiet: Duration::from_secs(29), at: Ticks::ZERO,
        };
        let long = Signal::Idle {
            stream: "s", quiet: Duration::from_secs(30), at: Ticks::ZERO,
        };
        let mut cursor = ScanCursor::new(64);
        assert_eq!(when.evaluate(&short, &mut cursor), Verdict::No);
        assert_eq!(when.evaluate(&long, &mut cursor), Verdict::Yes(None));
    }

    #[test]
    fn signal_exposes_stream_and_time_uniformly() {
        let s = Signal::Output { stream: "s1", text: "x", at: Ticks::from_millis(9) };
        assert_eq!(s.stream(), "s1");
        assert_eq!(s.at(), Some(Ticks::from_millis(9)));
        let b = Signal::BeforeConnect { stream: "s2" };
        assert_eq!(b.stream(), "s2");
        assert_eq!(b.at(), None);
    }
}

//! 响铃的宿主侧策略：去重 + 发声。
//!
//! 语义层（`term-core::vt`）只递增 `bell_seq`，不读墙钟也不碰音频。这里把
//! 「多少秒内只响一次」与「怎么响」放在一起：去重时钟用 `Instant`（单调，
//! 不受系统时间调整影响），发声走系统自带的小工具，**不经 shell**，所以
//! 用户填的声音文件路径不会被当成命令拼进去。任何失败都静默——响铃是提示，
//! 不该因为机器上没装 `afplay` 就让会话报错。

use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use term_core::session::SessionConfig;

/// 会话页的三档响铃方式。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BellStyle {
    Mute,
    Beep,
    Sound,
}

impl BellStyle {
    fn from_index(i: usize) -> Self {
        match i {
            1 => Self::Beep,
            2 => Self::Sound,
            _ => Self::Mute,
        }
    }
}

/// 从会话配置解析出来的响铃策略。
#[derive(Clone, Debug)]
pub struct BellPolicy {
    style: BellStyle,
    sound: String,
    /// `None` = 不去重（窗口 0 秒）。
    dedup: Option<Duration>,
}

impl BellPolicy {
    pub fn from_config(config: &SessionConfig) -> Self {
        Self {
            style: BellStyle::from_index(config.bell_style),
            sound: config.bell_sound.trim().to_string(),
            dedup: if config.bell_dedup_secs == 0 {
                None
            } else {
                Some(Duration::from_secs(u64::from(config.bell_dedup_secs)))
            },
        }
    }

    /// 造一个去重门。每个会话一份，跟着监管者线程走。
    pub fn gate(&self) -> BellGate {
        BellGate { policy: self.clone(), last: None }
    }

    /// 真的发一次声。失败静默见模块头。
    fn play(&self) {
        match self.style {
            BellStyle::Mute => {}
            BellStyle::Beep => {
                #[cfg(target_os = "macos")]
                let _ = Command::new("osascript")
                    .args(["-e", "beep"])
                    .stdout(Stdio::null())
                    .stderr(Stdio::null())
                    .spawn();
                #[cfg(target_os = "linux")]
                let _ = Command::new("beep")
                    .stdout(Stdio::null())
                    .stderr(Stdio::null())
                    .spawn();
            }
            BellStyle::Sound => {
                if self.sound.is_empty() {
                    return;
                }
                #[cfg(target_os = "macos")]
                let _ = Command::new("afplay")
                    .arg(&self.sound)
                    .stdout(Stdio::null())
                    .stderr(Stdio::null())
                    .spawn();
                #[cfg(target_os = "linux")]
                let _ = Command::new("paplay")
                    .arg(&self.sound)
                    .stdout(Stdio::null())
                    .stderr(Stdio::null())
                    .spawn();
            }
        }
    }
}

/// 一个会话的去重门：返回 `true` 表示这次 BEL 真的响了。
pub struct BellGate {
    policy: BellPolicy,
    last: Option<Instant>,
}

impl BellGate {
    pub fn on_bell(&mut self) -> bool {
        let now = Instant::now();
        if let (Some(last), Some(window)) = (self.last, self.policy.dedup) {
            if now.duration_since(last) < window {
                return false;
            }
        }
        self.last = Some(now);
        self.policy.play();
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use term_core::session::Protocol;

    fn policy(style: usize, dedup: u32) -> BellPolicy {
        let mut c = SessionConfig::new(Protocol::Shell);
        c.bell_style = style;
        c.bell_dedup_secs = dedup;
        BellPolicy::from_config(&c)
    }

    #[test]
    fn a_mute_bell_never_plays_but_still_counts_as_an_event() {
        // 静音只是不发声，去重门照样收下事件——「静音」不是「关掉 BEL」。
        let mut gate = policy(0, 0).gate();
        assert!(gate.on_bell());
        assert!(gate.on_bell());
    }

    #[test]
    fn a_long_window_suppresses_the_second_bell() {
        // 窗口设成 60 秒：同一份门上连续两次 BEL 必有一次被吞。
        let mut gate = policy(0, 60).gate();
        assert!(gate.on_bell());
        assert!(!gate.on_bell(), "去重窗口内的第二次不该再响");
    }

    #[test]
    fn a_zero_window_never_suppresses() {
        let mut gate = policy(0, 0).gate();
        for _ in 0..5 {
            assert!(gate.on_bell());
        }
    }
}

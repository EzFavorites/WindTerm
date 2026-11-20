//! 启动会话策略与「激活即连」判定（W1-a，纯语义层）。
//!
//! # 依据
//!
//! 官方公开文档 `windterm-docs-md/2020/01/22/restore-sessions/index.md:27`：
//! 「All last sessions will be restored after restart, and the session only
//! connects to target when its view is actived」。视觉基线
//! `No session / Last sessions / A specific session or set of sessions`，
//! 外加 Session 1–4 四个下拉。
//!
//! 两句合起来定了一个容易做反的边界：**恢复的是结构，不是进程**。
//! 启动时把 N 条 SSH 全连上会连着弹指纹窗与密码窗；参考实现把这些标签摆回
//! 断开态，等用户真的切到某个视图时才连那一条。所以本模块只回答两件事：
//!
//! 1. 这次启动该摆回哪些标签（[`StartupPolicy::tabs_to_restore`]）；
//! 2. 这一帧该连哪一条（[`slot_needing_connect`]）。
//!
//! 连接本身、弹窗、凭据注入都在投影层；这里没有 IO，也不认识 `egui`。
//!
//! # 为什么 tag 用字符串而不是序号
//!
//! `prefs.json` 的 `startup_mode` 会用 `#[serde(default)]` 读：老文件缺键时
//! 值是空串。若用 `u8`，`0` 就必须恰好等于「出厂默认那一档」，
//! 以后想把默认调到别的档就要 migration。空串 / 未知值统一回落到
//! `LastSessions`（视觉基线 `01-application.png` 里选中的正是这一档），
//! 老文件与手改坏的文件都不会把启动行为改掉。

/// 启动时要恢复什么。顺序 = 设置页三个单选的显示顺序。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum StartupMode {
    /// 不恢复上次的会话结构。
    NoSession,
    /// 恢复 `layout.json` 里记录的全部标签（参考实现出厂态）。
    /// `#[default]` 直接钉在变体上：老 prefs 缺键必须落到这一档，
    /// 而不是「枚举第一项」这种碰巧的默认值。
    #[default]
    LastSessions,
    /// 只开「Session 1–4」里指定的那几条，与上次布局无关。
    SpecificSessions,
}

impl StartupMode {
    /// 设置页的显示顺序。UI 不该自己再写一份名单。
    pub const ALL: [StartupMode; 3] = [
        StartupMode::NoSession,
        StartupMode::LastSessions,
        StartupMode::SpecificSessions,
    ];

    /// 落盘用的规范标签。空串不在这里出现（那是「老文件没这项」）。
    pub fn tag(self) -> &'static str {
        match self {
            StartupMode::NoSession => "none",
            StartupMode::LastSessions => "last",
            StartupMode::SpecificSessions => "specific",
        }
    }

    /// 读盘。空串 / 未知值 = 出厂默认，不是错误。
    pub fn parse_tag(tag: &str) -> StartupMode {
        match tag.trim() {
            "none" => StartupMode::NoSession,
            "specific" => StartupMode::SpecificSessions,
            // "last" 与一切别的值（含空串）都回落到默认档。
            _ => StartupMode::LastSessions,
        }
    }

    /// 设置页上的英文原文（视觉基线逐字抄的，不进 i18n 词表）。
    pub fn label(self) -> &'static str {
        match self {
            StartupMode::NoSession => "No session",
            StartupMode::LastSessions => "Last sessions",
            StartupMode::SpecificSessions => "A specific session or set of sessions",
        }
    }
}

/// 一条完整的启动策略。四个下拉固定四位，与视觉基线一致。
///
/// `Default` 能 derive 是因为 `StartupMode::default()` 已经是 `LastSessions`；
/// 枚举那里必须手写（默认不是第一项），这里不必再写一遍。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct StartupPolicy {
    pub mode: StartupMode,
    /// Session 1–4。空串 = 该行没指定；重复只保留第一次。
    pub specific: [String; 4],
}

impl StartupPolicy {
    /// 从 `prefs.json` 的两个字段构造。`specific` 长度不必正好是 4
    /// （手改文件可能写长写短），缺的补空、多的丢掉，不 panic。
    pub fn from_tags(mode: &str, specific: &[String]) -> Self {
        let mut slots: [String; 4] = Default::default();
        for (slot, value) in slots.iter_mut().zip(specific.iter()) {
            *slot = value.clone();
        }
        StartupPolicy {
            mode: StartupMode::parse_tag(mode),
            specific: slots,
        }
    }

    /// 这次启动要摆回哪些**会话名**（按打开顺序）。
    ///
    /// - `NoSession` → 空：不覆盖启动时那个默认本地 shell；
    /// - `LastSessions` → 原样返回上次布局的标签顺序（同名标签允许重复，
    ///   那是用户真的开过两条同名会话，不是脏数据）；
    /// - `SpecificSessions` → 四个下拉里非空、去重、保序的名字；
    ///   全空时也是空（等价于「没指定」），**绝不悄悄退回恢复全部**——
    ///   那样用户选了「指定会话」却看到昨天那堆标签，是最坏的一种意外。
    ///
    /// 会话库里是否还存在这些名字由投影层过滤（那一层才拿得到会话库）。
    pub fn tabs_to_restore(&self, saved_layout: &[String]) -> Vec<String> {
        match self.mode {
            StartupMode::NoSession => Vec::new(),
            StartupMode::LastSessions => saved_layout.to_vec(),
            StartupMode::SpecificSessions => {
                let mut out: Vec<String> = Vec::new();
                for name in &self.specific {
                    let name = name.trim();
                    if !name.is_empty() && !out.iter().any(|n| n == name) {
                        out.push(name.to_string());
                    }
                }
                out
            }
        }
    }
}

/// 一条标签在「激活即连」判定里需要的两个事实。
///
/// 故意不把 `SessionId` / 配置 / 窗口控件带进来：这里只关心
/// 「是不是还没连过的恢复标签」与「此刻是不是断开态」。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RestoreTab {
    /// 由本次启动的恢复流程登记、且还没有尝试过连接。
    pub pending: bool,
    /// `session == None`（内置 SSH 恢复出来就是这个状态）。
    pub disconnected: bool,
}

/// 这一帧该连哪条标签。**只可能是当前激活的那一条，或者一条都没有。**
///
/// 返回值恒为 `Some(active)` 或 `None`，这是「启动不弹 N 个窗」的结构保证：
/// 非激活标签再怎么 `pending` 也不会触发连接。已经连上的标签也不再触发，
/// 免得用户来回切标签时每次都重连一遍。
pub fn slot_needing_connect(tabs: &[RestoreTab], active: usize) -> Option<usize> {
    let tab = *tabs.get(active)?;
    (tab.pending && tab.disconnected).then_some(active)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn policy(mode: StartupMode, specific: [&str; 4]) -> StartupPolicy {
        StartupPolicy {
            mode,
            specific: specific.map(str::to_string),
        }
    }

    #[test]
    fn the_factory_default_is_last_sessions() {
        // 视觉基线 01-application.png 里选中的是第二档；老 prefs 缺键也要落到它。
        assert_eq!(StartupMode::default(), StartupMode::LastSessions);
        assert_eq!(StartupPolicy::default().mode, StartupMode::LastSessions);
        assert!(StartupPolicy::default().specific.iter().all(String::is_empty));
    }

    #[test]
    fn every_tag_round_trips() {
        for mode in StartupMode::ALL {
            assert_eq!(StartupMode::parse_tag(mode.tag()), mode);
            assert!(!mode.label().is_empty());
        }
        assert_eq!(StartupMode::ALL.len(), 3);
    }

    #[test]
    fn empty_and_unknown_tags_fall_back_to_last_sessions() {
        // 老文件（空串）、手改坏的文件、将来删掉某档的旧版本，都不能改掉启动行为。
        for tag in ["", "  ", "LAST", "last", "garbage", "0"] {
            assert_eq!(
                StartupMode::parse_tag(tag),
                StartupMode::LastSessions,
                "tag={tag:?}"
            );
        }
        assert_eq!(
            StartupMode::parse_tag(" last "),
            StartupMode::LastSessions,
            "带空格的合法值也要认"
        );
    }

    #[test]
    fn from_tags_pads_and_truncates_without_panicking() {
        let short = StartupPolicy::from_tags("specific", &["a".into(), "b".into()]);
        assert_eq!(short.mode, StartupMode::SpecificSessions);
        assert_eq!(short.specific[0], "a");
        assert_eq!(short.specific[1], "b");
        assert!(short.specific[2].is_empty());
        assert!(short.specific[3].is_empty());
        let long: Vec<String> = (0..9).map(|i| format!("s{i}")).collect();
        let long = StartupPolicy::from_tags("none", &long);
        assert_eq!(long.specific[3], "s3", "多出来的行丢掉，不越界");
    }

    #[test]
    fn no_session_restores_nothing_even_with_a_saved_layout() {
        let saved = ["db".into(), "web".into(), "shell".into()];
        assert!(policy(StartupMode::NoSession, ["db", "", "", ""])
            .tabs_to_restore(&saved)
            .is_empty());
    }

    #[test]
    fn last_sessions_restores_the_exact_saved_order_and_duplicates() {
        let saved = ["db".into(), "web".into(), "db".into()];
        let got = policy(StartupMode::LastSessions, ["web", "", "", ""])
            .tabs_to_restore(&saved);
        assert_eq!(got, saved, "指定档的四个下拉不该影响这一档");
    }

    #[test]
    fn specific_sessions_skip_blanks_and_duplicates_keep_order() {
        let p = policy(
            StartupMode::SpecificSessions,
            [" web ", "", "web", "db"],
        );
        assert_eq!(p.tabs_to_restore(&["old".into()]), vec!["web", "db"]);
    }

    #[test]
    fn specific_sessions_ignores_the_saved_layout_instead_of_falling_back() {
        // 四个下拉全空 = 一条都不恢复。悄悄退回「恢复全部」是最坏的意外。
        let saved = ["db".into(), "web".into()];
        assert!(policy(StartupMode::SpecificSessions, ["", " ", "", ""])
            .tabs_to_restore(&saved)
            .is_empty());
    }

    #[test]
    fn only_the_active_pending_disconnected_tab_connects() {
        let tabs = [
            RestoreTab { pending: true, disconnected: true },
            RestoreTab { pending: true, disconnected: true },
            RestoreTab { pending: true, disconnected: true },
        ];
        // 三个都待连，但一帧只许连激活的那一个。
        for active in 0..3 {
            assert_eq!(slot_needing_connect(&tabs, active), Some(active));
        }
    }

    #[test]
    fn connected_or_attempted_tabs_never_reconnect_on_tab_switch() {
        let alive = RestoreTab { pending: true, disconnected: false };
        let attempted = RestoreTab { pending: false, disconnected: true };
        assert_eq!(slot_needing_connect(&[alive], 0), None);
        assert_eq!(slot_needing_connect(&[attempted], 0), None);
        // 非激活标签再怎么 pending 也不该被连。
        let pending = RestoreTab { pending: true, disconnected: true };
        let active_alive = RestoreTab { pending: false, disconnected: false };
        assert_eq!(slot_needing_connect(&[pending, active_alive], 1), None);
    }

    #[test]
    fn an_out_of_range_active_slot_is_none_not_a_panic() {
        let tabs = [RestoreTab { pending: true, disconnected: true }];
        assert_eq!(slot_needing_connect(&tabs, 9), None);
        assert_eq!(slot_needing_connect(&[], 0), None);
    }
}

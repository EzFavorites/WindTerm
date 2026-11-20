//! 首次启动引导的**纯逻辑**——零 `egui`、零 IO、零线程、零解析依赖。
//!
//! `docs/design/09-session-import-and-onboarding.md` 要求：首次启动时探测 `~/.ssh/config` 与 WindTerm 的
//! `profiles/`，把认出来的主机**预填**进会话树，点一条就走既有的 TOFU 信任流程。
//! 这一层只回答两件事，一件比一件纯：
//!
//! 1. **主机 → 会话**：[`DetectedHost::into_session`]（`protocol = Ssh`、
//!    `builtin_ssh = true`，与侧栏导入同一条落地路径）。
//! 2. **走几步、勾了哪些**：[`OnboardingState`]——一个 `Welcome → Detect → Done`
//!    的三态机，加一条与 `detected` 等长的勾选位。
//!
//! # 为什么「文本 → 主机」不在这里
//!
//! 找文件的 IO 与文本解析都在投影层（`apps/desktop` 的引导浮层）：那边复用全仓唯一
//! 那份 ssh config 解析器，把解析结果摊成 [`DetectedHost`] 再交给
//! [`OnboardingState::load_detected`]。于是这里既没有第二个解析器，也不必为了一个
//! 薄适配器把带 `std::fs` / `dirs` / `current_exe()` 的 crate 拖进无前端应用层——
//! `term-app` 的依赖表保持只有 `term-core`，ADR-0016 的「依赖表即编译器强制」
//! 才不是一句空话。
//!
//! # 为什么 `note` 是 `&'static str` 而不是 `String`
//!
//! 它是要**显示给用户的一句话**，所以必须能进词表。`i18n::t(lang, en)` 只收
//! `&'static str`（key 就是英文原文），一句 `String` 进不了词表，只会在中文界面
//! 上露出英文。类型写成 `&'static str` 就把「这句话必须登记进词表」变成了编译期
//! 事实，而不是靠 review 记性。

use term_core::session::{Protocol, SessionConfig};

/// 引导的一步。
///
/// 只有三页，且 `Done` 不是「关闭」——它是「这一遍走完了」，由投影层收尾
/// （写 `prefs.onboarded`、关窗口）。分开的理由：`skip()` 也要走到 `Done`，
/// 而「用户按了跳过」与「窗口被 × 掉」在状态机里是同一种结局、在投影层是两条
/// 不同的收尾分支。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Step {
    /// 第一页：一两句说明 + Next / Skip。
    #[default]
    Welcome,
    /// 第二页：探测到的主机清单（勾选框）+ Back / Import N / Skip。
    Detect,
    /// 走完或跳过。**不再前进**，投影层见到它就收尾。
    Done,
}

/// 从 ssh config（或 WindTerm `profiles/`）里认出来的一台主机。
///
/// 只带**能连上**的四个字段加一个来源：密码 / 口令 / 私钥内容一律不进来
/// （与外部会话导入同一条底线）。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DetectedHost {
    /// 会话名。`Host` 的别名；没有别名时是 hostname。
    pub label: String,
    /// `HostName`；没有则回落成别名（`ssh web` 的 `web` 本身就能解析）。
    pub host: String,
    /// `User`。**可空**——ssh 会回落到登录名，导入时也留空，不要编一个出来。
    pub user: String,
    /// `Port`。没有就是 22（`term_core::config::SSH_PORT`）。
    pub port: u16,
    /// 来源文件路径。引导页要显示「这些是从哪来的」，所以它必须跟着每条走，
    /// 而不是在状态机里存一份（两个来源混在一个列表里时会串）。
    pub source: String,
}

impl DetectedHost {
    /// `user@host:port` 的展示串（`user` 为空时省掉前缀）。
    ///
    /// 端口是 22 时也照常显示：用户刚从 ssh config 里认出来的东西，
    /// 「为什么这里少一截」比「多三个字符」更费解。
    pub fn target(&self) -> String {
        if self.user.is_empty() {
            format!("{}:{}", self.host, self.port)
        } else {
            format!("{}@{}:{}", self.user, self.host, self.port)
        }
    }

    /// 落成会话库里的形状。
    ///
    /// `name` 由调用方给：撞名时它要换成 `unique_name` 的结果，那个判定需要
    /// 现有会话表，本层看不到。
    ///
    /// `builtin_ssh` 显式写 `true`：`SessionConfig::new(Protocol::Ssh)` 已经这么设了，
    /// 这里再写一遍是**表态**——引导导入的会话就走进程内 SSH（与侧栏
    /// `uses_builtin_ssh` 同一条路），将来 `new` 的默认值变了，这里不会跟着变。
    pub fn into_session(&self, name: String) -> SessionConfig {
        let mut cfg = SessionConfig::new(Protocol::Ssh);
        cfg.name = name;
        cfg.host = self.host.clone();
        cfg.port = self.port;
        cfg.username = self.user.clone();
        cfg.builtin_ssh = true;
        // 分组由投影层填（它才知道 `groups` 里现在有什么）。
        cfg
    }
}

/// 没探测到任何主机时给用户看的那句话（key 就是英文原文，中文在词表里）。
pub const NOTE_NOTHING_FOUND: &str =
    "No SSH hosts found — you can still add sessions from the sidebar later.";

/// 引导的纯状态机。
///
/// 字段全部公开（与 `ViewState` / `FindState` 同一个范式）：投影层只读它来画，
/// 改动一律经本文件的方法，于是「第几步、勾了哪些」只有一处可改。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct OnboardingState {
    /// 引导层是否可见。与 `ViewState::show_onboarding` **恒等**，两边在同一处
    /// 一起改（投影层的 `open_onboarding` / `finish_onboarding`）；这里留一份是
    /// 因为状态机自己要知道「现在算不算开着」才能收尾。
    pub open: bool,
    /// 当前第几步。
    pub step: Step,
    /// 探测到的主机，顺序即来源文件的顺序（ssh config 在前）。
    pub detected: Vec<DetectedHost>,
    /// 与 `detected` 等长：用户在 Detect 页勾了哪些要导入。
    pub accepted: Vec<bool>,
    /// 探测失败 / 没有可导入项时给用户看的一句话（**不弹 toast**：
    /// 那句话就画在清单原本的位置上，用户不必去别处找）。
    pub note: Option<&'static str>,
}

impl OnboardingState {
    /// 出厂态：关着、停在 Welcome、什么都没有。
    pub fn new() -> Self {
        Self::default()
    }

    /// 打开引导，并**回到第一页**。
    ///
    /// 为什么重开要重置页号：这是一次性引导，用户跳过之后不会再来回走第二遍；
    /// 「记住上次停在哪一步」只会让下次从中间开始，反而要在投影层多存一个状态。
    pub fn open(&mut self) {
        self.open = true;
        self.step = Step::Welcome;
    }

    /// 前进。`Done` 不再前进（它之后由投影层收尾，不是再翻一页）。
    pub fn next(&mut self) {
        self.step = match self.step {
            Step::Welcome => Step::Detect,
            Step::Detect => Step::Done,
            Step::Done => Step::Done,
        };
    }

    /// 后退。`Welcome` 不再后退。
    pub fn back(&mut self) {
        self.step = match self.step {
            Step::Welcome => Step::Welcome,
            Step::Detect => Step::Welcome,
            Step::Done => Step::Detect,
        };
    }

    /// 用户跳过：直接到 `Done`。**不是关闭**——关闭与写 `prefs.onboarded`
    /// 由投影层在见到 `Done` 时统一做，于是「走完」与「跳过」共用同一条收尾路径。
    pub fn skip(&mut self) {
        self.step = Step::Done;
    }

    /// 关掉引导层（不动页号；`open()` 会重置）。
    pub fn close(&mut self) {
        self.open = false;
    }

    /// 装载探测结果：全部默认勾选，`note` 按 `detected` 是否为空给出。
    ///
    /// 默认全勾是有意的：这是「帮你把已有的东西搬过来」，用户走进 Detect 页
    /// 的意图本来就是导入；要排除的那几条自己取消比全选一遍省事得多。
    pub fn load_detected(&mut self, hosts: Vec<DetectedHost>) {
        self.accepted = vec![true; hosts.len()];
        self.note = if hosts.is_empty() {
            Some(NOTE_NOTHING_FOUND)
        } else {
            None
        };
        self.detected = hosts;
    }

    /// 用户点了「导入」：返回要导入的那些，**保持 `detected` 的顺序**。
    ///
    /// 顺序有意义：来源文件里靠前的主机通常更常用，导入后会话树里
    /// `sort_sessions` 之前的那一瞬、以及同名去重的先后，都按这个顺序。
    pub fn take_accepted(&self) -> Vec<&DetectedHost> {
        self.detected
            .iter()
            .zip(self.accepted.iter())
            .filter(|(_, on)| **on)
            .map(|(host, _)| host)
            .collect()
    }

    /// 勾选 / 取消第 `index` 条。
    ///
    /// 越界**静默不动**而不是 panic：`index` 来自绘制层的行号，而
    /// `load_detected` 可能在两帧之间把列表换短（重新探测）。为一个迟早会发生的
    /// 竞态让整个 GUI abort，代价远大于「这一下没勾上」。
    pub fn toggle(&mut self, index: usize) {
        if let Some(slot) = self.accepted.get_mut(index) {
            *slot = !*slot;
        }
    }

    /// 勾了几条（「Import N sessions」按钮的数字）。
    pub fn accepted_count(&self) -> usize {
        self.accepted.iter().filter(|on| **on).count()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── OnboardingState ───────────────────────────────────────────────────

    fn host(label: &str) -> DetectedHost {
        DetectedHost {
            label: label.to_string(),
            host: format!("{label}.example"),
            user: "u".to_string(),
            port: 22,
            source: "s".to_string(),
        }
    }

    #[test]
    fn a_new_state_is_closed_at_welcome() {
        let st = OnboardingState::new();
        assert!(!st.open, "出厂是关着的");
        assert_eq!(st.step, Step::Welcome);
        assert!(st.detected.is_empty());
        assert!(st.accepted.is_empty());
        assert!(st.note.is_none());

        let mut st = OnboardingState::new();
        st.open();
        assert!(st.open);
        assert_eq!(st.step, Step::Welcome, "打开总是回到第一页");
    }

    #[test]
    fn next_and_back_stop_at_both_ends() {
        let mut st = OnboardingState::new();
        // Welcome 不能再后退。
        st.back();
        assert_eq!(st.step, Step::Welcome);

        st.next();
        assert_eq!(st.step, Step::Detect);
        st.next();
        assert_eq!(st.step, Step::Done);
        // Done 不能再前进，也不能靠 back 回到 Welcome 之外的地方。
        st.next();
        assert_eq!(st.step, Step::Done);
        st.back();
        assert_eq!(st.step, Step::Detect);
        st.back();
        assert_eq!(st.step, Step::Welcome);
        st.back();
        assert_eq!(st.step, Step::Welcome);
    }

    #[test]
    fn skip_jumps_straight_to_done() {
        let mut st = OnboardingState::new();
        st.open();
        st.next();
        assert_eq!(st.step, Step::Detect);
        st.skip();
        assert_eq!(st.step, Step::Done, "跳过 = 直接走完，不是关闭");
        assert!(st.open, "skip 不动 open：收尾由投影层做");
    }

    #[test]
    fn load_detected_accepts_everything_by_default() {
        let mut st = OnboardingState::new();
        st.load_detected(vec![host("a"), host("b"), host("c")]);
        assert_eq!(st.accepted, vec![true, true, true]);
        assert_eq!(st.accepted_count(), 3);
        assert!(st.note.is_none(), "探到了东西就不该有那句提示");
    }

    #[test]
    fn load_detected_with_nothing_sets_a_note() {
        let mut st = OnboardingState::new();
        st.load_detected(Vec::new());
        assert!(st.detected.is_empty());
        assert!(st.accepted.is_empty());
        let note = st.note.expect("空结果要给用户一句话");
        assert_eq!(note, NOTE_NOTHING_FOUND);
        assert_eq!(st.accepted_count(), 0);
    }

    #[test]
    fn take_accepted_returns_only_the_ticked_ones_in_order() {
        let mut st = OnboardingState::new();
        st.load_detected(vec![host("a"), host("b"), host("c")]);
        st.toggle(1); // 取消 b
        let picked = st.take_accepted();
        let labels: Vec<&str> = picked.iter().map(|h| h.label.as_str()).collect();
        assert_eq!(labels, vec!["a", "c"], "只返回勾上的，且保持原顺序");
        assert_eq!(st.accepted_count(), 2);

        st.toggle(1); // 又勾上 b
        let labels: Vec<&str> = st.take_accepted().iter().map(|h| h.label.as_str()).collect();
        assert_eq!(labels, vec!["a", "b", "c"]);
    }

    #[test]
    fn toggle_out_of_range_does_nothing() {
        let mut st = OnboardingState::new();
        st.load_detected(vec![host("a")]);
        st.toggle(9); // 不 panic
        assert_eq!(st.accepted, vec![true]);
        st.toggle(usize::MAX);
        assert_eq!(st.accepted, vec![true]);
        // 空列表上乱点也不能炸。
        let mut empty = OnboardingState::new();
        empty.toggle(0);
        assert!(empty.accepted.is_empty());
    }

    #[test]
    fn into_session_carries_the_detected_fields_and_uses_builtin_ssh() {
        let h = DetectedHost {
            label: "web".to_string(),
            host: "10.0.0.11".to_string(),
            user: "deploy".to_string(),
            port: 2222,
            source: "s".to_string(),
        };
        let cfg = h.into_session("web (2)".to_string());
        assert_eq!(cfg.name, "web (2)", "名字由调用方给（撞名时换成 unique_name）");
        assert_eq!(cfg.protocol, Protocol::Ssh);
        assert_eq!(cfg.host, "10.0.0.11");
        assert_eq!(cfg.port, 2222);
        assert_eq!(cfg.username, "deploy");
        assert!(cfg.builtin_ssh, "导入的会话走进程内 SSH");
    }
}

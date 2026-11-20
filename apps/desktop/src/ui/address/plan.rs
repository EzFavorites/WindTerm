//! 地址栏的**纯逻辑**：`Target` ↔ `SessionConfig`、候选拼接、激活意图、文案。
//!
//! # 为什么单开一层
//!
//! `term_app::address` 已经把「字符串 → 目标」「目标 ↔ 展示串」「候选去重 / 位移 /
//! MRU」钉成了带单测的纯函数。剩下三件属于**投影层**的事不在那里：
//!
//! 1. 目标要变成一条 `SessionConfig` 才能交给连接层（`term-core` 不认识地址栏）；
//! 2. 候选表里「已打开的标签」一段只有投影层拿得到（`tabs` 是视图状态）；
//! 3. 「按了这一行该做什么」是 UI 语义（切标签 / 连会话 / 新连接）。
//!
//! 这三件都不碰 `egui`、不碰 IO，所以照 `term_app::address` 的写法放在一个
//! 能单测的纯文件里（见 `address_tests.rs`）：地址栏最容易错的正是这些
//! 「看起来显然」的映射——名字少了 `:port`、`builtin_ssh` 忘了打开、点了
//! 一个已经被删掉的会话。

use term_app::address::{self, AddressKind, AddressProblem, Candidate, Target};
use term_app::i18n::{t, Lang};
use term_core::session::{Protocol, SessionConfig};

/// 「最近使用」保留几条。上限由**写者**执行（`push_recent` 的 `cap`），
/// 读的时候不再裁一次——手改过的文件多写几条不致命，读时裁剪会静默改用户的文件。
pub(in crate::ui) const RECENT_CAP: usize = term_runtime::prefs::RECENT_TARGETS_CAP;

/// 一帧最多拼几条候选。
///
/// 40 = 「已打开标签 + 会话库 + 最近使用」三段各十几条的合理上界。地址栏下拉
/// 一屏只画 10 行，再多也只是把 `window_start` 的滚动条拉长；真正的目的是
/// 给候选表一个**硬上限**，免得会话库涨到几千条时每帧都要克隆一遍。
pub(in crate::ui) const CANDIDATE_CAP: usize = 40;

/// 候选被回车 / 点击时该做什么。
///
/// 用枚举而不是「拿到 `Candidate` 就在绘制处 match 一遍」：三种 kind 的落点
/// （切标签 / 连会话 / 新连接）差别很大，把它固化成一个可断言的值，才能在
/// 单测里逐条钉住，而不必跑起一个窗口。
#[derive(Clone, Debug, PartialEq, Eq)]
pub(in crate::ui) enum Activation {
    /// 切到第 n 个标签。
    SwitchTab(usize),
    /// 按会话库第 n 条配置连接。
    ConnectSession(usize),
    /// 用输入框里那串地址新建一次连接（**不进会话库**）。
    QuickConnect(Target),
}

/// 候选 → 激活意图。
///
/// `None` 是**正常**返回值，不是错误路径：`Session` 那一段存的是下标，会话
/// 可能在打开下拉之后被删掉；`Host` 那一段存的是字符串，手改过的
/// `prefs.json` 里可能有一条根本解析不了的。两种情况都只能「什么都不做」——
/// 拿一个过期的下标去索引会话库才是真事故。
pub(in crate::ui) fn activation_for(candidate: &Candidate) -> Option<Activation> {
    match candidate.kind {
        AddressKind::Opened => candidate.index.map(Activation::SwitchTab),
        AddressKind::Session => candidate.index.map(Activation::ConnectSession),
        // 最近使用的行没有下标：它的目标串就是全部信息，重新解析一次即可。
        AddressKind::Host => address::parse_target(&candidate.target)
            .ok()
            .map(Activation::QuickConnect),
    }
}

/// 目标 → 一条**瞬时**会话配置（不进会话库、不落盘）。
///
/// # 为什么 `builtin_ssh = true`
///
/// 地址栏敲下的 `user@host:22` 没有对应的会话条目，也就没有地方存认证方式；
/// 进程内的 russh 客户端能用密码 / 交互式问答当场问，而拉系统 `ssh` 要用户
/// 自己在一个裸终端里再跟一遍提示。这里选内置客户端，语义是「输入即连接」。
///
/// 对非 SSH 协议这个标记是**惰性**的：`panels::sidebar::uses_builtin_ssh`
/// 要求 `protocol == Ssh` 才会为真，于是 Shell / Telnet / TCP 照常走 PTY 那条路。
///
/// # 不做什么
///
/// 不 `push` 进 `self.sessions`、不调 `save_sessions()`。地址栏是一次性入口，
/// 会话库是用户**显式创建**的记录——把随手敲的地址塞进去，会话树里就会多出
/// 一堆没人认领的条目（`Prefs.recent_targets` 的注释里记了同一条理由）。
pub(in crate::ui) fn config_for(target: &Target, name_hint: &str) -> SessionConfig {
    let mut config = SessionConfig::new(target.protocol);
    let hint = name_hint.trim();
    config.name = if hint.is_empty() {
        default_name(target)
    } else {
        hint.to_string()
    };
    config.host = target.host.clone();
    config.port = target.port;
    config.username = target.username.clone();
    // 断言同一条「输入即连接」：SSH 走进程内客户端。
    config.builtin_ssh = true;
    config
}

/// 地址栏里那条会话该叫什么。
///
/// 三条规则（都有单测）：
/// - 有用户 → `user@host`；
/// - 没有用户 → 只有 `host`（不拿当前登录用户去补，那是会话库新建时的规矩，
///   不是「我连了谁」的答案）；
/// - 端口**非默认**时带 `:port`。默认端口不写：`ssh://h:22` 与 `ssh://h` 是
///   同一台机器，标签标题里拖一个 `:22` 只是噪音；非默认端口必须写，否则
///   两台都叫 `jump` 的机器在标签条上分不出来。
pub(in crate::ui) fn name_hint(target: &Target) -> String {
    default_name(target)
}

/// [`name_hint`] 的本体：`config_for` 在提示为空时也走它，两处只能是同一份规则。
fn default_name(target: &Target) -> String {
    let mut name = String::new();
    if !target.username.is_empty() {
        name.push_str(&target.username);
        name.push('@');
    }
    name.push_str(&target.host);
    if target.port != 0 && target.port != target.protocol.default_port() {
        name.push(':');
        name.push_str(&target.port.to_string());
    }
    name
}

/// 会话配置 → 下拉右列的目标串（`Opened` 段用）。
///
/// 与 `term_app::address::session_candidates` 里那条会话串的规则**逐字相同**
/// （端口写全、本地 Shell 拿 `shell_command` 顶主机位、Shell 不显示用户名）。
/// 必须相同，否则「同一个会话既开着又在库里」时两行的 `target` 对不上，
/// `merge_candidates` 的 `label + target` 去重就失效，下拉里会出现两行
/// 长得一样却一个切标签一个重连的条目。
pub(in crate::ui) fn config_target(config: &SessionConfig) -> String {
    let local_shell = matches!(config.protocol, Protocol::Shell);
    let host = if config.host.trim().is_empty() && local_shell {
        let command = config.shell_command.trim();
        if command.is_empty() {
            config.name.trim().to_string()
        } else {
            command.to_string()
        }
    } else {
        config.host.trim().to_string()
    };
    let username = if local_shell {
        String::new()
    } else {
        config.username.trim().to_string()
    };
    address::render_target(&Target {
        protocol: config.protocol,
        username,
        host,
        port: config.port,
    })
}

/// 这一帧的下拉候选：已打开 → 会话库 → 最近使用，三段各自过 `filter`。
///
/// 前两段交给 `term_app::address` 拼（它按 `label + target` 去重、`Opened` 在前），
/// 这里只补第三段：最近使用是**裸目标串**，没有标签也没有下标，所以：
///
/// - `label` 与 `target` 都填那串本身——投影层画行时会因为两者相等而不重复写
///   一遍（见 `rows::draw_candidate_row`）；
/// - 已经出现在前两段里的目标串跳过。同一台机器既是开着的标签、又是上一个连过
///   的地址时，用户要的是「切过去」那条，不是再连一次。
pub(in crate::ui) fn candidates(
    open_tabs: &[(String, String)],
    sessions: &[SessionConfig],
    recent: &[String],
    filter: &str,
    cap: usize,
) -> Vec<Candidate> {
    if cap == 0 {
        return Vec::new();
    }
    let needle = filter.trim().to_lowercase();
    let opened: Vec<Candidate> = open_tabs
        .iter()
        .enumerate()
        .filter(|(_, (label, target))| row_matches(&needle, label, target))
        .map(|(index, (label, target))| Candidate {
            kind: AddressKind::Opened,
            label: label.clone(),
            target: target.clone(),
            // 与视觉基线那一列的 `switch to tab` 同一句话：这一列说的是
            // 「按了会发生什么」，不是状态。
            hint: "switch to tab".to_string(),
            index: Some(index),
        })
        .collect();
    let configured = address::session_candidates(sessions, filter, cap);
    let mut out = address::merge_candidates(opened, configured);
    for target in recent {
        if out.iter().any(|kept| kept.target == *target) {
            continue;
        }
        if !row_matches(&needle, target, target) {
            continue;
        }
        out.push(Candidate {
            kind: AddressKind::Host,
            label: target.clone(),
            target: target.clone(),
            hint: "connect".to_string(),
            index: None,
        });
    }
    out.truncate(cap);
    out
}

/// 大小写不敏感的「这一行要不要显示」。空过滤 = 全显示（打开即览全部候选）。
fn row_matches(needle: &str, label: &str, target: &str) -> bool {
    needle.is_empty()
        || label.to_lowercase().contains(needle)
        || target.to_lowercase().contains(needle)
}

/// 解析失败时画在输入框下方的那句话。
///
/// 为什么不直接用 `AddressProblem::message()`：那是 `term_app` 的中性层，
/// 拿不到当前界面语言（它也不该知道有界面）。这里把四种原因**各自**过一遍
/// 词表，`'{raw}' is not a port number` 这类带占位符的再把用户敲的原话填回去
/// ——报错要能指出是哪一段错，否则「端口不对」对 `host:abc` 毫无帮助。
pub(in crate::ui) fn problem_text(lang: Lang, problem: &AddressProblem) -> String {
    match problem {
        AddressProblem::Empty => {
            t(lang, "Enter a host, or an address like ssh://user@host:22.").to_string()
        }
        AddressProblem::NoHost => t(lang, "This address has no host name.").to_string(),
        AddressProblem::BadPort(raw) => {
            t(lang, "'{raw}' is not a port number (0-65535).").replace("{raw}", raw)
        }
        AddressProblem::BadUser(raw) => {
            t(lang, "'{raw}' is not a valid user name.").replace("{raw}", raw)
        }
    }
}

/// 候选行右侧那一列动作提示的译文。
///
/// `Candidate.hint` 是 `String` 而 `t()` 要 `&'static str`，所以这里按**已知
/// 取值**映射一遍：这两条是 `term_app::address` 里写死的（`switch to tab` /
/// `connect`），不认识的（将来新增的）原样显示，而不是退回空白。
pub(in crate::ui) fn hint_text(lang: Lang, hint: &str) -> String {
    match hint {
        "switch to tab" => t(lang, "switch to tab").to_string(),
        "connect" => t(lang, "connect").to_string(),
        other => other.to_string(),
    }
}

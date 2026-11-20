//! D3：地址栏（Address Bar）的纯逻辑层。
//!
//! # 为什么要单独一层
//!
//! 仓里原来那条「Address Bar」是一个**工作台页**：把一行字当 shell 命令发给
//! 当前终端。而参考夹具
//! （`apps/desktop/src/ui/shell/popup_lists.rs::draw_address_dropdown`）里的地址栏
//! 是另一件事：它是一个**连接目标输入框**——输入 `ssh://user@host:22`，
//! 下拉里列「已打开的标签 / 已配置的会话」，选中就连过去。
//!
//! 两件事共用一个名字，于是「Goto ▸ Goto Address Bar」点了之后打开的是命令页。
//! D3 把这一刀切开：字符串解析、候选拼接、选中位移、MRU 全部收进这里，
//! 命令页改名 `Command Line`（见 [`crate::vocab::CenterPage::GotoCommandLine`]），
//! 新的 `Window.GotoAddressBar` 落到 [`crate::vocab::Action::AddressBar`]。
//!
//! # 边界
//!
//! 零 `egui`、零 IO、零 `ViewState`：本模块只认字符串与会话配置，画成几个像素
//! 是投影层的事。这样「输入 `host:abc` 该说什么」可以在单测里逐条钉住，
//! 而不是靠读一遍 UI 代码去猜。
//!
//! 状态 [`AddressState`] 也只放**判定**需要的三个字段（开没开 / 输入 / 选中）；
//! 下拉内容不缓存——它是 `(已打开标签, 会话库, 输入)` 的纯函数值，缓存一次就
//! 会过期（`palette.rs` 里 F2 的教训）。

use term_core::session::{Protocol, SessionConfig};

/// 下拉行属于哪一类。
///
/// 三档对应视觉基线里那一列**左侧分组名**（绿字）。分成枚举而不是字符串常量：
/// 分组还决定这一行「点了做什么」（切标签 / 连会话 / 新连接），
/// 用一个类型把「显示什么」和「做什么」绑在一起，投影层就不必按文案再比一次。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum AddressKind {
    /// 已经开着的标签：选中即切过去。
    Opened,
    /// 会话库里配好的会话：选中即按它的配置连接。
    Session,
    /// 输入框里那串自己解析出来的目标：选中即新建一次连接。
    Host,
}

impl AddressKind {
    /// 分组名。英文原文（与视觉基线一致），翻译由投影层过一遍 `i18n`。
    pub const fn label(self) -> &'static str {
        match self {
            AddressKind::Opened => "Opened",
            AddressKind::Session => "Session",
            AddressKind::Host => "Host",
        }
    }
}

/// 下拉里的一行。
///
/// 字段与视觉基线的行形态一一对应：分组名 → [`AddressKind::label`]，
/// 图标与标题 → `label`，右侧灰字目标串 → [`Candidate::target`]，
/// 再右边的动作提示 → [`Candidate::hint`]（视觉基线里 `switch to tab` 那一列）。
/// 快捷键（`Alt+1`）不在结构里：那是「第 n 个标签 = Alt+n」的纯位置关系，
/// 由投影层按下标算，存进来只会有一份会过期的副本。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Candidate {
    /// 属于哪一段。
    pub kind: AddressKind,
    /// 标题栏文字：标签标题 / 会话名 / 主机名。
    pub label: String,
    /// 目标串，形如 `shell:///bash` / `ssh://user@host:22`。
    pub target: String,
    /// 动作提示，可以是空串（视觉基线里 `Session` 那一行就没有）。
    pub hint: String,
    /// `Opened` → 标签下标；`Session` → 会话下标；`Host` → `None`。
    ///
    /// 存下标而不是存会话配置的拷贝：会话库随时会被用户改，拷贝一份就是
    /// 第二个真相；下标越界时调用方按「这一行已经不存在」处理即可。
    pub index: Option<usize>,
}

/// 解析出来的连接目标。
///
/// 四个字段就是起一次连接所需的全部输入（认证方式不在这里：地址栏输入的是
/// 地址，不是会话配置，凭据仍由连接层按会话 / 对话框去取）。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Target {
    pub protocol: Protocol,
    pub username: String,
    pub host: String,
    pub port: u16,
}

/// 地址解析失败的原因。
///
/// 四种都对应一句**能告诉用户下一步做什么**的话（见 [`AddressProblem::message`]），
/// 而不是一个需要查文档的错误码：地址栏是用户随手敲的地方，报错就是全部的反馈。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AddressProblem {
    /// 一个字都没敲（或者只有空白）。
    Empty,
    /// 有端口 / 有用户，但没有主机——最常见的是漏打了冒号左边那半段（`:22`）。
    NoHost,
    /// 端口不是 0–65535 的十进制数。里面存的是**用户敲的原文**，不是解析后的值：
    /// 报错要把原话带回去，否则用户看不出是哪一段错（`host:abc` 里的 `abc`）。
    BadPort(String),
    /// 用户名里有会破坏地址语法的字符（空白 / `/` / `:` / `@` / 方括号），
    /// 或者 `@` 前面是空的。同样存原文。
    BadUser(String),
}

impl AddressProblem {
    /// 面向用户的一句话。
    ///
    /// 用 `String` 而不是 `&'static str`：其中两种要把用户敲的那段原话嵌进来。
    pub fn message(&self) -> String {
        match self {
            AddressProblem::Empty => "Enter a host, or an address like ssh://user@host:22.".to_string(),
            AddressProblem::NoHost => "This address has no host name.".to_string(),
            AddressProblem::BadPort(raw) => {
                format!("'{raw}' is not a port number (0-65535).")
            }
            AddressProblem::BadUser(raw) => format!("'{raw}' is not a valid user name."),
        }
    }
}

/// 解析 `user@host:port`；也接受 `host`、`host:port`、`ssh://user@host:port`、
/// `telnet://...`、IPv6 方括号 `[::1]:22`。
///
/// # 为什么默认协议是 SSH
///
/// 不带 scheme 的地址栏输入（`root@10.0.0.1`）在这类终端里几乎总是要 SSH：
/// 让默认值等于最常用的那一个，用户就不用每次都写全。要 Telnet / TCP 就写
/// scheme——这也是为什么本函数**只把已知 scheme 当 scheme**（见 [`split_scheme`]）。
///
/// # 端口省略时取谁的
///
/// 取协议的默认端口（[`Protocol::default_port`]），不是硬编码 22：地址栏填的是
/// 「一条到某协议主机的地址」，`telnet://h` 不写端口时该去 23。
pub fn parse_target(input: &str) -> Result<Target, AddressProblem> {
    let raw = input.trim();
    if raw.is_empty() {
        return Err(AddressProblem::Empty);
    }
    let (protocol, rest) = split_scheme(raw);

    // 用户段取最后一个 `@`：`ssh://user@host` 里的 `@` 只有一个，但 Windows
    // 域账号会写成 `DOMAIN\user@host`，用 `split_once` 会让 `DOMAIN\user` 里的
    // 反斜杠以后再加个 `@` 就悄悄错位。取最后一个是「主机是最后一个 @ 之后」，
    // 这条读法在两种写法下都对。
    let (username, authority, had_at) = match rest.rsplit_once('@') {
        Some((user, tail)) => (user, tail, true),
        None => ("", rest, false),
    };
    if had_at && invalid_user(username) {
        return Err(AddressProblem::BadUser(username.to_string()));
    }

    let (host, port_text) = split_host_port(authority)?;
    if host.is_empty() {
        return Err(AddressProblem::NoHost);
    }
    let port = match port_text {
        None => protocol.default_port(),
        Some(text) => {
            let trimmed = text.trim();
            match trimmed.parse::<u16>() {
                Ok(value) => value,
                // 存原文而不是存 `ParseIntError`：错误值要能原样显示给用户，
                // 而 `ParseIntError` 的 `Display` 说的是「invalid digit found in
                // string」，既没说是哪一段，也不是给人看的。
                Err(_) => return Err(AddressProblem::BadPort(trimmed.to_string())),
            }
        }
    };

    Ok(Target {
        protocol,
        username: username.to_string(),
        host: host.to_string(),
        port,
    })
}

/// 目标 → 展示串（与 [`parse_target`] 往返一致）。
///
/// 形态照参考夹具：`ssh://user@host:22`、`shell:///bin/bash`。三条规则都是
/// 「往返必须还原成同一个 [`Target`]」逼出来的：
///
/// - 端口非 0 就写出来（即使等于默认值）——视觉基线写的是 `...:22`，
///   而且写全了人眼不用记各协议默认端口；
/// - 端口 0 不写（Shell / Serial 没有端口，`:0` 是噪声，且解析回来仍是 0）；
/// - 主机里带 `:` 时包方括号——否则 `::1` 会被读成「主机为空 + 端口 `:1`」。
pub fn render_target(t: &Target) -> String {
    let scheme = match t.protocol {
        Protocol::Shell => "shell",
        Protocol::Ssh => "ssh",
        Protocol::Telnet => "telnet",
        Protocol::Serial => "serial",
        Protocol::Tcp => "tcp",
    };
    let host = if t.host.contains(':') {
        format!("[{}]", t.host)
    } else {
        t.host.clone()
    };
    let mut out = String::with_capacity(scheme.len() + host.len() + 12);
    out.push_str(scheme);
    out.push_str("://");
    if !t.username.is_empty() {
        out.push_str(&t.username);
        out.push('@');
    }
    out.push_str(&host);
    if t.port != 0 {
        out.push(':');
        out.push_str(&t.port.to_string());
    }
    out
}

/// 剥掉 `scheme://`，返回（协议，剩下的权威段）。
///
/// **只认已知 scheme**：`foo://bar` 里的 `foo` 不是一个我们支持过的协议，
/// 把它当 scheme 剥掉就等于默默替用户改写了输入（最后连错误都指向 `bar`）。
/// 不认时整串当权威段往下走，错误会落在 `foo` 的端口 `//bar` 上——
/// 报错点与用户敲错的那一段对得上。
fn split_scheme(raw: &str) -> (Protocol, &str) {
    if let Some((scheme, rest)) = raw.split_once("://") {
        let known = match scheme.trim().to_ascii_lowercase().as_str() {
            "ssh" => Some(Protocol::Ssh),
            "telnet" => Some(Protocol::Telnet),
            "shell" | "local" => Some(Protocol::Shell),
            "serial" => Some(Protocol::Serial),
            "tcp" => Some(Protocol::Tcp),
            _ => None,
        };
        if let Some(protocol) = known {
            return (protocol, rest);
        }
    }
    (Protocol::Ssh, raw)
}

/// 把权威段拆成（主机，端口原文）。返回的端口原文由调用方负责 `parse`。
///
/// IPv6 走方括号分支：`[::1]:2200` 里的冒号是地址的一部分，`split_once(':')`
/// 会把它拆成主机 `[` 加端口 `:1]:2200`。没有方括号的裸 `::1` 不支持——
/// 「裸 IPv6 + 可选端口」本身是有歧义的写法，与其猜一个不如让用户加括号。
fn split_host_port(authority: &str) -> Result<(&str, Option<&str>), AddressProblem> {
    if let Some(inner) = authority.strip_prefix('[') {
        let Some((host, tail)) = inner.split_once(']') else {
            // `[::1` 没有闭合括号：这一段整体不可解释，按「没有主机」报。
            return Err(AddressProblem::NoHost);
        };
        if tail.is_empty() {
            return Ok((host, None));
        }
        let Some(port) = tail.strip_prefix(':') else {
            // `[::1]junk`：括号后面只能接 `:端口`。
            return Err(AddressProblem::NoHost);
        };
        return Ok((host, Some(port)));
    }
    match authority.split_once(':') {
        Some((host, port)) => Ok((host, Some(port))),
        None => Ok((authority, None)),
    }
}

/// 用户名里有没有会破坏地址语法的字符。
///
/// 只挡**结构性**的字符（空白 / 分隔符），不挡 `+` `%` 这类合法但少见的字符：
/// 地址栏是随手敲的地方，挡太严会把「明明能连」的输入拒在门外。
fn invalid_user(username: &str) -> bool {
    username.is_empty()
        || username
            .chars()
            .any(|c| c.is_whitespace() || matches!(c, ':' | '/' | '@' | '[' | ']'))
}

/// 从已配置会话里按子串过滤出候选（大小写不敏感，匹配 name 或 host）。
///
/// `cap` 是上限，超出截断（下拉高度有限，列全了反而找不到）。
/// 已打开 tab 的候选由调用方另行拼接（见 [`merge_candidates`]）——本层拿不到
/// 标签列表，那是投影层的视图状态。
///
/// 匹配 name 与 host **两条**都要走：用户可能记得会话名（`prod-db`），
/// 也可能只记得主机（`10.0.0.5`）；只匹配一条会让另一半人觉得搜索失灵。
pub fn session_candidates(sessions: &[SessionConfig], filter: &str, cap: usize) -> Vec<Candidate> {
    let mut out: Vec<Candidate> = Vec::new();
    if cap == 0 {
        return out;
    }
    let needle = filter.trim().to_lowercase();
    for (index, cfg) in sessions.iter().enumerate() {
        let name = cfg.name.to_lowercase();
        let host = cfg.host.to_lowercase();
        let hit = needle.is_empty()
            || name.contains(needle.as_str())
            || host.contains(needle.as_str());
        if !hit {
            continue;
        }
        out.push(Candidate {
            kind: AddressKind::Session,
            label: cfg.name.clone(),
            target: session_target(cfg),
            // 与视觉基线 `Opened` 行的 `switch to tab` 对仗：同一列位置说的是
            // 「点了会发生什么」，所以它是固定文案而不是状态。
            hint: "connect".to_string(),
            index: Some(index),
        });
        if out.len() == cap {
            break;
        }
    }
    out
}

/// 会话配置 → 展示串。
///
/// 与投影层 `ui/effects.rs::session_url`（复制连接串那条命令）同一套形态，
/// **但端口写全**：那里的省略是为了贴到聊天框里好看，这里的右列是给人核对
/// 「这一行是不是那台机器」，`:22` / `:2222` 的差别必须看得见。
///
/// 本地 Shell 没有 host：拿 `shell_command`（`/bin/bash` / `zsh` …）顶主机位，
/// 于是显示成参考夹具那样的 `shell:///bin/bash`。用户名对 Shell 无意义，这里
/// 一律不显示，否则每一行都拖着同一个 `alex@`。
/// （2026-11-13 P1-5 订正：这里原写「`SessionConfig::new(Shell)` 会填当前登录
/// 用户」，那是 core 还读 `$USER` 时的行为。现在用户名由调用方注入、core 不读
/// 环境，多数构造路径给的是空串——不显示的结论不变，理由已换。）
fn session_target(cfg: &SessionConfig) -> String {
    let local_shell = matches!(cfg.protocol, Protocol::Shell);
    let host = if cfg.host.trim().is_empty() && local_shell {
        let command = cfg.shell_command.trim();
        if command.is_empty() {
            cfg.name.trim().to_string()
        } else {
            command.to_string()
        }
    } else {
        cfg.host.trim().to_string()
    };
    let username = if local_shell {
        String::new()
    } else {
        cfg.username.trim().to_string()
    };
    render_target(&Target {
        protocol: cfg.protocol,
        username,
        host,
        port: cfg.port,
    })
}

/// 把「已打开」与「已配置」两段拼起来，已打开的排前面。
///
/// 两段各自排序、各自截断，这里**只做拼接与去重**：同一个 `label + target`
/// 只留第一次出现的那条。为什么按这一对而不是按 `index`：两个段的下标空间
/// 不同（标签下标 vs 会话下标），拿下标去重等于没去重。
///
/// 去重是必要的，不是洁癖：同一个会话既开着、又在会话库里，两行长得一模一样
/// 会让用户以为自己手抖点重了；留 `Opened` 那条是对的，因为「切过去」比
/// 「再连一次」永远更便宜。
pub fn merge_candidates(opened: Vec<Candidate>, sessions: Vec<Candidate>) -> Vec<Candidate> {
    let mut out: Vec<Candidate> = Vec::with_capacity(opened.len() + sessions.len());
    for candidate in opened.into_iter().chain(sessions) {
        let duplicate = out
            .iter()
            .any(|kept| kept.label == candidate.label && kept.target == candidate.target);
        if !duplicate {
            out.push(candidate);
        }
    }
    out
}

/// 按住键盘上/下移动选中项，到头不回绕（夹在 `[0, total-1]`）。
///
/// 与命令面板的「回绕」相反，这里**故意**不回绕：地址栏的第一行通常是用户
/// 最想去的那个（最近用过的），从第一行再按一次上键就跳到末尾，会让手指
/// 按在「上」上却跑到底下去。夹紧更可预测。
///
/// `total == 0` 返回 0：没有候选时下标必须是 0，否则调用方拿它去索引会越界。
pub fn move_selection(selected: usize, delta: isize, total: usize) -> usize {
    if total == 0 {
        return 0;
    }
    let last = total as i64 - 1;
    let current = (selected as i64).min(last);
    let moved = (current + delta as i64).clamp(0, last);
    // `moved` 已被夹进 `[0, last]`，这里不可能是负数。
    moved as usize
}

/// 可视窗口起点：保证 `selected` 在 `[start, start+visible)` 内。
///
/// 算法与命令面板同源（`apps/desktop/src/ui/overlays/palette.rs::window_start`，
/// 本仓另抄一份而不是跨 crate 引——那个是私有的投影层函数，而这条判定要有单测）。
/// 选中项走到窗口下半部就开始滚，滚到底时窗口贴住末尾，不会出现半屏空白。
pub fn window_start(total: usize, selected: Option<usize>, visible: usize) -> usize {
    if total <= visible || visible == 0 {
        return 0;
    }
    let focus = selected.unwrap_or(0).min(total - 1);
    focus.saturating_sub(visible / 2).min(total - visible)
}

/// 「最近使用」MRU：去重（同串只留最新一次）、插到队首、裁到 `cap`。
///
/// 为什么先删再插而不是「找到了就移动」：那需要两趟且容易在重复项上打转；
/// `retain` + `insert(0, …)` 一趟读得出来，且「同串只留最新一次」是唯一的不变量。
///
/// 空串（或只有空白）是安静的空操作：地址栏回车失败时不该污染最近列表。
/// `cap == 0` 时列表被清空（不是「不插入」）：容量为 0 的可变状态只能为空。
pub fn push_recent(list: &mut Vec<String>, target: &str, cap: usize) {
    let trimmed = target.trim();
    if trimmed.is_empty() {
        return;
    }
    list.retain(|item| item != trimmed);
    list.insert(0, trimmed.to_string());
    list.truncate(cap);
}

/// 地址栏的可变状态（由 UI 持有，逻辑判断都在这里）。
///
/// 只放三个字段，每个都有唯一写者（本结构的方法）。下拉内容与会话库都不在这里：
/// 它们是**输入**，不是状态。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct AddressState {
    /// 下拉是不是打开着。
    pub open: bool,
    /// 输入框里的原文（未经解析）。
    pub input: String,
    /// 选中项下标；改输入就归零，见 [`AddressState::set_input`]。
    pub selected: usize,
}

impl AddressState {
    pub fn new() -> Self {
        Self::default()
    }

    /// 打开并预填：`prefill` 为空时保持空（打开即览全部候选），
    /// 非空时填入。
    ///
    /// 两种情况都把 `selected` 归零：候选集刚变了，旧的选中下标指向的已经不是
    /// 同一行——留着它就是「回车连到了别的机器」。
    pub fn open_with(&mut self, prefill: &str) {
        self.open = true;
        self.input.clear();
        self.input.push_str(prefill);
        self.selected = 0;
    }

    /// 关掉下拉。
    ///
    /// **输入保留**：下次打开时用户多半要继续改同一个地址（连错端口、改主机名
    /// 是常事），清空等于让他重敲一遍。
    pub fn close(&mut self) {
        self.open = false;
        self.selected = 0;
    }

    /// 用户改动了输入：`selected` 归零。
    ///
    /// 候选集是输入的纯函数值，改一个字它就可能换一批；选中项因此必须回到第一行，
    /// 否则回车会打到一行用户没看过的条目上。
    pub fn set_input(&mut self, text: &str) {
        self.input.clear();
        self.input.push_str(text);
        self.selected = 0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 造一条会话配置：`SessionConfig::new` 会把几十个字段填成出厂值，
    /// 测试只关心地址相关的四个。
    fn session(protocol: Protocol, name: &str, host: &str, port: u16) -> SessionConfig {
        let mut cfg = SessionConfig::new(protocol);
        cfg.name = name.to_string();
        cfg.host = host.to_string();
        cfg.port = port;
        cfg
    }

    fn target(protocol: Protocol, username: &str, host: &str, port: u16) -> Target {
        Target {
            protocol,
            username: username.to_string(),
            host: host.to_string(),
            port,
        }
    }

    #[test]
    fn a_full_address_parses_into_every_field() {
        assert_eq!(
            parse_target("user@host:2222"),
            Ok(target(Protocol::Ssh, "user", "host", 2222))
        );
    }

    #[test]
    fn a_bare_host_defaults_to_ssh_and_an_empty_user() {
        // 省略协议与用户：默认 SSH + 协议默认端口，用户名必须是空串（不是当前登录用户）。
        let parsed = parse_target("host").expect("裸主机必须能解析");
        assert_eq!(parsed.protocol, Protocol::Ssh);
        assert_eq!(parsed.username, "");
        assert_eq!(parsed.host, "host");
        assert_eq!(parsed.port, 22);
        assert_eq!(parsed.port, Protocol::Ssh.default_port());
    }

    #[test]
    fn an_explicit_port_is_kept() {
        assert_eq!(parse_target("host:8022").map(|t| t.port), Ok(8022));
    }

    #[test]
    fn a_scheme_is_optional_and_equivalent_to_the_bare_form() {
        // `ssh://u@h:22` 与 `u@h:22` 必须解析成同一个目标：菜单里有两种写法，
        // 用户不该因为多敲了 `ssh://` 就连到别处。
        assert_eq!(parse_target("ssh://u@h:22"), parse_target("u@h:22"));
    }

    #[test]
    fn a_telnet_scheme_switches_the_protocol_and_the_default_port() {
        let parsed = parse_target("telnet://h:23").expect("telnet 地址必须能解析");
        assert_eq!(parsed.protocol, Protocol::Telnet);
        assert_eq!(parsed.host, "h");
        assert_eq!(parsed.port, 23);
        // 不写端口时取 telnet 自己的默认端口，而不是 SSH 的 22。
        assert_eq!(
            parse_target("telnet://h").map(|t| t.port),
            Ok(Protocol::Telnet.default_port())
        );
    }

    #[test]
    fn an_ipv6_literal_keeps_its_colons() {
        // 方括号里的冒号是地址的一部分，不能被当成端口分隔符。
        assert_eq!(
            parse_target("[::1]:2200"),
            Ok(target(Protocol::Ssh, "", "::1", 2200))
        );
        // 不写端口同样成立。
        assert_eq!(parse_target("[::1]"), Ok(target(Protocol::Ssh, "", "::1", 22)));
    }

    #[test]
    fn empty_and_broken_addresses_report_a_reason() {
        assert_eq!(parse_target(""), Err(AddressProblem::Empty));
        assert_eq!(parse_target("   "), Err(AddressProblem::Empty));
        assert_eq!(parse_target(":22"), Err(AddressProblem::NoHost));
        assert_eq!(
            parse_target("host:abc"),
            Err(AddressProblem::BadPort("abc".to_string()))
        );
        // 65535 是 u16 的上界，越界必须报错而不是回绕成 34463。
        assert_eq!(
            parse_target("host:99999"),
            Err(AddressProblem::BadPort("99999".to_string()))
        );
        assert_eq!(
            parse_target("@host"),
            Err(AddressProblem::BadUser(String::new()))
        );
        // 每一种失败都得有一句能读的话，且原文要出现在里面。
        assert!(AddressProblem::Empty.message().contains("host"));
        assert!(AddressProblem::NoHost.message().contains("host"));
        assert!(AddressProblem::BadPort("abc".into())
            .message()
            .contains("abc"));
        assert!(AddressProblem::BadUser("a b".into())
            .message()
            .contains("a b"));
    }

    #[test]
    fn render_and_parse_are_a_round_trip() {
        // 展示串是**用户可见**的那一列，也是一次可能的重新输入（复制粘贴回来）。
        // 所以判据不是「看起来像」，而是「再解析一次得到同一个目标」。
        for input in [
            "user@host:2222",
            "host",
            "host:8022",
            "ssh://u@h:22",
            "telnet://h:23",
            "[::1]:2200",
            "serial:///dev/tty.usbserial",
            "tcp://10.0.0.1:8080",
            "shell:///bin/bash",
        ] {
            let parsed = parse_target(input).unwrap_or_else(|e| panic!("{input} 解析失败：{e:?}"));
            let rendered = render_target(&parsed);
            let again = parse_target(&rendered)
                .unwrap_or_else(|e| panic!("{rendered} 解析失败：{e:?}"));
            assert_eq!(again, parsed, "{input} → {rendered} 往返不一致");
            assert_eq!(render_target(&again), rendered, "{input} 展示串不稳定");
        }
        // 端口 0 不写（Shell / Serial 没有端口），`:0` 是噪声。
        assert_eq!(
            render_target(&target(Protocol::Shell, "", "/bin/bash", 0)),
            "shell:///bin/bash"
        );
        // IPv6 主机必须带括号，否则 `ssh://::1:22` 会被读成「主机为空」。
        assert_eq!(
            render_target(&target(Protocol::Ssh, "u", "::1", 22)),
            "ssh://u@[::1]:22"
        );
    }

    #[test]
    fn session_candidates_match_name_or_host_case_insensitively() {
        let sessions = vec![
            session(Protocol::Ssh, "Prod-DB", "10.0.0.5", 22),
            session(Protocol::Ssh, "staging", "10.0.0.9", 2222),
            session(Protocol::Ssh, "web", "prod.example.com", 22),
        ];
        // 匹配名字（大小写不敏感）。
        let by_name = session_candidates(&sessions, "prod", 10);
        assert_eq!(
            by_name.iter().map(|c| c.label.as_str()).collect::<Vec<_>>(),
            vec!["Prod-DB", "web"],
            "名字命中 Prod-DB，主机命中 prod.example.com"
        );
        // 匹配主机。
        let by_host = session_candidates(&sessions, "10.0.0", 10);
        assert_eq!(by_host.len(), 2);
        assert_eq!(by_host[0].index, Some(0));
        assert_eq!(by_host[1].index, Some(1));
        // 空过滤 = 打开即览全部。
        assert_eq!(session_candidates(&sessions, "", 10).len(), 3);
        assert_eq!(session_candidates(&sessions, "   ", 10).len(), 3);
        // 没命中就是空。
        assert!(session_candidates(&sessions, "zzz", 10).is_empty());
        // 候选行的形态：Session 段、右列是目标串、下标指回原表。
        let first = &by_name[0];
        assert_eq!(first.kind, AddressKind::Session);
        assert_eq!(first.target, "ssh://10.0.0.5:22");
        assert_eq!(first.hint, "connect");
        assert_eq!(first.index, Some(0));
    }

    #[test]
    fn session_candidates_are_truncated_to_the_cap() {
        let sessions: Vec<SessionConfig> = (0..30)
            .map(|i| session(Protocol::Ssh, &format!("host{i}"), &format!("10.0.0.{i}"), 22))
            .collect();
        assert_eq!(session_candidates(&sessions, "", 5).len(), 5);
        // `cap == 0` 是空集，不是「不限量」——上限就是上限。
        assert!(session_candidates(&sessions, "", 0).is_empty());
        // 截断保留的是**前 cap 条**（顺序与传入表一致），不打乱。
        let capped = session_candidates(&sessions, "", 3);
        assert_eq!(
            capped.iter().map(|c| c.label.as_str()).collect::<Vec<_>>(),
            vec!["host0", "host1", "host2"]
        );
    }

    #[test]
    fn a_local_shell_session_shows_its_command_instead_of_an_empty_host() {
        // 本地 Shell 没有 host：`shell://` 后面空着是一行废信息，
        // 顶 `shell_command` 进去才与参考夹具的 `shell:///bash` 同形。
        let mut cfg = session(Protocol::Shell, "bash", "", 0);
        cfg.shell_command = "/bin/bash".to_string();
        let rows = session_candidates(&[cfg], "", 5);
        assert_eq!(rows[0].target, "shell:///bin/bash");
        // 用户名对 Shell 无意义，不许出现在展示串里。
        let mut no_command = session(Protocol::Shell, "Local Shell", "", 0);
        no_command.shell_command.clear();
        assert_eq!(
            session_candidates(&[no_command], "", 5)[0].target,
            "shell://Local Shell"
        );
    }

    #[test]
    fn merge_puts_opened_first_and_drops_duplicates() {
        let opened = vec![
            Candidate {
                kind: AddressKind::Opened,
                label: "web".to_string(),
                target: "ssh://10.0.0.5:22".to_string(),
                hint: "switch to tab".to_string(),
                index: Some(0),
            },
            Candidate {
                kind: AddressKind::Opened,
                label: "local".to_string(),
                target: "shell:///bin/bash".to_string(),
                hint: "switch to tab".to_string(),
                index: Some(1),
            },
        ];
        let sessions = vec![
            // 同一条会话：既开着又在库里——只留 `Opened` 那行（切过去更便宜）。
            Candidate {
                kind: AddressKind::Session,
                label: "web".to_string(),
                target: "ssh://10.0.0.5:22".to_string(),
                hint: "connect".to_string(),
                index: Some(0),
            },
            Candidate {
                kind: AddressKind::Session,
                label: "db".to_string(),
                target: "ssh://10.0.0.7:2222".to_string(),
                hint: "connect".to_string(),
                index: Some(1),
            },
        ];
        let merged = merge_candidates(opened, sessions);
        assert_eq!(merged.len(), 3, "重复的那条只留一次");
        assert_eq!(merged[0].label, "web");
        assert_eq!(merged[0].kind, AddressKind::Opened, "已打开的排前面");
        assert_eq!(merged[1].label, "local");
        assert_eq!(merged[2].label, "db");
        // 同名不同目标不算重复（两台机器都叫 `web` 是常见的）。
        let same_label = merge_candidates(
            vec![Candidate {
                kind: AddressKind::Opened,
                label: "web".to_string(),
                target: "ssh://a:22".to_string(),
                hint: String::new(),
                index: Some(0),
            }],
            vec![Candidate {
                kind: AddressKind::Session,
                label: "web".to_string(),
                target: "ssh://b:22".to_string(),
                hint: String::new(),
                index: Some(1),
            }],
        );
        assert_eq!(same_label.len(), 2);
    }

    #[test]
    fn selection_clamps_at_both_ends_without_wrapping() {
        // 正常上下移动。
        assert_eq!(move_selection(0, 1, 3), 1);
        assert_eq!(move_selection(1, 1, 3), 2);
        // 下界夹紧：第一行再按上键仍停在第 0 行（不回绕到末尾）。
        assert_eq!(move_selection(0, -1, 3), 0);
        // 上界夹紧：最后一行再按下键仍停在末位。
        assert_eq!(move_selection(2, 1, 3), 2);
        // 一次跨多行也不能越界。
        assert_eq!(move_selection(0, -5, 3), 0);
        assert_eq!(move_selection(0, 9, 3), 2);
        // 没有候选时下标必须是 0，否则调用方拿它索引会越界。
        assert_eq!(move_selection(0, 1, 0), 0);
        // 旧的选中下标越界（候选刚变少）时按末位算。
        assert_eq!(move_selection(99, 0, 3), 2);
    }

    #[test]
    fn the_window_scrolls_only_when_the_selection_leaves_it() {
        // 装得下就永远从 0 开始。
        assert_eq!(window_start(3, Some(2), 10), 0);
        assert_eq!(window_start(0, None, 10), 0);
        assert_eq!(window_start(10, Some(9), 0), 0, "visible == 0 时不做除法");
        // 选中项在下半部：窗口跟着滚，但选中项仍在窗口内。
        let start = window_start(40, Some(30), 10);
        assert!(start <= 30 && 30 < start + 10, "选中项必须落在窗口内");
        assert!(start > 0, "滚了");
        // 到底时窗口贴住末尾，不出现空白。
        assert_eq!(window_start(40, Some(39), 10), 30);
        assert_eq!(window_start(40, None, 10), 0);
        // 越界选中项按末位算，不 panic。
        assert_eq!(window_start(40, Some(999), 10), 30);
    }

    #[test]
    fn recent_targets_are_mru_deduplicated_and_capped() {
        let mut list: Vec<String> = Vec::new();
        push_recent(&mut list, "ssh://a:22", 3);
        push_recent(&mut list, "ssh://b:22", 3);
        assert_eq!(list, vec!["ssh://b:22", "ssh://a:22"], "最新的在队首");
        // 同一个目标再用一次：不新增一行，而是挪到队首（去重）。
        push_recent(&mut list, "ssh://a:22", 3);
        assert_eq!(list, vec!["ssh://a:22", "ssh://b:22"]);
        // 超出容量时丢最旧的那条。
        push_recent(&mut list, "ssh://c:22", 3);
        push_recent(&mut list, "ssh://d:22", 3);
        assert_eq!(list, vec!["ssh://d:22", "ssh://c:22", "ssh://a:22"]);
        assert_eq!(list.len(), 3, "cap 是硬上限");
        // 空白串不污染列表（回车失败 / 空输入不该留下痕迹）。
        push_recent(&mut list, "   ", 3);
        assert_eq!(list.len(), 3);
        // 容量 0 时保持为空。
        let mut empty = vec!["x".to_string()];
        push_recent(&mut empty, "ssh://a:22", 0);
        assert!(empty.is_empty());
        // 首尾空白会被去掉：同一台机器敲了尾随空格不该占两行。
        let mut trimmed: Vec<String> = Vec::new();
        push_recent(&mut trimmed, "  ssh://a:22  ", 5);
        assert_eq!(trimmed, vec!["ssh://a:22"]);
    }

    #[test]
    fn opening_prefills_the_input_and_resets_the_selection() {
        let mut state = AddressState::new();
        assert!(!state.open);
        state.selected = 5;
        state.open_with("ssh://u@h:22");
        assert!(state.open);
        assert_eq!(state.input, "ssh://u@h:22");
        assert_eq!(state.selected, 0, "候选集刚变了，旧下标必须归零");
        // 空预填 = 打开即览全部候选（不保留上一次的字）。
        state.open_with("");
        assert!(state.open);
        assert!(state.input.is_empty());
        // 改输入同样把选中项归零。
        state.selected = 3;
        state.set_input("h");
        assert_eq!(state.input, "h");
        assert_eq!(state.selected, 0);
    }

    #[test]
    fn closing_keeps_the_input_for_the_next_open() {
        let mut state = AddressState::new();
        state.open_with("ssh://u@h:22");
        state.selected = 2;
        state.close();
        assert!(!state.open);
        assert_eq!(state.selected, 0);
        assert_eq!(state.input, "ssh://u@h:22", "下次打开要继续改同一个地址");
    }

    #[test]
    fn the_three_kinds_label_themselves() {
        // 分组名就是视觉基线那一列绿字，一个字母都不许漂。
        assert_eq!(AddressKind::Opened.label(), "Opened");
        assert_eq!(AddressKind::Session.label(), "Session");
        assert_eq!(AddressKind::Host.label(), "Host");
    }

    #[test]
    fn an_unknown_scheme_is_not_silently_rewritten() {
        // `foo://bar` 里的 `foo` 不是我们支持过的协议：不许当 scheme 剥掉
        // （那等于替用户改写输入），报错点应落在 `bar` 之前的端口位上。
        assert_eq!(
            parse_target("foo://bar"),
            Err(AddressProblem::BadPort("//bar".to_string()))
        );
    }
}

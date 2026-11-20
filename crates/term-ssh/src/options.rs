//! 连接一个 SSH 会话需要知道的全部事实。
//!
//! 这一层刻意**不认识** `term_core::SessionConfig`：SSH 层不关心「标签页叫什么
//! 颜色」「滚动多少行」，只关心主机、身份、主机密钥策略。把二者粘起来是应用层
//! 的事（`term-app` / `apps/*`），这样这个 crate 能被 CLI、导入器、隧道复用，
//! 也能在单测里直接构造。

use std::fmt;
use std::path::PathBuf;
use std::time::Duration;

use crate::error::SshError;

/// 默认端口。写成一个具名常量，是因为「22 要不要算进 known_hosts 的键」这件事
/// 在 russh 里是 `if port == 22 { host } else { [host]:port }`——同一个魔法数字
/// 出现两次就会有人改错一次。
pub const DEFAULT_PORT: u16 = 22;

/// 终端类型。没有 `$TERM` 的会话，配色与功能键都会退化成 1970 年代的样子。
pub const DEFAULT_TERM: &str = "xterm-256color";

/// 连上手之前最多等多久。SSH 的握手失败通常是「被丢包」，等 15 秒刚好卡在
/// 用户开始怀疑程序死了之前。
pub const DEFAULT_CONNECT_TIMEOUT: Duration = Duration::from_secs(15);

/// 心跳间隔：空闲时每 30 秒确认对端还在。
///
/// 为什么不设 `inactivity_timeout`：它会在空闲时**主动断开**，而交互式终端
/// 就是这么用的——开着一个窗口去看网页，回来还在。心跳只负责发现「已经断了」，
/// 不负责制造断开。
pub const DEFAULT_KEEPALIVE: Duration = Duration::from_secs(30);

/// 敏感字段在 `Debug` 里的统一占位符。
///
/// 抽成常量而不是在三个 `impl` 里各写一遍字面量：字面量写三遍，改了一处就会
/// 有一处悄悄不脱敏，而「悄悄不脱敏」正是这个文件要防的事。
const REDACTED: &str = "<redacted>";

/// `Option<String>` 的脱敏：**保留「有没有配」这一位**。
///
/// 一律抹成 `"<redacted>"` 会让 `None` 和 `Some(…)` 在日志里长得一样，
/// 而「没给口令」和「给了口令但错」恰好是排查私钥问题时最先要分清的两件事。
fn redacted_opt(value: &Option<String>) -> Option<&'static str> {
    value.as_ref().map(|_| REDACTED)
}

/// 认证方式。
#[derive(Clone, PartialEq, Eq)]
pub enum AuthMethod {
    /// 密码。**不要**把它写进会话文件：那是 `term-vault` 的活。
    Password { password: String },
    /// 私钥文件（OpenSSH 或 PEM）。`passphrase` 只在私钥本身加密时需要。
    PrivateKey {
        path: PathBuf,
        passphrase: Option<String>,
    },
    /// 交给 `$SSH_AUTH_SOCK` 上的 ssh-agent：私钥不出 agent。
    Agent,
    /// keyboard-interactive：服务端逐条提问，`answers` 按顺序回答。
    /// 不落盘、不进会话文件——和 `Password` 一样只在内存里活一次握手。
    KeyboardInteractive { answers: Vec<String> },
}

impl AuthMethod {
    /// 错误信息里怎么称呼这次尝试。
    pub fn label(&self) -> &'static str {
        match self {
            AuthMethod::Password { .. } => "password",
            AuthMethod::PrivateKey { .. } => "publickey",
            AuthMethod::Agent => "agent",
            AuthMethod::KeyboardInteractive { .. } => "keyboard-interactive",
        }
    }
}

/// 手写 `Debug`：三个凭据字段（`password` / `passphrase` / `answers`）一律脱敏。
///
/// 为什么不用 `#[derive(Debug)]`：这个类型会被上一层塞进错误上下文、测试失败
/// 信息和 `.expect()` 的格式化里，而 `{:?}` 一旦打印出来就会原样落到日志文件。
/// 密码出现在日志里不是「信息泄漏的风险」，是已经发生了的泄漏。
///
/// 脱敏只针对凭据：变体名、私钥**路径**、以及「口令给没给」都照常打印，
/// 否则 `Debug` 就退化成一句常量，调试时反而要回去改代码加 `println!`。
impl fmt::Debug for AuthMethod {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            AuthMethod::Password { .. } => f
                .debug_struct("Password")
                .field("password", &REDACTED)
                .finish(),
            AuthMethod::PrivateKey { path, passphrase } => {
                let passphrase = redacted_opt(passphrase);
                f.debug_struct("PrivateKey")
                    .field("path", path)
                    .field("passphrase", &passphrase)
                    .finish()
            }
            AuthMethod::Agent => f.write_str("Agent"),
            AuthMethod::KeyboardInteractive { .. } => f
                .debug_struct("KeyboardInteractive")
                .field("answers", &REDACTED)
                .finish(),
        }
    }
}

/// 主机密钥策略。
///
/// 三档的区别只有一件事：**未知主机**怎么办。已记录但不一致的情况在三档里
/// 都是拒绝（除了 `Insecure` 根本不看）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HostKeyPolicy {
    /// 必须在 `known_hosts` 里查到，查不到就拒绝。等价于 `StrictHostKeyChecking=yes`。
    Strict,
    /// 查不到就记下来再用（TOFU，首次使用即信任）。等价于 `accept-new`。
    Tofu,
    /// 完全不检查。只给测试与「我知道我在做什么」用。
    Insecure,
}

/// ProxyJump 的一跳。
///
/// `ssh -J` / `ProxyJump` 的语法是逗号分隔的一串 `[user@]host[:port]`，
/// 从左到右依次是「第一跳 → 第二跳 → …」，最后一跳之后的才是目标机。
/// 这里只**解析成结构**，不改写、不建链——真正建立 direct-tcpip 链是连接层
/// （Batch 2）的事。把语法与网络分开，解析就能在无网单测里钉死。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProxyHop {
    /// 用户名留空 = 沿用目标会话的用户名（`ssh -J` 的默认行为）。
    pub user: Option<String>,
    pub host: String,
    pub port: u16,
}

impl ProxyHop {
    pub fn parse(s: &str) -> Result<Self, SshError> {
        let s = s.trim();
        if s.is_empty() {
            return Err(SshError::Config("ProxyJump 里有空的跳".to_string()));
        }
        // `user@` 只在最后一个 `@` 之前；主机部分不再含 `@`。
        let (user, rest) = match s.rsplit_once('@') {
            Some((u, r)) if !u.is_empty() => (Some(u.to_string()), r),
            Some(_) => {
                return Err(SshError::Config(format!("ProxyJump 跳缺少用户名：{s}")));
            }
            None => (None, s),
        };
        let (host, port) = split_host_port(rest)?;
        if host.is_empty() {
            return Err(SshError::Config(format!("ProxyJump 跳缺少主机：{s}")));
        }
        Ok(ProxyHop {
            user,
            host,
            port,
        })
    }
}

/// `[user@]host[:port]` 里 host/port 的部分。
///
/// IPv6 必须写成 `[::1]:22`——和 OpenSSH 一样，裸 IPv6 在冒号上无法消歧，
/// 这里直接报错并提示加方括号，而不是猜一个错的主机名。
fn split_host_port(rest: &str) -> Result<(String, u16), SshError> {
    if let Some(stripped) = rest.strip_prefix('[') {
        let Some((host, tail)) = stripped.split_once(']') else {
            return Err(SshError::Config(format!(
                "ProxyJump 的 IPv6 地址缺少 ']'：{rest}"
            )));
        };
        let port = match tail.strip_prefix(':') {
            Some(p) if !p.is_empty() => parse_port(p)?,
            Some(_) | None => DEFAULT_PORT,
        };
        return Ok((host.to_string(), port));
    }
    if rest.matches(':').count() > 1 {
        return Err(SshError::Config(format!(
            "ProxyJump 的 IPv6 地址需要方括号：{rest}"
        )));
    }
    match rest.rsplit_once(':') {
        Some((host, p)) if !p.is_empty() => Ok((host.to_string(), parse_port(p)?)),
        _ => Ok((rest.to_string(), DEFAULT_PORT)),
    }
}

fn parse_port(raw: &str) -> Result<u16, SshError> {
    let port: u16 = raw
        .parse()
        .map_err(|_| SshError::Config(format!("ProxyJump 端口非法：{raw}")))?;
    if port == 0 {
        return Err(SshError::Config("ProxyJump 端口不能是 0".to_string()));
    }
    Ok(port)
}

/// 解析整条 `ProxyJump`：逗号分隔；`none` / 空串表示直连（返回空 Vec）。
pub fn parse_proxy_jump(raw: &str) -> Result<Vec<ProxyHop>, SshError> {
    let raw = raw.trim();
    if raw.is_empty() || raw.eq_ignore_ascii_case("none") {
        return Ok(Vec::new());
    }
    raw.split(',')
        .map(str::trim)
        .filter(|h| !h.is_empty())
        .map(ProxyHop::parse)
        .collect()
}

/// 出网代理的协议档位（W1-b / D8）。
///
/// **不含 `Jump`**：跳板机是 SSH 自己的 `direct-tcpip` 链（`proxy_jump`），
/// 与「把第一段 TCP 交给一个代理」是两件不同的事，混在一个枚举里会让
/// 「选了 HTTP 却还在走 -J」变成类型上合法的状态。
///
/// `ProxyCommand` 也不在这里：官方公开文档明确写「Strictly speaking,
/// ProxyCommand is not implemented in WindTerm」，按 D8 保持灰显，不造一个
/// 没有语义的档位。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProxyKind {
    /// HTTP `CONNECT` 隧道（可带 Basic 用户名密码）。
    Http,
    /// SOCKS5 `CONNECT`（RFC 1928，可带 RFC 1929 用户名密码）。
    Socks5,
}

impl ProxyKind {
    pub fn label(&self) -> &'static str {
        match self {
            ProxyKind::Http => "HTTP",
            ProxyKind::Socks5 => "SOCKS5",
        }
    }
}

/// 代理认证。密码只在内存里活一次握手，不进 `SessionConfig`、不落盘。
#[derive(Clone, PartialEq, Eq)]
pub struct ProxyAuth {
    pub username: String,
    pub password: String,
}

/// 手写 `Debug`：`password` 脱敏，`username` 照常打印——用户名是排查「代理
/// 认证被拒」的第一线索，把它也抹掉等于把这个类型从调试工具里删掉。
impl fmt::Debug for ProxyAuth {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ProxyAuth")
            .field("username", &self.username)
            .field("password", &REDACTED)
            .finish()
    }
}

/// 一条出网代理：`None` 表示直连，由 [`SshOptions::outbound_proxy`] 承载。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OutboundProxy {
    pub kind: ProxyKind,
    pub host: String,
    pub port: u16,
    pub auth: Option<ProxyAuth>,
}

impl OutboundProxy {
    /// 错误信息里怎么称呼这个代理。
    pub fn label(&self) -> String {
        format!("{}://{}:{}", self.kind.label(), self.host, self.port)
    }

    /// 碰网络之前就能判定的配置错误。
    pub fn validate(&self) -> Result<(), SshError> {
        if self.host.trim().is_empty() {
            return Err(SshError::Config(format!(
                "选择了 {} 出网代理却没填代理主机",
                self.kind.label()
            )));
        }
        if self.host.contains(char::is_whitespace) {
            return Err(SshError::Config(format!(
                "代理主机名里不能有空格：{:?}",
                self.host
            )));
        }
        if self.port == 0 {
            return Err(SshError::Config("代理端口不能是 0".to_string()));
        }
        Ok(())
    }
}

/// 算法白名单：`ssh -c` / `KexAlgorithms` / `MACs` 的等价物。
///
/// **白名单，不是排序。** 给了哪几个就只下发哪几个；空列表 = 用 russh 的默认
/// 清单（顺序也照它的）。
///
/// 为什么需要它：企业环境里常见「服务端只开某个旧算法」或「合规要求禁用
/// SHA-1 系列」。没有这个开关时，用户唯一的出路是改 `~/.ssh/config`，
/// 而我们的连接层根本不读那个文件——那会变成一种沉默的撒谎。
///
/// 顺序**按用户给的**：`KexAlgorithms=a,b` 在 OpenSSH 里也是「先试 a」，
/// 这里保持一致，不做悄悄的重排。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct AlgorithmPolicy {
    /// 对称加密（`aes256-gcm@openssh.com` / `chacha20-poly1305@openssh.com` …）。
    pub ciphers: Vec<String>,
    /// 密钥交换（`curve25519-sha256` / `diffie-hellman-group14-sha256` …）。
    pub kex: Vec<String>,
    /// 消息认证码（`hmac-sha2-256` …）。
    pub mac: Vec<String>,
}

impl AlgorithmPolicy {
    /// 三个清单都空 = 不限制（默认行为）。
    pub fn is_unrestricted(&self) -> bool {
        self.ciphers.is_empty() && self.kex.is_empty() && self.mac.is_empty()
    }

    /// 把白名单套到 russh 的默认偏好上。
    ///
    /// 认不出的名字直接报错，**不悄悄忽略**：`ciphers = ["aes128-ctr", "aesgcm"]`
    /// 里混进一个错名字，静默忽略等于让用户以为限制生效了，而实际只是少试了一个。
    pub fn apply(&self, base: russh::Preferred) -> Result<russh::Preferred, String> {
        let mut out = base;
        if !self.ciphers.is_empty() {
            out.cipher = std::borrow::Cow::Owned(pick::<russh::cipher::Name>(&self.ciphers, "ciphers")?);
        }
        if !self.kex.is_empty() {
            out.kex = std::borrow::Cow::Owned(pick::<russh::kex::Name>(&self.kex, "kex")?);
        }
        if !self.mac.is_empty() {
            out.mac = std::borrow::Cow::Owned(pick::<russh::mac::Name>(&self.mac, "mac")?);
        }
        Ok(out)
    }
}

/// 把字符串清单翻成 russh 的算法名，保持顺序、去掉重复。
///
/// 泛型只为三种 `Name` 共用一份逻辑：它们都是 `TryFrom<&str>`（错误类型 `()`），
/// 且都带一个字符串化的名字。
fn pick<T>(wanted: &[String], label: &str) -> Result<Vec<T>, String>
where
    T: for<'a> TryFrom<&'a str> + WireName,
{
    let mut out: Vec<T> = Vec::with_capacity(wanted.len());
    for (index, token) in wanted.iter().enumerate() {
        let name = T::try_from(token.as_str()).map_err(|_| {
            format!(
                "{label} 里的第 {} 项 {token:?} 不是本版本认识的算法名（拼错，或这个算法没编进来）",
                index + 1
            )
        })?;
        // 去重只能比字符串：`Name` 没有稳定的 Ord，但它的字符串形式就是协议名。
        if out.iter().any(|seen| seen.wire() == token) {
            continue;
        }
        out.push(name);
    }
    Ok(out)
}

/// 两种 `Name` 都要能拿到自己的字符串形式；这里用一个小 trait 把它收口，
/// 免得 `pick` 里出现三个 `if type_name::<T>() == ...` 的丑陋分支。
trait WireName {
    fn wire(&self) -> &str;
}

impl WireName for russh::cipher::Name {
    fn wire(&self) -> &str {
        self.as_ref()
    }
}
impl WireName for russh::kex::Name {
    fn wire(&self) -> &str {
        self.as_ref()
    }
}
impl WireName for russh::mac::Name {
    fn wire(&self) -> &str {
        self.as_ref()
    }
}

/// 连接参数。
#[derive(Clone)]
pub struct SshOptions {
    pub host: String,
    pub port: u16,
    pub user: String,
    pub auth: AuthMethod,
    /// D2（OneKey）：凭据回退链。**有序**，空 = 回落单一 `auth`（旧行为不变）。
    ///
    /// 非空时 `authenticate()` 按此顺序逐个尝试，第一个被接受即停。被拒
    /// （`SshError::Auth`）或该方式本地根本用不了（`SshError::Key`，例如私钥
    /// 读不出来 / 需要口令却没给）时继续下一种；遇到通道 / 本地环境错误则
    /// 立即返回——那两类换一种认证方式也不会好，继续试只会掩盖真正的病因。
    ///
    /// 与纯层 `term_core::auth::AuthChain` 的分工：链的**顺序**由那个状态机
    /// 决定（按本地能力过滤 + `prefer` 重排），本层只负责**执行**这条链，
    /// 不在连接层重新发明一套排序规则。
    pub auth_chain: Vec<AuthMethod>,
    pub host_key: HostKeyPolicy,
    /// `None` = `~/.ssh/known_hosts`。测试与「自带一份账本」的场景要给显式路径。
    pub known_hosts: Option<PathBuf>,
    pub term: String,
    pub cols: u32,
    pub rows: u32,
    pub connect_timeout: Duration,
    pub keepalive: Option<Duration>,
    /// Batch 2 契约：ProxyJump 链（已解析）。空 = 直连。
    ///
    /// 只放**结构**不放原始字符串：语法错误在构造时（`parse_proxy_jump`）
    /// 就报出来，连接层拿到的永远是能用的跳。
    pub proxy_jump: Vec<ProxyHop>,
    /// Batch 2 契约：把本地 agent 转发到远端（等价 `ssh -A`）。
    pub agent_forward: bool,
    /// X11 转发（等价 `ssh -X`）：在会话通道打开后发 `x11-req` 全局请求，
    /// 服务端有人连它的 `$DISPLAY` 时回开一条 `x11` 通道，本端再桥到**本地**
    /// 的 X server（`$DISPLAY`）。`Screen` 是 X11 screen number，通常 0。
    pub x11_forward: bool,
    /// X11 screen number，随 `x11-req` 一并下发给服务端（`-X` 的 `:<host>.<screen>`）。
    pub x11_screen: u32,
    /// 算法白名单（空 = 用 russh 默认清单）。语义见 [`AlgorithmPolicy`]。
    pub algorithms: AlgorithmPolicy,
    /// W1-b：出网代理。`None` = 第一段 TCP 直连目标（或第一跳跳板机）。
    ///
    /// 只作用于**整条链的第一段 TCP**：之后的每一跳都在上一条 SSH 连接里开
    /// `direct-tcpip`，早就不需要也不该再穿过一次代理。
    /// 有它时目标主机名不在本地解析——交给代理（官文「DNS 可走代理」）。
    pub outbound_proxy: Option<OutboundProxy>,
}

/// 手写 `Debug`：逐字段打印，凭据靠 [`AuthMethod`] / [`ProxyAuth`] 自己的
/// 脱敏实现收口（`auth` / `auth_chain` / `outbound_proxy` 三个字段因此天然安全，
/// 不需要在这里再判一次类型）。
///
/// 为什么不做成「只打印 host:port」：这个结构是连接失败时唯一能解释「当时到底
/// 用了哪套参数」的东西——算法白名单、超时、跳板链、X11 开关都在这里。把它压成
/// 一句话，排障的人就得回到代码里加 `println!` 并重新编译。
impl fmt::Debug for SshOptions {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SshOptions")
            .field("host", &self.host)
            .field("port", &self.port)
            .field("user", &self.user)
            .field("auth", &self.auth)
            .field("auth_chain", &self.auth_chain)
            .field("host_key", &self.host_key)
            .field("known_hosts", &self.known_hosts)
            .field("term", &self.term)
            .field("cols", &self.cols)
            .field("rows", &self.rows)
            .field("connect_timeout", &self.connect_timeout)
            .field("keepalive", &self.keepalive)
            .field("proxy_jump", &self.proxy_jump)
            .field("agent_forward", &self.agent_forward)
            .field("x11_forward", &self.x11_forward)
            .field("x11_screen", &self.x11_screen)
            .field("algorithms", &self.algorithms)
            .field("outbound_proxy", &self.outbound_proxy)
            .finish()
    }
}

impl SshOptions {
    /// 最小可用参数：主机、用户名、认证方式。其余取默认值。
    pub fn new(host: impl Into<String>, user: impl Into<String>, auth: AuthMethod) -> Self {
        Self {
            host: host.into(),
            port: DEFAULT_PORT,
            user: user.into(),
            auth,
            // D2：默认空链 = 只用上面这一个 `auth`（旧行为不变）。
            auth_chain: Vec::new(),
            host_key: HostKeyPolicy::Tofu,
            known_hosts: None,
            term: DEFAULT_TERM.to_string(),
            cols: 80,
            rows: 24,
            connect_timeout: DEFAULT_CONNECT_TIMEOUT,
            keepalive: Some(DEFAULT_KEEPALIVE),
            proxy_jump: Vec::new(),
            agent_forward: false,
            x11_forward: false,
            x11_screen: 0,
            algorithms: AlgorithmPolicy::default(),
            outbound_proxy: None,
        }
    }

    /// 表单校验：这些错在碰网络之前就该报出来。
    ///
    /// 放在这里而不是 `connect()` 里的第一行，是为了让 GUI 能在「点确定」那一刻
    /// 就把红字打在输入框旁边，而不是等三秒后弹一个连接失败。
    pub fn validate(&self) -> Result<(), SshError> {
        if self.host.trim().is_empty() {
            return Err(SshError::Config("没有填写主机名".to_string()));
        }
        if self.host.contains(char::is_whitespace) {
            return Err(SshError::Config(format!(
                "主机名里不能有空格：{:?}",
                self.host
            )));
        }
        if self.user.trim().is_empty() {
            return Err(SshError::Config("没有填写用户名".to_string()));
        }
        if self.port == 0 {
            return Err(SshError::Config("端口不能是 0".to_string()));
        }
        if self.cols == 0 || self.rows == 0 {
            return Err(SshError::Config(format!(
                "终端尺寸不能是 0：{}x{}",
                self.cols, self.rows
            )));
        }
        // 跳已经解析成结构，这里兜底检查「主机非空 / 端口非 0」——超出自解析器
        // 能保证的范围（比如调用方手动改了字段）也不放过去。
        for hop in &self.proxy_jump {
            if hop.host.trim().is_empty() {
                return Err(SshError::Config("ProxyJump 跳缺少主机".to_string()));
            }
            if hop.port == 0 {
                return Err(SshError::Config("ProxyJump 跳端口不能是 0".to_string()));
            }
        }
        if let Some(proxy) = &self.outbound_proxy {
            // 「选了 HTTP/SOCKS5 却没填主机」必须在碰网络前报错：否则
            // 连接层只能悄悄退化成直连，那正是 W1-b 要消灭的静默失效。
            proxy.validate()?;
        }
        if let AuthMethod::PrivateKey { path, .. } = &self.auth {
            if path.as_os_str().is_empty() {
                return Err(SshError::Config("选择了私钥认证但没选文件".to_string()));
            }
        }
        // D2：链里每一环也要过同样的「私钥必须有文件」检查：走到链中段才发现
        // 少填了文件，用户已经白等一轮握手了。
        for method in &self.auth_chain {
            if let AuthMethod::PrivateKey { path, .. } = method {
                if path.as_os_str().is_empty() {
                    return Err(SshError::Config(
                        "认证链里有私钥认证但没选文件".to_string(),
                    ));
                }
            }
        }
        Ok(())
    }

    /// 状态栏 / 日志用的一句话。
    ///
    /// 不含密码、不含私钥路径：描述会被写进日志，也是会话列表里唯一会
    /// 出现在截图上的东西。
    pub fn description(&self) -> String {
        if self.port == DEFAULT_PORT {
            format!("ssh://{}@{}", self.user, self.host)
        } else {
            format!("ssh://{}@{}:{}", self.user, self.host, self.port)
        }
    }

    /// 实际使用的 `known_hosts` 路径（显式路径优先，否则 `~/.ssh/known_hosts`）。
    ///
    /// 取不到家目录时返回 `None`：让调用方决定是「报错」还是「不检查」，
    /// 而不是在这里替它选一个目录出来。
    pub fn resolved_known_hosts(&self) -> Option<PathBuf> {
        if let Some(p) = &self.known_hosts {
            return Some(p.clone());
        }
        let home = std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE"))?;
        Some(PathBuf::from(home).join(".ssh").join("known_hosts"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn opts() -> SshOptions {
        SshOptions::new(
            "example.com",
            "root",
            AuthMethod::Password {
                password: "x".to_string(),
            },
        )
    }

    #[test]
    fn defaults_are_the_ones_a_terminal_needs() {
        let o = opts();
        assert_eq!(o.port, 22);
        assert_eq!(o.term, "xterm-256color");
        assert_eq!((o.cols, o.rows), (80, 24));
        assert!(o.keepalive.is_some());
        assert!(
            o.algorithms.is_unrestricted(),
            "默认不下发算法白名单，交给 russh"
        );
        // 默认不设「空闲就断」：交互式终端可以开一整天。
        assert_eq!(o.host_key, HostKeyPolicy::Tofu);
    }

    #[test]
    fn an_empty_host_is_refused_before_touching_the_network() {
        let mut o = opts();
        o.host = "  ".to_string();
        let e = o.validate().unwrap_err();
        assert!(matches!(e, SshError::Config(_)));
        assert!(e.to_string().contains("主机名"), "{e}");
    }

    #[test]
    fn a_host_with_spaces_is_refused() {
        let mut o = opts();
        o.host = "example .com".to_string();
        assert!(o.validate().is_err());
    }

    #[test]
    fn empty_user_port_and_size_are_refused() {
        let mut o = opts();
        o.user.clear();
        assert!(o.validate().is_err());

        let mut o = opts();
        o.port = 0;
        assert!(o.validate().is_err());

        let mut o = opts();
        o.rows = 0;
        assert!(o.validate().is_err());
    }

    #[test]
    fn a_private_key_auth_without_a_file_is_refused() {
        let mut o = opts();
        o.auth = AuthMethod::PrivateKey {
            path: PathBuf::new(),
            passphrase: None,
        };
        assert!(o.validate().is_err());
    }

    #[test]
    fn the_description_hides_the_port_when_it_is_the_default() {
        let o = opts();
        assert_eq!(o.description(), "ssh://root@example.com");

        let mut o2 = opts();
        o2.port = 2222;
        assert_eq!(o2.description(), "ssh://root@example.com:2222");
    }

    #[test]
    fn the_description_never_contains_the_password() {
        let mut o = opts();
        o.auth = AuthMethod::Password {
            password: "hunter2".to_string(),
        };
        assert!(!o.description().contains("hunter2"));
    }

    #[test]
    fn auth_method_debug_redacts_the_password_and_keeps_the_variant() {
        // 断言三件事：密码原文不出现（否则等于没脱敏）、出现占位符（否则看不出
        // 这里被有意抹掉，排障的人会以为字段真的是空的）、变体名还在（否则
        // Debug 失去「这是哪种认证」这一基本信息）。
        let auth = AuthMethod::Password {
            password: "hunter2".to_string(),
        };
        let shown = format!("{auth:?}");
        assert!(!shown.contains("hunter2"), "Debug 写出了密码原文：{shown}");
        assert!(shown.contains("redacted"), "没标出这里被脱敏了：{shown}");
        assert!(shown.contains("Password"), "变体名不该被一起抹掉：{shown}");
    }

    #[test]
    fn proxy_auth_debug_redacts_the_password_and_keeps_the_username() {
        let auth = ProxyAuth {
            username: "proxyuser".to_string(),
            password: "pr0xy-s3cret".to_string(),
        };
        let shown = format!("{auth:?}");
        assert!(
            !shown.contains("pr0xy-s3cret"),
            "Debug 写出了代理密码：{shown}"
        );
        assert!(shown.contains("redacted"), "没标出这里被脱敏了：{shown}");
        assert!(
            shown.contains("proxyuser"),
            "用户名不是秘密，排障要用：{shown}"
        );
    }

    #[test]
    fn ssh_options_debug_redacts_every_credential_it_carries() {
        // 一次把三条持有凭据的路径都走一遍：单 `auth`、`auth_chain` 里的口令、
        // 以及出网代理的密码。少测一条，日后新增一条凭据路径就会悄悄漏。
        let mut o = opts();
        o.auth = AuthMethod::Password {
            password: "hunter2".to_string(),
        };
        o.auth_chain = vec![AuthMethod::PrivateKey {
            path: PathBuf::from("/home/root/.ssh/id_ed25519"),
            passphrase: Some("key-passphrase".to_string()),
        }];
        o.outbound_proxy = Some(OutboundProxy {
            kind: ProxyKind::Http,
            host: "proxy.example.com".to_string(),
            port: 8080,
            auth: Some(ProxyAuth {
                username: "pu".to_string(),
                password: "proxy-pass".to_string(),
            }),
        });

        let shown = format!("{o:?}");
        for secret in ["hunter2", "key-passphrase", "proxy-pass"] {
            assert!(!shown.contains(secret), "Debug 泄露了 {secret}：{shown}");
        }
        assert!(shown.contains("redacted"), "没标出脱敏位置：{shown}");
    }

    #[test]
    fn debug_still_shows_non_secret_fields_and_keeps_none_distinguishable() {
        // 防「图省事把整个 Debug 打成常量串」：主机名、用户名、私钥路径都必须
        // 还在，否则脱敏的代价就是失去调试价值。
        let o = SshOptions::new(
            "example.com",
            "root",
            AuthMethod::PrivateKey {
                path: PathBuf::from("/home/root/.ssh/id_ed25519"),
                passphrase: None,
            },
        );
        let shown = format!("{o:?}");
        assert!(shown.contains("example.com"), "主机名不见了：{shown}");
        assert!(shown.contains("user: \"root\""), "用户名不见了：{shown}");
        assert!(shown.contains("id_ed25519"), "私钥路径不见了：{shown}");
        assert!(
            shown.contains("None"),
            "没配口令时要看得出是 None，而不是被抹成占位符：{shown}"
        );

        // 配了口令的同一种认证：必须能看出是和上面**不同**的状态（Some），
        // 但值依然不可见。
        let with = AuthMethod::PrivateKey {
            path: PathBuf::from("/home/root/.ssh/id_ed25519"),
            passphrase: Some("key-passphrase".to_string()),
        };
        let shown = format!("{with:?}");
        assert!(shown.contains("Some"), "配了口令要看得出是有值：{shown}");
        assert!(shown.contains("redacted"), "但有值也不该显形：{shown}");
        assert!(!shown.contains("key-passphrase"), "口令原文泄露：{shown}");
    }

    #[test]
    fn an_explicit_known_hosts_wins_over_the_home_directory() {
        let mut o = opts();
        o.known_hosts = Some(PathBuf::from("/tmp/k"));
        assert_eq!(o.resolved_known_hosts(), Some(PathBuf::from("/tmp/k")));
    }

    #[test]
    fn a_proxy_jump_chain_parses_into_ordered_hops() {
        let hops = parse_proxy_jump("deploy@bastion:2222, ops@hop2").unwrap();
        assert_eq!(hops.len(), 2);
        assert_eq!(
            hops[0],
            ProxyHop {
                user: Some("deploy".into()),
                host: "bastion".into(),
                port: 2222
            }
        );
        assert_eq!(
            hops[1],
            ProxyHop {
                user: Some("ops".into()),
                host: "hop2".into(),
                port: DEFAULT_PORT
            }
        );
    }

    #[test]
    fn none_or_empty_proxy_means_direct() {
        assert!(parse_proxy_jump("").unwrap().is_empty());
        assert!(parse_proxy_jump("none").unwrap().is_empty());
        assert!(parse_proxy_jump(" NONE ").unwrap().is_empty());
    }

    #[test]
    fn a_bare_host_uses_the_default_port_and_no_user() {
        assert_eq!(
            parse_proxy_jump("bastion").unwrap(),
            vec![ProxyHop {
                user: None,
                host: "bastion".into(),
                port: DEFAULT_PORT
            }]
        );
    }

    #[test]
    fn an_ipv6_hop_needs_brackets() {
        let hops = parse_proxy_jump("[::1]:2200").unwrap();
        assert_eq!(hops[0].host, "::1");
        assert_eq!(hops[0].port, 2200);
        assert!(ProxyHop::parse("::1").is_err(), "裸 IPv6 应被拒绝");
    }

    #[test]
    fn bad_ports_are_refused() {
        assert!(ProxyHop::parse("bastion:0").is_err());
        assert!(ProxyHop::parse("bastion:notaport").is_err());
        assert!(ProxyHop::parse("@bastion").is_err());
    }

    #[test]
    fn proxy_and_agent_default_to_direct_and_off() {
        let o = opts();
        assert!(o.proxy_jump.is_empty());
        assert!(!o.agent_forward);
    }

    #[test]
    fn validate_accepts_a_well_formed_proxy_chain() {
        let mut o = opts();
        o.proxy_jump = parse_proxy_jump("bastion:2222").unwrap();
        assert!(o.validate().is_ok());
    }

    #[test]
    fn an_outbound_proxy_without_a_host_is_refused_before_the_network() {
        let mut o = opts();
        o.outbound_proxy = Some(OutboundProxy {
            kind: ProxyKind::Http,
            host: "  ".to_string(),
            port: 8080,
            auth: None,
        });
        let e = o.validate().unwrap_err();
        assert!(matches!(e, SshError::Config(_)), "{e:?}");
        assert!(e.to_string().contains("没填代理主机"), "{e}");
    }

    #[test]
    fn a_complete_outbound_proxy_passes_validation() {
        let mut o = opts();
        o.outbound_proxy = Some(OutboundProxy {
            kind: ProxyKind::Socks5,
            host: "127.0.0.1".to_string(),
            port: 1080,
            auth: None,
        });
        assert!(o.validate().is_ok());
        assert_eq!(
            o.outbound_proxy.as_ref().unwrap().label(),
            "SOCKS5://127.0.0.1:1080"
        );
    }
}

#[cfg(test)]
mod algorithm_policy_tests {
    use super::*;

    fn names<T: AsRef<str>>(list: &[T]) -> Vec<String> {
        list.iter().map(|n| n.as_ref().to_string()).collect()
    }

    #[test]
    fn an_empty_policy_leaves_every_default_alone() {
        let base = russh::Preferred::default();
        let before = names(&base.cipher);
        let after = AlgorithmPolicy::default().apply(base).expect("空清单不该失败");
        assert_eq!(names(&after.cipher), before);
    }

    #[test]
    fn a_whitelist_keeps_only_the_listed_algorithms_in_the_given_order() {
        let policy = AlgorithmPolicy {
            ciphers: vec![
                "chacha20-poly1305@openssh.com".to_string(),
                "aes256-gcm@openssh.com".to_string(),
            ],
            ..Default::default()
        };
        let applied = policy.apply(russh::Preferred::default()).expect("合法名字");
        assert_eq!(
            names(&applied.cipher),
            [
                "chacha20-poly1305@openssh.com".to_string(),
                "aes256-gcm@openssh.com".to_string()
            ],
            "顺序按用户给的，且只留清单里的"
        );
    }

    #[test]
    fn a_typo_fails_before_we_touch_the_network() {
        let policy = AlgorithmPolicy {
            ciphers: vec!["aes256-gcm".to_string()], // 少了 @openssh.com
            ..Default::default()
        };
        let err = policy
            .apply(russh::Preferred::default())
            .expect_err("错名字必须报错，不能静默忽略");
        assert!(err.contains("ciphers"), "错误要说清是哪一类：{err}");
        assert!(err.contains("aes256-gcm"), "也要说清是哪个名字：{err}");
    }

    #[test]
    fn duplicates_collapse_instead_of_being_sent_twice() {
        let policy = AlgorithmPolicy {
            kex: vec![
                "curve25519-sha256".to_string(),
                "curve25519-sha256".to_string(),
            ],
            ..Default::default()
        };
        let applied = policy.apply(russh::Preferred::default()).expect("合法名字");
        assert_eq!(names(&applied.kex), ["curve25519-sha256"]);
    }

    #[test]
    fn the_three_lists_are_independent() {
        let policy = AlgorithmPolicy {
            mac: vec!["hmac-sha2-256".to_string()],
            ..Default::default()
        };
        let base = russh::Preferred::default();
        let cipher_default = names(&base.cipher);
        let applied = policy.apply(base).expect("合法名字");
        assert_eq!(names(&applied.mac), ["hmac-sha2-256"]);
        assert_eq!(
            names(&applied.cipher),
            cipher_default,
            "只限制 mac 时，加密算法清单不该被动过"
        );
        assert!(!policy.is_unrestricted());
    }
}

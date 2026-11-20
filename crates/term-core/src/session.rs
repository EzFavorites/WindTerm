//! Session definitions: what the user creates in the session tree, and what is
//! needed to actually start one. The protocol list, the SSH authentication
//! switches, the serial defaults and the terminal type all mirror a conventional terminal emulator's
//! program's own `terminal/configs/session.config` and its session classes
//! (`SshSession`, `TelnetSession`, `SerialSession`, `ShellSession`, `TcpSession`).

use serde::{Deserialize, Serialize};
use std::fmt;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Protocol {
    Shell,
    Ssh,
    Telnet,
    Serial,
    Tcp,
}

impl fmt::Display for Protocol {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.label())
    }
}

impl Protocol {
    pub const ALL: [Protocol; 5] = [
        Protocol::Shell,
        Protocol::Ssh,
        Protocol::Telnet,
        Protocol::Serial,
        Protocol::Tcp,
    ];

    pub fn label(&self) -> &'static str {
        match self {
            Protocol::Shell => "Local Shell",
            Protocol::Ssh => "SSH",
            Protocol::Telnet => "Telnet",
            Protocol::Serial => "Serial",
            Protocol::Tcp => "TCP",
        }
    }

    /// Which protocols this build can open. Every one of them runs: the local
    /// shell in a locally allocated pseudo-terminal, and the remote ones by
    /// running the system's own `ssh`/`telnet`/`nc`/`cu` client inside that
    /// pseudo-terminal (a conventional terminal bundles its own SSH/Telnet clients, which are
    /// not available here).
    pub fn is_connectable(&self) -> bool {
        true
    }

    pub fn default_port(&self) -> u16 {
        match self {
            Protocol::Ssh => crate::config::SSH_PORT,
            Protocol::Telnet => crate::config::TELNET_PORT,
            Protocol::Tcp => 80,
            _ => 0,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum AuthMethod {
    Password,
    PublicKey,
    /// `ssh.authAgent`
    Agent,
    /// `ssh.authGssapiWithMic`
    GssapiWithMic,
    /// `ssh.authKeyboardInteractive`：服务端逐条提问（密码 / OTP / 二次验证）。
    /// 答案在连接时由密码对话框收集，不落盘。
    KeyboardInteractive,
}

impl fmt::Display for AuthMethod {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            AuthMethod::Password => "Password",
            AuthMethod::PublicKey => "Public Key",
            AuthMethod::Agent => "Agent",
            AuthMethod::GssapiWithMic => "GSSAPI with MIC",
            AuthMethod::KeyboardInteractive => "Keyboard Interactive",
        })
    }
}

/// 会话走哪一种代理（W1-b / D8）。
///
/// 四档与 New Session ▸ Proxy 页的四个单选**一一对应**：配置里存的就是
/// 控件显示的那一个。用字符串 tag 落盘（见 [`ProxyType::from_tag`]）而不是
/// 序号：老配置没有这个键时缺省是空串，天然等于「没设置过」，不会把
/// `0` 误读成「第 0 项」而掩盖「其实没有这一项」。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ProxyType {
    #[default]
    None,
    /// `ssh -J`：SSH 自己的 direct-tcpip 跳板链，不是出网代理。
    Jump,
    /// HTTP `CONNECT` 隧道。
    Http,
    /// SOCKS5 `CONNECT`。
    Socks5,
}

impl ProxyType {
    pub const ALL: [ProxyType; 4] = [
        ProxyType::None,
        ProxyType::Jump,
        ProxyType::Http,
        ProxyType::Socks5,
    ];

    pub fn tag(self) -> &'static str {
        match self {
            ProxyType::None => "none",
            ProxyType::Jump => "jump",
            ProxyType::Http => "http",
            ProxyType::Socks5 => "socks5",
        }
    }

    /// 未知 tag 一律回落 `None`：手改过 JSON 也不能凭空造出一条代理。
    pub fn from_tag(raw: &str) -> Self {
        match raw.trim().to_ascii_lowercase().as_str() {
            "jump" => ProxyType::Jump,
            "http" => ProxyType::Http,
            "socks5" => ProxyType::Socks5,
            _ => ProxyType::None,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            ProxyType::None => "None",
            ProxyType::Jump => "Jump",
            ProxyType::Http => "Http proxy",
            ProxyType::Socks5 => "Socks v5 proxy",
        }
    }

    /// 需要「把第一段 TCP 交给代理」的两档。
    pub fn is_outbound(self) -> bool {
        matches!(self, ProxyType::Http | ProxyType::Socks5)
    }
}

/// `SessionConfig` 反序列化缺省：老会话库没有这些字段时，按 WindTerm 默认补齐，
/// 保证「重排页面结构」不会让既有会话突然改变行为（见下方 `system`/`onekey`/`sftp`）。
fn default_system() -> String {
    "linux".to_string()
}
fn default_onekey() -> String {
    "None".to_string()
}
fn default_true() -> bool {
    true
}

/// 光标周期 / 宽度的反序列化缺省。老会话库没有这两条时回落出厂值，
/// 行为与「2026-11-19 之前是渲染层常数」时完全一致。
fn default_caret_period() -> f32 {
    crate::config::DEFAULT_CARET_PERIOD
}
fn default_caret_width() -> f32 {
    crate::config::DEFAULT_CARET_WIDTH
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SessionConfig {
    pub name: String,
    pub protocol: Protocol,
    pub host: String,
    pub port: u16,
    pub username: String,
    /// `session.group` — the folder the session is filed under in the tree.
    pub group: String,
    /// `ssh.authPassword` / `authPublicKey` / `authAgent` / `authGssapiWithMic`
    ///
    /// D2：这是**首选**方法。回退链见 `auth_chain`。
    pub auth: AuthMethod,
    /// D2 OneKey：认证回退链（有序的**方法种类**，空 = 不回退，旧行为）。
    ///
    /// 执行顺序是 `auth` 在前、`auth_chain` 依次在后并去重，见
    /// [`SessionConfig::auth_chain_with_primary`]。这里只存种类不含凭据：
    /// 密码/交互答案仍在保险库或对话框，私钥路径仍在 `identity_file`；
    /// 连接层按种类到各自来源取凭据。
    ///
    /// `#[serde(default)]`：老会话库没有这个键时回落空链，行为与升级前一致。
    #[serde(default)]
    pub auth_chain: Vec<AuthMethod>,
    /// `session.term`
    pub term: String,
    /// `session.charset`
    pub charset: String,
    /// `session.system` —— 终端系统类型，影响功能键 / 转义序列（WindTerm Session 页 `System`）。
    #[serde(default = "default_system")]
    pub system: String,
    /// OneKey 宏名（WindTerm Session 页 `OneKey`，默认 `None`）。
    #[serde(default = "default_onekey")]
    pub onekey: String,
    /// Sftp 子系统开关（WindTerm 默认 ON）。
    #[serde(default = "default_true")]
    pub sftp: bool,
    /// 系统监控开关（WindTerm 默认 OFF）。
    #[serde(default)]
    pub sysmon: bool,
    /// 本地 Shell 的启动命令（仅 Shell 协议；其余协议为空）。
    #[serde(default)]
    pub shell_command: String,
    /// `serial.baudRate` / `dataBits` / `parity` / `stopBits` / `flowControl`
    pub baud_rate: u32,
    pub data_bits: u8,
    pub stop_bits: u8,
    pub parity: String,
    pub flow_control: String,
    /// `session.description` — the free-text note shown in the tree tooltip and
    /// the New Session dialog's Session page.
    pub description: String,
    /// `session.keepAlive` (seconds, 0 = off)
    pub keep_alive: u32,
    /// `session.connectionTimeout` (seconds)
    pub connection_timeout: u32,
    /// `session.tcpKeepAlive`
    pub tcp_keep_alive: bool,
    /// `session.tcpLowDelay`
    pub tcp_low_delay: bool,
    /// R13-d2：走进程内的 russh 客户端，而不是拉系统 `ssh`。
    ///
    /// `#[serde(default)]` 是**迁移策略**，不是偷懒：已经存在的 `user.sessions`
    /// 里没有这个字段，读出来是 `false`（继续用系统 `ssh`）；`SessionConfig::new`
    /// 给新会话写 `true`。于是「老会话不变、新会话默认原生」不需要写任何迁移代码。
    #[serde(default)]
    pub builtin_ssh: bool,
    /// R13-e2：这条会话在连接时一并要起的 SSH 转发（每人一行，`L` / `R` / `D` 开头）。
    ///
    /// 存的是**用户写的那一行**，不是解析后的结构：会话配置属于 `term-core`，
    /// 而规则语法由 `term-tunnel` 定义——把 `Forward` 搬进语义层等于让核心认识网络。
    /// 校验在表单保存时做（那一层能调 `term_tunnel::Forward::parse`）。
    ///
    /// `#[serde(default)]`：没有转发的会话（绝大多数）不需要这一项，
    /// 老配置读出来是空列表。
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub forwards: Vec<String>,
    /// `window.scrollbackLimit`
    pub scrollback: usize,
    /// V1-1：`ssh_config` 的 `ProxyJump` 链（系统 `ssh -J` 直接认这个形状）。
    /// 空串 = 没有跳板。`#[serde(default)]` 让老会话库读出来是空串。
    #[serde(default)]
    pub proxy_jump: String,
    /// V1-1：导入器带过来的 `IdentityFile` 路径，只记路径不记内容。
    #[serde(default)]
    pub identity_file: String,
    /// V1-3：允许把本地 ssh-agent 转发到远端（系统 `ssh -A`）。
    #[serde(default)]
    pub agent_forward: bool,
    /// X11 转发（系统 `ssh -X`）。`#[serde(default)]`：老会话库读出来是 false，
    /// `SessionConfig::new` 给新会话写 false（默认不开——X11 需要本机有 X server）。
    #[serde(default)]
    pub x11_forward: bool,
    /// X11 screen number，随 `x11-req` 下发给服务端（`-X` 的 `:<host>.<screen>`）。
    #[serde(default)]
    pub x11_screen: u32,
    /// W1-b：出网代理档位（tag 见 [`ProxyType::tag`]）。空串 = 老配置，
    /// 由 [`SessionConfig::effective_proxy_type`] 按 `proxy_jump` 推断。
    #[serde(default)]
    pub proxy_type: String,
    /// W1-b：HTTP / SOCKS5 代理主机。选了这两档却留空 = 「填了不生效」，
    /// 表单保存与 `SshOptions::validate` 都会挡下来。
    #[serde(default)]
    pub proxy_host: String,
    /// W1-b：代理端口。`0` 会被判为非法而不是悄悄取 1080。
    #[serde(default)]
    pub proxy_port: u16,
    /// `window.columns`：New Session `Window ▸ Selection` 页设置的初始终端网格列数。
    /// `0` = 未设置，沿用视口/面板尺寸（默认行为）。`#[serde(default)]` 让老会话库读出来是 0。
    #[serde(default)]
    pub cols: usize,
    /// `window.rows`：见 `cols`。
    #[serde(default)]
    pub rows: usize,
    /// `window.mouseWheelScrollLines`：一次滚轮滚动的行数（规格默认 1）。
    #[serde(default)]
    pub wheel_lines: u32,
    /// `window.caretStyle`：光标样式下标（0=Block … 7=Blinking Underline）。
    #[serde(default)]
    pub caret_style: usize,
    /// 光标闪烁周期（秒，「亮多久 = 灭多久」）。老会话库缺这条时回落出厂 0.5，
    /// 与 `caret_style` 同属会话属性。2026-11-19 起可调（此前是渲染层常数）。
    #[serde(default = "default_caret_period")]
    pub caret_period: f32,
    /// 光标宽度（像素，`Underline` / `Beam` 用）。老会话库缺这条时回落出厂 1.5。
    #[serde(default = "default_caret_width")]
    pub caret_width: f32,
    /// 本地 Shell 的启动工作目录；空串 = 沿用当前目录。
    #[serde(default)]
    pub working_dir: String,
    /// 登录脚本（SecureCRT 的 Logon Script）：会话**连上之后**自动跑一次的 `.rhai` 路径。
    ///
    /// 空串 = 没有登录脚本（绝大多数会话）。`#[serde(default)]` 让老会话库读出来是
    /// 空串，行为与升级前一致。
    ///
    /// 时机只有一种：**标签页带着活会话出现之后**。CRT 区分「TCP 连上」与「认证
    /// 通过」两个时刻，我们这里建标签页的那一刻已经是认证之后（内置 SSH 是
    /// `adopt_channel`，本地 Shell 是进程已经起来），所以不必再分两档。
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub logon_script: String,
}

/// 会话配置不足以生成启动计划。
///
/// 为什么单独一个类型而不是 `String`：这是**跨 crate 边界**的错误，调用方要按类别
/// 反应（缺主机 → 提示补表单；打不开 PTY → 提示系统问题；找不到程序 → 提示安装）。
/// 拍平成字符串之后这三种只能显示同一句话。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ConfigError {
    /// 该协议必须有主机名（`Serial` 则是设备路径），但字段是空的。
    MissingHost(Protocol),
}

impl std::fmt::Display for ConfigError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ConfigError::MissingHost(proto) => {
                write!(f, "{} 会话需要填写主机名或设备路径", proto.label())
            }
        }
    }
}

impl std::error::Error for ConfigError {}

impl SessionConfig {
    pub fn new(protocol: Protocol) -> Self {
        Self {
            name: match protocol {
                Protocol::Shell => "Local Shell".to_string(),
                other => format!("New {}", other.label()),
            },
            protocol,
            host: String::new(),
            port: protocol.default_port(),
            // 出厂配置里**没有**宿主身份：`username` 一律空串。
            //
            // 以前 Shell 走 `whoami()` 读 `$USER`，于是同一份代码在两台机器上
            // 产出的默认会话不同——`term-core` 的契约是「给定输入与状态，
            // 产物是什么」，读环境变量等于把宿主状态塞进纯函数（R0）。
            // 想要「新本地 Shell 带上当前登录名」的调用方读环境后走
            // [`SessionConfig::new_local_shell`]，值从参数进来。
            username: String::new(),
            auth: AuthMethod::Password,
            // D2：默认没有回退链（旧行为：只用上面这一个 `auth`）。
            auth_chain: Vec::new(),
            group: crate::config::DEFAULT_GROUP.to_string(),
            term: crate::config::DEFAULT_TERM.to_string(),
            charset: crate::config::CHARSET.to_string(),
            system: "linux".to_string(),
            onekey: "None".to_string(),
            sftp: true,
            sysmon: false,
            shell_command: String::new(),
            description: String::new(),
            baud_rate: u32::from(crate::config::SERIAL_BAUD),
            data_bits: crate::config::SERIAL_DATA_BITS,
            stop_bits: crate::config::SERIAL_STOP_BITS,
            parity: crate::config::SERIAL_PARITY.to_string(),
            flow_control: crate::config::SERIAL_FLOW_CONTROL.to_string(),
            keep_alive: crate::config::KEEP_ALIVE,
            connection_timeout: crate::config::CONNECTION_TIMEOUT,
            tcp_keep_alive: crate::config::TCP_KEEP_ALIVE,
            tcp_low_delay: crate::config::TCP_LOW_DELAY,
            // 新会话默认走原生 SSH（R13-d2）。老会话读出来是 false，保持系统 `ssh`。
            builtin_ssh: matches!(protocol, Protocol::Ssh),
            // 不给任何默认转发规则：一条隧道要用户明确写出来才存在。
            forwards: Vec::new(),
            scrollback: crate::config::DEFAULT_SCROLLBACK,
            proxy_jump: String::new(),
            identity_file: String::new(),
            agent_forward: false,
            x11_forward: false,
            x11_screen: 0,
            // **空串 = 未设置**，不是显式 "none"：代码里直接写 `proxy_jump`
            // 的路径（导入器、脚本、测试夹具）不该被一个默认值悄悄否掉。
            // 显式 "none" 只由表单保存时写入。
            proxy_type: String::new(),
            proxy_host: String::new(),
            proxy_port: 0,
            // 初始网格尺寸 0 = 未设置（沿用视口）；滚轮行数/光标样式取规格默认值。
            cols: 0,
            rows: 0,
            wheel_lines: crate::config::MOUSE_WHEEL_SCROLL_LINES,
            caret_style: 0,
            caret_period: crate::config::DEFAULT_CARET_PERIOD,
            caret_width: crate::config::DEFAULT_CARET_WIDTH,
            working_dir: String::new(),
            logon_script: String::new(),
        }
    }

    /// 新建一条**本地 Shell** 会话，登录名由宿主注入。
    ///
    /// `term-core` 不读 `$USER`：读环境的是允许有副作用的层（`term_io::host::login`），
    /// 这一层只收值。同样两次 `new_local_shell("alice")` 必须逐字段相等——
    /// 这是 R0 在会话模型上的判据，也是把「同一输入不同产物」这条缺陷关掉的地方。
    ///
    /// 只对 `Shell` 有意义：其余协议的 `username` 是用户填的远端账号，
    /// 不能拿本机登录名去冒充，所以那条路仍走 [`SessionConfig::new`]。
    pub fn new_local_shell(login: &str) -> Self {
        let mut config = Self::new(Protocol::Shell);
        config.username = login.to_string();
        config
    }

    /// D2：实际要尝试的**有序**认证方法链 = 首选 `auth` 在前 + 回退链依次在后，
    /// 去重（同一种方法只出现一次）。
    ///
    /// 回退链为空时返回单元素 `[auth]`，于是「没有回退」与「只有一个方法」在
    /// 连接层走同一条路径——连接层不需要区分这两种情况。
    pub fn auth_chain_with_primary(&self) -> Vec<AuthMethod> {
        let mut out = vec![self.auth];
        for method in &self.auth_chain {
            if !out.contains(method) {
                out.push(*method);
            }
        }
        out
    }

    /// **唯一**的「这条会话到底走哪种代理」判定。
    ///
    /// 老会话库没有 `proxy_type` 这个键，serde 缺省给空串；但其中不少是配过
    /// `ProxyJump` 的。若在这里直接返回 `None`，那些会话一升级就会从「走跳板」
    /// 变成「直连」——那是比缺字段严重得多的回归。于是：没显式档位 + 链非空
    /// ⇒ `Jump`。显式档位一旦存在就以它为准（包括显式 `none`）。
    pub fn effective_proxy_type(&self) -> ProxyType {
        let raw = self.proxy_type.trim();
        if raw.is_empty() {
            // 老配置：只可能靠链判断。
            return if self.proxy_jump.trim().is_empty() {
                ProxyType::None
            } else {
                ProxyType::Jump
            };
        }
        // 显式档位（包括显式 "none"）一律以它为准：用户当年确实选了直连，
        // 不能因为库里还留着一条旧链就替他改回去。
        ProxyType::from_tag(raw)
    }

    /// 给应用层造 `term_ssh::OutboundProxy` 用的**纯数据视图**。
    ///
    /// `term-core` 不依赖 `term-ssh`、`term-ssh` 也不认识 `SessionConfig`；
    /// 两边都只认这个三元组。判定只写这一遍，UI 连接路径与无窗口路径
    /// （`desk script` / `desk serve`）就不会出现「一边走代理一边直连」。
    pub fn outbound_proxy_spec(&self) -> Option<(ProxyType, &str, u16)> {
        let kind = self.effective_proxy_type();
        if !kind.is_outbound() {
            return None;
        }
        Some((kind, self.proxy_host.trim(), self.proxy_port))
    }

    /// 表单保存前的校验：选了出网代理就必须填得出一个能连的地址。
    /// `None` = 可以通过。返回的是给用户看的红字，不是日志。
    pub fn validate_proxy(&self) -> Option<String> {
        let kind = self.effective_proxy_type();
        if !kind.is_outbound() {
            return None;
        }
        // 前后空白由 `outbound_proxy_spec` 统一裁掉；这里只拒绝**中间**含空格
        // 的名字。否则「填的时候多打了一个空格」会被当成非法主机，
        // 而同一份值到了连接层却是合法的——两边必须同口径。
        let host = self.proxy_host.trim();
        if host.is_empty() {
            return Some(format!("选了 {} 就必须填代理主机", kind.label()));
        }
        if host.contains(char::is_whitespace) {
            return Some("代理主机名里不能有空格".to_string());
        }
        if self.proxy_port == 0 {
            return Some("代理端口不能是 0".to_string());
        }
        None
    }

    /// The tree item's tooltip. SessionTreeModel::data builds it
    /// as an HTML `<table>` whose rows are, in order: Target, Port, Protocol,
    /// Group, Label, Proxy Type, Proxy Host, Proxy Port, Proxy Command, Working
    /// Directory, Jump Server, Description. Rows with no value are left blank
    /// rather than dropped, so the table keeps its shape across sessions.
    pub fn tooltip_rows(&self) -> Vec<(&'static str, String)> {
        vec![
            ("Target", self.host.clone()),
            ("Port", self.port.to_string()),
            ("Protocol", self.protocol.label().to_string()),
            ("Group", self.group.clone()),
            ("Label", self.name.clone()),
            // W1-b：这三行以前恒为空 / 只认 Jump，会话树 tooltip 于是
            // 永远说「None」——用户填了代理也看不出来。现在与实际生效的
            // 档位同源（effective_proxy_type 是唯一判定入口）。
            ("Proxy Type", self.effective_proxy_type().label().to_string()),
            ("Proxy Host", if self.effective_proxy_type().is_outbound() { self.proxy_host.clone() } else { String::new() }),
            ("Proxy Port", if self.effective_proxy_type().is_outbound() { self.proxy_port.to_string() } else { String::new() }),
            ("Proxy Command", String::new()),
            ("Working Directory", "${HomeDir}".to_string()),
            ("Jump Server", self.proxy_jump.clone()),
            ("Description", self.description.clone()),
        ]
    }

    /// The command line this session would run, or the reason it cannot run.
    pub fn spawn_plan(&self) -> Result<SpawnPlan, ConfigError> {
        match self.protocol {
            Protocol::Shell => Ok(SpawnPlan::LocalShell),
            Protocol::Ssh => {
                if self.host.trim().is_empty() {
                    return Err(ConfigError::MissingHost(self.protocol));
                }
                let mut argv = vec!["ssh".to_string()];
                if !self.username.trim().is_empty() {
                    argv.extend(["-l".to_string(), self.username.trim().to_string()]);
                }
                if self.port != crate::config::SSH_PORT {
                    argv.extend(["-p".to_string(), self.port.to_string()]);
                }
                if self.keep_alive > 0 {
                    argv.extend(["-o".to_string(), format!("ServerAliveInterval={}", self.keep_alive)]);
                }
                // V1-1 / V1-3：ProxyJump 单跳链、IdentityFile、agent 转发。
                // W1-b：出网代理走官文给出的 `nc` 等价式
                // （官方 `proxy-http-socks5` 文档：`nc -X 5 -x 127.0.0.1:7777 %h %p`，
                // HTTP 用 `-X connect`）。**这不是**「ProxyCommand 用户字段」：
                // 那一栏兼容终端本身未实现（D8），仍保持灰显；这里只是让「用系统
                // ssh」这条路径也真的走代理，与内置客户端行为一致。
                // 四档互斥：单选说是 Http 就不能还偷偷带上 `-J`。
                match self.effective_proxy_type() {
                    ProxyType::Jump => {
                        argv.extend(["-J".to_string(), self.proxy_jump.trim().to_string()]);
                    }
                    ProxyType::Http | ProxyType::Socks5 => {
                        let flag = if self.effective_proxy_type() == ProxyType::Http {
                            "connect"
                        } else {
                            "5"
                        };
                        argv.extend([
                            "-o".to_string(),
                            format!(
                                "ProxyCommand=nc -X {flag} -x {}:{} %h %p",
                                self.proxy_host.trim(),
                                self.proxy_port
                            ),
                        ]);
                    }
                    ProxyType::None => {}
                }
                if !self.identity_file.trim().is_empty() {
                    argv.extend(["-i".to_string(), self.identity_file.trim().to_string()]);
                }
                if self.agent_forward {
                    argv.push("-A".to_string());
                }
                if self.x11_forward {
                    argv.push("-X".to_string());
                }
                let auth_opt = match self.auth {
                    AuthMethod::Password => "PreferredAuthentications=password",
                    AuthMethod::PublicKey => "PreferredAuthentications=publickey",
                    AuthMethod::Agent => "IdentitiesOnly=no",
                    AuthMethod::GssapiWithMic => "GSSAPIAuthentication=yes",
                    AuthMethod::KeyboardInteractive => {
                        "PreferredAuthentications=keyboard-interactive"
                    }
                };
                argv.extend(["-o".to_string(), auth_opt.to_string()]);
                argv.extend(["-o".to_string(), format!("TCPKeepAlive={}", if self.tcp_keep_alive { "yes" } else { "no" })]);
                // `tcp_low_delay`（TCP_NODELAY 开关）**不映射成 ssh 选项**：
                // OpenSSH 没有 `TCP_NODELAY` 这个配置项，写了它 ssh 会直接退出
                // （`Bad configuration option: tcp_nodelay`），整条会话就起不来。
                // 而 ssh 客户端本来就会给自己的 socket 设 NODELAY，这个开关在
                // 「用系统 ssh」这条路径上没有可表达的位置。字段保留给内置客户端；
                // 界面上勾不勾都不影响系统 ssh 的行为——见 R17 阶段记录。
                argv.push(self.host.trim().to_string());
                Ok(SpawnPlan::External(argv))
            }
            Protocol::Telnet => {
                if self.host.trim().is_empty() {
                    return Err(ConfigError::MissingHost(self.protocol));
                }
                Ok(SpawnPlan::External(vec![
                    "telnet".to_string(),
                    self.host.trim().to_string(),
                    self.port.to_string(),
                ]))
            }
            Protocol::Tcp => {
                if self.host.trim().is_empty() {
                    return Err(ConfigError::MissingHost(self.protocol));
                }
                Ok(SpawnPlan::External(vec![
                    "nc".to_string(),
                    self.host.trim().to_string(),
                    self.port.to_string(),
                ]))
            }
            Protocol::Serial => {
                // 设备路径就是 `host` 字段（表单上那一格问的是「设备/主机」）。
                // 曾经这里写死 `/dev/tty.usbserial`：界面填什么都连到同一个口，
                // 属于 R5 意义上的假入口——点了有效果，但效果与填写内容无关。
                if self.host.trim().is_empty() {
                    return Err(ConfigError::MissingHost(self.protocol));
                }
                Ok(SpawnPlan::External(vec![
                    "cu".to_string(),
                    "-l".to_string(),
                    self.host.trim().to_string(),
                    "-s".to_string(),
                    self.baud_rate.to_string(),
                ]))
            }
        }
    }
}

/// How a session gets its bytes.
pub enum SpawnPlan {
    /// Run the user's login shell in a locally allocated pseudo-terminal.
    LocalShell,
    /// Run an external client (ssh/telnet/nc/cu) inside a pseudo-terminal.
    External(Vec<String>),
}

pub struct SessionManager {
    pub sessions: Vec<SessionConfig>,
    pub selected: Option<usize>,
    /// Sessions currently running, by index into `sessions`.
    pub running: Vec<usize>,
}

impl SessionManager {
    pub fn new() -> Self {
        let mut mgr = Self {
            sessions: Vec::new(),
            selected: None,
            running: Vec::new(),
        };
        mgr.sessions.push(SessionConfig::new(Protocol::Shell));
        mgr.selected = Some(0);
        mgr
    }

    pub fn add_session(&mut self, config: SessionConfig) -> usize {
        self.sessions.push(config);
        let idx = self.sessions.len() - 1;
        self.selected = Some(idx);
        idx
    }

    pub fn remove_session(&mut self, index: usize) {
        if index >= self.sessions.len() {
            return;
        }
        self.sessions.remove(index);
        self.running.retain(|i| *i != index);
        self.running.iter_mut().for_each(|i| {
            if *i > index {
                *i -= 1;
            }
        });
        self.selected = if self.sessions.is_empty() {
            None
        } else {
            Some(self.selected.unwrap_or(0).min(self.sessions.len() - 1))
        };
    }

    pub fn selected_session(&self) -> Option<&SessionConfig> {
        self.selected.and_then(|i| self.sessions.get(i))
    }

    pub fn session(&self, index: usize) -> Option<&SessionConfig> {
        self.sessions.get(index)
    }
}

impl Default for SessionManager {
    fn default() -> Self {
        Self::new()
    }
}

// ── 打开会话的统一请求（ADR-0027 D4） ──────────────────────────────────
//
// 在 ADR-0025 决策 6 里，脚本与 RPC 各有两条打开路径：引用会话库里的名字，
// 或直接交一份完整配置。此前三个调用方（桥接 `/ 脚本 / CLI`）各自写一遍
// 「意图 → SessionConfig」的映射，于是同一个意图在三个地方各有一份默认值。
// 这个枚举把那句话收敛成**一个类型**，映射点只剩 `session_open::open_request`
// 一处；加一种打开方式时，穷尽 match 会在编译期点出所有需要更新的地方。

/// 打开一条会话的意图。字段是纯数据，不带任何凭据（凭据在保险库）。
///
/// `Profile` 与 `Config` 对应 ADR-0025 决策 6 的两条路径；后两个是脚本与
/// 测试长期在用的形态（CLI 的 `--exec` / 默认本地 shell），一并收进来是为了
/// 让「谁去起会话」只有一个入口。
#[derive(Clone, Debug, PartialEq)]
pub enum OpenRequest {
    /// 引用会话库里的一条会话（名字寻址，ADR-0024）。
    Profile { name: String },
    /// 直接给完整配置（不落库）。**只许进程内传递**：RPC 不接受内联配置，
    /// 否则凭据会进网络请求主体。
    Config(Box<SessionConfig>),
    /// 一次性命令（经 `/bin/sh -c`）。
    Command {
        command: String,
        name: Option<String>,
        cols: usize,
        rows: usize,
    },
    /// 本地登录 shell。
    LocalShell { cols: usize, rows: usize },
}

impl OpenRequest {
    /// 缺省尺寸：与能力面 `session_open` 的 `cols` / `rows` 缺省值同一个数。
    pub const DEFAULT_COLS: usize = 80;
    pub const DEFAULT_ROWS: usize = 24;

    /// 终端列数（两种不带尺寸的变体由调用方自己填，这里只做读取）。
    pub fn cols(&self) -> usize {
        match self {
            OpenRequest::Profile { .. } | OpenRequest::Config(_) => Self::DEFAULT_COLS,
            OpenRequest::Command { cols, .. } | OpenRequest::LocalShell { cols, .. } => *cols,
        }
    }

    pub fn rows(&self) -> usize {
        match self {
            OpenRequest::Profile { .. } | OpenRequest::Config(_) => Self::DEFAULT_ROWS,
            OpenRequest::Command { rows, .. } | OpenRequest::LocalShell { rows, .. } => *rows,
        }
    }

    /// 一条面向脚本作者的短描述（错误信息里用来说明「想开什么」）。
    pub fn describe(&self) -> String {
        match self {
            OpenRequest::Profile { name } => format!("会话库里的 {name:?}"),
            OpenRequest::Config(config) => format!("配置 {config:?} 的会话"),
            OpenRequest::Command { command, .. } => format!("命令 {command:?}"),
            OpenRequest::LocalShell { .. } => "本地 shell".to_string(),
        }
    }
}

/// 会话库的**只读 + 保存**面（ADR-0027 S1）。
///
/// 为什么是 trait 而不是直接读 `term_profiles`：脚本能看见哪些会话属于应用策略
/// （界面读窗口里的库，CLI 读磁盘上那份），`term-script` 不该认识 SQLite。
/// 读写两个方向放在同一个 trait 里，是因为 `profile_set` 与 `profile_get`
/// 必须是同一份账本——拆两个 trait 就会出现「读到一个库、写到另一个库」。
pub trait ProfileCatalog: Send + Sync {
    /// 全部会话（顺序即库里的顺序）。读不到时给一句能展示的原因。
    fn sessions(&self) -> Result<Vec<SessionConfig>, CatalogError>;

    /// 整表保存（调用方负责合并后再交进来）。
    fn save(&self, sessions: &[SessionConfig]) -> Result<(), CatalogError>;
}

/// 会话库读写失败的原因。
///
/// 为什么不是 `String`（`scripts/verify.sh` 4i 也盯着这条）：`term-core` 是
/// 语义层，跨 crate 的错误要能让调用方**按类别反应**——「读不到库」可以退化成
/// 空表继续跑，「写不进库」必须让用户知道他的改动没落下。拍平成一串文本之后，
/// 这两种只能显示同一句话。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CatalogError(pub String);

impl std::fmt::Display for CatalogError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for CatalogError {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_new_ssh_session_uses_the_builtin_client() {
        assert!(SessionConfig::new(Protocol::Ssh).builtin_ssh);
    }

    #[test]
    fn a_new_non_ssh_session_has_no_ssh_backend() {
        // 这个标志对 Shell / Telnet / Serial / TCP 没有意义，保持 false：
        // 免得将来有人拿它判断「是不是 SSH 会话」而把本地 shell 也判成远程。
        for p in [Protocol::Shell, Protocol::Telnet, Protocol::Serial, Protocol::Tcp] {
            assert!(!SessionConfig::new(p).builtin_ssh, "{p:?}");
        }
    }

    #[test]
    fn a_new_session_carries_no_host_identity() {
        // P1-5：`new()` 曾经给 Shell 读 `$USER`，于是同一份代码在两台机器上
        // 产出的默认会话不同。现在用户名一律空串——与跑测机器的环境无关。
        for p in Protocol::ALL {
            assert_eq!(SessionConfig::new(p).username, "", "{p:?}");
        }
    }

    #[test]
    fn the_local_shell_login_comes_from_the_parameter() {
        // 注入真的生效：换入参，产物就跟着换。
        assert_eq!(SessionConfig::new_local_shell("alice").username, "alice");
        assert_eq!(SessionConfig::new_local_shell("bob").username, "bob");
        assert_ne!(
            SessionConfig::new_local_shell("alice").username,
            SessionConfig::new_local_shell("bob").username
        );
        // 纯函数：同一个注入值 → 逐字段相同（没有环境、没有墙钟参与）。
        let a = SessionConfig::new_local_shell("alice");
        let b = SessionConfig::new_local_shell("alice");
        assert_eq!(a.username, b.username);
        assert_eq!(a.name, b.name);
        assert_eq!(a.protocol, b.protocol);
        assert_eq!(a.port, b.port);
        assert_eq!(a.shell_command, b.shell_command);
    }

    #[test]
    fn the_local_shell_plan_does_not_take_the_login() {
        // 本地 Shell 的计划是 `LocalShell`，没有 argv 可以塞登录名——
        // 注入值只落在配置（表单/属性页显示）上，不会改变启动计划。
        let plan = SessionConfig::new_local_shell("alice")
            .spawn_plan()
            .expect("本地 shell 总能起");
        assert!(matches!(plan, SpawnPlan::LocalShell));
    }

    #[test]
    fn a_different_login_produces_a_different_spawn_argv() {
        // 用户名真正落到 spawn 产物上的是 SSH 这条（`-l <user>`）。
        // 两个登录名必须给出两条不同的 argv：参数 → 产物的传导是实在的。
        let with = |user: &str| {
            let mut s = SessionConfig::new(Protocol::Ssh);
            s.host = "example.com".to_string();
            s.username = user.to_string();
            match s.spawn_plan().expect("填了主机就能出计划") {
                SpawnPlan::External(argv) => argv,
                SpawnPlan::LocalShell => panic!("SSH 不该出本地 shell 计划"),
            }
        };
        let alice = with("alice");
        let bob = with("bob");
        assert!(alice.windows(2).any(|w| w[0] == "-l" && w[1] == "alice"), "{alice:?}");
        assert!(bob.windows(2).any(|w| w[0] == "-l" && w[1] == "bob"), "{bob:?}");
        assert_ne!(alice, bob);
    }

    #[test]
    fn a_session_saved_before_the_field_existed_keeps_the_system_client() {
        // 老 user.sessions 里没有 builtin_ssh 这一项。读出来必须是 false——
        // 「升级后偷偷换了 SSH 实现」是这一刀最不该发生的事，用 serde(default) 守住。
        let old = r#"{
            "name": "Old", "protocol": "Ssh", "host": "example.com", "port": 22,
            "username": "root", "group": "Servers", "auth": "Password",
            "term": "xterm-256color", "charset": "UTF-8",
            "baud_rate": 115200, "data_bits": 8, "stop_bits": 1,
            "parity": "None", "flow_control": "None", "description": "",
            "keep_alive": 0, "connection_timeout": 0,
            "tcp_keep_alive": true, "tcp_low_delay": true, "scrollback": 1000
        }"#;
        let parsed: SessionConfig = serde_json::from_str(old).expect("旧会话应能读出来");
        assert_eq!(parsed.host, "example.com");
        assert!(!parsed.builtin_ssh, "旧会话必须继续用系统 ssh");
    }

    #[test]
    fn the_flag_survives_a_round_trip() {
        let mut s = SessionConfig::new(Protocol::Ssh);
        s.builtin_ssh = false;
        let text = serde_json::to_string(&s).expect("序列化");
        let back: SessionConfig = serde_json::from_str(&text).expect("反序列化");
        assert!(!back.builtin_ssh);
    }

    #[test]
    fn a_session_without_forwards_does_not_write_the_field() {
        // 绝大多数会话没有转发：落盘时不该多出一行 `"forwards": []`，
        // 否则每次保存都会把「什么都没有」写成显式内容。
        let text = serde_json::to_string(&SessionConfig::new(Protocol::Ssh)).expect("序列化");
        assert!(!text.contains("forwards"), "{text}");
        let back: SessionConfig = serde_json::from_str(&text).expect("反序列化");
        assert!(back.forwards.is_empty());
    }

    #[test]
    fn forwards_survive_a_round_trip_as_the_lines_the_user_typed() {
        let mut s = SessionConfig::new(Protocol::Ssh);
        s.forwards = vec![
            "L 8080:db.internal:5432".to_string(),
            "D 1080".to_string(),
        ];
        let text = serde_json::to_string(&s).expect("序列化");
        let back: SessionConfig = serde_json::from_str(&text).expect("反序列化");
        assert_eq!(
            back.forwards,
            vec!["L 8080:db.internal:5432".to_string(), "D 1080".to_string()]
        );
    }

    #[test]
    fn proxy_jump_identity_and_agent_forward_reach_the_system_ssh_argv() {
        let mut s = SessionConfig::new(Protocol::Ssh);
        s.host = "internal.example".to_string();
        s.proxy_jump = "user@bastion:2222".to_string();
        s.identity_file = "~/.ssh/id_ed25519".to_string();
        s.agent_forward = true;
        let Ok(SpawnPlan::External(argv)) = s.spawn_plan() else { panic!("应当生成外部命令") };
        let joined = argv.join(" ");
        assert!(joined.contains("-J user@bastion:2222"), "{joined}");
        assert!(joined.contains("-i ~/.ssh/id_ed25519"), "{joined}");
        assert!(joined.contains("-A"), "{joined}");
        assert!(joined.contains("internal.example"), "{joined}");
    }

    #[test]
    fn x11_forward_reaches_the_system_ssh_argv_as_minus_x() {
        let mut s = SessionConfig::new(Protocol::Ssh);
        s.host = "internal.example".to_string();
        s.x11_forward = true;
        let Ok(SpawnPlan::External(argv)) = s.spawn_plan() else { panic!("应当生成外部命令") };
        let joined = argv.join(" ");
        assert!(joined.contains("-X"), "开了 X11 转发应出现 -X：{joined}");
        // 没开就不该有。
        let mut off = SessionConfig::new(Protocol::Ssh);
        off.host = "internal.example".to_string();
        let Ok(SpawnPlan::External(argv_off)) = off.spawn_plan() else { panic!("应当生成外部命令") };
        assert!(!argv_off.join(" ").contains("-X"), "没开 X11 转发不应出现 -X");
    }

    #[test]
    fn the_generated_argv_only_uses_options_openssh_actually_knows() {
        // 这条是 Docker 里真 ssh 打出来的：`-o TCP_NODELAY=yes` 会让 ssh 直接
        // 以 "Bad configuration option" 退出，整条会话起不来。所以这里钉住
        // 「不要发明选项」——新增选项时先确认 `ssh -o` 认它。
        let mut s = SessionConfig::new(Protocol::Ssh);
        s.host = "example.com".to_string();
        let Ok(SpawnPlan::External(argv)) = s.spawn_plan() else {
            panic!("应当生成外部命令")
        };
        let joined = argv.join(" ");
        assert!(!joined.contains("TCP_NODELAY"), "OpenSSH 不认识这个选项：{joined}");
        // 认得的那些还在。
        assert!(joined.contains("TCPKeepAlive="), "{joined}");
        assert!(joined.contains("PreferredAuthentications="), "{joined}");
    }

    #[test]
    fn a_plain_session_does_not_carry_proxy_flags() {
        let mut s = SessionConfig::new(Protocol::Ssh);
        s.host = "example.com".to_string();
        let Ok(SpawnPlan::External(argv)) = s.spawn_plan() else { panic!() };
        let joined = argv.join(" ");
        assert!(!joined.contains("-J"));
        assert!(!joined.contains("-A"));
        assert!(!joined.contains("-i"));
    }

    #[test]
    fn keyboard_interactive_selects_that_ssh_auth_method() {
        let mut s = SessionConfig::new(Protocol::Ssh);
        s.host = "example.com".to_string();
        s.auth = AuthMethod::KeyboardInteractive;
        let Ok(SpawnPlan::External(argv)) = s.spawn_plan() else { panic!() };
        assert!(argv.join(" ").contains("keyboard-interactive"));
    }

    #[test]
    fn auth_chain_defaults_to_empty_and_prepends_the_primary() {
        // D2：默认没有回退链 → 链就是 [auth] 一项。
        let mut s = SessionConfig::new(Protocol::Ssh);
        assert!(s.auth_chain.is_empty());
        assert_eq!(s.auth_chain_with_primary(), vec![AuthMethod::Password]);

        // 首选 PublicKey + 回退 [Agent, PublicKey, Password]：首选在前、回退
        // 依次在后、且首选重复出现时要去重。
        s.auth = AuthMethod::PublicKey;
        s.auth_chain = vec![AuthMethod::Agent, AuthMethod::PublicKey, AuthMethod::Password];
        assert_eq!(
            s.auth_chain_with_primary(),
            vec![AuthMethod::PublicKey, AuthMethod::Agent, AuthMethod::Password],
            "首选在前、回退依次在后、去重"
        );
    }

    #[test]
    fn auth_chain_round_trips_through_json() {
        let mut s = SessionConfig::new(Protocol::Ssh);
        s.auth = AuthMethod::PublicKey;
        s.auth_chain = vec![AuthMethod::Agent];
        let json = serde_json::to_string(&s).unwrap();
        let back: SessionConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(back.auth, AuthMethod::PublicKey);
        assert_eq!(back.auth_chain, vec![AuthMethod::Agent]);
        assert_eq!(
            back.auth_chain_with_primary(),
            vec![AuthMethod::PublicKey, AuthMethod::Agent]
        );
    }

    #[test]
    fn new_session_fields_are_serde_optional_for_old_stores() {
        // 老会话库没有这些字段；读出来必须是空值而不是整份失败。
        let raw = r#"{"name":"old","protocol":"Ssh","host":"h","port":22,"username":"u","group":"Servers","auth":"Password","term":"xterm","charset":"UTF-8","baud_rate":9600,"data_bits":8,"stop_bits":1,"parity":"None","flow_control":"None","description":"","keep_alive":0,"connection_timeout":20,"tcp_keep_alive":false,"tcp_low_delay":true,"builtin_ssh":false,"scrollback":999999}"#;
        let back: SessionConfig = serde_json::from_str(raw).expect("老配置仍要能读");
        assert!(back.auth_chain.is_empty(), "老配置缺 auth_chain 键 → 空链");
        assert!(back.proxy_jump.is_empty());
        assert!(back.identity_file.is_empty());
        assert!(!back.agent_forward);
        assert_eq!(back.effective_proxy_type(), ProxyType::None);
        assert_eq!(back.proxy_host, "");
        assert_eq!(back.proxy_port, 0);
    }

    #[test]
    fn proxy_tags_round_trip_and_an_unknown_tag_falls_back_to_none() {
        for kind in ProxyType::ALL {
            assert_eq!(ProxyType::from_tag(kind.tag()), kind);
        }
        assert_eq!(ProxyType::from_tag(""), ProxyType::None);
        assert_eq!(ProxyType::from_tag(" SOCKS5 "), ProxyType::Socks5);
        assert_eq!(ProxyType::from_tag("telnet"), ProxyType::None);
        assert!(ProxyType::Http.is_outbound());
        assert!(ProxyType::Socks5.is_outbound());
        assert!(!ProxyType::Jump.is_outbound());
    }

    #[test]
    fn an_old_store_with_a_jump_chain_still_counts_as_jump() {
        // 缺 `proxy_type` 键，但有链：升级后绝不能悄悄变成直连。
        let mut s = SessionConfig::new(Protocol::Ssh);
        s.proxy_type = String::new();
        s.proxy_jump = "deploy@bastion".to_string();
        assert_eq!(s.effective_proxy_type(), ProxyType::Jump);
        // 显式 `none` 优先级更高：用户当年确实选了直连，只是链还留在库里。
        s.proxy_type = ProxyType::None.tag().to_string();
        assert_eq!(s.effective_proxy_type(), ProxyType::None);
    }

    #[test]
    fn choosing_an_outbound_proxy_without_its_address_is_refused() {
        let mut s = SessionConfig::new(Protocol::Ssh);
        s.proxy_type = ProxyType::Http.tag().to_string();
        assert!(s.validate_proxy().unwrap().contains("代理主机"));
        s.proxy_host = "proxy.internal".to_string();
        assert!(s.validate_proxy().unwrap().contains("端口"));
        s.proxy_port = 8080;
        assert!(s.validate_proxy().is_none());
        s.proxy_host = "bad host".to_string();
        assert!(s.validate_proxy().unwrap().contains("空格"));
        // Jump 档校验的是链，不是出网代理地址。
        s.proxy_type = ProxyType::Jump.tag().to_string();
        assert!(s.validate_proxy().is_none());
    }

    #[test]
    fn spawn_plan_uses_the_documented_nc_equivalent_for_outbound_proxies() {
        let mut s = SessionConfig::new(Protocol::Ssh);
        s.host = "internal.example".to_string();
        s.proxy_jump = "bastion".to_string();
        s.proxy_type = ProxyType::Socks5.tag().to_string();
        s.proxy_host = "127.0.0.1".to_string();
        s.proxy_port = 7777;
        let Ok(SpawnPlan::External(argv)) = s.spawn_plan() else { panic!() };
        let joined = argv.join(" ");
        assert!(
            joined.contains("ProxyCommand=nc -X 5 -x 127.0.0.1:7777 %h %p"),
            "{joined}"
        );
        // 四档互斥：选了 SOCKS5 就不能还带 -J。
        assert!(!joined.contains("-J"), "{joined}");

        s.proxy_type = ProxyType::Http.tag().to_string();
        let Ok(SpawnPlan::External(argv)) = s.spawn_plan() else { panic!() };
        assert!(
            argv.join(" ").contains("ProxyCommand=nc -X connect -x 127.0.0.1:7777 %h %p"),
            "{}",
            argv.join(" ")
        );
    }

    #[test]
    fn tooltip_rows_report_the_proxy_that_will_actually_be_used() {
        let mut s = SessionConfig::new(Protocol::Ssh);
        s.proxy_type = ProxyType::Http.tag().to_string();
        s.proxy_host = "gw.example".to_string();
        s.proxy_port = 3128;
        let rows: Vec<_> = s.tooltip_rows().into_iter().collect();
        let get = |key: &str| rows.iter().find(|(k, _)| *k == key).unwrap().1.clone();
        assert_eq!(get("Proxy Type"), "Http proxy");
        assert_eq!(get("Proxy Host"), "gw.example");
        assert_eq!(get("Proxy Port"), "3128");
    }
}

#[cfg(test)]
mod serial_plan_tests {
    use super::*;

    #[test]
    fn serial_uses_the_device_path_the_user_typed() {
        let mut s = SessionConfig::new(Protocol::Serial);
        s.host = "/dev/ttyUSB0".to_string();
        s.baud_rate = 115200;
        let Ok(SpawnPlan::External(argv)) = s.spawn_plan() else {
            panic!("serial 必须走外部客户端")
        };
        assert_eq!(argv, ["cu", "-l", "/dev/ttyUSB0", "-s", "115200"]);
        // 不得再出现写死的设备路径——那会让「填了什么」与「连到哪」脱钩。
        assert!(!argv.iter().any(|a| a == "/dev/tty.usbserial"), "{argv:?}");
    }

    #[test]
    fn serial_device_path_is_trimmed_but_not_rewritten() {
        let mut s = SessionConfig::new(Protocol::Serial);
        // macOS 与 Windows 的写法都原样透传：core 不认识平台命名，不做猜测。
        s.host = "  /dev/cu.usbserial-1420  ".to_string();
        let Ok(SpawnPlan::External(argv)) = s.spawn_plan() else {
            panic!()
        };
        assert_eq!(argv[2], "/dev/cu.usbserial-1420");

        s.host = "COM3".to_string();
        let Ok(SpawnPlan::External(argv)) = s.spawn_plan() else {
            panic!()
        };
        assert_eq!(argv[2], "COM3");
    }

    #[test]
    fn serial_without_a_device_is_a_config_error() {
        let mut s = SessionConfig::new(Protocol::Serial);
        s.host = "   ".to_string();
        // `SpawnPlan` 没有 `Debug`（它对调用方是不透明产物），所以不用 `unwrap_err`。
        match s.spawn_plan() {
            Err(e) => assert_eq!(e, ConfigError::MissingHost(Protocol::Serial)),
            Ok(_) => panic!("空设备路径必须报 MissingHost，而不是拼出一条 cu 命令"),
        }
    }

    #[test]
    fn serial_baud_rate_reaches_the_command_line() {
        let mut s = SessionConfig::new(Protocol::Serial);
        s.host = "/dev/ttyS0".to_string();
        for baud in [9600u32, 57600, 115200, 921600] {
            s.baud_rate = baud;
            let Ok(SpawnPlan::External(argv)) = s.spawn_plan() else {
                panic!()
            };
            assert_eq!(argv[4], baud.to_string(), "波特率必须原样传出");
        }
    }
}

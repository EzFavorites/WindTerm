//! Session definitions: what the user creates in the session tree, and what is
//! needed to actually start one. The protocol list, the SSH authentication
//! switches, the serial defaults and the terminal type all mirror a conventional terminal emulator's
//! program's own `terminal/configs/session.config` and its session classes
//! (`SshSession`, `TelnetSession`, `SerialSession`, `ShellSession`, `TcpSession`).

use serde::{Deserialize, Serialize};
use std::fmt;

// `lib.rs` 不在本改动的领地里，所以不新增顶层 `pub mod reconnect;`：用 `#[path]`
// 把同目录的新文件挂在 `session` 下，并从这里再导出一次。对外两个名字都能用——
// `term_core::session::reconnect::backoff` 与 `term_core::session::backoff`。
#[path = "reconnect.rs"]
pub mod reconnect;
pub use reconnect::{backoff, effective, is_terminal, ReconnectOverride, ReconnectPolicy};

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

/// 会话走哪一种代理。
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
fn default_one() -> usize {
    1
}
fn default_two() -> u32 {
    2
}
fn default_two_usize() -> usize {
    2
}

/// 光标周期 / 宽度的反序列化缺省。老会话库没有这两条时回落出厂值，
/// 行为与「2026-11-19 之前是渲染层常数」时完全一致。
fn default_caret_period() -> f32 {
    crate::config::DEFAULT_CARET_PERIOD
}
fn default_caret_width() -> f32 {
    crate::config::DEFAULT_CARET_WIDTH
}

/// X/Y/Z 三页签各自的上传命令（视觉基线 `serial/03..05-*-modem-tab.png`）。
fn default_modem_commands() -> [String; 3] {
    ["rx".into(), "rb".into(), "rz".into()]
}

/// Shell ▾ Process / Environment 页的金标准出厂值（`04-new-session/shell/02..03`）。
fn default_shell_arguments() -> String {
    "-1 -1".into()
}
fn default_shell_locale() -> String {
    "Automatic (zh_CN.UTF-8)".into()
}
fn default_shell_exec_delay() -> u32 {
    1
}

/// Authentication 页三格「Delay」的出厂值：与兼容终端一样是 1 秒。
fn default_auth_exec_delays() -> [u32; 3] {
    [1, 1, 1]
}

/// Key Exchange 页的两个重换密钥阈值（金标准：60 分钟 / 1000 MB）。
fn default_rekey_time() -> u32 {
    60
}
fn default_rekey_data() -> u32 {
    1000
}

/// 平台页签的顺序：`0 = macOS`、`1 = Linux`、`2 = Windows`。
///
/// 会话文件里三格按这个次序存；运行时只取本机那一格。
pub const AUTH_PLATFORMS: [&str; 3] = ["macOS", "Linux", "Windows"];

/// 本机在 [`AUTH_PLATFORMS`] 里的下标。
pub const fn auth_platform() -> usize {
    if cfg!(target_os = "macos") {
        0
    } else if cfg!(target_os = "windows") {
        2
    } else {
        1
    }
}

/// 三页签各自的包长档位：X=128、Y=1024、Z 没有包长行（该位不使用）。
fn default_modem_large_packets() -> [bool; 3] {
    [false, true, false]
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SessionConfig {
    pub name: String,
    /// 会话树图标的自定义颜色（ARGB/RGB 十六进制低 24 位）。`None` = 按协议上色。
    #[serde(default)]
    pub color: Option<u32>,
    pub protocol: Protocol,
    pub host: String,
    pub port: u16,
    pub username: String,
    /// `session.group` — the folder the session is filed under in the tree.
    pub group: String,
    /// `ssh.authPassword` / `authPublicKey` / `authAgent` / `authGssapiWithMic`
    ///
    /// 这是**首选**方法。回退链见 `auth_chain`。
    pub auth: AuthMethod,
    /// OneKey：认证回退链（有序的**方法种类**，空 = 不回退，旧行为）。
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
    /// 自动登录是 OneKey 方案的内置语义：选中了方案就在连接时按它登录
    /// （覆盖用户名/私钥、从保险库取密码与键盘交互应答、跑 Account 自动应答）。
    /// 不再需要单独的勾选框；出厂 `onekey = None` 表示没有方案，行为不变。
    /// Sftp 子系统开关（WindTerm 默认 ON）。
    #[serde(default = "default_true")]
    pub sftp: bool,
    /// `Session ▾ Sftp ▸ Preserve original file modification time`。
    ///
    /// WindTerm 出厂勾选；`term-io` 的两条传输链路（`term-sftp`）把 true 解释为
    /// 「传完把源端 mtime 写回对端」。老会话库缺这个键时露出的勾也是 true——
    /// **与页面上原本就画成勾的样子一致**，不是升级后偷偷改变外观。
    #[serde(default = "default_true")]
    pub sftp_preserve_mtime: bool,
    /// `Session ▾ Sftp ▸ Preserve original folder modification time`。
    ///
    /// 出厂 false（参考页这一格是空勾）：目录 `setstat` 并非所有服务端都支持，
    /// 文件 mtime 已经是默认保真；目录这一档由用户显式打开。
    #[serde(default)]
    pub sftp_preserve_dir_mtime: bool,
    /// `Session ▾ Sftp ▸ Automatically fix invalid path characters`。
    ///
    /// WindTerm 出厂勾选：目标路径每一段的非法字符（`< > : " / \ | ? *`
    /// 与控制字符）替换成 `_`，源端路径不动。
    #[serde(default = "default_true")]
    pub sftp_fix_invalid_path: bool,
    /// `Terminal ▾ Security`：远端读剪贴板的策略（0 = Ask / 1 = Allow / 2 = Forbid）。
    #[serde(default)]
    pub clipboard_read_policy: usize,
    /// `Terminal ▾ Security`：远端写剪贴板的策略（同上）。
    #[serde(default)]
    pub clipboard_write_policy: usize,
    /// `Modem` 页：拖放文件时用的协议（0 X / 1 Y / 2 Z，出厂 Z）。
    #[serde(default = "default_two_usize")]
    pub modem_protocol: usize,
    #[serde(default)]
    pub modem_upload_dir: String,
    #[serde(default)]
    pub modem_download_dir: String,
    /// X/Y/ZMODEM 接收侧同名冲突策略的下标（0..6，清单见
    /// `apps/desktop/src/xfer_conflict.rs::POLICIES`，0 = Overwrite）。
    /// `#[serde(default)]`：老会话库缺键时保持升级前的唯一行为——覆盖。
    #[serde(default)]
    pub modem_conflict_policy: usize,
    /// X/Y/ZMODEM 接收完成后是否用系统默认程序打开下载目录。出厂 false；
    /// `#[serde(default)]` 让老会话库缺键时也不突然弹文件夹窗口。
    #[serde(default)]
    pub modem_open_after: bool,
    /// 拖放时不再询问（当前仍会弹确认框以选择具体文件，目录取这里的值）。
    #[serde(default)]
    pub modem_use_default: bool,
    /// `Serial ▸ X/Y/Z Modem` 三页签的上传命令。下标与 `modem_protocol`
    /// 同口径（0 X / 1 Y / 2 Z）；老会话库缺键时回落到出厂 rx/rb/rz。
    #[serde(default = "default_modem_commands")]
    pub modem_commands: [String; 3],
    /// 三页签的包长档位（false=128 / true=1024）；Z 页不显示这一行。
    #[serde(default = "default_modem_large_packets")]
    pub modem_large_packets: [bool; 3],
    /// `Session ▾ Proxy ▸ Perform DNS lookup with proxy`。出厂 true：目标名
    /// 交给代理解析；关掉时先本地解析再连代理。
    #[serde(default = "default_true")]
    pub dns_over_proxy: bool,
    /// `Terminal ▾ Mode ▸ Auto Wrap Mode (DECAWM)` 的初始值。
    ///
    /// 出场 `true` 与内核 `Terminal::new` 的出厂态一致；远端随后发来的
    /// `DECSET/DECRST 7` 仍会覆盖它（初始模式只是开机画面）。
    #[serde(default = "default_true")]
    pub initial_auto_wrap: bool,
    /// `Terminal ▾ Mode ▸ Cursor Keys mode (DECCKM)` 的初始值。
    /// `true` = 方向键发 `ESC O A`（应用模式），出厂 `false` = `ESC [ A`。
    #[serde(default)]
    pub initial_cursor_keys_application: bool,
    /// `Terminal ▾ Keyboard ▸ Backspace sends:` 的下标（0 = Automatic / 1 = BS /
    /// 2 = Delete 序列）。编码在 `crate::keys::backspace_bytes`。
    #[serde(default)]
    pub backspace_sends: usize,
    /// `Terminal ▾ Keyboard ▸ Delete sends:` 的下标（0 = Automatic / 1 = BS /
    /// 2 = DEL）。编码在 `crate::keys::delete_bytes`。
    #[serde(default)]
    pub delete_sends: usize,
    /// `Keyboard ▸ Alt modifier key sends:`：0 = 8-bit characters（出厂）/ 1 = ESC 前缀。
    #[serde(default)]
    pub alt_modifier_sends: usize,
    /// `Keyboard ▸ Meta modifier key sends:`：0 = 8-bit characters / 1 = ESC 前缀（出厂）。
    #[serde(default = "default_one")]
    pub meta_modifier_sends: usize,
    /// `Terminal ▾ Bell ▸ Bell styles`：0 = Mute / 1 = Beep / 2 = 自定义声音。
    #[serde(default = "default_one")]
    pub bell_style: usize,
    /// `Bell ▸ Play the custom sound` 的文件路径。空 = 没选文件（不发声音）。
    #[serde(default)]
    pub bell_sound: String,
    /// `Bell ▸ Ignore bells received more than once within`：秒；0 = 不去重。
    #[serde(default = "default_two")]
    pub bell_dedup_secs: u32,
    /// 系统监控开关（WindTerm 默认 OFF）。
    #[serde(default)]
    pub sysmon: bool,
    /// 本地 Shell 的启动命令（仅 Shell 协议；其余协议为空）。
    #[serde(default)]
    pub shell_command: String,
    /// Shell ▾ Process：Arguments / Working directory 之外的启动参数。
    #[serde(default = "default_shell_arguments")]
    pub shell_arguments: String,
    /// Shell ▾ Process：启动后执行的命令与延迟秒数。
    #[serde(default)]
    pub shell_exec_command: String,
    #[serde(default = "default_shell_exec_delay")]
    pub shell_exec_delay: u32,
    /// Shell ▾ Environment：locale 档位与自定义环境变量。
    #[serde(default = "default_shell_locale")]
    pub shell_locale: String,
    #[serde(default)]
    pub shell_env_vars: Vec<(String, String)>,
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
    /// 走进程内的 russh 客户端，而不是拉系统 `ssh`。
    ///
    /// `#[serde(default)]` 是**迁移策略**，不是偷懒：已经存在的 `user.sessions`
    /// 里没有这个字段，读出来是 `false`（继续用系统 `ssh`）；`SessionConfig::new`
    /// 给新会话写 `true`。于是「老会话不变、新会话默认原生」不需要写任何迁移代码。
    #[serde(default)]
    pub builtin_ssh: bool,
    /// 这条会话在连接时一并要起的 SSH 转发（每人一行，`L` / `R` / `D` 开头）。
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
    /// `ssh_config` 的 `ProxyJump` 链（系统 `ssh -J` 直接认这个形状）。
    /// 空串 = 没有跳板。`#[serde(default)]` 让老会话库读出来是空串。
    #[serde(default)]
    pub proxy_jump: String,
    /// 导入器带过来的 `IdentityFile` 路径，只记路径不记内容。
    ///
    /// 新表单把它当**当前平台**那一格 [`Self::auth_identity`] 的镜像写盘：
    /// 系统 `ssh -i` 与内置客户端都只认这一个字段。
    #[serde(default)]
    pub identity_file: String,
    /// 自动认证的身份文件，按 macOS / Linux / Windows 分三格。
    ///
    /// 兼容终端的 Authentication 页把这一项放在三个平台页签里（同一份会话
    /// 可能同步到不同系统）。运行时只读**本机**那一格，见 [`auth_platform`]。
    #[serde(default)]
    pub auth_identity: [String; 3],
    /// 认证成功后要执行的命令，同样按平台分三格（空串 = 不执行）。
    #[serde(default)]
    pub auth_exec_command: [String; 3],
    /// 上面那条命令的延迟秒数，按平台分三格（默认 1s，与兼容终端一致）。
    #[serde(default = "default_auth_exec_delays")]
    pub auth_exec_delay: [u32; 3],
    /// 连接时校验服务器指纹（默认开）。关掉等于 `HostKeyPolicy::Insecure`：
    /// 未知主机会被静默接受——这是用户明确关掉检查才有的行为。
    #[serde(default = "default_true")]
    pub verify_host_key: bool,
    /// Key Exchange 页 `Max time before rekey (Minutes)`，默认 60。
    #[serde(default = "default_rekey_time")]
    pub rekey_time_minutes: u32,
    /// Key Exchange 页 `Max data before rekey (MBytes)`，默认 1000。
    #[serde(default = "default_rekey_data")]
    pub rekey_data_mbytes: u32,
    /// 允许把本地 ssh-agent 转发到远端（系统 `ssh -A`）。
    #[serde(default)]
    pub agent_forward: bool,
    /// X11 转发（系统 `ssh -X`）。`#[serde(default)]`：老会话库读出来是 false，
    /// `SessionConfig::new` 给新会话写 false（默认不开——X11 需要本机有 X server）。
    #[serde(default)]
    pub x11_forward: bool,
    /// X11 screen number，随 `x11-req` 下发给服务端（`-X` 的 `:<host>.<screen>`）。
    #[serde(default)]
    pub x11_screen: u32,
    /// 出网代理档位（tag 见 [`ProxyType::tag`]）。空串 = 老配置，
    /// 由 [`SessionConfig::effective_proxy_type`] 按 `proxy_jump` 推断。
    #[serde(default)]
    pub proxy_type: String,
    /// HTTP / SOCKS5 代理主机。选了这两档却留空 = 「填了不生效」，
    /// 表单保存与 `SshOptions::validate` 都会挡下来。
    #[serde(default)]
    pub proxy_host: String,
    /// `Session ▾ Proxy ▸ Proxy command`：用户自填的 `ProxyCommand`。
    ///
    /// **只作用于系统 `ssh` 路径**：内置 SSH 客户端走 `OutboundProxy`
    /// （kind/host/port），不执行 shell 命令。留空 = 用 Host/Port 生成
    /// 官方的 `nc -X …` 等价式，与升级前一致。
    #[serde(default)]
    pub proxy_command: String,
    /// 代理端口。`0` 会被判为非法而不是悄悄取 1080。
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
    /// `window.wordSeparators`：双击选词 / 按词移动用的分隔符集合。
    ///
    /// 空串 = 未设置，读侧回落到出厂串（`term-app` 的
    /// `selection::DEFAULT_SEPARATORS`）。`term-core` 不认识 `term-app`，所以
    /// 这里只存用户写下的值，默认值由读者给——与 `proxy_type` 同一套迁移口径。
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub word_separators: String,
    /// 备用屏（alternate screen）的选区分隔符；空串 = 与主屏同值。
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub word_separators_alt: String,
    /// 括号配对分隔符：每两个字符一对（如 `()[]{}<>`）。空串 = 未设置，
    /// 读者回落到出厂 `()[]{}`。与 `word_separators` 同一套迁移口径。
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub bracket_pairs: String,
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
    /// 自动重连的逐会话覆盖。**平铺**进会话 JSON 的六个可选键，每个都带
    /// `#[serde(default)]`（见 [`ReconnectOverride`]）：老会话库缺键时读出来
    /// 全是 `None` = 沿用全局策略（出厂关闭），不会让老文件反序列化失败。
    #[serde(flatten, default)]
    pub reconnect: ReconnectOverride,
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

/// 全局默认代理。Preferences ▸ Proxy 存这一份，会话没配代理时用它。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GlobalProxy {
    pub kind: ProxyType,
    pub host: String,
    pub port: u16,
}

impl SessionConfig {
    /// 会话自己没选代理时，回落到全局默认。已选 Jump / Http / Socks5 的一律不动。
    pub fn apply_global_proxy(&mut self, global: Option<&GlobalProxy>) {
        let Some(global) = global else {
            return;
        };
        if self.effective_proxy_type() != ProxyType::None {
            return;
        }
        self.proxy_type = global.kind.tag().to_string();
        self.proxy_host = global.host.trim().to_string();
        self.proxy_port = global.port;
    }

    pub fn new(protocol: Protocol) -> Self {
        Self {
            color: None,
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
            // 产物是什么」，读环境变量等于把宿主状态塞进纯函数。
            // 想要「新本地 Shell 带上当前登录名」的调用方读环境后走
            // [`SessionConfig::new_local_shell`]，值从参数进来。
            username: String::new(),
            auth: AuthMethod::Password,
            // 默认没有回退链（旧行为：只用上面这一个 `auth`）。
            auth_chain: Vec::new(),
            group: crate::config::DEFAULT_GROUP.to_string(),
            term: crate::config::DEFAULT_TERM.to_string(),
            charset: crate::config::CHARSET.to_string(),
            system: "linux".to_string(),
            onekey: "None".to_string(),
            sftp: true,
            sftp_preserve_mtime: true,
            sftp_preserve_dir_mtime: false,
            sftp_fix_invalid_path: true,
            clipboard_read_policy: 0,
            clipboard_write_policy: 0,
            modem_protocol: 2,
            modem_upload_dir: String::new(),
            modem_download_dir: String::new(),
            modem_conflict_policy: 0,
            modem_open_after: false,
            modem_use_default: false,
            modem_commands: default_modem_commands(),
            modem_large_packets: default_modem_large_packets(),
            dns_over_proxy: true,
            initial_auto_wrap: true,
            initial_cursor_keys_application: false,
            backspace_sends: 0,
            delete_sends: 0,
            alt_modifier_sends: 0,
            meta_modifier_sends: 1,
            bell_style: 1,
            bell_sound: String::new(),
            bell_dedup_secs: 2,
            sysmon: false,
            shell_command: String::new(),
            shell_arguments: default_shell_arguments(),
            shell_exec_command: String::new(),
            shell_exec_delay: default_shell_exec_delay(),
            shell_locale: default_shell_locale(),
            shell_env_vars: Vec::new(),
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
            // 新会话默认走原生 SSH。老会话读出来是 false，保持系统 `ssh`。
            builtin_ssh: matches!(protocol, Protocol::Ssh),
            // 不给任何默认转发规则：一条隧道要用户明确写出来才存在。
            forwards: Vec::new(),
            scrollback: crate::config::DEFAULT_SCROLLBACK,
            proxy_jump: String::new(),
            identity_file: String::new(),
            auth_identity: Default::default(),
            auth_exec_command: Default::default(),
            auth_exec_delay: default_auth_exec_delays(),
            verify_host_key: true,
            rekey_time_minutes: default_rekey_time(),
            rekey_data_mbytes: default_rekey_data(),
            agent_forward: false,
            x11_forward: false,
            x11_screen: 0,
            // **空串 = 未设置**，不是显式 "none"：代码里直接写 `proxy_jump`
            // 的路径（导入器、脚本、测试夹具）不该被一个默认值悄悄否掉。
            // 显式 "none" 只由表单保存时写入。
            proxy_type: String::new(),
            proxy_host: String::new(),
            proxy_command: String::new(),
            proxy_port: 0,
            // 初始网格尺寸 0 = 未设置（沿用视口）；滚轮行数/光标样式取规格默认值。
            cols: 0,
            rows: 0,
            wheel_lines: crate::config::MOUSE_WHEEL_SCROLL_LINES,
            caret_style: 0,
            caret_period: crate::config::DEFAULT_CARET_PERIOD,
            caret_width: crate::config::DEFAULT_CARET_WIDTH,
            working_dir: String::new(),
            // 空串 = 未设置，读者回落出厂分隔符；老会话库读出来也是空串。
            word_separators: String::new(),
            word_separators_alt: String::new(),
            bracket_pairs: String::new(),
            logon_script: String::new(),
            // 全协议共用同一套默认（None = 沿用全局）；本地 shell 例外：
            // 它显式写死 enabled=false，即使在全局策略被手改成开的情况下也
            // 不重连——本地进程退出是用户主动结束，不是链路断开。
            reconnect: if protocol == Protocol::Shell {
                ReconnectOverride {
                    enabled: Some(false),
                    ..ReconnectOverride::default()
                }
            } else {
                ReconnectOverride::default()
            },
        }
    }

    /// 新建一条**本地 Shell** 会话，登录名由宿主注入。
    ///
    /// `term-core` 不读 `$USER`：读环境的是允许有副作用的层（`term_io::host::login`），
    /// 这一层只收值。同样两次 `new_local_shell("alice")` 必须逐字段相等——
    /// 这是在会话模型上的判据，也是把「同一输入不同产物」这条缺陷关掉的地方。
    ///
    /// 只对 `Shell` 有意义：其余协议的 `username` 是用户填的远端账号，
    /// 不能拿本机登录名去冒充，所以那条路仍走 [`SessionConfig::new`]。
    pub fn new_local_shell(login: &str) -> Self {
        let mut config = Self::new(Protocol::Shell);
        config.username = login.to_string();
        config
    }

    /// 这条会话现在能不能进自动重连链：**唯一判据**。
    ///
    /// 替掉了 UI 里两处 `protocol != Shell` 的硬编码。本地 shell 永不重连；
    /// 其余协议看有效策略的 `enabled`（全局 + 本会话覆盖），退出原因另行由
    /// [`reconnect::allows_exit`] 判定——两件事分开，免得把「本地 shell」和
    /// 「终止性失败」揉成同一个布尔。
    pub fn allows_reconnect(&self, global: &ReconnectPolicy) -> bool {
        if self.protocol == Protocol::Shell {
            return false;
        }
        reconnect::effective(global, &self.reconnect).enabled
    }

    /// 实际要尝试的**有序**认证方法链 = 首选 `auth` 在前 + 回退链依次在后，
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

    /// 选了**一档真实方案**时，连接层让 OneKey 参与自动登录。
    ///
    /// 这是两条连接路径（界面的 `ui/dialogs/ssh.rs`、无窗口的 `session_open.rs`）
    /// 共用的**唯一判定**，以后改口径只动这里，不会出现「界面自动登了、脚本没登」
    /// 这种只在某一条路径上偶发的分叉。
    ///
    /// 自动登录是 OneKey 的内置语义：**选了方案就自动登录**，不再有独立开关。
    /// `None`（出厂值）与空串都不是方案名，因此返回 `false`——没有方案时
    /// 连接层不会凭空造登录动作。
    pub fn wants_onekey_auto_login(&self) -> bool {
        let scheme = self.onekey.trim();
        !scheme.is_empty() && scheme != default_onekey()
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
            // 这三行以前恒为空 / 只认 Jump，会话树 tooltip 于是
            // 永远说「None」——用户填了代理也看不出来。现在与实际生效的
            // 档位同源（effective_proxy_type 是唯一判定入口）。
            ("Proxy Type", self.effective_proxy_type().label().to_string()),
            ("Proxy Host", if self.effective_proxy_type().is_outbound() { self.proxy_host.clone() } else { String::new() }),
            ("Proxy Port", if self.effective_proxy_type().is_outbound() { self.proxy_port.to_string() } else { String::new() }),
            ("Proxy Command", if self.effective_proxy_type().is_outbound() { self.proxy_command.clone() } else { String::new() }),
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
                // ProxyJump 单跳链、IdentityFile、agent 转发。
                // 出网代理默认走官文给出的 `nc` 等价式
                // （官方 `proxy-http-socks5` 文档：`nc -X 5 -x 127.0.0.1:7777 %h %p`，
                // HTTP 用 `-X connect`）。用户在 Proxy 页填了 Proxy command 时
                // 以它为准（`%h` / `%p` 由 ssh 展开），留空仍用默认等价式。
                // 这一项只作用于系统 ssh：内置客户端走 `OutboundProxy`。
                // 四档互斥：单选说是 Http 就不能还偷偷带上 `-J`。
                match self.effective_proxy_type() {
                    ProxyType::Jump => {
                        // 用户显式填了 Proxy command 时以它为准：Jump 档也能用
                        // 自定义跳板命令（如 `ssh -W %h:%p gw`），不再只认 -J。
                        // 留空仍走 -J，与升级前的行为逐字一致。
                        if self.proxy_command.trim().is_empty() {
                            argv.extend(["-J".to_string(), self.proxy_jump.trim().to_string()]);
                        } else {
                            argv.extend([
                                "-o".to_string(),
                                format!("ProxyCommand={}", self.proxy_command.trim()),
                            ]);
                        }
                    }
                    ProxyType::Http | ProxyType::Socks5 => {
                        let command = if self.proxy_command.trim().is_empty() {
                            let flag = if self.effective_proxy_type() == ProxyType::Http {
                                "connect"
                            } else {
                                "5"
                            };
                            format!(
                                "nc -X {flag} -x {}:{} %h %p",
                                self.proxy_host.trim(),
                                self.proxy_port
                            )
                        } else {
                            self.proxy_command.trim().to_string()
                        };
                        argv.extend(["-o".to_string(), format!("ProxyCommand={command}")]);
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
                // 界面上勾不勾都不影响系统 ssh 的行为——见阶段记录。
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
                // 属于意义上的假入口——点了有效果，但效果与填写内容无关。
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

// ── 打开会话的统一请求（ADR-0027） ──────────────────────────────────
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

/// 会话库的**只读 + 保存**面（ADR-0027）。
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
        // `new()` 曾经给 Shell 读 `$USER`，于是同一份代码在两台机器上
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
        // 同一份老 JSON 也没有 modem 的两项：必须保持 Overwrite / 不打开目录。
        assert_eq!(parsed.modem_conflict_policy, 0);
        assert!(!parsed.modem_open_after);
    }

    #[test]
    fn new_sessions_default_to_overwrite_without_opening_the_folder() {
        for p in Protocol::ALL {
            let cfg = SessionConfig::new(p);
            assert_eq!(cfg.modem_conflict_policy, 0, "{p:?} 出厂是 Overwrite");
            assert!(!cfg.modem_open_after, "{p:?} 出厂不自动打开下载目录");
        }
    }

    #[test]
    fn the_modem_receive_options_survive_a_round_trip() {
        let mut cfg = SessionConfig::new(Protocol::Ssh);
        cfg.modem_conflict_policy = 5;
        cfg.modem_open_after = true;
        let text = serde_json::to_string(&cfg).expect("序列化");
        let back: SessionConfig = serde_json::from_str(&text).expect("反序列化");
        assert_eq!(back.modem_conflict_policy, 5);
        assert!(back.modem_open_after);
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
    fn a_jump_session_with_a_custom_proxy_command_uses_it_instead_of_minus_j() {
        let mut cfg = SessionConfig::new(Protocol::Ssh);
        cfg.host = "internal.example".to_string();
        cfg.proxy_jump = "user@bastion:2222".to_string();
        cfg.proxy_command = "ssh -W %h:%p gw.example".to_string();
        let Ok(SpawnPlan::External(argv)) = cfg.spawn_plan() else {
            panic!("应当生成外部命令")
        };
        let joined = argv.join(" ");
        assert!(joined.contains("ProxyCommand=ssh -W %h:%p gw.example"), "{joined}");
        assert!(!joined.contains(" -J "), "两条不能同时带：{joined}");
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
        // 默认没有回退链 → 链就是 [auth] 一项。
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
        assert_eq!(back.reconnect, ReconnectOverride::default(), "老配置缺重连平铺键 → 全 None");
    }

    #[test]
    fn reconnect_overrides_round_trip_and_shell_is_hard_disabled() {
        let mut s = SessionConfig::new(Protocol::Ssh);
        s.reconnect = ReconnectOverride {
            enabled: Some(true),
            initial_secs: Some(2),
            backoff: Some(3),
            max_delay_secs: Some(60),
            retry_window_secs: Some(300),
            max_attempts: Some(9),
        };
        let json = serde_json::to_string(&s).expect("新配置能写");
        let back: SessionConfig = serde_json::from_str(&json).expect("新配置能读");
        assert_eq!(back.reconnect, s.reconnect);

        let shell = SessionConfig::new(Protocol::Shell);
        assert_eq!(shell.reconnect.enabled, Some(false), "本地 shell 显式不重连");
        assert!(!shell.allows_reconnect(&ReconnectPolicy::from_enabled(true)));
        let ssh = SessionConfig::new(Protocol::Ssh);
        assert!(ssh.allows_reconnect(&ReconnectPolicy::from_enabled(true)));
        assert!(!ssh.allows_reconnect(&ReconnectPolicy::default()));
    }

    #[test]
    fn onekey_auto_login_is_built_in_and_needs_a_real_scheme() {
        // 自动登录内置于方案：选了就登录；没选 / 空串 / 出厂 `None` 都不登录。
        let mut s = SessionConfig::new(Protocol::Ssh);
        assert!(!s.wants_onekey_auto_login(), "出厂没有方案，不自动登录");
        s.onekey = "prod".to_string();
        assert!(s.wants_onekey_auto_login(), "选中方案即自动登录，无需额外开关");
        s.onekey = "  None  ".to_string();
        assert!(!s.wants_onekey_auto_login(), "出厂 `None` 不是方案名");
        s.onekey = String::new();
        assert!(!s.wants_onekey_auto_login(), "空串也不是方案名");
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

        // 用户自填的命令覆盖默认 `nc` 等价式；前后空白被裁掉，`%h`/`%p`
        // 原样留给 ssh 展开。
        s.proxy_command = "  nc -X 5 -x proxy.local:1080 %h %p  ".to_string();
        let Ok(SpawnPlan::External(argv)) = s.spawn_plan() else { panic!() };
        let joined = argv.join(" ");
        assert!(joined.contains("ProxyCommand=nc -X 5 -x proxy.local:1080 %h %p"), "{joined}");
        assert!(!joined.contains("nc -X connect"), "{joined}");
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

    #[test]
    fn a_global_proxy_only_fills_a_session_that_has_none() {
        let global = GlobalProxy {
            kind: ProxyType::Socks5,
            host: " gw.example ".to_string(),
            port: 1080,
        };
        let mut bare = SessionConfig::new(Protocol::Ssh);
        bare.apply_global_proxy(Some(&global));
        assert_eq!(bare.effective_proxy_type(), ProxyType::Socks5);
        assert_eq!(bare.proxy_host, "gw.example");
        assert_eq!(bare.proxy_port, 1080);

        // 会话自己配了 Http：全局默认一个字段都不许改。
        let mut own = bare.clone();
        own.proxy_type = ProxyType::Http.tag().to_string();
        own.proxy_host = "own.example".to_string();
        own.apply_global_proxy(Some(&global));
        assert_eq!(own.effective_proxy_type(), ProxyType::Http);
        assert_eq!(own.proxy_host, "own.example");
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

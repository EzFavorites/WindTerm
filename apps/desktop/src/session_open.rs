//! 控制面（脚本 / 桥接）开一条会话的**唯一入口**（ADR-0019 腿 A / 腿 B）。
//!
//! # 为什么需要这一层
//!
//! 此前 `SessionPool::open` 遇到内置 SSH（russh）只会报
//! [`OpenError::BuiltinChannelNeedsInjection`]，通道得由应用层建好后经
//! `open_with_channel` 注入；界面路径（`ui/dialogs/ssh_connect.rs`）有一整套
//! 连接状态机，脚本与桥接这两条**无窗口**路径却没有。这个文件补的就是
//! 那条缝：认证怎么解析、通道怎么建、失败怎么分类，只在这里写一遍。
//!
//! 控制面本体认了这条会话：本文件把那段逻辑注册成**通道工厂**
//! （`install_channel_factory`），`SessionPool::open` 自己调工厂并走同一套
//! 监管路径。于是脚本 / 桥接只调一个 `open `，界面仍用自己的对话框
//! （它要问密码、问指纹，工厂不该替它做这些决定）。
//! `BuiltinChannelNeedsInjection` 特意留着：客户端忘了装工厂时，报出来的
//! 是「漏装了一步」，而不是一句含糊的「连接失败」。
//!
//! # 三条不能越过的边界
//!
//! 1. **内置 SSH 失败绝不回退到系统 `ssh`。** 那会让「同一个会话名字，两次跑
//!    出两种行为」（ADR-0018 修掉的正是这类分叉）。失败就是失败，且分类要能
//!    让调用方反应：[`OpenFailure::NeedsVault`] 是「去解锁」，[`OpenFailure::Connect`]
//!    是「网络/主机给不出答案」，[`OpenFailure::Unsupported`] 是「这条认证方式
//!    内置客户端做不了，请改会话」。
//! 2. **本层不持有保险库。** 密码与 keyboard-interactive 的答案都在
//!    `term-vault` 里，而 `desk serve` 是无窗口的、根本没解锁过保险库
//!    （`bridge/mod.rs` 的 `serve_on` 只有 `SessionPool`）。所以这两档在这里
//!    只报 [`OpenFailure::NeedsVault`]，由调用方决定是提示解锁还是拒绝。
//! 3. **同步、阻塞调用线程。** `term_ssh::connect` 本身就是同步的（内部线程 +
//!    等一个 `Result`，最多等 `connect_timeout`）。两个消费者都在各自的线程里
//!    （脚本线程 / 桥接的连接线程），没有一帧要救。
//!
//! # 与界面路径的刻意差异
//!
//! 主机密钥策略用 `Tofu` 而不是界面用的 `Strict`：这条路径上没有对话框可以
//! 问「这指纹对吗」，而 `Strict` 会把**每一次**首连都变成一次硬失败。`Tofu`
//! 仍然拒绝「记录过但变了」的钥匙——中间人那一步该停住的地方照样停住。
//! 这正是 `term_ssh` 把库默认值设成 `Tofu` 的理由（见该 crate 的模块文档）。

use std::fmt;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use term_core::session::{AuthMethod, OpenRequest, Protocol, ProxyType, SessionConfig};
use term_core::session_runtime::SessionId;
use term_runtime::channel::TermChannel;
use term_session::{ChannelFactory, ChannelFailure, OpenError, SessionPool};
use term_ssh::{AuthMethod as SshAuth, HostKeyPolicy, OutboundProxy, ProxyKind, SshOptions};

/// 无窗口路径的首连策略。见模块文档「与界面路径的刻意差异」。
const HEADLESS_HOST_KEY_POLICY: HostKeyPolicy = HostKeyPolicy::Tofu;

/// 默认私钥的搜索顺序——照 OpenSSH 自己：同一个目录下三把都在时，先 ed25519
/// 才能和 `ssh` 连到同一把钥匙。
const DEFAULT_IDENTITIES: [&str; 3] = ["id_ed25519", "id_ecdsa", "id_rsa"];

/// 起一条会话的失败方式。每一条都对应**调用方不同的动作**，不是给日志看的。
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum OpenFailure {
    /// 这条会话用密码 / keyboard-interactive 认证：答案在保险库里，而本层
    /// （以及无窗口的调用方）手上没有保险库。
    ///
    /// 带 `session` 是为了让提示能说出**是哪一条**会话——一次脚本跑失败时，
    /// 「需要解锁保险库」而不说是谁，用户只能挨个猜。
    NeedsVault { session: String },
    /// 内置客户端做不了这条会话（GSSAPI / 没有可用私钥 / `ProxyJump` 写错）。
    /// 用户要改的是**会话配置**或关掉「内置 SSH 客户端」。
    Unsupported(String),
    /// 内置客户端连不上（DNS / TCP / 握手 / 主机密钥 / 认证被拒）。
    Connect(String),
    /// 控制面自己拒绝或通道起不来（本地 shell 没有 `$SHELL`、配置缺主机名……）。
    Session(OpenError),
}

impl fmt::Display for OpenFailure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            OpenFailure::NeedsVault { session } => write!(
                f,
                "会话 {session} 用密码认证，需要先解锁保险库；\
                 headless 请改用 Agent/私钥认证的会话"
            ),
            OpenFailure::Unsupported(why) => {
                write!(f, "内置 SSH 客户端打不开这条会话：{why}")
            }
            OpenFailure::Connect(why) => write!(f, "内置 SSH 连接失败：{why}"),
            OpenFailure::Session(err) => write!(f, "{err}"),
        }
    }
}

impl std::error::Error for OpenFailure {}

/// 开一条会话。**脚本与桥接都必须走这里**，不许各自 `match` 协议。
///
/// 此后控制面本体（`SessionPool::open`）就能开内置 SSH：这里只负责
/// 「把工厂装上去」和「把错误分类翻回应用层的说法」，会话本身仍由控制面
/// 唯一登记。非内置 SSH 那条支路与这一层出现之前完全一致。
pub(crate) fn open_session(
    pool: &SessionPool,
    config: &SessionConfig,
    cols: usize,
    rows: usize,
) -> Result<SessionId, OpenFailure> {
    if is_builtin_ssh(config) {
        install_channel_factory(pool);
    }
    // 无窗口路径没有人替控制面灌方案表（界面在构造时灌一次、每次落盘 OneKeys
    // 之后再灌一次）。这里读一次盘：`prefs.json` 才是这份数据的正本，
    // 内存里那一份只是界面编辑用的副本。
    // 「Auto login next time」没勾时把空表交给控制面：`SessionPool::open` 进门就
    // `resolve_onekey`，空表 = 方案不参与自动登录，用户自己登。判定与界面路径
    // 共用 `wants_onekey_auto_login`，不另起一套认证逻辑。
    let saved = term_runtime::prefs::load()
        .map(|p| p.onekeys)
        .unwrap_or_default();
    pool.set_onekeys(onekeys_for_open(config, saved));
    pool.open(config, cols, rows).map_err(open_failure)
}

/// 「Auto login next time」在无窗口连接路径上的落点。
///
/// `SessionPool::open` / `open_with_channel` 进门就 [`SessionPool::resolve_onekey`]，
/// 所以只要不给它方案表，那一次连接就不会应用 OneKey——这正是没勾开关时要的
/// 行为。勾了且有真选方案时原样交出落盘的方案表，让控制面用**已有的**解析能力
/// 覆盖用户名 / 私钥；密码与键盘交互应答仍由调用方按凭据来源补，这里不碰。
pub(crate) fn onekeys_for_open(
    config: &SessionConfig,
    saved: Vec<term_core::onekey::OneKey>,
) -> Vec<term_core::onekey::OneKey> {
    if config.wants_onekey_auto_login() {
        saved
    } else {
        Vec::new()
    }
}

/// [`OpenRequest`] → 真正的那一条会话。**脚本与桥接共用的唯一映射点**。
///
/// 四个变体在这里一次性说清，别处（`script_host` / `cli/script` / 桥接）都不许
/// 再各自 `match`：同一个脚本动词在两条路径上开出不同的会话，是最难查的一类
/// 「只在某一侧偶发」的 bug。
///
/// 返回值带上**建这条会话用的配置**：界面要靠它画标签、靠它重连，而池子只
/// 登记 `(id, name, state)`，事后拿不回原配置。
pub(crate) fn open_request(
    pool: &SessionPool,
    request: &OpenRequest,
) -> Result<(SessionId, SessionConfig), OpenFailure> {
    let (config, cols, rows) = match request {
        // 会话库里的名字：读一次库再按名字取。库读不出来是**配置层**的问题
        // （文件坏了 / 目录不可写），不是「这条会话不存在」，所以走
        // `Unsupported` 而不是 `Session(OpenError::…)`。
        OpenRequest::Profile { name } => {
            let (_, profiles) = term_runtime::persist::load().ok_or_else(|| {
                OpenFailure::Unsupported(format!(
                    "读不到会话库，无法按名字打开 {name:?}（先确认会话库文件在、且能读）"
                ))
            })?;
            let config = profiles
                .into_iter()
                .find(|config| &config.name == name)
                .ok_or_else(|| OpenFailure::Unsupported(format!("会话库里没有 {name:?}")))?;
            (config, OpenRequest::DEFAULT_COLS, OpenRequest::DEFAULT_ROWS)
        }
        // 由调用方直接给全量配置（脚本 `session_open(#{…})`）。名字撞车时由池子
        // 唯一化，与从库里打开同一条规则。
        OpenRequest::Config(config) => (
            (**config).clone(),
            OpenRequest::DEFAULT_COLS,
            OpenRequest::DEFAULT_ROWS,
        ),
        // 一次性命令：形状就是「本地 shell + 这条命令行」。
        OpenRequest::Command {
            command,
            name,
            cols,
            rows,
        } => {
            let mut config = SessionConfig::new_local_shell(&term_io::host::login());
            config.name = name
                .clone()
                .unwrap_or_else(|| DEFAULT_SCRIPT_SESSION_NAME.to_string());
            config.shell_command = command.clone();
            (config, *cols, *rows)
        }
        OpenRequest::LocalShell { cols, rows } => (
            SessionConfig::new_local_shell(&term_io::host::login()),
            *cols,
            *rows,
        ),
    };
    let id = open_session(pool, &config, cols, rows)?;
    // 名字以池子登记的为准（重名会被改写成 `name-2`）：界面标签栏要显示的是
    // 这条会话**实际**叫什么，否则会出现两条同名标签。
    let mut config = config;
    if let Some(actual) = pool.name(id) {
        config.name = actual;
    }
    Ok((id, config))
}

/// 脚本开出来的会话在池子里的默认名（`Command` 不给名字时用）。
pub(crate) const DEFAULT_SCRIPT_SESSION_NAME: &str = "script";

/// 把「怎么建一条内置 SSH 通道」注册给控制面。
///
/// 幂等：`desk script` / `desk serve` / 单测都可能先到，先到的那份生效，
/// 后来者忽略（工厂实现是同一份代码，换掉它没有意义）。
/// 工厂只认 `SessionConfig` 与 `TermChannel`，控制面因此仍然零协议知识。
pub(crate) fn install_channel_factory(pool: &SessionPool) {
    let factory: Arc<ChannelFactory> = Arc::new(|config, cols, rows| {
        let opts = ssh_options_for(config, cols, rows).map_err(channel_failure)?;
        let channel = term_ssh::connect(opts)
            .map_err(|e| ChannelFailure::Connect(e.to_string()))?;
        Ok(Box::new(channel) as Box<dyn TermChannel>)
    });
    let _ = pool.set_channel_factory(factory);
}

/// 会话配置 → SSH 连接参数（含无凭据回退链）。
///
/// **通道工厂与 SFTP 共用它**：同一个会话名，终端走一条参数、SFTP 走另一条，
/// 就会出现「能登录但传不了文件」这种最难解释的分歧。
///
/// **OneKey 不在这里解析**：控制面（`SessionPool::open` / `open_with_channel`）
/// 在把配置递到这两个消费者之前就已经覆盖好了用户名与私钥。这里再读一次
/// `prefs` 就是同一件事两个出处——改了一边不会提醒另一边（之前
/// 正是这样：只有无窗口路径吃方案，界面那条一次都没读）。
///
/// `cols` / `rows` 对 SFTP 没意义（子系统不要终端尺寸），那两处照终端尺寸传
/// 即可——它们只影响 `pty_req`，而子系统分支根本不发 `pty_req`。
pub(crate) fn ssh_options_for(
    config: &SessionConfig,
    cols: usize,
    rows: usize,
) -> Result<SshOptions, OpenFailure> {
    let auth = ssh_auth(config, dirs::home_dir().as_deref())?;
    let mut opts = ssh_options(config, auth.clone(), cols, rows)?;
    // 接上无凭据回退链。空链 = 保持单方法行为不变；非空则把首选排在最前，
    // 由 `term_ssh::authenticate` 按顺序逐个尝试。
    let fallbacks = ssh_fallback_chain(config, dirs::home_dir().as_deref())?;
    if !fallbacks.is_empty() {
        let mut chain = Vec::with_capacity(fallbacks.len() + 1);
        chain.push(auth);
        chain.extend(fallbacks);
        opts.auth_chain = chain;
    }
    Ok(opts)
}

/// 应用层的细分类 → 控制面的分类（工厂往上报时用）。
fn channel_failure(err: OpenFailure) -> ChannelFailure {
    match err {
        OpenFailure::NeedsVault { session } => ChannelFailure::NeedsVault { session },
        OpenFailure::Unsupported(why) => ChannelFailure::Unsupported(why),
        OpenFailure::Connect(why) => ChannelFailure::Connect(why),
        OpenFailure::Session(other) => ChannelFailure::Unsupported(other.to_string()),
    }
}

/// 控制面的分类 → 应用层的细分类（调用方看到时用）。
///
/// 一进一出都用显式 match：新增 `OpenError` / `ChannelFailure` 变体时编译器
/// 会逼这里回来想清楚映射，而不是被一个 `_ =>` 悄悄吞成「连接失败」。
fn open_failure(err: OpenError) -> OpenFailure {
    match err {
        OpenError::Channel(ChannelFailure::NeedsVault { session }) => {
            OpenFailure::NeedsVault { session }
        }
        OpenError::Channel(ChannelFailure::Unsupported(why)) => OpenFailure::Unsupported(why),
        OpenError::Channel(ChannelFailure::Connect(why)) => OpenFailure::Connect(why),
        other => OpenFailure::Session(other),
    }
}

/// 这条会话是不是「进程内 russh 客户端」那条路。
fn is_builtin_ssh(config: &SessionConfig) -> bool {
    config.protocol == Protocol::Ssh && config.builtin_ssh
}

/// 会话配置 → 内置客户端的认证方式。
///
/// `home` 由调用方传进来（而不是在这里读环境）：单测要能控制“有没有默认私钥”，
/// 而这一层没有理由自己去猜家目录。
fn ssh_auth(config: &SessionConfig, home: Option<&Path>) -> Result<SshAuth, OpenFailure> {
    match config.auth {
        // agent 不需要保险库：私钥不出 `$SSH_AUTH_SOCK`，这正是 headless 的正解。
        AuthMethod::Agent => Ok(SshAuth::Agent),
        AuthMethod::PublicKey => {
            let explicit = config.identity_file.trim();
            let path = if explicit.is_empty() {
                default_identity(home).ok_or_else(|| {
                    OpenFailure::Unsupported(
                        "这条会话用公钥认证，但既没指定私钥文件（identity_file），\
                         也没找到默认私钥（~/.ssh/id_ed25519 / id_ecdsa / id_rsa 都不在）。\
                         请在会话属性里指定私钥，或改用 Agent，\
                         或关掉「内置 SSH 客户端」改用系统 ssh"
                            .to_string(),
                    )
                })?
            } else {
                PathBuf::from(explicit)
            };
            // 私钥口令（passphrase）同密码一样属于保险库，本层不持有：
            // 加密私钥会在连接时由 `term_ssh` 报出来，不会静默变成空口令重试。
            Ok(SshAuth::PrivateKey {
                path,
                passphrase: None,
            })
        }
        // 这两档的答案在保险库里：本层只负责说清「缺什么」，不猜一个空密码去
        // 试一次（那会白多一次失败握手，还可能触发服务器侧的失败计数）。
        AuthMethod::Password | AuthMethod::KeyboardInteractive => {
            Err(OpenFailure::NeedsVault {
                session: config.name.clone(),
            })
        }
        // russh 没有 GSSAPI 这一档。给的不是「要不要继续」，是一条出路。
        AuthMethod::GssapiWithMic => Err(OpenFailure::Unsupported(
            "内置 SSH 客户端不支持 GSSAPI。请改用密码 / 公钥 / Agent，\
             或关掉「内置 SSH 客户端」改用系统 ssh"
                .to_string(),
        )),
    }
}

/// 把会话的回退链（`SessionConfig.auth_chain`，不含首选）解析成内置客户端
/// 能执行的方法。
///
/// **只含无凭据方法**：Agent / PublicKey。密码与键盘交互的答案在保险库 / 对话框，
/// headless 控制面拿不到——这里**跳过**它们而不是报错：回退链里勾了密码，不该让
/// 「本来能用的 Agent / PublicKey」整体失败（那等于把 OneKey 变成「多勾一个就全挂」）。
/// GSSAPI 本客户端不支持，同样跳过。用户真要用密码，走的是界面那条 `ssh_ask` 路径。
///
/// 没有显式 `identity_file` 又找不到默认私钥时，这一环也跳过（不是致命错误）：
/// 少一把可选钥匙不该拦住 Agent。
fn ssh_fallback_chain(
    config: &SessionConfig,
    home: Option<&Path>,
) -> Result<Vec<SshAuth>, OpenFailure> {
    let mut out: Vec<SshAuth> = Vec::new();
    for method in &config.auth_chain {
        match method {
            AuthMethod::Agent => {
                // 去重：回退链里重复的 Agent 不该被试两次。
                if !out.contains(&SshAuth::Agent) {
                    out.push(SshAuth::Agent);
                }
            }
            AuthMethod::PublicKey => {
                let explicit = config.identity_file.trim();
                let path = if explicit.is_empty() {
                    match default_identity(home) {
                        Some(p) => p,
                        None => continue,
                    }
                } else {
                    PathBuf::from(explicit)
                };
                let candidate = SshAuth::PrivateKey {
                    path,
                    passphrase: None,
                };
                // 同一把私钥重复出现也只试一次。
                if !out.contains(&candidate) {
                    out.push(candidate);
                }
            }
            // 需要凭据 / 本客户端不支持：headless 这条路上不可用，跳过。
            AuthMethod::Password
            | AuthMethod::KeyboardInteractive
            | AuthMethod::GssapiWithMic => {}
        }
    }
    Ok(out)
}

/// 按 OpenSSH 的顺序找默认身份文件。`None` = 三把都不在（或没有家目录）。
fn default_identity(home: Option<&Path>) -> Option<PathBuf> {
    let home = home?;
    DEFAULT_IDENTITIES
        .iter()
        .map(|name| home.join(".ssh").join(name))
        .find(|path| path.is_file())
}

/// 会话配置 → `SshOptions`。
///
/// 与界面那份（`ui/dialogs/ssh_ask.rs::options_for`）的差别只有主机密钥策略。
/// `ProxyJump` / `agent_forward` / X11 三项两条路径都接了——**不能**让 headless
/// 静默丢掉这些配置（与「无窗口路径不能与 GUI 分叉」同一条纪律）。
/// **不**复用界面那份函数是
/// 有意的——那一份是 `pub(in crate::ui)`，让核心路径去依赖投影层的可见性，
/// 等于把「谁都能改界面」接成「谁都能改无窗口连接行为」。
fn ssh_options(
    config: &SessionConfig,
    auth: SshAuth,
    cols: usize,
    rows: usize,
) -> Result<SshOptions, OpenFailure> {
    let mut opts = SshOptions::new(config.host.trim(), config.username.trim(), auth);
    if config.port != 0 {
        opts.port = config.port;
    }
    if !config.term.is_empty() {
        opts.term = config.term.clone();
    }
    opts.cols = cols as u32;
    opts.rows = rows as u32;
    if config.connection_timeout > 0 {
        opts.connect_timeout = Duration::from_secs(u64::from(config.connection_timeout));
    }
    opts.keepalive =
        (config.keep_alive > 0).then(|| Duration::from_secs(u64::from(config.keep_alive)));
    opts.host_key = if config.verify_host_key {
        HEADLESS_HOST_KEY_POLICY
    } else {
        term_ssh::HostKeyPolicy::Insecure
    };
    opts.rekey_time_minutes = config.rekey_time_minutes;
    opts.rekey_data_mbytes = config.rekey_data_mbytes;
    opts.agent_forward = config.agent_forward;
    // X11 转发与 GUI 那份（`ssh_ask.rs`）读同一组字段。后端是 best-effort：
    // 没有 $DISPLAY 时只 warn 不挂连接，所以 headless 也可以放心转发。
    // 漏读会让「desk script / serve 里配了 X11」变成静默无效——与 GUI 分叉。
    opts.x11_forward = config.x11_forward;
    opts.x11_screen = config.x11_screen;
    // 忽略 `ProxyJump` 会让「配了跳板机的会话」连到一个连不上的主机，
    // 而用户看到的是「连接失败」——那是沉默的撒谎。语法错误在碰网络之前报出来。
    opts.proxy_jump = term_ssh::parse_proxy_jump(&config.proxy_jump)
        .map_err(|e| OpenFailure::Unsupported(format!("这条会话的 ProxyJump 写不对：{e}")))?;
    // 脚本 / 桥接这两条无窗口路径也真的走出网代理。配不全时明确报
    // 「改会话配置」，不偷偷直连（ADR-0018 的行为分叉禁令）。
    if let Some(why) = config.validate_proxy() {
        return Err(OpenFailure::Unsupported(why));
    }
    opts.outbound_proxy = config.outbound_proxy_spec().map(|(kind, host, port)| OutboundProxy {
        // `outbound_proxy_spec` 只返回 Http / Socks5 两档。
        kind: if kind == ProxyType::Http { ProxyKind::Http } else { ProxyKind::Socks5 },
        host: host.to_string(),
        port,
        auth: None,
        dns_over_proxy: config.dns_over_proxy,
    });
    Ok(opts)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 一个「此刻一定没人监听」的本地端口：绑上再放掉。
    ///
    /// 不用伪造域名——那要靠 DNS 不解析，而 DNS 在 CI 里是最不可靠的一环。
    fn closed_port() -> u16 {
        let listener = std::net::TcpListener::bind(("127.0.0.1", 0)).expect("bind");
        listener.local_addr().expect("addr").port()
    }

    fn ssh_config(auth: AuthMethod, port: u16) -> SessionConfig {
        // `SessionConfig::new(Protocol::Ssh)` 已经把 builtin_ssh 置为 true。
        let mut config = SessionConfig::new(Protocol::Ssh);
        config.name = "qa-ssh".to_string();
        config.host = "127.0.0.1".to_string();
        config.port = port;
        config.username = "qa".to_string();
        config.auth = auth;
        config.connection_timeout = 5;
        config
    }

    fn scratch_home(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "desk-session-open-{tag}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        ));
        std::fs::create_dir_all(dir.join(".ssh")).expect("建 .ssh");
        dir
    }

    #[test]
    fn the_auto_login_bit_selects_whether_onekeys_reach_the_connection() {
        // 连接行为的选择分支：没勾 → 空表（控制面的 resolve_onekey 变成空操作）；
        // 勾了且真选了方案 → 原样交出方案表；勾了但没选 → 仍不交。
        let schemes = vec![term_core::onekey::OneKey {
            name: "prod".to_string(),
            username: "ops".to_string(),
            enabled: true,
            ..term_core::onekey::OneKey::default()
        }];
        let mut config = ssh_config(AuthMethod::Agent, closed_port());
        assert!(onekeys_for_open(&config, schemes.clone()).is_empty());

        config.auto_login = true;
        config.onekey = "prod".to_string();
        assert_eq!(onekeys_for_open(&config, schemes.clone()), schemes);

        config.onekey.clear();
        assert!(onekeys_for_open(&config, schemes).is_empty());
    }

    #[test]
    fn an_agent_session_reaches_the_builtin_client_instead_of_the_control_plane() {
        // 本刀的核心断言：内置 SSH 走的是 `term_ssh::connect`，不是
        // `pool.open` 的那条拒绝分支，也不是「静默换成系统 ssh」。
        let pool = SessionPool::new();
        let config = ssh_config(AuthMethod::Agent, closed_port());

        let err = open_session(&pool, &config, 80, 24).unwrap_err();
        assert!(
            !matches!(err, OpenFailure::NeedsVault { .. }),
            "agent 认证不需要保险库：{err:?}"
        );
        assert_ne!(
            err,
            OpenFailure::Session(OpenError::BuiltinChannelNeedsInjection),
            "内置 SSH 必须自己建通道，而不是把「需要注入」这个内伤丢给用户"
        );
        assert!(
            matches!(err, OpenFailure::Connect(_)),
            "没人监听的端口应当是连接失败：{err:?}"
        );
        assert!(pool.list().is_empty(), "失败之后不许留半个会话");
    }

    #[test]
    fn password_and_keyboard_interactive_ask_for_the_vault_first() {
        // 两档都走同一个「答案在保险库里」的口径；一个空密码去试握手是错的：
        // 白多一次失败握手，还可能触发服务器侧计数。
        for auth in [AuthMethod::Password, AuthMethod::KeyboardInteractive] {
            let pool = SessionPool::new();
            let config = ssh_config(auth, closed_port());
            match open_session(&pool, &config, 80, 24) {
                Err(OpenFailure::NeedsVault { session }) => {
                    assert_eq!(session, "qa-ssh", "提示必须说清是哪一条会话");
                }
                other => panic!("{auth:?} 应当报 NeedsVault，实际是 {other:?}"),
            }
            assert!(pool.list().is_empty(), "{auth:?}：拒绝后不许留半个会话");
        }
    }

    #[test]
    fn the_vault_message_names_the_way_out() {
        // 这句话是脚本作者唯一能看到的提示，两段信息缺一不可：
        // 「去哪解锁」与「headless 该改用哪种认证」。
        let text = OpenFailure::NeedsVault {
            session: "prod".to_string(),
        }
        .to_string();
        assert!(text.contains("prod"), "要点名会话：{text}");
        assert!(text.contains("解锁保险库"), "要给出解锁这条路：{text}");
        assert!(text.contains("Agent"), "headless 要有一条替代路：{text}");
    }

    #[test]
    fn a_public_key_session_with_an_identity_file_goes_straight_to_connect() {
        // identity_file 本身就够了——私钥文件是本地资产，不需要保险库。
        let pool = SessionPool::new();
        let mut config = ssh_config(AuthMethod::PublicKey, closed_port());
        config.identity_file = "/nonexistent/qa-key".to_string();

        let err = open_session(&pool, &config, 80, 24).unwrap_err();
        assert!(!matches!(err, OpenFailure::NeedsVault { .. }), "{err:?}");
        assert!(
            matches!(err, OpenFailure::Connect(_)),
            "指定了私钥就该真去连（这里必然连不上）：{err:?}"
        );
    }

    #[test]
    fn a_public_key_session_without_any_key_says_so_before_the_network() {
        let pool = SessionPool::new();
        let mut config = ssh_config(AuthMethod::PublicKey, closed_port());
        config.identity_file = String::new();

        let home = scratch_home("nokey");
        let err = ssh_auth(&config, Some(&home)).unwrap_err();
        match err {
            OpenFailure::Unsupported(why) => {
                assert!(why.contains("默认私钥"), "{why}");
                // 提示里必须给出退路，否则这条会话在 headless 下是死路。
                assert!(why.contains("系统 ssh"), "{why}");
            }
            other => panic!("{other:?}"),
        }
        // 无家目录同理：不能 panic，也不能静默猜一个路径。
        assert!(matches!(
            ssh_auth(&config, None),
            Err(OpenFailure::Unsupported(_))
        ));
        assert!(pool.list().is_empty());
        let _ = std::fs::remove_dir_all(&home);
    }

    #[test]
    fn ed25519_wins_over_rsa_for_the_default_identity() {
        let home = scratch_home("order");
        std::fs::write(home.join(".ssh/id_rsa"), b"x").expect("写 rsa");
        std::fs::write(home.join(".ssh/id_ed25519"), b"x").expect("写 ed25519");
        let mut config = ssh_config(AuthMethod::PublicKey, closed_port());
        config.identity_file = String::new();
        match ssh_auth(&config, Some(&home)) {
            Ok(SshAuth::PrivateKey { path, passphrase }) => {
                assert!(path.ends_with("id_ed25519"), "应当先用 ed25519：{path:?}");
                assert!(passphrase.is_none(), "口令属于保险库，这里不许编一个");
            }
            other => panic!("{other:?}"),
        }
        let _ = std::fs::remove_dir_all(&home);
    }

#[test]
    fn a_fallback_chain_keeps_only_credential_free_methods_deduped_in_order() {
        // 回退链只收 Agent / PublicKey；密码 / 键盘交互 / GSSAPI 在 headless
        // 这条路上跳过（不报错——勾了密码不该让能用的 Agent 一起挂）。
        let home = scratch_home("chain");
        std::fs::write(home.join(".ssh/id_ed25519"), b"x").expect("写 key");
        let mut config = ssh_config(AuthMethod::Agent, closed_port());
        config.identity_file = String::new();
        config.auth_chain = vec![
            AuthMethod::Password,
            AuthMethod::PublicKey,
            AuthMethod::KeyboardInteractive,
            AuthMethod::GssapiWithMic,
            AuthMethod::Agent,
            AuthMethod::PublicKey, // 重复：只保留一次
        ];
        let chain = ssh_fallback_chain(&config, Some(&home)).expect("不该失败");
        assert_eq!(
            chain,
            vec![
                SshAuth::PrivateKey {
                    path: home.join(".ssh/id_ed25519"),
                    passphrase: None,
                },
                SshAuth::Agent,
            ],
            "只留 Agent/PublicKey、按 auth_chain 顺序、去重"
        );
    }

    #[test]
    fn the_options_take_the_config_they_are_given_as_final() {
        // OneKey 的覆盖现在只在控制面发生一次（`SessionPool::open` /
        // `open_with_channel` 进门就 `resolve_onekey`）。这个函数只负责把
        // **递给它的那份配置**翻成连接参数——再回查一次 `prefs` 就是同一件事
        // 两个出处。用一份「已经解析过」的配置钉住这一点：`onekey` 还写着方案名，
        // 但用户名与私钥已经是方案给的那一份，原样进 `SshOptions`。
        let mut config = ssh_config(AuthMethod::Agent, closed_port());
        config.onekey = "prod".to_string();
        config.username = "ops".to_string();
        config.identity_file = "/macos/key".to_string();
        let opts = ssh_options_for(&config, 80, 24).expect("agent 会话不需要凭据");
        assert_eq!(opts.user, "ops", "用户名原样透传");
        assert_eq!(opts.auth, SshAuth::Agent, "认证方式按配置走");
    }

    #[test]
    fn a_fallback_chain_skips_public_key_when_no_key_exists() {
        // 没有显式 key 又找不到默认私钥：PublicKey 这一环跳过，不致命。
        let home = scratch_home("chain-skip");
        let empty_home = scratch_home("chain-empty");
        let mut config2 = ssh_config(AuthMethod::Agent, closed_port());
        config2.identity_file = String::new();
        config2.auth_chain = vec![AuthMethod::PublicKey];
        assert!(
            ssh_fallback_chain(&config2, Some(&empty_home))
                .unwrap()
                .is_empty(),
            "找不到私钥就跳过这一环，不该报错"
        );
        let _ = std::fs::remove_dir_all(&home);
        let _ = std::fs::remove_dir_all(&empty_home);
    }

    #[test]
    fn gssapi_is_refused_with_a_way_out() {
        let mut config = ssh_config(AuthMethod::Password, closed_port());
        config.auth = AuthMethod::GssapiWithMic;
        match ssh_auth(&config, None) {
            Err(OpenFailure::Unsupported(why)) => assert!(why.contains("系统 ssh"), "{why}"),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn a_broken_proxy_jump_is_reported_before_any_socket_is_opened() {
        let pool = SessionPool::new();
        let mut config = ssh_config(AuthMethod::Agent, closed_port());
        config.proxy_jump = "@bastion".to_string(); // 跳到没有用户名
        let err = open_session(&pool, &config, 80, 24).unwrap_err();
        match err {
            OpenFailure::Unsupported(why) => assert!(why.contains("ProxyJump"), "{why}"),
            other => panic!("配置错误不该被说成连接失败：{other:?}"),
        }
        assert!(pool.list().is_empty());
    }

    #[test]
    fn a_good_proxy_jump_is_carried_into_the_options() {
        // 反向断言：配好的跳板不该被**静默丢掉**（丢掉等于直连一个连不上的主机）。
        let config = ssh_config(AuthMethod::Agent, 2222);
        let mut config = config;
        config.proxy_jump = "bastion:2200".to_string();
        let opts = ssh_options(&config, SshAuth::Agent, 120, 40).expect("跳板能解析");
        assert_eq!(opts.proxy_jump.len(), 1);
        assert_eq!(opts.proxy_jump[0].host, "bastion");
        assert_eq!(opts.proxy_jump[0].port, 2200);
        // 尺寸也跟着走：PTY 申请用的是这两个值（不是库默认的 80×24）。
        assert_eq!((opts.cols, opts.rows), (120, 40));
        assert_eq!(opts.host_key, HostKeyPolicy::Tofu, "无窗口路径用 Tofu");
    }

    #[test]
    fn a_local_shell_still_goes_through_the_control_plane() {
        // 非内置协议这条支路必须与这一层出现之前**逐字一样**：直接 `pool.open`。
        let pool = SessionPool::new();
        let mut config = SessionConfig::new(Protocol::Shell);
        config.name = "qa-shell".to_string();
        let id = open_session(&pool, &config, 80, 24).expect("本地 shell 应当能开");
        let rows = pool.list();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].0, id);
        assert_eq!(rows[0].1, "qa-shell");
        pool.close(id);
    }

    #[test]
    fn a_system_ssh_session_is_not_taken_over_by_the_builtin_client() {
        // `builtin_ssh = false` 是用户明确选择「用系统 ssh」：本层不许抢过来。
        // 这里只断言判定函数——真去 spawn 系统 `ssh` 是控制面的活，环境里不一定有。
        let mut config = SessionConfig::new(Protocol::Ssh);
        assert!(is_builtin_ssh(&config));
        config.builtin_ssh = false;
        assert!(!is_builtin_ssh(&config));
        let shell = SessionConfig::new(Protocol::Shell);
        assert!(!is_builtin_ssh(&shell));
    }

    #[test]
    fn headless_path_also_uses_the_outbound_proxy() {
        // `desk script` / `desk serve` 这两条无窗口路径不能与 GUI 分叉：
        // 同一份会话配置，两边都得把第一段 TCP 交给代理。
        let mut config = ssh_config(AuthMethod::Agent, 22);
        config.proxy_type = ProxyType::Socks5.tag().to_string();
        config.proxy_host = "127.0.0.1".to_string();
        config.proxy_port = 1080;
        let opts = ssh_options(&config, SshAuth::Agent, 80, 24).expect("代理能构造");
        let p = opts.outbound_proxy.expect("headless 也必须走出网代理");
        assert_eq!(p.kind, ProxyKind::Socks5);
        assert_eq!(p.host, "127.0.0.1");
        assert_eq!(p.port, 1080);
        assert!(opts.proxy_jump.is_empty());
    }

    #[test]
    fn headless_path_also_honours_x11_forward() {
        // 与 `headless_path_also_uses_the_outbound_proxy` 同一条纪律：无窗口
        // 路径不能与 GUI 分叉。GUI 的 `ssh_ask.rs::options_for` 读了这两个字段，
        // headless 也必须读——否则在 `desk script` / `desk serve` 里配了 X11
        // 转发会静默无效。
        let mut config = ssh_config(AuthMethod::Agent, 22);
        config.x11_forward = true;
        config.x11_screen = 3;
        let opts = ssh_options(&config, SshAuth::Agent, 80, 24).expect("X11 配置能构造");
        assert!(opts.x11_forward, "headless 必须转发 X11");
        assert_eq!(opts.x11_screen, 3, "headless 必须用会话里配的 screen 号");
    }

    #[test]
    fn the_control_plane_itself_opens_builtin_ssh_once_the_factory_is_installed() {
        //  的判据：不经过 `open_session`，直接 `pool.open` 也不再是
        // 「需要注入」，而是真的走工厂（这里连不上一个没人监听的端口，
        // 所以拿到的是带分类的连接失败）。
        let pool = SessionPool::new();
        install_channel_factory(&pool);
        let config = ssh_config(AuthMethod::Agent, closed_port());
        let err = pool.open(&config, 80, 24).unwrap_err();
        assert!(
            matches!(err, OpenError::Channel(ChannelFailure::Connect(_))),
            "{err:?}"
        );
        assert_ne!(err, OpenError::BuiltinChannelNeedsInjection);
        assert!(pool.list().is_empty(), "失败不许留半个会话");
    }

    #[test]
    fn headless_refuses_an_incomplete_proxy_instead_of_connecting_directly() {
        let mut config = ssh_config(AuthMethod::Agent, 22);
        config.proxy_type = ProxyType::Http.tag().to_string();
        config.proxy_port = 3128;
        let err = ssh_options(&config, SshAuth::Agent, 80, 24).unwrap_err();
        assert!(
            matches!(err, OpenFailure::Unsupported(_)),
            "应当是「改会话配置」那一类，实际 {err:?}"
        );
        assert!(err.to_string().contains("代理主机"), "{err}");
    }
}

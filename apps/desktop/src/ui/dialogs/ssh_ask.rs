//! SSH 连接流程里**能单测的那一半**：缺什么凭据、失败之后问什么、参数怎么拼。
//!
//! 与 [`super`] 的分工沿用本仓库一贯的「纯的部分与 IO 分开」：
//!
//! | 文件 | 是什么 | 能不能单测 |
//! |---|---|---|
//! | 本文件 | `ask ` / `next ` / `options_for ` | 纯函数，能 |
//! | `ssh_connect.rs` | 后台线程 + 一步状态机 + 三个对话框 | 靠端到端与肉眼 |
//!
//! 拆成两个文件是 4j 的规模要求（`ui/**.rs` ≤ 450 行），不是分层——子模块挂在
//! `ssh_connect` 下，`Connector` 直接用 `super::` 取用，路径也没变：
//! `ssh_connect.rs` 把这里的东西原样再导出一遍，所以 `crate::ui::dialogs::
//! ::ssh_connect::options_for` 这类既有引用一处都不用改。
//!
//! 为什么这一半值钱：`next ` 决定「钥匙变了」是给按钮还是给一句错误。那种判断
//! 一旦退化成「在对话框里现场 match」，就再也没法用断言钉住它了。

use crate::ui::*;
use std::path::{Path, PathBuf};
use term_core::session::{AuthMethod, ProxyType};
use term_ssh::{AuthMethod as SshAuth, HostKeyPolicy, OutboundProxy, ProxyKind, SshError, SshOptions};

/// 首连用 `Strict`：图形界面应该**先问再信**。
///
/// 与 `term_ssh` 的库默认值（`Tofu`）不同，这是刻意的：库的默认要对脚本友好
/// （CLI 里没人回答对话框），而这里有对话框可以问。用户点了「信任」之后
/// 才用 `Tofu` 重连一次，把钥匙记进 `known_hosts`——于是下一次静默通过。
pub(in crate::ui) const FIRST_POLICY: HostKeyPolicy = HostKeyPolicy::Strict;
/// 这个会话现在缺什么。
///
/// `Connect` 只携带**首选**凭据。回退链不进这个枚举——它由 `options_for`
/// 从 `config.auth_chain` 现场解析后塞进 `SshOptions::auth_chain`，于是所有
/// 调用方（界面 / headless）都自动获得链，无需改判 `Ask` 的消费点。
#[derive(Clone, Debug, PartialEq, Eq)]
pub(in crate::ui) enum Ask {
    /// 凭据齐了，可以连（仅首选；回退链见 `options_for`）。
    Connect(SshAuth),
    /// 需要用户输入密码。
    Password,
    /// 原生客户端做不了，并说明为什么。
    Unsupported(String),
}

/// 「换掉记着的钥匙」需要的一切。
///
/// 为什么把这一坨单独成结构而不是塞进 `Step` 的字段里：换钥要用到**服务器这次
/// 给的公钥**，而它唯一的来源是错误值（`HostKeyProblem::Changed.presented`）。
/// 从「拿到错误」到「用户真的点替换」之间隔着至少一帧，所以必须先把它存下来。
#[derive(Clone, Debug, PartialEq, Eq)]
pub(in crate::ui) struct Replacement {
    pub host: String,
    pub port: u16,
    /// `known_hosts` 里那条旧记录的行号（1 起），窗口里要报给用户。
    pub line: usize,
    /// 服务器这次给的公钥（openssh 单行）。换钥就是把它写进账本。
    pub key_line: String,
    /// 上面那把钥匙的指纹，给人核对。
    pub fingerprint: String,
}

/// 连接失败之后下一步问用户什么。
#[derive(Clone, Debug, PartialEq, Eq)]
pub(in crate::ui) enum Next {
    /// 没见过这台主机：把指纹摆出来让用户决定。
    AskHostKey { fingerprint: String },
    /// 密码不对：重新问一次，并带上原因。
    AskPassword { reason: String },
    /// 钥匙变了：**不提供「继续」**，只允许「明确替换旧记录」或放弃。
    OfferReplace { changed: Replacement },
    /// 别的情况：显示错误，到此为止。
    ShowError { message: String },
}

/// 判断一个 SSH 会话需要什么凭据。
///
/// `password` 是手上已有的密码（表单里刚填的 / 保险库里拿的 / OneKey 方案
/// 存的那一条），`answers` 是 OneKey 方案**键盘交互**应答表里的内容（按服务端
/// 提示顺序）。两者分开是因为它们回答的不是同一个问题：密码是「你是谁」的
/// 答案，应答表是「验证码 / 令牌 / 二次确认」那一串。
///
/// `home` 由调用方传进来（而不是在这里读环境）：一是为了单测能控制，
/// 二是这一层没有理由自己去猜家目录。
pub(in crate::ui) fn ask(
    config: &SessionConfig,
    password: Option<&str>,
    answers: &[String],
    home: Option<&Path>,
) -> Ask {
    match config.auth {
        AuthMethod::Password => match password {
            Some(p) if !p.is_empty() => Ask::Connect(SshAuth::Password {
                password: p.to_string(),
            }),
            // 没带密码就先问，**不要**空密码去试一次：那会白白多一次失败握手，
            // 还可能触发服务器上的失败计数。
            _ => Ask::Password,
        },
        AuthMethod::Agent => Ask::Connect(SshAuth::Agent),
        // V1-3：表单里填了 `IdentityFile` 就**只认它**。
        //
        // 以前这里一律往下走默认私钥，于是「Public Keys 页填了路径」这件事对内置
        // 客户端完全不存在——能填、能存、不生效，正是这一批要收拾的病。
        //
        // 反过来也**不**偷偷降级（选的文件不在 → 改用 agent / 密码）：那会让
        // 「我明明选了这把钥匙」变成一次无从解释的认证失败。文件不在就报出来。
        AuthMethod::PublicKey => match configured_identity(config, home) {
            Ok(Some(path)) => Ask::Connect(SshAuth::PrivateKey {
                path,
                passphrase: None,
            }),
            // 明确报错：路径是用户自己填的，只有他能判断是笔误还是要换个文件。
            // 加密私钥这件事不在这里问口令——口令交互是连接层的活，本轮不做。
            Err(missing) => Ask::Unsupported(format!("私钥文件不存在：{}", missing.display())),
            Ok(None) => match default_private_key(home) {
                Some(path) => Ask::Connect(SshAuth::PrivateKey {
                    path,
                    passphrase: None,
                }),
                None => Ask::Unsupported(
                    "没有找到默认私钥（~/.ssh/id_ed25519、id_ecdsa、id_rsa 都没有）。\
                     在「会话属性 → Public Keys」里填一个 IdentityFile，\
                     或改用密码 / ssh-agent，或关掉「内置 SSH 客户端」改用系统 ssh"
                        .to_string(),
                ),
            },
        },
        AuthMethod::GssapiWithMic => Ask::Unsupported(
            "内置 SSH 客户端不支持 GSSAPI。请关掉「内置 SSH 客户端」改用系统 ssh".to_string(),
        ),
        // V1-3：交互认证的第一次提问通常就是密码 / OTP；没带答案先问，
        // 与 Password 路径共用同一个密码对话框（不新增一套弹窗）。
        //
        // OneKey 方案里配了应答表时**直接用**：那正是「自动登录」这句话的
        // 落地处——方案记住了服务端会问什么、该答什么，连接时不该再弹一次
        // 密码框把用户拦住。
        AuthMethod::KeyboardInteractive => {
            if !answers.is_empty() {
                return Ask::Connect(SshAuth::KeyboardInteractive {
                    answers: answers.to_vec(),
                });
            }
            match password {
                Some(p) if !p.is_empty() => Ask::Connect(SshAuth::KeyboardInteractive {
                    answers: vec![p.to_string()],
                }),
                _ => Ask::Password,
            }
        }
    }
}

/// 按 OpenSSH 的顺序找默认身份文件。
///
/// 顺序照 `ssh` 自己（ed25519 → ecdsa → rsa）：同一个目录下往往三把都在，
/// 先试 ed25519 才能和 `ssh` 连到同一把钥匙。
fn default_private_key(home: Option<&Path>) -> Option<PathBuf> {
    let home = home?;
    ["id_ed25519", "id_ecdsa", "id_rsa"]
        .iter()
        .map(|name| home.join(".ssh").join(name))
        .find(|p| p.is_file())
}

/// 用户手填的 `IdentityFile`。
///
/// 三种答案，调用方必须分开处理，不能糊成一句「没有钥匙」：
///
/// * `Ok(None)`：没填 → 才轮到默认私钥的探测顺序；
/// * `Ok(Some(path))`：填了且文件在 → 用它（**不要再试别的**）；
/// * `Err(path)`：填了但文件不在 → 用户要看到的正是这个路径。
fn configured_identity(
    config: &SessionConfig,
    home: Option<&Path>,
) -> Result<Option<PathBuf>, PathBuf> {
    let raw = config.identity_file.trim();
    if raw.is_empty() {
        return Ok(None);
    }
    let path = expand_tilde(raw, home);
    if path.is_file() {
        Ok(Some(path))
    } else {
        Err(path)
    }
}

/// 展开 `~` 前缀。
///
/// 表单里的提示语就是 `~/.ssh/id_ed25519`，而 `Path::is_file ` 不会替你展开
/// 波浪号——不展开就会把 OpenSSH 的惯用写法判成「文件不存在」。
fn expand_tilde(raw: &str, home: Option<&Path>) -> PathBuf {
    match (raw.strip_prefix("~/"), home) {
        (Some(rest), Some(home)) => home.join(rest),
        // 没有家目录时原样用：能不能打开由 `is_file ` 那一句给答案。
        _ => PathBuf::from(raw),
    }
}

/// 失败之后问什么。
pub(in crate::ui) fn next(err: &SshError, password_auth: bool) -> Next {
    match err {
        // 只有「未知主机」值得问：钥匙变了是危险信号，问「要不要继续」等于
        // 把中间人攻击变成一个默认按钮——那一步必须停住。
        SshError::HostKey(term_ssh::HostKeyProblem::Unknown { fingerprint, .. }) => {
            Next::AskHostKey {
                fingerprint: fingerprint.clone(),
            }
        }
        // 「停住」不等于「什么也不给」：SSH 自己的答案是让人去跑
        // `ssh-keygen -R`。这里给的是一个**独立窗口**，正文写清风险、按钮
        // 写的是「替换已记录的钥匙」而不是「继续连接」——动作是可审计的改账本，
        // 不是一次默认放行。
        SshError::HostKey(term_ssh::HostKeyProblem::Changed {
            host,
            port,
            line,
            presented,
            presented_fingerprint,
        }) => Next::OfferReplace {
            changed: Replacement {
                host: host.clone(),
                port: *port,
                line: *line,
                key_line: presented.clone(),
                fingerprint: presented_fingerprint.clone(),
            },
        },
        SshError::Auth(_) if password_auth => Next::AskPassword {
            reason: err.to_string(),
        },
        other => Next::ShowError {
            message: other.to_string(),
        },
    }
}

/// 连接参数：会话配置 → `SshOptions`。
///
/// 返回 `Result` 而不是裸 `SshOptions`：`ProxyJump` 链是**用户手写的字符串**，
/// 写错了必须当场报出来。若在这里 `unwrap_or_default ` 一下，用户填的跳板链
/// 就会静默变成直连——「能填、能存、不生效」正是这一批要修的病。
///
/// 空串 / `none` 不用特判：`parse_proxy_jump` 自己把它们解析成空链（= 直连）。
///
/// `term` 跟着会话走（用户可能选了 `xterm`），尺寸先给 80×24，
/// 真正的尺寸在 tab 建立后由 `handle.resize ` 逐帧纠正。
pub(in crate::ui) fn options_for(
    config: &SessionConfig,
    auth: SshAuth,
    policy: HostKeyPolicy,
) -> Result<SshOptions, SshError> {
    let mut opts = SshOptions::new(config.host.trim(), config.username.trim(), auth.clone());
    if config.port != 0 {
        opts.port = config.port;
    }
    if !config.term.is_empty() {
        opts.term = config.term.clone();
    }
    if config.connection_timeout > 0 {
        opts.connect_timeout = std::time::Duration::from_secs(config.connection_timeout as u64);
    }
    opts.keepalive = (config.keep_alive > 0)
        .then(|| std::time::Duration::from_secs(config.keep_alive as u64));
    opts.host_key = policy;
    // V1-3：跳板链与 agent 转发在这里真正落到连接参数上。此前这两项只存在
    // `SessionConfig` 里，内置 russh 路径从来没读过——填了等于没填。
    opts.proxy_jump = term_ssh::parse_proxy_jump(&config.proxy_jump)?;
    opts.agent_forward = config.agent_forward;
    opts.x11_forward = config.x11_forward;
    opts.x11_screen = config.x11_screen;
    // 出网代理真的落到第一段 TCP 上。选了 HTTP/SOCKS5 却没填主机时
    // 直接报错，绝不能退化成直连（那就是「能填、能存、不生效」）。
    if let Some(why) = config.validate_proxy() {
        return Err(SshError::Config(why));
    }
    opts.outbound_proxy = config.outbound_proxy_spec().map(|(kind, host, port)| OutboundProxy {
        // `outbound_proxy_spec` 只返回 Http / Socks5 两档。
        kind: if kind == ProxyType::Http { ProxyKind::Http } else { ProxyKind::Socks5 },
        host: host.to_string(),
        port,
        // 凭据不落盘（ 接表单 + 保险库后再由调用方注入）。
        auth: None,
        dns_over_proxy: config.dns_over_proxy,
    });
    // 界面路径的回退链。`auth` 里带着用户刚在对话框输入的密码，所以这里
    // 能把完整的「密码→公钥」OneKey 编进链（headless 路径拿不到密码，只能做
    // Agent / PublicKey）。链长 ≤1 时保持单方法旧行为不变。
    let chain = ui_fallback_chain(config, &auth, dirs::home_dir().as_deref());
    if chain.len() > 1 {
        opts.auth_chain = chain;
    }
    Ok(opts)
}

/// 界面路径的回退链解析。
///
/// 与 headless 的 `session_open::ssh_fallback_chain` 的差别只有一处：这里
/// **能拿到密码**（用户刚在密码框输入的那一个，藏在 `primary` 里），所以
/// Password / KeyboardInteractive 也能进链——「密码被拒后自动改试公钥」正是
/// 界面 OneKey 的核心场景。
///
/// 解析失败 / 缺件的某一环**跳过而不是报错**：回退链里勾了公钥但文件不在，
/// 不该让本来能用的密码首选整体失败。
fn ui_fallback_chain(
    config: &SessionConfig,
    primary: &SshAuth,
    home: Option<&Path>,
) -> Vec<SshAuth> {
    let mut chain = vec![primary.clone()];
    // 从 primary 里取密码：Password 变体自带；KeyboardInteractive 取第一条答案。
    let password: Option<String> = match primary {
        SshAuth::Password { password } => Some(password.clone()),
        SshAuth::KeyboardInteractive { answers } => answers.first().cloned(),
        _ => None,
    };
    for method in &config.auth_chain {
        let candidate = match method {
            AuthMethod::Agent => Some(SshAuth::Agent),
            AuthMethod::PublicKey => match configured_identity(config, home) {
                Ok(Some(path)) => Some(SshAuth::PrivateKey {
                    path,
                    passphrase: None,
                }),
                Ok(None) => default_private_key(home).map(|path| SshAuth::PrivateKey {
                    path,
                    passphrase: None,
                }),
                // 填的路径不存在：跳过这一环，不拦首选。
                Err(_) => None,
            },
            AuthMethod::Password => password
                .clone()
                .map(|p| SshAuth::Password { password: p }),
            AuthMethod::KeyboardInteractive => password
                .clone()
                .map(|p| SshAuth::KeyboardInteractive { answers: vec![p] }),
            // 本客户端不支持 GSSAPI：跳过。
            AuthMethod::GssapiWithMic => None,
        };
        if let Some(c) = candidate {
            if !chain.contains(&c) {
                chain.push(c);
            }
        }
    }
    chain
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ssh_session(auth: AuthMethod) -> SessionConfig {
        let mut c = SessionConfig::new(Protocol::Ssh);
        c.host = " example.com ".to_string();
        c.port = 2222;
        c.username = " root ".to_string();
        c.auth = auth;
        c.term = "xterm".to_string();
        c
    }

    #[test]
    fn a_password_session_without_a_password_asks_before_connecting() {
        let c = ssh_session(AuthMethod::Password);
        // 先问，而不是拿空密码去试一次：少一次失败握手，也少一次服务器侧计数。
        assert_eq!(ask(&c, None, &[], None), Ask::Password);
        assert_eq!(ask(&c, Some(""), &[], None), Ask::Password);
    }

    #[test]
    fn a_password_session_with_a_password_is_ready() {
        let c = ssh_session(AuthMethod::Password);
        assert_eq!(
            ask(&c, Some("s3cret"), &[], None),
            Ask::Connect(SshAuth::Password {
                password: "s3cret".to_string()
            })
        );
    }

    #[test]
    fn an_agent_session_needs_nothing_from_the_user() {
        let c = ssh_session(AuthMethod::Agent);
        assert_eq!(ask(&c, None, &[], None), Ask::Connect(SshAuth::Agent));
    }

    #[test]
    fn a_public_key_session_picks_the_first_default_identity() {
        let dir = std::env::temp_dir().join(format!("desk-ssh-key-{}", std::process::id()));
        let ssh_dir = dir.join(".ssh");
        std::fs::create_dir_all(&ssh_dir).expect("建目录");
        std::fs::write(ssh_dir.join("id_rsa"), b"x").expect("写文件");

        let c = ssh_session(AuthMethod::PublicKey);
        match ask(&c, None, &[], Some(&dir)) {
            Ask::Connect(SshAuth::PrivateKey { path, passphrase }) => {
                assert!(path.ends_with(".ssh/id_rsa"), "{path:?}");
                assert!(passphrase.is_none());
            }
            other => panic!("应当用默认私钥，实际是 {other:?}"),
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn ed25519_wins_over_rsa_when_both_exist() {
        let dir = std::env::temp_dir().join(format!("desk-ssh-order-{}", std::process::id()));
        let ssh_dir = dir.join(".ssh");
        std::fs::create_dir_all(&ssh_dir).expect("建目录");
        std::fs::write(ssh_dir.join("id_rsa"), b"x").expect("写 rsa");
        std::fs::write(ssh_dir.join("id_ed25519"), b"x").expect("写 ed25519");

        let c = ssh_session(AuthMethod::PublicKey);
        match ask(&c, None, &[], Some(&dir)) {
            Ask::Connect(SshAuth::PrivateKey { path, .. }) => {
                assert!(path.ends_with("id_ed25519"), "应当先用 ed25519：{path:?}");
            }
            other => panic!("{other:?}"),
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_public_key_session_without_any_default_key_explains_itself() {
        let dir = std::env::temp_dir().join(format!("desk-ssh-empty-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("建目录");
        let c = ssh_session(AuthMethod::PublicKey);
        match ask(&c, None, &[], Some(&dir)) {
            Ask::Unsupported(msg) => {
                assert!(msg.contains("默认私钥"), "{msg}");
                // 提示里必须给出**退路**：改回系统 ssh 是这个功能唯一的逃生门。
                assert!(msg.contains("系统 ssh"), "{msg}");
            }
            other => panic!("应当报不支持，实际是 {other:?}"),
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn gssapi_is_refused_with_a_way_out() {
        let c = ssh_session(AuthMethod::GssapiWithMic);
        match ask(&c, None, &[], None) {
            Ask::Unsupported(msg) => assert!(msg.contains("系统 ssh"), "{msg}"),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn options_are_carried_over_from_the_session() {
        let mut c = ssh_session(AuthMethod::Password);
        c.keep_alive = 30;
        c.connection_timeout = 5;
        let opts = options_for(&c, SshAuth::Agent, HostKeyPolicy::Strict).expect("空跳板链应当能解析");
        // 主机名与用户名要 trim：从表单粘进来的字符串常带空格，带空格的主机名
        // 在 DNS 层会失败成一句看不懂的错。
        assert_eq!(opts.host, "example.com");
        assert_eq!(opts.user, "root");
        assert_eq!(opts.port, 2222);
        assert_eq!(opts.term, "xterm");
        assert_eq!(opts.connect_timeout.as_secs(), 5);
        assert_eq!(opts.keepalive.map(|d| d.as_secs()), Some(30));
        assert_eq!(opts.host_key, HostKeyPolicy::Strict);
    }

    #[test]
    fn an_unknown_host_asks_and_a_changed_key_only_offers_a_replacement() {
        let unknown = SshError::HostKey(term_ssh::HostKeyProblem::Unknown {
            host: "h".to_string(),
            port: 22,
            fingerprint: "SHA256:abc".to_string(),
        });
        assert_eq!(
            next(&unknown, false),
            Next::AskHostKey {
                fingerprint: "SHA256:abc".to_string()
            }
        );

        // 钥匙变了：**不给「继续」**。问「要不要继续」就等于给中间人一个按钮。
        // 给的是「换掉记录」——那是改账本，不是放行这一次连接。
        let changed = SshError::HostKey(term_ssh::HostKeyProblem::Changed {
            host: "h".to_string(),
            port: 22,
            line: 3,
            presented: "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAInew".to_string(),
            presented_fingerprint: "SHA256:new".to_string(),
        });
        match next(&changed, false) {
            Next::OfferReplace { changed } => {
                // 换钥窗口要报三件事：哪台机器、旧记录在第几行、这次给的是哪把。
                assert_eq!(changed.host, "h");
                assert_eq!(changed.port, 22);
                assert_eq!(changed.line, 3);
                assert_eq!(changed.fingerprint, "SHA256:new");
                // 关键：要写进账本的是**服务器这次给的**那把钥匙，
                // 不是账本里的旧钥匙（写旧钥匙等于什么都没换）。
                assert!(changed.key_line.contains("AAAAC3NzaC1lZDI1NTE5AAAAInew"));
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn a_password_failure_re_asks_and_an_agent_failure_does_not() {
        let e = SshError::Auth(term_ssh::AuthProblem {
            user: "root".to_string(),
            method: "password",
            partial_success: false,
        });
        assert!(matches!(next(&e, true), Next::AskPassword { .. }));
        // agent 没有「再来一次」的意思：用户要改的是 agent，不是这个对话框。
        assert!(matches!(next(&e, false), Next::ShowError { .. }));
    }

    #[test]
    fn a_network_failure_is_terminal() {
        let e = SshError::Connect("connection refused".to_string());
        match next(&e, true) {
            Next::ShowError { message } => assert!(message.contains("无法连接"), "{message}"),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn keyboard_interactive_asks_for_an_answer_before_connecting() {
        let mut c = ssh_session(AuthMethod::Password);
        c.auth = term_core::session::AuthMethod::KeyboardInteractive;
        assert_eq!(ask(&c, None, &[], None), Ask::Password);
        match ask(&c, Some("otp-123"), &[], None) {
            Ask::Connect(SshAuth::KeyboardInteractive { answers }) => {
                assert_eq!(answers, vec!["otp-123".to_string()]);
            }
            other => panic!("{other:?}"),
        }
    }
}

// 跳板链 / agent 转发 / 私钥文件这一批单测单独一个文件：（verify 4j）要求
// `ui/**/*.rs` ≤ 450 行，全塞进上面那个 `mod tests` 会把本文件顶破。
// 拆法与 `session_nav_tests.rs` 相同——仍是 `super::*` 的私有作用域。
#[cfg(test)]
#[path = "ssh_ask_tests.rs"]
mod connection_tests;

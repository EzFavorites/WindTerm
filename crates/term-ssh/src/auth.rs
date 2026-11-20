//! 三种认证方式。
//!
//! | 方式 | 私钥在哪 | 要不要碰磁盘 | 典型场景 |
//! |---|---|---|---|
//! | 密码 | 没有私钥 | 否 | 临时登录、跳板机 |
//! | 私钥文件 | 本机文件 | 是（读 PEM / OpenSSH） | 日常运维 |
//! | ssh-agent | 在 agent 进程里 | 否（走 `$SSH_AUTH_SOCK`） | 私钥带口令、不想反复输 |
//!
//! 三种方式共用一件事：**失败要说清是哪一种失败**。服务器只回一句
//! "auth failed"，用户需要知道是密码错了还是服务器不认这把钥匙，否则下一步
//! 就是盲目重试。
//!
//! 关于 RSA：`authenticate_publickey` 需要自己选 `rsa-sha2-256` 还是 `-512`，
//! 选错的表现是「钥匙明明对却认证失败」。这里一律问服务器
//! （`best_supported_rsa_hash`），不写死。

use std::sync::Arc;

use russh::client;
use russh::keys::PrivateKeyWithHashAlg;
use russh::keys::load_secret_key;

use crate::error::{AuthProblem, SshError};
use crate::handler::HostKeyHandler;
use crate::options::{AuthMethod, SshOptions};

/// D2（OneKey）：执行一条认证链。
///
/// `opts.auth_chain` 非空时按序逐个尝试，第一个被接受即停；为空则回落单一的
/// `opts.auth`（旧行为，零破坏）。继续/停止的判据见 [`SshOptions::auth_chain`]：
/// 认证被拒或该方式本地用不了 → 继续；通道 / 本地环境错误 → 立即返回。
///
/// 每一次尝试都会在服务端留下一次 `userauth` 请求——这正是 OpenSSH 的做法，
/// 服务端的 `MaxAuthTries`（默认 6）是共同的约束，所以链长不该远超它。
pub(crate) async fn authenticate(
    handle: &mut client::Handle<HostKeyHandler>,
    opts: &SshOptions,
) -> Result<(), SshError> {
    if opts.auth_chain.is_empty() {
        return authenticate_one(handle, opts, &opts.auth).await;
    }
    // 记下最后一次「这一种方式不成立」的失败，供链跑完后统一报错。
    let mut last: Option<SshError> = None;
    for method in &opts.auth_chain {
        match authenticate_one(handle, opts, method).await {
            Ok(()) => return Ok(()),
            // 服务端拒绝这一种 → 继续下一种。
            Err(e @ SshError::Auth(_)) => last = Some(e),
            // 本地根本用不了（私钥读不出来 / 需要口令却没给）→ 等同不可用，继续。
            Err(e @ SshError::Key(_)) => last = Some(e),
            // 通道 / 本地环境 / 配置错误：换一种认证方式也不会好，直接返回，
            // 不用「继续试」掩盖真正的病因。
            Err(e) => return Err(e),
        }
    }
    Err(last.unwrap_or_else(|| {
        SshError::Auth(AuthProblem {
            user: opts.user.clone(),
            method: "auth-chain",
            partial_success: false,
        })
    }))
}

/// 用**某一种**方式认证一次。成功返回 `Ok(())`，失败返回带方式的 `AuthProblem`
/// （或私钥读不出来时的 `Key`）。
async fn authenticate_one(
    handle: &mut client::Handle<HostKeyHandler>,
    opts: &SshOptions,
    method: &AuthMethod,
) -> Result<(), SshError> {
    let label = method.label();
    match method {
        AuthMethod::Password { password } => {
            let result = handle
                .authenticate_password(opts.user.clone(), password.clone())
                .await
                .map_err(|e| SshError::Channel(format!("发送认证请求失败：{e}")))?;
            check(result, &opts.user, label)
        }
        AuthMethod::PrivateKey { path, passphrase } => {
            let key = load_secret_key(path, passphrase.as_deref()).map_err(|e| key_error(path, e))?;
            let hash_alg = handle
                .best_supported_rsa_hash()
                .await
                .map_err(|e| SshError::Channel(format!("查询服务器的 RSA 哈希算法失败：{e}")))?
                .flatten();
            let result = handle
                .authenticate_publickey(
                    opts.user.clone(),
                    PrivateKeyWithHashAlg::new(Arc::new(key), hash_alg),
                )
                .await
                .map_err(|e| SshError::Channel(format!("发送认证请求失败：{e}")))?;
            check(result, &opts.user, label)
        }
        AuthMethod::Agent => authenticate_with_agent(handle, opts).await,
        AuthMethod::KeyboardInteractive { answers } => {
            authenticate_interactive(handle, opts, answers).await
        }
    }
}

/// keyboard-interactive：服务端可以问任意多轮，`answers` 按提示顺序回答。
/// 答案不够就补空串（服务端会拒绝，但不会卡在这里等一个永远不来的输入）。
/// 真机握手未在本机验收（没有跑交互式认证的 SSH 服务端），见
/// `docs/design/01-sessions-and-connection.md`；这里落地的是 russh 的真实调用链与语义。
async fn authenticate_interactive(
    handle: &mut client::Handle<HostKeyHandler>,
    opts: &SshOptions,
    answers: &[String],
) -> Result<(), SshError> {
    let mut response = handle
        .authenticate_keyboard_interactive_start(opts.user.clone(), None::<String>)
        .await
        .map_err(|e| SshError::Channel(format!("发起交互认证失败：{e}")))?;
    loop {
        match response {
            client::KeyboardInteractiveAuthResponse::Success => return Ok(()),
            client::KeyboardInteractiveAuthResponse::Failure { .. } => {
                return Err(SshError::Auth(AuthProblem {
                    user: opts.user.clone(),
                    method: "keyboard-interactive",
                    partial_success: false,
                }));
            }
            client::KeyboardInteractiveAuthResponse::InfoRequest { prompts, .. } => {
                let reply: Vec<String> = (0..prompts.len())
                    .map(|i| answers.get(i).cloned().unwrap_or_default())
                    .collect();
                response = handle
                    .authenticate_keyboard_interactive_respond(reply)
                    .await
                    .map_err(|e| SshError::Channel(format!("回答交互认证失败：{e}")))?;
            }
        }
    }
}

fn check(
    result: client::AuthResult,
    user: &str,
    method: &'static str,
) -> Result<(), SshError> {
    if result.success() {
        return Ok(());
    }
    let partial_success = matches!(
        result,
        client::AuthResult::Failure {
            partial_success: true,
            ..
        }
    );
    Err(SshError::Auth(AuthProblem {
        user: user.to_string(),
        method,
        partial_success,
    }))
}

/// 私钥读不出来：把库的错误翻译成用户能动手的那句话。
fn key_error(path: &std::path::Path, e: russh::keys::Error) -> SshError {
    match e {
        russh::keys::Error::KeyIsEncrypted => SshError::Key(format!(
            "{} 有口令，请填写私钥口令（passphrase）",
            path.display()
        )),
        other => SshError::Key(format!("{}：{other}", path.display())),
    }
}

/// ssh-agent 认证：把 agent 里每一把钥匙都试一遍。
///
/// 为什么要遍历：agent 里通常躺着好几把钥匙，服务器只认其中一把，而
/// 「服务器到底认哪一把」只能靠试。这也正是 OpenSSH 的做法。
#[cfg(unix)]
async fn authenticate_with_agent(
    handle: &mut client::Handle<HostKeyHandler>,
    opts: &SshOptions,
) -> Result<(), SshError> {
    use russh::keys::agent::client::AgentClient;

    let mut agent = AgentClient::connect_env()
        .await
        .map_err(|e| SshError::Local(format!("连不上 ssh-agent（$SSH_AUTH_SOCK）：{e}")))?;
    let identities = agent
        .request_identities()
        .await
        .map_err(|e| SshError::Local(format!("向 ssh-agent 索要身份列表失败：{e}")))?;
    if identities.is_empty() {
        return Err(SshError::Auth(AuthProblem {
            user: opts.user.clone(),
            method: "agent",
            partial_success: false,
        }));
    }
    let hash_alg = handle
        .best_supported_rsa_hash()
        .await
        .map_err(|e| SshError::Channel(format!("查询服务器的 RSA 哈希算法失败：{e}")))?
        .flatten();

    for identity in identities {
        // `public_key()` 借用了 identity，而 `authenticate_publickey_with`
        // 要一个独立的 `PublicKey`：先克隆出来，避免同时借 agent。
        let public = identity.public_key().into_owned();
        match handle
            .authenticate_publickey_with(opts.user.clone(), public, hash_alg, &mut agent)
            .await
        {
            Ok(result) if result.success() => return Ok(()),
            Ok(_) => continue,
            Err(e) => {
                return Err(SshError::Local(format!(
                    "ssh-agent 签名失败（可能是不支持这把钥匙或需要确认）：{e}"
                )));
            }
        }
    }
    Err(SshError::Auth(AuthProblem {
        user: opts.user.clone(),
        method: "agent",
        partial_success: false,
    }))
}

/// 非 Unix 平台：`$SSH_AUTH_SOCK` 不存在，agent 通道暂不打通
/// （Windows 上要走 Pageant / 命名管道，等有实际需求再补）。
#[cfg(not(unix))]
async fn authenticate_with_agent(
    _handle: &mut client::Handle<HostKeyHandler>,
    _opts: &SshOptions,
) -> Result<(), SshError> {
    Err(SshError::Local(
        "此平台暂不支持 ssh-agent 认证，请改用密码或私钥文件".to_string(),
    ))
}

//! 连接链：把 `ProxyJump` 变成真实的隧道（`ssh -J`）。
//!
//! ```text
//! TCP ──▶ [第 1 跳] ──direct-tcpip──▶ [第 2 跳] ──direct-tcpip──▶ [目标]
//!          SSH 握手 + 认证              SSH 握手 + 认证            SSH 握手 + 认证
//! ```
//!
//! 每一跳都是一条**完整的 SSH 连接**，只有最底下那一段是 TCP；上面每一段都是
//! 前一条连接里的一条 `direct-tcpip` 通道，而这条通道本身就是一条双向字节流，
//! 于是同一份「握手 + 认证」代码可以原样复用在它上面（`client::connect_stream`
//! 只要求 `AsyncRead + AsyncWrite`）。
//!
//! 两条容易写错的地方，这里明确写成规则：
//!
//! 1. **中间连接的句柄不能掉。** `Handle` 一被 drop，那条连接的会话任务就结束，
//!    整条链就断了。所以每一跳的 `Handle` 都被塞进下一跳 `HostKeyHandler` 的
//!    `upstream` 字段里——它的生命周期于是正好等于「最终会话还活着」。
//! 2. **跳板机的身份是跳板机的。** known_hosts 校验、用户名、端口都用**这一跳**
//!    的参数（`Target::options` 造一份改写过主机/端口的 `SshOptions`），否则
//!    记账会记到目标机头上，用户名也会串味。
//!
//! 关于 agent 转发（`ssh -A`）的分工：
//! - **本模块**：连之前就问「本机到底有没有 agent」，没有就明确报错
//!   （[`agent_forward_error`]）；
//! - **`handler`**：接受服务端开来的 `auth-agent@openssh.com` 通道，桥到本地
//!   `$SSH_AUTH_SOCK`；
//! - **没做**：不解析 agent 协议（只搬字节）、不缓存也不重连通道、
//!   Windows 的命名管道未接线（在那些平台上开 `-A` 会直接报错，不会假装成功）。

use std::net::{SocketAddr, ToSocketAddrs};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use russh::client::{self, Handle};
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::net::TcpStream;

use crate::auth;
use crate::authenticated::ForwardSink;
use crate::error::SshError;
use crate::handler::{self, HostKeyHandler, Rejection};
use crate::options::{DEFAULT_PORT, SshOptions};
use crate::outbound;

/// 一条能承载 SSH 的字节流：第一段是 TCP，之后的每一段是上游连接里的一条通道。
///
/// 用 trait object 而不是枚举，是因为 `connect_stream` 只在每段内部用一次，
/// 装箱的那点开销远小于为「可能有几种流」写一个 enum 再手写两个 trait 的
/// `poll_*` 转发。
pub(crate) trait Stream: AsyncRead + AsyncWrite + Unpin + Send {}
impl<T: AsyncRead + AsyncWrite + Unpin + Send> Stream for T {}
pub(crate) type BoxStream = Box<dyn Stream>;

/// `connect_timeout = 0` 的语义是「不设超时」。但 `tokio::time::timeout(0, …)`
/// 是**立刻超时**——不统一映射的话，用户在表单里填 0 会在第一跳就失败。
/// 所有阶段（TCP / 代理协商 / KEX / 开通道）都经这里换算成一个长兜底，
/// 认证阶段另有自己的 120 秒下限（见 `handshake`）。
pub(crate) const NO_TIMEOUT_FALLBACK: Duration = Duration::from_secs(120);

fn stage_timeout(timeout: Duration) -> Duration {
    if timeout.is_zero() {
        NO_TIMEOUT_FALLBACK
    } else {
        timeout
    }
}

/// 链上的一个端点：先握手的是第 0 跳，最后一个是用户真正要连的机器。
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Target {
    /// 已经按 `hop.user.unwrap_or(session.user)` 解析过的用户名。
    pub user: String,
    pub host: String,
    pub port: u16,
    /// `None` = 最终目标；`Some(i)` = 第 i 跳（0 基）。
    pub hop: Option<usize>,
}

impl Target {
    /// 出错时怎么称呼这个端点。跳的身份必须出现在错误信息里——「连不上」与
    /// 「第 2 跳连不上」对用户是两件事。
    pub(crate) fn label(&self) -> String {
        match self.hop {
            Some(i) => format!("ProxyJump 第 {} 跳 ssh://{}@{}:{}", i + 1, self.user, self.host, self.port),
            None if self.port == DEFAULT_PORT => format!("ssh://{}@{}", self.user, self.host),
            None => format!("ssh://{}@{}:{}", self.user, self.host, self.port),
        }
    }

    /// 把会话参数改写成「连这一跳」的参数。
    ///
    /// 三处刻意的改写：
    /// - `proxy_jump` 清空：跳在这里已经被兑现成一条真实的流，子连接没有自己的跳；
    /// - `agent_forward` 只在最终目标上保留：`ssh -A` 不把本地 agent 交给跳板机；
    /// - known_hosts 路径不变（同一个账本），但主机/端口换了，于是每一跳各记各的。
    fn options(&self, base: &SshOptions) -> SshOptions {
        let mut opts = base.clone();
        opts.host.clone_from(&self.host);
        opts.port = self.port;
        opts.user.clone_from(&self.user);
        opts.proxy_jump = Vec::new();
        opts.agent_forward = self.hop.is_none() && base.agent_forward;
        opts
    }
}

/// 整条链：`proxy_jump` 的每一跳，加上最后的目标。
pub(crate) fn chain(opts: &SshOptions) -> Vec<Target> {
    let mut targets: Vec<Target> = opts
        .proxy_jump
        .iter()
        .enumerate()
        .map(|(i, hop)| Target {
            // `ssh -J` 的默认：跳上没写用户名就沿用会话的用户名。
            user: hop.user.clone().unwrap_or_else(|| opts.user.clone()),
            host: hop.host.clone(),
            port: hop.port,
            hop: Some(i),
        })
        .collect();
    targets.push(Target { user: opts.user.clone(), host: opts.host.clone(), port: opts.port, hop: None });
    targets
}

/// `direct-tcpip` 请求里报给服务端的「这条连接从哪来」。
///
/// 只有「本机地址取不到」时才编一个 `127.0.0.1:0`：这个字段是给跳板机写日志用的，
/// 不影响转发本身，为它失败一次是得不偿失。
pub(crate) fn origin(local: Option<SocketAddr>) -> (String, u16) {
    match local {
        Some(addr) => (addr.ip().to_string(), addr.port()),
        None => ("127.0.0.1".to_string(), 0),
    }
}

/// 本地 ssh-agent 的 socket 路径（`$SSH_AUTH_SOCK`）。
pub(crate) fn agent_socket() -> Option<PathBuf> {
    std::env::var_os("SSH_AUTH_SOCK")
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
}

/// 要求了 `ssh -A` 却没法转发时的错误；能转发就是 `None`。
///
/// 为什么在握手**之前**就报错：远端要等到真正用 agent 时才失败（典型的是
/// 「怎么跳到下一台还是要密码/要钥匙」），那时用户已经在终端里了，看到的是
/// 一句 `Permission denied (publickey)`。把配置问题提前到「点连接」这一刻说清，
/// 是这一层唯一能让用户少走一趟的做法。
///
/// 抽成纯函数（socket 由调用方查）是为了能在没有环境变量的单测里钉住这两个分支。
pub(crate) fn agent_forward_error(agent_forward: bool, socket: Option<&Path>) -> Option<SshError> {
    if !agent_forward {
        return None;
    }
    if !cfg!(unix) {
        return Some(SshError::Local(
            "此平台暂不支持 agent 转发（ssh -A）：Windows 上的 agent 走命名管道，尚未接线".to_string(),
        ));
    }
    match socket {
        Some(_) => None,
        None => Some(SshError::Local(
            "已请求 agent 转发（ssh -A），但本机没有 $SSH_AUTH_SOCK —— 先启动 ssh-agent 或关掉转发"
                .to_string(),
        )),
    }
}

/// 主机 + 端口 → 一个具体地址。DNS 失败与「连上了但被拒」是两类错误。
pub(crate) fn resolve_host(host: &str, port: u16) -> Result<SocketAddr, SshError> {
    (host, port)
        .to_socket_addrs()
        .map_err(|e| SshError::Resolve(format!("{host}:{port} —— {e}")))?
        .next()
        .ok_or_else(|| SshError::Resolve(format!("{host} 没有解析出任何地址")))
}

/// 带预算的解析：`getaddrinfo` 是阻塞调用，直接放在 async 线程里会把整条
/// 连接链钉住（DNS 卡住时连 `connect_timeout` 都没机会生效）。搬进
/// `spawn_blocking` 再套一层超时，时间到就返回 `Resolve`，工作线程自己收摊。
pub(crate) async fn resolve_host_async(
    host: &str,
    port: u16,
    timeout: Duration,
) -> Result<SocketAddr, SshError> {
    let owned = host.to_string();
    let task = tokio::task::spawn_blocking(move || resolve_host(&owned, port));
    await_resolve(host, port, timeout, task).await
}

/// 等解析任务但预算有界。单独抽出来是为了让「超时」分支能在单测里用一条
/// 保证不返回的 future 钉住——直接拿 `Duration::ZERO` 去赌本机 DNS 没跑完
/// 是会 flaky 的（回环解析有时真的能抢在计时器之前完成）。
async fn await_resolve(
    host: &str,
    port: u16,
    timeout: Duration,
    task: tokio::task::JoinHandle<Result<SocketAddr, SshError>>,
) -> Result<SocketAddr, SshError> {
    match tokio::time::timeout(timeout, task).await {
        Ok(Ok(result)) => result,
        Ok(Err(e)) => Err(SshError::Resolve(format!(
            "{host}:{port} —— DNS 工作线程退出：{e}"
        ))),
        Err(_) => Err(SshError::Resolve(format!(
            "{host}:{port} —— {timeout:?} 内没有解析出地址"
        ))),
    }
}

/// 建链。0 跳时就是一次直连，1 跳时是「跳板机 + 一条 direct-tcpip」，
/// N 跳时每一跳都在前一条连接里再开一条通道。
///
/// `connect_timeout` 是**每一段**各一份（TCP 连、每次握手、每次开通道），
/// 与 OpenSSH 的 `ConnectTimeout` 同义：它约束的是「这一段有没有响应」，
/// 不是「整条链最多花多久」——N 跳本来就该按跳数放宽。
///
/// `forward` 只交给**最终目标**那条连接：服务端推来的 `-R` 连接属于用户，
/// 跳板机不是目标，不该往这条队列里丢东西。
pub(crate) async fn connect(
    opts: &SshOptions,
    forward: Option<ForwardSink>,
) -> Result<Handle<HostKeyHandler>, SshError> {
    if let Some(e) = agent_forward_error(opts.agent_forward, agent_socket().as_deref()) {
        return Err(e);
    }

    let targets = chain(opts);
    let Some(first) = targets.first() else {
        return Err(SshError::Config("连接链是空的".to_string()));
    };

    // 第一段是真的 TCP（可能先经 HTTP/SOCKS5 代理），之后的每一段都替换成
    // 上一条连接里的通道。
    let (first_stream, local_addr) = dial(first, opts).await?;
    // 整条链只有一条 TCP，所以每一跳报给跳板机的 originator 都是它。
    let origin = origin(local_addr);
    let mut stream: BoxStream = first_stream;
    // 已经建好、但还需要继续活着的上游连接。
    let mut upstream: Vec<Handle<HostKeyHandler>> = Vec::new();
    let last = targets.len() - 1;

    for (index, target) in targets.iter().enumerate() {
        let hop_opts = target.options(opts);
        let sink = if target.hop.is_none() {
            forward.clone()
        } else {
            None
        };
        let handle = handshake(
            stream,
            &hop_opts,
            sink,
            std::mem::take(&mut upstream),
            &target.label(),
        )
        .await?;
        if index == last {
            return Ok(handle);
        }
        // 还没有下一段可走：向跳板机要一条通往下一跳的通道，它就成了下一段。
        let next = &targets[index + 1];
        let channel = open_tunnel(&handle, next, &origin, opts.connect_timeout).await?;
        upstream.push(handle);
        stream = Box::new(channel.into_stream());
    }

    // 循环在最后一跳必然 return；能走到这里说明只可能是空链（上面已挡）。
    Err(SshError::Config("连接链是空的".to_string()))
}

/// 建好第一段能承载 SSH 的流，并报出这条 TCP 的本机地址（供 `direct-tcpip`
/// 的 originator 字段）。超时沿用会话的 `connect_timeout`。
///
/// 有出网代理时**只解析代理地址**：目标主机（或第一跳跳板机）的名字默认
/// 原样交给代理去解析（`dns_over_proxy=true`）。内网名字本地必然解析不了，
/// 先 `getaddrinfo` 一次就等于把「DNS 走代理」废掉。关掉这一档时才本地解析。
/// 每一次本地解析都走 [`resolve_host_async`]，有界、不堵住整个 async worker。
async fn dial(
    target: &Target,
    opts: &SshOptions,
) -> Result<(BoxStream, Option<SocketAddr>), SshError> {
    let timeout = stage_timeout(opts.connect_timeout);
    let Some(proxy) = opts.outbound_proxy.as_ref() else {
        let addr = resolve_host_async(&target.host, target.port, timeout).await?;
        let tcp = connect_tcp(addr, timeout, &target.label(), opts).await?;
        let local = tcp.local_addr().ok();
        return Ok((Box::new(tcp), local));
    };

    let addr = resolve_host_async(&proxy.host, proxy.port, timeout)
        .await
        .map_err(|e| {
            SshError::Resolve(format!(
                "{} 出网代理 {} —— {e}",
                proxy.kind.label(),
                proxy.label()
            ))
        })?;
    let tcp = connect_tcp(addr, timeout, &proxy.label(), opts).await?;
    let local = tcp.local_addr().ok();
    // 默认把目标主机名交给代理解析（远程 DNS，内网名字才能通）；关掉开关
    // 时在本地解析好，再把 IP 交给代理。
    let dial_host = if proxy.dns_over_proxy {
        target.host.clone()
    } else {
        resolve_host_async(&target.host, target.port, timeout)
            .await?
            .ip()
            .to_string()
    };
    let stream = outbound::handshake(tcp, proxy, &dial_host, target.port, timeout).await?;
    Ok((stream, local))
}

async fn connect_tcp(
    addr: SocketAddr,
    timeout: Duration,
    label: &str,
    opts: &SshOptions,
) -> Result<TcpStream, SshError> {
    match tokio::time::timeout(timeout, TcpStream::connect(addr)).await {
        // 会话表单这两项过去只写不读：现在真的落到第一段 TCP 上。
        Ok(Ok(socket)) => crate::tcp_tune::tune(&socket, opts.tcp_low_delay, opts.tcp_keep_alive)
            .map_err(|e| SshError::Connect(format!("{label} —— {e}")))
            .map(|()| socket),
        Ok(Err(e)) => Err(SshError::Connect(format!("{label} —— {e}"))),
        Err(_) => Err(SshError::Connect(format!(
            "{} 秒内没有连上 {label}",
            timeout.as_secs()
        ))),
    }
}

/// 在一条既有流上跑一次 SSH 握手 + 认证。
async fn handshake(
    stream: BoxStream,
    opts: &SshOptions,
    forward: Option<ForwardSink>,
    upstream: Vec<Handle<HostKeyHandler>>,
    label: &str,
) -> Result<Handle<HostKeyHandler>, SshError> {
    // 算法白名单在这里生效：清单认不出名字时**连都没连**就报配置错，
    // 而不是等到 kex 阶段被服务端拒绝（那时的错误只会说「no common algorithm」，
    // 用户看不出是自己拼错了哪一个）。
    let preferred = opts
        .algorithms
        .apply(russh::Preferred::default())
        .map_err(SshError::Config)?;
    // Key Exchange 页那两个数值框在这里生效：russh 的 `Limits` 要求字节上限
    // ≤ 1 GiB（nonce 复用边界），所以按上限夹紧；时间 0 给 1 秒而不是 0，
    // 免得握手成功的一瞬间就被要求重换密钥。
    let byte_limit = (opts.rekey_data_mbytes as usize)
        .saturating_mul(1024 * 1024)
        .clamp(1, 1usize << 30);
    let time_limit = std::time::Duration::from_secs(
        opts.rekey_time_minutes.saturating_mul(60) as u64,
    )
    .max(std::time::Duration::from_secs(1));
    let config = Arc::new(client::Config {
        // 不设「空闲就断」：终端窗口可以开一整天。只用心跳发现「其实已经断了」。
        inactivity_timeout: None,
        keepalive_interval: opts.keepalive,
        keepalive_max: 3,
        nodelay: opts.tcp_low_delay,
        preferred,
        limits: russh::Limits::new(byte_limit, byte_limit, time_limit),
        ..Default::default()
    });
    let rejection: Rejection = Arc::new(std::sync::Mutex::new(None));
    let handler = HostKeyHandler::new(opts, Arc::clone(&rejection), forward, upstream);

    let connect = client::connect_stream(config, stream, handler);
    let stage_budget = stage_timeout(opts.connect_timeout);
    let mut handle = match tokio::time::timeout(stage_budget, connect).await {
        Ok(Ok(h)) => h,
        Ok(Err(e)) => {
            // `check_server_key` 只能回 false，说不出理由；真理由记在 rejection 里。
            return Err(handler::take(&rejection)
                .unwrap_or_else(|| SshError::Connect(format!("{label}：{e}"))));
        }
        Err(_) => {
            return Err(SshError::Connect(format!(
                "{} 秒内没有完成握手：{label}",
                opts.connect_timeout.as_secs()
            )));
        }
    };

    // 认证阶段必须**自己**有预算：`connect_timeout` 只包住了 KEX（上面那次
    // `timeout`），而服务端完全可以在握手成功后对认证请求一直不回——
    // russh 的 `wait_recv_reply` 没有内建超时，`channel::connect` 又在同步等
    // `ready_rx`，于是整个 UI 线程会永远卡在「连接中」。
    //
    // 下限 30 秒：认证要用的是**已经填好的**凭据（密码 / key / 交互答案都在
    // `SshOptions` 里），没有等人输入的场景，正常握手是毫秒级。
    // `connect_timeout = 0` 在 `stage_timeout` / 这里都映射成 120 秒兜底：
    // 「不限时」放给 KEX 或认证只会变成「界面永远转圈」。
    let auth_budget = if opts.connect_timeout.is_zero() {
        NO_TIMEOUT_FALLBACK
    } else {
        opts.connect_timeout.max(Duration::from_secs(30))
    };
    match tokio::time::timeout(auth_budget, auth::authenticate(&mut handle, opts)).await {
        Ok(result) => result?,
        Err(_) => {
            return Err(SshError::Connect(format!(
                "认证阶段 {} 秒内没有结果（服务端不回应或交互轮次过多）：{label}",
                auth_budget.as_secs()
            )));
        }
    }
    Ok(handle)
}

/// 请上游开一条通往下一跳的 `direct-tcpip` 通道。
async fn open_tunnel(
    handle: &Handle<HostKeyHandler>,
    next: &Target,
    origin: &(String, u16),
    timeout: Duration,
) -> Result<russh::Channel<client::Msg>, SshError> {
    let timeout = stage_timeout(timeout);
    let request = handle.channel_open_direct_tcpip(
        &next.host,
        u32::from(next.port),
        &origin.0,
        u32::from(origin.1),
    );
    match tokio::time::timeout(timeout, request).await {
        Ok(Ok(channel)) => Ok(channel),
        Ok(Err(e)) => Err(SshError::Channel(format!(
            "跳板机不允许转发到 {}（服务端的 AllowTcpForwarding 关着？）—— {e}",
            next.label()
        ))),
        Err(_) => Err(SshError::Channel(format!(
            "{} 秒内没有开出去往 {} 的转发通道",
            timeout.as_secs(),
            next.label()
        ))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::options::{AuthMethod, ProxyHop, parse_proxy_jump};

    fn session() -> SshOptions {
        SshOptions::new(
            "target.internal",
            "alice",
            AuthMethod::Password {
                password: "x".to_string(),
            },
        )
    }

    #[test]
    fn a_zero_stage_timeout_never_means_an_immediate_timeout() {
        assert_eq!(stage_timeout(Duration::ZERO), NO_TIMEOUT_FALLBACK);
        assert_eq!(stage_timeout(Duration::from_secs(5)), Duration::from_secs(5));
    }

    #[test]
    fn no_jump_means_the_chain_is_just_the_target() {
        let targets = chain(&session());
        assert_eq!(targets.len(), 1);
        assert_eq!(targets[0].hop, None);
        assert_eq!(targets[0].user, "alice");
        assert_eq!(targets[0].host, "target.internal");
        assert_eq!(targets[0].port, DEFAULT_PORT);
    }

    #[test]
    fn hops_come_first_in_the_order_they_were_written() {
        let mut opts = session();
        opts.port = 2200;
        opts.proxy_jump = parse_proxy_jump("deploy@bastion:2222,hop2").unwrap();
        let targets = chain(&opts);

        assert_eq!(targets.len(), 3);
        assert_eq!(
            targets[0],
            Target {
                user: "deploy".into(),
                host: "bastion".into(),
                port: 2222,
                hop: Some(0)
            }
        );
        // 第二跳没写用户名：沿用会话的 alice。
        assert_eq!(
            targets[1],
            Target {
                user: "alice".into(),
                host: "hop2".into(),
                port: DEFAULT_PORT,
                hop: Some(1)
            }
        );
        // 最后才是用户真正要连的机器，端口是会话的（不是跳的默认 22）。
        assert_eq!(
            targets[2],
            Target {
                user: "alice".into(),
                host: "target.internal".into(),
                port: 2200,
                hop: None
            }
        );
    }

    #[test]
    fn a_hop_user_never_leaks_into_the_target_or_the_next_hop() {
        let mut opts = session();
        opts.proxy_jump = parse_proxy_jump("deploy@bastion,ops@hop2").unwrap();
        let targets = chain(&opts);
        assert_eq!(targets[0].user, "deploy");
        assert_eq!(targets[1].user, "ops");
        assert_eq!(targets[2].user, "alice");
    }

    #[test]
    fn an_empty_hop_user_falls_back_to_the_session_user() {
        let mut opts = session();
        opts.proxy_jump = vec![ProxyHop {
            user: None,
            host: "bastion".into(),
            port: 22,
        }];
        assert_eq!(chain(&opts)[0].user, "alice");
    }

    #[test]
    fn hop_options_are_rewritten_and_lose_the_rest_of_the_chain() {
        let mut opts = session();
        opts.proxy_jump = parse_proxy_jump("deploy@bastion:2222,hop2").unwrap();
        let hop = Target {
            user: "deploy".into(),
            host: "bastion".into(),
            port: 2222,
            hop: Some(0),
        };
        let hop_opts = hop.options(&opts);

        assert_eq!(hop_opts.user, "deploy");
        assert_eq!(hop_opts.host, "bastion");
        assert_eq!(hop_opts.port, 2222);
        // 子连接不继承整条链：链已经在这里被兑现成一条流了。
        assert!(hop_opts.proxy_jump.is_empty());
        // `ssh -A` 只对最终目标生效。
        assert!(!hop_opts.agent_forward);
        // 证书账本是同一份。
        assert_eq!(hop_opts.known_hosts, opts.known_hosts);
        assert_eq!(hop_opts.term, opts.term);
    }

    #[test]
    fn the_target_keeps_the_agent_forwarding_the_session_asked_for() {
        let mut opts = session();
        opts.agent_forward = true;
        let target = Target {
            user: "alice".into(),
            host: "target.internal".into(),
            port: 22,
            hop: None,
        };
        assert!(target.options(&opts).agent_forward);
    }

    #[test]
    fn a_hop_label_says_which_hop_it_is() {
        let hop = Target {
            user: "deploy".into(),
            host: "bastion".into(),
            port: 2222,
            hop: Some(0),
        };
        assert_eq!(hop.label(), "ProxyJump 第 1 跳 ssh://deploy@bastion:2222");

        let target = Target {
            user: "alice".into(),
            host: "target.internal".into(),
            port: 22,
            hop: None,
        };
        // 与 `SshOptions::description()` 同一套写法：默认端口不写出来。
        assert_eq!(target.label(), "ssh://alice@target.internal");
    }

    #[test]
    fn the_origin_is_the_local_side_of_the_socket() {
        let addr: SocketAddr = "10.0.0.5:51234".parse().unwrap();
        assert_eq!(origin(Some(addr)), ("10.0.0.5".to_string(), 51234));
        // 取不到就报一个无害的回环地址，而不是让转发失败。
        assert_eq!(origin(None), ("127.0.0.1".to_string(), 0));
    }

    #[test]
    fn agent_forwarding_without_an_agent_is_refused_loudly() {
        assert!(agent_forward_error(false, None).is_none());
        assert!(agent_forward_error(true, Some(Path::new("/tmp/agent.sock"))).is_none());

        let e = agent_forward_error(true, None).expect("没有 agent 时必须报错");
        match e {
            SshError::Local(msg) => assert!(msg.contains("SSH_AUTH_SOCK"), "{msg}"),
            other => panic!("应当是 Local 错误，实际是 {other:?}"),
        }
    }

    /// 接线判据：`dial` 把**第一段 TCP**交给出网代理，而且目标主机名
    /// 原样送去（不在本地解析）。`target.internal` 在本机 DNS 里不存在——
    /// 只要有人把 `resolve_host(&target.host)` 加回去，这个用例就会红。
    #[tokio::test]
    async fn dial_uses_http_proxy_and_leaves_target_dns_to_the_proxy() {
        use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
        use tokio::net::TcpListener;

        let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
        let addr = listener.local_addr().unwrap();
        let task = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut reader = BufReader::new(&mut socket);
            let mut request = String::new();
            for _ in 0..32 {
                let mut line = String::new();
                reader.read_line(&mut line).await.unwrap();
                request.push_str(&line);
                if line == "\r\n" {
                    break;
                }
            }
            socket
                .write_all(b"HTTP/1.1 200 OK\r\n\r\nSSH-2.0")
                .await
                .unwrap();
            request
        });

        let mut opts = session();
        opts.outbound_proxy = Some(crate::options::OutboundProxy {
            kind: crate::options::ProxyKind::Http,
            host: addr.ip().to_string(),
            port: addr.port(),
            auth: None,
            dns_over_proxy: true,
        });
        let first = chain(&opts).remove(0);
        let (mut stream, local) = dial(&first, &opts).await.expect("经代理建链应当成功");
        assert!(local.is_some(), "originator 仍取自这条 TCP 的本机地址");
        let request = task.await.unwrap();
        assert!(
            request.starts_with("CONNECT target.internal:22 HTTP/1.1\r\n"),
            "{request}"
        );
        // 代理转发过来的目标字节没有被 CONNECT 响应头吃掉。
        let mut banner = [0u8; 7];
        tokio::io::AsyncReadExt::read_exact(&mut stream, &mut banner)
            .await
            .unwrap();
        assert_eq!(&banner, b"SSH-2.0");
    }

    /// 同一条判据的 SOCKS5 版本：代理收到的是域名 `ATYP=3`，不是解析后的 IP。
    #[tokio::test]
    async fn dial_uses_socks5_proxy_with_a_domain_target() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        use tokio::net::TcpListener;

        let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
        let addr = listener.local_addr().unwrap();
        let task = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut greeting = [0u8; 3];
            socket.read_exact(&mut greeting).await.unwrap();
            socket.write_all(&[5, 0]).await.unwrap();
            let mut head = [0u8; 4];
            socket.read_exact(&mut head).await.unwrap();
            let mut len = [0u8; 1];
            socket.read_exact(&mut len).await.unwrap();
            let mut target = vec![0u8; len[0] as usize + 2];
            socket.read_exact(&mut target).await.unwrap();
            socket
                .write_all(&[5, 0, 0, 1, 127, 0, 0, 1, 0, 0])
                .await
                .unwrap();
            (greeting.to_vec(), head.to_vec(), target)
        });

        let mut opts = session();
        opts.outbound_proxy = Some(crate::options::OutboundProxy {
            kind: crate::options::ProxyKind::Socks5,
            host: addr.ip().to_string(),
            port: addr.port(),
            auth: None,
            dns_over_proxy: true,
        });
        let first = chain(&opts).remove(0);
        let (_stream, local) = dial(&first, &opts).await.expect("SOCKS5 建链应当成功");
        assert!(local.is_some());
        let (greeting, head, target) = task.await.unwrap();
        assert_eq!(greeting, vec![5, 1, 0]);
        assert_eq!(head, vec![5, 1, 0, 3], "ATYP=3：域名交给代理解析");
        const DOMAIN: &[u8] = b"target.internal";
        assert_eq!(&target[..DOMAIN.len()], DOMAIN);
        assert_eq!(&target[DOMAIN.len()..], &22u16.to_be_bytes());
    }

    /// 用一条确定不返回的 future 钉住超时分支：DNS 卡住时必须在预算内
    /// 得到 Resolve 错误，而不是把整条连接链永久挂住。
    #[tokio::test]
    async fn dns_resolution_is_bounded_by_the_stage_timeout() {
        let stuck = tokio::spawn(async {
            tokio::time::sleep(Duration::from_secs(60)).await;
            Ok("127.0.0.1:22".parse().unwrap())
        });
        let expired = await_resolve("example.com", 22, Duration::from_millis(30), stuck)
            .await
            .expect_err("预算一到必须返回，不能跟着解析一起卡住");
        assert!(matches!(expired, SshError::Resolve(_)), "{expired:?}");
        assert!(expired.to_string().contains("没有解析出地址"), "{expired}");

        let ok = resolve_host_async("127.0.0.1", 22, Duration::from_secs(2))
            .await
            .expect("本机回环应当解析成功");
        assert_eq!(ok.ip().to_string(), "127.0.0.1");
    }
}

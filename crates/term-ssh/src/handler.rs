//! 握手期间的唯一回调：看见服务器公钥时决定要不要继续。
//!
//! russh 的 `Handler` 是异步 trait，但这个回调里只做一次同步的文件查找——
//! `known_hosts` 不过几十 KB，读它比往返一次网络便宜得多。**不**在这里做
//! 「弹窗问用户」：那需要把 UI 句柄搬进异步线程，是这一层不该背的东西。
//! 拒绝的理由被记在 `Arc<Mutex<..>>` 里，由 `connect()` 摘出来变成 `SshError`，
//! 上层再决定是否弹一个「接受这把新钥匙吗」的对话框。

use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use russh::Channel;
use russh::client::{self, Handle};
use russh::keys::PublicKeyOrCertificate;

use crate::error::SshError;
use crate::known_hosts;
use crate::options::{HostKeyPolicy, SshOptions};

/// 记录拒绝原因的信箱：`Handler` 只能返回 `false`，说不出为什么。
pub(crate) type Rejection = Arc<Mutex<Option<SshError>>>;

/// 一把「给用户看过的」公钥在内存里最多留多久、最多留几把。
///
/// 这张表是进程级的，按 host:port 记。没有上限时，长时间运行的桌面进程每
/// 连一台新机器就多留一条；更糟的是用户确认了一个**早就过时**的 key：
/// 服务器换了钥匙之后，对话框里 pin 的仍是几小时前那把。15 分钟是「用户
/// 看完指纹、点下信任」这件交互的自然上界；64 条远大于一个人一天里会确认
/// 的主机数，只是防止无界增长。
const PRESENTED_TTL: Duration = Duration::from_secs(15 * 60);
const PRESENTED_CAP: usize = 64;

/// 最近一次收到的服务器公钥（openssh 单行），按 host:port 记。
///
/// `HostKeyProblem::Unknown` 只带指纹，不带钥匙本体；界面在用户确认后需要把
/// **用户看过的那把** pin 进 known_hosts，否则重连时 Tofu 会把「那一刻服务器
/// 给的那把」悄悄写进去（TOCTOU）。这里只记公钥，不含任何私密材料。
static PRESENTED_KEYS: Mutex<Vec<PresentedKey>> = Mutex::new(Vec::new());

struct PresentedKey {
    token: String,
    key: String,
    at: Instant,
}

fn presented_key(host: &str, port: u16) -> String {
    format!("{host}:{port}")
}

/// 先按 TTL 淘汰过期的，再按「先进先出」把超出的裁掉。
///
/// `now` 由调用方给：时间本身不可测，把它做成参数后上限与过期都能在单测里
/// 用确定的值钉住，不必 sleep。
fn prune_presented(keys: &mut Vec<PresentedKey>, now: Instant, ttl: Duration, cap: usize) {
    keys.retain(|k| now.saturating_duration_since(k.at) < ttl);
    while keys.len() > cap {
        keys.remove(0);
    }
}

fn take_from(
    keys: &mut Vec<PresentedKey>,
    token: &str,
    now: Instant,
    ttl: Duration,
    cap: usize,
) -> Option<String> {
    prune_presented(keys, now, ttl, cap);
    let at = keys.iter().position(|k| k.token == token)?;
    Some(keys.remove(at).key)
}

pub(crate) fn remember_presented(host: &str, port: u16, key_line: &str) {
    if let Ok(mut keys) = PRESENTED_KEYS.lock() {
        let now = Instant::now();
        prune_presented(&mut keys, now, PRESENTED_TTL, PRESENTED_CAP);
        let token = presented_key(host, port);
        if let Some(slot) = keys.iter_mut().find(|k| k.token == token) {
            slot.key = key_line.to_string();
            slot.at = now;
        } else {
            keys.push(PresentedKey {
                token,
                key: key_line.to_string(),
                at: now,
            });
        }
        // 插入后把容量裁回上限：进程级表不能因为「一直在连新主机」而无界长。
        prune_presented(&mut keys, now, PRESENTED_TTL, PRESENTED_CAP);
    }
}

pub(crate) fn take_presented(host: &str, port: u16) -> Option<String> {
    let token = presented_key(host, port);
    let mut keys = PRESENTED_KEYS.lock().ok()?;
    take_from(
        &mut keys,
        &token,
        Instant::now(),
        PRESENTED_TTL,
        PRESENTED_CAP,
    )
}

/// 校验主机密钥的客户端 `Handler`。
pub(crate) struct HostKeyHandler {
    host: String,
    port: u16,
    policy: HostKeyPolicy,
    known_hosts: Option<PathBuf>,
    rejection: Rejection,
    /// `-R` 用：服务端推来的 `forwarded-tcpip` 连接往这里丢。
    /// `None` = 这条连接不接远程转发（终端会话就是这样）。
    forwards: Option<crate::authenticated::ForwardSink>,
    /// `-A` 用：是否把服务端开的 agent 通道桥到本地 `$SSH_AUTH_SOCK`。
    agent_forward: bool,
    /// `-X` 用：是否把服务端开的 X11 通道桥到本地 `$DISPLAY`。
    x11_forward: bool,
    /// `ProxyJump` 用：这条连接**底下**的每一跳。
    ///
    /// 为什么放在 handler 里：`Handle` 一被 drop，那一跳的会话任务就结束，
    /// 整条链会从上往下塌掉。把它挂在最终会话的 handler 上，生命周期就正好是
    /// 「最终会话还活着」——比 `mem::forget` 或不带超时地 spawn 一个守夜任务都干净。
    /// 这里**只负责持有**，不读不写。
    _upstream: Vec<Handle<HostKeyHandler>>,
}

impl HostKeyHandler {
    pub(crate) fn new(
        opts: &SshOptions,
        rejection: Rejection,
        forwards: Option<crate::authenticated::ForwardSink>,
        upstream: Vec<Handle<HostKeyHandler>>,
    ) -> Self {
        Self {
            host: opts.host.clone(),
            port: opts.port,
            policy: opts.host_key,
            known_hosts: opts.resolved_known_hosts(),
            rejection,
            forwards,
            agent_forward: opts.agent_forward,
            x11_forward: opts.x11_forward,
            _upstream: upstream,
        }
    }
}

impl russh::client::Handler for HostKeyHandler {
    type Error = russh::Error;

    async fn check_server_key(
        &mut self,
        server_public_key: &PublicKeyOrCertificate,
    ) -> Result<bool, Self::Error> {
        // 证书模式下信任的应该是 CA，但这里先按「证书里那把公钥」比对：
        // 这样即使用户连的是一台只会给证书的服务器，TOFU 也能记下一条能核对的记录。
        let key = match server_public_key {
            PublicKeyOrCertificate::PublicKey { key, .. } => key.clone(),
            PublicKeyOrCertificate::Certificate(cert) => {
                // 证书里包着一把公钥：`known_hosts` 记的就是这一把。
                russh::keys::PublicKey::new(cert.public_key().clone(), "")
            }
        };
        // 去掉注释再记：账本里只该有 token + 公钥本体（与 verify 的 Tofu 分支一致）。
        let bare = russh::keys::PublicKey::new(key.key_data().clone(), String::new());
        remember_presented(&self.host, self.port, &bare.to_openssh().unwrap_or_default());
        match known_hosts::verify(
            self.policy,
            &self.host,
            self.port,
            self.known_hosts.as_deref(),
            &key,
        ) {
            Ok(_) => Ok(true),
            Err(e) => {
                if let Ok(mut slot) = self.rejection.lock() {
                    *slot = Some(e);
                }
                Ok(false)
            }
        }
    }

    /// `-R`：服务端替我们接受了一条外来连接，把通道交给等待的转发器。
    ///
    /// 这里**不**做连接目标的选择：往哪里转发由 `term-tunnel` 按下发的规则决定
    /// （同一个 SSH 连接上可能有好几条 `-R`，靠 `connected_address/port` 区分）。
    /// 队列满了 / 没人等了就拒收——比悄悄接受再丢掉好，服务端会立刻告诉客户端
    /// 「连接被拒」，用户看到的是「端口在但没有响应」而不是「连上就断」。
    async fn server_channel_open_forwarded_tcpip(
        &mut self,
        channel: russh::Channel<russh::client::Msg>,
        connected_address: &str,
        connected_port: u32,
        originator_address: &str,
        originator_port: u32,
        reply: russh::client::ChannelOpenHandle,
        _session: &mut russh::client::Session,
    ) -> Result<(), Self::Error> {
        let Some(sink) = self.forwards.as_ref() else {
            reply.reject(russh::ChannelOpenFailure::AdministrativelyProhibited)
                .await;
            return Ok(());
        };
        let forwarded = crate::authenticated::Forwarded {
            channel,
            connected_address: connected_address.to_string(),
            connected_port,
            originator_address: originator_address.to_string(),
            originator_port,
        };
        if sink.send(forwarded).is_err() {
            reply.reject(russh::ChannelOpenFailure::ConnectFailed).await;
            return Ok(());
        }
        reply.accept().await;
        Ok(())
    }

    /// `-A`：服务端把「远端的 `$SSH_AUTH_SOCK` 被连了」这件事送过来，
    /// 我们要把它接到**本地**的 agent 上。
    ///
    /// 做了的：接受通道 → 连本地 `$SSH_AUTH_SOCK` → 双向原样搬字节。
    /// 没做的：不缓存、不解析 agent 协议（那是 agent 与远端 ssh 之间的事，
    /// 中间人只该是根管子）；不在通道关闭后重连（远端会重新开一条）。
    ///
    /// 失败的两种情形都**不会静默**：
    /// - 没请求 `-A` 却收到这种通道 → 拒绝（`AdministrativelyProhibited`），
    ///   绝不把本地 agent 暴露给一台用户没同意过的机器；
    /// - 请求了但本地 agent 连不上 → 拒绝（`ConnectFailed`）+ 一条 warn 日志。
    ///   让远端的 `ssh` 立刻失败，比让它挂在那里等一个永远不来的签名好。
    ///   **协议层的拒绝才是主信号**（远端会打印「连不上认证代理」），日志只是给
    ///   装了 logger 的宿主多一条线索——russh 自己也只用 `log`，本 crate 不额外造轮子。
    async fn server_channel_open_agent_forward(
        &mut self,
        channel: russh::Channel<client::Msg>,
        reply: russh::client::ChannelOpenHandle,
        _session: &mut russh::client::Session,
    ) -> Result<(), Self::Error> {
        if !self.agent_forward {
            reply
                .reject(russh::ChannelOpenFailure::AdministrativelyProhibited)
                .await;
            return Ok(());
        }
        let Some(socket) = crate::proxy::agent_socket() else {
            log::warn!("收到 agent 转发通道，但本机没有 $SSH_AUTH_SOCK：已拒绝");
            reply.reject(russh::ChannelOpenFailure::ConnectFailed).await;
            return Ok(());
        };
        bridge_agent(channel, &socket, reply).await;
        Ok(())
    }

    /// `-X`：服务端把「远端的 X 客户端连了 `$DISPLAY`」这件事送过来，
    /// 我们要把它接到**本地**的 X server 上。
    ///
    /// 与 agent 转发同构：接受通道 → 按本机 `$DISPLAY` 连到本地 X server
    /// （unix socket `/tmp/.X11-unix/X{n}` 或 TCP `6000+n`）→ 双向原样搬字节。
    /// 不解析 X 协议（中间人只该是根管子）。
    ///
    /// 失败的两种情形都**不会静默**：
    /// - 没请求 `-X` 却收到这种通道 → 拒绝（`AdministrativelyProhibited`），
    ///   绝不把本地 X server 暴露给一台用户没同意过的机器；
    /// - 请求了但本机没有 X server（macOS 没装 XQuartz、`$DISPLAY` 为空）→
    ///   拒绝（`ConnectFailed`）+ 一条 warn 日志，让远端的 X 客户端立刻失败，
    ///   比让它挂在那里好。
    async fn server_channel_open_x11(
        &mut self,
        channel: russh::Channel<russh::client::Msg>,
        originator_address: &str,
        originator_port: u32,
        reply: russh::client::ChannelOpenHandle,
        _session: &mut russh::client::Session,
    ) -> Result<(), Self::Error> {
        if !self.x11_forward {
            reply
                .reject(russh::ChannelOpenFailure::AdministrativelyProhibited)
                .await;
            return Ok(());
        }
        match local_x11_target() {
            Some(X11Target::Unix(socket)) => {
                bridge_x11(channel, socket, reply).await;
                Ok(())
            }
            Some(X11Target::Tcp(host, port)) => {
                bridge_x11_tcp(channel, &host, port, reply).await;
                Ok(())
            }
            None => {
                log::warn!(
                    "收到 X11 转发通道，但本机没有可用的 $DISPLAY（没装 X server？）：已拒绝 \
                     (远端 {originator_address}:{originator_port})"
                );
                reply
                    .reject(russh::ChannelOpenFailure::ConnectFailed)
                    .await;
                Ok(())
            }
        }
    }
}

/// 把一条 agent 通道接到本地 socket 上。
#[cfg(unix)]
async fn bridge_agent(
    channel: Channel<client::Msg>,
    socket: &std::path::Path,
    reply: russh::client::ChannelOpenHandle,
) {
    match tokio::net::UnixStream::connect(socket).await {
        Ok(local) => {
            reply.accept().await;
            // 用 spawn：Handler 的回调跑在会话事件循环里，阻塞它 =
            // 这条连接上的所有通道一起卡住。这座桥要活到对端关通道。
            tokio::spawn(async move {
                let mut remote = channel.into_stream();
                let mut local = local;
                match tokio::io::copy_bidirectional(&mut remote, &mut local).await {
                    Ok((to_remote, to_local)) => {
                        log::debug!("agent 转发结束：上行 {to_remote} B，下行 {to_local} B");
                    }
                    Err(e) => log::warn!("agent 转发中断：{e}"),
                }
            });
        }
        Err(e) => {
            log::warn!("agent 转发失败：连不上本地 {}：{e}", socket.display());
            reply.reject(russh::ChannelOpenFailure::ConnectFailed).await;
        }
    }
}

/// 非 Unix：没有 `$SSH_AUTH_SOCK` 这种东西（Windows 走命名管道），拒绝并说明。
#[cfg(not(unix))]
async fn bridge_agent(
    _channel: Channel<client::Msg>,
    socket: &std::path::Path,
    reply: russh::client::ChannelOpenHandle,
) {
    log::warn!(
        "agent 转发失败：此平台不走 Unix socket（{}）",
        socket.display()
    );
    reply.reject(russh::ChannelOpenFailure::ConnectFailed).await;
}

/// 取走拒绝理由（如果有）。
pub(crate) fn take(rejection: &Rejection) -> Option<SshError> {
    rejection.lock().ok().and_then(|mut slot| slot.take())
}

/// 本机 X server 的落点：`$DISPLAY` 解析后的两种可能。
#[derive(Debug)]
enum X11Target {
    /// 文件系统 unix socket（macOS/Linux 本机 `:0`）。
    Unix(PathBuf),
    /// TCP（跨机转发或 XQuartz 的 `localhost:10.0`）。
    Tcp(String, u16),
}

/// 解析 `$DISPLAY` → 本机 X server 落点。
///
/// 语法 `[host:]display[.screen]`：
/// - 无 host（`:n` / `unix:n`）→ 文件系统 socket `/tmp/.X11-unix/X{n}`；
/// - 有 host（`localhost:10.0` / `host:0`）→ TCP `host:(6000 + n)`。
///
/// screen 段（`.m`）只用于下发的 `x11-req`，桥接时只看 display 号。
///
/// 这一层只负责取变量，解析全在 [`resolve_x11_target`] 里——见那条函数的注释：
/// 读环境变量这件事本身不可测（并行用例会互相踩），所以别把逻辑写回这里。
fn local_x11_target() -> Option<X11Target> {
    resolve_x11_target(std::env::var("DISPLAY").ok().as_deref())
}

/// 纯解析层：`$DISPLAY` 的字符串（`None` = 变量未设）→ 落点。
///
/// 为什么要把「读变量」和「解析」拆开：原先三条单测直接
/// `std::env::set_var("DISPLAY", …)`，而 `cargo test` 默认多线程跑同一个
/// 进程——`empty_display_yields_no_target` 设了空串，`local_display_with_colon`
/// 又把它改成 `:0`，于是 `local_x11_target()` 返回 `Some`，断言随机红。
/// 2026-11-13 实测就是这样红过一次（第一次全绿、隔一轮复跑挂），不是理论风险。
///
/// 拆开之后测试只喂字符串，一行 `set_var` 都不留：既没有并行竞态，
/// 也顺带覆盖到原来测不到「变量根本不存在」（`None`）这个分支。
fn resolve_x11_target(display: Option<&str>) -> Option<X11Target> {
    let display = display?.trim();
    if display.is_empty() {
        return None;
    }
    if let Some(rest) = display.strip_prefix("unix:") {
        return parse_display_number(rest)
            .map(|n| X11Target::Unix(PathBuf::from(format!("/tmp/.X11-unix/X{n}"))));
    }
    if let Some(rest) = display.strip_prefix(':') {
        return parse_display_number(rest)
            .map(|n| X11Target::Unix(PathBuf::from(format!("/tmp/.X11-unix/X{n}"))));
    }
    // 含 host：`host:n.m`
    if let Some((host, num)) = display.rsplit_once(':') {
        if let Some(n) = parse_display_number(num) {
            // `6000 + n as u16` 在 n > 57535 时溢出：debug 直接 panic，release
            // 则绕回一个端口。先用 u32 饱和加法，再确认能装进 u16——装不下就
            // 当作没有落点（拒绝通道），绝不给一个错的端口。
            let port = u32::from(n).saturating_add(6000);
            return (port <= u16::MAX as u32)
                .then(|| X11Target::Tcp(host.to_string(), port as u16));
        }
    }
    None
}

/// `display[.screen]` → display 号（忽略 screen 段，仅取第一段整数）。
fn parse_display_number(s: &str) -> Option<u32> {
    s.split('.').next()?.trim().parse::<u32>().ok()
}

/// 把一条 X11 通道接到本地 unix socket 上（与 `bridge_agent` 同构）。
#[cfg(unix)]
async fn bridge_x11(
    channel: Channel<client::Msg>,
    socket: PathBuf,
    reply: russh::client::ChannelOpenHandle,
) {
    match tokio::net::UnixStream::connect(&socket).await {
        Ok(local) => {
            reply.accept().await;
            tokio::spawn(async move {
                let mut remote = channel.into_stream();
                let mut local = local;
                match tokio::io::copy_bidirectional(&mut remote, &mut local).await {
                    Ok((to_remote, to_local)) => {
                        log::debug!("X11 转发结束：上行 {to_remote} B，下行 {to_local} B");
                    }
                    Err(e) => log::warn!("X11 转发中断：{e}"),
                }
            });
        }
        Err(e) => {
            log::warn!("X11 转发失败：连不上本地 {}：{e}", socket.display());
            reply.reject(russh::ChannelOpenFailure::ConnectFailed).await;
        }
    }
}

/// 非 Unix：本机 X11 走 TCP（WSL / 远程显示），复用同一座桥。
#[cfg(not(unix))]
async fn bridge_x11(
    _channel: Channel<client::Msg>,
    _socket: PathBuf,
    _reply: russh::client::ChannelOpenHandle,
) {
    // 非 Unix 走不到这里（`local_x11_target` 在非 unix 上只产出 `Tcp`）；
    // `unix:` / `:n` 这种形态在非 Unix 没有意义，由调用侧判空拒绝。
}

/// 把一条 X11 通道接到本地 TCP X server 上（XQuartz 的 `localhost:10.0` 等）。
async fn bridge_x11_tcp(
    channel: Channel<client::Msg>,
    host: &str,
    port: u16,
    reply: russh::client::ChannelOpenHandle,
) {
    match tokio::net::TcpStream::connect((host, port)).await {
        Ok(local) => {
            reply.accept().await;
            tokio::spawn(async move {
                let mut remote = channel.into_stream();
                let mut local = local;
                match tokio::io::copy_bidirectional(&mut remote, &mut local).await {
                    Ok((to_remote, to_local)) => {
                        log::debug!("X11 转发结束：上行 {to_remote} B，下行 {to_local} B");
                    }
                    Err(e) => log::warn!("X11 转发中断：{e}"),
                }
            });
        }
        Err(e) => {
            log::warn!("X11 转发失败：连不上本地 {host}:{port}：{e}");
            reply
                .reject(russh::ChannelOpenFailure::ConnectFailed)
                .await;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn presented(token: &str, at: Instant) -> PresentedKey {
        PresentedKey {
            token: token.to_string(),
            key: format!("key-{token}"),
            at,
        }
    }

    /// 进程级表必须同时有容量与过期两道上界：既不能无界增长，也不能让用户
    /// 在几小时后还 pin 住一次早已过时的握手结果。
    #[test]
    fn presented_keys_are_capped_and_expire() {
        let now = Instant::now();
        let mut keys: Vec<PresentedKey> = (0..PRESENTED_CAP + 3)
            .map(|i| presented(&format!("h{i}:22"), now))
            .collect();
        prune_presented(&mut keys, now, PRESENTED_TTL, PRESENTED_CAP);
        assert_eq!(keys.len(), PRESENTED_CAP);
        // 淘汰的是最旧的那几条，最近握手的仍留着。
        assert!(!keys.iter().any(|k| k.token == "h0:22"));
        assert!(keys.iter().any(|k| k.token == "h66:22"));

        // 过期：即使还在容量内，TTL 一到也不该再交出去。
        let stale = now
            .checked_sub(PRESENTED_TTL + Duration::from_secs(1))
            .expect("测试用的时刻应当还能往前退");
        let mut keys = vec![presented("old:22", stale), presented("fresh:22", now)];
        assert!(take_from(
            &mut keys,
            "old:22",
            now,
            PRESENTED_TTL,
            PRESENTED_CAP
        )
        .is_none());
        assert_eq!(
            take_from(&mut keys, "fresh:22", now, PRESENTED_TTL, PRESENTED_CAP),
            Some("key-fresh:22".to_string())
        );
    }

    #[test]
    fn local_display_with_colon_resolves_to_unix_socket() {
        let target = resolve_x11_target(Some(":0")).expect("`:0` 应解析出落点");
        match target {
            X11Target::Unix(path) => assert_eq!(path, PathBuf::from("/tmp/.X11-unix/X0")),
            X11Target::Tcp(..) => panic!("`:0` 应走 unix socket，不是 TCP"),
        }

        let target = resolve_x11_target(Some(":10.0")).expect("`:10.0` 应解析出落点");
        match target {
            X11Target::Unix(path) => assert_eq!(path, PathBuf::from("/tmp/.X11-unix/X10")),
            X11Target::Tcp(..) => panic!("`:10.0` 应走 unix socket，不是 TCP"),
        }
    }

    #[test]
    fn local_display_with_host_resolves_to_tcp() {
        let target =
            resolve_x11_target(Some("localhost:10.0")).expect("`localhost:10.0` 应解析出落点");
        match target {
            X11Target::Tcp(host, port) => {
                assert_eq!(host, "localhost");
                assert_eq!(port, 6010);
            }
            X11Target::Unix(..) => panic!("带 host 的 DISPLAY 应走 TCP（6000 + display）"),
        }
    }

    #[test]
    fn empty_display_yields_no_target() {
        // 空串是「设了但没值」，`None` 是「变量根本不存在」——两种都要塌成无落点。
        // `None` 这一支原先测不到：那会儿只能靠 `set_var("")` 间接模拟，
        // 现在拆成纯函数后才第一次真正覆盖（见 `resolve_x11_target` 的注释）。
        assert!(resolve_x11_target(Some("")).is_none());
        assert!(resolve_x11_target(Some("   ")).is_none());
        assert!(resolve_x11_target(None).is_none());
    }

    #[test]
    fn unix_prefix_and_junk_display_are_handled() {
        // `unix:n` 是 X 的另一种写法，与兼容终端 `:n` 同落点。
        match resolve_x11_target(Some("unix:3")) {
            Some(X11Target::Unix(path)) => assert_eq!(path, PathBuf::from("/tmp/.X11-unix/X3")),
            Some(X11Target::Tcp(..)) => panic!("`unix:3` 应走 unix socket，不是 TCP"),
            None => panic!("`unix:3` 应解析出落点，实际无"),
        }
        // 没有冒号的串、display 号不是数字 —— 一律无落点，不许 panic。
        assert!(resolve_x11_target(Some("nonsense")).is_none());
        assert!(resolve_x11_target(Some("localhost:abc")).is_none());
    }

    /// 6000 + display 溢出：以前 `n as u16` 会绕回/溢出，现在必须判无落点。
    #[test]
    fn an_oversized_display_number_never_overflows_into_a_port() {
        assert!(resolve_x11_target(Some("localhost:59536")).is_none());
        assert!(resolve_x11_target(Some("localhost:4294967295")).is_none());
        match resolve_x11_target(Some("localhost:59535")) {
            Some(X11Target::Tcp(host, port)) => {
                assert_eq!(host, "localhost");
                assert_eq!(port, u16::MAX);
            }
            other => panic!("边界值应当还能用：{other:?}"),
        }
    }
}

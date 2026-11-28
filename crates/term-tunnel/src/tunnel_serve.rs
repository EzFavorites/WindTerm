//! 每条连接的服务端：`-L` 本地监听、`-D` SOCKS5、`-R` 远端推入，
//! 以及 SSH 通道 ↔ TCP 的双向搬字节与半包读取。
//!
//! 从 `lib.rs` 拆出：这里只管「一条连接怎么搬」。建监听 / 起任务 /
//! `Authenticated::connect` 的编排仍留在 `lib.rs`——后者也是 4k 的接线判据。

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use russh::Channel;
use russh::ChannelMsg;
use term_ssh::{ChannelOpener, Forwarded};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::Notify;

use crate::socks5;
use crate::Target;

/// 收到远端 `Eof` 之后等 `Close` / 末尾数据的短兜底（见 `pump`）。
const EOF_GRACE: std::time::Duration = std::time::Duration::from_secs(2);

/// 监听任务退出的统一收尾：先把自己这条旗记死，再叫醒主任务。
///
/// 顺序很要紧：主任务被叫醒后走 `alive.close()`，若先 notify 后 store，
/// 它会看到一条还「活着」的旗。反过来没问题——`is_alive()` 此刻已经为假，
/// 主任务醒来就撤 `-R`、挂断 SSH，而不是抱着一条已死的隧道不释放。
pub(crate) fn listener_gone(alive: &AtomicBool, stop: &Notify) {
    alive.store(false, Ordering::Relaxed);
    stop.notify_one();
}

/// `-L`：本地监听，每条进来的连接开一条 `direct-tcpip` 通道。
pub(crate) async fn serve_local(
    listener: TcpListener,
    target: Target,
    opener: ChannelOpener,
    alive: Arc<AtomicBool>,
    stop: Arc<Notify>,
) {
    loop {
        let Ok((tcp, peer)) = listener.accept().await else {
            listener_gone(&alive, &stop);
            return;
        };
        let opener = opener.clone();
        let target = target.clone();
        tokio::spawn(async move {
            let peer_ip = peer.ip().to_string();
            match opener.direct_tcpip(&target.host, target.port, (&peer_ip, peer.port())).await {
                Ok(channel) => pump(channel, tcp).await,
                // 服务器拒绝（目标不可达 / 不允许转发）：直接关掉本地这条连接。
                // 不往 TCP 里写任何东西——这不是应用层协议，没有「错误帧」可发。
                Err(_) => drop(tcp),
            }
        });
    }
}

/// 从 `reader` 读到 `parse` 认为「消息完整」为止。
///
/// greeting 与 request 都可能半包到达——客户端把 VER、NMETHODS、方法列表
/// 拆到三个 TCP 段是常态，只读一次就把 `Short` 当协议错误是在制造假失败。
/// 两阶段共用这一条 `filled` 循环：先解析已有字节，`Short` 才继续读；
/// 真正的协议错误原样交回调用方回失败帧。
pub(crate) async fn read_complete<R, T, F>(
    reader: &mut R,
    buf: &mut [u8],
    filled: &mut usize,
    parse: F,
) -> Result<(T, usize), socks5::Socks5Error>
where
    R: AsyncRead + Unpin,
    F: Fn(&[u8]) -> Result<(T, usize), socks5::Socks5Error>,
{
    loop {
        match parse(&buf[..*filled]) {
            Ok(parsed) => return Ok(parsed),
            Err(socks5::Socks5Error::Short) => {}
            Err(e) => return Err(e),
        }
        if *filled == buf.len() {
            return Err(socks5::Socks5Error::Short);
        }
        match reader.read(&mut buf[*filled..]).await {
            Ok(0) | Err(_) => return Err(socks5::Socks5Error::Short),
            Ok(n) => *filled += n,
        }
    }
}

/// `-D`：SOCKS5，只做 `CONNECT`。
pub(crate) async fn serve_socks5(
    listener: TcpListener,
    opener: ChannelOpener,
    alive: Arc<AtomicBool>,
    stop: Arc<Notify>,
) {
    loop {
        let Ok((mut tcp, peer)) = listener.accept().await else {
            listener_gone(&alive, &stop);
            return;
        };
        let opener = opener.clone();
        tokio::spawn(async move {
            use socks5::{failure_reply, greeting_reply, read_greeting, read_request, success_reply};
            let mut buf = [0u8; 512];
            let mut filled = 0usize;

            // 1. 问候：半包也继续读，不再「只读一次」。
            let no_auth = match read_complete(&mut tcp, &mut buf, &mut filled, read_greeting).await {
                Ok((no_auth, used)) => {
                    // 客户端可能把问候和请求塞进同一个段：把问候没吃掉的字节
                    // 挪到缓冲开头留给下一阶段，绝不丢字节。
                    buf.copy_within(used..filled, 0);
                    filled -= used;
                    no_auth
                }
                Err(_) => {
                    let _ = tcp.write_all(&greeting_reply(false)).await;
                    return;
                }
            };
            if tcp.write_all(&greeting_reply(no_auth)).await.is_err() || !no_auth {
                return;
            }

            // 2. 请求：复用同一块缓冲与 filled，半包继续读到完整为止。
            let req = match read_complete(&mut tcp, &mut buf, &mut filled, read_request).await {
                Ok((req, _)) => req,
                Err(e) => {
                    let code = match e {
                        socks5::Socks5Error::UnsupportedCommand(_) => {
                            socks5::REP_COMMAND_NOT_SUPPORTED
                        }
                        socks5::Socks5Error::UnsupportedAddress(_) => {
                            socks5::REP_ADDRESS_NOT_SUPPORTED
                        }
                        _ => socks5::REP_GENERAL_FAILURE,
                    };
                    let _ = tcp.write_all(&failure_reply(code)).await;
                    return;
                }
            };

            // 3. 开通道并回「成功」，然后原样搬字节。
            let peer_ip = peer.ip().to_string();
            match opener.direct_tcpip(&req.host, req.port, (&peer_ip, peer.port())).await {
                Ok(channel) => {
                    if tcp.write_all(&success_reply()).await.is_ok() {
                        pump(channel, tcp).await;
                    }
                }
                Err(_) => {
                    let _ = tcp.write_all(&failure_reply(socks5::REP_HOST_UNREACHABLE)).await;
                }
            }
        });
    }
}

/// `-R`：服务器推来的每条连接，回到本机连 `target`。
pub(crate) async fn serve_remote(
    mut forwarded: tokio::sync::mpsc::UnboundedReceiver<Forwarded>,
    rules: Vec<(String, u16, Target)>,
    opener: ChannelOpener,
    alive: Arc<AtomicBool>,
    stop: Arc<Notify>,
) {
    while let Some(conn) = forwarded.recv().await {
        let Some(target) = pick_target(&rules, conn.connected_port) else {
            // 没有匹配的规则：这条连接不该存在（我们没请求过这个端口）。
            let _ = conn.channel.close().await;
            continue;
        };
        tokio::spawn(async move {
            match TcpStream::connect((target.host.as_str(), target.port)).await {
                Ok(tcp) => pump(conn.channel, tcp).await,
                Err(_) => {
                    let _ = conn.channel.close().await;
                }
            }
        });
    }
    let _ = opener;
    // 服务端不再推连接 = 这条 `-R` 规则已经死了。同样要叫醒主任务收摊，
    // 否则 SSH 连接会一直留着。
    listener_gone(&alive, &stop);
}

/// 服务端报回来的 `connected_address/port` → 该走哪条规则。
///
/// 按端口匹配：服务器常常把 `0.0.0.0` 或 `localhost` 报成别的写法，
/// 端口是唯一稳定的东西。只有一条 `-R` 时端口也一定对得上（就是我们请求的那个）。
pub(crate) fn pick_target(rules: &[(String, u16, Target)], connected_port: u32) -> Option<Target> {
    rules
        .iter()
        .find(|(_, port, _)| u32::from(*port) == connected_port)
        .map(|(_, _, target)| target.clone())
        .or_else(|| {
            // 兜底只对「服务端把端口报成 0」生效，且只在只有一条规则时——
            // 报了一个我们没请求过的端口，说明这条连接不该被转发，
            // 硬套到唯一那条规则上就是把流量送到错误的地方。
            if connected_port == 0 && rules.len() == 1 {
                Some(rules[0].2.clone())
            } else {
                None
            }
        })
}

/// SSH 通道 ↔ TCP 连接，双向搬字节。
///
/// 任一方向结束就收摊：半开的转发连接没有用处，还会一直占着服务器那边的窗口。
async fn pump(mut channel: Channel<russh::client::Msg>, tcp: TcpStream) {
    let (mut read_half, mut write_half) = tcp.into_split();
    let mut buf = vec![0u8; 16 * 1024];
    // 本地读端到 EOF 之后就要**停止再 select 它**：已 EOF 的 socket 每次
    // `read` 都立刻返回 `Ok(0)`，留着这一臂等于把 loop 变成空转——CPU 打满，
    // 而且每个循环都往 SSH 通道灌一条 EOF。远端那一侧还没收尾时（回应还在
    // 路上），我们又不能直接 break，所以只撤掉这一臂，继续转发反方向。
    let mut local_eof = false;
    // 远端 Eof 只是半关闭：最后一批数据可能还在路上。一收到就 break 会把
    // 反向数据吞掉；再给一段短兜底超时，服务端只发 Eof 也不至于挂死。
    let mut remote_eof = false;
    loop {
        tokio::select! {
            msg = async {
                let waiting = channel.wait();
                if remote_eof {
                    match tokio::time::timeout(EOF_GRACE, waiting).await {
                        Ok(msg) => msg,
                        Err(_) => Some(ChannelMsg::Close),
                    }
                } else {
                    waiting.await
                }
            } => match msg {
                Some(ChannelMsg::Data { data }) => {
                    if write_half.write_all(&data).await.is_err() {
                        break;
                    }
                }
                Some(ChannelMsg::ExtendedData { data, .. }) => {
                    if write_half.write_all(&data).await.is_err() {
                        break;
                    }
                }
                Some(ChannelMsg::Eof) => remote_eof = true,
                Some(ChannelMsg::Close) | None => break,
                _ => {}
            },
            n = read_half.read(&mut buf), if !local_eof => match n {
                Ok(0) => {
                    // 本地写完了：告诉远端 EOF，但别急着关——回应可能还在路上。
                    let _ = channel.eof().await;
                    local_eof = true;
                }
                Ok(n) => {
                    if channel.data(&buf[..n]).await.is_err() {
                        break;
                    }
                }
                Err(_) => break,
            },
        }
    }
    let _ = write_half.shutdown().await;
    let _ = channel.eof().await;
    let _ = channel.close().await;
}

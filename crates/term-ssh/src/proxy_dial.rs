//! 连接链的第一段：本地 TCP，或经出网代理（HTTP / SOCKS5）的隧道。

use std::net::SocketAddr;
use std::time::Duration;

use tokio::net::TcpStream;

use crate::error::SshError;
use crate::options::SshOptions;
use crate::outbound;
use crate::proxy::{stage_timeout, BoxStream, Target};
use crate::proxy_resolve::resolve_host_async;

/// 建好第一段能承载 SSH 的流，并报出这条 TCP 的本机地址（供 `direct-tcpip`
/// 的 originator 字段）。超时沿用会话的 `connect_timeout`。
///
/// 有出网代理时**只解析代理地址**：目标主机（或第一跳跳板机）的名字默认
/// 原样交给代理去解析（`dns_over_proxy=true`）。内网名字本地必然解析不了，
/// 先 `getaddrinfo` 一次就等于把「DNS 走代理」废掉。关掉这一档时才本地解析。
/// 每一次本地解析都走 [`resolve_host_async`]，有界、不堵住整个 async worker。
pub(crate) async fn dial(
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::options::{AuthMethod, ProxyKind};
    use crate::proxy::chain;

    fn session() -> SshOptions {
        SshOptions::new(
            "target.internal",
            "alice",
            AuthMethod::Password {
                password: "x".to_string(),
            },
        )
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
            kind: ProxyKind::Http,
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
            kind: ProxyKind::Socks5,
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
}

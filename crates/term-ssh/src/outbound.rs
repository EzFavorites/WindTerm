//! 出网代理：HTTP CONNECT 与 SOCKS5 的**协议本体**。
//!
//! 官方公开文档 `2021/03/11/proxy-http-socks5/index.md` 把参考实现的行为写得很直白：
//! HTTP 代理与 SOCKS5 代理都是真功能，SOCKS5 那一档等价于
//! `/usr/bin/nc -X 5 -x 127.0.0.1:7777 %h %p`，并且「DNS 可以走代理」。
//! 这一层就是把这两句话落成字节：
//!
//! ```text
//! TCP ──▶ 代理 ──CONNECT / SOCKS5 CONNECT──▶ 目标主机
//!          │                                   ▲
//!          └── 之后这条流上跑的是原样的 SSH ────┘
//! ```
//!
//! 三条容易写错、因此在这里写成规则的地方：
//!
//! 1. **目标主机名不在本地解析。** 只解析代理的地址；目标 `host:port` 原样
//!    交给代理（SOCKS5 用 `ATYP=3` 域名，HTTP 用 `CONNECT` 的请求目标）。
//!    内网名字只有代理能解析，这正是出网代理的用途；本地先 `getaddrinfo`
//!    一次就把它废了。
//! 2. **HTTP 响应头之后可能已经带着目标的首批字节。** 逐行读到空行时
//!    `BufReader` 里往往还压着 `SSH-2.0-…`，把 TCP 裸流交上去就永久丢掉
//!    第一次握手标识。所以 HTTP 这条路返回的是 `BufReader<TcpStream>` 本身
//!    （它同时实现 `AsyncRead + AsyncWrite`），缓冲原封不动。
//! 3. **每一段都有超时。** 代理不回话时不能把 UI 卡在「Connecting…」，
//!    握手整体（含所有来回）由调用方的 `connect_timeout` 兜住。
//!
//! 认证：无认证与用户名密码两种形态都实现（官文 `:31`）。凭据由调用方给，
//! 本层不读环境、不读保险库、不落盘。

use std::net::{Ipv4Addr, Ipv6Addr};
use std::time::Duration;

use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::net::TcpStream;

use crate::error::SshError;
use crate::options::{OutboundProxy, ProxyKind};
use crate::proxy::BoxStream;

/// HTTP 响应头最多读这么多：代理坏了也不能让它把内存吃光。
const MAX_HEADER_BYTES: usize = 16 * 1024;
/// 头最多这么多行。恶意/坏代理一直发不含空行的头时，靠这两条一起收口。
const MAX_HEADER_LINES: usize = 64;
const SOCKS_VERSION: u8 = 0x05;
const SOCKS_AUTH_NONE: u8 = 0x00;
const SOCKS_AUTH_USERPASS: u8 = 0x02;
const SOCKS_NO_ACCEPTABLE_METHOD: u8 = 0xff;
const SOCKS_CMD_CONNECT: u8 = 0x01;
const SOCKS_ATYP_V4: u8 = 0x01;
const SOCKS_ATYP_DOMAIN: u8 = 0x03;
const SOCKS_ATYP_V6: u8 = 0x04;

/// 在一条**已经连到代理**的 TCP 上完成协商，返回可直接跑 SSH 的字节流。
///
/// `target_host` 不解析、不改写：代理要收到的就是用户在会话里填的那个名字。
pub(crate) async fn handshake(
    socket: TcpStream,
    proxy: &OutboundProxy,
    target_host: &str,
    target_port: u16,
    timeout: Duration,
) -> Result<BoxStream, SshError> {
    let negotiate = negotiate(socket, proxy, target_host, target_port);
    match tokio::time::timeout(timeout, negotiate).await {
        Ok(result) => result,
        Err(_) => Err(SshError::Connect(format!(
            "{} 秒内没有完成与 {} 代理 {} :{} 的协商",
            timeout.as_secs(),
            proxy.kind.label(),
            proxy.host,
            proxy.port
        ))),
    }
}

async fn negotiate(
    socket: TcpStream,
    proxy: &OutboundProxy,
    host: &str,
    port: u16,
) -> Result<BoxStream, SshError> {
    match proxy.kind {
        ProxyKind::Http => http_connect(socket, proxy, host, port).await,
        ProxyKind::Socks5 => socks5_connect(socket, proxy, host, port).await,
    }
}

/// HTTP `CONNECT`：把这条 TCP 变成一条通往目标的裸隧道。
async fn http_connect(
    socket: TcpStream,
    proxy: &OutboundProxy,
    host: &str,
    port: u16,
) -> Result<BoxStream, SshError> {
    let authority = format!("{host}:{port}");
    let mut request = format!(
        "CONNECT {authority} HTTP/1.1\r\nHost: {authority}\r\nProxy-Connection: Keep-Alive\r\n"
    );
    if let Some(auth) = &proxy.auth {
        let credentials = format!("{}:{}", auth.username, auth.password);
        request.push_str(&format!(
            "Proxy-Authorization: Basic {}\r\n",
            base64(credentials.as_bytes())
        ));
    }
    request.push_str("\r\n");

    let mut reader = BufReader::new(socket);
    reader
        .write_all(request.as_bytes())
        .await
        .map_err(|e| SshError::Connect(format!("HTTP 代理 {} 写 CONNECT 失败：{e}", proxy.label())))?;
    reader
        .flush()
        .await
        .map_err(|e| SshError::Connect(format!("HTTP 代理 {} 写 CONNECT 失败：{e}", proxy.label())))?;

    // 只吃「状态行 + 头」，把空行之后的一切留给 SSH。BufReader 的读缓冲
    // 会原样交给上层，所以目标在 200 之后立刻发来的那几个字节不会丢。
    let mut status: Option<String> = None;
    let mut seen = 0usize;
    for _ in 0..MAX_HEADER_LINES {
        let mut line = String::new();
        let n = reader
            .read_line(&mut line)
            .await
            .map_err(|e| SshError::Connect(format!("HTTP 代理 {} 的响应读不出来：{e}", proxy.label())))?;
        if n == 0 {
            return Err(SshError::Connect(format!(
                "HTTP 代理 {} 在回完响应之前就关了连接",
                proxy.label()
            )));
        }
        seen += n;
        if seen > MAX_HEADER_BYTES {
            return Err(SshError::Connect(format!(
                "HTTP 代理 {} 的响应头超过 {MAX_HEADER_BYTES} 字节",
                proxy.label()
            )));
        }
        if status.is_none() {
            status = Some(line.trim_end().to_string());
        }
        if line == "\r\n" || line == "\n" {
            let status = status.unwrap_or_default();
            let code = status
                .split_whitespace()
                .nth(1)
                .unwrap_or_default()
                .parse::<u16>()
                .unwrap_or(0);
            if !(200..300).contains(&code) || !status.starts_with("HTTP/1.") {
                return Err(SshError::Connect(format!(
                    "HTTP 代理 {} 拒绝 CONNECT {authority}：{status}",
                    proxy.label()
                )));
            }
            return Ok(Box::new(reader));
        }
    }
    Err(SshError::Connect(format!(
        "HTTP 代理 {} 的响应头没有结束（超过 {MAX_HEADER_LINES} 行）",
        proxy.label()
    )))
}

/// SOCKS5（RFC 1928）：方法协商 → 可选的用户名密码（RFC 1929）→ CONNECT。
///
/// 读回复时**必须**把 `BND.ADDR` 整段吃掉：代理把地址和端口写在
/// `VER REP RSV ATYP` 之后，只 `read_exact(4)` 就把这几个字节留在了流里，
/// SSH 握手会读到一个 `0x05` 开头然后莫名其妙地失败。
async fn socks5_connect(
    socket: TcpStream,
    proxy: &OutboundProxy,
    host: &str,
    port: u16,
) -> Result<BoxStream, SshError> {
    let mut socket = socket;
    let method = if proxy.auth.is_some() {
        SOCKS_AUTH_USERPASS
    } else {
        SOCKS_AUTH_NONE
    };
    socket
        .write_all(&[SOCKS_VERSION, 1, method])
        .await
        .map_err(|e| SshError::Connect(format!("SOCKS5 代理 {} 写握手失败：{e}", proxy.label())))?;

    let mut choice = [0u8; 2];
    read_exact_checked(&mut socket, &mut choice, proxy).await?;
    if choice[0] != SOCKS_VERSION {
        return Err(SshError::Connect(format!(
            "SOCKS5 代理 {} 回的不是 SOCKS5：版本 {}",
            proxy.label(),
            choice[0]
        )));
    }
    if choice[1] == SOCKS_NO_ACCEPTABLE_METHOD {
        return Err(SshError::Connect(format!(
            "SOCKS5 代理 {} 不接受{}认证方式",
            proxy.label(),
            if method == SOCKS_AUTH_USERPASS {
                "用户名密码"
            } else {
                "无认证"
            }
        )));
    }
    if choice[1] != method {
        return Err(SshError::Connect(format!(
            "SOCKS5 代理 {} 选了方法 {}，与客户端请求的 {} 不一致",
            proxy.label(),
            choice[1],
            method
        )));
    }

    if let Some(auth) = &proxy.auth {
        let user = auth.username.as_bytes();
        let pass = auth.password.as_bytes();
        if user.len() > 255 || pass.len() > 255 {
            return Err(SshError::Config(
                "SOCKS5 代理的用户名或密码超过 255 字节".to_string(),
            ));
        }
        let mut sub = vec![0x01, user.len() as u8];
        sub.extend_from_slice(user);
        sub.push(pass.len() as u8);
        sub.extend_from_slice(pass);
        socket
            .write_all(&sub)
            .await
            .map_err(|e| SshError::Connect(format!("SOCKS5 代理 {} 写认证失败：{e}", proxy.label())))?;
        let mut answer = [0u8; 2];
        read_exact_checked(&mut socket, &mut answer, proxy).await?;
        if answer[0] != 0x01 || answer[1] != 0 {
            return Err(SshError::Connect(format!(
                "SOCKS5 代理 {} 拒绝了用户名密码（状态 {}）",
                proxy.label(),
                answer[1]
            )));
        }
    }

    let (atyp, address) = encode_target(host)?;
    let mut request = vec![SOCKS_VERSION, SOCKS_CMD_CONNECT, 0x00, atyp];
    request.extend_from_slice(&address);
    request.extend_from_slice(&port.to_be_bytes());
    socket
        .write_all(&request)
        .await
        .map_err(|e| SshError::Connect(format!("SOCKS5 代理 {} 写 CONNECT 失败：{e}", proxy.label())))?;

    let mut head = [0u8; 4];
    read_exact_checked(&mut socket, &mut head, proxy).await?;
    if head[0] != SOCKS_VERSION {
        return Err(SshError::Connect(format!(
            "SOCKS5 代理 {} 的回复版本不对：{}",
            proxy.label(),
            head[0]
        )));
    }
    if head[1] != 0 {
        return Err(SshError::Connect(format!(
            "SOCKS5 代理 {} 拒绝 CONNECT {host}:{port}：{} (REP=0x{:02x})",
            proxy.label(),
            socks_reply_text(head[1]),
            head[1],
        )));
    }
    // 把 BND.ADDR 与 BND.PORT 读完，别把它们留给 SSH 握手。
    let addr_len = match head[3] {
        SOCKS_ATYP_V4 => 4usize,
        SOCKS_ATYP_V6 => 16usize,
        SOCKS_ATYP_DOMAIN => {
            let mut len = [0u8; 1];
            read_exact_checked(&mut socket, &mut len, proxy).await?;
            len[0] as usize
        }
        other => {
            return Err(SshError::Connect(format!(
                "SOCKS5 代理 {} 回复了不认识的地址类型 {other}",
                proxy.label()
            )));
        }
    };
    let mut tail = vec![0u8; addr_len + 2];
    read_exact_checked(&mut socket, &mut tail, proxy).await?;
    Ok(Box::new(socket))
}

async fn read_exact_checked(
    socket: &mut TcpStream,
    buf: &mut [u8],
    proxy: &OutboundProxy,
) -> Result<(), SshError> {
    socket
        .read_exact(buf)
        .await
        .map(|_| ())
        .map_err(|e| {
            SshError::Connect(format!(
                "SOCKS5 代理 {} 的回复不完整：{e}",
                proxy.label()
            ))
        })
}

/// SOCKS5 的目标地址。IP 字面量按地址类型发，其余一律当**域名**发给代理，
/// 由代理去做 DNS —— 这正是「DNS 可走代理」。
fn encode_target(host: &str) -> Result<(u8, Vec<u8>), SshError> {
    if let Ok(v4) = host.parse::<Ipv4Addr>() {
        return Ok((SOCKS_ATYP_V4, v4.octets().to_vec()));
    }
    if let Ok(v6) = host.parse::<Ipv6Addr>() {
        return Ok((SOCKS_ATYP_V6, v6.octets().to_vec()));
    }
    let bytes = host.as_bytes();
    if bytes.is_empty() || bytes.len() > 255 {
        return Err(SshError::Config(format!(
            "SOCKS5 目标主机名长度不合法（{host:?}）：必须是 1–255 字节"
        )));
    }
    let mut out = vec![bytes.len() as u8];
    out.extend_from_slice(bytes);
    Ok((SOCKS_ATYP_DOMAIN, out))
}

/// RFC 1928 的错误码 → 人能采取行动的话。
fn socks_reply_text(rep: u8) -> &'static str {
    match rep {
        0x01 => "一般性故障",
        0x02 => "规则不允许连接",
        0x03 => "网络不可达",
        0x04 => "主机不可达",
        0x05 => "连接被拒绝",
        0x06 => "TTL 过期",
        0x07 => "不支持的命令",
        0x08 => "不支持的地址类型",
        _ => "未知错误",
    }
}

/// RFC 4648 的 base64，只为 HTTP `Proxy-Authorization: Basic`。
///
/// 不引第三方 crate：这是整个仓库唯一需要它的地方，而它只有 20 行，
/// 加一个依赖换来的是供应链与离线构建两边都要多操一份心。
fn base64(input: &[u8]) -> String {
    const TABLE: &[u8; 64] =
        b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(input.len().div_ceil(3) * 4);
    for chunk in input.chunks(3) {
        let b0 = chunk[0] as u32;
        let b1 = chunk.get(1).copied().unwrap_or(0) as u32;
        let b2 = chunk.get(2).copied().unwrap_or(0) as u32;
        let word = (b0 << 16) | (b1 << 8) | b2;
        out.push(TABLE[((word >> 18) & 0x3f) as usize] as char);
        out.push(TABLE[((word >> 12) & 0x3f) as usize] as char);
        out.push(if chunk.len() > 1 {
            TABLE[((word >> 6) & 0x3f) as usize] as char
        } else {
            '='
        });
        out.push(if chunk.len() > 2 {
            TABLE[(word & 0x3f) as usize] as char
        } else {
            '='
        });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::options::ProxyAuth;

    const SHORT_TIMEOUT: Duration = Duration::from_secs(3);

    fn proxy(kind: ProxyKind, addr: std::net::SocketAddr, auth: Option<ProxyAuth>) -> OutboundProxy {
        OutboundProxy {
            kind,
            host: addr.ip().to_string(),
            port: addr.port(),
            auth,
        }
    }

    async fn listener() -> tokio::net::TcpListener {
        tokio::net::TcpListener::bind(("127.0.0.1", 0))
            .await
            .expect("bind loopback")
    }

    #[test]
    fn base64_matches_the_rfc_example() {
        assert_eq!(base64(b""), "");
        assert_eq!(base64(b"f"), "Zg==");
        assert_eq!(base64(b"fo"), "Zm8=");
        assert_eq!(base64(b"foo"), "Zm9v");
        assert_eq!(
            base64(b"Aladdin:open sesame"),
            "QWxhZGRpbjpvcGVuIHNlc2FtZQ=="
        );
    }

    #[test]
    fn target_addresses_are_encoded_without_local_dns() {
        assert_eq!(encode_target("10.1.2.3").unwrap(), (1, vec![10, 1, 2, 3]));
        let (atyp, addr) = encode_target("::1").unwrap();
        assert_eq!(atyp, 4);
        assert_eq!(addr.len(), 16);
        assert_eq!(addr[15], 1);
        let (atyp, addr) = encode_target("db.internal").unwrap();
        assert_eq!(atyp, 3);
        assert_eq!(addr[0], 11);
        assert_eq!(&addr[1..], b"db.internal");
        assert!(encode_target("").is_err());
        assert!(encode_target(&"x".repeat(256)).is_err());
    }

    #[test]
    fn every_socks_reply_code_has_a_human_reading() {
        assert_eq!(socks_reply_text(0x05), "连接被拒绝");
        assert_eq!(socks_reply_text(0xfe), "未知错误");
    }

    /// 模拟代理侧读一条 SOCKS5 CONNECT 请求：4 字节头 + 目标地址 + 端口。
    ///
    /// `ATYP=3` 时**域名长度是第 5 个字节**，不是头里的 `ATYP` 字段；
    /// 把两者搞混正是第一次跑红的原因（夹具少读了/多读了目标字节）。
    async fn read_socks_request(socket: &mut TcpStream) -> ([u8; 4], Vec<u8>) {
        let mut head = [0u8; 4];
        socket.read_exact(&mut head).await.unwrap();
        let addr_len = match head[3] {
            1 => 4usize,
            4 => 16usize,
            3 => {
                // 长度字节已经在上面单独读过：target 里只剩「域名 + 端口」。
                let mut len = [0u8; 1];
                socket.read_exact(&mut len).await.unwrap();
                len[0] as usize
            }
            other => panic!("不认识的 ATYP {other}"),
        };
        let mut target = vec![0u8; addr_len + 2];
        socket.read_exact(&mut target).await.unwrap();
        (head, target)
    }

    /// 读到一个空行为止的请求头（模拟代理侧）。
    async fn read_request(socket: &mut TcpStream) -> String {
        let mut reader = BufReader::new(socket);
        let mut out = String::new();
        for _ in 0..64 {
            let mut line = String::new();
            let n = reader.read_line(&mut line).await.expect("read line");
            out.push_str(&line);
            if n == 0 || line == "\r\n" || line == "\n" {
                break;
            }
        }
        out
    }

    #[tokio::test]
    async fn http_connect_request_and_status_are_real_bytes() {
        let server = listener().await;
        let addr = server.local_addr().unwrap();
        let handle = tokio::spawn(async move {
            let (mut socket, _) = server.accept().await.unwrap();
            let request = read_request(&mut socket).await;
            // 200 之后紧跟目标的首批字节：验证 BufReader 没有把它们吞掉。
            socket
                .write_all(b"HTTP/1.1 200 Connection established\r\nX-Proxy: mock\r\n\r\nSSH-2.0-MOCK\r\n")
                .await
                .unwrap();
            request
        });
        let socket = TcpStream::connect(addr).await.unwrap();
        let mut stream = handshake(
            socket,
            &proxy(ProxyKind::Http, addr, None),
            "internal.example",
            22,
            SHORT_TIMEOUT,
        )
        .await
        .expect("CONNECT 应当成功");
        let request = handle.await.unwrap();
        assert!(request.starts_with("CONNECT internal.example:22 HTTP/1.1\r\n"), "{request}");
        assert!(request.contains("Host: internal.example:22\r\n"), "{request}");
        assert!(request.ends_with("\r\n\r\n"), "{request}");
        // 目标的第一行必须还在（HTTP 头之后多读进缓冲的字节不能丢）。
        let mut banner = [0u8; 14];
        stream.read_exact(&mut banner).await.unwrap();
        assert_eq!(&banner, b"SSH-2.0-MOCK\r\n");
    }

    #[tokio::test]
    async fn http_connect_sends_basic_credentials_when_configured() {
        let server = listener().await;
        let addr = server.local_addr().unwrap();
        let handle = tokio::spawn(async move {
            let (mut socket, _) = server.accept().await.unwrap();
            let request = read_request(&mut socket).await;
            socket
                .write_all(b"HTTP/1.1 200 OK\r\n\r\n")
                .await
                .unwrap();
            request
        });
        let auth = ProxyAuth {
            username: "Aladdin".to_string(),
            password: "open sesame".to_string(),
        };
        let socket = TcpStream::connect(addr).await.unwrap();
        handshake(
            socket,
            &proxy(ProxyKind::Http, addr, Some(auth)),
            "h",
            2222,
            SHORT_TIMEOUT,
        )
        .await
        .expect("带认证的 CONNECT 应当成功");
        let request = handle.await.unwrap();
        assert!(
            request.contains("Proxy-Authorization: Basic QWxhZGRpbjpvcGVuIHNlc2FtZQ==\r\n"),
            "{request}"
        );
    }

    #[tokio::test]
    async fn http_connect_reports_the_status_code_on_refusal() {
        let server = listener().await;
        let addr = server.local_addr().unwrap();
        tokio::spawn(async move {
            let (mut socket, _) = server.accept().await.unwrap();
            let _ = read_request(&mut socket).await;
            socket
                .write_all(b"HTTP/1.1 407 Proxy Authentication Required\r\n\r\n")
                .await
                .unwrap();
        });
        let socket = TcpStream::connect(addr).await.unwrap();
        let err = handshake(
            socket,
            &proxy(ProxyKind::Http, addr, None),
            "h",
            22,
            SHORT_TIMEOUT,
        )
        .await
        .err()
        .expect("这一步应当失败");
        assert!(matches!(err, SshError::Connect(_)), "{err:?}");
        assert!(err.to_string().contains("407"), "{err}");
    }

    #[tokio::test]
    async fn http_connect_does_not_hang_on_a_proxy_that_never_answers() {
        let server = listener().await;
        let addr = server.local_addr().unwrap();
        tokio::spawn(async move {
            let (mut socket, _) = server.accept().await.unwrap();
            let _ = read_request(&mut socket).await;
            // 故意不回任何东西，也不关连接。
            tokio::time::sleep(Duration::from_secs(30)).await;
        });
        let socket = TcpStream::connect(addr).await.unwrap();
        let err = handshake(
            socket,
            &proxy(ProxyKind::Http, addr, None),
            "h",
            22,
            Duration::from_millis(150),
        )
        .await
        .err()
        .expect("这一步应当失败");
        assert!(err.to_string().contains("没有完成"), "{err}");
    }

    #[tokio::test]
    async fn socks5_no_auth_request_is_a_domain_connect() {
        let server = listener().await;
        let addr = server.local_addr().unwrap();
        let handle = tokio::spawn(async move {
            let (mut socket, _) = server.accept().await.unwrap();
            let mut greeting = [0u8; 3];
            socket.read_exact(&mut greeting).await.unwrap();
            socket.write_all(&[5, 0]).await.unwrap();
            let (request, target) = read_socks_request(&mut socket).await;
            socket
                .write_all(b"\x05\x00\x00\x01\x7f\x00\x00\x01\x00\x00SSH-2.0-MOCK\r\n")
                .await
                .unwrap();
            (greeting.to_vec(), request.to_vec(), target)
        });
        let socket = TcpStream::connect(addr).await.unwrap();
        let mut stream = handshake(
            socket,
            &proxy(ProxyKind::Socks5, addr, None),
            "db.internal",
            2200,
            SHORT_TIMEOUT,
        )
        .await
        .expect("SOCKS5 CONNECT 应当成功");
        let (greeting, request, target) = handle.await.unwrap();
        assert_eq!(greeting, vec![5, 1, 0]);
        assert_eq!(request, vec![5, 1, 0, 3]);
        assert_eq!(&target[..11], b"db.internal");
        assert_eq!(&target[11..], &2200u16.to_be_bytes());
        // BND.ADDR/PORT 被吃完，读到的是目标数据。
        let mut banner = [0u8; 14];
        stream.read_exact(&mut banner).await.unwrap();
        assert_eq!(&banner, b"SSH-2.0-MOCK\r\n");
    }

    #[tokio::test]
    async fn socks5_userpass_subnegotiation_is_real() {
        let server = listener().await;
        let addr = server.local_addr().unwrap();
        let handle = tokio::spawn(async move {
            let (mut socket, _) = server.accept().await.unwrap();
            let mut greeting = [0u8; 3];
            socket.read_exact(&mut greeting).await.unwrap();
            socket.write_all(&[5, 2]).await.unwrap();
            let mut sub = [0u8; 2];
            socket.read_exact(&mut sub).await.unwrap();
            let mut credentials = vec![0u8; sub[1] as usize + 1];
            socket.read_exact(&mut credentials).await.unwrap();
            let pass_len = credentials[credentials.len() - 1] as usize;
            let mut pass = vec![0u8; pass_len];
            socket.read_exact(&mut pass).await.unwrap();
            socket.write_all(&[1, 0]).await.unwrap();
            let _ = read_socks_request(&mut socket).await;
            socket.write_all(&[5, 0, 0, 1, 127, 0, 0, 1, 0, 0]).await.unwrap();
            (greeting.to_vec(), sub.to_vec(), credentials, pass)
        });
        let auth = ProxyAuth {
            username: "alice".to_string(),
            password: "s3cret".to_string(),
        };
        let socket = TcpStream::connect(addr).await.unwrap();
        handshake(
            socket,
            &proxy(ProxyKind::Socks5, addr, Some(auth)),
            "host",
            22,
            SHORT_TIMEOUT,
        )
        .await
        .expect("用户名密码 SOCKS5 应当成功");
        let (greeting, sub, credentials, pass) = handle.await.unwrap();
        assert_eq!(greeting, vec![5, 1, 2]);
        assert_eq!(sub, vec![1, 5]);
        assert_eq!(&credentials[..5], b"alice");
        assert_eq!(credentials[5], 6, "第 6 个字节是密码长度");
        assert_eq!(pass, b"s3cret");
    }

    #[tokio::test]
    async fn socks5_reports_auth_failure_and_rep_codes() {
        for (case, server_reply, expected) in [
            ("auth", vec![5u8, 2, 1, 1], "拒绝"),
            ("no-method", vec![5, 0xff], "不接受"),
            (
                "rep",
                vec![5, 0, 5, 5, 0, 1, 127, 0, 0, 1, 0, 0],
                "拒绝 CONNECT",
            ),
        ] {
            let server = listener().await;
            let addr = server.local_addr().unwrap();
            let auth = (case == "auth").then(|| ProxyAuth {
                username: "u".to_string(),
                password: "p".to_string(),
            });
            tokio::spawn(async move {
                let (mut socket, _) = server.accept().await.unwrap();
                let mut greeting = [0u8; 3];
                socket.read_exact(&mut greeting).await.unwrap();
                socket.write_all(&server_reply[..2]).await.unwrap();
                if case == "auth" {
                    let mut sub = [0u8; 2];
                    socket.read_exact(&mut sub).await.unwrap();
                    let mut rest = vec![0u8; sub[1] as usize + 2];
                    socket.read_exact(&mut rest).await.unwrap();
                    socket.write_all(&server_reply[2..]).await.unwrap();
                } else if case == "rep" {
                    let _ = read_socks_request(&mut socket).await;
                    socket.write_all(&server_reply[2..]).await.unwrap();
                }
            });
            let socket = TcpStream::connect(addr).await.unwrap();
            let err = handshake(
                socket,
                &proxy(ProxyKind::Socks5, addr, auth),
                "h",
                22,
                SHORT_TIMEOUT,
            )
            .await
            .err()
        .expect("这一步应当失败");
            assert!(err.to_string().contains(expected), "{case}: {err}");
        }
    }

    /// 进程内真代理：CONNECT 之后把两端真接起来，验证字节能往返，
    /// 而不只是「握手报文长对了」。
    #[tokio::test]
    async fn a_real_http_proxy_carries_ssh_bytes_both_ways() {
        let echo = listener().await;
        let echo_addr = echo.local_addr().unwrap();
        tokio::spawn(async move {
            let (mut target, _) = echo.accept().await.unwrap();
            let mut buf = [0u8; 5];
            target.read_exact(&mut buf).await.unwrap();
            target.write_all(&buf).await.unwrap();
        });

        let server = listener().await;
        let addr = server.local_addr().unwrap();
        tokio::spawn(async move {
            let (mut client, _) = server.accept().await.unwrap();
            let _ = read_request(&mut client).await;
            let upstream = TcpStream::connect(echo_addr).await.unwrap();
            client
                .write_all(b"HTTP/1.1 200 OK\r\n\r\n")
                .await
                .unwrap();
            // 真代理该做的事：双向搬字节。
            let (mut cr, mut cw) = tokio::io::split(client);
            let (mut ur, mut uw) = tokio::io::split(upstream);
            let a = tokio::io::copy(&mut cr, &mut uw);
            let b = tokio::io::copy(&mut ur, &mut cw);
            let _ = tokio::join!(a, b);
        });

        let socket = TcpStream::connect(addr).await.unwrap();
        let mut stream = handshake(
            socket,
            &proxy(ProxyKind::Http, addr, None),
            "internal",
            22,
            SHORT_TIMEOUT,
        )
        .await
        .unwrap();
        stream.write_all(b"ping!").await.unwrap();
        stream.flush().await.unwrap();
        let mut echo_back = [0u8; 5];
        stream.read_exact(&mut echo_back).await.unwrap();
        assert_eq!(&echo_back, b"ping!");
    }
}

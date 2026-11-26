//! 第一段 TCP 的 socket 级开关。
//!
//! 会话表单里的 `Disable Nagle's algorithm` / `Enable TCP keepalives` 过去
//! 只写进 `SessionConfig`，内建 SSH 路径从来不读——用户勾了等于没勾。
//!
//! tokio 的 `TcpStream` 只暴露了 `set_nodelay`，没有 `set_keepalive`；这里用
//! `socket2::SockRef` 直接对同一个 fd 设两项（socket2 已在依赖图里，不引入
//! 新的运行时），避免「一项真接了、另一项还是摆设」。

use socket2::SockRef;
use std::io;
use tokio::net::TcpStream;

pub(crate) fn tune(stream: &TcpStream, low_delay: bool, keep_alive: bool) -> io::Result<()> {
    let sock = SockRef::from(stream);
    sock.set_tcp_nodelay(low_delay)?;
    sock.set_keepalive(keep_alive)
}

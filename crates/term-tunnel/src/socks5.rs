//! SOCKS5 的协议面（RFC 1928）：**只做 `CONNECT`**，只接受「无认证」。
//!
//! 为什么只做这么点：这个代理是给「终端里临时连一下内网服务」用的，
//! `BIND`（让代理监听入站）与用户名密码认证在终端场景里没有调用方；
//! 多实现一个命令就多一份要维护的攻击面。参考实现（wt-ssh）同样只做 `CONNECT`。
//!
//! 全部是纯函数：字节进、语义出。所以协议错误能在单测里穷举，
//! 不需要开端口、不需要起 SSH——「半包怎么办」这类问题也就变成了纯函数问题。

use std::fmt;

/// SOCKS5 应答码（`REP` 字段）。只列我们真会发的几个。
pub const REP_GENERAL_FAILURE: u8 = 0x01;
pub const REP_HOST_UNREACHABLE: u8 = 0x04;
pub const REP_COMMAND_NOT_SUPPORTED: u8 = 0x07;
pub const REP_ADDRESS_NOT_SUPPORTED: u8 = 0x08;

const VERSION: u8 = 0x05;
const CMD_CONNECT: u8 = 0x01;
const ATYP_V4: u8 = 0x01;
const ATYP_DOMAIN: u8 = 0x03;
const ATYP_V6: u8 = 0x04;
const METHOD_NO_AUTH: u8 = 0x00;

#[derive(Debug, PartialEq, Eq)]
pub enum Socks5Error {
    /// 版本不是 5。
    BadVersion(u8),
    /// 客户端没提供「无认证」——我们只支持这一种。
    NoAcceptableAuth,
    /// 不是 `CONNECT`（`BIND` / `UDP ASSOCIATE` 都不做）。
    UnsupportedCommand(u8),
    /// `ATYP` 不认识。
    UnsupportedAddress(u8),
    /// 字节还不够一条完整消息（TCP 半包，继续读）。
    Short,
    /// 端口 0 或域名空。
    EmptyTarget,
}

impl fmt::Display for Socks5Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Socks5Error::BadVersion(v) => write!(f, "不是 SOCKS5（版本字节 {v}）"),
            Socks5Error::NoAcceptableAuth => write!(f, "只支持无认证的 SOCKS5 客户端"),
            Socks5Error::UnsupportedCommand(c) => write!(f, "只支持 CONNECT（收到命令 {c}）"),
            Socks5Error::UnsupportedAddress(a) => write!(f, "不认识的地址类型 {a}"),
            Socks5Error::Short => write!(f, "消息还没收全"),
            Socks5Error::EmptyTarget => write!(f, "目标主机或端口为空"),
        }
    }
}

/// 解析后的连接目标。域名保持字符串——解析交给服务器侧的 SSH 通道，
/// 本机不做 DNS（那会让「内网域名」只能在工作机上解析，而隧道正是为了跨过这一层）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Request {
    pub host: String,
    pub port: u16,
}

/// 客户端问候：`VER NMETHODS METHODS...`。返回（是否提供无认证, 吃掉的字节数）。
pub fn read_greeting(buf: &[u8]) -> Result<(bool, usize), Socks5Error> {
    if buf.len() < 2 {
        return Err(Socks5Error::Short);
    }
    if buf[0] != VERSION {
        return Err(Socks5Error::BadVersion(buf[0]));
    }
    let n = buf[1] as usize;
    if n == 0 {
        return Err(Socks5Error::NoAcceptableAuth);
    }
    if buf.len() < 2 + n {
        return Err(Socks5Error::Short);
    }
    let no_auth = buf[2..2 + n].contains(&METHOD_NO_AUTH);
    Ok((no_auth, 2 + n))
}

/// 方法选择应答。
pub fn greeting_reply(no_auth: bool) -> [u8; 2] {
    [VERSION, if no_auth { METHOD_NO_AUTH } else { 0xFF }]
}

/// 请求：`VER CMD RSV ATYP ADDR PORT`。返回（目标, 吃掉的字节数）。
pub fn read_request(buf: &[u8]) -> Result<(Request, usize), Socks5Error> {
    if buf.len() < 4 {
        return Err(Socks5Error::Short);
    }
    if buf[0] != VERSION {
        return Err(Socks5Error::BadVersion(buf[0]));
    }
    if buf[1] != CMD_CONNECT {
        return Err(Socks5Error::UnsupportedCommand(buf[1]));
    }
    let (host, mut at) = match buf[3] {
        ATYP_V4 => {
            if buf.len() < 4 + 4 {
                return Err(Socks5Error::Short);
            }
            let a = [buf[4], buf[5], buf[6], buf[7]];
            (format!("{}.{}.{}.{}", a[0], a[1], a[2], a[3]), 4 + 4)
        }
        ATYP_V6 => {
            if buf.len() < 4 + 16 {
                return Err(Socks5Error::Short);
            }
            let mut segs = Vec::with_capacity(8);
            for chunk in buf[4..4 + 16].chunks(2) {
                segs.push(format!("{:x}", u16::from_be_bytes([chunk[0], chunk[1]])));
            }
            (segs.join(":"), 4 + 16)
        }
        ATYP_DOMAIN => {
            if buf.len() < 5 {
                return Err(Socks5Error::Short);
            }
            let len = buf[4] as usize;
            if buf.len() < 5 + len + 2 {
                return Err(Socks5Error::Short);
            }
            let host = String::from_utf8_lossy(&buf[5..5 + len]).to_string();
            (host, 5 + len)
        }
        other => return Err(Socks5Error::UnsupportedAddress(other)),
    };
    let port = u16::from_be_bytes([buf[at], buf[at + 1]]);
    at += 2;
    if host.trim().is_empty() || port == 0 {
        return Err(Socks5Error::EmptyTarget);
    }
    Ok((Request { host, port }, at))
}

/// 成功应答：`REP=0`，绑定地址填 `0.0.0.0:0`（客户端不看这两个字段）。
pub fn success_reply() -> [u8; 10] {
    [VERSION, 0x00, 0x00, ATYP_V4, 0, 0, 0, 0, 0, 0]
}

/// 失败应答：`REP` 用上面的常量。
pub fn failure_reply(rep: u8) -> [u8; 10] {
    [VERSION, rep, 0x00, ATYP_V4, 0, 0, 0, 0, 0, 0]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_no_auth_greeting_is_accepted() {
        let (no_auth, used) = read_greeting(&[5, 1, 0]).unwrap();
        assert!(no_auth);
        assert_eq!(used, 3);
        assert_eq!(greeting_reply(true), [5, 0]);
    }

    #[test]
    fn a_client_that_only_offers_a_password_is_refused() {
        let (no_auth, _) = read_greeting(&[5, 1, 2]).unwrap();
        assert!(!no_auth);
        assert_eq!(greeting_reply(false), [5, 0xFF]);
    }

    #[test]
    fn a_half_arrived_greeting_is_not_misread() {
        assert_eq!(read_greeting(&[5]), Err(Socks5Error::Short));
        // 声明 3 种方法但只到了 2 个方法
        assert_eq!(read_greeting(&[5, 3, 0, 2]), Err(Socks5Error::Short));
    }

    #[test]
    fn an_ipv4_connect_request_yields_a_target() {
        let msg = [5, 1, 0, 1, 10, 0, 0, 7, 0x1F, 0x90];
        let (req, used) = read_request(&msg).unwrap();
        assert_eq!(
            req,
            Request {
                host: "10.0.0.7".into(),
                port: 8080
            }
        );
        assert_eq!(used, msg.len());
    }

    #[test]
    fn a_domain_request_keeps_the_name_for_the_server_to_resolve() {
        let host = b"db.internal";
        let mut msg = vec![5, 1, 0, 3, host.len() as u8];
        msg.extend_from_slice(host);
        msg.extend_from_slice(&5432u16.to_be_bytes());
        let (req, used) = read_request(&msg).unwrap();
        assert_eq!(req.host, "db.internal");
        assert_eq!(req.port, 5432);
        assert_eq!(used, msg.len());
    }

    #[test]
    fn an_ipv6_request_is_rendered_as_a_plain_literal() {
        let mut msg = vec![5, 1, 0, 4];
        msg.extend_from_slice(&[0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 1]);
        msg.extend_from_slice(&22u16.to_be_bytes());
        let (req, _) = read_request(&msg).unwrap();
        assert_eq!(req.host, "0:0:0:0:0:0:0:1");
        assert_eq!(req.port, 22);
    }

    #[test]
    fn only_connect_is_supported_and_it_says_which_command_it_got() {
        let msg = [5, 2, 0, 1, 1, 1, 1, 1, 0, 80]; // BIND
        assert_eq!(read_request(&msg), Err(Socks5Error::UnsupportedCommand(2)));
    }

    #[test]
    fn an_unknown_address_type_is_refused_rather_than_guessed() {
        let msg = [5, 1, 0, 9, 0, 0];
        assert_eq!(read_request(&msg), Err(Socks5Error::UnsupportedAddress(9)));
    }

    #[test]
    fn a_request_with_port_zero_is_refused() {
        let msg = [5, 1, 0, 1, 10, 0, 0, 7, 0, 0];
        assert_eq!(read_request(&msg), Err(Socks5Error::EmptyTarget));
    }

    #[test]
    fn a_truncated_request_asks_for_more_bytes_instead_of_failing() {
        // 域名长度说 10，但只给了 3 个字节 + 端口
        let msg = [5, 1, 0, 3, 10, b'a', b'b', b'c', 0, 80];
        assert_eq!(read_request(&msg), Err(Socks5Error::Short));
    }

    #[test]
    fn replies_are_ten_bytes_with_the_documented_rep_code() {
        assert_eq!(success_reply()[1], 0);
        assert_eq!(failure_reply(REP_HOST_UNREACHABLE)[1], REP_HOST_UNREACHABLE);
        assert_eq!(failure_reply(REP_HOST_UNREACHABLE).len(), 10);
    }
}

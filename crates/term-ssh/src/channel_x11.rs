//! X11 转发的 `x11-req` 全局请求载荷。
//!
//! 从 `channel` 拆出这一块：cookie 的随机性与线格式编码是一段完整的、
//! 与会话泵/超时无关的职责，单独放着才能把 `channel.rs` 压回 500 行以内。

use crate::error::SshError;

/// 拼 `x11-req` 全局请求的载荷（RFC 4254 §6.2 之后的那段）。
///
/// 顺序：`boolean single_connection` + `string auth_protocol`
/// + `string auth_cookie` + `uint32 screen_number`。
///
/// 随机源失败时返回错误，由 `handshake` 拒绝开启 X11 转发——不构造弱 cookie。
pub(crate) fn x11_request_payload(screen: u32) -> Result<Vec<u8>, SshError> {
    let cookie = x11_cookie()?;
    let mut buf = Vec::with_capacity(40);
    buf.push(0u8); // single_connection = false：一条 SSH 连接上可转发多个 X 连接
    put_ssh_string(&mut buf, b"MIT-MAGIC-COOKIE-1");
    put_ssh_string(&mut buf, &cookie);
    buf.extend_from_slice(&screen.to_be_bytes());
    Ok(buf)
}

/// SSH 线格式字符串：`uint32` 长度前缀 + 字节。
fn put_ssh_string(buf: &mut Vec<u8>, s: &[u8]) {
    buf.extend_from_slice(&(s.len() as u32).to_be_bytes());
    buf.extend_from_slice(s);
}

/// MIT-MAGIC-COOKIE-1：16 字节密码学随机 cookie。
///
/// 它只随 `x11-req` 下发给服务端，供 sshd 在远端 xauth 登记转发显示用，
/// **不影响本机桥接**（本机那段只是把回开通道的字节原样搬到本地 X socket）。
/// 但它就是远端 X display 的访问凭据：可预测值让任何能连上那个 display 的人
/// 直接通过认证。所以必须用 OS entropy，一次一密；取不到熵时返回错误，
/// 由 `handshake` 拒绝开启 X11 转发——绝不退化成时间戳 / 计数器之类可猜测的值
/// （那正是本函数早期版本的做法）。
fn x11_cookie() -> Result<[u8; 16], SshError> {
    let mut cookie = [0u8; 16];
    getrandom::getrandom(&mut cookie)
        .map_err(|e| SshError::Channel(format!("X11 cookie 随机源不可用：{e}")))?;
    Ok(cookie)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn x11_request_payload_is_well_formed() {
        let payload = x11_request_payload(0).expect("随机源在测试环境里必须可用");
        // single_connection = false
        assert_eq!(payload[0], 0);
        // auth protocol: len-prefixed "MIT-MAGIC-COOKIE-1"
        let proto_len = u32::from_be_bytes([payload[1], payload[2], payload[3], payload[4]]) as usize;
        assert_eq!(proto_len, 18);
        let proto = &payload[5..5 + proto_len];
        assert_eq!(proto, b"MIT-MAGIC-COOKIE-1");
        // auth cookie: 16 字节原始二进制（MIT-MAGIC-COOKIE-1 的线上形态）
        let cookie_off = 5 + proto_len;
        let cookie_len =
            u32::from_be_bytes(payload[cookie_off..cookie_off + 4].try_into().unwrap()) as usize;
        assert_eq!(cookie_len, 16);
        let cookie = &payload[cookie_off + 4..cookie_off + 4 + cookie_len];
        assert!(cookie.iter().any(|&b| b != 0), "不是全零占位：必须是随机字节");
        // screen number: trailing u32 BE
        let screen_off = cookie_off + 4 + cookie_len;
        let screen = u32::from_be_bytes(
            payload[screen_off..screen_off + 4]
                .try_into()
                .unwrap(),
        );
        assert_eq!(screen, 0);
    }

    #[test]
    fn x11_cookie_is_sixteen_random_bytes() {
        let first = x11_cookie().expect("随机源必须可用");
        let second = x11_cookie().expect("随机源必须可用");
        assert_eq!(first.len(), 16, "MIT-MAGIC-COOKIE-1 固定 16 字节");
        assert!(first.iter().any(|&b| b != 0), "不能是全零占位");
        // 两次独立取样不相等：不再依赖时间戳 / 计数器这种可预测的唯一性。
        assert_ne!(first, second);
    }
}

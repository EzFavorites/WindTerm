//! 会话日志的已知凭据脱敏。
//!
//! 单独成文件有两个理由：`log.rs` 受 4j 的单文件 500 行门禁约束；这里要守住
//! 一条更重要的语义——Debug 日志是 `LogType::All`，必须**逐字节保真**。
//! 因此替换只能在字节流上做，不能走 `String::from_utf8_lossy`。

/// 已知应答 / 密码登记表：写盘前统一脱敏。
///
/// 这些值是 UI 在提交密码框、OneKey 应答时登记的，只活在本进程内存里。
/// 日志会跟着会话备份、贴到工单里——明文应答一旦落盘就是一次已发生的泄漏。
static KNOWN_SECRETS: std::sync::Mutex<Vec<String>> = std::sync::Mutex::new(Vec::new());

pub fn register_secret(secret: &str) {
    if !secret.is_empty() {
        if let Ok(mut known) = KNOWN_SECRETS.lock() {
            if !known.iter().any(|s| s == secret) {
                known.push(secret.into());
            }
        }
    }
}

/// 字节级替换：非 UTF-8 的旧编码与转义序列原样保留，只把已登记的凭据换成
/// `[redacted]`。登记表每 chunk 只读一次，匹配窗口逐个secret 前进。
pub(crate) fn redact(bytes: &[u8]) -> Vec<u8> {
    let known = KNOWN_SECRETS.lock().map(|k| k.clone()).unwrap_or_default();
    if known.is_empty() {
        return bytes.to_vec();
    }
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        let mut hit = None;
        for secret in &known {
            let pat = secret.as_bytes();
            if !pat.is_empty() && bytes[i..].starts_with(pat) {
                hit = Some(pat.len());
                break;
            }
        }
        if let Some(n) = hit {
            out.extend_from_slice(b"[redacted]");
            i += n;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 脱敏必须逐字节保真：raw 日志里的非 UTF-8 与转义序列不能被改写。
    #[test]
    fn redaction_keeps_non_utf8_bytes_intact() {
        register_secret("TOPSECRET_XYZ");
        let input = [0x1b, b'[', b'3', b'1', b'm', 0xff, 0xfe, b'T', b'O', b'P'];
        let out = redact(&input);
        assert!(
            out.windows(2).any(|w| w == [0xff, 0xfe]),
            "非 UTF-8 被改写：{out:?}"
        );
        assert_eq!(&out[..5], &input[..5], "转义序列必须原样保留");
        assert_eq!(redact(b"aTOPSECRET_XYZb"), b"a[redacted]b");
    }
}

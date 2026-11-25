//! 文件结构：magic + 版本 + header(JSON) + 密文。
//!
//! ```text
//! 偏移  长度  内容
//! 0     6     magic "ZTVLT\0"
//! 6     1     版本（当前 1）
//! 7     1     保留（0）
//! 8     4     header 长度（小端 u32）
//! 12    N     header（UTF-8 JSON：KDF 参数 / AEAD 参数 / 可选提示）
//! 12+N  M     密文（XChaCha20-Poly1305 输出，末尾自带 16 字节 tag）
//! ```
//!
//! **关键设计：AAD 就是密文之前的那段字节**（`envelope()` 的返回值）。
//! 于是「把 header 里的 Argon2 内存开销从 64 MiB 改成 8 MiB 再离线爆破」
//! 这种攻击会直接让 AEAD 校验失败——参数被绑进了认证范围，改一个字都解不开。
//! 比起「把参数写在旁边、信任它」的做法，这一条是省钱又有效的那种正确。
//!
//! 格式刻意保持可读、可脚本恢复：密码忘了但记得主密码时可以手写解析器抢救条目，
//! 不需要我们的程序。

use serde::{Deserialize, Serialize};

use crate::error::VaultError;
use crate::kdf::KdfParams;

pub const MAGIC: &[u8; 6] = b"ZTVLT\0";
pub const FORMAT_VERSION: u8 = 1;
pub const NONCE_LEN: usize = 24;
pub const TAG_LEN: usize = 16;
const PREFIX_LEN: usize = MAGIC.len() + 1 + 1 + 4;
/// header 长度字段的上界。认证前就会读它并据此切片：伪造一个 `u32::MAX`
/// 不拦住的话，`bytes[PREFIX_LEN..body_at]` 直接越界 panic（生产路径的 panic），
/// 或按长度去分配内存。真实 header 只有几百字节，16 KiB 已宽得离谱。
pub const MAX_HEADER_LEN: usize = 16 * 1024;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AeadParams {
    /// 目前只有 `xchacha20poly1305`。写成字符串而不是枚举，是为了认得出
    /// 「未来版本用了别的算法」并给出可行动的提示，而不是解析失败。
    pub alg: String,
    /// base64(24 字节 nonce)。每次 `seal` 都会换新的。
    pub nonce: String,
}

impl AeadParams {
    pub fn new() -> Self {
        Self {
            alg: ALG.to_string(),
            // 创建时先放全 0 占位；seal 时换成随机 nonce（header 参与 AAD，
            // 所以 nonce 必须在算 AAD 之前就定下来）。
            nonce: base64_encode(&[0u8; NONCE_LEN]),
        }
    }

    pub fn nonce_bytes(&self) -> Result<Vec<u8>, VaultError> {
        let raw = base64_decode(&self.nonce)
            .ok_or(VaultError::Corrupt("AEAD nonce 不是合法 base64"))?;
        if raw.len() != NONCE_LEN {
            return Err(VaultError::Corrupt("AEAD nonce 长度不对"));
        }
        Ok(raw)
    }
}

impl Default for AeadParams {
    fn default() -> Self {
        Self::new()
    }
}

pub const ALG: &str = "xchacha20poly1305";

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Header {
    /// 文件格式版本（与第 6 字节一致；两处都存是为了让 header 自己也能被单独校验）。
    pub v: u8,
    pub kdf: KdfParams,
    pub aead: AeadParams,
    /// 主密码提示。**明文存储**，故意如此：忘了密码时的唯一线索，
    /// 所以往里写密码本身的人活该（文档与 UI 都要说清楚）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hint: Option<String>,
}

impl Header {
    pub fn new(kdf: KdfParams, hint: Option<String>) -> Self {
        Self {
            v: FORMAT_VERSION,
            kdf,
            aead: AeadParams::new(),
            hint,
        }
    }
}

/// 密文之前的那段字节；**既写文件，也当 AAD**。
pub fn envelope(header_json: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(PREFIX_LEN + header_json.len());
    out.extend_from_slice(MAGIC);
    out.push(FORMAT_VERSION);
    out.push(0); // 保留字节：将来要加「压缩」「分块」这类标志时用，不破坏 AAD
    out.extend_from_slice(&(header_json.len() as u32).to_le_bytes());
    out.extend_from_slice(header_json);
    out
}

/// 拆开文件：返回（header, AAD, 密文）。
pub fn split(bytes: &[u8]) -> Result<(Header, Vec<u8>, &[u8]), VaultError> {
    if bytes.len() < PREFIX_LEN {
        return Err(VaultError::Corrupt("文件太短"));
    }
    if &bytes[..MAGIC.len()] != MAGIC {
        return Err(VaultError::Corrupt("不是保险库文件（magic 不匹配）"));
    }
    let version = bytes[MAGIC.len()];
    if version > FORMAT_VERSION {
        return Err(VaultError::Unsupported(format!(
            "文件版本 {version} 高于程序支持的 {FORMAT_VERSION}"
        )));
    }
    let len_at = MAGIC.len() + 2;
    let header_len = u32::from_le_bytes([
        bytes[len_at],
        bytes[len_at + 1],
        bytes[len_at + 2],
        bytes[len_at + 3],
    ]) as usize;
    if header_len > MAX_HEADER_LEN {
        return Err(VaultError::Corrupt("header 长度超出上限"));
    }
    let body_at = PREFIX_LEN + header_len;
    if bytes.len() < body_at + TAG_LEN {
        return Err(VaultError::Corrupt("header 或密文被截断"));
    }
    let header_json = &bytes[PREFIX_LEN..body_at];
    let header: Header = serde_json::from_slice(header_json)
        .map_err(|_| VaultError::Corrupt("header 不是合法 JSON"))?;
    if header.v != version {
        return Err(VaultError::Corrupt("header 版本与文件头不一致"));
    }
    Ok((header, bytes[..body_at].to_vec(), &bytes[body_at..]))
}

pub(crate) fn base64_encode(bytes: &[u8]) -> String {
    use base64::Engine;
    base64::engine::general_purpose::STANDARD.encode(bytes)
}

pub(crate) fn base64_decode(text: &str) -> Option<Vec<u8>> {
    use base64::Engine;
    base64::engine::general_purpose::STANDARD.decode(text).ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::kdf::KdfParams;

    fn header() -> Header {
        Header::new(KdfParams::fast_for_tests(), Some("家里的猫叫什么".into()))
    }

    #[test]
    fn envelope_starts_with_magic_version_and_length() {
        let json = br#"{"v":1}"#;
        let env = envelope(json);
        assert_eq!(&env[..6], MAGIC);
        assert_eq!(env[6], FORMAT_VERSION);
        assert_eq!(env[7], 0);
        assert_eq!(u32::from_le_bytes([env[8], env[9], env[10], env[11]]) as usize, json.len());
        assert_eq!(&env[12..], json);
    }

    #[test]
    fn split_returns_the_envelope_as_aad() {
        let h = header();
        let json = serde_json::to_vec(&h).unwrap();
        let env = envelope(&json);
        let mut file = env.clone();
        file.extend_from_slice(&[0u8; NONCE_LEN]);

        let (parsed, aad, cipher) = split(&file).unwrap();
        assert_eq!(parsed, h);
        assert_eq!(aad, env, "AAD 必须逐字节等于文件前缀");
        assert_eq!(cipher.len(), NONCE_LEN);
    }

    #[test]
    fn a_wrong_magic_is_reported_as_corruption() {
        let json = serde_json::to_vec(&header()).unwrap();
        let mut file = envelope(&json);
        file[0] = b'X';
        file.extend_from_slice(&[0u8; NONCE_LEN]);
        assert_eq!(split(&file), Err(VaultError::Corrupt("不是保险库文件（magic 不匹配）")));
    }

    #[test]
    fn a_newer_version_asks_for_an_upgrade_instead_of_failing_obscurely() {
        let json = serde_json::to_vec(&header()).unwrap();
        let mut file = envelope(&json);
        file[6] = FORMAT_VERSION + 1;
        file.extend_from_slice(&[0u8; NONCE_LEN]);
        assert!(matches!(split(&file), Err(VaultError::Unsupported(_))));
    }

    #[test]
    fn truncated_files_are_rejected() {
        let json = serde_json::to_vec(&header()).unwrap();
        let env = envelope(&json);
        assert!(matches!(split(&env), Err(VaultError::Corrupt(_))), "没有密文");
        assert!(matches!(split(&env[..5]), Err(VaultError::Corrupt(_))), "连头都不全");
    }

    /// 恶意 header：长度字段写成 u32::MAX 时必须在切片之前报损坏，
    /// 不能 panic，也不能按那个长度去分配。
    #[test]
    fn a_hostile_header_length_is_reported_as_corruption() {
        let mut file = vec![0u8; PREFIX_LEN + 16];
        file[..MAGIC.len()].copy_from_slice(MAGIC);
        file[MAGIC.len()] = FORMAT_VERSION;
        file[MAGIC.len() + 2..MAGIC.len() + 6].copy_from_slice(&u32::MAX.to_le_bytes());
        assert!(matches!(split(&file), Err(VaultError::Corrupt(_))));
        file[MAGIC.len() + 2..MAGIC.len() + 6]
            .copy_from_slice(&((MAX_HEADER_LEN as u32) + 1).to_le_bytes());
        assert!(matches!(split(&file), Err(VaultError::Corrupt(_))));
    }
}

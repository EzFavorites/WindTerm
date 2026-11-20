//! 主密码 → 32 字节密钥：Argon2id。
//!
//! 参数写在文件 header 里（而 header 参与 AAD），所以「把内存开销改小再爆破解密」
//! 这条攻击路径是断的：改了 header 就解不开。
//!
//! 默认 `production()`：m = 64 MiB / t = 3 / p = 4。这是 OWASP 2024 对 Argon2id
//! 给出的最低档，在本机解锁约 200~400 ms——一次输入密码换这个延迟是合适的；
//! 每次打开都派生就不合适了，所以解锁后密钥留在内存里（见 `Vault`）。

use argon2::{Algorithm, Argon2, Params, Version};
use rand::RngCore;
use serde::{Deserialize, Serialize};
use zeroize::Zeroizing;

use crate::error::VaultError;
use crate::format::{base64_decode, base64_encode};

pub const SALT_LEN: usize = 16;
pub const KEY_LEN: usize = 32;
pub const ALG: &str = "argon2id";

fn argon2_error(e: argon2::Error) -> VaultError {
    VaultError::Crypto(format!("Argon2 参数非法：{e}"))
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct KdfParams {
    /// 目前只有 `argon2id`。
    pub alg: String,
    /// 内存开销，单位 KiB。
    pub m: u32,
    /// 迭代次数。
    pub t: u32,
    /// 并行度。
    pub p: u32,
    /// base64(16 字节盐)。
    pub salt: String,
}

impl KdfParams {
    /// 生产参数：64 MiB / 3 轮 / 4 路。
    pub fn production() -> Self {
        Self::with_cost(65_536, 3, 4)
    }

    /// 测试参数：8 MiB / 1 轮 / 1 路。**不得用于真实保险库**——
    /// 它存在的唯一理由是让 14 个单测不用每个都等 300 ms。
    pub fn fast_for_tests() -> Self {
        Self::with_cost(8_192, 1, 1)
    }

    pub fn with_cost(m: u32, t: u32, p: u32) -> Self {
        let mut salt = [0u8; SALT_LEN];
        rand::thread_rng().fill_bytes(&mut salt);
        Self {
            alg: ALG.to_string(),
            m,
            t,
            p,
            salt: base64_encode(&salt),
        }
    }

    /// 同样的开销、新的盐。改主密码时用：盐必须换，否则新旧密钥由同一份
    /// 盐加不同密码派生——虽然不致命，但换盐是免费的。
    pub fn resalted(&self) -> Self {
        let mut next = self.clone();
        let mut salt = [0u8; SALT_LEN];
        rand::thread_rng().fill_bytes(&mut salt);
        next.salt = base64_encode(&salt);
        next
    }

    pub fn salt_bytes(&self) -> Result<[u8; SALT_LEN], VaultError> {
        let raw = base64_decode(&self.salt).ok_or(VaultError::Corrupt("盐不是合法 base64"))?;
        let mut out = [0u8; SALT_LEN];
        if raw.len() != SALT_LEN {
            return Err(VaultError::Corrupt("盐长度不对"));
        }
        out.copy_from_slice(&raw);
        Ok(out)
    }

    /// 派生 32 字节密钥。返回值是 `Zeroizing`：离开作用域自动清零。
    pub fn derive(&self, password: &str) -> Result<Zeroizing<[u8; KEY_LEN]>, VaultError> {
        if self.alg != ALG {
            return Err(VaultError::Unsupported(format!("KDF {}", self.alg)));
        }
        let salt = self.salt_bytes()?;
        let params = Params::new(self.m, self.t, self.p, Some(KEY_LEN)).map_err(argon2_error)?;
        let argon = Argon2::new(Algorithm::Argon2id, Version::V0x13, params);
        let mut key = Zeroizing::new([0u8; KEY_LEN]);
        argon
            .hash_password_into(password.as_bytes(), &salt, &mut key[..])
            .map_err(|e| VaultError::Crypto(format!("派生失败：{e}")))?;
        Ok(key)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn production_cost_meets_the_owasp_floor() {
        let p = KdfParams::production();
        assert!(p.m >= 65_536, "内存下限 64 MiB");
        assert!(p.t >= 3);
        assert!(p.p >= 2);
    }

    #[test]
    fn the_same_password_and_salt_derive_the_same_key() {
        let p = KdfParams::fast_for_tests();
        assert_eq!(p.derive("correct horse battery").unwrap(), p.derive("correct horse battery").unwrap());
    }

    #[test]
    fn different_salts_derive_different_keys() {
        let a = KdfParams::fast_for_tests();
        let b = a.resalted();
        assert_ne!(a.salt, b.salt);
        assert_ne!(a.derive("correct horse battery").unwrap(), b.derive("correct horse battery").unwrap());
    }

    #[test]
    fn resalting_keeps_the_cost() {
        let a = KdfParams::with_cost(9_000, 2, 3);
        let b = a.resalted();
        assert_eq!((b.m, b.t, b.p), (9_000, 2, 3));
    }

    #[test]
    fn an_unknown_kdf_is_refused_instead_of_silently_treated_as_argon2() {
        let mut p = KdfParams::fast_for_tests();
        p.alg = "scrypt".into();
        assert!(matches!(p.derive("x"), Err(VaultError::Unsupported(_))));
    }

    #[test]
    fn a_broken_salt_is_reported_as_corruption() {
        let mut p = KdfParams::fast_for_tests();
        p.salt = "not base64!!".into();
        assert!(matches!(p.derive("x"), Err(VaultError::Corrupt(_))));
        p.salt = base64_encode(&[0u8; 8]);
        assert!(matches!(p.derive("x"), Err(VaultError::Corrupt(_))));
    }
}

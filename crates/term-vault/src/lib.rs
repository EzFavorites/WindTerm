//! `term-vault` — 凭据保险库。
//!
//! 解决什么问题：会话里的密码、私钥口令、代理口令现在**要么不存，要么明文进
//! JSON**。这一层把「主密码 + AEAD」做成一个可单测的纯字节变换：
//!
//! ```text
//! 主密码 ──Argon2id(salt, m, t, p)──▶ 32B 密钥 ──XChaCha20-Poly1305──▶ 密文
//!                    ▲                        ▲
//!                    └── 参数在 header 里 ────┴── header 进 AAD（改一个字节就解不开）
//! ```
//!
//! 三条边界（这一层刻意不做别的）：
//! 1. **不管文件在哪、不管 IO**：`seal()` 返回 `Vec<u8>`，写盘由调用方决定；
//! 2. **不管 UI**：没有「输入密码」的循环，也没有重试次数；
//! 3. **没有运行时**：同步、无线程、无 tokio。依赖表里只有加密库。
//!
//! 设计要点：文件形状、AAD 绑 header、默认 KDF 参数；两处**刻意不同**：
//! - header 里不存 `created` 时间戳：时间戳不是安全属性（可伪造），却会让
//!   单测依赖时钟。要审计「什么时候建的」用文件 mtime。
//! - 提供 `change_password()`：换主密码要换盐重新派生，这是最容易写错的一步，
//!   不该让每个调用方自己拼。

mod error;
mod format;
mod kdf;

pub use error::{VaultError, MIN_PASSWORD_LEN};
pub use format::{AeadParams, Header, FORMAT_VERSION, MAGIC, NONCE_LEN};
pub use kdf::{KdfParams, KEY_LEN, SALT_LEN};

use chacha20poly1305::aead::{Aead, Payload};
use chacha20poly1305::{Key, KeyInit, XChaCha20Poly1305, XNonce};
use rand::RngCore;
use serde::{Deserialize, Serialize};
use zeroize::{Zeroize, Zeroizing};

use format::{base64_encode, envelope, split};

/// 保险库里的一条凭据。
///
/// `kind` 只区分**用途**，不区分字段形状以外的语义：要不要把这条用在某个会话上，
/// 由调用方（会话配置里存 `entry_id`）决定，这里不建立反向引用。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Entry {
    Password {
        id: String,
        name: String,
        secret: String,
    },
    PrivateKey {
        id: String,
        name: String,
        pem: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        passphrase: Option<String>,
    },
    ProxyCredential {
        id: String,
        name: String,
        username: String,
        secret: String,
    },
}

impl Entry {
    pub fn password(id: impl Into<String>, name: impl Into<String>, secret: impl Into<String>) -> Self {
        Entry::Password {
            id: id.into(),
            name: name.into(),
            secret: secret.into(),
        }
    }

    pub fn id(&self) -> &str {
        match self {
            Entry::Password { id, .. } | Entry::PrivateKey { id, .. } | Entry::ProxyCredential { id, .. } => id,
        }
    }

    pub fn name(&self) -> &str {
        match self {
            Entry::Password { name, .. } | Entry::PrivateKey { name, .. } | Entry::ProxyCredential { name, .. } => name,
        }
    }

    pub fn kind(&self) -> &'static str {
        match self {
            Entry::Password { .. } => "password",
            Entry::PrivateKey { .. } => "private_key",
            Entry::ProxyCredential { .. } => "proxy_credential",
        }
    }

    /// 给「确实要拿它去认证」的调用方用的一条取密接口。
    pub fn secret(&self) -> Option<&str> {
        match self {
            Entry::Password { secret, .. } | Entry::ProxyCredential { secret, .. } => Some(secret),
            Entry::PrivateKey { .. } => None,
        }
    }

    /// 内存里的明文就地清零。`lock()` 与 `Drop` 都走这里。
    fn wipe(&mut self) {
        match self {
            Entry::Password { id, name, secret } => {
                id.zeroize();
                name.zeroize();
                secret.zeroize();
            }
            Entry::PrivateKey {
                id,
                name,
                pem,
                passphrase,
            } => {
                id.zeroize();
                name.zeroize();
                pem.zeroize();
                if let Some(p) = passphrase {
                    p.zeroize();
                }
            }
            Entry::ProxyCredential {
                id,
                name,
                username,
                secret,
            } => {
                id.zeroize();
                name.zeroize();
                username.zeroize();
                secret.zeroize();
            }
        }
    }
}

/// 一个已创建或已解锁的保险库。
///
/// **解锁态**（`key.is_some()`）下条目在内存里是明文——这是必然的，能做的是
/// `lock()` / `Drop` 时清零。**锁定态**下条目为空，任何写入都返回 `Locked`。
pub struct Vault {
    header: Header,
    entries: Vec<Entry>,
    key: Option<Zeroizing<[u8; KEY_LEN]>>,
}

impl Vault {
    /// 用生产参数新建一个空保险库。
    pub fn create(password: &str) -> Result<Self, VaultError> {
        Self::create_with(password, KdfParams::production(), None)
    }

    pub fn create_with(
        password: &str,
        kdf: KdfParams,
        hint: Option<String>,
    ) -> Result<Self, VaultError> {
        check_password(password)?;
        let key = kdf.derive(password)?;
        Ok(Self {
            header: Header::new(kdf, hint),
            entries: Vec::new(),
            key: Some(key),
        })
    }

    /// 不解密、只看提示：UI 在要求输入主密码时可以先显示它。
    pub fn peek_hint(bytes: &[u8]) -> Result<Option<String>, VaultError> {
        let (header, _, _) = split(bytes)?;
        Ok(header.hint)
    }

    /// 用主密码打开。密码错与文件被改过在这里是**同一个错误**（AEAD 的性质）。
    pub fn unlock(bytes: &[u8], password: &str) -> Result<Self, VaultError> {
        let (mut header, aad, ciphertext) = split(bytes)?;
        if header.aead.alg != format::ALG {
            return Err(VaultError::Unsupported(format!("AEAD {}", header.aead.alg)));
        }
        let key = header.kdf.derive(password)?;
        let nonce = header.aead.nonce_bytes()?;
        let plain = decrypt(&key, &nonce, ciphertext, &aad)?;
        let entries: Vec<Entry> = serde_json::from_slice(&plain)
            .map_err(|_| VaultError::Corrupt("条目不是合法 JSON"))?;
        // 解出来的 header 已经是「历史的」：seal 时会把 nonce 换成新的。
        header.aead = AeadParams {
            alg: header.aead.alg,
            nonce: header.aead.nonce,
        };
        Ok(Self {
            header,
            entries,
            key: Some(key),
        })
    }

    pub fn header(&self) -> &Header {
        &self.header
    }

    pub fn hint(&self) -> Option<&str> {
        self.header.hint.as_deref()
    }

    pub fn is_unlocked(&self) -> bool {
        self.key.is_some()
    }

    pub fn entries(&self) -> &[Entry] {
        &self.entries
    }

    pub fn find(&self, id: &str) -> Option<&Entry> {
        self.entries.iter().find(|e| e.id() == id)
    }

    /// 新增；同 id 已存在时报错（不做静默覆盖——那会让「改了密码没生效」变得难查）。
    pub fn insert(&mut self, entry: Entry) -> Result<(), VaultError> {
        self.ensure_unlocked()?;
        if self.find(entry.id()).is_some() {
            return Err(VaultError::DuplicateEntry(entry.id().to_string()));
        }
        self.entries.push(entry);
        Ok(())
    }

    /// 存在则替换、不存在则新增。给「保存这份表单」这种幂等写用。
    pub fn upsert(&mut self, entry: Entry) -> Result<(), VaultError> {
        self.ensure_unlocked()?;
        match self.entries.iter_mut().find(|e| e.id() == entry.id()) {
            Some(slot) => {
                slot.wipe();
                *slot = entry;
            }
            None => self.entries.push(entry),
        }
        Ok(())
    }

    pub fn remove(&mut self, id: &str) -> Result<(), VaultError> {
        self.ensure_unlocked()?;
        let at = self
            .entries
            .iter()
            .position(|e| e.id() == id)
            .ok_or_else(|| VaultError::EntryNotFound(id.to_string()))?;
        let mut removed = self.entries.remove(at);
        removed.wipe();
        Ok(())
    }

    /// 换主密码：换盐 → 重新派生 → 换 header 里的 KDF 参数。
    ///
    /// **调用方必须紧接着 `seal()` 落盘**：内存里的密钥已经换成新的，
    /// 磁盘上还是旧密码加密的旧文件。
    pub fn change_password(&mut self, new_password: &str) -> Result<(), VaultError> {
        self.ensure_unlocked()?;
        check_password(new_password)?;
        let kdf = self.header.kdf.resalted();
        let key = kdf.derive(new_password)?;
        self.header.kdf = kdf;
        self.key = Some(key);
        Ok(())
    }

    /// 加密成文件字节。每次调用都会换新 nonce，所以同样的条目两次结果不同。
    pub fn seal(&mut self) -> Result<Vec<u8>, VaultError> {
        let key = self.key.as_ref().ok_or(VaultError::Locked)?;
        let payload = Zeroizing::new(
            serde_json::to_vec(&self.entries)
                .map_err(|e| VaultError::Crypto(format!("序列化失败：{e}")))?,
        );
        let mut nonce = [0u8; NONCE_LEN];
        rand::thread_rng().fill_bytes(&mut nonce);
        // nonce 先写进 header——header 参与 AAD，顺序不能反。
        self.header.aead.nonce = base64_encode(&nonce);
        let header_json = serde_json::to_vec(&self.header)
            .map_err(|e| VaultError::Crypto(format!("header 序列化失败：{e}")))?;
        let aad = envelope(&header_json);
        let ctx = XChaCha20Poly1305::new(Key::from_slice(&key[..]));
        let ciphertext = ctx
            .encrypt(
                XNonce::from_slice(&nonce),
                Payload {
                    msg: &payload,
                    aad: &aad,
                },
            )
            .map_err(|_| VaultError::Crypto("加密失败".into()))?;
        let mut out = aad;
        out.extend_from_slice(&ciphertext);
        Ok(out)
    }

    /// 锁定：条目就地清零并释放，密钥丢弃。锁定后可以再 `unlock`。
    pub fn lock(&mut self) {
        for entry in &mut self.entries {
            entry.wipe();
        }
        self.entries.clear();
        self.entries.shrink_to_fit();
        self.key = None;
    }

    fn ensure_unlocked(&self) -> Result<(), VaultError> {
        if self.key.is_some() {
            Ok(())
        } else {
            Err(VaultError::Locked)
        }
    }
}

impl Drop for Vault {
    fn drop(&mut self) {
        self.lock();
    }
}

fn check_password(password: &str) -> Result<(), VaultError> {
    if password.chars().count() < MIN_PASSWORD_LEN {
        return Err(VaultError::WeakPassword);
    }
    Ok(())
}

fn decrypt(
    key: &[u8; KEY_LEN],
    nonce: &[u8],
    ciphertext: &[u8],
    aad: &[u8],
) -> Result<Zeroizing<Vec<u8>>, VaultError> {
    let ctx = XChaCha20Poly1305::new(Key::from_slice(key));
    let plain = ctx
        .decrypt(
            XNonce::from_slice(nonce),
            Payload {
                msg: ciphertext,
                aad,
            },
        )
        // 这里不区分「密码错」与「密文被改」：AEAD 也区分不了。
        .map_err(|_| VaultError::BadPassword)?;
    Ok(Zeroizing::new(plain))
}

#[cfg(test)]
mod tests {
    use super::*;

    const PW: &str = "correct horse battery staple";

    fn sealed_vault() -> Vec<u8> {
        let mut v = Vault::create_with(PW, KdfParams::fast_for_tests(), Some("猫的名字".into())).unwrap();
        v.insert(Entry::password("s1", "生产库", "hunter2")).unwrap();
        v.seal().unwrap()
    }

    #[test]
    fn entries_survive_a_seal_unlock_roundtrip() {
        let bytes = sealed_vault();
        let v = Vault::unlock(&bytes, PW).unwrap();
        assert!(v.is_unlocked());
        let e = v.find("s1").expect("条目丢了");
        assert_eq!(e.name(), "生产库");
        assert_eq!(e.secret(), Some("hunter2"));
        assert_eq!(e.kind(), "password");
        assert_eq!(v.hint(), Some("猫的名字"));
    }

    #[test]
    fn every_kind_of_entry_roundtrips() {
        let mut v = Vault::create_with(PW, KdfParams::fast_for_tests(), None).unwrap();
        v.insert(Entry::password("p", "密码", "hunter2")).unwrap();
        v.insert(Entry::PrivateKey {
            id: "k".into(),
            name: "私钥".into(),
            pem: "-----BEGIN OPENSSH PRIVATE KEY-----\nAAAA\n".into(),
            passphrase: Some("kp".into()),
        })
        .unwrap();
        v.insert(Entry::ProxyCredential {
            id: "x".into(),
            name: "代理".into(),
            username: "u".into(),
            secret: "p".into(),
        })
        .unwrap();
        let bytes = v.seal().unwrap();
        let back = Vault::unlock(&bytes, PW).unwrap();
        assert_eq!(back.entries(), v.entries());
        assert_eq!(back.entries()[1].kind(), "private_key");
        assert_eq!(back.entries()[1].secret(), None, "私钥不走 secret()");
    }

    #[test]
    fn a_wrong_password_is_indistinguishable_from_a_modified_file() {
        let bytes = sealed_vault();
        assert_eq!(Vault::unlock(&bytes, "wrong password 123").err(), Some(VaultError::BadPassword));

        let mut tampered = bytes.clone();
        let at = tampered.len() - 1;
        tampered[at] ^= 0x01;
        assert_eq!(Vault::unlock(&tampered, PW).err(), Some(VaultError::BadPassword));
    }

    /// 这一条是格式设计的全部意义：把 KDF 内存开销改小 → AAD 变 → 解不开。
    #[test]
    fn lowering_the_kdf_cost_in_the_header_breaks_the_vault() {
        let bytes = sealed_vault();
        let (mut header, _, _) = split(&bytes).unwrap();
        header.kdf.m = 8; // 想离线爆破的人会这么干
        let forged_header = serde_json::to_vec(&header).unwrap();
        let (_, _, ciphertext) = split(&bytes).unwrap();
        let mut forged = envelope(&forged_header);
        forged.extend_from_slice(ciphertext);

        assert_eq!(Vault::unlock(&forged, PW).err(), Some(VaultError::BadPassword));
    }

    #[test]
    fn a_short_master_password_is_refused_at_creation() {
        assert_eq!(
            Vault::create_with("short", KdfParams::fast_for_tests(), None).err(),
            Some(VaultError::WeakPassword)
        );
        // 长度按字符数算，不按字节：UTF-8 里一个汉字 3 字节，按字节数会在 4 个字
        // 的时候就「合格」，那是假的。12 个汉字必须正好过线。
        let twelve_cjk = "十二个汉字正好十二个字符";
        assert_eq!(twelve_cjk.chars().count(), 12);
        assert!(twelve_cjk.len() > 12, "字节数远大于 12，正好验证按字符算");
        assert!(Vault::create_with(twelve_cjk, KdfParams::fast_for_tests(), None).is_ok());
    }

    #[test]
    fn duplicate_ids_are_refused_and_upsert_replaces() {
        let mut v = Vault::create_with(PW, KdfParams::fast_for_tests(), None).unwrap();
        v.insert(Entry::password("a", "一", "s")).unwrap();
        assert_eq!(
            v.insert(Entry::password("a", "二", "s")).err(),
            Some(VaultError::DuplicateEntry("a".into()))
        );
        v.upsert(Entry::password("a", "二", "s2")).unwrap();
        assert_eq!(v.entries().len(), 1);
        assert_eq!(v.find("a").unwrap().name(), "二");
        assert_eq!(v.find("a").unwrap().secret(), Some("s2"));
    }

    #[test]
    fn removing_an_unknown_entry_says_so() {
        let mut v = Vault::create_with(PW, KdfParams::fast_for_tests(), None).unwrap();
        assert_eq!(
            v.remove("nope").err(),
            Some(VaultError::EntryNotFound("nope".into()))
        );
    }

    #[test]
    fn a_locked_vault_refuses_writes_and_sealing() {
        let mut v = Vault::create_with(PW, KdfParams::fast_for_tests(), None).unwrap();
        v.insert(Entry::password("a", "一", "s")).unwrap();
        v.lock();
        assert!(!v.is_unlocked());
        assert!(v.entries().is_empty(), "锁定时条目必须清空");
        assert_eq!(v.insert(Entry::password("b", "二", "s")).err(), Some(VaultError::Locked));
        assert_eq!(v.seal().err(), Some(VaultError::Locked));
        assert_eq!(v.change_password(PW).err(), Some(VaultError::Locked));
    }

    #[test]
    fn sealing_twice_never_reuses_a_nonce() {
        let mut v = Vault::create_with(PW, KdfParams::fast_for_tests(), None).unwrap();
        v.insert(Entry::password("a", "一", "s")).unwrap();
        let first = v.seal().unwrap();
        let n1 = v.header().aead.nonce.clone();
        let second = v.seal().unwrap();
        let n2 = v.header().aead.nonce.clone();
        assert_ne!(n1, n2, "每次 seal 必须换 nonce");
        assert_ne!(first, second, "密文不能重复");
        // 两份文件都能解，且内容一致。
        assert_eq!(
            Vault::unlock(&first, PW).unwrap().entries(),
            Vault::unlock(&second, PW).unwrap().entries()
        );
    }

    #[test]
    fn changing_the_password_retires_the_old_one() {
        let mut v = Vault::create_with(PW, KdfParams::fast_for_tests(), None).unwrap();
        v.insert(Entry::password("a", "一", "s")).unwrap();
        let old_salt = v.header().kdf.salt.clone();
        v.change_password("a whole new master password").unwrap();
        assert_ne!(v.header().kdf.salt, old_salt, "换密码必须换盐");
        let bytes = v.seal().unwrap();

        assert_eq!(
            Vault::unlock(&bytes, PW).err(),
            Some(VaultError::BadPassword)
        );
        let back = Vault::unlock(&bytes, "a whole new master password").unwrap();
        assert_eq!(back.find("a").unwrap().secret(), Some("s"));
    }

    #[test]
    fn the_hint_can_be_read_without_the_password() {
        let bytes = sealed_vault();
        assert_eq!(Vault::peek_hint(&bytes).unwrap(), Some("猫的名字".to_string()));
    }

    #[test]
    fn an_unknown_aead_is_refused_by_name() {
        let bytes = sealed_vault();
        let (mut header, _, _) = split(&bytes).unwrap();
        header.aead.alg = "aes-256-gcm".into();
        let (_, _, ciphertext) = split(&bytes).unwrap();
        let mut forged = envelope(&serde_json::to_vec(&header).unwrap());
        forged.extend_from_slice(ciphertext);
        assert!(matches!(Vault::unlock(&forged, PW), Err(VaultError::Unsupported(_))));
    }
}

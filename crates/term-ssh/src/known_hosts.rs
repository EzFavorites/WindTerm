//! 主机密钥校验（`known_hosts`）。
//!
//! 这一段是整个 SSH 层唯一**必须停下来问人**的地方，所以它被写成同步的纯判定：
//! 输入是（策略、主机、端口、文件路径、服务器给的公钥），输出是三档结论之一。
//! 好处是可以用临时文件把三条分支全测到，不需要真的连一台服务器。
//!
//! 判定表：
//!
//! | known_hosts 里的情况 | `Strict` | `Tofu` | `Insecure` |
//! |---|---|---|---|
//! | 记着同一把钥匙 | 接受 | 接受 | 接受（不查） |
//! | 没有这台主机 | **拒绝** | 记下来再接受 | 接受（不查） |
//! | 记着另一把钥匙 | **拒绝** | **拒绝** | 接受（不查） |
//!
//! 中间那一行是重点：`Tofu`（首次使用即信任）只在**第一次**宽松，
//! 一旦记下来就变成 `Strict`。第三行在三档里都不该「顺手覆盖」——
//! 覆盖等于把中间人攻击变成一次静默的写文件。

use std::path::Path;

use russh::keys::known_hosts::{known_host_keys_path, learn_known_hosts_path};
use russh::keys::PublicKey;

use crate::error::{HostKeyProblem, SshError};
use crate::options::HostKeyPolicy;

/// 校验结论。都是「可以用」，区别只在日志里怎么写这一笔。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Decision {
    /// `known_hosts` 里记着的就是这把钥匙。
    Trusted,
    /// 第一次见到这台主机，已把它记进 `known_hosts`。
    Learned,
    /// 策略要求不检查（`Insecure`）。
    Ignored,
}

impl Decision {
    /// 要不要在日志 / 状态栏里提示「这次记录了一把新钥匙」。
    pub fn learned(&self) -> bool {
        matches!(self, Decision::Learned)
    }
}

/// 人的指纹形式（`SHA256:...`）。用户能拿它跟服务器管理员给的核对。
pub fn fingerprint(key: &PublicKey) -> String {
    key.fingerprint(russh::keys::HashAlg::Sha256).to_string()
}

/// 按策略校验服务器公钥。
///
/// `path` 为 `None` 时：`Insecure` 照样放行（它本来就不查），其余两档报
/// 「找不到 known_hosts」——**不**悄悄换成「那就不查了」。
pub fn verify(
    policy: HostKeyPolicy,
    host: &str,
    port: u16,
    path: Option<&Path>,
    key: &PublicKey,
) -> Result<Decision, SshError> {
    if policy == HostKeyPolicy::Insecure {
        return Ok(Decision::Ignored);
    }
    let Some(path) = path else {
        return Err(SshError::HostKey(HostKeyProblem::Unreadable(
            "找不到 known_hosts 的位置（没有家目录，也没有显式指定）".to_string(),
        )));
    };

    match known_host_keys_path(host, port, path) {
        Ok(entries) => {
            // **不用** russh 的 `check_known_hosts_path`：它对 `PublicKey` 整体
            // 比较，而 `PublicKey` 里带着注释。`known_hosts` 的一条记录常写成
            // `host ssh-ed25519 AAAA... user@laptop`，注释和进来那把钥匙不一样，
            // 于是「同一把钥匙」被报成「钥匙变了」——那是把人往中间人恐慌里推的
            // 假警报。这里只比较公钥本体（`key_data`）。
            let mut same_type_line = None;
            for (line, recorded) in entries {
                if recorded.key_data() == key.key_data() {
                    return Ok(Decision::Trusted);
                }
                if recorded.algorithm() == key.algorithm() {
                    same_type_line = Some(line);
                }
            }
            // 同类型但不同钥匙 = 换了钥匙；一把同类型的都没有（比如账本里只有
            // RSA，服务器给 ed25519）算「这台主机没记录过这种钥匙」，与 OpenSSH
            // 一致：按未知处理，交给策略决定。
            if let Some(line) = same_type_line {
                // 把这次的公钥一起带出去：界面的「明确换钥」需要它才能写账本。
                // 去掉注释，理由同上（账本里那行只该有 token + 钥匙本体）。
                let bare = PublicKey::new(key.key_data().clone(), String::new());
                return Err(SshError::HostKey(HostKeyProblem::Changed {
                    host: host.to_string(),
                    port,
                    line,
                    presented: bare.to_openssh().unwrap_or_default(),
                    presented_fingerprint: fingerprint(key),
                }));
            }
            match policy {
                // 已经在上面的 return 里处理掉了，这里只是让穷尽性成立。
                HostKeyPolicy::Insecure => Ok(Decision::Ignored),
                HostKeyPolicy::Strict => Err(SshError::HostKey(HostKeyProblem::Unknown {
                    host: host.to_string(),
                    port,
                    fingerprint: fingerprint(key),
                })),
                HostKeyPolicy::Tofu => {
                    // 去掉注释再写：注释是给人看的自由文本，写进账本只会让下一个人
                    // 以为「这行有什么特别含义」。
                    let bare = PublicKey::new(key.key_data().clone(), String::new());
                    learn_known_hosts_path(host, port, &bare, path)
                        .map_err(|e| SshError::HostKey(HostKeyProblem::Unreadable(e.to_string())))?;
                    Ok(Decision::Learned)
                }
            }
        }
        Err(other) => Err(SshError::HostKey(HostKeyProblem::Unreadable(
            other.to_string(),
        ))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU32, Ordering};

    const CLIENT_PUB: &str = include_str!("../testdata/id_ed25519.pub");
    const HOST_PUB: &str = include_str!("../testdata/host_ed25519.pub");

    fn key(pub_openssh: &str) -> PublicKey {
        PublicKey::from_openssh(pub_openssh.trim()).expect("测试用公钥应能解析")
    }

    /// 每个用例一个独立目录，避免并行跑测试时互相看到对方写下的钥匙。
    fn scratch(tag: &str) -> PathBuf {
        static N: AtomicU32 = AtomicU32::new(0);
        let n = N.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!(
            "term-ssh-kh-{}-{}-{tag}-{n}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        ));
        fs::create_dir_all(&dir).expect("建临时目录");
        dir.join("known_hosts")
    }

    #[test]
    fn an_unknown_host_is_refused_under_strict_with_its_fingerprint() {
        let path = scratch("strict-unknown");
        let e = verify(HostKeyPolicy::Strict, "example.com", 22, Some(&path), &key(HOST_PUB))
            .unwrap_err();
        match e {
            SshError::HostKey(HostKeyProblem::Unknown { fingerprint, port, .. }) => {
                assert_eq!(port, 22);
                assert!(fingerprint.starts_with("SHA256:"), "{fingerprint}");
            }
            other => panic!("应当是 Unknown，实际是 {other:?}"),
        }
        // 拒绝时**不许**写文件：写下去就等于替用户做了「信任」的决定。
        assert!(!path.exists(), "Strict 拒绝后不该建 known_hosts");
    }

    #[test]
    fn tofu_learns_once_then_becomes_strict() {
        let path = scratch("tofu");
        let host_key = key(HOST_PUB);

        let first = verify(HostKeyPolicy::Tofu, "example.com", 22, Some(&path), &host_key).unwrap();
        assert_eq!(first, Decision::Learned);
        assert!(first.learned());

        let text = fs::read_to_string(&path).expect("应该写了 known_hosts");
        assert!(text.contains("example.com"), "{text}");

        let second = verify(HostKeyPolicy::Tofu, "example.com", 22, Some(&path), &host_key).unwrap();
        assert_eq!(second, Decision::Trusted);
    }

    #[test]
    fn a_non_default_port_is_recorded_with_brackets() {
        let path = scratch("port");
        verify(
            HostKeyPolicy::Tofu,
            "example.com",
            2222,
            Some(&path),
            &key(HOST_PUB),
        )
        .unwrap();
        let text = fs::read_to_string(&path).unwrap();
        // OpenSSH 的形状：[host]:port —— 少了方括号，同一个主机名的两个端口会互相覆盖。
        assert!(text.contains("[example.com]:2222"), "{text}");
        assert!(verify(
            HostKeyPolicy::Tofu,
            "example.com",
            2222,
            Some(&path),
            &key(HOST_PUB)
        )
        .unwrap()
            == Decision::Trusted);
    }

    #[test]
    fn a_different_key_for_the_same_host_is_refused_even_under_tofu() {
        let path = scratch("changed");
        verify(HostKeyPolicy::Tofu, "example.com", 22, Some(&path), &key(HOST_PUB)).unwrap();

        // 同一台主机，换了一把钥匙 —— 这就是「钥匙变了」。
        let e = verify(HostKeyPolicy::Tofu, "example.com", 22, Some(&path), &key(CLIENT_PUB))
            .unwrap_err();
        match e {
            // 行号是 2 而不是 1：`learn_known_hosts_path` 给**新建**的空文件
            // 先补一个换行（它无法区分「空文件」和「上一行没换行」），于是
            // 第一条记录落在第 2 行。这是库的行为，不是我们的记录错位——
            // 用户在编辑器里看到的也确实是第 2 行。
            SshError::HostKey(HostKeyProblem::Changed { line, .. }) => assert_eq!(line, 2),
            other => panic!("应当是 Changed，实际是 {other:?}"),
        }
    }

    #[test]
    fn a_stored_key_is_accepted_under_strict() {
        let path = scratch("strict-known");
        verify(HostKeyPolicy::Tofu, "example.com", 22, Some(&path), &key(HOST_PUB)).unwrap();
        let d = verify(HostKeyPolicy::Strict, "example.com", 22, Some(&path), &key(HOST_PUB)).unwrap();
        assert_eq!(d, Decision::Trusted);
    }

    #[test]
    fn insecure_accepts_without_reading_the_file() {
        let missing = PathBuf::from("/nonexistent/definitely/not/here");
        let d = verify(
            HostKeyPolicy::Insecure,
            "example.com",
            22,
            Some(&missing),
            &key(HOST_PUB),
        )
        .unwrap();
        assert_eq!(d, Decision::Ignored);
    }

    #[test]
    fn strict_without_a_known_hosts_path_refuses_instead_of_skipping_the_check() {
        let e = verify(HostKeyPolicy::Strict, "example.com", 22, None, &key(HOST_PUB)).unwrap_err();
        assert!(matches!(e, SshError::HostKey(HostKeyProblem::Unreadable(_))), "{e:?}");
    }

    #[test]
    fn the_fingerprint_is_stable_for_one_key_and_differs_across_keys() {
        let a = fingerprint(&key(HOST_PUB));
        let b = fingerprint(&key(HOST_PUB));
        let c = fingerprint(&key(CLIENT_PUB));
        assert_eq!(a, b);
        assert_ne!(a, c);
        assert!(a.starts_with("SHA256:"));
    }
}

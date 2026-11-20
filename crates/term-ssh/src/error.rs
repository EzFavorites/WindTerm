//! 连接失败的分类。
//!
//! 为什么不像 `term-runtime` 那样只用一个 `SpawnError`：SSH 的失败点比本地
//! shell 多得多，而用户能做的事在每一类里**完全不同**——
//!
//! | 类别 | 用户要做什么 | 例子 |
//! |---|---|---|
//! | `Config` | 改表单 | 没填主机名 / 没选认证方式 |
//! | `Connect` | 查网络、改端口 | 连不上、超时、被防火墙丢包 |
//! | `HostKey` | 判断「是不是中间人」 | 未知主机、密钥变了 |
//! | `Key` | 换私钥文件 / 补口令 | 私钥有口令、格式不认识 |
//! | `Auth` | 换账号 / 换密码 / 加公钥 | 密码错、服务器不认这把钥匙 |
//! | `Channel` | 重连 | 会话通道开不出来 |
//! | `Local` | 修环境（权限、线程） | 建不起运行时、线程起不来 |
//!
//! 把 `HostKey` 单独列出来是关键：它是唯一一类**必须停下来问人**的错误，
//! 拍平成字符串就会被写成「连接失败，是否重试」（而正确答案是「不要重试」）。

use std::fmt;

/// 一个 SSH 连接失败的原因。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SshError {
    /// 配置不足，还没碰到网络。
    Config(String),
    /// 地址解析 / 端口取值失败。
    Resolve(String),
    /// TCP 连不上或超时。
    Connect(String),
    /// 主机密钥不被接受（见 [`HostKeyProblem`]）。
    HostKey(HostKeyProblem),
    /// 读私钥失败（文件、格式、口令）。
    Key(String),
    /// 服务器拒绝认证（见 [`AuthProblem`]）。
    Auth(AuthProblem),
    /// 握手成功但会话通道建立失败。
    Channel(String),
    /// 本机环境问题：线程、运行时、权限。
    Local(String),
}

/// 主机密钥被拒的三种情况。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum HostKeyProblem {
    /// `known_hosts` 里没有这台主机，而策略是 `Strict`。
    Unknown {
        host: String,
        port: u16,
        fingerprint: String,
    },
    /// 记着的是另一把钥匙 —— 要么服务器换了钥匙，要么有人在中间。
    Changed {
        host: String,
        port: u16,
        /// `known_hosts` 的行号，方便用户自己去看那一条。
        line: usize,
        /// 服务器**这一次**给的公钥（openssh 单行，已去掉注释）。
        ///
        /// 为什么要把它带出来：界面要做的唯一正确动作是「明确替换旧记录」，
        /// 而替换必须有新钥匙可写。此前这里只有行号，于是界面除了报错
        /// 什么也做不了——只能让用户自己去命令行 `ssh-keygen -R`。
        /// 带出来的是公钥本体，不含任何私密材料。
        presented: String,
        /// 上面那把钥匙的指纹（`SHA256:...`），给人核对用。
        presented_fingerprint: String,
    },
    /// `known_hosts` 读不了（权限、目录不可写）。
    Unreadable(String),
}

/// 认证被拒。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AuthProblem {
    pub user: String,
    /// `"password"` / `"publickey"` / `"agent"` —— 用户看的是「哪种方式失败了」。
    pub method: &'static str,
    /// 服务器说「这一步算过了，但还要再认证一次」（多因子）。
    pub partial_success: bool,
}

impl fmt::Display for SshError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            SshError::Config(msg) => write!(f, "SSH 配置不完整：{msg}"),
            SshError::Resolve(msg) => write!(f, "无法解析主机：{msg}"),
            SshError::Connect(msg) => write!(f, "无法连接到服务器：{msg}"),
            SshError::HostKey(p) => write!(f, "{p}"),
            SshError::Key(msg) => write!(f, "无法使用私钥：{msg}"),
            SshError::Auth(p) => write!(f, "{p}"),
            SshError::Channel(msg) => write!(f, "无法打开会话通道：{msg}"),
            SshError::Local(msg) => write!(f, "本机环境问题：{msg}"),
        }
    }
}

impl fmt::Display for HostKeyProblem {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            HostKeyProblem::Unknown {
                host,
                port,
                fingerprint,
            } => write!(
                f,
                "首次连接 {host}:{port}，服务器密钥指纹是 {fingerprint}；\
                 确认无误后把它记入 known_hosts（策略改成 Tofu）"
            ),
            HostKeyProblem::Changed {
                host,
                port,
                line,
                presented_fingerprint,
                ..
            } => write!(
                f,
                "警告：{host}:{port} 的主机密钥与 known_hosts 第 {line} 行记录的不一致\
                 （这次给出的是 {presented_fingerprint}）。这可能是服务器换了密钥，\
                 也可能是中间人攻击 —— 已拒绝连接"
            ),
            HostKeyProblem::Unreadable(msg) => write!(f, "无法读写 known_hosts：{msg}"),
        }
    }
}

impl fmt::Display for AuthProblem {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let method = match self.method {
            "password" => "密码",
            "publickey" => "私钥",
            "agent" => "ssh-agent",
            other => other,
        };
        if self.partial_success {
            write!(
                f,
                "用户 {} 的{method}认证只过了第一步（服务器要求多因子）",
                self.user
            )
        } else {
            write!(f, "用户 {} 的{method}认证被拒绝", self.user)
        }
    }
}

impl std::error::Error for SshError {}

impl From<SshError> for term_core::channel::SpawnError {
    /// 应用层只认识 `SpawnError`；SSH 的失败要能被它接住。
    ///
    /// 映射规则：缺配置 → `Config` 类的前身，其余都算 `Program`（「你没法启动
    /// 这个会话」）——**除了**主机密钥，它必须原样保留措辞，否则用户看到的
    /// 就只是「无法启动程序」。
    fn from(e: SshError) -> Self {
        match e {
            SshError::Local(msg) => term_core::channel::SpawnError::Pty(msg),
            other => term_core::channel::SpawnError::Program(other.to_string()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_class_says_what_the_user_can_do() {
        let cases = [
            SshError::Config("没有填写主机名".to_string()),
            SshError::Connect("connection refused".to_string()),
            SshError::Key("私钥有口令".to_string()),
            SshError::Channel("channel open failed".to_string()),
            SshError::Local("建不起运行时".to_string()),
        ];
        for e in cases {
            let text = e.to_string();
            assert!(text.chars().count() > 6, "措辞太短：{text}");
        }
    }

    #[test]
    fn a_changed_host_key_is_not_worded_like_a_failure_to_retry() {
        let e = SshError::HostKey(HostKeyProblem::Changed {
            host: "example.com".to_string(),
            port: 22,
            line: 7,
            presented: "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAInew".to_string(),
            presented_fingerprint: "SHA256:newnewnewnewnewnewnewnewnewnewnewn".to_string(),
        });
        let text = e.to_string();
        assert!(text.contains("中间人"), "{text}");
        assert!(text.contains("已拒绝连接"), "{text}");
        assert!(text.contains("第 7 行"), "{text}");
        // 措辞里要给出这次那把钥匙的指纹，否则用户没法拿它去核对——
        // 只报「变了」等于让人去翻日志。
        assert!(text.contains("SHA256:newnewnew"), "{text}");
        // 而且不能出现「重试」这种把危险变成一次点击的字眼。
        assert!(!text.contains("重试"), "{text}");
    }

    #[test]
    fn an_unknown_host_tells_the_fingerprint_to_compare() {
        let e = SshError::HostKey(HostKeyProblem::Unknown {
            host: "example.com".to_string(),
            port: 2222,
            fingerprint: "SHA256:abc".to_string(),
        });
        let text = e.to_string();
        assert!(text.contains("SHA256:abc"), "{text}");
        assert!(text.contains("2222"), "{text}");
    }

    #[test]
    fn auth_failure_names_the_method_in_chinese() {
        let p = AuthProblem {
            user: "root".to_string(),
            method: "publickey",
            partial_success: false,
        };
        assert_eq!(p.to_string(), "用户 root 的私钥认证被拒绝");

        let half = AuthProblem {
            partial_success: true,
            ..p
        };
        assert!(half.to_string().contains("多因子"), "{half}");
    }
}

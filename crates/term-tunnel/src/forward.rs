//! 转发规则：把命令行里那句 `-L 8080:db.internal:5432` 变成一个值。
//!
//! 三种规则（对应 `ssh` 的 `-L` / `-R` / `-D`）：
//!
//! | 规则 | 谁在监听 | 连接往哪去 |
//! |---|---|---|
//! | `Local`（`-L`） | 本机 | 服务器那边再去连 `target` |
//! | `Remote`（`-R`） | 服务器 | 回到本机连 `target` |
//! | `Socks5`（`-D`） | 本机 | 由 SOCKS5 客户端临时指定（只支持 `CONNECT`） |
//!
//! 解析与校验都在这里做，**纯函数**，不碰网络——所以「`8080:x:22` 该不该被接受」
//! 这类问题是单测问题，不是集成问题。

use std::fmt;

use crate::error::TunnelError;

/// 转发的目标端。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Target {
    pub host: String,
    pub port: u16,
}

impl Target {
    pub fn new(host: impl Into<String>, port: u16) -> Self {
        Self {
            host: host.into(),
            port,
        }
    }
}

impl fmt::Display for Target {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}:{}", self.host, self.port)
    }
}

/// 一条转发规则。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Forward {
    /// `-L [bind:]port:host:port`
    Local { bind: String, target: Target },
    /// `-R [bind:]port:host:port`（`bind_port = 0` 时由服务器挑端口）
    Remote {
        bind_addr: String,
        bind_port: u16,
        target: Target,
    },
    /// `-D [bind:]port`（SOCKS5，只做 `CONNECT`）
    Socks5 { bind: String },
}

impl Forward {
    /// `-L`：本地 `bind` 监听 → 服务器去连 `target`。
    pub fn local(bind: impl Into<String>, host: impl Into<String>, port: u16) -> Self {
        Self::Local {
            bind: bind.into(),
            target: Target::new(host, port),
        }
    }

    /// `-R`：服务器 `bind_addr:bind_port` 监听 → 回到本机连 `target`。
    pub fn remote(
        bind_addr: impl Into<String>,
        bind_port: u16,
        host: impl Into<String>,
        port: u16,
    ) -> Self {
        Self::Remote {
            bind_addr: bind_addr.into(),
            bind_port,
            target: Target::new(host, port),
        }
    }

    /// `-D`：本地 `bind` 上开一个 SOCKS5 代理。
    pub fn socks5(bind: impl Into<String>) -> Self {
        Self::Socks5 { bind: bind.into() }
    }

    /// 规则能不能用。**在碰网络之前**就把错的说清楚——这类错是表单错，不是连接错。
    pub fn validate(&self) -> Result<(), TunnelError> {
        match self {
            Forward::Local { bind, target } => {
                check_bind(bind)?;
                check_target(target)
            }
            Forward::Remote {
                bind_addr,
                target,
                ..
            } => {
                if bind_addr.trim().is_empty() {
                    return Err(TunnelError::Config("远程转发的绑定地址不能为空".into()));
                }
                check_target(target)
            }
            Forward::Socks5 { bind } => check_bind(bind),
        }
    }

    /// 日志 / 状态栏用的一句话。
    pub fn describe(&self) -> String {
        match self {
            Forward::Local { bind, target } => format!("-L {bind} → {target}"),
            Forward::Remote {
                bind_addr,
                bind_port,
                target,
            } => format!("-R {bind_addr}:{bind_port} → {target}"),
            Forward::Socks5 { bind } => format!("-D {bind} (SOCKS5)"),
        }
    }

    /// 解析 `ssh` 风格的转发说明（`-L 8080:host:22` 里箭头后面那一段）。
    ///
    /// 支持三种写法：`port:host:port`、`bind_host:port:host:port`、
    /// `bind:port`（仅 SOCKS5）。**不**支持 IPv6 字面量的方括号写法——
    /// 那需要另一个解析器，而我们的表单是分字段填的，字符串只是给 CLI / 导入用。
    pub fn parse(kind: Kind, spec: &str) -> Result<Self, TunnelError> {
        let parts: Vec<&str> = spec.split(':').collect();
        let bad = || TunnelError::Config(format!("看不懂的转发说明：{spec}"));
        match kind {
            Kind::Local | Kind::Remote => {
                // `port:host:port`（绑 127.0.0.1）或 `bind_host:port:host:port`。
                let (bind_host, bind_port, host, port) = match parts.len() {
                    3 => ("127.0.0.1", parts[0], parts[1], parts[2]),
                    4 => (parts[0], parts[1], parts[2], parts[3]),
                    _ => return Err(bad()),
                };
                if bind_host.trim().is_empty() || host.trim().is_empty() {
                    return Err(bad());
                }
                let bind_port: u16 = bind_port.parse().map_err(|_| bad())?;
                let port: u16 = port.parse().map_err(|_| bad())?;
                if port == 0 {
                    return Err(bad());
                }
                Ok(match kind {
                    Kind::Local => Forward::Local {
                        bind: format!("{bind_host}:{bind_port}"),
                        target: Target::new(host, port),
                    },
                    _ => Forward::Remote {
                        bind_addr: bind_host.to_string(),
                        bind_port,
                        target: Target::new(host, port),
                    },
                })
            }
            Kind::Socks5 => {
                let bind = if spec.contains(':') {
                    spec.to_string()
                } else {
                    format!("127.0.0.1:{spec}")
                };
                let rule = Forward::Socks5 { bind };
                rule.validate()?;
                Ok(rule)
            }
        }
    }
}

/// `parse` 的三种规则类型。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Local,
    Remote,
    Socks5,
}

fn check_bind(bind: &str) -> Result<(), TunnelError> {
    let Some((host, port)) = bind.rsplit_once(':') else {
        return Err(TunnelError::Config(format!(
            "绑定地址要写成 host:port，收到的是 {bind:?}"
        )));
    };
    if host.trim().is_empty() {
        return Err(TunnelError::Config("绑定地址的主机部分不能为空".into()));
    }
    match port.parse::<u16>() {
        Ok(0) => Err(TunnelError::Config(
            "本地监听的端口不能是 0（那会让系统挑一个，用户没法知道连哪）".into(),
        )),
        Ok(_) => Ok(()),
        Err(_) => Err(TunnelError::Config(format!("端口不是数字：{port:?}"))),
    }
}

fn check_target(target: &Target) -> Result<(), TunnelError> {
    if target.host.trim().is_empty() {
        return Err(TunnelError::Config("转发的目标主机不能为空".into()));
    }
    if target.port == 0 {
        return Err(TunnelError::Config("转发的目标端口不能是 0".into()));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_local_rule_reads_as_one_line() {
        let f = Forward::local("127.0.0.1:8080", "db.internal", 5432);
        assert_eq!(f.describe(), "-L 127.0.0.1:8080 → db.internal:5432");
        f.validate().unwrap();
    }

    #[test]
    fn a_missing_target_port_is_refused_before_any_socket_opens() {
        let f = Forward::local("127.0.0.1:8080", "db.internal", 0);
        assert!(matches!(f.validate(), Err(TunnelError::Config(_))));
    }

    #[test]
    fn a_bind_without_a_port_is_refused() {
        let f = Forward::local("localhost", "db", 5432);
        let e = f.validate().unwrap_err();
        assert!(e.to_string().contains("host:port"), "{e}");
    }

    #[test]
    fn a_local_bind_on_port_zero_is_refused_but_a_remote_one_is_allowed() {
        // 本地监听 0 = 系统随便挑，用户不知道连哪 → 拒绝。
        assert!(Forward::local("127.0.0.1:0", "db", 5432).validate().is_err());
        // 远程监听 0 是有意义的：让服务器挑端口再告诉我们（ssh -R 0:... 同理）。
        assert!(Forward::remote("127.0.0.1", 0, "localhost", 3000).validate().is_ok());
    }

    #[test]
    fn the_ssh_style_spec_is_parsed_the_way_ssh_parses_it() {
        assert_eq!(
            Forward::parse(Kind::Local, "8080:db.internal:5432").unwrap(),
            Forward::local("127.0.0.1:8080", "db.internal", 5432)
        );
        assert_eq!(
            Forward::parse(Kind::Local, "0.0.0.0:8080:db.internal:5432").unwrap(),
            Forward::local("0.0.0.0:8080", "db.internal", 5432)
        );
        assert_eq!(
            Forward::parse(Kind::Remote, "9000:localhost:3000").unwrap(),
            Forward::remote("127.0.0.1", 9000, "localhost", 3000)
        );
        assert_eq!(
            Forward::parse(Kind::Socks5, "1080").unwrap(),
            Forward::socks5("127.0.0.1:1080")
        );
    }

    #[test]
    fn a_nonsense_spec_is_a_config_error_not_a_panic() {
        for spec in ["", "8080", "8080:host", "x:host:22", "8080:host:0"] {
            let e = Forward::parse(Kind::Local, spec).unwrap_err();
            assert!(matches!(e, TunnelError::Config(_)), "{spec} → {e:?}");
        }
    }
}

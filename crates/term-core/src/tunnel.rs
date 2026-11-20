//! Tunnels 配置：**一条命名隧道**的纯数据与纯校验。
//!
//! # 与 `term-tunnel` 的关系
//!
//! `term-tunnel` 的词表是 `ssh` 的：`Forward::{Local, Remote, Socks5}`，规则从
//! 一行文本解析（`Forward::parse`）。本模块说的是参考实现界面上的东西——一条隧道
//! 有名字、有归属会话、有「启用」开关、有两个勾选框，七拐八弯的字段拼不出
//! 那句 `-L 8080:db:5432`。
//!
//! 两者之间由 [`TunnelProfile::rule_line`] 一座桥连起来：把结构拼成那行文本，
//! 再交给既有的唯一解析入口 `parse_rule`。**不新开第二条解析路**——拼出来的
//! 字符串照样过 `Forward::parse` 的校验，所以这里漏掉的坏值到那边还会被拦一次。
//!
//! # 为什么放在 `term-core`
//!
//! 与 `trigger` / `onekey` 同一条纪律：要落盘进 `Prefs`（`term-profile` 拥有），
//! 而 `term-profile` 不得依赖 `term-app`（ADR-0014）。纯数据 + 纯函数，零 IO。
//!
//! # 已知边界：动态端口（`listen_port == 0`）
//!
//! 参考实现这一栏留空时显示 `Dynamic port allocation`，由系统挑端口。本仓内核
//! `check_bind` 明确拒绝 0 端口（「那会让系统挑一个，用户没法知道连哪」），
//! 所以 [`validate`](TunnelProfile::validate) 也拒绝它——**在表单这一层就说清楚**，
//! 而不是让用户点完 `Ok` 再看到一句来自 socket 层的抱怨。
//! 要放开得先动 `term-tunnel` 的 bind 路径与端口回填，见 `docs/design/01-sessions-and-connection.md`。

use serde::{Deserialize, Serialize};
use std::fmt;

/// 三种隧道类型，与参考实现 `Type` 下拉同序。
///
/// 与 `term_tunnel::Kind` 一一对应，但**不复用**那个枚举：`term-core` 不依赖
/// `term-tunnel`（那是带线程与 socket 的传输层），而这里只需要一个能落盘的标签。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum TunnelKind {
    /// 本地转发（`ssh -L`）：本机监听，服务器那边再去连目标。
    #[default]
    Local,
    /// 远程转发（`ssh -R`）：服务器监听，回到本机连目标。
    Remote,
    /// SOCKS5 代理（`ssh -D`）：本机监听，目标由客户端临时指定。
    Socks5,
}

impl TunnelKind {
    pub const ALL: [TunnelKind; 3] = [TunnelKind::Local, TunnelKind::Remote, TunnelKind::Socks5];

    /// 界面上那一行字（也是落盘的值）。
    pub const fn name(self) -> &'static str {
        match self {
            TunnelKind::Local => "Local",
            TunnelKind::Remote => "Remote",
            TunnelKind::Socks5 => "Socks5",
        }
    }

    /// 规则行开头的那个字母，与 `parse_rule` 的语法一致。
    pub const fn letter(self) -> char {
        match self {
            TunnelKind::Local => 'L',
            TunnelKind::Remote => 'R',
            TunnelKind::Socks5 => 'D',
        }
    }

    pub fn parse(name: &str) -> Option<TunnelKind> {
        TunnelKind::ALL.into_iter().find(|k| k.name() == name)
    }
}

/// 一条命名隧道（参考实现 `Tunnels` 设置对话框里的一行）。
///
/// 字段与视觉基线逐项对应，顺序也一致：`Name` / `Type` / `Session` / `Listen Port` /
/// `Destination Host` / `Destination Port` / `Status`，然后是两行勾选
/// （`Accept local connections only` / `Automatically open at startup`）。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TunnelProfile {
    /// 用户起的名字。同一份列表里必须唯一——`+` 新建时按它查重。
    pub name: String,
    #[serde(default)]
    pub kind: TunnelKind,
    /// 用哪条会话的 SSH 连接（会话名，不是序号：序号每次启动都会重排）。
    pub session: String,
    /// 本机（或服务器）监听的端口。`0` 见模块头的「已知边界」。
    #[serde(default)]
    pub listen_port: u16,
    /// 目标主机。SOCKS5 不用它。
    #[serde(default)]
    pub dest_host: String,
    /// 目标端口。SOCKS5 不用它。
    #[serde(default)]
    pub dest_port: u16,
    /// `Status` 开关：关掉 = 这条配置还在，但不自动起。
    #[serde(default)]
    pub status: bool,
    /// `Accept local connections only`：勾上就只绑 `127.0.0.1`。
    ///
    /// 缺键按 `true` 读——与 [`Default`] 一致。这一栏默认值只能是安全的那一侧：
    /// 老文件里没有这个键时若默认成 `false`，等于悄悄把隧道放开到全网卡。
    #[serde(default = "yes")]
    pub local_only: bool,
    /// `Automatically open at startup`：启动时自动起（还要 `status` 也为真）。
    #[serde(default)]
    pub auto_open: bool,
}

fn yes() -> bool {
    true
}

impl Default for TunnelProfile {
    fn default() -> Self {
        Self {
            name: String::new(),
            kind: TunnelKind::Local,
            session: String::new(),
            listen_port: 0,
            dest_host: String::new(),
            dest_port: 0,
            status: false,
            local_only: true,
            auto_open: false,
        }
    }
}

impl TunnelProfile {
    /// 新条目：只有类型与勾选有默认值，其余留空等用户填。
    ///
    /// `name` 由调用方给（`+` 时要按现有列表算出不重名的一个）。
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            ..Self::default()
        }
    }

    /// 这一条要不要在启用时真的起：`Status` 与 `auto_open` 是两件事
    /// （前者是「现在这条规则有效」，后者是「下次启动自动起」）。
    pub const fn should_run(&self) -> bool {
        self.status
    }

    /// 运行期的唯一键（`RunningTunnel::key`）。
    ///
    /// 加 `profile:` 前缀是为了与「会话自带的转发」(`session:<名>`) 分开：
    /// 两者都活在同一个 `Vec` 里，前缀让「谁的隧道」在日志与调试里一眼可读，
    /// 也避免一条叫 `prod` 的隧道撞上一条叫 `prod` 的会话。
    pub fn key(&self) -> String {
        format!("profile:{}", self.name)
    }

    /// 绑定地址：勾了「只接受本机连接」就 `127.0.0.1`，否则全网卡。
    pub fn bind_host(&self) -> &'static str {
        if self.local_only {
            "127.0.0.1"
        } else {
            "0.0.0.0"
        }
    }

    /// 拼成 `parse_rule` 认得的那一行。
    ///
    /// - `L` / `R`：`<bind_host>:<listen_port>:<dest_host>:<dest_port>`（四段，
    ///   因为绑定主机要能被 `local_only` 改）；
    /// - `D`：`<bind_host>:<listen_port>`。
    ///
    /// **不做校验**：这一步只管拼字符串，坏值由 `Forward::parse` 拦。
    /// 想先知道对不对就调 [`validate`](Self::validate)。
    pub fn rule_line(&self) -> String {
        match self.kind {
            TunnelKind::Socks5 => format!("D {}:{}", self.bind_host(), self.listen_port),
            _ => format!(
                "{} {}:{}:{}:{}",
                self.kind.letter(),
                self.bind_host(),
                self.listen_port,
                self.dest_host.trim(),
                self.dest_port
            ),
        }
    }

    /// 表单层的校验。返回**第一处**不对的地方（一次改一处，与 `parse_rules` 同风格）。
    ///
    /// 它**不能**替代 `Forward::parse`：那边还要查端口范围与目标主机格式。
    /// 这里拦的是「参考实现表单上有、但拼不进一行文本」的几种：
    /// 名字空、会话空、动态端口（见模块头）、SOCKS5 不需要目标但别的类型必须有。
    ///
    /// 返回 [`TunnelFault`] 而不是 `String`：`term-core` 是边界 crate，4i 判据
    /// 不许它的错误退回字符串（拍平之后调用方只剩一句话，分不清是「用户没填」
    /// 还是「本仓不支持」）。中文文案由 `Display` 给，UI 照原样显示。
    pub fn validate(&self) -> Result<(), TunnelFault> {
        if self.name.trim().is_empty() {
            return Err(TunnelFault::NoName);
        }
        if self.session.trim().is_empty() {
            return Err(TunnelFault::NoSession);
        }
        if self.listen_port == 0 {
            return Err(TunnelFault::DynamicPort);
        }
        if self.kind == TunnelKind::Socks5 {
            return Ok(());
        }
        if self.dest_host.trim().is_empty() {
            return Err(TunnelFault::NoDestHost);
        }
        if self.dest_port == 0 {
            return Err(TunnelFault::NoDestPort);
        }
        Ok(())
    }

    /// 新建条目时的默认名字：`Tunnel 1` / `Tunnel 2` …（取第一个没被占的）。
    pub fn fresh_name(existing: &[TunnelProfile]) -> String {
        for n in 1..=usize::MAX {
            let candidate = format!("Tunnel {n}");
            if !existing.iter().any(|t| t.name == candidate) {
                return candidate;
            }
        }
        unreachable!("显然到不了")
    }
}

/// [`TunnelProfile::validate`] 的失败分类。
///
/// 每一格对应表单上的一栏，而不是一句文案：UI 想加「红框标在哪一栏」时不必
/// 去 parse 字符串。中文说明在 [`Display`] 里，`to_string()` 就是参考实现那句提示。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TunnelFault {
    /// 名字空（列表里靠它认人）。
    NoName,
    /// 没选会话——隧道要借它的 SSH 连接。
    NoSession,
    /// `Listen Port` 是 0：本仓不支持动态端口（见模块头）。
    DynamicPort,
    /// 本地 / 远程转发缺目标主机。
    NoDestHost,
    /// 本地 / 远程转发缺目标端口。
    NoDestPort,
}

impl fmt::Display for TunnelFault {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::NoName => "名字不能为空。",
            Self::NoSession => "要选一条会话：隧道借用它的 SSH 连接。",
            Self::DynamicPort => {
                "本仓暂不支持动态端口（Listen Port 留空 / 0）——系统挑的端口没法写回配置，\
                 请填一个具体端口。"
            }
            Self::NoDestHost => "目标主机不能为空。",
            Self::NoDestPort => "目标端口不能为空（也不是 0）。",
        })
    }
}

/// 启动时该起哪些：`auto_open` 且 `status` 都为真的那几条。
///
/// 抄成独立函数而不是在两处各写一遍 `filter`：这个判据一旦两边不一致，
/// 症状是「开机自动起的和手动点起来的不一样」，而两边都不会报错。
pub fn auto_start(profiles: &[TunnelProfile]) -> Vec<&TunnelProfile> {
    profiles
        .iter()
        .filter(|t| t.auto_open && t.status)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn filled(kind: TunnelKind) -> TunnelProfile {
        TunnelProfile {
            name: "db".into(),
            kind,
            session: "prod".into(),
            listen_port: 8080,
            dest_host: "db.internal".into(),
            dest_port: 5432,
            status: true,
            local_only: true,
            auto_open: false,
        }
    }

    /// 一行文本要能被既有的 `parse_rule` 认出来——这是本模块唯一的出口约定。
    #[test]
    fn a_local_profile_turns_into_the_rule_line_the_parser_knows() {
        let p = filled(TunnelKind::Local);
        assert_eq!(p.rule_line(), "L 127.0.0.1:8080:db.internal:5432");
        // 放开到全网卡：四段写法里的绑定主机段跟着变，而不是另立一个字段。
        let open = TunnelProfile {
            local_only: false,
            ..filled(TunnelKind::Local)
        };
        assert_eq!(open.rule_line(), "L 0.0.0.0:8080:db.internal:5432");
    }

    /// SOCKS5 没有目标端，拼出来只有两段。
    #[test]
    fn a_socks5_profile_carries_no_target() {
        let mut p = filled(TunnelKind::Socks5);
        p.dest_host.clear();
        p.dest_port = 0;
        assert_eq!(p.rule_line(), "D 127.0.0.1:8080");
        p.validate().expect("SOCKS5 不要求目标端");
    }

    /// 远程转发与本地转发只差开头那个字母：绑的是服务器那边，所以 `bind_host`
    /// 仍然是「谁在监听」的本机侧语义，由 `Forward::Remote` 去解释。
    #[test]
    fn remote_and_local_differ_only_by_the_letter() {
        let l = filled(TunnelKind::Local).rule_line();
        let r = filled(TunnelKind::Remote).rule_line();
        assert!(l.starts_with('L') && r.starts_with('R'));
        assert_eq!(l[1..], r[1..]);
    }

    /// 动态端口必须**在表单层**就被拦住并说明原因，不能等到 socket 层。
    #[test]
    fn a_dynamic_port_is_refused_with_a_reason() {
        let p = TunnelProfile {
            listen_port: 0,
            ..filled(TunnelKind::Local)
        };
        let fault = p.validate().unwrap_err();
        assert_eq!(fault, TunnelFault::DynamicPort, "要说清是哪一栏");
        // 用户看到的仍是那句中文：分类是给代码用的，文案不能因此丢掉。
        assert!(fault.to_string().contains("动态端口"), "{fault}");
    }

    /// 四个必填项按参考实现表单的顺序报错，用户一次只看到一处。
    #[test]
    fn the_form_reports_the_first_missing_field() {
        let mut p = filled(TunnelKind::Local);
        p.name = "  ".into();
        assert_eq!(p.validate().unwrap_err(), TunnelFault::NoName);
        p.name = "db".into();
        p.session.clear();
        assert_eq!(p.validate().unwrap_err(), TunnelFault::NoSession);
        p.session = "prod".into();
        p.dest_host.clear();
        assert_eq!(p.validate().unwrap_err(), TunnelFault::NoDestHost);
        p.dest_host = "h".into();
        p.dest_port = 0;
        assert_eq!(p.validate().unwrap_err(), TunnelFault::NoDestPort);
        p.dest_port = 22;
        assert!(p.validate().is_ok());
        // 每一格都得有一句能直接显示的中文——空白文案等于对话框上什么都不说。
        for f in [
            TunnelFault::NoName,
            TunnelFault::NoSession,
            TunnelFault::DynamicPort,
            TunnelFault::NoDestHost,
            TunnelFault::NoDestPort,
        ] {
            assert!(!f.to_string().is_empty(), "{f:?} 没有文案");
        }
    }

    /// `+` 新建时不能撞名：跳过已被占用的那几号。
    #[test]
    fn a_fresh_name_skips_taken_numbers() {
        let taken = vec![
            TunnelProfile {
                name: "Tunnel 1".into(),
                ..filled(TunnelKind::Local)
            },
            TunnelProfile {
                name: "Tunnel 3".into(),
                ..filled(TunnelKind::Local)
            },
        ];
        assert_eq!(TunnelProfile::fresh_name(&taken), "Tunnel 2");
        assert_eq!(TunnelProfile::fresh_name(&[]), "Tunnel 1");
    }

    /// 两个勾选框管的是两件事：`status` 决定「现在起不起」，`auto_open` 只决定
    /// 「下次启动要不要自动起」。开机自动起要求两者同时为真。
    #[test]
    fn status_and_auto_open_are_two_different_flags() {
        let mut p = filled(TunnelKind::Local);
        p.status = false;
        p.auto_open = true;
        assert!(!p.should_run());
        assert!(auto_start(std::slice::from_ref(&p)).is_empty());
        p.status = true;
        assert_eq!(auto_start(std::slice::from_ref(&p)).len(), 1);
        p.auto_open = false;
        assert!(auto_start(std::slice::from_ref(&p)).is_empty());
        assert!(p.should_run(), "手动启用不看 auto_open");
    }

    /// 出厂的「只接受本机连接」是勾上的：更安全的那个方向必须由用户主动放开。
    #[test]
    fn a_new_profile_defaults_to_local_only() {
        let p = TunnelProfile::new("x");
        assert!(p.local_only);
        assert_eq!(p.bind_host(), "127.0.0.1");
        assert_eq!(p.kind, TunnelKind::Local);
        assert!(!p.status, "新条目默认不起");
    }

    /// 落盘形状：缺键的老文件按默认值补齐，不会整份作废。
    #[test]
    fn an_old_file_without_the_new_keys_still_loads() {
        let json = r#"{"name":"db","session":"prod","listen_port":8080,
                       "dest_host":"h","dest_port":22}"#;
        let p: TunnelProfile = serde_json::from_str(json).expect("缺键要能读");
        assert_eq!(p.kind, TunnelKind::Local);
        assert!(!p.status && !p.auto_open);
        assert!(
            p.local_only,
            "缺键必须按安全方向读成 true——悄悄放开到 0.0.0.0 是数据错误"
        );
    }
}

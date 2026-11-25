//! 会话的转发规则与「在跑的隧道」。
//!
//! 两件事分得很开：
//!
//! 1. **规则文本 ↔ `Forward`** 是纯函数（`parse_rule` / `parse_rules` / `split_rules`），
//!    所以「`L 8080:db:5432` 该不该被接受」是单测问题。语法故意只有一行一条：
//!    `<L|R|D> <ssh 风格的一段>`——`ssh` 用 `-L/-R/-D` 三个开关表达类型，
//!    而表单里没有开关，类型就得写在行内。
//! 2. **在跑的隧道**（`RunningTunnel`）只是一个持有者：句柄或隧道本体都在
//!    `term-tunnel` 里，这里只轮询与收摊（投影层不接触线程与队列）。

use crate::ui::*;
use crate::ui::dialogs::ssh_connect;
use term_tunnel::{Forward, Kind, Tunnel, TunnelHandle};

/// 解析一行规则。语法：`L 8080:db.internal:5432`、`R 9000:localhost:3000`、`D 1080`。
pub(in crate::ui) fn parse_rule(line: &str) -> Result<Forward, String> {
    let line = line.trim();
    let (kind, spec) = line
        .split_once(char::is_whitespace)
        .ok_or_else(|| format!("要写成「L|R|D + 一段转发说明」，收到的是 {line:?}"))?;
    let spec = spec.trim();
    let kind = match kind.trim().to_ascii_uppercase().as_str() {
        "L" => Kind::Local,
        "R" => Kind::Remote,
        "D" => Kind::Socks5,
        other => {
            return Err(format!(
                "看不懂的类型 {other:?}（只有 L 本地转发 / R 远程转发 / D SOCKS5）"
            ))
        }
    };
    // 报错里带上原始那一行：多行表单里只说「看不懂的转发说明」没人知道是哪一行。
    Forward::parse(kind, spec).map_err(|e| format!("{e}（这一行是 {line:?}）"))
}

/// 整张表单的规则。空行忽略；第一条错的就返回（用户一次改一行）。
pub(in crate::ui) fn parse_rules(text: &str) -> Result<Vec<Forward>, String> {
    split_rules(text).iter().map(|l| parse_rule(l)).collect()
}

/// 多行文本 → 非空行。表单里存的是用户打的那几行，不是解析后的结构。
pub(in crate::ui) fn split_rules(text: &str) -> Vec<String> {
    text.lines()
        .map(|l| l.trim().to_string())
        .filter(|l| !l.is_empty())
        .collect()
}

/// 隧道这条连接该用哪种主机密钥策略。
///
/// 以前不分青红皂白用 `Tofu`：会话那边用 `Strict` 先问过用户，可连接对话框
/// 确认后重连用的是另一条连接；隧道再走一遍 `Tofu` 等于绕开了那次裁决。
/// 现在只认**账本里已经记着的指纹**：记着就 `Strict` 真校验；没记着就明确
/// 报错让用户先去会话连接里确认指纹，绝不用 Tofu 偷偷记一把新钥匙。
fn tunnel_host_key_policy(config: &SessionConfig) -> Result<term_ssh::HostKeyPolicy, term_ssh::SshError> {
    use term_ssh::SshError;
    if !config.verify_host_key {
        // 会话显式关掉了校验：隧道跟着一致地不查，且绝不写账本。
        return Ok(term_ssh::HostKeyPolicy::Insecure);
    }
    let host = config.host.trim();
    let port = if config.port == 0 { term_ssh::DEFAULT_PORT } else { config.port };
    let opts = term_ssh::SshOptions::new(host, config.username.trim(), term_ssh::AuthMethod::Agent);
    let Some(path) = opts.resolved_known_hosts() else {
        return Err(SshError::Config("找不到 known_hosts 的位置，无法确认隧道用的主机密钥".into()));
    };
    let token = term_ssh::host_token(host, port);
    let known = term_ssh::known_hosts_admin::list(&path)?
        .iter()
        .any(|record| record.hosts.iter().any(|h| h == &token) && record.fingerprint.is_some());
    if known {
        Ok(term_ssh::HostKeyPolicy::Strict)
    } else {
        Err(SshError::Config(format!(
            "主机 {host}:{port} 没有已确认的指纹：先在会话连接里核对并信任一次，再起隧道"
        )))
    }
}

/// 一条在跑（或正在起、或起失败）的隧道。
pub(in crate::ui) struct RunningTunnel {
    /// **这一条是谁**：会话自带的转发用 `session:<会话名>`，Tunnels 设置里配的
    /// 命名隧道用它的名字。收摊、重起、查重全部按这个键，不按会话——
    /// 一个人可以给同一条会话配多条命名隧道（改不同的目标端口），
    /// 按会话收摊会把它们一起杀掉。
    pub(in crate::ui) key: String,
    /// 属于哪条会话（连接参数与 `poll_tunnels` 的回收判据）。
    pub(in crate::ui) session: String,
    /// 规则的可读形式（状态栏用）。
    pub(in crate::ui) rules: Vec<String>,
    /// 起来之后每条规则实际绑定的端口。
    pub(in crate::ui) ports: Vec<u16>,
    /// 起失败 / 中途断了的原因。
    pub(in crate::ui) error: Option<String>,
    /// 还在建的句柄（`None` = 已经有结果了）。
    handle: Option<TunnelHandle>,
    tunnel: Option<Tunnel>,
}

impl RunningTunnel {
    /// 起一条（后台）。
    pub(in crate::ui) fn start(
        key: String,
        session: String,
        opts: term_ssh::SshOptions,
        forwards: Vec<Forward>,
        ctx: &egui::Context,
    ) -> Self {
        let rules = forwards.iter().map(|f| f.describe()).collect();
        let ctx = ctx.clone();
        let handle = term_tunnel::start_in_background(opts, forwards, move || {
            ctx.request_repaint()
        });
        Self {
            key,
            session,
            rules,
            ports: Vec::new(),
            error: None,
            handle: Some(handle),
            tunnel: None,
        }
    }

    /// 一条起不来的隧道（规则解析失败之类，连线程都没起）。
    pub(in crate::ui) fn failed(key: String, session: String, message: String) -> Self {
        Self {
            key,
            session,
            rules: Vec::new(),
            ports: Vec::new(),
            error: Some(message),
            handle: None,
            tunnel: None,
        }
    }

    /// 每帧问一次「起来了吗 / 还好吗」。返回有没有新消息（用来请求重绘）。
    pub(in crate::ui) fn poll(&mut self) -> bool {
        if let Some(handle) = self.handle.as_mut() {
            match handle.try_result() {
                Some(Ok(tunnel)) => {
                    self.ports = tunnel.ports();
                    self.tunnel = Some(tunnel);
                    self.handle = None;
                    return true;
                }
                Some(Err(e)) => {
                    self.error = Some(e.to_string());
                    self.handle = None;
                    return true;
                }
                None => return false,
            }
        }
        // 已经起来的：连接断了 / 某条监听挂了就记一次，别每帧刷同一条。
        if let Some(tunnel) = self.tunnel.as_ref() {
            if !tunnel.is_alive() && self.error.is_none() {
                self.error = Some("隧道断开（SSH 连接已结束）".to_string());
                return true;
            }
        }
        false
    }

    pub(in crate::ui) fn is_ready(&self) -> bool {
        self.tunnel.is_some()
    }

    /// 状态栏那句：规则 + 实际端口；失败时只报失败。
    pub(in crate::ui) fn summary(&self) -> String {
        if let Some(error) = &self.error {
            return format!("Tunnel failed: {error}");
        }
        if self.rules.is_empty() {
            return "Tunnel starting…".to_string();
        }
        self.rules.join(" · ")
    }
}

/// 会话自带转发在 `RunningTunnel` 里的键。加前缀的理由见
/// `TunnelProfile::key`——两者共用同一个 `Vec`。
pub(in crate::ui) fn session_tunnel_key(session: &str) -> String {
    format!("session:{session}")
}

/// 起一条命名隧道时能遇到的失败。
///
/// 五格都是「用户能改的错」，但**来源不同**：表单校验来自 `term-core`（那边
/// 已经是枚举）、规则解析来自既有的 `parse_rule`、后三种是窗口此刻的状态。
/// 拍成一个 `String` 会让「该去改表单」与「该先去连那条会话」混成同一句话，
/// 而且 String 边界基线只许减不许增。中文说明在 `Display` 里。
enum TunnelStartFault {
    /// 表单那一栏没填对。
    Form(term_core::tunnel::TunnelFault),
    /// 拼出来的规则 `Forward::parse` 不认（端口越界之类，表单层拦不到）。
    Rule(String),
    /// 配置里写的会话不在窗口里（没打开）。
    NoSession(String),
    /// 会话还没连过——没有凭据可用。
    NoAuth(String),
    /// 连接参数拼不出来（配置层的问题）。
    Options(String),
}

impl std::fmt::Display for TunnelStartFault {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Form(e) => write!(f, "{e}"),
            Self::Rule(e) => write!(f, "{e}"),
            Self::NoSession(s) => write!(f, "会话「{s}」还没打开：隧道要借它的 SSH 连接。"),
            Self::NoAuth(s) => write!(f, "会话「{s}」还没有凭据（没连过？），隧道起不来。"),
            Self::Options(e) => write!(f, "连接参数有误，隧道无法建立：{e}"),
        }
    }
}

impl TerminalWindow {
    /// 起一条**命名隧道**（Tunnels 设置里配的那条，可以指向任意会话）。
    /// 返回 `true` = 已经在跑了。
    ///
    /// 三步都可能是用户能改的错：表单校验、规则拼不拼得出来、会话在不在。
    /// 失败**不走 `Result`**：调用方（总线开关打开时的逐条重起、启动时自动开）
    /// 对失败的反应只有一种——把这条留成「起不来」让面板说出原因。所以文案
    /// 就地塞进 `RunningTunnel::failed`，调用方只看布尔。
    ///
    /// 会话不在也算失败：隧道借用会话的 SSH 连接，没有那条连接就起不了，
    /// 硬起一条匿名连接比诚实报错更糟。
    pub(in crate::ui) fn start_profile_tunnel(
        &mut self,
        profile: &term_core::tunnel::TunnelProfile,
    ) -> bool {
        match self.try_start_profile_tunnel(profile) {
            Ok(()) => true,
            Err(why) => {
                let key = profile.key();
                self.tunnels.retain(|t| t.key != key);
                self.tunnels.push(RunningTunnel::failed(
                    key,
                    profile.session.clone(),
                    why.to_string(),
                ));
                false
            }
        }
    }

    /// [`start_profile_tunnel`](Self::start_profile_tunnel) 的落地：只负责「起」，
    /// 失败原因原样往上抛，由后者统一记成一条 failed。
    fn try_start_profile_tunnel(
        &mut self,
        profile: &term_core::tunnel::TunnelProfile,
    ) -> Result<(), TunnelStartFault> {
        profile.validate().map_err(TunnelStartFault::Form)?;
        let forward = parse_rule(&profile.rule_line()).map_err(TunnelStartFault::Rule)?;
        let Some(tab) = self.tabs.iter().find(|t| t.config.name == profile.session) else {
            return Err(TunnelStartFault::NoSession(profile.session.clone()));
        };
        let Some(auth) = tab.auth.clone() else {
            return Err(TunnelStartFault::NoAuth(profile.session.clone()));
        };
        let policy = tunnel_host_key_policy(&tab.config)
            .map_err(|e| TunnelStartFault::Options(e.to_string()))?;
        let opts = ssh_connect::options_for(&tab.config, auth, policy)
            .map_err(|e| TunnelStartFault::Options(e.to_string()))?;
        let key = profile.key();
        self.tunnels.retain(|t| t.key != key);
        self.tunnels.push(RunningTunnel::start(
            key,
            profile.session.clone(),
            opts,
            vec![forward],
            &self.ctx,
        ));
        Ok(())
    }

    /// 按 key 停一条隧道（命名隧道的「停」与「关掉这条配置」都走这里）。
    pub(in crate::ui) fn stop_tunnel_by_key(&mut self, key: &str) {
        self.tunnels.retain(|t| t.key != key);
    }

    /// 隧道**总线开关**。
    ///
    /// 关掉 = 现在起着的全停，配置一条不动；打开 = 按各自的 `Status` 重起。
    /// 不做「记住关之前起了哪几条」：那是第二份状态，迟早与实际不同步。
    /// 起失败的那几条不会消失——它们留在 `tunnels` 里带着 `error`，
    /// 状态栏与面板照旧能说明原因。
    pub(in crate::ui) fn set_tunnels_enabled(&mut self, on: bool) {
        self.tunnels_enabled = on;
        if !on {
            self.tunnels.clear();
            return;
        }
        let profiles = self.tunnel_profiles.clone();
        // 起不来的由 `start_profile_tunnel` 自己留成一条 failed，
        // 让面板能说出原因（与规则写错同一条路）——这里只看有没有起来。
        for profile in profiles.iter().filter(|p| p.should_run()) {
            self.start_profile_tunnel(profile);
        }
    }

    /// 会话连上之后起隧道（规则来自会话配置）。
    ///
    /// 用的是**另开一条 SSH 连接**，而不是复用终端那条：`term-ssh` 的会话通道
    /// 与 `term-tunnel` 的 `Authenticated` 是两种形状，合成一条要改动会话线程的
    /// 结构（crate 文档写了「两者失败方式不同」，独立连接正好对上）。
    /// 代价是第二次认证——密码/密钥已经在内存里，所以不会再问用户。
    pub(in crate::ui) fn start_session_tunnel(
        &mut self,
        config: &SessionConfig,
        auth: Option<&term_ssh::AuthMethod>,
    ) {
        let Some(auth) = auth else {
            return;
        };
        let key = session_tunnel_key(&config.name);
        let forwards = match parse_rules(&config.forwards.join("\n")) {
            Ok(rules) if rules.is_empty() => return,
            Ok(rules) => rules,
            Err(e) => {
                self.tunnels.retain(|t| t.key != key);
                self.tunnels
                    .push(RunningTunnel::failed(key, config.name.clone(), e));
                return;
            }
        };
        // 沿用会话那次「已信任指纹」的落盘结果：账本里有 → Strict 真校验；
        // 没有 → 面板上明确提示先去确认指纹，不再用 Tofu 绕开裁决。
        // 参数拼不出来（手写的 `ProxyJump` 链有误）时按「规则写错」同一条路走：
        // 面板上留一条 `failed` 记录说明原因，而不是 panic 或悄悄不起。
        let built = tunnel_host_key_policy(config)
            .map_err(|e| format!("主机密钥未确认，隧道无法建立：{e}"))
            .and_then(|policy| {
                ssh_connect::options_for(config, auth.clone(), policy)
                    .map_err(|e| format!("连接参数有误，隧道无法建立：{e}"))
            });
        let opts = match built {
            Ok(opts) => opts,
            Err(message) => {
                self.tunnels.retain(|t| t.key != key);
                self.tunnels.push(RunningTunnel::failed(key, config.name.clone(), message));
                return;
            }
        };
        self.tunnels.retain(|t| t.key != key);
        self.tunnels.push(RunningTunnel::start(
            key,
            config.name.clone(),
            opts,
            forwards,
            &self.ctx,
        ));
    }

    /// 每帧收一次隧道结果，并收掉「会话已经不在」的那些。
    pub(in crate::ui) fn poll_tunnels(&mut self) {
        let mut changed = false;
        for tunnel in &mut self.tunnels {
            changed |= tunnel.poll();
        }
        // 会话被关掉 / 删掉时，它的隧道跟着走——隧道没有自己的界面，
        // 留着它就是一条谁也看不见、谁也关不掉的连接。
        let open: Vec<String> = self.tabs.iter().map(|t| t.config.name.clone()).collect();
        let before = self.tunnels.len();
        self.tunnels.retain(|t| open.contains(&t.session));
        if self.tunnels.len() != before {
            changed = true;
        }
        if changed {
            self.ctx.request_repaint();
        }
    }

    /// 关掉某条会话的隧道（断开时调）。
    pub(in crate::ui) fn close_tunnels_of(&mut self, session: &str) {
        self.tunnels.retain(|t| t.session != session);
    }

    /// 隧道面板要的行（会话名 / 状态 / 规则 / 失败原因）。
    ///
    /// 只做「读」：面板不解析规则、不轮询、不起连接——那三件事分别属于
    /// `parse_rule`、`poll_tunnels` 与 `start_session_tunnel`。
    pub(in crate::ui) fn tunnel_rows(&self) -> Vec<(String, String, Vec<String>, Option<String>)> {
        self.tunnels
            .iter()
            .map(|t| {
                let state = if t.error.is_some() {
                    "failed".to_string()
                } else if t.is_ready() {
                    if t.ports.is_empty() {
                        "ready".to_string()
                    } else {
                        let ports: Vec<String> = t.ports.iter().map(|p| p.to_string()).collect();
                        format!("listening {}", ports.join(", "))
                    }
                } else {
                    "starting…".to_string()
                };
                (
                    t.session.clone(),
                    state,
                    t.rules.clone(),
                    t.error.clone(),
                )
            })
            .collect()
    }

    /// 某条会话配置里的转发规则（面板编辑的就是这份文本）。
    pub(in crate::ui) fn forwards_of(&self, session: &str) -> Vec<String> {
        self.tabs
            .iter()
            .find(|t| t.config.name == session)
            .map(|t| t.config.forwards.clone())
            .unwrap_or_default()
    }

    /// 停掉一条会话的隧道。
    pub(in crate::ui) fn stop_tunnel(&mut self, session: &str) {
        self.close_tunnels_of(session);
    }

    /// 按会话配置重起隧道（改完规则后调）。
    pub(in crate::ui) fn restart_tunnel(&mut self, session: &str) {
        let Some(tab) = self.tabs.iter().find(|t| t.config.name == session) else {
            return;
        };
        let config = tab.config.clone();
        let auth = tab.auth.clone();
        // 凭据只在内存里，会话断开后就没有了；没有凭据时面板会把规则留在
        // 配置里但起不了连接——这比「偷偷起一条匿名连接」诚实。
        self.start_session_tunnel(&config, auth.as_ref());
    }

    /// 给活动会话加一条转发规则。解析失败**不写配置**，只把原因记下来，
    /// 让用户还能改那一行（和新建会话对话框的规则页同一个约定）。
    pub(in crate::ui) fn add_forward(&mut self, text: &str) {
        match parse_rule(text) {
            // 存**输入语法**而不是 `describe()`：后者带 `→` 箭头，是给人看的，
            // 再解析会失败——那样「改一条规则」就等于把整份配置写坏。
            Ok(_) => {
                let line = text.trim().to_string();
                if line.is_empty() {
                    return;
                }
                let active = self.active_slot();
                let Some(tab) = self.tabs.get_mut(active) else {
                    return;
                };
                tab.config.forwards.push(line);
                let name = tab.config.name.clone();
                self.tunnel_input.clear();
                self.tunnel_error = None;
                self.restart_tunnel(&name);
            }
            Err(e) => self.tunnel_error = Some(e),
        }
    }

    /// 删掉活动会话的第 `index` 条规则。
    pub(in crate::ui) fn remove_forward(&mut self, index: usize) {
        let active = self.active_slot();
        let Some(tab) = self.tabs.get_mut(active) else {
            return;
        };
        if index < tab.config.forwards.len() {
            tab.config.forwards.remove(index);
        }
        let name = tab.config.name.clone();
        self.tunnel_error = None;
        self.restart_tunnel(&name);
    }

    /// 状态栏那一行（`None` = 这条会话没有隧道）。
    pub(in crate::ui) fn tunnel_label(&self) -> Option<String> {
        let name = self.tabs.get(self.active_slot())?.config.name.as_str();
        let tunnel = self.tunnels.iter().find(|t| t.session == name)?;
        Some(tunnel.summary())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_session_without_fingerprint_check_uses_insecure_for_its_tunnel() {
        let mut config = SessionConfig::new(term_core::session::Protocol::Ssh);
        config.verify_host_key = false;
        assert_eq!(
            tunnel_host_key_policy(&config).unwrap(),
            term_ssh::HostKeyPolicy::Insecure
        );
    }

    #[test]
    fn an_ssh_style_local_rule_is_parsed_with_its_kind() {
        assert_eq!(
            parse_rule("L 8080:db.internal:5432").unwrap(),
            Forward::local("127.0.0.1:8080", "db.internal", 5432)
        );
        assert_eq!(
            parse_rule("l 0.0.0.0:8080:db.internal:5432").unwrap(),
            Forward::local("0.0.0.0:8080", "db.internal", 5432)
        );
    }

    #[test]
    fn remote_and_socks_rules_are_recognised_too() {
        assert_eq!(
            parse_rule("R 9000:localhost:3000").unwrap(),
            Forward::remote("127.0.0.1", 9000, "localhost", 3000)
        );
        assert_eq!(parse_rule("D 1080").unwrap(), Forward::socks5("127.0.0.1:1080"));
    }

    #[test]
    fn a_rule_without_a_kind_says_how_to_write_one() {
        let e = parse_rule("8080:db:5432").unwrap_err();
        assert!(e.contains("L|R|D"), "{e}");
    }

    #[test]
    fn an_unknown_kind_is_refused_instead_of_guessed() {
        let e = parse_rule("X 8080:db:5432").unwrap_err();
        assert!(e.contains("看不懂的类型"), "{e}");
    }

    #[test]
    fn a_bad_spec_echoes_the_line_that_is_wrong() {
        // 多行表单里只说「看不懂的转发说明」没人知道改哪一行。
        let e = parse_rules("L 8080:db:5432\nL 8080").unwrap_err();
        assert!(e.contains("L 8080\""), "{e}");
    }

    #[test]
    fn blank_lines_are_dropped_so_an_empty_tunnel_page_means_no_tunnel() {
        assert!(parse_rules("   \n\n  ").unwrap().is_empty());
        assert_eq!(split_rules(" L 1:db:2 \n\n").len(), 1);
    }

    #[test]
    fn every_rule_is_parsed_in_order() {
        let rules = parse_rules("L 8080:db:5432\nD 1080\n").unwrap();
        assert_eq!(rules.len(), 2);
        assert_eq!(rules[0].describe(), "-L 127.0.0.1:8080 → db:5432");
        assert_eq!(rules[1].describe(), "-D 127.0.0.1:1080 (SOCKS5)");
    }

    /// 2026-11-17 Tunnels 设置对话框：表单里填的东西最终要变成**一行规则文本**
    /// 才能进 `Forward`。两处各写一套格式正是这一刀要拆掉的东西——所以
    /// `TunnelProfile::rule_line()` 拼出来必须被 `parse_rule` 一字不差读回来，
    /// 包括 `local_only` 决定的绑定地址（勾上 127.0.0.1 / 松开 0.0.0.0）。
    #[test]
    fn a_profile_round_trips_through_the_rule_parser() {
        use term_core::tunnel::{TunnelKind, TunnelProfile};

        let mut local = TunnelProfile::new("pg");
        local.kind = TunnelKind::Local;
        local.listen_port = 5432;
        local.dest_host = "db.internal".into();
        local.dest_port = 5432;
        assert_eq!(local.rule_line(), "L 127.0.0.1:5432:db.internal:5432");
        local.local_only = false;
        assert_eq!(
            parse_rule(&local.rule_line()).unwrap().describe(),
            "-L 0.0.0.0:5432 → db.internal:5432"
        );

        let mut remote = TunnelProfile::new("web");
        remote.kind = TunnelKind::Remote;
        remote.listen_port = 9000;
        remote.dest_host = "localhost".into();
        remote.dest_port = 3000;
        assert_eq!(
            parse_rule(&remote.rule_line()).unwrap().describe(),
            "-R 127.0.0.1:9000 → localhost:3000"
        );

        // SOCKS5 只要两段：它的目标由客户端在协议里指定，表单上那两栏是空的。
        let mut socks = TunnelProfile::new("socks");
        socks.kind = TunnelKind::Socks5;
        socks.listen_port = 1080;
        assert_eq!(socks.rule_line(), "D 127.0.0.1:1080");
        assert_eq!(
            parse_rule(&socks.rule_line()).unwrap().describe(),
            "-D 127.0.0.1:1080 (SOCKS5)"
        );
    }
}

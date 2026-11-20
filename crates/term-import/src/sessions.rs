//! 外部会话导入的**纯解析层**。
//!
//! 这一层只做「文本 → 会话」，不碰文件系统（读哪个文件由投影层决定），
//! 也**永远不读密码**：`docs/design/09-session-import-and-onboarding.md` 把「密码不迁移」写成
//! 产品底线——各家加密方案不公开，本应用不尝试解密：不可靠，也不合规。导入只带
//! 主机 / 端口 / 用户 / 分组 / 密钥路径 / 跳板链，首次连接让用户重输。
//!
//! 四种来源各有金样本单测，输入是固定文本，跑多少次结果都一样。
//! 格式未知的 WindTerm `profiles/` 用「宽松解析」：同时认 JSON 与 INI 两种
//! 常见序列化形态，其余行忽略而不是整份丢弃——这是刻意的取舍，写在
//! `docs/design/09-session-import-and-onboarding.md`。

use term_core::session::{Protocol, SessionConfig};

/// 一条待导入的会话。**没有密码字段**，这是类型层面的保证。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ImportedSession {
    pub name: String,
    pub host: String,
    pub port: u16,
    pub username: String,
    pub group: String,
    pub protocol: Protocol,
    /// `ProxyJump` 的完整链（`ssh -J` 认这个形状）。
    pub proxy_jump: Option<String>,
    /// `IdentityFile` / `PublicKeyFile`，只记路径不记内容。
    pub identity_file: Option<String>,
}

impl ImportedSession {
    /// 落到会话库里的形状。密码一律不带；WINTERM/SSH 配置里的 `Password`
    /// 字段在解析阶段就没有被读进来。
    pub fn into_config(self) -> SessionConfig {
        let mut cfg = SessionConfig::new(self.protocol);
        cfg.name = self.name;
        cfg.host = self.host;
        cfg.port = self.port;
        cfg.username = self.username;
        cfg.group = self.group;
        cfg.proxy_jump = self.proxy_jump.unwrap_or_default();
        cfg.identity_file = self.identity_file.unwrap_or_default();
        cfg
    }
}

/// 一个 `Host` 块里攒下来的字段。`Host *` 块当默认值，不生成会话。
#[derive(Clone, Default)]
struct Fields {
    names: Vec<String>,
    wildcard: bool,
    host: Option<String>,
    port: Option<u16>,
    user: Option<String>,
    identity: Option<String>,
    proxy: Option<String>,
}

/// 行尾注释：`Port 2222   # bastion` 里的 `# bastion`。
///
/// OpenSSH 自己只认**整行** `#` 注释，行尾 `#` 在它眼里是值的一部分。但手改过的
/// `~/.ssh/config` 里行尾注释极其常见（`import_sessions` 与首次启动
/// 探测都会读到同一批文件），而把 `2222  # bastion` 当端口解析的结果是
/// **静默回落到 22** ——一个能连上但连错机器的缺陷。
///
/// 只在 `#` 前面是空白时裁：`IdentityFile /keys/id#2` 这种没有空白的 `#`
/// 是路径的一部分，裁掉就找不到私钥了。
fn strip_inline_comment(value: &str) -> &str {
    let bytes = value.as_bytes();
    for (i, b) in bytes.iter().enumerate() {
        if *b == b'#' && i > 0 && bytes[i - 1].is_ascii_whitespace() {
            return value[..i].trim_end();
        }
    }
    value
}

fn split_kv(line: &str) -> Option<(&str, &str)> {
    let line = line.trim();
    if line.is_empty() || line.starts_with('#') || line.starts_with(';') {
        return None;
    }
    // OpenSSH 接受 `Key value`、`Key=value`，关键字大小写不敏感。
    if let Some(eq) = line.find('=') {
        let (k, v) = line.split_at(eq);
        return Some((
            k.trim(),
            strip_inline_comment(v[1..].trim()).trim_matches('"'),
        ));
    }
    let mut it = line.splitn(2, char::is_whitespace);
    let k = it.next()?;
    let v = it.next().unwrap_or("").trim();
    Some((k, strip_inline_comment(v).trim_matches('"')))
}

fn apply_key(f: &mut Fields, key: &str, value: &str) {
    if value.is_empty() {
        return;
    }
    if key.eq_ignore_ascii_case("hostname") || key.eq_ignore_ascii_case("host") && !f.names.is_empty() {
        // `HostName` 才是目标地址；`Host` 在 flush 前单独处理。
        if key.eq_ignore_ascii_case("hostname") {
            f.host = Some(value.to_string());
        }
        return;
    }
    if key.eq_ignore_ascii_case("user") {
        f.user = Some(value.to_string());
    } else if key.eq_ignore_ascii_case("port") {
        f.port = value.parse().ok();
    } else if key.eq_ignore_ascii_case("identityfile") {
        f.identity = Some(expand_home(value));
    } else if key.eq_ignore_ascii_case("proxyjump") {
        f.proxy = Some(value.to_string());
    }
    // `Password` / `PasswordEncrypted` / `Passphrase` 等一律不读。
}

fn expand_home(path: &str) -> String {
    if let Some(rest) = path.strip_prefix("~/") {
        if let Some(home) = dirs::home_dir() {
            return home.join(rest).to_string_lossy().into_owned();
        }
    }
    path.to_string()
}

fn flush(out: &mut Vec<ImportedSession>, f: &Fields, defaults: &Fields) {
    if f.wildcard || f.names.is_empty() {
        return;
    }
    for name in &f.names {
        out.push(ImportedSession {
            name: name.clone(),
            host: f.host.clone().or_else(|| defaults.host.clone()).unwrap_or_else(|| name.clone()),
            port: f.port.or(defaults.port).unwrap_or(term_core::config::SSH_PORT),
            username: f.user.clone().or_else(|| defaults.user.clone()).unwrap_or_default(),
            group: "Imported OpenSSH".to_string(),
            protocol: Protocol::Ssh,
            proxy_jump: f.proxy.clone().or_else(|| defaults.proxy.clone()),
            identity_file: f.identity.clone().or_else(|| defaults.identity.clone()),
        });
    }
}

/// 解析 `~/.ssh/config`：`Host` 块、`HostName` / `User` / `Port` /
/// `IdentityFile` / `ProxyJump`（含链），通配符块作为默认值，`Match` 段跳过。
pub fn parse_ssh_config(text: &str) -> Vec<ImportedSession> {
    let mut out = Vec::new();
    let mut defaults = Fields::default();
    let mut current: Option<Fields> = None;
    let mut in_match = false;

    for raw in text.lines() {
        let Some((key, value)) = split_kv(raw) else { continue };
        if key.eq_ignore_ascii_case("match") {
            flush_owned(&mut out, current.take(), &defaults);
            in_match = true;
            continue;
        }
        if key.eq_ignore_ascii_case("host") {
            flush_owned(&mut out, current.take(), &defaults);
            in_match = false;
            let names: Vec<String> = value
                .split_whitespace()
                .filter(|n| !n.contains('*') && !n.contains('?') && !n.starts_with('!'))
                .map(str::to_string)
                .collect();
            let wildcard = value.split_whitespace().any(|n| n.contains('*') || n.contains('?'));
            current = Some(Fields { names, wildcard, ..Fields::default() });
            continue;
        }
        if in_match {
            continue;
        }
        match current.as_mut() {
            Some(f) => apply_key(f, key, value),
            None => apply_key(&mut defaults, key, value),
        }
        // 通配符块里的指令同时作为后续块的默认值。
        if let Some(f) = current.as_ref() {
            if f.wildcard {
                defaults.host = f.host.clone().or(defaults.host.clone());
                defaults.port = f.port.or(defaults.port);
                defaults.user = f.user.clone().or(defaults.user.clone());
                defaults.identity = f.identity.clone().or(defaults.identity.clone());
                defaults.proxy = f.proxy.clone().or(defaults.proxy.clone());
            }
        }
    }
    flush_owned(&mut out, current.take(), &defaults);
    out
}

fn flush_owned(out: &mut Vec<ImportedSession>, f: Option<Fields>, defaults: &Fields) {
    if let Some(f) = f {
        if f.wildcard {
            return;
        }
        flush(out, &f, defaults);
    }
}

/// WindTerm `profiles/` 里的会话导出。格式未公开，这里同时认两种常见形态：
/// JSON（对象/数组里含 `host` 键）与 INI（`[Section]` + `key=value`）。
/// 密码类键一律不读。
pub fn parse_windterm(text: &str) -> Vec<ImportedSession> {
    let trimmed = text.trim_start();
    // `[section]` 也以 `[` 开头，不能只看首字符就当成 JSON 数组。
    // 先试 JSON，解析不出会话再按 INI 读；两种形态都不会误吞对方。
    if trimmed.starts_with('{') || trimmed.starts_with('[') {
        let as_json = parse_windterm_json(text);
        if !as_json.is_empty() {
            return as_json;
        }
    }
    let as_ini = parse_windterm_ini(text);
    if !as_ini.is_empty() {
        return as_ini;
    }
    Vec::new()
}

fn parse_windterm_json(text: &str) -> Vec<ImportedSession> {
    let Ok(value) = serde_json::from_str::<serde_json::Value>(text) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    collect_json(&value, &mut out);
    out
}

fn collect_json(v: &serde_json::Value, out: &mut Vec<ImportedSession>) {
    match v {
        serde_json::Value::Array(items) => {
            for item in items {
                collect_json(item, out);
            }
        }
        serde_json::Value::Object(map) => {
            let get = |keys: &[&str]| -> String {
                for k in keys {
                    if let Some(s) = map.get(*k).and_then(|x| x.as_str()) {
                        return s.to_string();
                    }
                    if let Some(n) = map.get(*k).and_then(|x| x.as_u64()) {
                        return n.to_string();
                    }
                }
                String::new()
            };
            let host = get(&["host", "hostname", "HostName", "Host"]);
            if !host.is_empty() {
                let name = get(&["name", "Name", "sessionName", "label"]);
                let port = get(&["port", "Port"]).parse().unwrap_or(term_core::config::SSH_PORT);
                out.push(ImportedSession {
                    name: if name.is_empty() { host.clone() } else { name },
                    host,
                    port,
                    username: get(&["user", "username", "User", "UserName"]),
                    group: {
                        let g = get(&["group", "Group", "folder"]);
                        if g.is_empty() { "Imported WindTerm".to_string() } else { g }
                    },
                    protocol: Protocol::Ssh,
                    proxy_jump: {
                        let j = get(&["jumpServer", "proxyJump", "JumpServer"]);
                        if j.is_empty() { None } else { Some(j) }
                    },
                    identity_file: {
                        let p = get(&["identityFile", "privateKey", "IdentityFile"]);
                        if p.is_empty() { None } else { Some(p) }
                    },
                });
                return;
            }
            for child in map.values() {
                collect_json(child, out);
            }
        }
        _ => {}
    }
}

fn parse_windterm_ini(text: &str) -> Vec<ImportedSession> {
    let mut out = Vec::new();
    let mut section = String::new();
    let mut map: std::collections::BTreeMap<String, String> = Default::default();

    let commit = |section: &str, map: &std::collections::BTreeMap<String, String>, out: &mut Vec<ImportedSession>| {
        let pick = |keys: &[&str]| -> String {
            for k in keys {
                if let Some(v) = map.get(*k) {
                    return v.clone();
                }
            }
            String::new()
        };
        let host = pick(&["HostName", "host", "Host", "hostname"]);
        if host.is_empty() {
            return;
        }
        let name = pick(&["Name", "name", "Label", "SessionName"]);
        out.push(ImportedSession {
            name: if name.is_empty() { section.to_string() } else { name },
            host,
            port: pick(&["Port", "port"]).parse().unwrap_or(term_core::config::SSH_PORT),
            username: pick(&["User", "user", "UserName", "username"]),
            group: {
                let g = pick(&["Group", "group", "Folder"]);
                if g.is_empty() { "Imported WindTerm".to_string() } else { g }
            },
            protocol: Protocol::Ssh,
            proxy_jump: {
                let j = pick(&["JumpServer", "ProxyJump"]);
                if j.is_empty() { None } else { Some(j) }
            },
            identity_file: {
                let p = pick(&["IdentityFile", "PrivateKey"]);
                if p.is_empty() { None } else { Some(p) }
            },
        });
    };

    for raw in text.lines() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with('#') || line.starts_with(';') {
            continue;
        }
        if line.starts_with('[') && line.ends_with(']') {
            commit(&section, &map, &mut out);
            map.clear();
            section = line[1..line.len() - 1].to_string();
            continue;
        }
        if let Some((k, v)) = split_kv(line) {
            // 密码类键直接略过，连进 map 都不进。
            if !["password", "passwordencrypted", "passphrase"].contains(&k.to_ascii_lowercase().as_str()) {
                map.insert(k.to_string(), v.to_string());
            }
        }
    }
    commit(&section, &map, &mut out);
    out
}

/// PuTTY 在 macOS 上的会话文件（`~/.putty/sessions/<name>`）是 INI 风格。
/// Windows 注册表导出不在本机可读范围内——投影层读不到就返回空，不伪造。
pub fn parse_putty(text: &str) -> Vec<ImportedSession> {
    let mut out = Vec::new();
    let mut section = "Putty Session".to_string();
    let mut map: std::collections::BTreeMap<String, String> = Default::default();

    let commit = |section: &str, map: &std::collections::BTreeMap<String, String>, out: &mut Vec<ImportedSession>| {
        let pick = |keys: &[&str]| -> String {
            for k in keys {
                if let Some(v) = map.get(*k) {
                    return v.clone();
                }
            }
            String::new()
        };
        let host = pick(&["HostName", "hostname"]);
        if host.is_empty() {
            return;
        }
        out.push(ImportedSession {
            name: section.to_string(),
            host,
            port: pick(&["PortNumber", "port"]).parse().unwrap_or(term_core::config::SSH_PORT),
            username: pick(&["UserName", "username", "User"]),
            group: "Imported PuTTY".to_string(),
            protocol: Protocol::Ssh,
            proxy_jump: None,
            identity_file: {
                let p = pick(&["PublicKeyFile", "IdentityFile"]);
                if p.is_empty() { None } else { Some(p) }
            },
        });
    };

    for raw in text.lines() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with('#') || line.starts_with(';') {
            continue;
        }
        if line.starts_with('[') && line.ends_with(']') {
            commit(&section, &map, &mut out);
            map.clear();
            section = line[1..line.len() - 1].to_string();
            continue;
        }
        if let Some((k, v)) = split_kv(line) {
            map.insert(k.to_string(), v.to_string());
        }
    }
    commit(&section, &map, &mut out);
    out
}

/// 本机可用的 Shell 会话：`/etc/shells` 每行一个路径，去重、跳过注释；
/// 当前 `$SHELL` 排在最前（它一定是最该出现的那一条）。
/// 这些是**本地**会话，不是 SSH——协议写成 `Shell`，host 留空。
pub fn parse_shell_sessions(shells_text: &str, current: &str) -> Vec<ImportedSession> {
    let mut paths: Vec<String> = Vec::new();
    if !current.trim().is_empty() {
        paths.push(current.trim().to_string());
    }
    for line in shells_text.lines() {
        let p = line.trim();
        if p.is_empty() || p.starts_with('#') || !p.starts_with('/') {
            continue;
        }
        if !paths.iter().any(|x| x == p) {
            paths.push(p.to_string());
        }
    }
    paths
        .into_iter()
        .map(|p| {
            let name = p.rsplit('/').next().unwrap_or("shell").to_string();
            ImportedSession {
                name,
                host: String::new(),
                port: 0,
                username: String::new(),
                group: "Imported Shell Sessions".to_string(),
                protocol: Protocol::Shell,
                proxy_jump: None,
                identity_file: None,
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    const SSH_CONFIG: &str = r#"
# 全局默认
Host *
    User ops
    IdentityFile ~/.ssh/id_ed25519

Host prod-web
    HostName 10.0.0.11
    Port 2222
    User deploy
    ProxyJump bastion,jump2

Host prod-db
    HostName db.internal
    User admin
    IdentityFile /keys/db.pem

Host legacy-*
    HostName old.example.com

Match host *.internal
    ProxyJump mgmt
"#;

    #[test]
    fn ssh_config_parses_real_hosts_wildcards_and_jump_chains() {
        let got = parse_ssh_config(SSH_CONFIG);
        assert_eq!(got.len(), 2, "通配符块与 Match 段都不该生成会话：{got:?}");
        assert_eq!(got[0].name, "prod-web");
        assert_eq!(got[0].host, "10.0.0.11");
        assert_eq!(got[0].port, 2222);
        assert_eq!(got[0].username, "deploy");
        assert_eq!(got[0].proxy_jump.as_deref(), Some("bastion,jump2"));
        assert!(got[0].identity_file.as_deref().unwrap().ends_with("id_ed25519"));
        assert_eq!(got[1].name, "prod-db");
        assert_eq!(got[1].username, "admin");
        assert_eq!(got[1].identity_file.as_deref(), Some("/keys/db.pem"));
        // 通配符块里的 User 作为默认值落到没有写 User 的块上。
        assert_eq!(got[0].group, "Imported OpenSSH");
    }

    #[test]
    fn ssh_config_works_with_equals_and_mixed_case() {
        let got = parse_ssh_config("HOST=alpha\n  hostname=Alpha.Example\n  PORT=2022\n");
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].host, "Alpha.Example");
        assert_eq!(got[0].port, 2022);
    }

    #[test]
    fn passwords_are_never_read() {
        let got = parse_ssh_config("Host x\n  HostName x.example\n  Password hunter2\n");
        assert_eq!(got.len(), 1);
        assert!(format!("{:?}", got[0]).contains("x.example"));
        assert!(!format!("{:?}", got[0]).contains("hunter2"));
    }

    #[test]
    fn windterm_json_and_ini_both_import() {
        let json = r#"{"sessions":[{"name":"wt-a","host":"a.example","port":2201,"user":"u","group":"Prod"}]}"#;
        let got = parse_windterm(json);
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].name, "wt-a");
        assert_eq!(got[0].port, 2201);
        assert_eq!(got[0].group, "Prod");

        let ini = "[wt-b]\nHostName=b.example\nPort=2202\nUser=v\nPassword=secret\nGroup=Stage\n";
        let got = parse_windterm(ini);
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].name, "wt-b");
        assert_eq!(got[0].host, "b.example");
        assert!(!format!("{:?}", got[0]).contains("secret"));
    }

    #[test]
    fn putty_sessions_import_hostname_port_and_user() {
        let got = parse_putty("[office]\nHostName=o.example\nPortNumber=2222\nUserName=bob\nProtocol=ssh\n");
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].name, "office");
        assert_eq!(got[0].port, 2222);
        assert_eq!(got[0].username, "bob");
    }

    #[test]
    fn shell_sessions_put_current_shell_first_and_local_protocol() {
        let got = parse_shell_sessions("/bin/zsh\n# comment\n/bin/bash\n/bin/zsh\n", "/bin/zsh");
        assert_eq!(got.len(), 2);
        assert_eq!(got[0].name, "zsh");
        assert_eq!(got[0].protocol, Protocol::Shell);
        assert_eq!(got[1].name, "bash");
        assert!(got.iter().all(|s| s.host.is_empty()));
    }

    #[test]
    fn inline_comments_do_not_swallow_the_value() {
        // 行尾注释是手改文件里最常见的形态。`Port 2222 # bastion` 若把整串
        // 交给 `parse::<u16>()`，端口会静默回落到 22——连得上、连错机器。
        let got = parse_ssh_config(
            "Host web   # 生产\n  HostName web.example  # 主入口\n  Port 2222  # 跳板\n  User = ops  # 运维\n",
        );
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].host, "web.example");
        assert_eq!(got[0].port, 2222, "行尾注释不该把端口吃掉");
        assert_eq!(got[0].username, "ops");
        // 没有空白的 `#` 是值的一部分（私钥文件名里很常见），不许裁。
        let got = parse_ssh_config("Host k\n  HostName k.example\n  IdentityFile /keys/id#2\n");
        assert!(got[0].identity_file.as_deref().unwrap().ends_with("id#2"));
    }

    #[test]
    fn garbage_never_panics_and_never_invents_sessions() {
        assert!(parse_ssh_config("not a config at all\n").is_empty());
        assert!(parse_windterm("{ broken json").is_empty());
        assert!(parse_putty("").is_empty());
        assert!(parse_shell_sessions("", "").is_empty());
    }

    #[test]
    fn into_config_carries_jump_and_identity_but_no_password() {
        let got = parse_ssh_config("Host x\n  HostName x.example\n  ProxyJump bastion\n  IdentityFile /k/x\n");
        let cfg = got[0].clone().into_config();
        assert_eq!(cfg.proxy_jump, "bastion");
        assert_eq!(cfg.identity_file, "/k/x");
        assert_eq!(cfg.protocol, Protocol::Ssh);
    }
}

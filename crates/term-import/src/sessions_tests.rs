//! 会话导入解析器的单测：四种旧来源金样本 + 原版 `user.sessions` 九条协议分流。
//!
//! 输入全部是固定文本、合成值；真实用户的会话值一条都不进仓库。挂成子模块的方式
//! 见 `sessions.rs` 末尾的 `#[cfg(test)]` + `#[path = …]` 两行声明。

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

// ---- 原版 user.sessions：9 条协议分流（全部合成数据，不含真实会话值） ----

/// 8 条 Shell + 1 条 Serial 的合成样本。字段形状照原版（扁平键、无 host），
/// 值一律用 `unit-test-*` / `/dev/ttyFAKE` 这类绝不会是真实用户的串。
const ORIGINAL_USER_SESSIONS: &str = r#"[
  {"session.uuid":"u0","session.protocol":"Serial","session.target":"/dev/ttyFAKE0",
   "session.dataType":"binary"},
  {"session.uuid":"u1","session.protocol":"Shell","session.label":"shell-one",
   "session.target":"/bin/synthetic-sh","process.arguments":"-l","process.workingDirectory":"/tmp/unit-1",
   "session.group":"Local"},
  {"session.uuid":"u2","session.protocol":"Shell","session.label":"shell-two",
   "session.target":"/bin/synthetic-sh","process.arguments":"-c 'echo hi'","process.workingDirectory":"/tmp/unit-2"},
  {"session.uuid":"u3","session.protocol":"Shell","session.label":"shell-three",
   "session.target":"/bin/synthetic-sh","process.arguments":"-l","process.workingDirectory":"/tmp/unit-3"},
  {"session.uuid":"u4","session.protocol":"Shell","session.label":"shell-four",
   "session.target":"/bin/synthetic-sh","process.arguments":"-l","process.workingDirectory":"/tmp/unit-4"},
  {"session.uuid":"u5","session.protocol":"Shell","session.label":"shell-five",
   "session.target":"/bin/synthetic-sh","process.arguments":"-l","process.workingDirectory":"/tmp/unit-5"},
  {"session.uuid":"u6","session.protocol":"Shell","session.label":"shell-six",
   "session.target":"/bin/synthetic-sh","process.arguments":"-l","process.workingDirectory":"/tmp/unit-6"},
  {"session.uuid":"u7","session.protocol":"Shell","session.label":"shell-seven",
   "session.target":"/bin/synthetic-sh","process.arguments":"-l","process.workingDirectory":"/tmp/unit-7"},
  {"session.uuid":"u8","session.protocol":"Shell","session.label":"shell-eight",
   "session.target":"/bin/synthetic-sh","process.arguments":"-l","process.workingDirectory":"/tmp/unit-8"}
]"#;

#[test]
fn original_user_sessions_split_into_eight_shell_and_one_serial() {
    let got = parse_windterm_sessions(ORIGINAL_USER_SESSIONS);
    assert_eq!(got.len(), 9, "原版 9 条一条都不许丢：{got:?}");
    assert_eq!(got.iter().filter(|s| s.protocol == Protocol::Shell).count(), 8);
    assert_eq!(got.iter().filter(|s| s.protocol == Protocol::Serial).count(), 1);
    // Serial 那条没有 label，名字回落到 target。
    assert_eq!(got[0].protocol, Protocol::Serial);
    assert_eq!(got[0].name, "/dev/ttyFAKE0");
    // 其余八条按数组顺序原样保留。
    assert_eq!(got[1].name, "shell-one");
    assert_eq!(got[8].name, "shell-eight");
}

#[test]
fn shell_target_goes_to_shell_command_and_never_to_host() {
    let got = parse_windterm_sessions(ORIGINAL_USER_SESSIONS);
    let shell = &got[1];
    assert_eq!(shell.shell_command.as_deref(), Some("/bin/synthetic-sh"));
    assert!(shell.host.is_empty(), "Shell 的 target 不是远端地址");
    assert_eq!(shell.shell_arguments.as_deref(), Some("-l"));
    assert_eq!(shell.working_dir.as_deref(), Some("/tmp/unit-1"));
    assert_eq!(shell.group, "Local");
    // 没写 group 的那条走来源默认分组。
    assert_eq!(got[2].group, "Imported WindTerm");
}

#[test]
fn serial_target_goes_to_host_and_has_no_shell_command() {
    let got = parse_windterm_sessions(ORIGINAL_USER_SESSIONS);
    let serial = &got[0];
    assert_eq!(serial.host, "/dev/ttyFAKE0");
    assert_eq!(serial.shell_command, None);
    // into_config 不把 Shell 程序误塞进会话的主机名。
    let cfg = serial.clone().into_config();
    assert_eq!(cfg.host, "/dev/ttyFAKE0");
    assert!(cfg.shell_command.is_empty());
}

#[test]
fn original_sessions_never_read_password_like_keys() {
    let text = r#"[{"session.protocol":"Shell","session.label":"x",
        "session.target":"/bin/sh","session.password":"SENTINEL-PW",
        "password":"SENTINEL-PW","process.password":"SENTINEL-PW"}]"#;
    let got = parse_windterm_sessions(text);
    assert_eq!(got.len(), 1);
    assert!(!format!("{got:?}").contains("SENTINEL-PW"));
    assert!(!format!("{:?}", got[0].clone().into_config()).contains("SENTINEL-PW"));
}

#[test]
fn unknown_protocol_and_garbage_do_not_invent_sessions() {
    assert!(parse_windterm_sessions("[{\"session.protocol\":\"Quantum\"}]").is_empty());
    assert!(parse_windterm_sessions("{ broken").is_empty());
    assert!(parse_windterm_sessions("[]").is_empty());
}

#[test]
fn into_config_does_not_erase_the_default_shell_arguments() {
    // `SessionConfig::new` 给 shell_arguments 的出厂值是 `-1 -1`；导进来一条
    // 没有 arguments 的 Shell 时不该把它清成空串。
    let cfg = parse_windterm_sessions(
        r#"[{"session.protocol":"Shell","session.label":"x","session.target":"/bin/sh"}]"#,
    )[0]
    .clone()
    .into_config();
    assert_eq!(cfg.shell_arguments, "-1 -1");
    assert_eq!(cfg.shell_command, "/bin/sh");
}

//! `ssh_ask.rs` 里那三项（跳板链 / agent 转发 / 私钥文件）的单测。
//!
//! 拆成独立文件的理由与 `session_nav_tests.rs` 相同：verify 要求
//! `ui/**/*.rs` ≤ 450 行。这里钉住的是「表单里填的三项真的进了连接参数」——
//! 在这一批之前它们能填、能存、不生效，而这恰好是单测最容易漏的部分：
//! 出错的形态是「什么都不报」，不是「报一个错」。

use super::*;

fn ssh_session(auth: AuthMethod) -> SessionConfig {
    let mut c = SessionConfig::new(Protocol::Ssh);
    c.host = "example.com".to_string();
    c.port = 2222;
    c.username = "root".to_string();
    c.auth = auth;
    c
}

/// 一个本次测试独占的临时目录（同 `ssh_ask.rs` 里既有用例的命名口径）。
fn temp_dir(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("desk-ssh-{tag}-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("建目录");
    dir
}

// ── ProxyJump ──────────────────────────────────────────────────────────

#[test]
fn an_empty_proxy_jump_means_a_direct_connection() {
    // 绝大多数会话没有跳板：空串必须得到空链，而不是「解析失败」。
    let c = ssh_session(AuthMethod::Password);
    let opts = options_for(&c, SshAuth::Agent, HostKeyPolicy::Strict).expect("空跳板链不该报错");
    assert!(
        opts.proxy_jump.is_empty(),
        "空 ProxyJump 应当直连，实际 {:?}",
        opts.proxy_jump
    );
}

#[test]
fn none_also_means_a_direct_connection() {
    // `ssh_config` 里写 `ProxyJump none` 是显式关掉继承来的跳板，
    // 与空串同义：不能因为「有字」就当成一条跳。
    let mut c = ssh_session(AuthMethod::Password);
    c.proxy_jump = " none ".to_string();
    let opts = options_for(&c, SshAuth::Agent, HostKeyPolicy::Strict).expect("none 应当直连");
    assert!(opts.proxy_jump.is_empty());
}

#[test]
fn a_single_hop_reaches_the_options() {
    let mut c = ssh_session(AuthMethod::Password);
    c.proxy_jump = "deploy@bastion:2222".to_string();
    let opts = options_for(&c, SshAuth::Agent, HostKeyPolicy::Strict).expect("合法一跳");
    assert_eq!(opts.proxy_jump.len(), 1);
    assert_eq!(opts.proxy_jump[0].host, "bastion");
    assert_eq!(opts.proxy_jump[0].port, 2222);
    assert_eq!(opts.proxy_jump[0].user.as_deref(), Some("deploy"));
}

#[test]
fn a_multi_hop_chain_keeps_its_order() {
    // 顺序是语义的一部分（先 bastion 再 hop2），掉个个儿就连不上。
    // 解析细节由 `term-ssh` 的单测钉住，这里只验「宽度与次序原样过桥」。
    let mut c = ssh_session(AuthMethod::Password);
    c.proxy_jump = "deploy@bastion:2222, ops@hop2".to_string();
    let opts = options_for(&c, SshAuth::Agent, HostKeyPolicy::Strict).expect("合法两跳");
    let hosts: Vec<&str> = opts.proxy_jump.iter().map(|h| h.host.as_str()).collect();
    assert_eq!(hosts, vec!["bastion", "hop2"]);
    assert_eq!(opts.proxy_jump[1].port, term_ssh::DEFAULT_PORT);
}

#[test]
fn a_malformed_proxy_jump_is_an_error_not_a_silent_direct_connection() {
    // 「user@」缺用户名、缺主机——`parse_proxy_jump` 会拒绝。这里要的是
    // **传出来**：吞掉它就等于把用户写的跳板链静默换成直连。
    let mut c = ssh_session(AuthMethod::Password);
    c.proxy_jump = "user@".to_string();
    let err =
        options_for(&c, SshAuth::Agent, HostKeyPolicy::Strict).expect_err("非法跳板链必须报错");
    assert!(matches!(err, SshError::Config(_)), "{err:?}");
    // 文案要能对上用户填的那一行，否则表单里不知道改哪。
    assert!(err.to_string().contains("ProxyJump"), "{err}");
}

#[test]
fn a_bad_port_in_the_chain_is_also_reported() {
    let mut c = ssh_session(AuthMethod::Password);
    c.proxy_jump = "bastion:0".to_string();
    assert!(options_for(&c, SshAuth::Agent, HostKeyPolicy::Strict).is_err());
}

// ── Agent forwarding ───────────────────────────────────────────────────

#[test]
fn agent_forwarding_is_carried_over_verbatim() {
    let mut c = ssh_session(AuthMethod::Agent);
    c.agent_forward = false;
    assert!(
        !options_for(&c, SshAuth::Agent, HostKeyPolicy::Strict)
            .expect("直连")
            .agent_forward
    );

    c.agent_forward = true;
    assert!(
        options_for(&c, SshAuth::Agent, HostKeyPolicy::Strict)
            .expect("直连")
            .agent_forward,
        "-A 勾上了就必须真的下发"
    );
}

// ── IdentityFile ───────────────────────────────────────────────────────

#[test]
fn a_configured_identity_file_wins_over_the_default_key() {
    let dir = temp_dir("identity-wins");
    let ssh_dir = dir.join(".ssh");
    std::fs::create_dir_all(&ssh_dir).expect("建 .ssh");
    // 默认私钥也在——用来证明「填了就以填的为准」，而不是又回落到它。
    std::fs::write(ssh_dir.join("id_ed25519"), b"x").expect("写默认钥匙");
    let chosen = dir.join("work.pem");
    std::fs::write(&chosen, b"x").expect("写指定钥匙");

    let mut c = ssh_session(AuthMethod::PublicKey);
    c.identity_file = chosen.to_string_lossy().into_owned();
    match ask(&c, None, Some(&dir)) {
        Ask::Connect(SshAuth::PrivateKey { path, passphrase }) => {
            assert_eq!(path, chosen, "应当用表单里填的那把");
            assert!(passphrase.is_none());
        }
        other => panic!("应当用指定的私钥，实际是 {other:?}"),
    }
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_tilde_identity_file_is_expanded_against_home() {
    let dir = temp_dir("identity-tilde");
    let ssh_dir = dir.join(".ssh");
    std::fs::create_dir_all(&ssh_dir).expect("建 .ssh");
    std::fs::write(ssh_dir.join("id_work"), b"x").expect("写钥匙");

    let mut c = ssh_session(AuthMethod::PublicKey);
    c.identity_file = "~/.ssh/id_work".to_string();
    match ask(&c, None, Some(&dir)) {
        Ask::Connect(SshAuth::PrivateKey { path, .. }) => {
            assert_eq!(path, ssh_dir.join("id_work"));
        }
        other => panic!("`~` 必须展开到家目录，实际是 {other:?}"),
    }
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_missing_identity_file_is_reported_by_name() {
    // 不允许静默改成 agent / 密码：用户选了文件，就得看见「这个文件不在」。
    let dir = temp_dir("identity-missing");
    let missing = dir.join("nope.pem");
    let mut c = ssh_session(AuthMethod::PublicKey);
    c.identity_file = missing.to_string_lossy().into_owned();

    match ask(&c, None, Some(&dir)) {
        Ask::Unsupported(msg) => {
            assert!(msg.contains("私钥文件不存在"), "{msg}");
            // 路径必须出现在文案里，否则用户不知道该改哪一行。
            assert!(msg.contains("nope.pem"), "{msg}");
        }
        other => panic!("应当明确报错，实际是 {other:?}"),
    }
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn an_identity_file_is_ignored_for_password_and_agent_sessions() {
    // 密码会话里那个残留路径不该改变认证方式（两者互不干涉）。
    let mut c = ssh_session(AuthMethod::Password);
    c.identity_file = "/definitely/not/here".to_string();
    assert_eq!(
        ask(&c, Some("s3cret"), None),
        Ask::Connect(SshAuth::Password {
            password: "s3cret".to_string()
        })
    );

    let mut c = ssh_session(AuthMethod::Agent);
    c.identity_file = "/definitely/not/here".to_string();
    assert_eq!(ask(&c, None, None), Ask::Connect(SshAuth::Agent));
}

#[test]
fn an_empty_identity_file_falls_back_to_the_default_key_order() {
    // 空串 = 没填，才轮到 `~/.ssh/id_*` 的探测顺序。
    let dir = temp_dir("identity-empty");
    let ssh_dir = dir.join(".ssh");
    std::fs::create_dir_all(&ssh_dir).expect("建 .ssh");
    std::fs::write(ssh_dir.join("id_rsa"), b"x").expect("写钥匙");

    let mut c = ssh_session(AuthMethod::PublicKey);
    c.identity_file = "   ".to_string();
    match ask(&c, None, Some(&dir)) {
        Ask::Connect(SshAuth::PrivateKey { path, .. }) => {
            assert!(path.ends_with(".ssh/id_rsa"), "{path:?}");
        }
        other => panic!("空白 IdentityFile 应当退回默认钥匙，实际是 {other:?}"),
    }
    let _ = std::fs::remove_dir_all(&dir);
}

// ── 出网代理（HTTP CONNECT / SOCKS5） ───────────────────────────

#[test]
fn an_http_proxy_reaches_the_connection_options() {
    let mut c = ssh_session(AuthMethod::Agent);
    c.proxy_type = ProxyType::Http.tag().to_string();
    c.proxy_host = " gw.example ".to_string();
    c.proxy_port = 3128;
    let opts = options_for(&c, SshAuth::Agent, HostKeyPolicy::Strict).expect("代理应当能构造");
    let p = opts.outbound_proxy.expect("选了 Http proxy 就必须有出网代理");
    assert_eq!(p.kind, ProxyKind::Http);
    assert_eq!(p.host, "gw.example");
    assert_eq!(p.port, 3128);
    assert!(p.auth.is_none(), "凭据不落盘");
    // 出网代理与 ProxyJump 互斥：不能同时带一条 -J。
    assert!(opts.proxy_jump.is_empty());
}

#[test]
fn a_socks5_proxy_reaches_the_connection_options() {
    let mut c = ssh_session(AuthMethod::Agent);
    c.proxy_type = ProxyType::Socks5.tag().to_string();
    c.proxy_host = "127.0.0.1".to_string();
    c.proxy_port = 1080;
    let opts = options_for(&c, SshAuth::Agent, HostKeyPolicy::Strict).expect("代理应当能构造");
    assert_eq!(opts.outbound_proxy.unwrap().kind, ProxyKind::Socks5);
}

#[test]
fn a_jump_session_does_not_carry_an_outbound_proxy() {
    let mut c = ssh_session(AuthMethod::Agent);
    c.proxy_jump = "bastion".to_string();
    let opts = options_for(&c, SshAuth::Agent, HostKeyPolicy::Strict).expect("老配置应当能读");
    assert!(opts.outbound_proxy.is_none());
    assert_eq!(opts.proxy_jump.len(), 1);
}

#[test]
fn an_incomplete_outbound_proxy_is_an_error_not_a_silent_direct_connection() {
    let mut c = ssh_session(AuthMethod::Agent);
    c.proxy_type = ProxyType::Http.tag().to_string();
    c.proxy_port = 3128; // 故意不填主机
    let err = options_for(&c, SshAuth::Agent, HostKeyPolicy::Strict)
        .expect_err("没填代理主机必须报错，不能退化成直连");
    assert!(matches!(err, SshError::Config(_)), "{err:?}");
    assert!(err.to_string().contains("代理主机"), "{err}");
}

// ── OneKey：界面路径的回退链 ─────────────────────────────────────────

#[test]
fn a_password_primary_plus_a_key_fallback_builds_a_two_step_chain() {
    // 密码被拒后自动改试公钥——这正是界面 OneKey 的核心场景。
    let dir = temp_dir("chain-ui");
    std::fs::write(dir.join("id_ed25519"), b"x").expect("写 key");
    let mut c = ssh_session(AuthMethod::Password);
    c.identity_file = dir.join("id_ed25519").to_string_lossy().to_string();
    c.auth_chain = vec![AuthMethod::PublicKey];
    let primary = SshAuth::Password {
        password: "pw".to_string(),
    };
    let chain = ui_fallback_chain(&c, &primary, Some(&dir));
    assert_eq!(chain.len(), 2, "首选 + 公钥回退：{chain:?}");
    assert_eq!(chain[0], primary);
    match &chain[1] {
        SshAuth::PrivateKey { path, .. } => assert!(path.ends_with("id_ed25519"), "{path:?}"),
        other => panic!("第二环应是公钥：{other:?}"),
    }
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn no_fallback_chain_keeps_the_single_primary() {
    // 旧会话（空 auth_chain）不许多出任何一环。
    let c = ssh_session(AuthMethod::Password);
    let primary = SshAuth::Password {
        password: "pw".to_string(),
    };
    let chain = ui_fallback_chain(&c, &primary, None);
    assert_eq!(chain, vec![primary], "空 auth_chain → 只有首选");
}

#[test]
fn options_for_wires_the_fallback_chain_into_ssh_options() {
    // 只看 `ui_fallback_chain` 还不够：它必须真的落到 `SshOptions.auth_chain` 上，
    // 否则连接层拿到的仍是单方法。
    let dir = temp_dir("chain-opts");
    std::fs::write(dir.join("id_ed25519"), b"x").expect("写 key");
    let mut c = ssh_session(AuthMethod::Password);
    c.identity_file = dir.join("id_ed25519").to_string_lossy().to_string();
    c.auth_chain = vec![AuthMethod::PublicKey];
    let opts = options_for(
        &c,
        SshAuth::Password {
            password: "pw".to_string(),
        },
        HostKeyPolicy::Strict,
    )
    .expect("能构造");
    assert_eq!(opts.auth_chain.len(), 2, "options_for 应接上回退链");
    let _ = std::fs::remove_dir_all(&dir);
}

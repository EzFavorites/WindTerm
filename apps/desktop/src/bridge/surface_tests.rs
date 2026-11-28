//! `DeskSurface` 的能力面翻译层回归：方法形状、错误分档、ADR-0023 的漂移防线。
//!
//! 单独成文件（由 `surface.rs` 的 `#[cfg(test)] #[path = "surface_tests.rs"]` 引入），
//! 与 `lock_tests.rs` 同一条理由：夹具与断言占的篇幅不该顶爆单文件上限，
//! 而且 `#[path]` 引入的纯测试模块不会污染门禁对生产代码的扫描。

use super::*;
// 之后新 shell 走 `new_local_shell`，bin 目标不再需要 `Protocol`；
// 只有这里的断言还按协议造夹具，所以导入落在测试模块里。
use term_core::session::Protocol;
use term_bridge::error::ErrorKind;
use term_script::{HostEnv, ScriptWorkspace};
use term_session::OpenError;

use crate::script_host::PooledSessions;
use crate::session_open::OpenFailure;

use std::time::Duration;

fn surface() -> DeskSurface {
    DeskSurface::new(Arc::new(SessionPool::new()))
}

fn kind(err: &SurfaceError) -> ErrorKind {
    err.kind
}

#[test]
fn describe_lists_the_same_methods_as_the_protocol_table() {
    let value = surface().call("describe", &json!({})).expect("describe");
    let methods: Vec<&str> = value["methods"]
        .as_array()
        .expect("methods 数组")
        .iter()
        .map(|m| m["name"].as_str().expect("name 是字符串"))
        .collect();
    for name in ["session_open", "write", "keys", "wait", "wait_idle", "screen", "state", "facets"] {
        assert!(methods.contains(&name), "describe 里缺 {name}");
    }
}

#[test]
fn an_unknown_method_is_method_not_found_not_an_empty_success() {
    let err = surface().call("session_teleport", &json!({})).unwrap_err();
    assert_eq!(kind(&err), ErrorKind::MethodNotFound);
    assert_eq!(err.code, -32601);
}

#[test]
fn an_empty_pool_lists_no_sessions() {
    let value = surface().call("session_list", &json!({})).expect("session_list");
    assert_eq!(value, json!({ "sessions": [] }));
}

#[test]
fn a_missing_session_is_not_found_and_names_it() {
    for method in ["state", "screen", "facets", "session_close"] {
        let err = surface().call(method, &json!({ "session": "ghost" })).unwrap_err();
        assert_eq!(kind(&err), ErrorKind::NotFound, "{method} 应报 not_found");
        assert_eq!(err.code, -32002);
        assert!(
            err.message.contains("ghost"),
            "{method} 的文案要点名是哪个名字：{}",
            err.message
        );
    }
}

#[test]
fn missing_or_wrong_params_are_invalid_params() {
    let surface = surface();
    // 缺 session
    assert_eq!(
        kind(&surface.call("write", &json!({ "text": "ls" })).unwrap_err()),
        ErrorKind::InvalidParams
    );
    // 有 session 没 text：**参数形状先于会话存在性**——即便这条会话不存在，
    // 也该先报「缺 text」，客户端改起来才直接。
    assert_eq!(
        kind(&surface.call("write", &json!({ "session": "ghost" })).unwrap_err()),
        ErrorKind::InvalidParams
    );
    // 空名字与数字 id 都不是合法寻址（ADR-0024 决策 4：不留 id 逃生门）
    for bad in [json!(""), json!(0)] {
        assert_eq!(
            kind(
                &surface
                    .call("write", &json!({ "session": bad, "text": "ls" }))
                    .unwrap_err()
            ),
            ErrorKind::InvalidParams,
            "{bad} 不该被当成会话名"
        );
    }
    // 空 pattern：立刻命中任何输出，必须拒
    assert_eq!(
        kind(
            &surface
                .call("wait", &json!({ "session": "ghost", "pattern": "" }))
                .unwrap_err()
        ),
        ErrorKind::InvalidParams
    );
}

// ── 内置 SSH 注入点（控制面开内置 SSH 会话） ──────────────────
//
// 能力面这八条方法不许出现协议专有参数（ADR-0019），所以 `session_open` 的线上形状
// 里没有「用内置 SSH 连哪台机器」。协议分流发生在
// `session_open::open_session`；这里测的是**错误映射**和「本地 shell 仍然通」。

fn closed_port() -> u16 {
    let listener = std::net::TcpListener::bind(("127.0.0.1", 0)).expect("bind");
    listener.local_addr().expect("addr").port()
}

fn builtin_ssh(auth: term_core::session::AuthMethod, port: u16) -> SessionConfig {
    let mut config = SessionConfig::new(Protocol::Ssh); // builtin_ssh = true
    config.name = "prod".to_string();
    config.host = "127.0.0.1".to_string();
    config.port = port;
    config.username = "qa".to_string();
    config.auth = auth;
    config.connection_timeout = 5;
    config
}

#[test]
fn a_session_that_needs_the_vault_is_locked_not_internal() {
    // 桥接是无窗口的、从不解锁保险库：这一档的语义是「策略拒绝，条件变了
    // 再试」，与锁屏门同一个码，客户端只需处理一种情形。
    let surface = surface();
    let config = builtin_ssh(term_core::session::AuthMethod::Password, 22);
    // 经 `open_failure` 走一遍：这里要钉住的是**线上**那三元组，不是内部枚举。
    let err = open_failure(surface.open_config(&config, 80, 24).unwrap_err());
    assert_eq!(kind(&err), ErrorKind::Locked);
    assert_eq!(err.code, -32003);
    assert!(
        err.message.contains("解锁保险库"),
        "提示要说清为什么做不了：{}",
        err.message
    );
    assert!(
        err.message.contains("prod"),
        "要点名是哪条会话：{}",
        err.message
    );
    assert!(surface.pool.list().is_empty(), "拒绝后不许留半个会话");
}

#[test]
fn a_builtin_ssh_that_cannot_connect_is_internal_not_invalid_params() {
    let surface = surface();
    let config = builtin_ssh(term_core::session::AuthMethod::Agent, closed_port());
    let err = open_failure(surface.open_config(&config, 80, 24).unwrap_err());
    assert_eq!(kind(&err), ErrorKind::Internal);
    assert_eq!(err.code, -32603);
    assert!(surface.pool.list().is_empty());
}

#[test]
fn an_auth_method_the_builtin_client_cannot_do_is_invalid_request() {
    // GSSAPI：改会话配置就能修，不是 internal。
    let surface = surface();
    let mut config = builtin_ssh(term_core::session::AuthMethod::Password, 22);
    config.auth = term_core::session::AuthMethod::GssapiWithMic;
    let err = open_failure(surface.open_config(&config, 80, 24).unwrap_err());
    assert_eq!(kind(&err), ErrorKind::InvalidRequest);
}

#[test]
fn a_local_shell_still_opens_through_the_shared_entry_point() {
    let surface = surface();
    let mut config = SessionConfig::new(Protocol::Shell);
    config.name = "qa-shell".to_string();
    let id = surface.open_config(&config, 80, 24).expect("本地 shell 应当能开");
    assert!(
        surface.pool.list().iter().any(|(i, name, _)| *i == id && name == "qa-shell"),
        "会话应当在册"
    );
    surface.pool.close(id);
}

// ── ADR-0023：RPC 侧漂移防线 ──────────────────────────────

#[test]
fn every_remote_verb_is_dispatched_and_script_only_verbs_are_not() {
    // 这是 RPC 侧的对账：表上说「远程看得见」的每一个动词，dispatch 都得
    // 接住（用空参数调用时不该是 method_not_found——缺参数是 invalid_params，
    // 那是另一回事）；表上说「只给脚本」的，必须恰好报 method_not_found。
    let surface = surface();
    for name in term_core::surface::names() {
        // `describe` / `session_list` 用空参数是能成功的；其余会报错。
        // 两种都行，唯一不许的是「这个方法不存在」。
        if let Err(err) = surface.call(name, &json!({})) {
            assert_ne!(
                kind(&err),
                ErrorKind::MethodNotFound,
                "{name} 在能力面上对远程可见，但 DeskSurface 没有分支接住它"
            );
        }
    }
    for spec in term_core::surface::METHODS {
        if spec.exposure == term_core::surface::Exposure::Script {
            let err = surface.call(spec.name, &json!({})).unwrap_err();
            assert_eq!(
                kind(&err),
                ErrorKind::MethodNotFound,
                "{} 是脚本专用，远程不该能调",
                spec.name
            );
        }
    }
}

#[test]
fn wait_any_rejects_a_missing_or_malformed_patterns_array() {
    let surface = surface();
    for bad in [json!({}), json!({ "session": "ghost" }), json!({ "session": "ghost", "patterns": [] }), json!({ "session": "ghost", "patterns": ["OK", 7] })] {
        let err = surface.call("wait_any", &bad).unwrap_err();
        assert_eq!(kind(&err), ErrorKind::InvalidParams, "{bad} 应报 invalid_params");
    }
    let err = surface
        .call("wait_any", &json!({ "session": "ghost", "patterns": ["OK", 7] }))
        .unwrap_err();
    assert!(err.message.contains("第 2 项"), "要点名是第几项：{}", err.message);
}

#[test]
fn read_until_rejects_an_empty_delimiter() {
    let surface = surface();
    let err = surface
        .call("read_until", &json!({ "session": "ghost", "delimiter": "" }))
        .unwrap_err();
    assert_eq!(kind(&err), ErrorKind::InvalidParams);
}

#[test]
fn script_then_api_read_until_continue_from_one_shared_cursor() {
    // ADR-0030 回归：宿主只造一份 ScriptSessions，同时交给 ScriptWorkspace
    // 与 DeskSurface。脚本读走第一段后，API 必须从同一个读点续读第二段；
    // 以前两侧各持 HashMap，会各自从头读、把同一段输出重复交出去。
    let pool = Arc::new(SessionPool::new());
    let id = pool.open_exec("cat", 80, 24).expect("开一条 cat 会话");
    let name = pool.name(id).expect("会话名");
    let sessions = ScriptSessions::new();
    let host: Arc<dyn term_script::Sessions> = Arc::new(PooledSessions::new(Arc::clone(&pool)));
    let session = host.attach(id).expect("挂在池子里的会话");
    let workspace = ScriptWorkspace::with_sessions(
        HostEnv::new(host),
        session,
        Duration::from_secs(5),
        sessions.clone(),
    );
    let surface = DeskSurface::with_sessions(Arc::clone(&pool), sessions.clone());

    // 两侧共用同一条会话、只建一份 ScriptSession。
    assert_eq!(sessions.len(), 1);
    workspace.write("one#A#two#B#\n").expect("写入 cat 回显");
    let first = workspace.read_until("A#", 5.0, None).expect("脚本读第一段");
    assert!(first.contains("one#A#"), "脚本第一段：{first:?}");

    let second = surface
        .call(
            "read_until",
            &json!({ "session": name, "delimiter": "B#", "timeout_ms": 5000 }),
        )
        .expect("API 从同一游标续读");
    let text = second["text"].as_str().unwrap_or_default();
    assert!(text.contains("two#B#"), "API 续读到第二段：{text:?}");
    assert!(!text.contains("one#A#"), "API 不得重读脚本已消费的段：{text:?}");
    assert_eq!(sessions.len(), 1, "两侧没有各建一份游标");
    assert!(pool.close(id));
}

#[test]
fn streaming_and_log_verbs_report_not_found_for_an_unknown_session() {
    let surface = surface();
    for (method, params) in [
        ("read", json!({ "session": "ghost" })),
        ("read_until", json!({ "session": "ghost", "delimiter": "x" })),
        ("wait_any", json!({ "session": "ghost", "patterns": ["x"] })),
        ("log_start", json!({ "session": "ghost", "dir": "/tmp" })),
        ("log_stop", json!({ "session": "ghost" })),
        ("log_status", json!({ "session": "ghost" })),
    ] {
        let err = surface.call(method, &params).unwrap_err();
        assert_eq!(kind(&err), ErrorKind::NotFound, "{method} 对未知名字应报 not_found");
    }
}

#[test]
fn keys_rejects_a_missing_empty_or_malformed_array() {
    let surface = surface();
    for bad in [
        json!({}),
        json!({ "session": "ghost" }),
        json!({ "session": "ghost", "keys": [] }),
        json!({ "session": "ghost", "keys": ["enter", 7] }),
    ] {
        let err = surface.call("keys", &bad).unwrap_err();
        assert_eq!(kind(&err), ErrorKind::InvalidParams, "{bad} 应报 invalid_params");
    }
    let err = surface
        .call("keys", &json!({ "session": "ghost", "keys": ["enter", 7] }))
        .unwrap_err();
    assert!(err.message.contains("第 2 项"), "要点名是第几项：{}", err.message);
}

#[test]
fn an_unknown_key_name_is_invalid_params_and_names_the_key() {
    // 认不出来的键名不能被当成文本写出去：报错要点名是哪个键，
    // 并给出可用清单（一次按好几个键时，只说「有个键不认识」得逐个试）。
    let surface = surface();
    let err = surface
        .call("keys", &json!({ "session": "ghost", "keys": ["ctrl+enter"] }))
        .unwrap_err();
    assert_eq!(kind(&err), ErrorKind::InvalidParams);
    assert!(
        err.message.contains("ctrl+enter"),
        "要点名认不出的键：{}",
        err.message
    );
    assert!(err.message.contains("enter"), "要给出可用清单：{}", err.message);
}

#[test]
fn write_and_keys_report_not_found_for_an_unknown_session() {
    let surface = surface();
    for (method, params) in [
        ("write", json!({ "session": "ghost", "text": "x" })),
        ("keys", json!({ "session": "ghost", "keys": ["enter"] })),
        ("wait_idle", json!({ "session": "ghost", "quiet_ms": 50 })),
    ] {
        let err = surface.call(method, &params).unwrap_err();
        assert_eq!(kind(&err), ErrorKind::NotFound, "{method} 对未知名字应报 not_found");
    }
}

#[test]
fn the_config_error_mapping_is_unchanged() {
    // 这一层出现之前 `OpenError::Config` 就是 invalid_params；映射改道之后
    // 逐字保持一致（客户端的分支不变）。
    let err = open_failure(OpenFailure::Session(OpenError::Config(
        term_core::session::ConfigError::MissingHost(Protocol::Ssh),
    )));
    assert_eq!(err.kind, ErrorKind::InvalidParams);
}

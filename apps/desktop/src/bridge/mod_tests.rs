//! `bridge/mod.rs` 的测试：loopback 绑定 / key 文件权限 / HMAC 挑战 /
//! 行限 / 锁屏门 / 真 TCP 端到端。
//!
//! 单独成文件（与 `surface_tests.rs` / `lock_tests.rs` 同一条理由）：夹具与
//! 断言占的篇幅不该顶爆单文件上限，而 `#[path]` 引入的纯测试模块
//! 不会污染门禁对生产代码的扫描。

use super::*;
use std::io::{BufReader, Cursor, Read, Write};
use std::net::{IpAddr, TcpListener, TcpStream};
use std::time::Duration;

use serde_json::{json, Value};
use term_bridge::error::ErrorKind;

use super::control::{
    constant_time_eq, hmac_hex, ip_is_loopback, key_path, parse_port, proof_message, random_hex,
    read_capped_line, read_frame, read_key, write_frame, write_key_file, LineRead, CHALLENGE_FIELD,
    DEFAULT_CONTROL_PORT, FIRST_REQUEST_TIMEOUT, KEY_FIELD, KEY_FILE_PREFIX, LINE_LIMIT,
    PROOF_FIELD,
};

struct NeverLocked;
impl lock::LockProbe for NeverLocked {
    fn locked(&self) -> Option<bool> {
        Some(false)
    }
}

struct AlwaysLocked;
impl lock::LockProbe for AlwaysLocked {
    fn locked(&self) -> Option<bool> {
        Some(true)
    }
}

static SCRATCH_SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

fn scratch_dir() -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "zterm-control-{}-{}",
        std::process::id(),
        SCRATCH_SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    ));
    std::fs::create_dir_all(&dir).expect("临时目录");
    dir
}

// ---- 端口与 key 文件 ----

#[test]
fn the_control_port_is_fixed_with_an_env_override() {
    assert_eq!(DEFAULT_CONTROL_PORT, 47_653);
    assert_eq!(parse_port(" 50001 "), Some(50001));
    assert_eq!(parse_port("0"), None, "端口 0 不能当固定端点");
    assert_eq!(parse_port("不是数字"), None);
    let path = key_path(DEFAULT_CONTROL_PORT).expect("能定位本机用户");
    assert!(
        path.file_name().and_then(|s| s.to_str()).unwrap().starts_with(KEY_FILE_PREFIX),
        "key 文件名必须带端口：{path:?}"
    );
    assert!(endpoint_description().contains("47653"));
    assert!(endpoint_description().contains("WINDTERM_CONTROL_PORT"));
}

#[test]
#[cfg(unix)]
fn the_key_file_is_owner_only_and_is_deleted_on_drop() {
    use std::os::unix::fs::PermissionsExt;

    let path = scratch_dir().join(format!("{KEY_FILE_PREFIX}47653"));
    let key = random_hex().expect("随机 key");
    assert_eq!(key.len(), 64, "256-bit = 32 字节 = 64 位 hex");
    assert!(key.bytes().all(|b| b.is_ascii_hexdigit()));
    write_key_file(&path, &key).expect("写 key");
    let mode = std::fs::metadata(&path).expect("stat").permissions().mode() & 0o777;
    assert_eq!(mode, 0o600, "key 文件只有主人可读写");
    assert_eq!(read_key(&path).expect("读回 key"), key);

    let bound = Bound::from_parts(
        TcpListener::bind(("127.0.0.1", 0)).expect("临时 listener"),
        key.clone(),
        path.clone(),
    );
    drop(bound);
    assert!(!path.exists(), "退出时尽力删掉 key 文件");
}

#[test]
fn a_malformed_key_file_is_refused() {
    constant_time_compare_is_length_aware();
    let path = scratch_dir().join(format!("{KEY_FILE_PREFIX}1"));
    std::fs::write(&path, "半截 key").expect("写坏 key");
    assert!(read_key(&path).is_err(), "格式不对的 key 不能拿去认证");
}

fn constant_time_compare_is_length_aware() {
    assert!(constant_time_eq(b"abcd", b"abcd"));
    assert!(!constant_time_eq(b"abcd", b"abce"));
    assert!(!constant_time_eq(b"abcd", b"abc"));
}

// ---- loopback 对端校验 ----

#[test]
fn only_loopback_peers_are_admitted() {
    assert!(ip_is_loopback("127.0.0.1".parse::<IpAddr>().unwrap()));
    assert!(ip_is_loopback("127.42.1.9".parse::<IpAddr>().unwrap()), "127/8 整段都算");
    assert!(ip_is_loopback("::1".parse::<IpAddr>().unwrap()));
    assert!(!ip_is_loopback("10.0.0.1".parse::<IpAddr>().unwrap()));
    assert!(!ip_is_loopback("192.168.1.1".parse::<IpAddr>().unwrap()));
    assert!(!ip_is_loopback("::ffff:10.0.0.1".parse::<IpAddr>().unwrap()));
}

// ---- 行限与超时口径 ----

#[test]
fn an_oversized_line_is_detected_before_it_grows_without_bound() {
    a_normal_line_still_reads_to_the_newline();
    let bytes = vec![b'x'; LINE_LIMIT + 64];
    let mut reader = Cursor::new(bytes);
    let mut line = Vec::new();
    assert_eq!(read_capped_line(&mut reader, &mut line).expect("读"), LineRead::TooLong);
    assert!(line.len() <= LINE_LIMIT, "不能无界累积");
    assert_eq!(FIRST_REQUEST_TIMEOUT, Duration::from_secs(30));
}

fn a_normal_line_still_reads_to_the_newline() {
    let mut reader = Cursor::new(&b"{\"id\":1}\n"[..]);
    let mut line = Vec::new();
    assert_eq!(read_capped_line(&mut reader, &mut line).expect("读"), LineRead::Line);
    assert_eq!(line, b"{\"id\":1}\n");
    line.clear();
    assert_eq!(read_capped_line(&mut reader, &mut line).expect("读"), LineRead::Eof);
}

// ---- 锁屏门（能力面） ----

#[test]
fn a_locked_screen_turns_every_call_into_locked() {
    a_locked_screen_also_refuses_to_open_new_sessions();
    let gate = LockedGate {
        inner: surface::DeskSurface::new(Arc::new(SessionPool::new())),
        probe: Arc::new(AlwaysLocked),
    };
    let err = gate.call("session_list", &json!({})).unwrap_err();
    assert_eq!(err.kind, ErrorKind::Locked);
    assert_eq!(err.code, -32003);
}

fn a_locked_screen_also_refuses_to_open_new_sessions() {
    let pool = Arc::new(SessionPool::new());
    let gate = LockedGate {
        inner: surface::DeskSurface::new(Arc::clone(&pool)),
        probe: Arc::new(AlwaysLocked),
    };
    for params in [json!({ "command": "echo hi" }), json!({ "name": "prod" })] {
        let err = gate.call("session_open", &params).unwrap_err();
        assert_eq!(err.kind, ErrorKind::Locked, "session_open({params}) 应被拒");
        assert_eq!(err.code, -32003);
    }
    assert!(pool.list().is_empty(), "锁屏态下不许留下任何会话");
}

// ---- 真 TCP 端到端 ----

/// 起一个真 loopback 服务，返回（端口、key）。服务端与测试共享同一个池。
fn spawn_server(probe: Arc<dyn lock::LockProbe>) -> (u16, String) {
    let listener = TcpListener::bind(("127.0.0.1", 0)).expect("绑定临时端口");
    let port = listener.local_addr().expect("地址").port();
    let key = random_hex().expect("随机 key");
    let key_path = scratch_dir().join(format!("{KEY_FILE_PREFIX}{port}"));
    write_key_file(&key_path, &key).expect("写 key");
    let bound = Bound::from_parts(listener, key.clone(), key_path);
    let pool = Arc::new(SessionPool::new());
    std::thread::spawn(move || {
        let _ = serve_on_with_pool(bound, probe, pool, term_script::ScriptSessions::new());
    });
    (port, key)
}

/// 完整跑一遍传输外壳：challenge → proof → key。返回已准入的连接。
fn admitted(port: u16, key: &str) -> TcpStream {
    let mut stream = TcpStream::connect(("127.0.0.1", port)).expect("连接");
    stream.set_read_timeout(Some(Duration::from_secs(5))).expect("超时");
    let mut reader = BufReader::new(stream.try_clone().expect("clone"));
    let nonce = random_hex().expect("nonce");
    write_frame(&mut stream, &json!({CHALLENGE_FIELD: {"nonce": nonce}})).expect("发挑战");
    let proof = read_frame(&mut reader).expect("读证明").expect("有证明");
    assert_eq!(proof[PROOF_FIELD]["alg"], json!("hmac-sha256"));
    let expected = hmac_hex(key.as_bytes(), &proof_message(&nonce)).expect("HMAC");
    assert_eq!(proof[PROOF_FIELD]["signature"], json!(expected), "服务端必须证明持有 key");
    write_frame(&mut stream, &json!({KEY_FIELD: {"key": key}})).expect("发 key");
    stream
}

fn request(stream: &mut TcpStream, value: &Value) -> Value {
    let line = serde_json::to_string(value).expect("请求序列化");
    stream.write_all(format!("{line}\n").as_bytes()).expect("写请求");
    stream.flush().expect("冲刷");
    // 逐字节读到换行：不在测试夹具里再用一层 BufReader。
    let mut out = Vec::new();
    let mut byte = [0u8; 1];
    while stream.read(&mut byte).expect("读回包") == 1 {
        out.push(byte[0]);
        if byte[0] == b'\n' {
            break;
        }
    }
    assert!(!out.is_empty(), "服务端没有回包（请求：{value}）");
    let text = String::from_utf8(out).expect("回包是 UTF-8");
    serde_json::from_str::<Value>(&text).unwrap_or_else(|err| panic!("回包不是 JSON：{err} / {text}"))
}

#[test]
fn the_first_request_is_an_ordinary_method_not_an_auth_handshake() {
    a_real_connection_drives_a_session_on_the_gui_pool();
    let (port, key) = spawn_server(Arc::new(NeverLocked));
    let mut stream = admitted(port, &key);
    let reply = request(&mut stream, &json!({"jsonrpc":"2.0","id":1,"method":"session_list"}));
    assert_eq!(reply["id"], json!(1));
    assert_eq!(reply["result"], json!({"sessions": []}), "首条就是普通方法：{reply}");
    assert!(reply.get("error").is_none(), "不该有 auth 这一层");

    let described = request(&mut stream, &json!({"jsonrpc":"2.0","id":2,"method":"describe"}));
    let methods = described["result"]["methods"].as_array().expect("方法清单");
    assert!(methods.iter().all(|m| m["name"].as_str() != Some("auth")),
        "auth 是传输外壳，不是能力面方法");
}

fn a_wrong_client_key_is_rejected_before_any_rpc() {
    let (port, key) = spawn_server(Arc::new(NeverLocked));
    let mut stream = TcpStream::connect(("127.0.0.1", port)).expect("连接");
    stream.set_read_timeout(Some(Duration::from_secs(5))).expect("超时");
    let mut reader = BufReader::new(stream.try_clone().expect("clone"));
    let nonce = random_hex().expect("nonce");
    write_frame(&mut stream, &json!({CHALLENGE_FIELD: {"nonce": nonce}})).expect("发挑战");
    let proof = read_frame(&mut reader).expect("读证明").expect("有证明");
    let expected = hmac_hex(key.as_bytes(), &proof_message(&nonce)).expect("HMAC");
    assert_eq!(proof[PROOF_FIELD]["signature"], json!(expected));
    // 服务端证明是真的，但客户端拿错 key → 服务端静默关，不进能力面。
    let wrong = "f".repeat(64);
    write_frame(&mut stream, &json!({KEY_FIELD: {"key": wrong}})).expect("发错 key");
    let mut buf = Vec::new();
    let _ = stream.read_to_end(&mut buf);
    assert!(buf.is_empty(), "准入失败必须静默关：{buf:?}");
}

#[test]
fn a_locked_screen_closes_the_connection_without_any_reply() {
    a_wrong_client_key_is_rejected_before_any_rpc();
    let (port, _key) = spawn_server(Arc::new(AlwaysLocked));
    let mut stream = TcpStream::connect(("127.0.0.1", port)).expect("连接");
    stream.set_read_timeout(Some(Duration::from_secs(5))).expect("超时");
    // 锁屏的服务端可能在挑战帧到达前就关了：写失败也是「静默关」的证据。
    let _ = write_frame(&mut stream, &json!({CHALLENGE_FIELD: {"nonce": random_hex().unwrap()}}));
    let mut buf = Vec::new();
    let _ = stream.read_to_end(&mut buf);
    assert!(buf.is_empty(), "锁屏时应直接关连接，一个字节都不回：{buf:?}");
}

#[test]
fn an_oversized_request_gets_a_parse_error_and_closes_the_connection() {
    let (port, key) = spawn_server(Arc::new(NeverLocked));
    let mut fresh = admitted(port, &key);
    let bytes = vec![b'x'; LINE_LIMIT + 16];
    fresh.write_all(&bytes).expect("写超长行");
    fresh.write_all(b"\n").expect("换行");
    fresh.flush().expect("冲刷");
    let mut out = Vec::new();
    let mut byte = [0u8; 1];
    while fresh.read(&mut byte).expect("读回包") == 1 {
        out.push(byte[0]);
        if byte[0] == b'\n' {
            break;
        }
    }
    let text = String::from_utf8(out).expect("UTF-8");
    let bad: Value = serde_json::from_str(text.trim()).expect("回包是 JSON");
    assert_eq!(bad["error"]["kind"], "parse_error", "超限回解析错误：{text}");
    assert!(bad["id"].is_null());
    let mut rest = Vec::new();
    let _ = fresh.read_to_end(&mut rest);
    assert!(rest.is_empty(), "回完解析错误必须关连接：{rest:?}");
}

fn a_real_connection_drives_a_session_on_the_gui_pool() {
    let (port, key) = spawn_server(Arc::new(NeverLocked));
    let mut stream = admitted(port, &key);
    let opened = request(
        &mut stream,
        &json!({"jsonrpc":"2.0","id":2,"method":"session_open",
                "params":{"command":"sh -c 'printf ready'"}}),
    );
    let session = opened["result"]["name"].as_str().expect("会话名");
    let state = request(
        &mut stream,
        &json!({"jsonrpc":"2.0","id":3,"method":"state","params":{"session":session}}),
    );
    assert!(state["result"]["alive"].is_boolean(), "真会话要有状态：{state}");
    let closed = request(
        &mut stream,
        &json!({"jsonrpc":"2.0","id":4,"method":"session_close","params":{"session":session}}),
    );
    assert_eq!(closed["result"]["closed"], json!(true));
}

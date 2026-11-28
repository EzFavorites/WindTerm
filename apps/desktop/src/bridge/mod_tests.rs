//! `bridge/mod.rs` 的测试：socket 权限 / peercred / 锁屏门 / 真 UDS 端到端。
//!
//! 单独成文件（与 `surface_tests.rs` / `lock_tests.rs` 同一条理由）：夹具与
//! 断言占的篇幅不该顶爆单文件上限，而 `#[path]` 引入的纯测试模块
//! 不会污染门禁对生产代码的扫描。

use super::*;
use std::io::{Read, Write};
use std::time::Duration;

use serde_json::{json, Value};
use term_bridge::error::ErrorKind;

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

/// 进程内序号：并行用例不能用同一时间戳当临时路径。
static SCRATCH_SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

#[cfg(unix)]
fn scratch_socket() -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "zterm-control-{}-{}",
        std::process::id(),
        SCRATCH_SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    ));
    std::fs::create_dir_all(&dir).expect("临时目录");
    dir.join(SOCKET_FILE)
}

#[test]
fn the_control_socket_path_is_fixed_under_home() {
    // 客户端没有 --socket 参数：路径是双方唯一的会合点。
    assert!(default_socket_path().expect("有家目录").ends_with(".zterm/control.sock"));
}

#[cfg(unix)]
#[test]
fn binding_creates_the_socket_with_owner_only_permissions() {
    let path = scratch_socket();
    let bound = bind(&Config::new(path.clone())).expect("绑定");
    let mode = std::fs::metadata(&path).expect("stat socket").permissions().mode() & 0o777;
    assert_eq!(mode, 0o600, "控制端点必须只有主人可连");
    drop(bound);
    assert!(!path.exists(), "收场时要清掉自己创建的 socket");
}

#[cfg(unix)]
#[test]
fn a_stale_socket_is_replaced_but_a_live_one_is_refused() {
    let path = scratch_socket();
    let first = bind(&Config::new(path.clone())).expect("首次绑定");
    assert!(matches!(
        bind(&Config::new(path.clone())),
        Err(ServeError::AlreadyRunning(_))
    ), "前台还活着时不许抢它的端点");
    drop(first);
    // 进程退出留下的 socket 壳可以清理并重新绑定。
    let _rebound = bind(&Config::new(path)).expect("陈旧 socket 应被替换");
}

#[cfg(unix)]
#[test]
fn peercred_admits_only_the_same_user() {
    let (_client, server) = UnixStream::pair().expect("socket pair");
    // pair 的对端就是本进程：它的 uid 即「自己」。
    assert!(admit(&server, &NeverLocked).expect("peercred"));
    let mine = self_uid().expect("自己的 uid");
    assert!(!same_user(mine.wrapping_add(1)).unwrap_or(false), "别的 uid 必须被拒");
    assert!(!admit(&server, &AlwaysLocked).expect("锁屏探测"), "锁屏时不许放行");
}

// ---- 锁屏门 ----

#[test]
fn a_locked_screen_turns_every_call_into_locked() {
    let gate = LockedGate {
        inner: surface::DeskSurface::new(Arc::new(SessionPool::new())),
        probe: Arc::new(AlwaysLocked),
    };
    let err = gate.call("session_list", &json!({})).unwrap_err();
    assert_eq!(err.kind, ErrorKind::Locked);
    assert_eq!(err.code, -32003);
}

#[test]
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

#[cfg(unix)]
fn request(stream: &mut UnixStream, value: &Value) -> Value {
    stream.write_all(format!("{value}\n").as_bytes()).expect("写请求");
    stream.flush().expect("冲刷");
    // 逐字节读到换行：不在测试夹具里再用一层 BufReader（它会预读多行，
    // 把「一个请求一行回包」的协议事实藏起来）。
    let mut line = Vec::new();
    let mut byte = [0u8; 1];
    while stream.read(&mut byte).expect("读回包") == 1 {
        line.push(byte[0]);
        if byte[0] == b'\n' {
            break;
        }
    }
    assert!(!line.is_empty(), "服务端没有回包（请求：{value}）");
    let text = String::from_utf8(line).expect("回包是 UTF-8");
    serde_json::from_str::<Value>(&text)
        .unwrap_or_else(|err| panic!("回包不是 JSON：{err} / {text}"))
}

/// 起一个真 UDS 服务，返回客户端流。服务端与测试共享同一个池。
#[cfg(unix)]
fn spawn_server(probe: Arc<dyn lock::LockProbe>) -> UnixStream {
    let path = scratch_socket();
    let bound = bind(&Config::new(path.clone())).expect("绑定 socket");
    let pool = Arc::new(SessionPool::new());
    std::thread::spawn(move || {
        let _ = serve_on_with_pool(bound, probe, pool, term_script::ScriptSessions::new());
    });
    let stream = UnixStream::connect(&path).expect("连接 socket");
    stream
        .set_read_timeout(Some(Duration::from_secs(10)))
        .expect("超时");
    stream
}

#[cfg(unix)]
#[test]
fn the_first_request_is_an_ordinary_method_not_an_auth_handshake() {
    let mut stream = spawn_server(Arc::new(NeverLocked));
    let reply = request(
        &mut stream,
        &json!({"jsonrpc":"2.0","id":1,"method":"session_list"}),
    );
    assert_eq!(reply["id"], json!(1));
    assert_eq!(reply["result"], json!({"sessions": []}), "第一条就能直接用：{reply}");
    assert!(reply.get("error").is_none(), "不该有 auth 这一层");
}

#[cfg(unix)]
#[test]
fn a_real_connection_drives_a_session_on_the_gui_pool() {
    let mut stream = spawn_server(Arc::new(NeverLocked));
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

#[cfg(unix)]
#[test]
fn a_locked_screen_closes_the_connection_without_any_reply() {
    let mut stream = spawn_server(Arc::new(AlwaysLocked));
    let mut buf = Vec::new();
    let _ = stream.read_to_end(&mut buf);
    assert!(buf.is_empty(), "锁屏时应直接关连接，一个字节都不回：{buf:?}");
}

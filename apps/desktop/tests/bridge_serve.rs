//! `term-desk serve` 的**二进制级**验收：真进程、真监听、真 JSON-RPC。
//!
//! 为什么要有这一层，而 `apps/desktop/src/bridge/mod.rs` 里已经有真 TCP 用例：
//! 那些用例走的是 `bind()` + `serve_on()`，**绕过了 CLI 这一侧的全部接线**
//! ——子命令分发、`--port 0` 的洪泛绑定、监听信息那一行（脚本靠它拿端口）、
//! token 文件的读写、以及「`serve` 一直阻塞不退出」这件事本身。于是
//! 「远程 API 能用」这句话在此之前从来没有在**产品二进制**上被证明过。
//!
//! 这一层只做一件事：把 `term-desk` 拉起来跑。所以断言刻意选得少而硬——
//! 每个动词的细节语义由 `bridge/mod.rs` 与 `bridge/surface_tests.rs` 负责，
//! 重复一遍只会让两处慢慢漂。
//!
//! 端口用 `--port 0` 让内核分配：固定端口在并行测试里会互相抢。
//!
//! # 锁屏：本用例唯一被钉死的外部状态
//!
//! `desk serve` 生产路径用 `ioreg` 做纵深防御：锁屏时连接被**静默**关掉，连
//! `locked` 错误都不回（`bridge/mod.rs::handle` 里 `probe.locked() == Some(true)`
//! 那一支）。这是正确的产品行为，但本用例断言的是「未握手请求会收到 -32003」——
//! 它真正假定的其实是「这台机器现在没锁屏」。2026-11-14 实测：屏幕锁着时
//! `read_line` 直接拿到 EOF，用例必红，且干净 HEAD 上同样红，与任何改动无关。
//!
//! 处理方式不是放宽断言，也不是给产品开「跳过锁屏检测」的后门（那等于把安全
//! 判定变成可配置），而是只在**测试子进程**的 PATH 前面放一个 `ioreg` 替身，
//! 把这台机器的回答钉成「未锁屏」。CLI 分发、`--port 0`、token 文件、TCP 握手、
//! JSON-RPC、真会话 `sleep/printf` 全都仍然是真的；系统其余命令仍从原 PATH 找。
//! 非 macOS 本来就没有锁屏探测，不需要注入（见 `bridge/lock.rs`）。

use std::io::{BufRead, BufReader, Write};
use std::net::{Ipv4Addr, TcpStream};
use std::process::{Child, Command, Stdio};
use std::time::Duration;

use serde_json::{json, Value};

/// 一条起来的 `desk serve`：Drop 时收尸，测试中途 panic 也不留孤儿进程。
struct Serve {
    child: Child,
    port: u16,
    token: String,
    _dir: std::path::PathBuf,
}

impl Drop for Serve {
    fn drop(&mut self) {
        // 先杀再等：只 kill 不 wait 会留僵尸；只 wait 会永远等下去。
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// 把测试子进程的锁屏探测回答钉成「未锁屏」（理由见文件头）。
///
/// `lock::judge` 只找 `CGSSessionScreenIsLocked` 键后面那个 token，所以替身
/// 输出一行足够；`No` 会走 `Some(false)` 分支，正常放行。
#[cfg(target_os = "macos")]
fn neutralize_lock_probe(dir: &std::path::Path, cmd: &mut Command) {
    use std::os::unix::fs::PermissionsExt;

    let bin = dir.join("bin");
    std::fs::create_dir_all(&bin).expect("建探针替身目录");
    let script = bin.join("ioreg");
    std::fs::write(
        &script,
        "#!/bin/sh\nprintf '%s\\n' '  \"IOConsoleUsers\" = ({\"CGSSessionScreenIsLocked\" = No})'\n",
    )
    .expect("写 ioreg 替身");
    std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755))
        .expect("给 ioreg 替身可执行位");
    // 只**前置**替身目录：后面的系统 PATH 原样保留，`session_open` 里的
    // `sleep` / `printf` 与 PTY 启动都照常找得到。
    let mut paths = vec![bin];
    paths.extend(std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default()));
    cmd.env("PATH", std::env::join_paths(paths).expect("拼 PATH"));
}

#[cfg(not(target_os = "macos"))]
fn neutralize_lock_probe(_dir: &std::path::Path, _cmd: &mut Command) {}

/// 拉起 `desk serve --port 0`，从 stdout 那一行里读回真实端口。
fn start() -> Serve {
    let dir = std::env::temp_dir().join(format!(
        "zterm-serve-e2e-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("时钟")
            .as_nanos()
    ));
    std::fs::create_dir_all(&dir).expect("建临时目录");
    let token_path = dir.join("bridge.token");

    let mut cmd = Command::new(env!("CARGO_BIN_EXE_term-desk"));
    cmd.args(["serve", "--port", "0", "--token-file"])
        .arg(&token_path);
    neutralize_lock_probe(&dir, &mut cmd);
    let mut child = cmd
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("拉起 term-desk serve");

    // 监听信息的形状由 `bridge::serve` 定义：
    //   term-desk serve 监听 127.0.0.1:<port>（token：<path>）
    // 只认「127.0.0.1:」后面那串数字，不整行匹配——那行带中文与路径，
    // 整行比对会让改一个字就红。
    let stdout = child.stdout.take().expect("stdout 管道");
    let mut line = String::new();
    BufReader::new(stdout)
        .read_line(&mut line)
        .expect("读监听信息");
    let port = line
        .split("127.0.0.1:")
        .nth(1)
        .map(|rest| {
            rest.chars()
                .take_while(char::is_ascii_digit)
                .collect::<String>()
        })
        .and_then(|digits| digits.parse::<u16>().ok())
        .unwrap_or_else(|| panic!("监听信息里没有端口：{line:?}（stderr 见下）"));

    let token = std::fs::read_to_string(&token_path).expect("token 文件必须已生成");
    Serve {
        child,
        port,
        token: token.trim().to_string(),
        _dir: dir,
    }
}

/// 一问一答（服务端一行一条 JSON）。
fn request(writer: &mut TcpStream, reader: &mut BufReader<TcpStream>, value: &Value) -> Value {
    writer
        .write_all(format!("{value}\n").as_bytes())
        .expect("写请求");
    writer.flush().expect("冲刷");
    let mut line = String::new();
    reader.read_line(&mut line).expect("读回包");
    assert!(!line.is_empty(), "服务端没有回包（请求：{value}）");
    serde_json::from_str(&line).unwrap_or_else(|err| panic!("回包不是 JSON：{err} / {line}"))
}

#[test]
fn the_serve_binary_answers_jsonrpc_on_the_port_it_announces() {
    let serve = start();
    let stream = TcpStream::connect((Ipv4Addr::LOCALHOST, serve.port))
        .unwrap_or_else(|err| panic!("连不上 {}：{err}", serve.port));
    stream
        .set_read_timeout(Some(Duration::from_secs(15)))
        .expect("超时");
    let mut reader = BufReader::new(stream.try_clone().expect("克隆"));
    let mut writer = stream;

    // 1) 未握手的请求必须被拒——这条同时证明「连上来的确实是我们起的那个服务」。
    let refused = request(
        &mut writer,
        &mut reader,
        &json!({"jsonrpc":"2.0","id":1,"method":"describe"}),
    );
    assert_eq!(refused["error"]["code"], json!(-32003), "未握手：{refused}");

    // 服务端拒绝之后收线：重新连一条走完流程（与真实客户端行为一致）。
    drop(reader);
    let stream = TcpStream::connect((Ipv4Addr::LOCALHOST, serve.port)).expect("重连");
    stream
        .set_read_timeout(Some(Duration::from_secs(15)))
        .expect("超时");
    let mut reader = BufReader::new(stream.try_clone().expect("克隆"));
    let mut writer = stream;

    // 2) 握手 + 自省：describe 的动词表必须与 `term-core` 的表逐字一致。
    let hello = request(
        &mut writer,
        &mut reader,
        &json!({"jsonrpc":"2.0","id":2,"method":"auth","params":{"token": serve.token}}),
    );
    assert_eq!(hello["result"]["ok"], json!(true), "握手：{hello}");

    let described = request(
        &mut writer,
        &mut reader,
        &json!({"jsonrpc":"2.0","id":3,"method":"describe"}),
    );
    let names: Vec<&str> = described["result"]["methods"]
        .as_array()
        .expect("methods 是数组")
        .iter()
        .map(|entry| entry["name"].as_str().expect("方法名是字符串"))
        .collect();
    assert_eq!(
        names,
        term_core::surface::names(),
        "产品二进制报出来的动词表与 term-core 漂移了"
    );

    // 3) 真会话走一圈：开 → 写 → 等 → 屏 → 关。
    let opened = request(
        &mut writer,
        &mut reader,
        &json!({"jsonrpc":"2.0","id":4,"method":"session_open",
                "params":{"command":"sleep 0.2; printf 'SERVE_E2E_OK\\n'; sleep 5"}}),
    );
    let session = opened["result"]["name"].as_str().expect("会话名");

    let waited = request(
        &mut writer,
        &mut reader,
        &json!({"jsonrpc":"2.0","id":5,"method":"wait",
                "params":{"session":session,"pattern":"SERVE_E2E_OK","timeout_ms":15_000}}),
    );
    assert!(
        waited["result"]["text_upto"]
            .as_str()
            .unwrap_or("")
            .contains("SERVE_E2E_OK"),
        "wait 该拿到 printf 的输出：{waited}"
    );

    let screen = request(
        &mut writer,
        &mut reader,
        &json!({"jsonrpc":"2.0","id":6,"method":"screen","params":{"session":session}}),
    );
    assert!(
        screen["result"]["text"]
            .as_str()
            .unwrap_or("")
            .contains("SERVE_E2E_OK"),
        "屏幕上该有它：{screen}"
    );

    let state = request(
        &mut writer,
        &mut reader,
        &json!({"jsonrpc":"2.0","id":7,"method":"state","params":{"session":session}}),
    );
    assert_eq!(state["result"]["alive"], json!(true), "会话还在跑：{state}");

    let closed = request(
        &mut writer,
        &mut reader,
        &json!({"jsonrpc":"2.0","id":8,"method":"session_close","params":{"session":session}}),
    );
    assert_eq!(closed["result"]["closed"], json!(true));
}

#[test]
fn a_bad_argument_exits_two_and_never_binds() {
    // 参数错是**用法错误**，不是运行失败：退出码 2，且不留下监听。
    // 这条与 `cli/serve.rs` 的单测互为表里——那边验 `parse`，这边验退出码
    // 与「二进制真的没起监听」。
    let out = Command::new(env!("CARGO_BIN_EXE_term-desk"))
        .args(["serve", "--port", "abc"])
        .stdin(Stdio::null())
        .output()
        .expect("拉起 term-desk serve");
    assert_eq!(out.status.code(), Some(2), "用法错误应是退出码 2");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("--port"),
        "报错要点名出问题的参数：{stderr:?}"
    );
    assert!(
        out.stdout.is_empty(),
        "失败时不该打印监听信息：{:?}",
        String::from_utf8_lossy(&out.stdout)
    );
}

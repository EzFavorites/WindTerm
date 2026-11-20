//! 验收：外部进程经 JSON-RPC over stdio 操作**真 PTY 会话**。
//!
//! 两条端到端：
//!
//! 1. [`a_bridge_session_flows_from_open_to_close_over_jsonrpc`] —— 进程内
//!    用 `dispatch` 走完 `describe → session_open → write → keys → wait →
//!    screen → state → facets → session_list → session_close`，全部真会话。
//! 2. [`an_external_python_script_completes_the_same_flow_over_stdio`] ——
//!    **外部 python3 进程**用生成的桩跑同一套流程，桥接这一端由
//!    [`term_bridge::serve_stdio`] 驱动。这是 ADR-0019 的原文口径
//!    （外部 Python 脚本完成同一登录 + 取输出）。
//!
//! # 寻址是会话名
//!
//! 每个动词的 `session` 参数装的是**名字**（ADR-0024），不是整数 id。
//! 名字跨进程稳定，所以「开一条 → 存下名字 → 稍后接着用」成立；线上形状里
//! 也根本没有 id 可拿。
//!
//! # 两个刻意的测试技巧
//!
//! - **`tr a-z A-Z` 消除回显陷阱**：PTY 会把我们写进去的字节回显一遍，
//!   所以"等到某个串"若用发送原文当模式，命中的可能是回显而不是程序的输出。
//!   这里发小写、等大写——只有 `tr` 真的跑起来才可能出现大写串。
//! - **`sleep` 消除登记竞态**：`wait` 只能在 `write` 之后调用（同一线程串行），
//!   所以让 `tr` 晚 200/300ms 才 `exec`，确保它产生输出时登记已经生效
//!   （§B.13 先登记后消费）。

mod support;

use std::io::BufReader;
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::time::{Duration, Instant};

use serde_json::{json, Value};

use term_bridge::{dispatch, serve_stdio};
use term_core::surface;
use term_session::SessionPool;

use support::PoolSurface;

/// 本轮真 PTY 用例的超时上限——给足余量，慢机器上也不该红。
const WAIT_MS: u64 = 10_000;

/// 发一个请求并解析回包。
fn call(surface: &PoolSurface, id: u64, method: &str, params: Value) -> Value {
    let request = json!({
        "jsonrpc": "2.0",
        "id": id,
        "method": method,
        "params": params,
    })
    .to_string();
    let reply = dispatch(surface, &request).unwrap_or_else(|| panic!("{method} 必须回包"));
    serde_json::from_str(&reply).unwrap_or_else(|err| panic!("{method} 的回包不是 JSON：{err}（{reply}）"))
}

/// 断言回包成功，返回 `result`。
fn ok(reply: Value) -> Value {
    assert!(reply.get("error").is_none(), "预期成功，实际失败：{reply}");
    reply
        .get("result")
        .cloned()
        .unwrap_or_else(|| panic!("成功回包缺 result：{reply}"))
}

/// 断言回包失败，返回 `error`。
fn err(reply: Value) -> Value {
    assert!(reply.get("result").is_none(), "预期失败，实际成功：{reply}");
    reply
        .get("error")
        .cloned()
        .unwrap_or_else(|| panic!("失败回包缺 error：{reply}"))
}

/// 轮询屏幕直到出现 `needle`（字节流先到、网格投影略滞后，这是设计内的）。
fn screen_until(surface: &PoolSurface, session: &str, needle: &str) -> String {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let result = ok(call(
            surface,
            900,
            "screen",
            json!({ "session": session }),
        ));
        let text = result["text"].as_str().expect("text 是字符串").to_string();
        if text.contains(needle) {
            return text;
        }
        assert!(
            Instant::now() < deadline,
            "屏幕上始终没出现 {needle:?}，当前屏幕：\n{text}"
        );
        std::thread::sleep(Duration::from_millis(20));
    }
}

#[test]
fn a_bridge_session_flows_from_open_to_close_over_jsonrpc() {
    let pool = Arc::new(SessionPool::new());
    let surface = PoolSurface::new(Arc::clone(&pool));

    // 1) describe 必须与 term-core 的表逐字一致（客户端靠它发现能力）。
    let described = ok(call(&surface, 1, "describe", json!({})));
    let names: Vec<&str> = described["methods"]
        .as_array()
        .expect("methods 是数组")
        .iter()
        .map(|entry| entry["name"].as_str().expect("方法名是字符串"))
        .collect();
    assert_eq!(names, surface::names(), "describe 与 METHODS 漂移了");

    // 2) session_open：真 PTY + /bin/sh -c。发小写、等大写，避开回显陷阱。
    const CMD: &str = "sleep 0.2; exec tr a-z A-Z";
    let opened = ok(call(
        &surface,
        2,
        "session_open",
        json!({ "command": CMD }),
    ));
    // 名字是寻址键：exec 会话以命令文本为名，返回的必须是它（不是 id）。
    let session = opened["name"].as_str().expect("name 是字符串").to_string();
    assert_eq!(session, CMD, "exec 会话以命令文本为名");
    assert!(
        opened.get("id").is_none(),
        "线上形状里不该再有 id（ADR-0024）：{opened}"
    );

    // 3) session_list 里应该正好有它。
    let listed = ok(call(&surface, 3, "session_list", json!({})));
    let names: Vec<&str> = listed["sessions"]
        .as_array()
        .expect("sessions 是数组")
        .iter()
        .map(|entry| entry["name"].as_str().expect("name 是字符串"))
        .collect();
    assert_eq!(names, vec![CMD], "刚开的会话必须在册且唯一");

    // 4) write + wait。
    let written = ok(call(
        &surface,
        4,
        "write",
        json!({ "session": session, "text": "bridge_e2e_ok\n" }),
    ));
    assert_eq!(written["written"], true);

    let hit = ok(call(
        &surface,
        5,
        "wait",
        json!({ "session": session, "pattern": "BRIDGE_E2E_OK", "timeout_ms": WAIT_MS }),
    ));
    let text_upto = hit["text_upto"].as_str().expect("text_upto 是字符串");
    assert!(
        text_upto.contains("BRIDGE_E2E_OK"),
        "wait 命中却不在 text_upto 里：{text_upto:?}"
    );

    // 4b) keys：**按名字**按回车。行是 write 送进去的，回车是 keys 按的——
    // 这条拆开验的是「write 不自动补换行、keys 认键名」两件事同时成立。
    let typed = ok(call(
        &surface,
        41,
        "write",
        json!({ "session": session, "text": "echo KEYS_E2E_OK" }),
    ));
    assert_eq!(typed["written"], true);
    let pressed = ok(call(
        &surface,
        42,
        "keys",
        json!({ "session": session, "keys": ["enter"] }),
    ));
    assert_eq!(pressed["written"], true);
    assert_eq!(pressed["bytes"], 1, "enter 是单个 CR");
    let keys_hit = ok(call(
        &surface,
        43,
        "wait",
        json!({ "session": session, "pattern": "KEYS_E2E_OK", "timeout_ms": WAIT_MS }),
    ));
    assert!(
        keys_hit["text_upto"]
            .as_str()
            .is_some_and(|text| text.contains("KEYS_E2E_OK")),
        "keys 按下的回车应当让 shell 真的执行了那条命令：{keys_hit}"
    );
    // 未知键名是参数错误，且**一个字节都不写**（整批翻译完才落盘）。
    let bad_key = err(call(
        &surface,
        44,
        "keys",
        json!({ "session": session, "keys": ["ctrl+enter"] }),
    ));
    assert_eq!(bad_key["kind"], "invalid_params");

    // 5) screen：纯读，且能反复调（不消费输出）。
    let text = screen_until(&surface, &session, "BRIDGE_E2E_OK");
    let first = ok(call(
        &surface,
        6,
        "screen",
        json!({ "session": session }),
    ));
    let second = ok(call(
        &surface,
        7,
        "screen",
        json!({ "session": session }),
    ));
    assert_eq!(first, second, "screen 不得改变任何状态");
    assert_eq!(first["cols"], 80, "缺省尺寸 80×24");
    assert_eq!(first["rows"], 24);
    assert!(text.contains("BRIDGE_E2E_OK"), "屏幕上有 tr 的大写输出");

    // 6) state / facets。
    let state = ok(call(
        &surface,
        8,
        "state",
        json!({ "session": session }),
    ));
    assert_eq!(state["alive"], true, "tr 还在跑，会话应是活的");
    assert!(state["exit"].is_null(), "活着的会话没有退出状态");
    let caps = ok(call(
        &surface,
        9,
        "facets",
        json!({ "session": session }),
    ));
    assert_eq!(
        caps["facets"],
        json!([]),
        "本地 PTY 不暴露任何 facet——空表不是错误"
    );

    // 7) 错误路径必须真的报错：空模式是参数问题，不存在的会话名是 not_found。
    let empty = err(call(
        &surface,
        10,
        "wait",
        json!({ "session": session, "pattern": "" }),
    ));
    assert_eq!(empty["code"], -32602);
    assert_eq!(empty["kind"], "invalid_params");
    // 名字打空报 not_found；**不会**悄悄打中另一条（ADR-0024 决策 4）。
    let missing = err(call(
        &surface,
        11,
        "screen",
        json!({ "session": "no-such-session" }),
    ));
    assert_eq!(missing["code"], -32002);
    assert_eq!(missing["kind"], "not_found");
    // 数字 id 不给逃生门：类型就不对，报参数错。
    let numeric = err(call(&surface, 15, "screen", json!({ "session": 3 })));
    assert_eq!(numeric["kind"], "invalid_params");

    // 8) session_close，然后会话真的不在了——再关一次必须是 not_found，
    // 而不是空成功。
    let closed = ok(call(
        &surface,
        12,
        "session_close",
        json!({ "session": session }),
    ));
    assert_eq!(closed["closed"], true);
    assert!(pool.list().is_empty(), "关闭后控制面登记表应为空");
    let again = err(call(
        &surface,
        13,
        "session_close",
        json!({ "session": session }),
    ));
    assert_eq!(again["kind"], "not_found");
    let after = ok(call(&surface, 14, "session_list", json!({})));
    assert_eq!(after["sessions"], json!([]));
}

#[test]
fn an_external_python_script_completes_the_same_flow_over_stdio() {
    let dir = std::env::temp_dir().join(format!("term-bridge-e2e-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("建临时目录");
    std::fs::write(dir.join("term_bridge_stub.py"), term_bridge::python_stub()).expect("写桩");
    std::fs::write(dir.join("driver.py"), PYTHON_DRIVER).expect("写驱动脚本");

    let python = python_interpreter();
    let mut child = Command::new(&python)
        .arg(dir.join("driver.py"))
        .current_dir(&dir)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap_or_else(|err| panic!("无法启动 {python}：{err}"));

    let child_stdout = child.stdout.take().expect("拿到 python 的 stdout");
    let mut child_stdin = child.stdin.take().expect("拿到 python 的 stdin");

    // 桥接这一端：python 的 stdout 是请求源，python 的 stdin 是响应流。
    // 同一个 `serve_stdio`，与将来 CLI 子命令跑的是同一份代码。
    let pool = Arc::new(SessionPool::new());
    let surface = PoolSurface::new(Arc::clone(&pool));
    serve_stdio(&surface, BufReader::new(child_stdout), &mut child_stdin)
        .expect("stdio 桥接服务不该失败");

    let output = child.wait_with_output().expect("等 python 退出");
    let stderr = String::from_utf8_lossy(&output.stderr).to_string();
    assert!(
        output.status.success(),
        "python 驱动退出码非 0（{:?}）：\n{stderr}",
        output.status.code()
    );

    let verdict_line = stderr
        .lines()
        .rev()
        .find(|line| line.trim_start().starts_with('{'))
        .unwrap_or_else(|| panic!("python 驱动没有吐出判定 JSON，stderr：\n{stderr}"));
    let verdict: Value = serde_json::from_str(verdict_line)
        .unwrap_or_else(|err| panic!("判定不是合法 JSON：{err}（{verdict_line}）"));

    assert_eq!(
        verdict["ok"], true,
        "python 端流程失败：{}",
        verdict["error"]
    );
    assert_eq!(
        verdict["describe_count"],
        // ADR-0023：远程 describe 只含 RPC 可见的动词，脚本专用不列。
        surface::names().len(),
        "python 看到的 describe 条目数与表不一致：{verdict}"
    );
    assert_eq!(verdict["names_match_stub"], true, "桩里的表与桥接的表漂移了：{verdict}");
    assert_eq!(verdict["wait_has"], true, "wait 的 text_upto 里没有命中串：{verdict}");
    assert_eq!(verdict["screen_has"], true, "屏幕里没有大写输出：{verdict}");
    assert_eq!(verdict["screen_cols"], 80);
    assert_eq!(verdict["screen_rows"], 24);
    assert_eq!(verdict["alive"], true, "tr 应仍在运行：{verdict}");
    assert_eq!(verdict["caps"], json!([]));
    let session = verdict["session"].as_str().expect("session 是字符串");
    assert_eq!(
        verdict["listed_before"],
        json!([session]),
        "开出来的会话必须在 session_list 里：{verdict}"
    );
    assert_eq!(verdict["closed"], true);
    assert_eq!(
        verdict["listed_after"],
        json!([]),
        "关闭后 session_list 应为空：{verdict}"
    );
    assert_eq!(
        verdict["empty_kind"], "invalid_params",
        "空模式必须是参数错误：{verdict}"
    );
    assert_eq!(
        verdict["missing_kind"], "not_found",
        "已关闭的会话再问状态必须报 not_found：{verdict}"
    );

    assert!(
        pool.list().is_empty(),
        "python 脚本自己关掉了会话，控制面不该留下残余"
    );
    // 收尾：临时目录不留垃圾（失败也只是垃圾，不影响判定）。
    drop(std::fs::remove_dir_all(&dir));
    // 打一行判定：`cargo test -- --nocapture` 时能直接看到外部客户端究竟看到了什么。
    println!("外部 python 客户端判定：{verdict}");
}

/// 找得到就返回，找不到**显式失败**。
///
/// 这条路刻意没有「跳过」分支：本用例就是 ADR-0019 的验收口径
/// （外部 Python 脚本完成同一套动作）。静默跳过等于把验收悄悄取消，
/// 而那正是 ADR 里最反对的「看起来全绿、其实没验」。
fn python_interpreter() -> String {
    for candidate in ["python3", "python"] {
        let available = Command::new(candidate)
            .arg("--version")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .map(|status| status.success())
            .unwrap_or(false);
        if available {
            return candidate.to_string();
        }
    }
    panic!("本机没有可用的 python3 / python，S-c 的外部客户端验收无法执行；请安装 python3 后重跑")
}

/// 外部客户端脚本：只用标准库，跑完整流程，最后把判定 JSON 打到 stderr。
///
/// 为什么判定走 stderr：stdout 是 JSON-RPC 的请求流，往里打一行人话就会
/// 污染协议；这也顺便验证了「桥接的 stdout 是纯 JSON」这条纪律。
const PYTHON_DRIVER: &str = r#"import json
import os
import sys
import time

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))

from term_bridge_stub import METHODS, BridgeClient, BridgeError

NEEDLE = "PYTHON_BRIDGE_OK"
TIMEOUT_MS = 10000

verdict = {"ok": False, "error": None}
client = BridgeClient()

try:
    described = client.describe()
    names = sorted(entry["name"] for entry in described["methods"])
    stub_names = sorted(entry["name"] for entry in METHODS)

    opened = client.session_open(command="sleep 0.3; exec tr a-z A-Z")
    session = opened["name"]
    client.write(session, "python_bridge_ok\n")
    hit = client.wait(session, NEEDLE, timeout_ms=TIMEOUT_MS)

    deadline = time.time() + 5.0
    screen = client.screen(session)
    while NEEDLE not in screen["text"] and time.time() < deadline:
        time.sleep(0.02)
        screen = client.screen(session)

    state = client.state(session)
    caps = client.facets(session)
    listed_before = [entry["name"] for entry in client.session_list()["sessions"]]

    try:
        client.wait(session, "")
        empty_kind = None
    except BridgeError as exc:
        empty_kind = exc.kind

    closed = client.session_close(session)["closed"]
    listed_after = [entry["name"] for entry in client.session_list()["sessions"]]

    try:
        client.state(session)
        missing_kind = None
    except BridgeError as exc:
        missing_kind = exc.kind

    verdict.update({
        "ok": True,
        "describe_count": len(described["methods"]),
        "names": names,
        "names_match_stub": names == stub_names,
        "session": session,
        "wait_has": NEEDLE in hit["text_upto"],
        "screen_has": NEEDLE in screen["text"],
        "screen_cols": screen["cols"],
        "screen_rows": screen["rows"],
        "alive": state["alive"],
        "exit": state["exit"],
        "caps": caps["facets"],
        "listed_before": listed_before,
        "closed": closed,
        "listed_after": listed_after,
        "empty_kind": empty_kind,
        "missing_kind": missing_kind,
    })
except Exception as exc:
    verdict["error"] = "%s: %s" % (type(exc).__name__, exc)

sys.stderr.write(json.dumps(verdict) + "\n")
sys.stderr.flush()
"#;

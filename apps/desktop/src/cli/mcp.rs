//! `term-desk mcp` —— stdio MCP server，把前台能力面暴露成一组 tools。
//!
//! # 工具从哪来
//!
//! **只有 describe**。启动连上前台控制端点后先发一条 `describe`，把每个方法的
//! `params_schema` 原样变成 MCP tool 的 `inputSchema`，`summary` 变成
//! `description`。一方法一工具；没有任何手写的第二份动词表。
//! `describe` 本身来自 `term_core::surface::METHODS` 的 RPC 投影，所以脚本
//! 专用动词（`fs_*` / `proc_run` / `app_quit` / `window` / `clipboard_*`）
//! **不会**出现在这里——"API 默认全开"不等于放宽可见性。
//!
//! # 调用边界
//!
//! - `session` 一律显式传，不自动绑定当前会话；
//! - `wait` 的 `timeout_ms` 在 schema 与实参两侧都压到宿主工具超时之下，
//!   避免 MCP 客户端先超时、前台还在等，回包永远送不到；
//! - 没有 resources / prompts / logging / 推送 / 订阅，capabilities 只有 tools。
//!
//! 前台没起来时启动即失败退出，绝不自建会话池。

use std::io::{BufRead, BufReader, BufWriter, Write};

use serde_json::{json, Value};

use crate::bridge::client::{Client, ClientError};

/// MCP 客户端给单个工具的墙钟上限。留 2 秒给前台回包与序列化。
const HOST_TOOL_TIMEOUT_MS: u64 = 10_000;
const MAX_WAIT_TIMEOUT_MS: u64 = 8_000;

/// 与 `cli/api` 同一份平台端点描述：unix 是固定 UDS 路径，Windows 是按用户
/// SID 命名的管道；CLI 侧不硬编码任何具体路径。
fn usage() -> String {
    format!(
        "\
term-desk mcp

  stdio MCP server：工具由前台 describe 的 params_schema 生成，一方法一工具。
  连前台窗口的 {}；前台未运行时明确报错退出；不提供推送 / 订阅 / 其它 capability。
",
        crate::bridge::endpoint_description()
    )
}

/// 从 describe 的结果生成 MCP tools。纯函数：便于证明「没有第二份清单」。
pub fn tools_from_describe(described: &Value) -> Vec<Value> {
    let Some(methods) = described.get("methods").and_then(Value::as_array) else {
        return Vec::new();
    };
    methods
        .iter()
        .filter_map(|method| {
            let name = method["name"].as_str()?;
            let summary = method["summary"].as_str().unwrap_or(name);
            let mut input = method["params_schema"].clone();
            if !input.is_object() {
                return None;
            }
            if name == "wait" {
                cap_wait_timeout(&mut input);
            }
            Some(json!({
                "name": name,
                "description": format!(
                    "{summary}\n\n会话参数 session 必须显式传入；本工具不会自动绑定当前会话。"
                ),
                "inputSchema": input,
            }))
        })
        .collect()
}

/// 把 `timeout_ms` 的上限写进 schema：客户端自己就别填超时的值。
fn cap_wait_timeout(schema: &mut Value) {
    if let Some(timeout) = schema.get_mut("properties").and_then(|p| p.get_mut("timeout_ms")) {
        timeout["maximum"] = json!(MAX_WAIT_TIMEOUT_MS);
        timeout["description"] = json!(format!(
            "等待上限（毫秒）；MCP 工具墙钟 {HOST_TOOL_TIMEOUT_MS}ms，此处最多 {MAX_WAIT_TIMEOUT_MS}ms"
        ));
    }
}

/// 实参侧再压一次：手写客户端绕过 schema 时也不会把工具拖过宿主超时。
fn clamp_wait_arguments(method: &str, arguments: &mut Value) {
    if method != "wait" {
        return;
    }
    let too_long = arguments
        .get("timeout_ms")
        .and_then(Value::as_u64)
        .is_some_and(|ms| ms > MAX_WAIT_TIMEOUT_MS);
    if too_long {
        arguments["timeout_ms"] = json!(MAX_WAIT_TIMEOUT_MS);
    }
}

fn error_content(err: &ClientError) -> Value {
    json!({
        "content": [{"type": "text", "text": err.to_string()}],
        "isError": true,
    })
}

/// 处理一条 MCP 消息；`None` = 通知，不回包。
fn handle(client: &mut Client, tools: &[Value], message: &Value) -> Option<Value> {
    let id = message.get("id").cloned().unwrap_or(Value::Null);
    let notification = message.get("id").is_none();
    let method = message["method"].as_str().unwrap_or("");
    let params = message.get("params").cloned().unwrap_or(json!({}));

    let reply = match method {
        "notifications/initialized" | "notifications/cancelled" => {
            if notification {
                return None;
            }
            json!({})
        }
        "initialize" => json!({
            "protocolVersion": "2024-11-05",
            "capabilities": {"tools": {}},
            "serverInfo": {"name": "windterm-control", "version": env!("CARGO_PKG_VERSION")},
        }),
        "ping" => json!({}),
        "tools/list" => json!({"tools": tools}),
        "tools/call" => {
            let name = params["name"].as_str().unwrap_or("");
            let mut arguments = params.get("arguments").cloned().unwrap_or(json!({}));
            if !arguments.is_object() {
                arguments = json!({});
            }
            clamp_wait_arguments(name, &mut arguments);
            match client.call(name, &arguments) {
                Ok(value) => {
                    let text = serde_json::to_string(&value).unwrap_or_else(|_| value.to_string());
                    json!({"content": [{"type": "text", "text": text}]})
                }
                Err(err) => error_content(&err),
            }
        }
        _ => {
            return Some(json!({
                "jsonrpc": "2.0",
                "id": id,
                "error": {
                    "code": -32601,
                    "message": format!("未知 MCP 方法 {method}"),
                    "kind": "method_not_found",
                },
            }))
        }
    };
    if notification {
        return None;
    }
    Some(json!({"jsonrpc": "2.0", "id": id, "result": reply}))
}

/// 启动即连前台并拉 describe；前台不在就报错退出，不进 stdio 循环。
pub fn run(rest: &[String]) -> i32 {
    if rest.iter().any(|arg| arg == "--help" || arg == "-h") {
        print!("{}", usage());
        return 0;
    }
    if !rest.is_empty() {
        eprintln!("mcp 不接受参数\n\n{}", usage());
        return 2;
    }
    let mut client = match Client::connect_default() {
        Ok(client) => client,
        Err(err) => {
            eprintln!("{err}");
            return 1;
        }
    };
    let described = match client.call("describe", &json!({})) {
        Ok(value) => value,
        Err(err) => {
            eprintln!("读取前台 describe 失败：{err}");
            return 1;
        }
    };
    let tools = tools_from_describe(&described);

    let stdin = std::io::stdin();
    let mut reader = BufReader::new(stdin.lock());
    let stdout = std::io::stdout();
    let mut writer = BufWriter::new(stdout.lock());
    let mut line = String::new();
    loop {
        line.clear();
        match reader.read_line(&mut line) {
            Ok(0) => return 0,
            Ok(_) => {}
            Err(err) => {
                eprintln!("读 MCP 请求失败：{err}");
                return 1;
            }
        }
        let message: Value = match serde_json::from_str(line.trim()) {
            Ok(value) => value,
            Err(err) => {
                let reply = json!({
                    "jsonrpc": "2.0",
                    "id": Value::Null,
                    "error": {"code": -32700, "message": err.to_string(), "kind": "parse_error"},
                });
                if write_reply(&mut writer, &reply).is_err() {
                    return 1;
                }
                continue;
            }
        };
        if let Some(reply) = handle(&mut client, &tools, &message) {
            if write_reply(&mut writer, &reply).is_err() {
                return 1;
            }
        }
    }
}

fn write_reply<W: Write>(writer: &mut BufWriter<W>, reply: &Value) -> std::io::Result<()> {
    serde_json::to_writer(&mut *writer, reply).map_err(std::io::Error::other)?;
    writer.write_all(b"\n")?;
    writer.flush()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tools_come_only_from_describe_and_never_expose_script_only_verbs() {
        let described = term_core::contract::describe_value();
        let tools = tools_from_describe(&described);
        let names: Vec<&str> = tools
            .iter()
            .map(|tool| tool["name"].as_str().expect("name"))
            .collect();
        assert_eq!(names, term_core::surface::names(), "工具表必须逐项等于 describe");
        for hidden in [
            "fs_read", "fs_write", "fs_list", "proc_run", "app_quit", "window",
            "clipboard_get", "clipboard_set",
        ] {
            assert!(!names.contains(&hidden), "脚本专用动词不得进 MCP：{hidden}");
        }
        // 每个 inputSchema 都是 describe 原样带来的对象，不是另一份手写清单。
        for (tool, spec) in tools.iter().zip(term_core::surface::rpc_specs()) {
            assert_eq!(tool["inputSchema"]["type"], json!("object"));
            let described: Value = serde_json::from_str(spec.params_schema).expect("合法 schema");
            if spec.name != "wait" {
                assert_eq!(tool["inputSchema"], described, "{} 的 schema 未逐字透传", spec.name);
            }
        }
    }

    #[test]
    fn session_is_an_explicit_argument_not_an_implicit_binding() {
        let tools = tools_from_describe(&term_core::contract::describe_value());
        let wait = tools.iter().find(|t| t["name"] == "wait").expect("wait 工具");
        let required = wait["inputSchema"]["required"].as_array().expect("required");
        assert!(required.iter().any(|v| v == "session"), "session 必须显式传");
        assert!(
            wait["description"]
                .as_str()
                .expect("description")
                .contains("必须显式传入"),
            "工具说明要说清不会自动绑定"
        );
    }

    #[test]
    fn wait_timeout_is_capped_below_the_host_tool_timeout() {
        assert!(MAX_WAIT_TIMEOUT_MS < HOST_TOOL_TIMEOUT_MS);
        let tools = tools_from_describe(&term_core::contract::describe_value());
        let wait = tools.iter().find(|t| t["name"] == "wait").expect("wait 工具");
        assert_eq!(wait["inputSchema"]["properties"]["timeout_ms"]["maximum"], json!(MAX_WAIT_TIMEOUT_MS));
        let mut args = json!({"timeout_ms": 999_999});
        clamp_wait_arguments("wait", &mut args);
        assert_eq!(args["timeout_ms"], json!(MAX_WAIT_TIMEOUT_MS));
        let mut other = json!({"timeout_ms": 999_999});
        clamp_wait_arguments("read", &mut other);
        assert_eq!(other["timeout_ms"], json!(999_999), "只压 wait");
    }

    #[test]
    fn a_rpc_failure_becomes_an_is_error_tool_result() {
        let err = ClientError::Rpc {
            code: -32000,
            message: "等待超时".to_string(),
            kind: "timeout".to_string(),
        };
        let result = error_content(&err);
        assert_eq!(result["isError"], json!(true));
        assert_eq!(result["content"][0]["text"], json!("[timeout:-32000] 等待超时"));
    }
}

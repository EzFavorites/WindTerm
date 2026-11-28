//! JSON-RPC 2.0 编解码与分派（ADR-0019 S-c）。
//!
//! 手工解析 `serde_json::Value`，**不用** `#[derive(Deserialize)]`：派生出来的
//! 报错只会说「missing field `method`」，而这一层要给客户端说清「哪个字段、
//! 实际是什么、应该是什么」。桥接的调用方是外部脚本，错误文案就是它的文档。
//!
//! 支持的范围（刻意就这么点，多一条都要有测试）：
//!
//! - 单请求、批量数组（按序回包；全是通知时**一个字节都不回**）；
//! - 通知（无 `id`）真实执行，但按规范不回包；
//! - 错误分十档见 [`term_core::surface_error::ErrorKind`]，码与 `kind` 一一对应。
//!
//! 明确**不支持**（不是遗漏，是决定）：`params` 用数组（位置参数）——桥接的
//! 参数一律按名传，数组形式会让「第 2 个参数是啥」随 schema 演进而静默错位；
//! 收到数组直接报 `invalid_params`。

use serde_json::{Map, Value};

use term_core::contract::Surface;
use term_core::surface_error::SurfaceError;

/// 本实现说的唯一协议版本。
pub const JSONRPC_VERSION: &str = "2.0";

/// 请求 / 响应的标识。
///
/// 规范允许 String / Number / Null。`Number` 用 `serde_json::Number` 原样
/// 携带，避免把 `1` 和 `1.0` 揉成同一个值（回包时 id 必须与请求**逐位**相同，
/// 否则客户端认不出自己的响应）。
#[derive(Clone, Debug, PartialEq)]
pub enum Id {
    Number(serde_json::Number),
    String(String),
    Null,
}

impl Id {
    /// 从 JSON 值取 id。非 String / Number / Null 是请求形状错误。
    pub fn from_value(value: &Value) -> Result<Id, SurfaceError> {
        match value {
            Value::String(text) => Ok(Id::String(text.clone())),
            Value::Number(number) => Ok(Id::Number(number.clone())),
            Value::Null => Ok(Id::Null),
            other => Err(SurfaceError::invalid_request(format!(
                "id 必须是字符串、数字或 null（实际：{other}）"
            ))),
        }
    }

    pub fn to_value(&self) -> Value {
        match self {
            Id::Number(number) => Value::Number(number.clone()),
            Id::String(text) => Value::String(text.clone()),
            Id::Null => Value::Null,
        }
    }
}

/// 线上的错误对象：`{code, message, kind}`。
///
/// `kind` 是**扩展成员**（JSON-RPC 2.0 规定 `code` / `message` 必填、`data`
/// 可选，未禁止其它成员）。不用 `data` 包一层，是因为 `kind` 是主信息，
/// 塞进「补充信息」里会让每个客户端都多写一行解包。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ErrorObject {
    pub code: i32,
    pub message: String,
    pub kind: String,
}

impl ErrorObject {
    pub fn to_value(&self) -> Value {
        let mut map = Map::new();
        map.insert("code".to_string(), Value::from(self.code));
        map.insert("message".to_string(), Value::String(self.message.clone()));
        map.insert("kind".to_string(), Value::String(self.kind.clone()));
        Value::Object(map)
    }
}

impl From<&SurfaceError> for ErrorObject {
    fn from(err: &SurfaceError) -> Self {
        Self {
            code: err.code,
            message: err.message.clone(),
            kind: err.kind.as_str().to_string(),
        }
    }
}

/// 回包的两种负载。不存在第三种：没有「成功但带警告」——
/// 有话说就写在结果里，有条件不满足就报错。
#[derive(Clone, Debug, PartialEq)]
pub enum Payload {
    Result(Value),
    Error(ErrorObject),
}

/// 一个响应信封。
#[derive(Clone, Debug, PartialEq)]
pub struct Response {
    pub id: Id,
    pub payload: Payload,
}

impl Response {
    pub fn result(id: Id, value: Value) -> Self {
        Self {
            id,
            payload: Payload::Result(value),
        }
    }

    pub fn error(id: Id, err: SurfaceError) -> Self {
        Self {
            id,
            payload: Payload::Error(ErrorObject::from(&err)),
        }
    }

    pub fn to_value(&self) -> Value {
        let mut map = Map::new();
        map.insert("jsonrpc".to_string(), Value::String(JSONRPC_VERSION.to_string()));
        map.insert("id".to_string(), self.id.to_value());
        match &self.payload {
            Payload::Result(value) => {
                map.insert("result".to_string(), value.clone());
            }
            Payload::Error(err) => {
                map.insert("error".to_string(), err.to_value());
            }
        }
        Value::Object(map)
    }

    pub fn to_json(&self) -> String {
        self.to_value().to_string()
    }
}

/// 解析后的请求。
#[derive(Clone, Debug, PartialEq)]
pub struct Request {
    pub id: Id,
    /// 无 `id` = 通知。规范给出 `"id": null` 是**合法请求**（要回包，
    /// id 为 null），与通知不是一回事——这一位专门分开这两种。
    pub notification: bool,
    pub method: String,
    pub params: Value,
}

impl Request {
    /// 解析一个请求对象。
    ///
    /// 失败时返回 `(可回显的 id, 错误)`：id 能解析出来就带上（规范要求
    /// 「能确定 id 就用它」），解析不出来是 null。
    pub fn parse(value: &Value) -> Result<Request, (Id, SurfaceError)> {
        let obj = match value {
            Value::Object(map) => map,
            other => {
                return Err((
                    Id::Null,
                    SurfaceError::invalid_request(format!(
                        "请求必须是 JSON 对象（实际：{other}）"
                    )),
                ))
            }
        };

        // 版本：缺失按 2.0 处理（本节只有一个版本，宽松接受省略是**显式决定**），
        // 写了但不是 2.0 则明确报错——那种情况一定是客户端接错了什么。
        match obj.get("jsonrpc") {
            None => {}
            Some(Value::String(version)) if version == JSONRPC_VERSION => {}
            Some(other) => {
                return Err((
                    Id::Null,
                    SurfaceError::invalid_request(format!(
                        "jsonrpc 必须是 \"{JSONRPC_VERSION}\"（实际：{other}）"
                    )),
                ))
            }
        }

        let (id, notification) = match obj.get("id") {
            None => (Id::Null, true),
            Some(value) => match Id::from_value(value) {
                Ok(id) => (id, false),
                Err(err) => return Err((Id::Null, err)),
            },
        };

        let method = match obj.get("method") {
            Some(Value::String(method)) if !method.is_empty() => method.clone(),
            Some(Value::String(_)) => {
                return Err((
                    id,
                    SurfaceError::invalid_request("method 不能是空串（要发现方法请调 describe）"),
                ))
            }
            None => {
                return Err((
                    id,
                    SurfaceError::invalid_request("缺少 method 字段（字符串）"),
                ))
            }
            Some(other) => {
                return Err((
                    id,
                    SurfaceError::invalid_request(format!("method 必须是字符串（实际：{other}）")),
                ))
            }
        };

        // 参数一律按名传：对象或省略。数组形式故意不接受（见模块文档）。
        let params = match obj.get("params") {
            None | Some(Value::Null) => Value::Object(Map::new()),
            Some(value @ Value::Object(_)) => value.clone(),
            Some(other @ Value::Array(_)) => {
                return Err((
                    id,
                    SurfaceError::invalid_params(format!(
                        "params 必须按名传（对象）；不接受位置参数数组（实际：{other}）"
                    )),
                ))
            }
            Some(other) => {
                return Err((
                    id,
                    SurfaceError::invalid_params(format!("params 必须是对象（实际：{other}）")),
                ))
            }
        };

        Ok(Request {
            id,
            notification,
            method,
            params,
        })
    }
}

/// 一条通知：真实执行，但按协议没有回包通道。
#[derive(Clone, Debug, PartialEq)]
pub struct Notification {
    pub method: String,
    pub params: Value,
}

/// 把一行输入变成一行输出（或 `None` = 不该回任何字节）。
///
/// 这是桥接唯一的协议入口：transport 只管读行/写行，规则全在这里，
/// 所以「为什么这个输入得到这个答案」永远只有一个地方要读。
///
/// 返回值形状：
/// - 单个请求 → 一个 JSON 对象的字符串；
/// - 批量请求 → 一个 JSON 数组的字符串（通知项被剔除，全是通知时返回 `None`）；
/// - 通知 → `None`（一个字节都不回）；
/// - 非法输入 → 一个 `id: null` 的错误对象（**不 panic、不中断**：stdio 上
///   一行写坏了不该打死整条管道，那会把「脚本里少了个逗号」变成「桥接挂了」）。
pub fn dispatch(surface: &dyn Surface, input: &str) -> Option<String> {
    let parsed: Value = match serde_json::from_str(input.trim()) {
        Ok(value) => value,
        Err(err) => {
            return Some(
                Response::error(
                    Id::Null,
                    SurfaceError::parse_error(format!("请求不是合法 JSON：{err}")),
                )
                .to_json(),
            )
        }
    };

    match parsed {
        Value::Array(items) => {
            if items.is_empty() {
                // 规范：空数组是 Invalid Request（不是「空成功」）。
                return Some(
                    Response::error(
                        Id::Null,
                        SurfaceError::invalid_request("批量请求不能是空数组"),
                    )
                    .to_json(),
                );
            }
            let mut replies = Vec::with_capacity(items.len());
            for item in items {
                if let Some(response) = handle_one(surface, &item) {
                    replies.push(response.to_value());
                }
            }
            if replies.is_empty() {
                None
            } else {
                Some(Value::Array(replies).to_string())
            }
        }
        other => handle_one(surface, &other).map(|response| response.to_json()),
    }
}

/// 分派一个请求对象。
fn handle_one(surface: &dyn Surface, value: &Value) -> Option<Response> {
    let request = match Request::parse(value) {
        Ok(request) => request,
        Err((id, err)) => {
            // 解析失败但连通知标志都没解析出来时一律回包：宁可多回一个错误，
            // 也不要让调用方对着沉默猜自己哪儿错了。
            return Some(Response::error(id, err));
        }
    };

    if request.notification {
        run_notification(
            surface,
            Notification {
                method: request.method,
                params: request.params,
            },
        );
        return None;
    }

    let outcome = surface.invoke(&request.method, &request.params);
    Some(match outcome {
        Ok(value) => Response::result(request.id, value),
        Err(err) => Response::error(request.id, err),
    })
}

/// 执行一条通知。分派只走 [`Surface::invoke`]：内建 `describe` 与后端方法
/// 分派都在 `term-core` 的契约里收口，本文件不再自己认方法名（也就没有
/// 第二份清单可漂）。
///
/// 通知没有回包通道（协议规定），失败也无处回告——所以这里**不写日志**：
/// 桥接的 stdout 是纯 JSON 流，往里写一行人话就会让客户端解析失败。
/// 需要知道成败的调用方请用带 `id` 的请求。
fn run_notification(surface: &dyn Surface, notification: Notification) {
    let Notification { method, params } = notification;
    if let Err(err) = surface.invoke(&method, &params) {
        drop(err);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::ErrorKind;
    use serde_json::json;

    /// 记录被调用的方法，按需返回成功或指定的失败。
    struct Probe {
        seen: std::sync::Mutex<Vec<(String, Value)>>,
        fail_with: Option<SurfaceError>,
    }

    impl Probe {
        fn new() -> Self {
            Self {
                seen: std::sync::Mutex::new(Vec::new()),
                fail_with: None,
            }
        }

        fn failing(err: SurfaceError) -> Self {
            Self {
                seen: std::sync::Mutex::new(Vec::new()),
                fail_with: Some(err),
            }
        }

        fn called(&self) -> Vec<String> {
            self.seen
                .lock()
                .expect("probe 锁")
                .iter()
                .map(|(method, _)| method.clone())
                .collect()
        }
    }

    impl Surface for Probe {
        fn call(&self, method: &str, params: &Value) -> Result<Value, SurfaceError> {
            self.seen
                .lock()
                .expect("probe 锁")
                .push((method.to_string(), params.clone()));
            if let Some(err) = &self.fail_with {
                return Err(err.clone());
            }
            Ok(json!({"echo": method}))
        }
    }

    fn parse_reply(text: &str) -> Value {
        serde_json::from_str(text).expect("回包必须是合法 JSON")
    }

    #[test]
    fn a_single_request_gets_a_result_with_the_same_id() {
        let probe = Probe::new();
        let reply = dispatch(
            &probe,
            r#"{"jsonrpc":"2.0","id":7,"method":"session_list","params":{"session":"prod"}}"#,
        )
        .expect("请求必须回包");
        let value = parse_reply(&reply);
        assert_eq!(value["jsonrpc"], "2.0");
        assert_eq!(value["id"], 7);
        assert_eq!(value["result"], json!({"echo": "session_list"}));
        assert_eq!(probe.called(), vec!["session_list".to_string()]);
        assert_eq!(
            probe.seen.lock().expect("probe 锁")[0].1,
            json!({"session": "prod"}),
            "params 必须原样递给实现层"
        );
    }

    #[test]
    fn a_request_without_params_gets_an_empty_object() {
        // 实现层永远拿到对象，不必自己处理 null——少一处「忘了判空」的可能。
        let probe = Probe::new();
        dispatch(&probe, r#"{"jsonrpc":"2.0","id":1,"method":"session_list"}"#).expect("回包");
        assert_eq!(probe.seen.lock().expect("probe 锁")[0].1, json!({}));
    }

    #[test]
    fn describe_is_built_in_and_matches_the_surface_table() {
        // 任何 Surface（哪怕是个只会 echo 的探针）都必须能回答 describe。
        let probe = Probe::new();
        let reply = dispatch(&probe, r#"{"jsonrpc":"2.0","id":"a","method":"describe"}"#).expect("回包");
        let value = parse_reply(&reply);
        assert_eq!(value["id"], "a", "字符串 id 必须原样回显");
        let methods = value["result"]["methods"]
            .as_array()
            .expect("describe 结果里有 methods 数组");
        let names: Vec<&str> = methods
            .iter()
            .map(|m| m["name"].as_str().expect("方法名是字符串"))
            .collect();
        assert_eq!(names, term_core::surface::names());
        assert!(probe.called().is_empty(), "describe 不该落到实现层");
        // 每个条目都要带齐字段——客户端靠它们生成桩。
        for method in methods {
            for key in ["name", "summary", "kind", "params_schema", "result_schema"] {
                assert!(!method[key].is_null(), "describe 条目缺 {key}：{method}");
            }
        }
    }

    #[test]
    fn a_notification_runs_but_replies_nothing() {
        let probe = Probe::new();
        assert_eq!(
            dispatch(&probe, r#"{"jsonrpc":"2.0","method":"send","params":{"tab":3,"text":"ls\n"}}"#),
            None,
            "通知不得回包"
        );
        assert_eq!(probe.called(), vec!["send".to_string()], "通知仍要真实执行");
    }

    #[test]
    fn an_explicit_null_id_is_a_request_not_a_notification() {
        let probe = Probe::new();
        let reply = dispatch(&probe, r#"{"jsonrpc":"2.0","id":null,"method":"session_list"}"#)
            .expect("id:null 是请求，必须回包");
        let value = parse_reply(&reply);
        assert!(value.get("id").is_some());
        assert!(value["id"].is_null());
        assert!(value.get("result").is_some());
    }

    #[test]
    fn batch_requests_reply_in_order_and_skip_notifications() {
        let probe = Probe::new();
        let batch = r#"[
            {"jsonrpc":"2.0","id":1,"method":"session_list"},
            {"jsonrpc":"2.0","method":"write","params":{"session":"prod","text":"x"}},
            {"jsonrpc":"2.0","id":2,"method":"state"}
        ]"#;
        let reply = dispatch(&probe, batch).expect("有带 id 的就得回包");
        let value = parse_reply(&reply);
        let array = value.as_array().expect("批量请求回数组");
        assert_eq!(array.len(), 2, "通知项不进回包");
        assert_eq!(array[0]["id"], 1);
        assert_eq!(array[1]["id"], 2);
        assert_eq!(probe.called(), vec!["session_list".to_string(), "write".to_string(), "state".to_string()]);
    }

    #[test]
    fn a_batch_of_only_notifications_replies_nothing_at_all() {
        let probe = Probe::new();
        let batch = r#"[{"jsonrpc":"2.0","method":"write","params":{"session":"prod","text":"x"}}]"#;
        assert_eq!(dispatch(&probe, batch), None);
        assert_eq!(probe.called(), vec!["write".to_string()]);
    }

    #[test]
    fn an_empty_batch_is_an_invalid_request() {
        let probe = Probe::new();
        let reply = dispatch(&probe, "[]").expect("空批量要回包");
        let value = parse_reply(&reply);
        assert_eq!(value["error"]["code"], -32600);
        assert_eq!(value["error"]["kind"], "invalid_request");
    }

    #[test]
    fn malformed_json_is_a_parse_error_with_a_null_id() {
        let probe = Probe::new();
        let reply = dispatch(&probe, "{not json").expect("非法输入要回包");
        let value = parse_reply(&reply);
        assert_eq!(value["error"]["code"], -32700);
        assert_eq!(value["error"]["kind"], "parse_error");
        assert!(value["id"].is_null());
        assert!(value.get("result").is_none(), "错误回包不得带 result");
        assert!(probe.called().is_empty(), "解析失败不该落到实现层");
    }

    #[test]
    fn a_scalar_request_is_an_invalid_request() {
        for text in ["42", "\"hello\"", "true"] {
            let probe = Probe::new();
            let reply = dispatch(&probe, text).expect("要回包");
            let value = parse_reply(&reply);
            assert_eq!(value["error"]["code"], -32600, "{text} 应报 invalid_request");
        }
    }

    #[test]
    fn a_wrong_jsonrpc_version_is_an_invalid_request_but_omission_is_tolerated() {
        let probe = Probe::new();
        let reply = dispatch(&probe, r#"{"jsonrpc":"1.0","id":1,"method":"session_list"}"#).expect("回包");
        assert_eq!(parse_reply(&reply)["error"]["code"], -32600);
        // 省略版本：按 2.0 处理（显式决定，写在这里以便将来有人想改时先看见）。
        let reply = dispatch(&probe, r#"{"id":1,"method":"session_list"}"#).expect("回包");
        assert!(parse_reply(&reply).get("result").is_some());
    }

    #[test]
    fn positional_params_are_refused_with_a_readable_reason() {
        let probe = Probe::new();
        let reply = dispatch(&probe, r#"{"jsonrpc":"2.0","id":1,"method":"send","params":[1,"x"]}"#)
            .expect("回包");
        let value = parse_reply(&reply);
        assert_eq!(value["error"]["code"], -32602, "位置参数是 invalid_params，不是 internal");
        assert!(value["error"]["message"].as_str().expect("有文案").contains("按名传"));
    }

    #[test]
    fn every_surface_error_kind_survives_the_wire_unchanged() {
        // 九种失败都必须能原样穿过线上格式——尤其 code 与 kind 不能对不上。
        let kinds = [
            ErrorKind::ParseError,
            ErrorKind::InvalidRequest,
            ErrorKind::MethodNotFound,
            ErrorKind::InvalidParams,
            ErrorKind::Internal,
            ErrorKind::Timeout,
            ErrorKind::Closed,
            ErrorKind::NotFound,
            ErrorKind::Locked,
        ];
        for kind in kinds {
            let probe = Probe::failing(SurfaceError::new(kind, "原因"));
            let reply = dispatch(&probe, r#"{"jsonrpc":"2.0","id":9,"method":"wait"}"#).expect("回包");
            let value = parse_reply(&reply);
            assert_eq!(value["id"], 9);
            assert_eq!(value["error"]["code"], kind.code(), "{kind:?} 的码不对");
            assert_eq!(value["error"]["kind"], kind.as_str());
            assert_eq!(value["error"]["message"], "原因");
            assert!(value.get("result").is_none(), "{kind:?} 的错误回包不得带 result");
        }
    }

    #[test]
    fn a_method_not_on_the_surface_is_reported_as_method_not_found() {
        // 探针不认得任何方法，但它必须能回答 describe；其余一律 method_not_found。
        struct Empty;
        impl Surface for Empty {
            fn call(&self, method: &str, _params: &Value) -> Result<Value, SurfaceError> {
                Err(SurfaceError::method_not_found(format!("未知方法 {method}")))
            }
        }
        let reply = dispatch(&Empty, r#"{"jsonrpc":"2.0","id":1,"method":"ssh_connect"}"#).expect("回包");
        let value = parse_reply(&reply);
        assert_eq!(value["error"]["code"], -32601);
        assert_eq!(value["error"]["kind"], "method_not_found");
    }

    #[test]
    fn an_id_of_the_wrong_type_comes_back_as_null() {
        let probe = Probe::new();
        let reply = dispatch(&probe, r#"{"jsonrpc":"2.0","id":{"a":1},"method":"session_list"}"#).expect("回包");
        let value = parse_reply(&reply);
        assert_eq!(value["error"]["code"], -32600);
        assert!(value["id"].is_null(), "id 本身坏了就只能回 null");
    }

    #[test]
    fn numeric_ids_keep_their_exact_json_form() {
        // 1 与 1.0 不能混为一谈：回包必须逐位相同，否则严格客户端认不出自己的响应。
        let probe = Probe::new();
        let reply = dispatch(&probe, r#"{"jsonrpc":"2.0","id":1.0,"method":"session_list"}"#).expect("回包");
        assert!(
            reply.contains("\"id\":1.0"),
            "浮点 id 应原样回显，实际回包：{reply}"
        );
    }

    #[test]
    fn whitespace_only_input_is_a_parse_error_not_a_panic() {
        let probe = Probe::new();
        let reply = dispatch(&probe, "   \n\t ").expect("要回包");
        assert_eq!(parse_reply(&reply)["error"]["kind"], "parse_error");
    }
}

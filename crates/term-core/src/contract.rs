//! 控制面契约：[`Surface`] trait、统一调用入口与 `describe` 的唯一生成处。
//!
//! 这一层刻意**薄**。契约只要求后端「认得这些方法名、按 schema 收参数、
//! 按 schema 还结果，失败要说清是哪类失败」；具体的会话实现（`DeskSurface`
//! / 开发期夹具 `PoolSurface`）住在应用层与测试里，本模块不持有任何会话。
//!
//! 传输层（`term-bridge`）只做编解码与管道，不拥有契约：`term_bridge::handler`
//! 与 `term_bridge::error` 都是本模块与 [`crate::surface_error`] 的再导出。
//! 因此「`match method` 的后端分派」与「JSON-RPC 的分派」走的是同一个
//! [`Surface`] 契约与同一个 [`Surface::invoke`] 入口，不存在第二份方法名清单。
//!
//! # 实现者的四条硬约束
//!
//! 1. **未知方法报 `method_not_found`**，不许返回 `Ok(Value::Null)`。空成功
//!    会让客户端以为「这个方法什么都不用做」，而不是「我调错了名字」。
//! 2. **失败要分类**。返回 `internal` 而实际是超时，等于把「再等等」和
//!    「桥接坏了」揉成一件事（详见 [`crate::surface_error`]）。
//! 3. **不阻塞在别的线程上**。`wait` 会阻塞调用线程，这是设计内的（脚本
//!    语义），但实现者不许在自己内部再起线程池去「并发处理请求」——
//!    一条 stdio 管道就是一个串行的事实流。
//! 4. **不发通知、不写 stdout**。stdout 是纯 JSON 流，往里写日志就毁了协议。

use serde_json::{json, Value};

use crate::surface::{rpc_specs, MethodSpec};
use crate::surface_error::SurfaceError;

/// 控制面对后端会话能力面的全部要求。
///
/// `Send + Sync`：控制面可能被多个线程共享（一个连接一个线程，共用同一个
/// 控制面），实现者也不必为此额外加锁——`PoolSurface` 内部持有的 `SessionPool`
/// 本身就是 `&self` 可共享的。
pub trait Surface: Send + Sync {
    /// 执行一个方法。
    ///
    /// `params` 一定是 JSON 对象（协议层已把省略的 `params` 归一成 `{}`），
    /// 但**不保证**里面有哪个键——缺参数是 [`SurfaceError::invalid_params`]，
    /// 不是 panic，也不是「用默认值糊过去」。
    fn call(&self, method: &str, params: &Value) -> Result<Value, SurfaceError>;

    /// 统一调用入口：`describe` 由契约内建，其余方法落到实现者的 [`Surface::call`]。
    ///
    /// 「我支持什么」不是后端的可选功能：任何实现都自动具备能力发现，
    /// 而且答案只有一个生成处（[`describe_value`]）。传输层只调这个方法，
    /// 不自己维护「内建方法」清单；直接调 `call` 的进程内调用方仍由后端
    /// 自己回答 `describe`（见 `DeskSurface` 的实现），两侧形状一致。
    fn invoke(&self, method: &str, params: &Value) -> Result<Value, SurfaceError> {
        if method == "describe" {
            Ok(describe_value())
        } else {
            self.call(method, params)
        }
    }
}

/// `describe` 的答案：把 `term-core` 那张表变成 JSON，**内嵌 schema 用对象**。
///
/// 刻意没有「传输层自己的一份副本」：单一真相就是 [`crate::surface::METHODS`]，
/// 这里只做一次形状转换。任何手工维护的第二份清单都会漂移，
/// 而漂移的表现是「客户端按 describe 调用，桥接说不认识」。
///
/// **只列远程看得见的那些**（ADR-0023）：`surface::rpc_specs()` 已经按
/// [`crate::surface::Exposure`] 过滤过，脚本专用动词（`cursor_pos` 之类）
/// 不在这里出现——出现在远程动词表里等于告诉客户端「可以调」，而它调不到。
///
/// schema 在线上是**对象**而不是字符串：客户端拿到就能直接用，不必自己再
/// `json.loads` 一遍——少一步就少一处「忘了解析」的坑。
pub fn describe_value() -> Value {
    let methods: Vec<Value> = rpc_specs().map(method_value).collect();
    json!({ "methods": methods })
}

/// 一条件目的线上形状。
fn method_value(spec: &MethodSpec) -> Value {
    json!({
        "name": spec.name,
        "summary": spec.summary,
        "kind": spec.kind,
        "params_schema": embed_schema(spec.name, "params_schema", spec.params_schema),
        "result_schema": embed_schema(spec.name, "result_schema", spec.result_schema),
    })
}

/// 把 schema 文本还原成 JSON 值嵌进 describe 的结果里。
///
/// 解析失败就把原文当字符串发出去，**不 panic**：桥接被一句 describe 打死，
/// 比「这次 schema 是字符串形式」严重得多，而且客户端仍然看得见内容。
/// 这条兜底实际上不可达——`term-core` 的单测已经把「每个 schema 都是合法
/// JSON 对象」钉死了（`surface::tests::every_schema_is_a_valid_json_object`）。
fn embed_schema(method: &str, label: &str, text: &str) -> Value {
    match serde_json::from_str(text) {
        Ok(value) => value,
        Err(_) => {
            debug_assert!(
                false,
                "{method} 的 {label} 不是合法 JSON——term-core 的单测本该先红"
            );
            Value::String(text.to_string())
        }
    }
}

/// 取必填的 `session` 参数（**会话名**，不是 id——ADR-0024）。
///
/// 只接受非空字符串：名字是寻址键，池内唯一；空串没有意义（也没有哪条会话
/// 叫这个名字），当场拒比让后端去查一遍更早暴露客户端 bug。
///
/// 数字 `3` 也**不**接受。这是 ADR-0024 决策 4 的正面表述：一个参数两种解释，
/// 猜错只表现为「打中了另一条会话」——比直接报错贵得多。
pub fn required_session(params: &Value) -> Result<String, SurfaceError> {
    let name = required_string(params, "session")?;
    if name.is_empty() {
        return Err(SurfaceError::invalid_params(
            "参数 session 不能是空串（会话名，见 session_list）",
        ));
    }
    Ok(name)
}

/// 取必填的字符串参数。
pub fn required_string(params: &Value, key: &str) -> Result<String, SurfaceError> {
    match params.get(key) {
        None | Some(Value::Null) => Err(SurfaceError::invalid_params(format!(
            "缺少必填参数 {key}（字符串）"
        ))),
        Some(Value::String(text)) => Ok(text.clone()),
        Some(value) => Err(SurfaceError::invalid_params(format!(
            "参数 {key} 必须是字符串（实际：{value}）"
        ))),
    }
}

/// 取可选的字符串参数。显式 `null` 与缺失同义（都是「没给」）。
pub fn optional_string(params: &Value, key: &str) -> Result<Option<String>, SurfaceError> {
    match params.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(text)) => Ok(Some(text.clone())),
        Some(value) => Err(SurfaceError::invalid_params(format!(
            "参数 {key} 必须是字符串（实际：{value}）"
        ))),
    }
}

/// 取可选的布尔参数。显式 `null` 与缺失同义。
pub fn optional_bool(params: &Value, key: &str) -> Result<Option<bool>, SurfaceError> {
    match params.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::Bool(flag)) => Ok(Some(*flag)),
        Some(value) => Err(SurfaceError::invalid_params(format!(
            "参数 {key} 必须是布尔值（实际：{value}）"
        ))),
    }
}

/// 取可选的非负整数参数。
pub fn optional_u64(params: &Value, key: &str) -> Result<Option<u64>, SurfaceError> {
    match params.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(value) => value.as_u64().map(Some).ok_or_else(|| {
            SurfaceError::invalid_params(format!("参数 {key} 必须是非负整数（实际：{value}）"))
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// 只认一个动词的最小实现：用来钉住「内建 describe 由契约收口」。
    struct Probe;

    impl Surface for Probe {
        fn call(&self, method: &str, _params: &Value) -> Result<Value, SurfaceError> {
            if method == "session_list" {
                Ok(json!({"sessions": []}))
            } else {
                Err(SurfaceError::method_not_found(format!("未知方法 {method}")))
            }
        }
    }

    #[test]
    fn invoke_owns_describe_and_delegates_everything_else() {
        let probe = Probe;
        // describe 由契约回答，不落到后端；后端自己的 match 里即使也认它，形状相同。
        assert_eq!(probe.invoke("describe", &json!({})).expect("describe"), describe_value());
        assert_eq!(
            probe.invoke("session_list", &json!({})).expect("delegate"),
            json!({"sessions": []})
        );
        assert_eq!(
            probe.invoke("no_such", &json!({})).unwrap_err().kind,
            crate::surface_error::ErrorKind::MethodNotFound
        );
    }

    #[test]
    fn describe_lists_exactly_the_remote_surface_table() {
        let value = describe_value();
        let methods = value["methods"].as_array().expect("methods 是数组");
        let expected: Vec<&MethodSpec> = rpc_specs().collect();
        assert_eq!(methods.len(), expected.len());
        for (entry, spec) in methods.iter().zip(expected.iter()) {
            assert_eq!(entry["name"], spec.name);
            assert_eq!(entry["summary"], spec.summary);
            assert_eq!(entry["kind"], json!(spec.kind));
            // schema 在 describe 里是**对象**（不是字符串）：客户端不必二次解析。
            assert!(
                entry["params_schema"].is_object(),
                "{} 的 params_schema 应是对象",
                spec.name
            );
            assert!(entry["result_schema"].is_object());
        }
    }

    #[test]
    fn describe_hides_script_only_verbs() {
        // ADR-0023：脚本专用动词不得出现在远程动词表里——出现在这里等于
        // 告诉客户端「可以调」，而 `DeskSurface::call` 会报 method_not_found。
        let value = describe_value();
        let names: Vec<&str> = value["methods"]
            .as_array()
            .expect("methods 数组")
            .iter()
            .map(|m| m["name"].as_str().expect("name"))
            .collect();
        for hidden in ["cursor_pos", "terminal_size"] {
            assert!(!names.contains(&hidden), "{hidden} 不该出现在远程 describe 里");
        }
        assert!(names.contains(&"read_until"), "流式动词应当对远程可见");
    }

    #[test]
    fn describe_is_stable_across_calls() {
        // 描述层是常量数据：两次答案必须一模一样（不允许夹带运行时状态）。
        assert_eq!(describe_value(), describe_value());
    }

    #[test]
    fn a_missing_session_is_invalid_params_with_an_actionable_message() {
        let err = required_session(&json!({})).expect_err("缺 session 必须报错");
        assert_eq!(err.kind, crate::surface_error::ErrorKind::InvalidParams);
        assert!(
            err.message.contains("session"),
            "文案要点名参数：{}",
            err.message
        );
    }

    #[test]
    fn a_session_that_is_not_a_non_empty_string_is_rejected() {
        // 数字 id 是 ADR-0024 决策 4 明确不留的逃生门：`{"session": 3}` 必须报错，
        // 否则「一个参数两种解释」会把打错目标变成一次静默。
        for bad in [json!(""), json!(3), json!(3.5), json!(-1), json!(true), json!([3])] {
            let err = required_session(&json!({ "session": bad }))
                .expect_err("类型/取值不对必须报错");
            assert_eq!(
                err.kind,
                crate::surface_error::ErrorKind::InvalidParams,
                "{bad} 应报 invalid_params"
            );
        }
        assert_eq!(
            required_session(&json!({"session": "prod-db"})).expect("名字可以"),
            "prod-db"
        );
    }

    #[test]
    fn required_and_optional_strings_behave_differently() {
        let missing = required_string(&json!({}), "pattern").expect_err("必填缺失要报错");
        assert_eq!(missing.kind, crate::surface_error::ErrorKind::InvalidParams);
        let wrong = required_string(&json!({"pattern": 7}), "pattern").expect_err("类型错要报错");
        assert!(wrong.message.contains("pattern"));
        assert_eq!(
            required_string(&json!({"pattern": "Password:"}), "pattern").expect("字符串"),
            "Password:"
        );

        assert_eq!(optional_string(&json!({}), "command").expect("可选缺失"), None);
        assert_eq!(optional_string(&json!({"command": null}), "command").expect("null 视同缺失"), None);
        assert_eq!(
            optional_string(&json!({"command": "ls"}), "command").expect("给了就有"),
            Some("ls".to_string())
        );
        assert!(
            optional_string(&json!({"command": 7}), "command").is_err(),
            "可选不等于可以乱来"
        );
    }

    #[test]
    fn optional_numbers_reject_floats_and_negatives() {
        assert_eq!(optional_u64(&json!({}), "timeout_ms").expect("缺失可以"), None);
        assert_eq!(
            optional_u64(&json!({"timeout_ms": 1500}), "timeout_ms").expect("整数可以"),
            Some(1500)
        );
        for bad in [json!(1.5), json!(-5), json!("1500")] {
            assert!(
                optional_u64(&json!({ "timeout_ms": bad }), "timeout_ms").is_err(),
                "{bad} 应被拒绝"
            );
        }
    }
}

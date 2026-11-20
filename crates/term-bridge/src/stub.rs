//! 外部 Python 客户端的**生成式桩**（ADR-0019 S-c 的验收形状：外部 Python 脚本
//! 完成同一套登录 + 取输出）。
//!
//! 为什么不手写一份 `bridge_client.py` 放进仓库：手写的那份会和 `METHODS` 表
//! 各自演化，漂移的表现是「客户端调了桥接不认识的方法」或者「少传一个参数」，
//! 而两边都不报错、只是行为不对。这里的做法是**把表当源码**——桩里的每个
//! 方法名、参数名、必填/可选顺序、说明文案，全部从 `term_core::surface::METHODS`
//! 生成。表改了，桩自动跟着改；桩改不动，单测立刻红（见本模块的 `tests`）。
//!
//! 纪律：`python_stub()` 是**纯字符串拼接**，零文件 IO、不写磁盘、不读环境变量。
//! 写到哪里、要不要写，是调用方的决定（测试写临时目录，将来 CLI 可以打到
//! `--emit-stub` 指定的位置）。
//!
//! 生成的客户端只用标准库（`json` / `sys`）：没有第三方依赖，`python3` 一行
//! 就能跑起来——这正是「外部脚本」这个验收口径的意义。

use serde::Serialize;
use serde_json::Value;

use term_core::surface::{rpc_specs, MethodSpec};

/// 生成一份完整的 Python 客户端源码。
///
/// 结构：模块文档 → `BridgeError` → `METHODS`（表的镜像，供客户端本地校验）
/// → `BridgeClient`（`call` + 每个方法一行 wrapper）。
pub fn python_stub() -> String {
    // 桩只镜像**远程可见**的动词（ADR-0023）：把脚本专用动词写进 Python 客户端
    // 等于给它一个必定 method_not_found 的方法。
    let specs: Vec<&MethodSpec> = rpc_specs().collect();
    let mut out = String::with_capacity(8 * 1024);
    out.push_str(PROLOGUE);
    out.push_str("\n\nMETHODS = [\n");
    for spec in &specs {
        out.push_str(&methods_entry(spec));
    }
    out.push_str("]\n\n");
    out.push_str(CLIENT_HEAD);
    for spec in &specs {
        out.push_str(&client_method(spec));
    }
    out
}

/// 生成的桩的模块头：只用标准库，说明用法与错误形状。
const PROLOGUE: &str = r#""""由 term-bridge::python_stub() 生成，不要手改。

协议的单一真相是 crates/term-core/src/surface.rs 里的 METHODS 表；
本文件是它的镜像，由 crates/term-bridge 的单测钉住「签名与 schema 逐字一致」。
"""

import json
import sys


JSONRPC_VERSION = "2.0"


class BridgeError(Exception):
    """桥接返回的失败三元组 {code, message, kind}（ADR-0019 §B.11）。

    按 ``kind`` 分支，不要 match ``message``：文案会改，kind 不会。
    常见 kind：invalid_params / timeout / closed / not_found / locked / internal。
    """

    def __init__(self, code, message, kind):
        super().__init__("[%s:%s] %s" % (kind, code, message))
        self.code = code
        self.message = message
        self.kind = kind
"#;

/// `BridgeClient` 的固定部分：传输、请求 id、错误解包。
const CLIENT_HEAD: &str = r#"class BridgeClient:
    """一行一个 JSON 的同步桥接客户端。

    默认读写自己的 stdin / stdout（桥接进程挂在这条管道的另一端）；
    测试或嵌入场景可以传入自己的 reader / writer 文件对象。

    刻意不并发：一条 stdio 管道是一条串行的事实流，同时压两个请求进去，
    "这个响应是谁的" 就只能靠猜。要并发请开两个进程。
    """

    def __init__(self, reader=None, writer=None):
        self._in = reader if reader is not None else sys.stdin
        self._out = writer if writer is not None else sys.stdout
        self._next_id = 0

    def call(self, method, params=None):
        """发一个请求并等它的响应。

        `error` 一律抛 :class:`BridgeError`，绝不返回 None 冒充结果——
        "调用失败" 与 "结果就是空" 必须是两件事。
        """
        self._next_id += 1
        request_id = self._next_id
        request = {"jsonrpc": JSONRPC_VERSION, "id": request_id, "method": method}
        if params is not None:
            request["params"] = params
        self._out.write(json.dumps(request, ensure_ascii=False) + "\n")
        self._out.flush()
        while True:
            line = self._in.readline()
            if not line:
                raise BridgeError(
                    -32603,
                    "桥接侧已关闭管道（EOF），没有收到 %s 的响应" % method,
                    "internal",
                )
            line = line.strip()
            if not line:
                continue
            message = json.loads(line)
            if not isinstance(message, dict):
                continue
            if message.get("id") is None and "error" in message:
                # id 为 null 的错误说的是"这行请求本身坏了"。我们只发合法 JSON，
                # 所以出现它一定是桥接或装配出了问题——报出来，别静默跳过。
                error = message["error"]
                raise BridgeError(
                    error.get("code", -32603),
                    error.get("message", ""),
                    error.get("kind", "internal"),
                )
            if message.get("id") != request_id:
                # 不是我的响应（理论上不会出现，出现就是协议错位）——跳过并继续等。
                continue
            if "error" in message:
                error = message["error"]
                raise BridgeError(
                    error.get("code", -32603),
                    error.get("message", ""),
                    error.get("kind", "internal"),
                )
            return message.get("result")
"#;

/// 一个 `METHODS` 条目：与 Rust 侧逐字段对应，schema 用 `json.loads` 还原成字典。
fn methods_entry(spec: &MethodSpec) -> String {
    format!(
        "    {{\n        \"name\": {name},\n        \"summary\": {summary},\n        \
         \"kind\": {kind},\n        \"params_schema\": json.loads({params}),\n        \
         \"result_schema\": json.loads({result}),\n    }},\n",
        name = json_literal(&spec.name),
        summary = json_literal(&spec.summary),
        kind = json_literal(&spec.kind),
        params = json_literal(&spec.params_schema),
        result = json_literal(&spec.result_schema),
    )
}

/// 一个客户端方法：签名由 schema 的 `required`（位置参数）与其余
/// `properties`（`=None` 关键字参数）决定。
fn client_method(spec: &MethodSpec) -> String {
    let (required, optional) = schema_params(spec);
    let mut signature: Vec<String> = vec!["self".to_string()];
    signature.extend(required.iter().map(|key| py_name(key)));
    for key in &optional {
        signature.push(format!("{}=None", py_name(key)));
    }

    let mut out = String::new();
    out.push_str(&format!(
        "\n    def {}({}):\n        {}\n",
        py_name(spec.name),
        signature.join(", "),
        json_literal(&spec.summary)
    ));
    if required.is_empty() && optional.is_empty() {
        out.push_str(&format!(
            "        return self.call({}, {{}})\n",
            json_literal(&spec.name)
        ));
        return out;
    }
    if required.is_empty() {
        out.push_str("        params = {}\n");
    } else {
        let pairs: Vec<String> = required
            .iter()
            .map(|key| format!("{}: {}", json_literal(key), py_name(key)))
            .collect();
        out.push_str(&format!("        params = {{{}}}\n", pairs.join(", ")));
    }
    for key in &optional {
        out.push_str(&format!(
            "        if {} is not None:\n            params[{}] = {}\n",
            py_name(key),
            json_literal(key),
            py_name(key)
        ));
    }
    out.push_str(&format!(
        "        return self.call({}, params)\n",
        json_literal(&spec.name)
    ));
    out
}

/// 从 `params_schema` 取 `(required, optional)`：前者保持 schema 里的顺序，
/// 后者按 `properties` 的键序（`serde_json` 默认有序表，故稳定）。
fn schema_params(spec: &MethodSpec) -> (Vec<String>, Vec<String>) {
    let schema: Value = serde_json::from_str(spec.params_schema).unwrap_or_else(|err| {
        panic!(
            "{} 的 params_schema 不是合法 JSON：{err}（term-core 的单测本应先挡住）",
            spec.name
        )
    });
    let required: Vec<String> = schema
        .get("required")
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(|item| item.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default();
    let mut optional: Vec<String> = Vec::new();
    if let Some(properties) = schema.get("properties").and_then(Value::as_object) {
        for key in properties.keys() {
            if !required.iter().any(|needed| needed == key) {
                optional.push(key.clone());
            }
        }
    }
    (required, optional)
}

/// JSON 字面量同时也是合法的 Python 字面量（字符串/数字/布尔/null 的写法一致，
/// 且 `serde_json` 会转义引号与反斜杠）——所以一个函数就能喂两边。
fn json_literal<T: Serialize + ?Sized>(value: &T) -> String {
    serde_json::to_string(value).expect("常量数据的 JSON 字面量序列化不会失败")
}

/// Python 标识符归一：schema 里的键都是 snake_case，但表是手写的常量，
/// 所以这里挡住「将来有人加了个 `timeout-ms`」把生成结果变成语法错误。
fn py_name(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    for (index, ch) in raw.chars().enumerate() {
        if ch.is_ascii_alphanumeric() || ch == '_' {
            if index == 0 && ch.is_ascii_digit() {
                out.push('_');
            }
            out.push(ch);
        } else {
            out.push('_');
        }
    }
    if out.is_empty() {
        out.push_str("_unnamed");
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_stub_declares_a_client_an_error_and_the_full_table() {
        let stub = python_stub();
        assert!(stub.contains("class BridgeClient:"), "缺客户端类");
        assert!(stub.contains("class BridgeError(Exception):"), "缺错误类型");
        assert!(stub.contains("import json"), "缺标准库导入");
        assert!(
            stub.contains("METHODS = ["),
            "缺表的镜像（客户端要能本地校验自己的能力面）"
        );
        // 条目数按 summary 计——schema 里的同名字符串是转义过的，不会误命中。
        assert_eq!(
            stub.matches("\"summary\": ").count(),
            rpc_specs().count(),
            "METHODS 镜像的条目数与表不一致"
        );
    }

    #[test]
    fn every_method_signature_matches_its_schema() {
        // 这是防漂移的主钉：签名必须能由 schema 逐字推出来。
        let stub = python_stub();
        for spec in rpc_specs() {
            let (required, optional) = schema_params(spec);
            let mut signature: Vec<String> = vec!["self".to_string()];
            signature.extend(required.iter().map(|key| py_name(key)));
            for key in &optional {
                signature.push(format!("{}=None", py_name(key)));
            }
            let expected = format!("def {}({}):", py_name(spec.name), signature.join(", "));
            assert!(
                stub.contains(&expected),
                "生成的桩缺少签名 {expected}（方法 {}）",
                spec.name
            );
        }
    }

    #[test]
    fn every_parameter_name_appears_in_the_generated_body() {
        // 签名对了但函数体漏传参数，同样是漂移；两头都钉住。
        let stub = python_stub();
        for spec in rpc_specs() {
            let (required, optional) = schema_params(spec);
            for key in required.iter().chain(optional.iter()) {
                assert!(
                    stub.contains(&format!("\"{}\": ", key)) || stub.contains(&format!("params[\"{}\"] = ", key)),
                    "方法 {} 的参数 {key} 没有出现在生成的函数体里",
                    spec.name
                );
            }
        }
    }

    #[test]
    fn every_method_name_is_callable_from_the_client() {
        let stub = python_stub();
        for name in term_core::surface::names() {
            assert!(
                stub.contains(&format!("def {}(", name)),
                "客户端缺方法 {name}"
            );
            assert!(
                stub.contains(&format!("self.call(\"{}\"", name)),
                "方法 {name} 没有真的走 call()"
            );
        }
        assert!(
            term_core::surface::names().contains(&"describe"),
            "describe 必须在能力面上（客户端靠它发现别的）"
        );
    }

    #[test]
    fn the_generated_source_has_no_obvious_python_syntax_hazards() {
        let stub = python_stub();
        // 不出现三引号嵌套（文档串一律用单行字面量）与制表符混排。
        // 四处文档串（模块 / BridgeError / BridgeClient / call），每处一对三引号。
        // 生成的方法一律用单行字面量当文档串，避免三引号与 schema 里的引号打架。
        assert_eq!(
            stub.matches("\"\"\"").count(),
            8,
            "三引号只应出现在四段固定文档串里"
        );
        assert!(!stub.contains('\t'), "生成的源码不要用制表符缩进");
        // 每个生成的方法体都以 return self.call(...) 收尾（不许空方法）。
        assert_eq!(
            stub.matches("        return self.call(").count(),
            rpc_specs().count(),
            "每个方法都应有且仅有一处 return self.call(...)"
        );
    }

    #[test]
    fn schemas_are_embedded_verbatim_so_python_sees_the_same_text() {
        let stub = python_stub();
        for spec in rpc_specs() {
            let literal = json_literal(&spec.params_schema);
            assert!(
                stub.contains(&format!("json.loads({literal})")),
                "{} 的 params_schema 没有逐字嵌入",
                spec.name
            );
        }
    }

    #[test]
    fn py_name_never_produces_an_invalid_identifier() {
        assert_eq!(py_name("timeout_ms"), "timeout_ms");
        assert_eq!(py_name("timeout-ms"), "timeout_ms");
        assert_eq!(py_name("2nd"), "_2nd");
        assert_eq!(py_name(""), "_unnamed");
        assert_eq!(py_name("a b"), "a_b");
    }

    #[test]
    fn generating_is_deterministic() {
        // 桩是纯函数：同一份表生成两次必须逐字节相同（否则每次生成都产生 diff）。
        assert_eq!(python_stub(), python_stub());
    }
}

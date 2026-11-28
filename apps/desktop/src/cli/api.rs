//! `term-desk api` —— 前台控制面的薄客户端（ADR-0030 §决策 4）。
//!
//! 只发 `describe` / `call` 两种请求，别的都不做：**不建会话池、不装配
//! HostEnv、没有能力面的 auth 方法**。前台没起来就明确报错退出，绝不退化成
//! 「客户端自己起一个后端」——那会让「远程操作窗口里的会话」变成
//! 「操作另一个进程里的另一批会话」，而且静默。
//!
//! 失败三元组不改一字：`{code, message, kind}` 按 `bridge::client` 的映射
//! 打到 stderr（`[kind:code] message`），退出码 1。
//!
//! 端点口径：连 `127.0.0.1:<port>`（默认 47653，`WINDTERM_CONTROL_PORT`
//! 可覆盖），key 文件 `~/.zterm/control-key.local-<port>`。连接后的
//! `auth.challenge` 是传输外壳，不是能力面动词。

use serde_json::{json, Value};

use crate::bridge::client::Client;

/// 端点描述由 bridge 统一生成：端口 / key 文件 / 挑战证明一并说明，
/// CLI 侧不硬编码端口，也不再提 socket 路径或命名管道。
fn usage() -> String {
    format!(
        "\
term-desk api describe
term-desk api call <method> ['{{\"json\":\"params\"}}']

  连前台窗口的 {} 发一条 JSON-RPC 请求。前台未运行时
  明确报错退出；本命令绝不自建会话池。没有 auth 子命令：准入由 loopback
  对端校验 + 0600 key 文件 + HMAC-SHA256 挑战证明完成
  （Unix 0600 / Windows 用户目录 ACL），这三帧不进 describe。
",
        crate::bridge::endpoint_description()
    )
}

#[derive(Debug, PartialEq)]
pub enum Command {
    Help,
    Describe,
    Call { method: String, params: Value },
}

/// 参数解析是纯函数：能在不连端点的情况下钉住用法。
pub fn parse(rest: &[String]) -> Result<Command, String> {
    let Some(first) = rest.first() else {
        return Err("api 后面需要 describe 或 call".to_string());
    };
    match first.as_str() {
        "--help" | "-h" | "help" => Ok(Command::Help),
        "describe" => {
            if rest.len() > 1 {
                return Err("describe 不接受参数".to_string());
            }
            Ok(Command::Describe)
        }
        "call" => {
            let method = rest
                .get(1)
                .ok_or_else(|| "call 后面需要方法名（可用 describe 查）".to_string())?
                .clone();
            if method.is_empty() {
                return Err("方法名不能是空串".to_string());
            }
            let params = match rest.get(2) {
                None => json!({}),
                Some(text) => serde_json::from_str(text)
                    .map_err(|err| format!("params 不是合法 JSON：{err}"))?,
            };
            if !params.is_object() {
                return Err("params 必须是 JSON 对象（按名传参）".to_string());
            }
            if rest.len() > 3 {
                return Err("call 只接受 <method> [params]".to_string());
            }
            Ok(Command::Call { method, params })
        }
        other => Err(format!("不认识的 api 动作：{other}（只有 describe / call）")),
    }
}

/// 连前台、发一条、按原样打印结果；返回退出码。
pub fn run(rest: &[String]) -> i32 {
    let command = match parse(rest) {
        Ok(command) => command,
        Err(msg) => {
            eprintln!("{msg}\n\n{}", usage());
            return 2;
        }
    };
    if command == Command::Help {
        print!("{}", usage());
        return 0;
    }
    let mut client = match Client::connect_default() {
        Ok(client) => client,
        Err(err) => {
            eprintln!("{err}");
            return 1;
        }
    };
    let outcome = match command {
        Command::Describe => client.call("describe", &json!({})),
        Command::Call { method, params } => client.call(&method, &params),
        // Help 已在上面打印并返回；到这里只可能是调用方改了 parse 却漏改这里。
        // 不 panic：显式当作成功退出，并且不碰端点。
        Command::Help => return 0,
    };
    match outcome {
        Ok(value) => {
            let text = serde_json::to_string_pretty(&value).unwrap_or_else(|_| value.to_string());
            println!("{text}");
            0
        }
        Err(err) => {
            eprintln!("{err}");
            1
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn argv(rest: &[&str]) -> Vec<String> {
        rest.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn describe_and_call_are_the_only_operations() {
        assert_eq!(parse(&argv(&["describe"])), Ok(Command::Describe));
        assert_eq!(
            parse(&argv(&["call", "session_list", "{\"session\":\"x\"}"])),
            Ok(Command::Call {
                method: "session_list".to_string(),
                params: json!({"session": "x"}),
            })
        );
        assert!(parse(&argv(&["auth", "x"])).is_err(), "auth 不是 API 动词");
    }

    #[test]
    fn omitted_params_are_an_empty_object_and_arrays_are_refused() {
        assert_eq!(
            parse(&argv(&["call", "session_list"])),
            Ok(Command::Call {
                method: "session_list".to_string(),
                params: json!({}),
            })
        );
        assert!(parse(&argv(&["call", "write", "[1,2]"])).is_err());
        assert!(parse(&argv(&["call", "write", "{bad"])).is_err());
    }

    #[test]
    fn help_never_touches_the_endpoint() {
        let text = usage();
        assert!(text.contains(&format!("127.0.0.1:{}", crate::bridge::control::control_port())),
            "用法必须点名端口：{text}");
        assert!(text.contains("control-key.local"), "用法必须点名 key 文件：{text}");
        assert!(text.contains("WINDTERM_CONTROL_PORT"), "用法必须说明端口覆盖：{text}");
        assert!(!text.contains("control.sock"), "不再提 socket 路径：{text}");
        assert!(!text.contains("命名管道"), "不再提命名管道：{text}");
        assert_eq!(run(&argv(&["--help"])), 0);
    }
}

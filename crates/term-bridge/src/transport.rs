//! 传输层：一行一个 JSON 的 stdio 管道。
//!
//! 承接点只有一个函数 [`serve_stdio`]，它做的事只有三件：读一行、交给
//! [`crate::jsonrpc::dispatch`]、该回就回一行并 flush。**协议规则一条都不在
//! 这里**——这样「为什么这个输入得到这个答案」永远只需要读 `jsonrpc.rs`。
//!
//! # 本刀的范围（刻意不开的口子）
//!
//! - **不开 TCP / WebSocket**。stdio 已经能证明「外部进程用同一套能力面办事」
//!   这个 S-c 的验收口径；网络监听会立刻带来鉴权、绑定地址、跨用户访问三件
//!   事，而那属于 §B.9 的 token / 锁屏门，是下一刀。这里的 dissymmetry 是
//!   记在账上的，不是忘了。
//! - **不做 token / 不做锁屏拒绝**。`ErrorKind::Locked` 的分类已经占位
//!   （`-32003`），加门的时候只改判定，不改线上形状。
//! - **不并发**。一条管道 = 一条串行的事实流。并发要靠多进程，不靠线程池。
//!
//! # 健壮性口径
//!
//! - 非法行回一个 `id: null` 的 ParseError，**不中断**：stdio 上写坏一行
//!   不该把整条管道打死（那会让「脚本里少个逗号」变成「桥接挂了」）。
//! - 读到 EOF 正常返回 `Ok(())`：EOF 是客户端关掉了管道，不是错误。
//! - 只有**写**失败才返回 `Err`：那时候对端已经听不见了，再循环下去毫无意义。

use std::io::{self, BufRead, Write};

use crate::handler::Surface;
use crate::jsonrpc::dispatch;

/// 在 stdio 上服务一个 [`Surface`]，直到 `reader` 返回 EOF。
///
/// `reader` / `writer` 是泛型：生产用 `stdin().lock()` / `stdout()`，
/// 测试用 `Cursor` 与 `Vec<u8>`，端到端测试用子进程的管道——同一份代码，
/// 三种挂法，不必为测试在生产路径上留 if。
///
/// 回包严格是**一行一个 JSON**（`\n` 结尾，写完立即 flush）：不加分隔符、
/// 不整形、不缩进。客户端可以放心用 `readline()`。
pub fn serve_stdio<R: BufRead, W: Write>(
    surface: &dyn Surface,
    mut reader: R,
    writer: &mut W,
) -> io::Result<()> {
    let mut line = String::new();
    loop {
        line.clear();
        let read = reader.read_line(&mut line)?;
        if read == 0 {
            // EOF：客户端挂了管道。正常收场，不是错误。
            return Ok(());
        }
        // 行尾归一：`\n` 与 `\r\n` 都剥掉，管道里不该留 CR。
        let text = line.trim_end_matches('\n').trim_end_matches('\r');
        if let Some(reply) = dispatch(surface, text) {
            writer.write_all(reply.as_bytes())?;
            writer.write_all(b"\n")?;
            writer.flush()?;
        }
        // `None` = 通知 / 全是通知的批量：一个字节都不回（协议规定）。
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::SurfaceError;
    use serde_json::{json, Value};
    use std::io::Cursor;

    struct Echo;

    impl Surface for Echo {
        fn call(&self, method: &str, _params: &Value) -> Result<Value, SurfaceError> {
            match method {
                "session_list" => Ok(json!({"sessions": []})),
                "boom" => Err(SurfaceError::internal("后端起不来了")),
                other => Err(SurfaceError::method_not_found(format!("未知方法 {other}"))),
            }
        }
    }

    fn serve(input: &str) -> String {
        let mut out = Vec::new();
        let mut reader = Cursor::new(input.as_bytes().to_vec());
        serve_stdio(&Echo, &mut reader, &mut out).expect("stdio 服务不该失败");
        String::from_utf8(out).expect("回包是 UTF-8")
    }

    #[test]
    fn each_request_gets_exactly_one_line() {
        let out = serve(
            "{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"session_list\"}\n\
             {\"jsonrpc\":\"2.0\",\"id\":2,\"method\":\"session_list\"}\n",
        );
        let lines: Vec<&str> = out.lines().collect();
        assert_eq!(lines.len(), 2, "一条请求一行回包，实际：{out}");
        for (index, line) in lines.iter().enumerate() {
            let value: Value = serde_json::from_str(line).expect("每行都是合法 JSON");
            assert_eq!(value["id"], index + 1);
            assert_eq!(value["result"], json!({"sessions": []}));
        }
        assert!(out.ends_with('\n'), "最后一行也要换行结尾");
    }

    #[test]
    fn a_bad_line_does_not_kill_the_pipe() {
        // 中间夹一行写坏的：前后两条正常请求仍然要回包。
        let out = serve(
            "{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"session_list\"}\n\
             {oops\n\
             {\"jsonrpc\":\"2.0\",\"id\":2,\"method\":\"session_list\"}\n",
        );
        let lines: Vec<&str> = out.lines().collect();
        assert_eq!(lines.len(), 3, "坏行也要回一条 ParseError，实际：{out}");
        let bad: Value = serde_json::from_str(lines[1]).expect("合法 JSON");
        assert_eq!(bad["error"]["kind"], "parse_error");
        assert!(bad["id"].is_null());
        let last: Value = serde_json::from_str(lines[2]).expect("合法 JSON");
        assert_eq!(last["id"], 2);
        assert!(last.get("result").is_some(), "坏行之后管道仍能用");
    }

    #[test]
    fn crlf_and_blank_lines_are_handled_without_panicking() {
        // CRLF 来自 Windows 侧管道；空行是脚本拼接时最常见的噪声。
        // 两者都不能让服务崩掉，空行按 ParseError 如实回报。
        let out = serve("{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"session_list\"}\r\n\r\n");
        let lines: Vec<&str> = out.lines().collect();
        assert_eq!(lines.len(), 2, "实际：{out}");
        let first: Value = serde_json::from_str(lines[0]).expect("合法 JSON");
        assert_eq!(first["id"], 1, "CRLF 必须被剥掉，否则 JSON 末尾多一个 CR");
        let second: Value = serde_json::from_str(lines[1]).expect("合法 JSON");
        assert_eq!(second["error"]["kind"], "parse_error");
    }

    #[test]
    fn notifications_produce_no_output_at_all() {
        let out = serve("{\"jsonrpc\":\"2.0\",\"method\":\"session_list\"}\n");
        assert_eq!(out, "", "通知不回包，一个字节都不该出现：{out}");
    }

    #[test]
    fn eof_ends_the_loop_cleanly() {
        let mut out = Vec::new();
        let mut reader = Cursor::new(Vec::new());
        serve_stdio(&Echo, &mut reader, &mut out).expect("空输入 = 立刻 EOF");
        assert!(out.is_empty());
    }

    #[test]
    fn an_error_reply_carries_the_kind_over_the_wire() {
        let out = serve("{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"boom\"}\n");
        let value: Value = serde_json::from_str(out.lines().next().expect("有回包")).expect("合法 JSON");
        assert_eq!(value["error"]["code"], -32603);
        assert_eq!(value["error"]["kind"], "internal");
        assert_eq!(value["error"]["message"], "后端起不来了");
    }

    #[test]
    fn a_failing_writer_surfaces_as_an_error_instead_of_looping() {
        struct Broken;
        impl Write for Broken {
            fn write(&mut self, _buf: &[u8]) -> io::Result<usize> {
                Err(io::Error::new(io::ErrorKind::BrokenPipe, "对端已关"))
            }
            fn flush(&mut self) -> io::Result<()> {
                Ok(())
            }
        }
        let mut reader = Cursor::new(
            b"{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"session_list\"}\n".to_vec(),
        );
        let err = serve_stdio(&Echo, &mut reader, &mut Broken).expect_err("写失败要上报");
        assert_eq!(err.kind(), io::ErrorKind::BrokenPipe);
    }
}

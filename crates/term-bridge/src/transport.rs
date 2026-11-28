//! 传输层：一行一个 JSON 的 stdio 管道。
//!
//! 承接点只有一个函数 [`serve_stdio`]，它做的事只有三件：读一行、交给
//! [`crate::jsonrpc::dispatch`]、该回就回一行并 flush。**协议规则一条都不在
//! 这里**——这样「为什么这个输入得到这个答案」永远只需要读 `jsonrpc.rs`。
//!
//! # 本模块的范围：只做 stdio
//!
//! - **不开 TCP / WebSocket**。stdio 已经能证明「外部进程用同一套能力面办事」
//!   这个 S-c 的验收口径。回环 TCP 与 token / 锁屏门（§B.9）由应用层
//!   `desk serve` 的 `bridge` shell 提供（ADR-0022）：它把同一条管道接到
//!   socket 上，并补上网络监听带来的鉴权、绑定地址、跨用户访问三件事；
//!   本模块只负责管道本身，不重复那层。
//! - **不做 token / 不做锁屏拒绝**。判定在应用层外壳；`ErrorKind::Locked` 的分类
//!   （`-32003`）在本模块仍然占位，线上形状不变。
//! - **不并发**。一条管道 = 一条串行的事实流。并发要靠多进程，不靠线程池。
//!
//! # 健壮性口径
//!
//! - 非法行回一个 `id: null` 的 ParseError，**不中断**：stdio 上写坏一行
//!   不该把整条管道打死（那会让「脚本里少个逗号」变成「桥接挂了」）。
//! - 读到 EOF 正常返回 `Ok(())`：EOF 是客户端关掉了管道，不是错误。
//! - 只有**写**失败才返回 `Err`：那时候对端已经听不见了，再循环下去毫无意义。

use std::io::{self, BufRead, Read, Write};

use term_core::contract::Surface;

use crate::jsonrpc::dispatch;

/// 单行上限。JSON-RPC 的一条请求不该有 1 MiB——真到了，不是脚本把整个文件
/// 塞进 `write` 的参数，就是对面在拿「永不换行的字节流」撑内存。
///
/// 有上限之后超长行的处理方式是**丢弃到行尾 + 回一个 ParseError**，而不是
/// 断开管道：stdio 那一侧的调用方可能是编辑器插件，断一次就再也不重连了。
const MAX_LINE_BYTES: u64 = 1024 * 1024;

/// 丢掉当前行剩下的字节（只消费、不累积），用于超长行之后的重新对齐。
fn skip_to_newline<R: BufRead>(reader: &mut R) -> io::Result<()> {
    loop {
        let buf = reader.fill_buf()?;
        if buf.is_empty() {
            return Ok(());
        }
        match buf.iter().position(|&b| b == b'\n') {
            Some(at) => {
                reader.consume(at + 1);
                return Ok(());
            }
            None => {
                let len = buf.len();
                reader.consume(len);
            }
        }
    }
}

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
        // 带硬上限地读一行：`take` 让 `read_line` 最多只吃到 `MAX_LINE_BYTES`，
        // 剩下的字节留在 reader 里（由 `skip_to_newline` 丢掉），
        // 于是「对面一直不发换行」最多占 1 MiB，而不是一路涨到 OOM。
        let read = {
            let mut limited = (&mut reader).take(MAX_LINE_BYTES);
            limited.read_line(&mut line)?
        };
        if read == 0 {
            // EOF：客户端挂了管道。正常收场，不是错误。
            return Ok(());
        }
        // 没吃到换行又正好顶到上限 = 超长行。丢掉剩余部分，回一个解析错误，
        // 管道继续服务下一条（不因为对面写坏一行就把整个桥接打死）。
        let truncated = !line.ends_with('\n') && read as u64 == MAX_LINE_BYTES;
        if truncated {
            skip_to_newline(&mut reader)?;
            // 空串一定解析失败 → ParseError（`id: null`），这正是我们想要的口径。
            if let Some(reply) = dispatch(surface, "") {
                writer.write_all(reply.as_bytes())?;
                writer.write_all(b"\n")?;
                writer.flush()?;
            }
            continue;
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

    /// 超长行（无换行）必须被截断丢弃，而不是无界吃内存；且管道要能继续服务。
    ///
    /// 这是 stdio 侧的对端可控路径：写一个永不换行的字节流就能把
    /// `read_line` 撑到 OOM。上限生效后，最多占 `MAX_LINE_BYTES`，
    /// 回一条 ParseError，后面的正常请求照常处理。
    #[test]
    fn an_oversized_line_is_dropped_and_the_pipe_keeps_serving() {
        let mut input: Vec<u8> = vec![b'x'; MAX_LINE_BYTES as usize + 64];
        input.push(b'\n');
        input.extend_from_slice(
            b"{\"jsonrpc\":\"2.0\",\"id\":9,\"method\":\"session_list\"}\n",
        );

        let mut out = Vec::new();
        serve_stdio(&Echo, Cursor::new(input), &mut out).expect("管道应正常收场");
        let text = String::from_utf8(out).expect("回包是 UTF-8");
        let lines: Vec<&str> = text.lines().collect();

        assert_eq!(lines.len(), 2, "超长行一条错误 + 后续一条正常，实际：{text}");
        let bad: Value = serde_json::from_str(lines[0]).expect("合法 JSON");
        assert_eq!(bad["error"]["kind"], "parse_error");
        assert!(bad["id"].is_null(), "超长行的 id 无法解析，只能是 null");
        let last: Value = serde_json::from_str(lines[1]).expect("合法 JSON");
        assert_eq!(last["id"], 9, "超长行之后的请求必须照常处理");
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

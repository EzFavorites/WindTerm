//! `term-bridge` — 会话能力面的 **JSON-RPC 2.0 over stdio** 桥接（ADR-0019 S-c）。
//!
//! 存在理由一句话：让**外部**进程（第一刀是 Python 脚本）能用与 UI、脚本引擎
//! 完全相同的一套动作操作会话，而不必链接本仓库的任何 Rust 代码。
//!
//! 分层（每一层只做一件事，规则不重不漏）：
//!
//! - `error`：`term_core::surface_error` 的再导出（失败分类与 `{code, message, kind}`）。
//! - `jsonrpc`：请求/响应/通知的编解码与分派（**协议规则唯一所在地**）。
//! - `handler`：`term_core::contract` 的再导出（[`handler::Surface`] 接缝、
//!   `describe` 生成与参数助手）——契约定义在 `term-core`，这里只留路径兼容。
//! - `transport`：一行一个 JSON 的 stdio 管道，非法行不中断。
//! - `stub`：由同一张表生成的 Python 客户端源码（外部脚本的落地形状）。
//!
//! # 本 crate 的边界：只做 stdio
//!
//! - 不开 TCP / WebSocket，也不开本地控制端点：stdio 已经能证明「外部进程
//!   用同一套能力面办事」这个 S-c 口径。前台 TCP loopback 控制端点（默认
//!   47653 / `WINDTERM_CONTROL_PORT`，0600 key 文件 + HMAC 挑战 + 锁屏门）
//!   在应用层，本 crate 不重复那层（ADR-0030）。
//! - 不做 token、不做锁屏拒绝：判定在前台应用层；`ErrorKind::Locked`
//!   （`-32003`）的线上形状在本 crate 仍然占位，判定放在应用层，协议不变。
//! - 不做并发：一条管道一条串行事实流；要并发就开两个进程。
//! - 不碰 UI：`crates/term-bridge` 不认识任何 UI 类型。
//!
//! # 依赖纪律
//!
//! 运行时只依赖 `term-core`（契约 + 描述层 + 会话抽象），外加两个纯数据 crate
//! `serde` / `serde_json`；零 tokio、零异步调用点、零 egui（ADR-0017 笼子；
//! `scripts/verify.sh` 的 4k 管 tokio 笼子，4l 用白名单逐条核对这里的运行时
//! 依赖表）。会话控制面 `term-session` 只在 dev-dependencies 里：参考实现
//! `PoolSurface` 是开发期夹具，放在 `tests/support/`——桥接本体不持有任何会话
//! 实现，这样「桥接只认得能力面」这句话才是真的。
//!
//! # jsonrpc 缺失按 2.0 处理是宽容策略，不是规范要求
//!
//! JSON-RPC 2.0 规范要求请求对象里 `jsonrpc` 成员**必须存在**且等于 `"2.0"`
//! （规范原文是 MUST）。本实现接受它缺失并按 2.0 处理，唯一理由是让手写调试行
//! （`echo '{"method":"describe","id":1}'`）不必每次补字段。这是**宽容策略**：
//! 既不是规范允许的写法，也不等于「我们实现了一个更宽松的协议」。
//!
//! 宽容只针对**成员缺省**，不针对**值错误**：`{"jsonrpc":"1.0"}` 仍然必须回
//! `-32600`（Invalid Request），绝不能因为「宽容」把值校验一起放开。这条界线
//! 落在 `jsonrpc::Request::parse` 里，并有用例钉住
//! （`a_wrong_jsonrpc_version_is_an_invalid_request_but_omission_is_tolerated`）。
//!
//! 我们生成的 Python 桩**始终**带 `"jsonrpc": "2.0"`，所以这份宽容只对人工输入
//! 生效，不会掩盖客户端 bug。
//!
//! 宽容保留至今：外部客户端走前台 TCP loopback 控制端点（ADR-0030），桥接本体仍是
//! stdio-only，所以仍未满足的收紧触发条件只剩
//! 「出现第一个非本仓库的第三方客户端之后」（缺失即 `-32600`）。届时改的只是
//! `jsonrpc.rs` 里的一处判定，线上形状（`{code, message, kind}`）一个字不动。
//!
//! # session_open 只能开本地会话
//!
//! `session_open` 的 `command` 是**本地** exec / shell 命令行（走 `/bin/sh -c`），
//! 不是「任意协议的连接请求」；不给 `command` 则起一个本地 shell。
//!
//! `session_open({"command": "ssh host"})` 在本机装了 `ssh` 时会如实跑起来，但那
//! **不是**一个 SSH 会话：`facets` 仍然是空表，没有主机键校验、没有
//! `known_hosts` 读取、没有认证状态，也没有文件传输 / 端口转发 facet。
//! **「能跑」与「是会话」是两件事**——桥接只提供前者，后者是应用层的职责。
//!
//! 真 SSH / SFTP / 隧道会话由 desk 建立通道后经
//! `SessionPool::open_with_channel` 注入（§B.14 协议无关：能力面不出现协议
//! 概念）。桥接这一层**永远**不自己造协议会话——它手上没有凭据，也不该有。
//!
//! 下一刀若有人想给 `session_open` 加 `protocol` 字段，请先回去改 ADR-0019，
//! 不要在桥接里旁路。
//!
//! # 一行用法
//!
//! ```no_run
//! use std::io::{self, BufWriter};
//! use term_bridge::handler::Surface;
//!
//! fn serve(surface: &dyn Surface) -> io::Result<()> {
//!     let stdin = io::stdin();
//!     let stdout = io::stdout();
//!     let mut out = BufWriter::new(stdout.lock());
//!     term_bridge::serve_stdio(surface, stdin.lock(), &mut out)
//! }
//! ```
//!
//! （那个 `serve` 只是形状示意：真正的 `Surface` 实现由应用层注入。）

pub mod error;
pub mod handler;
pub mod jsonrpc;
pub mod stub;
pub mod transport;

// 再导出指向 term-core 类型：`error` / `handler` 模块只为兼容既有路径而留。
pub use term_core::contract::Surface;
pub use term_core::surface_error::{ErrorKind, SurfaceError};
pub use jsonrpc::{dispatch, ErrorObject, Id, Notification, Payload, Request, Response};
pub use stub::{python_api_shim, python_stub};
pub use transport::serve_stdio;

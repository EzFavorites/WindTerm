//! 控制端点的平台实现（ADR-0030 §决策 2）。
//!
//! 两个平台向下提供**同一组符号**：`Bound` / `bind` / `serve_bound` /
//! `default_socket_path` / `endpoint_description` / `ClientStream` / `connect_client`。
//! 上层 [`super`] 与 `client` 因此完全平台无关；每个平台各自**恰好一处绑定调用**，
//! 门禁 `verify.sh` 4n 会逐平台数这个调用点。
//!
//! - unix：UDS + `0600` + peercred，行为保持不变（见 `unix` 模块）。
//! - windows：命名管道 + 只放行当前用户 SID 的 DACL + 对端 PID→SID 复核（见
//!   `windows` 模块）。管道名按 SID 隔离，不同用户根本看不到彼此的端点。
//!
//! 两条路径都没有 auth 握手、没有 token 文件、没有 TCP 入口。

#[cfg(unix)]
mod unix;
#[cfg(unix)]
pub use unix::*;

#[cfg(windows)]
mod windows;
#[cfg(windows)]
pub use windows::*;

// 其它目标（例如 wasm）没有本机进程间准入可用：显式 Unsupported，绝不退化成 TCP。
#[cfg(not(any(unix, windows)))]
mod unsupported;
#[cfg(not(any(unix, windows)))]
pub use unsupported::*;

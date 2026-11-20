//! `term-ssh` — 原生 SSH 连接层（R13-d）。
//!
//! 解决什么问题：v0.1 的所有远程会话都是「拉系统 `ssh` 客户端跑在一个 pty 里」。
//! 这在终端里能用，但**它把这个程序变成了一个终端模拟器外壳**：
//! 密码要用户在终端里敲、端口转发只能靠 `-L` 手写、会话保存不了凭据、
//! 断线重连只能重开一个窗口。这一层把 SSH 拿回进程内。
//!
//! ```text
//! SshOptions ──connect()──▶ SshChannel ──(impl TermChannel)──▶ Supervisor
//!    ▲                          │                                  │
//!    │                          └── 专用线程：Runtime::new_current_thread()
//!    └── 密码 / 私钥 / agent        russh 0.65 客户端 + known_hosts
//! ```
//!
//! 三条边界：
//! 1. **异步只活在这里**。整个仓库只有本 crate（以及将来的 `term-tunnel`）
//!    的依赖表里有 tokio；对外一律同步接口（ADR-0017 §决策 2）。
//! 2. **不碰 UI**。`check_server_key` 不弹窗，它只把拒绝理由记下来变成
//!    `SshError::HostKey`，由界面决定要不要问用户。
//! 3. **不认识 `SessionConfig`**。主机 / 身份 / 策略是本 crate 的输入，
//!    标签页颜色、滚动行数是应用层的输入，两边不互相借类型。
//!
//! 实现选型：russh + tokio、ring 后端，把「主机密钥变了」当成硬错误。
//! 一处刻意不同：
//! - **默认 `Tofu` 而不是 `Strict`**：图形终端第一次连一台机器的正常路径是
//!   「看一眼指纹再记下来」，而 `Strict` 会把第一次连接变成一次手工 `ssh-keyscan`。
//!
//! `exec` 一次性会话（W6-C3）**已经提供**，是「第二 exec 通道 / 远端 Sysmon」的
//! crate 层接缝，仍然守着边界 1：
//! - 异步接缝 [`ChannelOpener::exec`] 开裸 channel，给同 crate 内的库代码用；
//! - 同步门面 [`exec_capture`] 收全 stdout / stderr / 退出码，给 UI / CLI 用。
//!
//! 两者都不把 `tokio` / `.await` 漏到笼子外，也不碰 PTY / shell。

mod auth;
mod authenticated;
mod background;
mod channel;
mod error;
mod exec;
mod handler;
mod known_hosts;
/// `known_hosts` 的管理面（列举 / 遗忘 / 换钥）。**故意公开**：见模块文档
/// 「为什么现在还没有调用方」。
pub mod known_hosts_admin;
mod options;
mod outbound;
mod proxy;

pub use authenticated::{Authenticated, ChannelOpener, Forwarded};
pub use background::{connect_in_background, ConnectHandle};
pub use channel::{DEFAULT_CONNECT_TIMEOUT, SshChannel, connect};
pub use error::{AuthProblem, HostKeyProblem, SshError};
pub use exec::{EXEC_MAX_OUTPUT_BYTES, EXEC_TIMEOUT, ExecOutput, exec_capture};
pub use known_hosts::{Decision, fingerprint};
pub use known_hosts_admin::{HostRecord, TrustOutcome, host_token};
pub use options::{
    AlgorithmPolicy, AuthMethod, DEFAULT_KEEPALIVE, DEFAULT_PORT, DEFAULT_TERM, HostKeyPolicy,
    OutboundProxy, ProxyAuth, ProxyHop, ProxyKind, SshOptions, parse_proxy_jump,
};

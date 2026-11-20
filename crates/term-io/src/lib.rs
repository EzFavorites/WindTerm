//! `term-io` — 会话 IO 与线程层：真实设备、真实线程、真实字节。
//!
//! 2026-11-12 底层收敛从 `term-runtime` 拆出。拆分依据不是「文件现在长什么样」，
//! 而是**内部的 `crate::` 依赖图**：`channel` / `error` / `log` / `pty` /
//! `serial` / `serial_channel` / `supervisor` 互相只引用彼此（外加一条到
//! `term-paths` 的路径规则），与档案层（`term-profile`）之间一条边都没有。
//! 拆完实测：本 crate 里对 `crate::persist` / `crate::prefs` 的引用是 0 处。
//!
//! # 边界（依赖表即判据）
//!
//! | 有 | 没有 |
//! |---|---|
//! | `term-core`（纯语义）、`term-paths`（路径）、`portable-pty`、`nix` | SQLite、serde、tokio、russh、任何 UI 框架 |
//!
//! **同步**。异步只活在 `term-ssh` / `term-tunnel` / `term-sftp` 的笼子里
//! （ADR-0017 §决策 2）；这一层是「对外同步」那句话的服务端——Supervisor
//! 线程跑 `drain → vt → 快照`，调用方永远是阻塞调用加一个队列。
//!
//! # 模块
//!
//! - `channel`：`TermChannel` 的本地实现（本地 shell / 外部进程）。trait 本体在
//!   `term_core::channel`——传输层不该为了它依赖本 crate
//! - `error`：`SpawnError` 的再导出（类型本体同样在 `term_core::channel`）
//! - `pty`：PTY 所有权（`MasterPty` / `Child` 只在这里出现）
//! - `serial`：原生串口后端（termios 直接开设备，不借 `cu` 子进程）
//! - `serial_channel`：把串口包成 `TermChannel`（RX/TX 计数、写缓冲、拔线报一次）
//! - `telnet_channel`：原生 Telnet（`TcpStream` + 读线程 + 协商分流，不拉系统 `telnet`）
//! - `supervisor`：每会话一个线程，`drain → vt → 快照 / 范围窗口` 发布
//! - `log`：会话日志写入端（清洗在 `term_core::ansi`，落盘在这里）
//! - `local_dir`：本地目录枚举（Filer 的本地数据源；IO 不进 `ui/`）
//! - `host`：宿主环境事实（当前登录名）——`term-core` 不读环境，值从这里注入
//! - `preview`：文件头部读取 + 内容类型嗅探（D6 预览窗的数据底座）
//!
//! # 为什么 `local_dir` / `preview` 在这里
//!
//! 它是**本地文件系统的 IO**，与伪终端、串口同类：都要持有 OS 句柄，都不能进
//! `ui/`（B2）。它不是「用户档案」——不读写配置，只列目录。所以归 IO 层，
//! 不归 `term-profile`。`preview` 同理：打开文件句柄、读字节、嗅探魔数，全是
//! IO 与字节判定，投影层只该拿到一段已经变成 `String` 的文本和一个截断标记。

pub mod channel;
pub mod error;
pub mod host;
pub mod local_dir;
pub mod log;
pub mod pty;
pub mod preview;
pub mod serial;
pub mod serial_channel;
pub mod supervisor;
pub mod telnet_channel;

/// 把 `catch_unwind` 的 payload 变成一句人读得懂的话。
///
/// 崩溃防护（`docs/design-specs/O-crash-hardening.md` L1）在两处 `catch_unwind`：
/// 会话监管者线程（本 crate）与界面单帧（`apps/desktop`）。两边都要把 payload 翻成
/// 文本给用户看，所以这份实现放在这里给两边共用——**不要**再抄第二份。
///
/// 标准形态只有两种：`&'static str`（`panic!("…")`）与 `String`（`panic!("{e}")`、
/// `unwrap` 的格式化消息）。其余来自 `panic_any` 的自定义类型拿不到文本，只能报
/// 「读不出消息」——兜底分支**绝不能再 panic**，那会把一次可恢复的崩溃变成 abort。
///
/// 调用点注意：参数是 `&*payload` 而不是 `&payload`。`payload` 是
/// `Box<dyn Any + Send>`，而 `Box<dyn Any + Send>` 自己也是 `'static`（`dyn Any`
/// 隐含 `+ 'static`），所以 `&payload` 会 unsize 成「Box 本身」的 `&dyn Any`，
/// 两个 downcast 全落空——实测过，退化成「无法读取消息」。
pub fn panic_message(payload: &(dyn std::any::Any + Send)) -> String {
    if let Some(s) = payload.downcast_ref::<&'static str>() {
        (*s).to_string()
    } else if let Some(s) = payload.downcast_ref::<String>() {
        s.clone()
    } else {
        "panic（无法读取消息）".to_string()
    }
}

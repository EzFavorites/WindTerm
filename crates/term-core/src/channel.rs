//! 会话字节通道的**契约**：一个通道长什么样，以及它启动不起来时怎么分类。
//!
//! # 为什么这两个类型在 `term-core`，而不是原来的 `term-runtime`
//!
//! 2026-11-12 的底层收敛：`TermChannel` 有三个实现方（本地 PTY、外部进程、
//! 原生 SSH），其中 `term-ssh` 是**传输层**。它原来为了 `impl TermChannel`
//! 与 `From<SshError> for SpawnError` 这两件事，必须依赖 `term-runtime` ——
//! 而 `term-runtime` 拖着 portable-pty、nix、dirs、rusqlite 一整串副作用依赖。
//! 于是「SSH 能连」这件事被迫和「本机能开伪终端」绑在一起：反向层次。
//!
//! 契约本身是**纯类型**：trait 里没有一个方法碰 IO，`SpawnError` 只是一个
//! 分类枚举（`ConfigError` 本来就是 core 的类型）。把它们下沉之后依赖变成
//! `term-ssh → term-core`，`term-runtime` 只保留实现，并在原位再导出，
//! 既有调用路径（`term_runtime::channel::TermChannel` /
//! `term_runtime::error::SpawnError`）全部不变。
//!
//! # 与 [`crate::session_runtime::Session`] 的分工
//!
//! | 类型 | 层级 | 谁调用 | 形状 |
//! |---|---|---|---|
//! | `TermChannel` | 字节流 | `Supervisor` 线程独占 | `drain` / `write` / `resize`，没有等待语义 |
//! | `Session` | 控制面 | UI / 脚本 / 桥接 / agent | 带 `wait`、`facets`、快照的会话抽象 |
//!
//! 前者是「怎么搬字节」，后者是「会话能做什么」。控制面不认识 `TermChannel`，
//! 字节通道也不认识 `Session`——中间那层是 `Supervisor`。

use std::fmt;

use crate::session::ConfigError;

/// 启动会话失败的原因。
///
/// 为什么不继续用 `String`：这四类错误调用方的反应完全不同，拍平成字符串
/// 就只能显示同一句话。
///
/// | 类别 | 用户能做什么 | 例子 |
/// |---|---|---|
/// | `Config` | 补表单 | 「SSH 会话需要填写主机名」 |
/// | `Pty` | 查系统（权限 / 句柄耗尽） | 打不开 `/dev/ptmx` |
/// | `Program` | 装东西 / 改 PATH | 找不到 `ssh` |
/// | `Serial` | 插线 / 加 `dialout` 组 | 打不开 `/dev/ttyUSB0` |
/// | `Telnet` | 查主机名 / 端口 / 防火墙 | 连不上 `host:23` |
///
/// 边界在各 crate 的 `spawn` / `spawn_exec` / `open_channel` 这几个 `pub fn` 上；
/// 内部辅助函数仍可以是字符串，那是实现细节。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SpawnError {
    /// 会话配置不足，还没走到系统调用。
    Config(ConfigError),
    /// 申请伪终端失败（资源、权限、句柄耗尽）。
    Pty(String),
    /// 要启动的程序不存在或无法执行。
    Program(String),
    /// 打不开串口设备（不存在 / 没权限 / 被占用 / 参数不支持）。
    ///
    /// 单独一类而不是塞进 `Pty`：串口不是伪终端，用户要查的是设备与 `dialout`
    /// 组，不是 `/dev/ptmx`。文案由 `term-runtime::serial::SerialError` 给
    /// （含可操作提示），本模块只留分类。
    Serial(String),
    /// 原生 Telnet 连接失败（DNS / connect / socket 初始化）。
    ///
    /// 单独一类而不是塞进 `Program`：这里根本没有子进程，用户要查的是主机、
    /// 端口与网络，不是去装 `telnet` 客户端。文案由
    /// `term_io::telnet_channel::TelnetError` 给（含主机与端口）。
    Telnet(String),
}

impl From<ConfigError> for SpawnError {
    fn from(e: ConfigError) -> Self {
        SpawnError::Config(e)
    }
}

impl fmt::Display for SpawnError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            SpawnError::Config(e) => write!(f, "{e}"),
            SpawnError::Pty(msg) => write!(f, "无法打开伪终端：{msg}"),
            SpawnError::Program(msg) => write!(f, "无法启动程序：{msg}"),
            SpawnError::Serial(msg) => write!(f, "无法打开串口：{msg}"),
            SpawnError::Telnet(msg) => write!(f, "无法连接 Telnet：{msg}"),
        }
    }
}

impl std::error::Error for SpawnError {}

/// 一个会话字节通道。所有方法都由 Supervisor 线程独占调用。
///
/// 「独占」是契约的一部分而不是注释：实现方不必自带锁，也不该被多个线程同时
/// 调用。`drain` 必须是**非阻塞**的——Supervisor 的循环靠它推进，阻塞在这里
/// 等于把整个会话（含 UI 的刷新）一起停住。
pub trait TermChannel: Send {
    /// 非阻塞取走自上次调用以来到达的字节。
    fn drain(&mut self) -> Vec<u8>;
    /// 写回终端（键盘输入、设备应答）。
    fn write(&mut self, bytes: &[u8]);
    fn resize(&mut self, cols: usize, rows: usize);
    /// 子进程刚退出时返回一次状态文本。
    fn poll(&mut self) -> Option<String>;
    fn finished(&self) -> bool;
    fn close(&mut self);
    /// 状态栏 / 日志用的一句话描述。
    fn description(&self) -> &str;
    /// RX/TX 字节计数，`None` = 这类通道不统计（PTY / 外部进程）。
    fn counters(&self) -> Option<(u64, u64)> {
        None
    }
    /// 一句话的运行态补充（如 `rx 12.3 KB / tx 4 B, dropped 0`）。默认没有。
    fn status_note(&self) -> Option<String> {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session::Protocol;

    #[test]
    fn display_is_readable_per_class() {
        let cfg = SpawnError::Config(ConfigError::MissingHost(Protocol::Ssh));
        assert!(cfg.to_string().contains("需要填写主机名"), "{cfg}");

        let pty = SpawnError::Pty("/dev/ptmx: permission denied".to_string());
        assert!(pty.to_string().contains("伪终端"), "{pty}");

        let prog = SpawnError::Program("ssh".to_string());
        assert!(prog.to_string().contains("无法启动程序"), "{prog}");

        let serial = SpawnError::Serial("/dev/ttyUSB0 已被占用".to_string());
        assert!(serial.to_string().contains("无法打开串口"), "{serial}");
    }

    #[test]
    fn config_error_converts_into_spawn_error() {
        // `?` 要靠这个转换才能把 core 的配置错误带上来。
        let e: SpawnError = ConfigError::MissingHost(Protocol::Telnet).into();
        assert!(matches!(e, SpawnError::Config(_)));
    }
}

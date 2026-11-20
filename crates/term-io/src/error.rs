//! 启动会话时的错误分类 —— 类型本体**已下沉到 `term_core::channel`**（2026-11-12）。
//!
//! 这里只保留再导出，`use term_runtime::error::SpawnError` 继续可用。
//!
//! # 为什么下沉
//!
//! `term-ssh` 要把 `SshError` 转成 `SpawnError`（应用层只认识后者）。它是**传输层**，
//! 原来为了这一个 `impl From` 必须依赖 `term-runtime`——而后者拖着 portable-pty、
//! nix、dirs、rusqlite 一整串副作用依赖，于是「SSH 能连」被绑在「本机能开伪终端」上。
//! 枚举本身是纯类型，下沉到 core 之后依赖变成 `term-ssh → term-core`。
//!
//! 四类错误的用户动作表见 `term_core::channel` 的模块文档。
//! 原来这里的两条单测（分类文案 / `ConfigError` 转换）也一并搬了过去。

pub use term_core::channel::SpawnError;

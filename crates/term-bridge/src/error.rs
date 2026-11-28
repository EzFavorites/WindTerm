//! 统一错误分类（ADR-0019 §B.11）：`{code, message, kind}` 三元组。
//!
//! **本模块不再是定义处**：全部实现与测试已上移到 `term_core::surface_error`。
//! 这里只做再导出，保留 `term_bridge::error::{ErrorKind, SurfaceError}` 与
//! `crate::error::*` 这些既有路径——调用方不必因为契约搬家而改代码，
//! 而桥接本体（纯传输适配器）手上也确实不再有一份契约。
//!
//! 线上形状一个字未改：标准码 -32700 / -32600 / -32601 / -32602 / -32603，
//! 应用条件落在 -32000 … -32004；`kind` 平铺在 `error` 对象里（不是 `data`）。

pub use term_core::surface_error::{ErrorKind, SurfaceError};

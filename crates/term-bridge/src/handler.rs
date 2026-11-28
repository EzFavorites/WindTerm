//! 实现层接缝（**契约再导出**）：[`Surface`] trait、`describe` 的唯一生成处，
//! 以及 `required_*` / `optional_*` 参数读取助手。
//!
//! 这些定义的唯一真相已上移到 `term_core::contract`；本模块只是把既有路径
//! （`term_bridge::handler::*` 与 `crate::handler::*`）保留为再导出，
//! 让桥接保持纯传输适配器的身份。要点不变：
//!
//! - 未知方法报 `method_not_found`，不许返回 `Ok(Value::Null)`；
//! - 失败要分类（见 `term_core::surface_error`）；
//! - 不阻塞在别的线程上，不发通知、不写 stdout；
//! - `describe` 只从 `term_core::surface::METHODS` 生成，没有第二份清单。

pub use term_core::contract::{
    describe_value, optional_string, optional_u64, required_session, required_string, Surface,
};

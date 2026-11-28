//! 自动化内核：事件 → 匹配 → 动作的**纯语义层**。
//!
//! 本目录只回答「命中没有、命中了要做什么」，不执行任何东西：时钟由
//! [`signal::Ticks`] 注入，副作用以 [`rule::Effect`] 返回给外层 dispatcher。
//! 依赖方向保持 `term-core` 是叶子之外的最低层。
//!
//! # 两条独立的能力
//!
//! - [`Pattern`] / [`LinePolicy`] / [`apply_lines`]：模式与结果后清理。
//!   grep 只作用于**返回文本**，不碰匹配与游标（2026-11-19 用户裁定）。
//! - [`ScanCursor`] / [`Evaluator`]：跨 chunk 的双指针窗口与规则求值。
//!
//! 推（触发器）与拉（脚本 `await`）共用前两条原语，但**不共用调度**：
//! `ScanCursor::probe` 不消费，`ScanCursor::take` 推进游标，两者读同一个
//! buffer 的不同指针，类型层面就不可能互相吞数据。

pub mod cursor;
pub mod engine;
pub mod line;
pub mod pattern;
pub mod rule;
pub mod signal;

pub use cursor::ScanCursor;
pub use engine::Evaluator;
pub use line::{apply_lines, LinePolicy};
pub use pattern::{Pattern, PatternError, Span, Syntax, WindowHint};
pub use rule::{
    validate_rules, Action, AlertKind, AppAction, Effect, Filter, Guard, Invocation, LinkSpec,
    MarkSpec, MenuItemSpec, MenuSpec, OnceScope, Release, Rule, RuleFault, RuleId, TextSpec,
};
pub use signal::{CloseReason, Lifecycle, Signal, Target, Ticks, When};

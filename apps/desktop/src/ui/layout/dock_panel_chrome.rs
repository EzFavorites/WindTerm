//! 已迁往 [`super::dock_container`]（K1，2026-11-16）。
//!
//! 保留这个文件只为兼容 `dock_paint.rs` 里的全路径调用；绘制实现与
//! `PanelFilters` 的唯一出处现在是 `dock_container.rs`。

pub(in crate::ui) use super::dock_container::chrome_height;

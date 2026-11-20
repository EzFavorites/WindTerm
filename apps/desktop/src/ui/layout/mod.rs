//! 布局：分屏树 / 结构恢复 / 停靠与浮动位。
//!
//! 这一组是**投影层的视图结构**，不是 `ViewState` 字段：
//! reducer 只声明意图（`Action`），具体改动由 `effects` 落地。
//! 与 `panels` 的区别——`panels` 决定「画什么」，这里决定「画在哪、多大」。

mod dock;
pub(in crate::ui) mod dock_bottom;
pub(in crate::ui) mod dock_bottom_menu;
pub(in crate::ui) mod dock_container;
mod dock_drag;
mod dock_float_band;
mod dock_native;
mod dock_paint;
mod dock_panel_chrome;
mod pane_chrome;
mod pane_ops;
mod restore;
mod split;
mod swap;

// `Placement` 一起再导出：`ui/app_state.rs` 要按「停靠 / 浮动」分支采快照，
// 而 `dock` 是 `layout` 的私有子模块——不从这里转一手，外部拿不到这个枚举。
pub(in crate::ui) use dock::{DockLayout, Placement};
pub(in crate::ui) use dock_container::PanelFilters;
// ④-0：布局叶子现在是「窗格身份」（`PaneId`），每片窗格一个 `PaneGroup`
// （会话集合 + 当前显示的那条）。三者一起对 `crate::ui` 可见，调用方统一用
// `crate::ui::layout::{Layout, PaneId, PaneGroup}` 取用。
pub(in crate::ui) use split::{Layout, PaneGroup, PaneId};
// `draw_node` 是分屏递归绘制（纯终端绘制，不碰窗格语义）；`mint_pane_identity`
// 是新窗格恒在身份的铸造源。两者都从 `split.rs` 再导出，让 `pane_ops.rs` 与调用方
// 统一走 `crate::ui::layout::{draw_node, mint_pane_identity}`。
pub(in crate::ui) use pane_ops::{remove_from_group, GroupRemoval};
pub(in crate::ui) use split::{draw_node, mint_pane_identity, pane_group_name};

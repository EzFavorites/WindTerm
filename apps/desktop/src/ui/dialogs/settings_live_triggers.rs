//! `Terminal.ManageTriggers` 的对话框内容入口。
//!
//! 真正的实现搬到了 `dialogs/triggers/`（按视觉基线 1:1 对齐，11 种类型 ×
//! 多页面）。本文件只保留**一个**函数名，因为还有两处调用方
//! 认这个名字：
//!
//! - `dialogs/triggers_dialog.rs`（独立窗口宿主）
//! - `dialogs/settings.rs` 的 `SettingsPage::Triggers` 分支（组件夹具遗留路径）
//!
//! 两边必须共用同一份实现，否则会出现「点菜单和点夹具看到两个界面」的老问题。

use crate::ui::*;

/// 画整窗内容（内容原点取 `ui.cursor().min`，供设置页遗留路径使用）。
pub(in crate::ui) fn triggers(win: &mut TerminalWindow, ui: &mut egui::Ui) {
    triggers_at(win, ui, ui.cursor().min);
}

/// 画整窗内容，原点由调用方给（对话框宿主传窗口左上角）。
pub(in crate::ui) fn triggers_at(
    win: &mut TerminalWindow,
    ui: &mut egui::Ui,
    origin: egui::Pos2,
) {
    // 字号对齐视觉基线：正文 11 逻辑点，走统一 small 档（全 UI 等宽）。
    crate::ui::kit::type_scale::install_compact_styles(ui.style_mut());
    let _ = super::triggers::show(win, ui, origin);
}

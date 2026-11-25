//! 弹层「点外收起」那一击的**终端侧闸门**。
//!
//! `quick_config_menu::close_on_click_outside` 只删本帧 Primary 的 release，
//! egui 帧首已经算好的 `clicked()` / `released` 边沿还在。于是点在终端上把配置
//! 弹层收起来的那一击，会继续被 `pane.rs` 的两个输入分支当成真点击：折叠列被
//! 顺手翻掉，xterm 鼠标报告还会往会话里编码一个 click。
//!
//! 两个分支共用这里的判据，判据本身又直接复用 shell 公开的 `click_not_consumed`
//! ——「这一击已被弹层消费」全仓只有一份定义，快捷栏按钮与终端不会各自长歪。

use eframe::egui;

/// 折叠列点击的原始边沿此刻能不能真的动作。
pub(in crate::ui) fn fold_click_allowed(ui: &egui::Ui, raw_click: bool) -> bool {
    raw_click && crate::ui::shell::click_not_consumed(ui)
}

/// 本帧是否允许编码 xterm 鼠标报告。
///
/// 消费掉的是**整一帧**的这一击，所以鼠标报告整段都不编码：半帧的 down、
/// 半帧的 up 拼出来的 click 比不报更糟。
pub(in crate::ui) fn mouse_encoding_allowed(ui: &egui::Ui) -> bool {
    crate::ui::shell::click_not_consumed(ui)
}

//! `Terminal.ManageOneKeys` 的**独立对话框**（2026-11-15 用户对照兼容终端指出）。
//!
//! 兼容终端（视觉基线 `SSH / Account / Sessions` 三标签那份）是自有窗口：左列表 +
//! 右编辑区 + 页脚 `Cancel / Ok`。2026-11-15 之前它被挂成
//! `SettingsPage::OneKeys`，弹出来的是 Preferences——而该页早已按用户裁定
//! 移出左树，于是用户看到「点了进来一个翻不到 OneKeys 的设置窗口」。
//!
//! 老实现自己的模块文档就写着「窗口结构与 WindTerm 的 OneKeys 对话框
//! 逐项对应」——内容本来就是照兼容终端做的，缺的只是**宿主换成真正的窗口**。
//! 因此这里不重写内容，只换壳：内容仍由 `settings_live_onekeys::onekeys` 提供，
//! 组件夹具（`comp:dialog_settings_onekeys`）继续走 `draw_settings_dialog`。

use crate::ui::*;
use crate::ui::kit::window::{DialogId, DialogWindow};

/// 内容最小尺寸。2026-11-15：初版按兼容终端 438 高，结果「左栏列表 + 按钮行 +
/// 页脚」加起来超了 20 点，页脚整条被裁掉（截图里只剩 Cancel/Ok 的空白）。
/// 兼容终端窗口本身是 496 高，页脚在主体下方**独立一条**，所以这里给到 462。
const CONTENT_MIN: egui::Vec2 = egui::vec2(684.0, 653.0);

/// 内容区宽度：弹窗 702 宽 − 边框与内边距 ≈18。
const CONTENT_W: f32 = 684.0;

impl TerminalWindow {
    /// 画 OneKeys 对话框。关闭路径：页脚 `Close` / 标题栏 `×`。
    pub(in crate::ui) fn draw_onekeys_dialog(&mut self, ctx: &egui::Context) {
        // 兼容终端这个对话框**没有标题栏**（顶部直接是 SSH/Account/Sessions 标签），
        // 也没有右上角 ×：关闭只走页脚 `Cancel` / `Ok`。
        let open = DialogWindow::new(DialogId::OneKeys, "OneKeys")
            .without_title_bar()
            .tight()
            .show(ctx, |ui| {
            // 内容全是绝对定位，不撑宽也不撑高——必须显式钉死最小尺寸，
            // 否则 `StableWindow` 会按内容收缩（实测缩到 592×567，白丢 110 点）。
            ui.set_min_size(CONTENT_MIN);
            ui.set_max_width(CONTENT_W);
            super::settings_live_onekeys::onekeys(self, ui);
        });

        // 内容自带 `Cancel / Ok`，两者都会置 `show_onekeys=false`；这里只管
        // 标题栏的 ×。
        if !open {
            self.view.show_onekeys = false;
            self.ctx.request_repaint();
        }
    }
}

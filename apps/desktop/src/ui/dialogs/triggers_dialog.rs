//! `Terminal.ManageTriggers` 的**独立对话框**宿主。
//!
//! 本文件只负责窗口矩形 / 尺寸 / 关闭路径；编辑器全部在
//!
//! 尺寸取 `ui::view::trigger_spec` 的 768×562——这是视觉基线两套口径
//! （`1280×937 ×0.6` 与 `1536×1124 ÷2`）归一后的同一尺寸。

use crate::ui::view::trigger_spec as spec;
use crate::ui::*;
use crate::ui::kit::window::DialogWindow;

impl TerminalWindow {
    /// 画 Triggers 对话框。三条关闭路径共用同一个出口：页脚按钮、
    /// 上层把 `show_triggers` 置回 `false`。
    pub(in crate::ui) fn draw_triggers_dialog(&mut self, ctx: &egui::Context) {
        let open = DialogWindow::new_sized(
            "triggers",
            "Triggers",
            egui::vec2(spec::DIALOG_W, spec::DIALOG_H),
        )
        .without_title_bar()
        .tight()
        .show_with_rect(ctx, |ui, win| {
            // 宽度钉死：外壳只保证「不小于」，长文案会把窗口顶宽。
            ui.set_max_width(spec::DIALOG_W);
            ui.set_min_height(spec::DIALOG_H);
            // 原点用窗口矩形而不是 `ui.cursor()`：后者带着框架的内边距，
            // 会让整套绝对坐标整体偏移（实测 +18 逻辑点）。
            super::settings_live_triggers::triggers_at(self, ui, win.min);
        });

        if !open {
            self.view.show_triggers = false;
            self.ctx.request_repaint();
        }
    }
}

//! 终端区下沿那条 `Sysmon` 细带（新版截图，2026-11-08）。
//!
//! 视觉基线里它不是状态栏、也不属于底带：底带（Sender / Shell 标签条）在它
//! **下面**且横跨整窗；这条细带只占中央终端区的宽度，右端有一枚带边框的
//! 折叠标签 `Sysmon`。所以它在 `draw()` 里必须在左右竖带之后、中央区之前
//! 声明——egui 先声明的 panel 占外圈，顺序错了这条带就会横跨到竖带上方。
//!
//! R5：这枚标签是**真开关**。点一下翻 `TogglePanel(PanelId::Sysmon)`，再点
//! 一下收回；展开出来的面板也有自己的 ×。没有「点了没反应」的假标签。

use crate::ui::*;
use eframe::egui;
use term_app::action::Action;
use term_app::dock::PanelId;
use term_app::shell::ShellAction;

/// 细带高度（视觉基线 2× 图约 50px → 25pt）。
pub(in crate::ui) const SYSMON_BAR_H: f32 = 25.0;
/// 右端折叠标签宽度。
const TAB_W: f32 = 96.0;

/// 零内边距：标签的边框要贴住窗沿，不能被 `Frame` 的全局 margin 推走。
pub(in crate::ui) fn sysmon_bar_frame() -> egui::Frame {
    egui::Frame::none().fill(crate::theme::chrome(
        term_core::theme::CHROME_SCALE.dialog,
    ))
}

impl TerminalWindow {
    /// `pane_salt`：④-1 分屏时每片 pane 各画一条 Sysmon，`ui.interact` 的 id
    /// 必须带 pane 身份，否则三格同名 id 会撞（egui 报 "First/Second use of
    /// widget ID"）。全窗路径传 `None`，id 保持原来的 `"sysmon_tab"` 一字不变。
    pub(in crate::ui) fn draw_sysmon_bar(&mut self, ui: &mut egui::Ui, pane_salt: Option<u64>) {
        let rect = ui.max_rect();
        let open = self.panel_visible(PanelId::Sysmon);
        let tab = egui::Rect::from_min_max(
            egui::pos2(rect.right() - TAB_W, rect.top() + 1.0),
            egui::pos2(rect.right(), rect.bottom() - 1.0),
        );
        let p = ui.painter();
        let border = crate::theme::chrome(term_core::theme::CHROME_SCALE.border);
        if open {
            p.rect_filled(
                tab,
                0.0,
                crate::theme::chrome(term_core::theme::CHROME_SCALE.tab_active),
            );
        }
        p.rect_stroke(tab, 0.0, egui::Stroke::new(1.0, border));
        // 细带下沿那道分隔线：它把这条带与底带标签条分开。
        p.hline(
            rect.left()..=tab.left(),
            rect.bottom() - 0.5,
            egui::Stroke::new(1.0, border),
        );
        p.text(
            tab.center(),
            egui::Align2::CENTER_CENTER,
            self.tr("Sysmon"),
            crate::ui::type_scale::body(),
            crate::theme::chrome(if open {
                term_core::theme::CHROME_SCALE.text_strong
            } else {
                term_core::theme::CHROME_SCALE.text
            }),
        );
        let sysmon_id = match pane_salt {
            Some(p) => egui::Id::new(("sysmon_tab", p)),
            None => egui::Id::new("sysmon_tab"),
        };
        let resp = ui.interact(tab, sysmon_id, egui::Sense::click());
        if resp.hovered() {
            ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
        }
        if resp.clicked() {
            self.dispatch_action(Action::Shell(ShellAction::TogglePanel(PanelId::Sysmon)));
        }
    }
}

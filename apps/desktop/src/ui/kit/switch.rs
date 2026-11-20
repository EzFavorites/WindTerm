//! 拨动开关（trigger switch）：**一处定义、多处复用**的函数式原语。
//!
//! 兼容终端形状量自 `04-new-session/00-ssh-session.png`（2352×1532 @2×）的
//! `Sftp`（开）与 `Sysmon`（关）：
//!
//! | 项 | 物理像素 | 逻辑点 |
//! |---|---|---|
//! | 轨道 | 62×32（x 1159..1220, y 1002..1033） | 31×16 |
//! | 圆点 | 22×22 | 11×11 |
//! | 开态蓝 | `#007ACC` | — |
//! | 关态灰 | `#444444` | — |
//! | 圆点 | `#FFFFFF`，轨道内缩 5 物理点（2.5 逻辑点） | — |
//!
//! 遵守 ADR-0021：Props 只读，点击以返回值交回调用方，组件不碰业务状态。
//! 不引入 retained widget / 动画状态——兼容终端静态截图就是这个形状，交互反馈
//! 只靠 hover 提亮一档（蓝底永远是指向色，不是固有底色）。

use eframe::egui;

/// 轨道尺寸（逻辑点）：31×16，胶囊圆角 = 高的一半。
const TRACK_W: f32 = 31.0;
const TRACK_H: f32 = 16.0;
/// 滑块圆点直径与内缩。
const KNOB_D: f32 = 11.0;
const KNOB_INSET: f32 = 2.5;

/// 开态蓝。**不用主题 `accent`**：主题是 `#047CCC`，兼容终端这个控件量得
/// `#007ACC`，差一档；这是控件的固有状态色，不是指向高亮。
const ON: u32 = 0x007ACC;
/// 关态灰。
const OFF: u32 = 0x444444;
/// hover 提亮：蓝底变亮一档、灰底变浅一档。
const ON_HOVER: u32 = 0x2E9BE0;
const OFF_HOVER: u32 = 0x5A5A5A;

fn rgb(hex: u32) -> egui::Color32 {
    egui::Color32::from_rgb((hex >> 16) as u8, ((hex >> 8) & 0xFF) as u8, hex as u8)
}

/// 一个拨动开关的完整外观。只读；点击返回给调用方落地。
pub(in crate::ui) struct SwitchProps<'a> {
    /// 稳定 id：同一帧多个开关不能共用，否则 hover 会互相点亮。
    pub id: &'a str,
    /// 可选标签。给 `Some` 时画在开关**左侧**、右对齐到 `label_width`。
    pub label: Option<&'a str>,
    /// 标签列宽。`label` 为 `None` 时忽略。
    pub label_width: f32,
    /// 当前是否打开。
    pub on: bool,
}

impl<'a> SwitchProps<'a> {
    /// 带左标签的开关（兼容终端 `Sftp:` / `Sysmon:` 的形态）。
    pub(in crate::ui) fn labeled(id: &'a str, label: &'a str, label_width: f32, on: bool) -> Self {
        Self { id, label: Some(label), label_width, on }
    }

    /// 只有轨道、没有标签的开关。
    pub(in crate::ui) fn bare(id: &'a str, on: bool) -> Self {
        Self { id, label: None, label_width: 0.0, on }
    }
}

/// 画开关。**返回 `true` 表示这一帧被点击**，由调用方翻转自己的状态。
pub(in crate::ui) fn trigger_switch(ui: &mut egui::Ui, props: &SwitchProps<'_>) -> bool {
    let response = ui.horizontal(|ui| {
        if let Some(label) = props.label {
            let (rect, _) =
                ui.allocate_exact_size(egui::vec2(props.label_width, TRACK_H), egui::Sense::hover());
            let font = egui::TextStyle::Body.resolve(ui.style());
            let color = crate::theme::chrome(term_core::theme::CHROME_SCALE.text);
            let painter = ui.painter().clone();
            painter.text(
                rect.right_center() - egui::vec2(4.0, 0.0),
                egui::Align2::RIGHT_CENTER,
                label,
                font.clone(),
                color,
            );
            // 访问键下划线：兼容终端每个开关标签的首字母下都有一条（Sftp 的 S、
            // Sysmon 的首字母）。等宽字体下按字符数均分即可，不必为两个标签
            // 单独量字宽。
            let chars = label.chars().count().max(1) as f32;
            let galley = painter.layout_no_wrap(label.to_owned(), font, color);
            let cw = galley.size().x / chars;
            // 下划线画在**首字符**下（兼容终端 Sftp 的 S、Sysmon 的 S），不是末尾。
            let underline_left = rect.right() - 4.0 - galley.size().x;
            let y = rect.center().y + 6.0;
            painter.line_segment(
                [
                    egui::pos2(underline_left, y),
                    egui::pos2(underline_left + cw, y),
                ],
                egui::Stroke::new(1.0, color),
            );
        }
        let (rect, response) =
            ui.allocate_exact_size(egui::vec2(TRACK_W, TRACK_H), egui::Sense::click());
        let painter = ui.painter().clone();
        let (track, knob_x) = match (props.on, response.hovered()) {
            (true, true) => (ON_HOVER, rect.right() - KNOB_INSET - KNOB_D / 2.0),
            (true, false) => (ON, rect.right() - KNOB_INSET - KNOB_D / 2.0),
            (false, true) => (OFF_HOVER, rect.left() + KNOB_INSET + KNOB_D / 2.0),
            (false, false) => (OFF, rect.left() + KNOB_INSET + KNOB_D / 2.0),
        };
        painter.rect_filled(rect, TRACK_H / 2.0, rgb(track));
        painter.circle_filled(
            egui::pos2(knob_x, rect.center().y),
            KNOB_D / 2.0,
            egui::Color32::WHITE,
        );
        response
    });
    response.inner.clicked()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn track_and_knob_follow_the_measured_reference() {
        assert_eq!(TRACK_W, 31.0);
        assert_eq!(TRACK_H, 16.0);
        assert_eq!(KNOB_D, 11.0);
        assert_eq!(KNOB_INSET, 2.5);
        assert_eq!(ON, 0x007ACC);
        assert_eq!(OFF, 0x444444);
    }

    #[test]
    fn knob_centers_stay_inside_the_track() {
        // 开 / 关两端的圆心都必须在轨道内，且离边缘恰好是内缩 + 半径。
        let left = 0.0 + KNOB_INSET + KNOB_D / 2.0;
        let right = TRACK_W - KNOB_INSET - KNOB_D / 2.0;
        assert!(left >= KNOB_D / 2.0);
        assert!(right + KNOB_D / 2.0 <= TRACK_W);
        assert_eq!(right - left, TRACK_W - KNOB_D - 2.0 * KNOB_INSET);
    }
}

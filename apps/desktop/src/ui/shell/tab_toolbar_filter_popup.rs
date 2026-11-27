//! 过滤历史的下拉浮层：贴过滤行上方的 `egui::Area`，**不新增 `TopBottomPanel`、
//! 不切分终端高度**。与 `panels/list_popup.rs` 同一套手绘行 + 淡入关闭 + 点外收起。
//!
//! `fade_in(false)` 是必需的：视觉夹具把时钟钉在 0，egui 默认淡入会停在全透明上。

use eframe::egui;
use term_core::theme::CHROME_SCALE;

/// 一行高与浮层最高：超了就滚动，不把整窗盖住。
pub(super) const ROW_H: f32 = 18.0;
pub(super) const MAX_H: f32 = 168.0;

/// 画历史浮层，返回本帧鼠标点中的下标（`None` = 没点条目）。
/// `anchor` 是输入区（浮层贴它上沿），`button` 是 `▾`（点它不算「点外」）。
pub(super) fn history_popup(
    ui: &mut egui::Ui,
    id: egui::Id,
    anchor: egui::Rect,
    button: egui::Rect,
    items: &[String],
    selected: Option<usize>,
) -> Option<usize> {
    let screen = ui.ctx().screen_rect();
    let width = anchor.width().clamp(120.0, screen.width() - 8.0);
    // 上方空间不够时把锚点往下推，至少留 MAX_H + 8，浮层不会跑出窗口顶。
    let top = anchor.top().max(screen.top() + MAX_H + 8.0);
    let mut picked = None;
    let area = egui::Area::new(id)
        .order(egui::Order::Foreground)
        .fixed_pos(egui::pos2(anchor.left(), top))
        .pivot(egui::Align2::LEFT_BOTTOM)
        .fade_in(false)
        .show(ui.ctx(), |ui| {
            ui.set_width(width);
            let text_c = crate::theme::chrome(CHROME_SCALE.text);
            let active_bg = crate::theme::chrome(CHROME_SCALE.tab);
            let hover_bg = crate::theme::chrome(CHROME_SCALE.hover);
            let font = egui::TextStyle::Button.resolve(ui.style());
            // 壳用比选中行更暗的 dialog，当前项的 tab 底色才看得出来。
            egui::Frame::popup(ui.style())
                .fill(crate::theme::chrome(CHROME_SCALE.dialog))
                .stroke(egui::Stroke::new(
                    1.0,
                    crate::theme::chrome(CHROME_SCALE.border),
                ))
                .show(ui, |ui| {
                    if items.is_empty() {
                        ui.label(
                            egui::RichText::new("No filter history")
                                .small()
                                .color(text_c),
                        );
                        return;
                    }
                    egui::ScrollArea::vertical()
                        .max_height(MAX_H)
                        .show(ui, |ui| {
                            for (i, text) in items.iter().enumerate() {
                                let (r, resp) = ui.allocate_exact_size(
                                    egui::vec2(ui.available_width(), ROW_H),
                                    egui::Sense::click(),
                                );
                                let on = selected == Some(i);
                                if on || resp.hovered() {
                                    ui.painter().rect_filled(
                                        r,
                                        0.0,
                                        if on { active_bg } else { hover_bg },
                                    );
                                }
                                let g = ui
                                    .painter()
                                    .layout_no_wrap(text.clone(), font.clone(), text_c);
                                ui.painter().galley(
                                    egui::pos2(r.left() + 6.0, r.center().y - g.size().y / 2.0),
                                    g,
                                    text_c,
                                );
                                if resp.clicked() {
                                    picked = Some(i);
                                }
                            }
                        });
                });
        });
    let outside = ui.input(|i| {
        i.pointer.any_click()
            && i.pointer.interact_pos().is_some_and(|p| {
                !area.response.rect.contains(p) && !button.contains(p)
            })
    });
    if outside {
        ui.memory_mut(|m| m.close_popup());
    }
    picked
}

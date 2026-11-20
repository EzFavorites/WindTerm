//! 工具行下拉的**通用单选列表弹层** + 里面的一行。
//!
//! 三处共用同一份实现：Sender 的 `Targets`（五项枚举）、Script 的脚本列表、
//! Script 的执行会话列表。三者的差别只有「列什么」，不该长出三个壳。
//!
//! 为什么不用 `egui::ComboBox`：它自带一套自己的箭头、内边距和弹层壳，与工具
//! 行手绘的 18 点输入框对不齐（`tools_sender_row` 的 `dropdown` 是手绘的）。
//! 这里沿用 `popup_below_widget` 之外的做法——显式 `Area` + `fixed_pos` +
//! `.fade_in(false)`：夹具把时钟钉在 0，默认淡入会停在第一帧的全透明上。

use eframe::egui;
use term_core::theme::CHROME_SCALE;

use super::tools_sender_row::{RADIO_D, ROW_H};

/// 列表最高：项再多也不能把整个窗口盖住，超了就滚动。
const MAX_H: f32 = 260.0;
/// 弹层最小宽。正常跟随触发它的那一格，只有窄面板下才用这个下限。
const MIN_W: f32 = 160.0;

/// 弹层里的一行：单选圆 + 文字。返回是否被点。
pub(super) fn menu_row(ui: &mut egui::Ui, text: &str, on: bool) -> bool {
    let (rect, resp) = ui.allocate_exact_size(
        egui::vec2(ui.available_width(), ROW_H),
        egui::Sense::click(),
    );
    let p = ui.painter();
    if resp.hovered() {
        p.rect_filled(rect, 0.0, crate::theme::chrome(CHROME_SCALE.hover));
    }
    let k = egui::pos2(rect.left() + 14.0, rect.center().y);
    p.circle_stroke(
        k,
        RADIO_D / 2.0 - 0.5,
        egui::Stroke::new(1.0, crate::theme::chrome(CHROME_SCALE.border)),
    );
    if on {
        p.circle_filled(
            k,
            RADIO_D * 0.28,
            crate::theme::chrome(CHROME_SCALE.text_strong),
        );
    }
    let font = egui::TextStyle::Button.resolve(ui.style());
    let c = crate::theme::chrome(CHROME_SCALE.text);
    let g = p.layout_no_wrap(text.to_string(), font, c);
    p.galley(
        egui::pos2(rect.left() + 28.0, rect.center().y - g.size().y / 2.0),
        g,
        c,
    );
    resp.clicked()
}

/// 单选项的列表弹层。返回本帧被选中的下标（`None` = 没点）。
///
/// 单选语义：点中一项就 `close_popup`，壳跟着收起来（多选那份见
/// `tools_sender_menu::picked_popup`，它故意不关）。
pub(super) fn list_popup(
    ui: &mut egui::Ui,
    id: egui::Id,
    anchor: egui::Rect,
    items: &[String],
    selected: Option<usize>,
    empty: &str,
) -> Option<usize> {
    if !ui.memory(|m| m.is_popup_open(id)) {
        return None;
    }
    let mut picked = None;
    let area = egui::Area::new(id.with("area"))
        .order(egui::Order::Foreground)
        .fixed_pos(anchor.left_bottom())
        .pivot(egui::Align2::LEFT_TOP)
        .fade_in(false)
        .show(ui.ctx(), |ui| {
            egui::Frame::popup(ui.style())
                .fill(crate::theme::chrome(CHROME_SCALE.tab))
                .stroke(egui::Stroke::new(
                    1.0,
                    crate::theme::chrome(CHROME_SCALE.border),
                ))
                .show(ui, |ui| {
                    // Area 里的 `available_width` 是整屏宽，必须钉死；宽度跟随
                    // 触发它的那一格，否则弹层像另一件东西叠在下拉框下面。
                    ui.set_width(anchor.width().max(MIN_W));
                    if items.is_empty() {
                        ui.add_space(2.0);
                        ui.label(
                            egui::RichText::new(empty.to_string())
                                .color(crate::theme::chrome(CHROME_SCALE.text))
                                .small(),
                        );
                        return;
                    }
                    egui::ScrollArea::vertical()
                        .max_height(MAX_H)
                        .show(ui, |ui| {
                            for (i, text) in items.iter().enumerate() {
                                if menu_row(ui, text, selected == Some(i)) {
                                    picked = Some(i);
                                }
                            }
                        });
                });
        });
    if picked.is_some() {
        ui.memory_mut(|m| m.close_popup());
        return picked;
    }
    let rect = area.response.rect;
    let out = ui.input(|i| {
        i.pointer.any_click()
            && !i
                .pointer
                .interact_pos()
                .is_some_and(|p| rect.contains(p) || anchor.contains(p))
    });
    if out {
        ui.memory_mut(|m| m.close_popup());
    }
    None
}

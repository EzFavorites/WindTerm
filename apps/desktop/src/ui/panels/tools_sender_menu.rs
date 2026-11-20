//! `Targets` 下拉展开的那五行 + 它的弹层，以及 `Selected Sessions` 右边
//! 那个**多选**下拉的弹层。
//!
//! 与工具行分开是因为 R9：`tools_sender_row.rs` 再加这一块就顶到 500 行。
//! 两件事本来就不同——那边画「行的样子」，这边管「点开之后」。

use eframe::egui;
use term_app::i18n::{self, Lang};
use term_core::theme::CHROME_SCALE;

use super::list_popup::menu_row;
use super::tools_sender_row::{RADIO_D, ROW_H};
use super::tools_sender_state::Target;

/// `Targets` 下拉宽（视觉基线展开的列表 868..1097）。
const TARGET_W: f32 = 229.0;
/// 多选弹层的最高：标签再多也不能把整个窗口盖住，超了就滚动。
const PICK_MAX_H: f32 = 260.0;
/// 多选弹层的最小宽。正常跟随触发它的那一格（两者对齐看着是一件东西），
/// 只有窄面板下那一格被压得很小时才用这个下限。
const PICK_POPUP_MIN_W: f32 = 160.0;

pub(super) fn targets_menu_id() -> egui::Id {
    egui::Id::new("sender_targets_menu")
}

pub(super) fn picked_menu_id() -> egui::Id {
    egui::Id::new("sender_picked_menu")
}

/// `Targets` 展开的五行单选。点中返回新目标。
pub(super) fn targets_popup(
    ui: &mut egui::Ui,
    anchor: egui::Rect,
    current: Target,
    lang: Lang,
) -> Option<Target> {
    if !ui.memory(|m| m.is_popup_open(targets_menu_id())) {
        return None;
    }
    let mut picked = None;
    let area = egui::Area::new(targets_menu_id().with("area"))
        .order(egui::Order::Foreground)
        .fixed_pos(anchor.left_bottom())
        .pivot(egui::Align2::LEFT_TOP)
        // 与齿轮菜单同一个坑：`Area` 默认淡入，夹具时钟钉在 0 会停在第一帧。
        .fade_in(false)
        .show(ui.ctx(), |ui| {
            egui::Frame::popup(ui.style())
                .fill(crate::theme::chrome(CHROME_SCALE.tab))
                .stroke(egui::Stroke::new(
                    1.0,
                    crate::theme::chrome(CHROME_SCALE.border),
                ))
                .show(ui, |ui| {
                    // Area 里的 `available_width` 是整屏宽，必须钉死。
                    ui.set_width(TARGET_W);
                    for t in Target::ALL {
                        if menu_row(ui, i18n::t(lang, t.title()), t == current) {
                            picked = Some(t);
                        }
                    }
                });
        });
    if picked.is_some() {
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

/// 多选弹层的一行：会话名 + 是否已勾选。
///
/// 名字与勾选态都来自**本帧的标签列表**（`TerminalWindow::sender_pick_rows`），
/// 这里只管画，不持有任何状态——勾选集合在 `SenderState::picked`。
pub(super) struct PickRow {
    pub name: String,
    pub on: bool,
}

/// `Targets ▸ Selected Sessions` 右边那个多选下拉的弹层。
///
/// 与 [`targets_popup`] 有两处**故意**不同，都是多选的语义要求的：
/// - 点一行**不关闭**弹层：勾三四个目标时，每勾一个关一次壳等于重开四次；
/// - 打勾的是方块不是圆点：圆点在本界面里已经是单选的意思（`Targets` 那五行）。
///
/// 返回本帧被切换的行序号（`None` = 没点）。
pub(super) fn picked_popup(
    ui: &mut egui::Ui,
    anchor: egui::Rect,
    rows: &[PickRow],
    empty: &str,
) -> Option<usize> {
    if !ui.memory(|m| m.is_popup_open(picked_menu_id())) {
        return None;
    }
    let mut toggled = None;
    let area = egui::Area::new(picked_menu_id().with("area"))
        .order(egui::Order::Foreground)
        .fixed_pos(anchor.left_bottom())
        .pivot(egui::Align2::LEFT_TOP)
        // 与 `targets_popup` 同一个坑：`Area` 默认淡入，夹具时钟钉在 0 会停在第一帧。
        .fade_in(false)
        .show(ui.ctx(), |ui| {
            egui::Frame::popup(ui.style())
                .fill(crate::theme::chrome(CHROME_SCALE.tab))
                .stroke(egui::Stroke::new(
                    1.0,
                    crate::theme::chrome(CHROME_SCALE.border),
                ))
                .show(ui, |ui| {
                    // Area 里的 `available_width` 是整屏宽，必须钉死。宽度跟随
                    // 触发它的那一格：弹层比下拉框宽会像两个不相干的东西叠在一起。
                    ui.set_width(anchor.width().max(PICK_POPUP_MIN_W));
                    if rows.is_empty() {
                        // 没有开着的标签：给一句话，而不是一个空壳弹层。
                        ui.add_space(2.0);
                        ui.label(
                            egui::RichText::new(empty.to_string())
                                .color(crate::theme::chrome(CHROME_SCALE.text))
                                .small(),
                        );
                        return;
                    }
                    egui::ScrollArea::vertical()
                        .max_height(PICK_MAX_H)
                        .show(ui, |ui| {
                            for (i, row) in rows.iter().enumerate() {
                                if pick_row(ui, &row.name, row.on) {
                                    toggled = Some(i);
                                }
                            }
                        });
                });
        });
    if toggled.is_some() {
        // 多选：不关弹层，让用户接着勾下一个。
        return toggled;
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

/// 多选弹层里的一行：方块 + 文字。返回是否被点。
fn pick_row(ui: &mut egui::Ui, text: &str, on: bool) -> bool {
    let (rect, resp) = ui.allocate_exact_size(
        egui::vec2(ui.available_width(), ROW_H),
        egui::Sense::click(),
    );
    let p = ui.painter();
    if resp.hovered() {
        p.rect_filled(rect, 0.0, crate::theme::chrome(CHROME_SCALE.hover));
    }
    let k = egui::Rect::from_center_size(
        egui::pos2(rect.left() + 14.0 + RADIO_D / 2.0, rect.center().y),
        egui::vec2(RADIO_D, RADIO_D),
    );
    p.rect_stroke(
        k,
        0.0,
        egui::Stroke::new(1.0, crate::theme::chrome(CHROME_SCALE.border)),
    );
    if on {
        // 勾用两段线画，不排字：与工具行同一理由——不赌字体。
        let c = crate::theme::chrome(CHROME_SCALE.text_strong);
        let st = egui::Stroke::new(1.6, c);
        let a = egui::pos2(k.left() + 3.0, k.center().y);
        let b = egui::pos2(k.center().x - 0.5, k.bottom() - 3.5);
        let c2 = egui::pos2(k.right() - 3.0, k.top() + 3.5);
        p.line_segment([a, b], st);
        p.line_segment([b, c2], st);
    }
    let font = egui::TextStyle::Button.resolve(ui.style());
    let tc = crate::theme::chrome(CHROME_SCALE.text);
    let g = p.layout_no_wrap(text.to_string(), font, tc);
    p.galley(
        egui::pos2(rect.left() + 34.0, rect.center().y - g.size().y / 2.0),
        g,
        tc,
    );
    resp.clicked()
}

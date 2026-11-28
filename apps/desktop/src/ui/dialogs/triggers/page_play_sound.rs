//! `Play sound` 页（视觉基线 `28`）与它的修改弹窗（图 `29`）。
//!
//! # 为什么这一页是「规则表」而不是单条表单
//!
//! 视觉基线 `28` 的骨架与 Highlight Text 的 `Rules` 页同构：左栏列表 + 右侧三列表
//! （`Pattern` / `Description` / `Sound`）+ 表下工具条 + `Modify`。也就是说
//! `Play sound` 类型下可以挂多条规则，每条规则各自匹配自己的文本、各自选声音。
//! 所以本文件按「表 + 弹窗」实现，而不是像 `Run command` 那样一整页单条编辑。

use super::fields::{mono, rgb};
use super::View;
use crate::ui::view::trigger_pages_spec as ps;
use crate::ui::view::trigger_spec as spec;
use crate::ui::view::trigger_spec::color as c;
use crate::ui::*;
use crate::ui::dialogs::triggers::model::TriggerKind;

/// 画整页（表 + 工具条）。
pub(in crate::ui) fn page(win: &mut TerminalWindow, ui: &mut egui::Ui, o: egui::Pos2, view: &mut View) {
    let base = o + egui::vec2(spec::BODY.0, spec::BODY.1);
    let r = egui::Rect::from_min_size(base, egui::vec2(spec::BODY.2, spec::BODY_CONTENT_H));
    let p = ui.painter().clone();
    let head_h = spec::rules::HEAD_H;

    let head = egui::Rect::from_min_size(r.min, egui::vec2(r.width(), head_h));
    p.rect_filled(head, egui::Rounding::ZERO, rgb(c::HEAD));
    // 外框由 `panels::pane_outline` 统一画（视觉基线 `28` 的框底边在工具条之下）。
    let mut x0 = 0.0;
    for (i, name) in ps::play_sound::HEADERS.iter().enumerate() {
        p.text(
            head.min + egui::vec2(x0 + spec::rules::HEAD_PAD.0, spec::rules::HEAD_PAD.1),
            egui::Align2::LEFT_TOP,
            *name,
            mono(super::fields::FS),
            rgb(c::TEXT_2),
        );
        if i > 0 {
            p.line_segment(
                [
                    egui::pos2(head.left() + x0, head.top()),
                    egui::pos2(head.left() + x0, head.bottom()),
                ],
                egui::Stroke::new(0.6, rgb(0x3a3a3a)),
            );
        }
        x0 = spec::rules::COL_X[i];
    }
    p.line_segment(
        [head.left_bottom(), head.right_bottom()],
        egui::Stroke::new(0.6, rgb(0x3a3a3a)),
    );

    let rows: Vec<usize> = win
        .triggers
        .iter()
        .enumerate()
        .filter(|(_, q)| q.kind == TriggerKind::PlaySound)
        .map(|(i, _)| i)
        .collect();
    if rows.is_empty() {
        p.text(
            r.center(),
            egui::Align2::CENTER_CENTER,
            ps::play_sound::EMPTY,
            mono(super::fields::FS),
            rgb(0x555555),
        );
    } else {
        let row_h = 19.5;
        let mut top = r.top() + head_h;
        for i in rows {
            if top + row_h > r.bottom() {
                break;
            }
            let row = egui::Rect::from_min_size(egui::pos2(r.left(), top), egui::vec2(r.width(), row_h));
            let resp = ui.allocate_rect(row, egui::Sense::click());
            if view.sel == Some(i) {
                p.rect_filled(row, egui::Rounding::ZERO, rgb(0x242424));
            }
            let cell = |n: usize, text: &str| {
                let l = if n == 0 { 0.0 } else { spec::rules::COL_X[n - 1] };
                let w = spec::rules::COL_X[n] - l;
                if text.is_empty() {
                    return;
                }
                let cr = egui::Rect::from_min_size(
                    egui::pos2(row.left() + l, row.top()),
                    egui::vec2(w, row_h),
                );
                p.with_clip_rect(cr.shrink2(egui::vec2(4.0, 0.0))).text(
                    egui::pos2(cr.left() + 4.0, cr.center().y),
                    egui::Align2::LEFT_CENTER,
                    text,
                    mono(super::fields::FS),
                    rgb(c::TEXT_2),
                );
            };
            cell(0, &win.triggers[i].pattern);
            cell(1, &win.triggers[i].description);
            cell(2, &win.triggers[i].config.action.sound);
            if resp.clicked() {
                view.sel = Some(i);
                view.kind = TriggerKind::PlaySound;
            }
            if resp.double_clicked() {
                view.sel = Some(i);
                view.ps_dialog = true;
            }
            top += row_h;
        }
    }
}

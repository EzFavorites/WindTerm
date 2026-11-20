//! `Play sound` 的修改弹窗（视觉基线 `29`）：`Match` 组 + `Sound` 组。
//!
//! 从 `popups.rs` 拆出（R9 单文件 500 代码行上限）。三个弹窗同处一个文件时
//! `popups.rs` 到了 594 行：新建 / Highlight / Play sound 各自是独立的模态窗口，
//! 生命周期互不相干，按「一个弹窗一个文件」拆是自然的切法。
//!
//! `OK` 的可用性沿用 Highlight 弹窗那套「改动指纹」判定：未改动时是灰面暗字。

use super::fields::rgb;
use super::popups::{at, dialog_rect, group_title, parent_rect, scrim};
use super::{Open, View};
use crate::ui::view::trigger_spec::color as c;
use crate::ui::*;

/// `Play sound` 的修改弹窗（视觉基线 `29`）：`Match` 组 + `Sound` 组。
///
/// 与 Highlight 弹窗同构，但下半组换成「声音文件名 + 时长 + Test」。`OK` 的
/// 可用性沿用同一套「改动指纹」判定：未改动时是灰面暗字。
pub(in crate::ui) fn play_sound(
    win: &mut TerminalWindow,
    ui: &mut egui::Ui,
    o: egui::Pos2,
    view: &mut View,
) {
    use crate::ui::view::trigger_pages_spec::play_sound::dialog as d;

    scrim(ui, parent_rect(o));
    let r = dialog_rect(o, d::W, d::H);
    let p = ui.painter().clone();
    p.rect_filled(r, egui::Rounding::same(3.0), rgb(c::BG));
    p.rect_stroke(r, egui::Rounding::same(3.0), egui::Stroke::new(1.0, rgb(0x2e2e2e)));

    group_title(&p, r, "Match", d::MATCH_TITLE);
    group_title(&p, r, "Sound", d::SOUND_TITLE);
    box_at(&p, r, d::G1.0, d::G1.1);
    box_at(&p, r, d::G2.0, d::G2.1);

    let sel = view.sel.filter(|i| *i < win.triggers.len());
    let dirty = match sel {
        Some(i) => {
            let cur = ps_fingerprint(&win.triggers[i]);
            match view.ps_base {
                None => {
                    view.ps_base = Some(cur);
                    false
                }
                Some(base) => base != cur,
            }
        }
        None => false,
    };

    // ── Match 组：Pattern（含 flag bar）+ Description ──
    let pat = at(r, d::PATTERN);
    super::fields::label_ul(
        ui,
        r.left() + d::LABEL_X,
        r.top() + d::PATTERN_LABEL_Y,
        d::LABELS[0],
        'P',
    );
    let flag = egui::Rect::from_min_size(
        egui::pos2(pat.left(), pat.bottom()),
        egui::vec2(pat.width(), d::FLAG_H),
    );
    p.rect_filled(pat, egui::Rounding::ZERO, rgb(c::BG));
    p.rect_stroke(
        egui::Rect::from_min_max(pat.min, flag.max),
        egui::Rounding::ZERO,
        egui::Stroke::new(1.0, rgb(0x444444)),
    );
    if let Some(i) = sel {
        let rule = &mut win.triggers[i];
        super::fields::place(
            ui,
            egui::Rect::from_min_max(
                pat.min + egui::vec2(4.0, 3.0),
                egui::pos2(pat.right() - 4.0, flag.top() - 2.0),
            ),
            |ui| {
                ui.add(
                    egui::TextEdit::multiline(&mut rule.pattern)
                        .font(super::fields::mono(super::fields::FS))
                        .text_color(rgb(c::TEXT_2))
                        .frame(false)
                        .desired_width(f32::INFINITY),
                );
            },
        );
        super::fields::flag_bar(
            ui,
            flag,
            &mut rule.config.flags,
            &mut rule.literal,
            &mut rule.case_sensitive,
        );
    }

    let desc = at(r, d::DESC);
    super::fields::label_ul(
        ui,
        r.left() + d::LABEL_X,
        r.top() + d::DESC_LABEL_Y,
        d::LABELS[1],
        'D',
    );
    p.rect_filled(desc, egui::Rounding::ZERO, rgb(c::BG));
    p.rect_stroke(desc, egui::Rounding::ZERO, egui::Stroke::new(1.0, rgb(0x444444)));
    if let Some(i) = sel {
        super::fields::place(ui, desc.shrink2(egui::vec2(5.0, 3.0)), |ui| {
            ui.add(
                egui::TextEdit::singleline(&mut win.triggers[i].description)
                    .font(super::fields::mono(super::fields::FS))
                    .text_color(rgb(c::TEXT_2))
                    .frame(false)
                    .desired_width(f32::INFINITY),
            );
        });
    }

    // ── Sound 组 ──
    super::fields::label_ul(
        ui,
        r.left() + d::SOUND_LABEL.0,
        r.top() + d::SOUND_LABEL.1,
        "Sound Name:",
        'S',
    );
    let (cx, ctop, cw, ch) = d::SOUND_COMBO;
    let combo_r = egui::Rect::from_min_size(r.min + egui::vec2(cx, ctop), egui::vec2(cw, ch));
    let sound = sel
        .map(|i| win.triggers[i].config.action.sound.clone())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "None".to_owned());
    if super::fields::combo(ui, combo_r, &sound, view.open == Some(Open::SoundName)).clicked() {
        view.toggle_at(Open::SoundName, combo_r);
    }
    let secs = sel.map(|i| win.triggers[i].config.action.sound_secs).unwrap_or(0.0);
    p.text(
        r.min + egui::vec2(d::DURATION_X, d::DURATION_Y),
        egui::Align2::LEFT_CENTER,
        format!("Sound Duration: {}", crate::ui::view::trigger_pages_spec::duration_text(secs)),
        super::fields::mono(super::fields::FS),
        rgb(c::TEXT_2),
    );
    // `Test`：同页内那颗按钮（试听未接，见 README 待办）。
    let (bx, bt, bw, bh) = d::TEST_BTN;
    let tr = egui::Rect::from_min_size(r.min + egui::vec2(bx, bt), egui::vec2(bw, bh));
    super::fields::bevel(&p, tr, 0x424242, 0x3c3c3c, 0x292929, 0x232323);
    ui.allocate_rect(tr, egui::Sense::click());
    let tri = egui::pos2(tr.left() + 22.0, tr.center().y);
    p.add(egui::Shape::convex_polygon(
        vec![
            egui::pos2(tri.x - 4.0, tri.y - 5.0),
            egui::pos2(tri.x - 4.0, tri.y + 5.0),
            egui::pos2(tri.x + 5.0, tri.y),
        ],
        rgb(0x2e8b46),
        egui::Stroke::NONE,
    ));
    p.text(
        egui::pos2(tri.x + 10.0, tr.center().y),
        egui::Align2::LEFT_CENTER,
        "Test",
        super::fields::mono(super::fields::FS),
        rgb(c::TEXT_2),
    );

    // 页脚。
    let by = r.min.y + d::BTN_Y;
    let size = egui::vec2(d::BTN_W, d::BTN_H);
    if super::fields::footer_button_as(
        ui,
        egui::Rect::from_min_size(egui::pos2(r.min.x + d::BTN_CANCEL_X, by), size),
        "Cancel",
        super::fields::Btn::Secondary,
    ) {
        view.ps_dialog = false;
        view.ps_base = None;
        view.dismiss();
    }
    let ok = if dirty { super::fields::Btn::Secondary } else { super::fields::Btn::Muted };
    if super::fields::footer_button_as(
        ui,
        egui::Rect::from_min_size(egui::pos2(r.min.x + d::BTN_OK_X, by), size),
        "OK",
        ok,
    ) && dirty
    {
        view.ps_dialog = false;
        view.ps_base = None;
        view.dismiss();
    }
}

/// 弹窗内改动指纹（`OK` 可用性判据）：覆盖 Match 的四个可改点与声音选择。
fn ps_fingerprint(r: &term_app::trigger::Rule) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    r.pattern.hash(&mut h);
    r.description.hash(&mut h);
    r.literal.hash(&mut h);
    r.case_sensitive.hash(&mut h);
    r.config.flags.whole_word.hash(&mut h);
    r.config.action.sound.hash(&mut h);
    h.finish()
}

/// 分组框（`#2e2e2e` + 3 点圆角），几何按调用方给的 top / 高。
fn box_at(p: &egui::Painter, r: egui::Rect, top: f32, h: f32) {
    let gr = egui::Rect::from_min_size(
        r.min + egui::vec2(
            crate::ui::view::trigger_pages_spec::play_sound::dialog::GROUP_X,
            top,
        ),
        egui::vec2(
            crate::ui::view::trigger_pages_spec::play_sound::dialog::GROUP_W,
            h,
        ),
    );
    p.rect_stroke(gr, egui::Rounding::same(3.0), egui::Stroke::new(1.0, rgb(0x2e2e2e)));
}

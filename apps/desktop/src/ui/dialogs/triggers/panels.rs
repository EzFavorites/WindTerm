//! 右栏各视图的绘制：Rules / Sessions / Action / Custom link / Send text。
//!
//! 每个视图都从 [`spec::BODY`] 的原点出发用相对坐标排布，几何常数全在
//! `crate::ui::view::trigger_spec` 里——这里只做「画」和「读点击」。

use super::fields::{mono, rgb, text_w};
use super::style;
use super::View;
use crate::ui::view::trigger_spec as spec;
use crate::ui::view::trigger_spec::color as c;
use crate::ui::*;
use term_core::trigger::{RuleConfig, TriggerKind};

/// `BODY` 左上角（窗口坐标）。
fn origin(o: egui::Pos2) -> egui::Pos2 {
    o + egui::vec2(spec::BODY.0, spec::BODY.1)
}

fn body_rect(o: egui::Pos2) -> egui::Rect {
    egui::Rect::from_min_size(
        origin(o),
        egui::vec2(spec::BODY.2, spec::BODY_CONTENT_H),
    )
}

/// 右栏内容区的外框：`BODY` 整块（含工具条那一带）。
///
/// 视觉基线 `21` / `23` / `26` / `28` / `30` 的框都是**整块 `BODY`**——底边落在
/// 两条工具条之下（`520.8`），而不是表格单元格的下沿（`500.4`）。早期把框高
/// 写成 [`spec::BODY_CONTENT_H`]，框在工具条上沿就断了，工具栏成了框外的浮条。
///
/// 统一由本函数画，页面里不再各自 `rect_stroke`——否则会出现「表框 + 外框」
/// 两条线重叠。规则表页的单元格下沿由 `strip::strips` 的那条横线提供。
fn pane_outline(ui: &egui::Ui, o: egui::Pos2) {
    let r = egui::Rect::from_min_size(
        origin(o),
        egui::vec2(spec::BODY.2, spec::BODY.3),
    );
    ui.painter()
        .rect_stroke(r, egui::Rounding::ZERO, egui::Stroke::new(0.6, rgb(c::FIELD_LINE)));
}

/// 画整块右栏内容。
pub(in crate::ui) fn body(
    win: &mut TerminalWindow,
    ui: &mut egui::Ui,
    o: egui::Pos2,
    view: &mut View,
) {
    // `Sessions` 页是两张独立列表框，没有整块外框（视觉基线 `02`）。
    if view.tab == 1 {
        sessions(win, ui, o, view);
        return;
    }
    if view.kind == TriggerKind::PlaySound {
        super::page_play_sound::page(win, ui, o, view);
    } else {
        let idx = view.sel.filter(|i| *i < win.triggers.len());
        // 没有选中项时用一份草稿渲染，不让右栏空白（视觉基线打开就有内容）。
        let mut scratch = RuleStub::new(view.kind);
        let rule: &mut term_app::trigger::Rule = match idx {
            Some(i) => &mut win.triggers[i],
            None => &mut scratch.rule,
        };
        match view.kind {
            TriggerKind::HighlightText => rules(win, ui, o, view),
            TriggerKind::CustomLink => super::edit::custom(ui, o, view, rule),
            TriggerKind::CustomMenu => super::page_custom_menu::page(ui, o, view, rule),
            TriggerKind::SendText => super::edit::send(ui, o, view, rule),
            TriggerKind::ShowTooltip => super::page_tooltip::page(ui, o, view, rule),
            TriggerKind::RunCommand => super::page_run_command::page(ui, o, view, rule),
            _ => super::page_action::action_page(ui, o, view, rule),
        }
    }
    // 外框最后画：盖住表头底色与工具条，四边闭合成一个矩形。
    pane_outline(ui, o);
}

/// 无选中项时的草稿规则（页面照常可编辑，但不落库）。
struct RuleStub {
    rule: term_app::trigger::Rule,
}

impl RuleStub {
    fn new(kind: TriggerKind) -> Self {
        Self {
            rule: term_app::trigger::Rule {
            id: String::new(),
            name: String::new(),
            description: String::new(),
            kind,
            config: RuleConfig::default(),
            on: kind.trigger_on(),
            pattern: String::new(),
            literal: true,
            case_sensitive: true,
            cooldown_secs: 0,
            idle_secs: 0,
                actions: Vec::new(),
                enabled: true,
            },
        }
    }
}

// ── Rules ────────────────────────────────────────────────────────────────

/// `Highlight Text` 的规则表：表头三列 + 该类型的每条规则一行。
fn rules(win: &mut TerminalWindow, ui: &mut egui::Ui, o: egui::Pos2, view: &mut View) {
    let base = origin(o);
    let r = body_rect(o);
    let p = ui.painter().clone();
    let head_h = spec::rules::HEAD_H;

    let head = egui::Rect::from_min_size(r.min, egui::vec2(r.width(), head_h));
    p.rect_filled(head, egui::Rounding::ZERO, rgb(c::HEAD));
    // 表格外框由 `pane_outline` 统一画；这里只补单元格下沿（工具条上沿那条线
    // 由 `strip::strips` 画），否则外框会与表框重叠成双线。

    let mut x0 = 0.0;
    for (i, name) in spec::rules::HEADERS.iter().enumerate() {
        let x1 = spec::rules::COL_X[i];
        p.text(
            head.min + egui::vec2(x0 + spec::rules::HEAD_PAD.0, spec::rules::HEAD_PAD.1),
            egui::Align2::LEFT_TOP,
            *name,
            mono(crate::ui::type_scale::SIZE_SMALL),
            rgb(c::TEXT_2),
        );
        if i > 0 {
            p.line_segment(
                [egui::pos2(head.left() + x0, head.top()), egui::pos2(head.left() + x0, head.bottom())],
                egui::Stroke::new(0.6, rgb(0x3a3a3a)),
            );
        }
        x0 = x1;
    }
    p.line_segment(
        [head.left_bottom(), head.right_bottom()],
        egui::Stroke::new(0.6, rgb(0x3a3a3a)),
    );

    let rows: Vec<usize> = win
        .triggers
        .iter()
        .enumerate()
        .filter(|(_, q)| q.kind == TriggerKind::HighlightText)
        .map(|(i, _)| i)
        .collect();
    if rows.is_empty() {
        p.text(
            egui::pos2(r.center().x, r.center().y),
            egui::Align2::CENTER_CENTER,
            spec::rules::EMPTY,
            mono(crate::ui::type_scale::SIZE_SMALL),
            rgb(0x555555),
        );
        return;
    }
    let row_h = 19.5;
    let mut top = r.top() + head_h;
    for i in rows {
        if top + row_h > r.bottom() {
            break;
        }
        let row = egui::Rect::from_min_size(
            egui::pos2(r.left(), top),
            egui::vec2(r.width(), row_h),
        );
        if view.sel == Some(i) {
            p.rect_filled(row, egui::Rounding::ZERO, rgb(0x242424));
        }
        let cw = |n: usize| -> (f32, f32) {
            let l = if n == 0 { 0.0 } else { spec::rules::COL_X[n - 1] };
            let rr = spec::rules::COL_X[n];
            (l, rr - l)
        };
        // 视觉基线 `01` / `13` 里三列都是**只读展示**：编辑入口是下面工具条的
        // `Modify`（弹出图 `11` 那个设置弹窗），表内不摆输入框。
        let cell = |n: usize, text: &str| {
            let (lx, lw) = cw(n);
            let cr = egui::Rect::from_min_size(
                egui::pos2(row.left() + lx, row.top()),
                egui::vec2(lw, row_h),
            );
            if !text.is_empty() {
                let cp = p.with_clip_rect(cr.shrink2(egui::vec2(4.0, 0.0)));
                cp.text(
                    egui::pos2(cr.left() + 4.0, cr.center().y),
                    egui::Align2::LEFT_CENTER,
                    text,
                    mono(crate::ui::type_scale::SIZE_SMALL),
                    rgb(c::TEXT_2),
                );
            }
        };
        cell(0, &win.triggers[i].pattern);
        cell(1, &win.triggers[i].description);
        // Style 列：色块 + `Aa` + 线型样例。
        let (lx, lw) = cw(2);
        let sr = egui::Rect::from_min_size(
            egui::pos2(row.left() + lx, row.top()),
            egui::vec2(lw, row_h),
        );
        style::cell_sample(&p, sr, &win.triggers[i].config.mark);
        let resp = ui.allocate_rect(row, egui::Sense::click());
        if resp.clicked() || resp.double_clicked() {
            view.sel = Some(i);
            view.kind = TriggerKind::HighlightText;
        }
        if resp.double_clicked() {
            view.hl_dialog = true;
            view.hl_base = None;
        }
        top += row_h;
    }
    let _ = base;
}

// ── Sessions ─────────────────────────────────────────────────────────────

/// `Sessions` 页：两个带开关的列表 + 各自工具条。
fn sessions(win: &mut TerminalWindow, ui: &mut egui::Ui, o: egui::Pos2, view: &mut View) {
    let r = body_rect(o);
    let p = ui.painter().clone();
    let list_a = egui::Rect::from_min_size(
        r.min,
        egui::vec2(r.width(), spec::sessions::LIST_A.1),
    );
    // 视觉基线 `02`：框内**没有**标题，`Target Systems:` 只出现在框上方（由
    // `list::label_row` 画），行直接从框顶开始。
    p.rect_stroke(list_a, egui::Rounding::ZERO, egui::Stroke::new(0.6, rgb(c::FIELD_LINE)));
    let sel = view.sel.filter(|i| *i < win.triggers.len());
    for (k, name) in spec::sessions::SYSTEMS.iter().enumerate() {
        let y = list_a.top() + 1.0 + k as f32 * spec::sessions::ROW_A_H;
        let row = egui::Rect::from_min_size(
            egui::pos2(list_a.left(), y),
            egui::vec2(list_a.width(), spec::sessions::ROW_A_H),
        );
        let on = sel
            .map(|i| win.triggers[i].config.targets.systems.iter().any(|s| s == name))
            .unwrap_or(false);
        let mut val = on;
        let hit = egui::Rect::from_min_size(
            egui::pos2(row.right() - 30.0, row.center().y - 6.0),
            egui::vec2(24.0, 12.0),
        );
        if super::fields::toggle(ui, hit, &mut val) {
            if let Some(i) = sel {
                let list = &mut win.triggers[i].config.targets.systems;
                if val {
                    list.push((*name).to_owned());
                } else {
                    list.retain(|s| s != name);
                }
            }
        }
        p.text(
            egui::pos2(row.left() + 6.0, row.center().y),
            egui::Align2::LEFT_CENTER,
            *name,
            mono(crate::ui::type_scale::SIZE_SMALL),
            rgb(c::TEXT_2),
        );
    }
    // 工具条：搜索 + Check All / Uncheck All。
    tools_row(ui, &p, list_a, "sessions_sys", view, sel, &mut |w, all| {
        if let Some(i) = sel {
            if w {
                win.triggers[i].config.targets.systems =
                    spec::sessions::SYSTEMS.iter().map(|s| (*s).to_owned()).collect();
            } else if all {
                win.triggers[i].config.targets.systems.clear();
            }
        }
    });

    let label_b = egui::pos2(r.left(), r.top() + spec::sessions::LABEL_B_TOP);
    p.text(
        label_b,
        egui::Align2::LEFT_TOP,
        spec::sessions::LABEL_B,
        mono(crate::ui::type_scale::SIZE_SMALL),
        rgb(c::TEXT_2),
    );
    let list_b = egui::Rect::from_min_size(
        egui::pos2(r.left(), r.top() + spec::sessions::LIST_B_TOP),
        egui::vec2(r.width(), spec::sessions::LIST_B_BOTTOM - spec::sessions::LIST_B_TOP),
    );
    p.rect_stroke(list_b, egui::Rounding::ZERO, egui::Stroke::new(0.6, rgb(c::FIELD_LINE)));
    let row_h = spec::sessions::ROW_H;
    for (k, name) in spec::sessions::SESSIONS.iter().enumerate() {
        let y = list_b.top() + 2.0 + k as f32 * row_h;
        let row = egui::Rect::from_min_size(
            egui::pos2(list_b.left(), y),
            egui::vec2(list_b.width(), row_h),
        );
        // 视觉基线 `02`：首行（bash）有底色且文字提亮，行首有类别图标。
        if k == 0 {
            p.rect_filled(row, egui::Rounding::ZERO, rgb(0x2a2a2a));
        }
        session_icon(&p, row.left() + 7.0, row.center().y, name);
        let on = sel
            .map(|i| win.triggers[i].config.targets.sessions.iter().any(|s| s == name))
            .unwrap_or(false);
        let mut val = on;
        let hit = egui::Rect::from_min_size(
            egui::pos2(row.right() - 30.0, row.top() + 0.5),
            egui::vec2(24.0, 11.0),
        );
        if super::fields::toggle(ui, hit, &mut val) {
            if let Some(i) = sel {
                let list = &mut win.triggers[i].config.targets.sessions;
                if val {
                    list.push((*name).to_owned());
                } else {
                    list.retain(|s| s != name);
                }
            }
        }
        p.text(
            egui::pos2(row.left() + 20.0, row.center().y),
            egui::Align2::LEFT_CENTER,
            *name,
            mono(crate::ui::type_scale::SIZE_SMALL),
            rgb(if k == 0 { c::TEXT } else { c::TEXT_2 }),
        );
    }
    tools_row(ui, &p, list_b, "sessions_sess", view, sel, &mut |w, all| {
        if let Some(i) = sel {
            if w {
                win.triggers[i].config.targets.sessions =
                    spec::sessions::SESSIONS.iter().map(|s| (*s).to_owned()).collect();
            } else if all {
                win.triggers[i].config.targets.sessions.clear();
            }
        }
    });
    let _ = view;
}

/// 列表下方的「搜索框 + Check All / Uncheck All」。
fn tools_row(
    ui: &mut egui::Ui,
    p: &egui::Painter,
    list: egui::Rect,
    salt: &str,
    _view: &View,
    _sel: Option<usize>,
    apply: &mut dyn FnMut(bool, bool),
) {
    let h = spec::sessions::TOOLS_H;
    let bar = egui::Rect::from_min_size(
        egui::pos2(list.left(), list.bottom() - h),
        egui::vec2(list.width(), h),
    );
    p.line_segment(
        [bar.left_top(), bar.right_top()],
        egui::Stroke::new(0.6, rgb(c::FIELD_LINE)),
    );
    // 放大镜。
    p.circle_stroke(
        egui::pos2(bar.left() + 9.0, bar.center().y),
        3.0,
        egui::Stroke::new(1.0, rgb(c::TEXT_3)),
    );
    p.line_segment(
        [
            egui::pos2(bar.left() + 11.0, bar.center().y + 2.0),
            egui::pos2(bar.left() + 14.0, bar.center().y + 5.0),
        ],
        egui::Stroke::new(1.0, rgb(c::TEXT_3)),
    );
    let mut rx = bar.right() - 6.0;
    for (label, all) in [(spec::sessions::UNCHECK_ALL, true), (spec::sessions::CHECK_ALL, false)] {
        let w = text_w(label);
        rx -= w;
        let r = egui::Rect::from_min_size(egui::pos2(rx, bar.top()), egui::vec2(w, h));
        let resp = ui.allocate_rect(r, egui::Sense::click());
        p.text(
            egui::pos2(r.left(), r.center().y),
            egui::Align2::LEFT_CENTER,
            label,
            mono(crate::ui::type_scale::SIZE_SMALL),
            rgb(c::TEXT_2),
        );
        if resp.clicked() {
            apply(!all, all);
        }
        rx -= 12.0;
    }
    ui.push_id(salt, |_| {});
}

// ── Custom link ──────────────────────────────────────────────────────────


/// 会话行首的小图标（视觉基线 `02` 是 shell 类别符号，尺寸只有 7 点）。
///
/// 用矢量画而不是字形：等宽族不一定含 `⌘` / `⚡`，缺字会变成豆腐块。
fn session_icon(p: &egui::Painter, cx: f32, cy: f32, name: &str) {
    let lit = rgb(0x9a9a9a);
    let line = rgb(c::TEXT_3);
    if name.contains("debug-console") {
        let r = egui::Rect::from_center_size(egui::pos2(cx, cy), egui::vec2(6.0, 6.0));
        p.rect_filled(r, egui::Rounding::ZERO, lit);
        return;
    }
    if name.ends_with("bash") {
        // 菱形（实心）。
        p.add(egui::Shape::convex_polygon(
            vec![
                egui::pos2(cx, cy - 4.0),
                egui::pos2(cx + 4.0, cy),
                egui::pos2(cx, cy + 4.0),
                egui::pos2(cx - 4.0, cy),
            ],
            lit,
            egui::Stroke::NONE,
        ));
        return;
    }
    // 其余 shell 都是一个 `⌘`：圆角方框 + 十字。视觉基线 `02` 图标列逐行放大后，
    // `sh` 换成「方框里两道交叉箭头」，`csh`/`dash`/`ksh`/`tcsh`/`zsh` 全是 `⌘`。
    let box_r = egui::Rect::from_center_size(egui::pos2(cx, cy), egui::vec2(7.0, 7.0));
    p.rect_stroke(box_r, egui::Rounding::same(1.5), egui::Stroke::new(0.8, line));
    if name.ends_with("/ sh") {
        p.line_segment(
            [egui::pos2(cx - 2.0, cy + 2.0), egui::pos2(cx + 2.0, cy - 2.0)],
            egui::Stroke::new(0.9, line),
        );
        p.line_segment(
            [egui::pos2(cx - 2.0, cy - 2.0), egui::pos2(cx + 2.0, cy + 2.0)],
            egui::Stroke::new(0.9, line),
        );
        return;
    }
    p.line_segment(
        [egui::pos2(cx - 3.0, cy), egui::pos2(cx + 3.0, cy)],
        egui::Stroke::new(0.8, line),
    );
    p.line_segment(
        [egui::pos2(cx, cy - 3.0), egui::pos2(cx, cy + 3.0)],
        egui::Stroke::new(0.8, line),
    );
}

//! `Custom link` 与 `Send text` 两个页面的绘制。
//!
//! 从 `panels.rs` 拆出（单文件 500 行上限）。两者共用「Match 框 + flag bar」
//! 那段（[`match_box`]），差别在下半页。

use super::fields::{combo, field, flag_bar, label, label_ul, mono, multiline, rgb, text_w};
use super::{sync_actions, Open, View};
use crate::ui::view::trigger_spec as spec;
use crate::ui::view::trigger_spec::color as c;
use crate::ui::*;

fn origin(o: egui::Pos2) -> egui::Pos2 {
    o + egui::vec2(spec::BODY.0, spec::BODY.1)
}

/// Custom link 页面：Match + Action + 四个分支的字段区 + Mark Style/Color。
///
/// 视觉基线 `10` 只给了 `Action = Run command` 那一态；另外三项（`Send text` /
/// `Open hyperlink` / `Play sound`）没有对应视觉基线，字段区按「本页 Mark 行
/// 之前的可用高度」铺开，与事件页的分支共用同一套绘制（`page_action::fields`）。
pub(super) fn custom(ui: &mut egui::Ui, o: egui::Pos2, view: &mut View, rule: &mut term_app::trigger::Rule) {
    let base = origin(o);
    // 夹具专用：参考图 `10` / `23` 的 Match 框是聚焦蓝边，Action 下拉则是普通态。
    let match_focus = std::env::var_os("WINDTERM_TRIG_FOCUS_MATCH").is_some();
    match_box(ui, base, rule, spec::custom::MATCH_H, match_focus);
    let combo_top = spec::custom::MATCH_H + spec::custom::FLAG_H + 8.5;
    label_ul(ui, base.x, base.y + combo_top + 4.0, "Action:", 'A');
    let cr = egui::Rect::from_min_size(
        base + egui::vec2(0.0, combo_top + 19.0),
        egui::vec2(spec::BODY.2, spec::custom::COMBO_H),
    );
    if combo(ui, cr, rule.config.action.kind.label(), false).clicked() {
        view.toggle_at(Open::Action, cr);
    }

    let layout = super::page_action::OsLayout {
        tabs_top: spec::custom::OS_TABS_TOP,
        tabs_h: spec::custom::OS_TABS_H,
        rows: spec::custom::ROWS,
        left: spec::custom::FIELD_LEFT,
        width: spec::custom::FIELD_W,
        height: spec::custom::FIELD_H,
        labels: spec::action::LABELS,
        hide_y: None,
    };
    // 多行字段区：从 Action 下拉底（234.5）起，到两行 Mark（405）之前。
    super::page_action::fields(ui, base, view, rule, &layout, (241.0, 400.0), (241.0, 400.0));

    // Mark Style / Mark Color。
    let style = rule.config.mark.mark_style.label();
    mark_row(
        ui,
        base,
        MarkRow {
            top: spec::custom::MARK_STYLE_TOP,
            name: "Mark Style:",
            value: style,
            swatch: None,
            open: Open::LineStyle,
        },
        view,
    );
    let color_text = color_label(&rule.config.mark.mark_color);
    let swatch = super::style::color_of(&rule.config.mark.mark_color);
    mark_row(
        ui,
        base,
        MarkRow {
            top: spec::custom::MARK_COLOR_TOP,
            name: "Mark Color:",
            value: &color_text,
            swatch: Some(swatch),
            open: Open::MarkColor,
        },
        view,
    );
    sync_actions(rule);
}

/// `Mark Style` / `Mark Color` 一行的内容。
///
/// 收成一个结构体而不是继续摊参数：这一行有五个「描述它长什么样」的字段，
/// 摊平后 `mark_row` 会到 8 个参数（`clippy::too_many_arguments` 的上限是 7）。
pub(super) struct MarkRow<'a> {
    /// 行顶（相对 `BODY`）。
    pub top: f32,
    /// 左侧标签。
    pub name: &'a str,
    /// 下拉当前值的文本。
    pub value: &'a str,
    /// `Some(色)` 时下拉左侧画 8×8 色块（`Mark Color` 行），`None` 不画。
    pub swatch: Option<Option<u32>>,
    /// 点开时展开的浮层。
    pub open: Open,
}

/// `Mark Style` / `Mark Color` 一行：左标签 + 右下拉。
///
/// `Mark Color` 在视觉基线 `10` / `23` / `30` 里都带一个 8×8 色块，所以色值走
/// [`fields::color_combo`]；`Mark Style` 没有色块，传 `None`。
pub(super) fn mark_row(ui: &mut egui::Ui, base: egui::Pos2, row: MarkRow<'_>, view: &mut View) {
    let MarkRow { top, name, value, swatch, open } = row;
    label(ui, base + egui::vec2(0.0, top + 5.0), name);
    let r = egui::Rect::from_min_size(
        base + egui::vec2(spec::custom::MARK_COMBO_LEFT, top),
        egui::vec2(spec::BODY.2 - spec::custom::MARK_COMBO_LEFT, spec::custom::MARK_COMBO_H),
    );
    if super::fields::color_combo(ui, r, value, swatch, view.open == Some(open), true).clicked() {
        view.toggle_at(open, r);
    }
}

/// 颜色选择的显示文本。
pub(super) fn color_label(choice: &term_core::trigger::ColorChoice) -> String {
    use term_core::trigger::ColorChoice;
    match choice {
        ColorChoice::Empty => "Empty".to_owned(),
        ColorChoice::Ansi(s) => s.clone(),
        ColorChoice::Custom(s) => s.clone(),
    }
}

/// Match 多行框 + 底部 flag bar。
///
/// `focused` 只影响多行框自己的描边（`#007cd2` 蓝边 vs 灰边）：视觉基线 `23` /
/// `26` / `30` 的 Match 框都是蓝边（打开页面时焦点就在这个空框里），而 `10` /
/// `15` 的 Match 框是灰边。flag bar 始终灰边——视觉基线里蓝框只包住文本框，
/// 下面那条 flag 带是独立的窄框。
pub(super) fn match_box(
    ui: &mut egui::Ui,
    base: egui::Pos2,
    rule: &mut term_app::trigger::Rule,
    h: f32,
    focused: bool,
) {
    let r = egui::Rect::from_min_size(base, egui::vec2(spec::BODY.2, h));
    multiline(ui, r, &mut rule.pattern, "", focused);
    let flag = egui::Rect::from_min_size(
        egui::pos2(r.left(), r.bottom()),
        egui::vec2(r.width(), spec::custom::FLAG_H),
    );
    ui.painter().rect_stroke(
        flag,
        egui::Rounding::ZERO,
        egui::Stroke::new(0.6, rgb(c::FIELD_LINE)),
    );
    flag_bar(
        ui,
        flag,
        &mut rule.config.flags,
        &mut rule.literal,
        &mut rule.case_sensitive,
    );
}

// ── Send text ────────────────────────────────────────────────────────────

/// Script 模式的占位脚本（视觉基线 `18-script-tab.png` 的位置不变）。
///
/// 后端只支持 `return "字面量"`（先展开 `${match}`）或纯文本；JS 箭头函数会被
/// 原样当成文本发出去，所以占位必须用真能跑的写法。
const SCRIPT_HINT: &str = "E.g.:\nreturn \"echo ${match}\"";

/// `Send Text` 页面：Match + Send + Text/Script + Condition + Later + Captures。
pub(super) fn send(ui: &mut egui::Ui, o: egui::Pos2, view: &mut View, rule: &mut term_app::trigger::Rule) {
    let base = origin(o);
    let p = ui.painter().clone();
    match_box(ui, base, rule, spec::send::MATCH_H, false);

    label(ui, base + egui::vec2(0.0, spec::send::SEND_TOP - 15.0), spec::send::LABELS[1]);
    let send_r = egui::Rect::from_min_size(
        base + egui::vec2(0.0, spec::send::SEND_TOP),
        egui::vec2(spec::BODY.2, spec::send::SEND_H),
    );
    if view.script {
        // 视觉基线 `18`：Script 模式给一段占位脚本，不是空白框。
        multiline(ui, send_r, &mut rule.config.send.text, SCRIPT_HINT, true);
    } else {
        // 文本模式：带占位示例的两行提示。
        multiline(
            ui,
            send_r,
            &mut rule.config.send.text,
            &format!("{}\n{}", spec::send::SEND_HINT[0], spec::send::SEND_HINT[1]),
            true,
        );
    }
    // Text / Script 选项卡 + 右侧 Captures。
    let tabs_top = spec::send::SEND_TOP + spec::send::SEND_H;
    let mut x = 0.0;
    for (i, name) in spec::send::MODES.iter().enumerate() {
        let w = text_w(name) + 16.0;
        let r = egui::Rect::from_min_size(
            base + egui::vec2(x, tabs_top),
            egui::vec2(w, spec::send::MODE_TABS_H),
        );
        let active = view.script == (i == 1);
        p.rect_filled(r, egui::Rounding::ZERO, rgb(if active { 0x3a3a3a } else { c::BG }));
        p.rect_stroke(r, egui::Rounding::ZERO, egui::Stroke::new(0.6, rgb(c::LINE)));
        p.text(r.center(), egui::Align2::CENTER_CENTER, *name, mono(crate::ui::type_scale::SIZE_SMALL), rgb(if active { c::TEXT } else { c::TEXT_2 }));
        if ui.allocate_rect(r, egui::Sense::click()).clicked() {
            view.script = i == 1;
            rule.config.send.mode = if i == 1 {
                term_core::trigger::SendMode::Script
            } else {
                term_core::trigger::SendMode::Text
            };
        }
        x += w + 2.0;
    }
    // Text 模式右侧只是一个 `Captures` 文字标签（视觉基线 `15`）；Script 模式
    // 换成蓝底的值下拉，显示 `captures.match`（视觉基线 `18`）。
    let cap = if view.script {
        // 视觉基线 `18` 实测 137×26，比 Text 模式的 `Captures` 标签宽得多。
        let r = egui::Rect::from_min_size(
            base + egui::vec2(spec::BODY.2 - 137.0, tabs_top - 3.0),
            egui::vec2(137.0, spec::send::MODE_TABS_H + 4.0),
        );
        p.rect_filled(r, egui::Rounding::ZERO, rgb(c::BLUE_TRACK));
        p.text(
            r.left_center() + egui::vec2(7.0, 0.0),
            egui::Align2::LEFT_CENTER,
            "captures.match",
            mono(crate::ui::type_scale::SIZE_SMALL),
            rgb(c::TEXT),
        );
        r
    } else {
        let r = egui::Rect::from_min_size(
            base + egui::vec2(spec::BODY.2 - spec::send::CAPTURES_W, tabs_top),
            egui::vec2(spec::send::CAPTURES_W, spec::send::MODE_TABS_H),
        );
        p.text(
            egui::pos2(r.right(), r.center().y),
            egui::Align2::RIGHT_CENTER,
            "Captures",
            mono(crate::ui::type_scale::SIZE_SMALL),
            rgb(c::TEXT_2),
        );
        r
    };
    if ui.allocate_rect(cap, egui::Sense::click()).clicked() {
        view.toggle_at(Open::Captures, cap);
    }

    // Send Condition 行。
    let cond = egui::Rect::from_min_size(
        base + egui::vec2(0.0, spec::send::COND_TOP),
        egui::vec2(spec::send::FIELD_LEFT - 4.0, spec::send::ROW_H),
    );
    p.text(
        cond.right_top(),
        egui::Align2::RIGHT_TOP,
        spec::send::LABELS[2],
        mono(crate::ui::type_scale::SIZE_SMALL),
        rgb(c::TEXT_2),
    );
    let cond_f = egui::Rect::from_min_size(
        base + egui::vec2(spec::send::FIELD_LEFT, spec::send::COND_TOP - 2.0),
        egui::vec2(spec::send::FIELD_W, spec::send::ROW_H),
    );
    if combo(ui, cond_f, rule.config.send.condition.label(), view.open == Some(Open::SendCondition)).clicked() {
        view.toggle_at(Open::SendCondition, cond_f);
    }

    // Send Later 行（带上下箭头）。
    let later_l = egui::Rect::from_min_size(
        base + egui::vec2(0.0, spec::send::LATER_TOP),
        egui::vec2(spec::send::FIELD_LEFT - 4.0, spec::send::ROW_H),
    );
    p.text(
        later_l.right_top(),
        egui::Align2::RIGHT_TOP,
        spec::send::LABELS[3],
        mono(crate::ui::type_scale::SIZE_SMALL),
        rgb(c::TEXT_2),
    );
    let later_f = egui::Rect::from_min_size(
        base + egui::vec2(spec::send::FIELD_LEFT, spec::send::LATER_TOP - 2.0),
        egui::vec2(spec::send::FIELD_W, spec::send::ROW_H),
    );
    let mut text = format!("{}s", rule.config.send.later_secs);
    if field(ui, later_f, &mut text, "0s", false) {
        rule.config.send.later_secs = text.trim_end_matches('s').to_owned();
        // 0 / 空 = 不延迟，Send Condition 生效；正数则覆盖条件。
    }
    // 右侧上下小箭头。
    let ax = later_f.right() - 8.0;
    for (dy, up) in [(-3.0f32, true), (3.0, false)] {
        let cy = later_f.center().y + dy;
        p.add(egui::Shape::closed_line(
            vec![
                egui::pos2(ax - 3.0, cy + if up { 1.5 } else { -1.5 }),
                egui::pos2(ax + 3.0, cy + if up { 1.5 } else { -1.5 }),
                egui::pos2(ax, cy + if up { -1.5 } else { 1.5 }),
            ],
            egui::Stroke::new(1.0, rgb(c::TEXT_3)),
        ));
    }
    sync_actions(rule);
}

#[cfg(test)]
mod tests {
    use super::SCRIPT_HINT;

    #[test]
    fn the_script_hint_uses_syntax_the_trigger_backend_actually_evaluates() {
        // 占位若写 JS 箭头函数，用户照抄后会被原样当文本发出去——那是「写了跑不起来」。
        assert!(!SCRIPT_HINT.contains("=>"), "占位不能是 JS 箭头函数：{SCRIPT_HINT}");
        let body = SCRIPT_HINT.lines().nth(1).expect("第二行是示例表达式");
        assert_eq!(term_core::trigger_script::eval(body), "echo ${match}");
    }
}


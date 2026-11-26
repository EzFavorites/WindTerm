//! `Action` 下拉的四个字段区分支（视觉基线 `19` / `20` / `21`）与四类事件页。
//!
//! # 为什么四个分支必须分开画
//!
//! 视觉基线证明 `Action` 不是「同一个区块换标题」：选 `Send text` 时下方是一整块
//! 多行文本加 `Text`/`Script` 选项卡；选 `Open hyperlink` 时是同样大小的框但占位
//! 是 URL；选 `Play sound` 时变成一个分组框，里面是 `Sound Name` 下拉、时长与
//! `Test`。旧实现只画 `Run command` 的三行命令字段，选另外三项等于「下拉变了、
//! 字段没变」——这是本次修的主缺陷。

use super::fields::{bevel, combo, field, label, label_ul, mnemonic_at, mono, multiline, rgb};
use super::fields::{text_w, toggle, FS};
use super::{sync_actions, Open, View};
use crate::ui::view::trigger_pages_spec as ps;
use crate::ui::view::trigger_spec as spec;
use crate::ui::view::trigger_spec::color as c;
use crate::ui::*;
use term_core::trigger::{ActionKind, SendMode};

/// `BODY` 左上角（窗口坐标）。
fn origin(o: egui::Pos2) -> egui::Pos2 {
    o + egui::vec2(spec::BODY.0, spec::BODY.1)
}

/// `Action` 下拉本身（事件页与 Custom link 页共用）。
pub(super) fn action_combo(
    ui: &mut egui::Ui,
    base: egui::Pos2,
    view: &mut View,
    kind: ActionKind,
) -> egui::Rect {
    let (_, top, h) = spec::action::COMBO;
    let r = egui::Rect::from_min_size(base + egui::vec2(0.0, top), egui::vec2(spec::BODY.2, h));
    // 参考图 `02-trigger/04`（After connection）里 Action 下拉未展开时也是蓝色
    // 聚焦框——它是进入页面后的默认焦点控件，不是只有菜单展开才亮。
    if combo(ui, r, kind.label(), true).clicked() {
        view.toggle_at(Open::Action, r);
    }
    r
}

/// 事件页（`After connection` 等四种）整页。
pub(in crate::ui) fn action_page(
    ui: &mut egui::Ui,
    o: egui::Pos2,
    view: &mut View,
    rule: &mut term_app::trigger::Rule,
) {
    let base = origin(o);
    action_combo(ui, base, view, rule.config.action.kind);
    let layout = OsLayout {
        tabs_top: spec::action::OS_TABS_TOP,
        tabs_h: spec::action::OS_TABS_H,
        rows: spec::action::ROWS.map(|(t, _)| t),
        left: spec::action::FIELD_LEFT,
        width: spec::action::FIELD_W,
        height: 21.0,
        labels: spec::action::LABELS,
        hide_y: None,
    };
    fields(
        ui,
        base,
        view,
        rule,
        &layout,
        (ps::action_extra::FIELD_TOP, ps::action_extra::MODE_TABS_TOP + ps::action_extra::MODE_TABS_H),
        (ps::action_extra::SOUND_GROUP_TOP, ps::action_extra::SOUND_GROUP_BOTTOM),
    );
    sync_actions(rule);
}

/// 分 OS 三行字段的几何。每个页面一套，不做「统一推算」——视觉基线里三处的
/// 行距与起点都不同（事件页 66/96/126、Custom link 263/293/323、
/// Run command 295/325/355）。
pub(super) struct OsLayout {
    pub tabs_top: f32,
    pub tabs_h: f32,
    pub rows: [f32; 3],
    pub left: f32,
    pub width: f32,
    pub height: f32,
    pub labels: [&'static str; 3],
    /// `Hide console window` 那一行的 top（只在 Windows 选项卡下画）。
    pub hide_y: Option<f32>,
}

/// 字段区总入口：按 `Action` 取值分派。
///
/// `text` / `sound` 是 `Send text`·`Open hyperlink` 与 `Play sound` 两个分支的
/// 上下边界——三种版式的高度不同，由调用页面各自决定。
pub(super) fn fields(
    ui: &mut egui::Ui,
    base: egui::Pos2,
    view: &mut View,
    rule: &mut term_app::trigger::Rule,
    os: &OsLayout,
    text: (f32, f32),
    sound: (f32, f32),
) {
    match rule.config.action.kind {
        ActionKind::RunCommand => os_rows(ui, base, view, rule, os),
        ActionKind::PlaySound => sound_block(ui, base, sound.0, sound.1, view, rule),
        ActionKind::SendText => {
            let hint = format!("{}\n{}", ps::action_extra::SEND_HINT[0], ps::action_extra::SEND_HINT[1]);
            let (v, m) = (&mut rule.config.send.text, &mut rule.config.send.mode);
            text_block(ui, base, text.0, text.1, v, m, &hint);
        }
        ActionKind::OpenHyperlink => {
            let hint = format!(
                "{}\n{}",
                ps::action_extra::HYPERLINK_HINT[0],
                ps::action_extra::HYPERLINK_HINT[1]
            );
            let (v, m) = (&mut rule.config.action.hyperlink, &mut rule.config.action.mode);
            text_block(ui, base, text.0, text.1, v, m, &hint);
        }
    }
}

/// 分 OS 三行字段 + （Windows 下）`Hide console window` 开关。
pub(super) fn os_rows(
    ui: &mut egui::Ui,
    base: egui::Pos2,
    view: &mut View,
    rule: &mut term_app::trigger::Rule,
    layout: &OsLayout,
) {
    os_tabs(ui, base, layout.tabs_top, layout.tabs_h, view);
    let os = view.os.min(2);
    for (k, top) in layout.rows.iter().enumerate() {
        // 参考图 `04` / `10` 的 Command / Arguments / Working Directory 三行
        // 标签首字母都带助记符下划线，不是裸文字。
        label_ul(
            ui,
            base.x,
            base.y + top + (layout.height - FS) / 2.0,
            layout.labels[k],
            layout.labels[k].chars().next().unwrap_or(' '),
        );
        let fr = egui::Rect::from_min_size(
            base + egui::vec2(layout.left, *top),
            egui::vec2(layout.width, layout.height),
        );
        let cmd = &mut rule.config.action.os[os];
        match k {
            0 => {
                field(ui, fr, &mut cmd.command, "", false);
                browse_button(ui, fr, "...");
            }
            1 => {
                field(ui, fr, &mut cmd.arguments, "${match}", false);
                browse_button(ui, fr, "\u{25b6}");
            }
            _ => {
                field(ui, fr, &mut cmd.working_directory, spec::action::WORKDIR_HINT, false);
                browse_button(ui, fr, "...");
            }
        }
    }
    if let (Some(y), 2) = (layout.hide_y, os) {
        label(ui, base + egui::vec2(0.0, y + (FS - 1.0) / 2.0), ps::run_command::HIDE_LABEL);
        let r = egui::Rect::from_min_size(
            base + egui::vec2(ps::run_command::HIDE_TOGGLE_X, y),
            egui::vec2(ps::run_command::HIDE_TOGGLE_W, ps::run_command::HIDE_TOGGLE_H),
        );
        let mut on = rule.config.action.hide_console;
        if toggle(ui, r, &mut on) {
            rule.config.action.hide_console = on;
        }
    }
}

/// 三行字段右端的内嵌按钮（`...` 与 `▶`）。
///
/// 视觉基线里它们都在字段框**内部**右端；点开文件选择器需要平台对话框，依赖
/// 白名单未放行，这里只按原图保留外观与命中区（不派发任何动作）。
fn browse_button(ui: &mut egui::Ui, field: egui::Rect, mark: &str) {
    let r = egui::Rect::from_min_size(
        egui::pos2(field.right() - ps::run_command::BTN_INSET, field.top()),
        egui::vec2(ps::run_command::BTN_INSET, field.height()),
    );
    ui.allocate_rect(r, egui::Sense::click());
    ui.painter().text(
        r.center(),
        egui::Align2::CENTER_CENTER,
        mark,
        mono(FS - 1.0),
        rgb(c::TEXT_3),
    );
}

/// 三个 OS 选项卡（选中项浅底、文字带助记符下划线）。
pub(super) fn os_tabs(ui: &mut egui::Ui, base: egui::Pos2, top: f32, h: f32, view: &mut View) {
    let p = ui.painter().clone();
    let mut x = 0.0;
    for (i, name) in spec::action::OS.iter().enumerate() {
        let w = text_w(name) + 8.0;
        let r = egui::Rect::from_min_size(base + egui::vec2(x, top), egui::vec2(w, h));
        let active = view.os == i;
        p.rect_filled(r, egui::Rounding::ZERO, rgb(if active { 0x3a3a3a } else { c::BG }));
        p.rect_stroke(r, egui::Rounding::ZERO, egui::Stroke::new(0.6, rgb(c::LINE)));
        mnemonic_at(ui, &p, r, name, name.chars().next().unwrap_or(' '));
        if ui.allocate_rect(r, egui::Sense::click()).clicked() {
            view.os = i;
        }
        x += w + 1.5;
    }
}

/// 多行框 + 底部 `Text`/`Script` 选项卡（视觉基线 `19` / `20`）。
pub(super) fn text_block(
    ui: &mut egui::Ui,
    base: egui::Pos2,
    top: f32,
    bottom: f32,
    value: &mut String,
    mode: &mut SendMode,
    hint: &str,
) {
    let tabs_h = ps::action_extra::MODE_TABS_H;
    let box_r = egui::Rect::from_min_size(
        base + egui::vec2(0.0, top),
        egui::vec2(spec::BODY.2, (bottom - top - tabs_h).max(20.0)),
    );
    let shown = if *mode == SendMode::Script { SCRIPT_HINT } else { hint };
    multiline(ui, box_r, value, shown, true);

    let tabs = egui::Rect::from_min_size(
        egui::pos2(box_r.left(), box_r.bottom()),
        egui::vec2(box_r.width(), tabs_h),
    );
    let p = ui.painter().clone();
    p.line_segment(
        [tabs.left_bottom(), tabs.right_bottom()],
        egui::Stroke::new(0.75, rgb(c::FIELD_LINE)),
    );
    let mut x = tabs.left();
    for (i, name) in ps::action_extra::MODES.iter().enumerate() {
        let w = text_w(name) + 16.0;
        let r = egui::Rect::from_min_size(egui::pos2(x, tabs.top()), egui::vec2(w, tabs_h));
        let active = (*mode == SendMode::Script) == (i == 1);
        p.rect_filled(r, egui::Rounding::ZERO, rgb(if active { c::TAB } else { c::BG }));
        p.rect_stroke(r, egui::Rounding::ZERO, egui::Stroke::new(0.75, rgb(c::LINE)));
        mnemonic_at(ui, &p, r, name, name.chars().next().unwrap_or(' '));
        if ui.allocate_rect(r, egui::Sense::click()).clicked() {
            *mode = if i == 1 { SendMode::Script } else { SendMode::Text };
        }
        x += w;
    }
}

/// Script 模式的占位脚本（视觉基线 `18` / `31` 同源）。
///
/// 与 `edit.rs` 一致：触发器 Script 模式只求值 `return "字面量"` 或纯文本，
/// 不认 JS 箭头函数；`${match}` 在求值前由命中内核替换。
const SCRIPT_HINT: &str = "E.g.:\nreturn \"echo ${match}\"";

/// `Play sound` 分支：分组框 + `Sound Name` 下拉 + 时长 + `Test`。
pub(super) fn sound_block(
    ui: &mut egui::Ui,
    base: egui::Pos2,
    top: f32,
    bottom: f32,
    view: &mut View,
    rule: &mut term_app::trigger::Rule,
) {
    let p = ui.painter().clone();
    let group = egui::Rect::from_min_size(
        base + egui::vec2(0.0, top),
        egui::vec2(spec::BODY.2, bottom - top),
    );
    p.rect_stroke(group, egui::Rounding::same(2.0), egui::Stroke::new(1.0, rgb(0x2e2e2e)));

    label_ul(
        ui,
        group.left() + ps::action_extra::SOUND_LABEL_X,
        base.y + ps::action_extra::SOUND_COMBO_TOP + ps::action_extra::SOUND_COMBO_H / 2.0,
        "Sound Name:",
        'S',
    );
    let combo_r = egui::Rect::from_min_size(
        base + egui::vec2(ps::action_extra::SOUND_COMBO_X, ps::action_extra::SOUND_COMBO_TOP),
        egui::vec2(ps::action_extra::SOUND_COMBO_W, ps::action_extra::SOUND_COMBO_H),
    );
    let shown = if rule.config.action.sound.is_empty() {
        "None".to_owned()
    } else {
        rule.config.action.sound.clone()
    };
    // 视觉基线 `21`：这一页的焦点落在 `Sound Name` 上（蓝边），而上方 `Action`
    // 下拉仍是灰边。焦点态显式钉死，不跟随真实鼠标（截图基线要可复现）。
    if combo(ui, combo_r, &shown, true).clicked() {
        view.toggle_at(Open::SoundName, combo_r);
    }

    let dur = format!(
        "Sound Duration: {}",
        ps::duration_text(rule.config.action.sound_secs)
    );
    p.text(
        base + egui::vec2(ps::action_extra::SOUND_LABEL_X, ps::action_extra::DURATION_Y),
        egui::Align2::LEFT_CENTER,
        dur,
        mono(FS),
        rgb(c::TEXT_2),
    );

    // `Test` 按钮：外观按视觉基线 `21`（深底 + 绿三角 + `T` 带下划线）。试听依赖
    // 音频输出通道，本仓未接（README 待办），点击不产生副作用。
    let (bx, bt, bw, bh) = ps::action_extra::TEST_BTN;
    let tr = egui::Rect::from_min_size(base + egui::vec2(bx, bt), egui::vec2(bw, bh));
    bevel(&p, tr, 0x424242, 0x3c3c3c, 0x292929, 0x232323);
    ui.allocate_rect(tr, egui::Sense::click());
    let tri = egui::pos2(tr.left() + 26.0, tr.center().y);
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
        egui::pos2(tri.x + 12.0, tr.center().y),
        egui::Align2::LEFT_CENTER,
        "Test",
        mono(FS),
        rgb(c::TEXT_2),
    );
}

#[cfg(test)]
mod tests {
    use super::SCRIPT_HINT;

    #[test]
    fn the_script_hint_uses_syntax_the_trigger_backend_actually_evaluates() {
        assert!(!SCRIPT_HINT.contains("=>"), "占位不能是 JS 箭头函数：{SCRIPT_HINT}");
        let body = SCRIPT_HINT.lines().nth(1).expect("第二行是示例表达式");
        assert_eq!(term_core::trigger_script::eval(body), "echo ${match}");
    }
}

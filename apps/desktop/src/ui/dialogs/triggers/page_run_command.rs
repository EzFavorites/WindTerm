//! `Run command` 页（视觉基线 `26` / `27`）：Match + `Action:` 分 OS 三字段。
//!
//! 与事件页的区别只有两点：多一整块 `Match`（含 flag bar），以及 `Action:` 是
//! 一个**分组标题**而不是下拉——这一页的动作类型由触发器类型本身决定，不能再选。
//! `Windows` 选项卡下比另外两个多一行 `Hide console window:`（图 `27`）。

use super::View;
use crate::ui::view::trigger_pages_spec as ps;
use crate::ui::view::trigger_spec as spec;
use crate::ui::*;

fn origin(o: egui::Pos2) -> egui::Pos2 {
    o + egui::vec2(spec::BODY.0, spec::BODY.1)
}

/// 画整页。
pub(in crate::ui) fn page(
    ui: &mut egui::Ui,
    o: egui::Pos2,
    view: &mut View,
    rule: &mut term_app::trigger::Rule,
) {
    let base = origin(o);
    super::edit::match_box(ui, base, rule, ps::run_command::MATCH_H, true);

    super::fields::label(
        ui,
        base + egui::vec2(0.0, ps::run_command::ACTION_LABEL_Y),
        "Action:",
    );

    let layout = super::page_action::OsLayout {
        tabs_top: ps::run_command::TABS_TOP,
        tabs_h: ps::run_command::TABS_H,
        rows: ps::run_command::ROWS,
        left: ps::run_command::FIELD_LEFT,
        width: ps::run_command::FIELD_W,
        height: ps::run_command::FIELD_H,
        labels: spec::action::LABELS,
        hide_y: Some(ps::run_command::HIDE_LABEL_Y),
    };
    super::page_action::os_rows(ui, base, view, rule, &layout);
    super::sync_actions(rule);
}

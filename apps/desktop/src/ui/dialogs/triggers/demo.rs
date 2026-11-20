//! 调试用预置：把某个浮层 / 弹窗直接打开，便于截图与视觉基线比对。
//!
//! 只在 debug 构建里生效，靠 `WINDTERM_TRIG_OPEN` 选一个预置名；发布构建
//! 里整个模块被 `cfg` 摘掉。锚点坐标与真实控件同源（都从
//! `crate::ui::view::trigger_spec` 推），所以截图里的菜单位置就是实际位置。
//!
//! **每帧都重新赋值**（不是切一次就锁死）：第一帧窗口尚未完成布局，`o` 与
//! 之后各帧不同；若只在首帧记锚点，菜单会整体偏移几十点。这里用「直接赋值」
//! 而不是 `toggle_at`，保证幂等。
//!
//! 与 `comp/trig_sample.rs` 配合：那个给规则列表，这个开浮层。

use super::{Open, View};
use crate::ui::view::trigger_pages_spec as ps;
use crate::ui::view::trigger_spec as spec;

/// 窗口坐标下的 `BODY` 原点。
fn base(o: egui::Pos2) -> egui::Pos2 {
    o + egui::vec2(spec::BODY.0, spec::BODY.1)
}

fn rect(o: egui::Pos2, x: f32, y: f32, w: f32, h: f32) -> egui::Rect {
    egui::Rect::from_min_size(base(o) + egui::vec2(x, y), egui::vec2(w, h))
}

/// Highlight 弹窗的矩形（中心与父对话框中心重合）。
fn hl_rect(o: egui::Pos2) -> egui::Rect {
    egui::Rect::from_center_size(
        o + egui::vec2(spec::DIALOG_W / 2.0, spec::DIALOG_H / 2.0),
        egui::vec2(spec::highlight::W, spec::highlight::H),
    )
}

/// 按 `WINDTERM_TRIG_SAMPLE` 钉住当前类型页。
///
/// 有选中项时 `show()` 已经把 `kind` 对齐到那条规则；**空列表**（`WINDTERM_TRIG_EMPTY=1`）
/// 时没有可比对的规则，`kind` 会停在缺省的 `Highlight Text`——11 种类型里除
/// Highlight Text 以外的空态就都出不来。这里补上这唯一一条缺口。
fn pin_kind(view: &mut View) {
    if view.sel.is_some() {
        return;
    }
    let Ok(wanted) = std::env::var("WINDTERM_TRIG_SAMPLE") else {
        return;
    };
    if let Some(k) = term_core::trigger::TriggerKind::ALL
        .iter()
        .find(|k| format!("{k:?}") == wanted)
    {
        view.kind = *k;
        // `Sessions` 页没有类型概念，留在原标签页即可。
        view.tab = 0;
    }
}

/// 按 `WINDTERM_TRIG_OPEN` 预置视图状态。
pub(in crate::ui) fn preset(view: &mut View, ctx: &egui::Context, o: egui::Pos2) {
    // 先钉类型：这一步与「打开哪个浮层」无关，且必须在 `WINDTERM_TRIG_OPEN`
    // 缺失时也生效，否则空态只能出 Highlight Text 一种页。
    pin_kind(view);
    let Ok(which) = std::env::var("WINDTERM_TRIG_OPEN") else {
        return;
    };
    let _ = ctx;
    let open = |view: &mut View, which: Open, anchor: egui::Rect| {
        view.open = Some(which);
        view.anchor = Some(anchor);
    };
    match which.as_str() {
        // 新建触发器弹窗（`03` / `09`），`types` 连 Type 列表一起展开（`08`）。
        "new" | "types" => {
            view.new_dialog = true;
            view.new_name = "111".to_owned();
            view.type_menu = which == "types";
            // 视觉基线 `08` 高亮首项 Custom link；缺省 `new_kind=6`（Highlight text）
            // 会让类型列表的高亮行落在第 7 项，整图差分无从可比。`types` 预设
            // 钉成首项，出的图才能和 `08` 逐像素对齐（待办 1）。
            if which == "types" {
                view.new_kind = 0;
            }
        }
        // Highlight 设置弹窗（`11` / `13`）。
        "hl" | "hl-style" => {
            view.hl_dialog = true;
            if which == "hl-style" {
                let r = hl_rect(o);
                open(
                    view,
                    Open::LineStyle,
                    egui::Rect::from_min_size(
                        r.min + egui::vec2(spec::highlight::ROW_FIELD_X, spec::highlight::ROWS[2]),
                        egui::vec2(spec::highlight::ROW_FIELD_W, spec::highlight::ROW_H),
                    ),
                );
            }
        }
        // 取色器（`06`）：视觉基线数值区是白色 `#ffffff`，钉成白色才能对齐差分。
        "picker" => view.picker = Some(super::picker::from_hex(0xffffff)),
        // Sessions 页（`02`）。
        "sessions" => view.tab = 1,
        // Action 下拉（`05`，Action 类页面）。
        "action" => open(
            view,
            Open::Action,
            rect(o, 0.0, spec::action::COMBO.1, spec::BODY.2, spec::action::COMBO.2),
        ),
        // Line Style 下拉（`12`，Custom link 的 Mark Style 行）。
        "line" => open(
            view,
            Open::LineStyle,
            rect(
                o,
                spec::custom::MARK_COMBO_LEFT,
                spec::custom::MARK_STYLE_TOP,
                spec::BODY.2 - spec::custom::MARK_COMBO_LEFT,
                spec::custom::MARK_COMBO_H,
            ),
        ),
        // Color 下拉（`14`，Custom link 的 Mark Color 行）。
        "color" => open(
            view,
            Open::MarkColor,
            rect(
                o,
                spec::custom::MARK_COMBO_LEFT,
                spec::custom::MARK_COLOR_TOP,
                spec::BODY.2 - spec::custom::MARK_COMBO_LEFT,
                spec::custom::MARK_COMBO_H,
            ),
        ),
        // Send Condition 下拉（`16`）。
        "cond" => open(
            view,
            Open::SendCondition,
            rect(
                o,
                spec::send::FIELD_LEFT,
                spec::send::COND_TOP - 2.0,
                spec::send::FIELD_W,
                spec::send::ROW_H,
            ),
        ),
        // Captures 下拉（`17`）。
        "captures" => open(
            view,
            Open::Captures,
            rect(
                o,
                spec::BODY.2 - spec::send::CAPTURES_W,
                spec::send::SEND_TOP + spec::send::SEND_H,
                spec::send::CAPTURES_W,
                spec::send::MODE_TABS_H,
            ),
        ),
        // Script 选项卡（`18`）。
        "script" => view.script = true,
        // ── 第二批视觉基线（`19`~`31`）──
        //
        // `Play sound` 的修改弹窗（图 `29`）。
        "psdlg" => view.ps_dialog = true,
        // 声音库下拉（图 `22`）：锚点与真实控件同源（`action_extra::SOUND_COMBO_*`）。
        "sound" => open(
            view,
            Open::SoundName,
            rect(
                o,
                ps::action_extra::SOUND_COMBO_X,
                ps::action_extra::SOUND_COMBO_TOP,
                ps::action_extra::SOUND_COMBO_W,
                ps::action_extra::SOUND_COMBO_H,
            ),
        ),
        // `Custom menu` 的 `+` 下拉（图 `25`）。
        "add" => open(
            view,
            Open::AddMenu,
            rect(
                o,
                5.0,
                ps::custom_menu::TABLE_BOTTOM,
                16.0,
                ps::custom_menu::TOOLS_H,
            ),
        ),
        // `Custom menu` 的 `Mark Style` 下拉（图 `24`，与 `12` 同一份菜单，
        // 区别只是锚点在这一页的 Mark 行上）。
        "stylemenu" => open(
            view,
            Open::LineStyle,
            rect(
                o,
                ps::custom_menu::MARK_COMBO_LEFT,
                ps::custom_menu::MARK_STYLE_TOP,
                spec::BODY.2 - ps::custom_menu::MARK_COMBO_LEFT,
                ps::custom_menu::MARK_COMBO_H,
            ),
        ),
        // `Run command` 的 Windows 选项卡（图 `27` 的 `Hide console window`）。
        "cmdwin" => view.os = 2,
        other => eprintln!("WINDTERM_TRIG_OPEN={other} 未知，忽略"),
    }
}

//! Sender 的工具行：五颗图标 + 两组单选 + 两个数字框 + 目标下拉（+ `Selected
//! Sessions` 时的多选一格）+ 那五行菜单。
//!
//! 全部手绘（与 `tool_bar` / `dock_bottom` 同一条路）：**不赌字体**。运行时有个
//! 图标画不出来，按钮就成了一块空白；线段与多边形没有这个问题。
//!
//! 逐像素对照见 `tools_sender.rs` 的模块文档（那张表是总账，这里只放实现）。

use eframe::egui;
use term_core::theme::CHROME_SCALE;

use crate::ui::TerminalWindow;

use super::tools_sender_state::{picked_title, Target};

/// `Count` 框宽（视觉基线 519..594）。
const COUNT_W: f32 = 75.0;
/// `Interval` 框宽（视觉基线 685..784）。
const INTERVAL_W: f32 = 99.0;
/// `Targets` 下拉宽（视觉基线展开的列表 868..1097）。
const TARGET_W: f32 = 229.0;
/// `Targets ▸ Selected Sessions` 右边那一格多选下拉的宽（与 `Targets` 同宽，
/// 两格并排看着是一对）。
pub(super) const PICK_W: f32 = 229.0;
/// 多选下拉的最小可读宽。窄面板下宁可收窄到这个宽，也不许画到裁剪区外——
/// 那一格一旦看不见，「指定会话」就成了一个点了没反应的选项。
const PICK_MIN_W: f32 = 72.0;
/// 运行键的绿。视觉基线 `#008200`，整张图唯一的绿。
const RUN_GREEN: u32 = 0x008200;
/// 停止键的灰。视觉基线 `#646464`——比图标暗一档，表示「现在没在跑」。
const STOP_GREY: u32 = 0x646464;

/// 工具行上的意图。绘制只产出它，落地统一在 `TerminalWindow::apply_cmd`。
pub(super) enum Cmd {
    Run,
    Stop,
    /// `＋`：多开一页。
    NewTab,
    /// `−`：关掉当前页（只剩一页时不生效）。
    CloseTab,
    Clear,
    Encoding(bool),
    By(bool),
    Count(i32),
    Interval(i32),
    Target(Target),
    OpenTargets,
    /// 展开 / 收起「指定会话」那一格的多选弹层。
    OpenPicked,
    /// 勾 / 取消勾弹层里的第 `usize` 行（本帧标签序号）。
    PickToggle(usize),
}

/// 工具行的绘制结果：下拉框的锚点矩形 + 本帧的意图。
pub(super) struct RowOut {
    pub anchor: egui::Rect,
    /// `Target::Selected` 时右侧多选下拉的矩形（`None` = 本帧没画它）。
    pub picked: Option<egui::Rect>,
    pub cmd: Option<Cmd>,
}

/// 工具行。只读 `win`——本帧所有状态改动都攒成 `Cmd`，由调用方一次性落地。
pub(super) fn tool_row(win: &TerminalWindow, ui: &mut egui::Ui, rect: egui::Rect) -> RowOut {
    let mut row = Row::new(ui, rect);
    let mut cmd = None;
    let s = win.sender_state();

    if row.icon("sender_run", Glyph::Run, crate::theme::chrome(RUN_GREEN)).clicked() {
        cmd = Some(Cmd::Run);
    }
    row.gap(ICON_GAP);
    let stop_c = crate::theme::chrome(STOP_GREY);
    if row.icon("sender_stop", Glyph::Stop, stop_c).clicked() {
        cmd = Some(Cmd::Stop);
    }
    row.gap(ICON_GAP);
    let icon_c = crate::theme::chrome(CHROME_SCALE.icon);
    if row.icon("sender_add", Glyph::Plus, icon_c).clicked() {
        cmd = Some(Cmd::NewTab);
    }
    row.gap(ICON_GAP);
    if row.icon("sender_del", Glyph::Minus, icon_c).clicked() {
        cmd = Some(Cmd::CloseTab);
    }
    row.gap(ICON_GAP);
    if row.icon("sender_clear", Glyph::Cross, icon_c).clicked() {
        cmd = Some(Cmd::Clear);
    }
    row.gap(SEP_PAD);
    row.separator();
    row.gap(SEP_PAD);

    if row.radio(win.tr("Text"), !s.hex) {
        cmd = Some(Cmd::Encoding(false));
    }
    row.gap(GROUP_GAP);
    if row.radio(win.tr("Hex"), s.hex) {
        cmd = Some(Cmd::Encoding(true));
    }
    row.gap(GROUP_GAP);
    let text_c = row.text_c;
    row.label(win.tr("By:"), text_c);
    row.gap(LABEL_GAP);
    if row.radio(win.tr("Line"), !s.by_char) {
        cmd = Some(Cmd::By(false));
    }
    row.gap(GROUP_GAP);
    if row.radio(win.tr("Char"), s.by_char) {
        cmd = Some(Cmd::By(true));
    }
    row.gap(GROUP_GAP);

    row.label(win.tr("Count:"), text_c);
    row.gap(LABEL_GAP);
    let count_text = format!("{}", s.count);
    let (up, down) = row.field(&count_text, COUNT_W, "sender_count");
    if up {
        cmd = Some(Cmd::Count(1));
    } else if down {
        cmd = Some(Cmd::Count(-1));
    }
    row.gap(GROUP_GAP);

    row.label(win.tr("Interval:"), text_c);
    row.gap(LABEL_GAP);
    let iv_text = format!("{:.2}s", s.interval);
    let (up, down) = row.field(&iv_text, INTERVAL_W, "sender_interval");
    if up {
        cmd = Some(Cmd::Interval(1));
    } else if down {
        cmd = Some(Cmd::Interval(-1));
    }
    row.gap(GROUP_GAP);

    row.label(win.tr("Targets:"), text_c);
    row.gap(LABEL_GAP);
    let (resp, anchor) = row.dropdown(win.tr(s.target.title()), TARGET_W, "sender_targets");
    if resp.clicked() {
        cmd = Some(Cmd::OpenTargets);
    }

    // 「指定会话」多出来那一格：勾了谁只能看得见才有意义，所以它紧跟在
    // `Targets` 右边、只在 `Selected Sessions` 这一档出现。
    let mut picked = None;
    if s.target == Target::Selected {
        row.gap(GROUP_GAP);
        let names = win.sender_picked_names();
        let w = (rect.right() - 8.0 - row.x).min(PICK_W);
        // 剩余宽度不够一格可读的下拉时宁可不画（窄面板），也不画到裁剪区外。
        if w >= PICK_MIN_W {
            let title = picked_title(&names, win.tr("None"));
            let (resp, r) = row.dropdown(&title, w, "sender_picked");
            picked = Some(r);
            if resp.clicked() {
                cmd = Some(Cmd::OpenPicked);
            }
        }
    }

    // 一次性提示贴在行尾：工具行左侧已经排满，右边还剩一截空白。
    if let Some(note) = &s.note {
        let c = egui::Color32::from_rgb(0xE0, 0x60, 0x60);
        let g = row.p.layout_no_wrap(note.clone(), row.font.clone(), c);
        if row.x + g.size().x + 8.0 <= rect.right() {
            row.p.galley(
                egui::pos2(rect.right() - 8.0 - g.size().x, row.cy - g.size().y / 2.0),
                g,
                c,
            );
        }
    }
    RowOut {
        anchor,
        picked,
        cmd,
    }
}

// 通用绘制原语抽象到 kit/panel_row；Sender 自己的语义与命令仍留在本模块。
#[allow(unused_imports)]
pub(super) use crate::ui::kit::panel_row::{
    paint_glyph, paint_tri, Glyph, Row, FIELD_H, GROUP_GAP, ICON_GAP, ICON_W, LABEL_GAP, RADIO_D,
    ROW_H, SEP_PAD,
};

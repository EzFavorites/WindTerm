//! 会话树里「一行」的画法：分组头与会话行，以及折叠状态。
//!
//! R9：从 `sidebar_view.rs` 再拆一层（2026-11-07，该文件越过 450 行上限）。
//! 分工：`sidebar.rs` 管数据与落盘、`sidebar_view.rs` 管标题带与筛选行、
//! 本文件管树上那两种行、`session_icons.rs` 管最底层的图形原语。
//!
//! 所有行的竖向位置都取 `rect.center().y`——一行只有一个中线，文字、图标、
//! 三角、文件夹全部落在它上面。**不要**在某个元素上再加偏移量：那正是
//! 「同一行的东西看着不齐」的来源。

use super::session_icons::{
    paint_chevron, paint_folder, paint_protocol_icon, FOLDER_H, FOLDER_W,
};
use crate::ui::*;
use term_core::theme::CHROME_SCALE;

/// 折叠三角的中心 x（面板左边起算）。
const CHEVRON_X: f32 = 8.0;
/// 文件夹左沿。
const FOLDER_LEFT: f32 = 19.0;
/// 分组名左沿。
const GROUP_TEXT_X: f32 = 37.5;
/// 会话行图标中心 x（比分组再缩进一级）。
const SESSION_ICON_CX: f32 = 47.5;
/// 会话名左沿。图标右沿在 54.5，留 3px 才不粘字（实测视觉基线是 ~57.5）。
const SESSION_TEXT_X: f32 = 58.0;
/// 协议图标半径。
const ICON_R: f32 = 7.0;
/// 树上所有文字共用等宽字体——树里没有一个字符是比例字体。
pub(in crate::ui) const TREE_FONT: f32 = crate::ui::type_scale::SIZE_BODY;

/// 分组折叠状态存在 `egui` 的临时数据里（组件私有状态，不回流 `ViewState`）。
/// 默认展开，与旧版 `CollapsingHeader::default_open(true)` 一致。
pub(super) fn group_is_open(ui: &egui::Ui, name: &str) -> bool {
    let id = egui::Id::new(("session_group_open", name));
    ui.data(|d| d.get_temp::<bool>(id)).unwrap_or(true)
}

pub(super) fn set_group_open(ui: &egui::Ui, name: &str, open: bool) {
    let id = egui::Id::new(("session_group_open", name));
    ui.data_mut(|d| d.insert_temp(id, open));
}

/// 分组头：实心三角 + 青色渐变文件夹 + 等宽组名。
pub(super) fn paint_group_header(
    ui: &egui::Ui,
    rect: egui::Rect,
    group: &str,
    open: bool,
    hovered: bool,
) {
    let s = &CHROME_SCALE;
    let p = ui.painter();
    if hovered {
        p.rect_filled(rect, 0.0, crate::theme::chrome(s.hover));
    }
    let cy = rect.center().y;
    paint_chevron(
        p,
        egui::pos2(rect.left() + CHEVRON_X, cy),
        open,
        crate::theme::chrome(s.text).gamma_multiply(0.8),
    );
    paint_folder(
        p,
        egui::pos2(rect.left() + FOLDER_LEFT + FOLDER_W * 0.5, cy),
        FOLDER_W,
        FOLDER_H,
    );
    p.text(
        egui::pos2(rect.left() + GROUP_TEXT_X, cy),
        egui::Align2::LEFT_CENTER,
        group,
        crate::ui::type_scale::font(TREE_FONT),
        crate::theme::chrome(s.text),
    );
}

/// 会话行：整行底（选中 `selected` + 左 2px 焦点蓝；悬停 `hover`）+ 协议图标
/// + 名字 + 运行指示点。
pub(super) fn paint_session_row(
    ui: &egui::Ui,
    rect: egui::Rect,
    name: &str,
    protocol: Protocol,
    running: bool,
    selected: bool,
    hovered: bool,
) {
    let s = &CHROME_SCALE;
    let p = ui.painter();
    if selected {
        p.rect_filled(rect, 0.0, crate::theme::chrome(s.selected));
        p.rect_filled(
            egui::Rect::from_min_size(rect.min, egui::vec2(2.0, rect.height())),
            0.0,
            crate::theme::chrome(s.focus),
        );
    } else if hovered {
        p.rect_filled(rect, 0.0, crate::theme::chrome(s.hover));
    }

    paint_protocol_icon(
        p,
        egui::pos2(rect.left() + SESSION_ICON_CX, rect.center().y),
        ICON_R,
        protocol,
    );
    p.text(
        egui::pos2(rect.left() + SESSION_TEXT_X, rect.center().y),
        egui::Align2::LEFT_CENTER,
        name,
        crate::ui::type_scale::font(TREE_FONT),
        crate::theme::chrome(s.text),
    );
    if running {
        p.circle_filled(
            egui::pos2(rect.right() - 10.0, rect.center().y),
            3.2,
            crate::theme::chrome(s.focus),
        );
    }
}

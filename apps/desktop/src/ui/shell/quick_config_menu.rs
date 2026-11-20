//! Quick 命令栏左侧 `Config` 弹层 + 弹层的「点外收起」判定。
//!
//! 从 `quick_command_bar.rs` 拆出是为单文件代码行 ≤ 500。弹层结构按
//! 真图 `quick-popup-clear.png`（196×364 物理像素，2× Retina → 98×182pt）
//! 逐行还原，详见 [`config_menu`] 的注释。绘制逻辑与生产/夹具共用。

use eframe::egui;

use term_core::theme::CHROME_SCALE;
use term_runtime::quickbar::QuickBarConfig;

use crate::theme::chrome;
use crate::ui::tr;
use term_app::i18n::Lang;
use term_app::vocab::Action;

use super::quick_layout::Metrics;
use super::quick_command_bar::label_for;
use super::menu_style::check_rect_with_inset;

/// `Groups` 行的方框左内边距（视觉基线 `quick-popup-clear.png` 实测 8）。
/// 边长**不用这里定**，统一取 [`super::menu_style::CHECK_SIZE`]（11）。
const GROUP_CHECK_INSET: f32 = 8.0;

/// 弹层宽度：真图 196px / 2 = **98pt**。此前钉 190pt 是截图物理像素与
/// 逻辑点混淆，视觉上比视觉基线宽了一倍。
const CONFIG_MENU_W: f32 = 98.0;
/// 真图色带（物理 / 2）：Config 56px → 28pt；Quick 40px → 20pt；
/// Untitled 与每条 Groups 勾选行 56px → 28pt；Groups 标题 40px → 20pt。
const HEADER_H: f32 = 28.0;
const GROUP_NAME_H: f32 = 20.0;
const ROW_H: f32 = 28.0;
/// 行底色（弹层 Frame 那层）。文字/悬停色已收进 `kit::spec`。
const LIGHT: egui::Color32 = egui::Color32::from_rgb(0x33, 0x33, 0x33);

pub(super) fn config_menu_id() -> egui::Id {
    egui::Id::new("quick_command_config")
}

/// 左区第一个控件：`Config` 面板。真值 `06-button-popups/quick-popup-clear.png`
/// （2026-11-12 补采局部清晰图，用户确认语义）的结构是：
///
/// ```text
/// ┌ Config ─────────┐  按钮：点击 = 打开命令配置面板（同右端齿轮）；
///                     蓝底 = 指向色（hover/pointed），非固有底色
/// │ Quick           │  当前组名，纯文字，**没有下拉箭头**
/// │   Untitled      │  组内成员按钮（当前仅一条），缩进、文字正常亮度；
///                     常态 #333，hover 才出蓝（指向色）
/// ├ Groups ─────────┤
/// │ ☐ 1             │
/// │ ☑ 2             │
/// │ ☐ Quick         │
/// └─────────────────┘
/// ```
///
/// 用户 2026-11-12 明示三点：① 顶部 `Config` 是按钮，点击打开命令配置面板
/// （与右端齿轮同一个 `OpenQuickCommands` 动作）；② `Quick` 只是组名字，
/// 下面缩进的是组成员，没有置灰；③ 组切换下拉属于命令配置对话框
/// （`dialogs/quick_commands.rs` 顶行 `qc_group_combo`），**不在这个弹层里**。
/// 所以这里删掉原先铺满整行的 `ComboBox`——它既造出了右侧的下拉箭头，
/// 又把弹层视觉宽度撑大。当前组下标仍从 `config_group` 视图态读取，夹具
/// 定格在 `Quick`（下标 2），生产默认 0；切换组的入口只有配置对话框。
///
/// **不用 `popup_below_widget`**（与 `dock_bottom_menu.rs:35-40` 同一条规矩）：
/// ① 它向下弹，而快捷命令栏是窗口最底部第二条横栏，弹层会直接漏出窗口
/// 底边；② 它包出来的 `Area` 默认淡入，首帧又是 sizing pass。这里显式
/// 向上弹 + `.fade_in(false)` + 自己管点外收起。
pub(super) fn config_menu(
    ui: &mut egui::Ui,
    visible: &[bool],
    config: &QuickBarConfig,
    actions: &mut Vec<Action>,
    m: &Metrics,
    lang: Lang,
) {
    let id = config_menu_id();
    let (rect, response) =
        ui.allocate_exact_size(egui::vec2(m.config_w, m.button_h), egui::Sense::click());
    let c = rect.center();
    let stroke = egui::Stroke::new(1.5, chrome(CHROME_SCALE.text));
    let half = 5.0 * m.scale;
    for dy in [-3.0, 0.0, 3.0] {
        let y = c.y + dy * m.scale;
        ui.painter()
            .line_segment([egui::pos2(c.x - half, y), egui::pos2(c.x + half, y)], stroke);
    }
    let hovered = response.on_hover_text(tr::t(lang, "Configure command groups"));
    if hovered.clicked() {
        ui.memory_mut(|mem| mem.toggle_popup(id));
    }
    if !ui.memory(|mem| mem.is_popup_open(id)) {
        return;
    }
    let mut toggled = Vec::new();
    let mut open_commands = false;
    let area = egui::Area::new(id.with("area"))
        .order(egui::Order::Foreground)
        .fixed_pos(rect.left_top())
        .pivot(egui::Align2::LEFT_BOTTOM)
        .fade_in(false)
        .show(ui.ctx(), |ui| {
            egui::Frame::none()
                .fill(LIGHT)
                .stroke(egui::Stroke::new(1.0, chrome(CHROME_SCALE.border)))
                .show(ui, |ui| {
                    ui.set_width(CONFIG_MENU_W);
                    ui.spacing_mut().item_spacing.y = 0.0;

                    // ① `Config` 顶部按钮。用户强调：它是按钮，点击后和快捷栏
                    // 右端齿轮一样弹开命令配置面板（OpenQuickCommands），不是
                    // 纯标题条。蓝底仍不是固有底色——真图那一抹蓝是截图瞬间
                    // 鼠标划过的指向色；平时与 `Groups` 同深灰，hover / 夹具
                    // pointed 时才出 #007CD2。
                    let (hdr, hdr_resp) = ui.allocate_exact_size(
                        egui::vec2(CONFIG_MENU_W, HEADER_H),
                        egui::Sense::click(),
                    );
                    let pointed = hdr_resp.hovered()
                        || ui.memory(|mem| {
                            mem.data
                                .get_temp::<bool>(id.with("config_pointed"))
                                .unwrap_or(false)
                        });
                    crate::ui::kit::menu_row::paint(
                        ui,
                        hdr,
                        "Config",
                        crate::ui::kit::spec::MenuRowKind::QuickHeader,
                        pointed,
                        hdr.center(),
                        egui::Align2::CENTER_CENTER,
                    );
                    let hdr_resp =
                        hdr_resp.on_hover_text(tr::t(lang, "Quick command bar settings"));
                    if hdr_resp.clicked() {
                        open_commands = true;
                    }

                    // ② 当前组名：纯文字行，没有下拉框 / 箭头。
                    let group_id = id.with("config_group");
                    let mut current =
                        ui.memory(|m| m.data.get_temp::<usize>(group_id).unwrap_or(0));
                    if current >= config.groups.len() {
                        current = 0;
                    }
                    if let Some(g) = config.groups.get(current) {
                        paint_title_row(ui, label_for(&g.label, &g.label_zh, lang));
                        // ③ 当前组成员：每个都是按钮（真图当前只有一条
                        // `Untitled`），缩进 33pt、文字正常亮度；底色常态
                        // #333，hover 才出蓝——蓝色永远是指向色。
                        for cmd in &g.commands {
                            if paint_command_button(
                                ui,
                                label_for(&cmd.label, &cmd.label_zh, lang),
                                lang,
                            ) {
                                open_commands = true;
                            }
                        }
                    }

                    // ④ `Groups` 标题 + 各组可见性。Quick 也是普通组，可关。
                    paint_title_row(ui, "Groups");
                    for (i, g) in config.groups.iter().enumerate() {
                        let name = label_for(&g.label, &g.label_zh, lang);
                        let (row, resp) = ui.allocate_exact_size(
                            egui::vec2(CONFIG_MENU_W, ROW_H),
                            egui::Sense::click(),
                        );
                        // 可交互行同样遵守「蓝底 = hover 指向色」；配色与字号
                        // 由 `MenuRowKind::QuickGroup` 规格给，不再就地抄色值。
                        crate::ui::kit::menu_row::paint(
                            ui,
                            row,
                            name,
                            crate::ui::kit::spec::MenuRowKind::QuickGroup,
                            resp.hovered(),
                            egui::pos2(row.left() + 32.0, row.center().y),
                            egui::Align2::LEFT_CENTER,
                        );
                        // 2026-11-15：原先这里写死 14×14，比菜单行的 11×11
                        // 大一圈（用户点名「group 复选框太大」）。边长现在走
                        // 全仓唯一的 `CHECK_SIZE`，只保留本弹层自己的左内边距 8。
                        //
                        // 2026-11-17：框与勾整体交给 `kit::checkbox` 的 group 档
                        // （边框 `#888888`、勾 1.5pt `#DDDDDD`），本弹层不再自己画折线。
                        let box_rect = check_rect_with_inset(row, GROUP_CHECK_INSET);
                        let on = visible.get(i).copied().unwrap_or(true);
                        crate::ui::kit::checkbox::paint(
                            ui.painter(),
                            box_rect,
                            &crate::ui::kit::checkbox::CheckboxProps::group(on),
                        );
                        if resp.clicked() {
                            toggled.push(i);
                        }
                    }
                });
        });
    if open_commands {
        actions.push(Action::OpenQuickCommands);
        ui.memory_mut(|m| m.close_popup());
    }
    for i in toggled {
        actions.push(Action::QuickGroupToggle(i));
    }
    close_on_click_outside(ui, area.response.rect, rect);
}

/// 组成员按钮：整行可点，常态 #333，hover 才出蓝（指向色）。
/// 点击打开命令配置面板——这是配置菜单里的成员，不是发送命令的底栏按钮。
fn paint_command_button(ui: &mut egui::Ui, text: &str, lang: Lang) -> bool {
    let (row, resp) =
        ui.allocate_exact_size(egui::vec2(CONFIG_MENU_W, ROW_H), egui::Sense::click());
    crate::ui::kit::menu_row::paint(
        ui,
        row,
        text,
        crate::ui::kit::spec::MenuRowKind::QuickCommand,
        resp.hovered(),
        egui::pos2(row.left() + 33.0, row.center().y),
        egui::Align2::LEFT_CENTER,
    );
    resp.on_hover_text(tr::t(lang, "Quick command bar settings")).clicked()
}

/// 组名行（不可点）：深底 + 缩进 12 的左对齐文字，永远不出蓝底。
fn paint_title_row(ui: &mut egui::Ui, text: &str) {
    let (row, _) = ui.allocate_exact_size(
        egui::vec2(CONFIG_MENU_W, GROUP_NAME_H),
        egui::Sense::hover(),
    );
    crate::ui::kit::menu_row::paint(
        ui,
        row,
        text,
        crate::ui::kit::spec::MenuRowKind::QuickTitle,
        false,
        egui::pos2(row.left() + 12.0, row.center().y),
        egui::Align2::LEFT_CENTER,
    );
}

/// 手写 `Area` 的弹层要自己管「点外面收起」：点在弹层里或锚点按钮上不算。
/// 与 `dock_bottom_menu.rs:89-100` / `tools_sender_menu.rs:57-67` 同一份判定。
pub(super) fn close_on_click_outside(ui: &mut egui::Ui, popup: egui::Rect, anchor: egui::Rect) {
    let out = ui.input(|i| {
        i.pointer.any_click()
            && !i
                .pointer
                .interact_pos()
                .is_some_and(|p| popup.contains(p) || anchor.contains(p))
    });
    if out {
        ui.memory_mut(|m| m.close_popup());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui::shell::menu_style::{check_rect, CHECK_SIZE};

    /// 2026-11-15 用户反馈「group 复选框太大了，将所有复选框大小对齐」的防回归。
    /// 这里不再允许出现第二份边长：弹层可以保留自己的左内边距，但方框必须
    /// 与菜单行一样是 `CHECK_SIZE` 见方。
    #[test]
    fn group_checkboxes_are_exactly_as_large_as_menu_checkboxes() {
        let row = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(CONFIG_MENU_W, ROW_H));
        let group = check_rect_with_inset(row, GROUP_CHECK_INSET);
        let menu = check_rect(row);
        assert_eq!(group.size(), menu.size());
        assert_eq!(group.width(), CHECK_SIZE);
        assert_eq!(group.height(), 11.0);
        assert_eq!(group.left(), row.left() + 8.0);
        assert_eq!(group.center().y, row.center().y);
    }
}

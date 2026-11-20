//! 工具行右端的 `>` / `∨` / `⋮` 三件（新版截图，2026-11-08）。
//!
//! `tool_bar.rs` 原来特意**没画**这三件，理由写在它的模块头里：程序里没有对应
//! 的已接线动作，画出来就是 R5 禁止的假按钮。这次补上的是三颗**真菜单**：
//!
//! | 图形 | 菜单 | 落地 |
//! |---|---|---|
//! | `>` | Goto Line / Goto Session / Goto Address Bar | `win.dispatch(id)`，打开对应 Goto 浮层 |
//! | `∨` | 会话列表 | `win.connect_selected_session()`，真开一条会话 |
//! | `⋮` | 窗格动作（工具栏里已接线且未隐藏的那些） | `win.handle_tool_action(action)` |
//!
//! 命令 ID 全部来自 `app/menu.rs::GOTO_ITEMS`，投影层零字面量（I5）。
//! 拆成独立文件是为了 R9：`tool_bar.rs` 已有 409 行，再塞三个菜单会顶破 450。
//!
//! 几何（视觉基线 2× 实测，逻辑点）：`>` 与 `∨` 在 `#333333` 地址栏框内、
//! 分别距框右缘 34 / 12；`⋮` 在框外那 [`TAIL_W`] 宽的窗格底上。

use crate::ui::*;
use term_app::i18n::Lang;
use term_app::menu::GOTO_ITEMS;
use term_core::theme::CHROME_SCALE;

/// 地址栏框右端让给 `⋮` 的井位宽度。
pub(in crate::ui) const TAIL_W: f32 = 25.0;
const BTN_W: f32 = 18.0;

/// 三颗菜单收集到的动作。类型是投影层的强类型意图，不是命令 ID 字符串——
/// 只有 `dispatch` 那一条来自数据层常量。
#[derive(Default)]
pub(in crate::ui) struct TailWork {
    pub dispatch: Option<&'static str>,
    pub connect: Option<usize>,
    pub tool: Option<ToolAction>,
}

/// 画三件并注册菜单。`panel` 是收窄后的地址栏框，`tail` 是它右边那块井位。
pub(in crate::ui) fn draw(
    ui: &mut egui::Ui,
    panel: egui::Rect,
    tail: egui::Rect,
    sessions: &[String],
    entries: &[(ToolAction, &'static str)],
    lang: Lang,
    work: &mut TailWork,
) {
    let p = ui.painter();
    p.rect_filled(
        tail,
        0.0,
        crate::theme::chrome(CHROME_SCALE.dialog),
    );
    let ink = super::tool_bar_icons::chrome_icon();
    let cy = panel.center().y;
    let goto = egui::Rect::from_center_size(
        egui::pos2(panel.right() - 34.0, cy),
        egui::vec2(BTN_W, panel.height()),
    );
    let vee = egui::Rect::from_center_size(
        egui::pos2(panel.right() - 12.0, cy),
        egui::vec2(BTN_W, panel.height()),
    );
    let kebab = egui::Rect::from_center_size(
        tail.center(),
        egui::vec2(BTN_W, tail.height()),
    );
    paint_chevron(p, goto, ink);
    paint_vee(p, vee, ink);
    paint_kebab(p, kebab, ink);

    // 每颗菜单先把选择收进局部变量，函数返回前再写回 `work`：三个闭包依次
    // 借用 `work`，分开写能让借用关系一眼看清楚（R4）。
    let mut goto_pick: Option<&'static str> = None;
    menu_button(ui, goto, "tail_goto", |ui| {
        for (label, id) in GOTO_ITEMS {
            if ui.button(t(lang, label)).clicked() {
                goto_pick = Some(id);
                ui.close_menu();
            }
        }
    });
    if let Some(id) = goto_pick {
        work.dispatch = Some(id);
    }

    let mut session_pick = None;
    menu_button(ui, vee, "tail_sessions", |ui| {
        for (i, name) in sessions.iter().enumerate() {
            if ui.button(name.as_str()).clicked() {
                session_pick = Some(i);
                ui.close_menu();
            }
        }
    });
    if let Some(i) = session_pick {
        work.connect = Some(i);
    }

    let mut tool_pick = None;
    menu_button(ui, kebab, "tail_actions", |ui| {
        for (action, label) in entries {
            if ui.button(*label).clicked() {
                tool_pick = Some(*action);
                ui.close_menu();
            }
        }
    });
    if let Some(action) = tool_pick {
        work.tool = Some(action);
    }
}

/// 空按钮接管开合，图形由 [`draw`] 的 painter 画——`frame(false)` 保证不会
/// 冒出 egui 的按钮底与描边（视觉基线里这三颗都是纯线条）。
fn menu_button(
    ui: &mut egui::Ui,
    rect: egui::Rect,
    id: &'static str,
    content: impl FnOnce(&mut egui::Ui),
) {
    ui.scope_builder(
        egui::UiBuilder::new().id_salt(id).max_rect(rect),
        |ui| {
            egui::menu::menu_custom_button(
                ui,
                egui::Button::new("").frame(false).min_size(rect.size()),
                content,
            );
        },
    );
}

fn paint_chevron(p: &egui::Painter, r: egui::Rect, c: egui::Color32) {
    let (cx, cy) = (r.center().x, r.center().y);
    let stroke = egui::Stroke::new(1.2, c);
    p.line_segment(
        [egui::pos2(cx - 3.0, cy - 5.0), egui::pos2(cx + 3.0, cy)],
        stroke,
    );
    p.line_segment(
        [egui::pos2(cx + 3.0, cy), egui::pos2(cx - 3.0, cy + 5.0)],
        stroke,
    );
}

fn paint_vee(p: &egui::Painter, r: egui::Rect, c: egui::Color32) {
    let (cx, cy) = (r.center().x, r.center().y);
    let stroke = egui::Stroke::new(1.2, c);
    p.line_segment(
        [egui::pos2(cx - 4.0, cy - 2.0), egui::pos2(cx, cy + 2.0)],
        stroke,
    );
    p.line_segment(
        [egui::pos2(cx, cy + 2.0), egui::pos2(cx + 4.0, cy - 2.0)],
        stroke,
    );
}

fn paint_kebab(p: &egui::Painter, r: egui::Rect, c: egui::Color32) {
    for dy in [-4.5, 0.0, 4.5] {
        p.circle_filled(egui::pos2(r.center().x, r.center().y + dy), 1.1, c);
    }
}

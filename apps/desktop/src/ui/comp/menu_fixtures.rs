//! Session / Log 两个菜单展开态夹具：从 `comp/mod.rs` 外迁以满足
//! 500 代码行上限。绘制仍走 `shell::draw_menu_items_static`，
//! 与生产菜单弹层同一套三态色 / 快捷键列 / 自适应宽度。

use eframe::egui;

use super::CompId;
use crate::ui::TerminalWindow;
use crate::ui::{harness, shell};

pub(in crate::ui) fn draw(win: &mut TerminalWindow, ctx: &egui::Context, comp: CompId) {
    match comp {
        CompId::MenuSession => {
                // Session 菜单展开态：钉在菜单栏左下（与真实弹出位置同源），
                // 弹层底色取视觉基线（比正文亮一档的 #333333）。
                let menu = term_app::menu::menus().remove(0);
                harness::show_canvas(ctx, |_ui| {});
                egui::Area::new(egui::Id::new("comp_menu_session"))
                    .fixed_pos(egui::pos2(4.0, 26.0))
                    // 夹具时钟 t=0 + 默认淡入 = 整层低 alpha（旧基线里菜单底
                    // #333333 被合成成 #212121、蓝块 #007CD2 变成 (27,51,67)）。
                    // `StableWindow` / `tool_bar.rs` 早就这么处理，这里补齐。
                    .fade_in(false)
                    .order(egui::Order::Foreground)
                    .show(ctx, |ui| {
                        ui.set_width(shell::menu_popup_width(win, &menu.items, ui));
                        egui::Frame::none()
                            .fill(shell::popup_bg())
                            .inner_margin(egui::Margin::symmetric(4.0, 6.0))
                            .rounding(4.0)
                            .show(ui, |ui| {
                                // 2026-11-15：二级菜单父行改走 `Ui::menu_button`
                                // （需要真实 `MenuState`，hover 即在父菜单右侧展开）。
                                // 夹具是一块手写的 `Area`，没有那个上下文，硬调生产
                                // 路径会退回「顶级菜单按钮」。夹具改用静态手绘树：
                                // 行高 / 三态 / 四栏与生产同一份 `draw_menu_row`。
                                let _ = shell::draw_menu_items_static(win, ui, &menu.items, &[]);
                            });
                    });        }
        CompId::MenuLog => {
                // 2026-11-08 视觉基线：父菜单 `Log` 整行蓝 + 右侧 Log 子菜单
                // 展开、`Settings...` 整行蓝；Stop/Pause/Resume/Open Log File
                // 是禁用态。三态颜色与生产共用同一个 draw_menu_row。
                use term_app::menu::Item;
                let win: &TerminalWindow = win;
                let session = term_app::menu::menus().remove(0);
                let log_items: &[Item] = session
                    .items
                    .iter()
                    .find_map(|it| match it {
                        Item::Submenu { label, items } if *label == "Log" => {
                            Some(items.as_slice())
                        }
                        _ => None,
                    })
                    .unwrap_or(&[]);
                harness::show_canvas(ctx, |_ui| {});
                // 父菜单先画，量出高亮 `Log` 行的真实矩形；子菜单顶边直接对齐它，
                // 不再靠「行数 × 估计行高」推算（行距一变就会错位）。
                let mut log_rect = None;
                // 弹层宽度按内容自适应：父 / 子两层都走 `menu_popup_width`，
                // 子层左缘 = 父层左缘(4) + 父层宽 + 4px 间距，规避写死 288/200
                // 留下的右侧大段空白。
                let mut parent_w = 0.0_f32;
                egui::Area::new(egui::Id::new("comp_menu_log_parent"))
                    .fixed_pos(egui::pos2(4.0, 26.0))
                    .fade_in(false)
                    .order(egui::Order::Foreground)
                    .show(ctx, |ui| {
                        parent_w = shell::menu_popup_width(win, &session.items, ui);
                        ui.set_width(parent_w);
                        egui::Frame::none()
                            .fill(shell::popup_bg())
                            .inner_margin(egui::Margin::symmetric(4.0, 6.0))
                            .rounding(4.0)
                            .show(ui, |ui| {
                                log_rect = shell::draw_menu_items_static(
                                    win,
                                    ui,
                                    &session.items,
                                    &["Log"],
                                );
                            });
                    });
                let top = log_rect.map(|r: egui::Rect| r.top()).unwrap_or(26.0);
                egui::Area::new(egui::Id::new("comp_menu_log_child"))
                    .fixed_pos(egui::pos2(4.0 + parent_w + 4.0, top))
                    .fade_in(false)
                    .order(egui::Order::Foreground)
                    .show(ctx, |ui| {
                        ui.set_width(shell::menu_popup_width(win, log_items, ui));
                        egui::Frame::none()
                            .fill(shell::popup_bg())
                            .inner_margin(egui::Margin::symmetric(4.0, 6.0))
                            .rounding(4.0)
                            .show(ui, |ui| {
                                let _ = shell::draw_menu_items_static(
                                    win,
                                    ui,
                                    log_items,
                                    &["Settings"],
                                );
                            });
                    });        }
        _ => {}
    }
}

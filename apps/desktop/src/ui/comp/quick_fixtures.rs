//! 快捷命令栏相关的两个组件夹具：命令配置弹窗 / 左侧 `Config` 面板。
//!
//! 从 `comp/mod.rs` 拆出来是为了 R9（单文件代码行 ≤ 500）：这两个夹具一个
//! 要开 `StableWindow`、一个要在画布底部锚一条整宽横栏再播种 popup 状态，
//! 塞进主 match 会把装配文件顶过上限。

use eframe::egui;

use super::CompId;
use crate::ui::harness;
use crate::ui::TerminalWindow;

pub(in crate::ui) fn draw(win: &mut TerminalWindow, ctx: &egui::Context, comp: CompId) {
    match comp {
        CompId::QuickCommands => {
            win.view.show_quick_commands = true;
            win.draw_quick_commands(ctx);
        }
        CompId::QuickConfig => {
            // 左侧 `Config` 面板向上弹，所以栏必须放在画布**底部**；
            // `show_bar_canvas` 钉在左上角，向上弹会直接出画布。
            ctx.memory_mut(|m| m.open_popup(egui::Id::new("quick_command_config")));
            // 真图弹层展示第三个组 `Quick`（ Untitled 命令在它下面缩进展开）。
            // 组名是纯文字行、没有下拉箭头；切换组的下拉在独立的 Quick
            // Commands 配置对话框里，不在这个弹层。
            ctx.memory_mut(|m| m.data.insert_temp(
                egui::Id::new("quick_command_config").with("config_group"),
                2usize,
            ));
            // 真图 `Config` 行的蓝底是截图瞬间鼠标划过的指向色，不是固有底色；
            // 夹具定格这一瞬，生产路径没有此 flag、靠真实 hover 出蓝。
            ctx.memory_mut(|m| m.data.insert_temp(
                egui::Id::new("quick_command_config").with("config_pointed"),
                true,
            ));
            let mut actions = Vec::new();
            harness::show_canvas(ctx, |_ui| {});
            let h = crate::ui::shell::QUICK_BAR_H;
            let w = ctx.screen_rect().width() - 16.0;
            let y = ctx.screen_rect().height() - h - 8.0;
            egui::Area::new(egui::Id::new("qc_config_bar"))
                .fixed_pos(egui::pos2(0.0, y))
                .movable(false)
                .fade_in(false)
                .order(egui::Order::Foreground)
                .show(ctx, |ui| {
                    egui::Frame::none()
                        .fill(harness::canvas_bg())
                        .inner_margin(egui::Margin::symmetric(8.0, 3.0))
                        .show(ui, |ui| {
                            ui.set_width(w);
                            ui.set_min_height(h - 6.0);
                            // 真图 `06-button-popups/quick-popup-clear.png` 的配置态：
                            // 三组 `1`/`2`/`Quick`，下拉当前选中 `Quick`，其下一条
                            // 新增未命名命令 `Untitled`；`Quick` 排第三，与其它组一样可关。
                            use term_runtime::quickbar::{QuickBarConfig, QuickCommandConfig, QuickGroupConfig};
                            let ref_config = QuickBarConfig {
                                groups: vec![
                                    QuickGroupConfig {
                                        label: "1".into(),
                                        label_zh: String::new(),
                                        color: "#1E90FF".into(),
                                        commands: vec![],
                                    },
                                    QuickGroupConfig {
                                        label: "2".into(),
                                        label_zh: String::new(),
                                        color: "#1E90FF".into(),
                                        commands: vec![],
                                    },
                                    QuickGroupConfig {
                                        label: "Quick".into(),
                                        label_zh: String::new(),
                                        color: "#1E90FF".into(),
                                        commands: vec![QuickCommandConfig {
                                            label: "Untitled".into(),
                                            label_zh: String::new(),
                                            send: String::new(),
                                            mark: "square".into(),
                                            uuid: term_runtime::quickbar::new_uuid(),
                                        }],
                                    },
                                ],
                            };
                            // 右端图标的锚点是画布右沿（这个 Frame 内缩过），
                            // 不是窗口右沿——见 `draw_quick_command_bar` 的注释。
                            let right_edge = ui.max_rect().right();
                            crate::ui::shell::draw_quick_command_bar(
                                ui,
                                &[false, true, false, true], // 真图 ☐1 ☑2 ☐Quick
                                &ref_config,
                                // 出厂显示开关：真图即四组全显、带组标题、不字母序。
                                &term_runtime::quickbar::QuickBarDisplay::default(),
                                &mut actions,
                                term_app::i18n::Lang::En,
                                right_edge,
                            );
                        });
                });
        }
        _ => {}
    }
}

//! 独立的「快捷命令」配置弹窗 —— 按视觉基线 1:1：顶部 Group 管理、左命令
//! 列表、右 `Label / Type / Text` 表单、底部三勾选与 `Cancel / Ok`。
//!
//! 命令**内容**的编辑不放在 Preferences 的 Quick Bar 页里：那一页按视觉基线
//! 只有 `Features` 三个开关，右侧留白就是它的样子。这里另开一个窗口，
//! 入口是快捷命令栏左侧配置菜单底部的 `Edit commands`。
//!
//! `Ok` = 保存并关闭，`Cancel` = 回滚到**打开窗口那一刻**的快照并关闭。
//! 编辑期间改动即时反映到底栏（`quick_bar` 是渲染期副本），但只有 `Ok`
//! 才写 `quickbar.config`——每帧写盘会让输入框每敲一个字都触发一次 IO。
//! 三个显示开关则写 `prefs.json`（兼容终端 `user.config` 的位置）。

use eframe::egui;
use term_runtime::prefs;
use term_runtime::quickbar::{self, QuickBarConfig, QuickCommandConfig, QuickGroupConfig};

use crate::ui::kit::window::{DialogId, DialogWindow};
use crate::ui::TerminalWindow;

const LIST_W: f32 = 264.0;
const BODY_H: f32 = 286.0;
const ROW_H: f32 = 22.0;

/// 与 `quick_command_bar.rs::parse_mark` 同一张表：写错词回落 `square`。
const MARKS: [&str; 7] = ["square", "circle", "cross", "pin", "arrow", "star", "heart"];

/// 默认组名。这一组是内置的，**不可重命名 / 不可删除**（用户既定要求）。
const DEFAULT_GROUP_LABEL: &str = "Quick";

fn id(name: &'static str) -> egui::Id {
    egui::Id::new(name)
}

/// 组色预览。`#RRGGBB` 解析不了就给中性灰，绝不让配置弹窗跟着坏掉。
fn parse_color(s: &str) -> egui::Color32 {
    let hex = s.strip_prefix('#').unwrap_or(s);
    let v = u32::from_str_radix(hex, 16).unwrap_or(0x88_88_88);
    egui::Color32::from_rgb((v >> 16) as u8, (v >> 8) as u8, v as u8)
}

impl TerminalWindow {
    pub(in crate::ui) fn draw_quick_commands(&mut self, ctx: &egui::Context) {
        if !self.view.show_quick_commands {
            return;
        }
        let (g_id, c_id, snap_id, rename_id, rename_text_id) = (
            id("qc_group"),
            id("qc_command"),
            id("qc_snapshot"),
            id("qc_renaming"),
            id("qc_rename_text"),
        );
        // 首次打开记快照；Cancel 时整份回滚。关窗时置 None，下次打开重记。
        if ctx
            .data(|d| d.get_temp::<Option<QuickBarConfig>>(snap_id).unwrap_or(None))
            .is_none()
        {
            let snapshot = self.quick_bar.clone();
            ctx.data_mut(|d| d.insert_temp(snap_id, Some(snapshot)));
        }

        let mut cfg = self.quick_bar.clone();
        // 三个显示开关（兼容终端 `user.config` 的位置）从 prefs 读初值，单独存一份
        // 本地副本：勾选只改它，只有 Ok 才落 prefs（Cancel 不动盘，自动回滚）。
        let mut disp = self.quick_bar_display;
        let mut gi: usize = ctx.data(|d| d.get_temp(g_id).unwrap_or(0));
        let mut ci: usize = ctx.data(|d| d.get_temp(c_id).unwrap_or(0));
        let mut renaming: bool = ctx.data(|d| d.get_temp(rename_id).unwrap_or(false));
        let mut rename_text: String = ctx.data(|d| d.get_temp(rename_text_id).unwrap_or_default());
        if gi >= cfg.groups.len() {
            gi = 0;
        }
        let mut ok = false;
        let mut cancel = false;
        // Esc = Cancel（回滚打开时的快照）。改名态的 Esc/Enter 由控件自己处理，
        // 这里让开，避免一次按键既取消改名又关掉整个弹窗。
        if !renaming && DialogId::QuickCommands.keys().poll(ctx).cancel {
            cancel = true;
        }
        // 列表工具条只记意图，出了绘制闭包再改 Vec（遍历中不能改）。
        let mut add_cmd = false;
        let mut del_cmd = false;
        let mut up = false;
        let mut down = false;
        let mut duplicate = false;

        let open = DialogWindow::new(DialogId::QuickCommands, "Quick Commands").show(
            ctx,
            |ui| {
                ui.spacing_mut().item_spacing.y = 6.0;

                // ── 顶行：Group 下拉 + 三个组操作 ──
                ui.horizontal(|ui| {
                    ui.label("Group:");
                    if renaming {
                        ui.add(egui::TextEdit::singleline(&mut rename_text).desired_width(284.0));
                        if ui.button("Ok").clicked() {
                            let is_default =
                                cfg.groups.get(gi).is_some_and(|g| g.label == DEFAULT_GROUP_LABEL);
                            if !is_default && !rename_text.trim().is_empty() {
                                if let Some(g) = cfg.groups.get_mut(gi) {
                                    g.label = rename_text.trim().to_owned();
                                }
                            }
                            renaming = false;
                        }
                        if ui.button("Cancel").clicked() {
                            renaming = false;
                        }
                    } else {
                        let selected = cfg
                            .groups
                            .get(gi)
                            .map(|g| format!("▣  {}", g.label))
                            .unwrap_or_else(|| "(no group)".to_owned());
                        egui::ComboBox::from_id_salt("qc_group_combo")
                            .selected_text(selected)
                            .width(284.0)
                            .show_ui(ui, |ui| {
                                for (i, g) in cfg.groups.iter().enumerate() {
                                    ui.selectable_value(&mut gi, i, g.label.as_str());
                                }
                            });
                        // 组操作按钮：兼容终端是**纯文字、无边框**（用户反馈要去掉边框）。
                        if ui.add(egui::Button::new("Add Group").frame(false)).clicked() {
                            cfg.groups.push(QuickGroupConfig {
                                label: "Untitled".into(),
                                label_zh: String::new(),
                                color: "#1E90FF".into(),
                                commands: vec![QuickCommandConfig {
                                    label: "Untitled".into(),
                                    label_zh: String::new(),
                                    send: String::new(),
                                    mark: "square".into(),
                                    uuid: quickbar::new_uuid(),
                                }],
                            });
                            gi = cfg.groups.len() - 1;
                            ci = 0;
                        }
                        // Remove / Rename：组数 > 1，且**默认 Quick 组不可改**。
                        let is_default =
                            cfg.groups.get(gi).is_some_and(|g| g.label == DEFAULT_GROUP_LABEL);
                        let can_edit = cfg.groups.len() > 1 && !is_default;
                        if ui
                            .add_enabled(can_edit, egui::Button::new("Remove Group").frame(false))
                            .clicked()
                        {
                            cfg.groups.remove(gi);
                            gi = gi.min(cfg.groups.len().saturating_sub(1));
                            ci = 0;
                        }
                        if ui
                            .add_enabled(can_edit, egui::Button::new("Rename Group").frame(false))
                            .clicked()
                        {
                            rename_text = cfg
                                .groups
                                .get(gi)
                                .map(|g| g.label.clone())
                                .unwrap_or_default();
                            renaming = true;
                        }
                    }
                });
                ui.add_space(2.0);

                // ── 主体：左命令列表 / 右属性表单 ──
                ui.horizontal(|ui| {
                    // 左：带边框的列表框 + 底部 + - ▲ ▼ Duplicate 工具条。
                    egui::Frame::none()
                        .stroke(egui::Stroke::new(1.0, egui::Color32::from_rgb(0x4A, 0x4A, 0x4A)))
                        .rounding(3.0)
                        .show(ui, |ui| {
                            ui.set_width(LIST_W);
                            ui.set_min_height(BODY_H);
                            // bottom_up：先画的工具条钉在框底，后画的列表吃掉剩余高度。
                            ui.with_layout(egui::Layout::bottom_up(egui::Align::LEFT), |ui| {
                                ui.set_width(ui.available_width());
                                ui.horizontal(|ui| {
                                    add_cmd = ui.small_button("+").on_hover_text("Add").clicked();
                                    del_cmd = ui.small_button("-").on_hover_text("Remove").clicked();
                                    up = ui.small_button("▲").on_hover_text("Move up").clicked();
                                    down = ui.small_button("▼").on_hover_text("Move down").clicked();
                                    ui.with_layout(
                                        egui::Layout::right_to_left(egui::Align::Center),
                                        |ui| {
                                            duplicate = ui.button("Duplicate").clicked();
                                        },
                                    );
                                });
                                ui.separator();
                                egui::ScrollArea::vertical().show(ui, |ui| {
                                    // bottom_up 会继承到列表内部；显式改回
                                    // top_down，命令才从框顶正序往下排。
                                    ui.with_layout(
                                        egui::Layout::top_down(egui::Align::LEFT),
                                        |ui| {
                                    ui.set_width(ui.available_width());
                                    let names: Vec<String> = cfg
                                        .groups
                                        .get(gi)
                                        .map(|g| g.commands.iter().map(|c| c.label.clone()).collect())
                                        .unwrap_or_default();
                                    for (i, name) in names.iter().enumerate() {
                                        let (rect, resp) = ui.allocate_exact_size(
                                            egui::vec2(ui.available_width(), ROW_H),
                                            egui::Sense::click(),
                                        );
                                        let p = ui.painter();
                                        if i == ci || resp.hovered() {
                                            p.rect_filled(
                                                rect,
                                                0.0,
                                                egui::Color32::from_rgb(0x3A, 0x3A, 0x3A),
                                            );
                                        }
                                        p.text(
                                            egui::pos2(rect.left() + 8.0, rect.center().y),
                                            egui::Align2::LEFT_CENTER,
                                            name,
                                            egui::FontId::default(),
                                            egui::Color32::from_rgb(0xDD, 0xDD, 0xDD),
                                        );
                                        if resp.clicked() {
                                            ci = i;
                                        }
                                    }
                                        },
                                    );
                                });
                            });
                        });

                    ui.add_space(8.0);

                    // 右：Label / Type / Text。
                    ui.vertical(|ui| {
                        ui.set_min_height(BODY_H);
                        let exists = cfg
                            .groups
                            .get(gi)
                            .is_some_and(|g| !g.commands.is_empty());
                        if exists {
                            let group_color = cfg.groups[gi].color.clone();
                            let g = &mut cfg.groups[gi];
                            if ci >= g.commands.len() {
                                ci = 0;
                            }
                            let c = &mut g.commands[ci];
                            ui.horizontal(|ui| {
                                ui.label("Label:");
                                let label_w = (ui.available_width() - 116.0).max(90.0);
                                ui.add(egui::TextEdit::singleline(&mut c.label).desired_width(label_w));
                                egui::ComboBox::from_id_salt("qc_mark_combo")
                                    .selected_text(c.mark.as_str())
                                    .width(70.0)
                                    .show_ui(ui, |ui| {
                                        for m in MARKS {
                                            ui.selectable_value(&mut c.mark, m.to_owned(), m);
                                        }
                                    });
                                // 视觉基线 Label 行右端那颗彩色标记：当前组的颜色。
                                ui.label(
                                    egui::RichText::new("✦").color(parse_color(&group_color)).strong(),
                                );
                            });
                            ui.horizontal(|ui| {
                                ui.label("Type:");
                                // 目前只实现「发送文本」，不摆不能用的假选项。
                                let w = ui.available_width();
                                egui::ComboBox::from_id_salt("qc_type_combo")
                                    .selected_text("Send Text")
                                    .width(w)
                                    .show_ui(ui, |ui| {
                                        let _ = ui.selectable_label(true, "Send Text");
                                    });
                            });
                            ui.label("Text:");
                            let w = ui.available_width();
                            ui.add(
                                egui::TextEdit::multiline(&mut c.send)
                                    .desired_rows(13)
                                    .desired_width(w),
                            );
                        } else {
                            ui.label(egui::RichText::new("No command — press + to add one.").weak());
                        }
                    });
                });

                // ── 底部三勾选（文案逐字照视觉基线）──
                // 这三个是「显示开关」，兼容终端存于 user.config（≈本应用 prefs），
                // 不随命令表落 quickbar.config；勾选只改本地 `disp`，Ok 才写 prefs。
                ui.add_space(4.0);
                ui.horizontal(|ui| {
                    ui.checkbox(&mut disp.show_multiple_groups, "Show multiple groups");
                    ui.checkbox(&mut disp.show_group_title, "Show group title");
                    ui.checkbox(
                        &mut disp.alphabetical,
                        "Show commands in alphabetical order",
                    );
                });

                // ── Cancel / Ok 贴右下 ──
                // 走 kit::footer 原语：尺寸沿用裸 `ui.button` 的文字口径，
                // 配色/描边来自 `kit::spec`，与其余弹窗页脚同源。
                ui.add_space(2.0);
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    use crate::ui::kit::spec::FooterButton as FB;
                    let ok_spec = FB::QuickOk
                        .spec()
                        .with_size(crate::ui::kit::footer::text_button_size(ui, "Ok"));
                    let cancel_spec = FB::QuickCancel
                        .spec()
                        .with_size(crate::ui::kit::footer::text_button_size(ui, "Cancel"));
                    if crate::ui::kit::footer::widget_button(ui, "Ok", ok_spec, true).clicked() {
                        ok = true;
                    }
                    if crate::ui::kit::footer::widget_button(ui, "Cancel", cancel_spec, true)
                        .clicked()
                    {
                        cancel = true;
                    }
                });
            },
        );

        // 一次性应用列表工具条（遍历中不能改 Vec）。
        if let Some(g) = cfg.groups.get_mut(gi) {
            if add_cmd {
                g.commands.push(QuickCommandConfig {
                    label: "Untitled".into(),
                    label_zh: String::new(),
                    send: String::new(),
                    mark: "square".into(),
                    uuid: quickbar::new_uuid(),
                });
                ci = g.commands.len() - 1;
            }
            if del_cmd && !g.commands.is_empty() {
                g.commands.remove(ci.min(g.commands.len() - 1));
                ci = ci.min(g.commands.len().saturating_sub(1));
            }
            if duplicate && !g.commands.is_empty() {
                let at = ci.min(g.commands.len() - 1);
                let copy = g.commands[at].clone();
                g.commands.insert(at, copy);
            }
            if up && ci > 0 && ci < g.commands.len() {
                g.commands.swap(ci, ci - 1);
                ci -= 1;
            }
            if down && ci + 1 < g.commands.len() {
                g.commands.swap(ci, ci + 1);
                ci += 1;
            }
        }

        ctx.data_mut(|d| {
            d.insert_temp(g_id, gi);
            d.insert_temp(c_id, ci);
            d.insert_temp(rename_id, renaming);
            d.insert_temp(rename_text_id, rename_text);
        });

        if cancel {
            if let Some(snapshot) =
                ctx.data(|d| d.get_temp::<Option<QuickBarConfig>>(snap_id).unwrap_or(None))
            {
                self.quick_bar = snapshot;
            }
        } else {
            self.quick_bar = cfg;
            if ok {
                quickbar::save(&self.quick_bar);
                // 三个显示开关落 prefs（兼容终端 user.config 的位置）；并同步窗口上的
                // 渲染副本，底栏当帧就反映勾选结果。命令表与开关各写各的文件。
                let mut p = prefs::load().unwrap_or_default();
                p.quickbar_show_multiple_groups = disp.show_multiple_groups;
                p.quickbar_show_group_title = disp.show_group_title;
                p.quickbar_alphabetical = disp.alphabetical;
                prefs::save(&p);
                self.quick_bar_display = disp;
            }
        }
        if ok || cancel || !open {
            self.view.show_quick_commands = false;
            ctx.data_mut(|d| d.insert_temp(snap_id, None::<QuickBarConfig>));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_mark_list_covers_every_factory_shape() {
        for g in QuickBarConfig::default().groups {
            for c in g.commands {
                assert!(MARKS.contains(&c.mark.as_str()), "{} 的图形没进下拉", c.label);
            }
        }
    }

    #[test]
    fn the_three_switches_have_the_expected_factory_state() {
        // 三个显示开关出厂态（兼容终端 user.config 位置）：显示多组 + 显示组标题、
        // 不按字母序——main-dark.png 的 File 组顺序不是字母序。
        let d = term_runtime::quickbar::QuickBarDisplay::default();
        assert!(d.show_multiple_groups);
        assert!(d.show_group_title);
        assert!(!d.alphabetical, "出厂保持配置顺序，守住视觉基线底栏");
    }

    #[test]
    fn a_malformed_color_falls_back_to_neutral_gray() {
        assert_eq!(parse_color("#1E90FF"), egui::Color32::from_rgb(0x1E, 0x90, 0xFF));
        assert_eq!(parse_color("nope"), egui::Color32::from_rgb(0x88, 0x88, 0x88));
    }
}

//! 脚本注册表管理页：左侧方案列表 + 右侧字段 + 页脚 Cancel/Ok。
//!
//! 参照 OneKeys 配置弹窗的「草稿 + 整份写回」模型：改动只发生在 `draft`，
//! Ok 通过 `save_prefs` 整份写回 `prefs.scripts`，Cancel 直接丢弃草稿。
//! 脚本源码仍在 `profile/scripts/*.rhai`，这里只登记元数据并做验证。

use super::dialog_chrome::framed_button;
use super::settings_live::save_prefs;
use crate::ui::kit::window::{DialogId, DialogWindow};
use crate::ui::*;
use term_profile::prefs::ScriptEntry;

const LIST_W: f32 = 240.0;
const FIELD_W: f32 = 360.0;
const ROW_H: f32 = 22.0;

struct ValidationError(String);

/// 管理页的跨帧编辑态。挂在 `TerminalWindow` 上：含未落盘的草稿。
#[derive(Default)]
pub(in crate::ui) struct ScriptManagerEdit {
    draft: Option<Vec<ScriptEntry>>,
    selected: Option<usize>,
    name: String,
    path: String,
    args: String,
    description: String,
    enabled: bool,
    error: Option<String>,
    new_modal: bool,
    new_name: String,
}

impl ScriptManagerEdit {
    pub(in crate::ui) fn set_path(&mut self, path: String) {
        self.path = path;
        self.error = None;
    }
}

impl TerminalWindow {
    pub(in crate::ui) fn open_script_manager(&mut self) {
        let scripts = term_profile::prefs::load()
            .map(|p| p.scripts)
            .unwrap_or_default();
        let mut edit = ScriptManagerEdit {
            draft: Some(scripts),
            ..Default::default()
        };
        if edit.draft.as_ref().is_some_and(|v| !v.is_empty()) {
            edit.selected = Some(0);
        }
        self.script_manager = Some(edit);
        self.ctx.request_repaint();
    }

    pub(in crate::ui) fn draw_script_manager(&mut self, ctx: &egui::Context) {
        let Some(mut edit) = self.script_manager.take() else {
            return;
        };
        if edit.draft.is_none() {
            edit.draft = Some(
                term_profile::prefs::load()
                    .map(|p| p.scripts)
                    .unwrap_or_default(),
            );
        }
        let mut close = false;
        let open = DialogWindow::new(DialogId::ScriptManager, "Manage Scripts").show(ctx, |ui| {
            ui.set_min_size(egui::vec2(LIST_W + FIELD_W + 48.0, 420.0));
            if edit.new_modal {
                draw_new_modal(ui, &mut edit);
            }
            ui.horizontal_top(|ui| {
                ui.vertical(|ui| {
                    ui.set_width(LIST_W);
                    draw_list(ui, &mut edit);
                });
                ui.separator();
                ui.vertical(|ui| {
                    ui.set_width(FIELD_W);
                    draw_fields(self, ui, &mut edit);
                });
            });
            ui.separator();
            ui.horizontal(|ui| {
                if let Some(error) = &edit.error {
                    ui.colored_label(egui::Color32::from_rgb(0xE0, 0x60, 0x60), error);
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if framed_button(ui, "Cancel", true, false).clicked() {
                        close = true;
                    }
                    if framed_button(ui, "Ok", true, true).clicked() {
                        match validate(&mut edit) {
                            Ok(entries) => {
                                save_prefs(|p| p.scripts = entries);
                                close = true;
                            }
                            Err(message) => edit.error = Some(message.0),
                        }
                    }
                });
            });
        });
        if !open || close {
            self.ctx.request_repaint();
        } else {
            self.script_manager = Some(edit);
        }
    }
}

fn draw_list(ui: &mut egui::Ui, edit: &mut ScriptManagerEdit) {
    ui.horizontal(|ui| {
        if ui.button("+").clicked() {
            edit.new_modal = true;
            edit.new_name.clear();
        }
        if ui.button("-").clicked() {
            if let (Some(i), Some(draft)) = (edit.selected, edit.draft.as_mut()) {
                if i < draft.len() {
                    draft.remove(i);
                    edit.selected = None;
                    edit.name.clear();
                    edit.path.clear();
                }
            }
        }
        if ui.button("Duplicate").clicked() {
            if let (Some(i), Some(draft)) = (edit.selected, edit.draft.as_mut()) {
                if let Some(entry) = draft.get(i) {
                    let mut copy = entry.clone();
                    copy.name = format!("{} (copy)", copy.name);
                    draft.push(copy);
                    edit.selected = Some(draft.len() - 1);
                }
            }
        }
    });
    egui::ScrollArea::vertical().max_height(300.0).show(ui, |ui| {
        if let Some(draft) = edit.draft.as_ref() {
            for (i, entry) in draft.iter().enumerate() {
                let label = if entry.enabled {
                    entry.name.clone()
                } else {
                    format!("{} (disabled)", entry.name)
                };
                if ui.selectable_label(edit.selected == Some(i), label).clicked() {
                    edit.selected = Some(i);
                    edit.name = entry.name.clone();
                    edit.path = entry.path.clone();
                    edit.args = entry.args.join(" ");
                    edit.description = entry.description.clone();
                    edit.enabled = entry.enabled;
                    edit.error = None;
                }
            }
        }
    });
}

fn draw_fields(win: &mut TerminalWindow, ui: &mut egui::Ui, edit: &mut ScriptManagerEdit) {
    ui.label("Name:");
    ui.add_sized(egui::vec2(FIELD_W, ROW_H), egui::TextEdit::singleline(&mut edit.name));
    ui.label("Script file:");
    ui.horizontal(|ui| {
        ui.add_sized(
            egui::vec2(FIELD_W - 48.0, ROW_H),
            egui::TextEdit::singleline(&mut edit.path).hint_text("scripts/demo.rhai"),
        );
        if ui.button("...").clicked() {
            let current = edit.path.clone();
            win.open_path_picker(crate::ui::path_picker::PathTarget::ScriptPath, &current);
        }
    });
    ui.label("Arguments:");
    ui.add_sized(egui::vec2(FIELD_W, ROW_H), egui::TextEdit::singleline(&mut edit.args));
    ui.label("Description:");
    ui.add_sized(
        egui::vec2(FIELD_W, ROW_H),
        egui::TextEdit::singleline(&mut edit.description),
    );
    ui.checkbox(&mut edit.enabled, "Enabled");
    if edit.selected.is_some() {
        ui.colored_label(
            egui::Color32::from_rgb(0x88, 0x88, 0x88),
            "脚本源码保存在 profile/scripts/*.rhai；这里只登记入口。",
        );
    }
}

fn draw_new_modal(ui: &mut egui::Ui, edit: &mut ScriptManagerEdit) {
    let ctx = ui.ctx();
    let center = ctx.screen_rect().center();
    egui::Area::new(egui::Id::new("script_new_modal"))
        .fixed_pos(center - egui::vec2(180.0, 55.0))
        .order(egui::Order::Foreground)
        .show(ctx, |ui| {
            egui::Frame::none()
                .fill(egui::Color32::from_rgb(0x1C, 0x1C, 0x1C))
                .stroke(egui::Stroke::new(1.0, egui::Color32::from_rgb(0x44, 0x44, 0x44)))
                .inner_margin(egui::Margin::same(10.0))
                .show(ui, |ui| {
                    ui.set_width(340.0);
                    ui.label("Enter a name for the new script:");
                    ui.text_edit_singleline(&mut edit.new_name);
                    ui.horizontal(|ui| {
                        if framed_button(ui, "Cancel", true, false).clicked() {
                            edit.new_modal = false;
                        }
                        if framed_button(ui, "Create", true, true).clicked() {
                            let name = edit.new_name.trim().to_string();
                            let unique = edit
                                .draft
                                .as_ref()
                                .map_or(true, |v| v.iter().all(|e| e.name != name));
                            if !name.is_empty() && unique {
                                let entry = ScriptEntry {
                                    name: name.clone(),
                                    path: String::new(),
                                    args: Vec::new(),
                                    description: String::new(),
                                    enabled: true,
                                };
                                if let Some(draft) = edit.draft.as_mut() {
                                    draft.push(entry);
                                    edit.selected = Some(draft.len() - 1);
                                }
                                edit.name = name;
                                edit.path.clear();
                                edit.args.clear();
                                edit.description.clear();
                                edit.enabled = true;
                                edit.new_modal = false;
                                edit.error = None;
                            } else {
                                edit.error = Some("名字不能为空，且不能与已有脚本重名".to_string());
                            }
                        }
                    });
                });
        });
}

/// 纯验证 + 把界面草稿收敛成 `Vec<ScriptEntry>`。
fn validate(edit: &mut ScriptManagerEdit) -> Result<Vec<ScriptEntry>, ValidationError> {
    let Some(draft) = edit.draft.as_ref() else {
        return Ok(Vec::new());
    };
    let mut names = std::collections::BTreeSet::new();
    let mut out = Vec::new();
    for (i, entry) in draft.iter().enumerate() {
        let active = edit.selected == Some(i);
        let name = if active { edit.name.trim() } else { entry.name.trim() }.to_string();
        let path = if active { edit.path.trim() } else { entry.path.trim() }.to_string();
        let description = if active { edit.description.trim() } else { entry.description.trim() }.to_string();
        let args = if active {
            split_args(&edit.args)
        } else {
            entry.args.clone()
        };
        let enabled = if active { edit.enabled } else { entry.enabled };
        if name.is_empty() {
            return Err(ValidationError(format!("第 {} 条脚本的名字不能为空", i + 1)));
        }
        if !names.insert(name.clone()) {
            return Err(ValidationError(format!("脚本名重复：{name}")));
        }
        if !path.is_empty() {
            if !path.ends_with(".rhai") {
                return Err(ValidationError(format!("{name} 的脚本文件必须是 .rhai")));
            }
            if !std::path::Path::new(&path).is_file() {
                return Err(ValidationError(format!("{name} 的脚本文件不存在：{path}")));
            }
            let source = std::fs::read_to_string(&path)
                .map_err(|e| ValidationError(format!("读不了 {path}：{e}")))?;
            term_core::script_caps::parse_header(&source)
                .map_err(|e| ValidationError(format!("{name} 的授权头写错了：{e}")))?;
        }
        out.push(ScriptEntry { name, path, args, description, enabled });
    }
    Ok(out)
}

fn split_args(s: &str) -> Vec<String> {
    s.split_whitespace().map(str::to_string).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validation_rejects_duplicate_names_and_bad_paths() {
        let mut edit = ScriptManagerEdit {
            draft: Some(vec![
                ScriptEntry { name: "a".into(), path: String::new(), args: vec![], description: String::new(), enabled: true },
                ScriptEntry { name: "b".into(), path: String::new(), args: vec![], description: String::new(), enabled: true },
            ]),
            name: "a".into(),
            selected: Some(1),
            ..Default::default()
        };
        assert!(validate(&mut edit).unwrap_err().0.contains("重复"));
        edit.name = "b".into();
        edit.path = "demo.txt".into();
        assert!(validate(&mut edit).unwrap_err().0.contains(".rhai"));
    }
}

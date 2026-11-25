//! 统一的文件/目录选择器。
//!
//! 不需要原生文件对话框（那个会因平台不同而破坏视觉基线，还要新依赖）：
//! 这里用 `term_io::local_dir::list` 列目录，点目录进入、点文件选中，路径
//! 也可以直接敲。Bell 声音文件与 Modem 上传/下载目录共用同一份状态与弹层。

use eframe::egui;
use std::path::PathBuf;

use crate::ui::*;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::ui) enum PathTarget {
    BellSound,
    ModemUploadDir,
    ModemDownloadDir,
    /// 首次启动「Choose Profiles Directory」的指定目录。
    ProfileDir,
    /// Authentication 页的身份文件；`usize` 是平台页签下标，写回对应那一格。
    AuthIdentity(usize),
    /// 脚本注册表里选择脚本文件。
    ScriptPath,
}

pub(in crate::ui) struct PathPicker {
    target: PathTarget,
    dir_text: String,
    selected: String,
    error: Option<String>,
}

impl TerminalWindow {
    /// 从控件当前值打开选择器：已填的路径直接进入它的父目录。
    pub(in crate::ui) fn open_path_picker(&mut self, target: PathTarget, current: &str) {
        let path = PathBuf::from(current);
        let (dir, selected) = if path.is_dir() {
            (path.clone(), String::new())
        } else {
            (
                path.parent().filter(|p| !p.as_os_str().is_empty()).map(PathBuf::from)
                    .unwrap_or_else(term_io::local_dir::home),
                path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default(),
            )
        };
        self.path_picker = Some(PathPicker {
            target,
            dir_text: dir.to_string_lossy().into_owned(),
            selected,
            error: None,
        });
    }
}

fn apply(win: &mut TerminalWindow, target: PathTarget, path: String) {
    match target {
        PathTarget::BellSound => win.new_session.bell_sound = path,
        PathTarget::ModemUploadDir => win.new_session.modem_upload_dir = path,
        PathTarget::ModemDownloadDir => win.new_session.modem_download_dir = path,
        PathTarget::ProfileDir => win.profile_choice.custom_dir = path,
        PathTarget::AuthIdentity(tab) => win.new_session.auth_identity[tab] = path,
        PathTarget::ScriptPath => {
            if let Some(edit) = win.script_manager.as_mut() {
                edit.set_path(path);
            }
        }
    }
}

/// 每帧一次：选择弹层开着时画它；关着时零窗口、零 IO。
pub(in crate::ui) fn draw(ctx: &egui::Context, win: &mut TerminalWindow) {
    let Some(mut picker) = win.path_picker.take() else {
        return;
    };
    let title = match picker.target {
        PathTarget::BellSound => "Choose sound file",
        PathTarget::AuthIdentity(_) => "Choose identity file",
        PathTarget::ScriptPath => "Choose script file",
        PathTarget::ModemUploadDir | PathTarget::ModemDownloadDir | PathTarget::ProfileDir => {
            "Choose directory"
        }
    };
    let mut chosen: Option<String> = None;
    let mut cancel = false;
    // `show` 返回 None = 用户点了标题栏 ×（或窗口被外部收掉）。过去不看来
    // 返回值、无条件 `win.path_picker = Some(picker)`，于是 × 掉的下一帧
    // 选择器立刻重开，根本关不掉。
    let shown = egui::Window::new(title)
        .collapsible(false)
        .resizable(false)
        .anchor(egui::Align2::CENTER_CENTER, egui::Vec2::ZERO)
        .show(ctx, |ui| {
            ui.horizontal(|ui| {
                ui.label("Directory:");
                ui.add(
                    egui::TextEdit::singleline(&mut picker.dir_text)
                        .desired_width(320.0),
                );
            });
            let dir = PathBuf::from(&picker.dir_text);
            if ui.button("Up").clicked() {
                if let Some(parent) = dir.parent() {
                    picker.dir_text = parent.to_string_lossy().into_owned();
                    picker.error = None;
                }
            }
            let entries = term_io::local_dir::list(&dir, false);
            if entries.is_empty() {
                ui.label("Empty or unreadable directory");
            }
            egui::ScrollArea::vertical().max_height(220.0).show(ui, |ui| {
                for row in &entries {
                    let label = if row.is_dir {
                        format!("▸ {}/", row.name)
                    } else {
                        format!("  {}", row.name)
                    };
                    if ui.selectable_label(picker.selected == row.name, label).clicked() {
                        picker.selected = row.name.clone();
                        if row.is_dir {
                            picker.dir_text = dir.join(&row.name).to_string_lossy().into_owned();
                            picker.selected.clear();
                        }
                    }
                }
            });
            ui.horizontal(|ui| {
                ui.label("Selected:");
                ui.add(egui::TextEdit::singleline(&mut picker.selected).desired_width(260.0));
            });
            if let Some(err) = &picker.error {
                ui.colored_label(egui::Color32::from_rgb(0xE0, 0x60, 0x60), err);
            }
            ui.horizontal(|ui| {
                if ui.button("OK").clicked() {
                    let name = picker.selected.trim();
                    let path = if name.is_empty() {
                        dir.clone()
                    } else {
                        dir.join(name)
                    };
                    if path.exists() {
                        chosen = Some(path.to_string_lossy().into_owned());
                    } else {
                        picker.error = Some("路径不存在".to_string());
                    }
                }
                if ui.button("Cancel").clicked() {
                    cancel = true;
                }
            });
        });
    if let Some(path) = chosen {
        apply(win, picker.target, path);
    } else if !cancel && shown.is_some() {
        win.path_picker = Some(picker);
    }
    // Cancel 或标题栏 ×：picker 随 `take()` 一起丢弃，不重新挂回去。
}

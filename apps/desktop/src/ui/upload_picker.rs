//! 多选上传选择器（官网 page/21：一次上传多个文件夹或文件）。
//!
//! egui/eframe 没有原生「多选文件」对话框，原生对话框还会因平台不同而把视觉基线
//! 打乱。这里沿用 `path_picker.rs` 的同一份本地目录数据源（`term_io::local_dir`），
//! 画一个真正的模态窗口：每行一个勾选框，已选清单单独回显，OK 把每条路径交给
//! 既有的 `start_filer_transfer`（目录自动走 `enqueue_upload_tree`），Cancel 或关窗
//! 直接丢弃选择。
//!
//! 入口在 Filer 面板的右键菜单（`filer_view.rs`），不往路径栏塞新按钮：那排按钮的
//! 几何是视觉基线的一部分，多一颗就得重冻。

use eframe::egui;
use std::path::{Path, PathBuf};

use crate::ui::panels::filer::xfer::XferPick;
use crate::ui::*;

/// 一次多选上传的跨帧状态。`None` = 弹层没开，零窗口、零 IO。
pub(in crate::ui) struct UploadPicker {
    /// 当前正在列的本地目录（可直接改，和 `path_picker` 一样支持手输）。
    dir_text: String,
    /// 手输的条目名 / 路径；回车或点 Add 加入已选。
    typed: String,
    /// 已选的本地路径（绝对路径字符串，按勾选顺序、去重）。这也是回显清单。
    selected: Vec<String>,
    /// 本地校验失败时的一句话（不存在的手输路径 / 空选择）。
    error: Option<String>,
}

impl UploadPicker {
    pub(in crate::ui) fn new(start: PathBuf) -> Self {
        Self {
            dir_text: start.to_string_lossy().into_owned(),
            typed: String::new(),
            selected: Vec::new(),
            error: None,
        }
    }

    /// 勾选 / 取消一条：按字符串去重，保持勾选顺序（入队顺序就是它）。
    fn toggle(&mut self, path: &str) {
        match self.selected.iter().position(|p| p == path) {
            Some(i) => {
                self.selected.remove(i);
            }
            None => self.selected.push(path.to_string()),
        }
    }
}

/// 本地路径列表 → 上传意图。**纯映射**：不碰 egui、不碰会话，只读路径自身。
///
/// 不存在的路径与没有文件名的路径（`/`）直接跳过——把坏路径送进队列只会得到一条
/// 永远失败的任务。`is_dir` 由路径自身判定，落地时据此选 `enqueue_upload_tree`；
/// `size` 沿用 `modem_drop.rs` / 本地行右键的既有口径 0（进度由传输事件回填）。
pub(in crate::ui) fn paths_to_upload_picks(paths: &[String]) -> Vec<XferPick> {
    paths
        .iter()
        .filter_map(|local| {
            let path = Path::new(local);
            if !path.exists() {
                return None;
            }
            let name = path.file_name()?.to_string_lossy().into_owned();
            Some(XferPick::Upload {
                local: local.clone(),
                name,
                size: 0,
                is_dir: path.is_dir(),
            })
        })
        .collect()
}

/// 确认时如果有路径已经失效（`paths_to_upload_picks` 丢弃的），必须在回执里
/// 写明条数；全被丢弃时给出明确错误，不能画成「上传成功」。
fn append_dropped_notice(notes: &mut Vec<String>, dropped: usize) {
    if dropped == 0 {
        return;
    }
    notes.push(if notes.is_empty() {
        format!("所选 {dropped} 条路径都已失效，未入队")
    } else {
        format!("{dropped} 条路径已失效，未入队")
    });
}

/// 把手输的一行解析成绝对路径并加入选择；相对路径以当前列出的目录为基准。
fn add_typed(picker: &mut UploadPicker, dir: &Path) {
    let raw = picker.typed.trim();
    if raw.is_empty() {
        return;
    }
    let path = dir.join(raw);
    if !path.exists() {
        picker.error = Some(format!("路径不存在：{}", path.display()));
        return;
    }
    let full = path.to_string_lossy().into_owned();
    if !picker.selected.contains(&full) {
        picker.selected.push(full);
    }
    picker.typed.clear();
    picker.error = None;
}

impl TerminalWindow {
    /// 从 Filer 当前本地目录起开多选上传弹层。
    pub(in crate::ui) fn open_upload_picker(&mut self, start: PathBuf) {
        self.upload_picker = Some(UploadPicker::new(start));
    }
}

/// 每帧一次：弹层开着时画它；关着时零窗口、零 IO。
pub(in crate::ui) fn draw(ctx: &egui::Context, win: &mut TerminalWindow) {
    let Some(mut picker) = win.upload_picker.take() else {
        return;
    };
    // 目标目录只是回给用户核对；真正固定到任务里的是落地那一刻的 filer.path。
    let target = win.filer_remote_dir().to_string();
    let show_hidden = win.filer.show_hidden;
    let target_text = if target.is_empty() {
        "<unknown>".to_string()
    } else {
        target
    };
    let mut confirmed = false;
    let mut cancel = false;
    let mut remove_at: Option<usize> = None;

    let opened = egui::Window::new("Upload files / folders")
        .collapsible(false)
        .resizable(true)
        .anchor(egui::Align2::CENTER_CENTER, egui::Vec2::ZERO)
        .show(ctx, |ui| {
            ui.horizontal(|ui| {
                ui.label("Directory:");
                ui.add(egui::TextEdit::singleline(&mut picker.dir_text).desired_width(300.0));
                if ui.button("↑").clicked() {
                    if let Some(parent) = PathBuf::from(&picker.dir_text).parent() {
                        picker.dir_text = parent.to_string_lossy().into_owned();
                        picker.error = None;
                    }
                }
            });
            ui.label(format!("Target: {target_text}"));

            let dir = PathBuf::from(&picker.dir_text);
            let entries = term_io::local_dir::list(&dir, show_hidden);
            egui::ScrollArea::vertical().max_height(200.0).show(ui, |ui| {
                if entries.is_empty() {
                    ui.label("Empty or unreadable directory");
                }
                for row in &entries {
                    let full = dir.join(&row.name);
                    let full = full.to_string_lossy().into_owned();
                    ui.horizontal(|ui| {
                        // 勾选框只管选择；点名字（目录）才进入下一级。两件事分开，
                        // 不然「想选这个文件夹」会变成「钻进去」，多选根本选不上。
                        let mut checked = picker.selected.contains(&full);
                        if ui.checkbox(&mut checked, "").clicked() {
                            picker.toggle(&full);
                            picker.error = None;
                        }
                        let label = if row.is_dir {
                            format!("▸ {}/", row.name)
                        } else {
                            row.name.clone()
                        };
                        if ui.selectable_label(false, label).clicked() && row.is_dir {
                            picker.dir_text = full;
                            picker.error = None;
                        }
                    });
                }
            });

            ui.separator();
            ui.horizontal(|ui| {
                ui.label("Add path:");
                let typed = ui.add(
                    egui::TextEdit::singleline(&mut picker.typed).desired_width(240.0),
                );
                if typed.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                    add_typed(&mut picker, &dir);
                }
                if ui.button("Add").clicked() {
                    add_typed(&mut picker, &dir);
                }
            });

            ui.separator();
            ui.label(format!("Selected ({}):", picker.selected.len()));
            egui::ScrollArea::vertical().max_height(80.0).show(ui, |ui| {
                if picker.selected.is_empty() {
                    ui.label(
                        egui::RichText::new("勾选上面的条目，或手输路径后点 Add")
                            .weak()
                            .small(),
                    );
                }
                for (i, path) in picker.selected.iter().enumerate() {
                    ui.horizontal(|ui| {
                        ui.label(egui::RichText::new(path).small());
                        if ui.small_button("×").clicked() {
                            remove_at = Some(i);
                        }
                    });
                }
            });
            if let Some(err) = &picker.error {
                ui.colored_label(egui::Color32::from_rgb(0xE0, 0x60, 0x60), err);
            }
            ui.horizontal(|ui| {
                if ui.button("Upload selected").clicked() {
                    if picker.selected.is_empty() {
                        picker.error = Some("尚未选择任何文件/目录".to_string());
                    } else {
                        confirmed = true;
                    }
                }
                if ui.button("Cancel").clicked() {
                    cancel = true;
                }
            });
        });
    // 点标题栏的 × 也算取消，不能让状态卡在内存里下次一开还在。
    if opened.is_none() {
        cancel = true;
    }
    if let Some(i) = remove_at {
        if i < picker.selected.len() {
            picker.selected.remove(i);
        }
    }

    if confirmed {
        // 逐条进既有队列：目录由 start_filer_transfer 自动走 enqueue_upload_tree。
        // 回执沿用 Filer 状态行那一个槽，不新开提示位。
        let picks = paths_to_upload_picks(&picker.selected);
        let dropped = picker.selected.len().saturating_sub(picks.len());
        let mut notes: Vec<String> = picks
            .into_iter()
            .map(|pick| win.start_filer_transfer(pick))
            .collect();
        append_dropped_notice(&mut notes, dropped);
        win.note_multi_upload(notes);
        ctx.request_repaint();
    } else if !cancel {
        win.upload_picker = Some(picker);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 用仓库内一定存在的路径，测试不依赖家目录内容，也不写临时文件。
    fn repo_path(rel: &str) -> String {
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join(rel)
            .to_string_lossy()
            .into_owned()
    }

    #[test]
    fn directories_and_files_get_distinct_upload_intents() {
        let dir = repo_path("../../crates/term-io/src");
        let file = repo_path("../../crates/term-io/src/local_dir.rs");
        let picks = paths_to_upload_picks(&[dir.clone(), file.clone()]);
        assert_eq!(picks.len(), 2, "目录和文件都要各出一条上传意图");
        match &picks[0] {
            XferPick::Upload {
                local,
                name,
                is_dir,
                ..
            } => {
                assert_eq!(local, &dir);
                assert_eq!(name, "src");
                assert!(*is_dir, "目录必须带 is_dir 才会走树上传");
            }
            _ => panic!("目录应映射成 Upload 意图"),
        }
        match &picks[1] {
            XferPick::Upload {
                local,
                name,
                is_dir,
                ..
            } => {
                assert_eq!(local, &file);
                assert_eq!(name, "local_dir.rs");
                assert!(!*is_dir, "普通文件仍走单文件上传");
            }
            _ => panic!("文件应映射成 Upload 意图"),
        }
    }

    #[test]
    fn missing_paths_and_root_without_file_name_are_skipped() {
        let missing = repo_path("../../crates/definitely-not-here-page21");
        let picks = paths_to_upload_picks(&[missing, "/".to_string(), String::new()]);
        assert!(picks.is_empty(), "坏路径不该变成一条永远失败的任务");
    }

    #[test]
    fn caller_order_is_preserved() {
        let a = repo_path("../../crates/term-io/src/local_dir.rs");
        let b = repo_path("../../crates/term-io/Cargo.toml");
        let picks = paths_to_upload_picks(&[b.clone(), a.clone()]);
        assert_eq!(picks.len(), 2);
        match &picks[0] {
            XferPick::Upload { local, .. } => assert_eq!(local, &b),
            _ => panic!("类型不对"),
        }
        match &picks[1] {
            XferPick::Upload { local, .. } => assert_eq!(local, &a),
            _ => panic!("类型不对"),
        }
    }

    #[test]
    fn confirm_receipt_reports_dropped_paths_and_never_looks_silent() {
        let mut notes = vec!["已入队 a".to_string()];
        append_dropped_notice(&mut notes, 2);
        assert_eq!(notes.len(), 2);
        assert!(notes[1].contains("2 条路径已失效"), "{notes:?}");

        // 全部失效：回执必须是一条明确错误，而不是空列表伪装成成功。
        let mut notes = Vec::new();
        append_dropped_notice(&mut notes, 3);
        assert_eq!(notes.len(), 1);
        assert!(notes[0].contains("3 条路径都已失效"), "{notes:?}");

        // 没有丢弃时不添噪。
        let mut notes = vec!["已入队 a".to_string()];
        append_dropped_notice(&mut notes, 0);
        assert_eq!(notes.len(), 1);
    }

    #[test]
    fn toggle_dedups_and_removes_for_the_echo_list() {
        let mut picker = UploadPicker::new(PathBuf::from("/tmp"));
        picker.toggle("/tmp/a");
        picker.toggle("/tmp/b");
        picker.toggle("/tmp/a"); // 再点一次是取消，不是第二条
        assert_eq!(picker.selected, vec!["/tmp/b"]);
        picker.toggle("/tmp/b");
        assert!(picker.selected.is_empty());
    }
}

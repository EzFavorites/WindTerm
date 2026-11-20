//! 脚本编辑器单测（`script_editor.rs` 靠 `#[path]` 挂回）。
//!
//! 分两类：
//!
//! 1. **纯层**：`highlight_job` 的排版任务、`file_label` 的路径拆分、标题取词。
//!    它们不碰 egui 的绘制循环，红了就是逻辑被撞了。
//! 2. **一帧真绘制**：开窗 → 着色 → 行号 → 画一帧。纯层测不到 `TextEdit` /
//!    `layouter` / `TextEditState` 这些 API 的用法，而它们错了的后果是运行时
//!    panic——这一帧把整条链走一遍，API 用错会当场炸。

use super::*;
use std::path::PathBuf;

/// 一次性临时目录。`Drop` 里整个删掉（与 `preview_tests.rs` 同一套夹具）。
struct TempDir {
    path: PathBuf,
}

impl TempDir {
    fn new(tag: &str) -> TempDir {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let path = std::env::temp_dir().join(format!("windterm-scripted-{tag}-{nanos}"));
        std::fs::create_dir_all(&path).expect("临时目录要建得出来");
        TempDir { path }
    }

    fn file(&self, name: &str, text: &str) -> PathBuf {
        let path = self.path.join(name);
        std::fs::write(&path, text).expect("临时文件要写得进去");
        path
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.path);
    }
}

fn dummy_colors() -> TokenColors {
    TokenColors {
        text: egui::Color32::WHITE,
        keyword: egui::Color32::LIGHT_BLUE,
        primitive: egui::Color32::YELLOW,
        string: egui::Color32::GREEN,
        number: egui::Color32::LIGHT_RED,
        comment: egui::Color32::GRAY,
    }
}

#[test]
fn file_label_takes_the_last_segment_and_survives_empty_input() {
    assert_eq!(file_label("/a/b/c.rhai"), "c.rhai");
    assert_eq!(file_label("c.rhai"), "c.rhai");
    assert_eq!(file_label(""), "");
}

#[test]
fn title_uses_the_file_name_when_a_path_is_set() {
    let mut state = ScriptEditorState::closed();
    assert_eq!(state.title(Lang::En), "Script Editor");
    state.path = "/tmp/demo.rhai".to_string();
    assert_eq!(state.title(Lang::En), "demo.rhai");
}

#[test]
fn highlight_job_covers_the_whole_source_with_several_sections() {
    // 排版任务必须逐字覆盖原文（漏一个字节，屏幕上就少一段），且关键字 / 字符串 /
    // 注释各自成段（否则等于没上色）。不折行是 gutter 的前提，单独断言。
    let source = "let x = 1; // note\nsend(\"hi\");\n";
    let job = highlight_job(source, &crate::ui::type_scale::heading(), &dummy_colors());
    assert_eq!(job.text, source, "排版任务必须逐字覆盖原文");
    assert!(job.sections.len() >= 4, "应有多个上色段：{}", job.sections.len());
    assert!(
        job.wrap.max_width.is_infinite(),
        "必须不折行（gutter 依赖 rows == 行数）"
    );
}

#[test]
fn opening_a_missing_file_reports_an_error_but_still_opens_an_empty_buffer() {
    let ctx = egui::Context::default();
    let pool = std::sync::Arc::new(term_session::SessionPool::new());
    let mut window = crate::ui::lifecycle_test_support::window_with(pool, &ctx, Vec::new());
    window.open_script_editor(None, Some("/definitely/not/here.rhai"));
    let state = window.script_editor.as_ref().expect("编辑器应已打开");
    assert!(state.open);
    assert_eq!(state.doc.text(), "", "读不到文件也不能有内容");
    assert!(state.error.is_some(), "失败原因要说出来");
}

#[test]
fn saving_writes_the_buffer_and_clears_the_dirty_flag() {
    let dir = TempDir::new("save");
    let target = dir.path.join("saved.rhai");
    let ctx = egui::Context::default();
    let pool = std::sync::Arc::new(term_session::SessionPool::new());
    let mut window = crate::ui::lifecycle_test_support::window_with(pool, &ctx, Vec::new());
    window.open_script_editor(None, None);
    {
        let state = window.script_editor.as_mut().expect("编辑器应已打开");
        state.doc.set_text("send(\"hi\");\n");
        state.path = target.to_string_lossy().into_owned();
        assert!(state.doc.is_dirty(), "改过应脏");
    }
    window.save_editor(false);
    let state = window.script_editor.as_ref().expect("编辑器应还在");
    assert!(!state.doc.is_dirty(), "保存后应清脏");
    assert!(state.error.is_none(), "保存不该报错：{:?}", state.error);
    assert_eq!(
        std::fs::read_to_string(&target).expect("文件应存在"),
        "send(\"hi\");\n"
    );
}

#[test]
fn saving_a_never_saved_buffer_prefills_the_script_library_path() {
    // 新建后直接按保存，落点不该是当前工作目录——`cargo run` 给的 CWD 是
    // 仓库根，双击启动可能是 `$HOME`，同一句「保存」会写进三个地方。
    let ctx = egui::Context::default();
    let pool = std::sync::Arc::new(term_session::SessionPool::new());
    let mut window = crate::ui::lifecycle_test_support::window_with(pool, &ctx, Vec::new());
    window.open_script_editor(None, None);
    window.save_editor(false);
    let state = window.script_editor.as_ref().expect("编辑器应还在");
    let target = state.save_as.as_ref().expect("应打开另存为并预填落点");
    let parent_is_library = std::path::Path::new(target)
        .parent()
        .is_some_and(|parent| parent.ends_with("scripts"));
    assert!(
        target.ends_with(".rhai") && parent_is_library,
        "落点应在脚本库目录（…/scripts/*.rhai）：{target}"
    );
    assert!(state.error.is_none(), "预填不该报错：{:?}", state.error);
}

/// 关窗必须**摘掉 `Option`**，不能只把 `open` 置 false。
///
/// 事故面（P0）：`modal_dialog_open()` 判 `is_some()`，于是「关着但还在」的编辑器
/// 一直算模态——整屏灰罩不撤，而灰罩用 `Sense::click()` 吃掉全部鼠标交互，
/// 用户看到的是「界面变暗、点什么都没反应」。这条断言同时钉两层：
/// 关闭后没有状态、且不再被算作模态。
#[test]
fn closing_the_editor_drops_its_state_and_releases_the_modal_backdrop() {
    let ctx = egui::Context::default();
    let pool = std::sync::Arc::new(term_session::SessionPool::new());
    let mut window = crate::ui::lifecycle_test_support::window_with(pool, &ctx, Vec::new());

    window.open_script_editor(None, None);
    assert!(window.script_editor.is_some(), "打开后应持有状态");
    assert!(window.modal_dialog_open(), "编辑器打开时是模态");

    window.close_script_editor();

    assert!(window.script_editor.is_none(), "关闭后不该留着「关着的状态」");
    assert!(
        !window.modal_dialog_open(),
        "关闭后不能再算模态，否则灰罩会永久吃掉鼠标交互"
    );
}

#[test]
fn one_frame_draws_without_panicking_and_keeps_the_window_open() {
    // 真绘制一帧：layouter / 行号 gutter / TextEditState 任一 API 用错都会在这里炸。
    let dir = TempDir::new("frame");
    let path = dir.file("demo.rhai", "let n = 1;\nsend(\"hi\");\n");
    let ctx = egui::Context::default();
    let pool = std::sync::Arc::new(term_session::SessionPool::new());
    let mut window = crate::ui::lifecycle_test_support::window_with(pool, &ctx, Vec::new());
    window.open_script_editor(None, Some(&path.to_string_lossy()));
    let _ = ctx.run(Default::default(), |ctx| window.draw_script_editor(ctx));
    let state = window.script_editor.as_ref().expect("画完仍应在");
    assert!(state.open, "一帧绘制不该把窗口关掉");
    assert!(!state.doc.text().is_empty(), "文件内容应已读入");
}

//! Preferences 的隐藏末页：`Script & API`。
//!
//! # 为什么整页只读
//!
//! 脚本注册、触发器、桥接与 Python 宿主都已经各有权威（prefs / 库目录 /
//! `bridge` 配置 / `python_run` 的解释器判据）。这一页只把它们的**现状**摆出来，
//! 要改就去点按钮开既有的管理器——这里不再造第二套编辑器，也不新增 Prefs 字段。
//!
//! # token 的边界
//!
//! 只从 `std::fs::metadata` 取「文件在不在 / 多大」。**不调用**
//! `bridge::token::load_or_create`（会建文件并返回明文），不读文件内容，
//! 界面上永远不出现 token 值本身，只出现它的文件元数据。

use super::settings_pages_view::{
    field_row, group_box, label_col, read_only, set_row_rhythm, Deco, Slot,
};
use crate::ui::*;
use term_app::shell::ShellAction;

/// Python 解释器由谁给出名字。只记来源，不缓存可执行文件路径。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum PythonSource {
    /// `WINDTERM_PYTHON` 非空。
    Env,
    /// PATH 上第一个命中的名字（`python3` 或 `python`）。
    Path(&'static str),
    /// 两个候选都不在 PATH 上。
    Missing,
}

/// 与 `python_run::interpreter` 同一份候选顺序：unix 先 `python3`，Windows 先 `python`。
fn python_candidates() -> &'static [&'static str] {
    if cfg!(windows) {
        &["python", "python3"]
    } else {
        &["python3", "python"]
    }
}

/// 与 `python_run::which` 同形：只做存在性判断，不缓存位置。
fn on_path(name: &str) -> bool {
    let Some(paths) = std::env::var_os("PATH") else {
        return false;
    };
    std::env::split_paths(&paths).any(|dir| dir.join(name).is_file())
}

/// 纯判据：env 覆盖优先，其次 PATH 首个命中，最后才是缺失。
fn python_source(env: Option<&str>, lookup: impl Fn(&str) -> bool) -> PythonSource {
    if env.is_some_and(|value| !value.trim().is_empty()) {
        return PythonSource::Env;
    }
    for name in python_candidates() {
        if lookup(name) {
            return PythonSource::Path(name);
        }
    }
    PythonSource::Missing
}

/// 一帧一份的只读快照。全部取自既有权威，不在窗口 / prefs 里另存一份真相。
struct Status {
    registered: usize,
    recent: usize,
    library: usize,
    triggers: usize,
    port: u16,
    /// token **文件**的字节数；只来自 `fs::metadata`，绝不读内容。
    token_bytes: Option<u64>,
    python: PythonSource,
}

fn collect() -> Status {
    let prefs = term_profile::prefs::load().unwrap_or_default();
    let token_bytes = crate::bridge::token::default_path()
        .and_then(|path| std::fs::metadata(path).ok())
        .map(|meta| meta.len());
    let env = std::env::var("WINDTERM_PYTHON").ok();
    Status {
        registered: prefs.scripts.len(),
        recent: prefs.recent_scripts.len(),
        library: crate::ui::terminal::script_library::library().len(),
        triggers: prefs.triggers.len(),
        port: crate::bridge::DEFAULT_PORT,
        token_bytes,
        python: python_source(env.as_deref(), on_path),
    }
}

/// 与数据表页共用的只读字段行：外观全仓只有一份。
fn status_row(ui: &mut egui::Ui, label: &str, value: &str, col: f32, note: &str) {
    field_row(ui, label, Slot::Inline, col, note, |ui| {
        read_only(
            ui,
            value,
            crate::theme::chrome(term_core::theme::CHROME_SCALE.hint),
            Deco::None,
        );
    });
}

const NOTE: &str = "Read-only. Buttons open the existing managers.";

pub(in crate::ui) fn script_api(win: &mut TerminalWindow, ui: &mut egui::Ui) {
    set_row_rhythm(ui);
    let s = collect();
    let labels = [
        win.tr("Registered scripts"),
        win.tr("Recent runs"),
        win.tr("Library files"),
        win.tr("Trigger rules"),
        win.tr("Bridge port"),
        win.tr("Token file"),
        win.tr("Python interpreter"),
    ];
    let col = label_col(ui, &labels);
    // 说明挂在字段悬停上，不另占一行——末页的四组内容已接近内容区底边。
    let note = win.tr(NOTE);

    let token = match s.token_bytes {
        Some(bytes) => format!("{} ({} B)", win.tr("Token file present"), bytes),
        None => win.tr("Token file missing").to_string(),
    };
    let python = match s.python {
        PythonSource::Env => win.tr("Environment override").to_string(),
        PythonSource::Path(name) => format!("{} ({name})", win.tr("PATH fallback")),
        PythonSource::Missing => win.tr("Not found on PATH").to_string(),
    };

    group_box(ui, Some(win.tr("Scripts")), |ui| {
        set_row_rhythm(ui);
        status_row(ui, labels[0], &s.registered.to_string(), col, note);
        status_row(ui, labels[1], &s.recent.to_string(), col, note);
        status_row(ui, labels[2], &s.library.to_string(), col, note);
        ui.add_space(4.0);
        ui.horizontal(|ui| {
            if ui.button(win.tr("Manage Scripts")).clicked() {
                // 管理脚本是模态窗口：先把设置收掉，再走既有命令。
                win.view.show_settings = false;
                win.dispatch_action(Action::Shell(ShellAction::ManageScripts));
            }
            if ui.button(win.tr("Script Panel")).clicked() {
                // 面板开关不是模态，设置页留在原地即可。
                win.dispatch_action(Action::Shell(ShellAction::TogglePaneScript));
            }
        });
    });
    ui.add_space(2.0);

    group_box(ui, Some(win.tr("Triggers")), |ui| {
        set_row_rhythm(ui);
        status_row(ui, labels[3], &s.triggers.to_string(), col, note);
        ui.add_space(4.0);
        if ui.button(win.tr("Manage Triggers")).clicked() {
            win.view.show_settings = false;
            win.dispatch_action(Action::Shell(ShellAction::ManageTriggers));
        }
    });
    ui.add_space(2.0);

    group_box(ui, Some(win.tr("Bridge API")), |ui| {
        set_row_rhythm(ui);
        status_row(ui, labels[4], &s.port.to_string(), col, note);
        status_row(ui, labels[5], &token, col, note);
        ui.add_space(4.0);
        if ui.button(win.tr("Quick Commands")).clicked() {
            win.view.show_settings = false;
            win.dispatch_action(Action::OpenQuickCommands);
        }
    });
    ui.add_space(2.0);

    group_box(ui, Some(win.tr("Python")), |ui| {
        set_row_rhythm(ui);
        status_row(ui, labels[6], &python, col, note);
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn env_override_wins_over_path_lookup() {
        assert_eq!(
            python_source(Some(" /usr/bin/python3 "), |_| true),
            PythonSource::Env
        );
    }

    #[test]
    fn blank_env_falls_back_to_the_first_path_hit() {
        assert_eq!(
            python_source(Some("   "), |name| name == "python"),
            PythonSource::Path("python")
        );
        assert_eq!(python_source(None, |_| false), PythonSource::Missing);
    }

    #[test]
    fn path_candidates_follow_the_python_host_order() {
        let first = if cfg!(windows) { "python" } else { "python3" };
        assert_eq!(python_candidates()[0], first);
        assert_eq!(python_candidates().len(), 2);
    }
}

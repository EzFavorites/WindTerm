//! 脚本面板的「路径输入 + Browse」行（从 `script_panel.rs` 拆出，守 500 行门禁）。
//!
//! 面板原本只能从脚本库下拉里挑文件，库外的脚本（别人发来的、临时写的）根本
//! 填不进去。这里是本项目自己加的便利入口——兼容终端没有这条自由输入行，
//! 也没有「相对路径按脚本库目录展开」这个约定。
//!
//! 只画控件 / 产出高度，跨帧状态仍在 `ScriptPanelState.path` 上；选中哪个页签
//! 在打开 Browse 的那一刻就记进 `PathTarget::ScriptPanelPath(i)`，用户之后切页
//! 也不会把结果写错地方（见 `ui/path_picker.rs`）。

use crate::ui::path_picker::PathTarget;
use crate::ui::*;
use eframe::egui;
use term_runtime::paths::scripts_dir;

/// 输入行相对右侧按钮留出的宽度。
const BROWSE_W: f32 = 72.0;

/// 把用户打的路径规范化：`trim` 之后，相对路径按脚本库目录展开。
///
/// `args_from_prefs` 按**完整路径精确相等**查已登记脚本的 `args`；若这里存的是
/// `deploy.rhai` 而注册表里是 `<scripts_dir>/deploy.rhai`，参数就悄悄丢了——
/// 而且丢得没有报错。绝对路径原样保留；拿不到 `scripts_dir()` 时不猜目录。
/// 纯函数（`scripts` 由调用方传入），便于单测钉死边界。
pub(in crate::ui) fn normalize_with(path: &str, scripts: Option<&std::path::Path>) -> String {
    let path = path.trim();
    if path.is_empty() {
        return String::new();
    }
    let candidate = std::path::Path::new(path);
    if candidate.is_absolute() {
        return path.to_string();
    }
    match scripts {
        Some(dir) => dir.join(candidate).to_string_lossy().into_owned(),
        None => path.to_string(),
    }
}

/// 面板上未展开那一格显示什么。
///
/// 库里的行用清单自带的展示名；库外的路径不再退化成 `— pick a script —`
/// （那是**没选**的意思，不是「选了一个库外文件」），而是显示 basename，
/// 完整路径仍留在输入框里。复用 `script_library::display_name()`，避免两处
/// 各写一份取文件名逻辑。
pub(in crate::ui) fn label(path: &str, library: &[(String, String)], placeholder: &str) -> String {
    if let Some((_, name)) = library.iter().find(|(full, _)| full == path) {
        return name.clone();
    }
    if path.trim().is_empty() {
        return placeholder.to_string();
    }
    crate::ui::terminal::script_library::display_name(path)
}

/// 画路径行，返回它占掉的高度。调用方把结果区的顶边往下推这么多，避免重叠。
pub(in crate::ui) fn draw(win: &mut TerminalWindow, ui: &mut egui::Ui, body: egui::Rect) -> f32 {
    let mut c = ui.new_child(egui::UiBuilder::new().max_rect(body));
    let tab = win.script_panel.active.min(win.script_panel.tabs.len().saturating_sub(1));
    let t_path = win.tr("Path:");
    let t_browse = win.tr("Browse…");
    let t_hint = win.tr("/path/to/script.rhai (relative = script library)");
    let scripts = scripts_dir();
    // 跨帧缓冲先取走，闭包里只动局部变量；最后原样写回（Browse 与提交都改它）。
    let mut path = std::mem::take(&mut win.script_panel.tabs[tab].state.path);
    let mut browse = false;
    let mut commit = false;
    c.horizontal(|ui| {
        ui.label(t_path);
        let resp = ui.add(
            egui::TextEdit::singleline(&mut path)
                .id(egui::Id::new(("script_panel_path", tab)))
                .hint_text(t_hint)
                .desired_width((ui.available_width() - BROWSE_W).max(40.0)),
        );
        // 失焦或按回车才算一次「输完了」：输入中途就把 `foo.rhai` 改成绝对路径
        // 会让光标跳到末尾，用户根本没法接着打。
        commit = resp.lost_focus()
            || (resp.has_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)));
        if ui.button(t_browse).clicked() {
            browse = true;
        }
    });
    if commit {
        path = normalize_with(&path, scripts.as_deref());
    }
    win.script_panel.tabs[tab].state.path = path.clone();
    if browse {
        // 捕获的是**当前**页签下标：用户可能在系统对话框开着时切页。
        win.open_path_picker(PathTarget::ScriptPanelPath(tab), &path);
    }
    c.min_rect().height() + 2.0
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn a_relative_path_is_resolved_against_the_script_library() {
        let dir = PathBuf::from("/profiles/me/scripts");
        assert_eq!(
            normalize_with("  deploy.rhai  ", Some(&dir)),
            "/profiles/me/scripts/deploy.rhai"
        );
        // 绝对路径不动；没有库目录时不猜一个出来。
        assert_eq!(normalize_with("/tmp/x.rhai", Some(&dir)), "/tmp/x.rhai");
        assert_eq!(normalize_with("deploy.rhai", None), "deploy.rhai");
        assert_eq!(normalize_with("   ", Some(&dir)), "");
    }

    #[test]
    fn a_non_library_path_shows_its_basename_not_the_empty_placeholder() {
        let library = vec![("/lib/a.rhai".to_string(), "a.rhai".to_string())];
        let placeholder = "— pick a script —";
        assert_eq!(label("/lib/a.rhai", &library, placeholder), "a.rhai");
        // 库外文件：以前这里只写「— pick a script —」，把一个已选中的文件画成没选。
        assert_eq!(label("/tmp/deploy.rhai", &library, placeholder), "deploy.rhai");
        assert_eq!(label("deploy.rhai", &[], placeholder), "deploy.rhai");
        assert_eq!(label("", &[], placeholder), placeholder);
    }
}

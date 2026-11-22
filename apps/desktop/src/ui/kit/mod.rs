//! UI 工具箱：**函数式绘制原语**，不定义 widget trait。
//!
//! ADR-0021 明确不做 retained 组件抽象（理由见该 ADR「明确不做 1」：那等于
//! 在本仓库里再造一个小框架）。所以这里全部是 `fn draw_xxx(ui, &Props)`：
//! 输出只由入参决定，夹具能定格，视觉基线逐字节可比。
//!
//! 分层：这一层只认识 `egui::Ui` / `egui::Painter` / 主题色号，不认识任何
//! 业务状态（没有 `ViewState`、没有 `TerminalWindow`）。所以它既能被生产
//! 路径调用，也能被组件夹具直接调用。

pub(in crate::ui) mod checkbox;
pub(in crate::ui) mod dialog;
pub(in crate::ui) mod field;
pub(in crate::ui) mod footer;
pub(in crate::ui) mod form;
pub(in crate::ui) mod menu_row;
pub(in crate::ui) mod spec;
pub(in crate::ui) mod switch;
pub(in crate::ui) mod tabs;
pub(crate) mod type_scale;
pub(in crate::ui) mod window;

/// 架构门禁：扫描 `dialogs/` 源码，防止收敛过的重复实现被重新抄回去。
///
/// 这是**结构性**测试，不依赖视觉基线：新写一个弹窗若又自带 `fn col` /
/// 又手画页签 / 又内联 Close 按钮，这里立刻红，比 review 时靠肉眼可靠。
#[cfg(test)]
mod architecture_tests {
    use std::path::PathBuf;

    fn dialogs_sources() -> Vec<(String, String)> {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("src/ui/dialogs");
        let mut out = Vec::new();
        let mut stack = vec![root];
        while let Some(dir) = stack.pop() {
            for entry in std::fs::read_dir(&dir).expect("dialogs dir must exist") {
                let entry = entry.unwrap();
                let path = entry.path();
                if path.is_dir() {
                    stack.push(path);
                } else if path.extension().is_some_and(|e| e == "rs") {
                    out.push((
                        path.file_name().unwrap().to_string_lossy().into_owned(),
                        std::fs::read_to_string(&path).unwrap(),
                    ));
                }
            }
        }
        out
    }

    #[test]
    fn color_conversion_has_one_entry_point() {
        for (name, src) in dialogs_sources() {
            assert!(
                !src.contains("fn col(hex: u32) -> egui::Color32"),
                "{name} 又定义了本地 col()；请从 kit::spec::col 取色"
            );
            if name != "dialog_chrome.rs" {
                assert!(
                    !src.contains("Color32::from_rgb((hex >> 16)"),
                    "{name} 绕过了主题换算入口"
                );
            }
        }
    }

    #[test]
    fn underlined_label_job_is_not_copied_into_dialogs() {
        for (name, src) in dialogs_sources() {
            assert!(
                !src.contains("fn underlined_job("),
                "{name} 又抄了一份 underlined_job；请用 kit::field::underlined_job"
            );
        }
    }

    #[test]
    fn native_window_and_raw_footer_buttons_are_allowlisted_only() {
        for (name, src) in dialogs_sources() {
            assert!(
                !src.contains("egui::Window::new"),
                "{name} 用了原生 Window；请走 DialogWindow / harness"
            );
            if name != "quick_commands.rs" {
                assert!(
                    !src.contains("ui.button(\"Ok\")") && !src.contains("ui.button(\"Cancel\")"),
                    "{name} 仍有裸页脚按钮；请走 kit::footer + spec::FooterButton"
                );
            }
        }
    }

    #[test]
    fn absolute_coordinate_dialogs_keep_their_widgets_encapsulated() {
        // OneKeys / Triggers 是 768×562 绝对坐标体系，几何/色号与 kit 默认值不同，
        // 是**刻意保留的规格变体**。它们可以自带 helper，但不许把裸 checkbox/
        // radio 散到页面里——那会让「开关长什么样」重新分裂成多份。
        for (name, src) in dialogs_sources() {
            if name.starts_with("settings_live_onekeys")
                || name.starts_with("triggers")
            {
                for forbidden in [
                    "egui::Checkbox::new",
                    "egui::RadioButton::new",
                    "ui.checkbox(",
                    "ui.radio(",
                ] {
                    assert!(
                        !src.contains(forbidden),
                        "{name} 把 {forbidden} 散到了页面里；请用本地 fields/dialog_chrome helper"
                    );
                }
            }
        }
    }

    #[test]
    fn kit_modules_are_all_registered() {
        let mod_rs = std::fs::read_to_string(
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("src/ui/kit/mod.rs"),
        )
        .unwrap();
        for m in ["dialog", "field", "footer", "form", "spec", "tabs"] {
            assert!(mod_rs.contains(&format!("mod {m};")), "kit/{m} 未注册");
        }
    }
}

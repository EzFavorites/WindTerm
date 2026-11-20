//! 主题文件的「找路径 + 读文件」副作用；解析本身是 `term_core::theme` 的纯函数。
//!
//! ADR-0014 把这部分从语义平面迁出，使 `term-core` 里不再出现 `std::fs` / `dirs`。

use std::path::PathBuf;
use term_core::theme::Palette;

/// 构造默认调色板：优先读可用的主题文件，找不到则用内置回退。
pub fn load_palette() -> Palette {
    let theme_name = "dige-black";
    for root in candidate_roots() {
        let path = root
            .join("global")
            .join("themes")
            .join(theme_name)
            .join("scheme.theme");
        if let Ok(raw) = std::fs::read_to_string(&path) {
            if let Some(palette) = Palette::from_scheme(&raw, path.display().to_string(), theme_name) {
                return palette;
            }
        }
    }
    Palette::builtin(theme_name)
}

/// Roots that hold a `global/themes/...` tree next to the program's own files.
fn candidate_roots() -> Vec<PathBuf> {
    let mut out: Vec<PathBuf> = Vec::new();
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            out.push(dir.to_path_buf());
            if let Some(p) = dir.parent() {
                out.push(p.to_path_buf()); // Contents/MacOS
            }
        }
    }
    if let Some(dir) = dirs::config_dir() {
        out.push(dir.join("WindTerm"));
    }
    if let Some(dir) = dirs::home_dir() {
        out.push(dir.join(".windterm"));
        out.push(dir.join("Library/Application Support/WindTerm"));
    }
    out
}

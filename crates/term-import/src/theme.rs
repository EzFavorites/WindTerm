//! 主题文件的「找路径 + 读文件」副作用；解析本身是 `term_core::theme` 的纯函数。
//!
//! ADR-0014 把这部分从语义平面迁出，使 `term-core` 里不再出现 `std::fs` / `dirs`。

use std::path::PathBuf;
use term_core::theme::Palette;

/// 默认主题的调色板（保持既有调用点不变）。
pub fn load_palette() -> Palette {
    load_palette_named(term_core::theme::DEFAULT_THEME_NAME)
}

/// 按名字取调色板：先看磁盘主题文件，再看本仓内置的另外两套整窗主题，
/// 最后才回落通用 16 色方案。名字写错时也回落而不 panic。
pub fn load_palette_named(theme_name: &str) -> Palette {
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
    builtin_scheme(theme_name).unwrap_or_else(|| Palette::builtin(theme_name))
}

/// 内置整窗主题的 16 色（顺序同 `ANSI_NAMES`）。没有磁盘主题文件时，
/// 这三套保证设置页的下拉不是「只有一项」的假控件。
fn builtin_scheme(name: &str) -> Option<Palette> {
    const BLACK: [&str; 16] = [
        "#333333", "#C4265E", "#86B42B", "#D0A500", "#3465A4", "#8C6BC8", "#56ADBC", "#E3E3DD",
        "#666666", "#F92672", "#A6E22E", "#9E862F", "#819AFF", "#AE81FF", "#66D9EF", "#F8F8F2",
    ];
    const LIGHT: [&str; 16] = [
        "#404040", "#A31515", "#008000", "#8A6D00", "#0451A5", "#8B008B", "#008B8B", "#F5F5F5",
        "#707070", "#CD3131", "#00A000", "#B8860B", "#0451D0", "#9400D3", "#008B9B", "#FFFFFF",
    ];
    const GREEN: [&str; 16] = [
        "#001208", "#33FF66", "#66FF99", "#99FF66", "#00CC66", "#33FFCC", "#00FFCC", "#CCFFCC",
        "#006633", "#66FF99", "#99FF99", "#BBFF99", "#66FFCC", "#99FFCC", "#CCFFCC", "#EEFFEE",
    ];
    let (ansi, fg, bg) = match name {
        "dige-black" => (BLACK, "#F8F8F2", "#1C1C1C"),
        "dige-light" => (LIGHT, "#1E1E1E", "#FAFAFA"),
        "dige-green" => (GREEN, "#33FF66", "#001208"),
        _ => return None,
    };
    let mut json = String::from("{\"styles\":[");
    for (i, color) in ansi.iter().enumerate() {
        if i > 0 {
            json.push(',');
        }
        json.push_str(&format!(
            "{{\"name\":\"{}\",\"style\":{{\"foreground\":\"{}\"}}}}",
            term_core::theme::ANSI_NAMES[i],
            color
        ));
    }
    json.push_str(&format!(
        ",{{\"name\":\"text.default\",\"style\":{{\"foreground\":\"{fg}\",\"background\":\"{bg}\"}}}}]}}"
    ));
    // 默认 dige-black 沿用既有「内置回退」口径，doctor 自检认的就是这个串。
    let source = if name == term_core::theme::DEFAULT_THEME_NAME {
        term_core::theme::ANSI_SOURCE_BUILTIN.to_string()
    } else {
        format!("built-in theme '{name}'")
    };
    Palette::from_scheme(&json, source, name)
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn all_builtin_themes_load_with_distinct_backgrounds() {
        let mut seen = Vec::new();
        for name in term_core::theme::BUILTIN_THEME_NAMES {
            let p = builtin_scheme(name).unwrap_or_else(|| panic!("{name} 必须有内置调色板"));
            assert_eq!(p.theme_name, *name);
            assert!(!seen.contains(&p.default_bg), "{name} 的背景色要和别套不同");
            seen.push(p.default_bg);
        }
        assert_eq!(seen.len(), 3);
        assert!(builtin_scheme("no-such-theme").is_none());
    }
}

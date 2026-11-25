//! 原版 WindTerm 配置目录的**发现层**（B1 第三块）。
//!
//! 只做两件事：枚举**有限的候选根目录**、定位 `user.config`。它不读会话内容、
//! 不写任何文件、不扫描全盘——「要不要用其中某个目录」是产品决定，由调用方做。
//! 侧栏导入与首次引导都调用 [`candidate_windterm_roots`]，消除两处候选路径的双写。
//!
//! 候选来源：
//! - 调用方手动指定的目录（便携模式 / 用户从文件夹选择器挑的）；
//! - macOS app 包内 `WindTerm.app/Contents/MacOS/.wind/profiles`（出厂自带样本所在）；
//! - `profiles.config` 目录指针：`{"path":"."}` 是相对指针，按指针文件所在目录解析；
//! - 三处既有用户根：`~/.windterm/profiles`、`~/Library/Application Support/WindTerm/profiles`、
//!   `~/.config/WindTerm/profiles`；
//! - Windows `%APPDATA%` / `%LOCALAPPDATA%`、Linux `$XDG_CONFIG_HOME` 的常见位置。

use std::path::{Path, PathBuf};

pub const PROFILES_POINTER: &str = "profiles.config";
pub const USER_CONFIG_FILE: &str = "user.config";
pub const USER_SESSIONS_FILE: &str = "user.sessions";

const APP_BUNDLE_SUFFIX: &str = "Contents/MacOS/.wind/profiles";

/// 默认候选根（不含手动目录）。只枚举，不扫描。
pub fn candidate_windterm_roots() -> Vec<PathBuf> {
    candidate_windterm_roots_with(None)
}

/// 候选根 + 一个手动目录（放最前，优先命中）。
pub fn candidate_windterm_roots_with(manual: Option<&Path>) -> Vec<PathBuf> {
    let mut out: Vec<PathBuf> = Vec::new();
    let mut push = |p: PathBuf| {
        if p.is_absolute() && !out.contains(&p) {
            out.push(p);
        }
    };

    if let Some(p) = manual {
        push(p.to_path_buf());
    }

    // macOS app 包：/Applications 与用户自己的 ~/Applications 各一处。
    push(PathBuf::from("/Applications/WindTerm.app").join(APP_BUNDLE_SUFFIX));
    if let Some(home) = dirs::home_dir() {
        push(home.join("Applications/WindTerm.app").join(APP_BUNDLE_SUFFIX));
    }

    // profiles.config 指针：按指针文件所在目录解析，同时把 `profiles` 子目录
    // 也算一个根——"path": "." 既可指 .wind 本身，也可指它的 profiles 子目录，
    // 两种布局都能命中（见 find_user_config 的一层子目录枚举）。
    for pointer in pointer_candidates() {
        if let Some(root) = resolve_pointer(&pointer) {
            push(root.clone());
            push(root.join("profiles"));
        }
    }

    // 三处既有用户根（侧栏 / 引导此前各写一份的那三行，现在只在这里）。
    if let Some(home) = dirs::home_dir() {
        push(home.join(".windterm").join("profiles"));
        push(home.join("Library/Application Support/WindTerm/profiles"));
        push(home.join(".config/WindTerm/profiles"));
        push(home.join(".wind/profiles"));
        push(home.join(".wind"));
    }
    // Linux / 其它 Unix：$XDG_CONFIG_HOME 优先于 ~/.config。
    if let Some(xdg) = std::env::var_os("XDG_CONFIG_HOME") {
        let xdg = PathBuf::from(xdg);
        if xdg.is_absolute() {
            push(xdg.join("WindTerm/profiles"));
            push(xdg.join("windterm/profiles"));
        }
    }
    // Windows：应用数据目录（不读注册表；导出文件读不到就是读不到）。
    for key in ["APPDATA", "LOCALAPPDATA"] {
        if let Some(base) = std::env::var_os(key) {
            let base = PathBuf::from(base);
            if base.is_absolute() {
                push(base.join("WindTerm/.wind/profiles"));
                push(base.join("WindTerm/profiles"));
            }
        }
    }
    out
}

/// 指针文件本身的有限候选位置（不遍历目录树）。
fn pointer_candidates() -> Vec<PathBuf> {
    let mut out = vec![
        PathBuf::from("/Applications/WindTerm.app/Contents/MacOS/.wind").join(PROFILES_POINTER),
        PathBuf::from("/Applications/WindTerm.app/Contents/MacOS/.wind/profiles").join(PROFILES_POINTER),
    ];
    if let Some(home) = dirs::home_dir() {
        out.push(home.join("Applications/WindTerm.app/Contents/MacOS/.wind").join(PROFILES_POINTER));
        out.push(home.join(".wind").join(PROFILES_POINTER));
        out.push(home.join(".windterm").join(PROFILES_POINTER));
        out.push(home.join(".config/WindTerm").join(PROFILES_POINTER));
    }
    out
}

/// 解析 `{"path":"."}` 这类目录指针。相对路径按指针文件的父目录解析；
/// 其它形态（数组 / 缺键 / 坏 JSON）返回 `None`，不猜。
pub fn resolve_pointer(path: &Path) -> Option<PathBuf> {
    let raw = std::fs::read_to_string(path).ok()?;
    let value: serde_json::Value = serde_json::from_str(&raw).ok()?;
    let rel = value.get("path")?.as_str()?;
    if rel.is_empty() {
        return None;
    }
    let p = PathBuf::from(rel);
    let resolved = if p.is_absolute() { p } else { path.parent()?.join(p) };
    Some(normalize_dots(&resolved))
}

/// 去掉路径里的 `.` / `..` 段（纯词法处理，不碰文件系统）。
fn normalize_dots(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for comp in path.components() {
        match comp.as_os_str().to_str() {
            Some(".") => {}
            Some("..") => {
                out.pop();
            }
            _ => out.push(comp.as_os_str()),
        }
    }
    out
}

/// 在默认候选根里找 `user.config`。
pub fn find_user_config() -> Option<PathBuf> {
    find_user_config_in(&candidate_windterm_roots())
}

/// 在给定候选根里找 `user.config`（供调用方在手动选择目录后复用）。
///
/// 每个根只看两处：根本身、以及根的**一层**子目录（原版是 `default.v10/user.config`）。
/// 不递归下钻，更不扫描全盘——这是发现层的边界。
pub fn find_user_config_in(roots: &[PathBuf]) -> Option<PathBuf> {
    for root in roots {
        let direct = root.join(USER_CONFIG_FILE);
        if direct.is_file() {
            return Some(direct);
        }
        let Ok(entries) = std::fs::read_dir(root) else { continue };
        let mut dirs: Vec<PathBuf> = entries
            .flatten()
            .filter(|e| e.path().is_dir())
            .map(|e| e.path())
            .collect();
        dirs.sort();
        for dir in dirs {
            let candidate = dir.join(USER_CONFIG_FILE);
            if candidate.is_file() {
                return Some(candidate);
            }
        }
    }
    None
}

/// 与 [`find_user_config_in`] 同口径地找 `terminal/user.sessions`（会话导入用）。
/// 只查根本身、一层子目录、以及一层子目录下的 `terminal/`——原版三层布局就到这里。
pub fn find_user_sessions_in(roots: &[PathBuf]) -> Option<PathBuf> {
    for root in roots {
        for candidate in [root.join(USER_SESSIONS_FILE), root.join("terminal").join(USER_SESSIONS_FILE)] {
            if candidate.is_file() {
                return Some(candidate);
            }
        }
        let Ok(entries) = std::fs::read_dir(root) else { continue };
        let mut dirs: Vec<PathBuf> = entries
            .flatten()
            .filter(|e| e.path().is_dir())
            .map(|e| e.path())
            .collect();
        dirs.sort();
        for dir in dirs {
            for candidate in [dir.join(USER_SESSIONS_FILE), dir.join("terminal").join(USER_SESSIONS_FILE)] {
                if candidate.is_file() {
                    return Some(candidate);
                }
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp_dir(tag: &str) -> PathBuf {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let dir = std::env::temp_dir().join(format!(
            "windterm-discovery-{}-{}-{tag}",
            std::process::id(),
            nanos
        ));
        std::fs::create_dir_all(&dir).expect("临时目录");
        dir
    }

    #[test]
    fn pointer_dot_resolves_to_its_own_directory() {
        let root = tmp_dir("pointer-dot");
        let pointer = root.join(PROFILES_POINTER);
        std::fs::write(&pointer, r#"{"path" : "."}"#).expect("写指针");
        assert_eq!(resolve_pointer(&pointer), Some(root.clone()));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn pointer_absolute_and_bad_pointers_are_handled() {
        let root = tmp_dir("pointer-bad");
        let abs = root.join("elsewhere");
        std::fs::create_dir_all(&abs).expect("mkdir");
        let pointer = root.join(PROFILES_POINTER);
        std::fs::write(&pointer, format!(r#"{{"path":"{}"}}"#, abs.display())).expect("写指针");
        assert_eq!(resolve_pointer(&pointer), Some(abs));
        std::fs::write(&pointer, r#"["not","an","object"]"#).expect("写坏指针");
        assert_eq!(resolve_pointer(&pointer), None);
        std::fs::write(&pointer, "{ broken").expect("写坏 JSON");
        assert_eq!(resolve_pointer(&pointer), None);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn find_user_config_checks_root_and_one_subdirectory_only() {
        let root = tmp_dir("find-config");
        let profile = root.join("default.v10");
        std::fs::create_dir_all(&profile).expect("mkdir");
        let wanted = profile.join(USER_CONFIG_FILE);
        std::fs::write(&wanted, "{}").expect("写配置");
        // 更深一层不该被翻出来（发现层不递归）。
        let deep = profile.join("nested").join("deeper");
        std::fs::create_dir_all(&deep).expect("mkdir");
        std::fs::write(deep.join(USER_CONFIG_FILE), "{}").expect("写深层配置");
        assert_eq!(find_user_config_in(&[root.clone()]), Some(wanted));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn find_user_sessions_checks_the_original_three_level_layout() {
        let root = tmp_dir("find-sessions");
        let dir = root.join("default.v10").join("terminal");
        std::fs::create_dir_all(&dir).expect("mkdir");
        let wanted = dir.join(USER_SESSIONS_FILE);
        std::fs::write(&wanted, "[]").expect("写会话");
        assert_eq!(find_user_sessions_in(&[root.clone()]), Some(wanted));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn manual_root_is_first_and_candidates_are_deduplicated() {
        let manual = tmp_dir("manual-root");
        let roots = candidate_windterm_roots_with(Some(&manual));
        assert_eq!(roots[0], manual);
        let mut seen = std::collections::BTreeSet::new();
        for r in &roots {
            assert!(seen.insert(r), "候选根不许重复：{}", r.display());
            assert!(r.is_absolute());
        }
        let _ = std::fs::remove_dir_all(&manual);
    }
}

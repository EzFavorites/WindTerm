//! 宿主环境事实：当前登录名。
//!
//! 为什么单独一个模块：`term-core` 是纯语义层（R0：同样的输入必须得到同样的
//! 会话配置与启动计划），它**不能**自己读 `$USER`——那会让同一份代码在两台
//! 机器上产出不同的默认会话（P1-5）。读环境的动作收在这一层，本 crate 本来
//! 就持有本机事实（`local_dir::home` 的 `HOME`、`pty` 的 `LANG`）；需要登录名
//! 的地方把它**注入**给 `term_core::session::SessionConfig::new_local_shell`。
//!
//! 它不落盘、不进 UI：只把「本机是谁」变成一个值。

/// 当前登录名，作为 `SessionConfig::new_local_shell` 的入参。
///
/// `$USER` 缺失 / 为空 / 全空白时回落 `"user"`——与迁移前 core 里那处兜底
/// 逐字一致，避免「升级后默认用户名突然变成空串」这种静默改变。
///
/// 返回值可能是任何人写在环境里的字符串：调用方只把它当**展示用**的默认值
/// （Shell 会话的用户名），不要拿它做鉴权或路径拼接。
pub fn login() -> String {
    // Windows 没有 `$USER`，账号名在 `%USERNAME%`。两者都缺时仍回落 `"user"`。
    let raw = std::env::var("USER")
        .ok()
        .filter(|name| !name.trim().is_empty())
        .or_else(|| std::env::var("USERNAME").ok());
    login_from(raw.as_deref())
}

/// `$USER` 的原始值 → 可注入的登录名。
///
/// 抽成纯函数是为了让兜底规则可测：`login()` 依赖本机环境，测试不能把断言
/// 钉在跑测那台机器的登录名上。
fn login_from(raw: Option<&str>) -> String {
    match raw.map(str::trim) {
        Some(name) if !name.is_empty() => name.to_string(),
        _ => "user".to_string(),
    }
}

/// 当前 shell 的**基名**（`$SHELL` 的最后一段），给界面显示用。
///
/// 底带 Shell 面板里每条标签都叫 `序号.壳名`（视觉基线里的 `1.zsh`），壳名取的
/// 就是这里。为什么放在本模块：`$SHELL` 与 `$USER` 一样是**本机事实**，
/// `term-core` 不读环境（P1-5），而界面层（`ui/`）也不该自己去 `std::env`——
/// 需要本机事实的地方都从这一层取，读到的东西只有一处定义。
///
/// 真正的启动程序由 PTY 层按 `$SHELL` 解析（`CommandBuilder::new_default_prog`），
/// 这里只负责**显示名**：两者读的是同一个变量，但一个用于 spawn、一个用于文案，
/// 不要拿它去拼路径。
pub fn shell_basename() -> String {
    // Windows 上壳名在 `%COMSPEC%`（`C:\Windows\system32\cmd.exe`），没有 `$SHELL`；
    // 两者都缺时仍回落 `"shell"`。真正的启动程序由 PTY 层的 `new_default_prog`
    // 自己按平台解析，这里只影响标签上的显示名。
    let raw = std::env::var("SHELL")
        .ok()
        .filter(|path| !path.trim().is_empty())
        .or_else(|| std::env::var("COMSPEC").ok());
    shell_basename_from(raw.as_deref())
}

/// `$SHELL` 的原始值 → 显示用的壳名。
///
/// 纯函数版，便于单测（同 [`login_from`]）。
///
/// 兜底 `"shell"`：`$SHELL` 缺失（Windows、或从 GUI 启动而没继承环境）、为空白、
/// 或以 `/` 结尾时，标签仍然要有个名字——空名字会让面板标签条上出现一块
/// 只有一个 `×` 的空白块。
fn shell_basename_from(raw: Option<&str>) -> String {
    let path = raw.map(str::trim).unwrap_or("");
    // 同时按 `/` 与 `\` 切：跨平台显示名，不假设路径分隔符。
    let base = path
        .rsplit(['/', '\\'])
        .next()
        .unwrap_or("")
        .trim();
    if base.is_empty() {
        "shell".to_string()
    } else {
        base.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_present_login_is_used_as_is() {
        assert_eq!(login_from(Some("alice")), "alice");
    }

    #[test]
    fn surrounding_blanks_are_trimmed() {
        // 环境变量里带空格是最常见的复制粘贴事故：注入值不该原样拖进去。
        assert_eq!(login_from(Some("  alice\t")), "alice");
    }

    #[test]
    fn a_missing_or_blank_login_falls_back_to_user() {
        // 与迁移前 `env::var("USER").unwrap_or("user")` 的行为一致。
        assert_eq!(login_from(None), "user");
        assert_eq!(login_from(Some("")), "user");
        assert_eq!(login_from(Some("   ")), "user");
    }

    #[test]
    fn login_is_never_empty() {
        // 注入源永远给得出一个非空值：调用方不需要再写一遍兜底。
        assert!(!login().is_empty());
    }

    #[test]
    fn the_shell_basename_is_the_last_path_segment() {
        assert_eq!(shell_basename_from(Some("/bin/zsh")), "zsh");
        assert_eq!(shell_basename_from(Some("/usr/local/bin/bash")), "bash");
        // 相对路径 / 裸名字也成立。
        assert_eq!(shell_basename_from(Some("fish")), "fish");
        // 反斜杠路径（跨平台显示名同一条规则）。
        assert_eq!(shell_basename_from(Some(r"C:\bin\pwsh.exe")), "pwsh.exe");
    }

    #[test]
    fn a_missing_or_blank_shell_falls_back_to_shell() {
        // 缺失 / 空白 / 只有分隔符 —— 三种都要给得出可显示的名字，
        // 否则面板标签条上会出现一块只有 `×` 的空白块。
        assert_eq!(shell_basename_from(None), "shell");
        assert_eq!(shell_basename_from(Some("")), "shell");
        assert_eq!(shell_basename_from(Some("   ")), "shell");
        assert_eq!(shell_basename_from(Some("/bin/")), "shell");
    }

    #[test]
    fn shell_basename_is_never_empty() {
        // 与 `login_is_never_empty` 同一条：调用方不必再写一遍兜底。
        assert!(!shell_basename().is_empty());
    }
}

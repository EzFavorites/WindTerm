//! `term-desk doctor`：环境体检。
//!
//! 只回答一个问题：**这台机器上，终端能不能跑起来**。所以只查「缺了会真的
//! 出问题」的几项，且必须能在无窗口环境下查——查不到的（比如字体、GPU）就
//! 不查，宁可少一条也不要写一句「看起来应该没问题」。
//!
//! 判定和采集分开：`judge()` 是纯函数（输入是已经取到的值），所以规则本身
//! 可以单测；`collect()` 才是碰 OS 的那一半。这不是洁癖——体检逻辑最容易
//! 写成一堆 if 然后没人敢改，拆开后每条规则的「什么情况算失败」都能被钉住。

use std::path::Path;

use super::align::pad;
use term_app::capability;
use term_core::theme::ANSI_SOURCE_BUILTIN;
use term_runtime::{persist, theme_load};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Level {
    Ok,
    Warn,
    /// 缺了这一项，本地会话根本起不来。
    Fail,
}

#[derive(Debug, PartialEq, Eq)]
pub struct Check {
    pub name: &'static str,
    pub level: Level,
    pub detail: String,
}

fn check(name: &'static str, level: Level, detail: impl Into<String>) -> Check {
    Check {
        name,
        level,
        detail: detail.into(),
    }
}

/// 采集到的环境事实。所有字段都是「已经取到的值」，没有任何 IO。
#[derive(Debug, Default)]
pub struct Input {
    pub os: String,
    pub arch: String,
    pub term: Option<String>,
    pub shell: Option<String>,
    pub shell_exists: bool,
    pub profile_dir: Option<String>,
    pub profile_writable: bool,
    pub theme_name: String,
    pub theme_source: String,
    pub sessions_saved: Option<usize>,
    pub wired: usize,
    pub planned: usize,
}

pub fn judge(i: &Input) -> Vec<Check> {
    let mut out = vec![check("平台", Level::Ok, format!("{}/{}", i.os, i.arch))];

    match i.shell.as_deref() {
        None | Some("") => out.push(check(
            "$SHELL",
            Level::Fail,
            "未设置 —— 本地会话没有可启动的 shell",
        )),
        Some(shell) if !i.shell_exists => out.push(check(
            "$SHELL",
            Level::Fail,
            format!("{shell} 不存在或不可执行"),
        )),
        Some(shell) => out.push(check("$SHELL", Level::Ok, shell.to_string())),
    }

    match (&i.profile_dir, i.profile_writable) {
        (None, _) => out.push(check(
            "profile 目录",
            Level::Warn,
            "无法确定位置（没有 config 目录也没有 $HOME）——会话不会落盘",
        )),
        (Some(dir), true) => out.push(check("profile 目录", Level::Ok, format!("{dir}（可写）"))),
        (Some(dir), false) => out.push(check(
            "profile 目录",
            Level::Warn,
            format!("{dir} 不可写 —— 会话保存会静默失败"),
        )),
    }

    if i.theme_source == ANSI_SOURCE_BUILTIN {
        out.push(check(
            "主题",
            Level::Warn,
            format!(
                "{} 用内置回退（没找到 scheme.theme，颜色仍可用）",
                i.theme_name
            ),
        ));
    } else {
        out.push(check(
            "主题",
            Level::Ok,
            format!("{} 来自 {}", i.theme_name, i.theme_source),
        ));
    }

    out.push(match i.sessions_saved {
        Some(n) => check("会话文件", Level::Ok, format!("已保存 {n} 条")),
        None => check("会话文件", Level::Ok, "还没有保存过（首次保存时生成）"),
    });

    out.push(check(
        "能力面",
        Level::Ok,
        format!(
            "{} 条命令 / 已接线 {} / 置灰 {}",
            i.wired + i.planned,
            i.wired,
            i.planned
        ),
    ));

    match i.term.as_deref() {
        None | Some("") => out.push(check(
            "$TERM",
            Level::Warn,
            "未设置 —— 子进程会用它自己的默认值，无影响但值得知道",
        )),
        Some(t) => out.push(check("$TERM", Level::Ok, t.to_string())),
    }

    out
}

fn collect() -> Input {
    let shell = std::env::var("SHELL").ok();
    let shell_exists = shell
        .as_deref()
        .map(|s| Path::new(s).is_file())
        .unwrap_or(false);
    let profile = persist::sessions_path();
    let profile_dir = profile
        .as_ref()
        .and_then(|p| p.parent().map(|d| d.display().to_string()));
    let profile_writable = profile
        .as_ref()
        .and_then(|p| p.parent())
        .map(writable)
        .unwrap_or(false);
    let palette = theme_load::load_palette();
    let saved = persist::load().map(|(_, s)| s.len());
    let (wired, planned) = capability::counts();
    Input {
        os: std::env::consts::OS.to_string(),
        arch: std::env::consts::ARCH.to_string(),
        term: std::env::var("TERM").ok(),
        shell,
        shell_exists,
        profile_dir,
        profile_writable,
        theme_name: palette.theme_name,
        theme_source: palette.ansi_source,
        sessions_saved: saved,
        wired,
        planned,
    }
}

/// 真写一个探针文件再删掉：`readonly()` 只看权限位，遇到只读挂载或 ACL
/// 会给出「可写」的错误答案，而这个检查存在的意义就是别骗人。
fn writable(dir: &Path) -> bool {
    if std::fs::create_dir_all(dir).is_err() {
        return false;
    }
    let probe = dir.join(".doctor-probe");
    let ok = std::fs::write(&probe, b"").is_ok();
    let _ = std::fs::remove_file(&probe);
    ok
}

pub fn render(checks: &[Check]) -> String {
    let mut out = format!("环境体检（term-desk {}）\n", env!("CARGO_PKG_VERSION"));
    for c in checks {
        let tag = match c.level {
            Level::Ok => "ok",
            Level::Warn => "warn",
            Level::Fail => "fail",
        };
        out.push_str(&format!("  {tag:<4} {} {}\n", pad(c.name, 14), c.detail));
    }
    let fails = checks.iter().filter(|c| c.level == Level::Fail).count();
    let warns = checks.iter().filter(|c| c.level == Level::Warn).count();
    out.push_str(&if fails == 0 {
        match warns {
            0 => "结论：可以运行\n".to_string(),
            n => format!("结论：可以运行（{n} 项提醒）\n"),
        }
    } else {
        format!("结论：缺少 {fails} 项必需条件，本地会话不可用\n")
    });
    out
}

pub fn run() -> i32 {
    let checks = judge(&collect());
    print!("{}", render(&checks));
    if checks.iter().any(|c| c.level == Level::Fail) {
        1
    } else {
        0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn healthy() -> Input {
        let (wired, planned) = capability::counts();
        Input {
            os: "macos".into(),
            arch: "aarch64".into(),
            term: Some("xterm-256color".into()),
            shell: Some("/bin/zsh".into()),
            shell_exists: true,
            profile_dir: Some("/tmp/profile".into()),
            profile_writable: true,
            theme_name: "dige-black".into(),
            theme_source: "/themes/dige-black/scheme.theme".into(),
            sessions_saved: Some(3),
            wired,
            planned,
        }
    }

    fn level_of(checks: &[Check], name: &str) -> Level {
        checks.iter().find(|c| c.name == name).expect("缺项").level
    }

    #[test]
    fn a_healthy_machine_has_no_failures() {
        let checks = judge(&healthy());
        assert!(checks.iter().all(|c| c.level != Level::Fail));
        assert!(render(&checks).contains("结论：可以运行"));
    }

    #[test]
    fn a_missing_shell_blocks_local_sessions() {
        for shell in [None, Some(String::new())] {
            let mut i = healthy();
            i.shell = shell;
            i.shell_exists = false;
            assert_eq!(level_of(&judge(&i), "$SHELL"), Level::Fail);
        }
    }

    #[test]
    fn a_shell_that_is_not_on_disk_is_a_failure_too() {
        let mut i = healthy();
        i.shell = Some("/nope/zsh".into());
        i.shell_exists = false;
        let checks = judge(&i);
        assert_eq!(level_of(&checks, "$SHELL"), Level::Fail);
        assert!(render(&checks).contains("缺少 1 项必需条件"));
    }

    #[test]
    fn fallback_theme_and_read_only_profile_are_only_warnings() {
        let mut i = healthy();
        i.theme_source = ANSI_SOURCE_BUILTIN.into();
        i.profile_writable = false;
        let checks = judge(&i);
        assert_eq!(level_of(&checks, "主题"), Level::Warn);
        assert_eq!(level_of(&checks, "profile 目录"), Level::Warn);
        assert!(render(&checks).contains("（2 项提醒）"));
    }

    #[test]
    fn capability_numbers_come_from_the_inventory() {
        let (wired, planned) = capability::counts();
        let checks = judge(&healthy());
        let cap = checks.iter().find(|c| c.name == "能力面").unwrap();
        assert_eq!(
            cap.detail,
            format!(
                "{} 条命令 / 已接线 {wired} / 置灰 {planned}",
                wired + planned
            )
        );
    }
}

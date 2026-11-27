//! 首次启动的本地 shell 探测：宿主读事实，纯函数出列表。
//!
//! 分层与 [`crate::host`] 同一条判据：`/etc/shells`、环境变量、文件存在性都是
//! **本机事实**，只能在这一层读；UI（`construct.rs`）只消费 [`DetectedShell`]，
//! 不碰 `std::env`。真正的判定写成 [`detect_shells`]——所有输入都由调用方
//! **注入**，它自己既不读环境也不碰文件系统，同一份输入在任何机器上得到
//! 同一份输出（与 `host::login_from` 的口径一致，便于单测钉死行为）。
//!
//! 约定：`available_paths` 是宿主预先筛过的「存在且可执行」路径清单；
//! `detect_shells` 只用它做成员判定，不自己去 stat。这样 Windows 注入路径、
//! Unix 注入文本都不依赖跑测那台机器。

use crate::host::shell_basename_from;
use std::path::Path;

/// 探测平台。做成注入值，测试才能在 Unix 机器上验 Windows 分支。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ProbePlatform {
    Unix,
    Windows,
}

impl Default for ProbePlatform {
    fn default() -> Self {
        if cfg!(windows) {
            Self::Windows
        } else {
            Self::Unix
        }
    }
}

/// 一条探测结果：显示名 + 启动路径。
///
/// `name` 取自 [`shell_basename_from`] 的口径（最后一段路径，跨平台切 `/` 与 `\`），
/// `path` 是宿主筛过的可执行路径，写进 `SessionConfig::shell_command`。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DetectedShell {
    pub name: String,
    pub path: String,
}

/// [`detect_shells`] 的全部输入——没有一项来自函数内部的环境读取。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ShellProbeInput {
    pub platform: ProbePlatform,
    /// `/etc/shells` 全文；缺文件给 `None`。
    pub etc_shells: Option<String>,
    /// `$SHELL`（Unix）。
    pub shell: Option<String>,
    /// `%COMSPEC%`（Windows）。
    pub comspec: Option<String>,
    /// 已知候选路径（Windows 的 cmd/powershell/pwsh；Unix 可只给 `/bin/sh`）。
    pub known_paths: Vec<String>,
    /// 宿主筛过的「存在且可执行」路径清单；成员判定只信它。
    pub available_paths: Vec<String>,
}

/// 注入值 → 有序、去重的 shell 列表；探测不到时给**一条**兜底。
///
/// Unix：`$SHELL` 有效时排首位，其余按 `/etc/shells` 原顺序，最后补已知路径。
/// Windows：`%COMSPEC%` 优先，其后是注入的已知路径。路径或显示名重复都跳过
/// （Windows 下比较忽略 ASCII 大小写）；注释行、空行、不在可用清单里的路径
/// 一律丢弃。兜底是 Unix `/bin/sh` / Windows `cmd.exe`，保证新机器也有会话。
pub fn detect_shells(input: ShellProbeInput) -> Vec<DetectedShell> {
    let candidates = match input.platform {
        ProbePlatform::Unix => unix_candidates(&input),
        ProbePlatform::Windows => windows_candidates(&input),
    };
    let mut out: Vec<DetectedShell> = Vec::new();
    for raw in candidates {
        let path = raw.trim();
        if path.is_empty() || !available(&input, path) {
            continue;
        }
        if out.iter().any(|sh| same(input.platform, &sh.path, path)) {
            continue;
        }
        let name = shell_basename_from(Some(path));
        if out.iter().any(|sh| same(input.platform, &sh.name, &name)) {
            continue;
        }
        out.push(DetectedShell {
            name,
            path: path.to_string(),
        });
    }
    if out.is_empty() {
        out.push(fallback(input.platform));
    }
    out
}

/// 读本机事实并注入 [`detect_shells`]。UI 首次启动时只调这一函数。
///
/// 存在性 / 可执行位在这里判定：`is_usable` 是唯一的 stat 入口，纯函数那侧
/// 永远看不到文件系统。`/etc/shells` 只在 Unix 分支读，Windows 分支不碰它。
pub fn detect_host_shells() -> Vec<DetectedShell> {
    let platform = ProbePlatform::default();
    let shell = non_empty(std::env::var("SHELL").ok());
    let comspec = non_empty(std::env::var("COMSPEC").ok());
    let etc_shells = match platform {
        ProbePlatform::Unix => std::fs::read_to_string("/etc/shells").ok(),
        ProbePlatform::Windows => None,
    };
    let known_paths = match platform {
        ProbePlatform::Unix => vec!["/bin/sh".to_string()],
        ProbePlatform::Windows => windows_known_paths(),
    };

    let mut candidates: Vec<String> = Vec::new();
    if let Some(s) = &shell {
        candidates.push(s.clone());
    }
    if let Some(text) = &etc_shells {
        candidates.extend(
            text.lines()
                .map(str::trim)
                .filter(|line| !line.is_empty() && !line.starts_with('#'))
                .map(str::to_string),
        );
    }
    if let Some(c) = &comspec {
        candidates.push(c.clone());
    }
    candidates.extend(known_paths.iter().cloned());
    let available_paths = candidates
        .into_iter()
        .filter(|path| is_usable(path))
        .collect();

    detect_shells(ShellProbeInput {
        platform,
        etc_shells,
        shell,
        comspec,
        known_paths,
        available_paths,
    })
}

fn non_empty(raw: Option<String>) -> Option<String> {
    raw.filter(|s| !s.trim().is_empty())
}

fn unix_candidates(input: &ShellProbeInput) -> Vec<String> {
    let mut out = Vec::new();
    if let Some(shell) = &input.shell {
        out.push(shell.clone());
    }
    if let Some(text) = &input.etc_shells {
        out.extend(
            text.lines()
                .map(str::trim)
                .filter(|line| !line.is_empty() && !line.starts_with('#'))
                .map(str::to_string),
        );
    }
    out.extend(input.known_paths.iter().cloned());
    out
}

fn windows_candidates(input: &ShellProbeInput) -> Vec<String> {
    let mut out = Vec::new();
    if let Some(comspec) = &input.comspec {
        out.push(comspec.clone());
    }
    out.extend(input.known_paths.iter().cloned());
    out
}

/// 只认宿主注入的「可用清单」，比较规则按平台（Windows 忽略 ASCII 大小写）。
fn available(input: &ShellProbeInput, path: &str) -> bool {
    input
        .available_paths
        .iter()
        .any(|known| same(input.platform, known.trim(), path))
}

fn same(platform: ProbePlatform, a: &str, b: &str) -> bool {
    match platform {
        ProbePlatform::Unix => a == b,
        ProbePlatform::Windows => a.eq_ignore_ascii_case(b),
    }
}

fn fallback(platform: ProbePlatform) -> DetectedShell {
    let path = match platform {
        ProbePlatform::Unix => "/bin/sh",
        ProbePlatform::Windows => "cmd.exe",
    };
    DetectedShell {
        name: shell_basename_from(Some(path)),
        path: path.to_string(),
    }
}

fn windows_known_paths() -> Vec<String> {
    let root = std::env::var("SystemRoot").unwrap_or_else(|_| r"C:\Windows".to_string());
    let root = root.trim_end_matches(['/', '\\']);
    vec![
        format!(r"{root}\System32\cmd.exe"),
        format!(r"{root}\System32\WindowsPowerShell\v1.0\powershell.exe"),
        format!(r"{root}\System32\pwsh.exe"),
        r"C:\Program Files\PowerShell\7\pwsh.exe".to_string(),
    ]
}

/// 唯一的 filesystem 入口：普通文件 +（Unix）至少一个执行位。
fn is_usable(path: &str) -> bool {
    let Ok(meta) = std::fs::metadata(Path::new(path)) else {
        return false;
    };
    if !meta.is_file() {
        return false;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        meta.permissions().mode() & 0o111 != 0
    }
    #[cfg(not(unix))]
    {
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn probe(platform: ProbePlatform, available: &[&str]) -> ShellProbeInput {
        ShellProbeInput {
            platform,
            available_paths: available.iter().map(|s| s.to_string()).collect(),
            ..Default::default()
        }
    }

    #[test]
    fn etc_shells_are_kept_in_order_and_deduplicated() {
        let mut input = probe(ProbePlatform::Unix, &["/bin/bash", "/bin/zsh", "/bin/dash"]);
        input.etc_shells = Some("# shells\n/bin/bash\n/bin/zsh\n/bin/bash\n/bin/dash\n".into());
        assert_eq!(
            detect_shells(input),
            vec![
                DetectedShell {
                    name: "bash".into(),
                    path: "/bin/bash".into()
                },
                DetectedShell {
                    name: "zsh".into(),
                    path: "/bin/zsh".into()
                },
                DetectedShell {
                    name: "dash".into(),
                    path: "/bin/dash".into()
                },
            ]
        );
    }

    #[test]
    fn shell_is_first_and_unavailable_entries_are_dropped() {
        let mut input = probe(ProbePlatform::Unix, &["/bin/zsh", "/bin/bash"]);
        input.shell = Some("/bin/zsh".into());
        input.etc_shells = Some("/bin/bash\n/bin/zsh\n/bin/fish\n".into());
        let got = detect_shells(input);
        assert_eq!(
            got.iter().map(|sh| sh.name.as_str()).collect::<Vec<_>>(),
            ["zsh", "bash"]
        );
        assert_eq!(got[0].path, "/bin/zsh");
    }

    #[test]
    fn comments_blanks_and_non_executables_are_ignored() {
        let mut input = probe(ProbePlatform::Unix, &["/bin/sh"]);
        input.etc_shells = Some("\n# comment\n/bin/sh\n/usr/sbin/nologin\n".into());
        assert_eq!(
            detect_shells(input),
            vec![DetectedShell {
                name: "sh".into(),
                path: "/bin/sh".into()
            }]
        );
    }

    #[test]
    fn names_follow_the_host_basename_rule() {
        let mut input = probe(ProbePlatform::Unix, &["/usr/local/bin/pwsh"]);
        input.etc_shells = Some("/usr/local/bin/pwsh\n".into());
        assert_eq!(detect_shells(input)[0].name, "pwsh");
    }

    #[test]
    fn windows_uses_comspec_then_known_paths() {
        let mut input = probe(
            ProbePlatform::Windows,
            &[
                r"C:\Windows\System32\cmd.exe",
                r"C:\Windows\System32\WindowsPowerShell\v1.0\powershell.exe",
                r"C:\Program Files\PowerShell\7\pwsh.exe",
            ],
        );
        input.comspec = Some(r"C:\WINDOWS\SYSTEM32\CMD.EXE".into());
        input.known_paths = vec![
            r"C:\Windows\System32\cmd.exe".into(),
            r"C:\Windows\System32\WindowsPowerShell\v1.0\powershell.exe".into(),
            r"C:\Program Files\PowerShell\7\pwsh.exe".into(),
        ];
        let got = detect_shells(input);
        assert_eq!(
            got.iter().map(|sh| sh.name.as_str()).collect::<Vec<_>>(),
            ["CMD.EXE", "powershell.exe", "pwsh.exe"]
        );
        // COMSPEC 大小写不同也只出一条，且占首位。
        assert_eq!(got[0].path, r"C:\WINDOWS\SYSTEM32\CMD.EXE");
    }

    #[test]
    fn windows_drops_paths_that_are_not_available() {
        let mut input = probe(ProbePlatform::Windows, &[r"C:\Windows\System32\cmd.exe"]);
        input.comspec = Some(r"C:\Windows\System32\cmd.exe".into());
        input.known_paths = vec![r"C:\Windows\System32\pwsh.exe".into()];
        let got = detect_shells(input);
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].name, "cmd.exe");
    }

    #[test]
    fn unix_fallback_is_one_usable_shell() {
        let got = detect_shells(probe(ProbePlatform::Unix, &[]));
        assert_eq!(
            got,
            vec![DetectedShell {
                name: "sh".into(),
                path: "/bin/sh".into()
            }]
        );
    }

    #[test]
    fn windows_fallback_is_one_usable_shell() {
        let got = detect_shells(probe(ProbePlatform::Windows, &[]));
        assert_eq!(
            got,
            vec![DetectedShell {
                name: "cmd.exe".into(),
                path: "cmd.exe".into()
            }]
        );
    }

    #[test]
    fn host_probe_never_returns_an_empty_list() {
        // 本机事实只做「非空」断言，不把跑测机器上的 shell 集钉死。
        assert!(!detect_host_shells().is_empty());
    }
}

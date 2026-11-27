//! 窗口的默认会话列表：纯函数拆分，便于证明「已有会话库时绝不探测」。
//!
//! `TerminalWindow::new` 只在 `persist::load()` 失败（首次启动 / 库坏了）时
//! 走这里。输入里的 `loaded` 是「要么原样交回、要么完全不碰」的那一份用户
//! 数据；`detect` / `login` 都是闭包，测试可以传一个会 panic 的闭包来证明
//! 读库成功时一行探测都没执行。真正的宿主读取在 `term_io::host_shells`，
//! 这里只把结果投影成会话配置。

use term_core::config::DEFAULT_GROUP;
use term_core::session::SessionConfig;
use term_io::host_shells::DetectedShell;

/// 原版会话树里的分组名（`SPEC.md` / `design-spec.md` 的 `Shell sessions`）。
pub(in crate::ui) const SHELL_GROUP: &str = "Shell sessions";

/// 已读到的会话库 + 注入的探测器 → 窗口启动时用的分组与会话。
///
/// `loaded` 有值时**逐字交回**，不追加、不覆盖、不探测（用户数据优先）。
/// 否则按探测顺序造本地 Shell 会话：名字是 shell 显示名，用户名取宿主
/// 登录名，路径写进 `shell_command`；分组保留 `Servers` 供新建 SSH 使用，
/// 但不再造 `My Server` 这类假条目。
pub(in crate::ui) fn startup_defaults(
    loaded: Option<(Vec<String>, Vec<SessionConfig>)>,
    detect: impl FnOnce() -> Vec<DetectedShell>,
    login: impl FnOnce() -> String,
) -> (Vec<String>, Vec<SessionConfig>) {
    if let Some(found) = loaded {
        return found;
    }
    let login = login();
    let sessions = detect()
        .into_iter()
        .map(|shell| {
            let mut cfg = SessionConfig::new_local_shell(&login);
            cfg.name = shell.name;
            cfg.group = SHELL_GROUP.to_string();
            cfg.shell_command = shell.path;
            cfg
        })
        .collect();
    (
        vec![SHELL_GROUP.to_string(), DEFAULT_GROUP.to_string()],
        sessions,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use term_core::session::Protocol;

    fn detected(name: &str, path: &str) -> DetectedShell {
        DetectedShell {
            name: name.to_string(),
            path: path.to_string(),
        }
    }

    #[test]
    fn a_loaded_library_is_returned_as_is_without_probing() {
        let mut ssh = SessionConfig::new(Protocol::Ssh);
        ssh.name = "prod".to_string();
        let loaded = Some((vec!["My Group".to_string()], vec![ssh.clone()]));
        let got = startup_defaults(
            loaded,
            || panic!("已有会话库时不许探测"),
            || panic!("已有会话库时不许读登录名"),
        );
        assert_eq!(got, (vec!["My Group".to_string()], vec![ssh]));
    }

    #[test]
    fn first_start_builds_one_local_shell_per_detected_shell() {
        let (groups, sessions) = startup_defaults(
            None,
            || vec![detected("zsh", "/bin/zsh"), detected("bash", "/bin/bash")],
            || "alice".to_string(),
        );
        assert_eq!(groups, [SHELL_GROUP, DEFAULT_GROUP]);
        assert_eq!(sessions.len(), 2);
        for (cfg, (name, path)) in sessions
            .iter()
            .zip([("zsh", "/bin/zsh"), ("bash", "/bin/bash")])
        {
            assert_eq!(cfg.protocol, Protocol::Shell);
            assert_eq!(cfg.name, name);
            assert_eq!(cfg.group, SHELL_GROUP);
            assert_eq!(cfg.username, "alice");
            assert_eq!(cfg.shell_command, path);
        }
    }

    #[test]
    fn first_start_never_fabricates_an_ssh_entry() {
        let (_, sessions) = startup_defaults(
            None,
            || vec![detected("sh", "/bin/sh")],
            || "alice".to_string(),
        );
        assert_eq!(sessions.len(), 1);
        assert!(sessions.iter().all(|cfg| cfg.protocol == Protocol::Shell));
        assert!(sessions.iter().all(|cfg| cfg.host.is_empty()));
    }
}

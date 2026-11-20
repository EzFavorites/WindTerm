//! 自动补全的静态词库（I-auto-completion / AC-01）。
//!
//! # 边界
//!
//! 这里只放**纯数据**：命令名 → 子命令 / 选项 / 参数。零 IO、零墙钟、零 UI。
//! 排序与过滤在 [`crate::completion`]，本文件不认识偏好、历史或屏幕。
//!
//! # 覆盖面
//!
//! 刻意只收高频命令与 git / docker / cargo 的常用子表——目标是「常见的敲两下
//! 就能补」，不是 man page 全量镜像。词库不足时历史与 QuickBar 两个动态源
//! 仍然可用，不会让补全整体失效。
//!
//! # 为什么要分三张子表
//!
//! 设置页三个独立开关（Command Name / Command Option / Command Parameter）
//! 各自对应这里的一类词；合并成一张表就没法按视觉基线的三个勾分别控制。

/// 一个命令的静态补全词表。所有切片都是 `'static`，运行时零分配。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CommandSpec {
    /// 命令名（第一行第一个词）。
    pub name: &'static str,
    /// 子命令，如 `git status`。
    pub subcommands: &'static [&'static str],
    /// 选项，如 `git --oneline`。
    pub options: &'static [&'static str],
    /// 常见参数值，如 `git push origin`。
    pub parameters: &'static [&'static str],
}

const GIT: CommandSpec = CommandSpec {
    name: "git",
    subcommands: &[
        "status", "add", "commit", "push", "pull", "fetch", "clone", "checkout",
        "branch", "merge", "rebase", "log", "diff", "reset", "revert", "stash",
        "tag", "remote", "show", "init", "config",
    ],
    options: &[
        "--help", "--version", "--verbose", "--quiet", "--all", "--oneline",
        "--graph", "--dry-run", "--stat", "--cached", "--force",
    ],
    parameters: &["origin", "upstream", "main", "master", "HEAD", "origin/main"],
};

const DOCKER: CommandSpec = CommandSpec {
    name: "docker",
    subcommands: &[
        "run", "ps", "images", "build", "exec", "logs", "stop", "start", "rm",
        "rmi", "pull", "push", "compose", "inspect", "network", "volume",
    ],
    options: &[
        "--help", "--detach", "--interactive", "--tty", "--volume", "--publish",
        "--name", "--rm", "--env", "--workdir",
    ],
    parameters: &["latest", "alpine", "ubuntu"],
};

const CARGO: CommandSpec = CommandSpec {
    name: "cargo",
    subcommands: &[
        "build", "check", "test", "run", "fmt", "clippy", "add", "update", "doc",
        "clean", "metadata", "install",
    ],
    options: &[
        "--release", "--all-targets", "--quiet", "--verbose", "--features",
        "--package", "--workspace", "--locked",
    ],
    parameters: &["--all", "--lib", "--bins"],
};

const LS: CommandSpec = CommandSpec {
    name: "ls",
    subcommands: &[],
    options: &["-a", "-l", "-h", "-r", "-t", "-S", "-1", "--color", "--help"],
    parameters: &[],
};

const GREP: CommandSpec = CommandSpec {
    name: "grep",
    subcommands: &[],
    options: &["-i", "-n", "-r", "-v", "-E", "-w", "-l", "-c", "--color", "--help"],
    parameters: &[],
};

const SSH: CommandSpec = CommandSpec {
    name: "ssh",
    subcommands: &[],
    options: &["-p", "-i", "-v", "-L", "-R", "-N", "-f", "-o", "--help"],
    parameters: &[],
};

const FIND: CommandSpec = CommandSpec {
    name: "find",
    subcommands: &[],
    options: &["-name", "-type", "-mtime", "-exec", "-maxdepth", "-print"],
    parameters: &[],
};

const TAR: CommandSpec = CommandSpec {
    name: "tar",
    subcommands: &[],
    options: &["-x", "-c", "-z", "-f", "-v", "-t", "--help"],
    parameters: &[],
};

/// 有专用子表的命令。顺序即候选的默认展示顺序（同分时稳定）。
pub static SPECS: &[CommandSpec] = &[
    GIT, DOCKER, CARGO, LS, GREP, SSH, FIND, TAR,
];

/// 只有命令名、没有子表的常见命令。它们只贡献 `Command Name` 一类候选。
pub static PLAIN_COMMANDS: &[&str] = &[
    "cd", "pwd", "echo", "cat", "mkdir", "rm", "cp", "mv", "touch", "chmod",
    "chown", "head", "tail", "sed", "awk", "sort", "uniq", "wc", "curl", "wget",
    "scp", "zip", "unzip", "ps", "top", "kill", "export", "source", "alias",
    "history", "clear", "man", "which", "df", "du", "free", "uptime", "date",
    "env", "printenv", "sleep", "watch", "xargs", "tee", "diff", "patch",
    "python3", "pip3", "node", "npm", "vim", "nano", "tmux", "screen",
];

/// 按命令名找专用子表。
pub fn spec(name: &str) -> Option<&'static CommandSpec> {
    SPECS.iter().find(|s| s.name == name)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn specs_have_unique_names() {
        let mut names: Vec<&str> = SPECS.iter().map(|s| s.name).collect();
        names.sort_unstable();
        let before = names.len();
        names.dedup();
        assert_eq!(names.len(), before, "命令名不能重复，否则候选会重复");
    }

    #[test]
    fn plain_commands_do_not_overlap_specs() {
        for name in PLAIN_COMMANDS {
            assert!(spec(name).is_none(), "{name} 同时出现在两张表里");
        }
    }

    #[test]
    fn git_has_all_three_kinds() {
        let git = spec("git").expect("git 必须有子表");
        assert!(!git.subcommands.is_empty());
        assert!(!git.options.is_empty());
        assert!(!git.parameters.is_empty());
    }

    #[test]
    fn unknown_command_has_no_spec() {
        assert!(spec("definitely-not-a-command").is_none());
    }
}

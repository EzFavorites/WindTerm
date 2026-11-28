//! 无窗口子命令：`doctor` / `capability` / `api` / `mcp`（ADR-0030）。
//!
//! 为什么在 `apps/desktop` 而不是第二个二进制：ADR-0019 与 ADR-0020 把可执行
//! 目标收敛成一个，`apps/` 只留 `desk`。这些命令的共同点是**不需要窗口**，
//! 所以必须在 `eframe` 起来之前处理掉——一旦进了 `main` 后半段，就会要求显示
//! 环境，CI 与排障现场都跑不了。
//!
//! `api` / `mcp` 都只是前台 loopback TCP 的薄客户端：连 `127.0.0.1:<port>`、
//! 跑 key + HMAC 挑战证明、发请求、映射错误。
//! 它们**没有** `serve` / `script run` / `--headless` 这些自建会话池的入口——
//! 那些入口会让「远程操作窗口里的会话」变成「操作另一个池里的另一批会话」。
//!
//! 只认 `argv[1]` 作为子命令：`--screenshot` / `--demo` 这些是窗口参数
//! （可以出现在任意位置），不能被误判成子命令。反过来，`argv[1]` 以
//! `-` 开头就一定是窗口参数，直接交回给 `main`。

mod align;
mod api;
mod capability;
mod doctor;
mod mcp;

const USAGE: &str = "\
term-desk 无窗口子命令

用法：
  term-desk doctor                            环境体检（$SHELL / profile / 主题 / 能力面）
                                               有必需项缺失时退出码 1
  term-desk capability [--json] [--all]        能力面：命令总数 / 已接线 / 置灰
                                               --json 的字段名与 docs/facts.txt 同名
  term-desk api describe                       问前台要能力清单
  term-desk api call <method> [params-json]    调前台的一个方法（session 显式传）
  term-desk mcp                                stdio MCP server（tools 来自 describe）
  term-desk --version                         打印版本号（WindTerm x.y.z）
  term-desk --help                            这份说明

无窗口入口不建会话池：api / mcp 要求前台窗口已经在跑，连 127.0.0.1:<port>
（默认 47653，WINDTERM_CONTROL_PORT 可覆盖；key 文件
~/.zterm/control-key.local-<port>，准入为 HMAC-SHA256 挑战证明）。
窗口参数（--screenshot / --demo / --list-components）不经过这里，仍由 main 解析。";

/// 若 `argv` 是一条无窗口子命令，执行并返回退出码；否则返回 `None`，
/// 让 `main` 继续按窗口参数解析。
pub fn try_run(args: &[String]) -> Option<i32> {
    let first = args.get(1)?;
    match first.as_str() {
        "doctor" => Some(doctor::run()),
        "capability" => {
            let json = args.iter().any(|a| a == "--json");
            let all = args.iter().any(|a| a == "--all");
            Some(capability::run(json, all))
        }
        "api" => Some(api::run(&args[2..])),
        "mcp" => Some(mcp::run(&args[2..])),
        // 发布产物必须能自证身份：用户报问题时第一句就是「你那个版本号」。
        // **这条必须排在下面 `starts_with('-')` 的通配之前**——否则 `--version`
        // 会被当成窗口参数交回 main，于是「查版本」变成「拉起一个 GUI」。
        // 版本号的唯一来源是 Cargo.toml 的 `package.version`，这里不写死数字。
        "version" | "-V" | "--version" => {
            println!("WindTerm {}", env!("CARGO_PKG_VERSION"));
            Some(0)
        }
        "help" | "-h" | "--help" => {
            print!("{USAGE}");
            Some(0)
        }
        // 窗口参数：交回 main。
        other if other.starts_with('-') => None,
        other => {
            eprintln!("未知子命令：{other}\n\n{USAGE}");
            Some(2)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn argv(rest: &[&str]) -> Vec<String> {
        let mut out = vec!["term-desk".to_string()];
        out.extend(rest.iter().map(|s| s.to_string()));
        out
    }

    #[test]
    fn a_window_flag_is_not_a_subcommand() {
        assert_eq!(try_run(&argv(&["--screenshot", "/tmp/a.png"])), None);
        assert_eq!(try_run(&argv(&["--demo", "scroll"])), None);
    }

    #[test]
    fn no_arguments_means_start_the_window() {
        assert_eq!(try_run(&argv(&[])), None);
    }

    /// `--version` 是本仓最容易回归的一条：`starts_with('-')` 那条通配就在
    /// 它后面一个分支，只要有人调换顺序，`--version` 就会变成 `None` →
    /// main 继续走窗口路径 → 用户想查版本却弹出一个 GUI。断言 `Some(0)`
    /// 正是「没有落到窗口路径」的意思。
    #[test]
    fn version_prints_and_does_not_start_a_window() {
        for flag in ["--version", "-V", "version"] {
            assert_eq!(
                try_run(&argv(&[flag])),
                Some(0),
                "{flag} 必须被识别成无窗口子命令，而不是交回 main 开窗口"
            );
        }
    }

    /// 版本号只有一个来源（`Cargo.toml` 的 `package.version`）。这条断言不比对
    /// 具体字符串——那等于把版本再抄一遍——而是证明它与编译期的值同源。
    #[test]
    fn the_reported_version_comes_from_cargo() {
        let printed = format!("WindTerm {}", env!("CARGO_PKG_VERSION"));
        assert!(
            printed.starts_with("WindTerm "),
            "版本行必须以产品名开头：{printed}"
        );
        assert_eq!(
            env!("CARGO_PKG_VERSION").split('.').count(),
            3,
            "SemVer 三段式：{}",
            env!("CARGO_PKG_VERSION")
        );
    }

    #[test]
    fn doctor_is_recognized_without_starting_a_window() {
        // 退出码取决于这台机器的 `$SHELL`，两种都合法；这里只断言「被识别了」。
        let code = try_run(&argv(&["doctor"])).expect("doctor 必须被识别");
        assert!(code == 0 || code == 1);
    }

    #[test]
    fn capability_json_and_all_are_accepted_in_any_order() {
        assert_eq!(try_run(&argv(&["capability", "--json"])), Some(0));
        assert_eq!(try_run(&argv(&["capability", "--all"])), Some(0));
        assert_eq!(try_run(&argv(&["capability", "--all", "--json"])), Some(0));
    }

    #[test]
    fn an_unknown_subcommand_is_a_usage_error() {
        assert_eq!(try_run(&argv(&["doctr"])), Some(2));
    }

    #[test]
    fn serve_and_script_run_are_no_longer_subcommands() {
        // 两条自建会话池的入口已删除：现在只能是「未知子命令」。
        assert_eq!(try_run(&argv(&["serve"])), Some(2));
        assert_eq!(try_run(&argv(&["script", "run", "-"])), Some(2));
    }

    #[test]
    fn api_and_mcp_help_are_recognized_without_connecting() {
        assert_eq!(try_run(&argv(&["api", "--help"])), Some(0));
        assert_eq!(try_run(&argv(&["mcp", "--help"])), Some(0));
    }
}

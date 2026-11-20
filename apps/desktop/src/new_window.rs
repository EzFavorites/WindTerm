//! `Application.NewWindow`：**再起一个完整、独立的应用实例（新进程）**。
//!
//! 不是新 tab、不是分屏、也不是同进程第二 viewport。每个实例自带一份窗口态
//! （标签 / 分屏 / 活动会话），`prefs.json` 与 SQLite 会话库仍按应用级状态共享。
//! 新窗口**不恢复布局**：`layout.json` 是单份全局状态，若照常 `restore_layout()`
//! 会恢复成和旧窗口一模一样的标签与分屏，那就不是「独立」了。判据在
//! `ui/harness.rs::secondary_window`，跳过恢复后 `TerminalWindow::new`
//! 本来就先建一条 live 本地 shell 标签，所以出厂自然是一个干净窗口。
//!
//! # 为什么放在 `ui/` 之外
//!
//! `scripts/verify.sh` 第 5 项的判据是「`ui/` 不得持有 OS 资源」。拉起一个
//! 进程不是画——`open_with.rs` / `sysopen.rs` / `main.rs` 的模块区都写着同一条
//! 口径。本文件与它们同级，投影层只经 `Effect::NewWindow` 说一句意图，一个
//! `Command` 都不进 `ui/`。
//!
//! # 新进程零 argv
//!
//! `main.rs` 的 `--screenshot` / `--exec` / `--demo` / `--headless` /
//! `--list-components` 全是夹具与 CI 用途，没有用户级参数；`cli::try_run` 只认
//! `argv[1]`，任何裸位置子命令都会打印 USAGE 并 `exit(2)`。所以透传白名单
//! ([`FORWARDABLE`]) **为空**：全丢弃。将来真要透传某条 flag，必须显式加进白名单
//! 并补测试——别把「白名单为空」改写成「全透传」。
//!
//! # 环境变量：夹具态清，配置态留
//!
//! 光过滤 argv **不够**：`Command::spawn` 默认继承环境变量，而 harness 与状态栏
//! 只看环境变量、拿不到 argv。判据不是「谁设的」，而是**夹具态 vs 配置态**：
//!
//! | 变量 | 处置 | 理由 |
//! |---|---|---|
//! | `WINDTERM_SCREENSHOT_TO` | `env_remove` | 夹具态。`ui/harness.rs::fixture_mode` 用 `var_os().is_some()` 判**存在性**，必须真删，设空串无效。不清则 `main.rs` 让新窗口 `with_visible(false)` + macOS `ActivationPolicy::Prohibited` 隐形不上前台；`shot.rs` 存完图直接 `process::exit(0)`，用户看到的是「窗口闪一下就没了」，正是禁止的「点了没反应」；`image::save_buffer` 还直接写固定路径、非原子，两进程会并发写同一 PNG |
//! | `WINDTERM_COMP_DEMO` | `env_remove` | 夹具态。不清则新窗口进静态组件画面而非真实终端 |
//! | `WINDTERM_SECONDARY_WINDOW` | `env("1")` 显式设置 | 角色标记，`ui/construct.rs::init` 据此跳过 `restore_layout()` |
//! | `WINDTERM_PROFILE_DIR` | **保留继承，不清** | 配置态 / 共享范围开关（`term-paths` 读它）。清掉会让 `prefs.json`、SQLite 会话库、`layout.json` 全都不再共享，违背用户裁定「窗口态独立、配置与会话库共享」 |
//! | `WINDTERM_LANG` | **保留继承，不清** | 配置态。全仓唯一生产读取点 `ui/construct.rs`，UI 改语言走 `ShellAction::SetLanguage` → `prefs.language` 落盘，env 是启动优先级第一档。清掉只在「用户显式 `WINDTERM_LANG=en term-desk` 启动」时制造错误行为（新窗口变成 prefs 里的另一语言）；截图夹具是静态画面，不会有人在其中点 New Window，所谓「避免基线污染」没有真实场景 |
//!
//! 所以 [`STRIP_ENV`] **恰好两个**。单测同时钉住 `PROFILE_DIR` 与 `LANG` 不在清理
//! 清单、也不在 `env_set` 里，免得下一个人把共享开关当漏项补上。保留共享目录的
//! 代价是「后写覆盖」：两个实例都写 `layout.json`，下次启动恢复最后写的那份——
//! 这是共享配置目录的既有语义，由文档记账，不是本轮回归。
//!
//! 伏笔：将来若要**完全独立**的两个窗口，正确做法是给新进程设一个不同的
//! `WINDTERM_PROFILE_DIR`，不需要改架构，也不用再动上面这张表的其他行。
//!
//! # 机器门禁覆盖不到的那一跳（写清缺什么，不假装有）
//!
//! `#[must_use]` 能抓住「plan 造出来了却没调 `launch()`」，`Result` 的 must_use
//! 又能抓住「调了 `launch()` 却没处理失败」。但 **`apply_effect` 的 arm 写成空块
//! `{}` 时，两种都抓不到**——那一刻连 plan 都没造。穷举 match 只保证 arm **存在**，
//! 不保证 arm 里有东西。这与 `debug_paint.rs` 模块头记的 `tab.debug → paint` 是
//! 同一类缺口：`main.rs` 有 crate 级 `#![allow(dead_code)]`，没被调用的
//! [`new_window_plan`] 也不会报。这一跳只能靠人眼验收：点 `Session ▸ New Window`
//! 或按 `Cmd+Shift+N`，没有第二个窗口弹出来就是这一跳断了。**不要**为了补这一跳
//! 去造 egui 上下文或真 spawn 一个窗口（会弹窗、抢前台、CI 挂）。

use std::ffi::{OsStr, OsString};
use std::io;
use std::path::PathBuf;
use std::process::{Command, Stdio};

/// 允许透传给新进程的 flag。**今天为空**（零 argv 契约），见模块头。
const FORWARDABLE: &[&str] = &[];

/// 带一个值的夹具 flag：丢弃时必须连值一起丢，否则那个值会变成孤儿的
/// `argv[1]`，被 `cli::try_run` 当成未知子命令 → `exit(2)`。
const FLAGS_WITH_VALUE: &[&str] = &["--screenshot", "--exec", "--demo"];

/// 不带值的夹具 flag。
const FLAGS_ALONE: &[&str] = &["--headless", "--list-components"];

/// 无窗口位置子命令（`cli::try_run` 认的就是这些）。
const SUBCOMMANDS: &[&str] = &[
    "doctor", "capability", "serve", "script", "version", "-V", "--version", "help", "-h",
    "--help",
];

/// 子进程必须清掉的**夹具态**环境变量，恰好两个（见模块头的分类表）。
/// 配置态变量（`PROFILE_DIR` / `LANG`）一律不清、也不覆盖，由单测钉住。
const STRIP_ENV: &[&str] = &["WINDTERM_SCREENSHOT_TO", "WINDTERM_COMP_DEMO"];

/// 给新进程打的角色标记：`ui/construct.rs::init` 据此跳过 `restore_layout()`。
const ROLE_ENV: &[(&str, &str)] = &[("WINDTERM_SECONDARY_WINDOW", "1")];

/// `arg` 是不是一个带值的夹具 flag。
///
/// **一律用 `OsStr` 比较**：argv 不保证是 UTF-8，`to_str().unwrap()` 既会撞 4p
/// 的生产 panic 基线，又会在非 UTF-8 参数上真的崩。
fn takes_value(arg: &OsStr) -> bool {
    FLAGS_WITH_VALUE.iter().any(|f| OsStr::new(*f) == arg)
}

/// `arg` 是不是一个不带值的夹具 flag 或无窗口子命令。
fn is_fixture_token(arg: &OsStr) -> bool {
    FLAGS_ALONE.iter().any(|f| OsStr::new(*f) == arg)
        || SUBCOMMANDS.iter().any(|s| OsStr::new(*s) == arg)
}

/// 原始 argv（含 `argv[0]`）→ 交给新进程的 argv。
///
/// 白名单为空，所以**任何输入都返回空**：`Command::new(exe)` 自己会补 `argv[0]`。
/// 仍然逐项扫一遍，是为了把带值 flag 的**值**一起吃掉（配对规则），也是为了让
/// 「将来加一条白名单」只改 [`FORWARDABLE`] 一处，不必重写解析。
fn child_args(raw: &[OsString]) -> Vec<OsString> {
    let mut out: Vec<OsString> = Vec::new();
    let mut skip_value = false;
    for arg in raw {
        if skip_value {
            skip_value = false;
            continue;
        }
        let name = arg.as_os_str();
        if takes_value(name) {
            // 末尾无值时 skip_value 留在 true，循环自然收尾，一次都不索引越界。
            skip_value = true;
            continue;
        }
        if is_fixture_token(name) {
            continue;
        }
        if FORWARDABLE.iter().any(|f| OsStr::new(*f) == name) {
            out.push(arg.clone());
        }
    }
    out
}

/// 拉起新窗口的**计划**：只描述「起什么、带什么、改哪些环境变量」，不起进程。
///
/// `#[must_use]` 不是装饰：本仓 `main.rs` 有 crate 级 `#![allow(dead_code)]`，
/// 一个「造了 plan 却忘了 `launch()`」的 arm 在 `cargo test` 与
/// `clippy -D warnings` 下原本会双双全绿。标上之后它直接变成 `unused_must_use`。
#[must_use = "新窗口计划必须经 launch() 消费；拿到计划却不起进程等于点了没反应"]
pub struct SpawnPlan {
    exe: PathBuf,
    args: Vec<OsString>,
    env_remove: &'static [&'static str],
    env_set: &'static [(&'static str, &'static str)],
}

impl SpawnPlan {
    /// 真正拉起进程。**不在 UI 线程等它**：`Child` 交给一条 detach 线程 reap，
    /// 既不让 GUI 卡住，也不让常驻的父进程攒僵尸——`Child` 的 `Drop` 不会
    /// `wait()`，而首实例是长驻 GUI，每开一个窗口不 reap 就多一个僵尸。
    /// 回收线程只持有这一个句柄，不碰任何 `ui/` 资源。
    ///
    /// `exe` 为空说明构造 plan 时 `current_exe()` 没拿到；`new_window_plan()` 用
    /// `unwrap_or_default()` 留下空路径而不 panic，这里转成 `CurrentExe`。
    pub fn launch(self) -> Result<(), NewWindowError> {
        if self.exe.as_os_str().is_empty() {
            return Err(NewWindowError::CurrentExe(io::Error::new(
                io::ErrorKind::NotFound,
                "std::env::current_exe() is unavailable",
            )));
        }
        let mut cmd = Command::new(&self.exe);
        // stdin 不继承（新窗口不需要读父进程的终端）；stdout / stderr **继承**：
        // 三全 null 会吞掉 `main.rs` 的 eframe 启动错误、会话库保存失败等全部诊断，
        // 新窗口坏了将完全无声。GUI 从 Finder 启动时继承到 nowhere，无害。
        cmd.args(&self.args)
            .stdin(Stdio::null())
            .stdout(Stdio::inherit())
            .stderr(Stdio::inherit());
        for &key in self.env_remove {
            cmd.env_remove(key);
        }
        for &(key, value) in self.env_set {
            cmd.env(key, value);
        }
        let mut child = cmd.spawn().map_err(NewWindowError::Spawn)?;
        // 独立实例：绝不在 UI 线程阻塞等它，但必须 reap 防僵尸。
        std::thread::spawn(move || {
            let _ = child.wait();
        });
        Ok(())
    }
}

/// 收集本进程的信息，造一份计划。**不 spawn**，所以能在无头单测里直接调。
pub fn new_window_plan() -> SpawnPlan {
    let raw: Vec<OsString> = std::env::args_os().collect();
    SpawnPlan {
        exe: std::env::current_exe().unwrap_or_default(),
        args: child_args(&raw),
        env_remove: STRIP_ENV,
        env_set: ROLE_ENV,
    }
}

/// 失败原因。**不返回 `Result<_, String>`**：把全仓「错误边界 String 化」冻结在
/// 40 处（只许减不许增），而且「找不到自己」与「起不来」的处置完全不同。
#[derive(Debug)]
pub enum NewWindowError {
    /// 拿不到当前可执行文件路径。
    CurrentExe(io::Error),
    /// `spawn()` 失败（权限、路径没了、系统资源不足……）。
    Spawn(io::Error),
}

impl NewWindowError {
    /// 一行能画到状态栏 `log_note` 上的说明。
    pub fn message(&self) -> String {
        match self {
            NewWindowError::CurrentExe(e) => format!("找不到当前可执行文件：{e}"),
            NewWindowError::Spawn(e) => format!("新窗口起不来：{e}"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 造一条像 `main()` 收到的 argv（含 `argv[0]`）。
    fn argv(rest: &[&str]) -> Vec<OsString> {
        let mut out: Vec<OsString> = vec![OsString::from("term-desk")];
        out.extend(rest.iter().map(OsString::from));
        out
    }

    #[test]
    fn an_empty_arg_list_stays_empty() {
        assert!(child_args(&[]).is_empty());
        assert!(child_args(&argv(&[])).is_empty(), "argv[0] 也不透传");
    }

    #[test]
    fn screenshot_and_its_value_are_dropped_together() {
        let out = child_args(&argv(&["--screenshot", "/tmp/shot.png"]));
        assert!(out.is_empty(), "夹具 flag 与它的值都不许进新进程：{out:?}");
        // 配对规则本身也要钉住：漏了它，值就会变成孤儿 argv[1]。
        assert!(takes_value(OsStr::new("--screenshot")));
        assert!(!takes_value(OsStr::new("--headless")));
    }

    #[test]
    fn demo_and_its_value_are_dropped_together() {
        for rest in [
            vec!["--demo", "scroll"],
            vec!["--demo", "comp:status_bar"],
        ] {
            let out = child_args(&argv(&rest));
            assert!(out.is_empty(), "{rest:?} 不该留下任何 argv：{out:?}");
        }
        assert!(takes_value(OsStr::new("--demo")));
    }

    #[test]
    fn exec_headless_and_list_components_are_dropped() {
        let out = child_args(&argv(&[
            "--exec",
            "printf hello",
            "--headless",
            "--list-components",
        ]));
        assert!(out.is_empty(), "这四条全是夹具：{out:?}");
        assert!(takes_value(OsStr::new("--exec")));
        assert!(is_fixture_token(OsStr::new("--headless")));
        assert!(is_fixture_token(OsStr::new("--list-components")));
    }

    #[test]
    fn positional_subcommands_are_dropped() {
        for sub in ["doctor", "capability", "serve", "script", "version", "help"] {
            assert!(
                child_args(&argv(&[sub])).is_empty(),
                "{sub} 会打印 USAGE 并 exit(2)"
            );
            assert!(is_fixture_token(OsStr::new(sub)), "{sub} 该被认出来");
        }
    }

    #[test]
    fn a_trailing_screenshot_without_a_value_does_not_panic() {
        let out = child_args(&argv(&["--headless", "--screenshot"]));
        assert!(out.is_empty(), "末尾无值也不能 panic 或造出孤儿 argv");
    }

    /// argv 不保证是 UTF-8；整条路径上一次 `to_str()` 都不许有。
    #[cfg(unix)]
    #[test]
    fn non_utf8_argv_is_dropped_without_panicking() {
        use std::os::unix::ffi::OsStringExt;
        let mut raw = argv(&["--headless"]);
        raw.push(OsString::from_vec(vec![b'-', b'-', 0xFF, 0xFE]));
        raw.push(OsString::from_vec(vec![0xFF, 0x80]));
        assert!(child_args(&raw).is_empty());
    }

    /// 非 Unix 上没法用 `OsStringExt::from_vec` 造非法字节，至少钉住未知 flag。
    #[cfg(not(unix))]
    #[test]
    fn non_utf8_argv_is_dropped_without_panicking() {
        assert!(child_args(&argv(&["--unknown-fixture-flag"])).is_empty());
    }

    #[test]
    fn unknown_flags_are_dropped() {
        let out = child_args(&argv(&[
            "--nocapture",
            "--test-threads",
            "1",
            "--some-future-flag",
        ]));
        assert!(out.is_empty(), "白名单为空 → 一律丢弃：{out:?}");
        assert!(FORWARDABLE.is_empty(), "零 argv 契约");
    }

    /// 清单完整性：夹具态恰好这两个；配置态变量一个都不许被清、也不许被覆盖。
    /// 少清一个，新窗口就会隐形（截图模式）或只出一个组件（comp 夹具）；
    /// 多清一个 `PROFILE_DIR`，两窗口的 `prefs.json` / 会话库就不再共享。
    #[test]
    fn the_strip_list_covers_every_fixture_environment_variable() {
        assert_eq!(
            STRIP_ENV,
            &["WINDTERM_SCREENSHOT_TO", "WINDTERM_COMP_DEMO"],
            "夹具变量多一个少一个都要显式确认；配置态变量不许混进来"
        );
        for config_key in ["WINDTERM_PROFILE_DIR", "WINDTERM_LANG"] {
            assert!(
                !STRIP_ENV.contains(&config_key),
                "{config_key} 是共享范围 / 语言配置，清掉会违背用户裁定"
            );
            assert!(
                !ROLE_ENV.iter().any(|(k, _)| *k == config_key),
                "{config_key} 只能继承，不许被 env_set 覆盖"
            );
        }
        assert_eq!(
            ROLE_ENV,
            &[("WINDTERM_SECONDARY_WINDOW", "1")],
            "角色变量是唯一的 env_set"
        );
    }

    /// 计划本身：**绝不调 `launch()`**（会弹窗、抢前台、CI 挂）。
    /// 测试进程里 `current_exe()` 是 test binary，路径非空且合法。
    #[test]
    fn the_plan_has_no_argv_and_carries_its_role() {
        let plan = new_window_plan();
        assert!(!plan.exe.as_os_str().is_empty(), "要能拿到自己的可执行路径");
        assert!(plan.args.is_empty(), "新进程零 argv");
        assert_eq!(plan.env_remove, STRIP_ENV, "夹具环境变量一个都不许继承");
        assert!(
            plan.env_set
                .iter()
                .any(|(k, v)| *k == "WINDTERM_SECONDARY_WINDOW" && *v == "1"),
            "角色变量要打上，construct.rs 据此跳过 restore_layout"
        );

        // 失败文案能读懂，且两类原因分得开（不许静默失败）。
        let spawn_err = NewWindowError::Spawn(io::Error::other("denied"));
        assert!(spawn_err.message().contains("denied"));
        let exe_err = NewWindowError::CurrentExe(io::Error::other("gone"));
        assert!(exe_err.message().contains("gone"));
    }
}

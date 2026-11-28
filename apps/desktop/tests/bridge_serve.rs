//! `api` / `mcp` 的**二进制级**验收：真进程、无窗口、真 UDS 客户端。
//!
//! 前台控制端点只在 GUI 启动路径里起，所以本文件不试图在测试里拉起窗口
//! （那会触发 GUI 全量构建与抢前台）。它钉住的是这条边界的另一半：
//! **前台没运行时，薄客户端必须明确报错退出，绝不自建会话池**。
//!
//! 每个用例把 `HOME` 指向独立临时目录，因此固定路径 `~/.zterm/control.sock`
//! 一定不存在，也不会碰到开发者真窗口的端点。
//! 真 UDS 上的一问一答由 `apps/desktop/src/bridge/mod_tests.rs` 用真 socket
//! 在进程内覆盖（`0600` / peercred / 首条普通方法 / 锁屏静默关）。

use std::path::PathBuf;
use std::process::{Command, Output, Stdio};

/// 一条独立 HOME 下的无窗口命令。
fn run_in_empty_home(args: &[&str]) -> (Output, PathBuf) {
    static SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let home = std::env::temp_dir().join(format!(
        "zterm-uds-client-{}-{}",
        std::process::id(),
        SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    ));
    std::fs::create_dir_all(&home).expect("临时 HOME");
    let out = Command::new(env!("CARGO_BIN_EXE_term-desk"))
        .args(args)
        .env("HOME", &home)
        .stdin(Stdio::null())
        .output()
        .expect("拉起 term-desk");
    (out, home)
}

fn stderr(out: &Output) -> String {
    String::from_utf8_lossy(&out.stderr).to_string()
}

#[test]
fn api_reports_a_missing_front_end_instead_of_starting_a_pool() {
    let (out, home) = run_in_empty_home(&["api", "describe"]);
    assert_eq!(out.status.code(), Some(1), "前台不在必须非零退出：{out:?}");
    let text = stderr(&out);
    assert!(text.contains("前台未运行"), "诊断要说清原因：{text}");
    assert!(text.contains("control.sock"), "诊断要点名端点路径：{text}");
    assert!(
        !home.join(".zterm/control.sock").exists(),
        "客户端绝不能自己造一个端点"
    );
}

#[test]
fn mcp_reports_a_missing_front_end_before_speaking_mcp() {
    let (out, _home) = run_in_empty_home(&["mcp"]);
    assert_eq!(out.status.code(), Some(1), "stdio server 不能没有前台就开张：{out:?}");
    let text = stderr(&out);
    assert!(text.contains("前台未运行"), "诊断要说清原因：{text}");
    assert!(
        String::from_utf8_lossy(&out.stdout).is_empty(),
        "没连上前不许输出任何 MCP 消息"
    );
}

#[test]
fn the_thin_client_help_paths_never_touch_the_socket() {
    for sub in ["api", "mcp"] {
        let (out, _home) = run_in_empty_home(&[sub, "--help"]);
        assert_eq!(out.status.code(), Some(0), "{sub} --help 应成功：{out:?}");
        assert!(
            !stderr(&out).contains("前台未运行"),
            "help 不该去连 socket：{}",
            stderr(&out)
        );
    }
}

#[test]
fn serve_script_run_and_headless_exec_are_gone() {
    for args in [
        vec!["serve"],
        vec!["script", "run", "-"],
        vec!["--headless", "--exec", "echo hi"],
    ] {
        let (out, _home) = run_in_empty_home(&args);
        assert_eq!(out.status.code(), Some(2), "{args:?} 应作为用法错误退出：{out:?}");
    }
}

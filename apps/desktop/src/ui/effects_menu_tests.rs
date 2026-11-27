//! `Effect` 落地里**带命令 ID 入口**的那几条判据。
//!
//! # 为什么单独成文件
//!
//! 判据是 `grep '"Xxx.Yyy"' apps/desktop/src/ui` **一处都不许有**，
//! 它只放过被 `#[cfg(test)] #[path = …] mod …;` 两行声明拉进来的纯测试模块
//! （`effects.rs` 里内联的 `mod tests` 不在那条白名单里——判据不靠文件名后缀，
//! 而是回到声明处，否则随便起个 `_tests.rs` 就能藏 ID）。
//!
//! 而这里的判据**必须**走 `dispatch("…")` 字符串入口：直接调 `apply_effect`
//! 只能证明落地那两行对，证明不了「菜单点下去能走到它」——上一版 `Word Wrap`
//! 整套接线做完、点了屏幕逐字节不变，缺的正是这条端到端判据。
//!
//! # 与 `effects.rs::tests` 的分工
//!
//! 那边留着的是纯函数判据（`next_active_tab`），不需要窗口、也碰不到命令 ID。

use crate::ui::lifecycle_test_support::{fake_tab, window_with, CloseCount};
use crate::ui::TerminalWindow;
use eframe::egui;

/// 一条标签的真窗口（菜单命令要活动标签才走得通）。
fn one_tab_window(ctx: &egui::Context) -> TerminalWindow {
    let pool = std::sync::Arc::new(term_session::SessionPool::new());
    let closes = CloseCount::default();
    let tab = fake_tab(&pool, ctx, 0, &closes);
    window_with(pool, ctx, vec![tab])
}

/// `Terminal.ManageTriggers` 要打开**独立的 Triggers 对话框**。
///
/// 2026-11-15 改口径：原先这条断言「打开 Preferences 并停在 Triggers 页」。
/// 但 Triggers / OneKeys 两页早就按用户裁定移出了 Preferences 左树，于是
/// 那条路径弹出的是一个翻不到那一页的设置窗口——用户看到的就是「点了弹错
/// 窗口」。兼容终端是自有窗口（左列表 + `Cancel / Ok`），所以现在直接置
/// `show_triggers`，连 Preferences 都不碰。
#[test]
fn manage_triggers_opens_the_standalone_dialog() {
    let ctx = egui::Context::default();
    let mut win = one_tab_window(&ctx);
    assert!(!win.view.show_settings, "出厂不该已经弹着设置");
    assert!(!win.view.show_triggers, "出厂不该已经弹着 Triggers");

    win.dispatch("Terminal.ManageTriggers");

    assert!(win.view.show_triggers, "Triggers 对话框必须打开");
    assert!(
        !win.view.show_settings,
        "不该顺带把 Preferences 也打开——那是旧路径"
    );
}

/// `Tool ▸ Script` 三条要各自落到对应的脚本表面，而不是打开面板。
///
/// 与本文件已有的 `ManageTriggers` 判据同形：必须走 `dispatch("…")` 字符串入口，
/// 才能证明「菜单点下去 → Action → Effect → 落地」整条链通。三条刻意都是
/// **脚本工作入口**：面板显隐由 `Window.TogglePaneScript` 单独管，不在这里带动。
#[test]
fn tool_script_menu_opens_the_three_script_surfaces() {
    let ctx = egui::Context::default();
    let mut win = one_tab_window(&ctx);
    assert!(win.script.is_none());
    assert!(win.script_editor.is_none());
    assert!(win.script_manager.is_none());

    win.dispatch("Terminal.RunScript");
    assert!(win.script.is_some(), "Run Script 要打开运行脚本窗口");
    win.script = None;

    win.dispatch("Terminal.NewScript");
    assert!(win.script_editor.is_some(), "New Script 要打开编辑器");
    win.script_editor = None;

    win.dispatch("Terminal.ManageScripts");
    assert!(win.script_manager.is_some(), "Manage Scripts 要打开管理对话框");
}

/// `Terminal.StopScript` 与 `Terminal.RecordScript` 两条的端到端判据。
///
/// Stop 不新写遍历：先在面板里塞一条真在跑的脚本，走 `dispatch` 后断言它
/// 已被取消（与 `Mode ▸ Local Mode` 共用 `cancel_all_script_runs`）。录制开关
/// 连点两次应回到未录状态——菜单勾读的就是这个槽位，不是另一份镜像状态。
#[test]
fn tool_script_menu_stop_cancels_and_record_toggles() {
    use crate::script_run::{RunOptions, ScriptRun, ScriptRunRegistry};

    let ctx = egui::Context::default();
    let pool = std::sync::Arc::new(term_session::SessionPool::new());
    let session = pool.open_exec("cat", 80, 24).expect("起不了 `cat`");
    let registry = std::sync::Arc::new(ScriptRunRegistry::new());
    let options = RunOptions::default().with_registry(std::sync::Arc::clone(&registry), "menu-stop.rhai");
    let run = ScriptRun::start(
        std::sync::Arc::clone(&pool),
        session,
        "sleep(80); 1".to_string(),
        options,
    )
    .expect("起脚本");
    let mut win = crate::ui::lifecycle_test_support::window_with(pool.clone(), &ctx, Vec::new());
    win.script_panel.current_mut().run = Some(run);

    win.dispatch("Terminal.StopScript");
    assert!(
        win.script_panel
            .current()
            .run
            .as_ref()
            .is_some_and(|run| run.is_cancelled()),
        "菜单 Stop 必须叫停面板里在跑的脚本"
    );

    assert!(!win.is_recording());
    win.dispatch("Terminal.RecordScript");
    assert!(win.is_recording(), "第一次点开始录制");
    win.dispatch("Terminal.RecordScript");
    assert!(!win.is_recording(), "第二次点结束录制，空录制不留文件");

    // 等 Outcome 到达再收尾：句柄留着是设计，但别把租约与线程丢出用例。
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    loop {
        win.poll_script_panel();
        if win.script_panel.current().run.is_none() {
            break;
        }
        assert!(std::time::Instant::now() < deadline, "取消后 10 秒没 Outcome");
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    pool.close(session);
}

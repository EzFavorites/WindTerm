//! `Effect` 落地里**带命令 ID 入口**的那几条判据。
//!
//! # 为什么单独成文件
//!
//! 4e（I5）的判据是 `grep '"Xxx.Yyy"' apps/desktop/src/ui` **一处都不许有**，
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

//! 面板「停止」的接线测试。
//!
//! 钉的是整条链：按 Stop → `run.cancel()`（句柄留着）→ 引擎报
//! `Outcome::Failed(取消)` → 句柄置空 → 登记簿租约释放。少任何一环，
//! 用户都会看到「点了停止却还在跑」或「停止之后同会话再也起不来脚本」。

use super::*;
use crate::script_run::{RunOptions, ScriptRunRegistry};
use crate::ui::lifecycle_test_support::window_with;
use std::sync::Arc;

fn session_and_pool() -> (Arc<term_session::SessionPool>, SessionId) {
    let pool = Arc::new(term_session::SessionPool::new());
    let session = pool.open_exec("cat", 80, 24).expect("起不了 `cat`");
    (pool, session)
}

fn wait_outcome(win: &mut TerminalWindow) -> Outcome {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    loop {
        win.poll_script_panel();
        if let Some(outcome) = win.script_panel.current().outcome.clone() {
            return outcome;
        }
        assert!(std::time::Instant::now() < deadline, "取消后 10 秒没 Outcome");
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
}

#[test]
fn panel_stop_cancels_then_frees_the_lease_when_the_outcome_arrives() {
    let (pool, session) = session_and_pool();
    let registry = Arc::new(ScriptRunRegistry::new());
    let options = RunOptions::default().with_registry(Arc::clone(&registry), "panel-stop.rhai");
    let run = ScriptRun::start(
        Arc::clone(&pool),
        session,
        "sleep(80); 1".to_string(),
        options,
    )
    .expect("起脚本");
    let ctx = egui::Context::default();
    let mut win = window_with(Arc::clone(&pool), &ctx, Vec::new());
    win.script_panel.current_mut().run = Some(run);
    assert_eq!(registry.holder(session).as_deref(), Some("panel-stop.rhai"));

    // 真按一次面板的 Stop：走 `apply_script_cmd`，不是直接调 `cancel()`。
    let _ = ctx.run(egui::RawInput::default(), |ctx| {
        egui::CentralPanel::default().show(ctx, |ui| {
            win.apply_script_cmd(ui, ScriptCmd::Stop, &[], &[]);
        });
    });
    assert!(
        win.script_panel.current().run.is_some(),
        "Stop 只叫停，句柄要留到 Outcome"
    );
    assert!(
        win.script_panel
            .current()
            .run
            .as_ref()
            .is_some_and(|run| run.is_cancelled())
    );
    assert_eq!(
        registry.holder(session).as_deref(),
        Some("panel-stop.rhai"),
        "句柄还在，租约就不能放"
    );

    match wait_outcome(&mut win) {
        Outcome::Failed { message, .. } => {
            assert!(message.contains("取消"), "要说清是被取消的：{message}");
        }
        other => panic!("取消应当收场成 Failed：{other:?}"),
    }
    assert!(win.script_panel.current().run.is_none(), "Outcome 到了才置空");
    assert_eq!(registry.holder(session), None, "句柄置空即释放租约");
    pool.close(session);
}

#[test]
fn closing_a_running_tab_cancels_first_and_removes_it_only_after_the_outcome() {
    let (pool, session) = session_and_pool();
    let registry = Arc::new(ScriptRunRegistry::new());
    let options = RunOptions::default().with_registry(Arc::clone(&registry), "tab-close.rhai");
    let run = ScriptRun::start(
        Arc::clone(&pool),
        session,
        "sleep(80); 1".to_string(),
        options,
    )
    .expect("起脚本");
    let ctx = egui::Context::default();
    let mut win = window_with(Arc::clone(&pool), &ctx, Vec::new());
    win.script_panel.add();
    win.script_panel.select(1);
    win.script_panel.current_mut().run = Some(run);

    win.script_panel.close_active();
    assert_eq!(win.script_panel.tabs.len(), 2, "不能边跑边把页签摘掉");
    assert!(win.script_panel.tabs[1].closing);
    assert!(
        win.script_panel.tabs[1]
            .state
            .run
            .as_ref()
            .is_some_and(|run| run.is_cancelled())
    );
    assert_eq!(registry.holder(session).as_deref(), Some("tab-close.rhai"));

    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    while win.script_panel.tabs.len() > 1 {
        win.poll_script_panel();
        assert!(std::time::Instant::now() < deadline, "页签一直没关掉");
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    assert_eq!(registry.holder(session), None, "页签真摘掉时租约也放了");
    pool.close(session);
}

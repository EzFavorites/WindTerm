//! `prefs_rev` 不再死状态的判据（由 `#[path]` 挂回 `app_state.rs`）。
//!
//! 审查发现：偏好写了很多次，`AppSnapshot.prefs_rev` 永远是 0，脚本靠它
//! 判断「要不要重取偏好」永远以为没变过。这里走真实 `prefs::save` +
//! 窗口每帧同步那一步，验版本号真的推进。

use crate::ui::lifecycle_test_support::{fake_tab, window_with, with_temp_profile, CloseCount};
use crate::ui::TerminalWindow;
use eframe::egui;

#[test]
fn prefs_rev_advances_after_a_successful_write() {
    with_temp_profile(|| {
        let ctx = egui::Context::default();
        let pool = std::sync::Arc::new(term_session::SessionPool::new());
        let closes = CloseCount::default();
        let tab = fake_tab(&pool, &ctx, 0, &closes);
        let mut win: TerminalWindow = window_with(pool, &ctx, vec![tab]);

        let before = win.app_snapshot().prefs_rev;
        let generation = term_profile::prefs::write_generation();

        // 一次真实成功写盘（不是手工给字段 +1）。
        let mut prefs = term_profile::prefs::Prefs::default();
        prefs.language = "zh-CN".to_string();
        assert!(term_profile::prefs::save(&prefs));
        assert_eq!(
            term_profile::prefs::write_generation(),
            generation + 1,
            "写口必须推进 generation"
        );

        // 帧首同步之后，快照里的版本号才跟着走——与界面版本 rev 分开计。
        assert_eq!(win.app_snapshot().prefs_rev, before, "没同步前不该偷跑");
        win.sync_prefs_rev(&ctx);
        assert_eq!(
            win.app_snapshot().prefs_rev,
            generation + 1,
            "偏好版本与界面版本分开计：rev 仍是 0"
        );
        assert_eq!(win.app_snapshot().rev, 0);

        // 幂等：再同步一次不加数字，只有真写盘才推进。
        win.sync_prefs_rev(&ctx);
        assert_eq!(win.app_snapshot().prefs_rev, generation + 1);
    });
}

/// 冲突失败不能再静默丢掉：写盘失败原因由每帧的 sync_prefs_rev 取走，
/// 用 `SaveError::message()` 上屏到 `log_note`，取走后清空。
#[test]
fn a_conflicting_prefs_save_reaches_the_status_bar_note() {
    with_temp_profile(|| {
        let ctx = egui::Context::default();
        let pool = std::sync::Arc::new(term_session::SessionPool::new());
        let closes = CloseCount::default();
        let tab = fake_tab(&pool, &ctx, 0, &closes);
        let mut win: TerminalWindow = window_with(pool, &ctx, vec![tab]);
        let _ = crate::ui::dialogs::shortcuts::take_startup_migration_notice();

        let mut prefs = term_profile::prefs::Prefs::default();
        prefs.language = "en".to_string();
        assert!(term_profile::prefs::save(&prefs));
        assert_eq!(term_profile::prefs::last_save_error(), None);

        // 另一个进程 / 编辑器改了盘上内容，而窗口手里那份快照已经过期。
        let path = term_paths::profile_dir()
            .expect("临时 profile 目录")
            .join(term_profile::prefs::PREFS_FILE);
        std::fs::write(&path, r#"{"language":"zh-CN"}"#).unwrap();
        prefs.font_size = 20.0;
        assert!(!term_profile::prefs::save(&prefs));
        assert_eq!(
            term_profile::prefs::last_save_error(),
            Some(term_profile::prefs::SaveError::Conflict)
        );

        win.sync_prefs_rev(&ctx);
        assert!(
            win.log_note
                .as_deref()
                .is_some_and(|n| n.contains("请重新打开设置页")),
            "冲突必须用 SaveError::message() 上屏：{:?}",
            win.log_note
        );
        assert_eq!(term_profile::prefs::last_save_error(), None, "取走后必须清空");

        // 下一帧不再重复弹同一条提示。
        win.log_note = None;
        win.sync_prefs_rev(&ctx);
        assert!(win.log_note.is_none());
    });
}

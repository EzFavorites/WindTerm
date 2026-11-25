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

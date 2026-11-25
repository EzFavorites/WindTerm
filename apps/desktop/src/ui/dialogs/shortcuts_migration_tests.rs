//! 快捷键写口迁移 / Reset 双清的往返判据（由 `#[path]` 挂回 `shortcuts.rs`）。
//!
//! 配置批次提供了 `normalized_prefs_fields` / `reset_prefs_fields` /
//! `Prefs::reset_shortcut_bindings`；这里验证 UI 外壳批次真的把它们接上了，
//! 而不是自己抄一份迁移规则——两份规则一分叉，老文件里残留的
//! `shortcut_overrides` 就会在下次读盘时把旧绑定复活。

use super::*;
use crate::ui::lifecycle_test_support::{window_with, with_temp_profile};

/// 写盘时的数据层迁移必须真的接上：新字段落用户绑定，旧下标字段清空。
#[test]
fn persisting_shortcuts_clears_the_legacy_index_field() {
    with_temp_profile(|| {
        // 老文件：只有按数组下标的覆盖（下标 3 在老表里指向 Find）。
        let mut old = term_profile::prefs::Prefs::default();
        old.shortcut_overrides = vec![(3, "Cmd+K".into())];
        assert!(term_profile::prefs::save(&old));

        let ctx = egui::Context::default();
        let pool = std::sync::Arc::new(term_session::SessionPool::new());
        let mut win = window_with(pool, &ctx, Vec::new());
        let new = vec![("Text.Find".to_string(), "Cmd+Shift+F".to_string())];
        win.shortcuts = KeyBindingMap::new(&new, Platform::Macos);
        win.persist_shortcuts();

        let saved = term_profile::prefs::load().expect("必须真的写盘");
        assert_eq!(saved.shortcut_bindings, new);
        assert!(
            saved.shortcut_overrides.is_empty(),
            "旧下标字段没清：重启后迁移会重新点亮老绑定"
        );
    });
}

/// Reset 必须同时清两份字段：只清新字段时下次读盘迁移会把旧下标复活。
#[test]
fn reset_clears_both_shortcut_fields() {
    with_temp_profile(|| {
        let mut prefs = term_profile::prefs::Prefs::default();
        prefs.shortcut_bindings = vec![("Text.Find".into(), "Cmd+F".into())];
        prefs.shortcut_overrides = vec![(3, "Cmd+K".into())];
        assert!(term_profile::prefs::save(&prefs));

        let ctx = egui::Context::default();
        let pool = std::sync::Arc::new(term_session::SessionPool::new());
        let mut win = window_with(pool, &ctx, Vec::new());
        win.shortcuts = KeyBindingMap::new(&[], Platform::Macos);
        win.reset_persisted_shortcuts();

        let saved = term_profile::prefs::load().expect("必须真的写盘");
        assert!(saved.shortcut_bindings.is_empty());
        assert!(saved.shortcut_overrides.is_empty());
    });
}

/// 启动期迁移的两类丢弃项必须汇总到提示，不能只躺在测试里。
#[test]
fn startup_migration_reports_dropped_and_rejected_bindings() {
    with_temp_profile(|| {
        let _ = take_startup_migration_notice();

        // ① 越界旧下标（新字段为空时走迁移）。
        let mut old = term_profile::prefs::Prefs::default();
        old.shortcut_overrides = vec![(999, "Cmd+Q".into()), (1, "Cmd+F".into())];
        let (_map, _seq) = build_runtime(&old);
        let note = take_startup_migration_notice().expect("越界项必须汇总到启动提示");
        assert!(note.contains("#999"), "{note}");
        assert!(!note.contains("#1"), "合法下标的绑定不该报丢弃：{note}");
        assert!(note.contains("已清理"), "提示要说明已经清理：{note}");
        assert_eq!(take_startup_migration_notice(), None, "提示只上屏一次");

        // ② 冲突的用户绑定被 KeyBindingMap 回滚，也要一并报出来。
        let mut fresh = term_profile::prefs::Prefs::default();
        fresh.shortcut_bindings = vec![
            ("Text.Find".into(), "Cmd+F".into()),
            ("Text.FindNext".into(), "Cmd+F".into()),
        ];
        let (_map, _seq) = build_runtime(&fresh);
        let note = take_startup_migration_notice().expect("冲突项必须汇总到启动提示");
        assert!(note.contains("Text.FindNext"), "{note}");
        assert_eq!(take_startup_migration_notice(), None);

        // 清理写回后，旧下标字段不应残留。
        let saved = term_profile::prefs::load().expect("清理必须写盘");
        assert!(saved.shortcut_overrides.is_empty());
        assert_eq!(
            saved.shortcut_bindings,
            vec![("Text.Find".into(), "Cmd+F".into())]
        );
    });
}

/// 被回滚的绑定必须在启动时剔除并写回：重启后既不重报提示，也不再参与冲突。
#[test]
fn rejected_bindings_are_cleaned_on_startup_and_not_reported_again() {
    with_temp_profile(|| {
        let _ = take_startup_migration_notice();

        let mut prefs = term_profile::prefs::Prefs::default();
        prefs.shortcut_bindings = vec![
            ("Text.Find".into(), "Cmd+Shift+F".into()),
            ("Text.Paste".into(), "Cmd+Shift+F".into()),
            ("Window.WindowList".into(), "Cmd+L".into()),
        ];
        assert!(term_profile::prefs::save(&prefs));

        let (map, _seq) = build_runtime(&prefs);
        let note = take_startup_migration_notice().expect("首次启动必须报告清理");
        assert!(note.contains("已清理"), "{note}");
        assert!(note.contains("Text.Paste"), "{note}");
        assert_eq!(map.rejected_user_bindings().len(), 1);

        let saved = term_profile::prefs::load().expect("清理结果必须落盘");
        assert_eq!(
            saved.shortcut_bindings,
            vec![
                ("Text.Find".into(), "Cmd+Shift+F".into()),
                ("Window.WindowList".into(), "Cmd+L".into()),
            ],
            "冲突项必须从 prefs 删除，而不是只在内存里回滚"
        );

        // 重启：重新读盘再构造一次，提示槽必须保持为空。
        let reloaded = term_profile::prefs::load().expect("重启读盘");
        let (map2, _seq2) = build_runtime(&reloaded);
        assert!(map2.rejected_user_bindings().is_empty());
        assert_eq!(
            take_startup_migration_notice(),
            None,
            "已清理的绑定不得在下次重启重报"
        );
    });
}

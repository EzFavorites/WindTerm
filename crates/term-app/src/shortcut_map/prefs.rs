//! prefs 两份字段的收敛规则（与 [`super`] 的映射层同源，API 仍在 `shortcut_map` 下）。
//!
//! 这里只放「写盘前怎么把新字段 / 旧下标字段收敛成一份事实」的三条规则：
//! 启动清理与重绑写口共用同一份口径，放在映射层文件里只是为了靠近迁移入口。
//! 拆出来给 `shortcut_map.rs` 腾 4j 余量；路径不变，调用方仍写
//! `term_app::shortcut_map::{normalized_prefs_fields, reset_prefs_fields, drop_rejected_bindings}`。

use super::*;

/// 写盘前把两份字段收敛到「新字段 + 旧字段为空」的形态。
///
/// 与 [`migrate_legacy_with_report`] 同一套规则，但以整个 `prefs` 的两份输入为
/// 参数，方便调用方直接在 `save_prefs` 的闭包里写回两个字段。
pub fn normalized_prefs_fields(
    bindings: &[(String, String)],
    overrides: &[(usize, String)],
) -> (Vec<(String, String)>, Vec<(usize, String)>) {
    let migration = migrate_legacy_with_report(overrides, bindings);
    (migration.bindings, migration.overrides)
}

/// Reset 必须同时清两份字段：只清新字段时，下次读盘迁移会把旧下标重新点亮。
pub fn reset_prefs_fields() -> (Vec<(String, String)>, Vec<(usize, String)>) {
    (Vec::new(), Vec::new())
}

/// 写盘前把被回滚的冲突绑定从用户绑定里剔除。
///
/// [`KeyBindingMap::new`] 只把冲突项记进 `rejected_user_bindings`：调用方若照原样
/// 写回 prefs，下次重启会再次冲突、再报一次提示，而那条绑定永远不生效。剔除
/// 规则放在数据层，UI 的启动时清理与重绑写口共用同一份口径。
pub fn drop_rejected_bindings(
    bindings: &[(String, String)],
    rejected: &[(String, String)],
) -> Vec<(String, String)> {
    bindings
        .iter()
        .filter(|binding| {
            !rejected
                .iter()
                .any(|r| r.0 == binding.0 && r.1 == binding.1)
        })
        .cloned()
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 模拟「迁移写盘 → Reset → 重启读盘」的完整一圈：Resets 不清旧字段时，
    /// 第二次读盘会按下标重新点亮绑定，用户看到的是「Reset 后又复活」。
    #[test]
    fn migrated_bindings_do_not_come_back_after_reset_and_restart() {
        let legacy = vec![(11usize, "Cmd+L".to_string())];
        let first = migrate_legacy_with_report(&legacy, &[]);
        assert_eq!(
            first.bindings,
            vec![("Window.WindowList".to_string(), "Cmd+L".to_string())]
        );
        // 写盘：新字段写结果、旧字段清空。
        let (bindings, overrides) = (first.bindings, first.overrides);
        assert!(overrides.is_empty());
        assert!(!bindings.is_empty());

        // Reset：两份字段一起清。
        let (bindings, overrides) = reset_prefs_fields();
        assert!(bindings.is_empty() && overrides.is_empty());

        // 重启：simulate load-modify-save 的收敛，仍然全空。
        let (restart_bindings, restart_overrides) = normalized_prefs_fields(&bindings, &overrides);
        assert!(restart_bindings.is_empty(), "Reset 后不许复活");
        assert!(restart_overrides.is_empty());
        assert_eq!(
            KeyBindingMap::new(&restart_bindings, Platform::Macos).display("Window.WindowList"),
            default_display("Window.WindowList"),
            "Reset 后应回到出厂和弦，不是旧下标指向的重绑和弦"
        );
    }

    #[test]
    fn dropped_rejected_bindings_do_not_conflict_again_on_restart() {
        let user = vec![
            ("Text.Find".to_string(), "Cmd+Shift+F".to_string()),
            ("Text.Paste".to_string(), "Cmd+Shift+F".to_string()),
            ("Window.WindowList".to_string(), "Cmd+L".to_string()),
        ];
        let map = KeyBindingMap::new(&user, Platform::Macos);
        let kept = drop_rejected_bindings(&user, map.rejected_user_bindings());
        assert_eq!(kept, vec![user[0].clone(), user[2].clone()]);
        let restarted = KeyBindingMap::new(&kept, Platform::Macos);
        assert!(restarted.rejected_user_bindings().is_empty());
    }
}

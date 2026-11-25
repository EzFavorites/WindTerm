//! 把原版 `user.config` 的偏好增量并进我们的 `Prefs`（B1 的落库纯函数）。
//!
//! 边界：这个函数**不读文件、不写文件**。它只回答「给定一份增量和一份当前偏好，
//! 新的偏好是什么、改了哪几项」。写盘仍走 [`crate::prefs::save`]——坏文件另存、
//! 存不下就拒写那套保全逻辑全仓只有一份，导入不另起第二条写路径。
//!
//! 三条规矩：
//! - 只覆盖 `Some` 字段：`None`（原文件没有 / 值类型不对）不碰 `base` 的现值；
//! - 语义反转只做两处：`askBeforeCloseWindow` 取反、`columnMarkerVisible=false`
//!   落到 `view_column_marker=0`；`askBeforeCloseTab` 粒度不同（我们的确认只在
//!   关多标签时发生），**不映射**，连字段都不进 delta；
//! - 导入是一次显式动作：这里不触发连接、不改主题、不动会话树。

use crate::prefs::Prefs;
use term_import::user_config::UserConfigDelta;

/// 一项实际发生的改动。`key` 用我们 `Prefs` 的字段名（不是原版键名），
/// 这样差异展示 / 单测都只看得见落点，不会被原版的键名口径带偏。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Change {
    pub key: &'static str,
    pub from: String,
    pub to: String,
}

/// 把 `delta` 覆盖到 `base` 上，返回新偏好和逐项差异。
///
/// 值相同不记 `Change`（避免导入一次就在界面上刷出九个「改动」）。完全相同
/// 的增量可以重复应用，结果幂等。
pub fn apply_user_config(delta: &UserConfigDelta, base: &Prefs) -> (Prefs, Vec<Change>) {
    let mut next = base.clone();
    let mut changes = Vec::new();

    macro_rules! overwrite {
        ($field:ident, $value:expr) => {{
            if let Some(v) = $value {
                if next.$field != v {
                    changes.push(Change {
                        key: stringify!($field),
                        from: next.$field.to_string(),
                        to: v.to_string(),
                    });
                    next.$field = v;
                }
            }
        }};
    }

    overwrite!(language, delta.language.clone());
    overwrite!(lock_timeout_secs, delta.lock_timeout_secs);
    overwrite!(quickbar_alphabetical, delta.quickbar_alphabetical);
    overwrite!(suppress_close_window, delta.suppress_close_window);
    overwrite!(view_column_marker, delta.view_column_marker);
    overwrite!(view_line_endings, delta.view_line_endings);
    overwrite!(view_indent_guides, delta.view_indent_guides);
    overwrite!(wrap_mode, delta.wrap_mode);
    overwrite!(view_wrap_symbols, delta.view_wrap_symbols);

    (next, changes)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::prefs::{self, PREFS_FILE};
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicUsize, Ordering};
    use term_import::user_config::parse_user_config;

    static COUNTER: AtomicUsize = AtomicUsize::new(0);

    fn tmp() -> PathBuf {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        std::env::temp_dir().join(format!(
            "windterm-prefs-import-{}-{}-{}",
            std::process::id(),
            nanos,
            COUNTER.fetch_add(1, Ordering::Relaxed)
        ))
    }

    fn full_delta() -> UserConfigDelta {
        parse_user_config(
            r#"{
              "application.language":"zh-CN",
              "application.lockScreenTimeout":7,
              "quickbar.showCommandsInOrder":true,
              "terminal.askBeforeCloseWindow":false,
              "terminal.text.columnMarkerVisible":false,
              "terminal.text.eolVisible":true,
              "terminal.text.indentGuideVisible":true,
              "terminal.text.wrapMode":1,
              "terminal.text.wrapSymbolVisible":false,
              "terminal.askBeforeCloseTab":true,
              "application.fingerprint":"SENTINEL",
              "application.masterPassword":"SENTINEL"
            }"#,
        )
    }

    #[test]
    fn only_some_fields_are_overwritten() {
        let base = Prefs {
            language: "en".to_string(),
            quickbar_alphabetical: true,
            theme_name: "dige-black".to_string(),
            ..Prefs::default()
        };
        // delta 不携带 theme_name / quickbar_show_group_title：它们必须原样留下。
        let delta = parse_user_config(r#"{"application.language":"zh-CN"}"#);
        let (next, changes) = apply_user_config(&delta, &base);
        assert_eq!(next.language, "zh-CN");
        assert_eq!(next.theme_name, "dige-black");
        assert!(next.quickbar_alphabetical);
        assert_eq!(changes, vec![Change {
            key: "language",
            from: "en".to_string(),
            to: "zh-CN".to_string(),
        }]);
    }

    #[test]
    fn all_nine_mappings_land_in_the_right_fields() {
        // 基准把九项都设成与 delta 相反的值，于是九项都算「真改动」，
        // 逐条可核对；值本来就相同时不记 Change（见下一条幂等断言）。
        let opposite = Prefs {
            language: "en".to_string(),
            lock_timeout_secs: 1800,
            quickbar_alphabetical: true,
            suppress_close_window: false,
            view_column_marker: 80,
            view_line_endings: false,
            view_indent_guides: false,
            wrap_mode: 0,
            view_wrap_symbols: true,
            ..Prefs::default()
        };
        let (next, changes) = apply_user_config(&full_delta(), &opposite);
        assert_eq!(next.language, "zh-CN");
        assert_eq!(next.lock_timeout_secs, 420);
        assert!(!next.quickbar_alphabetical);
        assert!(next.suppress_close_window);
        assert_eq!(next.view_column_marker, 0);
        assert!(next.view_line_endings);
        assert!(next.view_indent_guides);
        assert_eq!(next.wrap_mode, 1);
        assert!(!next.view_wrap_symbols);
        assert_eq!(changes.len(), 9, "九项改动逐条可核对：{changes:?}");
        // 相同增量再应用一次不该刷出第二遍改动。
        let (again, changes2) = apply_user_config(&full_delta(), &next);
        assert_eq!(again, next);
        assert!(changes2.is_empty());
    }

    #[test]
    fn ask_before_close_tab_and_sentinels_have_no_landing_field() {
        // 这个 delta 里只有 askBeforeCloseTab=true（粒度不符）与两个哨兵；
        // 解析层就不给它们建字段，合并后什么都不该动。
        let delta = parse_user_config(
            r#"{"terminal.askBeforeCloseTab":true,
                "application.fingerprint":"SENTINEL",
                "application.masterPassword":"SENTINEL"}"#,
        );
        let (next, changes) = apply_user_config(&delta, &Prefs::default());
        assert_eq!(next, Prefs::default());
        assert!(changes.is_empty());
        assert!(!format!("{next:?}{changes:?}").contains("SENTINEL"));
    }

    #[test]
    fn applying_then_saving_reuses_the_broken_file_preservation() {
        // 端到端串一次「解析 → 合并 → 写盘」。写盘走 prefs::save_to_path（与
        // prefs::save 同一个 save_to），坏文件必须被逐字节另存，不许被出厂值顶掉。
        let path = tmp();
        let broken = "{\n  \"language\": \"zh-CN\"\n  \"font_size\": 18.0\n}\n";
        std::fs::write(&path, broken).expect("写坏文件");
        let load = |p: &Path| -> Option<Prefs> {
            std::fs::read_to_string(p).ok().and_then(|s| serde_json::from_str(&s).ok())
        };
        assert!(load(&path).is_none());

        let delta = parse_user_config(r#"{"application.language":"en"}"#);
        let base = load(&path).unwrap_or_default();
        let (next, _) = apply_user_config(&delta, &base);
        assert!(prefs::save_to_path(&path, &next));

        let name = path.file_name().unwrap().to_string_lossy().into_owned();
        let prefix = format!("{name}.bad-");
        let dir = path.parent().unwrap();
        let backups: Vec<PathBuf> = std::fs::read_dir(dir)
            .unwrap()
            .flatten()
            .map(|e| e.path())
            .filter(|p| p.file_name().is_some_and(|n| n.to_string_lossy().starts_with(&prefix)))
            .collect();
        assert!(!backups.is_empty(), "坏文件必须留一份备份");
        assert_eq!(std::fs::read_to_string(&backups[0]).unwrap(), broken);
        let written: Prefs = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(written.language, "en");
        let _ = std::fs::remove_file(&path);
        for b in backups {
            let _ = std::fs::remove_file(b);
        }
    }

    #[test]
    fn save_is_refused_when_the_target_cannot_be_written() {
        // 拒写：父路径是个普通文件，保全与原子写都无处落。返回 false，
        // 调用方据此提示失败，而不是谎报「导入成功」。
        let parent = tmp();
        std::fs::write(&parent, "not a directory").expect("写占位文件");
        let path = parent.join(PREFS_FILE);
        assert!(!prefs::save_to_path(&path, &Prefs::default()));
        let _ = std::fs::remove_file(&parent);
    }
}

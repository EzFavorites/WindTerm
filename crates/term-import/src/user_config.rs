//! 原版 WindTerm `user.config` 的**偏好增量解析**（B1）。
//!
//! 原版是「出厂值在 `wind.config`，用户差异在 `user.config`」的双层模型；本机那份
//! `user.config` 实测 **13 个键**（资料库报告写成 10 个是漏数了 `fingerprint` /
//! `masterPassword` / `wrapMode`）。本文件把那 13 个键划成三类：
//!
//! | 类别 | 键 | 处置 |
//! |---|---|---|
//! | 可迁移（9） | `application.language`、`application.lockScreenTimeout`、`quickbar.showCommandsInOrder`、`terminal.askBeforeCloseWindow`、`terminal.text.{columnMarkerVisible,eolVisible,indentGuideVisible,wrapMode,wrapSymbolVisible}` | 进 [`UserConfigDelta`] 的 `Some` 字段 |
//! | 哨兵（2） | `application.fingerprint`、`application.masterPassword` | **连读都不读**；类型里没有这两个字段 |
//! | 粒度不符 / 无落点（2） | `terminal.askBeforeCloseTab`、`terminal.text.hiddenMargins` | 忽略；不为凑数接空转开关（R5） |
//!
//! 密码/设备指纹是产品底线：不迁移、不解密、不落进任何结构体字段。这也是
//! `UserConfigDelta` **没有** `fingerprint` / `masterPassword` 字段的原因——不是
//! 约定，是类型层面根本没有位置可写。
//!
//! 解析用 `serde_json::Value` 手动挑键（严格 JSON：注释 / 尾逗号一律不认），
//! 未知键直接忽略。坏文件或顶层不是对象时返回空增量（全部 `None`），
//! 调用方据此「什么也不改」——与 `prefs` 的「读不出来不等于可以覆盖」同一条底线。

use serde_json::Value;

/// 原版 `user.config` 的全部 13 个键，按词表顺序。
///
/// 这是**白名单**（文档 + 单测口径）：只在解析时按这里列出的键取值；表外的键
/// 一个都不认。哨兵两键列在里面只表示「这份格式确实有这个键」，解析器**不会**
/// 把它们写进 [`UserConfigDelta`]。
pub const USER_CONFIG_KEYS: [&str; 13] = [
    "application.fingerprint",
    "application.language",
    "application.lockScreenTimeout",
    "application.masterPassword",
    "quickbar.showCommandsInOrder",
    "terminal.askBeforeCloseTab",
    "terminal.askBeforeCloseWindow",
    "terminal.text.columnMarkerVisible",
    "terminal.text.eolVisible",
    "terminal.text.hiddenMargins",
    "terminal.text.indentGuideVisible",
    "terminal.text.wrapMode",
    "terminal.text.wrapSymbolVisible",
];

/// 列标记打开时要画哪一列。原版用户层只存 `columnMarkerVisible`，列号表在
/// `wind.config`（不导入），这里用终端惯例第 80 列，落进 `Prefs.view_column_marker`
/// 时仍会被消费侧夹进 `8..=512`。
pub const COLUMN_MARKER_ON: usize = 80;

/// `application.lockScreenTimeout` 的单位换算：原版全局默认 30（分钟），本项目
/// `Prefs.lock_timeout_secs` 默认 1800（= 30 分钟），两者按 60 对齐。0 仍是 0
/// （不自动锁）。
const LOCK_TIMEOUT_SCALE: u64 = 60;

/// 一份 `user.config` 里**可迁移**的偏好增量。字段全是 `Option`：
/// `None` = 原文件没这项 / 值类型不对，写入侧只覆盖 `Some`（见
/// `term_profile::prefs_import::apply_user_config`）。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct UserConfigDelta {
    pub language: Option<String>,
    pub lock_timeout_secs: Option<u32>,
    pub quickbar_alphabetical: Option<bool>,
    pub suppress_close_window: Option<bool>,
    pub view_column_marker: Option<usize>,
    pub view_line_endings: Option<bool>,
    pub view_indent_guides: Option<bool>,
    pub wrap_mode: Option<u8>,
    pub view_wrap_symbols: Option<bool>,
}

/// 严格解析 `user.config` 文本。注释 / 尾逗号 / 非对象顶层都按「读不出来」处理，
/// 返回空增量；白名单外的键忽略，不连坐同份配置里的其它键。
pub fn parse_user_config(text: &str) -> UserConfigDelta {
    let Ok(Value::Object(map)) = serde_json::from_str::<Value>(text) else {
        return UserConfigDelta::default();
    };
    let mut delta = UserConfigDelta::default();
    for (key, value) in &map {
        match key.as_str() {
            // 哨兵两键、askBeforeCloseTab（粒度不同）、hiddenMargins（无落点）
            // 故意没有分支：连 `value` 都不碰。
            "application.language" => {
                delta.language = string_of(value).filter(|s| s != "auto");
            }
            "application.lockScreenTimeout" => {
                delta.lock_timeout_secs = number_of(value).map(|n| {
                    u32::try_from(n.saturating_mul(LOCK_TIMEOUT_SCALE)).unwrap_or(u32::MAX)
                });
            }
            // 原版 showCommandsInOrder=true ⟺ 按配置顺序 ⟺ 不按字母序。
            "quickbar.showCommandsInOrder" => {
                delta.quickbar_alphabetical = bool_of(value).map(|b| !b);
            }
            // 原版「关窗口前先问」取反才是我们的「关窗口不再问」。
            "terminal.askBeforeCloseWindow" => {
                delta.suppress_close_window = bool_of(value).map(|b| !b);
            }
            // false → 0（不画线）；true → 惯例第 80 列。
            "terminal.text.columnMarkerVisible" => {
                delta.view_column_marker = bool_of(value).map(|b| if b { COLUMN_MARKER_ON } else { 0 });
            }
            "terminal.text.eolVisible" => {
                delta.view_line_endings = bool_of(value);
            }
            "terminal.text.indentGuideVisible" => {
                delta.view_indent_guides = bool_of(value);
            }
            "terminal.text.wrapMode" => {
                delta.wrap_mode = number_of(value).and_then(|n| u8::try_from(n).ok());
            }
            "terminal.text.wrapSymbolVisible" => {
                delta.view_wrap_symbols = bool_of(value);
            }
            _ => {}
        }
    }
    delta
}

fn string_of(value: &Value) -> Option<String> {
    match value {
        Value::String(s) if !s.is_empty() => Some(s.clone()),
        _ => None,
    }
}

fn bool_of(value: &Value) -> Option<bool> {
    value.as_bool()
}

fn number_of(value: &Value) -> Option<u64> {
    value.as_u64().or_else(|| value.as_i64().and_then(|n| u64::try_from(n).ok()))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 13 键全齐的合成样本。哨兵值用 `SENTINEL-*` 串，真实数据绝不进仓库。
    const ALL_THIRTEEN: &str = r#"{
      "application.fingerprint": "SENTINEL-FINGERPRINT-DO-NOT-MIGRATE",
      "application.language": "zh-CN",
      "application.lockScreenTimeout": 7,
      "application.masterPassword": "SENTINEL-MASTER-PASSWORD-DO-NOT-MIGRATE",
      "quickbar.showCommandsInOrder": true,
      "terminal.askBeforeCloseTab": false,
      "terminal.askBeforeCloseWindow": false,
      "terminal.text.columnMarkerVisible": false,
      "terminal.text.eolVisible": true,
      "terminal.text.hiddenMargins": [],
      "terminal.text.indentGuideVisible": true,
      "terminal.text.wrapMode": 1,
      "terminal.text.wrapSymbolVisible": false
    }"#;

    #[test]
    fn whitelist_is_exactly_the_thirteen_keys_from_the_original_file() {
        assert_eq!(USER_CONFIG_KEYS.len(), 13);
        assert_eq!(USER_CONFIG_KEYS[0], "application.fingerprint");
        assert_eq!(USER_CONFIG_KEYS[12], "terminal.text.wrapSymbolVisible");
        assert!(USER_CONFIG_KEYS.contains(&"application.masterPassword"));
        assert!(USER_CONFIG_KEYS.contains(&"terminal.text.wrapMode"));
    }

    #[test]
    fn all_thirteen_keys_parse_without_error_and_map_the_nine_fields() {
        let d = parse_user_config(ALL_THIRTEEN);
        assert_eq!(d.language.as_deref(), Some("zh-CN"));
        assert_eq!(d.lock_timeout_secs, Some(420)); // 7 分钟 → 420 秒
        assert_eq!(d.quickbar_alphabetical, Some(false)); // showInOrder=true → 不按字母序
        assert_eq!(d.suppress_close_window, Some(true)); // askBefore=false → 不再问
        assert_eq!(d.view_column_marker, Some(0)); // false → 不画线
        assert_eq!(d.view_line_endings, Some(true));
        assert_eq!(d.view_indent_guides, Some(true));
        assert_eq!(d.wrap_mode, Some(1));
        assert_eq!(d.view_wrap_symbols, Some(false));
    }

    #[test]
    fn sentinel_keys_are_never_defined_or_written_down() {
        let d = parse_user_config(ALL_THIRTEEN);
        let debug = format!("{d:?}");
        assert!(!debug.contains("SENTINEL-FINGERPRINT"));
        assert!(!debug.contains("SENTINEL-MASTER-PASSWORD"));
        assert!(!debug.contains("fingerprint"));
        assert!(!debug.contains("masterPassword"));
        // 粒度不同的 askBeforeCloseTab 也没有落点。
        assert!(!debug.contains("askBeforeCloseTab"));
    }

    #[test]
    fn unknown_keys_are_ignored_and_do_not_spoil_known_ones() {
        let text = r#"{"application.language":"en","application.unknownThing":1,
                        "totally.bogus":{"deep":true},"terminal.text.wrapMode":0}"#;
        let d = parse_user_config(text);
        assert_eq!(d.language.as_deref(), Some("en"));
        assert_eq!(d.wrap_mode, Some(0));
        assert_eq!(d.lock_timeout_secs, None);
    }

    #[test]
    fn strict_json_and_wrong_types_yield_no_change() {
        // 原版 wind.* 那种带 `//` 注释的宽松格式在这里不认。
        assert_eq!(parse_user_config("{ // comment\n \"application.language\":\"en\" }"), UserConfigDelta::default());
        assert_eq!(parse_user_config("[1,2,3]"), UserConfigDelta::default());
        assert_eq!(parse_user_config("{ broken"), UserConfigDelta::default());
        // 键认识但值类型不对：当作没这项，不把默认值写坏。
        let d = parse_user_config(
            r#"{"application.language":9,"application.lockScreenTimeout":"7",
                 "terminal.askBeforeCloseWindow":"false","terminal.text.wrapMode":999}"#,
        );
        assert_eq!(d, UserConfigDelta::default());
    }

    #[test]
    fn column_marker_true_uses_the_conventional_column() {
        let d = parse_user_config(r#"{"terminal.text.columnMarkerVisible":true}"#);
        assert_eq!(d.view_column_marker, Some(COLUMN_MARKER_ON));
    }

    #[test]
    fn language_auto_is_left_to_the_application_default() {
        let d = parse_user_config(r#"{"application.language":"auto"}"#);
        assert_eq!(d.language, None);
    }
}

//! 行级过滤器（类 grep）：**结果后处理器，不影响前级**（2026-11-19 用户裁定）。
//!
//! 匹配、消费、缓冲与无过滤走完全同一条路；`grep` 只在**返回文本**上清理行。
//! 本模块是 `term_core::automation::LinePolicy` 的薄包装：模式统一由 `Pattern`
//! 编译（与 `re()` / 触发器同一语法），不再有 `filter_view` / `Seg` /
//! `raw_offset_of` / `mid_line` 这套视图↔原始坐标映射。

use std::sync::Arc;

use term_core::automation::{apply_lines, LinePolicy, Pattern, Syntax};

/// 行级过滤器：`grep(pat)` 只保留匹配行，`grep_v(pat)` 丢弃匹配行。
///
/// 内部只有一份 `LinePolicy`，无游标、无视图、无跨调用状态。
#[derive(Clone, Debug)]
pub struct LineFilter {
    policy: LinePolicy,
}

impl LineFilter {
    /// grep 语义：只保留匹配行。模式按**正则**编译，与 `re()` 同一路径。
    pub fn grep(pattern: &str) -> Result<Self, String> {
        Self::build(pattern, true)
    }

    /// grep -v 语义：丢弃匹配行（过滤无用输出）。
    pub fn grep_v(pattern: &str) -> Result<Self, String> {
        Self::build(pattern, false)
    }

    fn build(pattern: &str, keep_matches: bool) -> Result<Self, String> {
        let p = Pattern::compile(pattern, Syntax::Regex, true).map_err(|e| e.to_string())?;
        let policy = if keep_matches {
            LinePolicy::KeepIf(p)
        } else {
            LinePolicy::DropIf(p)
        };
        Ok(Self { policy })
    }

    /// 从已构造的 `Pattern` 构造（供新脚本面的 `grep(pat())` 复用同一编译器）。
    pub fn from_pattern(p: Pattern, keep_matches: bool) -> Self {
        let policy = if keep_matches {
            LinePolicy::KeepIf(p)
        } else {
            LinePolicy::DropIf(p)
        };
        Self { policy }
    }

    /// 对**一次返回文本**按行清理：不完整尾行透传不判（见 `apply_lines`）。
    pub fn apply(&self, text: &str) -> String {
        apply_lines(text, &self.policy)
    }

    /// 整行判定（去掉行尾 `\n` / `\r` 再匹配，与 grep 一致）。
    pub fn keep_line(&self, line: &str) -> bool {
        let content = line.strip_suffix('\n').unwrap_or(line);
        let content = content.strip_suffix('\r').unwrap_or(content);
        self.policy.admits(content)
    }
}

/// Rhai 侧的过滤器句柄（`grep(pat)` / `grep_v(pat)` 的返回值）。
/// 内部 Arc 共享，克隆廉价。
#[derive(Clone, Debug)]
pub struct GrepHandle(pub Arc<LineFilter>);

impl GrepHandle {
    pub fn grep(pattern: &str) -> Result<Self, String> {
        LineFilter::grep(pattern).map(|f| Self(Arc::new(f)))
    }

    pub fn grep_v(pattern: &str) -> Result<Self, String> {
        LineFilter::grep_v(pattern).map(|f| Self(Arc::new(f)))
    }

    pub fn from_pattern(p: Pattern, keep_matches: bool) -> Self {
        Self(Arc::new(LineFilter::from_pattern(p, keep_matches)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn grep_keeps_only_matching_lines() {
        let f = LineFilter::grep("keep").unwrap();
        assert!(f.keep_line("keep1\n"));
        assert!(!f.keep_line("drop\n"));
    }

    #[test]
    fn grep_v_drops_matching_lines() {
        let f = LineFilter::grep_v("^noise").unwrap();
        assert!(!f.keep_line("noise\n"));
        assert!(f.keep_line("data\n"));
    }

    #[test]
    fn line_matching_ignores_the_trailing_newline_like_grep() {
        let f = LineFilter::grep("^noise$").unwrap();
        assert!(f.keep_line("noise\n"));
        assert!(f.keep_line("noise"));
        assert!(!f.keep_line("noise x\n"));
    }

    #[test]
    fn filtering_happens_on_the_return_text_not_before_matching() {
        // 结果后处理：清理只作用于返回文本，不完整尾行透传不判。
        let f = LineFilter::grep_v("^noise").unwrap();
        assert_eq!(f.apply("noise\nkeep\nnoise"), "keep\nnoise");
    }

    #[test]
    fn from_pattern_uses_the_shared_pattern_compiler() {
        let p = Pattern::compile("ERR", Syntax::Regex, false).unwrap();
        let f = LineFilter::from_pattern(p, true);
        assert!(f.keep_line("err\n"), "大小写不敏感来自 Pattern");
        assert!(!f.keep_line("ok\n"));
    }

    #[test]
    fn extreme_empty_pattern_matches_every_line() {
        let f = LineFilter::grep("").unwrap();
        assert_eq!(f.apply("a\nb\nc\n"), "a\nb\nc\n");
    }

    #[test]
    fn extreme_anchor_pattern_matches_every_line() {
        let f = LineFilter::grep("^").unwrap();
        assert_eq!(f.apply("x\ny\n"), "x\ny\n");
    }

    #[test]
    fn extreme_grep_v_match_all_drops_everything() {
        let f = LineFilter::grep_v(".*").unwrap();
        assert_eq!(f.apply("a\nb\nc\n"), "");
    }
}

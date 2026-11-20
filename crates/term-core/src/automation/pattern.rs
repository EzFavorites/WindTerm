//! 模式：字面量与正则共用一个编译器、一套语法。
//!
//! 直接持有 `regex::Regex`（2026-11-19 拍板），不自研适配层。理由：
//! 线性时间自动机是**结构性**保证，而"步数预算"只能是尽力而为；捕获组
//! 也只有真正则能提供。
//!
//! 索引单位一律是**字节**。`regex` 产出的 `Match` 天然落在 UTF-8 char
//! boundary，调用方 `&window[span.start..span.end]` 直接切片即可，不需要
//! 像旧 `trigger.rs` 那样为每条规则留一份 `Vec<char>`。

use std::fmt;

use regex::Regex;
use serde::{Deserialize, Deserializer, Serialize, Serializer};

/// 用户写法：字面量（`pat()`）还是正则（`re()`）。元字符是否生效由它决定。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Syntax {
    /// 纯文本：`a.b` 只匹配字面三点串。
    Literal,
    /// 标准正则：`a.b` 的 `.` 是通配。
    Regex,
}

/// 一次命中的字节区间。**两端都落在 char boundary**。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Span {
    pub start: usize,
    pub end: usize,
}

impl Span {
    pub fn len(&self) -> usize {
        self.end - self.start
    }
    pub fn is_empty(&self) -> bool {
        self.start == self.end
    }
}

/// 跨 chunk 拼接需要保留多少历史。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum WindowHint {
    /// 模式最多匹配 `n` 字节；保留 `n-1` 字节就够跨 chunk 拼接。
    Bounded(usize),
    /// 含无上界重复（如 `a.*b`）；只能按固定上限截断，超出的命中会漏。
    Unbounded,
}

/// 编译期错误。运行期 `find` 永不返回它——模式在构造时就已编译好。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PatternError {
    message: String,
}

impl PatternError {
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
    pub fn message(&self) -> &str {
        &self.message
    }
}

impl fmt::Display for PatternError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for PatternError {}

/// 已编译的模式。`Clone` 廉价（`Regex` 内部共享）。
#[derive(Clone, Debug)]
pub struct Pattern {
    syntax: Syntax,
    source: String,
    case_sensitive: bool,
    re: Regex,
    hint: WindowHint,
}

/// 相等性只看三个输入与派生的窗口提示——`Regex` 自身没有 `PartialEq`，
/// 而且它本是这三个输入的纯派生量，逐个字段比反而可能引入实现差异。
impl PartialEq for Pattern {
    fn eq(&self, other: &Self) -> bool {
        self.syntax == other.syntax
            && self.source == other.source
            && self.case_sensitive == other.case_sensitive
            && self.hint == other.hint
    }
}

/// serde 的线上形状：只存源串与三个开关，`Regex` 在反序列化时重建。
#[derive(Serialize, Deserialize)]
struct PatternWire {
    syntax: Syntax,
    source: String,
    case_sensitive: bool,
}

impl Pattern {
    /// 编译一条模式。空模式是**合法正则**（匹配任意位置），与 grep 及各语言
    /// 正则一致；脚本层若要拒绝空输入，应在调用本方法之前处理。
    pub fn compile(
        source: &str,
        syntax: Syntax,
        case_sensitive: bool,
    ) -> Result<Self, PatternError> {
        let effective = match syntax {
            Syntax::Literal => {
                let escaped = regex::escape(source);
                if case_sensitive {
                    escaped
                } else {
                    format!("(?i){escaped}")
                }
            }
            Syntax::Regex => {
                if case_sensitive {
                    source.to_owned()
                } else {
                    format!("(?i){source}")
                }
            }
        };
        let re = Regex::new(&effective)
            .map_err(|e| PatternError::new(format!("无效模式 {source:?}：{e}")))?;
        // HIR 的 maximum_len 单位是**字节**，无上界重复返回 None。
        // 这是精确计算，不是"模式里有没有 *"的启发式。
        let hir = regex_syntax::Parser::new()
            .parse(&effective)
            .map_err(|e| PatternError::new(format!("模式解析失败 {source:?}：{e}")))?;
        let hint = match hir.properties().maximum_len() {
            Some(n) => WindowHint::Bounded(n),
            None => WindowHint::Unbounded,
        };
        Ok(Self {
            syntax,
            source: source.to_owned(),
            case_sensitive,
            re,
            hint,
        })
    }

    /// 字面量模式的便捷构造。
    pub fn literal(source: &str) -> Result<Self, PatternError> {
        Self::compile(source, Syntax::Literal, true)
    }

    /// 正则模式的便捷构造。
    pub fn regex(source: &str) -> Result<Self, PatternError> {
        Self::compile(source, Syntax::Regex, true)
    }

    pub fn syntax(&self) -> Syntax {
        self.syntax
    }
    pub fn source(&self) -> &str {
        &self.source
    }
    pub fn case_sensitive(&self) -> bool {
        self.case_sensitive
    }
    pub fn window_hint(&self) -> WindowHint {
        self.hint
    }

    /// 找第一个**结束位置晚于 `after_end`** 的命中。
    ///
    /// 搜索从 `0` 开始——这是跨 chunk 拼接的关键：命中可以起于 `after_end`
    /// 之前（上一批留下的尾巴），只要在本次新数据里闭合就算命中。
    /// 调用方若已把文本切到消费点，传 `after_end = 0` 即可。
    pub fn find(&self, window: &str, after_end: usize) -> Option<Span> {
        let bound = window.floor_char_boundary(after_end.min(window.len()));
        let mut pos = 0usize;
        loop {
            let m = self.re.find_at(window, pos)?;
            if m.end() > bound {
                return Some(Span {
                    start: m.start(),
                    end: m.end(),
                });
            }
            pos = if m.end() > pos { m.end() } else { pos + 1 };
        }
    }

    /// 是否存在**任意**匹配（含 `^` / 空模式这类零宽命中）。
    /// 与 [`Self::find`] 的"结束后于 `after_end`"不同：这里问的是 grep 式的
    /// "这一行有没有命中"，不关心位置，也不推进任何游标。
    pub fn matches(&self, text: &str) -> bool {
        self.re.is_match(text)
    }

    /// 正则模式的捕获组（第 0 组是整个匹配）。字面量模式恒返回空表。
    pub fn captures(&self, text: &str) -> Vec<String> {
        if self.syntax == Syntax::Literal {
            return Vec::new();
        }
        self.re
            .captures(text)
            .map(|c| {
                c.iter()
                    .map(|m| m.map(|m| m.as_str().to_owned()).unwrap_or_default())
                    .collect()
            })
            .unwrap_or_default()
    }
}

impl Serialize for Pattern {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        PatternWire {
            syntax: self.syntax,
            source: self.source.clone(),
            case_sensitive: self.case_sensitive,
        }
        .serialize(s)
    }
}

impl<'de> Deserialize<'de> for Pattern {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let wire = PatternWire::deserialize(d)?;
        Self::compile(&wire.source, wire.syntax, wire.case_sensitive)
            .map_err(serde::de::Error::custom)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn literal_treats_metacharacters_as_plain_text() {
        let p = Pattern::literal("a.b").unwrap();
        assert!(p.find("axb", 0).is_none());
        assert_eq!(p.find("xx a.b yy", 0).unwrap(), Span { start: 3, end: 6 });
    }

    #[test]
    fn regex_honours_anchors_and_classes() {
        let p = Pattern::regex("^ERR[0-9]+:").unwrap();
        assert_eq!(p.find("ERR42: bad", 0).unwrap().end, 6);
        assert!(p.find("ok ERR42:", 0).is_none(), "^ 锚在本串开头");
        assert!(p.find("err42:", 0).is_none());
    }

    #[test]
    fn case_insensitive_uses_unicode_folding() {
        let p = Pattern::compile("panic", Syntax::Regex, false).unwrap();
        assert!(p.find("PANIC: core dumped", 0).is_some());
        let lit = Pattern::compile("密码", Syntax::Literal, false).unwrap();
        assert!(lit.find("密 码", 0).is_none());
        assert!(lit.find("MIMA 密码", 0).is_some());
    }

    #[test]
    fn span_is_in_bytes_and_lands_on_char_boundary() {
        let p = Pattern::literal("密码").unwrap();
        let hit = p.find("输入 密码 ok", 0).unwrap();
        assert_eq!(&"输入 密码 ok"[hit.start..hit.end], "密码");
        assert_eq!(hit.len(), 6, "两个汉字各 3 字节");
    }

    #[test]
    fn find_skips_matches_ending_before_after_end() {
        let p = Pattern::literal("hi").unwrap();
        // "hi" 在 0..2（已扫过）与 5..7（新数据）各一次。
        assert_eq!(p.find("hi...hi", 2).unwrap(), Span { start: 5, end: 7 });
        assert!(p.find("hi...hi", 7).is_none());
    }

    #[test]
    fn find_after_end_handles_a_non_boundary_by_flooring() {
        let p = Pattern::literal("码").unwrap();
        // 3 落在"密"的 UTF-8 中间，向下取到 0，仍能找到 3..6。
        assert_eq!(p.find("密码", 3).unwrap(), Span { start: 3, end: 6 });
    }

    #[test]
    fn window_hint_is_computed_exactly_not_guessed() {
        assert_eq!(Pattern::literal("abc").unwrap().window_hint(), WindowHint::Bounded(3));
        assert_eq!(Pattern::regex("a.b").unwrap().window_hint(), WindowHint::Bounded(6));
        // Unicode 下 `.` 最多 4 字节；若正则有 `?i`，折叠类可能更宽。
        assert_eq!(Pattern::regex("a.*b").unwrap().window_hint(), WindowHint::Unbounded);
        assert_eq!(Pattern::regex("a{2,3}").unwrap().window_hint(), WindowHint::Bounded(3));
        assert_eq!(Pattern::regex("a{2,}").unwrap().window_hint(), WindowHint::Unbounded);
    }

    #[test]
    fn malformed_and_empty_patterns_are_rejected_at_compile_time() {
        // 此后 regex 是标准正则：`|` 与 `{n}` 已合法，不再是畸形样例。
        // 这里的坏串必须是真正被 regex crate 拒绝的。
        for bad in ["(", "[oops", "*x", "\\", "a{2,1}"] {
            assert!(
                Pattern::compile(bad, Syntax::Regex, true).is_err(),
                "{bad:?} 应被拒"
            );
            // 字面量模式下元字符都是普通字符。
            assert!(Pattern::compile(bad, Syntax::Literal, true).is_ok());
        }
        // 以前当畸形的两种写法现在是合法正则——钉住语法升级。
        assert!(Pattern::compile("a|b", Syntax::Regex, true).is_ok());
        assert!(Pattern::compile("a{2}", Syntax::Regex, true).is_ok());
        // 空模式是合法正则；拒绝空输入是脚本层的策略，不压在原语上。
        assert!(Pattern::compile("", Syntax::Regex, true).is_ok());
        assert!(Pattern::compile("", Syntax::Literal, true).is_ok());
    }

    #[test]
    fn matches_asks_existence_including_zero_width() {
        let anchor = Pattern::regex("^").unwrap();
        assert!(anchor.matches("anything"));
        assert!(anchor.matches(""), "空串也有开头");
        let empty = Pattern::regex("").unwrap();
        assert!(empty.matches("x"));
        assert!(!Pattern::regex("z").unwrap().matches("abc"));
    }

    #[test]
    fn captures_come_from_regex_and_are_empty_for_literal() {
        let p = Pattern::regex(r"ERR(\d+):").unwrap();
        let caps = p.captures("ERR42: bad");
        assert_eq!(caps, vec!["ERR42:", "42"]);
        assert!(Pattern::literal(r"ERR(\d+):").unwrap().captures("ERR42:").is_empty());
    }

    #[test]
    fn serde_roundtrip_rebuilds_regex_from_source() {
        let p = Pattern::compile("^ERR\\d+", Syntax::Regex, false).unwrap();
        let json = serde_json::to_string(&p).unwrap();
        let back: Pattern = serde_json::from_str(&json).unwrap();
        assert_eq!(back.source(), p.source());
        assert_eq!(back.syntax(), Syntax::Regex);
        assert!(!back.case_sensitive());
        assert!(back.find("err42: ok", 0).is_some());
    }

    #[test]
    fn deserialization_reports_a_bad_pattern_as_an_error() {
        let json = r#"{"syntax":"Regex","source":"(","case_sensitive":true}"#;
        assert!(serde_json::from_str::<Pattern>(json).is_err());
    }
}

//! 脚本编辑器的纯逻辑：文档缓冲区 + Rhai 词法着色。
//!
//! 这里**不知道** egui 的存在（投影层只画，逻辑留在这里）：
//! `apps/desktop/src/ui/overlays/script_editor.rs` 只把 [`ScriptDoc::text`] 塞进多行
//! 输入框、把 [`lex`] 的 token 映射成颜色。于是「读写文档」「算行号」「判断哪个
//! 词该上色」都能在没有窗口的环境里单测。
//!
//! # 行号口径（别混）
//!
//! 文档内部一律 **0 起**（[`ScriptDoc::line`] 的 0 是第一行），因为要当下标用；
//! Rhai 的 [`crate::ScriptError::line`] 是 **1 起**（脚本作者看到的行号），
//! 两者转换只走 [`ScriptDoc::byte_offset`]，别在展示层自己减一。
//!
//! # 为什么着色放在 crate 里而不是 UI 里
//!
//! 该上色的词正是本引擎**注册的原语**（`write` / `wait` / …）与 Rhai 关键字。
//! 原语清单跟着引擎走：以后加一个原语，改一个常量就同时反映到高亮——放 UI 层
//! 会因为「另一份清单」而漂移。

/// 高亮用的词类。只列「需要上色」的类别；标识符、运算符、空白不上色，直接吃
/// 默认前景色——这样 [`lex`] 的输出很小，UI 也只需给几段上色。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TokenKind {
    /// Rhai 语言关键字（`let` / `if` / `return` / `true` …）。
    Keyword,
    /// 本引擎注册的全局原语（`write` / `wait` / `wait_any` / `read` / `read_until` /
    /// `screen` / `grep` / `grep_v`）。
    Primitive,
    /// 字符串或字符字面量（含引号）。
    String,
    /// 数字字面量。
    Number,
    /// 行注释（`// …`）与块注释（`/* … */`）。
    Comment,
}

/// 一个带颜色的词，按**字节**区间给出，方便直接切 `&source[start..end]`。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Token {
    /// 起始字节（含）。
    pub start: usize,
    /// 结束字节（不含）。
    pub end: usize,
    /// 这个词该按什么颜色画。
    pub kind: TokenKind,
}

/// Rhai 关键字。只收语言层面的保留字；`is_def_var` 这类是内置**函数**，不在内。
const KEYWORDS: &[&str] = &[
    "as", "break", "case", "const", "continue", "default", "do", "else", "export", "false",
    "fn", "for", "global", "if", "import", "in", "let", "loop", "private", "return", "switch",
    "this", "throw", "true", "try", "until", "while",
];

/// 本引擎注册的全局原语。
///
/// **从能力面派生**，不是手抄一份：`term_core::surface` 的 `script_names()` 就是
/// 「脚本前端看得见的动词」的唯一真相，`grep` / `grep_v` 是脚本语言的过滤器
/// 构造器（不是会话动词，因此不在那张表里）。
///
/// 手抄清单曾经漂过：表里加了 `tab_*` / `log_*` / `cursor_pos`，这里没跟上，
/// 于是编辑器里那些真正注册过的原语一直是默认色——「能跑但不亮」比「跑不起来」
/// 更难发现。派生之后这类漂移在结构上不可能发生。
fn primitives() -> Vec<&'static str> {
    let mut names = term_core::surface::script_names();
    names.extend(["grep", "grep_v"]);
    names
}

/// 词法着色：把源码切成若干带颜色的区间（按字节偏移）。未列出的字节区
/// （标识符、运算符、空白）即默认色。
///
/// **不会失败**——畸形输入最多被当成默认色、或一路着色到文件尾：编辑器不该
/// 因为作者写了个未闭合引号就拒绝显示。
pub fn lex(source: &str) -> Vec<Token> {
    let bytes = source.as_bytes();
    // 一次算好：`primitives()` 每次都要问能力面要清单，别在标识符循环里反复问。
    let primitives = primitives();
    let mut out = Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        let b = bytes[i];
        // 行注释：`//` 到行尾（不含换行）。
        if b == b'/' && bytes.get(i + 1) == Some(&b'/') {
            let start = i;
            while i < bytes.len() && bytes[i] != b'\n' {
                i += 1;
            }
            out.push(Token {
                start,
                end: i,
                kind: TokenKind::Comment,
            });
            continue;
        }
        // 块注释：`/* … */`，支持嵌套（Rhai 允许）。
        if b == b'/' && bytes.get(i + 1) == Some(&b'*') {
            let start = i;
            i += 2;
            let mut depth = 1usize;
            while i < bytes.len() {
                if bytes[i] == b'/' && bytes.get(i + 1) == Some(&b'*') {
                    depth += 1;
                    i += 2;
                } else if bytes[i] == b'*' && bytes.get(i + 1) == Some(&b'/') {
                    depth -= 1;
                    i += 2;
                    if depth == 0 {
                        break;
                    }
                } else {
                    i += char_len_at(bytes, i);
                }
            }
            out.push(Token {
                start,
                end: i.min(bytes.len()),
                kind: TokenKind::Comment,
            });
            continue;
        }
        // 字符串 / 字符字面量：认 `"…"` 与 `'…'`，反斜杠转义下一个字符。
        if b == b'"' || b == b'\'' {
            let quote = b;
            let start = i;
            i += 1;
            while i < bytes.len() {
                if bytes[i] == b'\\' {
                    i += 1;
                    if i < bytes.len() {
                        i += char_len_at(bytes, i);
                    }
                    continue;
                }
                if bytes[i] == quote {
                    i += 1;
                    break;
                }
                i += char_len_at(bytes, i);
            }
            out.push(Token {
                start,
                end: i.min(bytes.len()),
                kind: TokenKind::String,
            });
            continue;
        }
        // 数字字面量（十进制 / 0x / 0o / 0b，允许 `_` 分隔与小数）。
        if b.is_ascii_digit() {
            let start = i;
            i = scan_number(bytes, i);
            out.push(Token {
                start,
                end: i,
                kind: TokenKind::Number,
            });
            continue;
        }
        // 标识符：关键字 / 原语 / 其它（其它不上色）。
        if b == b'_' || b.is_ascii_alphabetic() {
            let start = i;
            i += 1;
            while i < bytes.len() && (bytes[i] == b'_' || bytes[i].is_ascii_alphanumeric()) {
                i += 1;
            }
            let word = &source[start..i];
            if KEYWORDS.contains(&word) {
                out.push(Token {
                    start,
                    end: i,
                    kind: TokenKind::Keyword,
                });
            } else if primitives.contains(&word) {
                out.push(Token {
                    start,
                    end: i,
                    kind: TokenKind::Primitive,
                });
            }
            continue;
        }
        // 其余（运算符 / 空白 / 标点）：跳过一整个字符。
        i += char_len_at(bytes, i);
    }
    out
}

/// 数字字面量的结束位置。`1..5` 的 `..` 不会被吞成小数点（点后必须跟数字）。
fn scan_number(bytes: &[u8], mut i: usize) -> usize {
    // 进制前缀：0x / 0o / 0b。
    if bytes.get(i) == Some(&b'0') {
        if let Some(&p) = bytes.get(i + 1) {
            if matches!(p, b'x' | b'X' | b'o' | b'O' | b'b' | b'B') {
                i += 2;
                while i < bytes.len() && (bytes[i].is_ascii_alphanumeric() || bytes[i] == b'_') {
                    i += 1;
                }
                return i;
            }
        }
    }
    while i < bytes.len() && (bytes[i].is_ascii_digit() || bytes[i] == b'_') {
        i += 1;
    }
    // 小数点后必须跟数字才算浮点。
    if bytes.get(i) == Some(&b'.') && bytes.get(i + 1).is_some_and(u8::is_ascii_digit) {
        i += 1;
        while i < bytes.len() && (bytes[i].is_ascii_digit() || bytes[i] == b'_') {
            i += 1;
        }
    }
    i
}

/// 字节 `i` 处一个 UTF-8 字符的字节长度（按首字节判，越界给 1）。
fn char_len_at(bytes: &[u8], i: usize) -> usize {
    match bytes.get(i) {
        Some(b) if *b < 0x80 => 1,
        Some(b) if (*b & 0xE0) == 0xC0 => 2,
        Some(b) if (*b & 0xF0) == 0xE0 => 3,
        Some(b) if (*b & 0xF8) == 0xF0 => 4,
        _ => 1,
    }
}

/// 脚本编辑缓冲区：文本 + 脏标记 + 行索引。
///
/// 行索引只在 [`ScriptDoc::new`] / [`ScriptDoc::set_text`] 时重建（O(n)），查询
/// 都是 O(1) / O(log n)。编辑器每帧只读，不重复建索引。
#[derive(Clone, Debug)]
pub struct ScriptDoc {
    text: String,
    dirty: bool,
    /// 每行起始字节偏移；第一项恒为 0，按 `\n` 切分（`\r\n` 的 `\r` 留在行尾，
    /// 由 [`ScriptDoc::line`] 去掉）。
    line_starts: Vec<usize>,
}

impl Default for ScriptDoc {
    fn default() -> Self {
        Self::new(String::new())
    }
}

impl ScriptDoc {
    /// 从一段源码建文档。刚建出来是**干净**的（等同「刚打开一个文件」）。
    pub fn new(text: impl Into<String>) -> Self {
        let text = text.into();
        let mut doc = Self {
            text,
            dirty: false,
            line_starts: Vec::new(),
        };
        doc.reindex();
        doc
    }

    /// 当前文本。
    pub fn text(&self) -> &str {
        &self.text
    }

    /// 整段替换（用户编辑 / 打开另一个文件）。会置脏并重建行索引。
    pub fn set_text(&mut self, text: impl Into<String>) {
        self.text = text.into();
        self.dirty = true;
        self.reindex();
    }

    /// 自上次保存以来改过没有。
    pub fn is_dirty(&self) -> bool {
        self.dirty
    }

    /// 保存成功后清脏。
    pub fn mark_saved(&mut self) {
        self.dirty = false;
    }

    /// 行数。至少 1（空文档也算一行）。
    pub fn line_count(&self) -> usize {
        self.line_starts.len().max(1)
    }

    /// 第 `line0` 行（**0 起**）的内容，不含行尾 `\n` / `\r`。越界给 `None`。
    pub fn line(&self, line0: usize) -> Option<&str> {
        let start = *self.line_starts.get(line0)?;
        let end = self
            .line_starts
            .get(line0 + 1)
            .copied()
            .unwrap_or(self.text.len());
        let raw = &self.text[start..end];
        let raw = raw.strip_suffix('\n').unwrap_or(raw);
        Some(raw.strip_suffix('\r').unwrap_or(raw))
    }

    /// Rhai 的 **1 起** `(行, 列)` → 文档字节偏移。列按**字符**数（Rhai 的口径），
    /// 不是字节；列越界钳到行内容末尾，行号为 0 或越界给 `None`。
    ///
    /// 用它把「错误在第 3 行第 7 列」换算成光标字节位置。
    pub fn byte_offset(&self, line1: usize, column1: usize) -> Option<usize> {
        let line0 = line1.checked_sub(1)?;
        let start = *self.line_starts.get(line0)?;
        let content_end = self.content_end(line0);
        let want = column1.saturating_sub(1);
        for (index, (byte, _)) in self.text[start..content_end].char_indices().enumerate() {
            if index == want {
                return Some(start + byte);
            }
        }
        Some(content_end)
    }

    /// 字节偏移 → `(0 起行, 0 起列)`。列按字符数。`byte` 越界钳到末尾。
    pub fn line_of_byte(&self, byte: usize) -> (usize, usize) {
        let byte = byte.min(self.text.len());
        let line0 = match self.line_starts.binary_search(&byte) {
            Ok(index) => index,
            Err(index) => index.saturating_sub(1),
        };
        let start = self.line_starts[line0];
        let column = self.text[start..byte].chars().count();
        (line0, column)
    }

    /// 第 `line0` 行内容的结束字节（去掉行尾 `\n` 与 `\r`）。
    fn content_end(&self, line0: usize) -> usize {
        let start = self.line_starts[line0];
        let end = self
            .line_starts
            .get(line0 + 1)
            .copied()
            .unwrap_or(self.text.len());
        let mut content_end = end;
        if content_end > start && self.text.as_bytes().get(content_end - 1) == Some(&b'\n') {
            content_end -= 1;
        }
        if content_end > start && self.text.as_bytes().get(content_end - 1) == Some(&b'\r') {
            content_end -= 1;
        }
        content_end
    }

    fn reindex(&mut self) {
        self.line_starts.clear();
        self.line_starts.push(0);
        for (index, byte) in self.text.bytes().enumerate() {
            if byte == b'\n' {
                self.line_starts.push(index + 1);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kinds(source: &str) -> Vec<(TokenKind, &str)> {
        lex(source)
            .into_iter()
            .map(|token| (token.kind, &source[token.start..token.end]))
            .collect()
    }

    #[test]
    fn a_fresh_document_is_clean_and_has_one_line() {
        let doc = ScriptDoc::new("write(\"hi\");");
        assert_eq!(doc.line_count(), 1);
        assert!(!doc.is_dirty(), "刚打开的文档不该是脏的");
        assert_eq!(doc.line(0), Some("write(\"hi\");"));
        assert_eq!(doc.line(1), None);
    }

    #[test]
    fn line_index_splits_on_newline_and_keeps_a_trailing_empty_line() {
        let doc = ScriptDoc::new("a\nb\n");
        assert_eq!(doc.line_count(), 3, "结尾换行后还有一个空行");
        assert_eq!(doc.line(0), Some("a"));
        assert_eq!(doc.line(1), Some("b"));
        assert_eq!(doc.line(2), Some(""));
    }

    #[test]
    fn crlf_line_content_drops_the_carriage_return() {
        let doc = ScriptDoc::new("a\r\nb");
        assert_eq!(doc.line(0), Some("a"), "行内容不该带 \\r");
        assert_eq!(doc.line(1), Some("b"));
    }

    #[test]
    fn rhai_one_based_position_maps_to_byte_offsets() {
        // "let a = 1;\n" 是 11 字节；第 2 行行首 = 11。
        let doc = ScriptDoc::new("let a = 1;\nlet b = 2;\n");
        assert_eq!(doc.byte_offset(1, 1), Some(0));
        assert_eq!(doc.byte_offset(2, 1), Some(11));
        // 第 2 行第 5 个字符（'b'，0 起第 4 个）→ 11 + 4 = 15。
        assert_eq!(doc.byte_offset(2, 5), Some(15));
        // 列越界钳到行内容末尾（"let b = 2;" 长 10 字节 → 11 + 10 = 21）。
        assert_eq!(doc.byte_offset(2, 999), Some(21));
        // 行号 0 或越界 → None。
        assert_eq!(doc.byte_offset(0, 1), None);
        assert_eq!(doc.byte_offset(9, 1), None);
    }

    #[test]
    fn byte_offset_counts_characters_not_bytes_for_non_ascii() {
        // "中文x"：'中' 占 3 字节，'文' 起于字节 3。
        let doc = ScriptDoc::new("中文x");
        assert_eq!(doc.byte_offset(1, 1), Some(0));
        assert_eq!(doc.byte_offset(1, 2), Some(3));
        assert_eq!(doc.byte_offset(1, 3), Some(6));
    }

    #[test]
    fn line_of_byte_round_trips_the_line_start() {
        let doc = ScriptDoc::new("ab\ncd\n");
        assert_eq!(doc.line_of_byte(0), (0, 0));
        assert_eq!(doc.line_of_byte(3), (1, 0));
        assert_eq!(doc.line_of_byte(4), (1, 1));
    }

    #[test]
    fn editing_marks_the_document_dirty_until_saved() {
        let mut doc = ScriptDoc::new("x");
        assert!(!doc.is_dirty());
        doc.set_text("y");
        assert!(doc.is_dirty(), "改过应该脏");
        doc.mark_saved();
        assert!(!doc.is_dirty());
    }

    #[test]
    fn lexer_tags_keywords_primitives_strings_numbers_and_comments() {
        let source = "let n = 42; // note\nwrite(\"hi\");\n";
        let got = kinds(source);
        assert!(got.contains(&(TokenKind::Keyword, "let")));
        assert!(got.contains(&(TokenKind::Number, "42")));
        assert!(got.contains(&(TokenKind::Comment, "// note")));
        assert!(got.contains(&(TokenKind::Primitive, "write")));
        assert!(got.contains(&(TokenKind::String, "\"hi\"")));
    }

    #[test]
    fn a_primitive_name_inside_a_string_is_not_highlighted_as_a_primitive() {
        let source = "write(\"write\");";
        let primitives: Vec<_> = lex(source)
            .into_iter()
            .filter(|token| token.kind == TokenKind::Primitive)
            .collect();
        assert_eq!(primitives.len(), 1, "只有真正调用的 write 算原语");
        assert_eq!(&source[primitives[0].start..primitives[0].end], "write");
    }

    #[test]
    fn line_comment_inside_a_string_does_not_swallow_the_rest_of_the_line() {
        let source = "write(\"a // b\"); let x = 1;";
        let got = kinds(source);
        assert!(
            !got.iter().any(|(kind, _)| *kind == TokenKind::Comment),
            "字符串里的 // 不是注释"
        );
        assert!(got.contains(&(TokenKind::Keyword, "let")));
    }

    #[test]
    fn nested_block_comments_are_one_comment_token() {
        let source = "/* outer /* inner */ done */";
        assert_eq!(kinds(source), vec![(TokenKind::Comment, source)]);
    }

    #[test]
    fn a_range_is_not_swallowed_as_a_float() {
        // `1..5` 的 `..` 不是小数点：1 只是 Number，`..` 不上色。
        let tokens = lex("1..5");
        assert_eq!(tokens.len(), 2, "1 与 5 各是一个数字");
        assert!(tokens.iter().all(|token| token.kind == TokenKind::Number));
    }

    #[test]
    fn unterminated_string_and_comment_color_to_end_without_panicking() {
        assert_eq!(
            lex("\"open"),
            vec![Token {
                start: 0,
                end: 5,
                kind: TokenKind::String
            }]
        );
        assert_eq!(
            lex("/* open"),
            vec![Token {
                start: 0,
                end: 7,
                kind: TokenKind::Comment
            }]
        );
    }
}

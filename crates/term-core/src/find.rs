//! 纯核心：在事实窗口里查找文本，零 UI 依赖，可单测。
//!
//! 在事实窗口上做增量子串查找，并提供匹配导航（上/下一个、计数）。
//!
//! 扫描层不再绑定 `GridSnapshot`。同一套引擎既扫当前屏（`find_matches`），
//! 也扫回看窗口（`find_matches_in`）——查找的输入从「整块快照」变成「你看得见的
//! 那一段事实」，两个入口只是喂给引擎的行序列不同。
//!
//! 行号统一是**内容绝对行号**：`0` = 当前屏第一行，负数 = 回看缓冲里的历史行。
//! 这与 `LineSlice.row`、`Selection` 的 `Pos.row` 是同一坐标系，投影拿到
//! `Match.row` 就能直接换算成「屏上第几行」，不需要再补一层偏移。
//!
//! # 大小写折叠口径（字面版与正则版共用）
//!
//! 字面模式不自己实现大小写规则：把 query 交给 [`regex::escape`] 原样转义后，
//! 调用与正则模式**同一个** [`find_matches_regex_in`]。于是忽略大小写时两边
//! 必然都是 `regex` 的 `case_insensitive`（Unicode 简单大小写折叠），不再有
//! 「手写 lower/upper 双向等价」这种近似口径——它对 `I`/`ı`、`i`/`ı`、
//! `U+A7CE`/`U+A7CF` 等 28 对字符过度匹配。对等判据见 `find_tests.rs` 里的
//! `literal_and_regex_agree_*` 测试。

use crate::grid::{Cell, GridSnapshot, ScreenImage};

/// 一次匹配：内容绝对行号（负数 = 历史行）、起始列、结束列（不含）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Match {
    pub row: i64,
    pub col_start: usize,
    pub col_end: usize,
}

impl Match {
    pub fn new(row: i64, col_start: usize, col_end: usize) -> Self {
        Self {
            row,
            col_start,
            col_end,
        }
    }
}

/// 在当前屏（快照）中查找所有匹配项。
///
/// - `query` 为空返回空 Vec。
/// - `case_sensitive` 控制是否区分大小写（口径与正则版同一套）。
/// - 逐行扫描；列号是单元格下标（宽字符的占位格也算一格），可直接用于画高亮。
/// - 返回按行号、列号升序排列的所有匹配；行号为 `0..rows`。
///
/// 快照只是事实窗口的一种输入形状：转成 `ScreenImage` 后走与
/// [`find_matches_in`] 完全相同的引擎，不存在第二份字面扫描实现。
pub fn find_matches(snap: &GridSnapshot, query: &str, case_sensitive: bool) -> Vec<Match> {
    if query.is_empty() || snap.rows == 0 || snap.cols == 0 {
        return Vec::new();
    }
    find_matches_in(&snap.to_screen_image(), query, case_sensitive, false)
}

/// 在事实窗口（当前屏或回看缓冲）中查找所有匹配项。
///
/// 只扫窗口覆盖到的行——用户看到什么就搜什么。`Match.row` 直接是
/// `LineSlice.row`，回看时是负数。
///
/// `whole_word`（`W` 那颗按钮）要求命中的**两端**都落在词的边界上：
/// 前一个字不是词字符（行首也算），后一个字也不是词字符（行尾也算）。
/// 词字符按「字母 / 数字 / 下划线」判——这三样之外都当分隔符，
/// 所以 `foo-bar` 里搜 `foo` 在整词模式下仍然命中。
pub fn find_matches_in(
    image: &ScreenImage,
    query: &str,
    case_sensitive: bool,
    whole_word: bool,
) -> Vec<Match> {
    if query.is_empty() || image.cols == 0 {
        return Vec::new();
    }

    // 字面模式与正则模式共用同一台引擎：query 逐字符转义后只可能是一个合法
    // 的「字面量」正则，所以 build 失败在语义上不可达；真出了意外也没有 panic
    // 的路径，返回空结果即可（生产路径不增加 panic）。
    let pattern = regex::escape(query);
    let Ok(matches) = find_matches_regex_in(image, &pattern, case_sensitive) else {
        return Vec::new();
    };
    if whole_word {
        return whole_word_matches(image, matches);
    }
    matches
}

/// 在事实窗口中查找下一个匹配（相对于游标位置向后）。
///
/// 返回 (match_index_in_results, Match) 或 None。行号是内容绝对行号，
/// 所以 `cursor_row` 在回看时同样是负数。
///
/// 注意：投影层「下一个 / 上一个」用的是匹配集上的环形索引
/// （ADR-0015），不走这里——两者坐标系相同，但导航语义不同。
pub fn find_next(
    image: &ScreenImage,
    query: &str,
    case_sensitive: bool,
    whole_word: bool,
    cursor_row: i64,
    cursor_col: usize,
) -> Option<(usize, Match)> {
    let all = find_matches_in(image, query, case_sensitive, whole_word);
    if all.is_empty() {
        return None;
    }
    for (idx, m) in all.iter().enumerate() {
        if m.row > cursor_row || (m.row == cursor_row && m.col_start > cursor_col) {
            return Some((idx, *m));
        }
    }
    // 循环到第一个
    Some((0, all[0]))
}

/// 在事实窗口中查找上一个匹配（相对于游标位置向前）。
pub fn find_previous(
    image: &ScreenImage,
    query: &str,
    case_sensitive: bool,
    whole_word: bool,
    cursor_row: i64,
    cursor_col: usize,
) -> Option<(usize, Match)> {
    let all = find_matches_in(image, query, case_sensitive, whole_word);
    if all.is_empty() {
        return None;
    }
    for (idx, m) in all.iter().enumerate().rev() {
        if m.row < cursor_row || (m.row == cursor_row && m.col_end <= cursor_col) {
            return Some((idx, *m));
        }
    }
    // 循环到最后一个
    let last = all.len() - 1;
    Some((last, all[last]))
}

/// 校验正则语法；供 UI 在查询串上实时提示「表达式写错了」。
///
/// 与 [`find_matches_regex_in`] 用同一套 `regex` 语法（同一 builder 配置），
/// 避免 UI 说合法、引擎却报错的口径漂移。UI 无需直接依赖 regex crate。
pub fn validate_regex(pattern: &str) -> Result<(), regex::Error> {
    regex::Regex::new(pattern).map(|_| ())
}

/// 在事实窗口里用正则表达式查找，返回所有匹配。
///
/// 与字面版 [`find_matches_in`] 共用字形折叠与坐标系：`Match.row` 是内容绝对
/// 行号，`col_start/col_end` 是单元格列（宽字符占位格算在内），可直接画高亮。
///
/// `case_sensitive` 通过 `RegexBuilder::case_insensitive` 控制。它是唯一的大小写
/// 折叠口径：字面版 [`find_matches_in`] 把 query `regex::escape` 后也走这里，
/// 所以两边逐项相等。`whole_word` 整词语义留给字面版——正则用户自己写 `\b`
/// 更精确。
///
/// 语法错误返回 `Err` 而不是吞掉：调用方要能把「表达式写错了」与「没有匹配」
/// 区分开（后者显示 0/N，前者应当提示用户改表达式）。
pub fn find_matches_regex_in(
    image: &ScreenImage,
    pattern: &str,
    case_sensitive: bool,
) -> Result<Vec<Match>, regex::Error> {
    let re = regex::RegexBuilder::new(pattern)
        .case_insensitive(!case_sensitive)
        .build()?;
    let mut out = Vec::new();
    for line in &image.lines {
        let (cols, chars) = glyphs_and_cols(&line.cells);
        if chars.is_empty() {
            continue;
        }
        let text: String = chars.iter().collect();
        // 正则给的是字节偏移，折叠表按字形(char)索引。建一张 byte→char 的表：
        // 每个字节都映射到「包含它的那个字形」的下标。多字节字形的非首字节
        // （如汉字「你」的 2/3 号字节）也必须映射到它自己，不能假设偏移落在
        // 字形起点——`m.end()-1` 恰恰可能是多字节字形的最后一个字节。
        let mut byte_to_char = vec![0usize; text.len() + 1];
        for (idx, (start, ch)) in text.char_indices().enumerate() {
            for b in start..start + ch.len_utf8() {
                byte_to_char[b] = idx;
            }
        }
        let char_at = |byte: usize| -> usize { byte_to_char[byte] };
        for m in re.find_iter(&text) {
            if m.start() == m.end() {
                continue; // 空匹配（如 `a*`）不画高亮，否则零宽框没意义。
            }
            let first = char_at(m.start());
            let last = char_at(m.end() - 1);
            let col_start = cols[first];
            let cell_end = cols[last] + if line.cells[cols[last]].wide { 2 } else { 1 };
            out.push(Match::new(line.row, col_start, cell_end));
        }
    }
    Ok(out)
}

/// 正则的多行模式（`≡`）：`.` 可跨行，`^` / `$` 按行锚定。
///
/// 返回仍是逐行的 [`Match`]：一条跨行命中会被拆成它覆盖的每一行上的一段，
/// 每段用那一行的字形列区间。这样高亮、结果列表与单线正则共用同一个
/// `Match` 坐标系，不引入「跨行匹配」这种 UI 无法画的第二种值。
///
/// 行尾换行只作为分隔符参与匹配，不会变成任何一行上的高亮格。
pub fn find_matches_regex_multiline_in(
    image: &ScreenImage,
    pattern: &str,
    case_sensitive: bool,
) -> Result<Vec<Match>, regex::Error> {
    let re = regex::RegexBuilder::new(pattern)
        .case_insensitive(!case_sensitive)
        .dot_matches_new_line(true)
        .multi_line(true)
        .build()?;
    // 把整个窗口拼成一段文本，并留一张 byte → (行序号, 行内字形序号) 表。
    // 字节偏移来自 regex，字形序号来自折叠表，两者不能混用（宽字符先踩过坑）。
    let mut text = String::new();
    let mut lines: Vec<(i64, Vec<usize>)> = Vec::new();
    let mut byte_to_pos: Vec<(usize, usize)> = Vec::new();
    for line in &image.lines {
        let (cols, chars) = glyphs_and_cols(&line.cells);
        let li = lines.len();
        lines.push((line.row, cols));
        for (ci, ch) in chars.iter().enumerate() {
            let at = text.len();
            text.push(*ch);
            for _ in at..text.len() {
                byte_to_pos.push((li, ci));
            }
        }
        // 换行字节落在「本行末尾」，不作为任何字形参与高亮。
        text.push('\n');
        byte_to_pos.push((li, chars.len()));
    }
    byte_to_pos.push((lines.len().saturating_sub(1), 0));

    let mut out = Vec::new();
    for m in re.find_iter(&text) {
        if m.start() == m.end() {
            continue;
        }
        let (start_line, start_char) = byte_to_pos
            .get(m.start())
            .copied()
            .unwrap_or((0, 0));
        let (end_line, end_char) = byte_to_pos
            .get(m.end().saturating_sub(1))
            .copied()
            .unwrap_or((start_line, start_char));
        for li in start_line..=end_line {
            let Some((row, cols)) = lines.get(li) else { continue };
            let from = if li == start_line { start_char.min(cols.len()) } else { 0 };
            // `end_char` 来自 `m.end() - 1`，是**含端点**的字形下标；高亮区间是
            // 半开的，必须 +1。单行版也是这样从 `m.end()-1` 换到 `col_end` 的。
            // 落在换行字节上时 `end_char == chars.len()`，+1 再 min 到行宽，
            // 于是整行都高亮、换行本身仍不占任何格。
            let to = if li == end_line { (end_char + 1).min(cols.len()) } else { cols.len() };
            if to <= from {
                continue;
            }
            let col_start = cols[from];
            let last = cols[to - 1];
            let wide = image
                .lines
                .get(li)
                .and_then(|l| l.cells.get(last))
                .is_some_and(|c| c.wide);
            out.push(Match::new(*row, col_start, last + if wide { 2 } else { 1 }));
        }
    }
    Ok(out)
}

/// 把一行的单元格折叠成「字形 → 起始列」两条平行序列，裁掉行尾空白。
///
/// `cols[i]` 是第 i 个字形的起始列，`chars[i]` 是它的字符。宽字符占两格
/// （起始格带字、第二格是占位空格），占位格没有字形——按格扫会让「你好」
/// 中间隔一个空格，`查找 "你好"` 永远命中不了。匹配在字符序列上做，命中后
/// 再用 `cols` 还原成「屏上从第几格到第几格」，高亮框与文本才不会各说一套。
///
/// 字面扫描与正则扫描共用这一份折叠，避免两套口径在高亮列上漂移。
fn glyphs_and_cols(cells: &[Cell]) -> (Vec<usize>, Vec<char>) {
    // 起始列与行尾空白裁切统一走 `glyph_columns`；这里只补上每列的字符。
    let cols = crate::grid::glyph_columns(cells);
    let chars = cols.iter().map(|&c| cells[c].ch).collect();
    (cols, chars)
}

/// 在正则引擎给出的命中结果上做整词过滤。
///
/// 引擎返回的 `col_start/col_end` 是**单元格列**，边界判据吃的是**字形序列**，
/// 所以先用 `glyphs_and_cols` 把命中行还原成字形，再把两个列端映射回字形下标。
/// 宽字符占两格：`col_end` 是开区间，一个字形在 `cols` 里只有一个起始列，
/// 因此 `cols[at..].take_while(c < col_end).count()` 正好是这段命中的字形数。
fn whole_word_matches(image: &ScreenImage, matches: Vec<Match>) -> Vec<Match> {
    matches
        .into_iter()
        .filter(|m| {
            let Some(line) = image.lines.iter().find(|line| line.row == m.row) else {
                return false;
            };
            let (cols, chars) = glyphs_and_cols(&line.cells);
            let Some(at) = cols.iter().position(|&col| col == m.col_start) else {
                return false;
            };
            let len = cols[at..].iter().take_while(|&&col| col < m.col_end).count();
            len > 0 && on_word_boundaries(&chars, at, len)
        })
        .collect()
}

/// `at..at+len` 这一段是否两端都贴着词边界（行首 / 行尾也算边界）。
///
/// 只看**紧邻**的两个字符：`foo` 在 `foobar` 里不算整词（后面是 `b`），
/// 在 `foo_bar` 里也不算（`_` 是词字符），在 `foo.bar` 里算。
fn on_word_boundaries(chars: &[char], at: usize, len: usize) -> bool {
    let before_ok = at == 0 || !is_word_char(chars[at - 1]);
    let after_ok = at + len >= chars.len() || !is_word_char(chars[at + len]);
    before_ok && after_ok
}

/// 词字符：字母 / 数字 / 下划线。其余（空格、标点、制表）都当分隔符。
fn is_word_char(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

#[cfg(test)]
#[path = "find_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "find_multiline_tests.rs"]
mod multiline_tests;

//! Trigger 用的**简化正则**：只认 `^ $ . * + ? []` 与 `\` 转义。
//!
//! 为什么自己写：`term-app` 的依赖表只有 `term-core`（B1），为一个「看到某段输出
//! 就做点什么」的功能引入通用正则库，代价比收益大；而 Trigger 真正需要的语法
//! 也只有这几样——分组、选择、回溯引用在企业场景里几乎不用，却把回溯风险带进来。
//!
//! 三条纪律：
//!
//! 1. **装载期拒绝畸形模式**（`parse` 返回 `Err`），运行期只做 `Ok` / `Err` 分支，
//!    绝不 panic；
//! 2. **步数上限**：贪婪量词会上溯，`a*a*a*a*a*b` 对一长串 `a` 就是经典的指数
//!    回溯。`search` 共用一个预算，用完就当作「未命中」——宁可漏一次，
//!    也不能让终端卡在一条规则上；
//! 3. `.` 匹配任意字符（含换行）；`^` / `$` 锚在**本轮窗口**的首尾，不是整段会话。
//!
//! 不支持：分组 `()`、选择 `|`、计数重复 `{n,m}`、回溯引用 `\1`。它们在 `parse`
//! 里直接报错，而不是被当成普通字符静默放过。

/// 单字符原子。量词只作用在原子之上（`^*` 这类会在 `parse` 里被拒）。
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Atom {
    Char(char),
    Any,
    /// `[a-z0-9_]`；`negated` 对应 `[^…]`。区间按码位比较。
    Class {
        negated: bool,
        ranges: Vec<(char, char)>,
    },
}

impl Atom {
    fn matches(&self, ch: char, case_sensitive: bool) -> bool {
        match self {
            Atom::Char(c) => char_eq(*c, ch, case_sensitive),
            Atom::Any => true,
            Atom::Class { negated, ranges } => {
                class_contains(ranges, ch, case_sensitive) != *negated
            }
        }
    }
}

/// 模式里的一段：原子（可带量词）或锚点。
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Piece {
    /// `min` / `max` 是原子的重复次数；`max == usize::MAX` 表示无上界。
    Atom(Atom, usize, usize),
    Start,
    End,
}

/// 编译一条模式。空模式、悬空量词、未闭合字符类、逆序区间都在这里被拒。
pub(crate) fn parse(pattern: &str) -> Result<Vec<Piece>, String> {
    let chars: Vec<char> = pattern.chars().collect();
    if chars.is_empty() {
        return Err("空模式".to_string());
    }
    let mut pieces = Vec::with_capacity(chars.len());
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        match c {
            '^' => {
                pieces.push(Piece::Start);
                i += 1;
            }
            '$' => {
                pieces.push(Piece::End);
                i += 1;
            }
            '*' | '+' | '?' => return Err(format!("量词 {c} 前没有可重复的原子")),
            '(' | ')' | '|' | '{' | '}' => {
                return Err(format!("不支持 {c}（只认 ^ $ . * + ? [] 与反斜杠转义）"));
            }
            ']' => return Err("多余的 ]".to_string()),
            _ => {
                let atom = parse_atom(&chars, &mut i)?;
                let (min, max) = parse_quantifier(&chars, &mut i);
                pieces.push(Piece::Atom(atom, min, max));
            }
        }
    }
    Ok(pieces)
}

fn parse_atom(chars: &[char], i: &mut usize) -> Result<Atom, String> {
    let c = *chars.get(*i).ok_or_else(|| "模式意外结束".to_string())?;
    *i += 1;
    match c {
        '.' => Ok(Atom::Any),
        '\\' => {
            let escaped = *chars
                .get(*i)
                .ok_or_else(|| "转义符后没有字符".to_string())?;
            *i += 1;
            if escaped.is_ascii_digit() {
                return Err("不支持回溯引用（\\1）".to_string());
            }
            Ok(Atom::Char(escaped))
        }
        '[' => parse_class(chars, i),
        _ => Ok(Atom::Char(c)),
    }
}

fn parse_class(chars: &[char], i: &mut usize) -> Result<Atom, String> {
    let negated = chars.get(*i) == Some(&'^');
    if negated {
        *i += 1;
    }
    let mut ranges: Vec<(char, char)> = Vec::new();
    loop {
        let c = *chars
            .get(*i)
            .ok_or_else(|| "字符类没有闭合的 ]".to_string())?;
        if c == ']' {
            *i += 1;
            break;
        }
        let lo = read_class_char(chars, i)?;
        // `-` 只有在后面还有字符、且不是 `]` 时才是区间；`[a-]` 里的 `-` 是字面量。
        let is_range =
            chars.get(*i) == Some(&'-') && matches!(chars.get(*i + 1), Some(next) if *next != ']');
        if is_range {
            *i += 1;
            let hi = read_class_char(chars, i)?;
            if hi < lo {
                return Err(format!("字符区间 {lo}-{hi} 逆序"));
            }
            ranges.push((lo, hi));
        } else {
            ranges.push((lo, lo));
        }
    }
    if ranges.is_empty() {
        return Err("空字符类 []".to_string());
    }
    Ok(Atom::Class { negated, ranges })
}

fn read_class_char(chars: &[char], i: &mut usize) -> Result<char, String> {
    let c = *chars
        .get(*i)
        .ok_or_else(|| "字符类没有闭合的 ]".to_string())?;
    *i += 1;
    if c != '\\' {
        return Ok(c);
    }
    let escaped = *chars
        .get(*i)
        .ok_or_else(|| "字符类里的转义符后没有字符".to_string())?;
    *i += 1;
    Ok(escaped)
}

fn parse_quantifier(chars: &[char], i: &mut usize) -> (usize, usize) {
    match chars.get(*i) {
        Some('*') => {
            *i += 1;
            (0, usize::MAX)
        }
        Some('+') => {
            *i += 1;
            (1, usize::MAX)
        }
        Some('?') => {
            *i += 1;
            (0, 1)
        }
        _ => (1, 1),
    }
}

/// 在 `text` 里找**最早**的一个匹配，且匹配结束位置必须 `> min_end`。
///
/// `min_end` 是「上一轮已经扫描过的前缀长度」：流式分块时，完全落在旧前缀里的
/// 命中已经报过了，再报一次就是重复触发。
///
/// 返回 `(start, end)`（字符下标，左闭右开）。
pub(crate) fn search(
    pieces: &[Piece],
    text: &[char],
    case_sensitive: bool,
    min_end: usize,
    pattern_chars: usize,
) -> Option<(usize, usize)> {
    let mut budget = step_limit(pattern_chars, text.len());
    for start in 0..=text.len() {
        if budget == 0 {
            return None;
        }
        if let Some(end) = match_from(pieces, 0, text, start, case_sensitive, &mut budget) {
            if end > min_end {
                return Some((start, end));
            }
        }
    }
    None
}

/// 每一步匹配尝试都从这里扣一次预算；扣完即放弃。
fn match_from(
    pieces: &[Piece],
    index: usize,
    text: &[char],
    pos: usize,
    case_sensitive: bool,
    budget: &mut u64,
) -> Option<usize> {
    if *budget == 0 {
        return None;
    }
    *budget -= 1;
    let Some(piece) = pieces.get(index) else {
        // 模式走完了：当前位置就是一个合法结束点。
        return Some(pos);
    };
    match piece {
        Piece::Start => (pos == 0)
            .then(|| match_from(pieces, index + 1, text, pos, case_sensitive, budget))
            .flatten(),
        Piece::End => (pos == text.len())
            .then(|| match_from(pieces, index + 1, text, pos, case_sensitive, budget))
            .flatten(),
        Piece::Atom(atom, min, max) => {
            let cap = (*max).min(text.len() - pos);
            let mut run = 0;
            while run < cap {
                match text.get(pos + run) {
                    Some(ch) if atom.matches(*ch, case_sensitive) => run += 1,
                    _ => break,
                }
            }
            if run < *min {
                return None;
            }
            // 贪婪：先试最长，再逐个退回（经典回溯点，靠预算兜底）。
            let mut take = run;
            loop {
                if *budget == 0 {
                    return None;
                }
                if let Some(end) =
                    match_from(pieces, index + 1, text, pos + take, case_sensitive, budget)
                {
                    return Some(end);
                }
                if take == *min {
                    return None;
                }
                take -= 1;
            }
        }
    }
}

/// 步数上限：`pattern.len() * text.len() * 8`，并压一个绝对上限。
///
/// 为什么乘 8：合法匹配的朴素代价是 O(pattern × text)，乘 8 是给贪婪回溯留的
/// 正常余量；真正病态的模式（多个 `*` 串联）会在这条线上被截断。
/// 为什么加绝对上限：单个 chunk 可能是几 MB，`3 × 4MB × 8` 仍然过亿步；
/// 一条规则不该独占一次输出处理的时间片，800 万步在调试构建里也是毫秒级。
pub(crate) fn step_limit(pattern_chars: usize, text_len: usize) -> u64 {
    const CAP: u64 = 8_000_000;
    let pattern = pattern_chars.max(1) as u64;
    let text = text_len.max(1) as u64;
    pattern.saturating_mul(text).saturating_mul(8).min(CAP)
}

/// 不区分大小写时按 Unicode 简单折叠比较（`to_lowercase` 的多字符展开取首字符）。
pub(crate) fn char_eq(a: char, b: char, case_sensitive: bool) -> bool {
    if a == b {
        return true;
    }
    if case_sensitive {
        return false;
    }
    a.to_lowercase().eq(b.to_lowercase())
}

fn class_contains(ranges: &[(char, char)], ch: char, case_sensitive: bool) -> bool {
    let hit = |c: char| ranges.iter().any(|(lo, hi)| *lo <= c && *hi >= c);
    if hit(ch) {
        return true;
    }
    if case_sensitive {
        return false;
    }
    // `[a-z]` 要能匹配 `A`：把候选字符的大小写变体都试一遍。
    let lower = ch.to_lowercase().next().unwrap_or(ch);
    let upper = ch.to_uppercase().next().unwrap_or(ch);
    hit(lower) || hit(upper)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn find(pattern: &str, text: &str) -> Option<(usize, usize)> {
        let pieces = parse(pattern).expect("模式应可编译");
        let chars: Vec<char> = text.chars().collect();
        search(&pieces, &chars, true, 0, pattern.chars().count())
    }

    #[test]
    fn plain_and_dot_and_quantifiers() {
        assert_eq!(find("abc", "xabcx"), Some((1, 4)));
        assert_eq!(find("a.c", "abc"), Some((0, 3)));
        assert_eq!(find("ab*c", "ac"), Some((0, 2)));
        assert_eq!(find("ab+c", "ac"), None);
        assert_eq!(find("ab?c", "ac"), Some((0, 2)));
        assert_eq!(find("ab?c", "abc"), Some((0, 3)));
    }

    #[test]
    fn anchors_are_relative_to_the_window() {
        assert_eq!(find("^abc", "abcx"), Some((0, 3)));
        assert_eq!(find("^abc", "xabc"), None);
        assert_eq!(find("abc$", "xabc"), Some((1, 4)));
        assert_eq!(find("abc$", "abcx"), None);
    }

    #[test]
    fn classes_and_escapes() {
        assert_eq!(find("[a-c]+", "xxabcyy"), Some((2, 5)));
        assert_eq!(find("[^0-9]+", "12ab34"), Some((2, 4)));
        assert_eq!(find("a\\.b", "axb"), None);
        assert_eq!(find("a\\.b", "a.b"), Some((0, 3)));
        assert_eq!(find("[-a]", "x-"), Some((1, 2)));
    }

    #[test]
    fn case_folding_is_optional() {
        let pieces = parse("panic").expect("模式应可编译");
        let chars: Vec<char> = "PANIC".chars().collect();
        assert_eq!(search(&pieces, &chars, true, 0, 5), None);
        assert_eq!(search(&pieces, &chars, false, 0, 5), Some((0, 5)));
    }

    #[test]
    fn malformed_patterns_are_rejected() {
        for bad in [
            "", "*a", "a**", "a(", "a|b", "[abc", "[]", "a\\", "(a)", "a{2}", "]", "a\\1",
        ] {
            assert!(parse(bad).is_err(), "{bad:?} 应被拒");
        }
        for good in ["abc", "^a.*b$", "[a-z0-9_]+", "\\^x", "a?b+c*"] {
            assert!(parse(good).is_ok(), "{good:?} 应可编译");
        }
    }

    #[test]
    fn budget_stops_exponential_backtracking() {
        let pattern = "a*a*a*a*a*b";
        let text: Vec<char> = format!("{}c", "a".repeat(80)).chars().collect();
        let pieces = parse(pattern).expect("模式应可编译");
        let started = std::time::Instant::now();
        assert_eq!(
            search(&pieces, &text, true, 0, pattern.chars().count()),
            None
        );
        assert!(
            started.elapsed().as_millis() < 500,
            "步数上限应让回溯快速放弃"
        );
    }
}

//! 内置触发器的一类纯检测：URL 与 IPv4。
//!
//! 官方触发器族里的 hyperlink / ip 识别，在配置层需要一条用户规则；这里给出
//! 「看到输出就报命中」的后端原语，不生成 UI 规则、不弹菜单。宿主拿到命中后
//! 自己决定是加链接、加菜单还是只记一笔。检测是纯函数、零 IO，
//! 不引入通用正则引擎（沿用 trigger_regex 的「无回溯风险」纪律）。

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Builtin {
    Url,
    Ipv4,
}

/// 一次检测到的片段。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Found {
    pub kind: Builtin,
    pub text: String,
    pub start: usize,
}

fn is_url_start(chunk: &[u8], i: usize) -> Option<usize> {
    for prefix in [b"https://".as_slice(), b"http://".as_slice()] {
        if chunk.get(i..i + prefix.len()) == Some(prefix) {
            return Some(i + prefix.len());
        }
    }
    None
}

fn url_end(chunk: &[u8], mut i: usize) -> usize {
    while i < chunk.len() && !chunk[i].is_ascii_whitespace() && chunk[i] != b'"' && chunk[i] != b'\'' {
        i += 1;
    }
    // 常见标点属于 URL 之前的句子，不该带进链接。
    while i > 0 && matches!(chunk[i - 1], b'.' | b',' | b';' | b':' | b')' | b']') {
        i -= 1;
    }
    i
}

fn ipv4_at(chunk: &[u8], i: usize) -> Option<usize> {
    if i > 0 && (chunk[i - 1].is_ascii_digit() || chunk[i - 1] == b'.') {
        return None;
    }
    let mut end = i;
    let mut groups = 0;
    while groups < 4 {
        let start = end;
        while end < chunk.len() && chunk[end].is_ascii_digit() && end - start < 3 {
            end += 1;
        }
        if end == start || end - start > 3 {
            return None;
        }
        let value: u16 = std::str::from_utf8(&chunk[start..end]).ok()?.parse().ok()?;
        if value > 255 {
            return None;
        }
        groups += 1;
        if groups == 4 {
            break;
        }
        if chunk.get(end) != Some(&b'.') {
            return None;
        }
        end += 1;
    }
    if chunk.get(end).is_some_and(|b| b.is_ascii_digit() || *b == b'.') {
        return None;
    }
    Some(end)
}

/// 从一段输出里找所有 URL / IPv4。同一位置只报一种（URL 优先）。
pub fn detect(chunk: &str) -> Vec<Found> {
    let bytes = chunk.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        if let Some(body) = is_url_start(bytes, i) {
            let end = url_end(bytes, body);
            out.push(Found {
                kind: Builtin::Url,
                text: String::from_utf8_lossy(&bytes[i..end]).into_owned(),
                start: i,
            });
            i = end.max(i + 1);
            continue;
        }
        if bytes[i].is_ascii_digit() {
            if let Some(end) = ipv4_at(bytes, i) {
                out.push(Found {
                    kind: Builtin::Ipv4,
                    text: String::from_utf8_lossy(&bytes[i..end]).into_owned(),
                    start: i,
                });
                i = end.max(i + 1);
                continue;
            }
        }
        i += 1;
    }
    out
}

/// 一段输出末尾是否像一个命令提示符。启发式，只用于 Send Condition 的
/// `AfterMatchAndPrompt` 放行；宁可晚一点，也不能把普通输出误当提示符。
pub fn looks_like_prompt(chunk: &str) -> bool {
    // 只去掉行尾换行，不去掉提示符末尾那个空格。
    let tail = chunk.trim_end_matches(['\n', '\r']);
    tail.ends_with("$ ") || tail.ends_with("# ") || tail.ends_with("❯ ") || tail.ends_with("> ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_prompt_is_only_the_end_of_a_chunk() {
        assert!(looks_like_prompt("user@host:~$ "));
        assert!(looks_like_prompt("root# "));
        assert!(!looks_like_prompt("hello world\n"));
        assert!(!looks_like_prompt("$ echo hi"));
    }

    #[test]
    fn detects_urls_without_trailing_sentence_punctuation() {
        let found = detect("see https://example.com/a?x=1, ok");
        assert_eq!(
            found,
            vec![Found {
                kind: Builtin::Url,
                text: "https://example.com/a?x=1".into(),
                start: 4,
            }]
        );
    }

    #[test]
    fn detects_ipv4_but_not_version_numbers() {
        let found = detect("host 10.0.0.9 ready, v1.2.3.4.5");
        assert_eq!(found.len(), 1, "五段版本号不是 IPv4");
        assert_eq!(found[0].kind, Builtin::Ipv4);
        assert_eq!(found[0].text, "10.0.0.9");
        assert!(detect("256.1.1.1").is_empty(), "越界字节不是 IPv4");
    }
}

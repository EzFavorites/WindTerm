//! 内置触发器的纯检测：URL / IP / 十六进制 / 时间戳 / 压缩包文件名。
//!
//! 官方触发器族里的 hyperlink / ip / hex / timestamp / unarchiver 识别，在配置层
//! 需要一条用户规则；这里给出「看到输出就报命中」的后端原语，不生成 UI 规则、
//! 不弹菜单。宿主拿到命中后自己决定是加链接、加菜单还是只记一笔。检测与换算
//! 全是纯函数：零 IO、零墙钟、零正则依赖（沿用 trigger_regex 的「无回溯风险」纪律）。
//! 时间戳只把**学会的**纪元值格式化成 UTC 文本，绝不读系统时钟。

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Builtin {
    Url,
    Ipv4,
    Ipv6,
    /// 十六进制数（`0x` 前缀或至少四位的裸 hex 串）。
    Hex,
    /// Unix 时间戳（10 位秒 / 13 位毫秒，可带负号）。
    Timestamp,
    /// 压缩包文件名（zip / tar.gz / 7z / rar 等）。
    Archive,
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

/// 从 `i` 起尝试吃一个 IPv6 字面量，返回结束偏移。
///
/// 不引入正则引擎：只认 1–4 位十六进制的段，`::` 最多一次，完整写法 8 段，
/// 压缩写法少于 8 段。边界后面不能再跟十六进制、冒号或点，免得把版本号吞进来。
fn ipv6_at(chunk: &[u8], i: usize) -> Option<usize> {
    if i > 0 && (chunk[i - 1].is_ascii_hexdigit() || matches!(chunk[i - 1], b':' | b'.')) {
        return None;
    }
    let mut end = i;
    while end < chunk.len()
        && (chunk[end].is_ascii_hexdigit() || chunk[end] == b':')
    {
        end += 1;
    }
    let text = std::str::from_utf8(&chunk[i..end]).ok()?;
    if !text.contains(':') || text.matches("::").count() > 1 {
        return None;
    }
    let compressed = text.contains("::");
    let parts: Vec<&str> = if compressed {
        let mut halves = text.splitn(2, "::");
        vec![halves.next().unwrap_or(""), halves.next().unwrap_or("")]
    } else {
        vec![text]
    };
    let mut groups = 0usize;
    for part in parts {
        if part.is_empty() {
            continue;
        }
        for group in part.split(':') {
            if group.is_empty() || group.len() > 4 || !group.bytes().all(|b| b.is_ascii_hexdigit()) {
                return None;
            }
            groups += 1;
        }
    }
    if groups == 0 || groups > 8 || (!compressed && groups != 8) {
        return None;
    }
    if chunk.get(end).is_some_and(|b| b.is_ascii_hexdigit() || matches!(b, b':' | b'.')) {
        return None;
    }
    Some(end)
}

// ── 十六进制 ────────────────────────────────────────────────────────────────

/// 从 `i` 起尝试吃一个十六进制字面量，返回结束偏移。
///
/// `0x` / `0X` 前缀后至少一位；裸串至少四位。前后都不许再贴十六进制或字母
/// （否则会把长标识符 / IPv6 段的一半当成数），冒号也挡掉，避免和 IPv6 抢。
/// 32 位十六进制正好是 `u128` 的上限，更长的串不报命中（换算函数同样拒绝）。
fn hex_at(bytes: &[u8], i: usize) -> Option<usize> {
    if i > 0 {
        let p = bytes[i - 1];
        if p.is_ascii_hexdigit() || matches!(p, b'x' | b'X' | b':') {
            return None;
        }
    }
    let prefixed = bytes[i] == b'0' && matches!(bytes.get(i + 1), Some(b'x' | b'X'));
    let start = if prefixed { i + 2 } else { i };
    if start >= bytes.len() || !bytes[start].is_ascii_hexdigit() {
        return None;
    }
    let mut end = start;
    while end < bytes.len() && bytes[end].is_ascii_hexdigit() {
        end += 1;
    }
    let digits = end - start;
    if digits > 32 || (!prefixed && digits < 4) {
        return None;
    }
    if bytes
        .get(end)
        .is_some_and(|b| b.is_ascii_alphanumeric() || *b == b':')
    {
        return None;
    }
    Some(end)
}

/// 把十六进制文本换成 `(十进制, 八进制)` 两串。纯字符串解析，零 IO / 零墙钟。
///
/// `0x` / `0X` 前缀可选；超过 `u128`（32 位十六进制）判越界返回 `None`，
/// 调用方据此画「超出可换算范围」而不是 panic。
pub fn hex_parts(text: &str) -> Option<(String, String)> {
    let t = text.trim();
    let digits = t
        .strip_prefix("0x")
        .or_else(|| t.strip_prefix("0X"))
        .unwrap_or(t);
    if digits.is_empty()
        || digits.len() > 32
        || !digits.bytes().all(|b| b.is_ascii_hexdigit())
    {
        return None;
    }
    let value = u128::from_str_radix(digits, 16).ok()?;
    Some((value.to_string(), format!("{value:o}")))
}

// ── 时间戳 ──────────────────────────────────────────────────────────────────

/// 从 `i` 起尝试吃一个 Unix 时间戳，返回结束偏移。
///
/// 只认 10 位（秒）或 13 位（毫秒）十进制数字，前面可带一个负号。数字串必须
/// 整段结束：11/12 位或 14 位以上一律不算（宁可漏报，也不把长 id 的一半吃掉）。
fn timestamp_at(bytes: &[u8], i: usize) -> Option<usize> {
    let negative = bytes[i] == b'-';
    let start = if negative { i + 1 } else { i };
    if i > 0 && (bytes[i - 1].is_ascii_digit() || bytes[i - 1] == b'-') {
        return None;
    }
    if start >= bytes.len() || !bytes[start].is_ascii_digit() {
        return None;
    }
    let mut end = start;
    while end < bytes.len() && bytes[end].is_ascii_digit() {
        end += 1;
    }
    let n = end - start;
    if n != 10 && n != 13 {
        return None;
    }
    // 后面再贴数字 / 点说明这不是一个独立的时间戳（可能只是长串的一段）。
    if bytes
        .get(end)
        .is_some_and(|b| b.is_ascii_digit() || *b == b'.')
    {
        return None;
    }
    Some(end)
}

/// 把时间戳文本解析成**秒**；13 位按毫秒除以 1000。负号保留。
pub fn parse_timestamp(text: &str) -> Option<i64> {
    let t = text.trim();
    let (negative, digits) = match t.strip_prefix('-') {
        Some(rest) => (true, rest),
        None => (false, t),
    };
    if !digits.bytes().all(|b| b.is_ascii_digit()) || (digits.len() != 10 && digits.len() != 13) {
        return None;
    }
    let value: i64 = digits.parse().ok()?;
    let value = if negative { -value } else { value };
    Some(if digits.len() == 13 { value / 1000 } else { value })
}

/// 把某个**已知的**纪元秒数格式化成可读 UTC；不读墙钟，越界年份返回 `None`。
///
/// 用 civil-from-days 的整数算法（era / day-of-era / year-of-era 逐级拆），
/// 不用 `chrono`、不用 `SystemTime`：同样的秒数在任何机器上得到同一串文本。
pub fn format_utc(epoch_secs: i64) -> Option<String> {
    let days = epoch_secs.div_euclid(86_400);
    let secs = epoch_secs.rem_euclid(86_400);
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097; // 0..146096
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365; // 0..399
    let mut year = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100); // 0..365
    let mp = (5 * doy + 2) / 153; // 0..11
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = mp + if mp < 10 { 3 } else { -9 };
    if month <= 2 {
        year += 1;
    }
    if !(0..=9999).contains(&year) {
        return None;
    }
    let hh = secs / 3600;
    let mm = (secs % 3600) / 60;
    let ss = secs % 60;
    Some(format!(
        "{year:04}-{month:02}-{day:02}T{hh:02}:{mm:02}:{ss:02}Z"
    ))
}

/// 时间戳文本 → 可读 UTC 文本。两步都是纯函数，解析失败或年份越界返回 `None`。
pub fn timestamp_utc(text: &str) -> Option<String> {
    parse_timestamp(text).and_then(format_utc)
}

// ── 压缩包文件名 ────────────────────────────────────────────────────────────

/// 后缀 → 悬停时显示的归档类型。长后缀必须排在短后缀前面（`.tar.gz` 要先于
/// `.gz`），否则 `foo.tar.gz` 会被说成普通 gzip 文件。
const ARCHIVE_SUFFIXES: &[(&str, &str)] = &[
    (".tar.gz", "gzip compressed tar"),
    (".tar.bz2", "bzip2 compressed tar"),
    (".tar.xz", "xz compressed tar"),
    (".tar.zst", "zstd compressed tar"),
    (".tbz2", "bzip2 compressed tar"),
    (".txz", "xz compressed tar"),
    (".tgz", "gzip compressed tar"),
    (".zip", "ZIP archive"),
    (".7z", "7-Zip archive"),
    (".rar", "RAR archive"),
    (".tar", "tar archive"),
    (".gz", "gzip compressed file"),
    (".bz2", "bzip2 compressed file"),
    (".xz", "xz compressed file"),
    (".zst", "zstd compressed file"),
];

/// 文件名是不是已知压缩包；是则返回给人看的归档类型。纯查表，零 IO。
///
/// 后缀前的 stem 至少要有一个字母或数字，`.zip` 这种「只有后缀」不算命中。
pub fn archive_kind(name: &str) -> Option<&'static str> {
    let lower = name.to_ascii_lowercase();
    for (suffix, label) in ARCHIVE_SUFFIXES {
        if let Some(stem) = lower.strip_suffix(suffix) {
            if stem.bytes().any(|b| b.is_ascii_alphanumeric()) {
                return Some(*label);
            }
        }
    }
    None
}

fn is_name_char(b: u8) -> bool {
    b.is_ascii_alphanumeric() || matches!(b, b'/' | b'\\' | b'.' | b'-' | b'_' | b'+' | b'@')
}

/// 从 `i` 起尝试吃一个压缩包文件名，返回结束偏移。
///
/// 只吃「一段没有空白 / 引号的 token」；句末标点不算文件名的一部分。
/// 边界要求 token 前面不是标识符字符，免得从长路径中间起手。
fn archive_at(bytes: &[u8], i: usize) -> Option<usize> {
    if i > 0 && is_name_char(bytes[i - 1]) {
        return None;
    }
    let mut end = i;
    while end < bytes.len()
        && !bytes[end].is_ascii_whitespace()
        && !matches!(bytes[end], b'"' | b'\'' | b'<' | b'>' | b'|' | b';')
    {
        end += 1;
    }
    while end > i && matches!(bytes[end - 1], b',' | b'.' | b')' | b']' | b':') {
        end -= 1;
    }
    let name = std::str::from_utf8(&bytes[i..end]).ok()?;
    archive_kind(name)?;
    Some(end)
}

/// 从一段输出里找所有内置命中。同一位置只报一种：URL > IPv6 > IPv4 >
/// 归档文件名 > 时间戳 > 十六进制。纯函数、零 IO，不写 UI 状态。
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
        if bytes[i].is_ascii_hexdigit() || bytes[i] == b':' {
            if let Some(end) = ipv6_at(bytes, i) {
                out.push(Found {
                    kind: Builtin::Ipv6,
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
        }
        // 归档文件名要先整段吃掉，否则 "foo.tar.gz" 会被拆成 hex / 杂字符。
        if is_name_char(bytes[i]) {
            if let Some(end) = archive_at(bytes, i) {
                out.push(Found {
                    kind: Builtin::Archive,
                    text: String::from_utf8_lossy(&bytes[i..end]).into_owned(),
                    start: i,
                });
                i = end.max(i + 1);
                continue;
            }
        }
        // 10/13 位纯数字归时间戳；更短的纯数字串留给十六进制。
        if bytes[i] == b'-' || bytes[i].is_ascii_digit() {
            if let Some(end) = timestamp_at(bytes, i) {
                out.push(Found {
                    kind: Builtin::Timestamp,
                    text: String::from_utf8_lossy(&bytes[i..end]).into_owned(),
                    start: i,
                });
                i = end.max(i + 1);
                continue;
            }
        }
        // 不带冒号边界的十六进制数（前面几种都没认领时才轮到它）。
        if bytes[i].is_ascii_hexdigit() {
            if let Some(end) = hex_at(bytes, i) {
                out.push(Found {
                    kind: Builtin::Hex,
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
    fn detects_ipv6_in_compressed_and_full_forms() {
        for text in ["::1", "2001:db8::1", "fe80::1%lo0"] {
            let found = detect(text);
            assert!(
                found.iter().any(|f| f.kind == Builtin::Ipv6),
                "{text} 应识别为 IPv6：{found:?}",
            );
        }
        let full = detect("2001:0db8:85a3:0000:0000:8a2e:0370:7334");
        assert_eq!(full.len(), 1);
        assert_eq!(full[0].kind, Builtin::Ipv6);
        assert!(detect("1:2:3").is_empty(), "段数不够不是 IPv6");
        assert!(detect("2001:db8:::1").is_empty(), "三冒号不合法");
    }

    #[test]
    fn detects_ipv4_but_not_version_numbers() {
        let found = detect("host 10.0.0.9 ready, v1.2.3.4.5");
        assert_eq!(found.len(), 1, "五段版本号不是 IPv4");
        assert_eq!(found[0].kind, Builtin::Ipv4);
        assert_eq!(found[0].text, "10.0.0.9");
        assert!(detect("256.1.1.1").is_empty(), "越界字节不是 IPv4");
    }

    #[test]
    fn hex_converts_prefixed_and_bare_literals() {
        assert_eq!(hex_parts("0xFF"), Some(("255".into(), "377".into())));
        assert_eq!(hex_parts("0X2a"), Some(("42".into(), "52".into())));
        assert_eq!(hex_parts("deadbeef"), Some(("3735928559".into(), "33653337357".into())));
        assert_eq!(hex_parts("1234"), Some(("4660".into(), "11064".into())));
        assert_eq!(hex_parts("0x0"), Some(("0".into(), "0".into())));

        let found = detect("value=0xFF and deadbeef");
        assert_eq!(found.len(), 2, "{found:?}");
        assert_eq!(found[0].kind, Builtin::Hex);
        assert_eq!(found[0].text, "0xFF");
        assert_eq!(found[0].start, 6);
        assert_eq!(found[1].text, "deadbeef");
    }

    #[test]
    fn hex_rejects_non_hex_and_out_of_range() {
        assert_eq!(hex_parts("0xGG"), None);
        assert_eq!(hex_parts(""), None);
        assert_eq!(hex_parts("xyz123"), None);
        assert_eq!(hex_parts("123456789012345678901234567890123"), None, "超过 u128");
        assert!(detect("0xGG abcz xyz123").is_empty(), "非 hex 与短串不误报");
        // IPv6 形状的串不能被 hex 抢走（冒号是拒绝边界）。
        assert!(detect("2001:db8:::1").is_empty());
    }

    #[test]
    fn timestamp_converts_seconds_and_milliseconds_to_utc() {
        assert_eq!(format_utc(0), Some("1970-01-01T00:00:00Z".into()));
        assert_eq!(timestamp_utc("1700000000"), Some("2023-11-14T22:13:20Z".into()));
        assert_eq!(timestamp_utc("1700000000000"), Some("2023-11-14T22:13:20Z".into()));
        assert_eq!(parse_timestamp("-1"), None, "位数不对不是时间戳");

        let found = detect("at 1700000000 ms 1700000000000");
        assert_eq!(found.len(), 2, "{found:?}");
        assert!(found.iter().all(|f| f.kind == Builtin::Timestamp));
        assert_eq!(found[1].start, 17);
    }

    #[test]
    fn timestamp_handles_negative_and_rejects_out_of_range() {
        assert_eq!(parse_timestamp("-1"), None, "负数也必须是 10/13 位");
        assert_eq!(timestamp_utc("-1700000000"), Some("1916-02-18T01:46:40Z".into()));
        let found = detect("t=-1700000000 done");
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].text, "-1700000000");
        assert_eq!(parse_timestamp("17000000000"), None, "11 位不算");
        assert_eq!(parse_timestamp("170000000000000"), None, "15 位不算");
        assert_eq!(parse_timestamp("123456789a"), None, "非数字不算");
        assert_eq!(format_utc(i64::MAX), None, "年份越界返回 None，不 panic");
        assert_eq!(format_utc(i64::MIN), None);
    }

    #[test]
    fn archive_recognizes_common_formats_and_ignores_unknown() {
        for (name, kind) in [
            ("a.zip", "ZIP archive"),
            ("build.tar.gz", "gzip compressed tar"),
            ("src.tgz", "gzip compressed tar"),
            ("data.tar.bz2", "bzip2 compressed tar"),
            ("data.tar.xz", "xz compressed tar"),
            ("disk.img.7z", "7-Zip archive"),
            ("old.rar", "RAR archive"),
            ("/tmp/RELEASE.ZIP", "ZIP archive"),
        ] {
            assert_eq!(archive_kind(name), Some(kind), "{name}");
        }
        assert_eq!(archive_kind("notes.txt"), None);
        assert_eq!(archive_kind("archive.unknown"), None);
        assert_eq!(archive_kind(".zip"), None, "只有后缀不算文件名");
        assert!(archive_kind("file.zip.bak").is_none(), "后缀不在末尾不算");

        let found = detect("saved build/out.tar.gz and notes.txt");
        assert_eq!(found.len(), 1, "{found:?}");
        assert_eq!(found[0].kind, Builtin::Archive);
        assert_eq!(found[0].text, "build/out.tar.gz");
        assert_eq!(found[0].start, 6);
        let with_dot = detect("Got report.zip.");
        assert_eq!(with_dot[0].text, "report.zip", "句末句点不属于文件名");
    }

    #[test]
    fn detect_keeps_priority_and_byte_offsets_on_a_mixed_line() {
        let found = detect("url https://x.io/a ip 10.0.0.9 hex 0x2A ts 1700000000 zip a.zip");
        let kinds: Vec<_> = found.iter().map(|f| f.kind).collect();
        assert_eq!(
            kinds,
            vec![
                Builtin::Url,
                Builtin::Ipv4,
                Builtin::Hex,
                Builtin::Timestamp,
                Builtin::Archive,
            ],
            "{found:?}",
        );
        for f in &found {
            assert_eq!(&f.text, &"url https://x.io/a ip 10.0.0.9 hex 0x2A ts 1700000000 zip a.zip"[f.start..f.start + f.text.len()]);
        }
    }
}

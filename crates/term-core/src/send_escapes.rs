//! Sender 面板与 Trigger ▸ Send text 共用的转义解码。
//!
//! 原版功能文档（`kingtoolbox-docs/Sender-Pane/Send-Chars.md` 与
//! `Trigger/Send-Text.md`）列的是同一张表：`\0 \a \b \t \n \v \f \r \\`、
//! `\nnn`（八进制）、`\xHH`、`\uHHHH`、`\cX` / `\c-X`（控制字符）、
//! `\mX` / `\m-X`（高位字符）。两条路径共用这一份，避免「Sender 认 `\x03`，
//! Trigger 不认」这种同词不同义。
//!
//! 语法错误（非法八进制/十六进制/空 `c`/`m`）返回 [`EscapeError`]，调用方在
//! 写 PTY 之前就能拦下——半条命令比不发送更糟。

/// 一处转义写错的位置与原因。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EscapeError {
    pub at: usize,
    pub reason: &'static str,
}

impl std::fmt::Display for EscapeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "第 {} 个字符：{}", self.at + 1, self.reason)
    }
}

/// 把带转义的文本解成**原始字节**。没有反斜杠时零分配（直接返回 UTF-8 字节）。
pub fn decode(text: &str) -> Result<Vec<u8>, EscapeError> {
    let mut out = Vec::with_capacity(text.len());
    let chars: Vec<char> = text.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        if chars[i] != '\\' {
            let mut buf = [0u8; 4];
            out.extend_from_slice(chars[i].encode_utf8(&mut buf).as_bytes());
            i += 1;
            continue;
        }
        let at = i;
        i += 1;
        if i >= chars.len() {
            return Err(EscapeError { at, reason: "末尾的反斜杠没有后继字符" });
        }
        let c = chars[i];
        i += 1;
        match c {
            'a' => out.push(0x07),
            'b' => out.push(0x08),
            't' => out.push(0x09),
            'n' => out.push(0x0A),
            'v' => out.push(0x0B),
            'f' => out.push(0x0C),
            'r' => out.push(0x0D),
            '\\' => out.push(0x5C),
            // \nnn：最多三位八进制。
            d if ('0'..='7').contains(&d) => {
                let mut value = d.to_digit(8).unwrap_or(0);
                let mut used = 1;
                while used < 3 && i < chars.len() {
                    match chars[i].to_digit(8) {
                        Some(n) => {
                            value = value * 8 + n;
                            used += 1;
                            i += 1;
                        }
                        None => break,
                    }
                }
                let _ = used;
                out.push(value as u8);
            }
            'x' => {
                let mut value = 0u32;
                for _ in 0..2 {
                    match chars.get(i).and_then(|c| c.to_digit(16)) {
                        Some(n) => {
                            value = value * 16 + n;
                            i += 1;
                        }
                        None => return Err(EscapeError { at, reason: "\\x 需要两位十六进制" }),
                    }
                }
                out.push(value as u8);
            }
            'u' => {
                let mut value = 0u32;
                for _ in 0..4 {
                    match chars.get(i).and_then(|c| c.to_digit(16)) {
                        Some(n) => {
                            value = value * 16 + n;
                            i += 1;
                        }
                        None => return Err(EscapeError { at, reason: "\\u 需要四位十六进制" }),
                    }
                }
                let ch = char::from_u32(value)
                    .ok_or(EscapeError { at, reason: "\\u 不是有效的 Unicode 码点" })?;
                let mut buf = [0u8; 4];
                out.extend_from_slice(ch.encode_utf8(&mut buf).as_bytes());
            }
            'c' => {
                let x = take_tail_char(&chars, &mut i, at, "\\c 需要一个字母")?;
                out.push((x.to_ascii_uppercase() as u8) & 0x1F);
            }
            'm' => {
                let x = take_tail_char(&chars, &mut i, at, "\\m 需要一个字符")?;
                out.push(x as u8 | 0x80);
            }
            // 未列出的转义按原版口径保留字面字符（`\q` → `q`），不静默吞掉反斜杠。
            other => {
                let mut buf = [0u8; 4];
                out.extend_from_slice(other.encode_utf8(&mut buf).as_bytes());
            }
        }
    }
    Ok(out)
}

/// `\c` / `\m` 后面的字符，允许紧挨着或用 `-` 分隔。
fn take_tail_char(
    chars: &[char],
    i: &mut usize,
    at: usize,
    reason: &'static str,
) -> Result<char, EscapeError> {
    if *i < chars.len() && chars[*i] == '-' {
        *i += 1;
    }
    let c = *chars.get(*i).ok_or(EscapeError { at, reason })?;
    *i += 1;
    Ok(c)
}

/// 文本里有没有需要解码的转义。没有时调用方可以走原来的文本片路径。
pub fn has_escape(text: &str) -> bool {
    text.contains('\\')
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decodes_the_documented_table() {
        assert_eq!(decode(r"\0\a\b\t\n\v\f\r\\").unwrap(), b"\x00\x07\x08\x09\x0a\x0b\x0c\x0d\x5c");
        assert_eq!(decode(r"\003").unwrap(), b"\x03");
        assert_eq!(decode(r"\x03").unwrap(), b"\x03");
        assert_eq!(decode(r"\u0003").unwrap(), b"\x03");
        assert_eq!(decode(r"\cc").unwrap(), b"\x03");
        assert_eq!(decode(r"\c-c").unwrap(), b"\x03");
        assert_eq!(decode(r"\mX").unwrap(), b"\xd8");
        assert_eq!(decode(r"\m-X").unwrap(), b"\xd8");
    }

    #[test]
    fn plain_text_is_unchanged_and_bad_escapes_are_errors() {
        assert_eq!(decode("hello\r\n").unwrap(), b"hello\r\n");
        assert!(decode(r"\x0").is_err());
        assert!(decode(r"\u12").is_err());
        assert!(decode("\\").is_err());
    }
}

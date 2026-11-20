//! WSH `SendKeys` 语法解析（SecureCRT `Screen.SendKeys`）。
//!
//! # 为什么需要它
//!
//! CRT 的用户手册里，`SendKeys` 是最常被复制的一行：
//!
//! ```text
//! crt.Screen.SendKeys("^C")        ' Ctrl+C
//! crt.Screen.SendKeys("{F7}")      ' F7
//! crt.Screen.SendKeys("+{TAB}")    ' Shift+Tab
//! crt.Screen.SendKeys("ls~")       ' 输入 ls 并回车
//! ```
//!
//! 我们的 `keys()` 收的是**键名**（`ctrl+c` / `f7` / `shift+tab`），
//! `write()` 收的是字节。两者都不认这套前缀语法，所以补一个纯解析器：
//! 解析结果是「一段文本」与「一个键名」的序列，发送时分别走
//! [`crate::ScriptSession::write`] 与 [`crate::ScriptSession::keys`]。
//!
//! # 语法（WSH 子集，够覆盖 CRT 手册里的例子）
//!
//! | 写法 | 含义 |
//! |---|---|
//! | `^X` / `%X` / `+X` | Ctrl / Alt / Shift + 单字符 |
//! | `^a` … `^z` | 同上（字母大小写都接受） |
//! | `~` | 回车 |
//! | `{ENTER}` `{TAB}` `{ESC}` `{BACKSPACE}` `{DEL}` `{UP}` … `{F12}` | 命名键 |
//! | `{^}` `{%}` `{+}` `{~}` `{{}` `{}}` `{(}` `{)}` `{[}` `{]}` | 字面字符 |
//! | 其它字符 | 原样文本 |
//!
//! # 不做的事
//!
//! - 不组合多个修饰键（`^%X` 这种三键组合）：CRT 的手册里也没有，而我们的
//!   键表只到「一个修饰 + 一个键」。遇到就**报错**，不猜一个近似键。
//! - 不解释 `{PAUSE}` / `{SLEEP}`：等待是脚本自己的事（`sleep(ms)`）。

use crate::sendkeys::Stroke::{Key, Text};

/// 一次按键：要么是文本，要么是一个键名（与 `term_core::keys` 同一套）。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Stroke {
    /// 原样写出的文本（已合并相邻字面字符）。
    Text(String),
    /// 键名（`ctrl+c` / `f7` / `shift+tab` / `enter`）。
    Key(String),
}

/// 解析 `SendKeys` 表达式。
///
/// 错误一律带**偏移**（第几个字符）：脚本里 `SendKeys("^C{F99}")` 的错在
/// 哪一格，报错要能指出来。
pub fn parse(expr: &str) -> Result<Vec<Stroke>, &'static str> {
    let chars: Vec<char> = expr.chars().collect();
    let mut out: Vec<Stroke> = Vec::new();
    let mut literal = String::new();
    let mut i = 0;

    let flush = |literal: &mut String, out: &mut Vec<Stroke>| {
        if !literal.is_empty() {
            out.push(Text(std::mem::take(literal)));
        }
    };

    while i < chars.len() {
        let c = chars[i];
        match c {
            '^' | '%' | '+' => {
                // 修饰符必须跟一个键：单字符或 `{NAME}`。
                let Some(next) = chars.get(i + 1).copied() else {
                    return Err("SendKeys：修饰符后面缺少按键（^ 后要跟一个字符或 {键名}）");
                };
                let modifier = match c {
                    '^' => "ctrl",
                    '%' => "alt",
                    _ => "shift",
                };
                flush(&mut literal, &mut out);
                if next == '{' {
                    let (name, used) = take_braced(&chars[i + 2..])?;
                    out.push(Key(combine(modifier, &name)?));
                    i += 2 + used;
                } else if next == '}' {
                    return Err("SendKeys：多了一个 }");
                } else if "^%~+".contains(next) {
                    return Err("SendKeys：不支持多个修饰键组合（^%X 这类），请拆成两次发送");
                } else {
                    out.push(Key(combine(modifier, &next.to_string())?));
                    i += 2;
                }
            }
            '~' => {
                flush(&mut literal, &mut out);
                out.push(Key("enter".to_string()));
                i += 1;
            }
            '{' => {
                let (name, used) = take_braced(&chars[i + 1..])?;
                match literal_escape(&name) {
                    // 字面字符先攒进 `literal`，**不**先 flush：`a{{}b` 要合
                    // 成一段 `a{b`，否则会被拆成 `a` + `{b` 两段。
                    Some(text) => literal.push_str(text),
                    None => {
                        flush(&mut literal, &mut out);
                        out.push(Key(normalize_name(&name)?));
                    }
                }
                i += 1 + used;
            }
            '}' => return Err("SendKeys：多了一个 }（字面 } 要写 {}}）"),
            ')' | ']' => {
                // WSH 里 ) ] 只在「括号组」里有意义；CRT 的例子里没人用组，
                // 直接当字面字符更接近用户预期。
                literal.push(c);
                i += 1;
            }
            '(' | '[' => {
                literal.push(c);
                i += 1;
            }
            other => {
                literal.push(other);
                i += 1;
            }
        }
    }
    flush(&mut literal, &mut out);
    Ok(out)
}

/// 读 `{NAME}`（`chars[0]` 是 `{` 之后那一个）。返回名字与消耗的字符数（含 `}`）。
fn take_braced(chars: &[char]) -> Result<(String, usize), &'static str> {
    let mut name = String::new();
    for (index, ch) in chars.iter().enumerate() {
        if *ch == '}' {
            if name.is_empty() {
                return Err("SendKeys：空的 {} 没有意义");
            }
            return Ok((name, index + 1));
        }
        name.push(*ch);
    }
    Err("SendKeys：{ 没有闭合（缺 }）")
}

/// `{^}` 这类转义返回字面字符。
fn literal_escape(name: &str) -> Option<&'static str> {
    match name {
        "^" => Some("^"),
        "%" => Some("%"),
        "+" => Some("+"),
        "~" => Some("~"),
        "{" => Some("{"),
        "}" => Some("}"),
        "(" => Some("("),
        ")" => Some(")"),
        "[" => Some("["),
        "]" => Some("]"),
        _ => None,
    }
}

/// 命名键 → 我们的键名。未知键名显式报错（不悄悄当文本发出去）。
fn normalize_name(name: &str) -> Result<String, &'static str> {
    let lower = name.to_ascii_lowercase();
    let mapped = match lower.as_str() {
        "enter" | "return" => "enter",
        "tab" => "tab",
        "esc" | "escape" => "esc",
        "backspace" | "bs" => "backspace",
        "del" | "delete" => "delete",
        "ins" | "insert" => "insert",
        "home" => "home",
        "end" => "end",
        "pgup" | "pageup" => "pageup",
        "pgdn" | "pagedown" => "pagedown",
        "up" => "up",
        "down" => "down",
        "left" => "left",
        "right" => "right",
        f if f.starts_with('f') && f[1..].parse::<u8>().is_ok() => {
            let n: u8 = f[1..].parse().map_err(|_| "SendKeys：功能键编号有误")?;
            if (1..=12).contains(&n) {
                return Ok(format!("f{n}"));
            }
            return Err("SendKeys：只支持 F1…F12");
        }
        _ => return Err("SendKeys：认不出的键名（{ENTER} {TAB} {ESC} {F1}…{F12} 之类）"),
    };
    Ok(mapped.to_string())
}

/// 修饰符 + 基键 → 键名。只允许我们的键表支持的组合。
fn combine(modifier: &str, base: &str) -> Result<String, &'static str> {
    let lower = base.to_ascii_lowercase();
    match modifier {
        "ctrl" => {
            // 键表只有 ctrl+a..z。
            let mut chars = lower.chars();
            match (chars.next(), chars.next()) {
                (Some(c), None) if c.is_ascii_alphabetic() => Ok(format!("ctrl+{c}")),
                _ => Err("SendKeys：Ctrl 只支持 ctrl+a…ctrl+z"),
            }
        }
        "alt" => {
            let mut chars = lower.chars();
            match (chars.next(), chars.next()) {
                (Some(c), None) => Ok(format!("alt+{c}")),
                _ => Err("SendKeys：Alt 只支持 alt+<单个字符>"),
            }
        }
        // 键表里只有 shift+tab 这一种组合。
        _ => match lower.as_str() {
            "tab" => Ok("shift+tab".to_string()),
            _ => Err("SendKeys：Shift 只支持 +{TAB}"),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ctrl_letters_become_ctrl_key_names() {
        assert_eq!(parse("^C").expect("合法"), vec![Key("ctrl+c".into())]);
        assert_eq!(parse("^d").expect("合法"), vec![Key("ctrl+d".into())]);
    }

    #[test]
    fn named_keys_and_modifiers_are_translated() {
        assert_eq!(parse("{F7}").expect("合法"), vec![Key("f7".into())]);
        assert_eq!(parse("+{TAB}").expect("合法"), vec![Key("shift+tab".into())]);
        assert_eq!(parse("~").expect("合法"), vec![Key("enter".into())]);
        assert_eq!(parse("{ESC}").expect("合法"), vec![Key("esc".into())]);
        assert_eq!(parse("%x").expect("合法"), vec![Key("alt+x".into())]);
    }

    #[test]
    fn literal_text_is_merged_and_specials_escaped() {
        assert_eq!(
            parse("ls -l~").expect("合法"),
            vec![Text("ls -l".into()), Key("enter".into())]
        );
        assert_eq!(parse("{^}").expect("合法"), vec![Text("^".into())]);
        assert_eq!(
            parse("a{{}b").expect("合法"),
            vec![Text("a{b".into())],
            "字面字符与相邻文本合并，不拆成三段"
        );
    }

    #[test]
    fn malformed_expressions_are_errors_not_text() {
        for bad in ["^", "{", "}", "a}b", "{}", "^%X", "{F99}", "{NOPE}", "%{F4}"] {
            assert!(parse(bad).is_err(), "{bad:?} 应当报错");
        }
    }
}

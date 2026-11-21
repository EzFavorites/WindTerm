//! 键名 → 送往 PTY 的字节。
//!
//! # 为什么这是一张表而不是 UI 里的一段 match
//!
//! 界面把它收到的键盘事件写下字节（`apps/desktop/src/ui/terminal/pane.rs`），
//! 脚本与远程客户端则要**按名字**按下同一个键（`keys(["enter"])`）。
//! 两处各写一份「Enter 是 CR 还是 LF」「F5 是 `ESC[15~` 还是 `ESC O t`」，
//! 就会有两种终端：界面里跑得通的操作，脚本里发给程序的是别的字节，而且
//! **不报错**——程序只是收到一个它不认识的序列，然后不动。
//!
//! 所以映射表只有这一份，放在纯语义层（这台编译单元零 IO、零线程、零墙钟）。
//!
//! # 与 `DECSET 1` 的关系
//!
//! 只有**方向键**受 `application_cursor_keys` 影响（`ESC[A` ↔ `ESC OA`）。
//! 其余键名的字节与终端状态无关，因此 [`key_bytes`] 只在这一个参数上分叉：
//! 调用方不需要为了按一次 `enter` 去问屏幕。方向键的编码复用
//! [`crate::vt::arrow_bytes_for`]，与界面那条路是同一个函数。
//!
//! # 写法
//!
//! 键名大小写不敏感，`-` / `_` / `+` 视作同一个分隔符：`ctrl+c`、`Ctrl-C`、
//! `ctrl_c` 都是同一个键。别名只收公认的几组（`enter`/`return`、`esc`/`escape`、
//! `pgup`/`pageup`…），不做「猜你想按哪个键」——认不出来就报错，由调用方
//! 改键名。静默丢弃一个键名会让脚本在后面某一步等一个永远不会来的提示。

use crate::vt::{arrow_bytes_for, Arrow};

/// 这个键名是不是方向键（决定要不要问 `DECSET 1`）。
///
/// 调用方（桥接、脚本）据此决定「一次按键要不要多取一次屏幕」。
pub fn is_arrow(name: &str) -> bool {
    matches!(normalize(name).as_str(), "up" | "down" | "left" | "right")
}

/// `Session ▾ Keyboard ▸ Backspace sends:` 的三种编码。
///
/// `0` = Automatic，也就是 xterm / readline 通用的 0x7f（DEL）；
/// `1` = 0x08（BS）；`2` = `ESC [ 3 ~`（Delete 键序列）。
/// 越界一律回落 0，与设置页存的下标口径一致。
pub fn backspace_bytes(mode: usize) -> &'static str {
    match mode {
        1 => "\x08",
        2 => "\x1b[3~",
        _ => "\x7f",
    }
}

/// `Session ▾ Keyboard ▸ Delete sends:` 的三种编码。
///
/// `0` = Automatic（0x7f 之外另一半：Delete 键序列 `ESC [ 3 ~`）；
/// `1` = 0x08（BS）；`2` = 0x7f（DEL）。
pub fn delete_bytes(mode: usize) -> &'static str {
    match mode {
        1 => "\x08",
        2 => "\x7f",
        _ => "\x1b[3~",
    }
}

/// 键名 → 字节。认不出来返回 `None`（调用方报参数错，不要静默发空串）。
///
/// `application_cursor_keys` 只影响方向键，其余键名照旧。
pub fn key_bytes(name: &str, application_cursor_keys: bool) -> Option<String> {
    let name = normalize(name);
    if let Some(fixed) = fixed(&name) {
        return Some(fixed.to_string());
    }
    if let Some(arrow) = arrow_of(&name) {
        return Some(arrow_bytes_for(arrow, application_cursor_keys).to_string());
    }
    control(&name).or_else(|| alt(&name))
}

/// 全部可用的键名（报错文案用）。刻意写成一行可读的清单，不是机器清单：
/// 脚本作者看到的是「能按哪些键」，不是 enum 变体名。
pub fn known_names() -> &'static str {
    "enter / tab / space / esc / backspace / delete / insert / home / end / \
     pageup / pagedown / up / down / left / right / f1..f12 / shift+tab / \
     ctrl+a..ctrl+z（含 ctrl+space ctrl+[ ctrl+\\ ctrl+] ctrl+^ ctrl+_）/ alt+<单字符>"
}

/// 小写 + 分隔符归一（`-`/`_`/`+` → `+`）+ 去空白。
///
/// 只归一**分隔符**，不动别的字符：`ctrl++` 这种写不出来，`alt++` 要按 `+` 键
/// 本身——归一之后剩 `alt+` 再加一个 `+`，所以 `alt++` 里的第二个 `+` 会被
/// 当成键值本身（见 [`alt`]），不会变成一个空键名。
fn normalize(name: &str) -> String {
    let lowered = name.trim().to_ascii_lowercase();
    let mut out = String::with_capacity(lowered.len());
    for ch in lowered.chars() {
        match ch {
            '-' | '_' => out.push('+'),
            _ => out.push(ch),
        }
    }
    out
}

/// 与终端状态无关的固定键。
fn fixed(name: &str) -> Option<&'static str> {
    Some(match name {
        "enter" | "return" | "cr" => "\r",
        "tab" => "\t",
        "shift+tab" | "backtab" => "\x1b[Z",
        "space" => " ",
        "esc" | "escape" => "\x1b",
        // 退格键发 DEL 而不是 BS：xterm / Linux 控制台 / 所有 readline 用法
        // 都按 0x7f 理解 `Backspace`；发 0x08 会变成 ^H 插入。
        "backspace" | "bs" => "\x7f",
        "delete" | "del" => "\x1b[3~",
        "insert" | "ins" => "\x1b[2~",
        "home" => "\x1b[H",
        "end" => "\x1b[F",
        "pageup" | "pgup" | "prior" => "\x1b[5~",
        "pagedown" | "pgdn" | "next" => "\x1b[6~",
        // F1..F4 走 SS3（`ESC O x`），F5 起走 CSI（`ESC [ nn ~`）——
        // 这是 xterm 的既有约定，不是随意分配。
        "f1" => "\x1bOP",
        "f2" => "\x1bOQ",
        "f3" => "\x1bOR",
        "f4" => "\x1bOS",
        "f5" => "\x1b[15~",
        "f6" => "\x1b[17~",
        "f7" => "\x1b[18~",
        "f8" => "\x1b[19~",
        "f9" => "\x1b[20~",
        "f10" => "\x1b[21~",
        "f11" => "\x1b[23~",
        "f12" => "\x1b[24~",
        _ => return None,
    })
}

fn arrow_of(name: &str) -> Option<Arrow> {
    Some(match name {
        "up" => Arrow::Up,
        "down" => Arrow::Down,
        "left" => Arrow::Left,
        "right" => Arrow::Right,
        _ => return None,
    })
}

/// `ctrl+<键>` → C0 控制字符（`ctrl+a` = 0x01 … `ctrl+z` = 0x1a）。
///
/// 除字母外只收这几个公认的：`ctrl+space` / `ctrl+@`（NUL）、`ctrl+[`（ESC）、
/// `ctrl+\`（FS）、`ctrl+]`（GS）、`ctrl+^`（RS）、`ctrl+_`（US）。
/// 不臆造 `ctrl+enter` 之类的组合——那些在终端协议里没有统一定义，
/// 猜一个出来只会让程序收到错字节。
fn control(name: &str) -> Option<String> {
    let rest = name.strip_prefix("ctrl+")?;
    let byte = match rest {
        "space" | "@" => 0x00,
        "[" => 0x1b,
        "\\" => 0x1c,
        "]" => 0x1d,
        "^" => 0x1e,
        "_" => 0x1f,
        "?" => 0x7f,
        _ => {
            let mut chars = rest.chars();
            let ch = chars.next()?;
            if chars.next().is_some() || !ch.is_ascii_lowercase() {
                return None;
            }
            (ch as u8) - b'a' + 1
        }
    };
    Some((byte as char).to_string())
}

/// `alt+<单字符>` → `ESC` + 该字符（Meta 键的既有约定）。
///
/// 只收**单个字符**：`alt+enter` 这类组合在终端里没有统一编码，与其编一个
/// 出来，不如报错让脚本用 `write` 自己写清楚。
fn alt(name: &str) -> Option<String> {
    let rest = name.strip_prefix("alt+").or_else(|| name.strip_prefix("meta+"))?;
    let mut chars = rest.chars();
    let ch = chars.next()?;
    if chars.next().is_some() {
        return None;
    }
    let mut out = String::with_capacity(ch.len_utf8() + 1);
    out.push('\x1b');
    out.push(ch);
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backspace_and_delete_have_three_wire_formats() {
        // 0 档是接线前的行为：Backspace 发 DEL，Delete 发 CSI 3~。
        assert_eq!(backspace_bytes(0), "\x7f");
        assert_eq!(delete_bytes(0), "\x1b[3~");
        assert_eq!(backspace_bytes(1), "\x08");
        assert_eq!(backspace_bytes(2), "\x1b[3~");
        assert_eq!(delete_bytes(1), "\x08");
        assert_eq!(delete_bytes(2), "\x7f");
        // 手改配置写了越界值：与 0 档同，不猜、不 panic。
        assert_eq!(backspace_bytes(9), "\x7f");
        assert_eq!(delete_bytes(9), "\x1b[3~");
    }

    #[test]
    fn enter_is_cr_not_lf() {
        // 终端的「回车」是 CR。发 LF 在 cooked 模式碰巧也能跑，在 raw 模式
        // （vim / less / ssh 的密码提示）就是插一个 ^J——脚本会看起来「卡住」。
        assert_eq!(key_bytes("enter", false).as_deref(), Some("\r"));
        assert_eq!(key_bytes("return", false).as_deref(), Some("\r"));
    }

    #[test]
    fn arrows_follow_decset_1() {
        assert_eq!(key_bytes("up", false).as_deref(), Some("\x1b[A"));
        assert_eq!(key_bytes("up", true).as_deref(), Some("\x1bOA"));
        assert_eq!(key_bytes("left", true).as_deref(), Some("\x1bOD"));
    }

    #[test]
    fn only_arrows_care_about_the_mode() {
        // 非方向键的字节必须与 DECSET 1 无关，否则调用方每次按键都得先问屏幕。
        for name in ["enter", "tab", "esc", "f5", "ctrl+c"] {
            assert_eq!(
                key_bytes(name, false),
                key_bytes(name, true),
                "{name} 不该受 application_cursor_keys 影响"
            );
        }
    }

    #[test]
    fn control_letters_map_to_c0() {
        assert_eq!(key_bytes("ctrl+c", false).as_deref(), Some("\x03"));
        assert_eq!(key_bytes("ctrl+a", false).as_deref(), Some("\x01"));
        assert_eq!(key_bytes("ctrl+z", false).as_deref(), Some("\x1a"));
        assert_eq!(key_bytes("ctrl+space", false).as_deref(), Some("\0"));
    }

    #[test]
    fn names_are_case_and_separator_insensitive() {
        assert_eq!(key_bytes("Ctrl+C", false), key_bytes("ctrl+c", false));
        assert_eq!(key_bytes("ctrl_c", false), key_bytes("ctrl+c", false));
        assert_eq!(key_bytes(" Enter ", false).as_deref(), Some("\r"));
    }

    #[test]
    fn unknown_names_are_refused_instead_of_becoming_text() {
        // 认不出来就报错：静默把它当文本写出去，脚本会以为按了键。
        assert_eq!(key_bytes("ctrl+enter", false), None);
        assert_eq!(key_bytes("f13", false), None);
        assert_eq!(key_bytes("a", false), None);
        assert_eq!(key_bytes("", false), None);
    }

    #[test]
    fn alt_wraps_a_single_character() {
        assert_eq!(key_bytes("alt+b", false).as_deref(), Some("\x1bb"));
        assert_eq!(key_bytes("meta+b", false).as_deref(), Some("\x1bb"));
        // 两个字符的组合没有统一定义，不猜。
        assert_eq!(key_bytes("alt+enter", false), None);
    }

    #[test]
    fn known_names_mention_the_families_a_script_author_needs() {
        for needle in ["enter", "tab", "esc", "f1..f12", "ctrl+a..ctrl+z", "alt+"] {
            assert!(known_names().contains(needle), "清单里少了 {needle}");
        }
    }

    #[test]
    fn is_arrow_matches_exactly_the_four() {
        for name in ["up", "Down", "LEFT", "right"] {
            assert!(is_arrow(name), "{name} 应当算方向键");
        }
        for name in ["enter", "f1", "ctrl+up"] {
            assert!(!is_arrow(name), "{name} 不该算方向键");
        }
    }
}

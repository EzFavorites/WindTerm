//! 按**显示宽度**补空格。
//!
//! 中文一个字符占两列，`format!("{:<12}")` 数的是字符数，于是
//! 「平台」只补 10 个空格、「$SHELL」补 6 个，两行看起来是歪的。
//! 这层就这么点事，但散在各处写就会各歪各的。

use term_core::width::is_wide;

/// 这个字符串打印出来占几列。
pub fn display_width(s: &str) -> usize {
    s.chars().map(|c| if is_wide(c) { 2 } else { 1 }).sum()
}

/// 补到至少 `width` 列（已经更宽就不截断——信息比对齐重要）。
pub fn pad(s: &str, width: usize) -> String {
    let w = display_width(s);
    let mut out = String::from(s);
    for _ in w..width {
        out.push(' ');
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ascii_pads_by_char_count() {
        assert_eq!(pad("abc", 6), "abc   ");
        assert_eq!(display_width("abc"), 3);
    }

    #[test]
    fn wide_characters_take_two_columns() {
        assert_eq!(display_width("平台"), 4);
        // 4 列之后再补 4 个空格就是 8 列。
        assert_eq!(pad("平台", 8), "平台    ");
        assert_eq!(display_width(&pad("平台", 8)), 8);
        assert_eq!(display_width(&pad("$SHELL", 8)), 8);
    }

    #[test]
    fn a_value_wider_than_the_column_is_left_alone() {
        assert_eq!(pad("abcdef", 3), "abcdef");
    }
}

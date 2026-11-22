//! Trigger 的 `Script` 模式最小求值：只支持 `return "..."` 字符串字面量。
//!
//! 没有 JS 引擎，也不假装执行任意脚本：认不出来的内容原样当文本发，
//! 绝不静默丢。这样「返回一段文本」这条文档承诺有可验证的后端行为，
//! 而真正需要 JS 的场景仍由外部 RunCommand / 脚本入口承担。

/// 求值一段 `Script` 模式的文本。`${match}` 由调用方先替换好。
pub fn eval(script: &str) -> String {
    let trimmed = script.trim();
    let Some(rest) = trimmed.strip_prefix("return") else {
        return script.to_string();
    };
    let rest = rest.trim();
    if rest.len() < 2 {
        return script.to_string();
    }
    let quote = rest.as_bytes()[0];
    if (quote == b'"' || quote == b'\'') && rest.as_bytes()[rest.len() - 1] == quote {
        return rest[1..rest.len() - 1].to_string();
    }
    script.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_string_return_becomes_the_text_to_send() {
        assert_eq!(eval(r#"return "hello""#), "hello");
        assert_eq!(eval("return 'world'"), "world");
    }

    #[test]
    fn unknown_scripts_are_passed_through_not_silently_dropped() {
        assert_eq!(eval("echo hi"), "echo hi");
        assert_eq!(eval("return 42"), "return 42");
        assert_eq!(eval("return"), "return");
    }
}

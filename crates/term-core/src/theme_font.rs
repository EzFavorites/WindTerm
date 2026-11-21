//! 主题文件里可选的字体声明。
//!
//! 主题主体只有调色板；这一小块是「Prefer theme fonts」开关真正能用的数据。
//! 放在独立文件是为了让 `theme.rs` 守住单文件行数门禁。

use serde::Deserialize;

#[derive(Deserialize)]
pub(crate) struct ThemeFont {
    pub(crate) family: Option<String>,
    pub(crate) weight: Option<u16>,
}

/// 从**已去掉注释**的主题 JSON 里取字体。解析失败 = 没声明。
pub(crate) fn parse(stripped: &str) -> (Option<String>, Option<u16>) {
    #[derive(Deserialize)]
    struct File {
        font: Option<ThemeFont>,
    }
    let Ok(file) = serde_json::from_str::<File>(stripped) else {
        return (None, None);
    };
    match file.font {
        Some(font) => (
            font.family
                .map(|f| f.trim().to_string())
                .filter(|f| !f.is_empty()),
            font.weight.filter(|w| (100..=900).contains(w)),
        ),
        None => (None, None),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_theme_can_declare_its_own_font() {
        assert_eq!(
            parse(r#"{"font":{"family":" JetBrains Mono ","weight":500}}"#),
            (Some("JetBrains Mono".to_string()), Some(500))
        );
        assert_eq!(parse(r#"{}"#), (None, None));
        // 越界字重当没有：不能因为主题写错就换掉用户的字体。
        assert_eq!(
            parse(r#"{"font":{"family":"X","weight":9999}}"#),
            (Some("X".to_string()), None)
        );
    }
}

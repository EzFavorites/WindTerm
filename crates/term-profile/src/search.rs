//! 在线搜索的**引擎表**与 URL 拼装：`Preferences ▸ Search` 与
//! `Effect::SearchOnline` 共用的唯一一份数据。
//!
//! # 为什么在 runtime 而不是 `ui/`
//!
//! 表里每一项都要同时被两个地方读：设置页画下拉、菜单命令打开浏览器。
//! 两边各抄一份 URL 的话，用户在设置页把引擎换成 Bing，菜单那条命令还会
//! 按原来的模板搜——界面与行为对不上，而且没有任何编译错误提醒。
//! 放这里还有一个好处：`ui/` 不持有常量数据（B2），渲染层只认「名字 + 模板」
//! 这两个已经变成值的字段。
//!
//! # 占位符为什么是 `{}`
//!
//! 兼容终端用 `%s`，但 `%` 在 URL 里本来就有转义含义（`%20`），用户把模板
//! 贴进浏览器地址栏手改时很容易看混；`{}` 不会被任何查询串转义规则吃掉，
//! 也不会和真实的百分号编码撞车。

/// 一个搜索引擎：`name` 给人看，`url` 里的 `{}` 是查询词占位符。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SearchEngine {
    pub name: &'static str,
    pub url: &'static str,
}

/// 出厂引擎表，顺序与文案逐项对齐原版设置页的两列表格：
/// Google / Bing / Github / Stackoverflow / Wikipedia / DuckDuckGo。
///
/// 六条都是真实可用的搜索入口，且都不需要 key / 登录。出厂模板是最后那条
/// DuckDuckGo（见 [`default_url`]），所以「没设置过」与「打开设置页看到的
/// 那一项」仍是同一条 URL。
pub const SEARCH_ENGINES: &[SearchEngine] = &[
    SearchEngine {
        name: "Google",
        url: "https://www.google.com/search?q={}",
    },
    SearchEngine {
        name: "Bing",
        url: "https://www.bing.com/search?q={}",
    },
    SearchEngine {
        name: "Github",
        url: "https://github.com/search?q={}",
    },
    SearchEngine {
        name: "Stackoverflow",
        url: "https://stackoverflow.com/search?q={}",
    },
    SearchEngine {
        name: "Wikipedia",
        url: "https://en.wikipedia.org/w/index.php?search={}",
    },
    SearchEngine {
        name: "DuckDuckGo",
        url: "https://duckduckgo.com/?q={}",
    },
];

/// 出厂模板（`Prefs::search_url` 为空串时用它）：最后那条 DuckDuckGo。
pub fn default_url() -> &'static str {
    SEARCH_ENGINES[SEARCH_ENGINES.len() - 1].url
}

/// 这个模板对应表里的第几个引擎；用户手写的模板返回 `None`（页面显示 `Custom`）。
///
/// 按**整串相等**判断，不做前缀匹配：`https://www.google.com/search?q={}&hl=en`
/// 是用户改过的模板，标成 Google 会让人以为它还是出厂的那条。
pub fn engine_index_for(url: &str) -> Option<usize> {
    let url = url.trim();
    SEARCH_ENGINES.iter().position(|e| e.url == url)
}

/// 查询词 → URL 查询串：只保留 RFC 3986 的 unreserved 字符
/// （`A-Za-z0-9-._~`），其余按 UTF-8 字节转 `%XX`。
///
/// 空格是 `%20` 而不是 `+`：`+` 只在 `application/x-www-form-urlencoded`
/// 里有「空格」的意思，`https://…/?q=a+b` 在多条真实搜索引擎上会被当成
/// 字面的加号（例如 `?wd=a+b` 在百度就是搜「a+b」）。`%20` 在所有查询串
/// 语境下都无歧义。
///
/// 不引 `urlencoding` / `percent-encoding`：这里只需要「把任意字节塞进
/// `?q=`」这一件事，多一个依赖就多一份供应链面。
pub fn encode_query(query: &str) -> String {
    let mut out = String::with_capacity(query.len());
    for b in query.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char)
            }
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

/// 把查询词填进模板：第一个 `{}` 换成编码后的词，模板其余部分原样保留。
///
/// 模板里没有 `{}`（用户贴了一条不带占位的 URL）时把词接在末尾——比丢掉
/// 查询词、搜出一个空白首页更接近本意；`https://example.com/search?q=` 这种
/// 「只写了前缀」的模板也因此能用。
pub fn build_url(template: &str, query: &str) -> String {
    let q = encode_query(query);
    match template.split_once("{}") {
        Some((head, tail)) => format!("{head}{q}{tail}"),
        None => format!("{template}{q}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_factory_template_is_the_reference_default() {
        // 「空串 = 没设置」落到出厂模板 DuckDuckGo，设置页默认选中项也是它。
        assert_eq!(default_url(), "https://duckduckgo.com/?q={}");
        assert_eq!(engine_index_for(default_url()), Some(SEARCH_ENGINES.len() - 1));
        let names: Vec<&str> = SEARCH_ENGINES.iter().map(|e| e.name).collect();
        assert_eq!(names, ["Google", "Bing", "Github", "Stackoverflow", "Wikipedia", "DuckDuckGo"]);
    }

    #[test]
    fn every_engine_template_has_a_placeholder_that_round_trips() {
        // 六条都要能用真浏览器打开：https + 一个 `{}`，填词后不再残留占位符。
        for e in SEARCH_ENGINES {
            assert!(e.url.starts_with("https://"), "{} 不是 https", e.name);
            assert!(e.url.contains("{}"), "{} 缺查询词占位符", e.name);
            let filled = build_url(e.url, "rust");
            assert!(!filled.contains("{}"), "{} 填完还留着占位符", e.name);
            assert!(filled.contains("rust"), "{} 没把查询词填进去", e.name);
        }
    }

    #[test]
    fn spaces_are_encoded_as_percent_twenty_not_plus() {
        assert_eq!(encode_query("a b"), "a%20b");
        assert_eq!(build_url("https://example.com/?q={}", "a b"), "https://example.com/?q=a%20b");
    }

    #[test]
    fn non_ascii_is_encoded_byte_by_byte() {
        // 「中」= E4 B8 AD：按 UTF-8 字节百分号编码，不是按码点。
        assert_eq!(encode_query("中"), "%E4%B8%AD");
        assert_eq!(encode_query("终端"), "%E7%BB%88%E7%AB%AF");
    }

    #[test]
    fn characters_with_url_meaning_are_escaped() {
        // `&` / `?` / `#` 原样进查询串会把模板的后半截变成新参数或新片段。
        assert_eq!(encode_query("a&b"), "a%26b");
        assert_eq!(encode_query("a?b"), "a%3Fb");
        assert_eq!(encode_query("a#b"), "a%23b");
        assert_eq!(encode_query("a=b"), "a%3Db");
    }

    #[test]
    fn unreserved_characters_survive_untouched() {
        assert_eq!(encode_query("Az09-._~"), "Az09-._~");
    }

    #[test]
    fn a_template_without_a_placeholder_still_carries_the_query() {
        assert_eq!(
            build_url("https://example.com/search?q=", "a b"),
            "https://example.com/search?q=a%20b"
        );
    }

    #[test]
    fn text_after_the_placeholder_is_kept() {
        assert_eq!(
            build_url("https://example.com/?q={}&hl=en", "rust"),
            "https://example.com/?q=rust&hl=en"
        );
    }

    #[test]
    fn a_hand_edited_template_is_not_mistaken_for_an_engine() {
        // 前缀相同也不能认成 Google：否则下拉显示的名字会撒谎。
        assert_eq!(engine_index_for("https://www.google.com/search?q={}&hl=en"), None);
        assert_eq!(engine_index_for(""), None);
    }
}

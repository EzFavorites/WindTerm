//! 在线搜索六条命令的落地：名字 → URL → 系统默认浏览器。
//!
//! 唯一一份引擎表在 `term_profile::search`（设置页也读它），这里只按名字取模板，
//! 不抄第二份 URL。拉起浏览器统一走 `open_with::open_path`——它认识 URL，
//! 且是投影层唯一的 opener；这里不新开 `sysopen` 的第二个调用点。

use std::path::Path;

use crate::ui::TerminalWindow;

/// 引擎名 → 填好查询词的 URL。未知名字返回 `None`：六个 ID 在 `parse` 里写死，
/// 真走到这里说明表名与菜单漂移了，宁可什么都不做也不拿别的引擎冒充。
pub(super) fn search_url(name: &str, query: &str) -> Option<String> {
    let engine = term_profile::search::SEARCH_ENGINES
        .iter()
        .find(|e| e.name == name)?;
    Some(term_profile::search::build_url(engine.url, query))
}

/// 查询词口径（纯函数，便于逐条钉住）：有选区取选区；没有选区取输入缓冲；
/// 两者都空就是空串。空串**不是**「不做事」——URL 仍然拼出来并打开引擎页。
pub(super) fn query_from(selection: &str, input_buffer: &str) -> String {
    if selection.trim().is_empty() {
        input_buffer.to_string()
    } else {
        selection.to_string()
    }
}

impl TerminalWindow {
    /// 查询词口径见 [`query_from`]：选区优先，退回活动标签的输入缓冲，
    /// 再退回空串。与 `Effect::SearchOnline`（设置页默认引擎）共用 opener。
    pub(super) fn search_online_with(&self, name: &str) {
        let input = self
            .tabs
            .get(self.active_slot())
            .map(|tab| tab.input_line.current())
            .unwrap_or_default();
        let query = query_from(&self.selection_text, &input);
        if let Some(url) = search_url(name, &query) {
            let _ = crate::open_with::open_path(Path::new(&url));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_engine_builds_its_own_url() {
        let cases = [
            ("Google", "https://www.google.com/search?q=rust%20lang"),
            ("Bing", "https://www.bing.com/search?q=rust%20lang"),
            ("Github", "https://github.com/search?q=rust%20lang"),
            ("Stackoverflow", "https://stackoverflow.com/search?q=rust%20lang"),
            ("Wikipedia", "https://en.wikipedia.org/w/index.php?search=rust%20lang"),
            ("DuckDuckGo", "https://duckduckgo.com/?q=rust%20lang"),
        ];
        for (name, want) in cases {
            assert_eq!(search_url(name, "rust lang").as_deref(), Some(want), "{name}");
        }
    }

    #[test]
    fn an_empty_selection_still_produces_an_openable_url() {
        // 空选区不是「不做事」：URL 带空查询串，浏览器照样打开对应引擎。
        for engine in term_profile::search::SEARCH_ENGINES {
            let url = search_url(engine.name, "").expect("六个引擎都要能拼出 URL");
            assert!(url.starts_with("https://"), "{url}");
            assert!(!url.contains("{}"), "{url} 不能残留占位符");
        }
    }

    #[test]
    fn no_selection_falls_back_to_the_input_buffer_then_an_empty_query() {
        assert_eq!(query_from("  hello  ", "typed"), "  hello  ");
        assert_eq!(query_from("   ", "typed"), "typed");
        assert_eq!(query_from("", ""), "", "都空就拼空查询，仍然打开引擎页");
        let url = search_url("Google", &query_from("", "")).expect("空查询也要能开");
        assert_eq!(url, "https://www.google.com/search?q=");
    }

    #[test]
    fn an_unknown_engine_never_falls_back_to_another_one() {
        assert_eq!(search_url("Baidu", "x"), None);
        assert_eq!(search_url("", "x"), None);
    }
}

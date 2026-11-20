//! 「文本 → 主机」解析适配的单测（从 `term-app::onboarding` 等量搬来）。
//!
//! 这 11 条原本钉在应用层，随包装函数一起移到投影层：解析适配需要全仓唯一那份
//! ssh config 解析器，而应用层的依赖表必须保持只有 `term-core`。测试内容一行未删、
//! 断言一字未改——换的只是 `parse_ssh_config` / `parse_windterm_profiles` 的住处。
//!
//! 挂载方式与 `preview_tests.rs` 同一套：`onboarding.rs` 末尾的
//! `#[cfg(test)]` + `#[path = …]` 两行声明把本文件挂成子模块，所以这里
//! 不再写 `#[cfg(test)] mod tests { … }`。

use super::*;

// ── parse_ssh_config ──────────────────────────────────────────────────────

/// 一个最普通的主机块：四个字段全在。
const PLAIN: &str = "\
Host web
    HostName web.example.com
    User deploy
    Port 2222
";

#[test]
fn a_plain_block_yields_label_host_user_and_port() {
    let got = parse_ssh_config(PLAIN, "~/.ssh/config");
    assert_eq!(got.len(), 1, "一个 Host 块只该出一条：{got:?}");
    let h = &got[0];
    assert_eq!(h.label, "web");
    assert_eq!(h.host, "web.example.com");
    assert_eq!(h.user, "deploy");
    assert_eq!(h.port, 2222);
    assert_eq!(h.source, "~/.ssh/config", "来源要跟着每条走");
    assert_eq!(h.target(), "deploy@web.example.com:2222");
}

#[test]
fn keywords_are_case_insensitive() {
    let got = parse_ssh_config("HOST=alpha\n  hostname=Alpha.Example\n  PORT=2022\n  user=Ops\n", "s");
    assert_eq!(got.len(), 1);
    assert_eq!(got[0].label, "alpha");
    assert_eq!(got[0].host, "Alpha.Example", "值本身的大小写要保住");
    assert_eq!(got[0].port, 2022);
    assert_eq!(got[0].user, "Ops");
}

#[test]
fn both_key_value_and_key_equals_value_are_accepted() {
    // 同一个文件里两种写法混着用是常态（手改过的配置尤其如此）。
    let got = parse_ssh_config(
        "Host one\n  HostName one.example\n  User = ops\nHost two\n  HostName=two.example\n  Port = 2200\n",
        "s",
    );
    assert_eq!(got.len(), 2);
    assert_eq!(got[0].user, "ops", "`User = ops` 要认");
    assert_eq!(got[1].host, "two.example", "`HostName=…` 要认");
    assert_eq!(got[1].port, 2200, "`Port = …` 要认");
}

#[test]
fn multiple_aliases_on_one_host_line_expand_to_one_entry_each() {
    // 规则：`Host a b c` 展开成三条。每个别名都是用户能打出来的 `ssh a`，
    // 只取第一个会漏掉另外两个真能连的名字。
    let got = parse_ssh_config(
        "Host a b c\n  HostName shared.example\n  User u\n",
        "s",
    );
    assert_eq!(got.len(), 3, "三个别名 = 三条：{got:?}");
    let labels: Vec<&str> = got.iter().map(|h| h.label.as_str()).collect();
    assert_eq!(labels, vec!["a", "b", "c"], "顺序 = 写的顺序");
    assert!(
        got.iter().all(|h| h.host == "shared.example" && h.user == "u"),
        "三份共享同一个 HostName / User：{got:?}"
    );
}

#[test]
fn a_wildcard_block_is_defaults_not_a_host() {
    // `Host *` 是全局默认值，不是一台机器。它的 `User` 要下发给后面
    // 没写 `User` 的块，但它自己**不许**变成一条会话。
    let got = parse_ssh_config(
        "Host *\n    User ops\n\nHost web\n    HostName w.example\n",
        "s",
    );
    assert_eq!(got.len(), 1, "通配符块不该生成主机：{got:?}");
    assert_eq!(got[0].label, "web");
    assert_eq!(got[0].user, "ops", "通配符块的 User 作为默认值下发");
}

#[test]
fn a_match_block_is_skipped() {
    // `Match` 段是条件默认值，不是主机声明。
    let got = parse_ssh_config(
        "Host a\n  HostName a.example\nMatch host *.internal\n  User nope\nHost b\n  HostName b.example\n",
        "s",
    );
    assert_eq!(got.len(), 2, "只该有 a 与 b：{got:?}");
    assert_eq!(got[0].label, "a");
    assert_eq!(got[1].label, "b");
    assert!(
        got.iter().all(|h| h.user.is_empty()),
        "Match 段里的 User 不许漏到任何一块上：{got:?}"
    );
}

#[test]
fn comments_are_tolerated_whole_line_and_trailing() {
    // 整行注释、分号注释、以及行尾注释（手改文件里最常见）。
    let got = parse_ssh_config(
        "# 全局\n; 另一种注释\nHost web  # 生产\n  HostName web.example  # 主入口\n  Port 2222  # 跳板\n",
        "s",
    );
    assert_eq!(got.len(), 1);
    assert_eq!(got[0].label, "web", "行尾注释不该进别名");
    assert_eq!(got[0].host, "web.example");
    assert_eq!(got[0].port, 2222, "行尾注释不该把端口吃掉（否则静默回落 22）");
}

#[test]
fn a_block_without_hostname_falls_back_to_the_alias() {
    // `ssh web` 的 `web` 本身就是可解析的地址，所以没有 `HostName` 时
    // host 回落成别名，而不是留空（留空等于导出一条连不上的会话）。
    let got = parse_ssh_config("Host web\n  User deploy\n", "s");
    assert_eq!(got.len(), 1);
    assert_eq!(got[0].label, "web");
    assert_eq!(got[0].host, "web", "没有 HostName → host = 别名");
}

#[test]
fn a_missing_port_is_22_and_a_missing_user_is_empty() {
    let got = parse_ssh_config("Host web\n  HostName w.example\n", "s");
    assert_eq!(got.len(), 1);
    assert_eq!(got[0].port, 22, "没有 Port → 22");
    assert_eq!(got[0].user, "", "没有 User → 空串，不编一个登录名出来");
    assert_eq!(got[0].target(), "w.example:22", "空 user 时展示串省掉前缀");
}

#[test]
fn empty_text_yields_no_hosts() {
    assert!(parse_ssh_config("", "s").is_empty());
    assert!(parse_ssh_config("\n\n  \n", "s").is_empty());
    assert!(parse_ssh_config("这不是配置\n", "s").is_empty());
}

// ── parse_windterm_profiles ───────────────────────────────────────────────

#[test]
fn windterm_profiles_are_adapted_the_same_way() {
    // 第二个来源（PRD 要求 `profiles/` 也探测）走同一个适配器，
    // 所以 `source` 与 `label` 的语义与 ssh 那条完全一致。
    let got = parse_windterm_profiles(
        "[wt-b]\nHostName=b.example\nPort=2202\nUser=v\nPassword=secret\n",
        "~/.windterm/profiles/wt-b.wind",
    );
    assert_eq!(got.len(), 1);
    assert_eq!(got[0].label, "wt-b");
    assert_eq!(got[0].host, "b.example");
    assert_eq!(got[0].port, 2202);
    assert_eq!(got[0].user, "v");
    assert_eq!(got[0].source, "~/.windterm/profiles/wt-b.wind");
    assert!(
        !format!("{got:?}").contains("secret"),
        "密码永远不进 DetectedHost"
    );
}

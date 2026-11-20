//! `session_nav.rs` 的单测。
//! verify 要求 `ui/**/*.rs` ≤ 450 行：视觉基线树的逐字断言、
//! 协议差异、页表唯一性等一共十几条测试，留在主文件会把它顶到 516 行，
//! 所以按 `#[path]` 拆成同目录的测试模块（仍是 `super::*` 的私有作用域）。

use super::*;

fn labels_of(protocol: Protocol) -> Vec<&'static str> {
    let mut out = Vec::new();
    for entry in nav_for(protocol) {
        match entry {
            NavEntry::Leaf(l) => out.push(l.label),
            NavEntry::Group { label, children } => {
                out.push(label);
                out.extend(children.iter().map(|c| c.label));
            }
        }
    }
    out
}

fn leaves_of(protocol: Protocol) -> Vec<NavLeaf> {
    let mut out = Vec::new();
    for entry in nav_for(protocol) {
        match entry {
            NavEntry::Leaf(l) => out.push(*l),
            NavEntry::Group { children, .. } => out.extend(children.iter().copied()),
        }
    }
    out
}

/// **本轮的主断言**：视觉基线 `common/08-logging.png` 的 SSH 树，一个字都不能改。
///
/// 17 行（`Session` 起、`X/Y/Z Modem` 止）与截图逐字一致；本项目的
/// `Tunnels` 页保留在 PAGES 里供组件基线使用，但不再挂到可见导航树上，
/// 因为它不属于视觉基线（端口转发归 `Tool > Tunnels...`）。
#[test]
fn ssh_tree_matches_windterm_golden_word_for_word() {
    let golden: [&str; 17] = [
        "Session",
        "Terminal",
        "Bell",
        "Mode",
        "Keyboard",
        "Security",
        "Logging",
        "Window",
        "Selection",
        "SSH",
        "Connection",
        "Proxy",
        "Authentication",
        "Ciphers",
        "Key Exchange",
        "MAC Hashes",
        "Public Keys",
    ];
    let labels = labels_of(Protocol::Ssh);
    assert_eq!(labels[..golden.len()], golden, "视觉基线前 17 行被改动");
    assert_eq!(
        &labels[golden.len()..],
        &["SFTP", "X11", "X/Y/Z Modem"],
        "SSH 树末三项必须与视觉基线一致，Tunnels 不再挂在可见导航树上"
    );
    // 层级也要对：Session 是独立项，其余三项才是可折叠分组。
    assert!(matches!(SSH_NAV[0], NavEntry::Leaf(_)));
    let groups: Vec<&str> = SSH_NAV[1..]
        .iter()
        .filter_map(|e| match e {
            NavEntry::Group { label, .. } => Some(*label),
            _ => None,
        })
        .collect();
    assert_eq!(groups, vec!["Terminal", "Window", "SSH"]);
    assert_eq!(SSH_NAV.len(), 4, "视觉基线树只有四个一级条目");
    assert_eq!(pages_for(Protocol::Ssh).len(), 17, "SSH 树只保留 17 个视觉基线叶子");
}

/// 另外三棵树也有截图（Telnet / Tcp / Serial），一并钉死。
#[test]
fn the_other_three_trees_match_their_own_screenshots() {
    let telnet = [
        "Session", "Terminal", "Bell", "Mode", "Keyboard", "Security", "Logging", "Window",
        "Selection", "Telnet", "Connection", "Proxy", "X/Y/Z Modem",
    ];
    let tcp = [
        "Session", "Terminal", "Bell", "Mode", "Keyboard", "Security", "Logging", "Window",
        "Selection", "Tcp", "Connection", "Proxy", "X/Y/Z Modem",
    ];
    let serial = [
        "Session", "Terminal", "Bell", "Mode", "Keyboard", "Security", "Logging", "Window",
        "Selection", "Serial", "Protocol", "X/Y/Z Modem",
    ];
    assert_eq!(labels_of(Protocol::Telnet), telnet);
    assert_eq!(labels_of(Protocol::Tcp), tcp);
    assert_eq!(labels_of(Protocol::Serial), serial);
}

#[test]
fn every_protocol_has_a_different_configuration() {
    // 用户原话：「每个 tab 的二级导航都有差异，不同协议有不同配置」。
    assert_ne!(labels_of(Protocol::Shell), labels_of(Protocol::Ssh));
    assert_ne!(labels_of(Protocol::Shell), labels_of(Protocol::Serial));
    assert_ne!(labels_of(Protocol::Ssh), labels_of(Protocol::Telnet));
    assert_ne!(labels_of(Protocol::Serial), labels_of(Protocol::Tcp));
    assert_ne!(labels_of(Protocol::Telnet), labels_of(Protocol::Tcp));
    let ssh = labels_of(Protocol::Ssh);
    for word in ["Ciphers", "Key Exchange", "MAC Hashes", "Public Keys", "SFTP", "X11"] {
        assert!(ssh.contains(&word), "视觉基线 SSH 树缺少 {word:?}");
    }
    assert!(labels_of(Protocol::Serial).contains(&"Protocol"));
    assert!(labels_of(Protocol::Shell).contains(&"Command"));
    // Telnet / Tcp 树里没有 SSH 的认证与算法项，但有连接页。
    for protocol in [Protocol::Telnet, Protocol::Tcp] {
        let tree = labels_of(protocol);
        for word in ["Authentication", "Ciphers", "SFTP", "X11"] {
            assert!(!tree.contains(&word), "{protocol} 不该有 {word:?}");
        }
        assert!(tree.contains(&"Connection"));
    }
}

#[test]
fn protocol_specific_pages_only_exist_in_their_own_tree() {
    assert!(contains(Protocol::Ssh, 6), "Authentication 只属于 SSH");
    assert!(!contains(Protocol::Telnet, 6));
    assert!(!contains(Protocol::Shell, 6));
    assert!(!contains(Protocol::Ssh, 12), "Tunnels 已从可见 SSH 树摘下");
    assert!(!contains(Protocol::Serial, 12));
    assert!(contains(Protocol::Serial, 15), "Protocol 只属于串口");
    assert!(!contains(Protocol::Ssh, 15));
    // 视觉基线里 Telnet 与 Tcp 的 `Connection` 是 TCP 选项页，不是 SSH 那页。
    assert!(contains(Protocol::Telnet, 21));
    assert!(contains(Protocol::Tcp, 21));
    assert!(!contains(Protocol::Ssh, 21));
    assert!(!contains(Protocol::Serial, 21));
    assert!(!contains(Protocol::Shell, 4), "本地 Shell 没有连接页");
    assert!(!contains(Protocol::Shell, 5), "本地 Shell 没有代理页");
}

#[test]
fn every_protocol_tree_covers_all_its_pages_without_duplicates() {
    for protocol in Protocol::ALL {
        let mut pages = pages_for(protocol);
        let n = pages.len();
        pages.sort_unstable();
        pages.dedup();
        assert_eq!(pages.len(), n, "{protocol} 的树里有重复页");
        assert!(pages.contains(&0), "{protocol} 必须能回到 Session 页");
    }
}

#[test]
fn switching_to_a_protocol_that_lacks_the_page_falls_back() {
    assert_eq!(default_page(Protocol::Ssh), 0);
    assert_eq!(default_page(Protocol::Shell), 0);
    // 页 12（Tunnels）现在对任何协议的可见树都无效。
    assert!(!contains(Protocol::Telnet, 12));
    assert!(contains(Protocol::Telnet, default_page(Protocol::Telnet)));
}

#[test]
fn a_fixture_page_always_gets_a_protocol_that_shows_it() {
    for page in 0..PAGE_COUNT {
        // 页 12 是视觉基线外的组件专用页：仍可单独出图，但不在任何可见树里。
        if page == 12 {
            continue;
        }
        let protocol = protocol_for_page(page);
        assert!(contains(protocol, page), "页 {page} 找不到能显示它的协议");
    }
    assert_eq!(protocol_for_page(15), Protocol::Serial);
    assert_eq!(protocol_for_page(6), Protocol::Ssh);
    assert_eq!(protocol_for_page(13), Protocol::Shell);
}

/// 取代旧版错误的「导航项不许与协议名重名」：视觉基线里就有 `SSH ▾`。
/// 真正要防的是旧版那种「横竖两排同一批协议名、没有二级项」的平铺。
#[test]
fn horizontal_strip_and_vertical_tree_are_not_the_same_flat_list() {
    let protocols = protocol_labels();
    for protocol in Protocol::ALL {
        let nav = nav_for(protocol);
        assert!(matches!(nav[0], NavEntry::Leaf(_)));
        assert!(nav[1..]
            .iter()
            .all(|e| matches!(e, NavEntry::Group { .. })));
        // 二级叶子一个都不叫协议名：左侧不是把顶部条原样抄一遍。
        for leaf in leaves_of(protocol) {
            assert!(
                !protocols.contains(&leaf.label),
                "{protocol} 的二级项 {:?} 把顶部协议名平铺下来了",
                leaf.label
            );
        }
        // 每棵树恰好有一个以协议命名的分组（视觉基线的 `SSH ▾` 形状）。
        let named_groups = nav
            .iter()
            .filter(|e| matches!(e, NavEntry::Group { label, .. } if protocols.contains(label)))
            .count();
        assert_eq!(named_groups, 1, "{protocol} 的协议分组应恰好一个");
        // 有二级项才叫二级导航，别再退回十个平铺页签。
        // 门槛取 3：视觉基线里串口树最短（Selection / Protocol / X/Y/Z Modem）。
        assert!(pages_for(protocol).len() >= 3);
    }
}

#[test]
fn every_nav_leaf_label_is_the_page_table_label() {
    // 树里把视觉基线文字写一遍是为了可读；但抬头读的是 PAGES，两处不能漂。
    for protocol in Protocol::ALL {
        for leaf in leaves_of(protocol) {
            assert_eq!(
                leaf.label,
                label_for(leaf.page),
                "{protocol} 的 {} 号页文字与页表不一致",
                leaf.page
            );
        }
    }
}

#[test]
fn the_page_table_is_an_unambiguous_single_source() {
    assert_eq!(label_for(0), "Session");
    assert_eq!(label_for(99), "Session", "越界退回第一页，不 panic");
    assert_eq!(component_for(5), "dialog_new_session_proxy");
    assert_eq!(label_for(GOLDEN_FIXTURE.1), "Proxy");
    assert_eq!(GOLDEN_FIXTURE.0, Protocol::Ssh);
    assert_eq!(component_for(99), component_for(0));
    // 页名**允许重名**，因为视觉基线自己就有两个 `Connection`：SSH 那页画
    // Host/Port，Telnet/Tcp 那页画 Nagle/keepalive。抬头按下标取，
    // 不按文字取，所以重名不会指错。真正要防的是「一个协议树里两处同名」
    // （`labels_are_unique_within_a_tree` 守）和「两份内容共用一个组件名」。
    let mut repeated: Vec<&str> = PAGES
        .iter()
        .map(|p| p.label)
        .filter(|label| PAGES.iter().filter(|p| p.label == *label).count() > 1)
        .collect();
    repeated.sort_unstable();
    repeated.dedup();
    assert_eq!(repeated, vec!["Connection"], "只允许 Connection 按协议各占一页");
    let mut names: Vec<&str> = PAGES.iter().map(|p| p.component).collect();
    names.sort_unstable();
    names.dedup();
    assert_eq!(names.len(), PAGES.len(), "组件名重复会让两张基线互相覆盖");
}

#[test]
fn labels_are_unique_within_a_tree() {
    for protocol in Protocol::ALL {
        let mut labels = labels_of(protocol);
        labels.sort_unstable();
        let n = labels.len();
        labels.dedup();
        assert_eq!(labels.len(), n, "{protocol} 树里同名项会让选中态高亮两处");
    }
}

#[test]
fn protocol_tabs_match_the_windterm_golden() {
    // 视觉基线 `session-property.png` 顶排：Shell / SSH / Telnet / Tcp / Serial。
    // 尤其 Tcp 在 Serial 之前，且 Shell 不叫 "Local Shell"。
    assert_eq!(
        protocol_labels(),
        vec!["Shell", "SSH", "Telnet", "Tcp", "Serial"]
    );
    let mut seen = PROTOCOL_TABS.iter().map(|(p, _)| *p).collect::<Vec<_>>();
    seen.sort_by_key(|p| *p as u8);
    seen.dedup();
    assert_eq!(seen.len(), 5);
}

#[test]
fn every_content_page_is_reachable_from_some_protocol() {
    for page in 0..PAGE_COUNT {
        // 12 = Tunnels：**故意**没有叶子。2.6.1 视觉基线的 SSH 树里没有这一项
        // （端口转发归 `Tool > Tunnels...`），页本身留在 `PAGES` 里是为了
        // `forwards` 字段与 `dialog_new_session_tunnels` 组件基线还能被渲染。
        // 除了这一页，其余每一页都必须挂在某棵树上——新增页面忘了挂，
        // 这条断言就是唯一的提醒。
        if page == 12 {
            continue;
        }
        assert!(
            ALL_NAVS.iter().any(|nav| {
                nav.iter().any(|entry| match entry {
                    NavEntry::Leaf(l) => l.page == page,
                    NavEntry::Group { children, .. } => children.iter().any(|l| l.page == page),
                })
            }),
            "页 {page} 被所有协议排除，等于删了这页"
        );
    }
    // 反向：12 号页确实不在任何树上。哪天有人把它挂回去（那会与视觉基线
    // 的 SSH 树差出一行），这条会红，提醒同步 `04-new-session/ssh/11-ssh.png`。
    assert!(!ALL_NAVS.iter().any(|nav| {
        nav.iter().any(|entry| match entry {
            NavEntry::Leaf(l) => l.page == 12,
            NavEntry::Group { children, .. } => children.iter().any(|l| l.page == 12),
        })
    }));
}

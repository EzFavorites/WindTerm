//! New Session 的**按协议分派的二级导航树**。
//! 视觉基线：`视觉基线`（ / ）。
//!
//! # 五棵树逐字取自视觉基线，别再自造分组名
//!
//! 用户重采了 98 张原生截图（`windterm-ui-screenshots/`），其中
//! **四个协议各有自己的树**，不再只有 SSH 一张：
//!
//! ```text
//! Source: 02-session-dialog/common/08-logging.png   (SSH)
//!         telnet/01-telnet-connection.png           (Telnet)
//!         tcp/01-tcp-connection.png                 (Tcp)
//!         serial/01-serial-protocol.png             (Serial)
//!
//! Session                        (一级独立项，无展开箭头)
//! Terminal ▾                    (五个协议共用，视觉基线完全一致)
//!     Bell
//!     Mode
//!     Keyboard
//!     Security
//!     Logging
//! Window ▾                      (五个协议共用)
//!     Selection
//! SSH ▾ / Telnet ▾ / Tcp ▾ / Serial ▾   (各协议自己的分组)
//!     SSH    : Connection Proxy Authentication Ciphers Key Exchange
//!              MAC Hashes Public Keys SFTP X11 X/Y/Z Modem
//!     Telnet : Connection Proxy X/Y/Z Modem
//!     Tcp    : Connection Proxy X/Y/Z Modem
//!     Serial : Protocol X/Y/Z Modem
//! ```
//!
//! 上一版这里自造了 `Network / Security / Files / Algorithms / Transfer` 五组
//! 八个名字，用户指出「二级菜单列表怎么不一样，看对比图」——差的就是这张表。
//!
//! 一处与视觉基线的已知差异（记在 `docs/ui-alignment-audit.md`）：
//!
//! 1. **Shell 树无视觉基线**（`00-shell-tab.png` 拍成了主窗口），
//!    沿用同一结构类推，属未核实。
//!
//! `Tunnels` 页仍保留在 `PAGES` 里供组件基线与既有 `forwards` 字段使用，
//! 但原版 SSH 树确实没有这一项（⛔ NOT-AVAILABLE；端口转发归工具条 Tunnels
//! 按钮打开的独立 Tunnel Settings 对话框），
//! 所以**不再挂到可见导航树**，会话弹窗逐像素对齐视觉基线。
//!
//! # 两件事，别再搞混
//!
//! 顶部横排是**协议**（`Shell / SSH / Telnet / Tcp / Serial`），左侧是
//! **该协议自己的设置域树**——两者是上下级，不是两套并列的菜单。
//! 用户原话：「每个 tab 的二级导航都有差异，不同协议有不同配置」。
//!
//! 但「上下级」**不等于**「组名不能叫 SSH」：视觉基线里那个分组就是 `SSH ▾`。
//! 上一版加过一条「导航项不许与顶部协议标签重名」的测试，那是把自己造的
//! 结构当成了规格，已经删掉。真正要防的是旧版那种「横排五个协议名、左排
//! 又原样抄五个协议名当页签、且没有二级项」的平铺重复。
//!
//! # 单一来源
//!
//! [`PAGES`] 是内容页下标 → (显示名, 组件名) 的唯一表：右侧抬头读它、
//! 组件夹具的名字也读它。树里的 `NavLeaf.label` 故意把视觉基线文字**再写一遍**
//! （读代码就能逐字逐页比对），由 `every_nav_leaf_label_is_the_page_table_label`
//! 保证两处不漂移。一个叶子一个下标，因此点进去不会几项内容雷同。
//!
//! # 边界
//!
//! - 折叠状态交给 `egui::CollapsingHeader` 的 `id_salt` 存在 egui Memory，
//!   不给 `NewSessionData` 加视图字段。
//! - 切换协议时若当前页不在新树里，`default_page ` 把它收回本协议第一页。
//! - SSH / Telnet / Tcp / Serial 四棵树的**条目**有视觉基线可据；Shell 那棵没有
//!   （见上），不要把它当成「已核实」。
//! - 树对了不等于页对了：`Connection` 在视觉基线里有**两个内容完全不同的页**
//!   （SSH 是 Host/Port/…，Telnet/Tcp 是 Nagle/keepalive/超时），所以页表里
//!   有两个同名 `Connection`，靠下标区分——抬头读的是下标，不是文字。

use crate::ui::Protocol;

/// 内容页下标 → 显示名 / 组件名。下标就是 `NewSessionData::selected_page`。
#[derive(Clone, Copy)]
pub(in crate::ui) struct PageSpec {
    pub label: &'static str,
    pub component: &'static str,
}

/// 24 页：视觉基线 SSH 树 17 个叶子 + 其余协议的专属页 + 两个组头页。新增页一律**追加在
/// 末尾**，保住 0..16 的旧下标（`GOLDEN_FIXTURE`、组件夹具、既存测试都按
/// 旧下标引用）。
pub(in crate::ui) const PAGES: [PageSpec; 24] = [
    PageSpec { label: "Session",        component: "dialog_new_session_session" },
    PageSpec { label: "Keyboard",       component: "dialog_new_session_keyboard" },
    PageSpec { label: "Logging",        component: "dialog_new_session_logging" },
    PageSpec { label: "Selection",      component: "dialog_new_session_selection" },
    PageSpec { label: "Connection",     component: "dialog_new_session_connection" },
    PageSpec { label: "Proxy",          component: "dialog_new_session_proxy" },
    PageSpec { label: "Authentication", component: "dialog_new_session_authentication" },
    PageSpec { label: "Ciphers",        component: "dialog_new_session_ciphers" },
    PageSpec { label: "Key Exchange",   component: "dialog_new_session_key_exchange" },
    PageSpec { label: "MAC Hashes",     component: "dialog_new_session_mac_hashes" },
    PageSpec { label: "Public Keys",    component: "dialog_new_session_public_keys" },
    PageSpec { label: "X11",            component: "dialog_new_session_x11" },
    // 本项目自有页（视觉基线导航树没有）：`forwards` 编辑器；只为组件基线
    // 与既有字段保留，不挂到会话弹窗的 SSH 树上。
    PageSpec { label: "Tunnels",        component: "dialog_new_session_tunnels" },
    PageSpec { label: "Process",        component: "dialog_new_session_command" },
    PageSpec { label: "Environment",    component: "dialog_new_session_environment" },
    // 视觉基线串口树里这一项叫 `Protocol`（页面内容仍是波特率/数据位那一组）。
    PageSpec { label: "Protocol",       component: "dialog_new_session_serial_port" },
    // 视觉基线里四个协议树的末项都写作 `X/Y/Z Modem`，页内容仍是既有的 modem 页。
    PageSpec { label: "X/Y/Z Modem",    component: "dialog_new_session_modem" },
    // 视觉基线 `common/*.png` 补的五页（追加，不动旧下标）。
    PageSpec { label: "Bell",           component: "dialog_new_session_bell" },
    PageSpec { label: "Mode",           component: "dialog_new_session_mode" },
    PageSpec { label: "Security",       component: "dialog_new_session_security" },
    PageSpec { label: "SFTP",           component: "dialog_new_session_sftp" },
    // Telnet / Tcp 的 `Connection` 页：视觉基线里它画的是 TCP 选项，
    // 与 SSH 的 `Connection`（Host/Port/…）是两页，所以各占一个下标。
    PageSpec { label: "Connection",     component: "dialog_new_session_tcp_connection" },
    // 视觉基线 `common/01-terminal-page.png` / `common/07-window-page.png`：
    // 组头本身就是一页。此前组头只能用 `CollapsingHeader` 折叠、点不出内容，
    // 用户报「左侧树行为不对」。追加在末尾，旧下标一个不动。
    PageSpec { label: "Terminal",       component: "dialog_new_session_terminal_group" },
    PageSpec { label: "Window",         component: "dialog_new_session_window_group" },
];

pub(in crate::ui) const PAGE_COUNT: usize = PAGES.len();

/// 视觉基线 `session-property.png` 定格的状态：SSH 协议、`Proxy` 整行高亮。
/// 整窗弹窗夹具（`--demo newsession`）按它初始化，才能与视觉基线并排逐页比对；
/// 真实「＋ New Session」仍从 Shell / Session 页开始，不受夹具影响。
pub(in crate::ui) const GOLDEN_FIXTURE: (Protocol, usize) = (Protocol::Ssh, 5);

/// 树上的一个可点页（二级项，或没有子项的一级项）。
#[derive(Clone, Copy)]
pub(in crate::ui) struct NavLeaf {
    pub label: &'static str,
    pub page: usize,
}

pub(in crate::ui) enum NavEntry {
    /// 一级独立项：视觉基线里 `Session` 就是**没有展开箭头**的一级项。
    Leaf(NavLeaf),
    /// 可折叠分组（一级）+ 缩进的二级页。
    ///
    /// `page` 是**组头自己的页**：视觉基线里 `Terminal ▾` / `Window ▾` 点上去
    /// 右侧有内容（`common/01-terminal-page.png`、`common/07-window-page.png`），
    /// 组头既是页也是折叠开关。协议那几组（`SSH ▾` 等）没有独立页，
    /// 点文字＝展开并进第一个子页。
    Group {
        label: &'static str,
        page: Option<usize>,
        children: &'static [NavLeaf],
    },
}

const SESSION: NavEntry = NavEntry::Leaf(NavLeaf {
    label: "Session",
    page: 0,
});

// ── 视觉基线五棵树共用的两个分组（`common/03-bell.png` 起逐张核对过） ──

/// `Terminal ▾`：视觉基线四个协议截图里**完全一致**，五项按字母序。
const TERMINAL_GROUP: NavEntry = NavEntry::Group {
    label: "Terminal",
    page: Some(22),
    children: &[
        NavLeaf { label: "Bell", page: 17 },
        NavLeaf { label: "Mode", page: 18 },
        NavLeaf { label: "Keyboard", page: 1 },
        NavLeaf { label: "Security", page: 19 },
        NavLeaf { label: "Logging", page: 2 },
    ],
};

const WINDOW_GROUP: NavEntry = NavEntry::Group {
    label: "Window",
    page: Some(23),
    children: &[NavLeaf { label: "Selection", page: 3 }],
};

/// 视觉基线的 `SSH ▾` 分组（`common/08-logging.png`）：十项，顺序与大小写都按截图。
/// 本项目的 `Tunnels` 页已从可见树摘下，见模块头说明。
const SSH_GROUP: NavEntry = NavEntry::Group {
    page: None,
    label: "SSH",
    children: &[
        NavLeaf { label: "Connection", page: 4 },
        NavLeaf { label: "Proxy", page: 5 },
        NavLeaf { label: "Authentication", page: 6 },
        NavLeaf { label: "Ciphers", page: 7 },
        NavLeaf { label: "Key Exchange", page: 8 },
        NavLeaf { label: "MAC Hashes", page: 9 },
        NavLeaf { label: "Public Keys", page: 10 },
        NavLeaf { label: "SFTP", page: 20 },
        NavLeaf { label: "X11", page: 11 },
        NavLeaf { label: "X/Y/Z Modem", page: 16 },
    ],
};

// ── 各协议自己的分组 ──

/// Shell：**没有对应视觉基线**，沿用同一结构类推（未核实）。
const SHELL_GROUP: NavEntry = NavEntry::Group {
    page: None,
    label: "Shell",
    children: &[
        NavLeaf { label: "Process", page: 13 },
        NavLeaf { label: "Environment", page: 14 },
        NavLeaf { label: "X/Y/Z Modem", page: 16 },
    ],
};

/// Telnet（`telnet/01-telnet-connection.png`）：Connection / Proxy / X/Y/Z Modem。
const TELNET_GROUP: NavEntry = NavEntry::Group {
    page: None,
    label: "Telnet",
    children: &[
        NavLeaf { label: "Connection", page: 21 },
        NavLeaf { label: "Proxy", page: 5 },
        NavLeaf { label: "X/Y/Z Modem", page: 16 },
    ],
};

/// Tcp（`tcp/01-tcp-connection.png`）：与 Telnet 同构。
const TCP_GROUP: NavEntry = NavEntry::Group {
    page: None,
    label: "Tcp",
    children: &[
        NavLeaf { label: "Connection", page: 21 },
        NavLeaf { label: "Proxy", page: 5 },
        NavLeaf { label: "X/Y/Z Modem", page: 16 },
    ],
};

/// 串口（`serial/01-serial-protocol.png`）：`Protocol` + `X/Y/Z Modem`，没有代理。
const SERIAL_GROUP: NavEntry = NavEntry::Group {
    page: None,
    label: "Serial",
    children: &[
        NavLeaf { label: "Protocol", page: 15 },
        NavLeaf { label: "X/Y/Z Modem", page: 16 },
    ],
};

// ── 五套树 ──

/// SSH：逐字照抄视觉基线，不许「顺手优化」。
const SSH_NAV: &[NavEntry] = &[SESSION, TERMINAL_GROUP, WINDOW_GROUP, SSH_GROUP];

/// 本地 Shell：没有连接 / 代理 / 算法，只有启动命令与环境。
const SHELL_NAV: &[NavEntry] = &[SESSION, TERMINAL_GROUP, WINDOW_GROUP, SHELL_GROUP];

/// Telnet：`Connection / Proxy / X/Y/Z Modem`，没有 SSH 的认证 / 算法 / X11。
const TELNET_NAV: &[NavEntry] = &[SESSION, TERMINAL_GROUP, WINDOW_GROUP, TELNET_GROUP];

/// 原始 TCP：与 Telnet 同构（视觉基线两张截图除协议名外逐字一致）。
const TCP_NAV: &[NavEntry] = &[SESSION, TERMINAL_GROUP, WINDOW_GROUP, TCP_GROUP];

/// 串口：`Protocol / X/Y/Z Modem`，没有连接 / 代理 / 算法。
const SERIAL_NAV: &[NavEntry] = &[SESSION, TERMINAL_GROUP, WINDOW_GROUP, SERIAL_GROUP];

const ALL_NAVS: [&[NavEntry]; 5] = [
    SHELL_NAV,
    SSH_NAV,
    TELNET_NAV,
    SERIAL_NAV,
    TCP_NAV,
];

/// **按协议取树**。这是「不同协议有不同配置」的唯一入口。
pub(in crate::ui) fn nav_for(protocol: Protocol) -> &'static [NavEntry] {
    match protocol {
        Protocol::Shell => SHELL_NAV,
        Protocol::Ssh => SSH_NAV,
        Protocol::Telnet => TELNET_NAV,
        Protocol::Serial => SERIAL_NAV,
        Protocol::Tcp => TCP_NAV,
    }
}

/// 该协议树上的全部页（扁平化）。
///
/// **组头自己那一页也在册**（`Terminal` / `Window`）：它们能被选中、右侧有内容，
/// 所以「当前页是否属于这棵树」必须认它们，否则切协议时会把停在组头的选择
/// 当成失效页踢走，`protocol_for_page` 也找不到能显示它的树。
pub(in crate::ui) fn pages_for(protocol: Protocol) -> Vec<usize> {
    let mut out = Vec::new();
    for entry in nav_for(protocol) {
        match entry {
            NavEntry::Leaf(leaf) => out.push(leaf.page),
            NavEntry::Group { page, children, .. } => {
                out.extend(page);
                out.extend(children.iter().map(|l| l.page));
            }
        }
    }
    out
}

pub(in crate::ui) fn contains(protocol: Protocol, page: usize) -> bool {
    pages_for(protocol).contains(&page)
}

/// 切协议后当前页失效时落到哪一页。
pub(in crate::ui) fn default_page(protocol: Protocol) -> usize {
    pages_for(protocol).first().copied().unwrap_or(0)
}

/// 夹具用：给某页挑一个**真的包含它**的协议，让单页出图显示正确的树。
pub(in crate::ui) fn protocol_for_page(page: usize) -> Protocol {
    Protocol::ALL
        .into_iter()
        .find(|p| contains(*p, page))
        .unwrap_or(Protocol::Ssh)
}

/// 当前页在树上显示的文字，也用作右侧内容区抬头。
/// 越界退回第一页，不 panic（夹具会写它）。
pub(in crate::ui) fn label_for(page: usize) -> &'static str {
    PAGES.get(page).map(|p| p.label).unwrap_or(PAGES[0].label)
}

/// 页组件的基线名。组件清单不另抄一份 `NAMES` 表。
pub(in crate::ui) fn component_for(page: usize) -> &'static str {
    PAGES
        .get(page)
        .map(|p| p.component)
        .unwrap_or(PAGES[0].component)
}

/// 顶部协议标签条的**顺序与显示名**，逐字取自视觉基线
/// `session-property.png`：`Shell / SSH / Telnet / Tcp / Serial`。
///
/// 刻意不复用 `Protocol::label `：那是 core 的通用显示名，会话树、菜单、
/// 状态栏都在用（本地 Shell 显示成 `Local Shell`、顺序是 Serial 在 Tcp 前）。
/// 为了一张弹窗去改全局会波及整条产品线，所以这里只描述这一条标签条。
pub(in crate::ui) const PROTOCOL_TABS: [(Protocol, &str); 5] = [
    (Protocol::Shell, "Shell"),
    (Protocol::Ssh, "SSH"),
    (Protocol::Telnet, "Telnet"),
    (Protocol::Tcp, "Tcp"),
    (Protocol::Serial, "Serial"),
];

/// 顶部协议标签条的显示名。单测拿它守「横竖不是同一批平铺协议名」与视觉基线顺序。
pub(in crate::ui) fn protocol_labels() -> Vec<&'static str> {
    PROTOCOL_TABS.iter().map(|(_, label)| *label).collect()
}

#[cfg(test)]
#[path = "session_nav_tests.rs"]
mod tests;

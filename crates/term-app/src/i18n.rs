//! 多语言：`Lang` + 一张 `英文原文 → 中文` 词表。
//!
//! # 为什么 key 是英文原文（gettext 式），不是 `Key` 枚举
//!
//! 两个理由：
//!
//! 1. **缺词必须退回英文**，不能退回一个空串或者 `TODO`。用原文当 key，漏翻
//!    就是「照原样显示」，界面永远可用；换成 `Key::NewSession` 枚举，漏一条
//!    就是编译期逼人补全，或者运行期显示键名。
//! 2. 菜单树（`menu.rs`）里有 162 条 `&'static str`。改成枚举要动 882 行数据，
//!    而这条链上唯一的读者是绘制处的一行 `t(...)`。
//!
//! 代价是：key 与文案绑死，改英文原文等于改 key（词表那条也要跟着改）。这一条
//! 由 `every_visible_label_has_a_translation` 兜住——菜单里出现没翻译的项会红。
//!
//! # 词表怎么写
//!
//! `zh()` 是一个 `match`：`&str` 的 match 会被编译成按长度分桶 + memcmp，
//! 不是 215 次线性比较。写错（重复 key）时**先命中的赢**，不会报错——所以别
//! 靠编译器兜底，要靠那个覆盖测试。
//!
//! 词表只管**界面 chrome**（菜单 / 面板标题 / 右键菜单 / 查找栏 / 状态栏 /
//! 发送器工具行 / 浮层 / 关于）。会话名、分组名、终端输出、要送进 PTY 的
//! 命令行字都是数据，不进词表；新建会话表单等正文的双语化属于第二阶段
//! （见 `docs/design/12-capability-surface-and-menus.md`）。
//!
//! # 文件布局
//!
//! 本文件是**模块根**（路径仍是 `i18n.rs`，不改 `mod.rs`：门禁按路径写死）：
//! `Lang` 与 `t()` 在这里，那张约 550 行的词表搬到 `i18n/zh.rs`，本文件只
//! `use zh::zh;`。对外路径 `term_app::i18n::{Lang, t}` 一个字都没变。

mod zh;

use zh::zh;

/// 界面语言。
///
/// 默认 **英文**：整窗与组件基线都是英文的，默认值一变，42 张 PNG 全要重冻结，
/// 也会让「与 WindTerm 英文视觉基线逐像素比对」这件事失效。中文由用户选。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Lang {
    #[default]
    En,
    /// 简体中文。
    Zh,
}

impl Lang {
    pub const ALL: [Lang; 2] = [Lang::En, Lang::Zh];

    /// 落盘用的标签。`zh-CN` 沿用 `application.language` 的写法。
    pub fn tag(self) -> &'static str {
        match self {
            Lang::En => "en",
            Lang::Zh => "zh-CN",
        }
    }

    /// 下拉框里显示的名字：**用该语言自己的写法**。
    pub fn native_name(self) -> &'static str {
        match self {
            Lang::En => "English",
            Lang::Zh => "中文（简体）",
        }
    }

    /// 从 `en` / `en-US` / `zh` / `zh-CN` / `zh_CN` 这类标签解析。
    pub fn from_tag(s: &str) -> Option<Lang> {
        let lower = s.trim().to_ascii_lowercase();
        if lower.starts_with("zh") {
            Some(Lang::Zh)
        } else if lower.starts_with("en") {
            Some(Lang::En)
        } else {
            None
        }
    }
}

/// 取一条文案。缺词退回英文原文——这是本模块唯一的兜底规则。
pub fn t(lang: Lang, en: &'static str) -> &'static str {
    match lang {
        Lang::En => en,
        Lang::Zh => zh(en),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 绘制代码里写死的那些非菜单文案。菜单树由 `visit()` 自动扫，但这些
    /// 散落在十来个 `ui/*.rs` 里，只能列一份清单兜底——漏一条中文界面就露馅。
    const CHROME_KEYS: &[&str] = &[
        "Activate", "Close Others", "Reopen Closed", "Duplicate Session",
        "Split Horizontally", "Split Vertically", "Copy Session URL", "Tab Color",
        "Default", "Red", "Orange", "Yellow", "Green", "Teal", "Cyan", "Blue",
        "Purple", "Magenta", "Grey", "White",
        "Activate this pane", "Find:", "Search", "Match Case",
        // 行过滤栏右端的反转开关（默认关；grep -v 语义写在悬停里）。
        "Invert", "Invert filter (grep -v): hide matching lines",
        // 2026-11-08 右键菜单（终端 / 文件面板）
        "Run Script", "New folder", "Permissions", "Delete",
        "Find Previous (Shift+Enter)", "Find Next (Enter)", "Clear Screen",
        "In", "Out", "Reset", "Search commands", "Palette Anything",
        "Search all menu items", "No matches", "↑↓ Select · Enter Run · Esc Close",
        "{n} items, ↑↓ to scroll",
        "Line (0 = screen top, negative = history)",
        "Scrollback up to line {n}",
        "Enter an integer line number (may start with -)", "No open sessions",
        "Enter Jump · Esc Close", "↑↓ Select · Enter Switch · Esc Close",
        " (disconnected)", "No active session.", "Feed a control line:", "Parse",
        "No forwarding tunnel is running.", "Forwarding rules of the active session",
        "Stop", "Restart", "Add", "Open", "Duplicate", "Move to",
        "Properties", "New Group",
        "Import", "Shell Sessions", "WindTerm Sessions", "No session available",
        "Settings", "Close sidebar", "Filter", "Session Popup", "🔒 Screen locked",
        "Screen Locked", "Password",
        "Enter the vault master password to unlock.",
        "No vault yet — set a master password, or cancel to leave.",
        "Master password", "Wrong master password", "Unlock",
        "Create vault & unlock", "Cancel",
        // ── 2026-11-08 视觉基线：Log 子菜单 + Session Settings ▸ Logging ──
        // 菜单三项另由 visit() 自动扫；这里补的是配置页与左树里散落的文案。
        // SSH / Telnet / Tcp 三种协议名中英同形，zh() 返回原文，列进来会假红，
        // 所以故意不列（翻译表里仍登记，保持界面取词路径一致）。
        "Start", "Pause", "Resume",
        "Session Settings", "Logging", "Bell", "Mode", "Keyboard", "Security",
        "Window", "Selection", "Process", "X/Y/Z Modem", "Shell", "Serial",
        "Proxy", "Connection", "Authentication", "Serial Port",
        "Log types:", "Log nothing", "Log printable output", "Log all session output",
        "Log options:", "Overwrite log file", "Append log file",
        "Log file name:", "Log maximum size:", "Log content pattern:",
        "Edit Default Settings", "Open Preferences",
        "Logging settings saved", "Could not save logging settings",
        "Not available in the current log state", "Not wired (placeholder)",
        "Status:", "File:", "Size:",
        // ── 快捷命令栏：两个弹层 + 图标/命令按钮提示 ──
        "Configure command groups", "Show command groups",
        "Close quick command bar", "Quick command bar settings",
        "More commands", "Send `{cmd}` to the current session (no Enter)",
        "Edit commands", "Open the quick command editor",
        "Default group is always shown",
        // 快捷命令的 Run Script 类型：只填路径，前缀由界面补。
        "Script file:",
        // 脚本面板的录制按钮与单次授权折叠面板。
        "Record", "Stop Recording", "Nothing recorded (no keystrokes captured)",
        "Permissions (this run only)", "Run programs", "Read clipboard", "Control window",
        "Pick from script library", "Browse for a script file", "Run options (this run only)",
        "Pick a script, then press Run",
        "folder for file access", "Writable",
        "Sub-folders are included; `..` is rejected before comparing.",
        // ── 我们自己的运行配置 / 后台纪律（兼容终端没有这两层）──
        "Long-running", "10 min / 50M operations instead of 30 s / 1M",
        "A script is already running; wait for it to finish",
        "Permission denied? Declare it with // @cap in the script, or enable it under Script panel ▸ Permissions (this run only).",
        // 脚本面板的自由路径行（同样是本项目加的入口，不是兼容终端功能）。
        "Path:", "Browse…", "/path/to/script.rhai (relative = script library)",
        "— pick a script —",
        // ── 2026-11-11 视觉基线右键菜单（其余标签由菜单树 visit() 自动覆盖）──
        "Copy Command", "Clear Scrollback", "Clear All",
        "Session Triggers", "More",
        // ── 2026-11-12 未挂接弹层（其余由菜单树 visit() 自动覆盖）──
        "Enable Symbol Preview", "switch to tab", "Opened",
        // 地址栏下拉第三段的分组名（`term_app::address::AddressKind::Host`）。
        "Host",
        // 地址栏浮层（`apps/desktop/src/ui/address/`）——输入框占位、动作提示、
        // `▾` 菜单三行与四条解析报错。列进来才算被门禁管住：漏翻一条，中文界面
        // 就在地址栏里露一句英文。
        "Host, user@host, or pick a session", "connect", "Session Properties",
        "Clear Recent", "Enter a host, or an address like ssh://user@host:22.",
        "This address has no host name.", "'{raw}' is not a port number (0-65535).",
        "'{raw}' is not a valid user name.",
        // ── 首次启动引导（`apps/desktop/src/ui/overlays/onboarding.rs`）──
        // 窗口标题与两页的全部文案。带 `{n}` / `{path}` 的两条整句进表，
        // 占位符由调用方 `replace`（与 `… {n} items` 同一写法）。
        "Welcome", "Detected hosts", "Next", "Back", "Skip", "Finish",
        "Import {n} sessions", "From {path}",
        "WindTerm can seed your session tree from the SSH hosts already configured on this machine. Passwords are never copied.",
        "No SSH hosts found — you can still add sessions from the sidebar later.",
        // ── 预览窗（`apps/desktop/src/ui/overlays/preview.rs` + Filer 右键）──
        // 窗口标题、两种按钮、四种内容说明、三条提示。字符集名 / 图片格式 /
        // `MiB` 单位 / 文件名 / 路径都是**数据**，故意不进这张表。
        "Preview", "Open Externally", "Only the beginning is shown",
        "Binary file", "Empty file", "Cannot preview this file",
        "Cannot decode this image", "Image too large to preview",
        "Could not open with the system default program",
        // Filer 双击远端文件时那一句一次性提示（`log_note`）与它的两种失败。
        "Downloading to a temporary file", "Could not download the file",
        "Not connected to SFTP",
        // ── 脚本编辑器（`apps/desktop/src/ui/overlays/script_editor.rs`）──
        // 窗口标题 / 五个按钮 / 脏标记 / 空路径 / 两种结果 / 跳行。`{n}` 整句进表，
        // 占位符由调用方 `replace`（与 `… {n} items` 同一写法）。
        "Script Editor", "Run", "Save As", "Save as:", "● Unsaved", "(unsaved)",
        "This tab has no live session", "Done", "Failed", "Go to line {n}",
        // ── 「运行脚本」窗口的两份清单（`ui/terminal/script_library.rs`）──
        // 两个列头 + 两种空态。文件名 / 路径是数据，故意不进这张表。
        "Library", "Recent", "no scripts in library", "nothing run yet",
        // ── Sender ▸ Selected Sessions 的多选下拉（`tools_sender_menu.rs`）──
        // 未展开时那一格的空态词，以及一个都没勾就按 `▶` 时的那句提示。
        "None", "No target session is selected",
        // ── 会话树右键「Logon Script…」（`panels/sidebar_view.rs` / `sidebar_logon_script.rs`）──
        // 菜单行、对话框标题、字段标签、弱提示、提交失败与页脚按钮。
        "Logon Script…", "Logon script:", "Takes effect on the next connect",
        "Session not found", "Ok",
        // 消息框新增两组按钮的标签（`MessageButton::labels()`）。
        "abort", "retry", "ignore",
        // ── 隐藏末页 Preferences ▸ Script & API（`dialogs/settings_live_script_api.rs`）──
        // 整页只读：这些是页名 / 分组 / 状态行 / 按钮 / 来源说明，逐条登记，
        // 中文界面不许露出英文；数值与文件名 / 路径是数据，不进词表。
        "Script & API", "Scripts", "Script Panel", "Bridge API", "Python",
        "Registered scripts", "Recent runs", "Library files", "Trigger rules",
        "Bridge port", "Token file", "Token file present", "Token file missing",
        "Python interpreter", "Environment override", "PATH fallback",
        "Not found on PATH", "Quick Commands",
        "Read-only. Buttons open the existing managers.",
    ];

    /// 有没有可翻译的字母内容。纯数字 / 纯符号标签走 `zh()` 的原样返回。
    fn has_ascii_letter(s: &str) -> bool {
        s.chars().any(|c| c.is_ascii_alphabetic())
    }

    #[test]
    fn only_digit_labels_skip_the_translation_gate() {
        for label in ["2", "7", "8", "10", "16"] {
            assert_eq!(zh(label), label, "{label} 应原样显示");
            assert!(!has_ascii_letter(label), "{label} 不该被判成可翻译文案");
        }
        // 带字母的两条必须真翻了：豁免一旦放宽成「zh 返回原文也放行」，
        // 下面两句就会红，而不是让中文界面悄悄露出英文。
        assert!(has_ascii_letter("8 Columns"));
        assert_eq!(zh("8 Columns"), "8 列");
        assert_eq!(zh("Show All"), "全部显示");
    }

    /// 递归收集菜单树里所有 `label`（顶级 + 子菜单 + 命令）。
    fn visit(items: &[crate::menu::Item], out: &mut Vec<&'static str>) {
        for it in items {
            match it {
                crate::menu::Item::Action { label, .. } => out.push(*label),
                // 可勾选行同样是可见文案，必须能取到译文（`Item::Check`）。
                crate::menu::Item::Check { label, .. } => out.push(*label),
                crate::menu::Item::Submenu { label, items } => {
                    out.push(*label);
                    visit(items, out);
                }
                crate::menu::Item::Separator => {}
            }
        }
    }

    #[test]
    fn every_visible_label_has_a_translation() {
        // 这是本模块唯一真正的判据：菜单里冒出一条没翻译的，中文界面就露馅。
        let mut labels = Vec::new();
        for m in crate::menu::menus() {
            labels.push(m.label);
            visit(&m.items, &mut labels);
        }
        // 面板标题也会出现在中文界面里。
        for id in crate::dock::PanelId::ALL {
            labels.push(id.title());
        }
        // Hex Base 的 2 / 7 / 8 / 10 / 16 是纯数字标签，没有可翻译内容，
        // `zh()` 按设计返回原文。不作豁免会被判成「漏翻」，作出过宽的豁免
        // 又会让真漏翻的英文文案混过去——所以只按「有没有 ASCII 字母」放行，
        // 边界由 `only_digit_labels_skip_the_translation_gate` 单独钉住。
        let missing: Vec<&str> = labels
            .iter()
            .copied()
            .filter(|l| zh(l) == *l && has_ascii_letter(l))
            .collect();
        assert!(
            missing.is_empty(),
            "这 {} 条没有中文翻译：{missing:?}",
            missing.len()
        );
    }

    #[test]
    fn every_hardcoded_chrome_label_has_a_translation() {
        let missing: Vec<&str> = CHROME_KEYS
            .iter()
            .copied()
            .filter(|k| zh(k) == *k)
            .collect();
        assert!(
            missing.is_empty(),
            "这 {} 条 chrome 文案没有中文翻译：{missing:?}",
            missing.len()
        );
    }

    #[test]
    fn english_is_always_the_original() {
        // 英文模式下取词一律原样返回，绝不去查表。
        assert_eq!(t(Lang::En, "New Session"), "New Session");
    }

    #[test]
    fn invert_toggle_has_both_labels() {
        // 过滤栏那一格：英文 `Invert`，中文 `反转`；悬停把 grep -v 语义写明。
        assert_eq!(t(Lang::En, "Invert"), "Invert");
        assert_eq!(t(Lang::Zh, "Invert"), "反转");
        assert_eq!(
            t(Lang::En, "Invert filter (grep -v): hide matching lines"),
            "Invert filter (grep -v): hide matching lines"
        );
        assert_eq!(
            t(Lang::Zh, "Invert filter (grep -v): hide matching lines"),
            "反转过滤（grep -v）：隐藏命中行"
        );
    }

    #[test]
    fn an_unknown_string_falls_back_to_english() {
        // 缺词退回原文——界面永远可用，不出现空串或键名。
        assert_eq!(t(Lang::Zh, "Some String We Never Wrote"), "Some String We Never Wrote");
    }

    #[test]
    fn tags_round_trip() {
        for l in Lang::ALL {
            assert_eq!(Lang::from_tag(l.tag()), Some(l));
            assert!(!l.native_name().is_empty());
        }
        assert_eq!(Lang::from_tag("zh_CN"), Some(Lang::Zh));
        assert_eq!(Lang::from_tag("en-US"), Some(Lang::En));
        assert_eq!(Lang::from_tag("de"), None);
    }

    #[test]
    fn the_default_language_is_english() {
        // 这一条是基线契约：默认语言一变，42 张 PNG 全要重冻结。
        assert_eq!(Lang::default(), Lang::En);
    }
}

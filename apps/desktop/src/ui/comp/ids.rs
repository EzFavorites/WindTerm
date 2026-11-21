//! `CompId` 的名字与判定表：`ALL` 名单、`name ` / `has_baseline ` / `page ` / `parse `。
//!
//! 从 `comp/mod.rs` 拆出来是为了 （`ui/` 单文件代码行 ≤ 500，不含注释）：
//! `mod.rs` 767 行里 566 行是代码，越线了。拆分只挪位置，`CompId` 仍然只活在
//! `ui` 内部（可见性一个字没改）。

use super::{last_page, CompId};
use crate::ui::*;

// 页组件名单由 `dialogs::session_nav::PAGES` 提供，这里不另抄一张表。
// 用页名而不是页码：`dialog_new_session_proxy.png` 比 `..._page_5.png`
// 一眼知道是哪页；改页序时文件名跟着语义走，不会悄悄错位。

impl CompId {
    /// 全部已登记的组件。脚本与报错信息都从这一处取，避免两处名单对不上。
    pub(in crate::ui) const ALL: &'static [CompId] = &[
        CompId::About,
        CompId::Preferences,
        CompId::Shortcuts,
        CompId::QuickCommands,
        CompId::QuickConfig,
        CompId::TerminalContext,
        CompId::ViewContext,
        CompId::ShellListPopup,
        CompId::OutlineMoreMenu,
        CompId::AddressBarDropdown,
        CompId::SessionLog,
        CompId::SshConnecting,
        CompId::SshHostKey,
        CompId::SshPassword,
        CompId::SshFailed,
        CompId::SshReplaceKey,
        CompId::FilerPrompt,
        CompId::ScriptRun,
        CompId::ScriptEditor,
        CompId::Palette,
        CompId::Goto,
        CompId::Center,
        CompId::Tunnels,
        CompId::TunnelSettings,
        CompId::Tmux,
        CompId::FloatingSidebar,
        CompId::SessionTree,
        // New Session 按**页**登记，不登记"整张弹窗"：弹窗整体由
        // `screenshot-dialog.png` 守位置；组件图逐页守内容。
        // 注册数必须等于 session_nav::PAGE_COUNT，多一个少一个都会
        // 让「每页都有独立基线」这句契约失效（round_trips 单测守）。
        CompId::NewSessionPage(0),
        CompId::NewSessionPage(1),
        CompId::NewSessionPage(2),
        CompId::NewSessionPage(3),
        CompId::NewSessionPage(4),
        CompId::NewSessionPage(5),
        CompId::NewSessionPage(6),
        CompId::NewSessionPage(7),
        CompId::NewSessionPage(8),
        CompId::NewSessionPage(9),
        CompId::NewSessionPage(10),
        CompId::NewSessionPage(11),
        CompId::NewSessionPage(12),
        CompId::NewSessionPage(13),
        CompId::NewSessionPage(14),
        CompId::NewSessionPage(15),
        CompId::NewSessionPage(16),
        CompId::NewSessionPage(17),
        CompId::NewSessionPage(18),
        CompId::NewSessionPage(19),
        CompId::NewSessionPage(20),
        CompId::NewSessionPage(21),
        // ：`Terminal` / `Window` 两个组头页（组头自己也是一页）。
        CompId::NewSessionPage(22),
        CompId::NewSessionPage(23),
        // Preferences 同理按**页**登记：整张弹窗那张 `dialog_settings` 只守住
        // 默认页（是 Application）的位置与外观。
        // 下标即 `SettingsPage::ALL` 的组件夹具页序，共 24 页；其中 5 页不在
        // 左栏显示，但逐页基线仍在。
        CompId::SettingsPage(0),
        CompId::SettingsPage(1),
        CompId::SettingsPage(2),
        CompId::SettingsPage(3),
        CompId::SettingsPage(4),
        CompId::SettingsPage(5),
        CompId::SettingsPage(6),
        CompId::SettingsPage(7),
        CompId::SettingsPage(8),
        CompId::SettingsPage(9),
        CompId::SettingsPage(10),
        CompId::SettingsPage(11),
        CompId::SettingsPage(12),
        CompId::SettingsPage(13),
        CompId::SettingsPage(14),
        CompId::SettingsPage(15),
        CompId::SettingsPage(16),
        CompId::SettingsPage(17),
        CompId::SettingsPage(18),
        CompId::SettingsPage(19),
        CompId::SettingsPage(20),
        CompId::SettingsPage(21),
        //  新增的 Triggers 页：下标 = `SettingsPage::ALL` 里 Triggers 的位置（22）。
        CompId::SettingsPage(22),
        // `Terminal.ManageOneKeys`：凭据管理器页，下标 = 23。
        CompId::SettingsPage(23),
        CompId::QuickCommandBar,
        CompId::StatusBar,
        CompId::ConfirmCloseWindow,
        CompId::ConfirmCloseSession,
        CompId::MenuSession,
        CompId::MenuLog,
        CompId::MenuMode,
        CompId::MenuEdit,
        CompId::MenuSearch,
        CompId::MenuSelection,
        CompId::MenuGoto,
        CompId::MenuView,
        CompId::MenuWindow,
        CompId::MenuHelp,
    ];

    /// 视觉基线专用夹具：快捷命令 2 个 + 右键菜单 2 个 + 未挂接弹层 3 个。
    /// 装配文件用一个 arm 收口它们，避免  的 500 行上限。
    pub(in crate::ui) fn is_reference_popup(self) -> bool {
        matches!(
            self,
            Self::QuickCommands
                | Self::QuickConfig
                | Self::TerminalContext
                | Self::ViewContext
                | Self::ShellListPopup
                | Self::OutlineMoreMenu
                | Self::AddressBarDropdown
        )
    }

    pub(in crate::ui) fn name(self) -> &'static str {
        match self {
            CompId::About => "dialog_about",
            CompId::Preferences => "dialog_settings",
            CompId::Shortcuts => "dialog_shortcuts",
            CompId::QuickCommands => "dialog_quick_commands",
            CompId::QuickConfig => "bar_quick_config",
            CompId::TerminalContext => "menu_terminal_context",
            CompId::ViewContext => "menu_view_context",
            CompId::ShellListPopup => "menu_shell_list",
            CompId::OutlineMoreMenu => "menu_outline_more",
            CompId::AddressBarDropdown => "menu_address_bar",
            CompId::SessionLog => "dialog_session_logging",
            CompId::SshConnecting => "dialog_ssh_connecting",
            CompId::SshHostKey => "dialog_ssh_hostkey",
            CompId::SshPassword => "dialog_ssh_password",
            CompId::SshFailed => "dialog_ssh_failed",
            CompId::SshReplaceKey => "dialog_ssh_replace_key",
            CompId::FilerPrompt => "dialog_filer_prompt",
            CompId::ScriptRun => "dialog_script_run",
            // D8-3 的窗口 首次冻结：编辑器只能从运行窗口的 `Edit`
            // 进（两级入口），不单独出图就永远没人看过它的像素。
            CompId::ScriptEditor => "dialog_script_editor",
            CompId::Palette => "overlay_palette",
            CompId::Goto => "overlay_goto",
            CompId::Center => "overlay_center",
            CompId::Tunnels => "panel_tunnels",
            CompId::TunnelSettings => "dialog_tunnel_settings",
            CompId::Tmux => "panel_tmux",
            CompId::FloatingSidebar => "panel_floating_sidebar",
            CompId::SessionTree => "panel_session_tree",
            // 页名来自 `session_nav::PAGES` 这张唯一的表，所以能返回
            // `&'static str`（不分配、不泄漏），也不怕加页时漏登记。
            CompId::NewSessionPage(n) => {
                dialogs::session_nav::component_for((n as usize).min(last_page()))
            }
            // Preferences 页：`dialog_settings_<页名>`。页名逐页写在
            // `settings_component` 的穷尽 match 里——加页忘了配名字编译不过；
            // 下标越界（命令行写不出这种名字）收敛到默认页，不 panic。
            CompId::SettingsPage(n) => settings_component(
                dialogs::SettingsPage::from_index(n as usize).unwrap_or_default(),
            ),
            CompId::QuickCommandBar => "bar_quick_command",
            CompId::StatusBar => "bar_status",
            CompId::ConfirmCloseWindow => "dialog_confirm_window",
            CompId::ConfirmCloseSession => "dialog_confirm_session",
            CompId::MenuSession => "menu_session",
            CompId::MenuLog => "menu_log",
            CompId::MenuMode => "menu_mode",
            CompId::MenuEdit => "menu_edit",
            CompId::MenuSearch => "menu_search",
            CompId::MenuSelection => "menu_selection",
            CompId::MenuGoto => "menu_goto",
            CompId::MenuView => "menu_view",
            CompId::MenuWindow => "menu_window",
            CompId::MenuHelp => "menu_help",
        }
    }

    /// 这个组件当前是否已冻结基线（未冻结的不进 `--components` 门禁）。
    ///
    /// 和 `ALL` 分开是诚实的做法：登记了但还没修好的组件，出图能看、
    /// 但不该参与逐字节比对——否则修好它反而被脚本判成「回归」。
    pub(in crate::ui) fn has_baseline(self) -> bool {
        match self {
            CompId::About
            | CompId::Preferences
            | CompId::Shortcuts
            | CompId::SessionLog
            | CompId::SshConnecting
            | CompId::SshHostKey
            | CompId::SshPassword
            | CompId::SshFailed
            | CompId::Palette
            | CompId::Goto
            | CompId::Center
            | CompId::Tunnels
            | CompId::TunnelSettings
            | CompId::Tmux
            | CompId::FloatingSidebar
            | CompId::SessionTree
            | CompId::QuickCommandBar
            | CompId::StatusBar
            | CompId::ConfirmCloseWindow
            | CompId::ConfirmCloseSession
            | CompId::MenuSession
            | CompId::MenuLog => true,
            // 十页的现状就是「当前实现的事实」，先冻结住，P5 逐页逐页比对时
            // 任何改动都会立刻显形——这比"改完再看有没有变丑"可靠。
            //
            // ：17..21 是视觉基线补页（Bell / Mode / Security / SFTP /
            // Telnet·Tcp 的 Connection），还没出过图，所以**不参与**逐字节
            // 比对；等首次冻结后再并进来（否则脚本会因为缺 PNG 直接红）。
            // ：17..21 是视觉基线补页（Bell / Mode / Security / SFTP /
            // Telnet·Tcp 的 Connection）。首轮只登记不出图（否则脚本因缺 PNG
            // 直接红）；**同日已首次出图并逐张核对**，现在一起并进逐字节比对。
            CompId::NewSessionPage(_) => true,
            // OneKeys：组件名单里**从来没有** `dialog_settings_onekeys.png`
            // （git 无记录；整批基线在 窗口容器改造后有意清空，
            // 等 /K1/K2/K3 落定再重冻，见 docs/facts.txt 的 `baselines=0`）。
            // 标成 gated 会让 `--components` 每次都报「基线不存在」——那是假红：
            // 不是图坏了，是这个组件从未冻结过。名单保留（夹具出图照样能看），
            // 但不进逐字节比对；重冻时 PNG 落进 git 再并回 gated。
            CompId::SettingsPage(n)
                if dialogs::SettingsPage::ALL.get(n as usize)
                    == Some(&dialogs::SettingsPage::OneKeys) =>
            {
                false
            }
            // Preferences 的 24 页：首轮出图即冻结，之后「哪一页被改坏」是
            // 逐字节问题，不再需要在一整张默认页截图里靠肉眼找。
            CompId::SettingsPage(_) => true,
            // B1-c：Mode 菜单夹具已出图并与 `mode/01-mode-menu.png`
            // 对过（勾选栏几何见 视觉基线），纳入比对。
            CompId::MenuMode => true,
            CompId::MenuEdit
            | CompId::MenuSearch
            | CompId::MenuSelection
            | CompId::MenuGoto
            | CompId::MenuView
            | CompId::MenuWindow
            |             CompId::MenuHelp => true,
            // ：换钥窗口出图核对过（高度按实测内容收回过一次，见
            // `REPLACE_RECT` 的注释），随首次出图一起纳入逐字节比对。
            CompId::SshReplaceKey => true,
            CompId::FilerPrompt => true,
            CompId::ScriptRun => true,
            CompId::ScriptEditor => true,
            // ：7 个视觉基线组件已目视确认，纳入逐字节比对。
            CompId::QuickCommands => true,
            CompId::QuickConfig => true,
            CompId::TerminalContext => true,
            CompId::ViewContext => true,
            CompId::ShellListPopup => true,
            CompId::OutlineMoreMenu => true,
            CompId::AddressBarDropdown => true,
        }
    }

    /// 页组件的页码（`None` = 不是页组件）。
    ///
    /// 只认 New Session：它的页码是 `session_nav` 的下标契约（`PAGE_COUNT` 那一套）。
    /// `SettingsPage` 的页序在 `SettingsPage::ALL` 里，和这里无关，别混进来。
    pub(in crate::ui) fn page(self) -> Option<u8> {
        match self {
            CompId::NewSessionPage(n) => Some(n.min(last_page() as u8)),
            _ => None,
        }
    }

    /// 名字 → 组件。未知名字返回 `None`（调用方负责报错并退出非零）。
    pub(in crate::ui) fn parse(name: &str) -> Option<Self> {
        Self::ALL.iter().copied().find(|c| c.name() == name)
    }
}

/// Preferences 页的基线名：`dialog_settings_<页名>`。
///
/// `<页名>` 是 `SettingsPage::key ` 的 slug（空格 / 斜杠换下划线、全小写），
/// 例如 `Shortcut Keys` → `dialog_settings_shortcut_keys`。`&'static str` 没法
/// 在运行期拼出来，所以只能逐页写成字面量；漏一页编译不过（穷尽 match），
/// 「名字的次序与组件夹具页序一致」由 `settings_page_baselines_follow_the_page_keys` 守。
fn settings_component(page: dialogs::SettingsPage) -> &'static str {
    use dialogs::SettingsPage as SP;
    match page {
        SP::Application => "dialog_settings_application",
        SP::Appearance => "dialog_settings_appearance",
        SP::Proxy => "dialog_settings_proxy",
        SP::Search => "dialog_settings_search",
        SP::Security => "dialog_settings_security",
        SP::Tabs => "dialog_settings_tabs",
        SP::ShortcutKeys => "dialog_settings_shortcut_keys",
        SP::Behaviour => "dialog_settings_behaviour",
        SP::ToolbarMenu => "dialog_settings_toolbar_menu",
        SP::Terminal => "dialog_settings_terminal",
        SP::AutoCompletion => "dialog_settings_auto_completion",
        SP::CommandHistory => "dialog_settings_command_history",
        SP::MouseTracking => "dialog_settings_mouse_tracking",
        SP::Text => "dialog_settings_text",
        SP::Binary => "dialog_settings_binary",
        SP::Caret => "dialog_settings_caret",
        SP::Font => "dialog_settings_font",
        SP::Highlight => "dialog_settings_highlight",
        SP::Wrap => "dialog_settings_wrap",
        SP::Widgets => "dialog_settings_widgets",
        SP::Filer => "dialog_settings_filer",
        SP::QuickBar => "dialog_settings_quick_bar",
        SP::Triggers => "dialog_settings_triggers",
        SP::OneKeys => "dialog_settings_onekeys",
    }
}

//! Preferences 对话框：**左侧分类树 + 右侧页面**（视觉基线重排）。
//!
//! # 语言开关为什么放这里
//!
//! 设置路径为 `Session ▸ Preferences ▸ Settings ▸ Appearance ▸ Language`，
//! 值写进 `user.config` 的 `application.language`（`en` / `zh-CN`）。视觉基线里
//! 「外观」页的内容装在一个标题为 **Application** 的分组框里（Language /
//! Theme / Window opacity 三行）——左栏选中的是 Appearance，右栏组标题是
//! Application，两者不矛盾，照抄。
//!
//! 两处刻意为之的不同：
//! - **改完立即生效**。文案没有缓存，`ViewState.lang` 每帧都读，不需要重启。
//!   按兼容终端 `03-settings/01-application.png` 清理：页脚那句橙色的
//!   「什么时候生效」提示整条删掉——兼容终端页脚只有右下角 `Close`，而我们的
//!   长句被按钮压掉半截（用户反馈「糟乱」）。各页自己的说明仍在页内。
//! - **落盘在 `prefs.json`**，与 `layout.json` 分开。
//!
//! # 后的三条硬约束
//!
//! 1. **默认页是 `Application`**（兼容终端打开就停在这一页），不是 `Appearance`。
//! 2. **左栏只画 [`SettingsPage::NAV_TOP`] 那 6 项 + 三个组头**，与上游 19 页一致。
//!    `Shortcut Keys` / `Behaviour` / `Toolbar / Menu` 上游不存在，是 Rust 重制版
//!    一度新增的三页，现已从设置页移除（对应 prefs 字段保留给原读者）。
//!    `Triggers` / `OneKeys` 是重制版新增的管理页，仍由 `Terminal.ManageTriggers`
//!    / `ManageOneKeys` 直接翻到（见 `effects.rs`）。
//! 3. **无标题栏**：兼容终端没有 `Preferences` 标题条与右上角 ×，经
//!    [`StableWindow::without_title_bar`] 单独关掉；关闭只走页脚 `Close`。
//!
//! #  边界
//!
//! - `Application ▸ Startup` 三个单选与 Session 1–4 下拉：真开关，
//!   绑 `win.startup` 并落 `prefs.json`；下拉只在「A specific session」档可用。
//! - Language 组合框：真开关（reduce + prefs 落盘）。
//! - Theme 组合框：真数据源（`theme_load` 只有一个 `dige-black`，列表就一项）。
//! - `Text ▸ Caret`（ + ）：样式 / 周期 / 宽度三项全真，整页自带写回，
//!   住在 `settings_live_caret`。三项都是**新会话初值**（`SessionConfig`），
//!   改完只影响之后新建的标签，已经开着的会话不动。
//! - `Text ▸ Font ▸ Font size` ：真开关，DragValue 绑
//!   `Prefs.font_size`，改完当场 `TerminalFonts::set_size`（重装字形 + 重新量
//!   cell 盒），松手落盘；启动时由 `construct.rs` 在第一次 `install` 之前装上。
//!   字族 / 字重仍灰显——缺的是字体选择器与字重安装（见 `settings_live_font`）。
//! - `Tabs` ：**部分真控件**。关闭后激活 / 双击动作 / 省略模式
//!   三行有消费者（`tab_strip::close_tab`、`tab_bar` 的响应与标题绘制），
//!   其余五行灰显并逐行写清缺什么（见 `settings_live_tabs`）。
//! - `Terminal`（**整页真控件**）。两个粘贴确认开关落
//!   `prefs.paste_confirm_*`，判据在 `dialogs::confirm_paste`，读备用屏的
//!   `GridSnapshot.alt`；右键三档与「选中即复制」落 `Prefs.right_click_action` /
//!   `copy_on_select`；`Enable local mode` 勾的是活动标签的只读态
//!   （见 `settings_live_terminal`）。
//! - `Text` ：**部分真控件**。
//!   隔行底色、打字隐光标、新标签抢焦点三个勾真的改行为并落 prefs；工具行开关
//!   真的收起那条面板（但**不落盘**，见 `settings_live_text`）。同一页里剩下的
//!   行仍是灰显空框——理由写在每行的悬停与页底说明里。
//! - `Text ▸ Zoom level` ：真控件，与 Font 页的 `Font size` 同一个
//!   `Prefs.font_size`，显示成相对出厂字号的档位，改完当场重装字形并落盘。
//! - `Highlight` ：`caret line` 与 `incremental search` 两行接真——
//!   前者是 `Prefs.text_highlight_caret_line` + 活动标签 `Tab.marks` + 渲染器；
//!   后者是 `Prefs.text_highlight_search`，那个效果本来就无条件在画，补的是**关掉
//!   它的字段**（`ui/terminal/pane_overlays.rs` 的  那段），不是补渲染。
//!   其余三行无状态无渲染器，仍灰显（见 `settings_live_highlight`）。
//!   旧的「parses to no action」与「没有渲染器」两句说明都已更正。
//! - Window opacity（，**真控件**）：egui 0.29 的 `ViewportCommand` 确实没有
//!   数值 opacity 命令（只有 `Transparent(bool)`，管 OS 窗口能不能透），所以
//!   数值效果由 `egui::Visuals` 背景 alpha + 终端区底色合成，不靠框架升级。
//!   这里放开关 + `[0.3, 1.0]` 滑杆，改完写 `Prefs` 与运行期副本；运行期
//!   切换 OS 透明由 `ui/window_opacity.rs::sync_window_opacity` 只在值变化时
//!   补发一次 `ViewportCommand::Transparent`。旧对话框里那几行终端信息
//!   （Terminal Type / Scrollback）仍是展示，真实值在 `SettingsData` 里供
//!   命令中心当默认值。
//!
//! # 组件基线
//!
//! `dialog_settings.png` 与 22 张 `dialog_settings_<页>.png`（末页是隐藏的
//! 只读 `Script & API` 页）在这次清理后全量重冻结：默认页改 `Application`、
//! 去掉标题栏与橙色页脚、左栏回收成 6 项 + 三个组头。

use crate::ui::kit::window::{DialogId, DialogWindow};
use super::settings_live;
use super::settings_live_binary;
use super::settings_live_caret;
use super::settings_live_font;
use super::settings_live_highlight;
use super::settings_live_quickbar;
use super::settings_live_completion;
use super::settings_live_search;
use super::settings_live_proxy;
use super::settings_live_security;
use super::settings_live_tabs;
use super::settings_live_terminal;
use super::settings_live_mouse;
use super::settings_live_text;
use super::settings_live_triggers;
use super::settings_live_wrap;
use super::settings_live_onekeys;
use super::settings_live_script_api;
use super::settings_pages_view;
use crate::ui::*;
use term_app::i18n::Lang;
use term_app::shell::ShellAction;

// 弹窗 602×568（尺寸在 `kit::spec::DialogId::Settings`）。
// 视觉基线 `03-settings/01-application.png` 实测窗口外框物理 22..1225 × 22..1157
// ⇒ 逻辑 602×568；左栏导航物理 40..431 ⇒ 逻辑宽 196，右栏表单物理 458..1216。
// 高度 572 是 从 500 加上来的：左栏当时涨到 22 行装不下，页脚被顶
// 出窗口；收回 6 顶层页 + 三个组头后只剩 19 行（456），572 很宽裕，
// 正好留下兼容终端那种大片空白。**下面的 CONTENT_W / RIGHT_W / PAGE_H 与它成套**。
/// 内容整体下移：视觉基线左栏选中块从 y36 起，当前布局贴着弹窗上沿（y20）。
/// 统一平移 8 逻辑点 = 16 物理像素，左右栏一起走，避免各调各的。
const CONTENT_TOP_PAD: f32 = 8.0;
/// 左栏宽度（视觉基线实测：导航选中块右缘 x≈409 物理=204.5 逻辑，扣掉内容左内边距
/// 12 ⇒ 192.5，取 190+8 的整数档）。**改它会整体平移右栏**，与 `RIGHT_W` 一起动：
/// 视觉基线右栏分组框左缘 216.75、右缘 592.75，两块拼起来才是 1:1。
const NAV_W: f32 = 200.0;
/// 窗口内容区宽度（弹窗 560 宽 − 边框与内边距 ≈26）。
/// 只在设置弹窗里卡死：其它弹窗各有自己的最小尺寸策略，别在通用层动。
const CONTENT_W: f32 = 578.0;
/// 右栏可用宽度：视觉基线分组框外框实测 `454..1207` 物理 ⇒ 376 逻辑（扣 1.5 描边）。
/// 老值 365 会让框右边缩到 575，右边留出 17 逻辑点的空白——用户
/// 反馈的「留大段空白」有一半来自这里，不是输入框本身窄。
const RIGHT_W: f32 = 374.0;
/// 树行高。
const ROW_H: f32 = 24.0;
/// 子项缩进。
const INDENT: f32 = 20.0;
/// 内容区最小高度：切页时窗口不该跟着内容一起跳。
///
/// 440 → 512 与弹窗高度同步加高（72px）。左栏现在最多 19 行（456），
/// 页脚再留一行也够；这个数和 `kit::spec` 里的高度仍要一起动，否则又会出现
/// 「页脚被顶出窗口」。
const PAGE_H: f32 = 540.0;
/// 组合框宽度（视觉基线 ≈230）。
const COMBO_W: f32 = 238.0;
/// 树选中行的灰块 / 悬停（视觉基线选中 = #3A3A3A 整行）。
const TREE_SELECTED: u32 = 0x3A3A3A;
const TREE_HOVER: u32 = 0x2A2A2A;
/// 分组框描边。
const GROUP_STROKE: u32 = 0x4A4A4A;
/// 必填项红星。
const STAR_RED: u32 = 0xE0554D;
use crate::ui::kit::spec::col;

/// 右侧页面。顺序即左栏树的叶序；三个组头（Terminal/Text/Widgets）不是页。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub(in crate::ui) enum SettingsPage {
    /// 兼容终端打开 Preferences 就停在这一页（`03-settings/01-application.png`
    /// 里左栏高亮的是 Application）。
    #[default]
    Application,
    Appearance,
    Proxy,
    Search,
    Security,
    Tabs,
    /// 组头 `Terminal ▾` 自己也是一页：视觉基线 `07-terminal.png` 点了它之后
    /// 右侧有三个分组框。上一版把组头当纯折叠按钮，点了什么都不出。
    Terminal,
    AutoCompletion,
    CommandHistory,
    MouseTracking,
    /// 同上，`11-text.png`。
    Text,
    Binary,
    Caret,
    Font,
    Highlight,
    Wrap,
    /// 视觉基线 `17-widgets.png`：点组头是个**空页**（右侧一片空白）。
    Widgets,
    Filer,
    QuickBar,
    /// 触发器规则编辑器（事件 → 匹配 → 动作）。这一页是真的：编辑结果直接
    /// 写回 `prefs.triggers`，并由 `TerminalWindow.triggers` 在每帧 `poll` 里喂给求值器。
    Triggers,
    /// `Terminal.ManageOneKeys`：凭据管理器（SSH / Account / Sessions 三标签）。
    /// 编辑结果落 `prefs` 并由 `auth_chain` 在连接时读取。
    OneKeys,
    /// 隐藏的末页：只读的 `Script & API` 状态 + 打开既有管理器的按钮。
    /// 不进左栏树、不进 `TOP` / 组，只为组件基线留一个槽位。
    ScriptApi,
}

impl SettingsPage {
    /// 组头以下的叶序（画树时展开用）。
    const TERMINAL: [SettingsPage; 3] = [
        SettingsPage::AutoCompletion,
        SettingsPage::CommandHistory,
        SettingsPage::MouseTracking,
    ];
    const TEXT: [SettingsPage; 5] = [
        SettingsPage::Binary,
        SettingsPage::Caret,
        SettingsPage::Font,
        SettingsPage::Highlight,
        SettingsPage::Wrap,
    ];
    const WIDGETS: [SettingsPage; 2] = [SettingsPage::Filer, SettingsPage::QuickBar];

    /// **左栏可见的顶层叶**：兼容终端 `03-settings/01-application.png` 逐行数的
    /// 六项，顺序不许动。「混入了其他东西」指的就是后来
    /// 挂进这棵树的那五页；它们从树上去掉，但实现与逐页组件基线都保留。
    const NAV_TOP: [SettingsPage; 6] = [
        SettingsPage::Application,
        SettingsPage::Appearance,
        SettingsPage::Proxy,
        SettingsPage::Search,
        SettingsPage::Security,
        SettingsPage::Tabs,
    ];

    /// **全部顶层页**（含不显示在左栏的管理页），只用于派生 [`SettingsPage::ALL`]
    /// ——即组件夹具 `SettingsPage(i)` 的页序，不是左栏顺序。
    const TOP: [SettingsPage; 8] = [
        SettingsPage::Application,
        SettingsPage::Appearance,
        SettingsPage::Proxy,
        SettingsPage::Search,
        SettingsPage::Security,
        SettingsPage::Tabs,
        // 触发器规则编辑器。放在顶层叶末尾（组头之前）。
        SettingsPage::Triggers,
        // `Terminal.ManageOneKeys`：凭据管理器（SSH / Account / Sessions）。
        SettingsPage::OneKeys,
    ];

    /// 三个组头页，顺序与 `CHILD_PAGES` 一致。
    pub(in crate::ui) const GROUPS: [SettingsPage; 3] = [
        SettingsPage::Terminal,
        SettingsPage::Text,
        SettingsPage::Widgets,
    ];

    /// 全部页（测试与 `settings_pages` 的覆盖断言用它枚举）。
    pub(in crate::ui) const ALL_PAGES: [SettingsPage; 22] = [
        SettingsPage::Application,
        SettingsPage::Appearance,
        SettingsPage::Proxy,
        SettingsPage::Search,
        SettingsPage::Security,
        SettingsPage::Tabs,
        SettingsPage::Terminal,
        SettingsPage::AutoCompletion,
        SettingsPage::CommandHistory,
        SettingsPage::MouseTracking,
        SettingsPage::Text,
        SettingsPage::Binary,
        SettingsPage::Caret,
        SettingsPage::Font,
        SettingsPage::Highlight,
        SettingsPage::Wrap,
        SettingsPage::Widgets,
        SettingsPage::Filer,
        SettingsPage::QuickBar,
        SettingsPage::Triggers,
        SettingsPage::OneKeys,
        SettingsPage::ScriptApi,
    ];

    /// **组件夹具的页序**（`CompId::SettingsPage(i)` 的下标就是这里的下标）：
    /// `TOP` → 三个「组头 + 子页」。
    ///
    /// 它**不再等于左栏顺序**：左栏只画 [`SettingsPage::NAV_TOP`]
    /// 那六项，其余页仍要有独立基线逐页验收，所以继续留在 `ALL` 里。
    /// 从上面四个分组常量派生，不另抄一份名单；`[..; 22]` 的长度是最后一道
    /// 闸——少写一行就编译不过，也不会让 22 张已冻结的 PNG 集体错位。
    pub(in crate::ui) const ALL: [SettingsPage; 22] = {
        let mut out = [SettingsPage::Application; 22];
        let mut n = 0;
        let mut i = 0;
        while i < SettingsPage::TOP.len() {
            out[n] = SettingsPage::TOP[i];
            n += 1;
            i += 1;
        }
        let mut g = 0;
        while g < SettingsPage::GROUPS.len() {
            out[n] = SettingsPage::GROUPS[g];
            n += 1;
            let kids = CHILD_PAGES[g];
            let mut k = 0;
            while k < kids.len() {
                out[n] = kids[k];
                n += 1;
                k += 1;
            }
            g += 1;
        }
        // 隐藏末页：只在 ALL 尾端追加，TOP / NAV_TOP / CHILD_PAGES 一律不动，
        // 这样前 21 张已冻结的组件基线页序不变。
        out[n] = SettingsPage::ScriptApi;
        out
    };

    /// 页在 `ALL` 里的下标（组件夹具的 `SettingsPage(i)` 用的就是它）。
    pub(in crate::ui) fn index(self) -> usize {
        Self::ALL
            .iter()
            .position(|p| *p == self)
            .expect("SettingsPage::ALL 必须收录每一页")
    }

    /// `index` 的反函数；越界返回 `None`（夹具调用方自己决定回落哪一页）。
    pub(in crate::ui) fn from_index(i: usize) -> Option<Self> {
        Self::ALL.get(i).copied()
    }

    /// 英文原文 = 词表的 key（见 `term_app::i18n`）。
    fn key(self) -> &'static str {
        match self {
            SettingsPage::Application => "Application",
            SettingsPage::Appearance => "Appearance",
            SettingsPage::Proxy => "Proxy",
            SettingsPage::Search => "Search",
            SettingsPage::Security => "Security",
            SettingsPage::Tabs => "Tabs",
            SettingsPage::Terminal => "Terminal",
            SettingsPage::AutoCompletion => "Auto Completion",
            SettingsPage::CommandHistory => "Command History",
            SettingsPage::MouseTracking => "Mouse Tracking",
            SettingsPage::Text => "Text",
            SettingsPage::Binary => "Binary",
            SettingsPage::Caret => "Caret",
            SettingsPage::Font => "Font",
            SettingsPage::Highlight => "Highlight",
            SettingsPage::Wrap => "Wrap",
            SettingsPage::Widgets => "Widgets",
            SettingsPage::Filer => "Filer",
            SettingsPage::QuickBar => "Quick Bar",
            SettingsPage::Triggers => "Triggers",
            SettingsPage::OneKeys => "OneKeys",
            SettingsPage::ScriptApi => "Script & API",
        }
    }
}

/// Settings dialog state.
///
/// 注意：`term` / `scrollback` **不再显示在对话框里**（退场），
/// 但「命令中心」还拿它们当默认值的初值（`default_font_size` 同理）——
/// 别看到没人显示就顺手删掉。`groups_open` 是三个组头的折叠态。
pub(in crate::ui) struct SettingsData {
    pub(in crate::ui) term: String,
    pub(in crate::ui) scrollback: String,
    pub(in crate::ui) font: String,
    pub(in crate::ui) font_size: f32,
    // 这里曾有一个 `caret: usize`，全仓零读者（清掉）：光标样式
    // 现在的家是 `Prefs.caret_style` → `TerminalWindow.caret_style`，两个同名字段
    // 并排只会让人以为其中一个是「另一份真相」。
    pub(in crate::ui) groups_open: [bool; 3],
}

/// 三个组头的子页（画树展开用）。
const CHILD_PAGES: [&[SettingsPage]; 3] = [
    SettingsPage::TERMINAL.as_slice(),
    SettingsPage::TEXT.as_slice(),
    SettingsPage::WIDGETS.as_slice(),
];

impl TerminalWindow {
    /// 选语言：一次动作同时改内存（reduce）与磁盘（prefs）。
    ///
    /// 落盘失败不回滚——界面语言仍然当场生效，只是下次启动回到默认；
    /// `prefs::save` 与 `layout_state::save` 一样是「尽力而为」的可用性资产。
    fn set_language(&mut self, lang: Lang) {
        self.dispatch_action(Action::Shell(ShellAction::SetLanguage(lang)));
        // 同帧收口：reducer 写 ViewState.lang、这里落 prefs，两者必须是同一帧的
        // 一个原子变化。早先只靠 dispatch，万一派发在某些入口被排到下一帧，
        // 就会出现「界面仍是旧语言、prefs 已是新语言」的一帧分叉。这里再同步
        // 写一次（值相同则幂等），不让语言与偏好短暂各说各话。
        self.view.lang = lang;
        // load-modify-save：抑制位等其余字段必须跟着保留，整份重建会把
        // 「不再提示」的用户选择悄悄清掉。
        let mut prefs = term_runtime::prefs::load().unwrap_or_default();
        prefs.language = lang.tag().to_string();
        term_runtime::prefs::save(&prefs);
    }

    pub(in crate::ui) fn draw_settings_dialog(&mut self, ctx: &egui::Context) {
        // 文案在进闭包之前取好：闭包会以 `&mut self` 捕获窗口，
        // 里面再借 `self.tr ` 就会撞借用检查。
        let title = self.tr("Preferences");
        // 左栏只翻译 `NAV_TOP` 那六项；`TOP` 里剩下的五页没有树行。
        let top = SettingsPage::NAV_TOP.map(|p| self.tr(p.key()));
        let groups = ["Terminal", "Text", "Widgets"].map(|g| self.tr(g));
        let children: [&[&str]; 3] = [
            &(SettingsPage::TERMINAL.map(|p| self.tr(p.key())))[..],
            &(SettingsPage::TEXT.map(|p| self.tr(p.key())))[..],
            &(SettingsPage::WIDGETS.map(|p| self.tr(p.key())))[..],
        ];
        let close = self.tr("Close");
        let current = self.lang();
        let page = self.settings_page;
        let groups_open = self.settings.groups_open;
        // Appearance 页的窗口透明度同样先按值取出来（WindowOpacity 是 Copy）。
        let window_opacity = self.window_opacity;

        let open = DialogWindow::new(DialogId::Settings, title)
        // 兼容终端没有 `Preferences` 标题条和右上角 ×（用户对比图）。
        // 关掉标题栏后 `.open ` 不再产生关闭按钮，唯一的关闭入口是页脚 Close。
        .without_title_bar()
        // 视觉基线 `01-application.png` 实测：内容首行顶边在 dialog 内容区顶边
        // 下方约 16 逻辑像素。用 Frame 的 top margin 统一下移左右两栏，
        // 保持二者的相对布局不变（往 left/right 栏各塞 add_space 会让两栏错位）。
        .with_top_pad(16.0)
        .show_with_rect(ctx, |ui, win| {
            // 字体统一 ：字号阶梯与字体族只在 kit/type_scale 定义，
            // 设置页直接用全局那套等宽 ladder。之前这里另写一遍 11/12/13 的
            // monospace 循环，改字号必须改两处，正是「看起来不统一」的来源。
            crate::ui::kit::type_scale::install_text_styles(ui.style_mut());
            // 宽度也要钉死：`StableWindow` 的 `fixed_rect` 只保证「不小于」，
            // 内容更宽时窗口仍会被顶大，而页脚是按内层 ui 宽度右对齐的——
            // 结果就是「窗口长到 627、`Close` 画在 560 之外被裁掉」。
            // 上下都卡住，超宽的内容只会被裁在自己格子里，不会挤掉按钮。
            ui.set_max_width(CONTENT_W);
            // 视觉基线垂直基准校准见 settings.rs::CONTENT_TOP_PAD；对整体内容
            // 施加一次统一平移，避免左栏/右栏走不同布局路径时各移各的。
            let _ = crate::ui::kit::window::PAD;
            let mut picked_page = None;
            let mut picked_lang = None;
            let mut toggled_group = None;
            let mut open_group = None;
            let mut suppress_changed = false;
            let mut startup_changed = false;
            let mut want_close = false;
            ui.add_space(CONTENT_TOP_PAD);
            ui.horizontal(|ui| {
                ui.set_min_height(PAGE_H - 34.0);
                // ── 左栏：分类树 ──
                ui.vertical(|ui| {
                    ui.set_width(NAV_W);
                    ui.spacing_mut().item_spacing.y = 0.0;
                    for (i, p) in SettingsPage::NAV_TOP.into_iter().enumerate() {
                        if tree_row(ui, NAV_W - 8.0, 0.0, top[i], p == page) {
                            picked_page = Some(p);
                        }
                    }
                    for gi in 0..3 {
                        let mark = if groups_open[gi] { "▼" } else { "▸" };
                        let row = format!("{mark} {}", groups[gi]);
                        let group_page = SettingsPage::GROUPS[gi];
                        // 组头既是**页**也是折叠开关（视觉基线 `07-terminal.png`：
                        // 选中 Terminal 组头时右侧有三个分组框）。第一次点＝选中
                        // 并展开；已经在这一页时再点才收起。
                        if tree_row(ui, NAV_W - 8.0, 0.0, &row, group_page == page) {
                            picked_page = Some(group_page);
                            if page == group_page {
                                toggled_group = Some(gi);
                            } else if !groups_open[gi] {
                                open_group = Some(gi);
                            }
                        }
                        if groups_open[gi] {
                            for (ci, p) in CHILD_PAGES[gi].iter().enumerate() {
                                if tree_row(ui, NAV_W - 8.0, INDENT, children[gi][ci], *p == page) {
                                    picked_page = Some(*p);
                                }
                            }
                        }
                    }
                });
                // 分隔线：视觉基线是一条贴着导航右缘的**暗**竖线（#161616 左右），
                // egui 默认 separator 是亮灰（实测 #303030），会显得突兀。
                // 用 painter 画一条贯穿内容高度的暗线，视觉更贴参考。
                let sep_x = ui.cursor().left();
                let top = ui.cursor().top();
                let bottom = top + (PAGE_H - 34.0);
                ui.painter().line_segment(
                    [egui::pos2(sep_x, top), egui::pos2(sep_x, bottom)],
                    egui::Stroke::new(1.0, col(0x1A1A1A)),
                );
                ui.add_space(1.0);
                // ── 右栏：当前页 ──
                ui.vertical(|ui| {
                    // 右栏**硬上限**。窗口是钉死尺寸的（`StableWindow` + `SETTINGS_RECT`），
                    // 内容一旦比它宽，egui 会把窗口顶大，而裁掉的是页脚与 `Close` 按钮
                    //（有头实测：22 页里 12 页把窗口顶到 610–729）。
                    // 有了这条，任何新页再宽也只是自己的内容被截断，不会把按钮挤出屏幕。
                    ui.set_max_width(RIGHT_W);
                    // 高度也钉死：任何页内容再多也只在自己这块区域里滚动，
                    // 不许把固定尺寸的对话框顶高（否则页脚 Close 会压在内容上）。
                    egui::ScrollArea::vertical()
                        .max_height(PAGE_H - 34.0)
                        .auto_shrink([false, false])
                        .show(ui, |ui| {
                    // 行距在这里注入**一次**，所有页（数据页与 live 页）都从这里取。
                    // 各页原先各写 6 / 8 / 0，同一张弹窗里页与页之间的疏密不同；
                    // 现在只有一个出处，页内再调也只是重复同一个值。
                    settings_pages_view::set_row_rhythm(ui);
                    match page {
                        // 有状态的页在 `settings_live.rs`；其余页走数据表。
                        SettingsPage::Appearance => {
                            let edit = settings_live::appearance(
                                ui, current, window_opacity, &self.palette.theme_name.clone(),
                            );
                            if let Some(l) = edit.lang {
                                picked_lang = Some(l);
                            }
                            if let Some(name) = edit.theme_name {
                                self.apply_theme(&name);
                            }
                            // 开关 / 滑杆改完当场写运行期副本 + 落盘，
                            // 与菜单三档预设同一个入口。
                            if let Some(next) = edit.window_opacity {
                                self.set_window_opacity(next);
                            }
                        }
                        // Font 页的 Size 是真控件（改完当场重装字形并落 prefs），
                        // 字族 / 字重仍灰显——理由在那一页的说明里。
                        SettingsPage::Font => settings_live_font::font(self, ui, current),
                        // Quick Bar 页：三个开关读写 QuickBarDisplay + Prefs。
                        SettingsPage::QuickBar => settings_live_quickbar::quickbar(self, ui),
                        // Wrap 页：折行符号 / 参考列 / 列号三项真控件。
                        SettingsPage::Wrap => settings_live_wrap::wrap(self, ui),
                        // Auto Completion 页：十行真控件（I-AC）。
                        SettingsPage::AutoCompletion => {
                            settings_live_completion::auto_completion(self, ui)
                        }
                        // Tabs 页：关闭后激活 / 双击 / 省略模式三行是真控件。
                        SettingsPage::Tabs => settings_live_tabs::tabs(self, ui),
                        // Highlight 页：caret line 与 incremental search 是真勾选
                        // （前者 Tab.marks、后者窗口级），其余三行仍灰显。
                        SettingsPage::Highlight => settings_live_highlight::highlight(self, ui),
                        // Terminal 页：两个粘贴确认开关是真控件，其余两行灰显。
                        SettingsPage::Terminal => settings_live_terminal::terminal(self, ui),
                        // Mouse Tracking：四类事件掩码是真勾选；双击没有协议事件，仍灰。
                        SettingsPage::MouseTracking => settings_live_mouse::mouse_tracking(self, ui),
                        SettingsPage::Application => {
                            let change = settings_live::application(self, ui);
                            suppress_changed |= change.suppress;
                            startup_changed |= change.startup;
                        }
                        // Caret 页：样式 / 周期 / 宽度三项都是真控件，写回也在
                        // 那一页里（`settings_live_caret`），这里只调一行。
                        SettingsPage::Caret => settings_live_caret::caret(self, ui),
                        // 容量 / TTL / 落盘开关三个字段都有读者，改完当场
                        // 重建历史表并写回 prefs（见 `settings_live::command_history`）。
                        SettingsPage::CommandHistory => {
                            settings_live::command_history(self, ui);
                        }
                        // Search 页同样自带读写（模板落 `prefs.search_url`，
                        // 消费者是 `effects.rs::search_online`）。
                        SettingsPage::Search => settings_live_search::search(ui),
                        SettingsPage::Security => settings_live_security::security(self, ui),
                        SettingsPage::Proxy => settings_live_proxy::proxy_page(self, ui),
                        // Binary 页四个数字框 + 一格斑马线：**只写默认参数**
                        // （`hex_defaults` + `prefs.hex_*` /
                        // `prefs.alternate_column_bg`），一个字节都不碰
                        // 已经开着的标签——那正是「默认」的含义，也是这一页
                        // 看不见窗口的原因（见 `settings_live_binary` 模块头）。
                        SettingsPage::Binary => {
                            let edit = settings_live_binary::binary(ui, self.hex_defaults);
                            // 拖动时每帧跟手（数字得走），落盘只在松手那一下：
                            // 分开两个标志的理由见 `BinaryEdit`。
                            if let Some(next) = edit.apply {
                                self.hex_defaults = next;
                            }
                            if edit.persist {
                                let next = self.hex_defaults;
                                // load-modify-save：只动这五项，语言 / 字号 /
                                // 标签行为原样保留。
                                //
                                // 五项的写目标是**同一层**（prefs 里的默认值）。
                                // 斑马线曾经写的是窗口上的 `alternate_column_bg`，
                                // 而那个字段被 `bin::paint` 每帧直接读——于是这一
                                // 格改完当场改掉所有已开标签，同页四个数字却只影响
                                // 新标签。同一条路径写下来，五个参数一个语义。
                                settings_live::save_prefs(|p| {
                                    p.hex_base = next.base;
                                    p.hex_columns = next.columns;
                                    p.hex_group = next.group;
                                    p.hex_division = next.division;
                                    p.alternate_column_bg = next.zebra;
                                });
                            }
                        }
                        // Filer 页要动窗口上的面板（勾完当场重列），所以给 `self`。
                        SettingsPage::Filer => settings_live::filer(self, ui),
                        // ：三页**部分真控件**——每页只有一两行接了
                        // 字段，其余行仍走数据表那套灰显画法（同一页里两种控件
                        // 并存，是真接线的一半，不是回退）。见那边模块文档。
                        SettingsPage::Text => settings_live_text::text(self, ui),
                        // 触发器规则编辑器。这一页是真的——改完当场喂进每帧
                        // poll 的求值器，按「Save」落盘进 prefs.triggers。
                        SettingsPage::Triggers => settings_live_triggers::triggers(self, ui),
                        // `Terminal.ManageOneKeys`：凭据管理器（SSH / Account /
                        // Sessions 三标签）。编辑结果落 prefs，由 auth_chain 读取。
                        SettingsPage::OneKeys => settings_live_onekeys::onekeys(self, ui),
                        // 隐藏末页：只读状态 + 既有管理入口，按钮先关设置再开弹窗。
                        SettingsPage::ScriptApi => settings_live_script_api::script_api(self, ui),
                        // 只读的快捷键表也要运行期数据：覆盖表活在窗口上
                        // （`dialogs::shortcuts` 写入），数据层只给静态骨架。
                        other => {
                            settings_pages_view::draw(ui, other)
                        }
                    }
                        });
                });
            });
            // 页脚只有右下角 Close（兼容终端如此）。此前左侧那句橙色「什么时候生效」
            // 会随页变长并被 Close 压掉半截，按用户对比图整条删除；
            // 各页自己的说明留在页内，不重复挂到页脚。
            //
            // 按钮按**窗口矩形**绝对定位，不跟流式内容走：Tabs 页的长下拉曾把
            // 窗口顶宽，右对齐的页脚被裁到只剩一个 `C`。现在即便某一页再宽，
            // Close 也钉在右下角，且 clip 区正好是按钮自己的矩形。
            // 右 / 下内边距必须与 `DialogWindow` 的内容 clip 区一致（PAD=12），
            // 再小就会被裁掉；视觉基线的实际观感由按钮自身的描边补足。
            // 页脚 Close 用**窗口矩形**绝对定位，不跟随左栏/右栏的 top_pad 下移。
            // 边距按视觉基线 `01-application.png` 实测：蓝框右/下各距 dialog
            // 外框 18 物理像素 = 9 逻辑像素（外框 x20..1227/y20..1159，
            // 蓝框 x1050..1209/y1094..1141）。此前 pad=1 几乎贴在外框上。
            let pad = 9.0;
            let btn_w = 80.0;
            let btn_h = 24.0;
            let footer = egui::Rect::from_min_max(
                egui::pos2(win.right() - pad - btn_w, win.bottom() - pad - btn_h),
                egui::pos2(win.right() - pad, win.bottom() - pad),
            );
            super::dialog_chrome::place(ui, footer, |ui| {
                // 尺寸/配色来自 `kit::spec::FooterButton::SettingsClose`：
                // 这里只负责把按钮放进窗口绝对定位的矩形并收点击。
                let spec = crate::ui::kit::spec::FooterButton::SettingsClose.spec();
                if crate::ui::kit::footer::widget_button(ui, close, spec, true).clicked() {
                    want_close = true;
                }
            });
            if let Some(p) = picked_page {
                self.settings_page = p;
            }
            if let Some(l) = picked_lang {
                self.set_language(l);
            }
            if let Some(gi) = toggled_group {
                self.settings.groups_open[gi] = !self.settings.groups_open[gi];
            }
            if let Some(gi) = open_group {
                self.settings.groups_open[gi] = true;
            }
            // 注意：Caret 页（/）不在这里写回——它自带窗口副本与落盘，
            // 由 `settings_live_caret::caret` 一次做完，本处只管这三项。
            if suppress_changed || startup_changed {
                // 一次 load-modify-save 同时落两组字段：分开写会让后写那份
                // 把先写的覆盖掉（两者共用 prefs.json）。其余字段原样保留。
                let mut prefs = term_runtime::prefs::load().unwrap_or_default();
                if suppress_changed {
                    prefs.suppress_close_window = self.confirm.suppress_window;
                    prefs.suppress_close_session = self.confirm.suppress_session;
                }
                if startup_changed {
                    prefs.startup_mode = self.startup.mode.tag().to_string();
                    prefs.startup_sessions = self.startup.specific.clone();
                }
                term_runtime::prefs::save(&prefs);
            }
            if want_close {
                self.view.show_settings = false;
            }
        });
        // 无标题栏时 egui 不会产生右上角 ×，`open` 恒为 true；这一句留着是
        // 兜住「将来又打开标题栏」那条路径，与页脚 Close 同一个关闭状态。
        if !open {
            self.view.show_settings = false;
        }
    }
}

/// 左栏树的一行：整行灰块选中 / 悬停（视觉基线），等宽文本左对齐。
/// 返回是否被点。手绘而不是 `SelectableLabel`：后者只有文字宽，
/// 高亮不满行。
fn tree_row(ui: &mut egui::Ui, width: f32, indent: f32, label: &str, selected: bool) -> bool {
    let (rect, resp) = ui.allocate_exact_size(egui::vec2(width, ROW_H), egui::Sense::click());
    let painter = ui.painter_at(rect);
    if selected {
        painter.rect_filled(rect, 3.0, col(TREE_SELECTED));
    } else if resp.hovered() {
        painter.rect_filled(rect, 3.0, col(TREE_HOVER));
    }
    let font = egui::TextStyle::Body.resolve(ui.style());
    painter.text(
        rect.left_center() + egui::vec2(6.0 + indent, 0.0),
        egui::Align2::LEFT_CENTER,
        label,
        font,
        col(0xE0E0E0),
    );
    resp.clicked()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 选语言那一刻，界面语言与落盘偏好必须是同帧的同一个值，不许先分叉一帧。
    #[test]
    fn choosing_a_language_syncs_view_and_prefs_in_the_same_frame() {
        use term_app::i18n::Lang;
        crate::ui::lifecycle_test_support::with_temp_profile(|| {
            let ctx = egui::Context::default();
            let pool = std::sync::Arc::new(term_session::SessionPool::new());
            let mut win = crate::ui::lifecycle_test_support::window_with(pool, &ctx, Vec::new());
            win.view.lang = Lang::En;
            win.set_language(Lang::Zh);
            assert_eq!(win.view.lang, Lang::Zh, "ViewState 当帧就切到新语言");
            // 借既有的 load-modify-save 入口读盘（不为测试新增门面引用）：闭包里
            // 只断言不改字段，落回去的还是同一份 prefs。
            super::settings_live::save_prefs(|p| {
                assert_eq!(p.language, Lang::Zh.tag(), "prefs 与界面必须同值");
            });
        });
    }

    /// 用户 明确要求「设置默认打开应该是 application 页面才对」。
    #[test]
    fn preferences_defaults_to_the_application_page() {
        assert_eq!(SettingsPage::default(), SettingsPage::Application);
        assert_eq!(SettingsPage::from_index(0), Some(SettingsPage::Application));
    }

    /// 「混入了其他东西」的防回归：左栏顶层只有兼容终端那六项，两个新增管理页
    /// 不许再挂回树上（它们仍有自己的入口 / 组件基线）。
    #[test]
    fn the_nav_tree_has_only_the_six_original_top_level_pages() {
        assert_eq!(
            SettingsPage::NAV_TOP,
            [
                SettingsPage::Application,
                SettingsPage::Appearance,
                SettingsPage::Proxy,
                SettingsPage::Search,
                SettingsPage::Security,
                SettingsPage::Tabs,
            ]
        );
        for hidden in [
            SettingsPage::Triggers,
            SettingsPage::OneKeys,
        ] {
            assert!(!SettingsPage::NAV_TOP.contains(&hidden), "{hidden:?} 混进了左栏");
            assert!(SettingsPage::TOP.contains(&hidden), "{hidden:?} 仍须可翻到");
        }
    }

    /// 组件基线按 `ALL` 的下标登记（22 张 PNG：上游 19 页 + 2 个管理页 + 1 个
    /// 隐藏只读页）。
    #[test]
    fn hidden_pages_keep_their_component_baseline_slots() {
        assert_eq!(SettingsPage::ALL.len(), 22);
        for hidden in [
            SettingsPage::Triggers,
            SettingsPage::OneKeys,
            SettingsPage::ScriptApi,
        ] {
            assert!(
                SettingsPage::ALL.contains(&hidden),
                "{hidden:?} 的组件基线不能消失"
            );
        }
    }
}

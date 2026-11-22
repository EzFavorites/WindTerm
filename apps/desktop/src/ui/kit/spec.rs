//! UI 规格表：窗口尺寸、页脚按钮、菜单行的**唯一出处**。
//!
//! 位置不在这里——容器按屏幕中心推算（`Align2::CENTER_CENTER`），所以弹窗在
//! 任何窗口尺寸下都在正中。这里回答「多大 / 什么颜色 / 哪种字体」，不回答
//! 「怎么画」——绘制是 [`super::footer`] / [`super::menu_row`] 等原语的事。
//! 几套视觉刻意不一致的按钮/行作为变体并存，迁移时零像素变化。
//!
//! # 为什么是 enum 而不是 `&[(&str, Vec2)]` 表
//!
//! 字符串表要运行时查找，拼错 id 只有跑到那一步才暴露，而兜底只能是
//! `panic!` 或默认值——前者撞「生产路径 panic 冻结」门禁，后者让尺寸错误
//! 静默生效。enum 让拼错在编译期就红，零运行时成本。
//!
//! # 不在这里的两类
//!
//! - **动态尺寸**（`new_trigger` / `xfer_prompt`）：宽高由调用上下文算出来
//!   （文件名字数、批量条数），走 `DialogWindow::new_sized` 显式传。
//! - **变体标志**（`without_title_bar` / `tight`）：仍是链式调用，不进表。
//!   它们描述的是「画不画标题条」，与几何无关，混进几何表会让一个改动
//!   同时动两件事。

use eframe::egui;

/// 十六进制色号 → `Color32` 的**唯一**入口。各弹窗原来自带 `fn col`，
/// 其中一份直接用 `from_rgb` 绕过了主题换算；现在统一从这里取。
pub(in crate::ui) fn col(hex: u32) -> egui::Color32 {
    crate::theme::to_color32(term_core::theme::rgb(hex))
}

// ── 表单节奏（数据面）──────────────────────────────────────────
//
// 2026-11-22：这四个数曾经散在 `kit::form` / `settings_pages_view` /
// `settings_live_*` 三处，而且把「框底 → 下一标签」与「组框 → 下一组标题」
// 混成同一个 `item_spacing`，于是设置页的垂直间距是视觉基线的两倍。
// 金标准实测（卷一 `03-settings/07-terminal.png` @2x）：
//   LABEL_TO_FIELD 14px / 2 = 7pt    FIELD_TO_ROW 18px / 2 = 9pt
//   TITLE_GAP      7px  / 2 = 3.5pt  BLOCK_GAP 合计 23px / 2 = 11.5pt
pub(in crate::ui) const LABEL_TO_FIELD: f32 = 7.0;
pub(in crate::ui) const FIELD_TO_ROW: f32 = 9.0;
/// 组框之间的**额外**间距（与 `FIELD_TO_ROW` 相加后得到 11.5pt）。
pub(in crate::ui) const BLOCK_GAP: f32 = 2.5;
pub(in crate::ui) const TITLE_GAP: f32 = 3.5;
pub(in crate::ui) const GROUP_PAD: f32 = 10.0;

/// 固定尺寸弹窗的身份。
///
/// 新增弹窗：加一个变体 + 在 [`DialogId::key`] 与 [`DialogId::size`] 各加一行，
/// 并在 [`DialogId::keys`](super::dialog) 的键盘契约表里选一档。
#[derive(Clone, Copy)]
pub(in crate::ui) enum DialogId {
    About,
    NewSession,
    Settings,
    QuickCommands,
    Shortcuts,
    SessionLogging,
    SshConnecting,
    SshHostkey,
    SshPassword,
    SshFailed,
    SshReplaceKey,
    OneKeys,
    Triggers,
    Onboarding,
    /// 首次启动的 Choose Profiles Directory 对话框。
    ProfileChoice,
    Preview,
    ScriptEditor,
    ScriptRun,
    /// XFER 路径提示（原先只有裸字符串 id，现纳入规格表统一管键盘）。
    XferPrompt,
    FilerPrompt,
    FilerSyncScan,
    FilerSyncRun,
    FilerSyncReview,
}

impl DialogId {
    /// 全部固定尺寸弹窗，供穷举门禁（唯一 key / 正尺寸 / 键盘表）共用。
    pub(in crate::ui) const ALL: &'static [DialogId] = &[
        DialogId::About, DialogId::NewSession, DialogId::Settings, DialogId::QuickCommands,
        DialogId::Shortcuts, DialogId::SessionLogging, DialogId::SshConnecting, DialogId::SshHostkey,
        DialogId::SshPassword, DialogId::SshFailed, DialogId::SshReplaceKey, DialogId::OneKeys,
        DialogId::Triggers, DialogId::Onboarding, DialogId::ProfileChoice, DialogId::Preview, DialogId::ScriptEditor,
        DialogId::ScriptRun, DialogId::XferPrompt, DialogId::FilerPrompt, DialogId::FilerSyncScan, DialogId::FilerSyncRun, DialogId::FilerSyncReview,
    ];

    /// egui 的稳定 id。
    ///
    /// **不能用标题代替**：New Session 与 Session Properties 是两个标题、
    /// 同一个窗口，靠标题生成 id 会让二者各存一份布局状态。
    pub(in crate::ui) fn key(self) -> &'static str {
        match self {
            Self::About => "about",
            Self::NewSession => "new_session",
            Self::Settings => "settings",
            Self::QuickCommands => "quick_commands",
            Self::Shortcuts => "shortcuts",
            Self::SessionLogging => "session_settings_logging",
            Self::SshConnecting => "ssh_connecting",
            Self::SshHostkey => "ssh_hostkey",
            Self::SshPassword => "ssh_password",
            Self::SshFailed => "ssh_failed",
            Self::SshReplaceKey => "ssh_replace_key",
            Self::OneKeys => "onekeys",
            Self::Triggers => "triggers",
            Self::Onboarding => "onboarding",
            Self::ProfileChoice => "profile_choice",
            Self::Preview => "preview",
            Self::ScriptEditor => "script_editor",
            Self::ScriptRun => "script_run",
            Self::XferPrompt => "xfer_prompt",
            Self::FilerPrompt => "filer_prompt",
            Self::FilerSyncScan => "filer_sync_scan",
            Self::FilerSyncRun => "filer_sync_run",
            Self::FilerSyncReview => "filer_sync_review",
        }
    }

    /// 窗口尺寸（逻辑点）。数值逐字取自各自弹窗原先的常量，未做任何调整。
    pub(in crate::ui) fn size(self) -> egui::Vec2 {
        match self {
            Self::About => egui::vec2(350.0, 220.0),
            // 会话弹窗：视觉基线两张图（`default-session-ssh-session.png` /
            // `04-new-session/00-ssh-session.png`）都是内容 640×570；加标题条
            // 26 后外框 640×596。
            Self::NewSession => egui::vec2(640.0, 596.0),
            Self::Settings => egui::vec2(602.0, 568.0),
            Self::QuickCommands => egui::vec2(686.0, 404.0),
            Self::Shortcuts => egui::vec2(460.0, 360.0),
            Self::SessionLogging => egui::vec2(640.0, 570.0),
            Self::SshConnecting => egui::vec2(400.0, 110.0),
            Self::SshHostkey => egui::vec2(500.0, 250.0),
            Self::SshPassword => egui::vec2(460.0, 250.0),
            Self::SshFailed => egui::vec2(500.0, 130.0),
            Self::SshReplaceKey => egui::vec2(500.0, 214.0),
            Self::OneKeys => egui::vec2(702.0, 655.0),
            Self::Triggers => egui::vec2(745.0, 510.0),
            Self::Onboarding => egui::vec2(600.0, 380.0),
            Self::ProfileChoice => egui::vec2(720.0, 300.0),
            Self::Preview => egui::vec2(640.0, 540.0),
            Self::ScriptEditor => egui::vec2(920.0, 630.0),
            // 2026-11-18（脚本库收口）：多出来的是两份清单（脚本库 / 最近用过）。
            // 原来的 122 只装得下「一行路径 + 三个按钮 + 状态」——那时候窗口只
            // 会按路径跑，用户得自己记住脚本库目录在哪。清单占 108。
            Self::ScriptRun => egui::vec2(480.0, 240.0),
            Self::XferPrompt => egui::vec2(460.0, 168.0),
            Self::FilerPrompt => egui::vec2(320.0, 96.0),
            // 三个阶段共用一套尺寸：窗口在同一位置接力，用户看不出换窗。
            Self::FilerSyncScan => egui::vec2(440.0, 340.0),
            Self::FilerSyncRun => egui::vec2(440.0, 340.0),
            Self::FilerSyncReview => egui::vec2(440.0, 340.0),
        }
    }
}

// ── 页脚按钮规格（数据面）──────────────────────────────────────
//
// 2026-11-17：项目里并存四套页脚按钮（设置页 / 关闭确认 / 粘贴确认 /
// Tunnels 设置）。它们的**尺寸、圆角、描边宽度、字体口径都不同**，而且
// 差异是有意的（Tunnels 的注释写得很清楚：不与 Triggers 页脚同源）。
// 所以这里不追求「统一成一个样子」，只把每套的实测值钉成数据，绘制交给
// [`super::footer`] 一个原语——外观差异变成选哪个变体，而不是复制一份绘制代码。

/// 页脚按钮的八种身份。`primary` 不再做布尔开关：同一页脚里的两个按钮
/// 经常连字体都不同（确认弹窗是等宽 + 下划线，Tunnels 是按钮字体），
/// 用 bool 会有人偷偷把其中一半画错。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::ui) enum FooterButton {
    SettingsCancel,
    SettingsOk,
    /// Preferences 右下角唯一的 `Close`：80×24、蓝框、2pt 圆角。
    SettingsClose,
    ConfirmNo,
    ConfirmYes,
    PasteCancel,
    PastePaste,
    TunnelsCancel,
    TunnelsOk,
    /// 会话弹窗的主按钮（`Connect` / `Save`）。宽度由文字算，调用方用
    /// [`FooterButtonSpec::with_size`] 覆盖 `size`。
    SessionPrimary,
    /// 会话弹窗的次按钮（`Cancel`）。
    SessionSecondary,
    /// Log Settings 左下角 `Edit Default Settings...`。
    LogDefault,
    LogSave,
    LogCancel,
    QuickOk,
    QuickCancel,
    /// 裸 `ui.button` 的等价规格：默认按钮底/描边/文字，宽度由文字算。
    /// SSH 连接流程那些内联动作按钮走它，少一份手写 `Button` 配色。
    Plain,
    /// 单按钮弹窗（About）的右下角 OK：与 Preferences 的 Close 同尺寸同蓝框。
    DialogOk,
}

/// 对话框的保存/取消契约。**行为**层，不是绘制层：页脚按钮与键盘处理
/// 都从这一处取语义，避免同一个 `X` 在 A 弹窗是「完成」、在 B 弹窗是「放弃草稿」。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::ui) enum DialogLifecycle {
    /// 改动即时生效，只有关闭按钮（Preferences / Shortcuts / About）。
    Immediate,
    /// 草稿：Ok/Save 落盘，Cancel 丢弃（Session / Log / Tunnels / OneKeys）。
    Draft,
    /// 打开时记快照：Ok 提交，Cancel 回滚（Quick Commands）。
    Snapshot,
}

impl DialogLifecycle {
    /// 这个生命周期的次按钮文字。没有次按钮时是 `None`。
    pub(in crate::ui) fn cancel_label(self) -> Option<&'static str> {
        match self {
            Self::Immediate => None,
            Self::Draft | Self::Snapshot => Some("Cancel"),
        }
    }

    /// Esc 等价于次按钮吗？即时生效的弹窗没有可取消的东西。
    pub(in crate::ui) fn esc_cancels(self) -> bool {
        !matches!(self, Self::Immediate)
    }
}

/// 按钮文字的两套口径。含 `f32`，只派生 `PartialEq`（`f32` 无 `Eq`）。
#[derive(Clone, Copy, Debug, PartialEq)]
pub(in crate::ui) enum ButtonGlyph {
    /// egui `TextStyle::Button`（设置页 / Tunnels）。
    Widget,
    /// 等宽，可选首字母下划线（macOS 风格确认弹窗的键盘加速器）。
    Monospace { size: f32, underline_first: bool },
}

/// 一颗页脚按钮的全部可观测外观。颜色是 `u32` 色号，不在这里转 `Color32`：
/// 表是纯数据，转换交给绘图层，夹具和真界面走同一条换算路径。
#[derive(Clone, Copy, Debug, PartialEq)]
pub(in crate::ui) struct FooterButtonSpec {
    pub size: egui::Vec2,
    pub rounding: f32,
    pub fill: u32,
    pub hover_fill: u32,
    pub border: u32,
    pub border_width: f32,
    /// 描边矩形相对按钮矩形的内缩。Tunnels 那圈亮蓝必须落在像素中心
    /// （0.5），直接画在整数边界上会被抗锯齿摊成两行半亮的线。
    pub stroke_inset: f32,
    pub text: u32,
    pub glyph: ButtonGlyph,
    /// 有值时覆盖 `fill`，自上而下线性插值（Tunnels 独有的立体感）。
    pub gradient: Option<(u32, u32)>,
    /// 禁用态是否保留主色描边。会话弹窗的 `Connect` 即使不可点也仍是蓝框，
    /// 这是金标准的外观；其它弹窗的禁用态统一走中性灰。
    pub keep_border_when_disabled: bool,
}

impl FooterButtonSpec {
    /// 覆盖按钮尺寸。会话弹窗的按钮宽度随文字变，但配色/描边仍只有一处定义。
    pub(in crate::ui) fn with_size(mut self, size: egui::Vec2) -> Self {
        self.size = size;
        self
    }
}

impl FooterButton {
    /// 数值逐字取自迁移前各调用点的常量 / 字面量，未做任何「顺手统一」。
    pub(in crate::ui) fn spec(self) -> FooterButtonSpec {
        let widget = ButtonGlyph::Widget;
        let mono = |underline_first| ButtonGlyph::Monospace { size: 12.0, underline_first };
        match self {
            Self::SettingsCancel => FooterButtonSpec {
                size: egui::vec2(72.0, 22.0),
                rounding: 0.0,
                fill: 0x2E2E2E,
                hover_fill: 0x3A3A3A,
                border: 0x4A4A4A,
                border_width: 1.0,
                stroke_inset: 0.0,
                text: 0xE0E0E0,
                glyph: widget,
                gradient: None,
                keep_border_when_disabled: false,
            },
            Self::SettingsOk => FooterButtonSpec {
                size: egui::vec2(70.0, 22.0),
                rounding: 0.0,
                fill: 0x2E2E2E,
                hover_fill: 0x3A3A3A,
                border: 0x3D6E8C,
                border_width: 1.0,
                stroke_inset: 0.0,
                text: 0xE0E0E0,
                glyph: widget,
                gradient: None,
                keep_border_when_disabled: false,
            },
            Self::SettingsClose => FooterButtonSpec {
                size: egui::vec2(80.0, 24.0),
                rounding: 2.0,
                fill: 0x242424,
                hover_fill: 0x2A2A2A,
                border: 0x4A8FE8,
                border_width: 1.5,
                stroke_inset: 0.0,
                text: 0xBBBBBB,
                glyph: widget,
                gradient: None,
                keep_border_when_disabled: false,
            },
            Self::ConfirmNo => FooterButtonSpec {
                size: egui::vec2(80.0, 22.0),
                rounding: 5.0,
                fill: 0x292B2E,
                hover_fill: 0x323437,
                border: 0x4A8FE8,
                border_width: 1.5,
                stroke_inset: 0.0,
                text: 0xF0F0F0,
                glyph: mono(true),
                gradient: None,
                keep_border_when_disabled: false,
            },
            Self::ConfirmYes => FooterButtonSpec {
                size: egui::vec2(80.0, 22.0),
                rounding: 5.0,
                fill: 0x43464A,
                hover_fill: 0x4D5054,
                border: 0x2A2A2A,
                border_width: 1.0,
                stroke_inset: 0.0,
                text: 0xF0F0F0,
                glyph: mono(true),
                gradient: None,
                keep_border_when_disabled: false,
            },
            Self::PasteCancel => FooterButtonSpec {
                size: egui::vec2(76.0, 22.0),
                rounding: 5.0,
                fill: 0x292B2E,
                hover_fill: 0x323437,
                border: 0x4A8FE8,
                border_width: 1.5,
                stroke_inset: 0.0,
                text: 0xF0F0F0,
                glyph: mono(false),
                gradient: None,
                keep_border_when_disabled: false,
            },
            Self::PastePaste => FooterButtonSpec {
                size: egui::vec2(84.0, 22.0),
                rounding: 5.0,
                fill: 0x292B2E,
                hover_fill: 0x323437,
                border: 0x43464A,
                border_width: 1.0,
                stroke_inset: 0.0,
                text: 0xF0F0F0,
                glyph: mono(false),
                gradient: None,
                keep_border_when_disabled: false,
            },
            Self::TunnelsCancel => FooterButtonSpec {
                size: egui::vec2(80.5, 24.0),
                rounding: 0.0,
                fill: 0x000000,
                hover_fill: 0x000000,
                border: 0x3EA0EC,
                border_width: 1.0,
                stroke_inset: 0.5,
                text: 0xBBBBBB,
                glyph: widget,
                gradient: Some((0x4C5259, 0x2C3137)),
                keep_border_when_disabled: false,
            },
            Self::TunnelsOk => FooterButtonSpec {
                size: egui::vec2(80.5, 24.0),
                rounding: 0.0,
                fill: 0x000000,
                hover_fill: 0x000000,
                border: 0x424242,
                border_width: 1.0,
                stroke_inset: 0.5,
                text: 0xBBBBBB,
                glyph: widget,
                gradient: Some((0x292929, 0x232323)),
                keep_border_when_disabled: false,
            },
            Self::SessionPrimary => FooterButtonSpec {
                size: egui::vec2(74.0, 23.0),
                rounding: 2.0,
                fill: 0x333A42,
                hover_fill: 0x3E4650,
                border: 0x3EA0EC,
                border_width: 1.0,
                stroke_inset: 0.0,
                text: 0xE0E0E0,
                glyph: widget,
                gradient: None,
                keep_border_when_disabled: true,
            },
            Self::SessionSecondary => FooterButtonSpec {
                size: egui::vec2(74.0, 23.0),
                rounding: 2.0,
                fill: 0x262626,
                hover_fill: 0x2A2A2A,
                border: 0x3C3C3C,
                border_width: 1.0,
                stroke_inset: 0.0,
                text: 0xE0E0E0,
                glyph: widget,
                gradient: None,
                keep_border_when_disabled: false,
            },
            Self::LogDefault => FooterButtonSpec {
                size: egui::vec2(186.0, 24.0),
                rounding: 4.0,
                fill: 0x262626,
                hover_fill: 0x2A2A2A,
                border: 0x3EA0EC,
                border_width: 1.5,
                stroke_inset: 0.0,
                text: 0xBBBBBB,
                glyph: widget,
                gradient: None,
                keep_border_when_disabled: false,
            },
            Self::LogSave | Self::LogCancel => FooterButtonSpec {
                size: egui::vec2(77.0, 24.0),
                rounding: 0.0,
                fill: 0x262626,
                hover_fill: 0x2A2A2A,
                border: 0x444444,
                border_width: 1.0,
                stroke_inset: 0.0,
                text: 0xBBBBBB,
                glyph: widget,
                gradient: None,
                keep_border_when_disabled: false,
            },
            Self::DialogOk => FooterButtonSpec {
                size: egui::vec2(80.0, 24.0),
                rounding: 2.0,
                fill: 0x242424,
                hover_fill: 0x2A2A2A,
                border: 0x4A8FE8,
                border_width: 1.5,
                stroke_inset: 0.0,
                text: 0xBBBBBB,
                glyph: widget,
                gradient: None,
                keep_border_when_disabled: false,
            },
            Self::QuickOk | Self::QuickCancel | Self::Plain => FooterButtonSpec {
                // 裸 `ui.button` 的宽度随文字算；调用方用 `with_size` 覆盖。
                size: egui::vec2(46.0, 18.0),
                rounding: 0.0,
                fill: 0x262626,
                hover_fill: 0x2A2A2A,
                border: 0x444444,
                border_width: 1.0,
                stroke_inset: 0.0,
                text: 0xBBBBBB,
                glyph: widget,
                gradient: None,
                keep_border_when_disabled: false,
            },
        }
    }
}

// ── 菜单行规格（数据面）────────────────────────────────────────

/// 全仓共用的菜单三色。`ACCENT_BLUE` 来自内核调色板，不在 UI 里重抄。
pub(in crate::ui) const MENU_BG: u32 = 0x333333;
pub(in crate::ui) const MENU_TEXT: u32 = 0xBBBBBB;
pub(in crate::ui) const MENU_TEXT_DISABLED: u32 = 0x555555;
pub(in crate::ui) const MENU_SEPARATOR: u32 = MENU_TEXT_DISABLED;
pub(in crate::ui) const MENU_HIGHLIGHT: u32 = term_core::theme::ACCENT_BLUE;
/// Quick 配置弹层的深底与文字（视觉基线 `quick-popup-clear.png` 实测）。
const QUICK_BG_DARK: u32 = 0x1C1C1C;
const QUICK_BG_LIGHT: u32 = 0x333333;
const QUICK_TEXT: u32 = 0xDDDDDD;

/// 菜单行的五种身份。前一种是 `menu_bar` 树里的行（绘制仍在
/// `shell::menu_style::draw_menu_row`，它有快捷键/勾选栏/箭头四栏，
/// 不是一颗「底色 + 文字」就能表达的）；后四种是 Quick 配置弹层里
/// 原先各写一份的手绘行。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::ui) enum MenuRowKind {
    MenuBar,
    QuickHeader,
    QuickTitle,
    QuickGroup,
    QuickCommand,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(in crate::ui) enum MenuRowFont {
    /// 菜单栏用 egui 的按钮字体，行高由它派生。
    Widget,
    /// Quick 弹层钉死 Proportional 12，不随主题字号漂。
    Proportional(f32),
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(in crate::ui) struct MenuRowSpec {
    pub bg: u32,
    pub hover: u32,
    pub text: u32,
    pub font: MenuRowFont,
}

impl MenuRowKind {
    pub(in crate::ui) fn spec(self) -> MenuRowSpec {
        let quick = |bg, text| MenuRowSpec {
            bg,
            hover: MENU_HIGHLIGHT,
            text,
            font: MenuRowFont::Proportional(12.0),
        };
        match self {
            Self::MenuBar => MenuRowSpec {
                bg: MENU_BG,
                hover: MENU_HIGHLIGHT,
                text: MENU_TEXT,
                font: MenuRowFont::Widget,
            },
            // `Config` 按钮：常态深底、hover 蓝色指向色，文字 #BBBBBB。
            Self::QuickHeader => quick(QUICK_BG_DARK, MENU_TEXT),
            // `Quick` / `Groups` 两行标题：不可点，蓝底永不出现。
            Self::QuickTitle => MenuRowSpec {
                bg: QUICK_BG_DARK,
                hover: QUICK_BG_DARK,
                text: QUICK_TEXT,
                font: MenuRowFont::Proportional(12.0),
            },
            Self::QuickGroup => quick(QUICK_BG_LIGHT, QUICK_TEXT),
            Self::QuickCommand => quick(QUICK_BG_LIGHT, QUICK_TEXT),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 穷举表复用生产侧的清单，新增变体时两边不会漂。
    use super::DialogId as Id;
    const ALL: &[Id] = Id::ALL;

    #[test]
    fn every_key_is_unique() {
        // 两个变体撞 key 会让它们共用一份窗口状态（位置/尺寸互相污染），
        // 而这种错误在界面上只表现为「莫名跳位」，极难定位。
        let mut keys: Vec<&str> = ALL.iter().map(|d| d.key()).collect();
        keys.sort_unstable();
        let before = keys.len();
        keys.dedup();
        assert_eq!(before, keys.len(), "存在重复的窗口 key");
    }

    #[test]
    fn every_size_is_positive_and_fits_a_1280x760_canvas() {
        // 尺寸必须为正，且默认画布（1280×760）装得下——装不下时窗口会溢出
        // 屏幕两侧，用户只会看到中间一条。
        for d in ALL {
            let s = d.size();
            assert!(s.x > 0.0 && s.y > 0.0, "{} 尺寸非正", d.key());
            assert!(
                s.x <= 1280.0 && s.y <= 760.0,
                "{} 尺寸 {} 超出默认画布",
                d.key(),
                s
            );
        }
    }

    /// 四套页脚的几何差异是**实测事实**，不是历史包袱：有人哪天觉得
    /// 「按钮都该 80×22」顺手统一，这里会红。
    #[test]
    fn footer_specs_keep_the_four_measured_button_families() {
        use FooterButton::*;
        assert_eq!(SettingsCancel.spec().size, egui::vec2(72.0, 22.0));
        assert_eq!(SettingsOk.spec().size, egui::vec2(70.0, 22.0));
        assert_eq!(ConfirmNo.spec().size, egui::vec2(80.0, 22.0));
        assert_eq!(PasteCancel.spec().size, egui::vec2(76.0, 22.0));
        assert_eq!(PastePaste.spec().size, egui::vec2(84.0, 22.0));
        let tunnels = TunnelsCancel.spec();
        assert_eq!(tunnels.size, egui::vec2(80.5, 24.0));
        assert_eq!(tunnels.border, 0x3EA0EC);
        assert_eq!(tunnels.stroke_inset, 0.5);
        assert!(tunnels.gradient.is_some());
        assert_eq!(TunnelsOk.spec().border, 0x424242);
        assert_ne!(ConfirmNo.spec().glyph, TunnelsCancel.spec().glyph);
    }

    #[test]
    fn rhythm_constants_match_the_golden_settings_page() {
        // 卷一 `03-settings/07-terminal.png` @2x 实测：标签→框 14px、
        // 框→下一行 18px、标题→框 7px、组框之间 23px。
        assert_eq!((LABEL_TO_FIELD, FIELD_TO_ROW), (7.0, 9.0));
        assert_eq!((TITLE_GAP, GROUP_PAD), (3.5, 10.0));
        assert_eq!(FIELD_TO_ROW + BLOCK_GAP, 11.5);
        assert!(FIELD_TO_ROW > LABEL_TO_FIELD);
    }

    #[test]
    fn the_new_footer_families_keep_their_measured_geometry() {
        use FooterButton::*;
        assert_eq!(SettingsClose.spec().size, egui::vec2(80.0, 24.0));
        assert_eq!(SettingsClose.spec().border, 0x4A8FE8);
        assert_eq!(SessionPrimary.spec().size.y, 23.0);
        assert!(SessionPrimary.spec().keep_border_when_disabled);
        assert!(!SessionSecondary.spec().keep_border_when_disabled);
        assert_eq!(LogDefault.spec().size, egui::vec2(186.0, 24.0));
        assert_eq!(LogSave.spec().size, egui::vec2(77.0, 24.0));
        // 尺寸可覆盖（文字宽度随语言变），配色不变。
        let wide = SessionPrimary.spec().with_size(egui::vec2(120.0, 23.0));
        assert_eq!(wide.size, egui::vec2(120.0, 23.0));
        assert_eq!(wide.border, 0x3EA0EC);
        // 八套原本的规格不能被新变体顺手改掉。
        assert_eq!(FooterButton::ConfirmYes.spec().size, egui::vec2(80.0, 22.0));
    }

    #[test]
    fn lifecycle_encodes_the_cancel_contract() {
        assert_eq!(DialogLifecycle::Immediate.cancel_label(), None);
        assert!(!DialogLifecycle::Immediate.esc_cancels());
        assert_eq!(DialogLifecycle::Draft.cancel_label(), Some("Cancel"));
        assert!(DialogLifecycle::Snapshot.esc_cancels());
    }

    /// 蓝色 = 指向色（用户反复强调）：只有可点的行才允许 hover 变蓝，
    /// 标题行写成常量蓝就变成假的固有底色。
    #[test]
    fn only_interactive_menu_rows_can_point_blue() {
        assert_eq!(MenuRowKind::QuickTitle.spec().hover, QUICK_BG_DARK);
        assert_eq!(MenuRowKind::QuickHeader.spec().hover, MENU_HIGHLIGHT);
        assert_eq!(MenuRowKind::QuickGroup.spec().hover, MENU_HIGHLIGHT);
        assert_eq!(MenuRowKind::QuickCommand.spec().hover, MENU_HIGHLIGHT);
        assert_eq!(MENU_HIGHLIGHT, 0x007CD2);
        assert_ne!(QUICK_TEXT, MENU_TEXT, "Quick 弹层文字比菜单栏亮一档");
    }
}

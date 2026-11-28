//! 触发器编辑器的投影数据类型（从 `model.rs` 拆出，4j：单文件 500 代码行上限）。
//! 全是纯数据与其小方法；`model.rs` 原样再导出，外部路径与行为不变。

use super::TriggerKind;
use term_core::automation::RuleId;

/// 六条求值信号通道（对应 `When` 的六个分支）。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(in crate::ui) enum Channel {
    #[default]
    Output,
    Connect,
    Disconnect,
    Idle,
    BeforeConnect,
    BeforeDisconnect,
}

impl Channel {
    pub(in crate::ui) fn try_from_u8(v: u8) -> Result<Self, ()> {
        match v {
            0 => Ok(Self::Output),
            1 => Ok(Self::Connect),
            2 => Ok(Self::Disconnect),
            3 => Ok(Self::Idle),
            4 => Ok(Self::BeforeConnect),
            5 => Ok(Self::BeforeDisconnect),
            _ => Err(()),
        }
    }

    pub(in crate::ui) fn to_u8(self) -> u8 {
        match self {
            Self::Output => 0,
            Self::Connect => 1,
            Self::Disconnect => 2,
            Self::Idle => 3,
            Self::BeforeConnect => 4,
            Self::BeforeDisconnect => 5,
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(in crate::ui) enum ColorChoice {
    #[default]
    Empty,
    Ansi(String),
    Custom(String),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub(in crate::ui) enum LineStyle {
    #[default]
    None,
    Solid,
    Double,
    Thick,
    Strike,
    Wavy,
    Dot,
    Dash,
    DashDot,
    DashDotDot,
    Box,
    RoundBox,
    DashedBox,
    DottedBox,
}

impl LineStyle {
    pub(in crate::ui) fn label(self) -> &'static str {
        match self {
            Self::None => "None",
            Self::Solid => "Solid",
            Self::Double => "Double",
            Self::Thick => "Thick",
            Self::Strike => "Strike",
            Self::Wavy => "Wavy",
            Self::Dot => "Dot",
            Self::Dash => "Dash",
            Self::DashDot => "Dash dot",
            Self::DashDotDot => "Dash dot dot",
            Self::Box => "Box",
            Self::RoundBox => "Round box",
            Self::DashedBox => "Dashed box",
            Self::DottedBox => "Dotted box",
        }
    }

    pub(in crate::ui) const ALL: [Self; 14] = [
        Self::None,
        Self::Solid,
        Self::Double,
        Self::Thick,
        Self::Strike,
        Self::Wavy,
        Self::Dot,
        Self::Dash,
        Self::DashDot,
        Self::DashDotDot,
        Self::Box,
        Self::RoundBox,
        Self::DashedBox,
        Self::DottedBox,
    ];
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub(in crate::ui) enum ActionKind {
    #[default]
    RunCommand,
    PlaySound,
    OpenHyperlink,
    SendText,
}

impl ActionKind {
    pub(in crate::ui) fn label(self) -> &'static str {
        match self {
            Self::RunCommand => "Run command",
            Self::PlaySound => "Play sound",
            Self::OpenHyperlink => "Open hyperlink",
            Self::SendText => "Send text",
        }
    }

    pub(in crate::ui) const ALL: [Self; 4] = [
        Self::RunCommand,
        Self::PlaySound,
        Self::OpenHyperlink,
        Self::SendText,
    ];
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(in crate::ui) struct OsCommand {
    pub command: String,
    pub arguments: String,
    pub working_directory: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub(in crate::ui) enum SendMode {
    #[default]
    Text,
    Script,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub(in crate::ui) enum SendCondition {
    #[default]
    AfterMatchAndPrompt,
    AfterMatchAndLineBreak,
    AfterMatch,
}

impl SendCondition {
    pub(in crate::ui) fn label(self) -> &'static str {
        match self {
            Self::AfterMatchAndPrompt => "Send after text match and next command prompt",
            Self::AfterMatchAndLineBreak => "Send after text match and next line break",
            Self::AfterMatch => "Send after text match",
        }
    }

    pub(in crate::ui) const ALL: [Self; 3] = [
        Self::AfterMatchAndPrompt,
        Self::AfterMatchAndLineBreak,
        Self::AfterMatch,
    ];
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(in crate::ui) struct MatchFlags {
    pub dot_all: bool,
    pub whole_word: bool,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(in crate::ui) struct MarkConfig {
    pub mark_style: LineStyle,
    pub mark_color: ColorChoice,
    pub text_color: ColorChoice,
    pub background_color: ColorChoice,
    pub line_style: LineStyle,
    pub line_color: ColorChoice,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(in crate::ui) struct SendConfig {
    pub text: String,
    pub mode: SendMode,
    pub capture: String,
    pub condition: SendCondition,
    pub later_secs: String,
}

fn default_later() -> String {
    "0".to_owned()
}

impl Default for SendConfig {
    fn default() -> Self {
        Self {
            text: String::new(),
            mode: SendMode::default(),
            capture: "${match}".to_owned(),
            condition: SendCondition::default(),
            later_secs: default_later(),
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub(in crate::ui) struct ActionConfig {
    pub kind: ActionKind,
    pub os: [OsCommand; 3],
    pub hyperlink: String,
    pub mode: SendMode,
    pub sound: String,
    pub sound_secs: f32,
    pub hide_console: bool,
}

fn default_os() -> [OsCommand; 3] {
    std::array::from_fn(|_| OsCommand::default())
}

impl Default for ActionConfig {
    fn default() -> Self {
        Self {
            kind: ActionKind::default(),
            os: default_os(),
            hyperlink: String::new(),
            mode: SendMode::default(),
            sound: String::new(),
            sound_secs: 0.0,
            hide_console: false,
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(in crate::ui) struct MenuItem {
    pub label: String,
    pub action: ActionKind,
    pub separator: bool,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(in crate::ui) struct MenuConfig {
    pub items: Vec<MenuItem>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(in crate::ui) struct TooltipConfig {
    pub text: String,
    pub mode: SendMode,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(in crate::ui) struct TargetConfig {
    pub systems: Vec<String>,
    pub sessions: Vec<String>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub(in crate::ui) struct RuleConfig {
    pub flags: MatchFlags,
    pub mark: MarkConfig,
    pub action: ActionConfig,
    pub send: SendConfig,
    pub targets: TargetConfig,
    pub menu: MenuConfig,
    pub tooltip: TooltipConfig,
}

/// 编辑器里的规则投影。字段与 11 种类型的面板一一对应；落盘前展开成正交规则。
#[derive(Clone, Debug, Default, PartialEq)]
pub(in crate::ui) struct ViewRule {
    pub id: RuleId,
    pub name: String,
    pub description: String,
    pub kind: TriggerKind,
    pub config: RuleConfig,
    pub on: Channel,
    pub pattern: String,
    pub literal: bool,
    pub case_sensitive: bool,
    pub cooldown_secs: u64,
    pub idle_secs: u64,
    pub actions: Vec<UiAction>,
    pub enabled: bool,
}

/// 编辑器里沿用的「规则」名字，指向投影类型；真正的落盘 / 求值类型是
/// `term_core::automation::Rule`。
pub(in crate::ui) type Rule = ViewRule;

/// 一次命中的意图（编辑器侧）。内核产物是 `Effect`，这里只做转换辅助。
#[derive(Clone, Debug, PartialEq, Eq)]
pub(in crate::ui) struct ViewHit {
    pub rule_id: RuleId,
    pub actions: Vec<UiAction>,
    pub matched: String,
}

/// 命中后做什么。派发由宿主按 [`Action`] 的四类落点完成。
#[derive(Clone, Debug, PartialEq, Eq)]
pub(in crate::ui) enum UiAction {
    SendText(String),
    SendLine(String),
    Highlight,
    Beep,
    RunCommand(String),
    LocalSendLine(String),
    OpenLink(String),
}

/// `Script` 模式的最小求值：只认 `return "..."` 字面量，其它原样当文本发。
pub(in crate::ui) fn eval_script_template(script: &str) -> String {
    let trimmed = script.trim();
    let Some(rest) = trimmed.strip_prefix("return") else {
        return script.to_string();
    };
    let rest = rest.trim();
    if rest.len() < 2 {
        return script.to_string();
    }
    let quote = rest.as_bytes()[0];
    if (quote == b'"' || quote == b'\'') && rest.as_bytes()[rest.len() - 1] == quote {
        return rest[1..rest.len() - 1].to_string();
    }
    script.to_string()
}


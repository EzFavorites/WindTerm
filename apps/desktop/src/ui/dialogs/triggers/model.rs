//! 触发器编辑器的**投影模型**与 `automation::Rule` 的双向转换。
//!
//! 落盘与求值的唯一真相是 `term_core::automation::Rule`（ADR-0030）；这一页
//! 为了让 11 种类型各自的字段仍然可读可编辑，保留一份 UI 投影。草稿在投影上
//! 改，`Ok` 时经 [`ViewRule::to_core`] 展开成正交的 `when / filter / guard /
//! actions` 写回 `Prefs.triggers`；重新打开时经 [`ViewRule::from_core`] 还原。
//! 没有历史兼容：旧规则类型已经删除。

use std::time::Duration;

// 投影只在内存里用：落盘 / 传输的形状是 `term_core::automation::Rule`，
// 这里不派生 serde，也不给桌面端引入 serde 依赖（`json!` 只生成参数值）。
use serde_json::json;
use term_core::automation::{
    Action, AlertKind, AppAction, Filter, Guard, Invocation, LinkSpec, MarkSpec, MenuItemSpec,
    MenuSpec, OnceScope, Pattern, Rule as CoreRule, RuleId, Syntax, TextSpec, When,
};

/// 类型下拉的 11 项。只是**预设工厂的入口**，不是规则字段。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub(in crate::ui) enum TriggerKind {
    CustomLink,
    CustomMenu,
    AfterConnect,
    AfterDisconnect,
    BeforeConnect,
    BeforeDisconnect,
    #[default]
    HighlightText,
    PlaySound,
    RunCommand,
    SendText,
    ShowTooltip,
}

impl TriggerKind {
    pub(in crate::ui) fn trigger_on(self) -> Channel {
        self.channel()
    }

    pub(in crate::ui) fn channel(self) -> Channel {
        match self {
            Self::AfterConnect => Channel::Connect,
            Self::AfterDisconnect => Channel::Disconnect,
            Self::BeforeConnect => Channel::BeforeConnect,
            Self::BeforeDisconnect => Channel::BeforeDisconnect,
            _ => Channel::Output,
        }
    }

    pub(in crate::ui) fn label(self) -> &'static str {
        match self {
            Self::CustomLink => "Custom link",
            Self::CustomMenu => "Custom menu",
            Self::AfterConnect => "Event: after connection",
            Self::AfterDisconnect => "Event: after disconnection",
            Self::BeforeConnect => "Event: before connection",
            Self::BeforeDisconnect => "Event: before disconnection",
            Self::HighlightText => "Highlight text",
            Self::PlaySound => "Play sound",
            Self::RunCommand => "Run command",
            Self::SendText => "Send text",
            Self::ShowTooltip => "Show tooltip",
        }
    }

    pub(in crate::ui) const ALL: [Self; 11] = [
        Self::CustomLink,
        Self::CustomMenu,
        Self::AfterConnect,
        Self::AfterDisconnect,
        Self::BeforeConnect,
        Self::BeforeDisconnect,
        Self::HighlightText,
        Self::PlaySound,
        Self::RunCommand,
        Self::SendText,
        Self::ShowTooltip,
    ];
}

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

fn color_text(c: &ColorChoice) -> String {
    match c {
        ColorChoice::Empty => String::new(),
        ColorChoice::Ansi(s) | ColorChoice::Custom(s) => s.clone(),
    }
}

fn line_text(s: LineStyle) -> String {
    if s == LineStyle::None {
        String::new()
    } else {
        s.label().to_owned()
    }
}

impl UiAction {
    /// 展开成内核动作。`session = "current"` 由宿主替换为命中所在会话；
    /// `"local"` 走底带本地 Shell 面板，二者都不写死在具体标签上。
    pub(in crate::ui) fn to_core(&self) -> Option<Action> {
        match self {
            Self::SendText(s) => Some(Action::SessionInvocation {
                method: "write".into(),
                params: json!({ "session": "current", "text": s }),
            }),
            Self::SendLine(s) => {
                let mut text = s.clone();
                text.push('\r');
                Some(Action::SessionInvocation {
                    method: "write".into(),
                    params: json!({ "session": "current", "text": text }),
                })
            }
            Self::LocalSendLine(s) => {
                let mut text = s.clone();
                text.push('\r');
                Some(Action::SessionInvocation {
                    method: "write".into(),
                    params: json!({ "session": "local", "text": text }),
                })
            }
            Self::Beep => Some(Action::AppAction(AppAction::Alert(AlertKind::Beep))),
            Self::RunCommand(id) => Some(Action::AppAction(AppAction::Dispatch(id.clone()))),
            Self::OpenLink(url) => Some(Action::AppAction(AppAction::Link(LinkSpec {
                url: url.clone(),
                text: String::new(),
            }))),
            Self::Highlight => Some(Action::AppAction(AppAction::Mark(MarkSpec::default()))),
        }
    }

    pub(in crate::ui) fn from_core(action: &Action) -> Option<Self> {
        match action {
            Action::SessionInvocation { params, .. } => {
                let text = params.get("text").and_then(|v| v.as_str()).unwrap_or("");
                let local = params.get("session").and_then(|v| v.as_str()) == Some("local");
                let bare = text.strip_suffix('\r').unwrap_or(text).to_owned();
                Some(if local {
                    Self::LocalSendLine(bare)
                } else {
                    Self::SendText(bare)
                })
            }
            Action::AppAction(AppAction::Alert(AlertKind::Beep)) => Some(Self::Beep),
            Action::AppAction(AppAction::Dispatch(id)) => Some(Self::RunCommand(id.clone())),
            Action::AppAction(AppAction::Link(spec)) => Some(Self::OpenLink(spec.url.clone())),
            Action::AppAction(AppAction::Mark(_) | AppAction::Tooltip(_) | AppAction::Menu(_)) => {
                Some(Self::Highlight)
            }
            _ => None,
        }
    }
}

impl ViewRule {
    /// 11 种类型 → 四类正交意图。`Mark / Link / Menu / Tooltip` 都在这里带上
    /// 各自的编辑字段，派发端只认展开后的 `Action`。
    pub(in crate::ui) fn to_core(&self) -> CoreRule {
        let syntax = if self.literal { Syntax::Literal } else { Syntax::Regex };
        let pattern = Pattern::compile(&self.pattern, syntax, self.case_sensitive)
            .unwrap_or_else(|_| Pattern::compile("", Syntax::Literal, true).expect("空字面模式可编译"));
        let when = match self.on {
            Channel::Output => When::Output(pattern),
            Channel::Connect => When::Connected,
            Channel::Disconnect => When::Disconnected,
            Channel::Idle => When::Idle {
                after: Duration::from_secs(self.idle_secs),
            },
            Channel::BeforeConnect => When::BeforeConnect,
            Channel::BeforeDisconnect => When::BeforeDisconnect,
        };
        let mut actions: Vec<Action> = self.actions.iter().filter_map(UiAction::to_core).collect();
        // 类型专属的附加动作：编辑字段只在投影里，展开时落到对应的 AppAction。
        match self.kind {
            TriggerKind::ShowTooltip => actions.push(Action::AppAction(AppAction::Tooltip(TextSpec {
                text: self.config.tooltip.text.clone(),
                is_script: self.config.tooltip.mode == SendMode::Script,
            }))),
            TriggerKind::CustomMenu => actions.push(Action::AppAction(AppAction::Menu(MenuSpec {
                items: self
                    .config
                    .menu
                    .items
                    .iter()
                    .map(|item| MenuItemSpec {
                        label: item.label.clone(),
                        separator: item.separator,
                        action: if item.separator {
                            None
                        } else {
                            Some(match item.action {
                                ActionKind::RunCommand => Invocation::new(
                                    "app_command",
                                    json!({ "command": "Terminal.ClearScreen" }),
                                ),
                                ActionKind::SendText => Invocation::new(
                                    "write",
                                    json!({ "session": "current", "text": "" }),
                                ),
                                ActionKind::PlaySound => Invocation::new("notify", json!({ "message": "" })),
                                ActionKind::OpenHyperlink => Invocation::new("notify", json!({ "message": "" })),
                            })
                        },
                    })
                    .collect(),
            }))),
            _ => {}
        }
        for action in &mut actions {
            if let Action::AppAction(AppAction::Mark(spec)) = action {
                *spec = MarkSpec {
                    text_color: color_text(&self.config.mark.text_color),
                    background: color_text(&self.config.mark.background_color),
                    line_style: line_text(self.config.mark.line_style),
                    line_color: color_text(&self.config.mark.line_color),
                };
            }
        }
        CoreRule {
            id: self.id.clone(),
            name: self.name.clone(),
            enabled: self.enabled,
            when,
            filter: Filter {
                systems: self.config.targets.systems.clone(),
                sessions: self.config.targets.sessions.clone(),
            },
            guard: Guard {
                cooldown: (self.cooldown_secs > 0).then(|| Duration::from_secs(self.cooldown_secs)),
                once: OnceScope::Never,
            },
            actions,
        }
    }

    /// 内核规则 → 编辑器投影。`kind` 是按 `when` + 主动作推断的预设，
    /// 不是规则里的字段——正交规则改过之后不强行归类。
    pub(in crate::ui) fn from_core(rule: &CoreRule) -> Self {
        let (on, pattern, literal, case_sensitive, idle_secs) = match &rule.when {
            When::Output(p) => (
                Channel::Output,
                p.source().to_owned(),
                p.syntax() == Syntax::Literal,
                p.case_sensitive(),
                0,
            ),
            When::Connected => (Channel::Connect, String::new(), true, true, 0),
            When::Disconnected => (Channel::Disconnect, String::new(), true, true, 0),
            When::BeforeConnect => (Channel::BeforeConnect, String::new(), true, true, 0),
            When::BeforeDisconnect => (Channel::BeforeDisconnect, String::new(), true, true, 0),
            When::Idle { after } => (
                Channel::Idle,
                String::new(),
                true,
                true,
                after.as_secs(),
            ),
        };
        let mut actions = Vec::new();
        let mut kind = TriggerKind::HighlightText;
        for action in &rule.actions {
            if let Some(ui) = UiAction::from_core(action) {
                actions.push(ui);
            }
            match action {
                Action::AppAction(AppAction::Mark(_)) => kind = TriggerKind::HighlightText,
                Action::AppAction(AppAction::Link(_)) => kind = TriggerKind::CustomLink,
                Action::AppAction(AppAction::Menu(_)) => kind = TriggerKind::CustomMenu,
                Action::AppAction(AppAction::Tooltip(_)) => kind = TriggerKind::ShowTooltip,
                Action::AppAction(AppAction::Dispatch(_)) => kind = TriggerKind::RunCommand,
                Action::AppAction(AppAction::Alert(_)) => kind = TriggerKind::PlaySound,
                Action::SessionInvocation { .. } => kind = TriggerKind::SendText,
                Action::CredentialRef { .. } | Action::RunScript { .. } => {}
            }
        }
        // 事件类规则没有文本动作，按通道补回类型。
        if actions.is_empty() {
            kind = match on {
                Channel::Connect => TriggerKind::AfterConnect,
                Channel::Disconnect => TriggerKind::AfterDisconnect,
                Channel::BeforeConnect => TriggerKind::BeforeConnect,
                Channel::BeforeDisconnect => TriggerKind::BeforeDisconnect,
                Channel::Idle => TriggerKind::HighlightText,
                Channel::Output => TriggerKind::HighlightText,
            };
        }
        Self {
            id: rule.id.clone(),
            name: rule.name.clone(),
            description: String::new(),
            kind,
            config: RuleConfig {
                targets: TargetConfig {
                    systems: rule.filter.systems.clone(),
                    sessions: rule.filter.sessions.clone(),
                },
                ..RuleConfig::default()
            },
            on,
            pattern,
            literal,
            case_sensitive,
            cooldown_secs: rule
                .guard
                .cooldown
                .map(|d| d.as_secs())
                .unwrap_or(0),
            idle_secs,
            actions,
            enabled: rule.enabled,
        }
    }
}

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
#[path = "model_types.rs"]
mod model_types;
pub(in crate::ui) use model_types::*;

use term_core::automation::{
    Action, AlertKind, AppAction, Filter, Guard, Invocation, LinkSpec, MarkSpec, MenuItemSpec,
    MenuSpec, OnceScope, Pattern, Release, Rule as CoreRule, Syntax, TextSpec, When,
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

/// 编辑器里的「发送条件 + Send Later」→ 内核的正交 `Release`。
///
/// `Send Later` 填了正数就优先于条件；空串 / 非数字 / 负数都当不延迟，
/// 退回条件本身（缺省 Prompt 因此仍然扣得住，不会因空串变成立即发）。
fn release_of(cfg: &SendConfig) -> Release {
    let later = cfg
        .later_secs
        .trim()
        .parse::<f64>()
        .ok()
        .filter(|v| v.is_finite() && *v > 0.0);
    if let Some(secs) = later {
        return Release::Later(Duration::from_secs_f64(secs));
    }
    match cfg.condition {
        SendCondition::AfterMatch => Release::Immediate,
        SendCondition::AfterMatchAndPrompt => Release::Prompt,
        SendCondition::AfterMatchAndLineBreak => Release::LineBreak,
    }
}

impl ViewRule {
    /// 11 种类型 → 四类正交意图。`Mark / Link / Menu / Tooltip` 都在这里带上
    /// 各自的编辑字段，派发端只认展开后的 `Action`。
    pub(in crate::ui) fn to_core(&self) -> Result<CoreRule, String> {
        let syntax = if self.literal { Syntax::Literal } else { Syntax::Regex };
        // 无效正则不能悄悄退化成空模式（那会匹配每一行）；把错误交回调用方，
        // 由编辑页 / 事件泵显式拒绝这条规则并给出提示。
        let pattern = Pattern::compile(&self.pattern, syntax, self.case_sensitive)
            .map_err(|err| format!("规则 {} 的模式无效：{err}", self.name))?;
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
                                    json!({ "command": term_app::menu::context_ids::CLEAR_SCREEN }),
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
        Ok(CoreRule {
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
            release: release_of(&self.config.send),
            actions,
        })
    }

    /// 内核规则 → 编辑器投影。`kind` 是按 `when` + 主动作推断的预设，
    /// 不是规则里的字段——正交规则改过之后不强行归类。
    pub(in crate::ui) fn from_core(rule: &CoreRule) -> Self {
        let (condition, later_secs) = match rule.release {
            Release::Immediate => (SendCondition::AfterMatch, String::new()),
            Release::Prompt => (SendCondition::AfterMatchAndPrompt, String::new()),
            Release::LineBreak => (SendCondition::AfterMatchAndLineBreak, String::new()),
            Release::Later(delay) => (SendCondition::AfterMatch, delay.as_secs_f64().to_string()),
        };
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
                send: SendConfig {
                    condition,
                    later_secs,
                    ..SendConfig::default()
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

//! 触发器规则的**数据层**（roadmap）。
//!
//! 放在 `term-core` 而不是 `term-app`：规则要落盘进 `Prefs`（`term-profile` 拥有），
//! 而 `term-profile` 是低层 crate，按 ADR-0014 不得依赖 `term-app`（应用层）。
//! 求值内核（`Matcher` / `validate` / 简化正则）留在 `term-app::trigger`，只消费
//! 这里的类型——于是 `term-profile → term-core`、`term-app → term-core`，分层不被破坏。

use serde::{Deserialize, Serialize};

/// 触发源：什么事件会让规则进入匹配。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum TriggerOn {
    /// 终端输出里出现匹配文本。
    Output,
    /// 会话建立成功（一次性）。
    Connect,
    /// 会话断开（一次性）。
    Disconnect,
    /// 连续 `idle_secs` 秒无输出（每段空闲一次）。
    Idle,
    /// 会话建立之前（一次性）。对应类型 `Event: before connection`。
    BeforeConnect,
    /// 会话断开之前（一次性）。对应类型 `Event: before disconnection`。
    BeforeDisconnect,
}
impl TriggerOn {
    /// 从 u8 反序列化；用于外部存储（UI 暂存、偏好持久化等）的兼容性解析。
    #[allow(clippy::result_unit_err)]
    pub fn try_from_u8(v: u8) -> Result<Self, ()> {
        match v {
            0 => Ok(TriggerOn::Output),
            1 => Ok(TriggerOn::Connect),
            2 => Ok(TriggerOn::Disconnect),
            3 => Ok(TriggerOn::Idle),
            4 => Ok(TriggerOn::BeforeConnect),
            5 => Ok(TriggerOn::BeforeDisconnect),
            _ => Err(()),
        }
    }

    /// 序列化为 u8，与 [Self::try_from_u8] 互为逆运算。
    pub fn to_u8(&self) -> u8 {
        match self {
            TriggerOn::Output => 0,
            TriggerOn::Connect => 1,
            TriggerOn::Disconnect => 2,
            TriggerOn::Idle => 3,
            TriggerOn::BeforeConnect => 4,
            TriggerOn::BeforeDisconnect => 5,
        }
    }
}

/// 触发器的**类型**（New Trigger 弹窗 `Type` 下拉的 11 项，视觉基线
/// `08-trigger-type-list.png` / `09-new-trigger-dialog.png`）。
///
/// 与 [`TriggerOn`] 的分工：`TriggerOn` 是求值内核关心的「事件通道」，只有 6 种；
/// `TriggerKind` 是用户看到的「类型」，11 种。多个类型可落在同一个事件通道
/// （例如 `Custom link` / `Highlight text` / `Send text` 都是 `Output`），
/// 差别在编辑面板显示哪些字段、命中后做什么——那是 [`RuleConfig`] 的职责。
///
/// `serde` 默认按变体名存字符串；`Default = HighlightText` 让旧 `prefs.json`
/// 里没有 `kind` 键的规则读到后仍落在原来的「Highlight Text」页。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum TriggerKind {
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
    /// 该类型落在哪个求值事件通道（New Trigger 弹窗选完类型后 `on` 的取值）。
    pub fn trigger_on(self) -> TriggerOn {
        match self {
            TriggerKind::AfterConnect => TriggerOn::Connect,
            TriggerKind::AfterDisconnect => TriggerOn::Disconnect,
            TriggerKind::BeforeConnect => TriggerOn::BeforeConnect,
            TriggerKind::BeforeDisconnect => TriggerOn::BeforeDisconnect,
            // 其余都是「输出匹配」类：靠 Match 文本触发。
            _ => TriggerOn::Output,
        }
    }

    /// 视觉基线里的完整显示名（ underlined 助记符由 UI 层按类型自定）。
    pub fn label(self) -> &'static str {
        match self {
            TriggerKind::CustomLink => "Custom link",
            TriggerKind::CustomMenu => "Custom menu",
            TriggerKind::AfterConnect => "Event: after connection",
            TriggerKind::AfterDisconnect => "Event: after disconnection",
            TriggerKind::BeforeConnect => "Event: before connection",
            TriggerKind::BeforeDisconnect => "Event: before disconnection",
            TriggerKind::HighlightText => "Highlight text",
            TriggerKind::PlaySound => "Play sound",
            TriggerKind::RunCommand => "Run command",
            TriggerKind::SendText => "Send text",
            TriggerKind::ShowTooltip => "Show tooltip",
        }
    }

    /// 固定顺序，供 UI 的 `Type` 下拉直接遍历（与视觉基线同序）。
    pub const ALL: [TriggerKind; 11] = [
        TriggerKind::CustomLink,
        TriggerKind::CustomMenu,
        TriggerKind::AfterConnect,
        TriggerKind::AfterDisconnect,
        TriggerKind::BeforeConnect,
        TriggerKind::BeforeDisconnect,
        TriggerKind::HighlightText,
        TriggerKind::PlaySound,
        TriggerKind::RunCommand,
        TriggerKind::SendText,
        TriggerKind::ShowTooltip,
    ];
}

/// 颜色选择：既可能是预设 ANSI 色，也可能是「Empty」或自定义 HEX。
///
/// 视觉基线 `14-color-dropdown.png`（Color 下拉：首项 `Color…` 调出取色器，
/// 中间 16 个 ANSI 色，末项 `Empty`）与 `06-color-picker.png`（取色器）。
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum ColorChoice {
    /// `Empty`：不设置该颜色。
    #[default]
    Empty,
    /// ANSI 预设色，存视觉基线里的完整名字（如 `Ansi Bright Red`）。
    Ansi(String),
    /// 取色器给出的自定义色，`#rrggbb`。
    Custom(String),
}
/// 线型（Highlight 弹窗 `Line Style` 下拉 14 项，视觉基线 `12-line-style-dropdown.png`）。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum LineStyle {
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
    pub fn label(self) -> &'static str {
        match self {
            LineStyle::None => "None",
            LineStyle::Solid => "Solid",
            LineStyle::Double => "Double",
            LineStyle::Thick => "Thick",
            LineStyle::Strike => "Strike",
            LineStyle::Wavy => "Wavy",
            LineStyle::Dot => "Dot",
            LineStyle::Dash => "Dash",
            LineStyle::DashDot => "Dash dot",
            LineStyle::DashDotDot => "Dash dot dot",
            LineStyle::Box => "Box",
            LineStyle::RoundBox => "Round box",
            LineStyle::DashedBox => "Dashed box",
            LineStyle::DottedBox => "Dotted box",
        }
    }

    /// 固定顺序，与视觉基线 `12-line-style-dropdown.png` 同序。
    pub const ALL: [LineStyle; 14] = [
        LineStyle::None,
        LineStyle::Solid,
        LineStyle::Double,
        LineStyle::Thick,
        LineStyle::Strike,
        LineStyle::Wavy,
        LineStyle::Dot,
        LineStyle::Dash,
        LineStyle::DashDot,
        LineStyle::DashDotDot,
        LineStyle::Box,
        LineStyle::RoundBox,
        LineStyle::DashedBox,
        LineStyle::DottedBox,
    ];
}

/// `Action` 下拉（视觉基线 `05-action-dropdown.png`）。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum ActionKind {
    #[default]
    RunCommand,
    PlaySound,
    OpenHyperlink,
    SendText,
}
impl ActionKind {
    pub fn label(self) -> &'static str {
        match self {
            ActionKind::RunCommand => "Run command",
            ActionKind::PlaySound => "Play sound",
            ActionKind::OpenHyperlink => "Open hyperlink",
            ActionKind::SendText => "Send text",
        }
    }
    pub const ALL: [ActionKind; 4] = [
        ActionKind::RunCommand,
        ActionKind::PlaySound,
        ActionKind::OpenHyperlink,
        ActionKind::SendText,
    ];
}

/// 一个 OS 下的命令行配置（After connection / Custom Link 页的三个字段）。
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct OsCommand {
    #[serde(default)]
    pub command: String,
    #[serde(default)]
    pub arguments: String,
    #[serde(default)]
    pub working_directory: String,
}

/// `Send` 编辑区的模式（视觉基线 `15-send-text-page.png` 底部 `Text` / `Script` tab）。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum SendMode {
    #[default]
    Text,
    Script,
}

/// `Send Condition` 下拉的 3 项（视觉基线 `16-send-condition-dropdown.png`）。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum SendCondition {
    #[default]
    AfterMatchAndPrompt,
    AfterMatchAndLineBreak,
    AfterMatch,
}
impl SendCondition {
    pub fn label(self) -> &'static str {
        match self {
            SendCondition::AfterMatchAndPrompt => {
                "Send after text match and next command prompt"
            }
            SendCondition::AfterMatchAndLineBreak => {
                "Send after text match and next line break"
            }
            SendCondition::AfterMatch => "Send after text match",
        }
    }
    pub const ALL: [SendCondition; 3] = [
        SendCondition::AfterMatchAndPrompt,
        SendCondition::AfterMatchAndLineBreak,
        SendCondition::AfterMatch,
    ];
}

/// Match 框下方的 flag bar（视觉基线 `.* Aa W`）：
/// `dot_all` 对应 `.`（点匹配换行），`whole_word` 对应 `W`（全词匹配）。
///
/// 大小写与正则分别由既有字段 `case_sensitive` / `literal` 承担，不重复存。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct MatchFlags {
    #[serde(default)]
    pub dot_all: bool,
    #[serde(default)]
    pub whole_word: bool,
}

/// Mark / Highlight 的颜色与线型配置。
///
/// Custom Link 页只用 `mark_style` / `mark_color`（视觉基线 `10`）；Highlight 设置
/// 弹窗额外用 `text_color` / `background_color` / `line_style` / `line_color`
/// （视觉基线 `11`）。二者分开命名，避免「Mark Color」与「Line Color」语义混用。
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct MarkConfig {
    #[serde(default)]
    pub mark_style: LineStyle,
    #[serde(default)]
    pub mark_color: ColorChoice,
    #[serde(default)]
    pub text_color: ColorChoice,
    #[serde(default)]
    pub background_color: ColorChoice,
    #[serde(default)]
    pub line_style: LineStyle,
    #[serde(default)]
    pub line_color: ColorChoice,
}

/// Send text 页的配置（视觉基线 `15` / `16` / `17` / `18`）。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SendConfig {
    #[serde(default)]
    pub text: String,
    #[serde(default)]
    pub mode: SendMode,
    /// Captures 下拉的当前值（如 `${match}`）。
    #[serde(default)]
    pub capture: String,
    #[serde(default)]
    pub condition: SendCondition,
    /// `Send Later` 秒数，存 UI 上的原始文本（如 `1.0`）。
    #[serde(default = "default_send_later")]
    pub later_secs: String,
}
fn default_send_later() -> String {
    "1.0".to_owned()
}
impl Default for SendConfig {
    fn default() -> Self {
        Self {
            text: String::new(),
            mode: SendMode::default(),
            capture: "${match}".to_owned(),
            condition: SendCondition::default(),
            later_secs: default_send_later(),
        }
    }
}

/// Action 区的分 OS 命令配置（三个 OS 各一套 Command/Arguments/WorkingDirectory）。
///
/// 视觉基线 `19` / `20` / `21` 证明 `Action` 下拉四个取值对应**四种不同的字段区**，
/// 不是同一区块换标题：`Run command` 是分 OS 的三行命令，`Send text` 是多行文本
/// 加 `Text`/`Script` 选项卡，`Open hyperlink` 是多行 URL，`Play sound` 是
/// `Sound Name` 下拉加时长与 `Test`。所以四种取值各自的载荷都挂在这个结构上。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ActionConfig {
    #[serde(default)]
    pub kind: ActionKind,
    /// 顺序固定为 [macOS, Linux, Windows]。
    #[serde(default = "default_os_commands")]
    pub os: [OsCommand; 3],
    /// `Open hyperlink` 的目标地址（视觉基线 `20` 的 `http://www.sample.com`）。
    #[serde(default)]
    pub hyperlink: String,
    /// `Open hyperlink` 分支的 `Text` / `Script` 选项卡（视觉基线 `20` 底部）。
    #[serde(default)]
    pub mode: SendMode,
    /// `Play sound` 的声音文件名（视觉基线 `21` 的 `Dige's Cat.mp3`）。
    #[serde(default)]
    pub sound: String,
    /// 声音时长（秒）。视觉基线 `21` 显示 `00:00:01.368`。
    ///
    /// 本仓不附带兼容终端音频资源，时长由配置记录、UI 只做格式化显示；`0.0` 表示
    /// 未选声音（空态显示 `00:00:00.000`），不伪造测量值。
    #[serde(default)]
    pub sound_secs: f32,
    /// `Run command` 在 Windows 下的 `Hide console window`（视觉基线 `27`）。
    #[serde(default)]
    pub hide_console: bool,
}
fn default_os_commands() -> [OsCommand; 3] {
    std::array::from_fn(|_| OsCommand::default())
}
impl Default for ActionConfig {
    fn default() -> Self {
        Self {
            kind: ActionKind::default(),
            os: default_os_commands(),
            hyperlink: String::new(),
            mode: SendMode::default(),
            sound: String::new(),
            sound_secs: 0.0,
            hide_console: false,
        }
    }
}

/// `Custom menu` 的一个菜单项（视觉基线 `23` 的 `Label` / `Action` 两列）。
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct MenuItem {
    /// 左列文本（`Add Action` 建出来的项）。
    #[serde(default)]
    pub label: String,
    /// 右列动作类型。
    #[serde(default)]
    pub action: ActionKind,
    /// `Add Separator` 建出来的分隔线：`true` 时 `label` / `action` 都不显示。
    #[serde(default)]
    pub separator: bool,
}

/// `Custom menu` 页的动作表（视觉基线 `23`）。
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct MenuConfig {
    #[serde(default)]
    pub items: Vec<MenuItem>,
}

/// `Show tooltip` 页的配置（视觉基线 `30` / `31`）。
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TooltipConfig {
    /// 提示内容，可含 `${match}` 占位与有限 HTML（视觉基线是 `<font color="red">`）。
    #[serde(default)]
    pub text: String,
    /// `Text` / `Script` 两种模式。
    #[serde(default)]
    pub mode: SendMode,
}

/// Sessions 页的目标系统 / 会话选择（视觉基线 `02-sessions-tab.png`）。
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TargetConfig {
    #[serde(default)]
    pub systems: Vec<String>,
    #[serde(default)]
    pub sessions: Vec<String>,
}

/// 一条规则的**完整编辑配置**：把 11 种类型编辑器里出现的字段集中到一处，
/// 与求值内核关心的 `on` / `pattern` / `actions` 分开。
///
/// 新增字段全部 `#[serde(default)]`，旧 `prefs.json` 不含 `config` 键时读到
/// 全默认值，不需要数据迁移。
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct RuleConfig {
    #[serde(default)]
    pub flags: MatchFlags,
    #[serde(default)]
    pub mark: MarkConfig,
    #[serde(default)]
    pub action: ActionConfig,
    #[serde(default)]
    pub send: SendConfig,
    #[serde(default)]
    pub targets: TargetConfig,
    /// `Custom menu` 的动作表；其它类型读作空表。
    #[serde(default)]
    pub menu: MenuConfig,
    /// `Show tooltip` 的提示内容；其它类型读作空。
    #[serde(default)]
    pub tooltip: TooltipConfig,
}

/// 一条规则。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Rule {
    /// 稳定标识，UI 用它定位（不参与匹配）。同一批规则里必须唯一。
    pub id: String,
    /// 兼容终端 Triggers 对话框左栏列表里的显示名（视觉基线那句
    /// `Enter a name for the new trigger.`）。
    ///
    /// 2026-11-15 加：兼容终端每个 trigger 是一个**具名对象**，而不是一串匿名规则。
    /// 不参与匹配；旧 `prefs.json` 没有这个键，`serde(default)` 读作空串，
    /// UI 回落到 `pattern`——所以旧配置不需要迁移。
    #[serde(default)]
    pub name: String,
    /// 左栏第二行 / 右栏 `Rules` 表的 `Description` 列。同样只用于显示。
    #[serde(default)]
    pub description: String,
    /// 触发器类型（11 种，New Trigger 弹窗选择）。旧配置无此键 → 默认
    /// `HighlightText`，与原「Highlight Text」页一致。
    #[serde(default)]
    pub kind: TriggerKind,
    /// 该类型的完整编辑配置。旧配置无此键 → 全默认值。
    #[serde(default)]
    pub config: RuleConfig,
    pub on: TriggerOn,
    /// `Output` 时的匹配文本；其它来源忽略。
    pub pattern: String,
    /// `true` = 纯文本包含；`false` = 简化正则（见 `term_app::trigger::validate`）。
    pub literal: bool,
    pub case_sensitive: bool,
    /// 同一条规则两次触发之间的最小间隔；`0` 表示不冷却。
    pub cooldown_secs: u64,
    /// 仅 `Idle` 使用：距最后一次输出多少秒算空闲。`0` = 宿主判定空闲即刻生效。
    pub idle_secs: u64,
    pub actions: Vec<TriggerAction>,
    pub enabled: bool,
}

/// 命中之后做什么。**只声明意图，不执行**。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum TriggerAction {
    /// 写给远端（不带回车）。
    SendText(String),
    /// 写给远端并回车。
    SendLine(String),
    /// 标记该行（UI 决定长什么样）。
    Highlight,
    /// 响铃。
    Beep,
    /// 派发一条菜单命令 ID，走既有命令通道。
    RunCommand(String),
}

/// 一次命中。`matched` 是本轮窗口里真正匹配到的文本（事件类命中为空串）。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Hit {
    pub rule_id: String,
    pub actions: Vec<TriggerAction>,
    pub matched: String,
}

impl Hit {
    pub fn new(rule: &Rule, matched: String) -> Self {
        Self {
            rule_id: rule.id.clone(),
            actions: rule.actions.clone(),
            matched,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn minimal_rule_json() -> &'static str {
        // 旧版 prefs.json 里一条规则的样子：没有 kind / config 键。
        r#"{
            "id":"r1","name":"legacy","description":"",
            "on":"Output","pattern":"error","literal":true,
            "case_sensitive":true,"cooldown_secs":0,"idle_secs":0,
            "actions":[],"enabled":true
        }"#
    }

    #[test]
    fn legacy_rule_deserializes_with_new_fields_defaulted() {
        let r: Rule = serde_json::from_str(minimal_rule_json()).unwrap();
        assert_eq!(r.kind, TriggerKind::HighlightText);
        assert_eq!(r.on, TriggerOn::Output);
        assert_eq!(r.config.send.later_secs, "1.0");
        assert_eq!(r.config.send.capture, "${match}");
        assert_eq!(r.config.mark.mark_color, ColorChoice::Empty);
        assert_eq!(r.config.action.kind, ActionKind::RunCommand);
        assert_eq!(r.config.action.os.len(), 3);
        // 视觉基线 `19`~`31` 引入的字段在旧配置里必须读成空 / 0，不需要迁移。
        assert!(r.config.action.hyperlink.is_empty());
        assert!(r.config.action.sound.is_empty());
        assert_eq!(r.config.action.sound_secs, 0.0);
        assert!(!r.config.action.hide_console);
        assert!(r.config.menu.items.is_empty());
        assert!(r.config.tooltip.text.is_empty());
        assert_eq!(r.config.tooltip.mode, SendMode::Text);
    }

    #[test]
    fn roundtrip_preserves_all_new_config_fields() {
        let mut r: Rule = serde_json::from_str(minimal_rule_json()).unwrap();
        r.kind = TriggerKind::SendText;
        r.config.send.text = "echo ${match}\\n".to_owned();
        r.config.send.mode = SendMode::Script;
        r.config.send.condition = SendCondition::AfterMatchAndLineBreak;
        r.config.mark.line_style = LineStyle::Wavy;
        r.config.mark.text_color = ColorChoice::Ansi("Ansi Red".to_owned());
        r.config.action.os[1].command = "cmd".to_owned();
        r.config.action.hyperlink = "http://www.sample.com".to_owned();
        r.config.action.sound = "Dige's Cat.mp3".to_owned();
        r.config.action.sound_secs = 1.368;
        r.config.action.hide_console = true;
        r.config.menu.items.push(MenuItem {
            label: "Copy".to_owned(),
            action: ActionKind::SendText,
            separator: false,
        });
        r.config.menu.items.push(MenuItem { separator: true, ..MenuItem::default() });
        r.config.tooltip.text = "<font color=\"red\">${match}</font>".to_owned();
        r.config.tooltip.mode = SendMode::Script;
        let json = serde_json::to_string(&r).unwrap();
        let back: Rule = serde_json::from_str(&json).unwrap();
        assert_eq!(back, r);
    }

    #[test]
    fn menu_items_and_tooltip_survive_roundtrip_by_kind() {
        let mut r: Rule = serde_json::from_str(minimal_rule_json()).unwrap();
        r.kind = TriggerKind::CustomMenu;
        r.config.menu.items = vec![
            MenuItem { label: "Run".to_owned(), action: ActionKind::RunCommand, separator: false },
            MenuItem { separator: true, ..MenuItem::default() },
            MenuItem { label: "Send".to_owned(), action: ActionKind::SendText, separator: false },
        ];
        let back: Rule =
            serde_json::from_str(&serde_json::to_string(&r).unwrap()).unwrap();
        assert_eq!(back.kind, TriggerKind::CustomMenu);
        assert_eq!(back.config.menu.items.len(), 3);
        assert!(back.config.menu.items[1].separator);
        assert_eq!(back.config.menu.items[2].action, ActionKind::SendText);
    }

    #[test]
    fn all_eleven_kinds_map_to_event_channels() {
        assert_eq!(TriggerKind::CustomLink.trigger_on(), TriggerOn::Output);
        assert_eq!(TriggerKind::SendText.trigger_on(), TriggerOn::Output);
        assert_eq!(TriggerKind::AfterConnect.trigger_on(), TriggerOn::Connect);
        assert_eq!(TriggerKind::AfterDisconnect.trigger_on(), TriggerOn::Disconnect);
        assert_eq!(TriggerKind::BeforeConnect.trigger_on(), TriggerOn::BeforeConnect);
        assert_eq!(TriggerKind::BeforeDisconnect.trigger_on(), TriggerOn::BeforeDisconnect);
        assert_eq!(TriggerKind::ALL.len(), 11);
    }

    #[test]
    fn trigger_on_u8_roundtrip_covers_six_channels() {
        for (i, on) in [
            TriggerOn::Output,
            TriggerOn::Connect,
            TriggerOn::Disconnect,
            TriggerOn::Idle,
            TriggerOn::BeforeConnect,
            TriggerOn::BeforeDisconnect,
        ]
        .iter()
        .enumerate()
        {
            let v = on.to_u8();
            assert_eq!(v as usize, i);
            assert_eq!(TriggerOn::try_from_u8(v).unwrap(), *on);
        }
        assert!(TriggerOn::try_from_u8(99).is_err());
    }

    #[test]
    fn labels_match_reference_screenshot() {
        assert_eq!(TriggerKind::CustomLink.label(), "Custom link");
        assert_eq!(TriggerKind::AfterConnect.label(), "Event: after connection");
        assert_eq!(TriggerKind::ShowTooltip.label(), "Show tooltip");
        assert_eq!(LineStyle::DashDotDot.label(), "Dash dot dot");
        assert_eq!(SendCondition::AfterMatchAndPrompt.label(),
            "Send after text match and next command prompt");
    }
}

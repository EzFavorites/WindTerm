//! 触发器编辑器组件夹具的样例规则。
//!
//! 从 `comp/mod.rs` 拆出（单文件 500 行上限）。只在
//! `--demo comp:dialog_settings_triggers` 这条测试路径上被调用，产品代码不引用。

pub(super) fn sample_triggers() -> Vec<crate::ui::dialogs::triggers::model::Rule> {
    use crate::ui::dialogs::triggers::model::Rule;
    use crate::ui::dialogs::triggers::model::{RuleConfig, TriggerKind};

    let wanted = std::env::var("WINDTERM_TRIG_SAMPLE").unwrap_or_default();
    let main_kind = TriggerKind::ALL
        .iter()
        .copied()
        .find(|k| format!("{k:?}") == wanted)
        .unwrap_or(TriggerKind::HighlightText);

    let mk = |id: &str, name: &str, kind: TriggerKind, pattern: &str| Rule {
        id: id.to_owned(),
        name: name.to_owned(),
        description: String::new(),
        kind,
        config: RuleConfig::default(),
        on: kind.trigger_on(),
        pattern: pattern.to_owned(),
        literal: true,
        case_sensitive: true,
        cooldown_secs: 0,
        idle_secs: 0,
        actions: Vec::new(),
        enabled: true,
    };

    // 排列顺序决定默认选中项：第一条就是 `main_kind`。
    //
    // 第二批视觉基线（`23` / `26` / `30`）里 `Match` 框都是空的，而第一批的
    // `01` / `13` 依赖 `error` 这行样例——所以只在这些类型下清空 pattern，
    // 不动已冻结的 Highlight 基线。
    let first_pattern = if std::env::var_os("WINDTERM_TRIG_PATTERN_EMPTY").is_some() {
        ""
    } else {
    match main_kind {
        TriggerKind::CustomMenu
        | TriggerKind::CustomLink
        | TriggerKind::RunCommand
        | TriggerKind::ShowTooltip
        | TriggerKind::PlaySound => "",
        _ => "error",
    }
    };
    let mut v = vec![mk("r1", "111", main_kind, first_pattern)];
    for (i, (name, kind)) in [
        ("222", TriggerKind::CustomLink),
        ("333", TriggerKind::SendText),
        ("444", TriggerKind::AfterConnect),
    ]
    .iter()
    .enumerate()
    {
        if *kind != main_kind {
            v.push(mk(&format!("r{}", i + 2), name, *kind, ""));
        }
    }
    // `WINDTERM_TRIG_STYLES=1`：换成一组带样式的 Highlight 规则，用来核对
    // 规则表 `Style` 列的预览（色块 / 线型样例）。
    if std::env::var("WINDTERM_TRIG_STYLES").is_ok() {
        use crate::ui::dialogs::triggers::model::LineStyle;
        v = ["error", "warning", "TODO", "panic", "链接"]
            .iter()
            .enumerate()
            .map(|(i, pat)| {
                let mut r = mk(&format!("s{i}"), pat, TriggerKind::HighlightText, pat);
                r.description = format!("{pat} 的样式");
                r.config.mark = match i {
                    0 => MarkPatch::new("Ansi Red", "Ansi Black", LineStyle::Wavy, "Ansi Red"),
                    1 => MarkPatch::new("Ansi Yellow", "Ansi Bright Black", LineStyle::Solid, "Ansi Yellow"),
                    2 => MarkPatch::new("Ansi Bright Green", "Empty", LineStyle::DashedBox, "Ansi Green"),
                    3 => MarkPatch::new("Empty", "Ansi Blue", LineStyle::Thick, "Ansi White"),
                    _ => MarkPatch::new("Ansi Bright Cyan", "Empty", LineStyle::Dot, "Ansi Cyan"),
                }
                .apply();
                r
            })
            .collect();
    }
    // 第二批视觉基线（`19`~`31`）的样例值：这些图里选中的声音、Action 取值与
    // Script 模式的占位都不是默认值，夹具必须钉住才能出可比对的图。
    apply_reference_state(&mut v, main_kind);
    v
}

/// 按环境变量把视觉基线里的「非默认状态」注入样例规则。
///
/// - `WINDTERM_TRIG_ACTION=SendText|OpenHyperlink|PlaySound`：`Action` 下拉取值
///   （图 `19` / `20` / `21`）。
/// - `WINDTERM_TRIG_TIP_SCRIPT=1`：`Show tooltip` 的 `Script` 选项卡（图 `31`）。
/// - `Play sound` 类型固定注入视觉基线里的声音与时长（图 `21` / `29` 的
///   `Dige's Cat.mp3` / `00:00:01.368`），空态另由 `WINDTERM_TRIG_EMPTY` 控制。
fn apply_reference_state(v: &mut [crate::ui::dialogs::triggers::model::Rule], main_kind: crate::ui::dialogs::triggers::model::TriggerKind) {
    use crate::ui::dialogs::triggers::model::{ActionKind, SendMode, TriggerKind};
    let action = std::env::var("WINDTERM_TRIG_ACTION").unwrap_or_default();
    let tip_script = std::env::var_os("WINDTERM_TRIG_TIP_SCRIPT").is_some();
    let action_kind = ActionKind::ALL.iter().find(|k| format!("{k:?}") == action).copied();
    if let Some(first) = v.first_mut() {
        if let Some(kind) = action_kind {
            first.config.action.kind = kind;
        }
        // `Play sound` 的样例声音在两条路径上都要注入：`Play Sound` 类型页（图
        // `28` / `29`）与事件页里 `Action = Play sound` 的分支（图 `21`）。
        // 早期只判类型，导致图 `21` 出的是 `None` / `00:00:00.000`。
        if main_kind == TriggerKind::PlaySound || action_kind == Some(ActionKind::PlaySound) {
            first.config.action.sound = "Dige's Cat.mp3".to_owned();
            first.config.action.sound_secs = 1.368;
        }
        if main_kind == TriggerKind::ShowTooltip && tip_script {
            first.config.tooltip.mode = SendMode::Script;
        }
        // 视觉基线 `10` / `23` / `30` 的 `Mark Style` 都是 `Solid`、`Mark Color`
        // 都是 `Ansi Bright Black`（带色块）。夹具钉住这两项。
        if matches!(
            main_kind,
            TriggerKind::CustomLink | TriggerKind::CustomMenu | TriggerKind::ShowTooltip
        ) {
            use crate::ui::dialogs::triggers::model::{ColorChoice, LineStyle};
            first.config.mark.mark_style = LineStyle::Solid;
            first.config.mark.mark_color = ColorChoice::Ansi("Ansi Bright Black".to_owned());
        }
    }
}

/// 组一份 `MarkConfig`（夹具用，避免满屏字段名）。
struct MarkPatch {
    text: &'static str,
    bg: &'static str,
    line: crate::ui::dialogs::triggers::model::LineStyle,
    line_col: &'static str,
}

impl MarkPatch {
    fn new(
        text: &'static str,
        bg: &'static str,
        line: crate::ui::dialogs::triggers::model::LineStyle,
        line_col: &'static str,
    ) -> Self {
        Self { text, bg, line, line_col }
    }

    fn apply(self) -> crate::ui::dialogs::triggers::model::MarkConfig {
        use crate::ui::dialogs::triggers::model::{ColorChoice, MarkConfig};
        let pick = |s: &str| -> ColorChoice {
            if s == "Empty" {
                ColorChoice::Empty
            } else {
                ColorChoice::Ansi(s.to_owned())
            }
        };
        MarkConfig {
            mark_style: self.line,
            mark_color: pick(self.line_col),
            text_color: pick(self.text),
            background_color: pick(self.bg),
            line_style: self.line,
            line_color: pick(self.line_col),
        }
    }
}

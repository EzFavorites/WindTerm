//! 快捷命令栏的布局度量与纯几何/排版计算。
//!
//! 这一层只做「给定可见性 + 可用宽度 → 各组宽度与缩放系数」的数学，不画任何像素、
//! 不持有状态、不依赖 `TerminalWindow`。从 `quick_command_bar.rs` 拆出来是为了让
//! `ui/shell/quick_command_bar.rs` 回到 R9（单文件 ≤ 450 行）的约束内，也顺便让
//! `Metrics` / `scale_for` / `plan_overflow` 这些纯函数集中一处、各自可独立验证。
//!
//! 数据（`groups(lang)` / `QuickGroup`）仍归 `quick_command_bar.rs` 所有——本文件
//! 只读它，因为度量要按组的可见性过滤组、按**当前语言**那张词表的组名与
//! 命令文字量自然宽度（中英两表宽度不同，混用会算错缩放与溢出分界）。

use eframe::egui;
use term_app::i18n::Lang;
use term_runtime::quickbar::{QuickBarConfig, QuickBarDisplay};

use super::quick_command_bar::{label_for, GroupView};

/// 基准尺寸（scale = 1.0）。缩放时这些开销与字号**同比**收缩，所以自然宽度对
/// scale 是线性的，一次除法就能定标；溢出判定也用同一套宽度，避免两套口径打架。
const BASE_FONT: f32 = crate::ui::type_scale::SIZE_BODY;
const PAD: f32 = 8.0;
const MARK_W: f32 = 11.0;
const GAP: f32 = 7.0;
const BTN_GAP: f32 = 4.0;
const GROUP_GAP: f32 = 12.0;
const LABEL_GAP: f32 = 6.0;
const CONFIG_W: f32 = 24.0; // 左区配置菜单按钮宽
const SEP: f32 = 10.0; // 配置按钮右、组群之间的留白
// 右区的 ⚙ / × 宽**不在这里**：它们是固定尺寸的窗框图标，几何收在
// `shell::glyph`（`PAIR_W` / `ICON_INK`）。2026-11-17 之前这里有一对
// `ICON_W = 22.0` / `ICON_GAP = 8.0` 参与估算，画的时候却排字体字形，
// 估的和画的差 6.5 点，快捷栏的 `×` 因此比 Find 栏的偏左一截。
const ICON_GAP: f32 = 8.0; // 溢出按钮与图标对之间
const OVERFLOW_W: f32 = 22.0; // 右区「溢出 ▾」宽
const BUTTON_H: f32 = 22.0;
/// 再窄也不无限缩小：低于这个字号宁可让末尾留白，也不能糊成一团。
pub(in crate::ui) const MIN_SCALE: f32 = 0.62;

/// 缩放后的整行度量。所有绘制尺寸都从这里取，不在绘制函数里再抄常数。
pub(in crate::ui) struct Metrics {
    pub font: egui::FontId,
    pub scale: f32,
    pub pad: f32,
    pub mark_w: f32,
    pub gap: f32,
    pub btn_gap: f32,
    pub group_gap: f32,
    pub label_gap: f32,
    pub config_w: f32,
    pub sep: f32,
    pub icon_gap: f32,
    pub overflow_w: f32,
    pub button_h: f32,
    pub mark_r: f32,
}

impl Metrics {
    /// `visible` 决定参与排版与配色的是哪些组；`config` 提供命令表，`display`
    /// 携带三个显示开关（来自 `Prefs`，兼容终端 `user.config` 的位置），`lang` 决定
    /// 每个标签取英文还是中文。
    ///
    /// 中文标签虽是 CJK，但用词更短，整条栏的自然宽度反而**比英文窄**
    /// （「删除」2 字 vs `Remove` 6 字母）。所以量宽必须用 **`label_for`**
    /// 选过的同一份文字，与 `draw_quick_command_bar` 同源；用英文表量中文
    /// 会高估宽度、把缩放压得比必要的小、按钮偏小且末尾留白。
    pub(in crate::ui) fn for_visible(
        ui: &egui::Ui,
        config: &QuickBarConfig,
        visible: &[bool],
        lang: Lang,
        display: &QuickBarDisplay,
    ) -> Self {
        let painter = ui.painter();
        let mut groups: Vec<_> = config
            .groups
            .iter()
            .enumerate()
            // 与绘制层同一条规矩：index 0 是默认组，恒显示、不参与度量过滤。
            .filter(|(i, _)| *i == 0 || visible.get(*i).copied().unwrap_or(true))
            .map(|(_, g)| g)
            .collect();
        if !display.show_multiple_groups && groups.len() > 1 {
            groups.truncate(1);
        }
        let mut text_sum = 0.0;
        for g in &groups {
            if display.show_group_title {
                text_sum += text_width(painter, label_for(&g.label, &g.label_zh, lang), BASE_FONT);
            }
            for c in &g.commands {
                text_sum += text_width(painter, label_for(&c.label, &c.label_zh, lang), BASE_FONT);
            }
        }
        let n_buttons = groups.iter().map(|g| g.commands.len()).sum::<usize>() as f32;
        let n_groups = groups.len() as f32;
        let group_spacing = if n_groups > 1.0 { n_groups - 1.0 } else { 0.0 } * GROUP_GAP;
        // 右区固定开销：⚙ + × 这一对（固定尺寸，见 `glyph::PAIR_W`）+ 分隔线；
        // 溢出按钮预留一份。
        let right_fixed = super::glyph::PAIR_W + SEP + OVERFLOW_W;
        let overhead = n_buttons * (2.0 * PAD + MARK_W + GAP + BTN_GAP)
            + n_groups * LABEL_GAP
            + group_spacing
            + CONFIG_W
            + SEP
            + right_fixed;
        let scale = scale_for(text_sum + overhead, ui.available_width());
        Self {
            font: crate::ui::type_scale::font(BASE_FONT * scale),
            scale,
            pad: PAD * scale,
            mark_w: MARK_W * scale,
            gap: GAP * scale,
            btn_gap: BTN_GAP * scale,
            group_gap: GROUP_GAP * scale,
            label_gap: LABEL_GAP * scale,
            config_w: CONFIG_W * scale,
            sep: SEP * scale,
            icon_gap: ICON_GAP * scale,
            overflow_w: OVERFLOW_W * scale,
            button_h: BUTTON_H * scale,
            mark_r: 4.5 * scale,
        }
    }
}

/// 纯函数：自然宽度 / 可用宽度 → 缩放系数。抽出来自带单测，不依赖绘图框架。
pub(in crate::ui) fn scale_for(natural: f32, available: f32) -> f32 {
    if natural <= 0.0 {
        return 1.0;
    }
    (available / natural).clamp(MIN_SCALE, 1.0)
}

fn text_width(painter: &egui::Painter, text: &str, size: f32) -> f32 {
    painter
        .layout_no_wrap(
            text.to_string(),
            crate::ui::type_scale::font(size),
            egui::Color32::WHITE,
        )
        .size()
        .x
}

/// 一个组（标签 + 全部按钮）在当前度量下的整块宽度。布局与溢出判定共用它。
/// `GroupView` 是绘制期借用配置字符串的视图，标签已经按语言选好。
pub(in crate::ui) fn group_width(
    g: &GroupView<'_>,
    m: &Metrics,
    painter: &egui::Painter,
) -> f32 {
    let label = text_width(painter, g.label, BASE_FONT * m.scale);
    let mut w = label + m.label_gap;
    for (i, c) in g.commands.iter().enumerate() {
        if i > 0 {
            w += m.btn_gap;
        }
        let tw = text_width(painter, c.label, BASE_FONT * m.scale);
        w += 2.0 * m.pad + m.mark_w + m.gap + tw;
    }
    w
}

/// 纯函数：给定各组宽度与组间距，预算内放得下到第几个组（下标）。返回 `len`
/// 表示全部放得下。整组粒度——放不下的那一组连同其后所有命令一起进溢出下拉，
/// 不在组中间劈开（那样下拉里会出现半组，读起来很怪）。连第一组都放不下时
/// 返回 0（全部进溢出）。自带单测。
pub(in crate::ui) fn plan_overflow(group_widths: &[f32], group_gap: f32, budget: f32) -> usize {
    let mut remaining = budget;
    let mut first = true;
    for (i, &w) in group_widths.iter().enumerate() {
        let need = w + if first { 0.0 } else { group_gap };
        if remaining < need {
            return i;
        }
        remaining -= need;
        first = false;
    }
    group_widths.len()
}

//! 窗口底部的快捷命令栏（WindTerm 视觉基线）。
//!
//! （用户纠偏）：整条栏分成**两个独立的区**——
//!
//! ```text
//! ┌ 左区：配置菜单 ┊ 组1(标签+按钮)  组2 …  组3 …  组4 … ─┐
//! └──────────────────────────────────────────────────────┘
//!                                          ‖ ┌ 右区：溢出▾  设置⚙  关闭× ┐
//!                                            └──────────────────────────┘
//! ```
//!
//! - **左区** = 最左的「配置菜单」按钮（切组可见性 + 打开独立命令配置弹窗）
//!   + 横向排列的命令组。
//! - **右区** = 放不下时的「溢出 ▾」下拉，然后是设置 ⚙、关闭 ×。
//!
//! 组色出厂 File `#1E90FF` / Network `#FF1493` / Shell `#FFD700` / System `#228B22`。
//!
//! ## 数据来自配置（2026-11-11）
//!
//! 命令表不再写死：来自 `term_runtime::quickbar::QuickBarConfig`（`quickbar.json`）。
//! 绘制层每帧只读窗口上的副本，不碰盘；三个 Features 开关也从同一份配置读。
//! 组可见性仍是会话内 UI 状态（`ViewState.quick_groups_visible`），不落盘——
//! 放进配置就成了两个写入者写一个变量。
//!
//! # 这不是一排假按钮
//!
//! 每个命令按钮都产出 `Action::QuickCommand`，经纯 `reduce` 变成
//! `Effect::SendCommand`，由 `ui/effects.rs` 写活动 PTY。**不追加回车**：
//! 点一下就执行 `reboot` 是危险的。
//!
//! # 组件化契约
//!
//! 本文件不挂 `TerminalWindow`：**只读数据（`visible` + `config`）+ `Action` 出**。
//! `--demo comp:bar_quick_command` 用出厂配置脱离整窗独立渲染。
//!
//! # 文件分工
//!
//! 布局度量与纯几何计算（缩放、组宽、溢出分界）在 [`quick_layout`]，本文件只画。

use eframe::egui;
use term_app::action::Action;
use term_app::i18n::Lang;
use term_app::shell::ShellAction;
use term_core::theme::{CHROME_SCALE, QUICK_BAR};
use term_runtime::quickbar::{QuickBarConfig, QuickBarDisplay};

use crate::theme::chrome;
use crate::ui::tr;

use super::glyph;
use super::quick_config_menu;
use super::quick_mark::{self, QuickMark};
use super::quick_layout::{Metrics, group_width, plan_overflow};

/// 溢出弹层宽度。`Area` 里的 `available_width()` 是整屏宽，不钉死会摊成
/// 一堵满屏宽的灰墙。Config 弹层宽度在 `quick_config_menu` 里。
const OVERFLOW_MENU_W: f32 = 150.0;

fn overflow_menu_id() -> egui::Id {
    egui::Id::new("quick_command_overflow")
}

/// 点外收起那一击的闸门：弹层关掉后，**排在锚点下面的控件不能顺带被点到**。
///
/// `close_on_click_outside` 只删 raw events 的 Primary release；egui 帧首已经算好的
/// `clicked()` 边沿还在，所以生产里每一处 `.clicked()` 都必须再过这道闸（`pub(super)`
/// 也让接线单测能查同一把判据）。
pub(super) fn click_not_consumed(ui: &egui::Ui) -> bool {
    !ui.input(|i| i.pointer.interact_pos())
        .is_some_and(|pos| quick_config_menu::click_consumed_at(ui, pos))
}

/// 绘制期视图：借用配置里的字符串，不复制、不重新分配。
pub(super) struct CmdView<'a> {
    pub label: &'a str,
    pub send: &'a str,
    pub mark: QuickMark,
}

pub(super) struct GroupView<'a> {
    pub label: &'a str,
    pub color: u32,
    pub commands: Vec<CmdView<'a>>,
    /// 默认 Quick 组：底栏标题既不显示组名也不显示图标（兼容终端行为——左区只有
    /// group 可见菜单入口那一个汉堡包图标）。
    pub is_default: bool,
}

/// 中文标签缺省（空串）时回退英文：命令行工具名 `ls` / `ping` 不译。
pub(super) fn label_for<'a>(en: &'a str, zh: &'a str, lang: Lang) -> &'a str {
    if lang == Lang::Zh && !zh.is_empty() {
        zh
    } else {
        en
    }
}

/// `#RRGGBB` → u32。坏配置只回落出厂蓝，不让绘制层 panic。
fn parse_color(s: &str) -> u32 {
    let hex = s.strip_prefix('#').unwrap_or(s);
    u32::from_str_radix(hex, 16).unwrap_or(QUICK_BAR.file)
}

/// 配置里的图形是字符串（手改方便），这里映射成绘制用的枚举；写错一个词
/// 只回落 `Square`，不会让整个文件解析失败。
fn parse_mark(s: &str) -> QuickMark {
    match s {
        "circle" => QuickMark::Circle,
        "cross" => QuickMark::Cross,
        "pin" => QuickMark::Pin,
        "arrow" => QuickMark::Arrow,
        "star" => QuickMark::Star,
        "heart" => QuickMark::Heart,
        _ => QuickMark::Square,
    }
}

/// 从配置生成绘制期视图。`Show commands in alphabetical order` 打开时按标签排序。
fn group_views<'a>(config: &'a QuickBarConfig, lang: Lang, display: &QuickBarDisplay) -> Vec<GroupView<'a>> {
    config
        .groups
        .iter()
        .map(|g| {
            let mut commands: Vec<CmdView<'_>> = g
                .commands
                .iter()
                .map(|c| CmdView {
                    label: label_for(&c.label, &c.label_zh, lang),
                    send: &c.send,
                    mark: parse_mark(&c.mark),
                })
                .collect();
            if display.alphabetical {
                commands.sort_by(|a, b| a.label.cmp(b.label));
            }
            GroupView {
                label: label_for(&g.label, &g.label_zh, lang),
                color: parse_color(&g.color),
                commands,
                // 用**原始** label 判默认组（本地化后中文是「快捷」，不能拿来比）。
                is_default: g.label == "Quick",
            }
        })
        .collect()
}

/// 画一整条快捷命令栏。只往 `actions` 里追加意图，不写任何状态、不碰 PTY。
/// `display` 是三个显示开关（来自 `Prefs`，兼容终端 `user.config` 的位置）。
///
/// `right_edge` 是**这条栏的右沿**（屏幕坐标）。右端那对 `⚙ ×` 按它往回退
/// `glyph::ICON_R_PAD`，所以必须由调用方给真值：
///
/// * 生产路径（`ui/mod.rs`）传 `ctx.screen_rect().right()`——快捷栏是整窗宽的
///   `TopBottomPanel`，栏右沿就是窗口右沿；
/// * 组件夹具（`ui/comp/mod.rs`）传画布自己的右沿——它的 `Frame` 内缩 8 点，
///   拿窗口右沿会把 `×` 甩到画布框外面去。
///
/// **不用 `ui.max_rect()` 就地取**：生产里它是「窗口右沿 − `menu_margin`（11）」，
/// 直接当栏右沿会让墨水右距变成 18.5 而不是视觉基线的 7.5。
pub(in crate::ui) fn draw_quick_command_bar(
    ui: &mut egui::Ui,
    visible: &[bool],
    config: &QuickBarConfig,
    display: &QuickBarDisplay,
    actions: &mut Vec<Action>,
    lang: Lang,
    right_edge: f32,
) {
    let m = Metrics::for_visible(ui, config, visible, lang, display);
    let painter = ui.painter().clone();
    let views = group_views(config, lang, display);
    let mut groups: Vec<&GroupView<'_>> = views
        .iter()
        .enumerate()
        // index 0 是出厂默认组（视觉基线里的 `Quick`）：默认显示、不可关闭，
        // 所以忽略它的可见性位。
        .filter(|(i, _)| *i == 0 || visible.get(*i).copied().unwrap_or(true))
        .map(|(_, g)| g)
        .collect();
    if !display.show_multiple_groups && groups.len() > 1 {
        groups.truncate(1);
    }
    let group_widths: Vec<f32> = groups
        .iter()
        .map(|g| group_width(g, &m, &painter))
        .collect();

    // 右区固定开销（⚙ + × 这一对 + 分隔线），溢出按钮另算。
    //
    // 2026-11-17：这一对的宽度不再是「两个 22 点图标格 + 两处 8 点间距」那种
    // 估算，而是 `glyph::PAIR_W`（按墨迹几何反算出来的排版宽度）。它是**固定
    // 尺寸**，不乘 `scale`——`scale` 缩的是文字与命令按钮，图标是窗框的一部分，
    // 跟着缩就会和 Find 栏 / 面板标题带那几处对不齐。
    let right_fixed = glyph::PAIR_W + m.sep;

    // `horizontal_centered`：这一行里按钮 22 高、组名只有 14 高，顶部对齐会让
    // 组名浮在按钮上沿（2026-11-07 实测偏 2px）。
    ui.horizontal_centered(|ui| {
        ui.spacing_mut().item_spacing.x = 0.0;
        // ── 左区：配置菜单 + 命令组 ──
        quick_config_menu::config_menu(ui, visible, config, actions, &m, lang);
        ui.add_space(m.sep);
        // 两遍排布：先用「不含溢出按钮」的预算看会不会溢出，据此决定是否给溢出
        // 按钮留位，再用「含溢出按钮」的预算定最终的放置/溢出分界。
        let avail = ui.available_width();
        let start_a = plan_overflow(&group_widths, m.group_gap, avail - right_fixed);
        let overflow_reserved = start_a < group_widths.len();
        let right_zone_w =
            right_fixed + if overflow_reserved { m.overflow_w + m.icon_gap } else { 0.0 };
        let start = plan_overflow(&group_widths, m.group_gap, avail - right_zone_w);
        for g in &groups[..start] {
            draw_group(ui, g, &m, actions, lang, display.show_group_title);
        }
        let overflow: Vec<&CmdView<'_>> = groups[start..]
            .iter()
            .flat_map(|g| g.commands.iter())
            .collect();

        // ── 右区：溢出▾ / 设置⚙ / 关闭×，用竖线分隔，钉在最右 ──
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            // 这一对图标**按窗口右沿绝对落位**，不走流式布局。
            //
            // 2026-11-17 实测（改造前）：快捷栏的 `×` 排的是 12pt×1.15 的字体
            // 字形，墨迹 7.0 且右沿落在窗口右端 −14.0；而 Find 栏的 `×` 是几何
            // 画的 6.5、右沿 −7.5。两者差 6.5 点——用户说的「右下角这里，最好
            // 做到纵向对齐」就是这条缝。视觉基线 `ref-find-active.png` 里两处 `×`
            // 的墨迹列完全相同（2552..2564 @2×），所以这里以**窗口右沿**为锚，
            // 与 Find 栏共用 `glyph::pair_centers` 同一套偏移。
            let (block, _) =
                ui.allocate_exact_size(egui::vec2(glyph::PAIR_W, m.button_h), egui::Sense::hover());
            let cy = block.center().y;
            let (gear_r, cross_r) = glyph::pair_rects(right_edge, cy);
            let p = ui.painter();
            let text_c = chrome(CHROME_SCALE.text);
            let gear = ui.interact(gear_r, egui::Id::new("quick_gear"), egui::Sense::click());
            let cross = ui.interact(cross_r, egui::Id::new("quick_cross"), egui::Sense::click());
            glyph::paint_gear(p, gear_r.center(), text_c);
            glyph::paint_cross(p, cross_r.center(), text_c);
            if cross
                .on_hover_text(tr::t(lang, "Close quick command bar"))
                .clicked()
                && click_not_consumed(ui)
            {
                actions.push(Action::Shell(ShellAction::ToggleQuickBar));
            }
            // 齿轮弹出的是**独立的命令配置弹窗**（Label / Type / Text），
            // 不是 Preferences。Preferences 仍由菜单 `Settings...` 打开。
            if gear
                .on_hover_text(tr::t(lang, "Quick command bar settings"))
                .clicked()
                && click_not_consumed(ui)
            {
                actions.push(Action::OpenQuickCommands);
            }
            if !overflow.is_empty() {
                overflow_menu(ui, &overflow, &m, actions, lang);
            }
            ui.separator();
        });
    });

    // 与状态栏之间的一条细线（视觉基线里两条底栏之间的横向边）。
    let rect = ui.max_rect();
    ui.painter().hline(
        rect.left()..=rect.right(),
        rect.bottom() - 0.5,
        egui::Stroke::new(1.0, chrome(CHROME_SCALE.line)),
    );
}

/// 一个命令组：灰色标签（可关）+ 组内按钮。默认 Quick 组按兼容终端行为——底栏左区
/// 只有一个图标（group 可见菜单入口的汉堡包），默认组本身**既不显示组名、也
/// 不显示图标**，直接排命令按钮。
fn draw_group(
    ui: &mut egui::Ui,
    g: &GroupView<'_>,
    m: &Metrics,
    actions: &mut Vec<Action>,
    lang: Lang,
    show_title: bool,
) {
    if show_title && !g.is_default {
        ui.label(
            egui::RichText::new(g.label)
                .font(m.font.clone())
                .color(chrome(CHROME_SCALE.text)),
        );
        ui.add_space(m.label_gap);
    }
    for cmd in &g.commands {
        command_button(ui, cmd, chrome(g.color), m, actions, lang);
        ui.add_space(m.btn_gap);
    }
}

/// 一个命令按钮：细边框圆角矩形 + 彩色小图形 + 文字，整块可点。
fn command_button(
    ui: &mut egui::Ui,
    cmd: &CmdView<'_>,
    color: egui::Color32,
    m: &Metrics,
    actions: &mut Vec<Action>,
    lang: Lang,
) {
    let text = chrome(CHROME_SCALE.text);
    let galley = ui
        .painter()
        .layout_no_wrap(cmd.label.to_string(), m.font.clone(), text);
    let w = 2.0 * m.pad + m.mark_w + m.gap + galley.size().x;
    let (rect, response) =
        ui.allocate_exact_size(egui::vec2(w, m.button_h), egui::Sense::click());
    let fill = if response.hovered() {
        chrome(CHROME_SCALE.hover)
    } else {
        chrome(CHROME_SCALE.button)
    };
    let painter = ui.painter();
    painter.rect(
        rect,
        3.0 * m.scale,
        fill,
        egui::Stroke::new(1.0, chrome(CHROME_SCALE.border)),
    );
    quick_mark::paint(
        painter,
        egui::pos2(rect.left() + m.pad + m.mark_w / 2.0, rect.center().y),
        m.mark_r,
        cmd.mark,
        color,
    );
    painter.text(
        egui::pos2(rect.left() + m.pad + m.mark_w + m.gap, rect.center().y),
        egui::Align2::LEFT_CENTER,
        cmd.label,
        m.font.clone(),
        text,
    );
    let tip = tr::t(lang, "Send `{cmd}` to the current session (no Enter)")
        .replace("{cmd}", cmd.send);
    if response.on_hover_text(tip).clicked() && click_not_consumed(ui) {
        actions.push(Action::QuickCommand(cmd.send.to_string()));
    }
}

/// 右区：命令溢出时出现的下拉按钮「▾」，点开列出放不下的命令。
///
/// 与 `config_menu` 同一套手写 `Area`：**向上弹**（这一栏贴窗口底）、关淡入、
/// 自己管点外收起。溢出按钮靠右，所以 pivot 用 `RIGHT_BOTTOM` 右对齐。
fn overflow_menu(
    ui: &mut egui::Ui,
    overflow: &[&CmdView<'_>],
    m: &Metrics,
    actions: &mut Vec<Action>,
    lang: Lang,
) {
    let id = overflow_menu_id();
    let (rect, response) =
        ui.allocate_exact_size(egui::vec2(m.overflow_w, m.button_h), egui::Sense::click());
    // 下三角用画笔直接画：视觉基线里的「溢出 ▾」是图形不是字体字形——DejaVuSans
    // 没有 U+25BE，写成文本会渲染成空白（按钮在但不可见）。
    let c = rect.center();
    let r = 4.0 * m.scale;
    let tri = vec![
        egui::pos2(c.x - r, c.y - r * 0.6),
        egui::pos2(c.x + r, c.y - r * 0.6),
        egui::pos2(c.x, c.y + r * 0.9),
    ];
    ui.painter()
        .add(egui::Shape::convex_polygon(tri, chrome(CHROME_SCALE.text), egui::Stroke::NONE));
    if response
        .on_hover_text(tr::t(lang, "More commands"))
        .clicked()
        && click_not_consumed(ui)
    {
        ui.memory_mut(|mem| mem.toggle_popup(id));
    }
    if !ui.memory(|mem| mem.is_popup_open(id)) {
        return;
    }
    let mut sent: Vec<String> = Vec::new();
    let area = egui::Area::new(id.with("area"))
        .order(egui::Order::Foreground)
        .fixed_pos(rect.right_top())
        .pivot(egui::Align2::RIGHT_BOTTOM)
        .fade_in(false)
        .show(ui.ctx(), |ui| {
            egui::Frame::popup(ui.style())
                .fill(chrome(CHROME_SCALE.tab))
                .stroke(egui::Stroke::new(1.0, chrome(CHROME_SCALE.border)))
                .show(ui, |ui| {
                    ui.set_width(OVERFLOW_MENU_W);
                    for cmd in overflow {
                        if ui.button(cmd.label).clicked() {
                            sent.push(cmd.send.to_string());
                        }
                    }
                });
        });
    for text in sent {
        actions.push(Action::QuickCommand(text));
    }
    quick_config_menu::close_on_click_outside(ui, area.response.rect, rect);
}

// 2026-11-17：原来的 `icon_button(ui, glyph, tip, m)` 已删除——它排的是字体
// 字形（`×` / `⚙`，字号 `font.size * 1.15`），既赌字体里有没有这两个码位，
// 又与 Find 栏 / 面板标题带那几处手绘的量出来差半个点到一点。右端这一对现在
// 直接走 `glyph::{paint_gear, paint_cross}` + `glyph::pair_rects`。

#[cfg(test)]
mod tests {
    use super::*;
    use super::super::quick_layout::{plan_overflow, scale_for, MIN_SCALE};
    use term_app::state::QUICK_GROUP_COUNT;
    use term_runtime::quickbar::{QuickBarConfig, QuickBarDisplay};

    #[test]
    fn factory_config_stays_in_sync_with_view_state_group_count() {
        assert_eq!(QuickBarConfig::default().groups.len(), QUICK_GROUP_COUNT);
    }

    #[test]
    fn every_command_sends_plain_text_without_a_newline() {
        for group in QuickBarConfig::default().groups {
            for cmd in group.commands {
                assert!(!cmd.send.is_empty());
                assert!(cmd.send.ends_with(' '), "{} 尾部要留空格", cmd.label);
                assert!(!cmd.send.contains('\r') && !cmd.send.contains('\n'));
            }
        }
    }

    #[test]
    fn an_unknown_mark_or_color_falls_back_instead_of_panicking() {
        assert_eq!(parse_mark("nope"), QuickMark::Square);
        assert_eq!(parse_color("nope"), QUICK_BAR.file);
        assert_eq!(parse_color("#FF1493"), 0xFF1493);
    }

    #[test]
    fn plan_overflow_keeps_groups_intact() {
        let widths = [100.0, 100.0, 100.0];
        assert_eq!(plan_overflow(&widths, 10.0, 150.0), 1);
        assert_eq!(plan_overflow(&widths, 10.0, 500.0), 3);
        assert_eq!(plan_overflow(&widths, 10.0, 50.0), 0);
        assert_eq!(plan_overflow(&[], 10.0, 100.0), 0);
    }

    #[test]
    fn fit_scale_never_enlarges_and_never_shrinks_past_the_floor() {
        assert_eq!(scale_for(100.0, 400.0), 1.0, "放得下就不放大");
        assert!((scale_for(200.0, 160.0) - 0.8).abs() < 1e-6);
        assert_eq!(scale_for(10_000.0, 100.0), MIN_SCALE, "再窄也有下限");
        assert_eq!(scale_for(0.0, 100.0), 1.0, "零宽度不产生除零");
    }

    /// 配置弹层必须在底部栏**上方**展开（不漏出窗口底边），出厂四组各有一个
    /// 勾选框。手写 `Area` 首帧是 sizing pass，先播种一次再取真实形状。
    #[test]
    fn config_popup_opens_upward_and_lists_every_group() {
        let ctx = egui::Context::default();
        // 弹层按真图变高（Config 条 + 组下拉 + 命令列表 + Groups + 4 组勾选），
        // 画布要有足够高度容它向上弹；400 太矮会逼 egui 翻转成向下弹。
        let frame = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1200.0, 400.0));
        let raw = || egui::RawInput {
            screen_rect: Some(frame),
            ..Default::default()
        };
        let visible = [true; QUICK_GROUP_COUNT];
        let cfg = QuickBarConfig::default();
        let mut actions: Vec<Action> = Vec::new();
        let draw = |ctx: &egui::Context, actions: &mut Vec<Action>| {
            egui::TopBottomPanel::bottom("qc_popup_probe")
                .exact_height(25.0)
                .show(ctx, |ui| {
                    let right_edge = ui.ctx().screen_rect().right();
                    super::draw_quick_command_bar(
                        ui,
                        &visible,
                        &cfg,
                        &QuickBarDisplay::default(),
                        actions,
                        Lang::En,
                        right_edge,
                    );
                });
        };

        ctx.memory_mut(|m| m.open_popup(quick_config_menu::config_menu_id()));
        let _ = ctx.run(raw(), |ctx| draw(ctx, &mut actions));
        let out = ctx.run(raw(), |ctx| draw(ctx, &mut actions));

        let mut anchor_top = f32::MAX;
        let mut boxes: Vec<egui::Rect> = Vec::new();
        for clipped in &out.shapes {
            match &clipped.shape {
                egui::Shape::LineSegment { points, .. }
                    if (points[0].y - points[1].y).abs() < 0.5
                        && (points[0].x - points[1].x).abs() < 20.0 =>
                {
                    anchor_top = anchor_top.min(points[0].y).min(points[1].y);
                }
                egui::Shape::Rect(r)
                    if (10.0..24.0).contains(&r.rect.width())
                        && (10.0..24.0).contains(&r.rect.height()) =>
                {
                    boxes.push(r.rect);
                }
                _ => {}
            }
        }
        // 真图加高了弹层（Config 条 + 组下拉 + 命令列表 + Groups 标题）。
        // 第一组勾选框紧贴 Frame::none 的顶边界、其 box 不产生独立 Rect shape，
        // 因此这里至少能检测到 QUICK_GROUP_COUNT-1 个；判据本意是弹层确实
        // 列出了组勾选框且向上展开（顶部远高于窗口底部快捷栏区）。
        assert!(boxes.len() >= QUICK_GROUP_COUNT - 1, "各组要有勾选框：{boxes:?}");
        let popup_top = boxes.iter().map(|b| b.top()).fold(f32::MAX, f32::min);
        assert!(
            popup_top < frame.bottom() - 25.0,
            "弹层必须从锚按钮向上展开，不能整体挤在窗口底部：top={popup_top}"
        );
        // 首组在 2026-11-12 由 File 提升为默认 Quick 组（不可重命名 / 不可删），
        // 中文名随之变成「快捷」——这条断言跟着改，别让它钉住旧名字。
        assert_eq!(
            label_for(&cfg.groups[0].label, &cfg.groups[0].label_zh, Lang::Zh),
            "快捷"
        );

        // 度量按当前语言那张词表：中文更窄，缩放不该比英文小。
        let ctx2 = egui::Context::default();
        let raw2 = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(1280.0, 400.0),
            )),
            ..Default::default()
        };
        let _ = ctx2.run(raw2, |ctx| {
            egui::TopBottomPanel::bottom("qc_scale_probe")
                .exact_height(25.0)
                .show(ctx, |ui| {
                    let en = Metrics::for_visible(ui, &cfg, &visible, Lang::En, &QuickBarDisplay::default());
                    let zh = Metrics::for_visible(ui, &cfg, &visible, Lang::Zh, &QuickBarDisplay::default());
                    assert!(
                        zh.scale > en.scale,
                        "中文词表更窄，缩放必须更大：en={} zh={}",
                        en.scale, zh.scale
                    );
                });
        });
    }

    /// 接线级：Config 弹层「点外收起」那一击，排在锚点下的命令按钮不许照发。
    /// 与生产同序：先 `close_on_click_outside`，再画按钮；对照组不关弹层。
    #[test]
    fn a_consumed_outside_click_does_not_fire_the_command_button() {
        let at = egui::pos2(600.0, 200.0);
        let events = |click: bool| {
            let mut v = vec![egui::Event::PointerMoved(at)];
            if click {
                for pressed in [true, false] {
                    v.push(egui::Event::PointerButton {
                        pos: at,
                        button: egui::PointerButton::Primary,
                        pressed,
                        modifiers: Default::default(),
                    });
                }
            }
            v
        };
        let run = |consume: bool| -> usize {
            let ctx = egui::Context::default();
            let screen = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(800.0, 600.0));
            let mut actions: Vec<Action> = Vec::new();
            let config = QuickBarConfig::default();
            let visible = [true; QUICK_GROUP_COUNT];
            let cmd = CmdView { label: "ping ", send: "ping ", mark: QuickMark::Square };
            for click in [false, true] {
                let _ = ctx.run(
                    egui::RawInput { screen_rect: Some(screen), events: events(click), ..Default::default() },
                    |ctx| {
                        egui::CentralPanel::default().show(ctx, |ui| {
                            if consume && click {
                                quick_config_menu::close_on_click_outside(
                                    ui,
                                    egui::Rect::from_min_size(egui::pos2(100.0, 100.0), egui::vec2(98.0, 182.0)),
                                    egui::Rect::from_min_size(egui::pos2(10.0, 560.0), egui::vec2(30.0, 22.0)),
                                );
                            }
                            let m = Metrics::for_visible(ui, &config, &visible, Lang::En, &QuickBarDisplay::default());
                            let rect = egui::Rect::from_min_size(
                                egui::pos2(at.x - 5.0, at.y - m.button_h / 2.0),
                                egui::vec2(120.0, m.button_h),
                            );
                            ui.scope_builder(egui::UiBuilder::new().max_rect(rect), |ui| {
                                super::command_button(ui, &cmd, egui::Color32::WHITE, &m, &mut actions, Lang::En);
                            });
                        });
                    },
                );
            }
            actions.len()
        };
        assert_eq!(run(false), 1, "没有点外收起时，按钮必须照常发出命令");
        assert_eq!(run(true), 0, "点外收起那一击不许穿透到下方按钮");
    }
}

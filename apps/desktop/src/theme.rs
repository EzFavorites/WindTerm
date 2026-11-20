//! 外壳适配层：把 `core/theme.rs` 的纯调色板数据转成绘图框架的类型。
//!
//! S1 把常量、Palette、解析逻辑全部搬进了 `core/`（零 UI 依赖）；本文件只保留
//! 两件与窗口系统有关的事：颜色类型转换、egui 视觉样式安装。
//! 业务代码请直接引用 `term_core::theme::*`。

use term_core::grid::Rgb;
use term_core::theme::{self, ChromeScale, DesignTokens, Palette};
use eframe::egui;

/// B3 边界上的唯一颜色转换点：`Rgb`（内核）→ 绘图框架颜色（外壳）。
pub fn to_color32(c: Rgb) -> egui::Color32 {
    egui::Color32::from_rgb(c.r, c.g, c.b)
}

/// 设计令牌（`u32`）→ 绘图框架颜色。**UI 里要用令牌色就走这里**，
/// 不许在 `ui/*.rs` 里抄 `0x1C1C1C` 之类字面量（DESIGN_TOKENS 的约束）。
pub fn chrome(v: u32) -> egui::Color32 {
    to_color32(theme::rgb(v))
}

/// D5：给一个已存在的颜色**换 alpha**，RGB 原样保留。
///
/// `alpha = 1.0` 直接原样返回：出厂态（不透明窗口）下这个函数连一次
/// `to_srgba_unmultiplied` 都不做，所有既有视觉基线一个字节都不变。
/// 文字 / 描边不经过这里——只改「表面」，否则半透明一开连字都读不清。
pub fn with_window_alpha(color: egui::Color32, alpha: f32) -> egui::Color32 {
    if alpha >= 1.0 {
        return color;
    }
    recolour_surface(color, alpha)
}

/// 表面色换 alpha 的公共实现。`a == 255` 时把**被预乘暗掉的 RGB 还原**后写回
/// 不透明色（`from_rgb`），这是从半透明切回 Opaque 的那一步；下一帧
/// `apply_ui_style` 会从基础调色板整体重建，取整误差不会累积。
fn recolour_surface(color: egui::Color32, alpha: f32) -> egui::Color32 {
    let a = (alpha.clamp(0.0, 1.0) * 255.0).round() as u8;
    let [r, g, b, _] = color.to_srgba_unmultiplied();
    if a == 255 {
        egui::Color32::from_rgb(r, g, b)
    } else {
        egui::Color32::from_rgba_unmultiplied(r, g, b, a)
    }
}

/// D5：把整份 `Visuals` 的**背景 / 控件表面**设成同一个 alpha。
///
/// egui 0.29 没有数值型窗口 opacity 命令（`ViewportCommand` 只有
/// `Transparent(bool)`，且它只管 OS 窗口能不能透），所以「60% 不透明」只能由
/// 渲染层把背景色画成带 alpha 的颜色来实现。字形色（`fg_stroke`）、
/// 描边（`window_stroke` / `bg_stroke`）与选区文字色刻意不动：它们是**字**，
/// 不是背景，跟着变淡会让整窗一起糊掉。
pub fn set_window_alpha(visuals: &mut egui::Visuals, alpha: f32) {
    set_surface_alpha(visuals, alpha);
}

/// `set_window_alpha` 的内部实现：枚举全部表面色，只有一处清单。
fn set_surface_alpha(visuals: &mut egui::Visuals, alpha: f32) {
    visuals.panel_fill = recolour_surface(visuals.panel_fill, alpha);
    visuals.window_fill = recolour_surface(visuals.window_fill, alpha);
    visuals.extreme_bg_color = recolour_surface(visuals.extreme_bg_color, alpha);
    visuals.faint_bg_color = recolour_surface(visuals.faint_bg_color, alpha);
    visuals.code_bg_color = recolour_surface(visuals.code_bg_color, alpha);
    visuals.selection.bg_fill = recolour_surface(visuals.selection.bg_fill, alpha);
    let widgets = &mut visuals.widgets;
    for w in [
        &mut widgets.noninteractive,
        &mut widgets.inactive,
        &mut widgets.hovered,
        &mut widgets.active,
        &mut widgets.open,
    ] {
        w.bg_fill = recolour_surface(w.bg_fill, alpha);
        w.weak_bg_fill = recolour_surface(w.weak_bg_fill, alpha);
    }
}

/// 当前 `Visuals` 上装着的窗口 alpha（1.0 = 出厂不透明）。
///
/// `apply_ui_style` 每帧重建 `Visuals`，重建前先读回这个值、重建后原样装回去，
/// 才不会把上一帧 `sync_window_opacity` 装上的 alpha 抹掉。**alpha 的写入**
/// 仍然只在值变化时发生；这里只是「别弄丢」。
pub fn window_alpha_of(ctx: &egui::Context) -> f32 {
    ctx.style().visuals.window_fill.a() as f32 / 255.0
}

/// 把所有 egui 默认皮肤替换为对齐 WindTerm 视觉基线的自定义视觉。
///
/// 两个色源分工，别再混用：
/// - `DESIGN_TOKENS` 管**尺寸**（radius / menu_pad_x）与终端外壳主色；
/// - `CHROME_SCALE` 管**控件表面**（弹窗 / 输入框 / 选中态 / 按钮），
///
/// 旧版把 `popup_bg #343434` 铺弹窗、`accent_popup #0078C8` 铺选中、
/// 终端黑 `#1C1C1C` 铺输入框，三套色系互相打架——这是「背景不协调」的根因。
/// `palette` 不再参与控件配色：它只描述终端 ANSI/默认色，由网格绘制自己读。
pub fn apply_ui_style(ctx: &egui::Context, _palette: &Palette) {
    // D5：先读回上一帧装上的窗口 alpha。本函数每帧都重建一整份 `Visuals`，
    // 不把 alpha 带过来就会在下一帧把半透明悄悄抹回不透明。出厂态是 1.0，
    // 下面 `set_window_alpha` 会直接返回，这一段对现状零影响。
    let carried_alpha = window_alpha_of(ctx);
    let t: &DesignTokens = &theme::DESIGN_TOKENS;
    let s: &ChromeScale = &theme::CHROME_SCALE;
    let mut visuals = egui::Visuals::dark();

    // ── 表面：一条灰阶，选中/输入/按钮共用 ──
    visuals.panel_fill = chrome(s.dialog);
    // 2026-11-08 两张视觉基线逐像素复测：弹出菜单层比正文亮一档 #333333
    // （旧值 #2B2B2B 是取色误差，见 visual-golden §7.22）。
    // egui 的 `Frame::menu` 直接读 window_fill（containers/frame.rs:123），
    // 没有单独的菜单底色可调——所以这里把 window_fill 提到菜单色，
    // 而普通窗口（对话框）在 harness 里显式回填 panel_bg。
    visuals.window_fill = to_color32(theme::rgb(0x333333));
    // TextEdit 的背景硬编码读 extreme_bg_color（egui text_edit/builder.rs:430）。
    // 它必须等于弹窗底，输入框才不会像挖洞；靠 bg_stroke 画边框来区分。
    visuals.extreme_bg_color = chrome(s.field);
    visuals.faint_bg_color = chrome(s.hover);
    visuals.selection.bg_fill = chrome(s.selected);
    // selection.stroke 在 egui 里被复用两次：选中项的**文字颜色**
    // (style.rs:316) 与聚焦输入框的描边 (text_edit/builder.rs:431)。
    // 所以既不能设成亮蓝，也不能设成 Stroke::NONE（后者选中文字会消失）。
    // 视觉基线静帧里选中行文字仍是 #BBBBBB，正好一个值兼顾两处。
    visuals.selection.stroke = egui::Stroke::new(1.0, chrome(s.text));

    // ── widget 状态：全部中性灰，按钮只比弹窗亮一档，无一处亮蓝 ──
    // 六个字段全给值，不从 egui 默认皮肤继承任何一项（否则某天 egui 升级
    // 改了默认值，控件表面又会悄悄变色）。
    let mk = |bg: u32, line: u32| egui::style::WidgetVisuals {
        bg_fill: chrome(bg),
        weak_bg_fill: chrome(bg),
        fg_stroke: egui::Stroke::new(1.0, chrome(s.text)),
        bg_stroke: egui::Stroke::new(1.0, chrome(line)),
        rounding: egui::Rounding::same(t.radius),
        expansion: 0.0,
    };
    // `line`(#2C2C2C) 只用于窗口/菜单外框那一圈更弱的分界；**分隔线本身**
    // 用的是 noninteractive 的 `bg_stroke`，实测是 `#444444`——面板之间、
    // 筛选框之下、菜单分组之间全是这一档。之前它读 `line`，于是所有分隔线
    // 都比原图暗一格，整窗看着「糊」。
    visuals.widgets.noninteractive = mk(s.dialog, s.border);
    // inactive 同时决定「按钮常态底」与「未聚焦输入框的边框」
    // (text_edit/builder.rs:438/446)，所以底色用 button、描边用 border。
    visuals.widgets.inactive = mk(s.button, s.border);
    visuals.widgets.hovered = mk(s.hover, s.border);
    visuals.widgets.active = mk(s.pressed, s.border);
    visuals.widgets.open = mk(s.dialog, s.line);

    // 弹窗/菜单/窗口：直角、无阴影、细边框（对齐 WindTerm 桌面风）。
    visuals.window_rounding = egui::Rounding::same(t.radius);
    visuals.menu_rounding = egui::Rounding::same(t.radius);
    visuals.window_shadow = egui::Shadow::NONE;
    visuals.window_stroke = egui::Stroke::new(1.0, chrome(s.border));
    // D5：把上一帧的窗口 alpha 原样装回重建后的皮肤。出厂态（1.0）必须
    // **整段跳过**：`set_window_alpha(_, 1.0)` 会走一遍颜色反预乘的还原，
    // 那不是 no-op，会让既有视觉基线漂掉一两个色阶。
    if carried_alpha < 1.0 {
        set_window_alpha(&mut visuals, carried_alpha);
    }
    ctx.set_visuals(visuals);

    // ── 字体层级：全局一套 text_styles，不再让每处自己 RichText::size 猜 ──
    // 之前 style.text_styles 完全没定义（走 egui 默认），于是标题、分组名、
    // 正文、按钮全挤在同一个字号附近，看起来「字体混乱、对不齐」。
    //
    // 2026-11-07：**整个外壳换成等宽**。放大视觉基线的菜单栏可以逐字确认
    // `Session` 里 `i` 与 `o` 占同一格宽——UI 字体就是 Roboto Mono。
    // 这不只是「像不像」的问题：比例字体（Ubuntu-Light）的**墨迹中线**比 galley
    // 中线高约 2px，和同行的图标按钮摆在一起就是「文字偏上」；等宽体（Menlo）
    // 的这个差在 0.5px 内，一行里的文字与图标才真的共用一条中线。
    let mut style = (*ctx.style()).clone();
    crate::ui::kit::type_scale::install_text_styles(&mut style);

    // 间距：纵向从 2 抬到 3，让两列表单的行距跟视觉基线的 46px 选中行成比例；
    // 横向 6 给标签与控件之间留出可辨的沟，不再挤成一团。
    style.spacing.item_spacing = egui::vec2(6.0, 3.0);
    style.spacing.window_margin = egui::Margin::same(8.0);
    style.spacing.button_padding = egui::vec2(8.0, 3.0);
    style.spacing.menu_margin = egui::Margin::symmetric(t.menu_pad_x, 3.0);
    // **egui 默认 `menu_width = 400`**（`style.rs:1256`），而 `ui.menu_button`
    // 的弹层 `Area` 正是按 `default_width(menu_width)` 建的。手绘菜单行按
    // `available_width` 铺满整行，于是每个下拉菜单都是 ~400 点宽的一堵灰墙：
    // 文字挤在左边三分之一，右边一大片空白（2026-11-13 用户两次反馈的那个
    // 「所有 menu 都没有自适应宽度」就是它）。菜单栏 / 右键 / 面板齿轮都已改成
    // 按内容显式设宽（`shell::rows_width`），这里再把默认值收到 Qt 菜单的
    // 量级，兜住仍走 `ui.button` 的少数弹层（会话树 Move/Import、标签配色）。
    style.spacing.menu_width = 160.0;
    style.spacing.icon_width = 14.0;
    style.spacing.icon_spacing = 6.0;
    style.spacing.indent = 14.0;
    ctx.set_style(style);
}

//! 菜单栏右侧的四件工具：搜索 / `Tunnels` / `Focus Mode` / 布局下拉。
//!
//! # 为什么自己画，而不是排按钮
//!
//! 视觉基线里这四件**没有外框、没有底色**（只有最右那个布局按钮有底），
//! 图标是 9.5×9.5 的纯线条。用 `ui.button` 排出来的东西会带上主题的
//! `inactive` 底与描边，又变回「一排方块」——正是菜单项那一轮修掉的毛病。
//! 所以这里和 `session_icons.rs` / `quick_mark.rs` 同一原则：**能画就不排字**，
//! 图标全部走绘图原语，不依赖字体里有没有那个字形。
//!
//! # 几何依据
//!
//! 全部来自视觉基线逐像素测量（2× 图，除 2 后为逻辑点）：
//!
//! | 元素 | 视觉基线 | 逻辑点 |
//! |---|---|---|
//! | 图标方框 | 19×19 px | 9.5 |
//! | 图标→文字 | 13–14 px | 6.5 |
//! | 组→组 | 33–34 px | 16.5 |
//! | `Mode`→布局按钮 | 21 px | 10.5 |
//! | 布局按钮 | 44×46 px | 22×23 |
//! | 右边缘留白 | 9 px | 4.5 |

use super::menu_style;
use crate::ui::*;
use term_core::theme::CHROME_SCALE;

/// 点按区高度＝菜单栏高，这样悬停底能顶满一条。
const TOOL_H: f32 = 32.0;
/// 图标方框边长（视觉基线 19 px / 2）。
const ICON: f32 = 9.5;
/// 图标与文字的间隙（视觉基线 13–14 px / 2）。
const ICON_GAP: f32 = 6.5;
/// 两组之间的间隙（视觉基线 33–34 px / 2）。
const GROUP_GAP: f32 = 16.5;
/// 布局按钮的宽高（视觉基线 44×46 px / 2）。
const BUTTON_W: f32 = 22.0;
const BUTTON_H: f32 = 23.0;
/// `Mode` 文字到布局按钮（视觉基线 21 px / 2）。
const BUTTON_GAP: f32 = 10.5;
/// 悬停底向外扩的量：视觉基线没有可测的悬停态，取 4 px 让底色不贴着字形。
const HOVER_PAD: f32 = 4.0;

/// 画右侧四件。调用点已用 `horizontal_centered` 给出居中的一行，
/// 这里再套一层右对齐，让它们从**内容区右缘**往左排。
pub(super) fn draw(win: &mut TerminalWindow, ui: &mut egui::Ui) {
    let font = egui::TextStyle::Button.resolve(ui.style());

    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
        // 间隙全部显式给：组间距与图标间距不是一个值，靠 `item_spacing`
        // 一个数表达不了。
        ui.spacing_mut().item_spacing.x = 0.0;

        layout_button(win, ui);
        // 表是**从左到右**的视觉顺序，右对齐摆放要反过来走。
        for (i, t) in BAR_TOOLS.iter().rev().enumerate() {
            ui.add_space(if i == 0 { BUTTON_GAP } else { GROUP_GAP });
            let paint = match t.glyph {
                BarGlyph::Search => paint_search,
                BarGlyph::Tunnels => paint_branch,
                BarGlyph::Focus => paint_focus,
            };
            if t.label.is_empty() {
                icon_only(win, ui, t.id, paint);
            } else {
                let label = win.tr(t.label);
                // 2026-11-17：`Tunnels` 不再是「点了就开面板」的按钮，而是一个
                // **下拉**（视觉基线 `clipboard-…-577160c5.png`）：第一行是隧道
                // 总线开关，第二行进 Tunnels 设置。其余三件仍是单点按钮。
                if matches!(t.glyph, BarGlyph::Tunnels) {
                    tunnels_tool(win, ui, t.id, label.to_owned(), paint, &font);
                } else {
                    tool(win, ui, t.id, label, paint, &font);
                }
            }
        }
    });
}

/// `Tunnels` 那一件：**按钮 + 下拉**（视觉基线两行）。
///
/// 行结构与菜单栏弹层同一套 `menu_style::draw_menu_row`——第一行右端挂一颗
/// 胶囊开关（`paint_switch`），第二行是普通命令行。开关与文字同属一行：
/// 点整行都算切换，开关只是那个事实的可见形态。
fn tunnels_tool(
    win: &mut TerminalWindow,
    ui: &mut egui::Ui,
    id: &'static str,
    label: String,
    paint: fn(&egui::Painter, egui::Rect, egui::Color32),
    font: &egui::FontId,
) {
    let icon = crate::theme::chrome(CHROME_SCALE.icon);
    let text = crate::theme::chrome(CHROME_SCALE.text);
    let galley = ui
        .painter()
        .layout_no_wrap(label.clone(), font.clone(), text);
    let w = ICON + ICON_GAP + galley.size().x;
    let button = egui::Button::new("")
        .min_size(egui::vec2(w, TOOL_H))
        .frame(false);
    let inner = egui::menu::menu_custom_button(ui, button, |ui| tunnels_menu_body(win, ui));

    let rect = inner.response.rect;
    let p = ui.painter();
    // 下拉打开时按钮带一圈框（视觉基线里那颗按钮就是带框的）——那正是
    // 「这一件展开了」的状态，不靠猜。
    if inner.response.has_focus() || ui.memory(|m| m.is_popup_open(inner.response.id)) {
        p.rect_stroke(
            rect.shrink(0.5),
            2.0,
            egui::Stroke::new(1.0, crate::theme::chrome(CHROME_SCALE.border)),
        );
    } else {
        paint_hover(p, &inner.response, rect);
    }
    paint(p, left_icon_rect(rect), icon);
    p.galley(
        egui::pos2(
            rect.left() + ICON + ICON_GAP,
            rect.center().y - galley.size().y / 2.0,
        ),
        galley,
        text,
    );
    // 夹具入口（不入库）：静态图没有鼠标，`menu_custom_button` 的弹层拍不到。
    // `WINDTERM_TUNNELS_DEMO=1` 把**同一份内容**摆到按钮正下方，供
    // `--demo dock --screenshot` 与视觉基线逐行比对。默认不设 = 生产路径零变化。
    if std::env::var("WINDTERM_TUNNELS_DEMO").is_ok() {
        egui::Area::new(egui::Id::new("tunnels_dropdown_demo"))
            .order(egui::Order::Foreground)
            .fixed_pos(rect.left_bottom() + egui::vec2(1.0, 1.0))
            .fade_in(false)
            .show(ui.ctx(), |ui| {
                egui::Frame::menu(ui.style()).show(ui, |ui| tunnels_menu_body(win, ui));
            });
    }
    let _ = id;
}

/// `Tunnels` 下拉的内容：`T̲unnels` + 胶囊开关、`Tunnel S̲ettings`。
///
/// # 为什么自绘这两行
///
/// 试过 `menu_style::draw_menu_row`（菜单栏弹层那一套），两个死结：
/// 1. 它的行宽取 `available_width`，而 `Frame::menu` 的宽度由**内容**决定——
///    `set_width` 撑不住（内容窄就缩回去），`set_min_width` 又会把整条弹层
///    拉成横贯窗口的一堵墙；
/// 2. 视觉基线的行高是 28，那套是 31。
///
/// 所以按实测坐标自绘：行高 28、文字距左 11、开关右端距右 11.5、
/// 弹层外宽 165.5。视觉上仍与菜单行同一族（同一底色、同一悬停色、同一字体）。
/// 组件夹具专用：把 Tunnels 工具按钮下拉直接摆在画布上（生产仍走 `tunnels_tool`）。
pub(in crate::ui) fn draw_tunnels_popup_demo(win: &mut TerminalWindow, ctx: &egui::Context) {
    egui::Area::new(egui::Id::new("comp_tunnels_popup"))
        .fixed_pos(egui::pos2(4.0, 4.0))
        .fade_in(false)
        .order(egui::Order::Foreground)
        .show(ctx, |ui| {
            let margin = ui.spacing().menu_margin;
            // 按钮（原图左上那颗带边框的 Tunnels）与下方两行弹层。
            let btn = egui::Rect::from_min_size(
                egui::pos2(4.0, 4.0),
                egui::vec2(130.0, 26.0),
            );
            let p = ui.painter();
            p.rect_filled(btn, egui::Rounding::same(4.0), crate::theme::chrome(term_core::theme::CHROME_SCALE.hover));
            p.rect_stroke(btn, egui::Rounding::same(4.0), egui::Stroke::new(1.0, crate::theme::chrome(term_core::theme::CHROME_SCALE.border)));
            p.text(btn.center(), egui::Align2::CENTER_CENTER, win.tr("Tunnels"), egui::TextStyle::Button.resolve(ui.style()), crate::theme::chrome(term_core::theme::CHROME_SCALE.text));
            ui.add_space(30.0);
            egui::Frame::none()
                .fill(crate::ui::shell::popup_bg())
                .stroke(egui::Stroke::new(1.0, crate::theme::chrome(term_core::theme::CHROME_SCALE.border)))
                .inner_margin(margin)
                .show(ui, |ui| {
                    tunnels_menu_body(win, ui);
                });
        });
}

fn tunnels_menu_body(win: &mut TerminalWindow, ui: &mut egui::Ui) {
    /// 弹层外宽（物理 331 → 逻辑 165.5）。
    const MENU_W: f32 = 165.5;
    /// 行高（物理 56）。
    const ROW_H: f32 = 28.0;
    /// 文字距弹层**外**左边。
    const PAD_L: f32 = 11.0;
    /// 开关右端距弹层**外**右边。
    const PAD_R: f32 = 11.5;
    // `Frame::menu` 每边各有一圈 margin，而 `rect` 是内区矩形——
    // 视觉基线那两个数是相对外框量的，所以这里先把 margin 扣掉。
    let m = ui.spacing().menu_margin.left;
    let inner_w = (MENU_W - 2.0 * m).max(120.0);
    ui.spacing_mut().item_spacing = egui::vec2(0.0, 0.0);

    let lang = win.lang();
    let on = win.tunnels_enabled;
    let font = egui::TextStyle::Button.resolve(ui.style());
    // 两行：(文案, 访问字母下标, 是不是那条带开关的行, 命令 ID)。
    // **名单在数据层**（`TUNNEL_ITEMS`）：命令 ID 出现在 `ui/` 里就算硬编码，
    // 这里只负责把表里的四项搬上手绘的行。
    let rows: Vec<(&str, usize, bool, &'static str)> = term_app::menu::TUNNEL_ITEMS
        .iter()
        .map(|it| (crate::ui::t(lang, it.label), it.access, it.switch, it.id))
        .collect();
    let text_c = crate::theme::chrome(CHROME_SCALE.text);
    let mut fire: Option<&'static str> = None;
    for (label, acc, has_switch, id) in rows {
        let (rect, resp) = ui.allocate_exact_size(egui::vec2(inner_w, ROW_H), egui::Sense::click());
        let p = ui.painter();
        if resp.hovered() {
            // 悬停 = 整行蓝底（与菜单栏弹层同一个指向色）。
            p.rect_filled(rect, egui::Rounding::ZERO, crate::theme::chrome(CHROME_SCALE.hover));
        }
        let galley = p.layout_no_wrap(label.to_owned(), font.clone(), text_c);
        let tx = rect.left() + (PAD_L - m).max(2.0);
        let ty = rect.center().y - galley.size().y / 2.0;
        p.galley(egui::pos2(tx, ty), galley.clone(), text_c);
        // 访问字母下划线：取那个字形的起止 x（菜单栏那一套也是这么算的）。
        if let Some(glyph) = galley.rows.first().and_then(|r| r.glyphs.get(acc)) {
            let y = ty + galley.size().y - 2.0;
            p.line_segment(
                [
                    egui::pos2(tx + glyph.pos.x, y),
                    egui::pos2(tx + glyph.pos.x + glyph.advance_width, y),
                ],
                egui::Stroke::new(1.0, text_c),
            );
        }
        if has_switch {
            let sw = egui::Rect::from_center_size(
                egui::pos2(
                    rect.right() - (PAD_R - m).max(2.0) - menu_style::SWITCH_SIZE.x / 2.0,
                    rect.center().y,
                ),
                menu_style::SWITCH_SIZE,
            );
            menu_style::paint_switch(p, sw, on);
        }
        if resp.clicked() {
            fire = Some(id);
            if !has_switch {
                ui.close_menu();
            }
        }
    }
    if let Some(id) = fire {
        win.dispatch(id);
    }
}

/// 「图标 + 文字」一件。真正接线：点下去走 `dispatch`，和菜单项同一条路径。
fn tool(
    win: &mut TerminalWindow,
    ui: &mut egui::Ui,
    id: &'static str,
    label: &str,
    paint: fn(&egui::Painter, egui::Rect, egui::Color32),
    font: &egui::FontId,
) {
    let icon = crate::theme::chrome(CHROME_SCALE.icon);
    let text = crate::theme::chrome(CHROME_SCALE.text);
    let galley = ui
        .painter()
        .layout_no_wrap(label.to_owned(), font.clone(), text);
    let w = ICON + ICON_GAP + galley.size().x;
    let (rect, resp) = ui.allocate_exact_size(egui::vec2(w, TOOL_H), egui::Sense::click());

    let p = ui.painter();
    paint_hover(p, &resp, rect);
    paint(p, left_icon_rect(rect), icon);
    p.galley(
        egui::pos2(
            rect.left() + ICON + ICON_GAP,
            rect.center().y - galley.size().y / 2.0,
        ),
        galley,
        text,
    );

    if resp.clicked() {
        win.dispatch(id);
    }
}

/// 只有图标的一件（搜索）。
fn icon_only(
    win: &mut TerminalWindow,
    ui: &mut egui::Ui,
    id: &'static str,
    paint: fn(&egui::Painter, egui::Rect, egui::Color32),
) {
    let (rect, resp) = ui.allocate_exact_size(egui::vec2(ICON, TOOL_H), egui::Sense::click());
    let p = ui.painter();
    paint_hover(p, &resp, rect);
    // 点按区是整条高，图标始终贴同一中线——不能跟着「这一格多高」走。
    paint(p, icon_rect(rect), crate::theme::chrome(CHROME_SCALE.icon));
    if resp.clicked() {
        win.dispatch(id);
    }
}

fn icon_rect(rect: egui::Rect) -> egui::Rect {
    egui::Rect::from_center_size(rect.center(), egui::vec2(ICON, ICON))
}

/// 「图标在左、文字在右」的那一件里的图标位置。
///
/// 不能复用 [`icon_rect`]：那一格宽 = 图标 + 间隙 + 文字，按中心摆会把图标
/// 压到文字上（第一次出图就是这个下场——`Tunnels` 的 `n` 上叠着一个分叉）。
fn left_icon_rect(rect: egui::Rect) -> egui::Rect {
    egui::Rect::from_min_size(
        egui::pos2(rect.left(), rect.center().y - ICON / 2.0),
        egui::vec2(ICON, ICON),
    )
}

fn paint_hover(p: &egui::Painter, resp: &egui::Response, rect: egui::Rect) {
    if resp.hovered() {
        let bg = crate::theme::chrome(CHROME_SCALE.hover);
        p.rect_filled(rect.expand2(egui::vec2(HOVER_PAD, 0.0)), 2.0, bg);
    }
}

// ── 布局下拉 ──────────────────────────────────────────────────────────────

/// 最右那个带底的按钮。**用 `menu_custom_button` 借它的开合与定位**，
/// 按钮本身画成空的（`frame(false)` + 无文字），底色与图标全由这里画。
///
/// 展开的三项直接对应 `Window.Toggle{Left,Right,Bottom}Docks`——
/// 菜单 `Window › Panes` 里是同一个命令的另一个入口，不是第二套实现。
fn layout_button(win: &mut TerminalWindow, ui: &mut egui::Ui) {
    let button = egui::Button::new("")
        .min_size(egui::vec2(BUTTON_W, TOOL_H))
        .frame(false);
    let inner = egui::menu::menu_custom_button(ui, button, |ui| layout_menu_body(win, ui));

    let rect = egui::Rect::from_center_size(
        inner.response.rect.center(),
        egui::vec2(BUTTON_W, BUTTON_H),
    );
    // 夹具入口（不入库）：静态图没有鼠标，`menu_custom_button` 的弹层永远拍不到。
    // `WINDTERM_LAYOUT_DEMO=1` 把**同一份内容**摆到按钮正下方，供
    // `--demo dock --screenshot` 核对「下拉里到底有几条带」。
    // 只加一个渲染分支，不改按钮本身、不改生产路径的默认像素。
    if std::env::var("WINDTERM_LAYOUT_DEMO").is_ok() {
        // 贴右上角锚定（真实弹层在窗口右边缘也是这样往左收），
        // 免得夹具图里被窗口右边界切掉一截、看起来像少了一列。
        egui::Area::new(egui::Id::new("layout_dropdown_demo"))
            .order(egui::Order::Foreground)
            .anchor(
                egui::Align2::RIGHT_TOP,
                egui::vec2(rect.right() - ui.ctx().screen_rect().right(), rect.bottom() + 2.0),
            )
            .fade_in(false)
            .show(ui.ctx(), |ui| {
                egui::Frame::menu(ui.style()).show(ui, |ui| layout_menu_body(win, ui));
            });
    }

    let p = ui.painter();
    // 立体底色与「工具行」那颗共用一份实现（`shell::bevel`）：视觉基线里两处的
    // 逐行采样值完全相同，分开写迟早会走偏。
    super::bevel(p, rect);
    paint_layout(p, icon_rect(rect), crate::theme::chrome(CHROME_SCALE.text_strong));
}

/// 布局下拉的**内容**：三条带各一行勾选。
///
/// 抽成独立函数是给出图夹具用的——菜单栏按钮与夹具必须画同一份，
/// 否则「夹具里三条、真点开两条」照样看不见。
fn layout_menu_body(win: &mut TerminalWindow, ui: &mut egui::Ui) {
    // 弹层里的每一行与菜单栏弹层**同一套画法**：左侧 11 点方框（框内挖暗、
    // 细勾）、整行蓝底表示指向、文字落在同一条竖线上。
    //
    // 2026-11-17 用户点名的「菜单里的复选框长得不一样」：这里原先用
    // `ui.checkbox`，画的是 egui 自带形状，边长、描边、勾形都跟菜单栏弹层
    // 量的那一套不同，同一屏里两种框。现在改走 `kit::checkbox`。
    // 注意**只统一菜单/弹层**——对话框正文与关闭确认弹窗保持原画法。
    //
    // 必须显式设宽：`menu_custom_button` 的弹层是 `Area`，egui 拿
    // `spacing.menu_width`（默认 400）当默认宽，而 `draw_menu_row` 按
    // `available_width` 铺满整行——不设宽就是一堵 400 宽的灰墙。
    let cells = DOCK_ITEMS
        .iter()
        .map(|i| menu_style::text_cells(i.label))
        .max()
        .unwrap_or(0);
    ui.set_width(menu_style::rows_width(ui, cells, menu_style::CHECK_GUTTER));
    for item in DOCK_ITEMS {
        let look = menu_style::RowLook {
            state: menu_style::MenuRowState::Enabled,
            check: menu_style::CheckCell::Box(dock_on(win, item)),
            indent: true,
            cells,
        };
        if menu_style::draw_menu_row(ui, item.label, None, "", look, false).clicked() {
            win.dispatch(item.id);
        }
    }
}

/// 这一带的勾选状态。与 `term_app::shell::ToggleDocks` 的同名判定保持一致：
/// 只要带里有一块开着，勾就是亮的（点击 = 收起整带）。
///
/// **归属表只有一份**：`PanelId::default_zone()`。以前这里手抄了一遍
/// 「Left 看 `show_sidebar`，其余七个字段算右带」——底带一进下拉，
/// `_` 就会把 Sender / Shell / Script 认成右带，两边的勾互相打架，而且
/// 编译期不会有任何反馈。现在直接问同一张表。
fn dock_on(win: &TerminalWindow, item: &DockItem) -> bool {
    term_app::vocab::PanelId::ALL.into_iter().any(|p| {
        p.default_zone() == item.zone && term_app::shell::panel_visible(&win.view, p)
    })
}

// ── 图标（全部几何绘制） ──────────────────────────────────────────────────

/// 放大镜：圆 + 45° 手柄。
fn paint_search(p: &egui::Painter, r: egui::Rect, c: egui::Color32) {
    let st = egui::Stroke::new(1.1, c);
    let ctr = egui::pos2(r.left() + 0.38 * r.width(), r.top() + 0.36 * r.height());
    p.circle_stroke(ctr, 0.29 * r.width(), st);
    p.line_segment(
        [
            egui::pos2(r.left() + 0.60 * r.width(), r.top() + 0.58 * r.height()),
            egui::pos2(r.right() - 0.06 * r.width(), r.bottom() - 0.06 * r.height()),
        ],
        st,
    );
}

/// `Tunnels`：一根竖干上分叉出两条，三个端点各一个小方块。
///
/// 视觉基线按 12× 放大确认：左下节点 → 竖干上行 → 顶部横支与中部横支各接一个
/// 右侧节点。就是「一个转发端口对应两条隧道」的意思。
fn paint_branch(p: &egui::Painter, r: egui::Rect, c: egui::Color32) {
    // 视觉基线的墨迹只占方框宽的 0.84（16/19），不铺满——铺满会让这一件比
    // 左右两件都「重」，组间距看起来忽宽忽窄。
    let r = r.shrink2(egui::vec2(0.08 * r.width(), 0.02 * r.height()));
    let st = egui::Stroke::new(1.2, c);
    let x = r.left() + 0.20 * r.width();
    let top = r.top() + 0.14 * r.height();
    let mid = r.top() + 0.48 * r.height();
    let bot = r.bottom() - 0.16 * r.height();
    let right = r.right() - 0.22 * r.width();

    p.line_segment([egui::pos2(x, bot), egui::pos2(x, top)], st);
    p.line_segment([egui::pos2(x, top), egui::pos2(right, top)], st);
    p.line_segment([egui::pos2(x, mid), egui::pos2(right, mid)], st);

    for node in [egui::pos2(x, bot), egui::pos2(right, top), egui::pos2(right, mid)] {
        p.rect_stroke(
            egui::Rect::from_center_size(node, egui::vec2(0.22 * r.width(), 0.22 * r.width())),
            0.6,
            egui::Stroke::new(1.0, c),
        );
    }
}

/// `Focus Mode`：四角直角括号 + 中央方块（取景框）。
fn paint_focus(p: &egui::Painter, r: egui::Rect, c: egui::Color32) {
    let st = egui::Stroke::new(1.1, c);
    let arm = 0.28 * r.width();

    for (cx, cy, dx, dy) in [
        (r.left(), r.top(), 1.0, 1.0),
        (r.right(), r.top(), -1.0, 1.0),
        (r.left(), r.bottom(), 1.0, -1.0),
        (r.right(), r.bottom(), -1.0, -1.0),
    ] {
        let corner = egui::pos2(cx, cy);
        p.line_segment([corner, egui::pos2(cx + dx * arm, cy)], st);
        p.line_segment([corner, egui::pos2(cx, cy + dy * arm)], st);
    }

    p.rect_stroke(
        egui::Rect::from_center_size(r.center(), egui::vec2(0.44 * r.width(), 0.44 * r.height())),
        0.0,
        egui::Stroke::new(1.1, c),
    );
}

/// 布局：矩形 + 靠近顶部的一条横线（窗口带标题条）。
fn paint_layout(p: &egui::Painter, r: egui::Rect, c: egui::Color32) {
    // 描边是**骑在边线上**的，不内缩会画出 9.5+1.2 的墨迹，比参考的 9.5 胖一圈。
    let r = r.shrink(0.6);
    let st = egui::Stroke::new(1.2, c);
    p.rect_stroke(r, 0.5, st);
    let y = r.top() + 0.28 * r.height();
    p.line_segment(
        [
            egui::pos2(r.left() + 0.5, y),
            egui::pos2(r.right() - 0.5, y),
        ],
        st,
    );
}

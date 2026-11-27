//! Find 栏只读 `FindState`，输入经 `Action` 派发（已是受控组件）。
//!
//! # 2026-11-17 重做（按用户两张视觉基线逐像素对齐）
//!
//! 视觉基线实测（@2× 截图 ÷2 = 逻辑点，窗口逻辑宽 1273）：
//!
//! ```text
//! y  26.0   ── 1 点 #444444 上边线
//! y  36.0   输入框上沿（框高 23.5，底 59.5）
//! y  69.0   ── 1 点 #444444 下边线            → 整条高 43
//! 横坐标：Find: 11 起 | 输入框 52..1004.5 | < 1012..1090 | > 1093..1170.5
//!         | Find All 1179..1264.5 | × 1276..1282.5（6.5 见方）| 窗口右端 ≈1290（右距 7.5）
//!
//! **右端带一个无边框的关闭 `×`**。第一次量的时候漏了它：空态那张视觉基线被裁到
//! `Find All` 右沿 +8.5 就结束了，我把「图的右边界」当成了「窗口右边界」，
//! 于是算出的右距 8.5 其实是 `×` 该占的位置。命中态那张没裁到那里，一比就露馅。
//! 输入框内右侧图标组（高 19.5，框内上下各缩 2）：
//!         .* 815..842 | Aa 842..869 | W 869..896 | ≡ 896..923 | I 923..950
//!         | 空隙 5 | ⇄▾ 955..998 | 框内右距 6.5
//! ```
//!
//! **为什么整条手绘**：这里的控件之间只有 3~8.5 点的间距，还要在输入框**内部**
//! 右端塞一排图标——`ui.horizontal` 的 `item_spacing` 是一套全局值，改它会影响
//! 同一帧别处；`ui.put` 只解决控件落位，边框/内边距仍是 egui 默认（4 点 padding、
//! 圆角、悬停描边），与视觉基线对不上。所以除了真正需要输入法的那一个
//! `TextEdit`（`frame(false)`，边框手画），其余全部按坐标画。
//!
//! # 三条纪律没变
//!
//! - **状态只读**：`find_state` 是 `&FindState`，写入口只有 `dispatch`；
//! - **计数不存**：总数由调用方按 `(query, 窗口)` 算好传进来，本文件不缓存；
//! - **受控输入**：编辑写 `input` 缓冲，改动再以 `SetQuery` 派发（`edit_action`）。
//!
//! # 剩余三个图标 + Find All 的接线
//!
//! - `Find All` 展开一个 `egui::Area` 结果列表：数据由 `app::find::result_rows`
//!   每帧从事实窗口现算，点条目用已有的 `Next`/`Previous` 走到那一项；
//! - `⇄▾` 三态（前 / 后 / 全部）真改写 `FindState.direction`，`<`/`>`/`Enter`
//!   经 `app::find::nav_action` 改变派发方向；`All` 还展开结果列表；
//! - `≡` 仅在正则模式下生效，翻 `multi_line`（`.` 跨行 + `^/$` 逐行锚定），
//!   字面模式下明确禁用并在悬停里写明原因，不画假开关；
//! - `I` 保持禁用：本仓终端没有插入 / 编辑光标态，悬停照实说明。
//!
//! 三个新开关不新增命令 ID / `FindAction` 变体：它们经 `SetQuery` 的保留
//! 前缀以「动作参数」形式进入同一个 reducer（见 `app::find::ui_command_action`）。
//! 默认态一个像素都不变——视觉基线里的亮图标仍是亮的，只有真正切换后才有反馈。
//!
//! `.*`（正则）已接线：`term-core::find` 用 `regex` 编译表达式，字符列与
//! 字形坐标同源；语法错时图标和输入框边框转成红色，悬停给出编译错误，
//! 而不是让「0 / N」静默地像搜不到。

use crate::ui::tr::t;
use eframe::egui;
use term_app::action::Action;
use term_app::find::{
    current_index, jump_plan, nav_action, ui_command_action, FindAction, FindResults,
    FindUiCommand,
};
use term_app::i18n::Lang;
use term_app::state::{FindDirection, FindState};
use term_core::theme::CHROME_SCALE;

/// 查找栏在 `CentralPanel` 之前绘制，取的是上一帧由终端叠层写入的结果快照。
/// `ui/terminal/find_results_slot.rs` 在读取时同时校验活动标签身份与 query，
/// 分屏里别的叶子 / 旧查询都不会冒充当前答案。

use super::glyph;

/// 整条高度（含上下各 1 点边线）。视觉基线 y 26..69。
pub const FIND_BAR_H: f32 = 43.0;
/// `Find:` 标签距窗口左端。过滤行复用同一横坐标。
pub(in crate::ui) const LABEL_X: f32 = 11.0;
/// 输入框左沿。过滤行复用同一横坐标。
pub(in crate::ui) const FIELD_X: f32 = 52.0;
/// 输入框高。视觉基线 y 36..59.5。
pub(in crate::ui) const FIELD_H: f32 = 23.5;
/// 输入框上沿距栏顶（不含那条 1 点边线）。
pub(in crate::ui) const FIELD_TOP: f32 = 10.0;
/// 关闭 `×` 右沿距窗口右端。
///
/// 与 `glyph::ICON_R_PAD` 是同一个数：快捷栏、面板标题带用的也是它，这样几条
/// 栏右端的 `×` 才落在**同一条竖线**上（视觉基线 `ref-find-active.png` 里 Find
/// 行与快捷栏的 `×` 墨迹列完全相同）。这里的常量保留成显式值，是因为下面的
/// 横坐标累加式要它参与求和，直接引用 `glyph::` 反而看不出那笔账。
pub(in crate::ui) const CLOSE_R_PAD: f32 = glyph::ICON_R_PAD;
/// `×` 的**视觉边长**（@2× 实测 13×13 物理 = 6.5 逻辑）。
///
/// 两条对角线本身是 `CLOSE_SIZE - 1`：1 点线宽在两端各外扩 0.5，加回去才是
/// 屏幕上量到的那个方块。写 6.5 却按 6.5 画对角线，量出来会是 7.5——
/// 第一版就是这么比视觉基线粗了一整圈。画法本身在 `glyph::paint_cross`。
pub(in crate::ui) const CLOSE_SIZE: f32 = glyph::CROSS_INK;
/// `×` 左沿与 `Find All` 右沿之间的空隙。
pub(in crate::ui) const CLOSE_GAP: f32 = 11.5;
/// `<` / `>` 宽（视觉基线 78 与 77.5，取同宽）。
const NAV_W: f32 = 77.5;
/// `<` 与 `>` 之间的空隙。
const NAV_GAP: f32 = 3.0;
/// `Find All` 宽。
const ALL_W: f32 = 85.5;
/// `>` 与 `Find All` 之间的空隙。
const BEFORE_ALL_GAP: f32 = 8.5;
/// 输入框右沿到 `<` 左沿的空隙。
const FIELD_TO_NAV: f32 = 7.5;
/// 输入框内图标格宽（`.*` / `Aa` / `W` / `≡` / `I` 五格等宽）。过滤行的图标格复用它。
pub(in crate::ui) const ICON_W: f32 = 27.0;
/// `⇄▾` 那一格的宽（图标 + 下拉箭头，视觉基线 43）。
const CYCLE_W: f32 = 43.0;
/// `I` 与 `⇄▾` 之间的空隙。
const CYCLE_GAP: f32 = 5.0;
/// 图标组右端距输入框内右沿。过滤行的框内图标组复用它。
pub(in crate::ui) const ICON_R_PAD: f32 = 6.5;

/// 正则语法错的红色。与主题灰阶无关，是错误语义色；Find 栏与过滤行共用这一份，
/// 两处的 `.*` 图标 / 输入框边框在非法表达式下必须是同一个红。
pub(in crate::ui) const INVALID_COLOR: egui::Color32 = egui::Color32::from_rgb(0xE0, 0x6C, 0x6C);

/// 画 Find 栏。栏内交互统一以 `Action` 出队，由调用方交给 reducer。
///
/// - `find_state`：只读。`query` 是**提交后**的查询，不是正在敲的字。
/// - `input`：这一帧交给 `TextEdit` 的编辑缓冲（投影层持有，跨帧存活）。
/// - `total`：当前事实窗口内的匹配数——只读，写不回去（本文件不持有它）。
pub fn draw_find_bar(
    ui: &mut egui::Ui,
    find_state: &FindState,
    input: &mut String,
    total: usize,
    lang: Lang,
    dispatch: &mut dyn FnMut(Action),
) {
    if !find_state.visible {
        return;
    }
    // 外部改了查询（Open / Close / 菜单派发）才覆盖缓冲；用户正在敲的字
    // 已经在缓冲里，覆盖它会把光标和输入法预编辑一起打断。
    sync_input(input, &find_state.query);

    let bar = ui.max_rect();
    let full_w = bar.width();
    let top = bar.top() + 1.0; // 让开那条 1 点上边线
    let line = crate::theme::chrome(CHROME_SCALE.border);
    let text_c = crate::theme::chrome(CHROME_SCALE.text);
    let font = egui::TextStyle::Button.resolve(ui.style());
    // 正则语法错的**可见**反馈：边框 + `.*` 图标同时转红，悬停给出原因。
    // 编译只在查询/正则位变化时随这一帧的绘制做一次，结果复用给提示文本。
    let regex_error = if find_state.regex && !find_state.query.is_empty() {
        term_core::find::validate_regex(&find_state.query).err()
    } else {
        None
    };
    let invalid_c = INVALID_COLOR;
    let border_c = if regex_error.is_some() { invalid_c } else { line };

    // 上下各一条 1 点边线：视觉基线里它与快捷栏的分界就靠这两条线。
    ui.painter().line_segment(
        [
            egui::pos2(bar.left(), bar.top() + 0.5),
            egui::pos2(bar.right(), bar.top() + 0.5),
        ],
        egui::Stroke::new(1.0, line),
    );
    ui.painter().line_segment(
        [
            egui::pos2(bar.left(), bar.bottom() - 0.5),
            egui::pos2(bar.right(), bar.bottom() - 0.5),
        ],
        egui::Stroke::new(1.0, line),
    );

    // 右端「× + 三个按钮」的位置（从右往左算，才不会被窗口宽度牵着走）。
    let all_r = egui::Rect::from_min_size(
        egui::pos2(
            bar.left() + full_w - CLOSE_R_PAD - CLOSE_SIZE - CLOSE_GAP - ALL_W,
            top + FIELD_TOP,
        ),
        egui::vec2(ALL_W, FIELD_H),
    );
    let gt_r = egui::Rect::from_min_size(
        egui::pos2(all_r.left() - BEFORE_ALL_GAP - NAV_W, all_r.top()),
        egui::vec2(NAV_W, FIELD_H),
    );
    let lt_r = egui::Rect::from_min_size(
        egui::pos2(gt_r.left() - NAV_GAP - NAV_W, all_r.top()),
        egui::vec2(NAV_W, FIELD_H),
    );
    let field = egui::Rect::from_min_max(
        egui::pos2(bar.left() + FIELD_X, top + FIELD_TOP),
        egui::pos2(lt_r.left() - FIELD_TO_NAV, top + FIELD_TOP + FIELD_H),
    );

    // 标签 `Find:`——标签自带下划线（视觉基线 `F̲ind:`）。
    let galley = ui
        .painter()
        .layout_no_wrap(t(lang, "Find:").to_owned(), font.clone(), text_c);
    let ly = field.center().y - galley.size().y / 2.0;
    ui.painter().galley(
        egui::pos2(bar.left() + LABEL_X, ly),
        galley.clone(),
        text_c,
    );
    let fy = ly + galley.size().y - 1.0;
    if let Some(g) = galley.rows.first().and_then(|r| r.glyphs.first()) {
        let x0 = bar.left() + LABEL_X + g.pos.x;
        ui.painter().line_segment(
            [egui::pos2(x0, fy), egui::pos2(x0 + g.advance_width, fy)],
            egui::Stroke::new(1.0, text_c),
        );
    }

    // 输入框：只有边框要画，底色与栏底同色（视觉基线实测 #1C1C1C 两侧一致）。
    // `shrink(0.5)`：1 点描边画在整数边界上会被抗锯齿摊成两行半亮的线，
    // 与视觉基线那种干脆的一像素不同（Tunnels 对话框那轮踩过一次）。
    ui.painter()
        .rect_stroke(field.shrink(0.5), 0.0, egui::Stroke::new(1.0, border_c));

    // 文本编辑区：左侧留 6 点内边距，右侧让开图标组。
    let icons_left = field.right() - ICON_R_PAD - CYCLE_W - CYCLE_GAP - ICON_W * 5.0;
    let edit_rect = egui::Rect::from_min_max(
        egui::pos2(field.left() + 6.0, field.top()),
        egui::pos2(icons_left - 4.0, field.bottom()),
    );
    ui.scope_builder(egui::UiBuilder::new().max_rect(edit_rect), |ui| {
        let response = ui.add(
            egui::TextEdit::singleline(input)
                .frame(false)
                .desired_width(edit_rect.width()),
        );
        if response.changed() {
            dispatch(edit_action(input));
        }
        if response.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
            dispatch(Action::Find(nav_action(find_state, true)));
        }
        if response.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Escape)) {
            dispatch(Action::Find(FindAction::Close));
        }
    });

    // 输入框内右侧的图标组：五格等宽 + 一格里装「图标 + 下拉箭头」。
    let mut x = icons_left;
    let active_bg = crate::theme::chrome(CHROME_SCALE.tab);
    let hover_bg = crate::theme::chrome(CHROME_SCALE.hover);
    let icon_rect = |x: f32, w: f32| {
        egui::Rect::from_min_size(
            egui::pos2(x, field.top() + 2.0),
            egui::vec2(w, FIELD_H - 4.0),
        )
    };
    let step = ICON_W;

    // `.*` 正则（`ToggleRegex`）——与 Aa / W 同构：激活时铺底色，点击派发。
    let r = icon_rect(x, ICON_W);
    let resp = ui.interact(r, egui::Id::new("find_icon_regex"), egui::Sense::click());
    if find_state.regex {
        ui.painter().rect_filled(r, 0.0, active_bg);
    } else if resp.hovered() {
        ui.painter().rect_filled(r, 0.0, hover_bg);
    }
    let icon_c = if regex_error.is_some() { invalid_c } else { text_c };
    let g = ui
        .painter()
        .layout_no_wrap(".*".to_owned(), font.clone(), icon_c);
    ui.painter()
        .galley(r.center() - g.size() / 2.0, g, icon_c);
    // 表达式写错时给出明确提示——匹配的 0/N 静默失败最容易让人以为
    // 「搜不到」，其实是正则语法错。校验结果在上面已为边框/图标算过。
    let regex_hint = match &regex_error {
        Some(e) => format!("Regular expression — invalid pattern: {e}"),
        None if find_state.regex => "Regular expression (on)".to_string(),
        None => "Regular expression".to_string(),
    };
    if resp.on_hover_text(regex_hint).clicked() {
        dispatch(Action::Find(FindAction::ToggleRegex));
    }
    x += step;

    // `Aa` 大小写（真接线）
    let r = icon_rect(x, ICON_W);
    let resp = ui.interact(r, egui::Id::new("find_icon_case"), egui::Sense::click());
    if find_state.case_sensitive {
        ui.painter().rect_filled(r, 0.0, active_bg);
    } else if resp.hovered() {
        ui.painter().rect_filled(r, 0.0, hover_bg);
    }
    let g = ui
        .painter()
        .layout_no_wrap("Aa".to_owned(), font.clone(), text_c);
    ui.painter()
        .galley(r.center() - g.size() / 2.0, g, text_c);
    if resp
        .on_hover_text(t(lang, "Match Case"))
        .clicked()
    {
        dispatch(Action::Find(FindAction::ToggleCase));
    }
    x += step;

    // `W̲` 全词（真接线）
    let r = icon_rect(x, ICON_W);
    let resp = ui.interact(r, egui::Id::new("find_icon_word"), egui::Sense::click());
    if find_state.whole_word {
        ui.painter().rect_filled(r, 0.0, active_bg);
    } else if resp.hovered() {
        ui.painter().rect_filled(r, 0.0, hover_bg);
    }
    let g = ui
        .painter()
        .layout_no_wrap("W".to_owned(), font.clone(), text_c);
    let pos = r.center() - g.size() / 2.0;
    ui.painter().galley(pos, g.clone(), text_c);
    let uy = pos.y + g.size().y - 1.0;
    ui.painter().line_segment(
        [egui::pos2(pos.x, uy), egui::pos2(pos.x + g.size().x, uy)],
        egui::Stroke::new(1.0, text_c),
    );
    if resp.on_hover_text(t(lang, "Match Whole Word")).clicked() {
        dispatch(Action::Find(FindAction::ToggleWholeWord));
    }
    x += step;

    // `≡` 方框三条横线：正则模式下的多行标志（真开关），字面模式禁用。
    // 视觉基线里这一格不铺底，所以默认态保持原样；只有真的打开才铺 active_bg。
    let r = icon_rect(x, ICON_W);
    let multi_enabled = find_state.regex;
    let resp = ui.interact(
        r,
        egui::Id::new("find_icon_multi"),
        if multi_enabled {
            egui::Sense::click()
        } else {
            egui::Sense::hover()
        },
    );
    if find_state.multi_line {
        ui.painter().rect_filled(r, 0.0, active_bg);
    } else if resp.hovered() {
        ui.painter().rect_filled(r, 0.0, hover_bg);
    }
    let b = egui::Rect::from_center_size(r.center(), egui::vec2(13.0, 11.0));
    ui.painter()
        .rect_stroke(b, 1.0, egui::Stroke::new(1.0, text_c));
    for i in 1..4 {
        let y = b.top() + b.height() * i as f32 / 4.0;
        ui.painter().line_segment(
            [
                egui::pos2(b.left() + 2.0, y),
                egui::pos2(b.right() - 2.0, y),
            ],
            egui::Stroke::new(1.0, text_c),
        );
    }
    let multi_tip = if multi_enabled {
        format!(
            "多行匹配（{}）：`.` 可跨行，^/$ 逐行锚定",
            if find_state.multi_line { "开" } else { "关" }
        )
    } else {
        "多行匹配（禁用：仅在正则模式 .* 下有效）".to_string()
    };
    if multi_enabled {
        if resp.on_hover_text(multi_tip).clicked() {
            dispatch(ui_command_action(FindUiCommand::ToggleMultiLine));
        }
    } else {
        resp.on_hover_text(multi_tip);
    }
    x += step;

    // `I` 光标形态（未接线；视觉基线里它是亮着的，本仓无对应能力，只画形态）
    let r = icon_rect(x, ICON_W);
    let resp = ui.interact(r, egui::Id::new("find_icon_insert"), egui::Sense::click());
    // 视觉基线里这一格是**亮着**的（铺了底色）。视觉先对齐，行为仍是未接线：
    // 点了不派发任何动作，悬停会说明。宁可「亮着但说明未接线」，也不要
    // 让整条栏与视觉基线差一块底色。
    if resp.hovered() {
        ui.painter().rect_filled(r, 0.0, hover_bg);
    } else {
        ui.painter().rect_filled(r, 0.0, active_bg);
    }
    let g = ui
        .painter()
        .layout_no_wrap("I".to_owned(), font.clone(), text_c);
    ui.painter()
        .galley(r.center() - g.size() / 2.0, g, text_c);
    // 本仓终端只有「选中/未选中」与可变光标形态渲染，没有可编辑的插入态；
    // 不做假开关，悬停把原因说清楚。
    resp.on_hover_text("插入 / 编辑光标（禁用：本仓终端没有插入/编辑光标态）");
    x += step + CYCLE_GAP;

    // `⇄▾` 查找方向 / 范围（真三态开关：前 → 后 → 全部）
    let r = icon_rect(x, CYCLE_W);
    let resp = ui.interact(r, egui::Id::new("find_icon_dir"), egui::Sense::click());
    // 与 `I` 同底色：视觉基线里这两格是一块连续的高亮，与左边四格区分开。
    if resp.hovered() {
        ui.painter().rect_filled(r, 0.0, hover_bg);
    } else {
        ui.painter().rect_filled(r, 0.0, active_bg);
    }
    let c = egui::pos2(r.left() + 15.0, r.center().y);
    let rad = 4.5;
    for (dir, dy) in [(1.0_f32, -2.5_f32), (-1.0, 2.5)] {
        let mid = egui::pos2(c.x, c.y + dy);
        ui.painter().line_segment(
            [
                egui::pos2(mid.x - rad, mid.y),
                egui::pos2(mid.x + rad, mid.y),
            ],
            egui::Stroke::new(1.0, text_c),
        );
        let tip = egui::pos2(mid.x + rad * dir, mid.y);
        ui.painter().line_segment(
            [tip, tip + egui::vec2(-1.5 * dir, -2.0)],
            egui::Stroke::new(1.0, text_c),
        );
        ui.painter().line_segment(
            [tip, tip + egui::vec2(-1.5 * dir, 2.0)],
            egui::Stroke::new(1.0, text_c),
        );
    }
    let tpos = egui::pos2(r.right() - 8.0, r.center().y);
    ui.painter().add(egui::Shape::convex_polygon(
        vec![
            tpos + egui::vec2(-3.5, -1.8),
            tpos + egui::vec2(3.5, -1.8),
            tpos + egui::vec2(0.0, 2.2),
        ],
        text_c,
        egui::Stroke::NONE,
    ));
    let dir_tip = match find_state.direction {
        FindDirection::Forward => "查找方向 / 范围：向前（点击切到向后）",
        FindDirection::Backward => "查找方向 / 范围：向后（< 与 > 互换，点击切到全部）",
        FindDirection::All => "查找方向 / 范围：全部（展开结果列表，点击回到向前）",
    };
    if resp.on_hover_text(dir_tip).clicked() {
        dispatch(ui_command_action(FindUiCommand::CycleDirection));
    }
    // 非默认态给一点可见反馈（默认 Forward 时一个像素都不动）。
    if find_state.direction != FindDirection::Forward {
        let dot = egui::pos2(r.right() - 6.0, r.bottom() - 3.0);
        if find_state.direction == FindDirection::All {
            ui.painter().rect_filled(
                egui::Rect::from_center_size(dot + egui::vec2(-3.0, 0.0), egui::vec2(2.0, 2.0)),
                0.0,
                text_c,
            );
        }
        ui.painter().rect_filled(
            egui::Rect::from_center_size(dot, egui::vec2(2.0, 2.0)),
            0.0,
            text_c,
        );
    }

    // 右端三个按钮：描边 `#3B3B3B`、内部 `#262626`（视觉基线实测）。
    let btn_fill = egui::Color32::from_rgb(0x26, 0x26, 0x26);
    let btn_line = egui::Color32::from_rgb(0x3B, 0x3B, 0x3B);
    for (rect, label, id, tip) in [
        (
            lt_r,
            "<".to_owned(),
            "find_btn_prev",
            t(lang, "Find Previous").to_owned(),
        ),
        (
            gt_r,
            ">".to_owned(),
            "find_btn_next",
            t(lang, "Find Next").to_owned(),
        ),
        (
            all_r,
            t(lang, "Find All").to_owned(),
            "find_btn_all",
            format!("Find All（{total} 个匹配，点击展开/收起结果列表）"),
        ),
    ] {
        let resp = ui.interact(rect, egui::Id::new(id), egui::Sense::click());
        let fill = if resp.hovered() { hover_bg } else { btn_fill };
        ui.painter().rect_filled(rect, 0.0, fill);
        ui.painter()
            .rect_stroke(rect.shrink(0.5), 0.0, egui::Stroke::new(1.0, btn_line));
        let g = ui
            .painter()
            .layout_no_wrap(label, font.clone(), text_c);
        ui.painter()
            .galley(rect.center() - g.size() / 2.0, g, text_c);
        if resp.on_hover_text(tip).clicked() {
            match id {
                "find_btn_prev" => dispatch(Action::Find(nav_action(find_state, false))),
                "find_btn_next" => dispatch(Action::Find(nav_action(find_state, true))),
                "find_btn_all" => dispatch(ui_command_action(FindUiCommand::ToggleFindAll)),
                _ => {}
            }
        }
    }

    // 关闭 `×`：无边框、无底色，只有两条对角线。视觉基线里它就是一个纯图标，
    // 与 `Find All` 那种描边按钮明确区分开——它不是「执行」，是「离开」。
    //
    // 纵向钉在**整条栏的中线**上，不是按钮行的中线：视觉基线里 `×` 的墨迹中心
    // （y 16.25，栏体 43 高）比那排 23.5 高的按钮中心高 0.75，跟着栏居中才对。
    // 跟着按钮走会低 1.25 点，和快捷栏那颗对不上（用户要的「上下间距调整到
    // 合适」就是这件事）。
    let close_r = egui::Rect::from_center_size(
        egui::pos2(all_r.right() + CLOSE_GAP + CLOSE_SIZE / 2.0, bar.center().y),
        egui::vec2(CLOSE_SIZE, CLOSE_SIZE),
    );
    let resp = ui.interact(close_r, egui::Id::new("find_btn_close"), egui::Sense::click());
    if resp.hovered() {
        // 悬停给一块比图标大的底，否则 8 点见方的热区太难点。
        ui.painter().rect_filled(
            close_r.expand(5.0),
            0.0,
            crate::theme::chrome(CHROME_SCALE.hover),
        );
    }
    // 对角线比视觉边长少 1 点：那是线宽在两端各吃掉的 0.5。画法在
    // `glyph::paint_cross`——与标签条、面板标题带、快捷栏是同一份。
    glyph::paint_cross(ui.painter(), close_r.center(), text_c);
    if resp.on_hover_text(t(lang, "Close")).clicked() {
        dispatch(Action::Find(FindAction::Close));
    }

    // Find All 展开时，在终端区上方画结果列表。默认关着，视觉基线零变化。
    if find_state.find_all_open {
        let results = crate::ui::terminal::find_results_slot::read(ui.ctx(), &find_state.query);
        draw_find_all_panel(ui, bar, find_state, results.as_ref(), lang, dispatch);
    }

    // 计数本身不画在这条栏里：视觉基线把整条状态栏染色显示
    // `18 of 30 Found`，由 `status_bar.rs` 负责。这里只把命中信息留个
    // 记号，方便 `--screenshot` 之外的调试；没有读者时不产生任何像素。
    if let Some(idx) = current_index(find_state, total) {
        ui.ctx()
            .data_mut(|d| d.insert_temp(egui::Id::new("find_hit"), (idx + 1, total)));
    }
}

/// Find All 结果列表：`Area` 自绘，不改 `ui/mod.rs` 的面板声明。
///
/// 数据来自终端叠层每帧写入的一份 `FindResults`（查询串不符即丢弃）；
/// 点击一条不新增 GotoIndex 动作，按环形索引走较短的一侧到那一项。
fn draw_find_all_panel(
    ui: &mut egui::Ui,
    bar: egui::Rect,
    find_state: &FindState,
    results: Option<&FindResults>,
    lang: Lang,
    dispatch: &mut dyn FnMut(Action),
) {
    let w = (bar.width() - 16.0).clamp(200.0, 440.0);
    let screen_h = ui.ctx().screen_rect().height();
    let h = (screen_h - 24.0).clamp(120.0, 280.0);
    let pos = egui::pos2(bar.right() - w - 8.0, (bar.top() - h - 4.0).max(0.0));
    let active_bg = crate::theme::chrome(CHROME_SCALE.tab);
    let border = crate::theme::chrome(CHROME_SCALE.border);
    let mut close = false;
    egui::Area::new(egui::Id::new("find_all_panel"))
        .order(egui::Order::Foreground)
        .fixed_pos(pos)
        .pivot(egui::Align2::LEFT_TOP)
        .movable(false)
        .fade_in(false)
        .show(ui.ctx(), |ui| {
            egui::Frame::popup(ui.style())
                .fill(active_bg)
                .stroke(egui::Stroke::new(1.0, border))
                .show(ui, |ui| {
                    ui.set_width(w - 16.0);
                    ui.horizontal(|ui| {
                        ui.label(egui::RichText::new(t(lang, "Find All")).strong());
                        if ui.button("×").clicked() {
                            close = true;
                        }
                    });
                    ui.separator();
                    let fresh = results.filter(|r| r.query == find_state.query);
                    match fresh {
                        None => {
                            ui.label("结果列表在终端绘制后刷新");
                        }
                        Some(r) if r.rows.is_empty() => {
                            ui.label("当前窗口内没有匹配");
                        }
                        Some(r) => {
                            let current = current_index(find_state, r.rows.len());
                            egui::ScrollArea::vertical()
                                .max_height(h - 58.0)
                                .show(ui, |ui| {
                                    for row in &r.rows {
                                        let label = format!("{:>5}  {}", row.row, row.summary);
                                        let resp = ui.add(
                                            egui::Button::new(
                                                egui::RichText::new(label).monospace(),
                                            )
                                            .frame(false)
                                            .fill(if current == Some(row.index) {
                                                crate::theme::chrome(CHROME_SCALE.hover)
                                            } else {
                                                egui::Color32::TRANSPARENT
                                            }),
                                        );
                                        if resp.clicked() {
                                            if let Some((action, steps)) =
                                                jump_plan(find_state, r.rows.len(), row.index)
                                            {
                                                for _ in 0..steps {
                                                    dispatch(Action::Find(action.clone()));
                                                }
                                            }
                                        }
                                    }
                                });
                        }
                    }
                });
        });
    if close {
        dispatch(ui_command_action(FindUiCommand::ToggleFindAll));
    }
}

/// 编辑缓冲的一次改动 → 要派发的动作。
///
/// 抽出来是为了给受控契约一个测试点：输入框**只能**产出 `SetQuery`，
/// 不存在「顺手写一下状态」的第二条路径。
pub fn edit_action(input: &str) -> Action {
    Action::Find(FindAction::SetQuery(input.to_string()))
}

/// 把状态里的查询同步进编辑缓冲，返回是否真的覆盖了。
///
/// 只在两者不同时覆盖：相同还覆盖会让 `TextEdit` 的光标位置和输入法的
/// 预编辑文本被重置，表现为「打一个字光标跳回开头」。
pub fn sync_input(input: &mut String, query: &str) -> bool {
    if input == query {
        return false;
    }
    input.clear();
    input.push_str(query);
    true
}

#[cfg(test)]
#[path = "find_bar_tests.rs"]
mod tests;

//! 过滤行：**融进 tab tool 的同一条 25pt 细带**（用户 2026-11-27 裁定）。
//!
//! # 不是贴上去的第二块面板
//!
//! 过滤激活时工具条高度不变（`TAB_TOOLBAR_H` = 25pt），终端区也不少几行。
//! `Filter:` 下划线标签、内联文本、三个图标格、计数与 `×` 全部在同一条细带里
//! 垂直居中；整条只有一条与 Find 同口径的**顶部横线**，输入框没有常驻外壳。
//! 只有输入控件拿到焦点时才画一枚贴合内联输入区的 1px 指示框（正则非法转红），
//! 失焦即消失，回到与背景同色的内联文本。
//!
//! 视觉语言仍与 Find 同源：标签画法、图标格（`.*` / `Aa` / `Invert`）的
//! active / hover / 错误红、右端与 Find 同列的无边框 `×`（`glyph::paint_cross`）。
//! 只是纵向几何压进 25pt：图标格高 `ICON_H` = 19，上下各留 3pt。
//!
//! # 几何只有一份
//!
//! [`CLOSE_R_PAD`] / [`CLOSE_SIZE`] / [`ICON_W`] / [`ICON_R_PAD`] /
//! [`INVALID_COLOR`] 都从 `find_bar` 再导出（见 `shell/mod.rs`）。纯函数
//! [`filter_row_geom`] 不收 `&Ui`，所以「格子多高、`×` 在哪一列、窄条怎么让位」
//! 能直接单测，不用起窗口。
//!
//! # 状态与行为没动
//!
//! 依旧只有 `Tab.filter` / `Tab.filter_input` 持有状态；计数仍读 `filter_diag`
//! （与 `drop_rows` 同源）；绘制期只攒 `FilterAction`，最后 `apply_filter_on`
//! 落到本会话。所有控件 id 带 `pane_salt`，分屏每片 pane 独立。

use crate::ui::*;
use eframe::egui;
use term_app::filter::FilterMode;
use term_core::theme::CHROME_SCALE;

use crate::ui::shell::{
    glyph, label_width, CLOSE_R_PAD, CLOSE_SIZE, FIELD_X, ICON_R_PAD, ICON_W, INVALID_COLOR,
    LABEL_X, TAB_TOOLBAR_H,
};

/// 内联图标格：`.*` / `Aa` / `Invert` 反转开关，共三格。
const ICON_CELLS: usize = 3;
/// 图标格上下各留 3pt，25pt 条里可用高度 19pt（不是 Find 框内的 23.5pt）。
const ROW_INSET_Y: f32 = 3.0;
/// 图标格高：与输入文字、计数同处一条 25pt 中线。
pub(in crate::ui) const ICON_H: f32 = TAB_TOOLBAR_H - 2.0 * ROW_INSET_Y;
/// 计数文字与关闭 `×` 之间的空隙。
const COUNT_CLOSE_GAP: f32 = 8.0;
/// 内联区右沿到计数文字的空隙（与 Find 的 `FIELD_TO_NAV` 同档）。
const INLINE_COUNT_GAP: f32 = 7.5;
/// 窄条下再挤也要留下这么宽的内联区，否则查询根本敲不进去。
const MIN_INLINE_W: f32 = 96.0;
/// 图标组左沿离内联区左沿的最小空隙：标签 / 输入 / 图标不许挤在一起。
const ICON_LEFT_INSET: f32 = 4.0;
/// `Filter:` 比 `Find:` 多两个字符：略收字号，x=11 起也能停在 x≈52 之前。
const LABEL_FONT_PT: f32 = 9.0;
/// 标签墨迹右沿到输入文字左沿的最小间隙。
const LABEL_GAP: f32 = 3.0;
/// 输入文字左侧留白（与 Find 的 `field.left() + 6` 同档）。
const EDIT_PAD_L: f32 = 6.0;

/// 过滤行的纯几何（不依赖 `&egui::Ui`）。`count_w` / `label_w` / `mode_w` 由调用方
/// 量好文字宽传进来，这样「谁放不下、输入区剩多宽」可以直接断言。
#[derive(Clone, Copy, Debug)]
pub(in crate::ui) struct FilterRowGeom {
    /// 无边框的内联输入区（x 范围 + 25pt 条内的纵向位置）。
    pub edit: egui::Rect,
    /// 三格内联图标：`.*` / `Aa` / `Invert` 反转开关。
    pub icons: [egui::Rect; ICON_CELLS],
    /// `kept / hidden`；窄到放不下时返回 `None`（关闭 `×` 永远在）。
    pub count: Option<egui::Rect>,
    /// 整条右端的无边框 `×`，与 Find 栏同一列。
    pub close: egui::Rect,
}

/// `Invert` 格宽：文字实测宽 + `Aa` 格同样的左右内边距（`ICON_W - Aa 字宽`）。
///
/// 不写死一个数：字体一变两格一起变。它比 `Aa` 格宽是因为 "Invert" /
/// "反转" 本身比 "Aa" 长，不是因为多画了一块突兀的大按钮——底色、上下边距、
/// 垂直居中都和相邻两格完全一样。
pub(in crate::ui) fn invert_cell_width(label_w: f32, aa_w: f32) -> f32 {
    label_w + (ICON_W - aa_w)
}

/// 把过滤行排进 25pt 细带。
///
/// 从右往左定关闭 `×` → 计数 → 内联输入区 → 三枚图标；`×` 的横坐标与 Find 栏的
/// `all_r.right() + CLOSE_GAP + CLOSE_SIZE/2` 化简后完全相同（都落在离右沿
/// `CLOSE_R_PAD + CLOSE_SIZE/2` 处）。计数放不下就跳过，`×` 永不跳过。
pub(in crate::ui) fn filter_row_geom(
    bar: egui::Rect,
    count_w: f32,
    label_w: f32,
    mode_w: f32,
) -> FilterRowGeom {
    let cy = bar.center().y;
    let close = egui::Rect::from_center_size(
        egui::pos2(
            bar.right() - CLOSE_R_PAD - CLOSE_SIZE / 2.0,
            cy,
        ),
        egui::vec2(CLOSE_SIZE, CLOSE_SIZE),
    );
    let y_top = cy - ICON_H / 2.0;
    let y_bottom = cy + ICON_H / 2.0;

    let count_right = close.left() - COUNT_CLOSE_GAP;
    let inline_left = (bar.left() + FIELD_X).max(bar.left() + LABEL_X + label_w + LABEL_GAP);
    let count = (count_right - count_w >= inline_left + MIN_INLINE_W).then(|| {
        egui::Rect::from_min_max(
            egui::pos2(count_right - count_w, y_top),
            egui::pos2(count_right, y_bottom),
        )
    });
    let cap = close.left() - COUNT_CLOSE_GAP;
    let right = count
        .map_or(cap, |r| r.left() - INLINE_COUNT_GAP)
        .max(inline_left + MIN_INLINE_W)
        .min(cap);

    let widths = [ICON_W, ICON_W, mode_w];
    // 正常宽度下 scale == 1：三格都保持原尺寸。只有窄到装不下时才等比收一点，
    // 保证图标组不会越过内联区左沿去贴住 `Filter:` 标签。
    let avail = (right - inline_left - ICON_LEFT_INSET - ICON_R_PAD).max(0.0);
    let scale = (avail / widths.iter().sum::<f32>()).min(1.0);
    let mut x = right - ICON_R_PAD - widths.iter().sum::<f32>() * scale;
    let mut icons = [egui::Rect::ZERO; ICON_CELLS];
    for (r, w) in icons.iter_mut().zip(widths) {
        let w = w * scale;
        *r = egui::Rect::from_min_size(egui::pos2(x, y_top), egui::vec2(w, ICON_H));
        x += w;
    }
    let edit = egui::Rect::from_min_max(
        egui::pos2(inline_left, y_top),
        egui::pos2(icons[0].left() - 4.0, y_bottom),
    );
    FilterRowGeom {
        edit,
        icons,
        count,
        close,
    }
}

/// 左端 `Filter:` 的命中矩形：整条固定 25pt，右沿停在输入文字左沿 `FIELD_X` 之前。
pub(in crate::ui) fn filter_label_button_rect(bar: egui::Rect) -> egui::Rect {
    let h = TAB_TOOLBAR_H.min(bar.height());
    let cy = bar.center().y;
    egui::Rect::from_min_max(
        egui::pos2(bar.left() + super::EDGE_PAD, cy - h / 2.0),
        egui::pos2(bar.left() + FIELD_X, cy + h / 2.0),
    )
}

/// 过滤激活态那条**顶部横线**的两个端点：`bar.top() + 0.5`，横贯整条。
///
/// 与 Find 栏上边线同口径（同一条 `bar.top() + 0.5`、同一个
/// `CHROME_SCALE.border`）；Sysmon 收起态不调用这里，那里没有这条线。
pub(in crate::ui) fn filter_top_line(bar: egui::Rect) -> [egui::Pos2; 2] {
    [
        egui::pos2(bar.left(), bar.top() + 0.5),
        egui::pos2(bar.right(), bar.top() + 0.5),
    ]
}

/// 输入聚焦时的指示框：宽度**贴合内联输入区**，高度是文字行高且封顶 25pt，
/// 所以不会变成 Find 那种 23.5pt 的大盒子，也不会越过本条细带。
pub(in crate::ui) fn filter_focus_frame_rect(
    bar: egui::Rect,
    edit: egui::Rect,
    text_h: f32,
) -> egui::Rect {
    let h = text_h.min(bar.height());
    let cy = bar.center().y;
    egui::Rect::from_min_max(
        egui::pos2(edit.left(), cy - h / 2.0),
        egui::pos2(edit.right(), cy + h / 2.0),
    )
}

/// 指示框的 1px 颜色：非法正则时转 `INVALID_COLOR`，与 `.*` 图标同红。
pub(in crate::ui) fn filter_focus_frame_color(
    invalid: bool,
    line_c: egui::Color32,
) -> egui::Color32 {
    if invalid {
        INVALID_COLOR
    } else {
        line_c
    }
}

impl TerminalWindow {
    /// 内联过滤行：`Filter:` 下划线标签 + 无框输入 + 三格图标 + 计数 + `×`。
    ///
    /// 整条始终是 [`TAB_TOOLBAR_H`]（25pt）；`ui.max_rect()` 就是本帧真实高度。
    /// 面板不新增，T/C/M 与 R 芯片由调用方在过滤态跳过。
    pub(in crate::ui) fn draw_filter_row(
        &mut self,
        ui: &mut egui::Ui,
        bar: egui::Rect,
        session: Option<term_core::session_runtime::SessionId>,
        slot: Option<usize>,
        pane_salt: Option<u64>,
    ) {
        let Some(session) = session else { return };
        let Some(slot) = slot else { return };
        let lang = self.view.lang;
        // Button 解析到 Body 12pt：与 Sysmon 的 T/C/M 读数同一档字号。
        let font = egui::TextStyle::Button.resolve(ui.style());

        // 状态与每帧诊断同源：`ensure_view` 在 `drop_rows` 之前用同一次编译算好
        // 并存到 `tab.filter_diag`，这里只读——反算会把隐藏行数成可见，再编译一遍
        // 则一帧两次正则。
        let (mode, case_sensitive, regex) = {
            let f = &self.tabs[slot].filter;
            (f.mode, f.case_sensitive, f.regex)
        };
        let (kept, hidden, regex_error) = match self.tabs[slot].filter_diag.as_ref() {
            Some(d) => (
                d.kept,
                d.hidden,
                // 立刻 `to_string`：下面 `TextEdit` 要 `&mut` 同一条 tab。
                d.error
                    .as_ref()
                    .map(|e| e.message().to_string())
                    .unwrap_or_default(),
            ),
            None => (self.tabs[slot].view.lines.len(), 0, String::new()),
        };

        let mut pending: Vec<term_app::vocab::FilterAction> = Vec::new();
        let count_text = format!("{kept} / {hidden}");
        // 一枚反转开关：两态共用同一个标签，只有底色 / 字色区分开/关。
        // `grep` / `grep -v` 是它的语义，不再作为按钮文案出现在过滤行里。
        let mode_label = t(lang, "Invert");
        let mode_tip = t(lang, "Invert filter (grep -v): hide matching lines");
        let label = t(lang, "Filter:");
        let label_font = crate::ui::type_scale::font(LABEL_FONT_PT);
        // 计数用状态栏次要读数那一档小字，不抢 25pt 条里的主体。
        let count_font = crate::ui::type_scale::small();

        let line_c = crate::theme::chrome(CHROME_SCALE.border);
        let text_c = crate::theme::chrome(CHROME_SCALE.text);
        let strong_c = crate::theme::chrome(CHROME_SCALE.text_strong);
        let active_bg = crate::theme::chrome(CHROME_SCALE.tab);
        let hover_bg = crate::theme::chrome(CHROME_SCALE.hover);
        let invalid = !regex_error.is_empty();

        let label_galley = ui
            .painter()
            .layout_no_wrap(label.to_owned(), label_font, text_c);
        let count_w = label_width(ui, &count_text, &count_font);
        let mode_w = invert_cell_width(
            label_width(ui, mode_label, &font),
            label_width(ui, "Aa", &font),
        );
        let geom = filter_row_geom(bar, count_w, label_galley.size().x, mode_w);

        let hit = |name: &'static str| match pane_salt {
            Some(s) => egui::Id::new((name, s)),
            None => egui::Id::new(name),
        };

        ui.scope_builder(
            egui::UiBuilder::new()
                .id_salt(("tab_toolbar_filter", pane_salt))
                .max_rect(bar),
            |ui| {
                let p = ui.painter();
                // 与 Find 栏同口径的 1px 顶部横线，横贯整条；输入框没有常驻外壳。
                p.line_segment(filter_top_line(bar), egui::Stroke::new(1.0, line_c));

                // `Filter:` 与 Find 的 `Find:` 同款：galley + 首字下划线，x=11，
                // 在 25pt 条里垂直居中。
                let ly = bar.center().y - label_galley.size().y / 2.0;
                p.galley(
                    egui::pos2(bar.left() + LABEL_X, ly),
                    label_galley.clone(),
                    text_c,
                );
                let fy = ly + label_galley.size().y - 1.0;
                if let Some(g) = label_galley.rows.first().and_then(|r| r.glyphs.first()) {
                    let x0 = bar.left() + LABEL_X + g.pos.x;
                    p.line_segment(
                        [egui::pos2(x0, fy), egui::pos2(x0 + g.advance_width, fy)],
                        egui::Stroke::new(1.0, text_c),
                    );
                }
            },
        );

        // 输入必须先画：`ui.painter()` / `ui.scope_builder()` 不能同时借同一 ui。
        // 无框内联文本；把文字行高精确居中，margin 清零，避免自带 padding
        // 把字顶到细带上沿。缓冲克隆进 child，取回后写回本会话，再统一派发 `SetQuery`。
        if geom.edit.width() > 0.0 {
            let text_h = ui.text_style_height(&egui::TextStyle::Body);
            let edit_rect = filter_focus_frame_rect(bar, geom.edit, text_h);
            let mut focused = false;
            let edited = ui.scope_builder(egui::UiBuilder::new().max_rect(edit_rect), |ui| {
                let mut buf = self.tabs[slot].filter_input.clone();
                let resp = ui.add(
                    egui::TextEdit::singleline(&mut buf)
                        .id_source(("tab_toolbar_filter_query", pane_salt))
                        .frame(false)
                        .margin(egui::Margin::ZERO)
                        .desired_width(geom.edit.width())
                        .hint_text(t(lang, "Filter hint")),
                );
                focused = resp.has_focus();
                let changed = resp.changed();
                self.tabs[slot].filter_input = buf;
                changed
            });
            // 激活指示框只在输入控件拿到焦点时出现：1px、贴合内联输入区、封顶 25pt；
            // 失焦后这里不落笔，回到无外壳的内联文本。正则非法时与 `.*` 同红。
            if focused {
                ui.painter().rect_stroke(
                    edit_rect.shrink(0.5),
                    0.0,
                    egui::Stroke::new(1.0, filter_focus_frame_color(invalid, line_c)),
                );
            }
            if edited.inner {
                let q = self.tabs[slot].filter_input.clone();
                pending.push(term_app::vocab::FilterAction::SetQuery(q));
            }
            if edited.inner && ui.input(|i| i.key_pressed(egui::Key::Escape)) {
                pending.push(term_app::vocab::FilterAction::Close);
            }
        }

        // 三格内联图标：与 Find 的 `.*` / `Aa` 同构（激活铺 tab、悬停铺 hover）。
        // `.*` 语法错时图标与反转格一起转红，悬停给出错误——同 Find。
        let r = geom.icons[0];
        let resp = ui.interact(r, hit("tab_toolbar_filter_regex"), egui::Sense::click());
        if regex {
            ui.painter().rect_filled(r, 0.0, active_bg);
        } else if resp.hovered() {
            ui.painter().rect_filled(r, 0.0, hover_bg);
        }
        let icon_c = if invalid { INVALID_COLOR } else { text_c };
        let g = ui
            .painter()
            .layout_no_wrap(".*".to_owned(), font.clone(), icon_c);
        ui.painter().galley(r.center() - g.size() / 2.0, g, icon_c);
        let regex_hint = if invalid {
            format!("Regular expression — invalid pattern: {regex_error}")
        } else if regex {
            "Regular expression (on)".to_string()
        } else {
            "Regular expression".to_string()
        };
        if resp.on_hover_text(regex_hint).clicked() {
            pending.push(term_app::vocab::FilterAction::ToggleRegex);
        }

        let r = geom.icons[1];
        let resp = ui.interact(r, hit("tab_toolbar_filter_case"), egui::Sense::click());
        if case_sensitive {
            ui.painter().rect_filled(r, 0.0, active_bg);
        } else if resp.hovered() {
            ui.painter().rect_filled(r, 0.0, hover_bg);
        }
        let g = ui
            .painter()
            .layout_no_wrap("Aa".to_owned(), font.clone(), text_c);
        ui.painter().galley(r.center() - g.size() / 2.0, g, text_c);
        if resp.on_hover_text(t(lang, "Match Case")).clicked() {
            pending.push(term_app::vocab::FilterAction::ToggleCase);
        }

        // 反转开关：和 `.*` / `Aa` 同一个 icon_rect、同一套底色与字色，只有
        // 这一格是「关 → 不铺底，开 → 铺 tab」，与 Find 栏开关的口径完全相同。
        // 默认 KeepMatching（不激活）；点一下变 DropMatching（grep -v，隐藏命中行）。
        let drop = mode == FilterMode::DropMatching;
        let r = geom.icons[2];
        let resp = ui.interact(r, hit("tab_toolbar_filter_mode"), egui::Sense::click());
        if drop {
            ui.painter().rect_filled(r, 0.0, active_bg);
        } else if resp.hovered() {
            ui.painter().rect_filled(r, 0.0, hover_bg);
        }
        // 非法正则时跟 `.*` 一起转红，连激活态也不拿 strong 色盖掉错误。
        let mode_c = if invalid {
            INVALID_COLOR
        } else if drop {
            strong_c
        } else {
            text_c
        };
        let g = ui
            .painter()
            .layout_no_wrap(mode_label.to_owned(), font.clone(), mode_c);
        ui.painter().galley(r.center() - g.size() / 2.0, g, mode_c);
        if resp.on_hover_text(mode_tip).clicked() {
            pending.push(term_app::filter::ui_command(
                term_app::filter::FilterUiCommand::ToggleMode,
            ));
        }

        // 框外计数：状态栏次要读数同款小字与颜色，只读诊断不缓存。
        if let Some(r) = geom.count {
            let g = ui
                .painter()
                .layout_no_wrap(count_text.clone(), count_font, text_c);
            ui.painter().galley(r.center() - g.size() / 2.0, g, text_c);
            ui.interact(r, hit("tab_toolbar_filter_count"), egui::Sense::hover())
                .on_hover_text("kept / hidden");
        }

        // 右端 `×`：无边框、无常态底色，只两条对角线；画法与 Find / 快捷栏 / 面板
        // 标题带共用 `glyph::paint_cross`，这里是同一个函数、同一个右距。
        let resp = ui.interact(
            geom.close,
            hit("tab_toolbar_filter_close"),
            egui::Sense::click(),
        );
        if resp.hovered() {
            ui.painter()
                .rect_filled(geom.close.expand(5.0), 0.0, hover_bg);
        }
        glyph::paint_cross(ui.painter(), geom.close.center(), text_c);
        if resp.on_hover_text(t(lang, "Close")).clicked() {
            pending.push(term_app::vocab::FilterAction::Close);
        }

        for action in pending {
            // `Action::Filter` 不写 `ViewState`；对本会话那条 tab 落地（分屏互不串味）。
            self.apply_filter_on(session, action);
        }
    }
}

#[cfg(test)]
#[path = "tab_toolbar_filter_tests.rs"]
mod filter_row_tests;

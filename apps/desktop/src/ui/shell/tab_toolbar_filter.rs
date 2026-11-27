//! 过滤行：与 Find 栏**同一套视觉语言**（用户 2026-11-27 裁定）。
//!
//! # 为什么重写
//!
//! 旧版是一排 egui 默认横排文字按钮，与 Find 栏那种「标签 + 手绘边框输入框 +
//! 框内图标格 + 框外计数 + 无边框 `×`」完全两套语言。现在整行按
//! `find_bar.rs` 的布局顺序与常量画：横坐标、纵向几何、错误反馈都与 Find 同源，
//! 连关闭 `×` 都走 `glyph::paint_cross`，不出现第二套叉。
//!
//! # 几何只有一份
//!
//! [`CLOSE_R_PAD`] / [`CLOSE_SIZE`] / [`FIELD_H`] / [`FIELD_TOP`] / [`ICON_W`] /
//! [`INVALID_COLOR`] 都从 `find_bar` 再导出（见 `shell/mod.rs`）。纯函数
//! [`filter_row_geom`] 不收 `&Ui`，所以「字段多高、`×` 在哪一列、窄条怎么让位」
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
    glyph, label_width, CLOSE_R_PAD, CLOSE_SIZE, FIELD_H, FIELD_TOP, FIELD_X, ICON_R_PAD, ICON_W,
    INVALID_COLOR, LABEL_X, TAB_TOOLBAR_H,
};

/// 框内图标格：`.*` / `Aa` / grep 两态，共三格。
const ICON_CELLS: usize = 3;
/// 计数文字与关闭 `×` 之间的空隙。
const COUNT_CLOSE_GAP: f32 = 8.0;
/// 输入框右沿到计数文字的空隙（与 Find 的 `FIELD_TO_NAV` 同档）。
const FIELD_COUNT_GAP: f32 = 7.5;
/// 窄条下再挤也要留下这么宽的输入框，否则查询根本敲不进去。
const MIN_FIELD_W: f32 = 96.0;
/// `Filter:` 比 `Find:` 多两个字符：略收字号，x=11 起也能停在 x≈52 的框左沿之前。
const LABEL_FONT_PT: f32 = 9.0;
/// 标签墨迹右沿到输入框左沿的最小间隙。
const LABEL_GAP: f32 = 3.0;

/// 过滤行的纯几何（不依赖 `&egui::Ui`）。`count_w` / `label_w` / `mode_w` 由调用方
/// 量好文字宽传进来，这样「谁放不下、字段剩多宽」可以直接断言。
#[derive(Clone, Copy, Debug)]
pub(in crate::ui) struct FilterRowGeom {
    /// 手绘边框的输入框（Find 同高 23.5pt）。
    pub field: egui::Rect,
    /// `TextEdit` 的实际落位：左留 6pt，右让开图标组。
    pub edit: egui::Rect,
    /// 框内三格：`.*` / `Aa` / grep 两态。
    pub icons: [egui::Rect; ICON_CELLS],
    /// 框外的 `kept / hidden`；窄到放不下时返回 `None`（关闭 `×` 永远在）。
    pub count: Option<egui::Rect>,
    /// 整条右端的无边框 `×`，与 Find 栏同一列。
    pub close: egui::Rect,
}

/// 按 Find 栏的纵向几何排过滤行。
///
/// 从右往左定关闭 `×` → 计数 → 输入框 → 框内图标组；`×` 的横坐标与 Find 栏的
/// `all_r.right() + CLOSE_GAP + CLOSE_SIZE/2` 化简后完全相同（都落在离右沿
/// `CLOSE_R_PAD + CLOSE_SIZE/2` 处）。计数放不下就跳过，`×` 永不跳过。
pub(in crate::ui) fn filter_row_geom(
    bar: egui::Rect,
    count_w: f32,
    label_w: f32,
    mode_w: f32,
) -> FilterRowGeom {
    let top = bar.top() + 1.0; // 让开那条 1 点上边线（Find 同款）
    let field_top = top + FIELD_TOP;
    let close = egui::Rect::from_center_size(
        egui::pos2(
            bar.right() - CLOSE_R_PAD - CLOSE_SIZE / 2.0,
            bar.center().y,
        ),
        egui::vec2(CLOSE_SIZE, CLOSE_SIZE),
    );
    let count_right = close.left() - COUNT_CLOSE_GAP;
    let field_left = (bar.left() + FIELD_X).max(bar.left() + LABEL_X + label_w + LABEL_GAP);
    let count = (count_right - count_w >= field_left + MIN_FIELD_W).then(|| {
        egui::Rect::from_min_max(
            egui::pos2(count_right - count_w, field_top),
            egui::pos2(count_right, field_top + FIELD_H),
        )
    });
    let cap = close.left() - COUNT_CLOSE_GAP;
    let right = count.map_or(cap, |r| r.left() - FIELD_COUNT_GAP);
    // MIN_FIELD_W 是下限而不是上限：极窄时宁可字段短一点，也不许压到 `×`。
    let field_right = right.max(field_left + MIN_FIELD_W).min(cap);
    let field = egui::Rect::from_min_max(
        egui::pos2(field_left, field_top),
        egui::pos2(field_right, field_top + FIELD_H),
    );

    let widths = [ICON_W, ICON_W, mode_w];
    let mut x = field.right() - ICON_R_PAD - widths.iter().sum::<f32>();
    let mut icons = [egui::Rect::ZERO; ICON_CELLS];
    for (r, w) in icons.iter_mut().zip(widths) {
        *r = egui::Rect::from_min_size(
            egui::pos2(x, field.top() + 2.0),
            egui::vec2(w, FIELD_H - 4.0),
        );
        x += w;
    }
    let edit = egui::Rect::from_min_max(
        egui::pos2(field.left() + 6.0, field.top()),
        egui::pos2(icons[0].left() - 4.0, field.bottom()),
    );
    FilterRowGeom {
        field,
        edit,
        icons,
        count,
        close,
    }
}

/// 激活态左端 `Filter:` 的命中矩形：24pt 条里的 25pt 快速按钮化作 Find 式标签，
/// 高度仍钉在 [`TAB_TOOLBAR_H`]，右沿停在输入框左沿 `FIELD_X` 之前。
pub(in crate::ui) fn filter_label_button_rect(bar: egui::Rect) -> egui::Rect {
    let h = TAB_TOOLBAR_H.min(bar.height());
    let cy = bar.center().y;
    egui::Rect::from_min_max(
        egui::pos2(bar.left() + super::EDGE_PAD, cy - h / 2.0),
        egui::pos2(bar.left() + FIELD_X, cy + h / 2.0),
    )
}

impl TerminalWindow {
    /// Find 式过滤行：`Filter:` 标签 + 手绘边框输入框 + 框内三格 + 计数 + `×`。
    ///
    /// 整条在 [`TAB_TOOLBAR_FILTER_H`]（= Find 栏 43pt）内绘制；`ui.max_rect()`
    /// 就是本帧真实高度。面板不新增，T/C/M 与 R 芯片由调用方在过滤态跳过。
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
        let mode_label = match mode {
            FilterMode::KeepMatching => t(lang, "grep"),
            FilterMode::DropMatching => t(lang, "grep -v"),
        };
        let label = t(lang, "Filter:");
        let label_font = crate::ui::type_scale::font(LABEL_FONT_PT);

        let line_c = crate::theme::chrome(CHROME_SCALE.border);
        let text_c = crate::theme::chrome(CHROME_SCALE.text);
        let strong_c = crate::theme::chrome(CHROME_SCALE.text_strong);
        let active_bg = crate::theme::chrome(CHROME_SCALE.tab);
        let hover_bg = crate::theme::chrome(CHROME_SCALE.hover);
        let invalid = !regex_error.is_empty();
        let border_c = if invalid { INVALID_COLOR } else { line_c };

        let label_galley = ui
            .painter()
            .layout_no_wrap(label.to_owned(), label_font, text_c);
        let count_w = label_width(ui, &count_text, &font);
        let mode_w = label_width(ui, t(lang, "grep -v"), &font) + 10.0;
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
                // 上下各一条 1 点分界线——Find 栏靠这两条线与邻居分开。
                for y in [bar.top() + 0.5, bar.bottom() - 0.5] {
                    p.line_segment(
                        [egui::pos2(bar.left(), y), egui::pos2(bar.right(), y)],
                        egui::Stroke::new(1.0, line_c),
                    );
                }

                // `Filter:` 与 Find 的 `Find:` 同款：galley + 首字下划线，x=11。
                let ly = geom.field.center().y - label_galley.size().y / 2.0;
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

                // 输入框只画边框；`shrink(0.5)` 与 Find 同款，1 点描边不被抗锯齿摊成两行。
                p.rect_stroke(
                    geom.field.shrink(0.5),
                    0.0,
                    egui::Stroke::new(1.0, border_c),
                );
            },
        );

        // 输入框必须先画：`ui.painter()` / `ui.scope_builder()` 不能同时借同一 ui。
        // 缓冲克隆进 child，取回后写回本会话，再统一派发 `SetQuery`。
        if geom.edit.width() > 0.0 {
            let edited = ui.scope_builder(egui::UiBuilder::new().max_rect(geom.edit), |ui| {
                let mut buf = self.tabs[slot].filter_input.clone();
                let resp = ui.add(
                    egui::TextEdit::singleline(&mut buf)
                        .id_source(("tab_toolbar_filter_query", pane_salt))
                        .frame(false)
                        .desired_width(geom.edit.width())
                        .hint_text(t(lang, "Filter hint")),
                );
                let changed = resp.changed();
                self.tabs[slot].filter_input = buf;
                changed
            });
            if edited.inner {
                let q = self.tabs[slot].filter_input.clone();
                pending.push(term_app::vocab::FilterAction::SetQuery(q));
            }
            if edited.inner && ui.input(|i| i.key_pressed(egui::Key::Escape)) {
                pending.push(term_app::vocab::FilterAction::Close);
            }
        }

        // 框内三格：与 Find 的 `.*` / `Aa` 同构（激活铺 tab、悬停铺 hover）。
        // `.*` 语法错时图标与输入框边框同时转红，悬停给出错误——同 Find。
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

        // grep / grep -v 两态格：默认 grep 不铺底，切到 grep -v 才铺 tab。
        let drop = mode == FilterMode::DropMatching;
        let r = geom.icons[2];
        let resp = ui.interact(r, hit("tab_toolbar_filter_mode"), egui::Sense::click());
        if drop {
            ui.painter().rect_filled(r, 0.0, active_bg);
        } else if resp.hovered() {
            ui.painter().rect_filled(r, 0.0, hover_bg);
        }
        let mode_c = if drop { strong_c } else { text_c };
        let g = ui
            .painter()
            .layout_no_wrap(mode_label.to_owned(), font.clone(), mode_c);
        ui.painter().galley(r.center() - g.size() / 2.0, g, mode_c);
        let mode_tip = format!("{} / {} — click to toggle", t(lang, "grep"), t(lang, "grep -v"));
        if resp.on_hover_text(mode_tip).clicked() {
            pending.push(term_app::filter::ui_command(
                term_app::filter::FilterUiCommand::ToggleMode,
            ));
        }

        // 框外计数：像 Find 的 `N / M`，这里是 `kept / hidden`。不存、不算，只读诊断。
        if let Some(r) = geom.count {
            let g = ui
                .painter()
                .layout_no_wrap(count_text.clone(), font.clone(), text_c);
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
mod filter_row_tests {
    use super::*;
    use super::super::find_bar::CLOSE_GAP;
    use super::super::tab_toolbar::{filter_button_rect, quick_button_rect};
    use super::super::{FIND_BAR_H, TAB_TOOLBAR_FILTER_H};

    /// 过滤行与 Sysmon 读数共用同一条细带、不产生第二个 `TopBottomPanel` 声明。
    #[test]
    fn filter_row_shares_the_sysmon_band_no_second_panel() {
        let src = include_str!("tab_toolbar.rs");
        let self_src = include_str!("tab_toolbar_filter.rs");
        let mod_src = include_str!("../mod.rs");
        let shell_src = include_str!("mod.rs");

        assert!(
            !mod_src.contains("TopBottomPanel::bottom(\"filter_bar\")"),
            "不许有独立过滤底栏"
        );
        assert!(!mod_src.contains("filter_bar"), "ui/mod.rs 不许再有 filter_bar");
        assert!(!shell_src.contains("mod sysmon_bar;"), "sysmon_bar.rs 已删除");
        assert!(!shell_src.contains("draw_filter_bar"), "不再有 draw_filter_bar");
        assert!(src.contains("fn draw_tab_toolbar"), "工具条绘制仍在 tab_toolbar");
        assert!(src.contains("readout_segments"), "Sysmon 读数仍在同一条带");
        assert!(src.contains("draw_filter_row"), "过滤行画在工具条上");
        assert!(self_src.contains("egui::TextEdit::singleline"), "过滤行有 TextEdit");
        assert!(self_src.contains("FilterUiCommand::ToggleMode"), "grep / grep -v 切换");
        assert!(self_src.contains("FilterAction::ToggleCase"), "大小写");
        assert!(self_src.contains("FilterAction::ToggleRegex"), "正则");
        assert!(self_src.contains("FilterAction::Close"), "关闭");
        assert!(
            self_src.contains("glyph::paint_cross"),
            "关闭叉必须复用 glyph，不许第二套画法"
        );
        assert!(src.contains("(\"sysmon_filter\", p)"), "Filter 按钮 id 带 pane_salt");
        assert!(
            self_src.contains("(\"tab_toolbar_filter_query\", pane_salt)"),
            "输入 id 带 pane_salt"
        );
        assert!(
            self_src.contains("(\"tab_toolbar_filter\", pane_salt)"),
            "过滤行 id 带 pane_salt"
        );
    }

    fn wide_bar() -> egui::Rect {
        egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(900.0, TAB_TOOLBAR_FILTER_H))
    }

    /// Find 栏输入框高 23.5，过滤行必须逐点相同，且整条高度就是 `FIND_BAR_H`。
    #[test]
    fn filter_field_is_find_bar_height() {
        assert_eq!(TAB_TOOLBAR_FILTER_H, FIND_BAR_H);
        assert_eq!(FIELD_H, 23.5);
        let geom = filter_row_geom(wide_bar(), 46.0, 38.0, 52.0);
        assert_eq!(geom.field.height(), FIELD_H);
        assert_eq!(geom.field.top(), 1.0 + FIELD_TOP);
        assert_eq!(geom.icons[0].height(), FIELD_H - 4.0);
        assert_eq!(geom.icons[0].width(), ICON_W);
    }

    /// 过滤行的 `×` 与 Find 栏的 `×` 落在同一列（横坐标化简后是同一个式子），
    /// 并且按 find_bar 的画法钉在整条中线上。
    #[test]
    fn close_sits_in_find_bar_column() {
        let bar = wide_bar();
        let geom = filter_row_geom(bar, 46.0, 38.0, 52.0);
        // Find: close_cx = all_r.right() + CLOSE_GAP + CLOSE_SIZE/2
        //       all_r.right() = bar.right() - CLOSE_R_PAD - CLOSE_SIZE - CLOSE_GAP
        let find_close_cx = bar.right()
            - CLOSE_R_PAD
            - CLOSE_SIZE
            - CLOSE_GAP
            + CLOSE_GAP
            + CLOSE_SIZE / 2.0;
        assert_eq!(geom.close.center().x, find_close_cx);
        assert_eq!(geom.close.right(), bar.right() - CLOSE_R_PAD);
        assert_eq!(geom.close.center().y, bar.center().y);
    }

    /// 43pt 条里的快速按钮仍是 25pt，与 Sysmon 按钮同高（同一个 `quick_button_rect`）。
    #[test]
    fn filter_button_stays_25pt_in_43pt_bar() {
        let bar = wide_bar();
        let r = filter_label_button_rect(bar);
        assert_eq!(r.height(), TAB_TOOLBAR_H);
        assert_eq!(r.height(), 25.0);
        assert!(r.right() <= bar.left() + FIELD_X);
        let natural = filter_button_rect(bar, 55.0);
        assert_eq!(natural.height(), TAB_TOOLBAR_H);
        assert_eq!(natural.center().y, bar.center().y);
        let sysmon = quick_button_rect(bar, bar.right() - 60.0, 55.0);
        assert_eq!(sysmon.height(), natural.height());
    }

    /// 宽条三格全在、互不重叠、计数贴着 `×` 左侧；窄条先丢计数，`×` 永在。
    #[test]
    fn wide_and_narrow_layouts_never_overlap() {
        let geom = filter_row_geom(wide_bar(), 46.0, 38.0, 52.0);
        let count = geom.count.expect("宽条要放得下计数");
        assert!(geom.field.right() <= count.left());
        assert!(count.right() <= geom.close.left());
        assert!(geom.icons[0].right() <= geom.icons[1].left());
        assert!(geom.icons[1].right() <= geom.icons[2].left());
        assert!(geom.icons[2].right() <= geom.field.right() + 0.01);

        let narrow = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(190.0, TAB_TOOLBAR_FILTER_H));
        let small = filter_row_geom(narrow, 46.0, 38.0, 52.0);
        assert!(small.count.is_none(), "窄条先丢计数");
        assert!(small.close.right() <= narrow.right());
        assert!(small.field.right() <= small.close.left());
        assert!(small.field.width() > 0.0);
    }
}

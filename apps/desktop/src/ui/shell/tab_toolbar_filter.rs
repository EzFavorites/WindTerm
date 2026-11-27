//! 过滤行：**融进 tab tool 的同一条 25pt 细带**（用户 2026-11-27 裁定）。
//!
//! # 不是贴上去的第二块面板
//!
//! 过滤激活时工具条高度不变（`TAB_TOOLBAR_H` = 25pt），终端区也不少几行。
//! `Filter:` 下划线标签、内联文本、四个图标格与 `▾`、计数与 `×` 都在同一条细带里
//! 垂直居中；整条只有一条与 Find 同口径的**顶部横线**，输入框没有常驻外壳。
//! 功能区之间用 1px 竖线分隔（与上边线同色、`ROW_INSET_Y` 收在条内），细淡不抢
//! 内容，也不给每个图标格描框。收起态（只剩 Sysmon）不画这些线。
//! 只有输入控件拿到焦点时才画一枚贴合内联输入区的 1px 指示框（正则非法转红），
//! 失焦即消失，回到与背景同色的内联文本。
//!
//! 视觉语言仍与 Find 同源：标签画法、图标格（`.*` / `Aa` / 纯画笔两向箭头）的
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
//! 依旧只有 `Tab.filter` / `Tab.filter_input` 持有查询状态；计数仍读 `filter_diag`
//! （与 `drop_rows` 同源）；绘制期只攒 `FilterAction`，最后 `apply_filter_on`
//! 落到本会话。历史是同属会话的 `Tab.filter_history`，只在 `Commit` 时写；
//! 浮层的开/选中放 egui 临时态（带 `pane_salt`），不进 `ViewState`、不落盘。
//! 所有控件 id 带 `pane_salt`，分屏每片 pane 独立。

use crate::ui::*;
use eframe::egui;
use term_app::filter::FilterMode;
use term_core::theme::CHROME_SCALE;

use crate::ui::shell::{
    glyph, label_width, CLOSE_R_PAD, CLOSE_SIZE, FIELD_X, ICON_R_PAD, ICON_W, INVALID_COLOR,
    LABEL_X, TAB_TOOLBAR_H,
};

/// 内联图标格：`.*` / `Aa` / `W` / 两向箭头反转开关，共四格。
const ICON_CELLS: usize = 4;
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
/// 每条会话保留的最近过滤查询数上限。
pub(in crate::ui) const FILTER_HISTORY_CAP: usize = 16;
/// `▾` 按钮宽：贴输入区右端、图标格之前；命中区就是绘制区。
const HIST_BTN_W: f32 = 14.0;

/// 提交一条查询到本会话历史：最近在前、去重、封顶 [`FILTER_HISTORY_CAP`]。
/// 空串与内部 NUL 前缀子命令不记——它们不是查询，记下来只会污染下拉。
pub(in crate::ui) fn push_filter_history(history: &mut Vec<String>, query: &str) {
    if query.is_empty() || query.starts_with('\0') {
        return;
    }
    history.retain(|q| q != query);
    history.insert(0, query.to_owned());
    history.truncate(FILTER_HISTORY_CAP);
}

/// ↑↓ 循环：`None` 时 ↑ 取最后一条、↓ 取第一条；有选中则取模回绕。
pub(in crate::ui) fn cycle_filter_history(
    selected: Option<usize>,
    total: usize,
    delta: i32,
) -> Option<usize> {
    if total == 0 {
        return None;
    }
    let n = total as i32;
    let next = match selected {
        None if delta < 0 => n - 1,
        None => 0,
        Some(i) => ((i.min(total - 1) as i32) + delta).rem_euclid(n),
    };
    Some(next as usize)
}

/// `▾` 历史按钮的命中/绘制矩形：贴输入区右端，与图标格之间仍留原空隙。
pub(in crate::ui) fn filter_history_button_rect(edit: egui::Rect) -> egui::Rect {
    egui::Rect::from_min_size(
        egui::pos2(edit.right() - HIST_BTN_W, edit.top()),
        egui::vec2(HIST_BTN_W, edit.height()),
    )
}

/// 过滤行的纯几何（不依赖 `&egui::Ui`）。`count_w` / `label_w` / `mode_w` 由调用方
/// 量好文字宽传进来，这样「谁放不下、输入区剩多宽」可以直接断言。
#[derive(Clone, Copy, Debug)]
pub(in crate::ui) struct FilterRowGeom {
    /// 无边框的内联输入区（x 范围 + 25pt 条内的纵向位置）。
    pub edit: egui::Rect,
    /// 四格内联图标：`.*` / `Aa` / `W` / 纯画笔两向箭头（反转开关）。
    pub icons: [egui::Rect; ICON_CELLS],
    /// `kept / hidden`；窄到放不下时返回 `None`（关闭 `×` 永远在）。
    pub count: Option<egui::Rect>,
    /// 整条右端的无边框 `×`，与 Find 栏同一列。
    pub close: egui::Rect,
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

    let widths = [ICON_W, ICON_W, ICON_W, mode_w];
    // 正常宽度下 scale == 1：四格都保持原尺寸。只有窄到装不下时才等比收一点，
    // 保证图标组不会越过内联区左沿去贴住 `Filter:` 标签（四格 + ▾ 都不重叠）。
    // 窄窗先给 `▾` + 间隙留位，再保住 MIN_INLINE_W 的文字区；不够时四格等比
    // 收缩（极端窄宽到 0，也不越过输入区去压标签 / 按钮）。
    let reserved = HIST_BTN_W + 2.0 + MIN_INLINE_W;
    let avail = (right - inline_left - ICON_LEFT_INSET - ICON_R_PAD - reserved).max(0.0);
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

/// 四条竖线的 x：标签|输入、输入|图标、图标|计数、计数|`×`，取两区之间的中点。
/// 窄条丢掉计数时最后一根为 `None`（没两区可隔）；收起态由调用点不入这里。
pub(in crate::ui) fn filter_separators(geom: &FilterRowGeom) -> [Option<f32>; 4] {
    let mid = |a: f32, b: f32| (a + b) / 2.0;
    let after_icons = geom.count.map_or_else(
        || mid(geom.icons[3].right(), geom.close.left()),
        |c| mid(geom.icons[3].right(), c.left()),
    );
    [
        Some(geom.edit.left() - LABEL_GAP / 2.0),
        Some(mid(geom.edit.right(), geom.icons[0].left())),
        Some(after_icons),
        geom.count.map(|c| mid(c.right(), geom.close.left())),
    ]
}

/// 纯画笔两向箭头：与 Find 方向按钮的两条水平箭头同一画法（`line_segment` / 1px），
/// 不排字，不依赖字体里有没有该字形。只出墨迹；底色/红色由调用方给。
pub(in crate::ui) fn paint_invert_icon(
    p: &egui::Painter,
    center: egui::Pos2,
    color: egui::Color32,
) {
    let st = egui::Stroke::new(1.0, color);
    let rad = 4.5;
    for (dir, dy) in [(1.0_f32, -2.5_f32), (-1.0, 2.5)] {
        let mid = egui::pos2(center.x, center.y + dy);
        p.line_segment(
            [mid - egui::vec2(rad, 0.0), mid + egui::vec2(rad, 0.0)],
            st,
        );
        let tip = egui::pos2(mid.x + rad * dir, mid.y);
        p.line_segment([tip, tip + egui::vec2(-1.5 * dir, -2.0)], st);
        p.line_segment([tip, tip + egui::vec2(-1.5 * dir, 2.0)], st);
    }
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

/// 指示框的 1px 颜色：合法时取主题焦点色 `CHROME_SCALE.focus`（Find 蓝），
/// 非法正则时转 `INVALID_COLOR`，与 `.*` 图标同红——红线优先于蓝。
/// 顶部横线 / 竖线仍用 `border` 灰，不跟着这里变蓝。
pub(in crate::ui) fn filter_focus_frame_color(invalid: bool) -> egui::Color32 {
    if invalid {
        INVALID_COLOR
    } else {
        crate::theme::chrome(CHROME_SCALE.focus)
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
        let (mode, case_sensitive, regex, whole_word) = {
            let f = &self.tabs[slot].filter;
            (f.mode, f.case_sensitive, f.regex, f.whole_word)
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
        // 一枚反转开关：两态共用同一枚画笔箭头，只有底色 / 墨色区分开/关。
        // `grep` / `grep -v` 是它的语义；`Invert` 只留在悬停提示里，不上界面。
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
        let mode_w = ICON_W; // 箭头和 `.*` / `Aa` 同宽，不再按文案宽度撑开。
        let geom = filter_row_geom(bar, count_w, label_galley.size().x, mode_w);

        let hit = |name: &'static str| match pane_salt {
            Some(s) => egui::Id::new((name, s)),
            None => egui::Id::new(name),
        };

        // ── 历史：查询只在本会话 `Tab`；浮层开/选中放 egui 临时态，带 pane_salt ──
        let edit_id = ui.make_persistent_id(("tab_toolbar_filter_query", pane_salt));
        let popup_id = hit("tab_toolbar_filter_history");
        let sel_id = popup_id.with("selection");
        let history = self.tabs[slot].filter_history.clone();
        let mut popup_open = ui.memory(|m| m.is_popup_open(popup_id));
        let input_focused = ui.memory(|m| m.has_focus(edit_id));
        let mut selected = ui.data(|d| d.get_temp::<usize>(sel_id));
        // ↑↓ 只把候选文本同步进输入缓冲，**不改 `filter.query`**：选「先同步、
        // Enter 才提交」，所以逐字符的 SetQuery 不会把 "e"/"er"/"err" 灌进历史。
        let (up, down) = ui.input(|i| {
            (
                i.key_pressed(egui::Key::ArrowUp),
                i.key_pressed(egui::Key::ArrowDown),
            )
        });
        if input_focused && (up || down) && !history.is_empty() {
            selected = cycle_filter_history(selected, history.len(), if up { -1 } else { 1 });
            if let Some(i) = selected {
                self.tabs[slot].filter_input = history[i].clone();
            }
            if !popup_open {
                ui.memory_mut(|m| m.open_popup(popup_id));
                popup_open = true;
            }
        }
        ui.data_mut(|d| {
            if let Some(i) = selected {
                d.insert_temp(sel_id, i);
            } else {
                d.remove_temp::<usize>(sel_id);
            }
        });
        let synced = self.tabs[slot].filter_input.clone();

        ui.scope_builder(
            egui::UiBuilder::new()
                .id_salt(("tab_toolbar_filter", pane_salt))
                .max_rect(bar),
            |ui| {
                let p = ui.painter();
                // 与 Find 栏同口径的 1px 顶部横线，横贯整条；输入框没有常驻外壳。
                p.line_segment(filter_top_line(bar), egui::Stroke::new(1.0, line_c));

                // 功能区之间的 1px 竖线：同色口径，高度收在 25pt 条内上下各 3pt，
                // 细淡不抢内容；只隔四区，不给每个图标格描框。
                for x in filter_separators(&geom).into_iter().flatten() {
                    p.line_segment(
                        [
                            egui::pos2(x, bar.top() + ROW_INSET_Y),
                            egui::pos2(x, bar.bottom() - ROW_INSET_Y),
                        ],
                        egui::Stroke::new(1.0, line_c),
                    );
                }

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
                let mut buf = synced.clone();
                let resp = ui.add(
                    egui::TextEdit::singleline(&mut buf)
                        .id(edit_id)
                        .frame(false)
                        .margin(egui::Margin::ZERO)
                        // 右端给 `▾` 留位：文字不钻到按钮底下，按钮仍压在输入区内。
                        .desired_width((geom.edit.width() - HIST_BTN_W - 2.0).max(1.0))
                        .hint_text(t(lang, "Filter hint")),
                );
                focused = resp.has_focus();
                // ↑↓ 同步进来的文本不算「用户改字」，不派 SetQuery、更不记历史。
                let changed = resp.changed() && buf != synced;
                self.tabs[slot].filter_input = buf;
                changed
            });
            // 激活指示框只在输入控件拿到焦点时出现：1px、贴合内联输入区、封顶 25pt；
            // 失焦后这里不落笔，回到无外壳的内联文本。正则非法时与 `.*` 同红。
            if focused {
                ui.painter().rect_stroke(
                    edit_rect.shrink(0.5),
                    0.0,
                    egui::Stroke::new(1.0, filter_focus_frame_color(invalid)),
                );
            }
            let (enter, escape) = ui.input(|i| {
                (
                    i.key_pressed(egui::Key::Enter),
                    i.key_pressed(egui::Key::Escape),
                )
            });
            // Esc 关浮层时连本帧的 SetQuery 一起丢，候选文本绝不落到生效查询。
            if edited.inner && !(escape && popup_open) {
                let q = self.tabs[slot].filter_input.clone();
                pending.push(term_app::vocab::FilterAction::SetQuery(q));
            }
            if enter && focused {
                pending.push(term_app::vocab::FilterAction::Commit);
                if popup_open {
                    ui.memory_mut(|m| m.close_popup());
                    popup_open = false;
                }
            }
            if escape && popup_open {
                // Esc 只关浮层：未提交的候选退回已生效查询，历史不动。
                ui.memory_mut(|m| m.close_popup());
                self.tabs[slot].filter_input = self.tabs[slot].filter.query.clone();
                ui.data_mut(|d| d.remove_temp::<usize>(sel_id));
                popup_open = false;
            } else if escape && edited.inner {
                // 浮层没开时保留原行为：Esc 关掉过滤栏（关栏同样记一条历史）。
                pending.push(term_app::vocab::FilterAction::Commit);
                pending.push(term_app::vocab::FilterAction::Close);
            }
        }

        // `▾` 历史入口：贴输入区右端、图标组之前。无系统边框，命中区 == 绘制区，
        // hover 铺底色 + 指向光标；三角直接用 painter 画，与 Find 手绘按钮同源。
        let hist_btn = filter_history_button_rect(geom.edit);
        let hist_resp = ui.interact(
            hist_btn,
            hit("tab_toolbar_filter_history_btn"),
            egui::Sense::click(),
        );
        if hist_resp.hovered() {
            ui.painter().rect_filled(hist_btn, 0.0, hover_bg);
            ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
        }
        let hc = hist_btn.center();
        ui.painter().add(egui::Shape::convex_polygon(
            vec![
                hc + egui::vec2(-3.5, -1.6),
                hc + egui::vec2(3.5, -1.6),
                hc + egui::vec2(0.0, 2.2),
            ],
            text_c,
            egui::Stroke::NONE,
        ));
        if hist_resp
            .on_hover_text("Filter history — ↑↓ browse · Enter apply · Esc close")
            .clicked()
        {
            ui.memory_mut(|m| m.toggle_popup(popup_id));
            popup_open = !popup_open;
            // 点完按钮把焦点还给输入，↑↓ 可以接着翻历史。
            ui.memory_mut(|m| m.request_focus(edit_id));
        }

        // 四格内联图标：与 Find 的 `.*` / `Aa` / `W` 同构（激活铺 tab、悬停铺 hover）。
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

        // `W` 全词：Find 同款一格（`ICON_W` 宽、19pt 高、active/hover/错误红），
        // 紧挨 `.*` / `Aa`。开关走 NUL 前缀 `SetQuery` 子命令 'w'，不新增命令 ID。
        let r = geom.icons[2];
        let resp = ui.interact(r, hit("tab_toolbar_filter_whole_word"), egui::Sense::click());
        if whole_word {
            ui.painter().rect_filled(r, 0.0, active_bg);
        } else if resp.hovered() {
            ui.painter().rect_filled(r, 0.0, hover_bg);
        }
        let wc = if invalid { INVALID_COLOR } else { text_c };
        let g = ui
            .painter()
            .layout_no_wrap("W".to_owned(), font.clone(), wc);
        ui.painter().galley(r.center() - g.size() / 2.0, g, wc);
        if resp.on_hover_text(t(lang, "Match Whole Word")).clicked() {
            pending.push(term_app::filter::ui_command(
                term_app::filter::FilterUiCommand::ToggleWholeWord,
            ));
        }

        // 反转开关：和 `.*` / `Aa` / `W` 同一个 icon_rect、同一套底色与墨色，
        // 只有这一格是「关 → 不铺底，开 → 铺 tab」，与 Find 栏开关口径相同。
        // 默认 KeepMatching（不激活）；点一下变 DropMatching（grep -v，隐藏命中行）。
        // 图标用纯 painter 两向箭头：无系统按钮边框，命中区就是这格绘制区。
        let drop = mode == FilterMode::DropMatching;
        let r = geom.icons[3];
        let resp = ui.interact(r, hit("tab_toolbar_filter_mode"), egui::Sense::click());
        if resp.hovered() {
            ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
        }
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
        paint_invert_icon(ui.painter(), r.center(), mode_c);
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
            // 关栏也把当前输入记一条历史，再清掉生效查询。
            pending.push(term_app::vocab::FilterAction::Commit);
            pending.push(term_app::vocab::FilterAction::Close);
        }

        // 历史浮层：`egui::Area` 覆盖在过滤行上方，不新增面板、不切终端高度。
        if popup_open {
            if let Some(i) = super::tab_toolbar_filter_popup::history_popup(
                ui,
                popup_id,
                geom.edit,
                hist_btn,
                &history,
                selected,
            ) {
                let q = history[i].clone();
                self.tabs[slot].filter_input = q.clone();
                pending.push(term_app::vocab::FilterAction::SetQuery(q));
                pending.push(term_app::vocab::FilterAction::Commit);
                ui.memory_mut(|m| m.close_popup());
                ui.data_mut(|d| d.insert_temp(sel_id, i));
            }
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

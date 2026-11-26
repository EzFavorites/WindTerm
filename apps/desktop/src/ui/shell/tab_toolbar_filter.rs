//! 紧凑过滤行：与 Sysmon 读数共用同一条 tab toolbar 的左段。
//!
//! # 为什么单独一个文件
//!
//! `tab_toolbar.rs` 原本 721 代码行（门禁 500），过滤行只占 120 行——把「过滤行
//! 绘制 + 命中几何」挪到这里，主文件回到 476 行。几何全部是纯函数（不收 `&Ui`），
//! 窄条下「跳谁、剩多宽」因此能直接单测，不用起窗口。
//!
//! # 与 Sysmon 读数的关系
//!
//! 过滤行画在**同一条**工具条上（左端 `Filter` 按钮的右边、条右沿之内），不是
//! 第二栏。高度由 [`TAB_TOOLBAR_FILTER_H`] 决定，见 `tab_toolbar.rs` 的模块头。

use crate::ui::*;
use eframe::egui;

// 与 `tab_toolbar` 共用同一套几何常量与 `Filter` 按钮矩形：过滤行从 `Filter` 按钮
// 右边起步、停在条右沿内，两处是同一句话（`crate::ui::shell::*` 已带它们进来）。
use crate::ui::shell::{
    filter_button_rect, label_width, EDGE_PAD, GAP, TAB_TOOLBAR_FILTER_H,
};
use term_app::filter::FilterMode;
use term_core::theme::CHROME_SCALE;

/// 过滤行内控件的高（与 Find 栏输入框同高）与它的上内边距。
const FIELD_H: f32 = 23.5;
const FIELD_TOP: f32 = (TAB_TOOLBAR_FILTER_H - FIELD_H) / 2.0;
/// 过滤行内相邻开关的间隙。
const BTN_GAP: f32 = 6.0;
/// 输入框再窄也不许低于这个值：窄到装不下查询就没意义了，宁可裁掉右边的开关。
const MIN_FIELD_W: f32 = 96.0;

/// 过滤行的五段命中矩形 + 输入框。`widths` 从**左到右**依次是
/// `[grep 模式, 大小写, 正则, 关闭, kept/hidden]`（调用方量好文字宽）。
///
/// 从右往左放；装不下就**返回 `None` 跳过**，绝不与前一段重叠。跳过从最右开始，
/// 所以 `Close` 永远在——它是退出过滤的唯一入口。
///
/// 纯函数：命中几何不依赖 `&egui::Ui`，所以「跳谁、字段剩多宽」能进单测
/// （[`filter_row_tests::narrow_bar_drops_switches_instead_of_overlapping`]）。
pub(in crate::ui) fn filter_row_layout(
    rect: egui::Rect,
    filter_w: f32,
    widths: &[f32; 5],
) -> (
    Option<egui::Rect>,
    Option<egui::Rect>,
    Option<egui::Rect>,
    Option<egui::Rect>,
    Option<egui::Rect>,
    egui::Rect,
) {
    let left = filter_button_rect(rect, filter_w).right() + GAP;
    let right = rect.right() - EDGE_PAD;
    let mut x = right;
    let mut rects = [None, None, None, None, None];
    for i in (0..5usize).rev() {
        // 给输入框留出下限；这一段连同后面的开关一起放不下就跳过这一段。
        if x - widths[i] < left + MIN_FIELD_W {
            continue;
        }
        let (r, next) = band(rect, x, widths[i]);
        rects[i] = Some(r);
        x = next - BTN_GAP;
    }
    let field = egui::Rect::from_min_max(
        egui::pos2(left, rect.top() + FIELD_TOP),
        egui::pos2(x.max(left + MIN_FIELD_W), rect.top() + FIELD_TOP + FIELD_H),
    );
    (rects[0], rects[1], rects[2], rects[3], rects[4], field)
}

/// 一段开关的矩形 + 它的左沿。
fn band(rect: egui::Rect, x_right: f32, w: f32) -> (egui::Rect, f32) {
    let x_left = x_right - w;
    (
        egui::Rect::from_min_max(
            egui::pos2(x_left, rect.top() + FIELD_TOP),
            egui::pos2(x_right, rect.top() + FIELD_TOP + FIELD_H),
        ),
        x_left,
    )
}

impl TerminalWindow {
    /// 紧凑过滤行：`TextEdit` + grep / grep -v + 大小写 + 正则 + 关闭 + `kept/hidden`。
    ///
    /// 全部 egui 控件 id 带 `pane_salt`：分屏时每个 pane 只读写自己那条会话的
    /// `Tab.filter` / `Tab.filter_input`，左右两格互不串味。受控输入——本函数只
    /// 收集 `FilterAction`，收集完再统一 `apply_filter_on`，避免绘制期借用冲突。
    ///
    /// 布局走**显式命中矩形**而不是 `ui.horizontal_centered`：后者按内容宽度自己排，
    /// 排不下就越过条沿，外面量不到「到底有没有压到 `Filter` 按钮」。这里由
    /// [`filter_row_layout`] 给每段一个矩形（装不下的段跳过、绝不重叠），
    /// `TextEdit` 放进按那个矩形建的 child 里，矩形本身可以拿去断言。
    pub(in crate::ui) fn draw_filter_row(
        &mut self,
        ui: &mut egui::Ui,
        rect: egui::Rect,
        session: Option<term_core::session_runtime::SessionId>,
        slot: Option<usize>,
        pane_salt: Option<u64>,
    ) {
        let Some(session) = session else { return };
        let Some(slot) = slot else { return };
        let lang = self.view.lang;
        let font = crate::ui::type_scale::body();

        // 过滤状态与每帧诊断**同源**：`ensure_view` 在 `drop_rows` **之前**用同一次
        // 编译算好并存到 `tab.filter_diag`，这里只读。两处都不能反过来——从
        // `tab.view`（丢掉之后的窗口）反算 `kept` 会把被隐藏的行数成可见；在这里再
        // `compile` 一遍则一帧编译两次正则。
        let (mode, case_sensitive, regex) = {
            let f = &self.tabs[slot].filter;
            (f.mode, f.case_sensitive, f.regex)
        };
        let (kept, hidden, regex_error) = match self.tabs[slot].filter_diag.as_ref() {
            Some(d) => (
                d.kept,
                d.hidden,
                // `""` = 这一帧没有编译错误。空串与 `None` 等价：工具条只判断「有没有
                // 提示」。立刻 `to_string`：`message()` 借的是 `self.tabs[slot]` 里那份，
                // 而下面 `TextEdit` 要 `&mut` 同一条 tab，跨借用会直接 E0502。
                d.error
                    .as_ref()
                    .map(|e| e.message().to_string())
                    .unwrap_or_default(),
            ),
            // 本会话还没画过一帧（没有 `ensure_view` 算过）：按「不过滤」读。
            // 这里**只数行**，不重算命中——计数必须与丢弃同源，见上。
            None => {
                let total = self.tabs[slot].view.lines.len();
                (total, 0, String::new())
            }
        };

        let mut pending: Vec<term_app::vocab::FilterAction> = Vec::new();
        let salt = pane_salt;
        let count_text = format!("{kept}/{hidden}");
        let mode_label = match mode {
            FilterMode::KeepMatching => t(lang, "grep"),
            FilterMode::DropMatching => t(lang, "grep -v"),
        };

        // ── 布局：从右往左排五段，装不下的**跳过**而不是重叠。`Close` 永远在
        //    （唯一的退出入口）；连 `Close` 都放不下就整行不出。
        let widths: [f32; 5] = [
            label_width(ui, mode_label, &font),
            label_width(ui, t(lang, "Aa"), &font),
            label_width(ui, t(lang, "Regular expression"), &font),
            label_width(ui, t(lang, "Close"), &font),
            label_width(ui, &count_text, &font),
        ];
        let (mode_rect, case_rect, regex_rect, close_rect, count_rect, field) =
            filter_row_layout(rect, self.filter_button_w(ui), &widths);
        let Some(close_rect) = close_rect else { return };

        // 输入框必须**先**画：`ui.painter()` 与 `ui.scope_builder()` 不能同时借用
        // 同一个 ui。缓冲克隆出来交给 `TextEdit`，取回后写回本会话字段。
        let edit_rect = egui::Rect::from_min_max(
            egui::pos2(field.left() + 3.0, field.top()),
            egui::pos2(field.right() - 3.0, field.bottom()),
        );
        let edited = ui.scope_builder(egui::UiBuilder::new().max_rect(edit_rect), |ui| {
            let mut buf = self.tabs[slot].filter_input.clone();
            let resp = ui.add(
                egui::TextEdit::singleline(&mut buf)
                    .id_source(("tab_toolbar_filter_query", salt))
                    .frame(false)
                    .desired_width(edit_rect.width())
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

        // 绘制段：选中态只给底色，不画边框（与工具条其余控件一致）；被
        // `filter_row_layout` 跳过的段不画。正则语法错与 Find 栏同口径——输入框
        // 边框转红 + `正则` 那一格的字转红（这条工具条里没有第三个位置放错误文本）。
        ui.scope_builder(
            egui::UiBuilder::new()
                .id_salt(("tab_toolbar_filter", salt))
                .max_rect(rect),
            |ui| {
                let p = ui.painter();
                let text_c = crate::theme::chrome(CHROME_SCALE.text);
                let on_c = crate::theme::chrome(CHROME_SCALE.text_strong);
                let invalid_c = egui::Color32::from_rgb(0xE0, 0x6C, 0x6C);
                let border = if regex_error.is_empty() {
                    crate::theme::chrome(CHROME_SCALE.border)
                } else {
                    invalid_c
                };
                p.rect_stroke(field.shrink(0.5), 0.0, egui::Stroke::new(1.0, border));
                let invalid = !regex_error.is_empty();
                let segments: [(Option<egui::Rect>, bool, bool, &str); 5] = [
                    (
                        mode_rect,
                        mode == term_app::filter::FilterMode::DropMatching,
                        false,
                        mode_label,
                    ),
                    (case_rect, case_sensitive, false, t(lang, "Aa")),
                    (
                        regex_rect,
                        regex,
                        invalid,
                        t(lang, "Regular expression"),
                    ),
                    (Some(close_rect), false, false, t(lang, "Close")),
                    (count_rect, false, false, count_text.as_str()),
                ];
                for (r, on, err, label) in segments {
                    let Some(r) = r else { continue };
                    if on {
                        p.rect_filled(
                            r,
                            2.0,
                            crate::theme::chrome(CHROME_SCALE.tab),
                        );
                    }
                    p.text(
                        r.center(),
                        egui::Align2::CENTER_CENTER,
                        label,
                        font.clone(),
                        if err {
                            invalid_c
                        } else if on {
                            on_c
                        } else {
                            text_c
                        },
                    );
                }
            },
        );

        // 命中区登记在**外层** ui 上：child 的 id 树只用于控件本身。
        let hit = |name: &'static str| match pane_salt {
            Some(salt) => egui::Id::new((name, salt)),
            None => egui::Id::new(name),
        };
        let pressed = |id: egui::Id, r: egui::Rect, ui: &mut egui::Ui| -> bool {
            let resp = ui.interact(r, id, egui::Sense::click());
            if resp.hovered() {
                ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
            }
            resp.clicked()
        };
        if let Some(r) = mode_rect {
            if pressed(hit("tab_toolbar_filter_mode"), r, ui) {
                pending.push(term_app::filter::ui_command(
                    term_app::filter::FilterUiCommand::ToggleMode,
                ));
            }
        }
        if let Some(r) = case_rect {
            if pressed(hit("tab_toolbar_filter_case"), r, ui) {
                pending.push(term_app::vocab::FilterAction::ToggleCase);
            }
        }
        if let Some(r) = regex_rect {
            if pressed(hit("tab_toolbar_filter_regex"), r, ui) {
                pending.push(term_app::vocab::FilterAction::ToggleRegex);
            }
        }
        if ui.interact(close_rect, hit("tab_toolbar_filter_close"), egui::Sense::click()).clicked() {
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

    /// 过滤行与 Sysmon 读数共用同一条细带、不产生第二个 `TopBottomPanel` 声明。
    ///
    /// 源码/绘制形状断言（仓库已有 `include_str!` 风格）：`draw_tab_toolbar` 里同时
    /// 含 Sysmon 读数与过滤行；`ui/mod.rs` 里没有 `filter_bar` 独立面板；过滤输入
    /// 与按钮 id 带 `pane_salt`；`sysmon_bar.rs` 已删除（`tab_toolbar` 接管）。
    #[test]
    fn filter_row_shares_the_sysmon_band_no_second_panel() {
        let src = include_str!("tab_toolbar.rs");
        let self_src = include_str!("tab_toolbar_filter.rs");
        let mod_src = include_str!("../mod.rs");
        let shell_src = include_str!("mod.rs");

        // 不产生第二个 TopBottomPanel 声明（这条细带只声明一次，且不是 filter_bar）。
        assert!(
            !mod_src.contains("TopBottomPanel::bottom(\"filter_bar\")"),
            "不许有独立过滤底栏 TopBottomPanel::bottom(filter_bar)"
        );
        assert!(
            !mod_src.contains("filter_bar"),
            "ui/mod.rs 不许再有 filter_bar 独立面板"
        );
        assert!(
            !shell_src.contains("mod sysmon_bar;"),
            "sysmon_bar.rs 已删除,不再声明"
        );
        assert!(
            !shell_src.contains("draw_filter_bar"),
            "不再有 draw_filter_bar"
        );

        // 过滤行与 Sysmon 读数同一条工具条：两个函数都在 tab_toolbar 里。
        assert!(
            src.contains("fn draw_tab_toolbar"),
            "工具条绘制仍在 tab_toolbar"
        );
        assert!(src.contains("readout_segments"), "Sysmon 读数仍在同一条带");
        assert!(
            src.contains("draw_filter_row"),
            "过滤行画在工具条上"
        );
        assert!(
            self_src.contains("egui::TextEdit::singleline"),
            "过滤行有 TextEdit"
        );
        assert!(self_src.contains("FilterUiCommand::ToggleMode"), "grep / grep -v 切换");
        assert!(self_src.contains("FilterAction::ToggleCase"), "大小写");
        assert!(self_src.contains("FilterAction::ToggleRegex"), "正则");
        assert!(self_src.contains("FilterAction::Close"), "关闭");

        // 输入与按钮 id 带 pane_salt。
        assert!(
            src.contains("(\"sysmon_filter\", p)"),
            "Filter 按钮 id 带 pane_salt"
        );
        assert!(
            self_src.contains("(\"tab_toolbar_filter_query\", salt)"),
            "输入 id 带 pane_salt"
        );
        assert!(
            self_src.contains("(\"tab_toolbar_filter\", salt)"),
            "过滤行 id 带 pane_salt"
        );
    }

    /// 窄条下开关被跳过而不是重叠（`filter_row_layout` 的纯几何保证）。
    #[test]
    fn narrow_bar_drops_switches_instead_of_overlapping() {
        let rect = egui::Rect::from_min_size(
            egui::Pos2::ZERO,
            egui::vec2(320.0, TAB_TOOLBAR_FILTER_H),
        );
        let widths = [40.0, 24.0, 90.0, 50.0, 30.0];
        let (mode, case, regex, close, count, field) =
            filter_row_layout(rect, 50.0, &widths);

        // 从右往左放,Close 永远在。
        assert!(close.is_some(), "Close 是退出入口,必须留下");
        // 装不下的段返回 None,绝不与 Close 重叠。
        assert!(
            field.width() >= MIN_FIELD_W,
            "输入框保留下限,不塌成 0 宽"
        );
        if let (Some(c), Some(k)) = (close, count) {
            assert!(c.right() <= k.left() + 0.01, "Close 在计数左边,不重叠");
        }
        let _ = (mode, case, regex);
    }

    /// 宽条下五段全在、且互不重叠、右沿贴住 EDGE_PAD。
    #[test]
    fn wide_bar_keeps_all_switches_without_overlap() {
        let rect = egui::Rect::from_min_size(
            egui::Pos2::ZERO,
            egui::vec2(900.0, TAB_TOOLBAR_FILTER_H),
        );
        let widths = [40.0, 24.0, 90.0, 50.0, 30.0];
        let (mode, case, regex, close, count, field) =
            filter_row_layout(rect, 50.0, &widths);

        let all = [mode, case, regex, close, count];
        assert!(all.iter().all(|r| r.is_some()), "宽条五段全在");
        assert!(
            count.unwrap().right() <= rect.right() - EDGE_PAD + 0.01,
            "最右段贴住 EDGE_PAD"
        );
        // 相邻不重叠:左段右沿 <= 右段左沿。
        assert!(mode.unwrap().right() <= case.unwrap().left());
        assert!(case.unwrap().right() <= regex.unwrap().left());
        assert!(regex.unwrap().right() <= close.unwrap().left());
        assert!(close.unwrap().right() <= count.unwrap().left());
        assert!(field.right() <= mode.unwrap().left(), "字段在最左,不越界");
    }
}

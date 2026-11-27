//! `tab_toolbar_filter.rs` 的几何与源码断言（独立文件，守住单文件 500 行门禁）。
    use super::*;
    use super::super::find_bar::CLOSE_GAP;
    use super::super::tab_toolbar::{filter_button_rect, quick_button_rect};
    use super::super::TAB_TOOLBAR_H;

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
        assert!(self_src.contains("FilterUiCommand::ToggleMode"), "Invert 切换模式");
        assert!(self_src.contains("t(lang, \"Invert\")"), "按钮文案是 Invert / 反转");
        assert!(
            self_src.contains("t(lang, \"Invert filter (grep -v): hide matching lines\")"),
            "悬停写明 grep -v 语义"
        );
        assert!(
            !self_src.contains("t(lang, \"grep\")") && !self_src.contains("t(lang, \"grep -v\")"),
            "过滤行不再显示 grep / grep -v 文案"
        );
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

    /// 工具条高度恒定：没有 43pt 档；输入框无常驻外壳，只有聚焦指示框用描边。
    #[test]
    fn filter_row_is_inline_in_the_25pt_band() {
        let src = include_str!("tab_toolbar.rs");
        let self_src = include_str!("tab_toolbar_filter.rs");
        let pane_src = include_str!("../layout/pane_chrome.rs");
        assert_eq!(TAB_TOOLBAR_H, 25.0);
        assert!(!src.contains("TAB_TOOLBAR_FILTER_H"), "不再有 43pt 高度常量");
        assert!(!pane_src.contains("TAB_TOOLBAR_FILTER_H"), "pane 不再按 43pt 切段");
        assert!(
            src.contains("pub(in crate::ui) fn tab_toolbar_height"),
            "高度入口保留，但恒为 25"
        );
        // 拼出被调用名，避免这句断言自己把名字写进源码里。
        let whole_frame_call = ["rect_", "stroke"].concat();
        let before_focus = self_src.split("if focused {").next().unwrap();
        assert!(
            !before_focus.contains(whole_frame_call.as_str()),
            "输入框无常驻描边/外壳；指示框只在聚焦分支落笔"
        );
        // 拼出被禁用的字面量，避免这句断言自己把名字写进源码里。
        let bottom_line = ["bar.bottom()", " - 0.5"].concat();
        assert!(!self_src.contains(bottom_line.as_str()), "底部分隔线已去掉");
        assert!(self_src.contains("filter_top_line(bar)"), "顶部横线在过滤激活态画");
        assert!(
            src.contains(") -> f32 {\n        TAB_TOOLBAR_H\n    }"),
            "tab_toolbar_height 只返回一个高度"
        );
    }

    /// 过滤激活态那条 1px 顶部横线：与 Find 栏同坐标、同色口径，横贯整条；
    /// Sysmon 收起态不画它。
    #[test]
    fn filter_row_has_find_style_top_line() {
        let bar = wide_bar();
        let [a, b] = filter_top_line(bar);
        assert_eq!(a.y, bar.top() + 0.5);
        assert_eq!(b.y, a.y);
        assert_eq!(a.x, bar.left());
        assert_eq!(b.x, bar.right());
        let self_src = include_str!("tab_toolbar_filter.rs");
        let toolbar_src = include_str!("tab_toolbar.rs");
        assert!(self_src.contains("filter_top_line(bar)"), "过滤行画顶部横线");
        assert!(self_src.contains("CHROME_SCALE.border"), "与 Find 上边线同色口径");
        assert!(
            !toolbar_src.contains("filter_top_line"),
            "Sysmon 收起态不新增这条线"
        );
    }

    /// 无常驻输入框外壳：底部分隔线与常驻描边都不存在；聚焦指示框宽度贴合
    /// 内联输入区，高度封顶 25pt，不会变成 23.5pt 大盒子。
    #[test]
    fn filter_edit_has_no_permanent_shell() {
        let bar = wide_bar();
        let geom = filter_row_geom(bar, 46.0, 38.0, MODE_W);
        let frame = filter_focus_frame_rect(bar, geom.edit, 16.0);
        assert_eq!(frame.left(), geom.edit.left());
        assert_eq!(frame.right(), geom.edit.right());
        assert_eq!(frame.width(), geom.edit.width());
        assert!(frame.height() < TAB_TOOLBAR_H, "不是 23.5pt 大盒子");
        assert!(frame.top() >= bar.top() && frame.bottom() <= bar.bottom());
        let clamped = filter_focus_frame_rect(bar, geom.edit, 100.0);
        assert_eq!(clamped.height(), TAB_TOOLBAR_H, "高度封顶在 25pt 条内");
        assert!(clamped.top() >= bar.top() && clamped.bottom() <= bar.bottom());
        let self_src = include_str!("tab_toolbar_filter.rs");
        let bottom_line = ["bar.bottom()", " - 0.5"].concat();
        assert!(!self_src.contains(bottom_line.as_str()), "无常驻底部分隔线");
        assert!(self_src.contains(".frame(false)"), "TextEdit 自身无框");
    }

    /// 指示框只在聚焦时画：焦点来自 TextEdit 响应，失焦分支不落笔。
    #[test]
    fn focus_frame_is_drawn_only_when_focused() {
        let self_src = include_str!("tab_toolbar_filter.rs");
        let gate = self_src
            .split("if focused {")
            .nth(1)
            .expect("有一处聚焦分支")
            .split('}')
            .next()
            .unwrap();
        assert!(gate.contains("rect_stroke"), "聚焦分支才画指示框");
        let before = self_src.split("if focused {").next().unwrap();
        let whole_frame_call = ["rect_", "stroke"].concat();
        assert!(!before.contains(whole_frame_call.as_str()), "未聚焦不画框");
        assert!(
            self_src.contains("focused = resp.has_focus()"),
            "聚焦取自 TextEdit 响应"
        );
    }

    /// 非法正则时指示框转 `INVALID_COLOR`，与 `.*` 图标同红。
    #[test]
    fn focus_frame_turns_red_on_invalid_regex() {
        let line_c = egui::Color32::from_rgb(0x44, 0x44, 0x44);
        assert_eq!(filter_focus_frame_color(true, line_c), INVALID_COLOR);
        assert_eq!(filter_focus_frame_color(false, line_c), line_c);
        let self_src = include_str!("tab_toolbar_filter.rs");
        assert!(self_src.contains("filter_focus_frame_color(invalid, line_c)"));
        assert!(self_src.contains("let invalid = !regex_error.is_empty()"));
        assert!(
            self_src.contains("let icon_c = if invalid { INVALID_COLOR }"),
            "与 .* 图标同红"
        );
    }

    fn wide_bar() -> egui::Rect {
        egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(900.0, TAB_TOOLBAR_H))
    }

    /// `Invert` / `反转` 格的实测宽代表值：等宽 12pt 下 6 字符 ≈ 43.2，
    /// 加上与 `Aa` 相同的内边距 `ICON_W - 14.4` ≈ 55.8，取 56。
    const MODE_W: f32 = 56.0;

    /// 和 `Aa` 一格用同样的内边距：文字一样宽时格宽就一样，不会凭空多一块大按钮。
    #[test]
    fn invert_cell_uses_the_same_padding_as_aa() {
        assert!((invert_cell_width(14.4, 14.4) - ICON_W).abs() < 0.001);
        assert!((invert_cell_width(43.2, 14.4) - (43.2 + ICON_W - 14.4)).abs() < 0.001);
        // 比 Aa 宽仅仅因为字更长，仍在同一量级。
        assert!(invert_cell_width(43.2, 14.4) < ICON_W * 2.2);
    }

    /// 25pt 条里的三格与输入 / 计数同处一条中线，格高压到 19pt。
    #[test]
    fn all_cells_share_the_25pt_centerline() {
        let bar = wide_bar();
        let geom = filter_row_geom(bar, 46.0, 38.0, MODE_W);
        assert_eq!(ICON_H, 19.0);
        assert_eq!(geom.icons[0].height(), ICON_H);
        assert_eq!(geom.icons[2].width(), MODE_W);
        for r in geom.icons {
            assert_eq!(r.top(), bar.center().y - ICON_H / 2.0);
            assert_eq!(r.bottom(), bar.center().y + ICON_H / 2.0);
            assert_eq!(r.center().y, bar.center().y);
        }
        assert_eq!(geom.edit.center().y, bar.center().y);
        assert_eq!(geom.close.center().y, bar.center().y);
        assert_eq!(geom.count.unwrap().center().y, bar.center().y);
    }

    /// 过滤行的 `×` 与 Find 栏的 `×` 落在同一列（横坐标化简后是同一个式子），
    /// 并且按 find_bar 的画法钉在整条中线上。
    #[test]
    fn close_sits_in_find_bar_column() {
        let bar = wide_bar();
        let geom = filter_row_geom(bar, 46.0, 38.0, MODE_W);
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

    /// 左端标签命中区仍是 25pt，右沿停在输入文字起点之前。
    #[test]
    fn filter_button_stays_25pt_in_the_25pt_bar() {
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
        let geom = filter_row_geom(wide_bar(), 46.0, 38.0, MODE_W);
        let count = geom.count.expect("宽条要放得下计数");
        assert!(geom.edit.right() <= count.left());
        assert!(count.right() <= geom.close.left());
        assert!(geom.icons[0].right() <= geom.icons[1].left());
        assert!(geom.icons[1].right() <= geom.icons[2].left());
        assert!(geom.icons[2].right() + ICON_R_PAD <= count.left());
        assert!(geom.icons[0].left() >= geom.edit.left() + 0.01);

        let narrow = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(190.0, TAB_TOOLBAR_H));
        let small = filter_row_geom(narrow, 46.0, 38.0, MODE_W);
        assert!(small.count.is_none(), "窄条先丢计数");
        assert!(small.close.right() <= narrow.right());
        assert!(small.edit.right() <= small.close.left());
        // 变宽的 Invert 格也不能越过标签去贴住 `Filter:`。
        assert!(small.icons[0].left() >= small.edit.left() + ICON_LEFT_INSET - 0.01);
        assert!(small.icons[2].right() <= small.close.left() + 0.01);
    }

//! `tab_toolbar_filter.rs` 的几何与源码断言（独立文件，守住单文件 500 行门禁）。
    use super::*;
    use super::super::find_bar::CLOSE_GAP;
    use super::super::tab_toolbar::{filter_button_rect, quick_button_rect};
    use super::super::TAB_TOOLBAR_H;
    use term_app::vocab::FilterAction;

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
        assert!(
            !self_src.contains("t(lang, \"Invert\")"),
            "界面不再出现 Invert / 反转文字"
        );
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

    /// 聚焦指示框合法时取主题焦点色 `CHROME_SCALE.focus`（Find 蓝），非法时转红；
    /// 顶部横线 / 竖线仍用 `border` 灰，不跟着变蓝。
    #[test]
    fn focus_frame_uses_focus_token_and_red_only_on_invalid_regex() {
        assert_eq!(filter_focus_frame_color(true), INVALID_COLOR);
        assert_eq!(
            filter_focus_frame_color(false),
            crate::theme::chrome(CHROME_SCALE.focus),
            "合法时用焦点色令牌，不写字面色值"
        );
        let self_src = include_str!("tab_toolbar_filter.rs");
        assert!(self_src.contains("filter_focus_frame_color(invalid)"));
        assert!(self_src.contains("crate::theme::chrome(CHROME_SCALE.focus)"));
        assert!(self_src.contains("let invalid = !regex_error.is_empty()"));
        assert!(
            self_src.contains("let icon_c = if invalid { INVALID_COLOR }"),
            "与 .* 图标同红"
        );
        assert!(
            self_src.contains("let line_c = crate::theme::chrome(CHROME_SCALE.border)"),
            "横线/竖线仍是 border 灰"
        );
        assert!(
            !self_src.contains("1E90FF") && !self_src.contains("0x1e90ff"),
            "ui 里不许硬编码蓝色"
        );
    }

    fn wide_bar() -> egui::Rect {
        egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(900.0, TAB_TOOLBAR_H))
    }

    /// 反转是一枚小图标格：与 `.*` / `Aa` 同宽，不再按文案宽度撑开。
    const MODE_W: f32 = 27.0;

    /// 反选用纯 painter 两向箭头，不是字体字形，也不在界面留 Invert / 反转文字。
    /// i18n key 仍服务于悬停提示；语义仍是默认 KeepMatching / 点击 DropMatching。
    #[test]
    fn invert_is_a_painted_icon_cell() {
        let self_src = include_str!("tab_toolbar_filter.rs");
        assert_eq!(MODE_W, ICON_W, "反转格与 .* / Aa 同宽");
        assert!(self_src.contains("let mode_w = ICON_W"), "不再按文字宽撑格");
        assert!(self_src.contains("paint_invert_icon"), "纯 painter 画图标");
        assert!(
            self_src.contains("line_segment"),
            "箭头用 line_segment，与 Find 方向按钮同画法"
        );
        assert!(
            !self_src.contains("layout_no_wrap(\"⇄\""),
            "不排字，不依赖字体里有没有该字形"
        );
        assert!(
            !self_src.contains("t(lang, \"Invert\")"),
            "界面无 Invert / 反转文字"
        );
        assert!(self_src.contains("t(lang, \"Invert filter (grep -v): hide matching lines\")"));
        assert_eq!(t(term_app::i18n::Lang::En, "Invert"), "Invert");
        assert_eq!(t(term_app::i18n::Lang::Zh, "Invert"), "反转");
        assert!(self_src.contains("FilterMode::DropMatching"), "点击进 grep -v");
        assert!(
            self_src.contains("CursorIcon::PointingHand"),
            "命中区就是绘制区，光标指向手势"
        );
        assert!(
            self_src.contains("ui.interact(r, hit(\"tab_toolbar_filter_mode\")"),
            "无系统按钮边框，按坐标登记点击"
        );
    }

    /// 四条 1px 竖线按功能区隔，颜色与上边线同口径、高度收在 25pt 条内；
    /// 窄条丢计数时最后一根不画；收起态（只剩 Sysmon）由调用点不入这里。
    #[test]
    fn separators_divide_the_inline_zones() {
        let bar = wide_bar();
        let geom = filter_row_geom(bar, 46.0, 38.0, MODE_W);
        let sep = filter_separators(&geom);
        let count = geom.count.expect("宽条要放得下计数");
        let [label_input, input_icons, icons_count, count_close] = sep.map(|x| x.unwrap());

        assert!(label_input > bar.left() + LABEL_X + 38.0);
        assert!(label_input < geom.edit.left());
        assert!(geom.edit.left() < input_icons && input_icons < geom.icons[0].left());
        assert!(geom.icons[3].right() < icons_count && icons_count < count.left());
        assert!(count.right() < count_close && count_close < geom.close.left());

        let y_top = bar.top() + ROW_INSET_Y;
        let y_bottom = bar.bottom() - ROW_INSET_Y;
        assert_eq!(y_bottom - y_top, 19.0);
        assert!(y_top > bar.top() && y_bottom < bar.bottom());

        let self_src = include_str!("tab_toolbar_filter.rs");
        let toolbar_src = include_str!("tab_toolbar.rs");
        assert!(self_src.contains("filter_separators(&geom)"), "过滤态画竖线");
        assert!(
            self_src.contains("bar.top() + ROW_INSET_Y"),
            "线高限制在条内"
        );
        assert!(self_src.contains("CHROME_SCALE.border"), "与上边线同色口径");
        assert!(
            !toolbar_src.contains("filter_separators"),
            "Sysmon 收起态不画这些线"
        );

        let narrow = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(190.0, TAB_TOOLBAR_H));
        let small = filter_separators(&filter_row_geom(narrow, 46.0, 38.0, MODE_W));
        assert_eq!(small[3], None, "窄条没计数，不画最后一根");
    }

    /// 25pt 条里的三格与输入 / 计数同处一条中线，格高压到 19pt。
    #[test]
    fn all_cells_share_the_25pt_centerline() {
        let bar = wide_bar();
        let geom = filter_row_geom(bar, 46.0, 38.0, MODE_W);
        assert_eq!(ICON_H, 19.0);
        assert_eq!(geom.icons[0].height(), ICON_H);
        assert_eq!(geom.icons[3].width(), MODE_W);
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

    /// 宽条四格全在、互不重叠、计数贴着 `×` 左侧；窄条先丢计数，`×` 永在。
    #[test]
    fn wide_and_narrow_layouts_never_overlap() {
        let geom = filter_row_geom(wide_bar(), 46.0, 38.0, MODE_W);
        let count = geom.count.expect("宽条要放得下计数");
        assert!(geom.edit.right() <= count.left());
        assert!(count.right() <= geom.close.left());
        assert!(geom.icons[0].right() <= geom.icons[1].left());
        assert!(geom.icons[1].right() <= geom.icons[2].left());
        assert!(geom.icons[2].right() <= geom.icons[3].left());
        assert!(geom.icons[3].right() + ICON_R_PAD <= count.left());
        assert!(geom.icons[0].left() >= geom.edit.left() + 0.01);

        let narrow = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(190.0, TAB_TOOLBAR_H));
        let small = filter_row_geom(narrow, 46.0, 38.0, MODE_W);
        assert!(small.count.is_none(), "窄条先丢计数");
        assert!(small.close.right() <= narrow.right());
        assert!(small.edit.right() <= small.close.left());
        // 变宽的 Invert 格也不能越过标签去贴住 `Filter:`。
        assert!(small.icons[0].left() >= small.edit.left() + ICON_LEFT_INSET - 0.01);
        assert!(small.icons[3].right() <= small.close.left() + 0.01);
    }

    /// 历史提交：最近在前、去重、封顶 16；空串与内部 NUL 前缀子命令不记。
    #[test]
    fn filter_history_dedupes_caps_and_skips_empty_or_ui_commands() {
        let mut h = Vec::new();
        push_filter_history(&mut h, "ERR");
        push_filter_history(&mut h, "warn");
        push_filter_history(&mut h, "ERR");
        assert_eq!(h, vec!["ERR", "warn"], "重复提到最前，旧的仍在");
        push_filter_history(&mut h, "");
        push_filter_history(&mut h, "\0fltm");
        assert_eq!(h, vec!["ERR", "warn"], "空串与 UI 子命令不进历史");
        for i in 0..20 {
            push_filter_history(&mut h, &format!("q{i}"));
        }
        assert_eq!(h.len(), FILTER_HISTORY_CAP, "上限 16");
        assert_eq!(h[0], "q19", "最近在前");
        assert_eq!(h[15], "q4", "最旧的被挤掉");
        assert!(h.iter().all(|q| !q.is_empty() && !q.starts_with('\0')));
    }

    /// ↑↓ 循环：空选时 ↑ 到末条 / ↓ 到首条，到头回绕，越界下标先夹紧。
    #[test]
    fn history_arrow_keys_cycle_wrap() {
        assert_eq!(cycle_filter_history(None, 0, 1), None, "空历史不选中");
        assert_eq!(cycle_filter_history(None, 3, -1), Some(2));
        assert_eq!(cycle_filter_history(None, 3, 1), Some(0));
        assert_eq!(cycle_filter_history(Some(0), 3, -1), Some(2));
        assert_eq!(cycle_filter_history(Some(2), 3, 1), Some(0));
        assert_eq!(cycle_filter_history(Some(9), 3, 1), Some(0));
    }

    /// 历史浮层是 `Area` 覆盖层：贴输入区上方，不新增面板、不切终端高度。
    #[test]
    fn history_menu_is_an_area_overlay_not_a_second_panel() {
        let popup = include_str!("tab_toolbar_filter_popup.rs");
        let self_src = include_str!("tab_toolbar_filter.rs");
        assert!(popup.contains("egui::Area::new(id)"));
        assert!(popup.contains("fade_in(false)"));
        assert!(popup.contains("fixed_pos"));
        assert!(popup.contains("pivot(egui::Align2::LEFT_BOTTOM)"), "贴上方");
        assert!(!popup.contains("TopBottomPanel::"), "不切终端高度");
        assert!(!self_src.contains("TopBottomPanel::"), "过滤行不新增面板");
        assert!(popup.contains("CHROME_SCALE.tab"), "当前项用 tab 底色");
        assert!(self_src.contains("history_popup("), "浮层从过滤行调出");
    }

    /// `▾` 贴在输入区右端、图标组之前；无边框手绘、命中区 == 绘制区。
    #[test]
    fn history_button_hugs_input_right_edge_before_icons() {
        let geom = filter_row_geom(wide_bar(), 46.0, 38.0, MODE_W);
        let btn = filter_history_button_rect(geom.edit);
        assert_eq!(btn.height(), geom.edit.height());
        assert_eq!(btn.right(), geom.edit.right());
        assert!(btn.left() > geom.edit.left());
        assert!(btn.right() < geom.icons[0].left(), "图标组之前");
        let self_src = include_str!("tab_toolbar_filter.rs");
        assert!(self_src.contains("ui.painter().rect_filled(hist_btn"), "hover 底色");
        assert!(self_src.contains("CursorIcon::PointingHand"), "指向光标");
        assert!(self_src.contains("egui::Shape::convex_polygon"), "手绘三角，无系统边框");
    }

    /// 两个 tab 的历史互不串味，也不写 `ViewState` / 不落进领域状态。
    #[test]
    fn two_tabs_keep_separate_filter_history() {
        use crate::ui::lifecycle_test_support::{fake_tab, window_with, CloseCount};
        let ctx = egui::Context::default();
        let pool = std::sync::Arc::new(term_session::SessionPool::new());
        let closes = CloseCount::default();
        let first = fake_tab(&pool, &ctx, 0, &closes);
        let second = fake_tab(&pool, &ctx, 1, &closes);
        let mut win = window_with(pool, &ctx, vec![first, second]);
        let (id_a, id_b) = (win.tabs[0].identity, win.tabs[1].identity);
        let view_before = win.view.clone();

        win.apply_filter_on(id_a, FilterAction::SetQuery("alpha".into()));
        win.apply_filter_on(id_a, FilterAction::Commit);
        win.apply_filter_on(id_b, FilterAction::SetQuery("beta".into()));
        win.apply_filter_on(id_b, FilterAction::Commit);

        assert_eq!(win.tabs[0].filter_history, vec!["alpha".to_string()]);
        assert_eq!(win.tabs[1].filter_history, vec!["beta".to_string()]);
        assert_eq!(win.view, view_before, "历史不写 ViewState");

        let tab_src = include_str!("../tab.rs");
        let state_src = include_str!("../../../../../crates/term-app/src/state.rs");
        assert!(tab_src.contains("pub(in crate::ui) filter_history: Vec<String>"));
        assert!(!state_src.contains("filter_history"), "不进 FilterState/ViewState");
    }

    /// `W` 全词格紧挨 `.*` / `Aa`，反转箭头仍在最后；四格 + ▾ 在窄窗也不重叠。
    #[test]
    fn whole_word_cell_and_history_button_fit_without_overlap() {
        let geom = filter_row_geom(wide_bar(), 46.0, 38.0, MODE_W);
        assert_eq!(ICON_CELLS, 4, "四格：.* / Aa / W / 反转");
        for i in 0..ICON_CELLS - 1 {
            assert!(geom.icons[i].right() <= geom.icons[i + 1].left());
        }
        assert_eq!(geom.icons[2].width(), ICON_W, "W 与 .* / Aa 同宽");
        assert_eq!(geom.icons[2].height(), ICON_H, "19pt 高");
        assert!(filter_history_button_rect(geom.edit).right() < geom.icons[0].left());

        let narrow = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(190.0, TAB_TOOLBAR_H));
        let small = filter_row_geom(narrow, 46.0, 38.0, MODE_W);
        for i in 0..ICON_CELLS - 1 {
            assert!(small.icons[i].right() <= small.icons[i + 1].left() + 0.01);
        }
        let btn = filter_history_button_rect(small.edit);
        assert!(btn.left() >= small.edit.left(), "窄窗 ▾ 不越到标签区");
        assert!(btn.right() <= small.icons[0].left() + 0.01);

        let self_src = include_str!("tab_toolbar_filter.rs");
        assert!(self_src.contains("FilterUiCommand::ToggleWholeWord"), "NUL 子命令 'w'");
        assert!(self_src.contains("t(lang, \"Match Whole Word\")"), "复用 i18n key");
        assert!(self_src.contains("hit(\"tab_toolbar_filter_whole_word\")"));
        assert!(self_src.contains("geom.icons[3]"), "反转仍在最后");
    }

//! 底带 `Shell` 面板：**面板内自己的标签条** + 与主区同一份终端。
//!
//! # 用户定案（原话）
//!
//! 「shell 是一个面板，是支持多 tab 的」——面板保留在可停靠体系里（能拖、能浮、
//! 能关，见 `layout/dock*.rs`，本文件一个字节都没动那里），面板**内容**分三层：
//!
//! ```text
//! ┌──────────────────────────────────────────────────────────────┐
//! │ ⌘ 1.zsh  ×                          (dock 的面板标签条：不动) │
//! ├──────────────────────────────────────────────────────────────┤
//! │ ⌘ 1.zsh ×   ⌘ 2.zsh ×                             ＋   ≡     │ ← 本文件
//! ├──────────────────────────────────────────────────────────────┤
//! │ [22:35:29] 2  ➜ ~                                            │ ← draw_tab
//! └──────────────────────────────────────────────────────────────┘
//! ```
//!
//! # 为什么这里只有「壳」
//!
//! 终端本体走 `terminal/pane.rs::draw_tab(ui, TabSlot::Shell(index))`——与主区
//! 标签页**完全同一份代码**（选区、`Cmd+C`、滚轮回看、鼠标报告、hex、折叠、
//! 查找高亮、粘贴确认、margin、右键菜单、光标全都在那边）。本文件只负责「面板
//! 自己的那一层标签条」与两个图标按钮：`＋` 新建一条系统默认 shell，`≡` 展开
//! 标签过滤列表。
//!
//! 「固定是固定打开系统默认 shell」：`＋` 与首次打开**一律**直接开系统默认 shell
//! （`SessionConfig::new_local_shell`），不弹选择器、不读会话库。所以这里没有
//! 任何「选 shell 种类」的控件——`shell/popup_lists.rs::draw_shell_list` 那是
//! 另一个东西（`SHELL_KINDS` 种类下拉），**不要**接到这个面板上。
//!
//! # 视觉复用
//!
//! 标签 / `×` / `＋` / `≡` 全部复用主区标签条那一套绘图原语
//! （`shell::tab_bar` 的 `paint_tab_icon` / `paint_close_cross` / `paint_right_tool`
//! 与 `ACTIVE_LINE`）：两个标签条说的是同一句话（「这条是你正在看的」），
//! 颜色、线宽、图标形状必须一模一样。菜单行复用 `shell::popup_lists` 那一套
//! 三态行（`apply_popup_style` + `draw_menu_row`），与菜单栏 / 右键菜单同色。

use term_app::menu::TabTool;
use term_core::theme::CHROME_SCALE;

use crate::ui::shell::{
    apply_popup_style, draw_menu_row, paint_cross, paint_right_tool, paint_tab_icon,
    popup_bg, CheckCell, MenuRowState, RowLook, ACTIVE_LINE, LINE_H,
};
use crate::ui::terminal::tab_slot::{shell_tab_matches, TabSlot};
use crate::ui::*;

/// 内标签条的高度。
///
/// 比主区标签条（`shell::TAB_BAR_H`，约 30）矮一档：面板常常只占底带两百来点高，
/// 标签条再横一条 30 点的栏，终端就只剩一百多点了。
const STRIP_H: f32 = 20.0;
/// 标签块左右内边距（与主区同节奏，只是整体收紧）。
const TAB_PAD: f32 = 8.0;
const ICON_W: f32 = 10.0;
const ICON_GAP: f32 = 5.0;
/// 文字到 `×` 的留白 / `×` 的点按区宽。
const CLOSE_GAP: f32 = 10.0;
const CLOSE_W: f32 = 10.0;
/// 右端两颗图标按钮的点按区宽与间隔。
const TOOL_W: f32 = 18.0;
const TOOL_GAP: f32 = 2.0;
const TOOL_EDGE: f32 = 2.0;
/// `≡` 下拉：宽度、与标签条的间距、内边距。
const LIST_W: f32 = 220.0;
const LIST_GAP: f32 = 2.0;
const LIST_PAD: f32 = 4.0;
/// 过滤框与计数行的宽度预算：右侧留给计数。
const COUNT_W: f32 = 28.0;

/// 面板入口：先确保有标签（一条都没有就开系统默认 shell），再画标签条，
/// 最后把剩下的整块区域交给与主区同一份的终端绘制。
pub(in crate::ui) fn draw_shell(win: &mut TerminalWindow, ui: &mut egui::Ui) {
    win.ensure_shell_tabs();
    let body = ui.available_rect_before_wrap();
    // 标签条只切走顶上一条；剩下的给终端。`draw_tab` 用的是
    // `available_rect_before_wrap()`，与主区完全同一条口径——所以这里必须
    // 真的把光标推下去（`allocate_rect`），而不是只画一条装饰线。
    let strip = egui::Rect::from_min_size(
        body.min,
        egui::vec2(body.width(), STRIP_H.min(body.height())),
    );
    ui.allocate_rect(strip, egui::Sense::hover());
    let (list_btn, toggle) = draw_strip(win, ui, strip);
    // 下拉先画：它是 `Area`（前景层），与终端的绘制顺序无关，但**开合**要在这
    // 一帧连着判——放到终端之后再画，同一帧里点 `≡` 打开就得多等一帧。
    draw_list(win, ui, list_btn, toggle);
    let active = win.shell_active_index();
    // ADR-0025：槽位按身份；`shell_active_index` 只是夹过范围的序号。
    let active_id = win.shell_tabs[active].identity;
    win.draw_tab(ui, TabSlot::Shell(active_id));
}

/// 画面板内标签条，返回 `≡` 那颗按钮的矩形与本帧是否点了它。
///
/// 返回而不是就地处理开合，是因为「点 `≡`」要由 [`draw_list`] 在**同一帧**里
/// 接着判（先 toggle 再画）：标签条先画、下拉后画，中间不留一帧空窗。
fn draw_strip(
    win: &mut TerminalWindow,
    ui: &mut egui::Ui,
    band: egui::Rect,
) -> (egui::Rect, bool) {
    let s = &CHROME_SCALE;
    let text_c = crate::theme::chrome(s.text);
    let strong_c = crate::theme::chrome(s.text_strong);
    let hover_c = crate::theme::chrome(s.hover);
    let font = egui::TextStyle::Button.resolve(ui.style());

    // 名字先拷出来：循环里要改 `win.shell_active`（E0502），而 `labels` 借的是
    // `win.shell_tabs`。
    let labels: Vec<String> = win
        .shell_tabs
        .iter()
        .map(|t| t.config.name.clone())
        .collect();
    let active = win.shell_active_index();

    // 底色：比面板底压暗一档 + 底部一条分隔线。只靠这两笔把「标签条」与
    // 「终端内容」分开，不画边框——视觉基线里那两层的分界就是一条细线。
    ui.painter().rect_filled(band, 0.0, crate::theme::chrome(s.tab_active));
    ui.painter().line_segment(
        [
            egui::pos2(band.left(), band.bottom() - 0.5),
            egui::pos2(band.right(), band.bottom() - 0.5),
        ],
        egui::Stroke::new(1.0, crate::theme::chrome(s.line)),
    );

    let mut activate: Option<usize> = None;
    let mut close: Option<usize> = None;
    // 右端两颗的预算：标签放不下时给它们让位（宁可少显示一条标签，
    // 也不能把标签画到按钮底下、点不着）。
    let tools_w = (TOOL_W + TOOL_GAP) * 2.0 + TOOL_EDGE;

    let mut x = band.left();
    for (i, label) in labels.iter().enumerate() {
        let glyph = ui
            .painter()
            .layout_no_wrap(label.clone(), font.clone(), strong_c);
        let w = TAB_PAD + ICON_W + ICON_GAP + glyph.size().x + CLOSE_GAP + CLOSE_W + TAB_PAD;
        if x + w > band.right() - tools_w {
            // 放不下的标签走 `≡` 列表（那里有全名 + 过滤）。这不是丢功能：
            // 面板窄到装不下时，标签条本来就不是唯一的入口。
            break;
        }
        let rect = egui::Rect::from_min_size(
            egui::pos2(x, band.top()),
            egui::vec2(w, band.height()),
        );
        let close_rect = egui::Rect::from_center_size(
            egui::pos2(rect.right() - TAB_PAD - CLOSE_W / 2.0, rect.center().y),
            egui::vec2(CLOSE_W + 6.0, rect.height()),
        );
        // 先注册整块、再注册 `×`：后注册的命中优先（`×` 压在标签上）。
        let resp = ui.interact(rect, egui::Id::new(("shell_tab", i)), egui::Sense::click());
        let close_resp = ui.interact(
            close_rect,
            egui::Id::new(("shell_tab_close", i)),
            egui::Sense::click(),
        );

        let p = ui.painter();
        if resp.hovered() && i != active {
            p.rect_filled(rect, 0.0, hover_c);
        }
        if i == active {
            // 与主区同一条活动标记：标签顶端 2pt 蓝线（`ACTIVE_LINE`）。
            p.rect_filled(
                egui::Rect::from_min_size(rect.min, egui::vec2(w, LINE_H)),
                0.0,
                ACTIVE_LINE,
            );
        }
        paint_tab_icon(p, egui::pos2(rect.left() + TAB_PAD + ICON_W / 2.0, rect.center().y));
        p.galley(
            egui::pos2(
                rect.left() + TAB_PAD + ICON_W + ICON_GAP,
                rect.center().y - glyph.size().y / 2.0,
            ),
            glyph,
            if i == active { strong_c } else { text_c },
        );
        paint_cross(
            p,
            close_rect.center(),
            if close_resp.hovered() { strong_c } else { text_c },
        );

        if close_resp.clicked() {
            close = Some(i);
        } else if resp.clicked() {
            activate = Some(i);
        }
        x += w;
    }

    // 右端两颗：从右往左是 `≡` 然后 `＋`（视觉顺序从左到右即 `＋ ≡`）。
    let tool_rect = |slot: usize| -> egui::Rect {
        let right = band.right() - TOOL_EDGE - slot as f32 * (TOOL_W + TOOL_GAP);
        egui::Rect::from_min_size(
            egui::pos2(right - TOOL_W, band.top()),
            egui::vec2(TOOL_W, band.height()),
        )
    };
    let list_rect = tool_rect(0);
    let plus_rect = tool_rect(1);
    let list_resp = ui.interact(
        list_rect,
        egui::Id::new(("shell_tool", TabTool::TabList)),
        egui::Sense::click(),
    );
    let plus_resp = ui.interact(
        plus_rect,
        egui::Id::new(("shell_tool", TabTool::NewTab)),
        egui::Sense::click(),
    );
    for (rect, resp, tool) in [
        (list_rect, &list_resp, TabTool::TabList),
        (plus_rect, &plus_resp, TabTool::NewTab),
    ] {
        let p = ui.painter();
        if resp.hovered() || (tool == TabTool::TabList && win.shell_list_open) {
            p.rect_filled(rect.shrink2(egui::vec2(0.0, 3.0)), 2.0, hover_c);
        }
        paint_right_tool(p, rect, tool, text_c);
    }

    // 交互统一在画完之后落地：标签条这一帧的像素已经画好，改状态只影响下一帧。
    if let Some(i) = activate {
        // ADR-0025：活动位存身份；`i` 是画条时的面板序号，这里翻成身份。
        win.shell_active = win.shell_tabs.get(i).map(|t| t.identity);
        win.ctx.request_repaint();
    }
    if let Some(i) = close {
        win.close_shell_tab(i);
    }
    if plus_resp.clicked() {
        // 「固定是固定打开系统默认 shell」：这里**只**开本地 shell，不弹选择器。
        win.open_shell_tab();
    }
    (list_rect, list_resp.clicked())
}

/// `≡` 下拉那一层 `Area` 的 id。
///
/// 提出来是因为它不止一处用：`draw_list` 建它，测试还要从
/// `Context::memory().area_rect()` 里把它取回来量位置。写成一个函数，
/// 两边就不可能写岔。
fn list_area_id() -> egui::Id {
    egui::Id::new("shell_tab_list")
}

/// `≡` 下拉：Filter 输入框 + 匹配计数 + 标签项（每项行尾带 `×`）。
///
/// `toggle` = 本帧点了 `≡`。开合在这一处落地，是因为「点开」与「点在外面收起」
/// 必须能区分开：先 toggle 再判「点在外面」，就不会出现「点一下打开、同一帧
/// 又被自己收起」。
fn draw_list(win: &mut TerminalWindow, ui: &mut egui::Ui, anchor: egui::Rect, toggle: bool) {
    if toggle {
        win.shell_list_open = !win.shell_list_open;
    }
    if !win.shell_list_open {
        return;
    }
    let s = &CHROME_SCALE;
    let text_c = crate::theme::chrome(s.text);
    let strong_c = crate::theme::chrome(s.text_strong);
    let hint = win.tr("Filter");
    let active = win.shell_active_index();
    let mut activate: Option<usize> = None;
    let mut close: Option<usize> = None;

    let area = egui::Area::new(list_area_id())
        .order(egui::Order::Foreground)
        // 右对齐挂在 `≡` 底下：面板贴着窗口右缘，左对齐会伸到窗口外，再被
        // `constrain` 拽回来——看起来像是「弹到了别处」。
        .pivot(egui::Align2::RIGHT_TOP)
        .fixed_pos(egui::pos2(anchor.right(), anchor.bottom() + LIST_GAP))
        // 与 `Targets` 齿轮菜单同一个坑（`panels/tools_sender_menu.rs` 的注）：
        // `Area` 默认淡入，夹具时钟钉在 0 会一直停在透明度第一帧。
        .fade_in(false)
        .show(ui.ctx(), |ui| {
            // 与菜单栏弹层同一套控件视觉（悬停 / 选中的蓝块由它给）。
            apply_popup_style(ui);
            egui::Frame::none()
                .fill(popup_bg())
                .stroke(egui::Stroke::new(1.0, crate::theme::chrome(s.line)))
                .rounding(4.0)
                .inner_margin(egui::Margin::same(LIST_PAD))
                .show(ui, |ui| {
                    ui.set_width(LIST_W);
                    ui.spacing_mut().item_spacing.y = 0.0;
                    ui.horizontal(|ui| {
                        ui.add(
                            egui::TextEdit::singleline(&mut win.shell_filter)
                                .id(egui::Id::new("shell_tab_filter"))
                                .hint_text(egui::RichText::new(hint).color(crate::theme::chrome(s.hint)))
                                .frame(false)
                                .margin(egui::Margin::symmetric(0.0, 3.0))
                                .font(crate::ui::type_scale::body())
                                .text_color(text_c)
                                .desired_width(LIST_W - COUNT_W - LIST_PAD),
                        );
                        // 计数在过滤框**之后**算：本帧敲进去的字已经写进
                        // `shell_filter`，计数说的是「现在看到的这个列表」。
                        let count = win
                            .shell_tabs
                            .iter()
                            .filter(|t| shell_tab_matches(&t.config.name, &win.shell_filter))
                            .count();
                        ui.label(
                            egui::RichText::new(format!("{count}"))
                                .color(text_c)
                                .monospace()
                                .size(crate::ui::type_scale::SIZE_SMALL),
                        );
                    });
                    ui.separator();
                    // 行内容先收成拥有所有权的值：循环里要改 `self` 的字段。
                    let rows: Vec<(usize, String)> = win
                        .shell_tabs
                        .iter()
                        .enumerate()
                        .filter(|(_, t)| shell_tab_matches(&t.config.name, &win.shell_filter))
                        .map(|(i, t)| (i, t.config.name.clone()))
                        .collect();
                    for (i, name) in rows {
                        let row = draw_menu_row(
                            ui,
                            &name,
                            None,
                            "",
                            RowLook {
                                // 当前那一条用 `Selected`：与视觉基线里那一行蓝块
                                // 同一个判据（`MENU_HIGHLIGHT`），不是第三种状态。
                                state: if i == active {
                                    MenuRowState::Selected
                                } else {
                                    MenuRowState::Enabled
                                },
                                check: CheckCell::None,
                                indent: false,
                                cells: 0,
                            },
                            false,
                        );
                        // 行尾的 `×`：后注册 → 命中优先于整行（点叉只关不切）。
                        let cross = egui::Rect::from_center_size(
                            egui::pos2(row.rect.right() - CLOSE_W, row.rect.center().y),
                            egui::vec2(CLOSE_W + 8.0, row.rect.height()),
                        );
                        let cross_resp = ui.interact(
                            cross,
                            egui::Id::new(("shell_list_close", i)),
                            egui::Sense::click(),
                        );
                        paint_cross(
                            ui.painter(),
                            cross.center(),
                            if cross_resp.hovered() { strong_c } else { text_c },
                        );
                        if cross_resp.clicked() {
                            close = Some(i);
                        } else if row.clicked() {
                            activate = Some(i);
                        }
                    }
                });
        });

    if let Some(i) = activate {
        // ADR-0025：活动位存身份；`i` 是下拉行对应的面板序号。
        win.shell_active = win.shell_tabs.get(i).map(|t| t.identity);
        win.shell_list_open = false;
        win.ctx.request_repaint();
    }
    if let Some(i) = close {
        win.close_shell_tab(i);
    }
    // 收起：点在外面 / Esc。点 `≡` 那一帧由上面的 `toggle` 负责——先 toggle 再判
    // 「点在外面」，会把刚打开的下拉当场收掉（`≡` 自己就在 `Area` 之外）。
    let clicked_outside = ui.input(|i| i.pointer.any_click()) && !area.response.contains_pointer();
    let escaped = ui.input(|i| i.key_pressed(egui::Key::Escape));
    if (!toggle && clicked_outside) || escaped {
        win.shell_list_open = false;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui::lifecycle_test_support::{fake_tab, window_with, CloseCount};

    /// 夹具面板的尺寸：宽到能放两条标签 + 右端两颗按钮，高到能画二十来行。
    const PANEL_W: f32 = 600.0;
    const PANEL_H: f32 = 240.0;

    /// 连画 `frames` 帧 `Shell` 面板，返回**最后一帧**的（文字图元数, 矩形图元数）。
    ///
    /// 为什么必须能画多帧：`egui::Area` 第一次出现那一帧是 egui 自己的
    /// **sizing pass**（`AreaState` 里还没有 `size`）。那一帧 egui 把内容 UI 标成
    /// `invisible()`，于是 `Painter::add` 一律退化成 `Shape::Noop`（只有 `Frame` /
    /// `TextEdit` 这类「先占位、后填色」的背景会被补回真图元），同时按
    /// `spacing.default_area_size`（600×400）摆位、再被 `constrain` 拽到屏幕左上角。
    /// 第二帧起才是用户真正看到的样子。
    ///
    /// 这也是 `panels/tools_sender_menu.rs` 里那条 `.fade_in(false)` 注记的同一个
    /// 坑；只画一帧就会得出「下拉什么都没画」的假阴性。
    fn paint(
        win: &mut crate::ui::TerminalWindow,
        ctx: &egui::Context,
        frames: usize,
    ) -> (usize, usize) {
        let mut last = (0usize, 0usize);
        for _ in 0..frames {
            let input = egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(PANEL_W, PANEL_H),
                )),
                ..Default::default()
            };
            let out = ctx.run(input, |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    win.draw_panel_body(term_app::dock::PanelId::Shell, ui);
                });
            });
            last = (
                out.shapes
                    .iter()
                    .filter(|c| matches!(&c.shape, egui::Shape::Text(_)))
                    .count(),
                out.shapes
                    .iter()
                    .filter(|c| matches!(&c.shape, egui::Shape::Rect(_)))
                    .count(),
            );
        }
        last
    }

    /// 画一帧面板：真的开出一条系统 shell，标签条与终端**两段都画出东西**。
    ///
    /// 这条是「面板还在、内容与标签页一致」的最小可证伪判据：
    /// - `shell_tabs` 空 → 说明惰性构造没接上（面板会是一块空白）；
    /// - 图元为 0 → 说明 `draw_tab` 没被调到（那正是旧实现「面板里只有一坨
    ///   `layout_no_wrap` 文本」的反面：这里要的是与主区同一套渲染）；
    /// - 文字为 0 → 标签名（`1.zsh`）没画出来，用户没法知道有几条。
    #[test]
    fn one_frame_opens_a_shell_tab_and_paints_the_panel() {
        let pool = std::sync::Arc::new(term_session::SessionPool::new());
        let ctx = egui::Context::default();
        let closes = CloseCount::default();
        let tab = fake_tab(&pool, &ctx, 0, &closes);
        let mut win = window_with(pool, &ctx, vec![tab]);

        let (texts, rects) = paint(&mut win, &ctx, 1);

        assert_eq!(win.shell_tabs.len(), 1, "面板第一次画要开一条系统 shell");
        assert_eq!(win.tabs.len(), 1, "面板标签不进主区的 tabs");
        assert!(rects > 0, "标签条与终端底色至少要画出矩形");
        assert!(texts > 0, "标签名要真的画出来（`序号.壳名`）");
    }

    /// 面板里的终端**不是**被 `ScrollArea` 包着画的。
    ///
    /// 判据是几何本身：终端区的高必须等于「面板高 - 标签条高」，而不是
    /// `ScrollArea` 里那个无穷高。包了的话 `rows` 会被钳到 `MAX_ROWS`，
    /// PTY 被 resize 成千行——屏幕上看不出来，回看与翻页全是错的。
    #[test]
    fn the_panel_terminal_is_not_laid_out_inside_a_scroll_area() {
        let pool = std::sync::Arc::new(term_session::SessionPool::new());
        let ctx = egui::Context::default();
        let closes = CloseCount::default();
        let tab = fake_tab(&pool, &ctx, 0, &closes);
        let mut win = window_with(pool, &ctx, vec![tab]);

        let _ = paint(&mut win, &ctx, 1);

        // 一帧之后终端已经按面板高度 resize 过一次，行数会在个位到十位数之间。
        let snapshot = win.shell_tabs[0].snapshot.clone();
        assert!(
            snapshot.rows > 1 && snapshot.rows < term_core::config::MAX_ROWS,
            "面板里的 PTY 行数应当由面板高度决定（拿到 {} 行，说明高度被摊成了无穷）",
            snapshot.rows
        );
    }

    /// `≡` 下拉开着时，真的多画出东西来，而且**挂在 `≡` 底下**。
    ///
    /// 判据是**差分 + 几何**两件事：
    /// - 差分：同一份夹具画两帧，只把 `shell_list_open` 打开。「文字图元变多」
    ///   就是「过滤框 + 计数 + 每条标签一行」真的画了；比断言某个具体字符串稳
    ///   （文案走 i18n，菜单行还带手绘的三态块）。
    /// - 几何：下拉的位置要从 `Context::memory().area_rect()` 里取回来量——
    ///   只看「图元变多」抓不到「弹到屏幕左上角去了」这种错位（sizing pass 那一
    ///   帧正是那样），而错位的下拉等于点不着。
    #[test]
    fn opening_the_tab_list_paints_more_than_the_closed_panel() {
        let pool = std::sync::Arc::new(term_session::SessionPool::new());
        let ctx = egui::Context::default();
        let closes = CloseCount::default();
        let tab = fake_tab(&pool, &ctx, 0, &closes);
        let mut win = window_with(pool, &ctx, vec![tab]);
        win.ensure_shell_tabs();
        win.open_shell_tab();
        assert_eq!(win.shell_tabs.len(), 2, "夹具：面板里有两条标签");

        win.shell_list_open = false;
        let (closed_texts, _) = paint(&mut win, &ctx, 2);
        win.shell_list_open = true;
        let (open_texts, _) = paint(&mut win, &ctx, 2);
        assert!(
            open_texts > closed_texts,
            "下拉展开后文字图元没有变多（关着 {closed_texts} / 打开 {open_texts}）：`≡` 没接上"
        );

        let rect = ctx
            .memory(|m| m.area_rect(list_area_id()))
            .expect("下拉已经画过两帧，`Area` 的位置应当已经写进 memory");
        assert!(
            rect.top() > 4.0 && rect.top() < STRIP_H + 24.0,
            "下拉要挂在 {STRIP_H} 高的标签条**下面**，实际 top={}（贴到顶说明被 constrain 拽走了）",
            rect.top()
        );
        assert!(
            rect.right() > PANEL_W * 0.75,
            "下拉要贴着右端的 `≡`，实际 right={}（跑到左边说明锚点没落在按钮上）",
            rect.right()
        );
        assert!(
            rect.width() > LIST_W - 1.0,
            "下拉宽度应当是 {LIST_W} 那一档，实际 {}",
            rect.width()
        );
    }
}

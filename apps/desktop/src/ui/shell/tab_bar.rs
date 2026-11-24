//! 标签条：无框标签 + 活动标签顶部蓝线 + 右端三件。
//!
//! # 与视觉基线的差距（2026-11-08 逐像素重做）
//!
//! 上一版用 `ui.selectable_label` + `ui.small_button("x")` + `ui.separator()`，
//! 画出来是一串**带框方块**，标签之间还有竖线，右端一整片空白。视觉基线里：
//!
//! ```text
//! ┌──────────────────────────────────────────────────────────┐
//! │ ▬▬▬▬▬▬▬▬  ← 活动标签顶部 2pt 蓝线 #007CD2                 │
//! │ ⌘  1.zsh                    ×              A   ＋   ≡      │
//! └──────────────────────────────────────────────────────────┘
//! ```
//!
//! 实测（2352×1532 @2× 视觉基线）：条底 `#1C1C1C`，活动标签底**与条底同色**
//! （不是 `#333333`），所以活动态只靠顶部 2pt 蓝线区分；蓝线 `#007CD2`
//! 与主题的 `focus` (`#1E90FF`) 不是同一个色，就近定义，不污染令牌。
//! 标签左侧那枚粉红四环 `#C48D8E` 是会话类型图标；`×` 是两条斜线、不是带框按钮。
//!
//! # 右端三件都有真实效果
//!
//! `A` 弹字体缩放菜单、`＋` 派发新建会话、`≡` 弹标签列表并切换活动标签。
//! 命令 ID 全部来自 `app/menu.rs`（`TAB_TOOLS` / `TAB_NEW_TAB_ID` / `ZOOM_ITEMS`），
//! 投影层一个字面量都不写。

use crate::ui::*;
use crate::ui::layout::pane_group_name;
use term_app::menu::{TabTool, TAB_NEW_TAB_ID, TAB_TOOLS};
use term_app::vocab::tmux_palette::RENAME_PANE_PROMPT_ID;
use term_core::theme::{CHROME_SCALE, ACCENT_BLUE};

/// 活动标签顶部那条线：视觉基线 y=122..125（2× 图）→ 2pt、`#007CD2`。
///
/// `pub(in crate::ui)`：底带 Shell 面板的内标签条用**同一条**活动标记——两个
/// 标签条长在两处、说的是同一句话（「这条是你正在看的」），色与高必须一致。
pub(in crate::ui) const ACTIVE_LINE: egui::Color32 =
    egui::Color32::from_rgb((ACCENT_BLUE >> 16) as u8, (ACCENT_BLUE >> 8) as u8, ACCENT_BLUE as u8);
/// 标签左侧四环图标：视觉基线最饱和像素 `#C48D8E`。
const TAB_ICON: egui::Color32 = egui::Color32::from_rgb(0xC4, 0x8D, 0x8E);
/// 活动标记的高度（两个标签条共用）。
pub(in crate::ui) const LINE_H: f32 = 2.0;
/// 标签块左右内边距。
const TAB_PAD: f32 = 10.0;
const ICON_W: f32 = 11.0;
const ICON_GAP: f32 = 6.0;
/// 文字到 `×` 的留白（视觉基线里 `1.zsh` 与 `×` 之间是空的，不是紧贴）。
const CLOSE_GAP: f32 = 14.0;
const CLOSE_W: f32 = 11.0;
/// 右端三件的点按区宽度与间隔。
const RIGHT_W: f32 = 18.0;
const RIGHT_GAP: f32 = 3.0;
const RIGHT_EDGE: f32 = 2.0;

/// 标题超过这么多**字符**才谈得上省略。
///
/// 终端字体等宽，字符数≈像素宽；24 个字符大约是视觉基线里 `1.zsh` 那种短标题的
/// 四倍，够看清版本号与目录名，又不至于让一条标签吃掉半条带。数字偏保守是有
/// 意的：省略是**用户显式选的**（出厂不省略，见 `Prefs.tab_elide`），选了的
/// 人想要的是「别再撑爆」，不是「能省则省」。
const TAB_TITLE_MAX_CHARS: usize = 24;

/// `Tabs ▸ Tab text elide mode:` 的四档，下标与 `Prefs.tab_elide` 一一对应。
/// 0 = 不省略 = 接线前的行为（`layout_no_wrap` 根本不截断）。
pub(in crate::ui) const ELIDE_MODES: [&str; 4] =
    ["None", "Elide left", "Elide right", "Elide middle"];

/// 标签编号的前缀。`mode` 与 `Prefs.tab_numbers` 一一对应（0/1/2），
/// `index` 是**本组内**的从 0 开始的位置。
///
/// 编号拼进标题再走同一条 `layout_no_wrap`，所以宽度自然变宽——不需要给
/// 「编号列」另算一套宽度算法（那正是这一行原来灰着的理由）。
pub(in crate::ui) fn tab_number_prefix(mode: usize, index: usize) -> String {
    let numbered = match mode {
        1 => index < 9,
        2 => true,
        _ => false,
    };
    if numbered {
        format!("{}. ", index + 1)
    } else {
        String::new()
    }
}

/// 官网 page/15：标签列表菜单里的 Filter 判据。抽成纯函数与 Shell 面板的
/// `shell_tab_matches` 同款口径——大小写不敏感的子串，空词不筛。
fn tab_title_matches(title: &str, filter: &str) -> bool {
    let needle = filter.trim().to_lowercase();
    needle.is_empty() || title.to_lowercase().contains(&needle)
}

/// 双击标签该派发什么。抽成纯函数是为了把判据钉在单测里：设置档决定
/// 「双击有没有动作」，当前标签的 tmux 控制模式决定「是关标签还是改窗格名」。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::ui) enum TabDoubleClick {
    None,
    CloseTab,
    RenamePane,
}

pub(in crate::ui) fn double_click_action(mode: usize, tmux_active: bool) -> TabDoubleClick {
    if mode != 1 {
        return TabDoubleClick::None;
    }
    if tmux_active {
        TabDoubleClick::RenamePane
    } else {
        TabDoubleClick::CloseTab
    }
}

impl TerminalWindow {
    pub(in crate::ui) fn draw_tabs(&mut self, ui: &mut egui::Ui) {
        let font = egui::TextStyle::Button.resolve(ui.style());
        // 先把标题与颜色收成拥有所有权的值：循环里还要改 `self.active_slot()`，
        // 右键菜单也要同时改 `self`（E0502）。
        let labels: Vec<(String, Option<egui::Color32>)> = self
            .tabs
            .iter()
            .enumerate()
            .map(|(i, tab)| {
                let title = if tab.is_live() {
                    if let Some(status) = tab.status().exit_status {
                        format!("{} [{}]", tab.config.name, status)
                    } else if !tab.snapshot.title.is_empty() {
                        tab.snapshot.title.clone()
                    } else {
                        tab.config.name.clone()
                    }
                } else {
                    format!("{}{}", tab.config.name, self.tr(" (disconnected)"))
                };
                (title, self.tab_color(i))
            })
            .collect();

        let band = ui.max_rect();
        let text_c = crate::theme::chrome(CHROME_SCALE.text);
        let strong_c = crate::theme::chrome(CHROME_SCALE.text_strong);
        let hover_c = crate::theme::chrome(CHROME_SCALE.hover);

        let mut x = band.left();
        let mut close_tab: Option<usize> = None;
        let mut rename_pane: Option<usize> = None;
        for (i, (title, color)) in labels.iter().enumerate() {
            let is_active = self.active_slot() == i;
            // 省略只改**画出来的文本**：宽度仍由下面同一个公式、从（截断后的）
            // 字符串量出来。这样标签跟着变窄，而不是「框还是那么宽、字被裁掉」。
            // 标签列表（右端 ≡）读的是 `labels` 里那份全名，不受影响。
            let shown = format!(
                "{}{}",
                tab_number_prefix(self.tab_numbers, i),
                elide_title(title, self.tab_elide)
            );
            let g = ui
                .painter()
                .layout_no_wrap(shown, font.clone(), strong_c);
            let text_w = g.size().x;
            let w = TAB_PAD + ICON_W + ICON_GAP + text_w + CLOSE_GAP + CLOSE_W + TAB_PAD;
            let rect = egui::Rect::from_min_size(
                egui::pos2(x, band.top()),
                egui::vec2(w, band.height()),
            );
            let cy = rect.center().y;
            let p = ui.painter();

            if is_active {
                p.rect_filled(
                    egui::Rect::from_min_size(rect.min, egui::vec2(w, LINE_H)),
                    0.0,
                    ACTIVE_LINE,
                );
            }
            // 四环会话图标：四个小圆环 + 中心一点，不赌字体里有没有 `⌘`。
            paint_tab_icon(p, egui::pos2(rect.left() + TAB_PAD + ICON_W / 2.0, cy));

            let text_x = rect.left() + TAB_PAD + ICON_W + ICON_GAP;
            p.galley(
                egui::pos2(text_x, cy - g.size().y / 2.0),
                g,
                color.unwrap_or(if is_active { strong_c } else { text_c }),
            );

            let close_cx = rect.right() - TAB_PAD - CLOSE_W / 2.0;
            let close = egui::Rect::from_center_size(
                egui::pos2(close_cx, cy),
                egui::vec2(CLOSE_W + 6.0, rect.height()),
            );
            // 先注册整块（单击激活 / 双击动作 + 右键菜单 + 拖拽源），再注册 `×`：后注册的命中优先。
            let resp = ui.interact(rect, egui::Id::new(("tab", i)), egui::Sense::click_and_drag());
            // egui 0.29 的双击判定就挂在 click sense 上（`double_clicked()` =
            // 本帧 clicked + 指针双击），不需要换一种 `Sense`。双击命中时不再
            // 顺带激活：`Close` 档的语义是「关掉它」，先把焦点切过去再关会
            // 让另一条标签闪一下。
            if resp.double_clicked() && self.tab_double_click == 1 {
                // 官网 page/3：tmux 控制模式下的双击是「改窗格名」，不是关标签。
                match double_click_action(self.tab_double_click, self.tabs[i].tmux.active) {
                    TabDoubleClick::CloseTab => close_tab = Some(i),
                    TabDoubleClick::RenamePane => rename_pane = Some(i),
                    TabDoubleClick::None => {}
                }
            } else if self.tab_middle_click == 1 && resp.middle_clicked() {
                // 中键的语义固定为「关闭」：动作集里只有这一项，不做另一个
                // 下拉。`middle_clicked` 与 `clicked` 正交，不会顺带激活。
                close_tab = Some(i);
            } else if resp.clicked() {
                self.set_active_slot(i);
            }
            // 标签本身也是拖拽源。按下移动 = 拖起这条会话，松手落在某片窗格上即
            // 与那片窗格交换（见 `draw_node`）。`dragged()` 在拖拽全程为真（含起手帧），
            // 每帧续上 `drag_session`；松手那帧 `dragged()` 为假、`drag_stopped()` 为真，
            // 置 `drag_released_this_frame` 交给帧末 flush 清拖拽态（若没被落点消费）。
            // 右键菜单不受影响：`resp.context_menu` 走 secondary 点击，与 click/drag 语义正交。
            if resp.drag_started() || resp.dragged() {
                self.drag_session = Some(self.tabs[i].identity);
            } else if resp.drag_stopped() {
                self.drag_released_this_frame = true;
            }
            // 悬停激活：同一条标签停够设置里的毫秒数才切；点击 / 拖动期间不抢。
            let identity = self.tabs[i].identity;
            if self.tab_thumbnail_pct > 0 && resp.hovered() && !resp.dragged() {
                let pct = self.tab_thumbnail_pct;
                crate::ui::tab_thumbnail::paint(
                    ui,
                    rect,
                    &self.tabs[i].view,
                    &self.palette,
                    pct,
                );
            }
            crate::ui::tab_hover::note_hover(
                self,
                ui,
                i,
                identity,
                resp.hovered(),
                is_active,
                resp.dragged(),
            );
            match self.tab_right_click {
                1 => {
                    if resp.secondary_clicked() {
                        close_tab = Some(i);
                    }
                }
                // 2 = 没有动作：连菜单都不注册，右键在标签上彻底静默。
                2 => {}
                // 0 = 标签菜单（接线前的行为）。
                _ => self.draw_tab_context_menu(&resp, i, is_active),
            }
            let close_resp = ui.interact(
                close,
                egui::Id::new(("tab_close", i)),
                egui::Sense::click(),
            );
            let cc = if close_resp.hovered() { strong_c } else { text_c };
            super::glyph::paint_cross(p, close.center(), cc);
            if close_resp.clicked() {
                close_tab = Some(i);
            }
            x += w;
        }

        // 右端三件：右对齐。表是**从左到右**的视觉顺序，从右往左走要 `.rev()`。
        // ADR-0026：第一件是 **group 名字**（`A/B/C/D`，每个 group 各不相同），
        // 不是字体缩放按钮；名字按 `PaneId` 恒在身份派生，跟着 group 走。
        // 单窗格时唯一那片 pane 是第一个铸造的 → `A`，与旧图逐像素一致。
        let group_name = self
            .pane_of_session(self.active.unwrap_or(0))
            .or_else(|| self.layout.leaf_panes().into_iter().next())
            .map(pane_group_name)
            .unwrap_or_else(|| "A".to_string());
        let mut right = band.right() - RIGHT_EDGE;
        let mut dispatch: Option<&'static str> = None;
        let mut activate: Option<usize> = None;
        for tool in TAB_TOOLS.iter().rev() {
            let rect = egui::Rect::from_min_size(
                egui::pos2(right - RIGHT_W, band.top()),
                egui::vec2(RIGHT_W, band.height()),
            );
            // 只有 `＋` 自己收点击；另两颗的点击由菜单按钮接管，这里只要 hover。
            let sense = if *tool == TabTool::NewTab {
                egui::Sense::click()
            } else {
                egui::Sense::hover()
            };
            let resp = ui.interact(rect, egui::Id::new(("tab_tool", *tool)), sense);
            {
                let p = ui.painter();
                if resp.hovered() {
                    p.rect_filled(rect.shrink2(egui::vec2(0.0, 3.0)), 2.0, hover_c);
                }
                if *tool == TabTool::GroupName {
                    // 逐像素保持旧图的「字母 A」位置与字色；只是内容由固定 A
                    // 换成该 group 的名字（单窗格恰好仍是 A，视觉基线不变）。
                    p.text(
                        rect.center(),
                        egui::Align2::CENTER_CENTER,
                        &group_name,
                        font.clone(),
                        text_c,
                    );
                } else {
                    paint_right_tool(p, rect, *tool, text_c);
                }
            }

            match tool {
                TabTool::NewTab => {
                    if resp.clicked() {
                        dispatch = Some(TAB_NEW_TAB_ID);
                    }
                }
                TabTool::GroupName => {
                    // 只读标识：兼容终端这里显示 group 名字，点它不会弹缩放菜单。
                    // 缩放三条命令在 `View ▸ Zoom` 有独立入口（`menu/tree.rs`），
                    // 与标签条无关（2026-11-15 用户裁定，修掉把字母当 Font 工具的误读）。
                }
                TabTool::TabList => {
                    let mut pick: Option<usize> = None;
                    let active = self.active_slot();
                    ui.scope_builder(egui::UiBuilder::new().max_rect(rect), |ui| {
                        egui::menu::menu_custom_button(
                            ui,
                            egui::Button::new("").frame(false).min_size(rect.size()),
                            |ui| {
                                ui.add(
                                    egui::TextEdit::singleline(&mut self.tab_list_filter)
                                        .hint_text("Filter sessions")
                                        .desired_width(160.0),
                                );
                                for (i, (title, _)) in labels.iter().enumerate() {
                                    if !tab_title_matches(title, &self.tab_list_filter) {
                                        continue;
                                    }
                                    if ui
                                        .selectable_label(i == active, title.as_str())
                                        .clicked()
                                    {
                                        pick = Some(i);
                                        ui.close_menu();
                                    }
                                }
                            },
                        );
                    });
                    if let Some(i) = pick {
                        activate = Some(i);
                    }
                }
            }
            right -= RIGHT_W + RIGHT_GAP;
        }

        if let Some(i) = activate {
            self.set_active_slot(i);
        }
        if let Some(id) = dispatch {
            self.dispatch(id);
        }
        if rename_pane.is_some() {
            // 与命令面板里的 `Mux: Rename Pane` 是同一个 ID、同一条落地路径：
            // 打开 tmux 面板并聚焦 Pane name 输入框。
            self.dispatch(RENAME_PANE_PROMPT_ID);
        }
        if let Some(i) = close_tab {
            // 标签栏 × 与菜单「Close」共用同一条关闭路径，右键
            // Reopen Closed 才能开回来。
            self.close_tab_with_history(i);
            self.save_layout();
        }
    }
}

/// 会话类型图标：四环 + 中心一点（视觉基线里标签左侧那枚）。
///
/// 抽成函数是因为**两个**标签条画的是同一枚图标：主区这条（`draw_tabs`）与底带
/// Shell 面板的内标签条（`panels/shell_panel.rs`）。两处各写一遍迟早就漂——
/// 颜色、半径、环宽三样里改漏一处，同一张脸上就会出现两种图标。
pub(in crate::ui) fn paint_tab_icon(p: &egui::Painter, center: egui::Pos2) {
    for (dx, dy) in [(-2.6, -2.6), (2.6, -2.6), (-2.6, 2.6), (2.6, 2.6)] {
        p.circle_stroke(
            egui::pos2(center.x + dx, center.y + dy),
            1.5,
            egui::Stroke::new(1.2, TAB_ICON),
        );
    }
    p.circle_filled(center, 1.1, TAB_ICON);
}

/// 右端三件的图形：`A` 是字母，`＋` / `≡` 走绘图原语。
///
/// `pub(in crate::ui)`：底带 Shell 面板的内标签条右端也是同一对 `＋` / `≡`，
/// 复用这两枚图形（含线宽与半长），面板那边不自己再画一遍。
pub(in crate::ui) fn paint_right_tool(
    p: &egui::Painter,
    r: egui::Rect,
    tool: TabTool,
    c: egui::Color32,
) {
    let center = r.center();
    let stroke = egui::Stroke::new(1.2, c);
    match tool {
        // group 名字由 `draw_tabs` 直接画（它才知道是哪个 group）；底带 Shell
        // 面板不画这一件，所以这里留空而不是画一个固定 `A`。
        TabTool::GroupName => {}
        TabTool::NewTab => {
            p.line_segment(
                [
                    egui::pos2(center.x - 5.0, center.y),
                    egui::pos2(center.x + 5.0, center.y),
                ],
                stroke,
            );
            p.line_segment(
                [
                    egui::pos2(center.x, center.y - 5.0),
                    egui::pos2(center.x, center.y + 5.0),
                ],
                stroke,
            );
        }
        TabTool::TabList => {
            for dy in [-4.0, 0.0, 4.0] {
                p.line_segment(
                    [
                        egui::pos2(center.x - 5.0, center.y + dy),
                        egui::pos2(center.x + 5.0, center.y + dy),
                    ],
                    stroke,
                );
            }
        }
    }
}

/// 按 `Tabs ▸ Tab text elide mode:` 把标题截到 [`TAB_TITLE_MAX_CHARS`] 个字符。
///
/// 三件必须说清的事：
/// - **按字符截，不按字节**：`CJK` 一个字三字节、emoji 四字节，按字节切会切出
///   半个码位，`String` 那一步就 panic（或者画出一个豆腐块）。
/// - **只在超长时才截**：短标题四档都一样——「Elide middle」不是一个会把
///   `1.zsh` 变成 `1…h` 的开关。
/// - **省略号自己占一格**：正文留 `MAX-1` 个字，所以总长仍然 ≤ 上限。
/// - `mode` 越界 / 0 = 不省略，与 `Prefs::tab_elide_mode` 的兜底同一条。
pub(in crate::ui) fn elide_title(title: &str, mode: usize) -> String {
    let chars: Vec<char> = title.chars().collect();
    if !matches!(mode, 1..=3) || chars.len() <= TAB_TITLE_MAX_CHARS {
        return title.to_string();
    }
    let keep = TAB_TITLE_MAX_CHARS - 1;
    match mode {
        1 => {
            let tail: String = chars[chars.len() - keep..].iter().collect();
            format!("…{tail}")
        }
        2 => {
            let head: String = chars[..keep].iter().collect();
            format!("{head}…")
        }
        // 中间档：前后各一半，省略号落在正中间——一眼能看出「这是同一个开头、
        // 同一个结尾」，比右侧省略更能认出是哪条会话。
        _ => {
            let head: String = chars[..keep / 2].iter().collect();
            let tail: String = chars[chars.len() - (keep - keep / 2)..].iter().collect();
            format!("{head}…{tail}")
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_short_title_is_never_touched_by_any_mode() {
        // 出厂档之外的三档也不能动短标题：golden 里的 `1.zsh` 一旦被省略，
        // 四张整窗基线会一起红，而那不是用户想要的「省略」。
        for mode in 0..ELIDE_MODES.len() {
            assert_eq!(elide_title("1.zsh", mode), "1.zsh", "mode={mode}");
        }
    }

    #[test]
    fn the_three_elide_modes_cut_on_the_right_side() {
        let long = "abcdefghijklmnopqrstuvwxyz0123456789";
        // 24 个字符的预算，省略号占一格 → 正文 23 个字：左省留尾、右省留头、
        // 中省前后 11 + 12（`keep/2` 向下取整，尾巴多一个）。
        assert_eq!(elide_title(long, 1), "…nopqrstuvwxyz0123456789");
        assert_eq!(elide_title(long, 2), "abcdefghijklmnopqrstuvw…");
        assert_eq!(elide_title(long, 3), "abcdefghijk…yz0123456789");
    }

    #[test]
    fn eliding_counts_characters_not_bytes() {
        // 30 个汉字 = 90 字节。按字节切会切碎码位（`String` 那一步 panic），
        // 按字符切出来必须正好是 24 个字符、且省略号前后的字仍是完整的。
        let cjk = "会".repeat(30);
        assert_eq!(cjk.len(), 90, "前提：一个字三字节");
        for mode in 1..=3 {
            let out = elide_title(&cjk, mode);
            assert_eq!(out.chars().count(), TAB_TITLE_MAX_CHARS, "mode={mode}");
            assert!(out.chars().any(|c| c == '…'), "mode={mode} 要画出省略号");
            // 左省略保留结尾、右省略保留开头、中省略两头都留。
            let keep_tail = matches!(mode, 1 | 3);
            let keep_head = matches!(mode, 2 | 3);
            assert_eq!(out.starts_with('会'), keep_head, "mode={mode}");
            assert_eq!(out.ends_with('会'), keep_tail, "mode={mode}");
        }
    }

    #[test]
    fn an_unknown_mode_means_no_elision() {
        // 手改 prefs 写了 9：与 0 一样原样画，不 panic、不猜一档。
        let long = "abcdefghijklmnopqrstuvwxyz0123456789";
        assert_eq!(elide_title(long, 9), long);
    }

    #[test]
    fn double_click_closes_a_plain_tab_and_renames_a_tmux_pane() {
        // 设置档是「Close tab」时，普通标签仍是关闭（老行为一字不改），
        // tmux 控制模式下的同一手势改走官网 page/3 的 Rename Pane 入口。
        assert_eq!(double_click_action(1, false), TabDoubleClick::CloseTab);
        assert_eq!(double_click_action(1, true), TabDoubleClick::RenamePane);
        assert_eq!(
            term_app::vocab::tmux_palette::TMUX_PALETTE
                .iter()
                .find(|(label, _, _)| *label == "Mux: Rename Pane")
                .map(|(_, id, _)| *id),
            Some(RENAME_PANE_PROMPT_ID)
        );
    }

    #[test]
    fn a_disabled_double_click_never_renames_or_closes() {
        for mode in [0, 2, 9] {
            assert_eq!(double_click_action(mode, false), TabDoubleClick::None, "mode={mode}");
            assert_eq!(double_click_action(mode, true), TabDoubleClick::None, "mode={mode}");
        }
    }

    #[test]
    fn tab_list_filter_is_a_case_insensitive_substring() {
        assert!(tab_title_matches("Local Shell", ""));
        assert!(tab_title_matches("Local Shell", " shell "));
        assert!(!tab_title_matches("My Server", "shell"));
    }

    #[test]
    fn tab_numbers_only_appear_when_the_mode_asks_for_them() {
        // 0 档 = 接线前的行为（一条编号都没有）；越界同样不画，与 elide 同款兜底。
        assert_eq!(tab_number_prefix(0, 0), "");
        assert_eq!(tab_number_prefix(9, 0), "");
        // 1 档只给前九个，第 10 个（下标 9）开始不编号。
        assert_eq!(tab_number_prefix(1, 0), "1. ");
        assert_eq!(tab_number_prefix(1, 8), "9. ");
        assert_eq!(tab_number_prefix(1, 9), "");
        // 2 档全编号，下标 9 起照样有。
        assert_eq!(tab_number_prefix(2, 9), "10. ");
    }
}

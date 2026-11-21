//! `Preferences` 里**部分真控件**的三页：`Text` / `Behaviour` / `ToolbarMenu`。
//!
//! # 什么叫「部分真控件」
//!
//! 这三页以前整页走 `settings_pages` 的数据表——所有控件一起灰显，因为一个
//! 字段都没有。这一轮每页只把**真有消费者**的那几行接成真控件，剩下的行照旧
//! 按数据表那套画（`settings_pages_view::draw_row` 的空框 + 灰字 + 悬停理由）。
//! 所以同一页里会同时出现「能点的勾」与「空框」，这是**故意的**，也是这三页
//! 与 `settings_live.rs` 那七页唯一的差别：那七页整页都真，这三页只真一半。
//!
//! 规矩没变：不许把灰行画成能点（假装有读者），也不许把真控件画成灰的
//! （假装没接线）；**灰行永远是空框**——勾代表「这项已生效」（ 的假勾选）。
//!
//! # 三页各自真在哪
//!
//! - `Text ▸ Alternate line background color` → `Prefs.text_alternate_row_bg`，
//!   读者是 `ui/terminal/view.rs::paint_rows`（按绝对行号的奇偶叠一层淡底色）。
//! - `Text ▸ Hide cursor when typing` → `Prefs.text_hide_cursor_while_typing`，
//!   读者是 `ui/terminal/pane.rs` 画光标那一段（最近 500ms 有输入就先不画）。
//! - `Behaviour ▸ Focus the new tab` → `Prefs.focus_new_tab`（出厂 `true`），
//!   读者是新标签的三个落地点（见 `ui/effects.rs::activate_new_tab`）。
//! - `Behaviour ▸ Confirm before closing multiple tabs` →
//!   `Prefs.confirm_close_multi_tabs`（出厂 `true`），读者是
//!   `confirm::should_confirm_close`——三条关闭路径共用那一个判据。
//! - `ToolbarMenu ▸ Show toolbar` → `ViewState.show_toolbar`，读者是
//!   `ui/mod.rs` 声明 `pane_tools` 那条面板的那一行。**它不落盘**（见页脚）。
//!
//! # 为什么这几个开关要活在窗口上
//!
//! 上面四个值在构造时从 `prefs.json` 读一次存到窗口（`caret_style` 同一写法），
//! 设置页改完写回 prefs 并同步那份副本。`paint_rows` 每行都要问一次「要不要
//! 叠底色」，`pane.rs` 每帧都要问一次「要不要藏光标」——这两处绝不能读盘。

use super::settings_live::save_prefs;
use super::settings_pages::Row;
use super::settings_pages_view::{draw_row, field_label, group_box, label_col, set_row_rhythm, spin_field};
use crate::ui::*;
use term_app::shell::ShellAction;

/// 页底说明：这一页哪几行真、哪几行还缺什么。
///
/// 必须点名，「尚未实现」四个字对下一个人没有信息量（与 `CARET_PAGE_NOTE`
/// 同一条要求）。
const TEXT_NOTE: &str = "Alternate line background, hiding the cursor while typing and the zoom \
     level apply immediately. Multiple carets, automatic copy-on-select and line/character spacing \
     have no field to bind yet — see each row's tooltip.";

/// `Behaviour` 页的说明。
///
/// 两个勾选都当场生效（下一次关闭 / 下一次开标签就读到新值）。灰的是另外两行：
/// 自动重连缺「重试策略 + 事件钩子」，启动那一行是给 `Application ▸ Startup`
/// 的指路牌。
const BEHAVIOUR_NOTE: &str = "Confirm before closing multiple tabs and focus the new tab both apply \
     immediately. Auto-reconnect and the startup row still have no field — see each row's tooltip.";

/// `Toolbar / Menu` 页的说明。
///
/// ：Preferences 的橙色页脚按兼容终端清空，「工具行开关不落盘」这条
/// 边界从页脚并进这里，仍然写在用户看得见的地方。
const TOOLBAR_NOTE: &str = "Show toolbar takes effect immediately, but the switch is not saved \
     (it is back on the next launch). There is no address bar in this build, and auto-hiding the \
     menu bar would need hover / Alt-key detection over the menu strip.";

/// 多光标： 已把编辑器语义删掉，连状态都没有。
const MULTI_CARET_NOTE: &str = "Multiple carets were removed with the editor semantics (R10); \
     there is no caret collection to store.";

/// 自动复制：会覆盖系统剪贴板，本轮不做。
const AUTO_COPY_NOTE: &str = "Copying on every selection would overwrite whatever else you have in \
     the system clipboard; copy stays explicit (Text ▸ Copy) in this round.";

/// 行距 / 字距：渲染用的是实测出来的单元格盒，没有「每行额外留白」这个概念。
const SPACING_NOTE: &str = "The grid renders on the measured cell box; there is no per-row or \
     per-cell padding to store, so these two rows have no consumer.";

/// 缩放（已接真）：底下就是 `Prefs.font_size` —— 与 Font 页的
/// `Font size` 同一个值，这里是它的「相对出厂档位」视图（0 = 出厂字号）。
/// 这条常量保留只是让测试能钉住「这页已无 ZOOM_NOTE 灰行」，绘制里不再用它。
const ZOOM_NOTE: &str = "Zoom level is the offset from the factory font size; it is the same \
     stored value as Preferences ▸ Font ▸ Font size.";

// 关多标签确认那一行原先共用一条 `CONFIRM_TABS_NOTE`（「没有这个粒度」）。
// 接上字段后它**删掉**了而不是留着：同一页上「已经能勾」与「没有这个粒度」
// 并存，正是 Tabs 页上次那份自相矛盾的说明。一页只能有一份说明。

/// 自动重连：需要重试策略（退避 / 次数上限）并订阅会话状态事件。
const RECONNECT_NOTE: &str = "Auto-reconnect would need a retry policy (backoff, attempt limit) \
     and a hook on the session status events; today reconnect is an explicit action.";

/// 恢复会话：已经实现，但在 Application 页。
const RESTORE_NOTE: &str = "Already implemented in Application ▸ Startup (three modes plus \
     Session 1–4). This row is a pointer, not a second field — change it there.";

/// 地址栏：这一版根本没有地址栏。
const ADDRESS_BAR_NOTE: &str = "There is no address bar anywhere in this build; the breadcrumb in \
     the toolbar is read-only text, not an editable address.";

/// 自动隐藏菜单栏：需要悬停 / Alt 探测。
const AUTO_HIDE_MENU_NOTE: &str = "Auto-hiding would need hover / Alt-key detection over the menu \
     strip; the menu bar is an always-on panel today.";

/// 一行**没有消费者**的控件：整行交给数据表的画法。
///
/// 不在这里另抄一份「灰勾选 / 灰下拉长什么样」——同一页里真控件与灰行必须一眼
/// 能分开，而「灰」的定义只有 `draw_row` 那一份实现。页面底部的说明也由它
/// 挂在悬停上，两处说法不会是两份。
fn disabled(ui: &mut egui::Ui, row: Row, note: &str, col: f32) {
    draw_row(ui, &row, note, col);
}

/// `Text` 页：`Features` 两个真勾选 + 三个仍灰显的分组。
///
/// 顺序照视觉基线（多光标 → 隔行底色 → 自动复制 → 打字隐光标）：真控件夹在灰行
/// 中间看着有点奇怪，但改顺序就等于改视觉基线的排布，而基线图是逐字节比的。
pub(in crate::ui) fn text(win: &mut TerminalWindow, ui: &mut egui::Ui) {
    let mut alt_row = win.alt_row_bg;
    let mut hide_cursor = win.hide_cursor_typing;
    let mut changed = false;

    // 视觉基线 `11-text.png`：内容区第一个字是组标题 `Features`，没有页级灰标题。
    // 标签列按本页最长的 `Character spacing:` 量，三个框的控件左缘因此对齐。
    let col = label_col(ui, &["Line spacing:", "Character spacing:", "Zoom level:"]);
    group_box(ui, Some("Features"), |ui| {
        set_row_rhythm(ui);
        disabled(ui, Row::DisabledCheck("Activate multiple carets"), MULTI_CARET_NOTE, col);
        changed |= ui
            .checkbox(&mut alt_row, "Alternate line background color")
            .changed();
        disabled(
            ui,
            Row::DisabledCheck("Automatically copy selection"),
            AUTO_COPY_NOTE,
            col,
        );
        changed |= ui
            .checkbox(&mut hide_cursor, "Hide cursor when typing")
            .changed();
    });

    ui.add_space(6.0);
    group_box(ui, Some("Spacing"), |ui| {
        set_row_rhythm(ui);
        disabled(
            ui,
            Row::Combo { label: "Line spacing:", value: "Automatic" },
            SPACING_NOTE,
            col,
        );
        disabled(
            ui,
            Row::Combo { label: "Character spacing:", value: "Automatic" },
            SPACING_NOTE,
            col,
        );
    });

    ui.add_space(6.0);
    group_box(ui, Some("Zoom"), |ui| {
        set_row_rhythm(ui);
        // ：Zoom level 接真。底下就是 `Prefs.font_size`（与 Font 页
        // `Font size` 同一个值），这里显示「相对出厂字号的档位」，出厂 = 0。
        // 改完当场 `fonts.set_size`（重装字形 + 重新量 cell），松手落 prefs。
        ui.horizontal(|ui| {
            field_label(ui, "Zoom level:", col);
            let mut level = win.fonts.size() - crate::fonts::TERMINAL_FONT_SIZE;
            // 字段框占满整行（视觉基线 `11-text.png` 的 Zoom 框从标签列一路到框右边）。
            let mut changed = false;
            spin_field(ui, |ui| {
                changed = ui
                    .add(
                        egui::DragValue::new(&mut level)
                            .range(-7.0..=35.0)
                            .speed(1.0)
                            .suffix(" levels"),
                    )
                    .changed();
            });
            if changed {
                let size = crate::fonts::TERMINAL_FONT_SIZE + level;
                win.fonts.set_size(size);
                win.settings.font_size = size;
                save_prefs(|prefs| prefs.font_size = size);
            }
        });
    });
    // 视觉基线没有页脚说明；灰行理由仍在各自的 hover 上。

    if changed {
        // 先改内存里的副本（本帧剩下的绘制立刻按新值走），再 load-modify-save。
        // 只写这两个位：语言 / 主题 / 光标样式 / 搜索模板原样保留。
        win.alt_row_bg = alt_row;
        win.hide_cursor_typing = hide_cursor;
        save_prefs(|prefs| {
            prefs.text_alternate_row_bg = alt_row;
            prefs.text_hide_cursor_while_typing = hide_cursor;
        });
    }
}

/// `Behaviour` 页：`Session` 两个真勾选 + 一行灰行，`Startup` 一行指路。
pub(in crate::ui) fn behaviour(win: &mut TerminalWindow, ui: &mut egui::Ui) {
    let mut focus_new = win.focus_new_tab;
    let mut multi = win.confirm.multi_tabs;

    // `Behaviour` / `Toolbar / Menu` 两页没有视觉基线，结构与其它页保持一致：
    // 内容区从组标题开始，不另画页级灰标题（那份标题与左栏选中项重复）。
    let col = label_col(ui, &[]);
    group_box(ui, Some("Session"), |ui| {
        set_row_rhythm(ui);
        // 真勾选：判据在 `confirm::should_confirm_close`，三处关闭共用。
        // 写回是**同一帧**的 `win.confirm.multi_tabs`——门控读的就是那份缓存，
        // 所以勾完立刻生效，不是「下次启动」。
        let multi_toggled = ui
            .checkbox(&mut multi, "Confirm before closing multiple tabs")
            .changed();
        let focus_toggled = ui
            .checkbox(&mut focus_new, "Focus the new tab after opening")
            .changed();
        disabled(
            ui,
            Row::DisabledCheck("Reconnect automatically when the connection drops"),
            RECONNECT_NOTE,
            col,
        );
        if multi_toggled {
            win.confirm.multi_tabs = multi;
            save_prefs(|prefs| prefs.confirm_close_multi_tabs = multi);
        }
        if focus_toggled {
            // 只在真的点了才回写：`win.focus_new_tab` 是闭包外那份副本，
            // 在这里改它等于每帧都「改一次」，会把 prefs 写成一堆无意义的 IO。
            win.focus_new_tab = focus_new;
            save_prefs(|prefs| prefs.focus_new_tab = focus_new);
        }
    });

    ui.add_space(6.0);
    group_box(ui, Some("Startup"), |ui| {
        set_row_rhythm(ui);
        disabled(
            ui,
            Row::DisabledCheck("Restore the previous session on launch"),
            RESTORE_NOTE,
            col,
        );
    });
    // 视觉基线没有页脚说明；灰行理由仍在各自的 hover 上。
}

/// `Toolbar / Menu` 页：`Show toolbar` 是真开关，其余两行仍灰。
///
/// 工具行在此之前**无条件绘制**，所以这个勾不勾都从来没有过「关掉」这条路；
/// 现在它走 `ShellAction::ToggleToolbar` 翻 `ViewState.show_toolbar`，同一帧
/// 下面那条 `pane_tools` 面板就不再声明（专注模式仍然无条件压掉它）。
///
/// 它**不落盘**：`layout_state.json` 只存标签 / 分屏 / 停靠位，菜单栏 / 状态栏
/// / 快捷栏那三个 `show_*` 也从不入盘，这里跟着它们——重启回默认（显示）。
/// 页内 [`TOOLBAR_NOTE`] 写明这条边界；要它跨重启记住，得先给那三个一起做
/// 持久化。（之前它挂在页脚，页脚已按兼容终端清空。）
pub(in crate::ui) fn toolbar_menu(win: &mut TerminalWindow, ui: &mut egui::Ui) {
    let mut show = win.view.show_toolbar;
    let mut toggled = false;
    // 本页只有勾选行，没有标签列（`col` 用兜底值）。
    let col = label_col(ui, &[]);

    group_box(ui, Some("Toolbar"), |ui| {
        set_row_rhythm(ui);
        toggled = ui.checkbox(&mut show, "Show toolbar").changed();
        disabled(
            ui,
            Row::DisabledCheck("Show address bar"),
            ADDRESS_BAR_NOTE,
            col,
        );
    });

    ui.add_space(6.0);
    group_box(ui, Some("Menu"), |ui| {
        set_row_rhythm(ui);
        disabled(
            ui,
            Row::DisabledCheck("Auto-hide the menu bar"),
            AUTO_HIDE_MENU_NOTE,
            col,
        );
    });
    // 视觉基线没有页脚说明；灰行理由仍在各自的 hover 上。

    if toggled {
        // `ViewState` 只由 reducer 写 ：这里派发动作，不直接赋值。
        // 勾选是「翻一下」，与 `ShellAction::ToggleToolbar` 的语义正好对齐。
        win.dispatch_action(Action::Shell(ShellAction::ToggleToolbar));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_text_note_separates_the_live_rows_from_the_dead_ones() {
        // 页底必须两件事都写：哪几行当场生效、哪几行没有字段可绑。
        assert!(TEXT_NOTE.contains("immediately"), "要写清哪些当场生效");
        assert!(TEXT_NOTE.contains("spacing"), "要点出仍然灰显的是哪几段");
        assert!(TEXT_NOTE.contains("zoom"), "缩放那一行也要点名");
    }

    #[test]
    fn zoom_level_is_now_a_live_row_over_font_size() {
        // ：Zoom level 不再是「没有字段」的灰行——它读写
        // `Prefs.font_size`，显示值与 Font 页的 Font size 是同一个底层值。
        // 页底不能再说 zoom「没有字段可绑」（否则是反向的谎）。
        assert!(!ZOOM_NOTE.contains("not stored"), "接真后不能再说缩放没存储");
        assert!(ZOOM_NOTE.contains("Font size"), "要说清它绑的是 Font 页字号字段");
        assert!(
            TEXT_NOTE.find("zoom").unwrap() < TEXT_NOTE.find("have no field").unwrap(),
            "zoom 要归到「当场生效」那一句，而不是「没有字段」那一句"
        );
    }

    #[test]
    fn the_behaviour_page_points_restore_at_the_application_page() {
        // 「恢复上次会话」已经实现了，只是不在这一页——不指过去，用户会以为没有。
        assert!(RESTORE_NOTE.contains("Application ▸ Startup"));
        // 自动重连缺的是「策略 + 事件钩子」，不能只说「未实现」。
        assert!(RECONNECT_NOTE.contains("backoff"));
    }

    #[test]
    fn the_behaviour_note_claims_both_live_switches_and_names_the_dead_one() {
        // 两个勾选当场生效，页底必须说清；灰的那一行也要点名缺什么。
        assert!(BEHAVIOUR_NOTE.contains("immediately"), "要写清哪些当场生效");
        assert!(BEHAVIOUR_NOTE.contains("Auto-reconnect"), "灰行要逐行点名");
        // 关多标签那一行已经接上字段：页底不许再说「没有这个粒度」。
        assert!(
            !BEHAVIOUR_NOTE.contains("no 'only when several tabs are open'"),
            "接上字段的开关不能再挂「没有粒度」的旧理由"
        );
    }

    #[test]
    fn the_toolbar_page_says_the_switch_is_not_saved() {
        // 页脚按兼容终端清空后，这条边界搬进**页内**说明
        // （`TOOLBAR_NOTE`），仍然对用户可见，不能只写在注释里。
        assert!(TOOLBAR_NOTE.contains("not saved"), "页内要写清不落盘");
        assert!(TOOLBAR_NOTE.contains("immediately"), "同时要写清当场生效");
        // 地址栏是全仓都没有的东西，理由不能写成「还没接」。
        assert!(ADDRESS_BAR_NOTE.contains("no address bar"));
    }
}

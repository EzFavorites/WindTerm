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
//! - `ToolbarMenu ▸ Show address bar` → `Prefs.show_address_bar`（出厂 `true`），
//!   读者是 `shell/tool_bar.rs`：关掉时工具行右段那块面包屑面板整段不画。
//! - `ToolbarMenu ▸ Auto-hide the menu bar` → `Prefs.auto_hide_menu_bar`（出厂 `false`），
//!   读者是 `ui/mod.rs` 的菜单条高度：开启后收成细带，指针靠近顶端 / Alt / 菜单弹层时展开。
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
const TEXT_NOTE: &str = "Alternate line background, hiding the cursor while typing, automatic \
     copy-on-select and the zoom level apply immediately. Line and character spacing still have \
     no field to bind — see each row's tooltip.";

/// `Behaviour` 页的说明。三个勾选都当场生效：关闭确认、新标签焦点、断线自动重连。
const BEHAVIOUR_NOTE: &str = "Confirm before closing multiple tabs, focus the new tab and \
     automatic reconnect all apply immediately. Reconnect uses a 1/2/4/8 second backoff and gives \
     up after four attempts; an explicit Disconnect is never retried.";

/// `Toolbar / Menu` 页的说明。
///
/// ：Preferences 的橙色页脚按兼容终端清空，「工具行开关不落盘」这条
/// 边界从页脚并进这里，仍然写在用户看得见的地方。
const TOOLBAR_NOTE: &str = "Show toolbar, Show address bar and Auto-hide the menu bar all apply \
     immediately. Only the toolbar switch is not saved (it is back on the next launch); the other \
     two are.";

// 多光标那一行**删掉**了，而不是继续灰着：R10 已经把编辑器语义从代码里移除，
// 连「有多个光标」这个状态都不存在。留一个永远点不动的勾，等于对用户说
// 「这项只是暂时没接」；真相是这个功能已经不存在。
//
// 自动复制那一行接的是 `Prefs.copy_on_select`（终端页也有一份，同一个值）。
// 它原来灰着的理由是「会覆盖系统剪贴板」——那是**故意不做**，不是没有字段；
// 把一个已存在的开关说成没有消费者，比灰着更误导。

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

// 自动重连 2026-11-21 接真（`Prefs.auto_reconnect`）：断线边沿排一次退避，
// 1/2/4/8 秒后原地重连，连上清零；显式 Disconnect 不重试。策略在 `ui/reconnect.rs`。

// 恢复会话那一行也从 Behaviour 页**删掉**了：它不是「缺字段」，而是在
// Application ▸ Startup 里已经有一个真控件。同一件事有两个入口、其中一个还是
// 灰的，用户只会学到「这个功能不可用」。要改就去那个真控件所在的页改。

// 地址栏那一行 2026-11-21 接真（`Prefs.show_address_bar`，出厂显示）：
// 读者是 `shell/tool_bar.rs`，关掉时工具行右段那块面包屑面板整段不画。
// 之前的 `ADDRESS_BAR_NOTE` 声称「这一版根本没有地址栏」是**过期说明**——
// `ui::address::AddressBar` / `draw_address_bar` 一直都在，缺的只是开关。

/// 一行**没有消费者**的控件：整行交给数据表的画法。
///
/// 不在这里另抄一份「灰勾选 / 灰下拉长什么样」——同一页里真控件与灰行必须一眼
/// 能分开，而「灰」的定义只有 `draw_row` 那一份实现。页面底部的说明也由它
/// 挂在悬停上，两处说法不会是两份。
fn disabled(ui: &mut egui::Ui, row: Row, note: &str, col: f32) {
    draw_row(ui, &row, note, col);
}

/// `Text` 页：`Features` 三个真勾选 + `Spacing` 两行灰显。
///
/// 顺序照视觉基线（多光标 → 隔行底色 → 自动复制 → 打字隐光标）。第一行的位置
/// 空出来了，不把后面的行往上提：视觉基线是逐字节比的，挪行等于给一次纯删除
/// 配上一整页重排，看不出「只删了一项」。
pub(in crate::ui) fn text(win: &mut TerminalWindow, ui: &mut egui::Ui) {
    let mut alt_row = win.alt_row_bg;
    let mut hide_cursor = win.hide_cursor_typing;
    let mut copy_on_select = win.copy_on_select;
    let mut changed = false;

    // 视觉基线 `11-text.png`：内容区第一个字是组标题 `Features`，没有页级灰标题。
    // 标签列按本页最长的 `Character spacing:` 量，三个框的控件左缘因此对齐。
    let col = label_col(ui, &["Line spacing:", "Character spacing:", "Zoom level:"]);
    group_box(ui, Some("Features"), |ui| {
        set_row_rhythm(ui);
        changed |= ui
            .checkbox(&mut alt_row, "Alternate line background color")
            .changed();
        // 与 Terminal 页的「When I select text, copy it to clipboard」同一个值。
        // 两处各有入口、写的是同一个字段——不是两份真相（`win` 上的那份副本
        // 也在下面同一帧写回）。
        changed |= ui
            .checkbox(&mut copy_on_select, "Automatically copy selection")
            .changed();
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
        // 只写这三个位：语言 / 主题 / 光标样式 / 搜索模板原样保留。
        win.alt_row_bg = alt_row;
        win.hide_cursor_typing = hide_cursor;
        win.copy_on_select = copy_on_select;
        save_prefs(|prefs| {
            prefs.text_alternate_row_bg = alt_row;
            prefs.text_hide_cursor_while_typing = hide_cursor;
            prefs.copy_on_select = copy_on_select;
        });
    }
}

/// `Behaviour` 页：`Session` 三个真勾选。
///
/// 原来的 `Startup` 组只有一行指向 `Application ▸ Startup` 的灰指针，已整组删除：
/// 指路牌不是控件，而那个真控件一直都在。
pub(in crate::ui) fn behaviour(win: &mut TerminalWindow, ui: &mut egui::Ui) {
    let mut focus_new = win.focus_new_tab;
    let mut multi = win.confirm.multi_tabs;
    let mut reconnect = win.auto_reconnect;
    let mut reconnect_toggled = false;

    // `Behaviour` / `Toolbar / Menu` 两页没有视觉基线，结构与其它页保持一致：
    // 内容区从组标题开始，不另画页级灰标题（那份标题与左栏选中项重复）。
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
        reconnect_toggled = ui
            .checkbox(
                &mut reconnect,
                "Reconnect automatically when the connection drops",
            )
            .changed();
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
        if reconnect_toggled {
            win.auto_reconnect = reconnect;
            save_prefs(|prefs| prefs.auto_reconnect = reconnect);
        }
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
    let mut show_address = win.show_address_bar;
    let mut auto_hide = win.auto_hide_menu_bar;
    let mut toggled = false;
    let mut address_toggled = false;
    let mut auto_hide_toggled = false;

    group_box(ui, Some("Toolbar"), |ui| {
        set_row_rhythm(ui);
        toggled = ui.checkbox(&mut show, "Show toolbar").changed();
        // 与 `Show toolbar` 同在一组，但**落盘**：读者是 `tool_bar::draw`
        // 里那段面板绘制，重启后仍然按用户选的来。
        address_toggled = ui
            .checkbox(&mut show_address, "Show address bar")
            .on_hover_text(
                "Hides the breadcrumb panel (the (i) | > group > session strip) on the \
                 toolbar. The buttons on both ends stay.",
            )
            .changed();
    });

    ui.add_space(6.0);
    group_box(ui, Some("Menu"), |ui| {
        set_row_rhythm(ui);
        // 展开状态是投影态（靠近顶端 / Alt / 菜单弹层），只有这个开关落盘。
        auto_hide_toggled = ui
            .checkbox(&mut auto_hide, "Auto-hide the menu bar")
            .on_hover_text(
                "Collapses the menu bar to a thin strip; hover the top edge or hold \
                 Alt to reveal it. A menu that is already open keeps it visible.",
            )
            .changed();
    });
    // 视觉基线没有页脚说明；灰行理由仍在各自的 hover 上。

    if toggled {
        // `ViewState` 只由 reducer 写 ：这里派发动作，不直接赋值。
        // 勾选是「翻一下」，与 `ShellAction::ToggleToolbar` 的语义正好对齐。
        win.dispatch_action(Action::Shell(ShellAction::ToggleToolbar));
    }
    if address_toggled {
        // 地址栏开关**落盘**：读者在 `tool_bar::draw`，与 `Show toolbar`
        // （不落盘，见页底说明）不是一条路，所以分开写。
        win.show_address_bar = show_address;
        save_prefs(|prefs| prefs.show_address_bar = show_address);
    }
    if auto_hide_toggled {
        win.auto_hide_menu_bar = auto_hide;
        win.menu_bar_peek_until = 0.0;
        save_prefs(|prefs| prefs.auto_hide_menu_bar = auto_hide);
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
    fn the_behaviour_page_states_the_reconnect_policy() {
        // 接真之后页底要写清退避与上限，而不是「还没实现」。
        assert!(BEHAVIOUR_NOTE.contains("backoff"));
        assert!(BEHAVIOUR_NOTE.contains("four attempts"));
        assert!(BEHAVIOUR_NOTE.contains("explicit Disconnect"));
    }

    #[test]
    fn the_text_page_no_longer_calls_copy_on_select_a_missing_field() {
        // 自动复制接的是已有的 `Prefs.copy_on_select`；页底不能再把它归到
        // 「没有字段可绑」那一句里（那会与真勾选自相矛盾）。
        assert!(TEXT_NOTE.contains("copy-on-select"));
        let dead = TEXT_NOTE.find("have no field").unwrap();
        assert!(
            TEXT_NOTE.find("copy-on-select").unwrap() < dead,
            "自动复制要归到「当场生效」那一句"
        );
        assert!(!TEXT_NOTE.contains("Multiple carets"), "多光标已随编辑器语义删除");
    }

    #[test]
    fn the_behaviour_note_claims_all_three_live_switches() {
        // 三个勾选当场生效，页底必须说清；自动重连还要写清上限与显式断开的边界。
        assert!(BEHAVIOUR_NOTE.contains("immediately"), "要写清哪些当场生效");
        assert!(BEHAVIOUR_NOTE.contains("automatic reconnect"), "三个开关都要点名");
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
        // 地址栏现在是真开关（落盘），页底必须把它与不落盘的工具行分开说，
        // 不能再出现「这一版根本没有地址栏」这种过期说法。
        assert!(TOOLBAR_NOTE.contains("address bar"), "要提到地址栏开关");
        assert!(
            TOOLBAR_NOTE.find("Show toolbar").unwrap() < TOOLBAR_NOTE.find("address bar").unwrap(),
            "先说工具行再说地址栏"
        );
    }
}

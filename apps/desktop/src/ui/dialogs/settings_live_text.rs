//! `Preferences` 的 `Text` 页：四个真开关 + 两行灰显。
//!
//! 上游 WindTerm 的 Preferences 只有 19 页，其中 `Text` 页的 `Features` 三行与 `Zoom level`
//! 在这版里有真实读者，接成真控件；`Spacing` 两行没有存储字段，仍画成空框 +
//! 灰字 + 悬停理由。同一页里「能点的勾」与「空框」并存是故意的。
//!
//! `Shortcut Keys` / `Behaviour` / `Toolbar / Menu` 上游没有，是重制版一度新增
//! 的三页，现已移除；它们背后的 prefs 字段保留给原有读者，但设置页不再暴露。
//!
use crate::ui::kit::field::{choice_row_resp, ChoiceKind, UnderlineStyle};
use crate::ui::kit::spec::BLOCK_GAP;
use super::settings_live::save_prefs;
use super::settings_pages::Row;
use super::settings_pages_view::{draw_row, field_label, group_box, label_col, set_row_rhythm, spin_field_delta};
use crate::ui::*;

/// 页底说明：这一页哪几行真、哪几行还缺什么。
///
/// 必须点名，「尚未实现」四个字对下一个人没有信息量（与 `CARET_PAGE_NOTE`
/// 同一条要求）。
const TEXT_NOTE: &str = "Alternate line background, hiding the cursor while typing, automatic \
     copy-on-select and the zoom level apply immediately. Line and character spacing still have \
     no field to bind — see each row's tooltip.";

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

// 自动重连已参数化：默认关闭、仅连接断开触发、180 秒窗口、连上清零；
// 显式 Disconnect 不重试，密码/主机密钥类失败立即停止。策略在 `ui/reconnect.rs`，
// 控件在会话属性的 Session 页（本页不再有入口）。

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
        changed |= choice_row_resp(ui, ChoiceKind::Checkbox, "Alternate line background color", "", &mut alt_row, true, UnderlineStyle::None).clicked();
        // 与 Terminal 页的「When I select text, copy it to clipboard」同一个值。
        // 两处各有入口、写的是同一个字段——不是两份真相（`win` 上的那份副本
        // 也在下面同一帧写回）。
        changed |= choice_row_resp(ui, ChoiceKind::Checkbox, "Automatically copy selection", "", &mut copy_on_select, true, UnderlineStyle::None).clicked();
        changed |= choice_row_resp(ui, ChoiceKind::Checkbox, "Hide cursor when typing", "", &mut hide_cursor, true, UnderlineStyle::None).clicked();
    });

    ui.add_space(BLOCK_GAP);
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

    ui.add_space(BLOCK_GAP);
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
            let (_, stepped) = spin_field_delta(
                ui,
                |ui| {
                    changed = ui
                        .add(
                            egui::DragValue::new(&mut level)
                                .range(-7.0..=35.0)
                                .speed(1.0)
                                .suffix(" levels"),
                        )
                        .changed();
                },
            );
            if let Some(d) = stepped {
                if d > 0 { level = (level + 1.0).min(35.0); }
                else { level = (level - 1.0).max(-7.0); }
                changed = true;
            }
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

}

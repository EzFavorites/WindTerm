//! 选区命令在投影层的落地（菜单与键盘共用一条路径）。
//!
//! 为什么不把位移写进 reducer：落点取决于「此刻画在屏上的那一屏」，
//! 只有 `draw_terminal_area` 手上有这份事实窗口（`tab.view`）。所以菜单点
//! 「选择到下一行」时先把 `Op` 记成待办，本帧拿到窗口后再算一次——
//! 与鼠标拖动写的是同一份 `tab.selection`，于是**鼠标 / 键盘 / 菜单只有一个结果口径**。
//!
//! 本文件只做**键 → 意图**这一件事（`key_to_op`）。意图 → 新选区的位移规则
//! 在 `app::selection::apply`：纯函数、可单测，菜单命令走的也是它。
//!
//! W2 起它还管一件视觉口径：一批选区里**活动那份满档、其余淡一档**
//! （`tint`）。多选区的模型在 `term-app/selection.rs`，画在 `view.rs`——
//! 「用哪一档浓度区分活动」是选择能力的呈现规则，所以留在本文件，
//! 免得 `view.rs` 里散落一个魔法系数。

use crate::ui::*;

use super::pane_pointer::visible_separators;

/// 菜单派人送来的选区命令落地（`draw_tab` 每帧调一次）。
///
/// 两道门，顺序与原先内联在 `pane.rs` 时逐字相同：
/// ① `pending` 立刻取走——留到下一帧会变成「上一次点的命令过几帧才生效」；
/// ② Bin 下**就地丢弃**（hex 视图没有字符坐标），不留到切回 Text 再生效。
/// 「只作用于活动标签页的主区槽位」由调用方按槽位判，不在这道门里。
pub(in crate::ui) fn dispatch_pending_select(pending: &mut Option<Cmd>, tab: &mut Tab, bin: bool) {
    let Some(cmd) = pending.take() else {
        return;
    };
    if bin {
        return;
    }
    // `GotoPreviousCommand` / `GotoNextCommand` / `GotoPreviousScope` /
    // `GotoNextScope` 四条要 OSC 133 块表（绝对行号）；M1 的 Fold 三条要
    // 折叠输入行表。两者都在 `Tab` 上、不在事实窗口里，而这里正好三个都在
    // 手上——所以走带上下文的入口，决策仍全部在纯函数里。其余命令不看这两张
    // 表，多传两个参数没有代价。
    // 先算出输入行表再借 `tab.selections`：直接把 `&tab.fold_inputs()` 写在参数里
    // 会和可变借用撞上（编译期 E0502），而这块表本来就是每帧的一份只读快照。
    let inputs = tab.fold_inputs();
    term_app::selection::apply_cmd_with_context(
        cmd,
        &tab.view,
        &tab.snapshot.blocks,
        &mut tab.selections,
        visible_separators(&tab.config, tab.snapshot.alt),
        &term_app::selection::Pairs::parse(&tab.config.bracket_pairs),
        &inputs,
    );
}

/// 一批选区的底色浓度：活动那份用原色，其余淡一档。
///
/// 为什么必须区分：`Text.ActivateNextSelection` 改的只是「谁是活动的」，
/// 如果两种状态画得一模一样，那条命令点下去就是零反馈——比置灰更糟。
/// 淡一档也不是「看不见」：`IDLE_TINT` 取 0.45，高亮仍然清晰可辨。
pub(in crate::ui) fn tint(is_active: bool, base: egui::Color32) -> egui::Color32 {
    if is_active {
        base
    } else {
        base.gamma_multiply(IDLE_TINT)
    }
}

/// 非活动选区的浓度系数。
const IDLE_TINT: f32 = 0.45;

/// 键 → 选区操作。只在按住 Shift 时成立；Alt/Option 把移动单位换成「词」。
///
/// 为什么 Shift+方向键不转发给程序：终端里它是**外壳的选择手势**
/// （macOS Terminal、iTerm2 都是这个约定）。程序要方向键时用不带修饰键的版本，
/// 所以这里吞掉按键是安全的——用户在 vim 里按 Shift+方向键本来也没有额外含义。
///
/// `Cmd+Shift+↑/↓` 是「选到文档首尾」，与 macOS 的文本惯例一致。
pub(in crate::ui) fn key_to_op(key: egui::Key, mods: egui::Modifiers) -> Option<Op> {
    if !mods.shift {
        return None;
    }
    let word = mods.alt;
    let doc = mods.command;
    let op = match key {
        egui::Key::ArrowLeft if word => Op::WordStartPrev,
        egui::Key::ArrowRight if word => Op::WordEndNext,
        egui::Key::ArrowLeft => Op::CharPrev,
        egui::Key::ArrowRight => Op::CharNext,
        egui::Key::ArrowUp if doc => Op::ToDocumentStart,
        egui::Key::ArrowDown if doc => Op::ToDocumentEnd,
        egui::Key::ArrowUp => Op::LinePrev,
        egui::Key::ArrowDown => Op::LineNext,
        egui::Key::PageUp => Op::PagePrev,
        egui::Key::PageDown => Op::PageNext,
        egui::Key::Home => Op::ToLineHome,
        egui::Key::End => Op::ToLineEnd,
        _ => return None,
    };
    Some(op)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn idle_selections_are_drawn_lighter_than_the_active_one() {
        // 这条断言守的是「点了看得见」：只有活动那份用原色，其余必须淡一档，
        // 但也不能淡到看不见。
        let base = egui::Color32::from_rgba_unmultiplied(255, 255, 255, 100);
        let active = tint(true, base);
        let idle = tint(false, base);
        assert_eq!(active, base, "活动选区用原色");
        assert!(idle.a() < active.a(), "非活动选区必须比活动那份淡：{idle:?}");
        assert!(idle.a() > 0, "淡一档不等于看不见");
    }

    #[test]
    fn shift_arrow_selection_ops_are_the_same_ones_the_menu_uses() {
        // 键盘与菜单共用一套 `Op`：Shift+← 与 Goto › Char Left 必须同义，
        // 否则同一次移动会有两种落点。
        let shift = egui::Modifiers {
            shift: true,
            ..Default::default()
        };
        assert_eq!(key_to_op(egui::Key::ArrowLeft, shift), Some(Op::CharPrev));
        assert_eq!(key_to_op(egui::Key::ArrowRight, shift), Some(Op::CharNext));
        assert_eq!(key_to_op(egui::Key::ArrowUp, shift), Some(Op::LinePrev));
        assert_eq!(key_to_op(egui::Key::ArrowDown, shift), Some(Op::LineNext));
        // 没按 Shift 时不产生选区意图——方向键要转给程序。
        assert_eq!(key_to_op(egui::Key::ArrowLeft, egui::Modifiers::default()), None);
    }
}

//! 鼠标手势的**行为档**：`Terminal ▸ Mouse Actions` 那两行的纯逻辑。
//!
//! # 为什么这部分不在投影层
//!
//! 「右键该干什么」是**意图判定**，不是像素：`Show menu` / `Paste text` /
//! `Copy text if selected, otherwise paste text` 三档之间的区别只有一句话——
//! 此刻有没有选区。把这句话写成纯函数，三件事同时变简单：
//!
//! 1. 三档 × 有/无选区的六种组合可以逐条单测，不必开窗截图；
//! 2. 投影层只剩「照判决执行」，`right click` 的 if 链不会自己长出第四档；
//! 3. `prefs.json` 里存的是下标（`term_profile::prefs::RIGHT_CLICK_ACTIONS`），
//!    档位文案与语义在这里，prefs 不装 UI 词——与 `Tabs ▸ Double click:`
//!    同一套分工。
//!
//! # 为什么不复用 `Tabs` 页那张表
//!
//! `Tabs ▸ Right click:` 是**标签条**的右键（`No action`），与终端内容区的
//! 右键是两份菜单、两个对象。同名不同义的两项共用一张档位表，只会让「改错
//! 了哪一项」变得无法从代码看出来。

use crate::selection::Selection;

/// `Terminal ▸ Mouse Actions ▸ Right click:` 的三个档位。
///
/// 下标与 `term_profile::prefs::Prefs::right_click_action` 一一对应；
/// 越界一律按 [`RightClickAction::Menu`] 处理（见 `Prefs::right_click_kind`）。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum RightClickAction {
    /// 显示终端右键菜单（出厂态，也是接线之前的行为）。
    #[default]
    Menu,
    /// 直接粘贴剪贴板（与中键、`Cmd+V` 同一条 `Event::Paste` 路径）。
    Paste,
    /// 有选区就复制选区、否则粘贴。
    CopyOrPaste,
}

impl RightClickAction {
    /// 三档，顺序即设置页下拉的顺序。
    pub const ALL: [RightClickAction; 3] = [
        RightClickAction::Menu,
        RightClickAction::Paste,
        RightClickAction::CopyOrPaste,
    ];

    /// 下标 → 档位。越界回落 `Menu`：宁可弹菜单，也不替用户往 shell 里灌字节。
    pub const fn from_index(index: usize) -> Self {
        match index {
            1 => RightClickAction::Paste,
            2 => RightClickAction::CopyOrPaste,
            _ => RightClickAction::Menu,
        }
    }

    /// 档位 → 下标（写回 `Prefs` 用）。
    pub const fn index(self) -> usize {
        match self {
            RightClickAction::Menu => 0,
            RightClickAction::Paste => 1,
            RightClickAction::CopyOrPaste => 2,
        }
    }
}

/// 一次右键按下要做什么。**判决结果只有三项**，且都是既有的落地路径：
/// 菜单走 `draw_terminal_context`，粘贴走 `request_paste`，
/// 复制走 `ctx.copy_text`。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RightClickDecision {
    /// 弹菜单。
    Menu,
    /// 粘贴剪贴板。
    Paste,
    /// 复制当前选区。
    Copy,
}

/// 三档 × 有没有选区 = 六种结果。
///
/// `has_selection` 只看「选区非空」——`CopyOrPaste` 的文案就是
/// `Copy text if selected, otherwise paste text`，判据与文案同一句话。
pub const fn decide(action: RightClickAction, has_selection: bool) -> RightClickDecision {
    match action {
        RightClickAction::Menu => RightClickDecision::Menu,
        RightClickAction::Paste => RightClickDecision::Paste,
        RightClickAction::CopyOrPaste => {
            if has_selection {
                RightClickDecision::Copy
            } else {
                RightClickDecision::Paste
            }
        }
    }
}

/// 选中即复制的判据：这一段选区**该不该现在写剪贴板**。
///
/// 三条都成立才写，任何一条缺失都意味着「还不到时候」：
///
/// 1. 开关是开的（`copy_on_select`）——关着时复制必须显式；
/// 2. 选区非空——空选区写剪贴板等于把用户剪贴板清空；
/// 3. 指针没按着——拖动过程中每帧都写一次，既吵又会让中间态覆盖最终态。
///
/// `previous` 是上一次写出去的内容：同一段选区只写一次，
/// 用户把它改回来（重新选同一段）时才再写一次。
pub fn should_copy_on_select(
    enabled: bool,
    text: &str,
    pointer_down: bool,
    previous: Option<&str>,
) -> bool {
    if !enabled || pointer_down || text.is_empty() {
        return false;
    }
    previous != Some(text)
}

/// 选区是否非空。取的是**归一化后的跨度**，不是 `head != anchor`：
/// 双击选词之后 head 可能与 anchor 相同（单字），但选区是实打实的。
pub fn has_selection(selection: Option<&Selection>) -> bool {
    selection.is_some_and(|sel| !sel.is_collapsed())
}

/// `Preferences ▸ Mouse Tracking` 的四类事件掩码。
///
/// xterm 鼠标协议只有「总开关 + mode + encoding」，没有「move / wheel /
/// click / middle 各一个 bool」。这一层不是第二套协议，而是**送进编码器之前
/// 的一道掩码**：DECSET 仍决定编码档位，掩码只决定某类本地事件要不要上报。
/// 双击不是协议事件，所以这里没有 `double_click` 字段。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct EventMask {
    pub move_event: bool,
    pub wheel_event: bool,
    pub click_event: bool,
    pub middle_event: bool,
}

impl EventMask {
    /// 出厂 = 全开：升级一次不改变已经接通的鼠标报告行为。
    pub const ALL: Self = Self {
        move_event: true,
        wheel_event: true,
        click_event: true,
        middle_event: true,
    };

    /// 把掩码套到一帧指针事件上。**只清被关掉的那一类**，别的不动——
    /// 协议编码器仍然按 mode 决定要不要发，掩码不替它做决定。
    pub fn apply(&self, frame: &mut term_core::mouse_report::PointerFrame) {
        // 下标口径跟 `PointerFrame` 一致：0 = Primary / 1 = Middle / 2 = Secondary。
        // 这里不 import egui——term-app 零 UI 依赖（R3/F1）。
        if !self.move_event {
            frame.dragged = false;
            frame.hovered = false;
        }
        if !self.wheel_event {
            frame.scroll = 0.0;
        }
        if !self.click_event {
            frame.pressed[0] = false;
            frame.released[0] = false;
            frame.down[0] = false;
        }
        if !self.middle_event {
            frame.pressed[1] = false;
            frame.released[1] = false;
            frame.down[1] = false;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::selection::Pos;

    #[test]
    fn index_round_trips_and_out_of_range_falls_back_to_menu() {
        for action in RightClickAction::ALL {
            assert_eq!(RightClickAction::from_index(action.index()), action);
        }
        // 手改 prefs 写了 9：宁可弹菜单，也不替用户往 shell 里灌字节。
        assert_eq!(RightClickAction::from_index(9), RightClickAction::Menu);
        assert_eq!(RightClickAction::from_index(usize::MAX), RightClickAction::Menu);
        assert_eq!(RightClickAction::default(), RightClickAction::Menu);
    }

    #[test]
    fn the_three_modes_decide_exactly_as_their_labels_say() {
        use RightClickDecision::*;
        // Show menu：有选区也弹菜单，不看选区。
        assert_eq!(decide(RightClickAction::Menu, true), Menu);
        assert_eq!(decide(RightClickAction::Menu, false), Menu);
        // Paste text：有选区也照粘，剪贴板内容与选区无关。
        assert_eq!(decide(RightClickAction::Paste, true), Paste);
        assert_eq!(decide(RightClickAction::Paste, false), Paste);
        // Copy text if selected, otherwise paste text。
        assert_eq!(decide(RightClickAction::CopyOrPaste, true), Copy);
        assert_eq!(decide(RightClickAction::CopyOrPaste, false), Paste);
    }

    #[test]
    fn copy_on_select_needs_all_three_conditions() {
        // 关着：多新的选区都不写。
        assert!(!should_copy_on_select(false, "abc", false, None));
        // 空选区：不写（否则等于清空用户剪贴板）。
        assert!(!should_copy_on_select(true, "", false, None));
        // 指针按着：拖动中间态不写。
        assert!(!should_copy_on_select(true, "abc", true, None));
        // 同一段选区第二次看到：不重复写。
        assert!(!should_copy_on_select(true, "abc", false, Some("abc")));
        // 三段都满足：写。
        assert!(should_copy_on_select(true, "abc", false, None));
        // 改选另一段：写。
        assert!(should_copy_on_select(true, "abcd", false, Some("abc")));
    }

    #[test]
    fn an_empty_selection_is_not_a_selection() {
        let empty = Selection::new(Pos::new(3, 5));
        assert!(!has_selection(None));
        assert!(!has_selection(Some(&empty)));

        let mut dragged = empty;
        dragged.drag_to(Pos::new(3, 8));
        assert!(has_selection(Some(&dragged)));
    }

    #[test]
    fn the_event_mask_only_clears_its_own_event_kind() {
        let mut frame = term_core::mouse_report::PointerFrame {
            x: 3.0,
            y: 4.0,
            has_pos: true,
            pressed: [true, true, true],
            released: [true, true, true],
            down: [true, true, true],
            dragged: true,
            hovered: true,
            scroll: 1.0,
            modifiers: term_core::mouse_report::Modifiers {
                shift: false,
                alt: false,
                ctrl: false,
            },
        };
        // 出厂掩码什么都不清：接线前的鼠标报告一字不变。
        EventMask::ALL.apply(&mut frame);
        assert!(frame.dragged && frame.hovered && frame.scroll > 0.0);
        assert_eq!(frame.pressed, [true, true, true]);

        // 关掉中键：只清下标 1，主键与右键原样。
        let mask = EventMask { middle_event: false, ..EventMask::ALL };
        mask.apply(&mut frame);
        assert_eq!(frame.pressed, [true, false, true]);
        assert_eq!(frame.released, [true, false, true]);
        assert_eq!(frame.down, [true, false, true]);
        assert!(frame.dragged && frame.scroll > 0.0);

        // 四个掩码全关：move / wheel / 主键 / 中键都清掉，坐标与修饰键仍留给
        // 编码器。右键（下标 2）不在这四个掩码里——它的行为在 Terminal 页配。
        let none = EventMask {
            move_event: false,
            wheel_event: false,
            click_event: false,
            middle_event: false,
        };
        none.apply(&mut frame);
        assert!(!frame.dragged && !frame.hovered);
        assert_eq!(frame.scroll, 0.0);
        assert_eq!(frame.pressed, [false, false, true]);
        assert!(frame.has_pos);
    }
}
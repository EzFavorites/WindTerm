//! 菜单数据表的底线断言。
//!
//! 前两组是 `Item::Check` 的老债：**形状**照视觉基线逐行抄，**勾选**不许写死在
//! 已接线的命令上。
//!
//! 后四组是键位表的：一张菜单里有两列键位数据——绑定表 `DEFAULT_SHORTCUTS`
//! 与展示列 `shortcut_text`——外加一个匹配层 `shortcuts::matches`。三者
//! 各说各话时用户看到的是「菜单写了一个键，按下去什么也没发生」，而在此之前
//! 没有任何一条既有测试覆盖得到（唯一的绑定断言只遍历那 10 条绑定表）。
//!
//! 四条的判据分两类，别把它们看成一类：
//! - **冻结清单**（两表分歧 / 标了键却没接线）：现状是「视觉基线优先」的结果，
//!   已知、暂时接受，断言只守「集合与清单精确相等」——只许减不许增。
//! - **真缺陷判据**（键要按得响 / 一个键不挂两处）：没有清单可走，永远只能绿。
//!
//! 拆成独立文件是为了让 menu.rs 保持「一张数据表 + 几个查表函数」的样子；
//! 这里只放断言，不放任何生产代码。

use super::*;
use crate::capability;

/// 取一张顶级菜单的条目（`menus()` 每次现建，所以按值取回来）。
fn items_of(label: &str) -> Vec<Item> {
    menus()
        .into_iter()
        .find(|m| m.label == label)
        .unwrap_or_else(|| panic!("没有名为 {label} 的顶级菜单"))
        .items
}

/// 把一张菜单摊平成「文本形状」：命令与勾选行都写标签，勾选行前面加 ✓/☐/◻，
/// 分隔线写 `|`，子菜单写 `<label>`。
///
/// 之所以不直接比 `Item`：`Item` 里还有 id，而这张测试关心的是**屏幕上看
/// 得到的顺序与勾**；id 由另一条测试（能力台账）兜。
///
/// 第三种前缀 `◻` 是 [`CheckState::Live`]：这一行的框真实存在，但勾由投影层
/// 每帧按当前会话现读，**不在数据表里**。用 ✓/☐ 之一去写它就是在数据层断言
/// 一个它表达不了的事实，所以这里如实写「运行期」。
fn shape(items: &[Item]) -> Vec<String> {
    items
        .iter()
        .map(|it| match it {
            Item::Action { label, .. } => (*label).to_string(),
            Item::Check {
                label, checked, ..
            } => {
                let mark = match checked {
                    CheckState::Fixed(true) => "✓",
                    CheckState::Fixed(false) => "☐",
                    CheckState::Live => "◻",
                };
                format!("{mark} {label}")
            }
            Item::Separator => "|".to_string(),
            Item::Submenu { label, .. } => format!("<{label}>"),
        })
        .collect()
}

/// `01-menu-bar/mode/01-mode-menu.png` 逐行抄。
///
/// 与旧版的三处差异都是这张图定的，改文案前先回去看图：
/// 1. 没有 `Set` / `Toggle` 动词前缀；
/// 2. `Remote Mode` / `Word Wrap` 打勾，`Virtual Space` / `Multiple Caret` 空框；
/// 3. `Local Mode` 与 `Hex Edit` **没有**勾选框——它们不是同一个单选组的成员，
///    视觉基线里就是普通命令行。
#[test]
fn mode_menu_matches_the_reference_row_for_row() {
    assert_eq!(
        shape(&items_of("Mode")),
        vec![
            // Remote Mode 现在是 CheckState::Live：出厂远端态时真机画勾，
            // 这里 `shape()` 只标占位符「◻」，不预计算运行期值。
            "◻ Remote Mode",
            "Local Mode",
            "Hex Edit",
            // Word Wrap 现在是 CheckState::Live：出厂关时真机画空框，
            // 这里 `shape()` 只标占位符「◻」，不预计算运行期值。
            "◻ Word Wrap",
            "☐ Virtual Space",
            "✓ Multiple Caret",
            "Focus Mode",
            "Focus Mode Settings...",
            "Full Screen",
            "Always On Top",
            "Lock Screen",
            "Debug Mode",
        ]
    );
}

/// `01-menu-bar/view/01-view-menu.png`：全菜单只有一个勾选项，且它落在
/// 「Syntax Schemes 之后、Margins 之前」那一段。
#[test]
fn the_view_menu_has_exactly_one_check_row() {
    let view = items_of("View");
    let checks: Vec<&str> = view
        .iter()
        .filter_map(|it| match it {
            Item::Check { label, .. } => Some(*label),
            _ => None,
        })
        .collect();
    assert_eq!(checks, vec!["Highlight Caret Line"]);

    let at = view
        .iter()
        .position(|it| matches!(it, Item::Check { .. }))
        .expect("View 菜单里要有勾选行");
    let before = &view[at - 2];
    assert!(
        matches!(before, Item::Submenu { label, .. } if *label == "Syntax Schemes"),
        "勾选行前面应是 Syntax Schemes 子菜单 + 一条分隔线"
    );
    // 2026-11-23 原版 `01-view-menu.png`：勾选行与 Margins 之间也有一条分隔线。
    assert!(matches!(view[at + 1], Item::Separator));
    let after = &view[at + 2];
    assert!(
        matches!(after, Item::Submenu { label, .. } if *label == "Margins"),
        "勾选行后经一条分隔线到 Margins 子菜单"
    );
}

/// 勾的方向必须与「接没接线」一致，两个方向都抓。
///
/// 理由：接线的开关一旦被写死成「永远勾上」（或永远空框），用户点它就既不改
/// 状态也不改显示，比不勾更误导；反过来，一条**没接线**的命令若写成 `Live`，
/// 屏幕上会出现一个点了不动、勾也不动的框——两种都是屏幕在撒谎。
///
/// 所以判据是双向的：
/// - `CheckState::Fixed` ⇒ 必须 `!is_implemented`（没有运行期状态可读，才能写常量）；
/// - `CheckState::Live` ⇒ 必须 `is_implemented`（有读者才能现读）。
///
/// 2026-11-13：十行栅格可视化接线，`Live` 第一次有了真读者
/// （`ui/shell/menu_items.rs::live_check`）。`Fixed` 只剩 Mode 的四条「模式」行。
/// 2026-11-22：hex 三组互斥档位（13 行）从 `act` 改成 `Live`——它们本就被视觉基线
/// 画成勾选行，出厂档位（16 / 16 Columns / 1 Char）现在由活动标签的 `Tab.hex` 现读，
/// 点另一档勾就挪过去。依然是「`Live` ⇔ 已接线」，没有松动判据。
#[test]
fn a_wired_command_must_read_live_state() {
    let mut fixed_rows = Vec::new();
    let mut live_rows = Vec::new();
    fn walk(items: &[Item], fixed: &mut Vec<&'static str>, live: &mut Vec<&'static str>) {
        for it in items {
            match it {
                Item::Check { label, id, checked } => match checked {
                    CheckState::Fixed(_) => {
                        assert!(
                            !capability::is_implemented(id),
                            "{label}（{id}）已经接线，勾选态不能再写死常量——\
                             改成 CheckState::Live，让投影层按当前会话现读，\
                             否则用户点了它既不改状态也不改显示"
                        );
                        fixed.push(label);
                    }
                    CheckState::Live => {
                        assert!(
                            capability::is_implemented(id),
                            "{label}（{id}）还没接线，不能写成 CheckState::Live——\
                             没有读者就画不出真状态，屏幕上的框会点了不动、勾也不动"
                        );
                        live.push(label);
                    }
                },
                Item::Submenu { items, .. } => walk(items, fixed, live),
                _ => {}
            }
        }
    }
    for m in menus() {
        walk(&m.items, &mut fixed_rows, &mut live_rows);
    }
    // 按视觉基线补齐的 18 个子菜单里原本有 11 条勾选行，勾是截图里的出厂态
    // （Indent Guides / Hide All 打勾），且这些命令一条都还没接线——那时它们
    // 全落在 `Fixed` 这一侧，靠「Fixed ⇒ 未接线」这条准入条件自证。
    //
    // 2026-11-11：hex 那 3 条（16 / 8 Columns / 1 Char）接成真命令，按规矩
    // 改成了 `act`，于是从这里消失、只剩 8 条。
    // 2026-11-13：栅格可视化那 10 条接线，其中 6 条带框（Symbols 四项 +
    // Hide All + View 顶层的 Highlight Caret Line）改成 `Live`；
    // 另外 4 档 White Spaces 在视觉基线里本来就没有框，仍是 `act`，不在这张表里。
    assert_eq!(
        live_rows,
        vec![
            // View › Hex Base 五档互斥单选（出厂 16 打勾）
            "2",
            "7",
            "8",
            "10",
            "16",
            // View › Hex Columns 四档（出厂 16 Columns 打勾）
            "8 Columns",
            "16 Columns",
            "24 Columns",
            "32 Columns",
            // View › Hex Group Chars 四档（出厂 1 Char 打勾）
            "1 Char",
            "2 Chars",
            "4 Chars",
            "8 Chars",
            // View › Bars / Panes：原版是短名字 + 真勾选框，不再是 Toggle 字样
            "Menu Bar",
            "Status Bar",
            "Quick Bar",
            "Explorer",
            "Filer",
            "Outline",
            "Sender",
            "Session",
            "Shell",
            "Transfer",
            // View 顶层唯一的勾选行（`Text.HighlightCaretLine`）
            "Highlight Caret Line",
            // View › Symbols 四项
            "Line Endings",
            "Column Marker",
            "Indent Guides",
            "Wrap Symbols",
            // View › White Spaces：只有 Hide All 带框
            "Hide All",
            // Mode › Remote Mode：2026-11-23 接线后改 Live，勾由 `local_mode_on` 现读。
            "Remote Mode",
            // Mode › Word Wrap：2026-11-24 接线改 Live，勾由 `word_wrap_on` 现读。
            "Word Wrap",
        ],
        "读运行期状态的勾选行变了：它们必须与已接线命令一一对应"
    );
    assert_eq!(
        fixed_rows,
        vec![
            // Mode：Remote Mode（2026-11-23）与 Word Wrap（2026-11-24）已接线
            // 改 Live，移出这张表；余下两条仍没有运行期状态可读（口径表见
            // `menu.rs::Item::Check`），勾是常数、行也置灰点不动。
            "Virtual Space",
            "Multiple Caret",
        ],
        "写死常量的勾选行变了：新增一条必须同时回答「它勾的是什么事实」"
    );
}

// ── 键位表的四条交叉断言 ────────────────────────────────────────────────────
//
// 三条既有事实让这四条成为必要：
// 1. `shortcut_text`（展示列）与 `DEFAULT_SHORTCUTS`（绑定表）是两张手写的表，
//    同一个 ID 可以一边写 `Cmd+W` 一边写 `Meta+Shift+Q`，全仓没有任何断言管这件事；
// 2. 展示列里标了键的命令可以根本没接线；
// 3. 两段式键（`Alt+W, Alt+H`）的匹配路径曾经按 `+` 切而把逗号当键名，
//    `matches` 恒为 false——键写在菜单上，按下去永远不响。
//
// 口径（2026-11-11 定，改前请先读）：
// - 1 与 2 是**已知且暂时接受**的现状，不是待修的空白：菜单列照视觉基线一字不差，
//   视觉基线上标着键的项就得标着，删字是拿保真度换整洁。所以这两条断言写成
//   **冻结清单**（`KNOWN_*`）——基线是现状的事实，不是许可额度。
//   「只许减不许增」：新增一条不一致立刻红，修好一条不删表项也立刻红。
// - 3 与 4 是**真缺陷的判据**，没有清单可走：键写在菜单上就必须按得响，
//   一个键也不许挂两条命令。这两条永远只能绿。

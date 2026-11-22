//! 18 个空子菜单按视觉基线补齐结构（**只补结构，不假装接线**）。
//!
//! # 为什么独立成文件
//!
//! `menu.rs` 是 10 棵顶级菜单的数据表，18 个子菜单的 68 条命令逐字抄自
//! 把这些明细塞回 `menu.rs` 会让那张表再长 300 行，且「父级结构」与
//! 「子项逐行比对」混在一起。这里只放子项，父级仍由 `menu.rs` 摆位次。
//!
//! # 口径
//!
//! 这些条目**全部是 `Planned`**：`Action::parse` 对它们一律返回 `None`，
//! 所以 UI 画成置灰、悬停给「Not wired (placeholder)」。交付的是
//! 「点开子菜单不再只有一条横线」，不是「这些命令已经能用」——
//! 后者逐波接成真功能，列入清零清单与 `docs/design/12-capability-surface-and-menus.md`。
//! 本文件末尾的断言会钉住「一条都没偷偷接线」；将来接上一条，
//! 就把那条从 `EXPECTED_PLANNED` 里划掉，不许删断言。
//!
//! 勾选行（`Item::Check`）分成两种口径，见 `menu.rs::CheckState`：
//!
//! - **写死常量**（`Fixed`）：只允许出现在「没有运行期状态可读」的行上；
//! - **读运行期状态**（`Live`）：由投影层每帧按当前会话现读。
//!
//! 2026-11-13 之前本文件里的 5 条勾选行全是写死常量（截图里的出厂态：
//! `Indent Guides` / `Hide All` 打勾，其余空框），因为那 9 条命令一条都没接线。
//! 这一刀把它们接成真命令之后，5 条全部改成 `Live`——**不跟着状态走的勾正是
//!**。`menu_check_tests.rs::a_wired_command_must_read_live_state` 从两头锁：
//! `Fixed` 必须没接线、`Live` 必须已接线。
//!
//! 2026-11-11：hex 那三张子菜单（Base / Columns / Group Chars）的 13 条命令
//! 接通之后，原本写死的勾先被拆掉（`act`）——编译期常量不跟着当前值变，
//! 屏幕会撒谎。顺带修掉一处保真错误：视觉基线 `view/03` 打勾的
//! 是 **16 Columns**，本文件原先把它写在 `8 Columns` 上。
//! 2026-11-22：这 13 行改回**勾选行**，但勾读运行期状态（`Live`）：它们现在是
//! 真的互斥单选组，只有当前生效的那一档打勾，点另一档勾就跟着挪过去。
//! 出厂态与视觉基线一致（Base 16 / Columns 16 / Group 1 Char 打勾），
//! 但那不是抄进数据表里的常量，而是投影层每帧从活动标签的 `Tab.hex` 现读
//! （`ui/shell/menu_items.rs::live_check`）。表头（`bin.rs`）仍每帧现算同一份值。
//!
//! # 逐张证据
//!
//! | 子菜单 | 截图 |
//! |---|---|
//! | Open Recent | `session/02-session-open-recent-submenu.png` |
//! | Select Column To | `selection/04-selection-select-column-to-submenu.png` |
//! | Word…Document（10 个） | `goto/02-…` … `goto/11-…` |
//! | Hex Base / Hex Columns / Hex Group Chars | `view/02-…` / `03-…` / `05-…` |
//! | Symbols / White Spaces / Folds | `view/11-…` / `12-…` / `13-…` |
//!
//! **留白**：Open Recent 截图里两条 `shell://shell sessions/…` 是**运行时**
//! 的最近 URL，由会话库生成。静态菜单树写死它们就是制造假入口，
//! 所以这里只放静态的两条命令，动态行留到数据驱动时接。
//! 同理，若干快捷键列（Meta+Shift+Left 等）随逐屏比对补，
//! 本刀不往 `shortcut_text` 里塞没有读者的绑定。

use super::{CheckState, Item, Menu};

fn act(label: &'static str, id: &'static str) -> Item {
    Item::Action { label, id }
}

/// 一条**勾读运行期状态**的勾选行（[`CheckState::Live`]）。
///
/// 这里只能造 Live，造不出 `Fixed`：子菜单里的勾选行全部是已接线的开关 / 单选档位
/// （`View ▸ Hex Base/Columns/Group Chars` 的 13 档 + `View ▸ Symbols` 四项 +
/// `View ▸ White Spaces ▸ Hide All`），勾必须跟着当前会话的 `Tab.hex` / `Tab.marks`
/// 走。写死的勾只允许出现在顶层 Mode 菜单那四条「没有运行期状态可读」的模式行上
/// （见 `menu/tree.rs`）——两条护栏在 `menu_check_tests.rs::a_wired_command_must_read_live_state`。
fn live(label: &'static str, id: &'static str) -> Item {
    Item::Check {
        label,
        id,
        checked: CheckState::Live,
    }
}

/// `session/02-session-open-recent-submenu.png`。
///
/// 第一项的命令 ID 是 `Terminal.TabReopenClosed`（改），不是截图上的名字
/// `Reopen Closed View`：它与标签右键菜单里的「Reopen Closed」是**同一件事**，
/// 都落到 `ui/shell/tab_menu.rs::reopen_closed`。以前这里挂的是第二个 ID
/// `Terminal.ReopenClosedView`，`parse` 不认识它，于是同一条能力在能力面上
/// 被算成「能点的一条 + 置灰的一条」。现在两个入口共用一个 ID，`parse` 里
/// 也**只留一条** arm——给旧 ID 补 arm 才会让「两条命令指向一个实现」真的虚高。
///
/// 菜单标签仍照截图写「Reopen Closed View」：截图里就是这三个词，
/// 不为了对齐 ID 去改用户看得见的文案。
fn open_recent() -> Vec<Item> {
    vec![
        act("Reopen Closed View", "Terminal.TabReopenClosed"),
        Item::Separator,
        act("Clear Recent Urls", "Terminal.ClearRecentUrls"),
    ]
}

/// `selection/04-selection-select-column-to-submenu.png`：4 + 4 + 2 三组。
fn select_column_to() -> Vec<Item> {
    vec![
        act("Previous Char", "Text.SelectColumnToPreviousChar"),
        act("Previous Line", "Text.SelectColumnToPreviousLine"),
        act("Previous Page", "Text.SelectColumnToPreviousPage"),
        act("Previous Paragraph", "Text.SelectColumnToPreviousParagraph"),
        Item::Separator,
        act("Next Char", "Text.SelectColumnToNextChar"),
        act("Next Line", "Text.SelectColumnToNextLine"),
        act("Next Page", "Text.SelectColumnToNextPage"),
        act("Next Paragraph", "Text.SelectColumnToNextParagraph"),
        Item::Separator,
        act("Home Of Line", "Text.SelectColumnToLineHome"),
        act("End Of Line", "Text.SelectColumnToLineEnd"),
    ]
}

/// `goto/02-goto-word-submenu.png`。
fn goto_word() -> Vec<Item> {
    vec![
        act("Previous Word", "Text.GotoPreviousWord"),
        act("Previous Word End", "Text.GotoPreviousWordEnd"),
        act("Previous Word Part", "Text.GotoPreviousWordPart"),
        act("Next Word", "Text.GotoNextWord"),
        act("Next Word End", "Text.GotoNextWordEnd"),
        act("Next Word Part", "Text.GotoNextWordPart"),
    ]
}

/// `goto/03-goto-page-submenu.png`。
fn goto_page() -> Vec<Item> {
    vec![
        act("Previous Page", "Text.GotoPreviousPage"),
        act("Next Page", "Text.GotoNextPage"),
        act("Previous Stuttered Page", "Text.GotoPreviousStutteredPage"),
        act("Next Stuttered Page", "Text.GotoNextStutteredPage"),
    ]
}

/// `goto/04-goto-paragraph-submenu.png`。
fn goto_paragraph() -> Vec<Item> {
    vec![
        act("Previous Paragraph", "Text.GotoPreviousParagraph"),
        act("Next Paragraph", "Text.GotoNextParagraph"),
    ]
}

/// `goto/05-goto-command-submenu.png`（快捷键 Meta+Shift+Left / Right，补）。
fn goto_command() -> Vec<Item> {
    vec![
        act("Goto Previous Command", "Text.GotoPreviousCommand"),
        act("Goto Next Command", "Text.GotoNextCommand"),
    ]
}

/// `goto/06-goto-fold-submenu.png`。
fn goto_fold() -> Vec<Item> {
    vec![
        act("Previous Fold", "Text.GotoPreviousFold"),
        act("Next Fold", "Text.GotoNextFold"),
    ]
}

/// `goto/07-goto-scope-submenu.png`。
fn goto_scope() -> Vec<Item> {
    vec![
        act("Previous Scope", "Text.GotoPreviousScope"),
        act("Next Scope", "Text.GotoNextScope"),
    ]
}

/// `goto/08-goto-indentation-submenu.png`：`Next Indentation` 在截图里就是灰的。
fn goto_indentation() -> Vec<Item> {
    vec![
        act("Previous Indentation", "Text.GotoPreviousIndentation"),
        act("Next Indentation", "Text.GotoNextIndentation"),
    ]
}

/// `goto/09-goto-pair-submenu.png`。
fn goto_pair() -> Vec<Item> {
    vec![
        act("Start Of Pair", "Text.GotoPairStart"),
        act("End Of Pair", "Text.GotoPairEnd"),
        act("Previous Pair", "Text.GotoPreviousPair"),
        act("Next Pair", "Text.GotoNextPair"),
    ]
}

/// `goto/10-goto-line-submenu.png`。
fn goto_line() -> Vec<Item> {
    vec![
        act("Home Of Line", "Text.GotoLineHome"),
        act("End Of Line", "Text.GotoLineEnd"),
    ]
}

/// `goto/11-goto-document-submenu.png`。
fn goto_document() -> Vec<Item> {
    vec![
        act("Start Of Document", "Text.GotoDocumentStart"),
        act("End Of Document", "Text.GotoDocumentEnd"),
    ]
}

/// `view/02-view-hex-base-submenu.png`。
///
/// 第二项截图里确实是 `7`（不是 `?`，放大 4× 复核过）；按原样保留，
/// 不替原程序猜它应该是 8 进制还是别的什么——它就是一个真能用的进制：
/// 一个字节写成 3 位（7³ = 343 > 255），渲染层照做。
///
/// **五档是互斥单选**：勾读运行期状态（[`CheckState::Live`]），只有当前生效的
/// 那一档打勾，出厂是 16（与视觉基线一致）。点另一档，勾就跟着挪过去——
/// reader 是投影层的 `Tab.hex`（`ui/shell/menu_items.rs::live_check`），
/// 不是抄进数据表里的常量。表头（`bin.rs`）每帧现算同一份值。
fn hex_base() -> Vec<Item> {
    vec![
        live("2", "View.HexBase2"),
        live("7", "View.HexBase7"),
        live("8", "View.HexBase8"),
        live("10", "View.HexBase10"),
        live("16", "View.HexBase16"),
    ]
}

/// `view/03-view-hex-columns-submenu.png`。出厂打勾的是 **16 Columns**。
fn hex_columns() -> Vec<Item> {
    vec![
        live("8 Columns", "View.HexColumns8"),
        live("16 Columns", "View.HexColumns16"),
        live("24 Columns", "View.HexColumns24"),
        live("32 Columns", "View.HexColumns32"),
    ]
}

/// `view/05-view-hex-group-chars-submenu.png`。出厂打勾的是 **1 Char**。
fn hex_group_chars() -> Vec<Item> {
    vec![
        live("1 Char", "View.HexGroupChars1"),
        live("2 Chars", "View.HexGroupChars2"),
        live("4 Chars", "View.HexGroupChars4"),
        live("8 Chars", "View.HexGroupChars8"),
    ]
}

/// `view/11-view-symbols-submenu.png`：四项都是可勾选行。
///
/// 四项的勾现在**读运行期状态**（[`CheckState::Live`]）：`View ▸ Symbols`
/// 是真的能切的四个开关，读者是投影层的 `Tab.marks`。所以这里不再写死
/// 「Indent Guides 打勾」那种出厂态——截图里那一勾是当时的运行状态，
/// 抄成常量就变成「永远勾着」的假反馈。
fn symbols() -> Vec<Item> {
    vec![
        live("Line Endings", "View.ToggleLineEndings"),
        live("Column Marker", "View.ToggleColumnMarker"),
        live("Indent Guides", "View.ToggleIndentGuides"),
        live("Wrap Symbols", "View.ToggleWrapSymbols"),
    ]
}

/// `view/12-view-white-spaces-submenu.png`：只有 Hide All 带框。
///
/// **结构照截图一字不动**：五档互斥模式里只有 `Hide All` 画成勾选行，另外四档
/// 是普通命令行（`act`）。这不是遗漏——截图里就是这样，而「哪个模式生效」
/// 由 `Hide All` 那一格回答：勾着 = 不画任何空白符号，空框 = 四档里有一档生效。
fn white_spaces() -> Vec<Item> {
    vec![
        live("Hide All", "View.WhiteSpacesHideAll"),
        act("Show All", "View.WhiteSpacesShowAll"),
        act("Show In Selection", "View.WhiteSpacesShowInSelection"),
        act("Show In Line Start", "View.WhiteSpacesShowInLineStart"),
        act("Show In Line End", "View.WhiteSpacesShowInLineEnd"),
    ]
}

/// `view/13-view-folds-submenu.png`：2 + 2 + 2 三组。
fn folds() -> Vec<Item> {
    vec![
        act("Fold", "View.Fold"),
        act("Unfold", "View.Unfold"),
        Item::Separator,
        act("Fold Inside", "View.FoldInside"),
        act("Unfold Inside", "View.UnfoldInside"),
        Item::Separator,
        act("Fold All", "View.FoldAll"),
        act("Unfold All", "View.UnfoldAll"),
    ]
}

/// 一条「父级标签 → 子项构造器」记录。
///
/// 不用 `(&str, fn() -> Vec<Item>)` 元组：那个类型会踩 clippy
/// `type_complexity`（`-D warnings` 下直接编译失败）；而「标签 + 构造器」
/// 本来就是一条记录，不是随手配的一对。
#[derive(Clone, Copy)]
pub struct SubmenuSpec {
    pub label: &'static str,
    pub build: fn() -> Vec<Item>,
}

/// 18 个标签在整棵树里唯一，`fill` 才能按名替换而不误伤别处。
pub const TABLE: &[SubmenuSpec] = &[
    SubmenuSpec { label: "Open Recent", build: open_recent },
    SubmenuSpec { label: "Select Column To", build: select_column_to },
    SubmenuSpec { label: "Word", build: goto_word },
    SubmenuSpec { label: "Page", build: goto_page },
    SubmenuSpec { label: "Paragraph", build: goto_paragraph },
    SubmenuSpec { label: "Command", build: goto_command },
    SubmenuSpec { label: "Fold", build: goto_fold },
    SubmenuSpec { label: "Scope", build: goto_scope },
    SubmenuSpec { label: "Indentation", build: goto_indentation },
    SubmenuSpec { label: "Pair", build: goto_pair },
    SubmenuSpec { label: "Line", build: goto_line },
    SubmenuSpec { label: "Document", build: goto_document },
    SubmenuSpec { label: "Hex Base", build: hex_base },
    SubmenuSpec { label: "Hex Columns", build: hex_columns },
    SubmenuSpec { label: "Hex Group Chars", build: hex_group_chars },
    SubmenuSpec { label: "Symbols", build: symbols },
    SubmenuSpec { label: "White Spaces", build: white_spaces },
    SubmenuSpec { label: "Folds", build: folds },
];

/// 这棵子树里有没有任何能派发的命令（与 `ui/shell/menu_items.rs::has_command` 同口径）。
fn has_command(items: &[Item]) -> bool {
    items.iter().any(|it| match it {
        Item::Action { .. } | Item::Check { .. } => true,
        Item::Submenu { items, .. } => has_command(items),
        Item::Separator => false,
    })
}

/// 按父级标签，把 `menu.rs` 里的 `vec![]` / `vec![Item::Separator]` 占位换成结构。
///
/// 只动**没有命令**的子菜单：将来某个子菜单被逐波接成真功能、自己写了内容，
/// 这里不会覆盖它。替换是递归的，嵌套子菜单同样受管。
pub fn fill(menus: &mut [Menu]) {
    for menu in menus {
        fill_items(&mut menu.items);
    }
}

fn fill_items(items: &mut [Item]) {
    for item in items {
        if let Item::Submenu { label, items: sub } = item {
            if !has_command(sub) {
                if let Some(spec) = TABLE.iter().find(|spec| spec.label == *label) {
                    *sub = (spec.build)();
                }
            }
            fill_items(sub);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;

    /// 摊平成「屏幕上看得到的形状」：命令 / 勾选行写标签（`✓`/`☐` 前缀），
    /// 分隔线写 `|`，子菜单写 `<label>`。与 `menu_check_tests.rs::shape` 同口径。
    ///
    /// 第三种前缀 `◻` 是**运行期勾**（`CheckState::Live`）：它既不是 ✓ 也不是 ☐，
    /// 因为同一张表在两个不同时刻画出来可能一个是勾一个是空框。用同一个符号去
    /// 假装「出厂就打勾」正是这一刀要拆掉的东西。
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

    /// 18 个子菜单逐行比对。改一行文案前先回去看对应截图。
    const EXPECTED: &[(&str, &[&str])] = &[
        (
            "Open Recent",
            &["Reopen Closed View", "|", "Clear Recent Urls"],
        ),
        (
            "Select Column To",
            &[
                "Previous Char",
                "Previous Line",
                "Previous Page",
                "Previous Paragraph",
                "|",
                "Next Char",
                "Next Line",
                "Next Page",
                "Next Paragraph",
                "|",
                "Home Of Line",
                "End Of Line",
            ],
        ),
        (
            "Word",
            &[
                "Previous Word",
                "Previous Word End",
                "Previous Word Part",
                "Next Word",
                "Next Word End",
                "Next Word Part",
            ],
        ),
        (
            "Page",
            &[
                "Previous Page",
                "Next Page",
                "Previous Stuttered Page",
                "Next Stuttered Page",
            ],
        ),
        (
            "Paragraph",
            &["Previous Paragraph", "Next Paragraph"],
        ),
        (
            "Command",
            &["Goto Previous Command", "Goto Next Command"],
        ),
        ("Fold", &["Previous Fold", "Next Fold"]),
        ("Scope", &["Previous Scope", "Next Scope"]),
        (
            "Indentation",
            &["Previous Indentation", "Next Indentation"],
        ),
        (
            "Pair",
            &["Start Of Pair", "End Of Pair", "Previous Pair", "Next Pair"],
        ),
        ("Line", &["Home Of Line", "End Of Line"]),
        (
            "Document",
            &["Start Of Document", "End Of Document"],
        ),
        (
            "Hex Base",
            &["◻ 2", "◻ 7", "◻ 8", "◻ 10", "◻ 16"],
        ),
        (
            "Hex Columns",
            &[
                "◻ 8 Columns",
                "◻ 16 Columns",
                "◻ 24 Columns",
                "◻ 32 Columns",
            ],
        ),
        (
            "Hex Group Chars",
            &["◻ 1 Char", "◻ 2 Chars", "◻ 4 Chars", "◻ 8 Chars"],
        ),
        (
            "Symbols",
            &[
                // `◻` = 勾读运行期状态（`CheckState::Live`）。视觉基线里 Indent Guides
                // 打着勾，那是**运行状态**不是结构，抄成常量就变成永远打勾。
                "◻ Line Endings",
                "◻ Column Marker",
                "◻ Indent Guides",
                "◻ Wrap Symbols",
            ],
        ),
        (
            "White Spaces",
            &[
                // 结构照截图：只有 Hide All 带框，另外四档是普通命令行。
                "◻ Hide All",
                "Show All",
                "Show In Selection",
                "Show In Line Start",
                "Show In Line End",
            ],
        ),
        (
            "Folds",
            &[
                "Fold",
                "Unfold",
                "|",
                "Fold Inside",
                "Unfold Inside",
                "|",
                "Fold All",
                "Unfold All",
            ],
        ),
    ];

    #[test]
    fn every_p1_submenu_matches_the_reference_row_for_row() {
        assert_eq!(TABLE.len(), 18);
        assert_eq!(EXPECTED.len(), 18);
        for (label, expected) in EXPECTED {
            let spec = TABLE
                .iter()
                .find(|spec| spec.label == *label)
                .unwrap_or_else(|| panic!("表里没有 {label}"));
            let got = shape(&(spec.build)());
            assert_eq!(got, *expected, "{label} 子菜单与截图不一致");
        }
    }

    /// 68 条 ID 全局唯一，且**没有一条「假装接线」**。
    ///
    /// 只补结构时这条断言是「一条都没接线」；接通了其中 11 条
    /// （Goto 10 条映射到既有 `Op`，Open Recent 的 Reopen 并入
    /// `Terminal.TabReopenClosed`），于是判据收窄成「只允许名单里这些条」。
    /// 2026-11-11 又接通 hex 三张子菜单的 13 条档位命令（读者
    /// `ui/effects.rs::set_hex`），名单从 11 条涨到 24 条。
    /// 2026-11-13 接通 26 条光标 / 选区命令（段落 / 词段 / 缩进 / 配对括号 /
    /// 半页 / OSC 133 命令块），名单 24 → 50 条。
    /// 同日再接通 9 条栅格可视化命令（`View ▸ Symbols` 四项 +
    /// `View ▸ White Spaces` 五档），名单 50 → 59 条。第 10 条
    /// `Text.HighlightCaretLine` **不在**这张表里：它是 View 顶层的勾选行，
    /// 不在这 68 条子菜单 ID 之中，归属由 `capability.rs::inventory_matches_the_r3_baseline`
    /// 与 `menu_check_tests.rs` 管。
    /// 2026-11-14 再接通折叠族 8 条（`View ▸ Folds` 六条 + `Goto ▸ Fold` 两条），
    /// 名单 59 → 68 条 —— 再到 68 就是这张表**全部接通**了：`wired.len()` 与
    /// `ids.len()` 相等，于是「结构对齐」与「能力接线」在这 68 条上不再有差集。
    /// 第 9 条折叠命令 `Text.MarginFold` 同样不在表里（它在 View 顶层）。
    /// 名单必须显式：再接一条就得在这里加一行，同时改 `capability.rs` 的三个
    /// 数字与 `docs/design/12-capability-surface-and-menus.md`——这条断言是「结构对齐」与「能力接线」的分界，
    /// 不许删掉或改成「无所谓」。
    #[test]
    fn the_new_rows_are_unique_and_none_is_fake_wired() {
        let ids: Vec<&'static str> = TABLE
            .iter()
            .flat_map(|spec| (spec.build)())
            .filter_map(|it| match it {
                Item::Action { id, .. } | Item::Check { id, .. } => Some(id),
                _ => None,
            })
            .collect();
        assert_eq!(ids.len(), 68, "补进来的命令条数变化需显式确认");
        let unique: BTreeSet<&str> = ids.iter().copied().collect();
        assert_eq!(unique.len(), ids.len(), "子菜单里有重复命令 ID");
        let wired: BTreeSet<&str> = [
            // 与标签右键的「Reopen Closed」是同一个命令，所以菜单这条
            // 改挂同一个 ID，而不是新起一个只有菜单认识的 `ReopenClosedView`。
            "Terminal.TabReopenClosed",
            // Goto 的 10 条，全部只挪活动选区的 head（见 `selection::parse`）。
            "Text.GotoPreviousWord",
            "Text.GotoPreviousWordEnd",
            "Text.GotoNextWord",
            "Text.GotoNextWordEnd",
            "Text.GotoPreviousPage",
            "Text.GotoNextPage",
            "Text.GotoLineHome",
            "Text.GotoLineEnd",
            "Text.GotoDocumentStart",
            "Text.GotoDocumentEnd",
            // 2026-11-11：hex 三个参数子菜单的 13 条。与上面 Goto 那批的
            // 区别是它们**真的改值**：读者 `ui/effects.rs::set_hex` 改
            // `TerminalWindow::hex` 并落 `prefs.hex_*`，渲染器当场按新参数排版。
            "View.HexBase2",
            "View.HexBase7",
            "View.HexBase8",
            "View.HexBase10",
            "View.HexBase16",
            "View.HexColumns8",
            "View.HexColumns16",
            "View.HexColumns24",
            "View.HexColumns32",
            "View.HexGroupChars1",
            "View.HexGroupChars2",
            "View.HexGroupChars4",
            "View.HexGroupChars8",
            // 2026-11-13：26 条光标 / 选区命令（读者：`Effect::Select(Cmd::Move)`
            // → `selection::apply_with_blocks`）。每条的边界判定在
            // `selection/geometry.rs` / `selection/words.rs`，不是在这里现算。
            // 注意 `Text.GotoPreviousFold` / `Text.GotoNextFold` **不在**这一组里：
            // 它们不挪选区（`Op` 管的那件事），而是移动视口。2026-11-14 接通，
            // 写在下面折叠族那一段。
            "Text.SelectColumnToPreviousChar",
            "Text.SelectColumnToNextChar",
            "Text.SelectColumnToPreviousLine",
            "Text.SelectColumnToNextLine",
            "Text.SelectColumnToPreviousPage",
            "Text.SelectColumnToNextPage",
            "Text.SelectColumnToPreviousParagraph",
            "Text.SelectColumnToNextParagraph",
            "Text.SelectColumnToLineHome",
            "Text.SelectColumnToLineEnd",
            "Text.GotoPreviousWordPart",
            "Text.GotoNextWordPart",
            "Text.GotoPreviousStutteredPage",
            "Text.GotoNextStutteredPage",
            "Text.GotoPreviousParagraph",
            "Text.GotoNextParagraph",
            "Text.GotoPreviousCommand",
            "Text.GotoNextCommand",
            "Text.GotoPreviousScope",
            "Text.GotoNextScope",
            "Text.GotoPreviousIndentation",
            "Text.GotoNextIndentation",
            "Text.GotoPairStart",
            "Text.GotoPairEnd",
            "Text.GotoPreviousPair",
            "Text.GotoNextPair",
            // 2026-11-13：栅格可视化的 9 条（View 顶层那条另算，见上）。
            // 读者是投影层的 `Tab.marks`：`Effect::SetMarks` →
            // `ui/effects.rs::set_marks_knob` → 四个翻转 + 五档设档；
            // 像素在 `ui/terminal/marks_paint.rs` / `view.rs`。
            "View.ToggleLineEndings",
            "View.ToggleColumnMarker",
            "View.ToggleIndentGuides",
            "View.ToggleWrapSymbols",
            "View.WhiteSpacesHideAll",
            "View.WhiteSpacesShowAll",
            "View.WhiteSpacesShowInSelection",
            "View.WhiteSpacesShowInLineStart",
            "View.WhiteSpacesShowInLineEnd",
            // 2026-11-13：`Session ▸ Open Recent ▸ Clear Recent Urls`。读者是
            // `ui/address/mod.rs::clear_recent_targets`（经
            // `Effect::ClearRecentUrls`）——清空 `address.recent` 并落盘，
            // 地址栏右键的「Clear Recent」走同一个函数。
            "Terminal.ClearRecentUrls",
            // 2026-11-14：折叠族八条（`View ▸ Folds` 六条 + `Goto ▸ Fold` 两条）。
            // 第 9 条 `Text.MarginFold` 在 View 顶层、不在这 68 条里，它的归属由
            // `capability.rs::the_nine_fold_commands_are_wired_and_owned_by_margin` 管。
            // 读者是 `ui/effects.rs::apply_fold`（决策是纯函数 `decide_fold`），
            // 屏幕上的效果在 `terminal/setup.rs::folded_view`：折叠非空时取窗口
            // 路径换一条，被折的行真的从这一屏里消失。
            "View.Fold",
            "View.Unfold",
            "View.FoldInside",
            "View.UnfoldInside",
            "View.FoldAll",
            "View.UnfoldAll",
            "Text.GotoPreviousFold",
            "Text.GotoNextFold",
        ]
        .into_iter()
        .collect();
        assert_eq!(wired.len(), 68, "已接线条数变化需显式确认");
        for id in &ids {
            assert_eq!(
                crate::capability::is_implemented(id),
                wired.contains(id),
                "{id}：接线状态与这份名单不一致——新接一条请同时改这里、\
                 `capability.rs` 的三个数字与 `docs/design/12-capability-surface-and-menus.md`"
            );
        }
    }

    /// `fill` 只替换占位，不碰已经写了内容的子菜单。
    #[test]
    fn fill_leaves_populated_submenus_alone() {
        let mut tree = vec![Menu {
            label: "Session",
            items: vec![
                Item::Submenu {
                    label: "Open Recent",
                    items: vec![Item::Separator],
                },
                Item::Submenu {
                    label: "Log",
                    items: vec![act("Start", "Terminal.StartLogger")],
                },
            ],
        }];
        fill(&mut tree);
        let items = &tree[0].items;
        assert_eq!(shape(&sub(items, "Open Recent")), EXPECTED[0].1);
        assert_eq!(shape(&sub(items, "Log")), vec!["Start"]);
    }

    fn sub(items: &[Item], label: &str) -> Vec<Item> {
        match items
            .iter()
            .find(|it| matches!(it, Item::Submenu { label: l, .. } if *l == label))
        {
            Some(Item::Submenu { items, .. }) => items.clone(),
            _ => panic!("没有子菜单 {label}"),
        }
    }

    /// 生产树里 18 个占位一个不剩。先红后绿验过：把 `fill` 的调用注掉即转红。
    #[test]
    fn the_production_tree_has_no_empty_submenu_left() {
        fn walk(items: &[Item], out: &mut Vec<String>) {
            for it in items {
                if let Item::Submenu { label, items } = it {
                    assert!(
                        has_command(items),
                        "{label} 子菜单点开仍然只有横线/空内容"
                    );
                    walk(items, out);
                    out.push((*label).to_string());
                }
            }
        }
        let mut seen = Vec::new();
        for menu in super::super::menus() {
            walk(&menu.items, &mut seen);
        }
        for spec in TABLE {
            assert!(
                seen.iter().any(|l| l.as_str() == spec.label),
                "生产树里少了 {}",
                spec.label
            );
        }
    }
}

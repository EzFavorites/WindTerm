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
use crate::shortcuts::{self, Keymap, Scope};
use std::collections::{BTreeMap, BTreeSet};

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
            "|",
            "Hex Edit",
            "|",
            "✓ Word Wrap",
            "☐ Virtual Space",
            "☐ Multiple Caret",
            "|",
            "Focus Mode",
            "Focus Mode Settings",
            "|",
            "Full Screen",
            "Always On Top",
            "|",
            "Lock Screen",
            "|",
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
        ],
        "读运行期状态的勾选行变了：它们必须与已接线命令一一对应"
    );
    assert_eq!(
        fixed_rows,
        vec![
            // Mode：Remote Mode 2026-11-23 已接线改 Live，移出这张表；
            // 余下三条仍没有运行期状态可读（口径表见 `menu.rs::Item::Check`），
            // 勾是常数、行也置灰点不动。
            "Word Wrap",
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

/// 一张「用户看得见的键位表」：从菜单树 + 影子入口里捞出所有带键位的命令。
///
/// 返回 `(命令 ID, 展示列文本)`，一条命令一行。用 `inventory()` 而不是手抄名单：
/// 菜单里加一条带键位的命令，这四条断言自动覆盖它，不需要改测试。
fn shortcut_rows() -> Vec<(&'static str, &'static str)> {
    let mut rows = Vec::new();
    for cap in capability::inventory() {
        let shown = shortcut_text(cap.id);
        if !shown.is_empty() && !rows.iter().any(|(id, _)| *id == cap.id) {
            rows.push((cap.id, shown));
        }
    }
    rows
}

/// 把一串键位文本折成**同一把尺子**上的规范弦文本。
///
/// 归一交给 parser 自己做（`parse_chord` 认 `Cmd` / `Ctrl` / `Meta` 三种拼写，
/// `Return` 折成 `Enter`，键名大小写与多余空格一并吃掉），再把结果按
/// `Chord::text()` 输出。这样剩下的任何差异都只可能是「真的指向两个不同的键」，
/// 而不是两张表的书写口味不同。
///
/// 解析失败直接 panic：一条写错的键位不该表现成「两张表恰好一致」。
fn canonical_seq(what: &str, text: &str) -> Vec<String> {
    let seq = shortcuts::parse_chord_seq(text)
        .unwrap_or_else(|| panic!("{what} 的键位文本解析不了：{text:?}"));
    seq.iter().map(|c| c.text()).collect()
}

/// 已知的两表分歧：`(命令 ID, 绑定表弦, 菜单列弦)`。**只许减不许增。**
///
/// 现状是「视觉基线优先」的直接后果：菜单列照 2.6.1 视觉基线逐项抄，绑定表是
/// v0.1 出厂值，两边各自都对，合起来就是不对。要修就两边一起改，改完把这一条
/// 从表里删掉——断言是**精确集合**相等，所以「修好了却忘了删表项」也会红。
const KNOWN_CHORD_DISAGREEMENTS: [(&str, &str, &str); 7] = [
    ("Window.Close", "Cmd+W", "Meta+Shift+Q"),
    (
        "Window.SplitViewHorizontally",
        "Cmd+Shift+T",
        "Alt+W, Alt+H",
    ),
    ("Window.LockScreen", "Cmd+Shift+L", "Meta+Alt+L"),
    ("Window.ShowPaletteCommand", "Cmd+Shift+P", "Alt+;"),
    ("Window.ShowPaletteAnything", "Cmd+Shift+O", "Alt+S"),
    // 视图切换：菜单列是 Meta+PgUp/PgDown，另有 Meta 原绑定与 Alt+[/] 两条
    // 文档键。四条都真能按响，但「显示的那一条」与另三条仍是已知分歧。
    ("Window.ActivatePreviousView", "Alt+OpenBracket", "Meta+PgUp"),
    ("Window.ActivateNextView", "Alt+CloseBracket", "Meta+PgDown"),
];

/// 同一 ID 在两张表里给的键，实测集合必须**恰好**等于冻结清单。
///
/// 两个方向都抓：
/// - 多出来一条（新写了不一致）→ `新增` 那一栏点名它；
/// - 清单里有一条已经一致了（修好了没删表项）→ `已修好但未销账` 那一栏点名它。
///
/// 归一先做（`Cmd` / `Ctrl` / `Meta` 同一键、`Return`≡`Enter`、大小写与空格），
/// 所以 `Cmd+N` / `Meta+N` 不算分歧，只有真的指两个不同的键才算。
#[test]
fn the_two_shortcut_tables_do_not_contradict_each_other() {
    let mut found: Vec<(&str, String, String)> = Vec::new();
    for s in DEFAULT_SHORTCUTS {
        let shown = shortcut_text(s.id);
        if shown.is_empty() {
            continue;
        }
        let bound = canonical_seq("DEFAULT_SHORTCUTS", s.chord);
        let shown_seq = canonical_seq("shortcut_text", shown);
        if bound != shown_seq {
            found.push((s.id, bound.join(" "), shown_seq.join(" ")));
        }
    }

    let frozen: BTreeSet<(&str, String, String)> = KNOWN_CHORD_DISAGREEMENTS
        .iter()
        .map(|(id, bound, shown)| {
            (
                *id,
                canonical_seq("KNOWN_CHORD_DISAGREEMENTS", bound).join(" "),
                canonical_seq("KNOWN_CHORD_DISAGREEMENTS", shown).join(" "),
            )
        })
        .collect();
    let found: BTreeSet<(&str, String, String)> = found.into_iter().collect();

    let added: Vec<String> = found
        .difference(&frozen)
        .map(|(id, bound, shown)| format_disagreement(id, bound, shown))
        .collect();
    let fixed: Vec<String> = frozen
        .difference(&found)
        .map(|(id, bound, shown)| format_disagreement(id, bound, shown))
        .collect();

    assert!(
        added.is_empty() && fixed.is_empty(),
        "两表分歧集合与冻结清单不符（清单是现状的事实，只许减不许增）：\n\
         新增的不一致（{} 条，要么改回去，要么登记进清单并说明理由）：\n{}\n\
         已修好但未销账（{} 条，把它从 KNOWN_CHORD_DISAGREEMENTS 删掉）：\n{}",
        added.len(),
        or_none(&added),
        fixed.len(),
        or_none(&fixed),
    );
}

/// 一条两表分歧的可读写法：ID + 两张表各自的原文折成规范弦。
fn format_disagreement(id: &str, bound: &str, shown: &str) -> String {
    format!("{id}：绑定表 → {bound}，菜单列 → {shown}")
}

/// 把点名的清单排成断言正文里的一段；空清单写 `（无）`，
/// 免得断言文本里出现「0 条」下面跟一片空白。
fn or_none(items: &[String]) -> String {
    if items.is_empty() {
        "  （无）".to_string()
    } else {
        format!("  {}", items.join("\n  "))
    }
}

/// 菜单列标了键、命令却还没接线的那些 ID。**只许减不许增。**
///
/// 这几条不是漏改：视觉基线（2.6.1）上它们就标着键，菜单按视觉基线保真，
/// 删字是偏离视觉基线。行本来就是置灰点不动的（`capability::describe` 说 `Planned`），
/// 「灰项 + 视觉基线上的键」是诚实的状态。命令接线那天，从这张表里删掉即可。
const KNOWN_UNWIRED_SHORTCUTS: [&str; 1] = [
    // `Application.NewWindow` 已删：2026-11-15 接线，并补进 `DEFAULT_SHORTCUTS`。
    // `Window.GotoSymbol` 已接线（符号表 = OSC 133 命令块），2026-11-23 销账。
    // `Window.SetRemoteMode` 已接线（Remote Mode = 退出只读），2026-11-23 销账。
    "Text.ToggleWordWrap",
];

/// 接线之后菜单那一列字不能是「看得见键、按下去不动」：绑定表必须有这一行。
///
/// `handle_global_shortcuts` 只读 `DEFAULT_SHORTCUTS`，不读 `shortcut_text`——
/// 只补展示列等于半个接线。
#[test]
fn new_window_has_a_runnable_shortcut_binding() {
    let entry = DEFAULT_SHORTCUTS
        .iter()
        .find(|s| s.id == "Application.NewWindow")
        .unwrap_or_else(|| panic!("Cmd+Shift+N 必须进绑定表"));
    assert_eq!(entry.chord, "Cmd+Shift+N");
    assert_eq!(entry.label, "New Window", "与菜单行逐字一致");
}

/// 实测「标了键但没接线」的集合必须**恰好**等于冻结清单。
///
/// 与上一条同构：多条要点名（新标了键却没接线），少一条也要点名
/// （接线了没销账）。
#[test]
fn a_shortcut_column_only_marks_wired_commands() {
    let mut found: BTreeSet<&str> = BTreeSet::new();
    for cap in capability::inventory() {
        if !cap.is_implemented() && !shortcut_text(cap.id).is_empty() {
            found.insert(cap.id);
        }
    }
    let frozen: BTreeSet<&str> = KNOWN_UNWIRED_SHORTCUTS.into_iter().collect();

    let added: Vec<String> = found
        .difference(&frozen)
        .map(|id| (*id).to_string())
        .collect();
    let fixed: Vec<String> = frozen
        .difference(&found)
        .map(|id| (*id).to_string())
        .collect();

    assert!(
        added.is_empty() && fixed.is_empty(),
        "「标了键但没接线」的集合与冻结清单不符（只许减不许增）：\n\
         新增（{} 条，视觉基线上有这行字吗？没有就别标键）：\n{}\n\
         清单里已经不成立的（{} 条——要么接线了，要么菜单列撤了键；\
         两种都去 KNOWN_UNWIRED_SHORTCUTS 删掉它）：\n{}",
        added.len(),
        or_none(&added),
        fixed.len(),
        or_none(&fixed),
    );
}

/// 展示列里的每一个键位都必须真的能命中一串按键。
///
/// 判据挂在 `matches_seq`（`matches` 的整段版，派发路径走的就是它）上。
/// **不能**写成「按逗号切段、逐段 `parse_chord` 都成功就算过」：那样七条
/// 两段式键每段本身都合法，断言全绿——而它们的病根恰恰在匹配路径，
/// 不在单段解析。
///
/// 喂进去的按键序列由测试自己切段构造，不借生产的 `parse_chord_seq`：
/// 生产那边若把逗号吞了，这里构造的序列仍是两段，`matches_seq` 就会判不中，
/// 红出来的是「列里有这个键、按了不响」，而不是测试跟着一起歪。
#[test]
fn every_shortcut_column_can_actually_match_a_keypress() {
    let mut bad = Vec::new();
    for (id, shown) in shortcut_rows() {
        let pressed: Vec<shortcuts::Chord> = shown
            .split(',')
            .map(|seg| {
                shortcuts::parse_chord(seg).unwrap_or_else(|| {
                    panic!("{id} 的键位 {:?} 里 {:?} 不是合法的单条弦", shown, seg)
                })
            })
            .collect();
        if !shortcuts::matches_seq(shown, &pressed) {
            bad.push(format!(
                "{}：菜单列写 {:?}（{} 段），但按这 {} 段键也命中不了",
                id,
                shown,
                pressed.len(),
                pressed.len()
            ));
        }
    }
    assert!(
        bad.is_empty(),
        "快捷键列里有点不响的键（派发层按 matches_seq 判命中，它说 false）：\n{}",
        bad.join("\n")
    );
}

/// 一个键只能挂一条命令，且这件事交给 `Keymap::conflicts()` 自己回答。
///
/// 两段式键在 `Keymap`（一个和弦 → 一条命令的平坦表）里表达不了整段，
/// 所以进 `Keymap` 的只有单段键位；两段式由下面那张规范文本表覆盖。
/// `Keymap` 那半条的意义是：**冲突检测不再是死代码**——它第一次有名有姓地
/// 接在一张真实数据表上（以前 `Keymap::new()` 只出现在自己的测试里）。
#[test]
fn no_chord_is_claimed_by_two_commands() {
    // 规范键位文本 → 认领它的命令。两段式与单段一视同仁（整段文本作键）。
    let mut owners: BTreeMap<Vec<String>, Vec<&'static str>> = BTreeMap::new();
    // 单段键位另外装一份进真 `Keymap`，让生产冲突检测器亲口回答一次。
    let mut keymap = Keymap::new();
    let mut single = 0usize;
    for (id, shown) in shortcut_rows() {
        let seq = canonical_seq(id, shown);
        owners.entry(seq.clone()).or_default().push(id);
        if let [chord_text] = seq.as_slice() {
            let chord = shortcuts::parse_chord(chord_text)
                .unwrap_or_else(|| panic!("{id}：规范弦文本 {chord_text:?} 解析不回去"));
            keymap.bind(chord, id, Scope::Global);
            single += 1;
        }
    }

    let mut bad: Vec<String> = owners
        .iter()
        .filter(|(_, ids)| ids.len() > 1)
        .map(|(seq, ids)| {
            format!(
                "{} 被这两条命令同时认领：{}",
                seq.join(" ` "),
                ids.join(" / ")
            )
        })
        .collect();

    assert!(
        keymap.len() == single && single > 0,
        "进 Keymap 的单段键位数不对：绑了 {} 条，单段键位有 {single} 条",
        keymap.len()
    );
    for (chord, first, last) in keymap.conflicts() {
        bad.push(format!(
            "Keymap 认为 {} 同作用域内绑给了 {} 又绑给了 {}",
            chord.text(),
            first,
            last
        ));
    }

    assert!(
        bad.is_empty(),
        "同一个键被两条命令认领：\n{}",
        bad.join("\n")
    );
}

// ── 两段式键：三张表（`DEFAULT_SHORTCUT_SEQS` / 菜单列 / 接线状态）必须对齐 ──
//
// 前四组管单段键位。两段式键多了一张表（`DEFAULT_SHORTCUT_SEQS`，派发读它）
// 与一个状态机（`shortcuts::KeySeqState`，判「这一击算什么」），于是多出
// 三种新的对不上的方式：表里写了但菜单列没有、菜单列有但表里漏了、
// 表里写了却接不上（第一段白吞按键）。这五条各守一种。

/// `DEFAULT_SHORTCUT_SEQS` 解析成「整段按键 + ID」，保持表内顺序。
///
/// 解析失败直接 panic：一条写错的键序不该表现成「这条不存在」。
fn seq_bindings() -> Vec<(Vec<shortcuts::Chord>, &'static str)> {
    DEFAULT_SHORTCUT_SEQS
        .iter()
        .map(|s| {
            let seq = shortcuts::parse_chord_seq(s.chord)
                .unwrap_or_else(|| panic!("DEFAULT_SHORTCUT_SEQS 里 {:?} 解析不了", s.chord));
            (seq, s.id)
        })
        .collect()
}

/// 每条**已接线**的键序都要能被状态机从头走到 `Fire`。
///
/// 「写了键位」与「按得响」是两件事：`DEFAULT_SHORTCUT_SEQS` 里有一条，
/// 不代表那两击真能开火——状态机任一步判错（前缀判定、过期、整段优先）
/// 都会让它停在 `Pending` 或落到 `Pass`。这条逐段喂，中间必须 `Pending`，
/// 最后必须是这一条自己的下标。
#[test]
fn every_two_segment_binding_is_reachable_through_the_state_machine() {
    let table = seq_bindings();
    let mut bad = Vec::new();
    for (want_idx, (seq, id)) in table.iter().enumerate() {
        if !capability::is_implemented(id) {
            continue;
        }
        let mut state = shortcuts::KeySeqState::new();
        for (step_idx, chord) in seq.iter().enumerate() {
            let step = state.feed(&table, *chord, 0);
            let last = step_idx + 1 == seq.len();
            let ok = if last {
                step == shortcuts::SeqStep::Fire(want_idx)
            } else {
                step == shortcuts::SeqStep::Pending
            };
            if !ok {
                bad.push(format!(
                    "{id}（{}）：第 {} 段 {:?} 得到 {step:?}，应为 {}",
                    crate::menu::shortcut_text(id),
                    step_idx + 1,
                    chord.text(),
                    if last { format!("Fire({want_idx})") } else { "Pending".to_string() }
                ));
                break;
            }
        }
    }
    assert!(
        bad.is_empty(),
        "键序表里有按不响的条目（状态机走不到 Fire）：\n{}",
        bad.join("\n")
    );
}

/// `DEFAULT_SHORTCUT_SEQS` 与「菜单快捷键列里的两段式行」必须**恰好**同一批。
///
/// 两个方向都点名：表里有、列里没有（凭空造了一条键位，用户永远看不到提示）；
/// 列里有、表里没有（菜单写着键、按下去什么也不发生——本刀要修的就是这个）。
#[test]
fn the_sequence_table_equals_the_menu_columns_two_segment_rows() {
    let from_table: BTreeSet<(&str, Vec<String>)> = DEFAULT_SHORTCUT_SEQS
        .iter()
        .map(|s| (s.id, canonical_seq("DEFAULT_SHORTCUT_SEQS", s.chord)))
        .collect();
    let from_column: BTreeSet<(&str, Vec<String>)> = shortcut_rows()
        .into_iter()
        .map(|(id, shown)| (id, canonical_seq(id, shown)))
        .filter(|(_, seq)| seq.len() > 1)
        // 右键菜单的两段式键不在菜单栏的 shortcut_text 里，但在同一张绑定表
        // 里；它们也算「菜单写着键」的那一类，必须一起参与精确比对。
        .chain(
            crate::menu::shortcuts::CONTEXT_SEQ_ROWS
                .iter()
                .map(|(id, shown)| (*id, canonical_seq(id, shown))),
        )
        .collect();

    let added: Vec<String> = from_table
        .difference(&from_column)
        .map(|(id, seq)| format!("  {id}：表里有 {}，菜单列里没有", seq.join(" ` ")))
        .collect();
    let missing: Vec<String> = from_column
        .difference(&from_table)
        .map(|(id, seq)| format!("  {id}：菜单列写 {}，表里没有 → 按了不响", seq.join(" ` ")))
        .collect();

    assert!(
        added.is_empty() && missing.is_empty(),
        "两段式键的第三张表与菜单列对不上：\n\
         表里多出来的（{} 条）：\n{}\n\
         表里漏掉的（{} 条）：\n{}",
        added.len(),
        or_none(&added),
        missing.len(),
        or_none(&missing)
    );
}

/// 「已接线的键序」必须**恰好**等于「两段式行 ∖ 已知未接线清单」。
///
/// 这条把两种偏差分开：一种是「新写了一条两段式键、命令却没接线」（用户按了
/// 没反应），另一种是「接线了但没从 `KNOWN_UNWIRED_SHORTCUTS` 销账」。
#[test]
fn the_wired_sequences_are_exactly_the_ones_missing_from_known_unwired() {
    let unwired: BTreeSet<&str> = KNOWN_UNWIRED_SHORTCUTS.into_iter().collect();
    let two_segment: BTreeSet<&str> = shortcut_rows()
        .into_iter()
        .filter(|(id, shown)| canonical_seq(id, shown).len() > 1)
        .map(|(id, _)| id)
        .chain(
            crate::menu::shortcuts::CONTEXT_SEQ_ROWS
                .iter()
                .map(|(id, _)| *id),
        )
        .collect();
    let expected: BTreeSet<&str> = two_segment.difference(&unwired).copied().collect();
    let wired: BTreeSet<&str> = DEFAULT_SHORTCUT_SEQS
        .iter()
        .filter(|s| capability::is_implemented(s.id))
        .map(|s| s.id)
        .collect();

    let added: Vec<String> = wired.difference(&expected).map(|id| format!("  {id}")).collect();
    let missing: Vec<String> = expected.difference(&wired).map(|id| format!("  {id}")).collect();
    assert!(
        added.is_empty() && missing.is_empty(),
        "「已接线的两段式键」与「两段式行 ∖ 已知未接线」不一致：\n\
         多接的（{} 条，它不在两段式行里，或还在未接线清单里）：\n{}\n\
         漏接的（{} 条，两段式行已接线但表里没有）：\n{}",
        added.len(),
        or_none(&added),
        missing.len(),
        or_none(&missing)
    );
}

/// 每个前缀都至少有一个合法后继——不许有「按了第一段就再也出不来」的键序。
///
/// 判据不是「这一条自己能走通」（那是上一条），而是「第一段之后，表里**存在**
/// 某个和弦能把它变成 `Fire`」。若某条只有一个前缀、后继全被删掉，它会变成
/// 白吞按键的死前缀，这条独立地把它抓出来。
#[test]
fn no_sequence_prefix_is_left_pending_forever() {
    let table = seq_bindings();
    // 所有出现过的和弦：后继只可能在这批里（拿全集当搜索空间，不预设答案）。
    let candidates: Vec<shortcuts::Chord> = {
        let mut all: Vec<shortcuts::Chord> = Vec::new();
        for (seq, _) in &table {
            for chord in seq {
                if !all.contains(chord) {
                    all.push(*chord);
                }
            }
        }
        all
    };

    let mut bad = Vec::new();
    for (seq, id) in &table {
        let Some(first) = seq.first() else {
            bad.push(format!("{id}：键序是空的"));
            continue;
        };
        let mut state = shortcuts::KeySeqState::new();
        if state.feed(&table, *first, 0) != shortcuts::SeqStep::Pending {
            // 一段式条目不是本条的判据对象（它不该在这张表里，第 2 条管）。
            continue;
        }
        let fires_some = candidates.iter().any(|c| {
            let mut probe = state.clone();
            matches!(probe.feed(&table, *c, 1), shortcuts::SeqStep::Fire(_))
        });
        if !fires_some {
            bad.push(format!("{id}：按了第一段 {:?} 之后，没有任何后继能开火", first.text()));
        }
    }
    assert!(
        bad.is_empty(),
        "有键序的第一段被白吞（等不到后继）：\n{}",
        bad.join("\n")
    );
}

/// 七条键序两两不等（整段比较，不看顺序之外的写法差异）。
#[test]
fn no_two_sequences_are_identical() {
    let mut seen: BTreeMap<Vec<String>, Vec<&'static str>> = BTreeMap::new();
    for (seq, id) in seq_bindings() {
        let key: Vec<String> = seq.iter().map(|c| c.text()).collect();
        seen.entry(key).or_default().push(id);
    }
    let dupes: Vec<String> = seen
        .iter()
        .filter(|(_, ids)| ids.len() > 1)
        .map(|(seq, ids)| format!("  {} 被这几条命令共用：{}", seq.join(" ` "), ids.join(" / ")))
        .collect();
    assert!(
        dupes.is_empty(),
        "两条键序一模一样——后一条永远按不到自己：\n{}",
        dupes.join("\n")
    );
}

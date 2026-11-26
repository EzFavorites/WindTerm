//! 快捷键数据：出厂绑定表、两段式键序表，以及菜单右侧那一列展示文案。
//!
//! 从 `menu.rs` 搬出来的一段，全是纯数据 + 三个查表函数，零 UI 类型。
//! 三张表（`DEFAULT_SHORTCUTS` / `DEFAULT_SHORTCUT_SEQS` / `shortcut_text`）
//! 各说各话时用户看到的是「菜单写了一个键，按下去什么也没发生」，
//! 所以分歧清单与断言在 `menu_check_tests.rs` 里钉着，改哪一边都要一起改表。

/// 一条默认快捷键绑定：`chord` 是和弦文本，`id` 是菜单命令 ID。
///
/// 为什么放在 `menu.rs`：命令 ID 只出现在数据层与各能力的 `parse()`，
/// 投影层一个字面量都不许有。绑定表是纯数据，正是数据层的职责。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Shortcut {
    pub chord: &'static str,
    pub id: &'static str,
    pub label: &'static str,
}

/// v0.1 出厂绑定。`Cmd` 在 Win/Linux 上归一为 Ctrl（见 `app::shortcuts`）。
///
/// # 与菜单展示列的关系：**已对齐，真相在 `shortcut_map`**
///
/// 旧实现把同一件事抄成三张表，其中本表与菜单列有 7 条对不上（见
/// `menu_check_tests.rs` 的历史清单）。2026-11-24 起展示与派发都只认
/// `shortcut_map::DEFAULT_BINDINGS`，本表降级为用户覆盖迁移用的「旧下标
/// → 命令 ID」兼容门面，弦必须与菜单列一致，否则
/// `the_two_shortcut_tables_do_not_contradict_each_other` 立刻红。
///
/// 对齐过的 7 条：`Window.Close`、`SplitViewHorizontally`、`LockScreen`、
/// `ShowPaletteCommand`、`ShowPaletteAnything` 与两条视图切换的 Alt 别名。
/// 两段式键不在这张表里（一个 `Chord` 放不下），真绑定在
/// `DEFAULT_SHORTCUT_SEQS`，展示列则以 `DEFAULT_BINDINGS` 的第一行为准；
/// 测试对两段式行改用 `DEFAULT_SHORTCUT_SEQS` 比对，不会把它当成分歧。
pub const DEFAULT_SHORTCUTS: &[Shortcut] = &[
    Shortcut {
        chord: "Alt+;",
        id: "Window.ShowPaletteCommand",
        label: "Command Palette",
    },
    Shortcut {
        chord: "Alt+S",
        id: "Window.ShowPaletteAnything",
        label: "Palette: Anything",
    },
    Shortcut {
        chord: "Cmd+N",
        id: "Terminal.NewSession",
        label: "New Session",
    },
    // `Application.NewWindow` 接线时补的：菜单列（Meta+Shift+N）早就写着这个键，
    // 绑定表却一直缺这一行，于是「看得见键、按下去不动」。Cmd 与 Meta 归一后
    // 两表同值，不进 `KNOWN_CHORD_DISAGREEMENTS`。
    Shortcut {
        chord: "Cmd+Shift+N",
        id: "Application.NewWindow",
        label: "New Window",
    },
    Shortcut {
        chord: "Cmd+F",
        id: "Text.Find",
        label: "Find",
    },
    Shortcut {
        chord: "Alt+F3",
        id: "Text.FindNext",
        label: "Find Next",
    },
    Shortcut {
        chord: "Alt+G",
        id: "Window.GotoLine",
        label: "Goto Line",
    },
    // D3：`Alt+D` 此前只出现在 `shortcut_text` 里（菜单右侧那一列字），
    // 派发层读的是本表，于是「菜单写着 Alt+D、按下去什么也不发生」。
    // 现在两表同值（`Alt+D` / `Alt+D`），既有断言
    // `the_two_shortcut_tables_do_not_contradict_each_other` 仍然绿——
    // 它只在两表**指向不同的键**时才计入分歧清单。
    Shortcut {
        chord: "Alt+D",
        id: "Window.GotoAddressBar",
        label: "Address Bar",
    },
    Shortcut {
        chord: "Cmd+Alt+L",
        id: "Window.LockScreen",
        label: "Lock Screen",
    },
    Shortcut {
        chord: "Cmd+Shift+Q",
        id: "Window.Close",
        label: "Close Tab",
    },
    Shortcut {
        chord: "Cmd+Shift+T",
        id: "Window.SplitViewHorizontally",
        label: "Split Right",
    },
    // 视图切换：菜单只写 Meta+PgUp/PgDown，文档另写 Alt+[ / Alt+]。四条都装进
    // 绑定表——菜单列与绑定表的已知分歧在 menu_check_tests 里逐条销账。
    // 重新打开最近关闭的标签：文档写 Cmd+Shift+T（macOS = Meta+Shift+T），
    // 而菜单上的 Ctrl+Shift+T 是「水平分屏」且文案冻结，不能改。两者不同弦，
    // 各按各的——给 Reopen 一条真绑定，而不是让它永远按不响。
    Shortcut {
        chord: "Meta+Shift+T",
        id: "Terminal.TabReopenClosed",
        label: "Reopen Closed Tab",
    },
    Shortcut {
        chord: "Meta+PgUp",
        id: "Window.ActivatePreviousView",
        label: "Previous View",
    },
    Shortcut {
        chord: "Meta+PgDown",
        id: "Window.ActivateNextView",
        label: "Next View",
    },
    Shortcut {
        chord: "Cmd+Shift+I",
        id: "Window.WindowList",
        label: "Window List",
    },
    // 原版 Edit ▸ IntelliSense 的 Complete History 右侧写 Alt+R。
    Shortcut {
        chord: "Alt+R",
        id: "Terminal.CompleteHistory",
        label: "Complete History",
    },
];

/// 一条**两段式**（键序）绑定：`chord` 写原文（`"Alt+W, Alt+H"`），
/// `id` 是菜单命令 ID。
///
/// 为什么不复用 `Shortcut`：它的 `chord` 是「一条弦」，而键序是「一串弦」。
/// 混进 `DEFAULT_SHORTCUTS` 会让 `parse_chord` 的「只解析单条弦」契约失效
/// （`parse_chord("Alt+W, Alt+H") == None` 是既有契约，`dialogs::shortcuts`
/// 与 `settings_pages` 都靠它），也会把 `KNOWN_CHORD_DISAGREEMENTS` 与
/// `Rebind`（只能捕一次按下）一起搅乱。所以分开一张表，字段也只有两个：
/// 键序没有 `label`，它不出现在「Keyboard Shortcuts」对话框的行里。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SeqShortcut {
    pub chord: &'static str,
    pub id: &'static str,
}

/// 两段式键的出厂绑定，原文照 `shortcut_text`（菜单快捷键列）那一行抄，
/// 一个字都不许自己编：这是**同一件事的第三张手写表**，抄错就是「菜单写着
/// A、按下去是 B」。
///
/// 七条都在这里，**全部已接线**（最后一条 `Text.ToggleWordWrap`
/// `Alt+M, Alt+W` 于 2026-11-24 接通）：数据层登记「视觉基线上写了这个键」，
/// 投影层构造时仍用 `capability::is_implemented` 过滤——只装能真派发的那几条。
/// 保留这道过滤不是为了当前（现在全接线），而是因为「菜单列有哪些两段式行」
/// 与「这张表」必须是同一个集合，`menu_check_tests.rs` 只守一遍；若将来有项
/// 回退成占位，过滤会把它自动拿出运行期表，不必在这里分两张表。
///
/// **前缀不许白吞按键**：若某条两段式键未接线却装进运行期表，按下去会进前缀态
/// 却永远等不到后继动作，顺手搅浑同前缀的其他键序——过滤正是为此保留的。
pub const DEFAULT_SHORTCUT_SEQS: &[SeqShortcut] = &[
    SeqShortcut {
        chord: "Alt+M, Alt+H",
        id: "Text.ToggleHexEdit",
    },
    SeqShortcut {
        chord: "Alt+M, Alt+W",
        id: "Text.ToggleWordWrap",
    },
    SeqShortcut {
        chord: "Meta+T, Meta+K",
        id: "Terminal.ManageOneKeys",
    },
    SeqShortcut {
        chord: "Meta+T, Meta+T",
        id: "Terminal.ManageTriggers",
    },
    SeqShortcut {
        chord: "Alt+W, Alt+H",
        id: "Window.SplitViewHorizontally",
    },
    SeqShortcut {
        chord: "Alt+W, Alt+V",
        id: "Window.SplitViewVertically",
    },
    SeqShortcut {
        chord: "Alt+W, Alt+G",
        id: "Window.CloseActiveGroup",
    },
    SeqShortcut { chord: "Alt+W, Alt+M", id: "Window.ToggleMenuBar" },
    SeqShortcut { chord: "Alt+W, Alt+U", id: "Window.ToggleStatusBar" },
    SeqShortcut { chord: "Alt+W, Alt+Q", id: "Window.ToggleQuickBar" },
    SeqShortcut { chord: "Alt+W, Alt+E", id: "Window.TogglePaneExplorer" },
    SeqShortcut { chord: "Alt+W, Alt+F", id: "Window.TogglePaneFiler" },
    SeqShortcut { chord: "Alt+W, Alt+L", id: "Window.TogglePaneOutline" },
    SeqShortcut { chord: "Alt+W, Alt+X", id: "Window.TogglePaneSender" },
    SeqShortcut { chord: "Alt+W, Alt+S", id: "Window.TogglePaneSession" },
    SeqShortcut { chord: "Alt+W, Alt+C", id: "Window.TogglePaneShell" },
    SeqShortcut { chord: "Alt+W, Alt+P", id: "Window.TogglePaneScript" },
    SeqShortcut { chord: "Alt+W, Alt+T", id: "Window.TogglePaneTransfer" },
    // 右键菜单 margin 五列：菜单列与这里同源（CONTEXT_SEQ_ROWS），按下去真翻列。
    SeqShortcut {
        chord: "Alt+T, Alt+B",
        id: "Text.MarginBlank",
    },
    SeqShortcut {
        chord: "Alt+T, Alt+N",
        id: "Text.MarginNumber",
    },
    SeqShortcut {
        chord: "Alt+T, Alt+F",
        id: "Text.MarginFoldColumn",
    },
    SeqShortcut {
        chord: "Alt+T, Alt+S",
        id: "Text.MarginSymbol",
    },
    SeqShortcut {
        chord: "Alt+T, Alt+T",
        id: "Text.MarginTimestamp",
    },
    // View ▸ Symbols 四行照原版快捷键列抄；与 shortcut_map 的同条绑定同源。
    SeqShortcut {
        chord: "Alt+T, Alt+E",
        id: "View.ToggleLineEndings",
    },
    SeqShortcut {
        chord: "Alt+T, Alt+L",
        id: "View.ToggleColumnMarker",
    },
    SeqShortcut {
        chord: "Alt+T, Alt+I",
        id: "View.ToggleIndentGuides",
    },
    SeqShortcut {
        chord: "Alt+T, Alt+W",
        id: "View.ToggleWrapSymbols",
    },
];

/// 右键菜单里显示的两段式键：margin 五列。
///
/// 它们不在菜单栏的 `shortcut_text` 表里（右键菜单自成一份），但**必须**与
/// `DEFAULT_SHORTCUT_SEQS` 的绑定逐字一致——审计里「菜单写着键、按下去不动」
/// 的那一类偏差就是从这里漏掉的。右键菜单绘制直接读这张表，不再手写字符串。
pub const CONTEXT_SEQ_ROWS: &[(&str, &str)] = &[
    ("Text.MarginBlank", "Alt+T, Alt+B"),
    ("Text.MarginNumber", "Alt+T, Alt+N"),
    ("Text.MarginFoldColumn", "Alt+T, Alt+F"),
    ("Text.MarginSymbol", "Alt+T, Alt+S"),
    ("Text.MarginTimestamp", "Alt+T, Alt+T"),
];

/// margin 列 → 右键菜单显示（且绑定表已装）的两段式键原文。
/// UI 按枚举取，不写命令 ID 字面量。
pub fn margin_seq(part: crate::vocab::MarginPart) -> &'static str {
    use crate::vocab::MarginPart;
    let id = match part {
        MarginPart::Blank => "Text.MarginBlank",
        MarginPart::Number => "Text.MarginNumber",
        MarginPart::Fold => "Text.MarginFoldColumn",
        MarginPart::Symbol => "Text.MarginSymbol",
        MarginPart::Timestamp => "Text.MarginTimestamp",
    };
    context_seq_for(id).unwrap_or("")
}

/// 右键菜单行 id → 它显示的两段式键原文。
pub fn context_seq_for(id: &str) -> Option<&'static str> {
    CONTEXT_SEQ_ROWS
        .iter()
        .find(|(candidate, _)| *candidate == id)
        .map(|(_, chord)| *chord)
}

/// 绑定表里按和弦查一条命令 ID（投影层捕获按键后调用）。
pub fn shortcut_for(chord_text: &str) -> Option<&'static str> {
    DEFAULT_SHORTCUTS
        .iter()
        .find(|s| s.chord.eq_ignore_ascii_case(chord_text))
        .map(|s| s.id)
}

/// 菜单项右侧展示的快捷键（2026-11-08 视觉基线逐项抄录）。id → 文案；
/// 没有的项返回空串（不占位）。文案照视觉基线用 `Meta+` / `Alt+` 写法，
/// 不在本程序里换算成平台符号——对照截图时一字不差才好比对。
///
/// # 这一列有两类「已知的不一致」，都有断言钉着，都不是随便可改的
///
/// 1. **与 `DEFAULT_SHORTCUTS` 有五条分歧**（本列照视觉基线，绑定表是出厂值）。
///    清单在 `DEFAULT_SHORTCUTS` 的文档段里，断言是
///    `menu_check_tests.rs::KNOWN_CHORD_DISAGREEMENTS`。
/// 2. **「标着键却没接线」这一类现已清空**：`KNOWN_UNWIRED_SHORTCUTS` 为空数组。
///    历史上在此清单、现已销账的：`Application.NewWindow`（2026-11-15）、
///    `Window.GotoSymbol`（2026-11-23，符号表 = OSC 133 命令块）、
///    `Window.SetRemoteMode`（2026-11-23，退出只读）、`Text.ToggleWordWrap`
///    （2026-11-24，软换行段表按位现算）。
///
/// 第 2 类是本轮特别定过口径的：**照 2.6.1 保真优先**。视觉基线上写着
/// `Meta+Shift+N`，菜单就该写着——把它删掉是拿保真度换整洁，方向错了。
/// 用户点那一行本来就是置灰的（`capability::describe` 说它是 `Planned`），
/// 「灰项 + 视觉基线上的键」是诚实的状态，而「少一列字」是对视觉基线的偏离。
/// 哪天又有命令从占位变成已接线，把 ID 从 `KNOWN_UNWIRED_SHORTCUTS` 删掉即可
/// （断言会因为你没删而红，这是有意的）。`Application.NewWindow` /
/// `Text.ToggleWordWrap` 就是这么销账的：绑定表补一行、清单删一项，菜单列一个字没动。
pub fn shortcut_text(id: &str) -> &'static str {
    crate::shortcut_map::default_display(id)
}

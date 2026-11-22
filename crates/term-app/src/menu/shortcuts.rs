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
/// # 与 `shortcut_text`（菜单右侧那一列）的关系：**两条数据源，已知对不上**
///
/// 这两张表指的是同一件事（哪个键叫出哪条命令），但它们各自照不同的依据抄：
/// 本表是 v0.1 的出厂绑定，`shortcut_text` 是 2026-11-08 照视觉基线逐项抄的
/// 快捷键列。**视觉基线优先**——菜单要按 2.6.1 保真，图上写什么就写什么，
/// 所以本表**不**向它对齐，五条已知分歧原样留着：
///
/// | 命令 | 本表 | 菜单列 |
/// |---|---|---|
/// | `Window.Close` | `Cmd+W` | `Meta+Shift+Q` |
/// | `Window.SplitViewHorizontally` | `Cmd+Shift+T` | `Alt+W, Alt+H` |
/// | `Window.LockScreen` | `Cmd+Shift+L` | `Meta+Alt+L` |
/// | `Window.ShowPaletteCommand` | `Cmd+Shift+P` | `Alt+;` |
/// | `Window.ShowPaletteAnything` | `Cmd+Shift+O` | `Alt+S` |
///
/// 这张清单**只许减不许增**：新写一条对不上的键，`menu_check_tests.rs` 的
/// `KNOWN_CHORD_DISAGREEMENTS` 断言会立刻红；真修好一条也要去那张表里删掉，
/// 否则同样红（清单是精确集合，不是许可额度）。改哪一边都行，但改完要一起改表。
///
/// 为什么先不对齐：**单段绑定今天就在派发**（`dialogs::shortcuts` 的
/// `handle_global_shortcuts` 读的就是本表，`Cmd+W` → `Window.Close` 全链路通），
/// 但派发只认单弦；而菜单列是用户唯一看得见的一列，它对齐视觉基线这件事
/// 比两表自洽更重要。要动这张表，得先定哪一边是真相，再连
/// `KNOWN_CHORD_DISAGREEMENTS` 一起改——那是独立一刀。
///
/// **两段式键不在这张表里**（它一个 `Chord` 都放不下），在
/// `DEFAULT_SHORTCUT_SEQS`；那张表直到 2026-11-11 才接上派发。
pub const DEFAULT_SHORTCUTS: &[Shortcut] = &[
    Shortcut {
        chord: "Cmd+Shift+P",
        id: "Window.ShowPaletteCommand",
        label: "Command Palette",
    },
    Shortcut {
        chord: "Cmd+Shift+O",
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
        chord: "Cmd+Shift+F",
        id: "Text.Find",
        label: "Find",
    },
    Shortcut {
        chord: "Cmd+G",
        id: "Text.FindNext",
        label: "Find Next",
    },
    Shortcut {
        chord: "Cmd+K",
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
        chord: "Cmd+Shift+L",
        id: "Window.LockScreen",
        label: "Lock Screen",
    },
    Shortcut {
        chord: "Cmd+W",
        id: "Window.Close",
        label: "Close Tab",
    },
    Shortcut {
        chord: "Cmd+Shift+T",
        id: "Window.SplitViewHorizontally",
        label: "Split Right",
    },
    Shortcut {
        chord: "Cmd+Shift+I",
        id: "Window.WindowList",
        label: "Window List",
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
/// 七条都在这里，**还没接线的只有一条**（`Text.ToggleWordWrap`
/// `Alt+M, Alt+W`）：数据层登记「视觉基线上写了这个键」，投影层构造时再用
/// `capability::is_implemented` 过滤——只在运行期装能真派发的那几条。
/// 之所以在构造时过滤而不是在这里分开两张表：那样「菜单列有哪些两段式行」
/// 与「这张表」就有了两个不同的集合，`menu_check_tests.rs` 得分别守两遍。
/// （`ManageOneKeys` / `ManageTriggers` 2026-11-13 已接线，`is_implemented`
/// 过滤让它们自动回到运行期表——这里不用改，是因为过滤是运行时判的。）
///
/// **前缀不许白吞按键**：未接线的两段式键第一段若装进运行期表，按下去会
/// 进前缀态却永远等不到后继动作，顺手搅浑同前缀的其他键序。
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
/// 2. **有两条命令还没接线，视觉基线上却标着键**：
///    `Window.SetRemoteMode`（`Alt+Return`）、
///    `Text.ToggleWordWrap`（`Alt+M, Alt+W`）。清单与断言见
///    `menu_check_tests.rs::KNOWN_UNWIRED_SHORTCUTS`。
///    已销账：`Application.NewWindow`（2026-11-15）、`Window.GotoSymbol`（2026-11-23，
///    符号表就是 OSC 133 命令块）。
///
/// 第 2 类是本轮特别定过口径的：**照 2.6.1 保真优先**。视觉基线上写着
/// `Meta+Shift+N`，菜单就该写着——把它删掉是拿保真度换整洁，方向错了。
/// 用户点那一行本来就是置灰的（`capability::describe` 说它是 `Planned`），
/// 「灰项 + 视觉基线上的键」是诚实的状态，而「少一列字」是对视觉基线的偏离。
/// 哪天剩下的命令接线了，把 ID 从 `KNOWN_UNWIRED_SHORTCUTS` 里删掉即可
/// （断言会因为你没删而红，这是有意的）。`Application.NewWindow` 就是这么
/// 销账的：绑定表补一行、清单删一项，菜单列一个字没动。
pub fn shortcut_text(id: &str) -> &'static str {
    match id {
        "Terminal.NewSession" => "Meta+N",
        "Application.NewWindow" => "Meta+Shift+N",
        "Terminal.ReconnectSession" => "Return",
        "Terminal.OpenSession" => "Meta+O",
        "Terminal.Save" => "Meta+S",
        "Terminal.SaveSelectionAs" => "Meta+Shift+S",
        "Window.CloseActiveView" => "Meta+Shift+W",
        "Window.Close" => "Meta+Shift+Q",
        "Application.Exit" => "Alt+F4",
        // 2026-11-08 视觉基线逐项抄录：Goto / Tool / Mode / View / Window 的
        // 快捷键列。文案与截图一字不差，供并排比对。
        //
        // D3：这一行与 `DEFAULT_SHORTCUTS` 里的 `Alt+D` 现在**一致**——
        // 之前只在这里标着，派发层根本不知道有这条绑定。
        "Window.GotoAddressBar" => "Alt+D",
        "Window.GotoSession" => "Alt+O",
        "Window.GotoSymbol" => "Alt+'",
        "Text.LineUp" => "Up",
        "Text.LineDown" => "Down",
        "Terminal.SyncInput" => "Meta+Shift+M",
        "Terminal.ManageOneKeys" => "Meta+T, Meta+K",
        "Terminal.ManageTriggers" => "Meta+T, Meta+T",
        "Terminal.ManageTunnels" => "Meta+Shift+U",
        "Window.SetRemoteMode" => "Alt+Return",
        "Text.ToggleHexEdit" => "Alt+M, Alt+H",
        "Text.ToggleWordWrap" => "Alt+M, Alt+W",
        "Window.ToggleFocusMode" => "Ctrl+Return",
        "Window.ToggleFullScreen" => "Meta+F11",
        "Window.LockScreen" => "Meta+Alt+L",
        "Window.ShowPaletteAnything" => "Alt+S",
        "Window.ShowPaletteCommand" => "Alt+;",
        "Window.SplitViewHorizontally" => "Alt+W, Alt+H",
        "Window.SplitViewVertically" => "Alt+W, Alt+V",
        "Window.CloseActiveGroup" => "Alt+W, Alt+G",
        "Window.ActivateNextView" => "Meta+PgDown",
        "Window.ActivatePreviousView" => "Meta+PgUp",
        _ => "",
    }
}

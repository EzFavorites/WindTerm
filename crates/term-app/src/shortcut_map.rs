//! 快捷键的**唯一映射层**：命令 ID ↔ 按键序列，带平台展示与用户覆盖。
//!
//! 旧实现有三张手写表（`DEFAULT_SHORTCUTS` 派发、`DEFAULT_SHORTCUT_SEQS`
//! 两段式、`menu::shortcut_text` 菜单列），同一个语义被抄三遍；用户重绑又以
//! `DEFAULT_SHORTCUTS` 的**数组下标**为键写进 prefs，表一改顺序就绑到别的命令上。
//!
//! 这里收成一份：
//!
//! - [`BindingRow`] 是语义事实：`命令 ID → 按键序列文本`，`Cmd` 表示「平台主修饰键」
//!   （macOS = Command / 上游写作 `Meta`，Win/Linux = Ctrl），`Ctrl` 表示字面 Ctrl；
//! - [`KeyBindingMap`] 把出厂表与用户覆盖合成一张表，提供正向（命令 → 展示）、
//!   反向（按键 → 命令）与冲突检测；
//! - [`Platform`] 只负责**展示**：同一份绑定在 macOS 显示 `Meta+F`，在 Windows 显示
//!   `Ctrl+F`。匹配仍走 `shortcuts` 纯层——那层本来就把 Cmd/Ctrl/Meta 归一。
//!
//! `menu::shortcuts::shortcut_text` 与两张 DEFAULT_* 表暂时保留为兼容门面，
//! 所有新代码都必须走本模块；迁移完成后再由测试驱动删除旧表。

use crate::shortcuts::{self, Chord};
use std::sync::OnceLock;

// prefs 两份字段的收敛规则拆到子模块，给本文件的 4j 余量腾地方；调用方看到的
// 路径仍是 `term_app::shortcut_map::*`，这里只做透明再导出。
mod prefs;
pub use prefs::{drop_rejected_bindings, normalized_prefs_fields, reset_prefs_fields};

/// 出厂展示文案的缓存。每个 id 只 leak 一次（条目数有界 = 绑定表长度），
/// 让兼容接口 `menu::shortcut_text` 仍返回 `&'static str` 而不再手写第二份。
static DISPLAY_CACHE: OnceLock<std::collections::HashMap<&'static str, &'static str>> =
    OnceLock::new();

/// 命令的默认展示文案（macOS 金标准写法）。菜单列的历史接口走这里。
pub fn default_display(id: &str) -> &'static str {
    let cache = DISPLAY_CACHE.get_or_init(|| {
        let mut rows = std::collections::HashMap::new();
        for binding in DEFAULT_BINDINGS {
            rows.entry(binding.id).or_insert_with(|| {
                let display: &'static str =
                    Box::leak(display_chord(binding.chord, Platform::Macos).into_boxed_str());
                display
            });
        }
        rows
    });
    cache.get(id).copied().unwrap_or("")
}

/// 运行平台。`current()` 是唯一的宿主事实入口，夹具可用环境变量覆盖。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Platform {
    Macos,
    Windows,
    Linux,
}

impl Platform {
    pub fn current() -> Self {
        if cfg!(target_os = "macos") {
            Platform::Macos
        } else if cfg!(target_os = "windows") {
            Platform::Windows
        } else {
            Platform::Linux
        }
    }

    /// 平台主修饰键在菜单里的写法。参考图里 macOS 用 `Meta`，不是 `⌘`。
    pub fn primary_label(self) -> &'static str {
        match self {
            Platform::Macos => "Meta",
            Platform::Windows | Platform::Linux => "Ctrl",
        }
    }
}

/// 一条语义绑定。`chord` 用 `Cmd+` 表示平台主修饰键，字面 Ctrl 写 `Ctrl+`。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BindingRow {
    pub id: &'static str,
    pub chord: &'static str,
}

/// 出厂绑定：菜单列的文字与派发的键都从这里派生。
///
/// 同一命令可以有多行（别名），**第一行是菜单展示用的那一条**。例如
/// `Window.ActivateNextView` 同时认 `Cmd+PgDown` 与 `Alt+]`，菜单只写前者。
pub const DEFAULT_BINDINGS: &[BindingRow] = &[
    // ── Session / Edit / Search ──
    BindingRow { id: "Terminal.NewSession", chord: "Cmd+N" },
    BindingRow { id: "Application.NewWindow", chord: "Cmd+Shift+N" },
    BindingRow { id: "Terminal.ReconnectSession", chord: "Return" },
    BindingRow { id: "Terminal.OpenSession", chord: "Cmd+O" },
    BindingRow { id: "Terminal.Save", chord: "Cmd+S" },
    BindingRow { id: "Terminal.SaveSelectionAs", chord: "Cmd+Shift+S" },
    BindingRow { id: "Text.Copy", chord: "Shift+Del" },
    BindingRow { id: "Text.Paste", chord: "Shift+Ins" },
    BindingRow { id: "Terminal.PasteSelectedText", chord: "Cmd+Shift+X" },
    BindingRow { id: "Text.Find", chord: "Cmd+F" },
    BindingRow { id: "Text.FindNext", chord: "Alt+F3" },
    BindingRow { id: "Text.FindPrevious", chord: "Shift+F3" },
    BindingRow { id: "Text.FindNextSelection", chord: "Cmd+F3" },
    BindingRow { id: "Text.FindPreviousSelection", chord: "Cmd+Shift+F3" },
    // ── Goto / Tool / Mode / View / Window ──
    BindingRow { id: "Window.CloseActiveView", chord: "Cmd+Shift+W" },
    BindingRow { id: "Window.Close", chord: "Cmd+Shift+Q" },
    BindingRow { id: "Application.Exit", chord: "Alt+F4" },
    BindingRow { id: "Window.GotoAddressBar", chord: "Alt+D" },
    BindingRow { id: "Window.GotoSession", chord: "Alt+O" },
    BindingRow { id: "Window.GotoSymbol", chord: "Alt+'" },
    BindingRow { id: "Window.GotoLine", chord: "Alt+G" },
    // 官网 page/27：Remote Mode 下 Ctrl+Shift+←/→ 跳上/下一个 prompt 行
    // （OSC 133 命令块）；Local Mode 对应 zk/zj，在 vim.rs 里解析。
    BindingRow { id: "Text.GotoPreviousCommand", chord: "Cmd+Shift+Left" },
    BindingRow { id: "Text.GotoNextCommand", chord: "Cmd+Shift+Right" },
    // View ▸ Bars / Panes 的短标签 + 两段式快捷键（参考图逐项抄）。
    BindingRow { id: "Window.ToggleMenuBar", chord: "Alt+W, Alt+M" },
    BindingRow { id: "Window.ToggleStatusBar", chord: "Alt+W, Alt+U" },
    BindingRow { id: "Window.ToggleQuickBar", chord: "Alt+W, Alt+Q" },
    BindingRow { id: "Window.TogglePaneExplorer", chord: "Alt+W, Alt+E" },
    BindingRow { id: "Window.TogglePaneFiler", chord: "Alt+W, Alt+F" },
    BindingRow { id: "Window.TogglePaneOutline", chord: "Alt+W, Alt+L" },
    BindingRow { id: "Window.TogglePaneSender", chord: "Alt+W, Alt+X" },
    BindingRow { id: "Window.TogglePaneSession", chord: "Alt+W, Alt+S" },
    BindingRow { id: "Window.TogglePaneShell", chord: "Alt+W, Alt+C" },
    BindingRow { id: "Window.TogglePaneScript", chord: "Alt+W, Alt+P" },
    BindingRow { id: "Window.TogglePaneTransfer", chord: "Alt+W, Alt+T" },
    BindingRow { id: "Text.LineUp", chord: "Up" },
    BindingRow { id: "Text.LineDown", chord: "Down" },
    BindingRow { id: "Terminal.SyncInput", chord: "Cmd+Shift+M" },
    BindingRow { id: "Terminal.ManageOneKeys", chord: "Cmd+T, Cmd+K" },
    BindingRow { id: "Terminal.ManageTriggers", chord: "Cmd+T, Cmd+T" },
    BindingRow { id: "Terminal.ManageTunnels", chord: "Cmd+Shift+U" },
    BindingRow { id: "Window.SetRemoteMode", chord: "Alt+Return" },
    BindingRow { id: "Text.ToggleHexEdit", chord: "Alt+M, Alt+H" },
    BindingRow { id: "Text.ToggleWordWrap", chord: "Alt+M, Alt+W" },
    BindingRow { id: "Window.ToggleFocusMode", chord: "Ctrl+Return" },
    BindingRow { id: "Window.ToggleFullScreen", chord: "Cmd+F11" },
    BindingRow { id: "Window.LockScreen", chord: "Cmd+Alt+L" },
    BindingRow { id: "Window.ShowPaletteAnything", chord: "Alt+S" },
    BindingRow { id: "Window.ShowPaletteCommand", chord: "Alt+;" },
    BindingRow { id: "Window.SplitViewHorizontally", chord: "Alt+W, Alt+H" },
    BindingRow { id: "Window.SplitViewVertically", chord: "Alt+W, Alt+V" },
    BindingRow { id: "Window.CloseActiveGroup", chord: "Alt+W, Alt+G" },
    BindingRow { id: "Window.ActivateNextView", chord: "Cmd+PgDown" },
    BindingRow { id: "Window.ActivatePreviousView", chord: "Cmd+PgUp" },
    BindingRow { id: "Terminal.TabReopenClosed", chord: "Cmd+Shift+T" },
    BindingRow { id: "Window.WindowList", chord: "Cmd+Shift+I" },
    // Tmux-Integration 文档的四个面板快捷键：Alt+X 杀当前窗格、Alt+方向键选窗格。
    // 都是无目标命令，作用对象是 tmux 的当前窗格（由 tmux 自己解析）。
    BindingRow { id: "Tmux.KillPaneActive", chord: "Alt+X" },
    BindingRow { id: "Tmux.SelectPaneLeft", chord: "Alt+ArrowLeft" },
    BindingRow { id: "Tmux.SelectPaneRight", chord: "Alt+ArrowRight" },
    BindingRow { id: "Tmux.SelectPaneAbove", chord: "Alt+ArrowUp" },
    BindingRow { id: "Tmux.SelectPaneBelow", chord: "Alt+ArrowDown" },
    // Common Keyboard Shortcuts 的其余组合：Alt+B 面板、Alt+Z 缩放、
    // Alt+- / Alt+Shift+- 左右分屏、Alt+[ / ] 切 window。
    // Alt+[ / ] 在旧表是「切换视图」的别名，但与 tmux 文档冲突；菜单列的
    // 视图切换仍是 Meta+PgUp/PgDown，所以把 Alt 那两条让给 tmux。
    BindingRow { id: "Window.ShowPaletteCommand", chord: "Alt+B" },
    BindingRow { id: "Tmux.TogglePaneZoom", chord: "Alt+Z" },
    BindingRow { id: "Tmux.SplitHorizontal", chord: "Alt+Minus" },
    BindingRow { id: "Tmux.SplitPaneLeft", chord: "Alt+Shift+Minus" },
    BindingRow { id: "Tmux.SelectWindowPrevious", chord: "Alt+OpenBracket" },
    BindingRow { id: "Tmux.SelectWindowNext", chord: "Alt+CloseBracket" },
    // ── 右键菜单 margin 五列（与 CONTEXT_SEQ_ROWS 同源）──
    BindingRow { id: "Text.MarginBlank", chord: "Alt+T, Alt+B" },
    BindingRow { id: "Text.MarginNumber", chord: "Alt+T, Alt+N" },
    // View ▸ Folds 六条（金图 13-view-folds-submenu.png）：显示列与派发共用本表。
    // `Ctrl` 是字面 Ctrl（原图 Unfold 那条就是 Ctrl+Alt+=，不是 Meta）。
    BindingRow { id: "View.Fold", chord: "Cmd+Alt+Minus" },
    BindingRow { id: "View.Unfold", chord: "Ctrl+Alt+Equal" },
    BindingRow { id: "View.FoldInside", chord: "Cmd+9" },
    BindingRow { id: "View.UnfoldInside", chord: "Cmd+0" },
    BindingRow { id: "View.FoldAll", chord: "Cmd+Alt+OpenBracket" },
    BindingRow { id: "View.UnfoldAll", chord: "Cmd+Alt+CloseBracket" },
    BindingRow { id: "Text.MarginFoldColumn", chord: "Alt+T, Alt+F" },
    BindingRow { id: "Text.MarginSymbol", chord: "Alt+T, Alt+S" },
    BindingRow { id: "Text.MarginTimestamp", chord: "Alt+T, Alt+T" },
];

/// 把绑定文本里的 `Cmd+` 换成平台主修饰键写法，其余 token 原样保留。
///
/// 两段式键按 `, ` 分段分别格式化；字面 `Ctrl+` 在任何平台都显示 `Ctrl+`，
/// 这正是参考图里 `Ctrl+Return` 与 `Meta+Alt+L` 并存的原因。
pub fn display_chord(chord: &str, platform: Platform) -> String {
    chord
        .split(',')
        .map(|part| {
            part.trim()
                .split('+')
                .map(|token| {
                    if token.eq_ignore_ascii_case("Cmd") {
                        return platform.primary_label();
                    }
                    // 解析层的键名（shortcuts.rs 的 KEY_NAMES）与参考图里的菜单
                    // 写法不同：前者写 Minus/OpenBracket，参考图写 -/[。
                    match token {
                        "Minus" => "-",
                        "Equal" => "=",
                        "OpenBracket" => "[",
                        "CloseBracket" => "]",
                        _ => token,
                    }
                })
                .collect::<Vec<_>>()
                .join("+")
        })
        .collect::<Vec<_>>()
        .join(", ")
}

/// 旧 prefs 里 `shortcut_overrides` 的下标**只对这一份快照有效**。
///
/// `menu::DEFAULT_SHORTCUTS` 后来在中间插过行（`Terminal.TabReopenClosed`），
/// 拿当前表反查历史下标会把「11 = WindowList」读成「11 = TabReopenClosed」，
/// 用户明明重绑的是窗口列表，升级后却变成重开标签——而追加在尾部的行还会让
/// 原本越界（应当丢弃）的条目静默绑到一个意外命令上。这张表**只用于迁移**，
/// 不参与展示与派发；新代码一律走 [`DEFAULT_BINDINGS`]。
///
/// 快照取自引入按 ID 存储之前那张 12 行的出厂表（`Window.WindowList` 是最后一行）。
pub const LEGACY_SHORTCUT_IDS: &[&str] = &[
    "Window.ShowPaletteCommand",
    "Window.ShowPaletteAnything",
    "Terminal.NewSession",
    "Application.NewWindow",
    "Text.Find",
    "Text.FindNext",
    "Window.GotoLine",
    "Window.GotoAddressBar",
    "Window.LockScreen",
    "Window.Close",
    "Window.SplitViewHorizontally",
    "Window.WindowList",
];

/// 一次性迁移的结果。三份数据一起返回是为了让调用方**在同一写盘动作里**
/// 同时写新字段、清旧字段——只写新字段会让旧文件在 Reset 后把绑定「复活」。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LegacyMigration {
    /// 写进 `prefs.shortcut_bindings` 的结果。
    pub bindings: Vec<(String, String)>,
    /// 写进 `prefs.shortcut_overrides` 的结果。迁移成功后恒为空：旧字段必须清空。
    pub overrides: Vec<(usize, String)>,
    /// 下标越界、无法迁移的条目（只用于日志/提示，绝不绑到猜测的命令）。
    pub dropped_indices: Vec<usize>,
}

/// 把旧下标覆盖迁到命令 ID，并给出「旧字段该写什么」。
///
/// `current_bindings`（新字段）非空时以它为准，旧字段一律清空：两份字段同时
/// 有值是重启后行为不确定的根源。新字段为空时才按下标快照迁移；越界条目
/// 记进 [`LegacyMigration::dropped_indices`] 而不是静默丢弃。
pub fn migrate_legacy_with_report(
    overrides: &[(usize, String)],
    current_bindings: &[(String, String)],
) -> LegacyMigration {
    if !current_bindings.is_empty() {
        return LegacyMigration {
            bindings: current_bindings.to_vec(),
            overrides: Vec::new(),
            dropped_indices: Vec::new(),
        };
    }
    let mut bindings = Vec::new();
    let mut dropped_indices = Vec::new();
    for (index, chord) in overrides {
        match LEGACY_SHORTCUT_IDS.get(*index) {
            Some(id) => bindings.push(((*id).to_string(), chord.clone())),
            None => dropped_indices.push(*index),
        }
    }
    LegacyMigration {
        bindings,
        overrides: Vec::new(),
        dropped_indices,
    }
}

/// 兼容旧调用方的简版入口：只取迁出的新绑定（旧字段的清空由写盘方负责）。
pub fn migrate_legacy(overrides: &[(usize, String)]) -> Vec<(String, String)> {
    migrate_legacy_with_report(overrides, &[]).bindings
}

/// 两段按键是否是真前缀（差一段以上、且短的逐段等于长的开头）。
///
/// 相等不算前缀：那是 [`KeyBindingMap::conflicts`] 的另一半判据。
fn is_proper_prefix(short: &[Chord], long: &[Chord]) -> bool {
    short.len() < long.len() && short.iter().zip(long).all(|(a, b)| a == b)
}

/// 出厂表 + 用户覆盖合成的绑定表。
#[derive(Clone, Debug)]
pub struct KeyBindingMap {
    rows: Vec<(String, String)>,
    /// 被用户覆盖过的命令 ID，写回 prefs 时只导这些，不把出厂表整张落盘。
    user_ids: Vec<String>,
    /// 与先前用户绑定冲突、被显式回滚的条目（手改 prefs 时不能静默忽略）。
    rejected_user_bindings: Vec<(String, String)>,
    platform: Platform,
}

impl KeyBindingMap {
    /// 合成绑定表。
    ///
    /// 冲突规则：同一命令的多条用户绑定按输入顺序全部保留（别名）；**不同命令**
    /// 抢同一串按键（含前缀）时，先出现的用户绑定胜出，后来的那条回滚并记进
    /// [`KeyBindingMap::rejected_user_bindings`]。用户行排在出厂行前面，所以
    /// 派发时用户绑定胜出；其它命令的出厂行仍留在表里，供 `conflicts()` 报告，
    /// 重绑编辑器据此提示「已被占用」并拒绝这次写入，不会偷偷把默认命令摘掉。
    pub fn new(user: &[(String, String)], platform: Platform) -> Self {
        let mut accepted: Vec<(String, String)> = Vec::new();
        let mut rejected_user_bindings = Vec::new();
        for (id, chord) in user {
            let seq = shortcuts::parse_chord_seq(chord);
            let clash = seq.as_ref().is_some_and(|want| {
                accepted.iter().any(|(other_id, other_chord)| {
                    if other_id == id {
                        return false;
                    }
                    shortcuts::parse_chord_seq(other_chord).is_some_and(|other| {
                        other == *want || is_proper_prefix(&other, want) || is_proper_prefix(want, &other)
                    })
                })
            });
            if clash {
                rejected_user_bindings.push((id.clone(), chord.clone()));
                continue;
            }
            accepted.push((id.clone(), chord.clone()));
        }
        let user_ids: Vec<String> = accepted.iter().map(|(id, _)| id.clone()).collect();
        let mut rows: Vec<(String, String)> = DEFAULT_BINDINGS
            .iter()
            // 覆盖该命令的所有出厂别名，避免旧别名在重绑后继续触发。
            .filter(|r| !user_ids.iter().any(|id| id == r.id))
            .map(|r| (r.id.to_string(), r.chord.to_string()))
            .collect();
        // 展示顺序必须与出厂表一致（快捷键对话框的基线快照钉住了行序），
        // 用户行仍追加在尾部；派发时的优先级由 `dispatch` 的两段查找保证。
        rows.extend(accepted);
        Self { rows, user_ids, rejected_user_bindings, platform }
    }

    /// 被回滚的用户绑定：调用方写盘时应当用 [`drop_rejected_bindings`] 把这些
    /// 条目一并删掉，避免下次重启再冲突（启动清理也走同一个写口规则）。
    pub fn rejected_user_bindings(&self) -> &[(String, String)] {
        &self.rejected_user_bindings
    }

    pub fn platform(&self) -> Platform {
        self.platform
    }

    /// 菜单右侧那一列：命令的默认（或用户重绑后的）按键，按平台展示。
    pub fn display(&self, id: &str) -> String {
        self.rows
            .iter()
            .find(|(row_id, _)| row_id == id)
            .map(|(_, chord)| display_chord(chord, self.platform))
            .unwrap_or_default()
    }

    /// 一个命令的所有按键序列（含别名），用于派发。
    pub fn sequences_for(&self, id: &str) -> Vec<Vec<Chord>> {
        self.rows
            .iter()
            .filter(|(row_id, _)| row_id == id)
            .filter_map(|(_, chord)| shortcuts::parse_chord_seq(chord))
            .collect()
    }

    /// 一串按键 → 命令 ID。比较走 typed `Chord`，不受 `Meta/Cmd/Ctrl` 写法影响。
    ///
    /// 两遍查找：用户绑定先 match，之后才是出厂表。这样即使手改 prefs 让一条
    /// 用户绑定与另一条出厂绑定撞键，用户的那条仍然胜出（冲突由 `conflicts()`
    /// 如实报告，重绑编辑器会拦下这种写入）。
    pub fn dispatch(&self, pressed: &[Chord]) -> Option<&str> {
        for (id, chord) in self.rows.iter().filter(|(id, _)| self.user_ids.contains(id)) {
            if shortcuts::parse_chord_seq(chord).as_deref() == Some(pressed) {
                return Some(id.as_str());
            }
        }
        for (id, chord) in &self.rows {
            if shortcuts::parse_chord_seq(chord).as_deref() == Some(pressed) {
                return Some(id.as_str());
            }
        }
        None
    }

    /// 全部绑定（命令 ID, 展示文本），供快捷键设置页列表使用。
    pub fn rows(&self) -> Vec<(String, String)> {
        self.rows
            .iter()
            .map(|(id, chord)| (id.clone(), display_chord(chord, self.platform)))
            .collect()
    }

    /// 用户重绑后的绑定（按命令 ID 写回 prefs 的形态）。
    pub fn user_bindings(&self) -> Vec<(String, String)> {
        self.rows
            .iter()
            .filter(|(id, _)| self.user_ids.contains(id))
            .cloned()
            .collect()
    }

    /// 冲突检测：两个**不同命令**的按键序列相等，或者一条是真前缀。
    ///
    /// 前缀也算冲突：`Alt+W` 会把 `Alt+W, Alt+H` 的第一段吃掉，于是后一条
    /// 永远等不到第二段；两段式绑定表里这种重叠比完全相等更常见，也更难查。
    /// 返回 `(较短/先出现的那条按键文本, 命令 A, 命令 B)`。
    pub fn conflicts(&self) -> Vec<(String, String, String)> {
        let mut out = Vec::new();
        for (i, (id, chord)) in self.rows.iter().enumerate() {
            let seq = match shortcuts::parse_chord_seq(chord) {
                Some(seq) => seq,
                None => continue,
            };
            for (other_id, other_chord) in &self.rows[i + 1..] {
                if id == other_id {
                    continue;
                }
                if let Some(other_seq) = shortcuts::parse_chord_seq(other_chord) {
                    let collide = other_seq == seq
                        || is_proper_prefix(&seq, &other_seq)
                        || is_proper_prefix(&other_seq, &seq);
                    if collide {
                        out.push((display_chord(chord, self.platform), id.clone(), other_id.clone()));
                    }
                }
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn platform_switch_only_changes_the_primary_modifier_label() {
        let map = KeyBindingMap::new(&[], Platform::Macos);
        assert_eq!(map.display("Text.Find"), "Meta+F");
        assert_eq!(map.display("Window.ToggleFocusMode"), "Ctrl+Return");
        assert_eq!(
            map.display("Window.SplitViewHorizontally"),
            "Alt+W, Alt+H"
        );
        let win = KeyBindingMap::new(&[], Platform::Windows);
        assert_eq!(win.display("Text.Find"), "Ctrl+F");
        assert_eq!(win.display("Window.ToggleFocusMode"), "Ctrl+Return");
        assert_eq!(
            win.display("Window.SplitViewHorizontally"),
            "Alt+W, Alt+H"
        );
    }

    #[test]
    fn user_binding_is_keyed_by_command_not_array_index() {
        let map = KeyBindingMap::new(
            &[("Text.Find".to_string(), "Cmd+Shift+F".to_string())],
            Platform::Macos,
        );
        assert_eq!(map.display("Text.Find"), "Meta+Shift+F");
        let pressed = shortcuts::parse_chord_seq("Cmd+Shift+F").unwrap();
        assert_eq!(map.dispatch(&pressed), Some("Text.Find"));
        // 旧和弦不再触发。
        let old = shortcuts::parse_chord_seq("Cmd+F").unwrap();
        assert_ne!(map.dispatch(&old), Some("Text.Find"));
    }

    #[test]
    fn legacy_indices_are_aligned_to_the_frozen_snapshot_not_the_current_table() {
        // 历史下标 3 = Application.NewWindow、6 = GotoLine、11 = WindowList。
        // 当前表里 11 已被插入的 TabReopenClosed 占掉，拿当前表反查必然错绑。
        let migrated = migrate_legacy(&[
            (3, "Cmd+Shift+N".to_string()),
            (6, "Cmd+K".to_string()),
            (11, "Cmd+L".to_string()),
        ]);
        assert_eq!(
            migrated,
            vec![
                ("Application.NewWindow".to_string(), "Cmd+Shift+N".to_string()),
                ("Window.GotoLine".to_string(), "Cmd+K".to_string()),
                ("Window.WindowList".to_string(), "Cmd+L".to_string()),
            ]
        );
        assert_eq!(LEGACY_SHORTCUT_IDS.len(), 12);
        assert_eq!(LEGACY_SHORTCUT_IDS[11], "Window.WindowList");
        assert_ne!(
            crate::menu::DEFAULT_SHORTCUTS[11].id,
            LEGACY_SHORTCUT_IDS[11],
            "当前表已经插过行——必须用快照而不是当前下标"
        );
    }

    #[test]
    fn out_of_range_legacy_indices_are_reported_not_silently_bound() {
        let migration = migrate_legacy_with_report(&[(14, "X".into()), (999, "Y".into())], &[]);
        assert!(migration.bindings.is_empty());
        assert_eq!(migration.dropped_indices, vec![14, 999]);
        assert!(migration.overrides.is_empty(), "旧字段必须在写盘时清空");
    }

    #[test]
    fn a_new_field_wins_and_the_old_field_is_cleared() {
        let new = vec![("Text.Find".to_string(), "Cmd+F".to_string())];
        let migration = migrate_legacy_with_report(&[(11, "Cmd+L".into())], &new);
        assert_eq!(migration.bindings, new);
        assert!(migration.overrides.is_empty());
    }

    #[test]
    fn a_conflicting_user_binding_is_rolled_back_and_recorded() {
        let map = KeyBindingMap::new(
            &[
                ("Text.Find".to_string(), "Cmd+Shift+F".to_string()),
                ("Text.Paste".to_string(), "Cmd+Shift+F".to_string()),
            ],
            Platform::Macos,
        );
        assert_eq!(map.rejected_user_bindings(), &[("Text.Paste".to_string(), "Cmd+Shift+F".to_string())]);
        assert!(!map.user_bindings().iter().any(|(id, _)| id == "Text.Paste"));
        let pressed = shortcuts::parse_chord_seq("Cmd+Shift+F").unwrap();
        assert_eq!(map.dispatch(&pressed), Some("Text.Find"));
    }

    #[test]
    fn an_equal_chord_from_a_user_wins_over_a_default() {
        // 手改 prefs 把 Paste 绑到 Cmd+F：用户那条派发命中，默认 Find 仍留在
        // 表里被 conflicts() 报告，编辑器据此拒绝这次写入（不偷偷摘掉 Find）。
        let map = KeyBindingMap::new(
            &[("Text.Paste".to_string(), "Cmd+F".to_string())],
            Platform::Macos,
        );
        let pressed = shortcuts::parse_chord_seq("Cmd+F").unwrap();
        assert_eq!(map.dispatch(&pressed), Some("Text.Paste"));
        assert!(map.conflicts().iter().any(|(_, a, b)| {
            (a == "Text.Find" && b == "Text.Paste") || (a == "Text.Paste" && b == "Text.Find")
        }));
    }

    #[test]
    fn a_true_prefix_is_a_conflict_but_equal_length_is_not() {
        let map = KeyBindingMap::new(
            &[("Text.Find".to_string(), "Alt+W".to_string())],
            Platform::Macos,
        );
        assert!(
            map.conflicts().iter().any(|(_, a, b)| {
                (a == "Text.Find" && b == "Window.ToggleMenuBar")
                    || (a == "Window.ToggleMenuBar" && b == "Text.Find")
            }),
            "Alt+W 是 Alt+W, Alt+M 的真前缀，必须报冲突：{:?}",
            map.conflicts()
        );
        // 两条两段式键共享第一段但互不为前缀，不算冲突。
        let map = KeyBindingMap::new(&[], Platform::Macos);
        assert!(
            !map.conflicts().iter().any(|(_, a, b)| {
                (a == "Text.ToggleHexEdit" && b == "Text.ToggleWordWrap")
                    || (a == "Text.ToggleWordWrap" && b == "Text.ToggleHexEdit")
            })
        );
    }

    #[test]
    fn two_commands_on_the_same_chord_are_reported_as_a_conflict() {
        let map = KeyBindingMap::new(
            &[("Text.Paste".to_string(), "Cmd+F".to_string())],
            Platform::Macos,
        );
        let conflicts = map.conflicts();
        assert!(conflicts.iter().any(|(_, a, b)| {
            (a == "Text.Find" && b == "Text.Paste")
                || (a == "Text.Paste" && b == "Text.Find")
        }));
    }
}

//! 快捷键的**唯一映射层**：命令 ID ↔ 按键序列，带平台展示与用户覆盖。
//!
//! 旧实现有三张手写表（`DEFAULT_SHORTCUTS` 派发、`DEFAULT_SHORTCUT_SEQS`
//! 两段式、`menu::shortcut_text` 菜单列），同一个语义被抄三遍；用户重绑又以
//! `DEFAULT_SHORTCUTS` 的**数组下标**为键写进 prefs，表一改顺序就绑到别的命令上。
//!
//! 这里收成一份：
//!
//! - [`BindingRow`] 是语义事实：`命令 ID → 按键序列文本`，`Cmd` 表示「平台主修饰键」
//!   （macOS = Command / 原版写作 `Meta`，Win/Linux = Ctrl），`Ctrl` 表示字面 Ctrl；
//! - [`KeyBindingMap`] 把出厂表与用户覆盖合成一张表，提供正向（命令 → 展示）、
//!   反向（按键 → 命令）与冲突检测；
//! - [`Platform`] 只负责**展示**：同一份绑定在 macOS 显示 `Meta+F`，在 Windows 显示
//!   `Ctrl+F`。匹配仍走 `shortcuts` 纯层——那层本来就把 Cmd/Ctrl/Meta 归一。
//!
//! `menu::shortcuts::shortcut_text` 与两张 DEFAULT_* 表暂时保留为兼容门面，
//! 所有新代码都必须走本模块；迁移完成后再由测试驱动删除旧表。

use crate::shortcuts::{self, Chord};
use std::sync::OnceLock;

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

    /// 平台主修饰键在菜单里的写法。原版 macOS 截图用 `Meta`，不是 `⌘`。
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
    // View ▸ Bars / Panes 的短标签 + 两段式快捷键（原版截图逐项抄）。
    BindingRow { id: "Window.ToggleMenuBar", chord: "Alt+W, Alt+M" },
    BindingRow { id: "Window.ToggleStatusBar", chord: "Alt+W, Alt+U" },
    BindingRow { id: "Window.ToggleQuickBar", chord: "Alt+W, Alt+Q" },
    BindingRow { id: "Window.TogglePaneExplorer", chord: "Alt+W, Alt+E" },
    BindingRow { id: "Window.TogglePaneFiler", chord: "Alt+W, Alt+F" },
    BindingRow { id: "Window.TogglePaneOutline", chord: "Alt+W, Alt+L" },
    BindingRow { id: "Window.TogglePaneSender", chord: "Alt+W, Alt+X" },
    BindingRow { id: "Window.TogglePaneSession", chord: "Alt+W, Alt+S" },
    BindingRow { id: "Window.TogglePaneShell", chord: "Alt+W, Alt+C" },
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
/// 这正是原版 macOS 截图里 `Ctrl+Return` 与 `Meta+Alt+L` 并存的原因。
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
                    // 解析层的键名（shortcuts.rs 的 KEY_NAMES）与菜单列的原版
                    // 写法不同：前者写 Minus/OpenBracket，原图写 -/[。
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

/// 把旧 prefs 里以数组下标为键的覆盖迁到「命令 ID 为键」。
/// 下标越界的条目直接丢弃（老文件残留不该让程序 panic，也不该绑到别的命令）。
pub fn migrate_legacy(overrides: &[(usize, String)]) -> Vec<(String, String)> {
    overrides
        .iter()
        .filter_map(|(i, chord)| {
            crate::menu::DEFAULT_SHORTCUTS
                .get(*i)
                .map(|s| (s.id.to_string(), chord.clone()))
        })
        .collect()
}

/// 出厂表 + 用户覆盖合成的绑定表。
#[derive(Clone, Debug)]
pub struct KeyBindingMap {
    rows: Vec<(String, String)>,
    /// 被用户覆盖过的命令 ID，写回 prefs 时只导这些，不把出厂表整张落盘。
    user_ids: Vec<String>,
    platform: Platform,
}

impl KeyBindingMap {
    pub fn new(user: &[(String, String)], platform: Platform) -> Self {
        let mut rows: Vec<(String, String)> = DEFAULT_BINDINGS
            .iter()
            .map(|r| (r.id.to_string(), r.chord.to_string()))
            .collect();
        for (id, chord) in user {
            // 覆盖该命令的**所有**出厂别名，避免旧别名在重绑后继续触发。
            rows.retain(|(row_id, _)| row_id != id);
            rows.push((id.clone(), chord.clone()));
        }
        let user_ids = user.iter().map(|(id, _)| id.clone()).collect();
        Self { rows, user_ids, platform }
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
    pub fn dispatch(&self, pressed: &[Chord]) -> Option<&str> {
        self.rows.iter().find_map(|(id, chord)| {
            let want = shortcuts::parse_chord_seq(chord)?;
            (want.as_slice() == pressed).then_some(id.as_str())
        })
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

    /// 冲突检测：两个不同命令绑到同一串按键。
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
                    if other_seq == seq {
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
    fn legacy_index_overrides_are_migrated_to_command_ids() {
        let idx = crate::menu::DEFAULT_SHORTCUTS
            .iter()
            .position(|s| s.id == "Terminal.NewSession")
            .unwrap();
        let migrated = migrate_legacy(&[(idx, "Cmd+Shift+N".to_string()), (999, "X".to_string())]);
        assert_eq!(migrated, vec![("Terminal.NewSession".to_string(), "Cmd+Shift+N".to_string())]);
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

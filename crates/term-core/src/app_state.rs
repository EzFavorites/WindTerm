//! 应用状态快照的**纯数据**（ADR-0027）。
//!
//! 这是「脚本能读到什么」的唯一形状：面板可见性与停靠位、主区窗格数、
//! 窗口旗标、偏好白名单、以及两个互相独立的版本号。放 `term-core` 的理由与
//! `surface` / `script_caps` 同一条：原型只有一份，`term-app`、`term-script`、
//! `apps/desktop` 都从这一份取，谁也不许自己再拼一个「差不多」的结构。
//!
//! # 三条纪律
//!
//! 1. **面板恒十条**（`PanelId::ALL` 的集合是编译期事实）。用「数组长度变化」
//!    表达面板增删会让客户端写两套差分逻辑，而面板根本不能在运行期增删。
//! 2. **两个版本号分家**：`rev` 只标记界面状态（面板 / 布局 / 窗口），
//!    `prefs_rev` 只标记偏好。合成一个的话，「靠 rev 判断要不要重取」会漏掉偏好变化。
//! 3. **`rect` 用整数**（逻辑点），与落盘形态 `[i32; 4]` 一致；浮点只出现在
//!    `opacity`。本模块不读文件、不读时钟（`verify.sh` 4b）。
//!
//! # 谁来填
//!
//! 采集在投影层（`apps/desktop/src/ui/app_state.rs`）：面板停靠位来自 `DockLayout`，
//! 可见性来自 `ViewState`，透明度来自 `WindowOpacity`——三样都只有 `ui` 里拿得到。
//! 桌面宿主的三段都在了：端口（`AppControl` → 队列）→ 采集（本文件的形状）→
//! 动词（`surface` 的 `app_state` / `app_panel_set`，`engine.rs` 注册）。
//! 没有界面端口的宿主落到 `NoApp`，两个动词显式报 `Unsupported`——不返回
//! 空快照冒充成功。

use serde::Serialize;

/// 一块停靠面板在某一刻的样子。
#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
pub struct PanelState {
    /// `PanelId::name()` 的小写名（`filer` / `pane_session` …）。
    pub id: &'static str,
    /// 是否可见（`ViewState.show_*`）。
    pub visible: bool,
    /// 停靠带：`left` / `right` / `bottom`。
    pub zone: &'static str,
    /// 带内位次（0 起）。
    pub order: u16,
    /// 是否浮在上面。
    pub floating: bool,
    /// 浮动时的矩形 `[x, y, w, h]`（逻辑点）；停靠时为 `None`。
    pub rect: Option<[i32; 4]>,
}

/// 窗口旗标。**注意**：这是「请求值」，不是窗口系统已应用的值。
///
/// `sync_window` 每帧比较已应用值后下发 `ViewportCommand`；OS 拒绝时两者会分叉。
/// 快照给脚本的是「我们要求的是什么」，与 `ViewState` 同源。
#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
pub struct WindowState {
    pub full_screen: bool,
    pub always_on_top: bool,
    pub focus_mode: bool,
    /// 整窗不透明度（已按 `clamp_window_opacity` 夹取过的渲染值）。
    pub opacity: f32,
}

/// 一份应用状态快照。
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct AppSnapshot {
    /// 界面状态版本：每次面板置位 / 布局动作成功后 +1。
    pub rev: u64,
    /// 偏好版本：每次偏好写入成功后 +1，与 `rev` 相互独立。
    pub prefs_rev: u64,
    /// 主区窗格数（`Layout::leaf_count()`）。
    pub leaves: usize,
    /// 恒十条，顺序即 `PanelId::ALL`。
    pub panels: Vec<PanelState>,
    pub window: WindowState,
    /// 偏好白名单里当前落的**文件值**（不含设置页未保存的编辑）。
    /// 目前只有语言一项：它是唯一一条「改了能立刻生效」的偏好。
    pub prefs_language: String,
}

impl AppSnapshot {
    /// 按名字找一块面板。找不到 = `None`（调用方据此报 `UnknownName`）。
    pub fn panel(&self, id: &str) -> Option<&PanelState> {
        self.panels.iter().find(|p| p.id == id)
    }

    /// 面板下标（`app_panel_set` 回执里 `changed` 的次序与它一致）。
    pub fn panel_index(&self, id: &str) -> Option<usize> {
        self.panels.iter().position(|p| p.id == id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> AppSnapshot {
        AppSnapshot {
            rev: 7,
            prefs_rev: 2,
            leaves: 1,
            panels: vec![
                PanelState {
                    id: "sidebar",
                    visible: true,
                    zone: "left",
                    order: 0,
                    floating: false,
                    rect: None,
                },
                PanelState {
                    id: "filer",
                    visible: false,
                    zone: "left",
                    order: 1,
                    floating: false,
                    rect: None,
                },
            ],
            window: WindowState {
                full_screen: false,
                always_on_top: false,
                focus_mode: false,
                opacity: 1.0,
            },
            prefs_language: "zh-CN".to_string(),
        }
    }

    #[test]
    fn panel_lookup_is_exact_and_reports_missing_names() {
        let snap = sample();
        assert_eq!(snap.panel("filer").map(|p| p.visible), Some(false));
        assert_eq!(snap.panel_index("filer"), Some(1));
        // 名字必须是 `PanelId::name()` 的原样：大小写不同就是另一个名字，
        // 不做模糊匹配（脚本拼错时宁可报 UnknownName，也不要猜它想改谁）。
        assert!(snap.panel("Filer").is_none());
        assert!(snap.panel_index("nope").is_none());
    }

    #[test]
    fn the_two_version_counters_are_independent_fields() {
        // 结构上就分家：改一个不会动另一个（这条防的是将来有人把两者合成一个字段）。
        let mut snap = sample();
        snap.rev += 1;
        assert_eq!(snap.prefs_rev, 2);
        snap.prefs_rev += 1;
        assert_eq!(snap.rev, 8);
    }
}
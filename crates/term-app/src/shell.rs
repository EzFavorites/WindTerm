//! Shell 能力：窗口 chrome、模式开关、对话框与连接动作。
//!
//! 起 `Action` 不再是一个平铺的全局枚举。每个能力自己持有
//! `Action` 子枚举、自己的字符串入口和自己的 reducer；`app::action`
//! 只负责把各能力的入口串起来。新增能力 = 新增一个模块 + 在
//! `Action` 上挂一个变体，不需要改别的能力的 reducer。
//!
//! 本模块是纯函数：不碰 PTY / 窗口 / 文件系统，副作用以 `Effect` 值返回。

use crate::effect::Effect;
use crate::dock::{DockZone, PanelId};
use crate::state::ViewState;

pub use crate::vocab::ShellAction;

pub use crate::vocab::HexKnob;

pub use crate::vocab::MarksKnob;


/// 命令 ID → 本能力的动作。未接线的 ID 返回 `None`。
///
/// 2026-11-12：实现只有一份，在 [`crate::parse`]（入口层）。这里保留同名入口，
/// 一是因为「每个能力自带字符串入口」是这个 crate 对外的约定，二是因为
/// `apps/desktop` 有直接调 `term_app::log::parse` 的地方——把路径改掉没有架构价值。
pub fn parse(id: &str) -> Option<ShellAction> {
    crate::parse::shell_parse(id)
}

pub fn reduce(state: &mut ViewState, action: ShellAction) -> Vec<Effect> {
    match action {
        ShellAction::NewSession => {
            state.show_new_session = true;
            vec![Effect::OpenNewSession]
        }
        ShellAction::OpenSettings => {
            state.show_settings = true;
            vec![Effect::OpenSettings]
        }
        ShellAction::ShowAbout => {
            state.show_about = true;
            vec![Effect::ShowAbout]
        }
        ShellAction::DisconnectSession | ShellAction::CloseActiveView => vec![Effect::Disconnect],
        ShellAction::ReconnectSession => vec![Effect::Reconnect],
        ShellAction::ToggleMenuBar => {
            state.show_menu_bar = !state.show_menu_bar;
            vec![]
        }
        ShellAction::SetLanguage(lang) => {
            state.lang = lang;
            vec![]
        }
        ShellAction::ToggleStatusBar => {
            state.show_status_bar = !state.show_status_bar;
            vec![]
        }
        ShellAction::ToggleQuickBar => {
            state.show_quick_bar = !state.show_quick_bar;
            vec![]
        }
        ShellAction::ToggleToolbar => {
            state.show_toolbar = !state.show_toolbar;
            vec![]
        }
        // ── 面板开关：8 个专用动作 + 通用 `TogglePanel` 都走 `toggle()` ──
        //
        // 原来这 8 个分支各自写 `state.show_xxx = !state.show_xxx`，
        // 而 `TogglePanel(panel)` 走 `panel_visible` / `set_panel_visible` 那张表——
        // 同一位开关有两条翻法。表里挪一个字段、分支里没跟上，就会变成
        // 「菜单勾上了，面板没出来」，而且两处都编译得过。现在只剩一条路。
        ShellAction::ToggleSidebar => toggle(state, PanelId::Sidebar),
        ShellAction::TogglePaneFiler => toggle(state, PanelId::Filer),
        ShellAction::TogglePaneExplorer => toggle(state, PanelId::Explorer),
        ShellAction::TogglePaneOutline => toggle(state, PanelId::Outline),
        ShellAction::TogglePaneSender => toggle(state, PanelId::Sender),
        ShellAction::TogglePaneSession => toggle(state, PanelId::Session),
        ShellAction::TogglePaneShell => toggle(state, PanelId::Shell),
        ShellAction::TogglePaneScript => toggle(state, PanelId::Script),
        ShellAction::TogglePaneTransfer => toggle(state, PanelId::Transfer),
        // tmux 面板不在 `PanelId::ALL` 里（它没有停靠位，是每帧由
        // `ui/mod.rs` 调 `draw_tmux_panel` 画的浮层），所以进不了那张表。
        // 这是 `PanelId` 与真实面板集合唯一的已知偏差，别当成遗漏。
        ShellAction::TogglePaneTmux => {
            state.show_tmux = !state.show_tmux;
            vec![]
        }
        ShellAction::TogglePanel(panel) => toggle(state, panel),
        // 置位版：给脚本的 `app_panel_set` 用（端口 / 队列 / 采集 / 动词
        // 四段都已接）。幂等——设成同值不改变任何字段，调用方可以放心重发；
        // `toggle` 是它的取反包装。
        ShellAction::SetPanel(panel, on) => {
            set_panel_visible(state, panel, on);
            vec![]
        }
        ShellAction::ToggleDocks(zone) => {
            // 「带里还有一块亮着」= 收起；否则 = 恢复。以 state 现值为准，
            // 不缓存「关之前长什么样」——多存一份就会和实际开关不同步。
            let any = zone_panels(zone)
                .into_iter()
                .any(|p| panel_visible(state, p));
            for p in zone_panels(zone) {
                set_panel_visible(state, p, !any);
            }
            vec![]
        }
        ShellAction::ToggleFocusMode => {
            state.focus_mode = !state.focus_mode;
            vec![]
        }
        ShellAction::ToggleFullScreen => {
            state.full_screen = !state.full_screen;
            vec![]
        }
        ShellAction::ToggleAlwaysOnTop => {
            state.always_on_top = !state.always_on_top;
            vec![]
        }
        // ── 下列动作把副作用全部交给投影层，reducer 只带意图 ──
        // 它们不写 `ViewState`：标签列表、会话配置、选区、字号都活在 `apps/*`，
        // 纯函数的 reducer 拿不到也不该拿到。边界里「副作用以值返回」的本意。
        ShellAction::ActivateNextView => vec![Effect::ActivateTab(1)],
        ShellAction::ActivatePreviousView => vec![Effect::ActivateTab(-1)],
        ShellAction::CopySessionName => vec![Effect::CopySessionName],
        ShellAction::CopySessionUrl => vec![Effect::CopySessionUrl],
        ShellAction::Help => vec![Effect::OpenHelp],
        ShellAction::SearchOnline => vec![Effect::SearchOnline],
        ShellAction::ZoomIn => vec![Effect::Zoom(1)],
        ShellAction::ZoomOut => vec![Effect::Zoom(-1)],
        ShellAction::ZoomReset => vec![Effect::Zoom(0)],
        // ── E 类三条 + 新进程窗口，全部只声明副作用 ──
        ShellAction::NewWindow => vec![Effect::NewWindow],
        ShellAction::Exit => vec![Effect::Exit],
        ShellAction::Close => vec![Effect::CloseActiveTab],
        ShellAction::CloseAllViews => vec![Effect::CloseAllTabs],
        // ── 分屏六条，reducer 只声明方向与范围 ──
        ShellAction::SplitViewHorizontally => vec![Effect::SplitView(true)],
        ShellAction::SplitViewVertically => vec![Effect::SplitView(false)],
        ShellAction::SplitGroupHorizontally => vec![Effect::SplitGroup(true)],
        ShellAction::SplitGroupVertically => vec![Effect::SplitGroup(false)],
        ShellAction::CloseActiveGroup => vec![Effect::CloseActiveGroup],
        ShellAction::MoveGroupToGroup => vec![Effect::MoveGroupToGroup],
        // ── 同步输入 / 选中命令 / 隧道管理 ──
        ShellAction::SyncInput => {
            state.sync_input = !state.sync_input;
            vec![]
        }
        ShellAction::SelectCommand => vec![Effect::SelectCommand],
        // 显示模式归投影层的 `Tab`：模式是**每条会话**的，而 `ViewState` 是全局的。
        // 所以这里不改 `ViewState`——改了就是把「哪个标签在 hex」压成一份全局 bool。
        ShellAction::ToggleHexEdit => vec![Effect::ToggleHex],
        // 调试覆盖层：与 `ToggleHexEdit` 同因同形——显示方式是**每条会话**的，
        // reducer 不改 `ViewState`，只把「切一下」这个意图抛给投影层。
        ShellAction::ToggleDebugMode => vec![Effect::ToggleDebug],
        // 只读查看：同上——「哪条会话冻着」是每条会话的属性，reducer 只说意图。
        ShellAction::ToggleLocalMode => vec![Effect::ToggleLocalMode],
        ShellAction::SetRemoteMode => vec![Effect::SetRemoteMode],
        ShellAction::ToggleWordWrap => vec![Effect::ToggleWordWrap],
        // 两条都是「投影层才知道落地细节」的同族。
        // 开关值活在投影层（`TerminalWindow::tunnels_enabled`）并落盘，
        // 设置草稿更是纯投影态——reducer 一个字段都不碰，只转述意图。
        ShellAction::ToggleTunnels => vec![Effect::ToggleTunnels],
        ShellAction::TunnelSettings => vec![Effect::OpenTunnelSettings],
        // 三个旋钮共用一条 Effect：落地动作完全一样（改一个字段、夹一次、
        // 写三个字段）。分成三条只会得到三份一模一样的代码。
        ShellAction::HexBase(base) => vec![Effect::SetHex(HexKnob::Base(base))],
        ShellAction::HexColumns(columns) => vec![Effect::SetHex(HexKnob::Columns(columns))],
        ShellAction::HexGroupChars(group) => vec![Effect::SetHex(HexKnob::Group(group))],
        // 栅格可视化十条共用一条 Effect，理由与上面三行一字不差：reducer 手上
        // 没有「现在开没开」（那是 `Tab.marks`，每条会话一份）。翻转 / 设档都在
        // 投影层，`ui/effects.rs::set_marks_knob` 一处落地。
        ShellAction::Marks(knob) => vec![Effect::SetMarks(knob)],
        ShellAction::ManageTunnels => {
            // 开关而不是「只开」：面板自己的 × 与这条命令走同一条路径。
            state.show_tunnels = !state.show_tunnels;
            vec![]
        }
        // `Terminal.ManageTriggers`：打开**独立的** Triggers 对话框。
        //
        // 2026-11-15 改：原先这里置 `show_settings` 再发一个「翻到 Triggers 页」
        // 的效应，结果弹的是 Preferences、Triggers 只是它左树里翻不到的一页
        // （因为那两项已按用户裁定移出左树）。兼容终端是自有窗口（左列表 +
        // `Cancel / Ok`），所以落点改成独立对话框字段，不需要效应。
        ShellAction::ManageTriggers => {
            state.show_triggers = true;
            vec![]
        }
        // `Terminal.ManageOneKeys`：同理，独立的 OneKeys 对话框。
        ShellAction::ManageOneKeys => {
            state.show_onekeys = true;
            vec![]
        }
        // `Terminal.ClearRecentUrls`：一个字段都不写（列表在盘上，缓存副本在
        // 投影层），只把意图带出去。落地是 `ui/address/mod.rs` 里那条早就在的
        // `clear_recent_targets`——地址栏右键的「Clear Recent」走同一个函数。
        ShellAction::ClearRecentUrls => vec![Effect::ClearRecentUrls],
        // `View ▸ Window Opacity` 三档预设：一条带参数的 `Effect` 就够了，
        // 落地动作完全一样（改运行期副本 + load-modify-save 写回 Prefs）。
        // 分成三条只会得到三份一模一样的代码，见 `effect.rs` 的说明。
        ShellAction::SetWindowOpacity(preset) => vec![Effect::SetWindowOpacity {
            transparent: preset.transparent(),
            opacity_permille: preset.permille(),
        }],
        // `Application.SetWindowOpacity`：打开设置并预选 `Appearance` 页。
        // 与 `ManageTriggers` / `ManageOneKeys` 同形——前半截 `show_settings`
        // 在这里，后半截（翻到哪一页）是投影层的类型，只能以效应带出去。
        ShellAction::ManageWindowOpacity => {
            state.show_settings = true;
            vec![Effect::FocusAppearancePage]
        }
    }
}

/// 一个停靠带里有哪些面板。
///
/// 归属只有一份，来自 `PanelId::default_zone()`：侧栏在左，`Sender` / `Shell`
/// 在底（底带是一条带 + 标签条），其余全在右。这张表说的是**默认归属**，
/// 用户拖过之后真实位置在投影层的 `DockLayout` 里——整带开关按默认归属收，
/// 拖到底带的面板因此也能被「Toggle Bottom Docks」一起收走。
fn zone_panels(zone: DockZone) -> Vec<PanelId> {
    PanelId::ALL
        .into_iter()
        .filter(|p| p.default_zone() == zone)
        .collect()
}

/// 面板可见性 ⇒ `ViewState` 字段的**唯一**映射。
///
/// 这张表既是动作侧的写点，也是绘制侧的读点。之前 `ui/panels/mod.rs`
/// 里还有一份一模一样的 9 分支 match；两份都是对 `PanelId` 的穷尽 match，所以
/// 「一边改了另一边没改」会被编译器挡住——但它挡不住「同一个开关有两条翻法」，
/// 那才是之前真实的坏法（8 个专用动作各写各的赋值）。现在：
///
/// · 写：只经 [`set_panel_visible`]；
/// · 翻：只经 [`toggle`]；
/// · 读：`ui` 直接调**本函数**（`term_app::shell::panel_visible`），不再自己列一份。
///
/// 为什么读点留在 `shell` 而不是 `dock`：`dock::PanelId` 是词表，`menu/bars.rs`
/// 用它；把它改成依赖 `ViewState` 会引出 `palette → menu → dock → state → palette`
/// 这个四节点环——正是拆掉的那类病。`shell` 本来就同时依赖 `dock` 与
/// `state`，把表放这里不新增任何一条边。
///
/// 门禁 4h 为此加了一条**受检的间接读**：这 9 个字段允许在 `ui/` 里没有直接读点，
/// 但必须同时满足「本函数的生产段确实读了它」和「`ui/` 里确实有一行调本函数」，
/// 缺一半就判红。删掉 `ui` 那一行调用，4h 立刻红——白名单不会无声失效。
pub fn panel_visible(state: &ViewState, panel: PanelId) -> bool {
    match panel {
        PanelId::Sidebar => state.show_sidebar,
        PanelId::Filer => state.show_filer,
        PanelId::Explorer => state.show_pane_explorer,
        PanelId::Outline => state.show_pane_outline,
        PanelId::Sender => state.show_pane_sender,
        PanelId::Session => state.show_pane_session,
        PanelId::Shell => state.show_pane_shell,
        PanelId::Transfer => state.show_pane_transfer,
        PanelId::Sysmon => state.show_pane_sysmon,
        PanelId::Script => state.show_pane_script,
    }
}

/// 面板开关的**唯一**写点。见 [`panel_visible`] 的说明。
pub fn set_panel_visible(state: &mut ViewState, panel: PanelId, on: bool) {
    let slot = match panel {
        PanelId::Sidebar => &mut state.show_sidebar,
        PanelId::Filer => &mut state.show_filer,
        PanelId::Explorer => &mut state.show_pane_explorer,
        PanelId::Outline => &mut state.show_pane_outline,
        PanelId::Sender => &mut state.show_pane_sender,
        PanelId::Session => &mut state.show_pane_session,
        PanelId::Shell => &mut state.show_pane_shell,
        PanelId::Transfer => &mut state.show_pane_transfer,
        PanelId::Sysmon => &mut state.show_pane_sysmon,
        PanelId::Script => &mut state.show_pane_script,
    };
    *slot = on;
}

/// 翻转一个面板的开关：读表取反、写表落地。**面板开关的唯一入口**。
///
/// 专用动作（`TogglePaneExplorer` …）和通用 `TogglePanel(panel)` 都收在这里，
/// 免得同一个 `ViewState` 字段被两条互不知情的路径翻。
fn toggle(state: &mut ViewState, panel: PanelId) -> Vec<Effect> {
    let on = !panel_visible(state, panel);
    set_panel_visible(state, panel, on);
    Vec::new()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// tmux 面板不能只活在截图夹具里。
    /// 「ID 能 parse → reduce 真翻开关 → 再点一次能关回去」三件事一起守，
    /// 少一件就又是「菜单亮着、点了没反应」或「关掉没回程」。
    #[test]
    fn the_tmux_panel_has_a_wired_toggle_with_a_return_path() {
        assert_eq!(
            parse("Window.TogglePaneTmux"),
            Some(ShellAction::TogglePaneTmux)
        );
        let mut state = ViewState::default();
        let before = state.clone();
        assert!(!state.show_tmux, "出厂态不该占屏");
        assert!(reduce(&mut state, ShellAction::TogglePaneTmux).is_empty());
        assert!(state.show_tmux, "菜单点了却没把面板打开");
        assert_eq!(state.show_tunnels, before.show_tunnels);
        assert_eq!(state.show_shortcuts, before.show_shortcuts);
        reduce(&mut state, ShellAction::TogglePaneTmux);
        assert_eq!(state, before, "关掉以后点不回来了");
    }

    /// 底带标签的 ×、齿轮菜单勾选、Sysmon 细带按钮都发 `TogglePanel(id)`。
    /// 每块面板都要能翻自己的字段，且连翻两次回到原状——「关掉没有回程」
    /// 是明令禁止的。新增面板时这条会直接把漏接映射的那块指出来。
    #[test]
    fn toggle_panel_flips_its_own_slot_for_every_id_and_is_involutive() {
        for id in PanelId::ALL {
            let mut state = ViewState::default();
            let before = state.clone();
            let want = !panel_visible(&state, id);
            assert!(
                reduce(&mut state, ShellAction::TogglePanel(id)).is_empty(),
                "{}",
                id.name()
            );
            assert_eq!(panel_visible(&state, id), want, "{} 没翻自己的字段", id.name());
            for other in PanelId::ALL.into_iter().filter(|p| *p != id) {
                assert_eq!(
                    panel_visible(&state, other),
                    panel_visible(&before, other),
                    "{} 不该被 {} 波及",
                    other.name(),
                    id.name()
                );
            }
            reduce(&mut state, ShellAction::TogglePanel(id));
            assert_eq!(state, before, "{} 连翻两次没回到原状", id.name());
        }
    }

    /// 置位版（`SetPanel`）：幂等，且「置成取反值」与 `TogglePanel` 结果相同。
    ///
    /// 这三条就是脚本 `app_panel_set` 敢重试的全部依据：
    /// 幂等（同值不改变）→ 重发安全；与 toggle 等价 → 两条路不会各说一套。
    #[test]
    fn set_panel_is_idempotent_and_agrees_with_toggle() {
        for id in PanelId::ALL {
            // 置成当前值：一个字段都不许动。
            let mut state = ViewState::default();
            let on = panel_visible(&state, id);
            let before = state.clone();
            assert!(reduce(&mut state, ShellAction::SetPanel(id, on)).is_empty());
            assert_eq!(state, before, "{} 置成同值不该有任何变化", id.name());

            // 置成取反值 == toggle 一次。
            let mut via_set = ViewState::default();
            reduce(&mut via_set, ShellAction::SetPanel(id, !on));
            let mut via_toggle = ViewState::default();
            reduce(&mut via_toggle, ShellAction::TogglePanel(id));
            assert_eq!(via_set, via_toggle, "{} 置位与取反的结果必须相同", id.name());

            // 连发两次同一个置位：第二次是空操作（幂等）。
            reduce(&mut via_set, ShellAction::SetPanel(id, !on));
            assert_eq!(via_set, via_toggle, "{} 第二次置位不该翻回去", id.name());
        }
    }

    /// 专用动作（`TogglePaneExplorer` 那 8 个）与通用 `TogglePanel(id)` 必须翻同一件事。
    ///
    /// R3-c 之前这两条路各写各的赋值语句：专用动作 `state.show_xxx = !state.show_xxx`，
    /// 通用动作走 `panel_visible` / `set_panel_visible` 那张表。上面那条测试只覆盖了
    /// 通用动作，所以「专用动作翻错了字段」「表里挪了字段、专用动作没跟上」这两类
    /// 都编译得过、测试也全绿，只有人手点菜单才发现。这里逐面板比对两条路的**结果状态**，
    /// 并顺带守住回程（连发两次必须回到出厂态）。
    #[test]
    fn the_dedicated_toggles_are_the_same_action_as_toggle_panel() {
        fn dedicated(id: PanelId) -> Option<ShellAction> {
            Some(match id {
                PanelId::Sidebar => ShellAction::ToggleSidebar,
                PanelId::Filer => ShellAction::TogglePaneFiler,
                PanelId::Explorer => ShellAction::TogglePaneExplorer,
                PanelId::Outline => ShellAction::TogglePaneOutline,
                PanelId::Sender => ShellAction::TogglePaneSender,
                PanelId::Session => ShellAction::TogglePaneSession,
                PanelId::Shell => ShellAction::TogglePaneShell,
                PanelId::Script => ShellAction::TogglePaneScript,
                PanelId::Transfer => ShellAction::TogglePaneTransfer,
                // Sysmon 没有专用菜单项，只走 `TogglePanel`。
                PanelId::Sysmon => return None,
            })
        }

        let mut covered = 0usize;
        for id in PanelId::ALL {
            let Some(action) = dedicated(id) else {
                continue;
            };
            covered += 1;

            let mut via_dedicated = ViewState::default();
            let mut via_generic = ViewState::default();
            assert!(reduce(&mut via_dedicated, action).is_empty(), "{}", id.name());
            assert!(
                reduce(&mut via_generic, ShellAction::TogglePanel(id)).is_empty(),
                "{}",
                id.name()
            );
            assert_eq!(
                via_dedicated,
                via_generic,
                "{} 的专用动作和 TogglePanel({}) 翻的不是同一件事",
                id.name(),
                id.name()
            );

            reduce(
                &mut via_dedicated,
                dedicated(id).expect("上面刚匹配到 Some，这里必然还是 Some"),
            );
            assert_eq!(
                via_dedicated,
                ViewState::default(),
                "{} 的专用动作没有回程（连发两次没回到出厂态）",
                id.name()
            );
        }

        assert_eq!(
            covered,
            PanelId::ALL.len(),
            "PanelId 加了新变体：要么给它接上专用动作，要么在上面的 dedicated() 里 \
             显式 return None 登记「它只有 TogglePanel 一条路」——别让它悄悄漏掉。\
             Sysmon 不在 ALL 里（它不是停靠面板），所以现在全覆盖。"
        );
    }
}

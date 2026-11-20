//! `Window ▸ Local Mode`（只读查看）的命令链路：**纯数据层**的那一半。
//!
//! 这个模块只钉三件事，一条都不碰像素：
//!
//! 1. 命令 ID 被 `Action::parse` 认出来，且认成 `ToggleLocalMode` 而不是被
//!    `Action::Shell(_)` 的兜底吞掉（后者会让能力面数不清）；
//! 2. reducer 吐出 `Effect::ToggleLocalMode`——**一个 `ViewState` 字段都不写**，
//!    因为「哪条会话冻着」是每条会话的属性（`Tab.local_mode`）；
//! 3. 连点两次得到两条同款 Effect（投影层翻两次回到原状，没有回程的开关
//!    是本仓明令禁止的形态）。
//!
//! 另一半（`Tab::write` 被拦、视口不被输出推走、触发器不求值、脚本不起）在
//! `apps/desktop` 的 `local_mode_tests`，两边合起来才叫「只读真的生效」。

use crate::action::Action;
use crate::capability;
use crate::effect::Effect;
use crate::vocab::FeatureId;
use crate::shell::{parse as shell_parse, reduce, ShellAction};
use crate::state::ViewState;

/// 命令 ID 只在这张表里出现一次：改 ID 必须同时改菜单树与解析，
/// 否则「菜单里能点、点了没反应」会以最自然的方式溜回来。
const ID: &str = "Window.SetLocalMode";

#[test]
fn the_local_mode_command_parses_to_its_own_action() {
    let action = Action::parse(ID)
        .unwrap_or_else(|| panic!("{ID} 解析不出动作——菜单里它就是灰的"));
    assert_eq!(action, Action::Shell(ShellAction::ToggleLocalMode));
    // 归属走 `Action::Shell(_)` 的兜底：命令 ID 本身就是 `Window.*` 前缀，
    // 与 `Window.ToggleDebugMode` 一族，不新增 `FeatureId`。
    assert_eq!(action.feature(), FeatureId::Shell);
    assert!(capability::is_implemented(ID), "接线了就必须进已实现集合");
}

#[test]
fn the_short_parse_path_agrees_with_the_entry_table() {
    // 投影层走的是 `shell::parse`，菜单判态走的是 `Action::parse`：
    // 两条路必须同源，否则同一行在菜单里能点、在快捷键里没反应。
    assert_eq!(shell_parse(ID), Some(ShellAction::ToggleLocalMode));
}

#[test]
fn reducing_it_leaves_the_window_state_untouched() {
    // 只读是**每条会话**的属性：reducer 手上只有全局 `ViewState`，
    // 所以它必须什么都不写，只把意图抛给投影层。
    let mut state = ViewState::default();
    let before = state.clone();
    let effects = reduce(&mut state, ShellAction::ToggleLocalMode);
    assert_eq!(effects, vec![Effect::ToggleLocalMode]);
    assert_eq!(state, before, "reducer 不该碰任何 ViewState 字段");

    // 连点两次 → 两条 Effect；投影层翻两次回到原状（幂等成对）。
    let again = reduce(&mut state, ShellAction::ToggleLocalMode);
    assert_eq!(again, vec![Effect::ToggleLocalMode]);
    assert_eq!(state, before);
}

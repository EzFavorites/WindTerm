//! 同步输入的 4 条通道：加入 / 离开 / 目标的纯数据操作。
//!
//! 旧的 `sync_input: bool` 表示「开着就写给所有标签」，用户无法只同步其中几台。
//! 通道表把「谁和谁同步」变成显式成员关系；`ViewState.sync_input` 保留为总开关，
//! 由这里的 helper 同时维护，reducer 不需要认识会话身份。

use term_core::session_runtime::SessionId;

use crate::state::ViewState;

pub const CHANNEL_COUNT: usize = 4;

fn channel_mut(state: &mut ViewState, index: usize) -> &mut Vec<SessionId> {
    &mut state.sync_channels[index.min(CHANNEL_COUNT - 1)]
}

/// 加入一条通道（幂等）。同时打开总开关。
pub fn join(state: &mut ViewState, index: usize, id: SessionId) {
    let members = channel_mut(state, index);
    if !members.contains(&id) {
        members.push(id);
    }
    state.sync_input = true;
}

/// 离开一条通道；通道空了就把总开关关掉。
pub fn leave(state: &mut ViewState, index: usize, id: SessionId) {
    let members = channel_mut(state, index);
    members.retain(|m| *m != id);
    state.sync_input = state.sync_channels.iter().any(|c| !c.is_empty());
}

/// 离开所有通道（会话关闭时必须调用，否则身份残留）。
pub fn leave_all(state: &mut ViewState, id: SessionId) {
    for members in &mut state.sync_channels {
        members.retain(|m| *m != id);
    }
    state.sync_input = state.sync_channels.iter().any(|c| !c.is_empty());
}

pub fn is_member(state: &ViewState, index: usize, id: SessionId) -> bool {
    state.sync_channels[index.min(CHANNEL_COUNT - 1)].contains(&id)
}

/// 某条通道的接收者（去掉发起者自己）。
pub fn targets(state: &ViewState, index: usize, exclude: SessionId) -> Vec<SessionId> {
    state.sync_channels[index.min(CHANNEL_COUNT - 1)]
        .iter()
        .copied()
        .filter(|id| *id != exclude)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_channel_only_broadcasts_to_its_own_members() {
        let mut state = ViewState::default();
        join(&mut state, 0, 1);
        join(&mut state, 0, 2);
        join(&mut state, 1, 3);
        assert!(state.sync_input);
        assert_eq!(targets(&state, 0, 1), vec![2]);
        assert_eq!(targets(&state, 1, 1), vec![3]);
        assert_eq!(targets(&state, 2, 1), Vec::<SessionId>::new());
        assert!(is_member(&state, 0, 2));
        assert!(!is_member(&state, 1, 2));
    }

    #[test]
    fn leaving_the_last_member_closes_the_master_switch() {
        let mut state = ViewState::default();
        join(&mut state, 0, 1);
        join(&mut state, 0, 2);
        leave(&mut state, 0, 1);
        assert!(state.sync_input, "还有 2 号成员，开关要留着");
        leave(&mut state, 0, 2);
        assert!(!state.sync_input, "最后一位离开后自动关");
    }

    #[test]
    fn a_closed_session_does_not_leave_a_ghost_member() {
        let mut state = ViewState::default();
        join(&mut state, 0, 7);
        join(&mut state, 3, 7);
        leave_all(&mut state, 7);
        assert!(!state.sync_input);
        assert_eq!(targets(&state, 0, 0), Vec::<SessionId>::new());
        assert_eq!(targets(&state, 3, 0), Vec::<SessionId>::new());
    }
}

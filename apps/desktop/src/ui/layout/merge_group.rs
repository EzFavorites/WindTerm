//! Move Group To Next 的合并语义（纯函数）。从 `split.rs` 拆出：那份文件的
//! 代码行已贴 500 门禁，树剪接留在那里，目标选择与分组归并放这里。

use super::{PaneGroup, PaneId};
use term_core::session_runtime::SessionId;

/// 遍历序里的下一片窗格身份及其下标；单叶子 / 目标不在树里 / 回环时 `None`。
/// 返回的 `PaneId` 是会被覆盖的旧窗格，调用方据此摘掉它的 `pane_groups` 键。
pub(in crate::ui) fn merge_target(panes: &[PaneId], active_pane: PaneId) -> Option<(usize, PaneId)> {
    if panes.len() < 2 {
        return None;
    }
    let pos = panes.iter().position(|p| *p == active_pane)?;
    let next = (pos + 1) % panes.len();
    (next != pos).then(|| (next, panes[next]))
}

/// 把被覆盖窗格的会话并入幸存窗格：两组的 `order` 合成一份，`SessionId` 去重
/// （一个会话至多属于一个 group），活动会话优先取仍在集合里的 `active`。
/// 树里少了一片叶子，`pane_groups` 也必须少一个键，不能留孤儿。
pub(in crate::ui) fn merge_groups(
    survivor: &PaneGroup,
    overwritten: &PaneGroup,
    active: SessionId,
) -> PaneGroup {
    let mut order = survivor.order.clone();
    for s in &overwritten.order {
        if !order.contains(s) {
            order.push(*s);
        }
    }
    let active = if order.contains(&active) { active } else { survivor.active };
    PaneGroup { order, active }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn merge_target_is_the_next_visible_pane_and_reports_the_overwritten_identity() {
        let panes = [PaneId(10), PaneId(11), PaneId(12)];
        assert_eq!(merge_target(&panes, PaneId(10)), Some((1, PaneId(11))));
        assert_eq!(merge_target(&panes, PaneId(11)), Some((2, PaneId(12))));
        // 遍历序末尾回环到第一片，仍是「下一片」而不是自己。
        assert_eq!(merge_target(&panes, PaneId(12)), Some((0, PaneId(10))));
        assert_eq!(merge_target(&[PaneId(10)], PaneId(10)), None);
        assert_eq!(merge_target(&panes, PaneId(99)), None);
    }

    #[test]
    fn merging_groups_keeps_every_session_in_one_group() {
        let survivor = PaneGroup {
            order: vec![10, 11],
            active: 11,
        };
        let overwritten = PaneGroup {
            order: vec![12, 11],
            active: 12,
        };
        let merged = merge_groups(&survivor, &overwritten, 11);
        assert_eq!(merged.order, vec![10, 11, 12], "重复的会话只留一份");
        assert_eq!(merged.active, 11);
        // 活动身份不在并集里时退回幸存组原来的活动会话。
        assert_eq!(merge_groups(&survivor, &overwritten, 99).active, 11);
    }
}

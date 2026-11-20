//! 拖拽移动 / 交换 pane 的纯函数——只动叶子身份，不动树形、不新增叶子。

use super::Layout;
use crate::ui::layout::split::PaneId;
// 只在单测里构造 `pane_groups` 用；非测试构建不需要，加 cfg 免「未使用」警告。
#[cfg(test)]
use std::collections::HashMap;

impl Layout {
    /// 把一个窗格拖到另一片窗格上 = 两片窗格交换显示的会话。
    ///
    /// ④-0：每片窗格都是一个 `PaneId`，且总在布局树里（一个会话只要还开着，就
    /// 属于某个 group、挂在某个 `PaneId` 叶子上）。所以拖拽永远是「两片都在树里」
    /// 的原地交换：树形不变、身份集合不变，`swap_two_in_tree` 完成。「拖回自己那片」
    /// 由 `drop_drag_on_pane` 在调用前挡下（`from == target` 返回 false），这里不再
    /// 处理。恒不变：交换后身份集合不变、不重复（每个 `from` 叶子变 `to`、`to` 叶子
    /// 变 `from`，无中间态、不会制造重复身份）。
    pub fn swap_leaves(&mut self, from: PaneId, to: PaneId) {
        if from == to {
            return;
        }
        swap_two_in_tree(self, from, to);
    }
}

/// 两个身份都在树里时，原地交换两片叶子的身份。两身份各出现一次（不变量：
/// 叶子身份唯一），所以每个 `from` 叶子变 `to`、`to` 叶子变 `from`，无中间态、
/// 不会制造重复身份。
fn swap_two_in_tree(node: &mut Layout, from: PaneId, to: PaneId) {
    match node {
        Layout::Leaf(id) => {
            if *id == from {
                *id = to;
            } else if *id == to {
                *id = from;
            }
        }
        Layout::Split { first, second, .. } => {
            swap_two_in_tree(first, from, to);
            swap_two_in_tree(second, from, to);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 两个身份都在树里：交换后身份集合不变、树形不变，只换两片叶子的内容。
    #[test]
    fn swapping_two_leaves_exchanges_them_without_changing_shape() {
        let mut l = Layout::Split {
            horizontal: true,
            ratio: 500,
            first: Box::new(Layout::Leaf(PaneId(10))),
            second: Box::new(Layout::Leaf(PaneId(11))),
        };
        l.swap_leaves(PaneId(10), PaneId(11));
        assert_eq!(l.leaf_panes(), vec![PaneId(11), PaneId(10)], "两片窗格内容互换");
        assert_eq!(l.leaf_count(), 2);
        let set: std::collections::HashSet<_> = l.leaf_panes().into_iter().collect();
        assert_eq!(set.len(), 2, "身份集合不变、不重复");
    }

    /// 嵌套三叶树里交换任意两片：身份集合守恒，树仍是合法二叉树。
    #[test]
    fn swapping_in_a_nested_tree_keeps_the_identity_set() {
        let mut l = Layout::single(PaneId(10));
        l.split(PaneId(10), true, PaneId(11));
        l.split(PaneId(11), false, PaneId(12)); // 树：Split{ 10 , Split{ 11 , 12 } }
        let before: std::collections::HashSet<_> = l.leaf_panes().into_iter().collect();
        assert_eq!(before.len(), 3);
        l.swap_leaves(PaneId(10), PaneId(12));
        let after: std::collections::HashSet<_> = l.leaf_panes().into_iter().collect();
        assert_eq!(after, before, "交换后身份集合守恒");
        assert_eq!(l.leaf_count(), 3, "叶子数不变");
        assert!(
            l.leaf_panes().contains(&PaneId(10)) && l.leaf_panes().contains(&PaneId(12)),
            "两片内容互换"
        );
    }

    /// 拖回自己那一片：空操作，树原样。
    #[test]
    fn swapping_a_leaf_with_itself_is_a_no_op() {
        let mut l = Layout::Split {
            horizontal: true,
            ratio: 500,
            first: Box::new(Layout::Leaf(PaneId(10))),
            second: Box::new(Layout::Leaf(PaneId(11))),
        };
        l.swap_leaves(PaneId(10), PaneId(10));
        assert_eq!(l.leaf_panes(), vec![PaneId(10), PaneId(11)]);
    }

    /// 交换后整棵树的「会话并集」也跟着换（经 pane_groups 解析）。
    #[test]
    fn swapping_updates_the_visible_session_union() {
        let mut l = Layout::Split {
            horizontal: false,
            ratio: 500,
            first: Box::new(Layout::Leaf(PaneId(11))),
            second: Box::new(Layout::Leaf(PaneId(12))),
        };
        let mut groups: HashMap<PaneId, crate::ui::layout::split::PaneGroup> =
            std::collections::HashMap::new();
        groups.insert(
            PaneId(11),
            crate::ui::layout::split::PaneGroup {
                order: vec![110],
                active: 110,
            },
        );
        groups.insert(
            PaneId(12),
            crate::ui::layout::split::PaneGroup {
                order: vec![120],
                active: 120,
            },
        );
        l.swap_leaves(PaneId(11), PaneId(12));
        assert_eq!(l.tabs(&groups), vec![120, 110], "两片窗格内容互换");
        let set: std::collections::HashSet<_> = l.tabs(&groups).into_iter().collect();
        assert_eq!(set.len(), 2, "仍无重复身份");
    }
}

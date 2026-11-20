//! 结构领域模型的承载层（P2③-a）。
//!
//! 这一层只做到「造不出非法值」，**不接 `ViewState`、不接 `Tab`、不接任何命令**。
//! 蓝图 §2 把结构领域模型列为「三块架构缺位」之一，`full-audit §4` 的结论是：
//! planned 清单里相当一部分不是接线工作，而是等承载类型出现；在承载类型出现之前
//! 逐条接线只会得到「能点没效果」的假开关（R5/I5 明令禁止）。
//!
//! 因此本模块刻意只提供类型与不变量，不制造无主的视图状态：
//! - `FoldRegion`：折叠区间，两端固定 `HistoryRow`，**不引入第二套行号**（§8 第 3 条）。
//! - `Carets`：主光标必须存在，附加光标不得与主光标重复。
//! - `ScopeStack`：作用域/配对栈不得为空，`pop` 不允许把栈清空。
//! - `StructureModel`：折叠区间不得互相重叠。
//!
//! 生产路径一律返回 `Result` / `Option`，不新增 panic（4p 冻结基线只许减不许增）。

use crate::selection::Pos;

// `FoldRegion` 与 `StructureError` 已于 2026-11-14 搬到 `crate::folds`：
// 那边有真实生产读者（`Folds` 被投影层读取），而本模块整体仍挂在 `planned`
// 后面（`Carets` / `ScopeStack` / `StructureModel` 至今零读者）。类型跟着
// 读者走，所以在这里再导出，保持原有 `structure::FoldRegion` 路径不变——
// 本文件内的测试与本模块的使用方都不需要改 import。
pub use crate::folds::{FoldRegion, StructureError};

/// 多光标集合：主光标 + 零个或多个附加光标。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Carets {
    primary: Pos,
    others: Vec<Pos>,
}

impl Carets {
    /// 构造。主光标由类型本身保证非空；附加光标与主光标重合时拒绝。
    pub fn new(primary: Pos, others: Vec<Pos>) -> Result<Self, StructureError> {
        if others.contains(&primary) {
            return Err(StructureError::DuplicateCaret);
        }
        Ok(Self { primary, others })
    }

    pub fn primary(&self) -> Pos {
        self.primary
    }

    pub fn others(&self) -> &[Pos] {
        &self.others
    }
}

/// 非空作用域/配对栈。元素类型由调用方决定，本层不发明业务作用域语义。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ScopeStack<T> {
    items: Vec<T>,
}

impl<T> ScopeStack<T> {
    /// 构造。空栈被拒绝。
    pub fn new(items: Vec<T>) -> Result<Self, StructureError> {
        if items.is_empty() {
            return Err(StructureError::EmptyScopeStack);
        }
        Ok(Self { items })
    }

    pub fn push(&mut self, item: T) {
        self.items.push(item);
    }

    /// 弹出栈顶，但拒绝把栈清空（空状态造不出来）。
    pub fn pop(&mut self) -> Option<T> {
        if self.items.len() <= 1 {
            return None;
        }
        self.items.pop()
    }

    pub fn len(&self) -> usize {
        self.items.len()
    }

    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    pub fn top(&self) -> Option<&T> {
        self.items.last()
    }
}

/// 结构领域模型：折叠区间 + 光标集合 + 作用域栈的组合。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StructureModel<T> {
    folds: Vec<FoldRegion>,
    carets: Carets,
    scopes: ScopeStack<T>,
}

impl<T> StructureModel<T> {
    /// 构造。任意两个折叠区间相交时拒绝；保留调用方给出的区间顺序。
    pub fn new(
        folds: Vec<FoldRegion>,
        carets: Carets,
        scopes: ScopeStack<T>,
    ) -> Result<Self, StructureError> {
        for (i, a) in folds.iter().enumerate() {
            for b in folds.iter().skip(i + 1) {
                if a.start() <= b.end() && b.start() <= a.end() {
                    return Err(StructureError::OverlappingFoldRegions);
                }
            }
        }
        Ok(Self {
            folds,
            carets,
            scopes,
        })
    }

    pub fn folds(&self) -> &[FoldRegion] {
        &self.folds
    }

    pub fn carets(&self) -> &Carets {
        &self.carets
    }

    pub fn scopes(&self) -> &ScopeStack<T> {
        &self.scopes
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    // `HistoryRow` 原先是从文件顶部的 `use` 经 `super::*` 带进来的；那两个类型
    // 搬去 `crate::folds` 之后顶部不再导入它，测试自己带上，避免「搬家顺手弄坏
    // 一个不相干的测试」。
    use term_core::coords::HistoryRow;

    fn row(n: i64) -> HistoryRow {
        HistoryRow::new(n).expect("测试用的行号必须非负")
    }

    #[test]
    fn fold_region_accepts_single_line_and_non_empty_span() {
        let one = FoldRegion::new(row(3), row(3)).expect("单行区间合法");
        assert_eq!(one.start(), row(3));
        assert_eq!(one.end(), row(3));
        assert!(one.contains(row(3)));

        let span = FoldRegion::new(row(2), row(9)).expect("正区间合法");
        assert!(span.contains(row(2)));
        assert!(span.contains(row(9)));
        assert!(!span.contains(row(10)));
    }

    #[test]
    fn fold_region_rejects_reversed_span() {
        assert_eq!(
            FoldRegion::new(row(5), row(4)).err(),
            Some(StructureError::ReversedFoldRegion)
        );
    }

    #[test]
    fn carets_accepts_primary_only_and_distinct_others() {
        let only = Carets::new(Pos::new(1, 2), Vec::new()).expect("单主光标合法");
        assert_eq!(only.primary(), Pos::new(1, 2));
        assert!(only.others().is_empty());

        let many = Carets::new(Pos::new(1, 2), vec![Pos::new(1, 9)]).expect("不同附加光标合法");
        assert_eq!(many.others(), &[Pos::new(1, 9)]);
    }

    #[test]
    fn carets_rejects_caret_duplicating_the_primary() {
        assert_eq!(
            Carets::new(Pos::new(1, 2), vec![Pos::new(1, 2)]).err(),
            Some(StructureError::DuplicateCaret)
        );
    }

    #[test]
    fn scope_stack_rejects_empty_and_refuses_to_pop_the_last_item() {
        assert_eq!(
            ScopeStack::<i32>::new(Vec::new()).err(),
            Some(StructureError::EmptyScopeStack)
        );

        let mut stack = ScopeStack::new(vec![1]).expect("单元素栈合法");
        assert_eq!(stack.top(), Some(&1));
        assert_eq!(stack.pop(), None, "最后一个元素不许弹出");
        assert_eq!(stack.len(), 1);
        assert!(!stack.is_empty());

        stack.push(2);
        assert_eq!(stack.pop(), Some(2));
        assert_eq!(stack.top(), Some(&1));
    }

    #[test]
    fn structure_model_accepts_disjoint_folds_and_preserves_order() {
        let a = FoldRegion::new(row(0), row(2)).expect("区间 a 合法");
        let b = FoldRegion::new(row(4), row(6)).expect("区间 b 合法");
        let carets = Carets::new(Pos::new(0, 0), Vec::new()).expect("光标合法");
        let scopes = ScopeStack::new(vec!["file"]).expect("栈合法");

        let model =
            StructureModel::new(vec![b.clone(), a.clone()], carets, scopes).expect("不相交合法");
        assert_eq!(model.folds(), &[b, a], "构造不得重排调用方顺序");
    }

    #[test]
    fn structure_model_rejects_overlapping_folds() {
        let a = FoldRegion::new(row(0), row(5)).expect("区间 a 合法");
        let b = FoldRegion::new(row(5), row(9)).expect("区间 b 合法");
        let carets = Carets::new(Pos::new(0, 0), Vec::new()).expect("光标合法");
        let scopes = ScopeStack::new(vec!["file"]).expect("栈合法");

        assert_eq!(
            StructureModel::new(vec![a, b], carets, scopes).err(),
            Some(StructureError::OverlappingFoldRegions)
        );
    }

    #[test]
    fn structure_model_accepts_empty_fold_list() {
        // 没有任何折叠也是合法模型：折叠是可选结构，不是必填字段。
        let carets = Carets::new(Pos::new(0, 0), Vec::new()).expect("光标合法");
        let scopes = ScopeStack::new(vec!["file"]).expect("栈合法");

        let model = StructureModel::new(Vec::new(), carets, scopes).expect("空折叠列表合法");
        assert!(model.folds().is_empty());
        assert_eq!(model.carets().primary(), Pos::new(0, 0));
        assert_eq!(model.scopes().top(), Some(&"file"));
    }
}

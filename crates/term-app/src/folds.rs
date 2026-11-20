//! 折叠：**块从哪来** + **块怎么存**。
//!
//! 两部分：
//!
//! - [`FoldRegion`] + [`Folds`]：折叠区间的值类型与它的**有序、互不重叠的集合**。
//!   `FoldRegion` 负责「造不出非法值」（两端是历史绝对行号、闭区间、尾行不得在
//!   首行之前）；`Folds` 负责能插入、能删除、能回答视图真正要问的三个问题——
//!   「这一行现在藏不藏」「下一个折叠头在哪」「上一个折叠头在哪」。
//! - [`blocks_from_inputs`] + [`block_at`]：**折叠块从哪来**，见下方「按用户输入算」一节。
//!
//! 集合维护两条不变式，靠 [`Folds::add`] 与 [`Folds::remove_*`] 守住：
//!
//! 1. `regions` 按 [`FoldRegion::start`] **严格升序**排列；
//! 2. **任意两个区间不相交**（闭区间语义：共享端点即相交，`[0,2]` 与 `[3,5]` 不相交）。
//!
//! # 为什么本模块在**默认编译面**上
//!
//! 它是折叠功能的唯一领域模型，而折叠要被投影层（`apps/desktop` 的 `Tab`）真正读取。
//! `planned` 这个 feature 在本仓的定义是「已实现、已测试，但**零生产读者**」——
//! 有读者还挂在它后面，那个 feature 的语义就废了，后来人也就没法靠它区分
//! 「只是地基」和「已经在用」。
//!
//! 因此 [`FoldRegion`] 与 [`StructureError`] 在 2026-11-14 从 `structure` **搬到了
//! 这里**（类型跟着读者走）：`structure` 里剩下的 `Carets` / `ScopeStack` /
//! `StructureModel` 仍然零读者，继续留在 `planned`，并由它 `pub use` 回本模块的
//! 这两个类型，原来的 `structure::FoldRegion` 路径因此没有断。
//!
//! # 折叠块从哪来：按**用户输入**算，不按空行/段落
//!
//! 用户每敲一次回车，终端在那一行留下一条「输入行」（命令 + 提示符）。相邻两条
//! 输入行之间的那些行，就是上一条命令的**输出**。折叠 = 把这段输出收起来，只留
//! 输入行当折叠头。见 [`blocks_from_inputs`]。
//!
//! 块的闭区间是 **`[输入行, 下一条输入行 - 1]`**——输入行自己**在块内**，靠
//! [`Folds::hides`] 的「区间首行不藏」规则天然成为可见的折叠头。这样「命令行可见、
//! 输出全藏」只需要一条规则、零例外分支；若把输入行留在块外，输出的第一行就会
//! 漏在折叠之外。
//!
//! 这条定义的一个直接后果：最后一条输入之后的内容**不折**——那是正在产生的输出，
//! 折掉它用户就看不见命令执行的结果了。
//!
//! # 生产路径零 panic（4p 冻结基线）
//!
//! 全程不用 `unwrap` / `expect` / 下标索引 / 裸算术：集合操作一律走
//! `iter` / `retain` / `partition_point`，计数用 `saturating_sub`；行号加减走
//! `checked_add` / `checked_sub` 再经 `HistoryRow::new` 校验。越界与溢出在这一层
//! 不成立，而不是「成立但没人碰到」。

use term_core::coords::HistoryRow;

// ── 从 `structure` 平移过来的两个类型（2026-11-14）────────────────────────
//
// 为什么搬：`Folds` 要被 `apps/desktop` 的 `Tab` 真正读取，而 `planned` 这个
// feature 的**定义**是「已实现、已测试，但零生产读者」。一旦有读者还挂在
// planned 下，那个 feature 的语义就废了，后来人也没法靠它判断「哪些模块只是
// 地基」。
//
// 所以把「折叠」这一条线真正需要的两个类型搬到这个默认编译面的模块里，
// 让 `structure`（`Carets` / `ScopeStack` / `StructureModel` 仍然零读者）
// 反过来 `pub use` 它们。搬的是**所有权**，不是语义：字段、不变量、
// 错误变体一字未改。

/// 结构领域值的构造失败原因。
///
/// 四个变体保持原样：本模块只用 `ReversedFoldRegion` / `OverlappingFoldRegions`，
/// 但 `structure` 那边还在用另外两个，删变体会连带改到它。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StructureError {
    /// 折叠区间的尾行在首行之前。
    ReversedFoldRegion,
    /// 附加光标与主光标重合。
    DuplicateCaret,
    /// 作用域/配对栈为空。
    EmptyScopeStack,
    /// 两个折叠区间相交。
    OverlappingFoldRegions,
}

/// 一段折叠区间；两端都是历史绝对行号，闭区间。
///
/// 「首行可见、其余隐藏」是**渲染侧的约定**，不是本类型的不变量——本类型只保证
/// 区间合法（不逆序、不引入第二套行号）。哪一行算折叠头由 [`Folds::hides`] 回答。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FoldRegion {
    start: HistoryRow,
    end: HistoryRow,
}

impl FoldRegion {
    /// 构造。尾行在首行之前时拒绝。
    pub fn new(start: HistoryRow, end: HistoryRow) -> Result<Self, StructureError> {
        if end < start {
            return Err(StructureError::ReversedFoldRegion);
        }
        Ok(Self { start, end })
    }

    pub fn start(&self) -> HistoryRow {
        self.start
    }

    pub fn end(&self) -> HistoryRow {
        self.end
    }

    pub fn contains(&self, row: HistoryRow) -> bool {
        self.start <= row && row <= self.end
    }
}

/// 有序、互不重叠的折叠区间集合。
///
/// 不变式（由本类型的每个 `&mut self` 方法共同维护）：
///
/// - `regions` 按 `start()` 严格升序；
/// - 任意两个区间的闭区间**不相交**（相邻合法）。
///
/// 因为不相交，一行**最多**落在一个区间里，[`Folds::containing`] 因此可以只返回
/// 一个引用，视图层不必处理「同一行被折了两次」的退化情形。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Folds {
    /// 按 `start()` 严格升序、两两不相交的区间。
    ///
    /// 「有序」不只是为了好看：[`Folds::next_header_after`] 与
    /// [`Folds::prev_header_before`] 靠它把「找下一个头」从扫描变成短路返回，
    /// [`Folds::add`] 靠它用二分定位插入点。
    regions: Vec<FoldRegion>,
}

impl Folds {
    /// 空集合：一个折叠也没有。
    ///
    /// 空集合是**合法状态**，不是错误：折叠是可选的视图结构，绝大多数会话一辈子
    /// 都不会有一个区间，所以这里不学 `ScopeStack` 拒绝空值。
    pub fn new() -> Self {
        Self {
            regions: Vec::new(),
        }
    }

    /// 有没有折叠。
    pub fn is_empty(&self) -> bool {
        self.regions.is_empty()
    }

    /// 折叠个数。
    pub fn len(&self) -> usize {
        self.regions.len()
    }

    /// 只读视图，按 `start()` 升序。
    ///
    /// 投影层渲染折叠列时直接遍历它；调用方拿不到 `&mut`，因此不可能绕开
    /// [`Folds::add`] 把集合改成无序或相交。
    pub fn regions(&self) -> &[FoldRegion] {
        &self.regions
    }

    /// 插入一个折叠区间。
    ///
    /// 与集合里任一区间**相交**时返回 [`StructureError::OverlappingFoldRegions`]，
    /// 且**集合原封不动**（先全量检查再插入，不做「插一半回滚」）。
    ///
    /// 相邻不算相交：`[0,2]` 与 `[3,5]` 是闭区间但 2 与 3 不共享端点，两者可以共存。
    /// 共享端点（`[0,5]` 与 `[5,9]` 都有 5）才算相交——这与
    /// `structure::StructureModel::new` 的判据完全一致（`a.start <= b.end && b.start <= a.end`），
    /// 两处不能各写一套。
    pub fn add(&mut self, region: FoldRegion) -> Result<(), StructureError> {
        if self
            .regions
            .iter()
            .any(|existing| overlaps(existing, &region))
        {
            return Err(StructureError::OverlappingFoldRegions);
        }

        // 到这里已确定与所有区间不相交，插入点唯一：第一个 start 严格大于新 start
        // 的位置。不相交保证不会有两区间同 start，所以这里不需要 tie-break。
        // `partition_point` 的返回值恒在 `0..=len`，`Vec::insert` 因此不会越界 panic。
        let index = self
            .regions
            .partition_point(|existing| existing.start() < region.start());
        self.regions.insert(index, region);
        Ok(())
    }

    /// 删掉包含 `row` 的那个区间（闭区间语义）。删掉了返回 `true`。
    ///
    /// 因为区间两两不相交，最多命中一个；`retain` 同时守住「删完仍然有序」。
    /// 删不存在的行是**正常情况**（用户把光标放在平地上按「折叠」切换），返回
    /// `false` 而不报错。
    pub fn remove_containing(&mut self, row: HistoryRow) -> bool {
        let before = self.regions.len();
        self.regions.retain(|region| !region.contains(row));
        self.regions.len() != before
    }

    /// 删掉所有与 `[start, end]` **相交**的区间，返回删除个数（`Unfold Inside` 用）。
    ///
    /// `start > end` 时不会 panic：`overlaps` 是纯比较，此时没有任何区间能与它相交，
    /// 返回 0。调用方若想报错，应在自己那层校验，本层只保证「不炸」。
    pub fn remove_overlapping(&mut self, start: HistoryRow, end: HistoryRow) -> usize {
        let before = self.regions.len();
        self.regions
            .retain(|region| !(region.start() <= end && start <= region.end()));
        before.saturating_sub(self.regions.len())
    }

    /// 清空，返回清掉几个。
    ///
    /// 返回个数是为了让「全部展开」这条命令能说出「展开了 3 处」，
    /// 也为了让调用方能区分「本来就全展开」与「刚展开了一批」。
    pub fn clear(&mut self) -> usize {
        let removed = self.regions.len();
        self.regions.clear();
        removed
    }

    /// 包含 `row` 的那个区间；不在任何区间里时 `None`。
    pub fn containing(&self, row: HistoryRow) -> Option<&FoldRegion> {
        self.regions.iter().find(|region| region.contains(row))
    }

    /// 这一行现在是否**被藏起来**：落在某个区间内，且**不是该区间的首行**。
    ///
    /// 首行是折叠头——它会渲染成「▸ 42 lines」这样的一行摘要，必须可见，
    /// 否则用户无处点击展开。所以 `hides` 不是 `containing(...).is_some()`，
    /// 而是再排除掉首行。这条区别是折叠能用的前提，不能省。
    ///
    /// 单行区间 `[3,3]` 因此**什么都藏不住**（3 是它的首行），但仍然是一个折叠：
    /// 它照样占 `len()`、照样是 `next_header_after(2)` 的答案。
    pub fn hides(&self, row: HistoryRow) -> bool {
        self.containing(row)
            .is_some_and(|region| region.start() != row)
    }

    /// 严格大于 `row` 的下一个折叠头（某区间的 `start`）；没有就 `None`。
    ///
    /// 「折叠头」= 区间首行。集合升序，所以第一个满足 `start > row` 的区间就是答案，
    /// 找到即返回（不需要 `min`）。
    ///
    /// 语义是**严格大于**：`row` 本身正好是某个折叠头时，答案不是它自己——否则
    /// 「跳到下一个折叠」会在原地打转。
    pub fn next_header_after(&self, row: HistoryRow) -> Option<HistoryRow> {
        self.regions
            .iter()
            .map(FoldRegion::start)
            .find(|start| *start > row)
    }

    /// 严格小于 `row` 的上一个折叠头；没有就 `None`（**不环绕**）。
    ///
    /// 不环绕是刻意的：回看缓冲是有顶的，环绕到末尾会让「上一个折叠」跳到一个
    /// 用户看不见的位置。集合升序，所以反向扫描的第一个命中就是最近的。
    pub fn prev_header_before(&self, row: HistoryRow) -> Option<HistoryRow> {
        self.regions
            .iter()
            .map(FoldRegion::start)
            .rev()
            .find(|start| *start < row)
    }
}

impl Default for Folds {
    /// 默认就是空集合——与 [`Folds::new`] 同义，让 `#[derive(Default)]` 的宿主
    /// 结构体（将来的 `ViewState`）不必为折叠写特例。
    fn default() -> Self {
        Self::new()
    }
}

/// 两个闭区间是否相交。
///
/// 与 `structure::StructureModel::new` 用同一个判据：`a.start <= b.end && b.start <= a.end`。
/// 共享端点算相交（`[0,5]` 与 `[5,9]` 都含 5），相邻不算（`[0,2]` 与 `[3,5]`）。
fn overlaps(a: &FoldRegion, b: &FoldRegion) -> bool {
    a.start() <= b.end() && b.start() <= a.end()
}

/// 由「每次用户输入所在的历史行」推出可折叠块。
///
/// # 语义
///
/// 折叠点按**用户输入**计算，折叠的是**两次输入之间的内容**。相邻两个输入 `a < b`
/// 之间形成一个块，闭区间 `[a, b - 1]`：
///
/// - 块的**首行就是输入行 `a`**（命令 + 提示符）。它落在区间内，而
///   [`Folds::hides`] 的规则是「在区间内且不是区间首行才藏」，所以输入行天然可见、
///   成为折叠头——领域层因此**不需要任何例外分支**；
/// - 块的其余行 `a + 1 ..= b - 1` 是这条命令的输出，全部被藏；
/// - `b - a < 2`（两条输入紧挨着，中间没有输出）时这一对**不产生**块。
///
/// 这条跳过判据必须**显式**写。块含输入行之后，`b == a + 1` 会算出单行区间
/// `[a, a]`——它合法（`FoldRegion` 允许单行），但里面除了头行没有任何可折内容，
/// 注册进去就是一个「点了等于没点」的空折叠头。不能靠「倒序判据」兜住它。
///
/// 最后一条输入之后的内容**不折叠**：下一个边界还不存在，那段是正在产生的输出。
///
/// # 为什么块不写成「只含输出」的 `[a + 1, b - 1]`
///
/// 那样会把折叠头放在块外，于是 [`Folds::hides`] 会把 `a + 1`（输出的第一行）
/// 判为可见——第一条输出行漏在折叠之外。要补救就得让渲染层知道「真正的头是
/// `start - 1`」，那是把领域不变量泄漏成调用方的隐式约定。
/// 块含输入行之后，[`Folds::next_header_after`] 返回的也是真实可见的命令行，
/// 「跳到下一个折叠」落点合法。
///
/// # 边界只相邻，不相交
///
/// 相邻两块是 `[a_i, a_{i+1} - 1]` 与 `[a_{i+1}, a_{i+2} - 1]`：前一块的 `end`
/// 恒为后一块的 `start - 1`，只**相邻**、永不相等，所以把结果逐个灌进 [`Folds`]
/// 全部成功（`[0,2]` 与 `[3,5]` 这种相邻本来就合法）。
///
/// # 输入约定与违约行为
///
/// `inputs` 应当**升序去重**（调用方保证）。本函数**不排序**：排序需要一段可变的
/// 临时缓冲，而契约是「调用方按时间顺序把输入行喂进来」，替调用方排序反而会掩盖
/// 它的 bug。
///
/// 违约不会 panic，也不会产出非法区间：`b - a < 2` 把 `b == a + 1` 与 `b <= a`
/// 一并跳过，逆序的一对因此不会去算 `b - 1`（那可能触到负数）。`[7, 0, 3]`
/// 只得到 `[0, 2]`。
///
/// # 零 panic
///
/// `b - a` 与 `b - 1` 都走 `checked_sub`，构造走 `HistoryRow::new`（负行返回
/// `None`）与 [`FoldRegion::new`]（倒序返回 `Err`）；任一环节失败即跳过该块，
/// 不 `expect`。
pub fn blocks_from_inputs(inputs: &[HistoryRow]) -> Vec<FoldRegion> {
    let mut blocks: Vec<FoldRegion> = Vec::new();

    for pair in inputs.windows(2) {
        // `windows(2)` 的长度恒为 2，用切片模式取两端，不碰下标。
        let [a, b] = pair else { continue };

        // 两条输入之间至少要有一行输出才值得折。`b - a < 2` 同时覆盖
        // `b == a + 1`（紧挨着，无输出）与 `b <= a`（调用方没排序，含逆序与重复）。
        let Some(gap) = b.get().checked_sub(a.get()) else {
            continue;
        };
        if gap < 2 {
            continue;
        }

        // 块的尾行 = 下一条输入的前一行。`gap >= 2` 已保证它 > a 且非负。
        let Some(end_row) = b.get().checked_sub(1).and_then(HistoryRow::new) else {
            continue;
        };

        // 起点就是输入行 `a` 本身：它既是折叠头，也在区间内。
        let Ok(block) = FoldRegion::new(*a, end_row) else {
            continue;
        };
        blocks.push(block);
    }

    blocks
}

/// 光标/选区所在的那一个块。
///
/// 命中 = `row` 落在某个块的闭区间 `[输入行, 下一条输入行 - 1]` 内。
///
/// 块的首行就是输入行本身，所以「光标停在自己刚敲的命令行上」这种情形**天然**
/// 命中，不需要任何 `start - 1` 之类的特殊规则——命中判据只有一条，领域层自洽。
///
/// 返回 `None` 的两种情形：
///
/// - `row` 在最后一条输入之后：下一个边界还不存在，那段是正在产生的输出，不折；
/// - `row` 在第一条输入之上：那里还没有任何块。
///
/// 注意 `row` 恰好是**最后一条**输入行时也返回 `None`——它没有「下一条输入」来
/// 划出尾行，因此不属于任何块。
pub fn block_at(inputs: &[HistoryRow], row: HistoryRow) -> Option<FoldRegion> {
    blocks_from_inputs(inputs)
        .into_iter()
        .find(|block| block.contains(row))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 测试专用：把非负整数变成 `HistoryRow`。
    ///
    /// 生产路径不许 `expect`，但测试里字面量行号必须是构造成功的——失败就说明
    /// 调用点写错了，早点炸比静默跳过好。
    fn row(n: i64) -> HistoryRow {
        HistoryRow::new(n).expect("测试用的行号必须非负")
    }

    /// 测试专用：造区间。
    fn region(start: i64, end: i64) -> FoldRegion {
        FoldRegion::new(row(start), row(end)).expect("测试用的区间必须正序")
    }

    #[test]
    fn empty_set_is_empty_and_hides_nothing() {
        // 最重要的一条：折叠是可选结构，空集合必须是一个**正常可用**的状态。
        // 视图每次重绘都会对每一行问一次 `hides`，空集合答错就是全屏错位。
        let folds = Folds::new();
        assert!(folds.is_empty());
        assert_eq!(folds.len(), 0);
        assert!(folds.regions().is_empty());
        assert_eq!(folds.containing(row(0)), None);
        // 从 0 到很远的行号都不能被藏、不能有头。
        assert!(!folds.hides(row(0)));
        assert!(!folds.hides(row(1)));
        assert!(!folds.hides(row(999)));
        assert_eq!(folds.next_header_after(row(0)), None);
        assert_eq!(folds.prev_header_before(row(999)), None);
    }

    #[test]
    fn default_matches_new() {
        // `Default` 与 `new` 必须同义：将来 `ViewState` 会 derive 或手写 Default，
        // 两条路得到的东西不能不一样。
        assert_eq!(Folds::default(), Folds::new());
        assert!(Folds::default().is_empty());
    }

    #[test]
    fn adding_one_region_reports_len_one() {
        // 最朴素的接线：插进去就数得到。这条挂了说明 `add` 根本没落到 `regions` 上。
        let mut folds = Folds::new();
        folds.add(region(2, 5)).expect("空集合插入必须成功");
        assert!(!folds.is_empty());
        assert_eq!(folds.len(), 1);
        assert_eq!(folds.regions().len(), 1);
        let only = folds.regions().first().expect("刚插入的区间必须存在");
        assert_eq!((only.start(), only.end()), (row(2), row(5)));
    }

    #[test]
    fn adjacent_regions_are_allowed() {
        // 相邻（闭区间但端点不共享）**不算**相交。这条如果写错，用户就没法把两个
        // 挨着的函数分别折叠——这是折叠功能最基本的可用性。
        let mut folds = Folds::new();
        folds.add(region(0, 2)).expect("第一个区间合法");
        folds.add(region(3, 5)).expect("相邻区间必须被接受");

        assert_eq!(folds.len(), 2);
        let starts: Vec<i64> = folds.regions().iter().map(|r| r.start().get()).collect();
        assert_eq!(starts, vec![0, 3], "相邻区间按 start 升序并存");
    }

    #[test]
    fn overlapping_region_sharing_one_endpoint_is_rejected() {
        // 闭区间语义：`[0,5]` 与 `[5,9]` 都含 5，因此**相交**。若这里放行，
        // 「这一行属于哪个折叠」就有两个答案，`hides` / 渲染都会不确定。
        let mut folds = Folds::new();
        folds.add(region(0, 5)).expect("第一个区间合法");

        let err = folds.add(region(5, 9)).err();
        assert_eq!(err, Some(StructureError::OverlappingFoldRegions));
        assert_eq!(folds.len(), 1, "被拒绝的插入不得改变集合");
        let only = folds.regions().first().expect("原区间还在");
        assert_eq!((only.start(), only.end()), (row(0), row(5)));
    }

    #[test]
    fn fully_contained_region_is_rejected() {
        // 被包含也是相交（`[2,3] ⊂ [0,9]`）。这条单独测，是因为「包含」这种退化
        // 形态容易被「只比较端点谁大」的实现漏掉。
        let mut folds = Folds::new();
        folds.add(region(0, 9)).expect("第一个区间合法");

        assert_eq!(
            folds.add(region(2, 3)).err(),
            Some(StructureError::OverlappingFoldRegions)
        );
        assert_eq!(folds.len(), 1, "被拒绝的插入不得改变集合");
        // 反向包含同样拒绝：`[1,8]` 也被 `[0,9]` 包含。
        assert_eq!(
            folds.add(region(1, 8)).err(),
            Some(StructureError::OverlappingFoldRegions)
        );
        assert_eq!(folds.len(), 1);
    }

    #[test]
    fn out_of_order_inserts_keep_ascending_order() {
        // 用户的操作顺序是随机的（先折下面的函数，再折上面的），但集合必须始终有序：
        // `next_header_after` / `prev_header_before` 的正确性直接建立在有序上。
        let mut folds = Folds::new();
        folds.add(region(4, 6)).expect("先插后面的区间");
        folds.add(region(0, 2)).expect("再插前面的区间");

        assert_eq!(folds.len(), 2);
        let starts: Vec<i64> = folds.regions().iter().map(|r| r.start().get()).collect();
        assert_eq!(starts, vec![0, 4], "乱序插入后 start 必须升序");

        // 再插一个夹在中间的，验证二分插入点真的是「中间」而不是追加到末尾。
        // 注意选 `[3,3]` 而不是 `[2,3]`：后者与 `[0,2]` 共享端点 2，属于相交，
        // 会被（正确地）拒绝——那已经是另一条用例的事。
        folds.add(region(3, 3)).expect("插到中间也不能与邻居相交");
        let starts: Vec<i64> = folds.regions().iter().map(|r| r.start().get()).collect();
        assert_eq!(starts, vec![0, 3, 4]);
    }

    #[test]
    fn hides_excludes_the_header_row() {
        // 折叠头（首行）必须可见——它是用户点开折叠的唯一入口。
        // 若 `hides(start)` 为真，折叠一旦创建就再也点不开，功能等于自锁。
        let mut folds = Folds::new();
        folds.add(region(2, 5)).expect("区间合法");

        assert!(!folds.hides(row(2)), "首行是折叠头，必须可见");
        assert!(folds.hides(row(3)), "中间行被藏");
        assert!(folds.hides(row(4)), "中间行被藏");
        assert!(folds.hides(row(5)), "尾行（非首行）也被藏");
        assert!(!folds.hides(row(6)), "区间之后不受影响");
        assert!(!folds.hides(row(1)), "区间之前不受影响");
    }

    #[test]
    fn containing_returns_both_endpoints_and_none_outside() {
        // `containing` 是 `hides` 与渲染的共同底层：端点必须都算在内（闭区间），
        // 边界外的两个方向都必须不命中（早一行 / 晚一行）。
        let mut folds = Folds::new();
        folds.add(region(2, 5)).expect("区间合法");

        let hit = folds.containing(row(2)).expect("首行必须命中");
        assert_eq!((hit.start(), hit.end()), (row(2), row(5)));
        let hit = folds.containing(row(4)).expect("中间行必须命中");
        assert_eq!((hit.start(), hit.end()), (row(2), row(5)));
        let hit = folds.containing(row(5)).expect("尾行必须命中");
        assert_eq!((hit.start(), hit.end()), (row(2), row(5)));

        assert_eq!(folds.containing(row(1)), None);
        assert_eq!(folds.containing(row(6)), None);
        assert_eq!(folds.containing(row(0)), None);
    }

    #[test]
    fn remove_containing_removes_only_the_hit() {
        // 「切换折叠」命令的落地：光标在折叠里就展开它。删中间行必须命中整个区间
        // （不只是那一行），因为一个区间就是一个折叠单位。
        let mut folds = Folds::new();
        folds.add(region(2, 5)).expect("区间合法");

        assert!(folds.remove_containing(row(4)), "删中间行必须命中");
        assert_eq!(folds.len(), 0);
        assert!(folds.is_empty());

        // 再删一次：平地上没有折叠可展开，必须安静地返回 false 而不是报错。
        assert!(
            !folds.remove_containing(row(4)),
            "已空集合删任何行都是 false"
        );
        assert!(folds.is_empty());
    }

    #[test]
    fn remove_containing_with_other_regions_leaves_them_alone() {
        // 只删命中的那一个：邻接区间不得被牵连，且删完仍然有序。
        let mut folds = Folds::new();
        folds.add(region(0, 2)).expect("区间 a");
        folds.add(region(4, 6)).expect("区间 b");
        folds.add(region(8, 9)).expect("区间 c");

        assert!(folds.remove_containing(row(5)), "命中区间 b");
        let starts: Vec<i64> = folds.regions().iter().map(|r| r.start().get()).collect();
        assert_eq!(starts, vec![0, 8], "只删 b，a 与 c 都在");

        // 删不存在的行：集合一个都不许动。
        assert!(!folds.remove_containing(row(7)), "7 不在任何区间里");
        assert_eq!(folds.len(), 2);
    }

    #[test]
    fn remove_overlapping_prunes_every_intersecting_region() {
        // `Unfold Inside`（展开选中范围内的全部折叠）就靠这条。命中两个就必须删两个，
        // 删少了用户会看到「按了没反应」，删多了会把选区外的折叠一起展开。
        let mut folds = Folds::new();
        folds.add(region(0, 2)).expect("区间 a");
        folds.add(region(4, 6)).expect("区间 b");
        folds.add(region(8, 9)).expect("区间 c");

        assert_eq!(folds.remove_overlapping(row(1), row(5)), 2, "命中 a 与 b");
        assert_eq!(folds.len(), 1);
        let only = folds.regions().first().expect("剩下的必须是 c");
        assert_eq!((only.start(), only.end()), (row(8), row(9)));
    }

    #[test]
    fn remove_overlapping_without_hits_changes_nothing() {
        // 空手而归是常态（选区落在平地上）。返回 0 且集合不变，调用方才能安全地
        // 拿这个数字做「有没有展开事情发生」的判断。
        let mut folds = Folds::new();
        folds.add(region(0, 2)).expect("区间 a");
        folds.add(region(8, 9)).expect("区间 b");

        assert_eq!(folds.remove_overlapping(row(4), row(6)), 0, "中间是平地");
        assert_eq!(folds.len(), 2);

        // 反序范围（start > end）不得 panic，也不得误删。
        assert_eq!(
            folds.remove_overlapping(row(9), row(0)),
            0,
            "反序范围不命中"
        );
        assert_eq!(folds.len(), 2);
    }

    #[test]
    fn clear_returns_count_and_empties_the_set() {
        // 「全部展开」需要能说出展开了几处；空集合再清一次必须返回 0 而不是炸。
        let mut folds = Folds::new();
        folds.add(region(0, 2)).expect("区间 a");
        folds.add(region(4, 6)).expect("区间 b");

        assert_eq!(folds.clear(), 2, "清掉两个");
        assert!(folds.is_empty());
        assert_eq!(folds.len(), 0);

        assert_eq!(folds.clear(), 0, "已空再清返回 0");
        assert!(folds.is_empty());
    }

    #[test]
    fn next_header_after_is_strict_and_short_circuits() {
        // 「跳到下一个折叠」必须是**严格大于**：若把等号算进去，光标停在折叠头上
        // 按一次就会得到「还是这个头」，用户会以为快捷键坏了。
        let mut folds = Folds::new();
        folds.add(region(0, 2)).expect("区间 a");
        folds.add(region(5, 7)).expect("区间 b");

        assert_eq!(
            folds.next_header_after(row(0)),
            Some(row(5)),
            "从 0（已是头）→ 5"
        );
        assert_eq!(
            folds.next_header_after(row(4)),
            Some(row(5)),
            "从 4（平地）→ 5"
        );
        // 关键边界：5 本身就是第二个区间的头，严格大于 5 的头**不存在**。
        assert_eq!(
            folds.next_header_after(row(5)),
            None,
            "从 5（第二个头）→ 无"
        );
        assert_eq!(folds.next_header_after(row(6)), None, "从 6（折叠内）→ 无");
        assert_eq!(folds.next_header_after(row(7)), None, "从 7（尾行）→ 无");
        assert_eq!(folds.next_header_after(row(8)), None, "越过最后一个头 → 无");
    }

    #[test]
    fn prev_header_before_is_strict_and_does_not_wrap() {
        // 对称的一条：**严格小于**，且到顶就停，不许环绕到末尾。
        // 环绕会让「上一个折叠」跳到用户看不见的地方，比做不到更糟。
        let mut folds = Folds::new();
        folds.add(region(0, 2)).expect("区间 a");
        folds.add(region(5, 7)).expect("区间 b");

        assert_eq!(
            folds.prev_header_before(row(6)),
            Some(row(5)),
            "从 6（折叠内）→ 5"
        );
        assert_eq!(
            folds.prev_header_before(row(5)),
            Some(row(0)),
            "从 5（第二个头）→ 0"
        );
        assert_eq!(
            folds.prev_header_before(row(1)),
            Some(row(0)),
            "从 1（折叠内）→ 0"
        );
        assert_eq!(
            folds.prev_header_before(row(0)),
            None,
            "从 0（第一个头）→ 无，不环绕"
        );
        assert_eq!(
            folds.prev_header_before(row(9)),
            Some(row(5)),
            "从头之后很远 → 最近的头"
        );
    }

    #[test]
    fn single_line_region_folds_nothing_but_still_counts() {
        // `[3,3]` 是退化折叠：只有头行，因此**什么都不藏**。但它仍然是集合里的一个
        // 成员（`len` 算它、导航算它）。若把它当「无效折叠」过滤掉，`FoldRegion`
        // 允许单行区间这件事就在集合层被偷偷改写了。
        let mut folds = Folds::new();
        folds.add(region(3, 3)).expect("单行区间合法");

        assert_eq!(folds.len(), 1);
        assert!(!folds.hides(row(3)), "唯一的一行就是头行，不能藏");
        assert!(!folds.hides(row(2)));
        assert!(!folds.hides(row(4)));

        let hit = folds
            .containing(row(3))
            .expect("单行区间仍能被包含判定命中");
        assert_eq!((hit.start(), hit.end()), (row(3), row(3)));
        assert_eq!(
            folds.next_header_after(row(2)),
            Some(row(3)),
            "单行区间照样是下一个折叠头"
        );
        assert_eq!(folds.prev_header_before(row(4)), Some(row(3)), "反向同理");
    }

    // ── 折叠块推导（`blocks_from_inputs` / `block_at`）────────────────────
    //
    // 块的定义：相邻两个输入 `a < b` → 闭区间 `[a, b - 1]`。**首行就是输入行 `a`**，
    // 靠 `Folds::hides` 的「区间首行不藏」规则天然成为可见的折叠头。
    // 因此「命令行可见、输出全藏」在领域层只有一条规则、零例外分支。

    #[test]
    fn block_spans_from_command_line_to_before_next_input() {
        // 核心语义：折叠的是「两次输入之间的内容」，折叠头是那条命令行本身。
        // `[0,3]` → 块 `[0, 2]`：0 是命令行（头，可见），1..=2 是输出（藏）。
        // 注意**不是** `[1,2]`——块若不含命令行，`hides` 会把输出的第一行判为可见。
        let blocks = blocks_from_inputs(&[row(0), row(3)]);
        assert_eq!(blocks, vec![region(0, 2)]);
    }

    #[test]
    fn adjacent_inputs_yield_no_block() {
        // `[0,1]`：0 这条命令没有任何输出行。块含输入行之后，这一对会算出单行区间
        // `[0,0]`——合法（`FoldRegion` 允许单行），但里面除了头行没有可折内容，
        // 注册进去就是个「点了等于没点」的空折叠头。
        // 所以 `b - a < 2` 必须显式跳过，不能靠倒序判据兜。
        assert!(
            blocks_from_inputs(&[row(0), row(1)]).is_empty(),
            "相邻输入中间无输出，不产生块"
        );
    }

    #[test]
    fn single_output_line_still_forms_a_block() {
        // `[0,2]`：输出只有一行（行 1）。块 `[0,1]`——头 + 一行输出，值得折。
        // 钉住「gap 恰好 2」的下边界：不跳过，也不吞掉那唯一一行输出。
        assert_eq!(blocks_from_inputs(&[row(0), row(2)]), vec![region(0, 1)]);
    }

    #[test]
    fn empty_or_single_input_yields_no_blocks() {
        // 会话刚开始（还没跑过命令）或只跑过一条命令时，没有任何「两次输入之间」的
        // 内容可折。空输入必须安静返回空 Vec，而不是 panic 或造出一个头。
        assert!(blocks_from_inputs(&[]).is_empty());
        assert!(blocks_from_inputs(&[row(7)]).is_empty());
    }

    #[test]
    fn three_inputs_produce_two_blocks_skipping_the_adjacent_pair() {
        // `[5,9,10,14]` 一次覆盖三种情形：
        //   (5,9)   → `[5,8]`   正常块
        //   (9,10)  → 跳过      相邻输入，中间无输出
        //   (10,14) → `[10,13]` 正常块
        let blocks = blocks_from_inputs(&[row(5), row(9), row(10), row(14)]);
        assert_eq!(blocks, vec![region(5, 8), region(10, 13)]);
    }

    #[test]
    fn blocks_are_adjacent_never_overlapping_and_fit_into_folds() {
        // 交叉验证两层：相邻块是 `[a_i, a_{i+1}-1]` 与 `[a_{i+1}, a_{i+2}-1]`，
        // 前一块的 end 恒等于后一块的 start - 1 → 只相邻、永不相等，因此逐个
        // `Folds::add` 必须全部成功（`[0,2]` 与 `[3,5]` 这种相邻本来就合法）。
        //
        // 若哪天块边界算错（比如改成含两端、或把跳过判据删掉），共享端点会让
        // `add` 报 OverlappingFoldRegions，这里立刻红。
        let inputs = [row(0), row(3), row(7), row(8), row(20)];
        let blocks = blocks_from_inputs(&inputs);
        assert_eq!(
            blocks,
            vec![region(0, 2), region(3, 6), region(8, 19)],
            "(0,3)→[0,2]、(3,7)→[3,6]、(7,8) 相邻跳过、(8,20)→[8,19]"
        );

        let mut folds = Folds::new();
        for block in &blocks {
            folds.add(block.clone()).expect("块之间只相邻，不得相交");
        }
        assert_eq!(folds.len(), blocks.len());
        assert_eq!(folds.regions(), blocks.as_slice(), "顺序也一致");
    }

    #[test]
    fn first_input_at_row_zero_never_produces_negative_rows() {
        // 首行输入是 0 时最容易触到 -1。块起点直接用 `a`（不加减），尾行走
        // `checked_sub(1)`；`[0,1]` 由 gap 判据先跳过，根本走不到减法。
        assert!(blocks_from_inputs(&[row(0), row(1)]).is_empty());
        assert_eq!(blocks_from_inputs(&[row(0), row(2)]), vec![region(0, 1)]);
        assert!(
            HistoryRow::new(-1).is_none(),
            "负行号本就构造不出来，任何负数都必须被 checked_sub 拦下"
        );
    }

    #[test]
    fn block_at_covers_the_whole_closed_block() {
        // `[0,3]` → 块 `[0,2]`。闭区间两端都要命中：0 是命令行（用户刚敲完就停在
        // 那里），2 是最后一行输出。少一端就会出现「停在这一行折不动」。
        let inputs = [row(0), row(3)];
        assert_eq!(block_at(&inputs, row(0)), Some(region(0, 2)), "命令行本身");
        assert_eq!(block_at(&inputs, row(1)), Some(region(0, 2)), "输出首行");
        assert_eq!(block_at(&inputs, row(2)), Some(region(0, 2)), "输出尾行");
        // 3 是**最后一条**输入：没有「下一条输入」来划出尾行，它之后是正在产生的
        // 输出，不折，所以它不属于任何块。
        assert_eq!(block_at(&inputs, row(3)), None, "最后一条输入之后");
        assert_eq!(block_at(&inputs, row(4)), None, "正在产生的输出");
    }

    #[test]
    fn block_at_hits_the_command_line_without_any_special_rule() {
        // 方案 A 的好处：命令行命中是 `contains` 的自然结果，不需要 `start - 1`
        // 这类特殊规则。多个块各验一遍，防止有人把起点又算成 `a + 1` 退回去。
        let inputs = [row(0), row(3), row(7)];
        assert_eq!(
            block_at(&inputs, row(0)),
            Some(region(0, 2)),
            "第一条命令行"
        );
        assert_eq!(
            block_at(&inputs, row(3)),
            Some(region(3, 6)),
            "第二条命令行"
        );
        assert_eq!(block_at(&inputs, row(7)), None, "最后一条输入之后没有块");
    }

    #[test]
    fn out_of_order_inputs_are_skipped_instead_of_panicking() {
        // 调用方违约（`[7,0,3]` 不是升序）。本函数**不排序**，按原序看每一对：
        // (7,0) 的 gap 为负 → 跳过（且不会去算 `0 - 1`，因此碰不到负数行）；
        // (0,3) → `[0,2]`。
        //
        // 选「跳过退化对」而不是「排序后重算」：排序要临时缓冲，更糟的是会把
        // 调用方的顺序 bug 掩盖掉。契约是「按时间顺序喂输入行」，违约应当产出
        // 可预测的小结果。这里钉住「不 panic、不产出非法区间」。
        let blocks = blocks_from_inputs(&[row(7), row(0), row(3)]);
        assert_eq!(blocks, vec![region(0, 2)], "逆序的一对被跳过");

        let mut folds = Folds::new();
        for block in &blocks {
            assert!(block.start() <= block.end(), "产出的块必须合法");
            folds.add(block.clone()).expect("合法块必须能插入");
        }
    }

    #[test]
    fn folding_a_block_hides_everything_but_the_command_line() {
        // 决定性的一条，也是方案 A 的验收：把 `[0,3]` 推出的块灌进 `Folds`，
        // 验证「只留命令行」这个用户意图在领域层自洽——规则只有 `hides` 一条，
        // 没有任何例外分支：
        //   命令行 0 可见（区间首行）、输出 1 与 2 全藏、3（下一条命令）可见。
        // 其中 `hides(1) == true` 正是方案 B 会漏掉的那一行。
        let blocks = blocks_from_inputs(&[row(0), row(3)]);
        let mut folds = Folds::new();
        for block in &blocks {
            folds.add(block.clone()).expect("块必须能插入");
        }

        assert!(!folds.hides(row(0)), "命令行是折叠头，必须可见");
        assert!(folds.hides(row(1)), "输出的第一行就要被藏");
        assert!(folds.hides(row(2)), "输出的尾行被藏");
        assert!(!folds.hides(row(3)), "下一条命令行不在块内");

        // 导航边界：末尾之后没有下一个头、开头之前没有上一个头，且不环绕。
        assert_eq!(folds.next_header_after(row(99)), None, "末尾之后没有头");
        assert_eq!(
            folds.prev_header_before(row(0)),
            None,
            "开头之前没有头，不环绕"
        );
        assert_eq!(
            folds.next_header_after(row(0)),
            None,
            "只有一个头且光标就在其上"
        );
        assert_eq!(
            folds.prev_header_before(row(1)),
            Some(row(0)),
            "块内回跳到头"
        );

        // 负数行这个边界在**类型层**就不存在（`HistoryRow::new(-1) == None`），
        // 所以导航函数不需要运行时兜底——这比断言「不 panic」更强。
    }
}

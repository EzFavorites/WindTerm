//! 三套坐标系的类型化边界：历史行 / 视口行 / 字节偏移。
//!
//! 终端里同时存在三个互不相同、却都以整数在代码里流动的量：
//!
//! - **历史行**（[`HistoryRow`]）：从历史第一条算起的绝对行号，恒 ≥ 0；
//! - **视口行**（[`ViewRow`]）：`0` 是**贴底时**的屏幕首行（回看时它已经在屏外），负数是向上回看，没有下界；
//! - **视口行区间**（[`ViewRange`]）：两端都是 [`ViewRow`] 的半开区间；
//! - **字节偏移**（[`ByteOffset`]）：原始字节流里的绝对位置，无符号。
//!
//! 过去它们都是裸 `i64`，于是「这个数是历史行还是视口行」只能靠调用点的上下文和
//! 注释判断。`crates/term-app/src/blocks.rs` 里那几处 `row - history` 就是典型：
//! 减对了是回看，减错了就是另一套坐标系里的行号，而编译器一声不吭。
//!
//! 本模块把换算收成唯一一处 [`history_to_view`] / [`view_to_history`]，并用私有
//! 字段 + sealed trait 堵住跨类型误用。
//!
//! # 与旧裸算式的等价边界
//!
//! **可达域内逐值等价；超界与溢出处是刻意的安全修正。**
//!
//! - `HistoryRow::new` 拒绝负值，故 `row ∈ [0, i64::MAX]`、`history ∈ [0, i64::MAX]`，
//!   两者相减永不溢出，[`history_to_view`] 的 `saturating_sub` 在可达域内不会饱和；
//! - 两处刻意的差异：① `history > i64::MAX` 时旧式 `history as i64` 会回绕成负数，
//!   减法反而变大，这里钳到 `i64::MAX` 并保持结果为负；② [`view_to_history`] 用
//!   `checked_add`，加法溢出返回 `None`，旧式没有这个出口。
//!
//! 因此 P1 迁移必须在**同一组 `history` 实参下逐值比对**；超界与溢出输入按
//! 「刻意的安全修正」单独验收，不要求与旧行为一致。

/// 从历史第一条算起的绝对行号，恒 ≥ 0。
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct HistoryRow(i64);

/// 视口行：`0` = 贴底时的屏幕首行，负数 = 向上回看（`-1` 是最近一行历史）。
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ViewRow(i64);

/// 原始字节流里的绝对偏移。
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ByteOffset(u64);

mod sealed {
    pub trait Sealed {}
}

impl sealed::Sealed for HistoryRow {}
impl sealed::Sealed for ViewRow {}
// `ByteOffset` 刻意不实现：它不是「行空间」，参与行换算没有语义。

/// 可以参与「历史 ↔ 视口」换算的行空间。
///
/// sealed：外部 crate 无法实现，[`ByteOffset`] 也因此被排除在外。
pub trait RowSpace: sealed::Sealed + Copy {
    /// 取出底层整数。给 crate 内部做通用处理用，业务代码请用具体类型的 `get`。
    fn raw(self) -> i64;
}

impl RowSpace for HistoryRow {
    fn raw(self) -> i64 {
        self.0
    }
}

impl RowSpace for ViewRow {
    fn raw(self) -> i64 {
        self.0
    }
}

impl HistoryRow {
    /// 构造。负数不是合法的历史行号，返回 `None`。
    pub const fn new(row: i64) -> Option<Self> {
        if row < 0 {
            None
        } else {
            Some(Self(row))
        }
    }

    /// 取绝对行号。
    pub const fn get(self) -> i64 {
        self.0
    }
}

impl ViewRow {
    /// 构造。`0` 是贴底时的屏首、负数是回看，因此任何 `i64` 都合法。
    pub const fn new(row: i64) -> Self {
        Self(row)
    }

    /// 取视口行号（可为负）。
    pub const fn get(self) -> i64 {
        self.0
    }
}

impl ByteOffset {
    /// 构造。
    pub const fn new(offset: u64) -> Self {
        Self(offset)
    }

    /// 取字节偏移。
    pub const fn get(self) -> u64 {
        self.0
    }

    /// 前进 `n` 字节；溢出返回 `None`，不回绕。
    pub const fn checked_add(self, n: u64) -> Option<Self> {
        match self.0.checked_add(n) {
            Some(v) => Some(Self(v)),
            None => None,
        }
    }
}

/// 视口行区间：`start..end`，两端都是 [`ViewRow`]（`0` = 贴底时的屏首，负数 = 回看）。
///
/// 存在的理由：`Viewport::range` 产出的裸 `Range<i64>`、`ScreenImage.range`、
/// `crop(range)`、`slice_range(range)` 收的裸 `Range<i64>`，四者类型完全一样。
/// 编译器分不出哪个在说视口行、哪个在说随便一个整数区间。把视口这一侧收成
/// 一个类型之后，往裸值下游走必须显式写 [`ViewRange::to_i64`]——擦除点从
/// 「到处隐式」变成「grep 得到、review 看得见」。
///
/// 刻意**不实现** `Iterator`：`Range<Idx>` 的迭代要求 `Idx: Step`（nightly-only），
/// 而且「反向区间怎么迭代」对本类型没有确切语义。需要逐行处理请先 `to_i64()`，
/// 把「我在按裸行号走」写在脸上。
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct ViewRange {
    start: ViewRow,
    end: ViewRow,
}

impl ViewRange {
    /// 构造。半开区间 `start..end`，与 `Range` 的语义一致。
    pub const fn new(start: ViewRow, end: ViewRow) -> Self {
        Self { start, end }
    }

    /// 区间首行。
    pub const fn start(&self) -> ViewRow {
        self.start
    }

    /// 区间尾行（不含）。
    pub const fn end(&self) -> ViewRow {
        self.end
    }

    /// 区间长度（行数）。
    ///
    /// 非空时就是 `end - start`；空区间与反向区间一律给 `0`，溢出也给 `0`——
    /// 「行数」在语义上没有负数，返回负数只会让调用方在下游再钳一次。
    /// 用 `checked_sub` 而不是直接相减：视口区间正常不会溢出，但这里不靠
    /// 「正常输入」成立，靠 `checked_sub` 成立。
    pub const fn len(&self) -> i64 {
        match self.end.0.checked_sub(self.start.0) {
            Some(v) => {
                if v < 0 {
                    0
                } else {
                    v
                }
            }
            None => 0,
        }
    }

    /// 是否为空区间（`end <= start`，或长度溢出）。
    pub const fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// 是否包含某一行。半开语义，与 `Range::contains` 一致。
    pub const fn contains(&self, row: ViewRow) -> bool {
        row.0 >= self.start.0 && row.0 < self.end.0
    }

    /// **唯一的擦除出口**：换成下游还在收的裸 `Range<i64>`。
    ///
    /// 每调一次都在放弃一层类型保护，所以调用点要能一眼看出「这里跨进了裸行号
    /// 的世界」。P1-c 第二刀起，这个出口的用途收窄为 **core 内部委托**：
    /// `ScreenImage` 的 `view_range` 与 `blank_view` / `crop_view` /
    /// `slice_view` 三个类型化入口在内部各调它一次，下游不再自己调。
    /// 裸签名与 `ScreenImage` 字段形状都保持原状（架构蓝图 §8 冻结金样本数据源）。
    pub const fn to_i64(&self) -> std::ops::Range<i64> {
        self.start.0..self.end.0
    }
}

/// 内容绝对行 → 屏内下标：相对窗口首行数行。
///
/// **可达域内逐值等价**于旧的下游裸算式 `row - window_start`。差异只在两处
/// 刻意的安全修正：落在窗口之前（结果为负）与下标溢出 `usize` 时返回 `None`，
/// 旧式会给出一个负下标或在 32 位平台上静默截断。
pub const fn content_row_to_screen_index(row: ViewRow, window_start: ViewRow) -> Option<usize> {
    let offset = match row.0.checked_sub(window_start.0) {
        Some(v) => v,
        None => return None,
    };
    if offset < 0 {
        return None;
    }
    // 32 位平台上 `usize` 上限低于 `i64`，显式守卫截断；64 位平台上这一支恒假，
    // 但保留它才能让同一份源码在两种平台上都正确，而不是靠「屏幕没这么高」。
    if usize::BITS < 64 && offset as u64 > usize::MAX as u64 {
        return None;
    }
    Some(offset as usize)
}

/// 屏内下标 → 内容绝对行：把窗口首行加回来。
///
/// 与 [`view_to_history`] 的区别：这里只做「窗口内相对行 → 窗口坐标系里的
/// 绝对行」，不碰历史行空间，因此结果允许为负（回看窗口的首行本来就可能是负的）。
pub const fn screen_index_to_content_row(index: usize, window_start: ViewRow) -> Option<ViewRow> {
    if index as u64 > i64::MAX as u64 {
        return None;
    }
    match window_start.0.checked_add(index as i64) {
        Some(v) => Some(ViewRow(v)),
        None => None,
    }
}

/// 历史行 → 视口行：减去已经滚出屏幕的行数。
///
/// **可达域内逐值等价**于旧的裸算式 `row - history`（见模块头）。`history` 超过
/// `i64::MAX` 时钳到上界，结果为负而不是回绕变大。
pub const fn history_to_view(row: HistoryRow, history: u64) -> ViewRow {
    let h = if history > i64::MAX as u64 {
        i64::MAX
    } else {
        history as i64
    };
    ViewRow(row.0.saturating_sub(h))
}

/// 视口行 → 历史行：加回已经滚出屏幕的行数。
///
/// 落在历史第一条之前（结果 < 0）或加法溢出时返回 `None`——这两种情况都不是一条
/// 真实存在的历史行。
pub const fn view_to_history(row: ViewRow, history: u64) -> Option<HistoryRow> {
    if history > i64::MAX as u64 {
        return None;
    }
    let abs = match row.0.checked_add(history as i64) {
        Some(v) => v,
        None => return None,
    };
    if abs < 0 {
        None
    } else {
        Some(HistoryRow(abs))
    }
}

// 编译期护栏：只放 `const` 上下文真的能求值的断言。结构体相等、`Result` 匹配这类
// 在 1.94.0 的 const 里仍报 E0015，一律交给下面的真单测，不假装覆盖。
const _: () = {
    assert!(HistoryRow::new(-1).is_none());
};
const _: () = {
    assert!(HistoryRow::new(0).is_some());
};
const _: () = {
    assert!(history_to_view(HistoryRow::new(9).unwrap(), 4).get() == 5);
};
const _: () = {
    assert!(history_to_view(HistoryRow::new(0).unwrap(), 4).get() == -4);
};
const _: () = {
    assert!(view_to_history(ViewRow::new(5), 4).unwrap().get() == 9);
};
const _: () = {
    assert!(view_to_history(ViewRow::new(-4), 4).unwrap().get() == 0);
};
const _: () = {
    assert!(view_to_history(ViewRow::new(-5), 4).is_none());
};
const _: () = {
    assert!(ViewRange::new(ViewRow::new(-3), ViewRow::new(21)).len() == 24);
};
const _: () = {
    assert!(ViewRange::new(ViewRow::new(0), ViewRow::new(0)).is_empty());
};
const _: () = {
    assert!(ViewRange::new(ViewRow::new(-3), ViewRow::new(21)).contains(ViewRow::new(-1)));
};
const _: () = {
    // 半开：尾行本身不在区间里。
    assert!(!ViewRange::new(ViewRow::new(-3), ViewRow::new(21)).contains(ViewRow::new(21)));
};
const _: () = {
    assert!(
        ViewRange::new(ViewRow::new(-3), ViewRow::new(21))
            .to_i64()
            .start
            == -3
    );
};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn history_row_only_accepts_non_negative() {
        assert!(HistoryRow::new(-1).is_none());
        assert!(HistoryRow::new(i64::MIN).is_none());
        assert_eq!(HistoryRow::new(0).map(HistoryRow::get), Some(0));
        assert_eq!(HistoryRow::new(42).map(HistoryRow::get), Some(42));
        assert_eq!(
            HistoryRow::new(i64::MAX).map(HistoryRow::get),
            Some(i64::MAX)
        );
    }

    #[test]
    fn view_row_allows_negative() {
        assert_eq!(ViewRow::new(-1).get(), -1);
        assert_eq!(ViewRow::new(i64::MIN).get(), i64::MIN);
        assert_eq!(ViewRow::new(0).get(), 0);
    }

    #[test]
    fn history_to_view_subtracts_history() {
        assert_eq!(history_to_view(HistoryRow::new(10).unwrap(), 4).get(), 6);
        assert_eq!(history_to_view(HistoryRow::new(10).unwrap(), 0).get(), 10);
    }

    #[test]
    fn history_to_view_goes_negative_for_scrollback() {
        assert_eq!(history_to_view(HistoryRow::new(0).unwrap(), 7).get(), -7);
    }

    #[test]
    fn history_to_view_matches_bare_arithmetic_within_reachable_domain() {
        // 可达域内：row ≥ 0、history ≤ i64::MAX，裸减法不会溢出，两者必须逐值相等。
        for row in 0..=64i64 {
            for history in 0..=64u64 {
                let old = row - history as i64;
                let new = history_to_view(HistoryRow::new(row).unwrap(), history).get();
                assert_eq!(new, old, "row={row} history={history}");
            }
        }
    }

    #[test]
    fn view_to_history_round_trips() {
        for row in 0..=64i64 {
            for history in 0..=64u64 {
                let view = history_to_view(HistoryRow::new(row).unwrap(), history);
                assert_eq!(
                    view_to_history(view, history).map(HistoryRow::get),
                    Some(row),
                    "row={row} history={history}"
                );
            }
        }
    }

    #[test]
    fn view_to_history_rejects_before_first_row() {
        assert!(view_to_history(ViewRow::new(-5), 4).is_none());
        assert_eq!(
            view_to_history(ViewRow::new(-4), 4).map(HistoryRow::get),
            Some(0)
        );
    }

    #[test]
    fn view_to_history_matches_bare_arithmetic_within_reachable_domain() {
        // 可达域内：加法不溢出；落在第一条之前的两边都该是「没有这一行」。
        for row in -64..=64i64 {
            for history in 0..=64u64 {
                let old = row + history as i64;
                let expected = if old < 0 { None } else { Some(old) };
                let new = view_to_history(ViewRow::new(row), history).map(HistoryRow::get);
                assert_eq!(new, expected, "row={row} history={history}");
            }
        }
    }

    #[test]
    fn history_to_view_clamps_when_history_exceeds_i64() {
        let big = i64::MAX as u64 + 1;
        assert_eq!(
            history_to_view(HistoryRow::new(0).unwrap(), big).get(),
            -i64::MAX
        );
        assert_eq!(
            history_to_view(HistoryRow::new(i64::MAX).unwrap(), big).get(),
            0
        );
    }

    #[test]
    fn view_to_history_rejects_when_history_exceeds_i64() {
        let big = i64::MAX as u64 + 1;
        assert!(view_to_history(ViewRow::new(0), big).is_none());
    }

    #[test]
    fn view_to_history_rejects_overflow() {
        assert!(view_to_history(ViewRow::new(i64::MAX), 1).is_none());
        assert_eq!(
            view_to_history(ViewRow::new(i64::MAX - 1), 1).map(HistoryRow::get),
            Some(i64::MAX)
        );
    }

    #[test]
    fn byte_offset_is_checked_not_wrapping() {
        assert_eq!(
            ByteOffset::new(5).checked_add(3).map(ByteOffset::get),
            Some(8)
        );
        assert!(ByteOffset::new(u64::MAX).checked_add(1).is_none());
    }

    #[test]
    fn view_range_keeps_negative_starts_for_scrollback() {
        // 视口行区间的起点可以是负数（回看），这正是它不能和 `HistoryRow` 混的原因。
        let r = ViewRange::new(ViewRow::new(-7), ViewRow::new(17));
        assert_eq!(r.start().get(), -7);
        assert_eq!(r.end().get(), 17);
        assert_eq!(r.len(), 24);
        assert!(!r.is_empty());
    }

    #[test]
    fn view_range_len_matches_the_bare_arithmetic_within_reachable_domain() {
        // 非空半开区间：`end - start` 不溢出时，长度必须逐值等于裸减法。
        for start in -16..=16i64 {
            for end in start..=16i64 {
                let bare = end - start;
                let typed = ViewRange::new(ViewRow::new(start), ViewRow::new(end)).len();
                assert_eq!(typed, bare, "start={start} end={end}");
            }
        }
    }

    #[test]
    fn view_range_len_is_zero_for_empty_and_inverted() {
        assert_eq!(ViewRange::new(ViewRow::new(5), ViewRow::new(5)).len(), 0);
        assert_eq!(ViewRange::new(ViewRow::new(5), ViewRow::new(4)).len(), 0);
        let extreme = ViewRange::new(ViewRow::new(i64::MAX), ViewRow::new(i64::MIN));
        assert_eq!(extreme.len(), 0);
        assert!(extreme.is_empty());
        // 溢出也不 panic：`i64::MIN - i64::MAX` 用裸减法在 debug 下会炸。
        assert_eq!(extreme.end().get(), i64::MIN);
    }

    #[test]
    fn view_range_contains_matches_range_contains() {
        for start in -8..=8i64 {
            for end in -8..=8i64 {
                for row in -8..=8i64 {
                    let bare = (start..end).contains(&row);
                    let typed = ViewRange::new(ViewRow::new(start), ViewRow::new(end))
                        .contains(ViewRow::new(row));
                    assert_eq!(typed, bare, "start={start} end={end} row={row}");
                }
            }
        }
    }

    #[test]
    fn view_range_to_i64_matches_the_bare_range() {
        for start in -40..=40i64 {
            for end in -40..=40i64 {
                let typed = ViewRange::new(ViewRow::new(start), ViewRow::new(end)).to_i64();
                assert_eq!(typed, start..end, "start={start} end={end}");
            }
        }
    }

    #[test]
    fn content_row_to_screen_index_matches_bare_subtraction_in_reachable_domain() {
        // 窗口内的行：下标必须逐值等于旧的裸减法 `row - window_start`。
        for start in -16..=16i64 {
            for len in 0..=16i64 {
                let window_start = ViewRow::new(start);
                for i in 0..len {
                    let row = ViewRow::new(start + i);
                    assert_eq!(
                        content_row_to_screen_index(row, window_start),
                        Some(i as usize),
                        "start={start} len={len} i={i}"
                    );
                }
            }
        }
    }

    #[test]
    fn content_row_to_screen_index_rejects_rows_before_the_window() {
        let window_start = ViewRow::new(-3);
        assert_eq!(
            content_row_to_screen_index(ViewRow::new(-4), window_start),
            None
        );
        assert_eq!(
            content_row_to_screen_index(ViewRow::new(-3), window_start),
            Some(0)
        );
    }

    #[test]
    fn content_row_to_screen_index_rejects_overflow_and_negative_offsets() {
        // 减法本身溢出：`i64::MIN - i64::MAX`，旧式裸减在 debug 下会直接炸。
        assert_eq!(
            content_row_to_screen_index(ViewRow::new(i64::MIN), ViewRow::new(i64::MAX)),
            None
        );
        // 负数偏移一律拒绝，不给「负下标」留下解释空间。
        assert_eq!(
            content_row_to_screen_index(ViewRow::new(-1), ViewRow::new(0)),
            None
        );
    }

    #[test]
    fn screen_index_to_content_row_round_trips_with_the_window_start() {
        for start in -8..=8i64 {
            for i in 0..=8usize {
                let back = screen_index_to_content_row(i, ViewRow::new(start));
                assert_eq!(
                    back.map(ViewRow::get),
                    Some(start + i as i64),
                    "start={start} i={i}"
                );
                // 再换算回去必须回到同一下标，证明两个方向互为逆运算。
                let forward = content_row_to_screen_index(back.unwrap(), ViewRow::new(start));
                assert_eq!(forward, Some(i), "start={start} i={i}");
            }
        }
    }
}

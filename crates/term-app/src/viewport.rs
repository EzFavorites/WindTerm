//! F1：回看视口——纯函数，零 UI 依赖。
//!
//! 只有一个状态：向上滚动的行数（0 = 贴底，跟随输出）。所有换算都是纯的：
//!
//! - `range(rows)` 给出要渲染/请求的行区间 `-scroll .. -scroll + rows`，
//!   负数即历史行（与 `Terminal::slice_range` 的约定一致）；
//! - `max` 由内核在 `ScreenImage.history` 里报告，投影不自己猜上限；
//! - 历史变短（清屏 / resize / 切回主屏）时用 `clamp()` 收回视口，
//!   否则用户会停在一片空白里，看不出发生了什么。

use std::ops::Range;

use term_core::coords::{ViewRange, ViewRow};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Viewport {
    scroll: i64,
}

impl Viewport {
    pub const fn new() -> Self {
        Self { scroll: 0 }
    }

    pub const fn scroll(&self) -> i64 {
        self.scroll
    }

    /// 贴底时为真：此时应跟随输出、显示光标。
    pub const fn is_bottom(&self) -> bool {
        self.scroll == 0
    }

    /// 视口顶部落在**哪一行**（历史绝对行：`0` = 最早的行，`history` = 屏首）。
    ///
    /// 2026-11-19 **只读查看**（`Window ▸ Local Mode`）用的就是这一对纯函数：
    /// 进入只读时用 `top` 记下「视口顶是哪一行」，之后每帧用
    /// [`hold_top`](Self::hold_top) 把它钉回去——`scroll` 是**相对贴底**的位移，
    /// 新输出会把贴底位置抬高，同一个 `scroll` 下一帧就指向了别的内容，画面
    /// 于是自己往上爬；换成绝对行锚才不会。
    pub fn top(&self, history: i64) -> i64 {
        history.max(0) - self.scroll
    }

    /// 把视口顶部钉在**绝对历史行** `top` 上（`0` = 最早的行，`history` = 屏首）。
    ///
    /// 新输出让 `history` 变大时，`scroll` 跟着变大，于是**内容停在原处**；
    /// 历史被裁剪（`history` 变小）时按新上限夹住，而不是停在空白区。
    /// 与 [`jump_to_row`](Self::jump_to_row) 是同一套坐标，区别只在「给的是
    /// 绝对行还是相对回看位移」。
    pub fn hold_top(&mut self, top: i64, history: i64) {
        let history = history.max(0);
        self.scroll = (history - top).clamp(0, history);
    }

    /// 向上滚动（增大回看量）。负数输入视为向下，**0 是空操作**。
    ///
    /// 0 必须单独拦掉：`scroll_up` 与 `scroll_down` 互相把负数甩给对方，
    /// 而 0 既不大于 0 也不小于 0 —— 两边各调一次就回到原地，无限互递归，
    /// 栈直接爆掉（2026-11-07 加 margin 夹具时踩到：那份夹具 `scroll = 0`）。
    pub fn scroll_up(&mut self, lines: i64, max: i64) {
        if lines == 0 {
            return;
        }
        if lines < 0 {
            self.scroll_down(lines.saturating_neg(), max);
            return;
        }
        self.scroll = (self.scroll + lines).min(max.max(0));
    }

    pub fn scroll_down(&mut self, lines: i64, max: i64) {
        if lines == 0 {
            return;
        }
        if lines < 0 {
            self.scroll_up(lines.saturating_neg(), max);
            return;
        }
        self.scroll = (self.scroll - lines).max(0);
        self.clamp(max);
    }

    /// 回到贴底（改名避开 clippy 的 `to_*` + Copy 规则）。
    pub fn jump_to_bottom(&mut self) {
        self.scroll = 0;
    }

    /// R14-d：把某个**回看绝对行号**放到视口顶部。
    ///
    /// 坐标系与 `ScreenImage.range` 一致：0 = 当前屏首行，负数 = 历史行。
    /// 视口只能向上回看，所以 `scroll = -row` 再按内核报告的上限钳制；
    /// 正数行号（当前屏内更靠下的行）等价于贴底，不能用负位移画到屏外。
    /// `saturating_neg` 是刻意的：行号来自用户输入的 `i64`，`i64::MIN` 取负
    /// 会溢出，直接钳到上限比 panic 更符合「用户输入」的语义。
    pub fn jump_to_row(&mut self, row: i64, history: i64) {
        let max = history.max(0);
        self.scroll = row.saturating_neg().clamp(0, max);
    }

    /// 内核历史变短后收回视口。
    pub fn clamp(&mut self, max: i64) {
        let max = max.max(0);
        if self.scroll > max {
            self.scroll = max;
        }
        if self.scroll < 0 {
            self.scroll = 0;
        }
    }

    /// 要展示的行区间，**类型化**版本。`rows` 是当前可见行数。
    ///
    /// 区间说的是视口行（`0` = 贴底时的屏首，负数 = 回看），与 `ScreenImage.range` 同一套
    /// 语义，换成 [`ViewRange`] 之后就不会和平铺的裸 `Range<i64>` 混为一谈。
    pub fn rows(&self, rows: usize) -> ViewRange {
        let rows = rows as i64;
        ViewRange::new(
            ViewRow::new(-self.scroll),
            ViewRow::new(-self.scroll + rows),
        )
    }

    /// 要展示的行区间，裸值版本。
    ///
    /// 现在它就是 `self.rows(rows).to_i64()`：**构造上**保证与类型化版本逐值一致，
    /// 不存在两份会各自漂移的实现。K2 之后生产链（`RequestRange` / `crop_view` /
    /// `blank_view` / `slice_view`）已全部改收 [`ViewRange`]，这里只剩两类读者：
    /// `ScreenImage.range` 这个裸字段的比对断言，以及 demo 夹具的窗口断言。
    pub fn range(&self, rows: usize) -> Range<i64> {
        self.rows(rows).to_i64()
    }

    /// 是否需要向内核请求范围事实（贴底时用快照即可）。
    pub const fn needs_range(&self) -> bool {
        self.scroll > 0
    }

    // ── N：scroll_top <-> scroll 换算（纯函数，零副作用）──

    /// 从当前 scroll 与 history_len 换算出 scroll_top（持久化格式）。
    ///
    /// 语义：
    /// - `scroll_top = 0`  = 视口在历史最顶端（最老的行）
    /// - `scroll_top = history_len` = 视口贴底（跟随输出）
    /// - `scroll = 0` = 贴底，`scroll > 0` = 向上滚动了 N 行
    ///
    /// 换算：`scroll_top = history_len.saturating_sub(scroll as u64)`
    /// （贴底时 scroll=0 => scroll_top=history_len；向上滚 scroll 行 => scroll_top 减少）
    pub fn scroll_top(&self, history_len: u64) -> u64 {
        history_len.saturating_sub(self.scroll.max(0) as u64)
    }

    /// 从 scroll_top 与 history_len 还原 scroll（恢复时使用）。
    ///
    /// 换算：`scroll = (history_len - scroll_top).clamp(0, history_len) as i64`
    /// - scroll_top=history_len => scroll=0 (贴底)
    /// - scroll_top=0 => scroll=history_len (历史顶端)
    /// - scroll_top 超界 => clamp 到 [0, history_len]
    pub fn apply_scroll_top(&mut self, scroll_top: u64, history_len: u64) {
        let max = history_len as i64;
        let scroll = (history_len as i64 - scroll_top as i64).clamp(0, max);
        self.scroll = scroll;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn starts_at_the_bottom() {
        let v = Viewport::new();
        assert!(v.is_bottom());
        assert_eq!(v.scroll(), 0);
        assert_eq!(v.range(24), 0..24);
        assert!(!v.needs_range());
    }

    #[test]
    fn scrolling_up_offsets_the_range_into_history() {
        let mut v = Viewport::new();
        v.scroll_up(3, 100);
        assert_eq!(v.scroll(), 3);
        assert_eq!(v.range(24), -3..21);
        assert!(v.needs_range());
        assert!(!v.is_bottom());
    }

    #[test]
    fn scrolling_by_zero_is_a_no_op() {
        // 回归（2026-11-07）：`scroll_up` 与 `scroll_down` 把「非正数」甩给对方，
        // 而 0 既不属于正也不属于负，两边各来一次就无限互递归 —— margin 夹具
        // `scroll = 0` 直接把测试线程的栈打爆。0 必须是空操作。
        let mut v = Viewport::new();
        v.scroll_up(5, 10);
        v.scroll_up(0, 10);
        assert_eq!(v.scroll(), 5);
        v.scroll_down(0, 10);
        assert_eq!(v.scroll(), 5, "0 行既不该上滚也不该下滚");
        // 极端值也不能靠 `-lines` 溢出：往下甩一个 i64::MIN 就是「滚到最底」。
        v.scroll_up(i64::MIN, 10);
        assert_eq!(v.scroll(), 0);
    }

    #[test]
    fn scroll_is_clamped_to_available_history() {
        let mut v = Viewport::new();
        v.scroll_up(999, 7);
        assert_eq!(v.scroll(), 7, "不得超过内核报告的历史行数");
        v.scroll_up(1, 7);
        assert_eq!(v.scroll(), 7);
    }

    #[test]
    fn scrolling_down_returns_to_the_bottom_and_stops() {
        let mut v = Viewport::new();
        v.scroll_up(5, 100);
        v.scroll_down(2, 100);
        assert_eq!(v.scroll(), 3);
        v.scroll_down(99, 100);
        assert_eq!(v.scroll(), 0);
        assert!(v.is_bottom());
    }

    #[test]
    fn history_shrinking_pulls_the_viewport_back() {
        let mut v = Viewport::new();
        v.scroll_up(40, 100);
        v.clamp(5);
        assert_eq!(v.scroll(), 5, "历史变短后停在新上限，而不是空白区");
        v.clamp(0);
        assert!(v.is_bottom());
    }

    #[test]
    fn negative_input_is_treated_as_the_opposite_direction() {
        let mut v = Viewport::new();
        v.scroll_up(-4, 100);
        assert_eq!(v.scroll(), 0);
        v.scroll_up(4, 100);
        v.scroll_down(-2, 100);
        assert_eq!(v.scroll(), 6);
    }

    #[test]
    fn jump_to_bottom_resets() {
        let mut v = Viewport::new();
        v.scroll_up(9, 100);
        v.jump_to_bottom();
        assert!(v.is_bottom());
        assert_eq!(v.range(10), 0..10);
    }

    #[test]
    fn goto_row_uses_the_same_absolute_row_coordinates_as_the_fact_window() {
        let mut v = Viewport::new();
        v.jump_to_row(-25, 100);
        assert_eq!(v.scroll(), 25);
        assert_eq!(v.range(24), -25..-1);
        assert!(!v.is_bottom());
        v.jump_to_row(0, 100);
        assert!(v.is_bottom());
    }

    #[test]
    fn goto_row_is_clamped_to_available_history() {
        let mut v = Viewport::new();
        v.jump_to_row(-999, 7);
        assert_eq!(v.scroll(), 7);
        // 正数行号表示「当前屏内」，视口不该向下越过贴底。
        v.jump_to_row(10, 7);
        assert_eq!(v.scroll(), 0);
    }

    #[test]
    fn goto_row_never_panics_on_the_smallest_integer() {
        let mut v = Viewport::new();
        v.jump_to_row(i64::MIN, 12);
        assert_eq!(v.scroll(), 12);
    }

    #[test]
    fn range_length_always_matches_the_visible_rows() {
        let mut v = Viewport::new();
        v.scroll_up(2, 100);
        for rows in [1usize, 24, 60] {
            let r = v.range(rows);
            assert_eq!((r.end - r.start) as usize, rows);
        }
    }

    #[test]
    fn rows_and_range_agree_for_every_scroll_and_row_count() {
        // `range()` 现在是 `rows().to_i64()`，两者由构造保证一致；这条测试守的是
        // 将来有人把 `range()` 改回自己算一份——那时它会红。
        let mut v = Viewport::new();
        for scroll in 0..=8i64 {
            v.jump_to_bottom();
            v.scroll_up(scroll, 100);
            for rows in [0usize, 1, 24, 60] {
                let typed = v.rows(rows);
                assert_eq!(typed.to_i64(), v.range(rows), "scroll={scroll} rows={rows}");
                assert_eq!(typed.len() as usize, rows, "scroll={scroll} rows={rows}");
                // 回看时起点必须是负数，贴底时起点是 0。
                assert_eq!(typed.start().get(), -scroll);
                assert_eq!(typed.end().get(), -scroll + rows as i64);
            }
        }
    }

    #[test]
    fn top_and_hold_top_round_trip_at_every_scroll() {
        // 只读查看的冻结锚点：`top` 与 `hold_top` 必须互为逆运算，否则「进入
        // 只读」那一帧画面就会跳一下（用户看到的第一眼就错了）。
        let mut v = Viewport::new();
        for scroll in 0..=8i64 {
            v.jump_to_bottom();
            v.scroll_up(scroll, 100);
            let top = v.top(100);
            assert_eq!(top, 100 - scroll);
            let mut back = Viewport::new();
            back.hold_top(top, 100);
            assert_eq!(back.scroll(), scroll, "scroll={scroll}");
        }
    }

    #[test]
    fn holding_a_top_freezes_the_content_when_history_grows() {
        // 只读查看的核心判据：新输出进来（history 变大）时，**内容不许动**。
        // 先贴底，再钉住屏首那一行；然后模拟输出 10 行。
        let mut v = Viewport::new();
        v.scroll_up(3, 40);
        let top = v.top(40);
        assert_eq!(top, 37);
        v.hold_top(top, 50);
        assert_eq!(v.scroll(), 13, "history 涨 10，scroll 也得涨 10");
        assert_eq!(v.range(24).start, -13, "视口顶对应的仍是同一行内容");
        // 相对位移 13 ≠ 3，但绝对行没变——这正是「同一个 scroll 值会漂」的反例。
        assert_eq!(v.top(50), top);
    }

    #[test]
    fn holding_a_top_clamps_when_history_is_trimmed() {
        // 历史被裁剪（history 变小）：钉不住就夹回可显示区间 `[0, history]`，
        // 绝不留空白区。`top = history` 就是屏首（贴底），`top = 0` 是最早一行。
        let mut v = Viewport::new();
        v.hold_top(37, 5);
        assert_eq!(v.top(5), 5, "只比屏首还靠后就等价于贴底");
        assert_eq!(v.scroll(), 0);
        v.hold_top(-10, 5);
        assert_eq!(v.top(5), 0, "负的 top 夹到最早一行");
        assert_eq!(v.scroll(), 5);
        v.hold_top(9_999, 5);
        assert_eq!(v.scroll(), 0, "超过屏首的 top 等价于贴底");
    }

    // ── N：scroll_top 换算测试 ──

    #[test]
    fn n_scroll_top_at_bottom_is_history_len() {
        let v = Viewport::new(); // scroll = 0
        assert_eq!(v.scroll_top(1000), 1000);
        assert_eq!(v.scroll_top(0), 0);
    }

    #[test]
    fn n_scroll_top_after_scroll_up() {
        let mut v = Viewport::new();
        v.scroll_up(50, 1000);
        assert_eq!(v.scroll(), 50);
        assert_eq!(v.scroll_top(1000), 950); // 1000 - 50
    }

    #[test]
    fn n_scroll_top_clamped_at_history_top() {
        let mut v = Viewport::new();
        v.scroll_up(2000, 1000); // 超过历史上限，clamp 到 1000
        assert_eq!(v.scroll(), 1000);
        assert_eq!(v.scroll_top(1000), 0);
    }

    #[test]
    fn n_apply_scroll_top_at_bottom() {
        let mut v = Viewport::new();
        v.apply_scroll_top(1000, 1000); // scroll_top = history_len => 贴底
        assert!(v.is_bottom());
        assert_eq!(v.scroll(), 0);
    }

    #[test]
    fn n_apply_scroll_top_at_history_top() {
        let mut v = Viewport::new();
        v.apply_scroll_top(0, 1000); // scroll_top = 0 => 历史最顶端
        assert_eq!(v.scroll(), 1000);
        assert!(!v.is_bottom());
    }

    #[test]
    fn n_apply_scroll_top_midway() {
        let mut v = Viewport::new();
        v.apply_scroll_top(400, 1000); // 中间位置
        assert_eq!(v.scroll(), 600); // 1000 - 400
        assert!(!v.is_bottom());
    }

    #[test]
    fn n_apply_scroll_top_clamped_when_out_of_bounds() {
        let mut v = Viewport::new();
        // scroll_top > history_len => clamp 到 history_len => 贴底
        v.apply_scroll_top(1500, 1000);
        assert!(v.is_bottom());
        assert_eq!(v.scroll(), 0);

        // scroll_top = 0 是合法边界（历史最顶端）
        v.apply_scroll_top(0, 1000);
        assert_eq!(v.scroll(), 1000);
    }

    #[test]
    fn n_scroll_top_round_trip() {
        // scroll -> scroll_top -> scroll 逐值恢复
        let test_cases = [(0u64, 1000), (50, 1000), (100, 1000), (500, 1000), (999, 1000), (1000, 1000)];
        for (scroll_before, history) in test_cases {
            let mut v1 = Viewport::new();
            v1.scroll_up(scroll_before as i64, history as i64);
            let st = v1.scroll_top(history);
            let mut v2 = Viewport::new();
            v2.apply_scroll_top(st, history);
            assert_eq!(v1.scroll(), v2.scroll(), "round trip: scroll={scroll_before} history={history}");
        }
    }
}

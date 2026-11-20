//! 跨 chunk 的双指针窗口（R 规格 §4d）。
//!
//! 一个 buffer、两个单调指针：
//!
//! - `scan`：**已检查到哪**。`probe`（触发器路径）用它避免重复报同一个命中，
//!   但**不消费**任何数据。
//! - `consume`：**已交给调用方到哪**。`take` / `drain`（脚本路径）推进它。
//!
//! 两者都只前进，且恒有 `consume <= scan <= buf.len()`。`probe` 永远抬 `scan`，
//! `take` 同时抬两者——于是触发器不可能吃掉脚本要读的数据，脚本也不可能让
//! 触发器重复报同一段文本。
//!
//! 洪水下 `trim` 从头部丢字节，两个指针同步左移（clamp 到 0）。这会丢掉
//! 尚未消费的旧数据，是"保尾"的既定取舍——调用方须把 `cap` 设为所有活跃
//! 模式的最大 `window_hint` 所需长度。

use crate::automation::pattern::{Pattern, Span, WindowHint};

/// 跨 chunk 的滑动窗口。
#[derive(Clone, Debug)]
pub struct ScanCursor {
    buf: String,
    scan: usize,
    consume: usize,
    cap: usize,
}

impl ScanCursor {
    /// `cap` 是窗口的最大字节数，须 ≥ 所有活跃模式 `window_hint - 1` 的最大值。
    /// 传 0 会被抬到 1，避免出现一个永远空的窗口。
    pub fn new(cap: usize) -> Self {
        Self {
            buf: String::new(),
            scan: 0,
            consume: 0,
            cap: cap.max(1),
        }
    }

    /// 由模式的 `window_hint` 推出足够保留的窗口长度。
    pub fn for_pattern(p: &Pattern, floor: usize) -> Self {
        let need = match p.window_hint() {
            WindowHint::Bounded(n) => n.saturating_sub(1),
            WindowHint::Unbounded => floor,
        };
        Self::new(need.max(floor))
    }

    pub fn cap(&self) -> usize {
        self.cap
    }
    pub fn buffered_len(&self) -> usize {
        self.buf.len()
    }
    pub fn consume_offset(&self) -> usize {
        self.consume
    }

    /// 从 `consume` 起找命中并消费，返回「自旧 consume 到命中结尾」的文本，
    /// 以及命中串相对该文本的 `Span`。文本类等待动词共用它：`text` 含命中串，
    /// `span` 给出命中串在 `text` 里的位置（跨游标命中的起点钳到 0）。
    pub fn take_text(&mut self, p: &Pattern) -> Option<(String, Span)> {
        let from = self.consume;
        let hit = self.take(p)?;
        let text = self.buf[from..hit.end].to_owned();
        let span = Span {
            start: hit.start.saturating_sub(from),
            end: hit.end - from,
        };
        Some((text, span))
    }

    /// 追加一批（已清洗的）输出。`scan` 不动——新数据仍在"未检查"区。
    pub fn push(&mut self, chunk: &str) {
        self.buf.push_str(chunk);
        self.trim();
    }

    /// 只检查、不消费：找第一个结束位置晚于 `scan` 的命中，并把 `scan`
    /// 抬到命中结尾，保证同一命中不会报第二次。
    ///
    /// 没有命中时把 `scan` 抬到 `buf.len() - cap`，仅保留尾部供下一批拼接；
    /// 这是"每批每规则最多一命中"与有界内存的共同基础。
    pub fn probe(&mut self, p: &Pattern) -> Option<Span> {
        if let Some(hit) = p.find(&self.buf, self.scan) {
            self.scan = hit.end;
            self.raise_consume_to_scan();
            return Some(hit);
        }
        let keep_from = self.buf.len().saturating_sub(self.cap);
        self.scan = self.scan.max(keep_from);
        self.raise_consume_to_scan();
        None
    }

    /// 检查并消费：从 `consume` 起找命中，把 `consume` 推到命中结尾。
    pub fn take(&mut self, p: &Pattern) -> Option<Span> {
        let hit = p.find(&self.buf, self.consume)?;
        self.consume = hit.end;
        self.scan = self.scan.max(hit.end);
        Some(hit)
    }

    /// 把 `consume` 直接推进到缓冲的绝对偏移 `to`（同步 `scan`，不变量不破）。
    /// 多模式等待在各自 probe 后用它推进到选中的命中结尾，避免重新查找带来
    /// 的基准漂移。`to` 会 clamp 到 `[consume, buf.len()]` 的 char boundary。
    pub fn consume_to(&mut self, to: usize) {
        let to = to.clamp(self.consume, self.buf.len());
        let to = self.buf.floor_char_boundary(to);
        self.consume = to;
        self.scan = self.scan.max(to);
    }

    /// 取走全部未消费文本并推进游标。缓冲不清空（保留尾部供 `probe` 拼接），
    /// 由 `trim` 把它压回 `cap` 以内。
    pub fn drain(&mut self) -> String {
        let to = self.buf.len();
        let out = if self.consume <= to && self.buf.is_char_boundary(self.consume) {
            self.buf[self.consume..to].to_owned()
        } else {
            String::new()
        };
        self.consume = to;
        self.scan = self.scan.max(to);
        self.trim();
        out
    }

    /// 当前未消费文本（只读，不推进）。
    pub fn unconsumed(&self) -> &str {
        if self.consume <= self.buf.len() && self.buf.is_char_boundary(self.consume) {
            &self.buf[self.consume..]
        } else {
            ""
        }
    }

    /// 会话结束：清掉全部状态与缓冲。
    pub fn reset(&mut self) {
        self.buf.clear();
        self.scan = 0;
        self.consume = 0;
    }

    /// 维护 `consume <= scan`。`take` 之后两者可能短暂相等，`probe` 只抬 scan，
    /// 这里保证任何路径结束后不变量都成立。
    fn raise_consume_to_scan(&mut self) {
        debug_assert!(self.consume <= self.scan);
    }

    /// 从头部丢到 `cap` 以内，两个指针同步左移。
    fn trim(&mut self) {
        if self.buf.len() <= self.cap {
            return;
        }
        let drop = self.buf.floor_char_boundary(self.buf.len() - self.cap);
        if drop == 0 {
            return;
        }
        self.buf.drain(..drop);
        self.scan = self.scan.saturating_sub(drop);
        self.consume = self.consume.saturating_sub(drop);
        debug_assert!(self.consume <= self.scan && self.scan <= self.buf.len());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::automation::pattern::Syntax;

    fn lit(s: &str) -> Pattern {
        Pattern::compile(s, Syntax::Literal, true).unwrap()
    }

    #[test]
    fn a_match_may_span_two_chunks() {
        let p = lit("password:");
        let mut c = ScanCursor::for_pattern(&p, 16);
        c.push("login: pass");
        assert!(c.probe(&p).is_none());
        c.push("word: ");
        let hit = c.probe(&p).unwrap();
        assert_eq!(&c.buf[hit.start..hit.end], "password:");
    }

    #[test]
    fn probe_does_not_consume_what_take_will_read() {
        let p = lit("hi");
        let mut c = ScanCursor::new(16);
        c.push("hi there");
        assert!(c.probe(&p).is_some(), "触发器报了一次");
        assert!(c.probe(&p).is_none(), "同一命中不重复报");
        let hit = c.take(&p).unwrap();
        assert_eq!(&c.buf[hit.start..hit.end], "hi", "脚本仍能读到同一段");
    }

    #[test]
    fn take_advances_consume_and_drain_returns_the_rest() {
        let p = lit("OK");
        let mut c = ScanCursor::new(32);
        c.push("noise OK tail");
        assert!(c.take(&p).is_some());
        assert_eq!(c.unconsumed(), " tail");
        assert_eq!(c.drain(), " tail");
        assert_eq!(c.unconsumed(), "");
        assert!(c.take(&p).is_none());
    }

    #[test]
    fn probe_and_take_use_different_pointers() {
        let a = lit("A");
        let b = lit("B");
        let mut c = ScanCursor::new(64);
        c.push("xAxBx");
        assert!(c.probe(&a).is_some());
        assert!(c.probe(&a).is_none());
        assert!(c.take(&b).is_some());
        assert_eq!(c.unconsumed(), "x");
    }

    #[test]
    fn trim_keeps_the_cap_and_the_invariant() {
        let p = lit("END");
        let mut c = ScanCursor::new(8);
        c.push("0123456789");
        assert!(c.buffered_len() <= 8);
        assert!(c.consume <= c.scan && c.scan <= c.buffered_len());
        c.push("END");
        assert!(c.probe(&p).is_some(), "尾巴没被裁掉");
    }

    #[test]
    fn unicode_is_counted_in_bytes_and_never_split() {
        let p = lit("密码");
        let mut c = ScanCursor::new(16);
        c.push("输入 密");
        assert!(c.probe(&p).is_none());
        c.push("码 ok");
        let hit = c.probe(&p).unwrap();
        assert_eq!(&c.buf[hit.start..hit.end], "密码");
        assert_eq!(hit.len(), 6);
    }

    #[test]
    fn reset_clears_everything() {
        let p = lit("x");
        let mut c = ScanCursor::new(8);
        c.push("x");
        assert!(c.probe(&p).is_some());
        c.reset();
        assert_eq!(c.unconsumed(), "");
        assert!(c.probe(&p).is_none());
    }

    #[test]
    fn take_text_returns_consumed_through_hit_with_a_relative_span() {
        let p = lit("hi");
        let mut c = ScanCursor::new(32);
        c.push("xx hi tail");
        let (text, span) = c.take_text(&p).unwrap();
        assert_eq!(text, "xx hi");
        assert_eq!(&text[span.start..span.end], "hi");
        assert_eq!(c.unconsumed(), " tail");
    }

    #[test]
    fn zero_cap_is_clamped_to_one() {
        let mut c = ScanCursor::new(0);
        assert_eq!(c.cap(), 1);
        c.push("ab");
        assert!(c.buffered_len() <= 1);
    }
}

//! Vim 本地模式键位族的**解析层**：按键 → 一个动作意图。
//!
//! 审计指出的缺口是「六篇 Vim 文档的按键语法全仓零解析代码」。这里补的正是
//! 那一层：一个不认识 `ViewState`、不认识终端视图、不读盘的纯状态机。它只把
//! 字符序列翻译成 motion + 重复次数，落地由投影层决定（本地模式下移动视口 /
//! 选区，永远不写 PTY）。
//!
//! 本批覆盖的是**导航键位**：`h/j/k/l`、`w/b`、`0/$`、`gg/G` 与数字前缀。
//! operator（d/y/c）、register（"a）、可视模式等属于完整 Vim 模拟，范围更大，
//! 不在这一个增量里假装实现。

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Motion {
    Left,
    Right,
    Up,
    Down,
    WordForward,
    WordBack,
    LineStart,
    LineEnd,
    FirstLine,
    LastLine,
}

#[derive(Clone, Debug, Default)]
pub struct VimState {
    count: usize,
    pending_g: bool,
}

impl VimState {
    pub fn reset(&mut self) {
        self.count = 0;
        self.pending_g = false;
    }

    /// 喂一个字符。返回 `(motion, 重复次数)`；还在等后续字符（数字、`g`）时
    /// 返回 `None`。非法字符清掉未完成的序列，不把半截前缀留到下一次按键。
    pub fn feed(&mut self, ch: char) -> Option<(Motion, usize)> {
        let out = match ch {
            '1'..='9' => {
                self.count = self.count.saturating_mul(10).saturating_add((ch as u8 - b'0') as usize);
                return None;
            }
            'g' if self.pending_g => {
                self.pending_g = false;
                Motion::FirstLine
            }
            'g' => {
                self.pending_g = true;
                return None;
            }
            'G' => Motion::LastLine,
            'h' => Motion::Left,
            'l' => Motion::Right,
            'j' => Motion::Down,
            'k' => Motion::Up,
            'w' => Motion::WordForward,
            'b' => Motion::WordBack,
            // `0` 在 Vim 里就是「行首」，不是计数的一部分；计数只用 1–9 起头。
            '0' => Motion::LineStart,
            '$' => Motion::LineEnd,
            _ => {
                self.reset();
                return None;
            }
        };
        let count = self.count;
        self.reset();
        Some((out, count.max(1)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn basic_motions_carry_a_default_count_of_one() {
        let mut v = VimState::default();
        assert_eq!(v.feed('h'), Some((Motion::Left, 1)));
        assert_eq!(v.feed('j'), Some((Motion::Down, 1)));
        assert_eq!(v.feed('$'), Some((Motion::LineEnd, 1)));
    }

    #[test]
    fn digit_prefix_multiplies_and_a_motion_clears_it() {
        let mut v = VimState::default();
        assert_eq!(v.feed('1'), None);
        assert_eq!(v.feed('2'), None);
        assert_eq!(v.feed('j'), Some((Motion::Down, 12)));
        assert_eq!(v.feed('k'), Some((Motion::Up, 1)), "计数用完就清");
    }

    #[test]
    fn gg_is_a_two_key_sequence_and_an_unknown_key_aborts_it() {
        let mut v = VimState::default();
        assert_eq!(v.feed('g'), None);
        assert_eq!(v.feed('g'), Some((Motion::FirstLine, 1)));
        assert_eq!(v.feed('G'), Some((Motion::LastLine, 1)));
        assert_eq!(v.feed('g'), None);
        assert_eq!(v.feed('x'), None);
        assert_eq!(v.feed('g'), None, "非法键要清掉挂起的 g");
        assert_eq!(v.feed('g'), Some((Motion::FirstLine, 1)));
    }
}

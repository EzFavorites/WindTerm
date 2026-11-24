//! Vim 本地模式**导航键**的解析层：按键序列 → motion + 重复次数。
//!
//! 依据：`kingtoolbox-docs/Keyboard/Navigation-Keys.md`（官方 page/22）列出的
//! 26 个键位，全部在这里解析：h/j/k/l、0/$/^、w/W/b/B/e/E/ge/gE、gg/nG/G、
//! -/+、(/)、{/}。复制/粘贴（page/23）是另一组 operator，见 `VimYank` 的
//! 独立状态；本文件只管「往哪走」，落地由投影层决定（本地模式下移视口/选区，
//! 永不写 PTY）。
//!
//! 这是纯状态机：不认识 `ViewState`、不认识终端视图、不读盘。未知字符清掉
//! 未完成的数字/两键前缀，不把半截序列留到下一次按键。

/// 一个导航意图。词族区分「空白分隔」与「会话配置的分隔符」两套口径：
/// Vim 的大写键（W/B/E/gE）永远按空白分词，所以落地层要能传另一套 separators。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Motion {
    Left,
    Right,
    Up,
    Down,
    WordForward,
    WordForwardBlank,
    WordBack,
    WordBackBlank,
    WordEnd,
    WordEndBlank,
    WordEndBack,
    WordEndBackBlank,
    LineStart,
    LineStartNonBlank,
    LineEnd,
    FirstLine,
    LastLine,
    SentencePrev,
    SentenceNext,
    ParagraphPrev,
    ParagraphNext,
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

    /// 喂一个字符。返回 `(motion, 重复次数)`；等后续字符时返回 `None`。
    /// 数字前缀 1–9 相乘累加，`0` 在 Vim 里是「行首」而不是计数的一部分。
    pub fn feed(&mut self, ch: char) -> Option<(Motion, usize)> {
        let out = if self.pending_g {
            self.pending_g = false;
            match ch {
                'g' => Motion::FirstLine,
                'e' => Motion::WordEndBack,
                'E' => Motion::WordEndBackBlank,
                _ => {
                    self.reset();
                    return None;
                }
            }
        } else {
            match ch {
                '1'..='9' => {
                    self.count = self
                        .count
                        .saturating_mul(10)
                        .saturating_add((ch as u8 - b'0') as usize);
                    return None;
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
                'W' => Motion::WordForwardBlank,
                'b' => Motion::WordBack,
                'B' => Motion::WordBackBlank,
                'e' => Motion::WordEnd,
                'E' => Motion::WordEndBlank,
                '0' => Motion::LineStart,
                '$' => Motion::LineEnd,
                '^' => Motion::LineStartNonBlank,
                '-' | '(' => Motion::SentencePrev,
                '+' | ')' => Motion::SentenceNext,
                '{' => Motion::ParagraphPrev,
                '}' => Motion::ParagraphNext,
                _ => {
                    self.reset();
                    return None;
                }
            }
        };
        let count = self.count.max(1);
        self.reset();
        Some((out, count))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 把一个键序喂完，断言最后产出；中间步骤必须都是「还在等」。
    fn feed_all(keys: &str) -> Option<(Motion, usize)> {
        let mut v = VimState::default();
        let mut out = None;
        for (i, ch) in keys.chars().enumerate() {
            let got = v.feed(ch);
            if i + 1 < keys.len() {
                assert_eq!(got, None, "{keys:?} 的第 {i} 键不该提前产出");
            }
            out = got;
        }
        out
    }

    #[test]
    fn basic_motions_carry_a_default_count_of_one() {
        let mut v = VimState::default();
        assert_eq!(v.feed('h'), Some((Motion::Left, 1)));
        assert_eq!(v.feed('j'), Some((Motion::Down, 1)));
        assert_eq!(v.feed('$'), Some((Motion::LineEnd, 1)));
        assert_eq!(v.feed('^'), Some((Motion::LineStartNonBlank, 1)));
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

    #[test]
    fn every_word_key_from_the_doc_has_a_distinct_motion() {
        let cases = [
            ('w', Motion::WordForward),
            ('W', Motion::WordForwardBlank),
            ('b', Motion::WordBack),
            ('B', Motion::WordBackBlank),
            ('e', Motion::WordEnd),
            ('E', Motion::WordEndBlank),
        ];
        for (key, want) in cases {
            assert_eq!(feed_all(&key.to_string()), Some((want, 1)), "键 {key}");
        }
        assert_eq!(feed_all("ge"), Some((Motion::WordEndBack, 1)));
        assert_eq!(feed_all("gE"), Some((Motion::WordEndBackBlank, 1)));
        assert_eq!(feed_all("3ge"), Some((Motion::WordEndBack, 3)));
    }

    #[test]
    fn sentence_and_paragraph_keys_and_nth_line_are_parsed() {
        for (key, want) in [
            ('-', Motion::SentencePrev),
            ('+', Motion::SentenceNext),
            ('(', Motion::SentencePrev),
            (')', Motion::SentenceNext),
            ('{', Motion::ParagraphPrev),
            ('}', Motion::ParagraphNext),
        ] {
            assert_eq!(feed_all(&key.to_string()), Some((want, 1)), "键 {key}");
        }
        assert_eq!(feed_all("12G"), Some((Motion::LastLine, 12)), "nG 的 n 走计数");
        assert_eq!(feed_all("G"), Some((Motion::LastLine, 1)), "裸 G 到末行");
        assert_eq!(feed_all("gg"), Some((Motion::FirstLine, 1)), "gg 到首行");
    }
}

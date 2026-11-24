//! Vim 本地模式键位的解析层：按键序列 → 一个动作意图。
//!
//! 依据：`kingtoolbox-docs/Keyboard/Navigation-Keys.md`（官方 page/22）与
//! `Copy-And-Paste-Keys.md`（page/23）。前者 26 个导航键全在这里解析；后者
//! 的 `y{motion}` / yy / y$ / ytx / yfx / p / P 与 `"a` / `"+` 寄存器也在。
//!
//! 这是纯状态机：不认识 `ViewState`、不认识终端视图、不读盘、不碰剪贴板——
//! 它只把字符序列翻译成意图，落地（移视口/选区、取文本、写剪贴板）由投影层
//! 决定。本地模式下任何动作都不写 PTY。

use std::collections::BTreeMap;

/// 一个导航意图。词族区分「空白分隔」与「会话配置的分隔符」两套口径：
/// Vim 的大写键（W/B/E/gE）永远按空白分词，落地层要能传另一套 separators。
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

/// 解析出的完整动作。复制/粘贴带寄存器：`"` 是未命名寄存器，`+` 是系统
/// 剪贴板寄存器；其它字符是用户自定义寄存器。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum VimAction {
    Motion(Motion, usize),
    /// `y{motion}` / `count y{motion}`。
    Yank { motion: Motion, count: usize, register: char },
    /// `yy`：复制当前行（含换行）。
    YankLine { count: usize, register: char },
    /// `ytx`（到 x 之前）/ `yfx`（含 x）。
    YankToChar { before: bool, ch: char, count: usize, register: char },
    /// `p` 粘贴后回远端模式；`P` 粘贴后留在本地模式。
    Paste { text_from: char, remote: bool },
}

/// 未命名寄存器的名字，Vim 里写作 `"`。
pub const UNNAMED_REGISTER: char = '"';
/// 系统剪贴板寄存器，Vim 里写作 `+`。
pub const CLIPBOARD_REGISTER: char = '+';

#[derive(Clone, Debug)]
pub struct VimState {
    count: usize,
    pending_g: bool,
    pending_yank: bool,
    pending_register: bool,
    pending_find_char: Option<bool>,
    register: char,
    registers: BTreeMap<char, String>,
}

impl Default for VimState {
    fn default() -> Self {
        Self {
            count: 0,
            pending_g: false,
            pending_yank: false,
            pending_register: false,
            pending_find_char: None,
            register: UNNAMED_REGISTER,
            registers: BTreeMap::new(),
        }
    }
}

impl VimState {
    pub fn reset(&mut self) {
        self.count = 0;
        self.pending_g = false;
        self.pending_yank = false;
        self.pending_register = false;
        self.pending_find_char = None;
        self.register = UNNAMED_REGISTER;
    }

    /// 全部状态（含寄存器）清空。进入本地模式时调用，避免上一条会话的
    /// 剪贴板内容漏到下一条。
    pub fn clear_all(&mut self) {
        self.reset();
        self.registers.clear();
    }

    pub fn register_text(&self, name: char) -> Option<&str> {
        self.registers.get(&name).map(String::as_str)
    }

    pub fn set_register(&mut self, name: char, text: String) {
        self.registers.insert(name, text);
        // 未命名寄存器总是跟着最近一次 yank 走（Vim 的 `"` 语义）。
        if name != UNNAMED_REGISTER {
            self.registers.insert(UNNAMED_REGISTER, text_of(&self.registers, name));
        }
    }

    /// 喂一个字符。返回 `None` 表示还在等后续字符（数字、`g`、`y`、
    /// 寄存器名、`yt`/`yf` 的目标字符）。非法字符清掉未完序列。
    pub fn feed(&mut self, ch: char) -> Option<VimAction> {
        if let Some(before) = self.pending_find_char {
            self.pending_find_char = None;
            let out = VimAction::YankToChar {
                before,
                ch,
                count: self.count.max(1),
                register: self.register,
            };
            self.reset();
            return Some(out);
        }
        if self.pending_register {
            self.pending_register = false;
            self.register = ch;
            return None;
        }
        // `yy` 是 operator 自己重复一次，不是「y + 某个 motion」。
        if self.pending_yank && ch == 'y' {
            let out = VimAction::YankLine {
                count: self.count.max(1),
                register: self.register,
            };
            self.reset();
            return Some(out);
        }
        if let Some((motion, count)) = self.take_motion(ch) {
            let out = if self.pending_yank {
                self.pending_yank = false;
                VimAction::Yank { motion, count, register: self.register }
            } else {
                VimAction::Motion(motion, count)
            };
            self.reset();
            return Some(out);
        }
        match ch {
            '1'..='9' => {
                self.count = self
                    .count
                    .saturating_mul(10)
                    .saturating_add((ch as u8 - b'0') as usize);
                None
            }
            'g' => {
                self.pending_g = true;
                None
            }
            '"' => {
                self.pending_register = true;
                None
            }
            'y' => {
                self.pending_yank = true;
                None
            }
            't' if self.pending_yank => {
                self.pending_find_char = Some(true);
                self.pending_yank = false;
                None
            }
            'f' if self.pending_yank => {
                self.pending_find_char = Some(false);
                self.pending_yank = false;
                None
            }
            'p' | 'P' => {
                let remote = ch == 'p';
                let reg = self.register;
                self.reset();
                Some(VimAction::Paste { text_from: reg, remote })
            }
            _ => {
                self.reset();
                None
            }
        }
    }

    /// 把「一个字符 = 某个导航 motion」的映射抽出来，`y` 复用它来表达
    /// `y{motion}`。`g` 的两键序列在这里消费，`None` 交给 feed 继续等。
    fn take_motion(&mut self, ch: char) -> Option<(Motion, usize)> {
        let out = if self.pending_g {
            self.pending_g = false;
            match ch {
                'g' => Motion::FirstLine,
                'e' => Motion::WordEndBack,
                'E' => Motion::WordEndBackBlank,
                _ => return None,
            }
        } else {
            match ch {
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
                _ => return None,
            }
        };
        Some((out, self.count.max(1)))
    }
}

/// 把具名寄存器的内容同步到未命名寄存器。`set_register` 已先插入具名项，
/// 这里取出来克隆一份，避免同一帧里两次可变借 `self.registers`。
fn text_of(registers: &BTreeMap<char, String>, name: char) -> String {
    registers.get(&name).cloned().unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 把一个键序喂完，断言最后产出；中间步骤必须都是「还在等」。
    fn feed_all(keys: &str) -> Option<VimAction> {
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

    fn mot(action: Option<VimAction>) -> Option<(Motion, usize)> {
        match action {
            Some(VimAction::Motion(m, n)) => Some((m, n)),
            _ => None,
        }
    }

    fn one(v: &mut VimState, ch: char) -> Option<(Motion, usize)> {
        mot(v.feed(ch))
    }

    #[test]
    fn basic_motions_carry_a_default_count_of_one() {
        let mut v = VimState::default();
        assert_eq!(one(&mut v, 'h'), Some((Motion::Left, 1)));
        assert_eq!(one(&mut v, 'j'), Some((Motion::Down, 1)));
        assert_eq!(one(&mut v, '$'), Some((Motion::LineEnd, 1)));
        assert_eq!(one(&mut v, '^'), Some((Motion::LineStartNonBlank, 1)));
    }

    #[test]
    fn digit_prefix_multiplies_and_a_motion_clears_it() {
        let mut v = VimState::default();
        assert_eq!(one(&mut v, '1'), None);
        assert_eq!(one(&mut v, '2'), None);
        assert_eq!(one(&mut v, 'j'), Some((Motion::Down, 12)));
        assert_eq!(one(&mut v, 'k'), Some((Motion::Up, 1)), "计数用完就清");
    }

    #[test]
    fn gg_is_a_two_key_sequence_and_an_unknown_key_aborts_it() {
        let mut v = VimState::default();
        assert_eq!(one(&mut v, 'g'), None);
        assert_eq!(one(&mut v, 'g'), Some((Motion::FirstLine, 1)));
        assert_eq!(one(&mut v, 'G'), Some((Motion::LastLine, 1)));
        assert_eq!(one(&mut v, 'g'), None);
        assert_eq!(one(&mut v, 'x'), None);
        assert_eq!(one(&mut v, 'g'), None, "非法键要清掉挂起的 g");
        assert_eq!(one(&mut v, 'g'), Some((Motion::FirstLine, 1)));
    }

    #[test]
    fn every_word_key_from_the_doc_has_a_distinct_motion() {
        for (key, want) in [
            ('w', Motion::WordForward),
            ('W', Motion::WordForwardBlank),
            ('b', Motion::WordBack),
            ('B', Motion::WordBackBlank),
            ('e', Motion::WordEnd),
            ('E', Motion::WordEndBlank),
            ('-', Motion::SentencePrev),
            ('(', Motion::SentencePrev),
            ('+', Motion::SentenceNext),
            (')', Motion::SentenceNext),
            ('{', Motion::ParagraphPrev),
            ('}', Motion::ParagraphNext),
        ] {
            assert_eq!(mot(feed_all(&key.to_string())), Some((want, 1)), "键 {key}");
        }
        assert_eq!(mot(feed_all("ge")), Some((Motion::WordEndBack, 1)));
        assert_eq!(mot(feed_all("gE")), Some((Motion::WordEndBackBlank, 1)));
        assert_eq!(mot(feed_all("3ge")), Some((Motion::WordEndBack, 3)));
        assert_eq!(mot(feed_all("12G")), Some((Motion::LastLine, 12)));
    }

    #[test]
    fn yank_motions_and_registers_match_page_23() {
        assert_eq!(
            feed_all("yw"),
            Some(VimAction::Yank {
                motion: Motion::WordForward,
                count: 1,
                register: UNNAMED_REGISTER,
            }),
        );
        assert_eq!(
            feed_all("y$"),
            Some(VimAction::Yank {
                motion: Motion::LineEnd,
                count: 1,
                register: UNNAMED_REGISTER,
            }),
        );
        assert_eq!(
            feed_all("\"ay2w"),
            Some(VimAction::Yank {
                motion: Motion::WordForward,
                count: 2,
                register: 'a',
            }),
        );
        assert_eq!(
            feed_all("\"+ytx"),
            Some(VimAction::YankToChar {
                before: true,
                ch: 'x',
                count: 1,
                register: CLIPBOARD_REGISTER,
            }),
        );
        assert_eq!(
            feed_all("yfx"),
            Some(VimAction::YankToChar {
                before: false,
                ch: 'x',
                count: 1,
                register: UNNAMED_REGISTER,
            }),
        );
    }

    #[test]
    fn yy_and_paste_switch_modes_as_documented() {
        assert_eq!(
            feed_all("2yy"),
            Some(VimAction::YankLine { count: 2, register: UNNAMED_REGISTER }),
        );
        assert_eq!(
            feed_all("p"),
            Some(VimAction::Paste { text_from: UNNAMED_REGISTER, remote: true }),
        );
        assert_eq!(
            feed_all("\"aP"),
            Some(VimAction::Paste { text_from: 'a', remote: false }),
        );
    }

    #[test]
    fn yanked_named_register_updates_the_unnamed_one() {
        let mut v = VimState::default();
        v.set_register('a', "hello".to_string());
        assert_eq!(v.register_text('a'), Some("hello"));
        assert_eq!(v.register_text(UNNAMED_REGISTER), Some("hello"));
    }
}

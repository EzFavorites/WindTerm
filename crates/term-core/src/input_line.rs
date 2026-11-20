//! 输入行模型（W5-I）：把原始按键字节还原成「回车之前那一行」，供 `CommandHistory` 记录。
//!
//! # 边界（ADR-0014 / I3 / B1）
//!
//! 纯语义单元：零 UI、零 IO、零线程、零墙钟。只消费字节、只吐出行。
//!
//! # 为什么不是「读屏幕上的行」
//!
//! 回看流是**输出**，含 shell 提示符、回显、颜色序列；把输出当输入历史等于把
//! `$ ` 也存进历史。这里走**输入侧**：逐字节重建用户敲进去的那一行。
//!
//! # 覆盖范围与已知近似
//!
//! - 可打印字节（含 UTF-8 多字节）追加到行；
//! - 回车（`\r` / `\n`）结束一行并吐出（可一次吐多行，覆盖多行粘贴）；
//! - `Backspace`（`\x7f` / `\x08`）按字符删除（UTF-8 边界安全）；
//! - `Ctrl-C`（`\x03`）/ `Ctrl-U`（`\x15`）清空当前行；
//! - 其余控制字节忽略；
//! - ESC 序列（方向键 / 功能键等）整段吞掉，不污染行内容。
//!
//! **近似**：shell 自己的行编辑（readline 的 Ctrl-A/Ctrl-K、历史 ↑ 取回旧行等）
//! 不在输入字节里给出「最终行」，本模型只能重建「敲了什么」。这是输入行模型的
//! 固有边界，不是缺陷；对照 `WindTerm-设计规格/I-auto-completion.md` 的 AC 由该层裁定。

/// 输入行缓冲。每 tab 一个。
#[derive(Clone, Debug, Default)]
pub struct InputLine {
    /// 当前行已累计的字节（未按字符切分，回车时按 UTF-8 lossy 解码）。
    buf: Vec<u8>,
    /// ESC 序列状态：0 正常 / 1 刚见 ESC / 2 CSI(`ESC [`) 参数中 / 3 SS3(`ESC O`) 后一字节。
    esc: u8,
}

impl InputLine {
    /// 空行。
    pub fn new() -> Self {
        Self {
            buf: Vec::new(),
            esc: 0,
        }
    }

    /// 当前行（未回车）的可见文本。用于调试 / 断言，不参与历史记录。
    pub fn current(&self) -> String {
        String::from_utf8_lossy(&self.buf).into_owned()
    }

    /// 是否为空行。
    pub fn is_empty(&self) -> bool {
        self.buf.is_empty()
    }

    /// 清空当前行（如外部主动丢弃）。
    pub fn clear(&mut self) {
        self.buf.clear();
        self.esc = 0;
    }

    /// 喂入一段输入字节，返回本次因回车而**完成**的行（可能 0 条或多条）。
    ///
    /// 空行（仅空白）不产出。行内容按 `trim` 归一（去掉首尾空白与回车残留）。
    pub fn feed(&mut self, bytes: &[u8]) -> Vec<String> {
        let mut out = Vec::new();
        for &b in bytes {
            match self.esc {
                // 刚见到 ESC：判断是 CSI(`[`)、SS3(`O`) 还是单字节序列。
                1 => {
                    self.esc = match b {
                        b'[' => 2,
                        b'O' => 3,
                        _ => 0,
                    };
                    continue;
                }
                // CSI：吞到 final byte（0x40..=0x7e）。
                2 => {
                    if (0x40..=0x7e).contains(&b) {
                        self.esc = 0;
                    }
                    continue;
                }
                // SS3：再吞一个字节即可。
                3 => {
                    self.esc = 0;
                    continue;
                }
                _ => {}
            }
            match b {
                0x1b => self.esc = 1,
                b'\r' | b'\n' => {
                    if let Some(line) = self.take_line() {
                        out.push(line);
                    }
                }
                0x7f | 0x08 => pop_char(&mut self.buf),
                0x03 | 0x15 => self.buf.clear(),
                b if b < 0x20 => { /* 其余控制字节忽略 */ }
                _ => self.buf.push(b),
            }
        }
        out
    }

    /// 收走当前行并清空缓冲。空白行返回 `None`。
    fn take_line(&mut self) -> Option<String> {
        let line = String::from_utf8_lossy(&self.buf).trim().to_string();
        self.buf.clear();
        if line.is_empty() {
            None
        } else {
            Some(line)
        }
    }
}

/// 从尾部删掉一个 UTF-8 字符（跳过 10xxxxxx 续字节，退到首字节）。
fn pop_char(buf: &mut Vec<u8>) {
    if buf.is_empty() {
        return;
    }
    let mut i = buf.len() - 1;
    while i > 0 && (buf[i] & 0xC0) == 0x80 {
        i -= 1;
    }
    buf.truncate(i);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn simple_line_on_enter() {
        let mut l = InputLine::new();
        let lines = l.feed(b"ls\n");
        assert_eq!(lines, vec!["ls"]);
        assert!(l.current().is_empty(), "回车后当前行应清空");
    }

    #[test]
    fn accumulates_across_feeds() {
        let mut l = InputLine::new();
        assert!(l.feed(b"ec").is_empty());
        assert_eq!(l.current(), "ec");
        let lines = l.feed(b"ho hi\n");
        assert_eq!(lines, vec!["echo hi"]);
        assert!(l.current().is_empty());
    }

    #[test]
    fn multiline_paste_emits_all_lines() {
        let mut l = InputLine::new();
        let lines = l.feed(b"a\nb\r\nc\n");
        assert_eq!(lines, vec!["a", "b", "c"]);
    }

    #[test]
    fn backspace_deletes_one_char() {
        let mut l = InputLine::new();
        l.feed(b"lsx");
        l.feed(&[0x7f]);
        assert_eq!(l.current(), "ls");
        l.feed(&[0x08]);
        assert_eq!(l.current(), "l");
    }

    #[test]
    fn backspace_is_utf8_boundary_safe() {
        let mut l = InputLine::new();
        l.feed("你".as_bytes());
        assert_eq!(l.current(), "你");
        l.feed(&[0x7f]);
        assert_eq!(l.current(), "", "删一个汉字应整字删除，不留半个 UTF-8");
    }

    #[test]
    fn ctrl_c_and_ctrl_u_clear_the_line() {
        let mut l = InputLine::new();
        l.feed(b"abc");
        l.feed(&[0x03]); // Ctrl-C
        assert_eq!(l.current(), "");
        l.feed(b"xyz");
        l.feed(&[0x15]); // Ctrl-U
        assert_eq!(l.current(), "");
    }

    #[test]
    fn arrow_key_sequence_does_not_pollute_the_line() {
        let mut l = InputLine::new();
        l.feed(b"ls");
        l.feed(b"\x1b[A"); // ArrowUp
        l.feed(b"\x1b[B"); // ArrowDown
        assert_eq!(l.current(), "ls", "CSI 方向键应整段吞掉");
        let lines = l.feed(b"\n");
        assert_eq!(lines, vec!["ls"]);
    }

    #[test]
    fn ss3_sequence_does_not_pollute_the_line() {
        let mut l = InputLine::new();
        l.feed(b"ls");
        l.feed(b"\x1bOA"); // SS3 ArrowUp
        assert_eq!(l.current(), "ls", "SS3 序列应整段吞掉");
    }

    #[test]
    fn empty_enter_emits_nothing() {
        let mut l = InputLine::new();
        assert!(l.feed(b"\n").is_empty());
        assert!(l.feed(b"\r\n").is_empty());
        assert!(l.feed(b"   \n").is_empty(), "纯空白行不算一条命令");
    }

    #[test]
    fn surrounding_whitespace_trimmed() {
        let mut l = InputLine::new();
        let lines = l.feed(b"  git status  \n");
        assert_eq!(lines, vec!["git status"]);
    }

    #[test]
    fn utf8_command_recorded_intact() {
        let mut l = InputLine::new();
        let lines = l.feed("echo 你好\n".as_bytes());
        assert_eq!(lines, vec!["echo 你好"]);
    }

    #[test]
    fn esc_state_persists_across_feeds() {
        // 序列被拆到两次 feed（真实按键分批到达的常见情形）。
        let mut l = InputLine::new();
        l.feed(b"ls");
        l.feed(&[0x1b]);
        l.feed(b"[A");
        assert_eq!(l.current(), "ls");
        let lines = l.feed(b"\n");
        assert_eq!(lines, vec!["ls"]);
    }

    #[test]
    fn tab_and_other_controls_ignored() {
        let mut l = InputLine::new();
        l.feed(b"cd\x09/doc"); // Tab 不进入命令行文本
        assert_eq!(l.current(), "cd/doc");
    }

    #[test]
    fn clear_resets() {
        let mut l = InputLine::new();
        l.feed(b"abc");
        l.clear();
        assert!(l.is_empty());
        assert_eq!(l.current(), "");
    }
}

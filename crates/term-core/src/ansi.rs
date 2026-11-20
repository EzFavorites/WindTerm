//! 会话日志的文本清洗：把 PTY 原始字节变成「能 grep 的文本」。
//!
//! **为什么需要它**：日志是事后翻看 / 检索的，不是回放录像。原始字节里塞满颜色、
//! 光标定位、窗口标题（`ESC ] 0 ; … BEL`），直接落盘的话 `grep` 出来的每一行都
//! 带着 `^[[38;5;12m`，还会因为 `\r` 把整行覆盖成乱码。
//!
//! **为什么复用 `vte` 而不是自己写状态机**：主终端（`vt.rs`）用同一个解析器吃同一份
//! 字节流，两处语义必须一致——自己写一遍「哪些序列要吃掉」等于给日志一份独立的、
//! 迟早分叉的协议解释。顺带白拿两个好处：转义序列跨 chunk 的状态由 `Processor`
//! 自己保留，UTF-8 的半截字符也由它拼完整再交给我们（中文输出被 2048 字节的读边界
//! 切开是常态，自己按字节拼 `String` 会得到一片 `�`）。
//!
//! **保留什么**：可打印字符、`\t`、换行。`\r` 一律当换行（`\r\n` 只算一个）——
//! 进度条那种「回行首重画」在纯文本里最自然的形态就是多行，丢掉 `\r` 反而会让
//! 同一行被反复追加成一行超长文本。退格、响铃等其余控制字符直接丢弃：它们在
//! 纯文本里没有对应物。
//!
//! 本模块是纯语义：不碰文件、不碰时钟、不碰线程（I3）。

use vte::ansi::{Handler, Processor};

/// 把终端输出「洗」成纯文本，跨 chunk 保留解析状态。
pub struct TextCleaner {
    parser: Processor,
    out: String,
    /// 见过 `\r` 但还没看到后面是什么。与 `\n` 相邻时两次只产出一个换行。
    pending_cr: bool,
}

impl TextCleaner {
    pub fn new() -> Self {
        Self {
            parser: Processor::new(),
            out: String::new(),
            pending_cr: false,
        }
    }

    /// 喂一段字节，返回**这次新产生**的文本（可能是空串）。
    ///
    /// 返回增量而不是累积量：调用方要的是「往文件里追加什么」。
    pub fn feed(&mut self, bytes: &[u8]) -> String {
        if bytes.is_empty() {
            return String::new();
        }
        // `advance` 需要 `&mut` 解析器，而回调又写 `out`——两者都在 `self` 上，
        // 所以先把解析器换出来（与 `vt::Terminal::process` 同一手法，换回去时
        // 解析状态不会丢）。
        let mut parser = std::mem::replace(&mut self.parser, Processor::new());
        parser.advance(self, bytes);
        self.parser = parser;
        std::mem::take(&mut self.out)
    }

    fn newline(&mut self) {
        self.pending_cr = false;
        self.out.push('\n');
    }

    fn flush_cr(&mut self) {
        if self.pending_cr {
            self.newline();
        }
    }
}

impl Default for TextCleaner {
    fn default() -> Self {
        Self::new()
    }
}

impl Handler for TextCleaner {
    fn input(&mut self, c: char) {
        // 单独的 `\r` 是「回到行首重画」：在日志里记成换行（上一次画的那些字
        // 已经落盘了，覆盖掉它们等于篡改历史）。
        self.flush_cr();
        self.out.push(c);
    }

    fn linefeed(&mut self) {
        self.newline();
    }

    fn carriage_return(&mut self) {
        // 先记下：若紧接着是 LF，两者合起来只算一个换行。
        self.pending_cr = true;
    }

    fn put_tab(&mut self, count: u16) {
        self.flush_cr();
        for _ in 0..count {
            self.out.push('\t');
        }
    }

    // 其余回调（光标移动、SGR 颜色、标题、剪贴板、私有模式……）保持默认的空实现：
    // 日志不需要它们，而**吃掉**它们正是本模块的职责。
}

#[cfg(test)]
mod tests {
    use super::*;

    fn clean(bytes: &[u8]) -> String {
        TextCleaner::new().feed(bytes)
    }

    #[test]
    fn plain_text_passes_through_unchanged() {
        assert_eq!(clean(b"hello world\n"), "hello world\n");
    }

    #[test]
    fn colour_and_cursor_sequences_are_dropped() {
        // 真实 shell 的输出形状：提示符带色、清屏、光标归位。
        let raw = b"\x1b[1;32muser@host\x1b[0m:\x1b[38;5;12m~/src\x1b[0m$ ls\r\n";
        assert_eq!(clean(raw), "user@host:~/src$ ls\n");
    }

    #[test]
    fn window_title_osc_is_dropped() {
        // OSC 以 BEL 结尾是最常见的形式；ST（ESC \）也要吃掉。
        assert_eq!(clean(b"\x1b]0;~/src\x07after"), "after");
        assert_eq!(clean(b"\x1b]0;title\x1b\\after"), "after");
    }

    #[test]
    fn crlf_becomes_one_newline() {
        assert_eq!(clean(b"a\r\nb\r\n"), "a\nb\n");
    }

    #[test]
    fn bare_carriage_return_starts_a_new_line() {
        // 进度条：`\r` 回行首重画，日志里记成多行而不是互相覆盖。
        assert_eq!(clean(b"10%\r50%\r100%\n"), "10%\n50%\n100%\n");
    }

    #[test]
    fn trailing_cr_is_held_until_the_next_byte() {
        // `\r` 落在 chunk 末尾时不能立刻吐换行——下一块可能以 `\n` 开头。
        let mut c = TextCleaner::new();
        assert_eq!(c.feed(b"done\r"), "done");
        assert_eq!(c.feed(b"\n"), "\n");
        // 反过来：下一块不是 LF，那前面那个 `\r` 就是一个换行。
        let mut c = TextCleaner::new();
        assert_eq!(c.feed(b"a\r"), "a");
        assert_eq!(c.feed(b"b"), "\nb");
    }

    #[test]
    fn sequence_split_across_chunks_is_still_dropped() {
        let mut c = TextCleaner::new();
        assert_eq!(c.feed(b"before\x1b["), "before");
        assert_eq!(c.feed(b"31m"), "");
        assert_eq!(c.feed(b"after"), "after");
    }

    #[test]
    fn utf8_split_across_chunks_is_reassembled() {
        let bytes = "中文".as_bytes();
        let mut c = TextCleaner::new();
        // 故意切在第一个字符的中间：日志里不该出现替换字符。
        assert_eq!(c.feed(&bytes[..2]), "");
        assert_eq!(c.feed(&bytes[2..]), "中文");
    }

    #[test]
    fn backspace_and_bell_are_dropped() {
        assert_eq!(clean(b"abc\x08\x07d"), "abcd");
    }

    #[test]
    fn tab_is_kept() {
        assert_eq!(clean(b"a\tb"), "a\tb");
    }

    #[test]
    fn full_reset_sequence_is_dropped() {
        assert_eq!(clean(b"\x1bcAFTER"), "AFTER");
    }

    #[test]
    fn alternate_screen_keeps_the_written_text() {
        // 全屏程序（less / vim）会切备用屏：日志照样记它写了什么，
        // 只是不记「切回主屏」这件事——这正是纯文本日志的边界。
        let raw = b"\x1b[?1049h\x1b[2Jline one\r\nline two\x1b[?1049l";
        assert_eq!(clean(raw), "line one\nline two");
    }
}

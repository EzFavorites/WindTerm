//! OSC 133（FinalTerm shell integration）的**纯语义层**。
//!
//! 前置，也是 Select Command / 命令块折叠 / 跨标签搜索共用
//! 的那一层。`term-core` 零副作用（verify 4b）：这里只把字节流翻成标记，
//! 不碰 PTY、不碰时间、不碰文件。
//!
//! 为什么单独一个模块：`vt.rs` 是 `vte::ansi::Handler`，职责是维护网格；
//! 命令块边界是**另一种**语义（哪个字节区间属于哪条命令），混进去会让
//! 网格渲染和块识别共用一个状态机，改一个就影响另一个。
//!
//! 序列形状：`ESC ] 133 ; <Mark> [; <args>] (BEL | ESC \)`。
//! - `A` prompt start，`B` prompt end，`C` command start，`D` command end
//!   （`D;exit` 带退出码）。`P` 是旧版属性标记，这里只识别不解释。

/// 一个 OSC 133 标记及其在字节流里的**起始偏移**。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mark {
    PromptStart,
    PromptEnd,
    CommandStart,
    /// `D;N` 的退出码；没带参数就是 `None`（不能猜成 0）。
    CommandEnd(Option<i32>),
    /// `P;Key=Value` 属性，保留键与值原样。
    Property,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MarkEvent {
    pub offset: usize,
    pub mark: Mark,
}

/// 一条命令的块边界。坐标类型 `T` 由调用方决定：`term-app/blocks.rs` 用
/// **字节偏移**（`usize`）做文本区间，事实窗口的选区用**绝对行号**（`i64`）。
///
/// `prompt_end`（OSC 133 的 `B`）是「用户开始打命令」的位置——`A..B` 是提示符，
/// `B..C` 才是命令本身。缺了它，`Select Command` 只能把提示符一起选进来。
/// `end` 为 `None` 表示命令还在跑。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CommandBlock<T = usize> {
    pub prompt_start: T,
    pub prompt_end: Option<T>,
    pub command_start: T,
    pub end: Option<T>,
    pub exit: Option<i32>,
}

/// 一段字节流被切开后的片段：普通输出（原样喂给 `vte`）或一条标记。
///
/// `vt.rs` 需要「在标记处记下行号」，所以它不能只拿标记列表——必须知道
/// 标记插在哪两个字节片段之间。切分规则只有这里一份，`parse_marks` 也用它。
pub enum Segment<'a> {
    Bytes(&'a [u8]),
    Mark { at: usize, mark: Mark },
}

/// 按 OSC 133 标记切分字节流。畸形 / 未终止的序列当作普通输出，不 panic。
pub fn segments(bytes: &[u8]) -> Vec<Segment<'_>> {
    let mut out = Vec::new();
    let mut start = 0usize;
    let mut i = 0usize;
    while i + 4 < bytes.len() {
        if bytes[i] != 0x1b || bytes[i + 1] != b']' {
            i += 1;
            continue;
        }
        // 找终止符：BEL(0x07) 或 ST(ESC \)。
        let mut j = i + 2;
        let mut term = None;
        while j < bytes.len() {
            if bytes[j] == 0x07 {
                term = Some((j, 1));
                break;
            }
            if bytes[j] == 0x1b && j + 1 < bytes.len() && bytes[j + 1] == b'\\' {
                term = Some((j, 2));
                break;
            }
            j += 1;
        }
        let Some((end, term_len)) = term else { break };
        let body = &bytes[i + 2..end];
        match parse_body(body) {
            Some(mark) => {
                if start < i {
                    out.push(Segment::Bytes(&bytes[start..i]));
                }
                out.push(Segment::Mark { at: i, mark });
                i = end + term_len;
                start = i;
            }
            // 不是 133 的 OSC（标题、配色）：整段留给 `vte` 自己处理。
            None => i = end + term_len,
        }
    }
    if start < bytes.len() {
        out.push(Segment::Bytes(&bytes[start..]));
    }
    out
}

/// 从一段字节流里扫出全部 OSC 133 标记。畸形序列直接跳过，不 panic。
pub fn parse_marks(bytes: &[u8]) -> Vec<MarkEvent> {
    segments(bytes)
        .into_iter()
        .filter_map(|s| match s {
            Segment::Mark { at, mark } => Some(MarkEvent { offset: at, mark }),
            Segment::Bytes(_) => None,
        })
        .collect()
}

fn parse_body(body: &[u8]) -> Option<Mark> {
    let s = std::str::from_utf8(body).ok()?;
    let mut parts = s.split(';');
    if parts.next()?.trim() != "133" {
        return None;
    }
    let code = parts.next()?.trim();
    let arg = parts.next().map(str::trim);
    match code {
        "A" => Some(Mark::PromptStart),
        "B" => Some(Mark::PromptEnd),
        "C" => Some(Mark::CommandStart),
        "D" => Some(Mark::CommandEnd(arg.and_then(|a| a.parse().ok()))),
        "P" => Some(Mark::Property),
        _ => None,
    }
}

/// 把标记流聚合成命令块。`A` 起一块，`C` 记命令起点，`D` 收尾。
/// 没有 `A` 的孤立 `C`/`D` 不成块（shell 没集成时不能凭空造边界）。
pub fn blocks(marks: &[MarkEvent]) -> Vec<CommandBlock> {
    blocks_at(marks.iter().map(|e| (e.offset, e.mark)))
}

/// 任意坐标下的同一台状态机（字节偏移或行号都走这里，规则只有一份）。
pub fn blocks_at<T: Copy>(events: impl IntoIterator<Item = (T, Mark)>) -> Vec<CommandBlock<T>> {
    let mut out: Vec<CommandBlock<T>> = Vec::new();
    let mut open: Option<CommandBlock<T>> = None;
    for (at, mark) in events {
        match mark {
            Mark::PromptStart => {
                if open.is_none() {
                    open = Some(CommandBlock {
                        prompt_start: at,
                        prompt_end: None,
                        command_start: at,
                        end: None,
                        exit: None,
                    });
                }
            }
            Mark::PromptEnd => {
                if let Some(b) = open.as_mut() {
                    b.prompt_end = Some(at);
                }
            }
            Mark::CommandStart => {
                if let Some(b) = open.as_mut() {
                    b.command_start = at;
                }
            }
            Mark::CommandEnd(exit) => {
                if let Some(mut b) = open.take() {
                    b.end = Some(at);
                    b.exit = exit;
                    out.push(b);
                }
            }
            Mark::Property => {}
        }
    }
    if let Some(b) = open {
        out.push(b);
    }
    out
}

/// 生成一条 shell 集成标记（给 Golden / 测试夹具用，产品不调用）。
pub fn emit(code: char, arg: Option<&str>) -> Vec<u8> {
    let mut v = Vec::new();
    v.extend_from_slice(b"\x1b]133;");
    v.push(code as u8);
    if let Some(a) = arg {
        v.push(b';');
        v.extend_from_slice(a.as_bytes());
    }
    v.push(0x07);
    v
}

#[cfg(test)]
mod tests {
    use super::*;

    fn stream() -> Vec<u8> {
        let mut v = Vec::new();
        v.extend_from_slice(b"prompt$ ");
        v.extend(emit('A', None));
        v.extend_from_slice(b"$ ");
        v.extend(emit('B', None));
        v.extend_from_slice(b"ls -la\r\n");
        v.extend(emit('C', None));
        v.extend_from_slice(b"total 4\r\n");
        v.extend(emit('D', Some("0")));
        v
    }

    #[test]
    fn a_full_command_is_one_block_with_exit_code() {
        let bytes = stream();
        let marks = parse_marks(&bytes);
        assert_eq!(marks.len(), 4, "{marks:?}");
        assert_eq!(marks[0].mark, Mark::PromptStart);
        assert_eq!(marks[3].mark, Mark::CommandEnd(Some(0)));
        let blocks = blocks(&marks);
        assert_eq!(blocks.len(), 1);
        assert_eq!(blocks[0].exit, Some(0));
        // `B` 落在 `A` 与 `C` 之间：提示符与命令本身分得开。
        let b = blocks[0].prompt_end.expect("B 应被记下");
        assert!(b >= blocks[0].prompt_start && b <= blocks[0].command_start);
        assert!(blocks[0].command_start > blocks[0].prompt_start);
        assert!(blocks[0].end.unwrap() > blocks[0].command_start);
    }

    #[test]
    fn st_terminator_and_missing_exit_are_both_handled() {
        let mut v = emit('A', None);
        v.extend(emit('C', None));
        v.extend_from_slice(b"\x1b]133;D\x1b\\");
        let marks = parse_marks(&v);
        assert_eq!(marks.len(), 3);
        assert_eq!(marks[2].mark, Mark::CommandEnd(None));
        let b = blocks(&marks);
        assert_eq!(b.len(), 1);
        assert_eq!(b[0].exit, None);
        assert!(b[0].end.is_some());
    }

    #[test]
    fn a_running_command_yields_an_open_block() {
        let marks = parse_marks(&emit('A', None));
        let b = blocks(&marks);
        assert_eq!(b.len(), 1);
        assert_eq!(b[0].end, None);
    }

    #[test]
    fn orphan_marks_do_not_invent_block_boundaries() {
        let mut v = emit('C', None);
        v.extend(emit('D', Some("1")));
        assert!(blocks(&parse_marks(&v)).is_empty());
    }

    #[test]
    fn non_133_osc_and_broken_sequences_are_ignored() {
        let mut v = Vec::new();
        v.extend_from_slice(b"\x1b]0;title\x07");
        v.extend_from_slice(b"\x1b]133;Z\x07");
        v.extend_from_slice(b"\x1b]133;A"); // 没有终止符
        assert!(parse_marks(&v).is_empty());
    }

    #[test]
    fn property_marks_are_present_but_do_not_split_blocks() {
        let mut v = emit('A', None);
        v.extend(emit('P', Some("Cwd=/tmp")));
        v.extend(emit('C', None));
        v.extend(emit('D', Some("2")));
        let marks = parse_marks(&v);
        assert!(marks.iter().any(|m| m.mark == Mark::Property));
        assert_eq!(blocks(&marks).len(), 1);
    }

    #[test]
    fn segments_keep_plain_output_and_marks_in_order() {
        let mut v = Vec::new();
        v.extend_from_slice(b"a");
        v.extend(emit('A', None));
        v.extend_from_slice(b"b");
        assert_eq!(segments(&v).len(), 3, "文字 / 标记 / 文字");
    }

    #[test]
    fn a_non_133_osc_stays_inside_the_text_segment() {
        let mut v = Vec::new();
        v.extend_from_slice(b"\x1b]0;title\x07");
        v.extend(emit('A', None));
        let segs = segments(&v);
        assert_eq!(segs.len(), 2);
        match segs[0] {
            Segment::Bytes(b) => assert_eq!(b, b"\x1b]0;title\x07"),
            _ => panic!("标题不该被当成 133 标记"),
        }
    }

    #[test]
    fn blocks_at_works_on_row_numbers_too() {
        let marks = vec![
            (10i64, Mark::PromptStart),
            (10, Mark::PromptEnd),
            (11, Mark::CommandStart),
            (14, Mark::CommandEnd(Some(0))),
        ];
        let b = blocks_at(marks);
        assert_eq!(b.len(), 1);
        assert_eq!(b[0].prompt_end, Some(10));
        assert_eq!(b[0].command_start, 11);
        assert_eq!(b[0].end, Some(14));
    }
}

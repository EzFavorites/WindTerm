//! 会话字符集：把远端原始字节变成 VT 能吃的 UTF-8。
//!
//! `SessionConfig.charset` 是会话属性，解码发生在**宿主监管线程**（本 crate），
//! `term-core` 只拿到已经变成 UTF-8 的字节——纯语义层不认识编码转换。
//!
//! # 两条路径
//!
//! - **直传**：UTF-8（含别名）、未知 / 空标签、以及一切非 ASCII 兼容编码
//!   （UTF-16 / UTF-32 / ISO-2022-JP）。返回的 [`Cow::Borrowed`] 与现有行为
//!   逐字节相同，默认路径零拷贝、零回归。
//! - **增量解码**：其余 ASCII 兼容编码用 `encoding_rs` 的流式 `Decoder`。
//!   `last = false` 让不完整的多字节序列留在 decoder 里，下一批字节到达时
//!   接着解。encoding_rs 的 `Decoder` 没有公开的 pending 查询（公开的
//!   `has_pending_state()` 只在 `Encoder` 上），所以这里另记一个**保守**标记：
//!   多字节编码的批次以高位字节收尾时，认为 decoder 可能压着半个序列；
//!   宁可多解一次，也不能在真有 pending 时走 ASCII 直传而丢字节。
//!
//! ASCII 兼容编码的 `0x00..=0x7F` 是不变量：ESC、CSI、OSC 等控制字节原样
//! 穿过，不会被转码拆坏。纯 ASCII 的一批字节（且没有可疑 pending）直接借用
//! 原缓冲，不进 decoder——这是绝大多数 shell 输出走的路。
//!
//! # 为什么 UTF-16 明确不支持
//!
//! VT 是 vte 0.15，按 UTF-8 解释可打印字节；UTF-16 的 NUL 字节和 BOM 会
//! 被当成控制字符 / 乱码落到网格上。UTF-16 不在会话表单候选里（见
//! `session_form_body::CHARSETS`），这里也只做直传兜底，不假装支持。

use std::borrow::Cow;

use encoding_rs::{CoderResult, Decoder, Encoding, UTF_8};

/// 一路会话输出的有状态解码器。每会话一个，活在监管线程里。
pub struct CharsetDecoder {
    state: State,
}

enum State {
    /// 不做转换，原始字节直接进 VT / cleaner。
    Passthrough,
    /// ASCII 兼容编码的增量解码器（跨 chunk 持有未完结的多字节序列）。
    Incremental {
        decoder: Decoder,
        /// 单字节编码（ISO-8859-* / KOI8-R）永远没有半个多字节。
        multibyte: bool,
        /// 保守的「可能压着未完成序列」标记，见模块文档。
        pending: bool,
    },
}

impl CharsetDecoder {
    /// 按会话配置里的 charset 标签造解码器。
    ///
    /// 标签解析走 WHATWG 规则（大小写、前后空白不敏感，别名 `gb2312`
    /// 归到 GBK，`iso-8859-1` 归到 windows-1252）。未知标签一律回退直传，
    /// 绝不 panic。非 ASCII 兼容编码（UTF-16 / UTF-32 / ISO-2022-JP）也
    /// 直传：VT 管道处理不了它们。
    pub fn new(label: &str) -> Self {
        let enc = match Encoding::for_label_no_replacement(label.as_bytes()) {
            Some(enc) if enc != UTF_8 && enc.is_ascii_compatible() => enc,
            _ => return Self { state: State::Passthrough },
        };
        Self {
            state: State::Incremental {
                decoder: enc.new_decoder(),
                multibyte: !enc.is_single_byte(),
                pending: false,
            },
        }
    }

    /// 解码一批输出。返回值生命周期跟随输入：直传路径借用，不分配。
    pub fn feed<'a>(&mut self, bytes: &'a [u8]) -> Cow<'a, [u8]> {
        let (decoder, multibyte, pending) = match &mut self.state {
            State::Passthrough => return Cow::Borrowed(bytes),
            State::Incremental {
                decoder,
                multibyte,
                pending,
            } => (decoder, *multibyte, pending),
        };
        // ASCII 不变量：ASCII 兼容编码的 0x00..=0x7F 逐字节不变，而且不改变
        // decoder 状态。只有「上一批可能压着半个多字节」时才必须走 decoder。
        if !*pending && bytes.iter().all(|b| *b < 0x80) {
            return Cow::Borrowed(bytes);
        }
        let mut out: Vec<u8> = Vec::new();
        let mut rest = bytes;
        loop {
            let cap = decoder
                .max_utf8_buffer_length(rest.len())
                .unwrap_or_else(|| rest.len().saturating_mul(3).saturating_add(16));
            let start = out.len();
            out.resize(start + cap, 0);
            let (result, read, written, _) = decoder.decode_to_utf8(rest, &mut out[start..], false);
            out.truncate(start + written);
            // `read` 由 encoding_rs 保证不超过 `rest.len()`；这里仍用 `get`
            // 兜住异常，避免生产路径出现切片 panic（4p 基线只许减不许增）。
            rest = match rest.get(read..) {
                Some(tail) => tail,
                None => &[],
            };
            if matches!(result, CoderResult::InputEmpty) || rest.is_empty() {
                break;
            }
            // 缓冲不够时继续喂剩余输入；两个计数都不动说明上游没进展，退出
            // 而不是在监管线程里死循环。
            if read == 0 && written == 0 {
                break;
            }
        }
        if !bytes.is_empty() {
            // 多字节编码以高位字节收尾 => 可能还有半个序列没走完。保守即可，
            // 多解一遍 ASCII 不会改变字节，只会多一次分配。
            *pending = multibyte && bytes[bytes.len() - 1] >= 0x80;
        }
        Cow::Owned(out)
    }

    /// 上一批字节之后，是否可能还有跨 chunk 的未完成多字节序列。
    pub fn has_pending(&self) -> bool {
        match &self.state {
            State::Passthrough => false,
            State::Incremental { pending, .. } => *pending,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 用 encoding_rs 自己编码，避免测试里手写字节表抄错。
    fn gbk(text: &str) -> Vec<u8> {
        encoding_rs::GBK.encode(text).0.into_owned()
    }

    #[test]
    fn gbk_zhongwen_bytes_decode_to_the_right_hanzi() {
        let raw = gbk("中文");
        let mut d = CharsetDecoder::new("GBK");
        let decoded = d.feed(&raw);
        println!(
            "GBK raw = {:02X?} -> UTF-8 = {:02X?} ({})",
            raw,
            &*decoded,
            std::str::from_utf8(&decoded).unwrap_or("?")
        );
        assert_eq!(&*decoded, "中文".as_bytes());
    }

    #[test]
    fn a_gbk_hanzi_split_across_chunks_is_restored() {
        let raw = gbk("中文");
        let mut d = CharsetDecoder::new("GBK");
        // 逐字节到达：每个汉字的 lead byte 都会压在 decoder 里等 trail byte。
        let mut all = Vec::new();
        for b in &raw {
            let one = [*b];
            all.extend_from_slice(&d.feed(&one));
        }
        assert_eq!(all, "中文".as_bytes());
        // 再走一遍更贴近真实的「第一批只有 lead byte」路径，显式验 pending。
        let mut d = CharsetDecoder::new("GBK");
        let lead = [raw[0]];
        assert!(d.feed(&lead).is_empty(), "孤立 lead byte 不该产出字符");
        assert!(d.has_pending(), "lead byte 必须跨 chunk 留着");
        let out = d.feed(&raw[1..]);
        assert_eq!(&*out, "中文".as_bytes());
        // 后续纯 ASCII Chunk 仍要原样穿过，pending 随之清掉。
        assert_eq!(&*d.feed(b"\r\n"), b"\r\n");
        assert!(!d.has_pending());
    }

    #[test]
    fn utf8_default_path_is_byte_identical_even_for_invalid_sequences() {
        // 含一个截断的多字节序列和孤立 0xFF：直传路径不做任何修复 / 替换。
        let bytes = [b'a', 0xE4, 0xB8, b'\xFF', b'z'];
        let mut d = CharsetDecoder::new("UTF-8");
        let out = d.feed(&bytes);
        assert!(matches!(out, Cow::Borrowed(_)), "UTF-8 必须零拷贝直传");
        assert_eq!(&*out, &bytes);
        assert!(!d.has_pending());
    }

    #[test]
    fn escape_sequences_survive_gbk_decoding_verbatim() {
        let raw = gbk("中文");
        let mut bytes = b"\x1b[31m".to_vec();
        bytes.extend_from_slice(&raw);
        bytes.extend_from_slice(b"\x1b[0m\r\n");
        let mut d = CharsetDecoder::new("GBK");
        let out = d.feed(&bytes);
        let want = [
            b"\x1b[31m".as_slice(),
            "中文".as_bytes(),
            b"\x1b[0m\r\n",
        ]
        .concat();
        assert_eq!(&*out, want.as_slice());
    }

    #[test]
    fn ascii_only_chunks_are_borrowed_even_in_gbk_mode() {
        let mut d = CharsetDecoder::new("GBK");
        let out = d.feed(b"Password: \x1b[0m");
        assert!(matches!(out, Cow::Borrowed(_)), "ASCII 不该触发分配");
        assert_eq!(&*out, b"Password: \x1b[0m");
    }

    #[test]
    fn unknown_and_empty_labels_fall_back_to_utf8_passthrough() {
        for label in ["", "not-a-charset", "UTF-16", "UTF-16LE", "ISO-2022-JP"] {
            let mut d = CharsetDecoder::new(label);
            let bytes = b"abc\xFF";
            let out = d.feed(bytes);
            assert!(matches!(out, Cow::Borrowed(_)), "标签 {label} 意外转码");
            assert_eq!(&*out, bytes);
            assert!(!d.has_pending(), "标签 {label} 不该有 pending");
        }
    }

    #[test]
    fn every_ui_candidate_except_utf8_actually_decodes() {
        // 与 session_form_body::CHARSETS 一一对应；UTF-8 走直传，其余必须
        // 解析成 ASCII 兼容编码，否则那个下拉项就是「点了没效果」的假控件。
        let candidates = [
            "UTF-8", "GBK", "GB2312", "ISO-8859-1", "ISO-8859-15", "KOI8-R", "EUC-KR",
            "EUC-JP", "Shift_JIS", "Big5",
        ];
        for label in candidates {
            let enc = Encoding::for_label_no_replacement(label.as_bytes())
                .expect("UI 候选必须是已知标签");
            assert!(enc.is_ascii_compatible(), "候选 {label} 不是 ASCII 兼容编码");
        }
        // 单字节 / 多字节候选各抽一个真实解码结果。
        let latin = encoding_rs::WINDOWS_1252.encode("café").0;
        let mut d = CharsetDecoder::new("ISO-8859-1");
        assert_eq!(&*d.feed(&latin), "café".as_bytes());
        let big5 = encoding_rs::BIG5.encode("中文").0;
        let mut d = CharsetDecoder::new("Big5");
        assert_eq!(&*d.feed(&big5), "中文".as_bytes());
    }
}

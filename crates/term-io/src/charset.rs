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
//!   接着解；会话结束时调用 [`CharsetDecoder::finish`] 以 `last = true` 再解
//!   一次，把压着的残序列按替换字符收尾，不让它静默消失。
//!
//! # ASCII 快路径只对单字节编码有效
//!
//! 单字节编码（ISO-8859-* / KOI8-R）的 `0x00..=0x7F` 是不变量：纯 ASCII 批次
//! 直接借用原缓冲，不进 decoder。
//! **多字节编码一律进 decoder**：`pending` 只是 [`CharsetDecoder::has_pending`]
//! 的保守提示，不能当不变量——GB18030 四字节序列的第二 / 第四字节是 ASCII
//! 数字，一批以它们收尾时 `pending` 会被误清；此时若还走 ASCII 快路径，后续
//! 纯 ASCII chunk 会绕过仍压着半个序列的 decoder，同一字节流拆不拆 chunk
//! 就会解出不同结果。正确性优先于这批 ASCII 的一次分配。
//!
//! # BOM 不改写显式标签
//!
//! decoder 用 `new_decoder_without_bom_handling()` 构造：会话表单选了哪个编码
//! 就是哪个，流入的 BOM 字节按该编码解，不让数据里的 BOM 成为表单之外的
//! 第二个配置源。

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
        /// 保守的「可能压着未完成序列」标记，只供 `has_pending()` 参考。
        pending: bool,
        /// `finish()` 已以 `last=true` 收尾；encoding_rs 的 decoder 用完不能
        /// 再用，此后只做直传兜底，绝不把会话带进 panic。
        finished: bool,
    },
}

/// 把一段字节交给 encoding_rs；`last=true` 时同时冲刷 decoder 里压着的残序列。
///
/// 每次最多向 encoding_rs 要 64 KiB 输出缓冲：缓冲不足（`OutputFull`）时靠外层
/// 循环续喂剩余输入，绝不把没解完的字节丢掉（大块输出不截断）。
fn decode_into(decoder: &mut Decoder, mut rest: &[u8], last: bool) -> Vec<u8> {
    const MAX_STEP: usize = 64 * 1024;
    let mut out = Vec::new();
    loop {
        let cap = decoder
            .max_utf8_buffer_length(rest.len())
            .unwrap_or_else(|| rest.len().saturating_mul(3).saturating_add(16))
            .min(MAX_STEP)
            .max(16);
        let start = out.len();
        out.resize(start + cap, 0);
        let (result, read, written, _) = decoder.decode_to_utf8(rest, &mut out[start..], last);
        out.truncate(start + written);
        // `read` 由 encoding_rs 保证不超过 `rest.len()`；这里仍用 `get` 兜住
        // 异常，避免生产路径出现切片 panic（4p 基线只许减不许增）。
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
    out
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
                // 显式标签说了算：BOM sniff 会把数据里的字节当成编码声明，
                // 那是会话表单之外的第二个配置源。
                decoder: enc.new_decoder_without_bom_handling(),
                multibyte: !enc.is_single_byte(),
                pending: false,
                finished: false,
            },
        }
    }

    /// 解码一批输出。返回值生命周期跟随输入：只有单字节编码的纯 ASCII
    /// 批次借用，其余路径分配。
    pub fn feed<'a>(&mut self, bytes: &'a [u8]) -> Cow<'a, [u8]> {
        let (decoder, multibyte, pending, finished) = match &mut self.state {
            State::Passthrough => return Cow::Borrowed(bytes),
            State::Incremental {
                decoder,
                multibyte,
                pending,
                finished,
            } => (decoder, *multibyte, pending, finished),
        };
        // 收尾之后不该再收到字节；真收到了就直传，不碰已 finished 的 decoder。
        if *finished {
            return Cow::Borrowed(bytes);
        }
        // 单字节编码的 ASCII 不变量：0x00..=0x7F 逐字节不变且不改变 decoder
        // 状态。多字节编码的 pending 不可靠（见模块文档），一律进 decoder。
        if !multibyte && !*pending && bytes.iter().all(|b| *b < 0x80) {
            return Cow::Borrowed(bytes);
        }
        let out = decode_into(decoder, bytes, false);
        if !bytes.is_empty() {
            // 仅供 `has_pending()` 参考的保守标记；解码正确性不再依赖它。
            *pending = multibyte && bytes[bytes.len() - 1] >= 0x80;
        }
        Cow::Owned(out)
    }

    /// 会话结束（EOF）收尾：以 `last=true` 再解一次，把 decoder 里压着的
    /// 未完结多字节序列按替换字符吐出。不调用它，最后一个残序列会静默丢失。
    /// 幂等：重复调用返回空。
    pub fn finish(&mut self) -> Vec<u8> {
        match &mut self.state {
            State::Passthrough => Vec::new(),
            State::Incremental {
                decoder,
                pending,
                finished,
                ..
            } => {
                if *finished {
                    return Vec::new();
                }
                *finished = true;
                *pending = false;
                decode_into(decoder, &[], true)
            }
        }
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

        // 大块输出：单次 decode 的缓冲上限 64 KiB，超出会返回 OutputFull；
        // 外层循环必须续喂剩余输入，一个字节都不许截断。
        let big = "中文".repeat(40_000);
        let raw = gbk(&big);
        assert!(raw.len() > 128 * 1024, "用例本身要大到触发 OutputFull");
        let mut d = CharsetDecoder::new("GBK");
        let decoded = d.feed(&raw);
        assert_eq!(decoded.len(), big.len(), "大块输出被截断");
        assert_eq!(&*decoded, big.as_bytes());
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

        // GB18030 四字节序列的第二字节是 ASCII 数字：以它收尾时 pending 会被
        // 误清。取消多字节快路径之后，夹在中间的纯 ASCII chunk 仍须进 decoder，
        // 「拆开喂」与「整块喂」必须逐字节一致。
        let raw = encoding_rs::GB18030.encode("𐀀").0.into_owned();
        assert_eq!(raw.as_slice(), &[0x90, 0x30, 0x81, 0x30]);
        let mut stream = Vec::new();
        stream.extend_from_slice(&raw[..2]);
        stream.extend_from_slice(b"HELLO");
        stream.extend_from_slice(&raw[2..3]); // 只剩 lead byte 0x81：EOF 处是残序列
        let whole = {
            let mut d = CharsetDecoder::new("GB18030");
            let mut out = d.feed(&stream).into_owned();
            out.extend_from_slice(&d.finish());
            out
        };
        let mut split = Vec::new();
        let mut d = CharsetDecoder::new("GB18030");
        for chunk in [&stream[..2], &stream[2..7], &stream[7..]] {
            split.extend_from_slice(&d.feed(chunk));
        }
        split.extend_from_slice(&d.finish());
        assert_eq!(split, whole, "拆 chunk 改变了 GB18030 的解码结果");
        // 与审查里的参考输出逐字节一致：EF BF BD（坏 0x90）+ '0' + HELLO
        // + EF BF BD（EOF 处未完结的 0x81）。旧快路径会漏掉第一个替换字符。
        let want = [
            b"\xEF\xBF\xBD".as_slice(),
            b"0",
            b"HELLO",
            b"\xEF\xBF\xBD",
        ]
        .concat();
        assert_eq!(split, want, "畸形流的替换字符位置不对");
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
        assert!(d.finish().is_empty());
    }

    #[test]
    fn escape_sequences_survive_gbk_decoding_verbatim() {
        let raw = gbk("中文");
        let want = [
            b"\x1b[31m".as_slice(),
            "中文".as_bytes(),
            b"\x1b[0m\r\n",
        ]
        .concat();

        let mut bytes = b"\x1b[31m".to_vec();
        bytes.extend_from_slice(&raw);
        bytes.extend_from_slice(b"\x1b[0m\r\n");
        let mut d = CharsetDecoder::new("GBK");
        let out = d.feed(&bytes);
        assert_eq!(&*out, want.as_slice());

        // 控制序列自己也会跨 chunk：多字节编码取消 ASCII 快路径后，
        // "\x1b[" 与 "31m" 分两批到达仍逐字节穿过 decoder。
        let zh = gbk("中文");
        let mut d = CharsetDecoder::new("GBK");
        let mut all = Vec::new();
        for chunk in [
            b"\x1b[".as_slice(),
            b"31m",
            zh.as_slice(),
            b"\x1b[0m",
            b"\r\n",
        ] {
            all.extend_from_slice(&d.feed(chunk));
        }
        assert_eq!(all, want);
    }

    #[test]
    fn ascii_fast_path_is_single_byte_only() {
        // 单字节编码保留零拷贝快路径。
        let mut d = CharsetDecoder::new("ISO-8859-1");
        let out = d.feed(b"Password: \x1b[0m");
        assert!(matches!(out, Cow::Borrowed(_)), "单字节 ASCII 不该触发分配");
        assert_eq!(&*out, b"Password: \x1b[0m");

        // 多字节编码取消快路径：字节结果仍逐字节相同，但必须走 decoder，
        // 否则 GB18030 的 ASCII 数字会把 decoder 的 pending 状态绕过。
        let mut d = CharsetDecoder::new("GBK");
        let out = d.feed(b"Password: \x1b[0m");
        assert!(matches!(out, Cow::Owned(_)), "多字节编码不能走 ASCII 快路径");
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
            assert!(d.finish().is_empty());
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
        // 候选逐项真解码，不再只验标签能被解析。
        let samples = [
            ("GBK", "中文"),
            ("GB2312", "中文"),
            ("ISO-8859-1", "café"),
            ("ISO-8859-15", "café"),
            ("KOI8-R", "Привет"),
            ("EUC-KR", "안녕"),
            ("EUC-JP", "こんにちは"),
            ("Shift_JIS", "こんにちは"),
            ("Big5", "中文"),
        ];
        for (label, text) in samples {
            let enc = Encoding::for_label_no_replacement(label.as_bytes()).expect("已知标签");
            let raw = enc.encode(text).0;
            let mut d = CharsetDecoder::new(label);
            assert_eq!(&*d.feed(&raw), text.as_bytes(), "候选 {label} 没真解码");
        }
        // BOM 不能改写显式标签：GBK 会话里喂 UTF-16BE BOM（两字节，且 GBK
        // 会把它们当两个独立无效字节吃掉，不破坏后面汉字的双字节对齐），
        // 中文仍要按显式标签解出来。
        let mut payload = vec![0xFE, 0xFF];
        payload.extend_from_slice(&gbk("中文"));
        let mut d = CharsetDecoder::new("GBK");
        let decoded_bytes = d.feed(&payload);
        let decoded = String::from_utf8_lossy(&decoded_bytes);
        assert!(decoded.contains("中文"), "BOM 改写了显式标签：{decoded:?}");
        // 对照：encoding_rs 的默认 decoder 会因为 FE FF 切成 UTF-16BE，
        // 同一份字节流里就解不出「中文」——这条保证测试真能抓住回归。
        let mut sniff_decoder = encoding_rs::GBK.new_decoder();
        let cap = sniff_decoder
            .max_utf8_buffer_length(payload.len())
            .unwrap_or(payload.len() * 3 + 16);
        let mut sniff_out = vec![0u8; cap];
        let (_, _, written, _) = sniff_decoder.decode_to_utf8(&payload, &mut sniff_out, false);
        let sniffed = String::from_utf8_lossy(&sniff_out[..written]);
        assert!(!sniffed.contains("中文"), "对照组意外没走 BOM sniff");
        // EOF 收尾：孤立 lead byte 以替换字符结束，不静默丢弃。
        let mut d = CharsetDecoder::new("GBK");
        assert!(d.feed(&gbk("中")[..1]).is_empty());
        assert_eq!(d.finish(), "\u{FFFD}".as_bytes());
        assert!(d.finish().is_empty(), "finish 必须幂等");
    }
}

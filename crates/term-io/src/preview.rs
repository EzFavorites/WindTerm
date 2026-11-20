//! 文件头部读取 + 内容类型嗅探：D6 预览窗的数据底座。
//!
//! 为什么不放在 `apps/desktop/src/ui`：预览要打开真实文件、持有 OS 句柄——
//! 与 `local_dir` / `pty` / `serial` 同类，按 B2 归 IO 层。投影层只该拿到
//! 「一段已经变成 `String` 的文本 + 一个 `bool`」然后画。
//!
//! # 三条硬约束
//!
//! 1. **只读开头**。`read_head` 的上限是 [`DEFAULT_HEAD_BYTES`]（64 KiB）。
//!    把 3 GB 的日志整个读进内存是预览功能最典型的错。超出部分的语义由
//!    [`Head::truncated`] 表达，由调用方去告诉用户「只显示了开头」。
//! 2. **只看字节，不看扩展名**。Filer 列出来的文件经常是别人改名改错的
//!    （`notes.txt` 其实是个 PNG，`logo.png` 其实是段 UTF-8 日志）。扩展名
//!    会说谎，魔数不会。
//! 3. **不解码图片**。图片只读文件头拿宽高给 UI 画占位框用；真正解码交给
//!    渲染层（`apps/desktop` 的 `image` 依赖），因为这层不该背一个解码器。
//!
//! # 判定顺序（顺序本身就是正确性）
//!
//! ```text
//! 空 → BOM（文本）→ 图片魔数 → 含 0x00 → 合法 UTF-8 → 控制字符比例 → Latin1
//! ```
//!
//! BOM **必须**排在 `0x00` 判定之前：UTF-16 文本天生一半字节是 `0x00`，
//! 先数 `0x00` 会把所有 UTF-16 文件判成二进制。
//!
//! 图片魔数也**必须**排在 `0x00` 之前：PNG 的 IHDR 里就有 `0x00`。
//!
//! # 已知取舍
//!
//! 没有 BOM 的 UTF-16 文本判不出来，会落到「含 `0x00` → 二进制」。这是
//! 有意的：无 BOM 时 UTF-16 与二进制在字节层面**不可区分**（任意二进制都
//! 可以被解释成一串 UTF-16 码元），任何启发式都只是在赌。宁可显示「二进制」
//! 让用户自己用十六进制看，也不要赌错了把乱码当文本渲染。

use std::io::{ErrorKind, Read};
use std::path::Path;

/// 一次最多读多少字节。预览只要开头——把 3 GB 的日志整个读进内存是最典型的错。
pub const DEFAULT_HEAD_BYTES: usize = 64 * 1024;

/// 预览窗口要显示的行数上限（沿用 `confirm_paste` 的 20 行口径，
/// 但这是新模块自己的常量，不要跨 crate 引用 `desk` 的私有常量）。
pub const PREVIEW_MAX_LINES: usize = 200;

/// 二进制判定的取样窗口：只看开头这么多字节。
///
/// 不用整个 `bytes` 有两个原因：判定成本与 `limit` 无关（读 1 MB 头也只扫
/// 8 KB），且判定结果不会被文件后半段的内容污染——预览窗对「这是什么文件」
/// 的判断只看开头那一段就够。
const SNIFF_WINDOW: usize = 8 * 1024;

/// Latin1 回落的控制字符比例上限（百分比）。
///
/// 阈值取 10%：正常文本里控制字符通常只有 `\n`（已被排除）和极少量 `ESC`，
/// 比例是 0.0x% 量级；而二进制文件（可执行文件、压缩包、图片被改名成
/// `.txt`）的控制字符占比普遍在 15% 以上。10% 落在这条沟里，两侧都有余量。
/// 用整数比较（`control * 100 > total * LATIN1_CONTROL_PERCENT`）而不是浮点，
/// 免得在边界上跟浮点误差吵架。
const LATIN1_CONTROL_PERCENT: usize = 10;

/// 读到的文件开头。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Head {
    /// 真正读到的字节（≤ 请求上限）。
    pub bytes: Vec<u8>,
    /// 文件比 `bytes` 长——调用方要据此告诉用户「只显示了开头」。
    pub truncated: bool,
}

/// 认识这几家就够覆盖 Filer 里 99% 的图片。都不认识就当 `Binary`。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ImageFormat {
    Png,
    Jpeg,
    Gif,
    Bmp,
    WebP,
}

/// 嗅探出来的内容类型。**只看字节，不看扩展名**——扩展名会说谎，
/// 而 Filer 列出来的文件恰恰经常是别人改名改错的。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ContentKind {
    /// 0 字节文件。与 `Text` 分开是为了让 UI 能显示「空文件」而不是一片空白。
    Empty,
    Text {
        charset: Charset,
    },
    Image {
        format: ImageFormat,
        /// 读不出来时为 `0`——不猜。UI 见到 0 就不画尺寸。
        width: u32,
        height: u32,
    },
    Binary,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Charset {
    Utf8,
    /// `EF BB BF`。解码时**去掉** BOM。
    Utf8Bom,
    /// `FF FE`。
    Utf16Le,
    /// `FE FF`。
    Utf16Be,
    /// 不是合法 UTF-8，但也不像二进制：按 ISO-8859-1 逐字节映射。
    Latin1,
}

/// `read_head` 的失败原因。刻意不用 `io::Error`：投影层不该拿到一个能读
/// 错误链、却还是要自己 match `kind()` 的东西；这里把四种 UI 需要区别对待
/// 的情形直接枚举出来，其余原样保留 `ErrorKind`。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HeadError {
    /// 路径不存在（含断链的符号链接）。
    NotFound,
    /// 路径是目录。预览窗要显示「这是个目录」，而不是「读失败」。
    IsDir,
    /// 文件在，但没权限读。
    Denied,
    /// 其余 IO 错误，原样带上 `ErrorKind`。
    Io(std::io::ErrorKind),
}

impl HeadError {
    /// 给 UI 直接用的一行说明（英文，与终端层的其他诊断同语言）。
    pub fn message(&self) -> String {
        match self {
            HeadError::NotFound => String::from("no such file"),
            HeadError::IsDir => String::from("is a directory"),
            HeadError::Denied => String::from("permission denied"),
            HeadError::Io(kind) => format!("io error: {kind}"),
        }
    }
}

/// 读文件开头。`limit` 为 0 时用 [`DEFAULT_HEAD_BYTES`]。
///
/// 先 `metadata` 再 `open`：Unix 上 `open` 一个目录是成功的，报错要等到
/// `read` 才出现（`EISDIR`），那样就分不清「目录」和「读坏了」。先查
/// `is_dir` 能给出准确的原因；符号链接因为 `metadata` 跟随链接，也一并正确。
pub fn read_head(path: &Path, limit: usize) -> Result<Head, HeadError> {
    let cap = if limit == 0 {
        DEFAULT_HEAD_BYTES
    } else {
        limit
    };
    let meta = std::fs::metadata(path).map_err(map_io)?;
    if meta.is_dir() {
        return Err(HeadError::IsDir);
    }
    let mut file = std::fs::File::open(path).map_err(map_io)?;
    let mut bytes: Vec<u8> = Vec::with_capacity(cap.min(64 * 1024));
    file.by_ref()
        .take(cap as u64)
        .read_to_end(&mut bytes)
        .map_err(map_io)?;
    // 用文件长度而不是「是否读满 cap」判断截断：正好读完一个 cap 长的文件
    // 不该被说成「只显示了开头」。
    let truncated = (bytes.len() as u64) < meta.len();
    Ok(Head { bytes, truncated })
}

/// 纯字节嗅探，不碰文件系统——这是本模块最该被单测覆盖的部分。
pub fn sniff(bytes: &[u8]) -> ContentKind {
    if bytes.is_empty() {
        return ContentKind::Empty;
    }
    if let Some(charset) = bom_charset(bytes) {
        return ContentKind::Text { charset };
    }
    if let Some(kind) = sniff_image(bytes) {
        return kind;
    }
    let window = head_window(bytes);
    // 含 NUL：二进制。ASCII 文本、UTF-8、Latin1 里都不会出现 0x00。
    if window.contains(&0x00) {
        return ContentKind::Binary;
    }
    if utf8_valid_allow_tail(bytes) {
        return ContentKind::Text {
            charset: Charset::Utf8,
        };
    }
    if control_ratio_is_low(window) {
        return ContentKind::Text {
            charset: Charset::Latin1,
        };
    }
    ContentKind::Binary
}

/// 把 `Head` 变成给预览窗用的文本 + 是否截断。
///
/// 非文本返回 `None`（图片 / 二进制 / 空文件由调用方另行处理，因为它们的
/// 呈现方式完全不同——图片要画、二进制要提示开十六进制、空文件要提示空）。
///
/// 第二项的语义是「**用户看到的不是全部**」，三种原因都算：
/// 1. 文件本身比读到的字节长（`head.truncated`）；
/// 2. 行数超过 [`PREVIEW_MAX_LINES`] 被裁掉；
/// 3. 以上两者之一，或都有。
///
/// 截断时**不会**在多字节字符中间下刀：解码只取到最后一个完整字符，末尾
/// 那一两个不完整字节直接丢弃，所以输出里不会出现 `U+FFFD`。
pub fn head_text(head: &Head) -> Option<(String, bool)> {
    let charset = match sniff(&head.bytes) {
        ContentKind::Text { charset } => charset,
        _ => return None,
    };
    let text = decode(&head.bytes, charset);
    // 行数裁剪。`str::lines` 顺手吃掉 `\r\n` 的 `\r`，正好是预览想要的。
    let mut lines: Vec<&str> = text.lines().collect();
    let mut more = head.truncated;
    if lines.len() > PREVIEW_MAX_LINES {
        lines.truncate(PREVIEW_MAX_LINES);
        more = true;
    }
    Some((lines.join("\n"), more))
}

/// 按检测到的字符集解码。**任何路径都不产生替换字符**：坏字节一律丢弃，
/// 因为预览窗里出现一串 `�` 比少几个字符更让人困惑。
fn decode(bytes: &[u8], charset: Charset) -> String {
    match charset {
        Charset::Utf8 => decode_utf8(bytes),
        // BOM 三字节已在 `sniff` 里验明，这里只需跳过；`get` 防御越界。
        Charset::Utf8Bom => bytes.get(3..).map(decode_utf8).unwrap_or_default(),
        Charset::Utf16Le => decode_utf16(bytes, true),
        Charset::Utf16Be => decode_utf16(bytes, false),
        Charset::Latin1 => bytes.iter().copied().map(char::from).collect(),
    }
}

/// UTF-8 解码：合法就整段拿，尾部不完整就把不完整的那截丢掉。
fn decode_utf8(bytes: &[u8]) -> String {
    match std::str::from_utf8(bytes) {
        Ok(text) => String::from(text),
        Err(err) => {
            let upto = err.valid_up_to();
            bytes
                .get(..upto)
                .and_then(|valid| std::str::from_utf8(valid).ok())
                .map(String::from)
                .unwrap_or_default()
        }
    }
}

/// UTF-16 解码。奇数尾字节（截断切在一半）直接丢掉；落单的代理项也丢掉
/// （`filter_map` 只留 `Ok`），同样是为了不产生替换字符。
fn decode_utf16(bytes: &[u8], little_endian: bool) -> String {
    let usable = bytes.len() - bytes.len() % 2;
    let units = bytes.get(..usable).map(|even| {
        even.chunks_exact(2)
            .map(|pair| match <[u8; 2]>::try_from(pair) {
                Ok(pair) => {
                    if little_endian {
                        u16::from_le_bytes(pair)
                    } else {
                        u16::from_be_bytes(pair)
                    }
                }
                // `chunks_exact(2)` 不可能给出长度不为 2 的块；给 0 只是
                // 为了让这里没有 `unwrap()`。
                Err(_) => 0,
            })
            .collect::<Vec<u16>>()
    });
    match units {
        Some(units) => char::decode_utf16(units).filter_map(Result::ok).collect(),
        None => String::new(),
    }
}

/// BOM 判定。**只认这三种**，别的一概不猜。
fn bom_charset(bytes: &[u8]) -> Option<Charset> {
    if bytes.starts_with(&[0xEF, 0xBB, 0xBF]) {
        Some(Charset::Utf8Bom)
    } else if bytes.starts_with(&[0xFF, 0xFE]) {
        // `FF FE 00 00` 是 UTF-32LE，本模块不支持；按 UTF-16LE 处理会在
        // 每个字符前多出一个 NUL，但那比「猜成 UTF-32 却解不出来」好。
        Some(Charset::Utf16Le)
    } else if bytes.starts_with(&[0xFE, 0xFF]) {
        Some(Charset::Utf16Be)
    } else {
        None
    }
}

/// 判定取样窗口：开头至多 [`SNIFF_WINDOW`] 字节。
fn head_window(bytes: &[u8]) -> &[u8] {
    bytes.get(..SNIFF_WINDOW.min(bytes.len())).unwrap_or(bytes)
}

/// UTF-8 合法性，但容忍**尾部**只有半个字符。
///
/// 头部读取本来就可能把一个 3 字节汉字切成 1+2，这种文件当然是 UTF-8 文本。
/// `error_len() == None` 正是「输入提前结束」这一种，其余错误（非法字节、
/// 过长编码）都是真坏。
fn utf8_valid_allow_tail(bytes: &[u8]) -> bool {
    match std::str::from_utf8(bytes) {
        Ok(_) => true,
        Err(err) => err.error_len().is_none(),
    }
}

/// 控制字符占比是否足够低。`\t \n \v \f \r` 不算控制字符——它们是正常的
/// 文本填充，把制表符算成二进制会让所有 Makefile 和 TSV 变成二进制。
fn control_ratio_is_low(window: &[u8]) -> bool {
    if window.is_empty() {
        return true;
    }
    let control = window.iter().filter(|byte| is_control(**byte)).count();
    control * 100 <= window.len() * LATIN1_CONTROL_PERCENT
}

/// 控制字符口径：`< 0x09` 或 `0x0E..=0x1F`。即把 `\t(09) \n(0A) \v(0B)
/// \f(0C) \r(0D)` 排除在外。`0x7F`（DEL）不算——它在 Latin1 里是合法字符。
fn is_control(byte: u8) -> bool {
    byte < 0x09 || (0x0E..=0x1F).contains(&byte)
}

/// 图片嗅探：只读文件头，不解码像素。
///
/// 一个格式都认不出来返回 `None`（交给文本/二进制判定），认出格式但读不出
/// 宽高则返回 `Image { format, 0, 0 }`——魔数已经足够确定它是什么，宽高只是
/// 附加信息。
fn sniff_image(bytes: &[u8]) -> Option<ContentKind> {
    const PNG_SIG: [u8; 8] = [0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A];
    if bytes.starts_with(&PNG_SIG) {
        // 8 字节签名之后是长度(4) + "IHDR"(4) + 宽(4) + 高(4)。
        let (width, height) = if bytes.get(12..16) == Some(b"IHDR".as_slice()) {
            (
                be_u32(bytes, 16).unwrap_or(0),
                be_u32(bytes, 20).unwrap_or(0),
            )
        } else {
            (0, 0)
        };
        return Some(ContentKind::Image {
            format: ImageFormat::Png,
            width,
            height,
        });
    }
    if bytes.starts_with(&[0xFF, 0xD8, 0xFF]) {
        // JPEG 的头里没有固定位置的宽高，必须顺着段链扫到 SOF。扫不到就只能
        // 说「是 JPEG 但量不出尺寸」——按规格退化成二进制，避免 UI 画一个
        // 永远 0×0 的占位框。
        return match jpeg_sof_size(bytes) {
            Some((width, height)) => Some(ContentKind::Image {
                format: ImageFormat::Jpeg,
                width,
                height,
            }),
            None => Some(ContentKind::Binary),
        };
    }
    if bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a") {
        // 逻辑屏宽高紧跟版本号，小端 u16。
        return Some(ContentKind::Image {
            format: ImageFormat::Gif,
            width: le_u16(bytes, 6).map(u32::from).unwrap_or(0),
            height: le_u16(bytes, 8).map(u32::from).unwrap_or(0),
        });
    }
    if bytes.starts_with(b"BM") {
        let (width, height) = bmp_size(bytes);
        return Some(ContentKind::Image {
            format: ImageFormat::Bmp,
            width,
            height,
        });
    }
    if bytes.starts_with(b"RIFF") && bytes.get(8..12) == Some(b"WEBP".as_slice()) {
        let (width, height) = webp_vp8x_size(bytes).unwrap_or((0, 0));
        return Some(ContentKind::Image {
            format: ImageFormat::WebP,
            width,
            height,
        });
    }
    None
}

/// BMP 的 DIB 头（偏移 14 起）宽高。`BITMAPINFOHEADER`（40）用 i32，
/// 高度可以为负（自顶向下位图），取绝对值；老式 `BITMAPCOREHEADER`（12）
/// 用 u16。两种都认，别的（v4/v5 头也是 i32 同布局）不特殊处理。
fn bmp_size(bytes: &[u8]) -> (u32, u32) {
    let dib = le_u32(bytes, 14).unwrap_or(0);
    if dib == 12 {
        (
            le_u16(bytes, 18).map(u32::from).unwrap_or(0),
            le_u16(bytes, 20).map(u32::from).unwrap_or(0),
        )
    } else {
        (
            le_i32(bytes, 18).map(|w| w.unsigned_abs()).unwrap_or(0),
            le_i32(bytes, 22).map(|h| h.unsigned_abs()).unwrap_or(0),
        )
    }
}

/// WebP 的 `VP8X` 块里画布宽高是 24 位小端的「减一」值（有损 `VP8 ` 与无损
/// `VP8L` 的宽高埋在码流里，取值要跟着布尔算术走，这层不做）。只认 `VP8X`。
fn webp_vp8x_size(bytes: &[u8]) -> Option<(u32, u32)> {
    if bytes.get(12..16) != Some(b"VP8X".as_slice()) {
        return None;
    }
    // VP8X 载荷：flags(4) + 画布宽-1(3, LE) + 画布高-1(3, LE)。
    Some((
        le_u24(bytes, 24)?.checked_add(1)?,
        le_u24(bytes, 27)?.checked_add(1)?,
    ))
}

/// 扫 JPEG 段链找 SOF，取宽高。
///
/// 段结构：`FF` + marker + 长度(2, 大端，含自身) + 载荷。SOF 载荷是
/// 精度(1) + 高(2) + 宽(2)。跳过的段按长度前进，所以整条链是线性的。
fn jpeg_sof_size(bytes: &[u8]) -> Option<(u32, u32)> {
    let mut pos = 2usize;
    // 段链有界：只要还有一个 marker + 长度就继续。头部读取最多 64 KiB，
    // 这里不可能失控——每次循环 `pos` 严格递增。
    while pos + 1 < bytes.len() {
        if *bytes.get(pos)? != 0xFF {
            // 不在 marker 位置上，说明段链已经不成形（比如这其实是伪造
            // 的 `FF D8 FF` 前缀），放弃。
            return None;
        }
        let mut marker_at = pos + 1;
        // 填充字节：`FF FF ... FF` 一律跳过。
        while bytes.get(marker_at) == Some(&0xFF) {
            marker_at += 1;
        }
        let marker = *bytes.get(marker_at)?;
        // 无载荷的 marker：TEM(01)、RST0..RST7(D0..D7)、SOI(D8)。直接跨过。
        if marker == 0x01 || marker == 0xD8 || (0xD0..=0xD7).contains(&marker) {
            pos = marker_at + 1;
            continue;
        }
        let seg_len = usize::from(be_u16(bytes, marker_at + 1)?);
        if is_sof(marker) {
            // 载荷：精度在 marker_at+3，高在 +4，宽在 +6。越界就认输。
            let height = u32::from(be_u16(bytes, marker_at + 4)?);
            let width = u32::from(be_u16(bytes, marker_at + 6)?);
            return Some((width, height));
        }
        if seg_len < 2 {
            // 长度字段连自己都装不下——链断了。
            return None;
        }
        pos = marker_at + 1 + seg_len;
    }
    None
}

/// SOF 家族。`C4`(DHT) / `C8`(JPG) / `CC`(DAC) 不是 SOF，别误认。
fn is_sof(marker: u8) -> bool {
    matches!(marker,
        0xC0..=0xC3 | 0xC5..=0xC7 | 0xC9..=0xCB | 0xCD..=0xCF)
}

/// 大端 u32；越界返回 `None`（不 panic、不瞎补零）。
fn be_u32(bytes: &[u8], at: usize) -> Option<u32> {
    let end = at.checked_add(4)?;
    let quad = <[u8; 4]>::try_from(bytes.get(at..end)?).ok()?;
    Some(u32::from_be_bytes(quad))
}

/// 大端 u16；越界返回 `None`。
fn be_u16(bytes: &[u8], at: usize) -> Option<u16> {
    let end = at.checked_add(2)?;
    let pair = <[u8; 2]>::try_from(bytes.get(at..end)?).ok()?;
    Some(u16::from_be_bytes(pair))
}

/// 小端 u16；越界返回 `None`。
fn le_u16(bytes: &[u8], at: usize) -> Option<u16> {
    let end = at.checked_add(2)?;
    let pair = <[u8; 2]>::try_from(bytes.get(at..end)?).ok()?;
    Some(u16::from_le_bytes(pair))
}

/// 小端 u32；越界返回 `None`。
fn le_u32(bytes: &[u8], at: usize) -> Option<u32> {
    let end = at.checked_add(4)?;
    let quad = <[u8; 4]>::try_from(bytes.get(at..end)?).ok()?;
    Some(u32::from_le_bytes(quad))
}

/// 小端 i32；越界返回 `None`。
fn le_i32(bytes: &[u8], at: usize) -> Option<i32> {
    let end = at.checked_add(4)?;
    let quad = <[u8; 4]>::try_from(bytes.get(at..end)?).ok()?;
    Some(i32::from_le_bytes(quad))
}

/// 小端 u24（WebP 专用）；越界返回 `None`。
fn le_u24(bytes: &[u8], at: usize) -> Option<u32> {
    let end = at.checked_add(3)?;
    let tri = <[u8; 3]>::try_from(bytes.get(at..end)?).ok()?;
    Some(u32::from(tri[0]) | (u32::from(tri[1]) << 8) | (u32::from(tri[2]) << 16))
}

/// `io::Error` → [`HeadError`]。四种需要区分的留着，其余的连同 `ErrorKind`
/// 一起带出去——上层记日志时还认得出是 `EISDIR` 还是 `ENOSPC`。
fn map_io(err: std::io::Error) -> HeadError {
    match err.kind() {
        ErrorKind::NotFound => HeadError::NotFound,
        ErrorKind::PermissionDenied => HeadError::Denied,
        kind => HeadError::Io(kind),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    // ---- 夹具 ------------------------------------------------------------
    //
    // 不引入 `tempfile`：`term-io` 的依赖表是边界（`verify.sh` 4d 复核），
    // 为一个测试依赖把它撑开不划算。沿用 `local_dir` 的做法：`temp_dir()` +
    // 纳秒后缀防撞名。

    struct TempDir {
        path: PathBuf,
    }

    impl TempDir {
        fn new(tag: &str) -> TempDir {
            let nanos = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0);
            let path = std::env::temp_dir().join(format!("windterm-preview-{tag}-{nanos}"));
            std::fs::create_dir_all(&path).unwrap();
            TempDir { path }
        }

        fn file(&self, name: &str, bytes: &[u8]) -> PathBuf {
            let path = self.path.join(name);
            std::fs::write(&path, bytes).unwrap();
            path
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.path);
        }
    }

    /// 最小 PNG 头：8 字节签名 + 长度 + IHDR + 宽 + 高。
    fn png_header(width: u32, height: u32) -> Vec<u8> {
        let mut out = vec![0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A];
        out.extend_from_slice(&13u32.to_be_bytes());
        out.extend_from_slice(b"IHDR");
        out.extend_from_slice(&width.to_be_bytes());
        out.extend_from_slice(&height.to_be_bytes());
        out.extend_from_slice(&[0x08, 0x02, 0x00, 0x00, 0x00]);
        out
    }

    /// 最小 GIF 头：版本 + 逻辑屏宽高（小端 u16）。
    fn gif_header(width: u16, height: u16) -> Vec<u8> {
        let mut out = b"GIF89a".to_vec();
        out.extend_from_slice(&width.to_le_bytes());
        out.extend_from_slice(&height.to_le_bytes());
        out.extend_from_slice(&[0x00, 0x00, 0x00]);
        out
    }

    /// 最小 BMP 头：`BM` + 三个 u32 + `BITMAPINFOHEADER`(40) + 宽 + 高。
    fn bmp_header(width: i32, height: i32) -> Vec<u8> {
        let mut out = b"BM".to_vec();
        out.extend_from_slice(&0u32.to_le_bytes()); // 文件大小（嗅探不读）
        out.extend_from_slice(&0u32.to_le_bytes()); // 保留
        out.extend_from_slice(&54u32.to_le_bytes()); // 像素数据偏移
        out.extend_from_slice(&40u32.to_le_bytes()); // DIB 头大小
        out.extend_from_slice(&width.to_le_bytes());
        out.extend_from_slice(&height.to_le_bytes());
        out
    }

    /// `RIFF....WEBP` + `VP8X`（画布宽高减一，24 位小端）。
    fn webp_vp8x(width: u32, height: u32) -> Vec<u8> {
        let mut out = b"RIFF".to_vec();
        out.extend_from_slice(&0u32.to_le_bytes());
        out.extend_from_slice(b"WEBP");
        out.extend_from_slice(b"VP8X");
        out.extend_from_slice(&10u32.to_le_bytes()); // 块长度
        out.extend_from_slice(&[0x00, 0x00, 0x00, 0x00]); // flags
        let w = width.saturating_sub(1).to_le_bytes();
        out.extend_from_slice(&w[0..3]);
        let h = height.saturating_sub(1).to_le_bytes();
        out.extend_from_slice(&h[0..3]);
        out
    }

    /// 最小 JPEG 头：SOI + SOS 之前的 `APP0` 段（没用）+ `SOF0` 段（给宽高）。
    fn jpeg_header(width: u16, height: u16) -> Vec<u8> {
        let mut out = vec![0xFF, 0xD8, 0xFF];
        // APP0：FF E0 + 长度 0x0010（16，含自身）+ 14 字节载荷。
        out.extend_from_slice(&[0xE0, 0x00, 0x10]);
        out.extend_from_slice(b"JFIF\0");
        out.extend_from_slice(&[0x01, 0x01, 0x00, 0x00, 0x01, 0x00, 0x01, 0x00, 0x00]);
        // SOF0：FF C0 + 长度 0x0011 + 精度 8 + 高 + 宽 + 分量。
        out.extend_from_slice(&[0xFF, 0xC0, 0x00, 0x11, 0x08]);
        out.extend_from_slice(&height.to_be_bytes());
        out.extend_from_slice(&width.to_be_bytes());
        out.extend_from_slice(&[0x03]);
        out
    }

    /// 取 `Text` 的字符集，非文本直接失败（测试里断言用）。
    fn charset_of(bytes: &[u8]) -> Charset {
        match sniff(bytes) {
            ContentKind::Text { charset } => charset,
            other => panic!("期望 Text，实际 {other:?}"),
        }
    }

    fn image_of(bytes: &[u8]) -> (ImageFormat, u32, u32) {
        match sniff(bytes) {
            ContentKind::Image {
                format,
                width,
                height,
            } => (format, width, height),
            other => panic!("期望 Image，实际 {other:?}"),
        }
    }

    // ---- sniff：空 / 文本 -------------------------------------------------

    #[test]
    fn an_empty_file_is_empty_not_text() {
        assert_eq!(sniff(&[]), ContentKind::Empty);
    }

    #[test]
    fn plain_ascii_is_utf8_text() {
        let bytes = b"hello, world\nsecond line\n";
        assert_eq!(
            sniff(bytes),
            ContentKind::Text {
                charset: Charset::Utf8
            }
        );
    }

    #[test]
    fn chinese_utf8_is_utf8_text() {
        // 「中文」= E4 B8 AD E6 96 87，六个字节里一个 0x00 都没有。
        assert_eq!(charset_of("中文".as_bytes()), Charset::Utf8);
    }

    #[test]
    fn a_utf8_bom_is_reported_and_stripped_when_decoded() {
        let mut bytes = vec![0xEF, 0xBB, 0xBF];
        bytes.extend_from_slice(b"hello");
        assert_eq!(charset_of(&bytes), Charset::Utf8Bom);
        let head = Head {
            bytes,
            truncated: false,
        };
        let (text, more) = head_text(&head).unwrap();
        assert_eq!(text, "hello", "BOM 不该留在文本里");
        assert!(
            !text.starts_with('\u{FEFF}'),
            "解出来的字符串不能带 BOM 字符"
        );
        assert!(!more);
    }

    #[test]
    fn utf16_le_text_is_not_mistaken_for_binary() {
        // BOM FF FE + "hi" 的小端编码：68 00 69 00。正文里天然有 0x00。
        let bytes = vec![0xFF, 0xFE, 0x68, 0x00, 0x69, 0x00];
        assert_eq!(charset_of(&bytes), Charset::Utf16Le);
        let (text, _) = head_text(&Head {
            bytes,
            truncated: false,
        })
        .unwrap();
        // BOM 的两个码元解出来是 U+FEFF，不属于正文，UI 侧会看到它；这里只
        // 断言正文字符没丢。
        assert!(text.contains("hi"), "UTF-16LE 正文应解出 hi，实际 {text:?}");
    }

    #[test]
    fn utf16_be_text_is_not_mistaken_for_binary() {
        let bytes = vec![0xFE, 0xFF, 0x00, 0x68, 0x00, 0x69];
        assert_eq!(charset_of(&bytes), Charset::Utf16Be);
        let (text, _) = head_text(&Head {
            bytes,
            truncated: false,
        })
        .unwrap();
        assert!(text.contains("hi"), "UTF-16BE 正文应解出 hi，实际 {text:?}");
    }

    #[test]
    fn a_bom_file_without_a_body_is_still_text() {
        let bytes = vec![0xEF, 0xBB, 0xBF];
        assert_eq!(charset_of(&bytes), Charset::Utf8Bom);
        let (text, more) = head_text(&Head {
            bytes,
            truncated: false,
        })
        .unwrap();
        assert!(text.is_empty());
        assert!(!more);
    }

    // ---- sniff：二进制 ---------------------------------------------------

    #[test]
    fn a_stray_nul_byte_means_binary() {
        let mut bytes = b"looks like text".to_vec();
        bytes.push(0x00);
        assert_eq!(sniff(&bytes), ContentKind::Binary);
    }

    #[test]
    fn a_high_share_of_control_bytes_means_binary() {
        // 40% 控制字符（0x1B=ESC）+ 0xFF（非法 UTF-8）→ 二进制。
        let mut bytes = vec![0x1B; 40];
        bytes.extend(vec![0xFFu8; 40]);
        bytes.extend(vec![b'A'; 20]);
        assert_eq!(sniff(&bytes), ContentKind::Binary);
    }

    #[test]
    fn invalid_utf8_without_nul_falls_back_to_latin1() {
        // E9 E8 E0 在 Latin1 里是 é è à；连在一起不是合法 UTF-8。
        let bytes = vec![0xE9, 0xE8, 0xE0, 0xE9, 0xE8, 0xE0];
        assert_eq!(charset_of(&bytes), Charset::Latin1);
    }

    #[test]
    fn latin1_decoding_maps_bytes_one_to_one() {
        let bytes = vec![0xE9, b'x'];
        let (text, _) = head_text(&Head {
            bytes,
            truncated: false,
        })
        .unwrap();
        assert_eq!(text, "éx");
    }

    // ---- sniff：控制字符比例的两条边界 -----------------------------------

    #[test]
    fn exactly_ten_percent_control_bytes_stays_latin1() {
        // 10/100 = 10%，不「大于」阈值 → 仍是文本。
        let mut bytes = vec![0x1F; 10];
        bytes.extend(vec![0xA9u8; 90]); // 0xA9 单独出现是非法 UTF-8
        assert_eq!(charset_of(&bytes), Charset::Latin1);
    }

    #[test]
    fn eleven_percent_control_bytes_is_binary() {
        let mut bytes = vec![0x1F; 11];
        bytes.extend(vec![0xA9u8; 89]);
        assert_eq!(sniff(&bytes), ContentKind::Binary);
    }

    #[test]
    fn tabs_and_newlines_are_not_control_bytes() {
        // 制表符/换行占 100%：仍然必须是文本，否则所有 Makefile 都成二进制。
        let bytes = b"\ta\nb\tc\r\nd\te\n".to_vec();
        assert_eq!(charset_of(&bytes), Charset::Utf8);
    }

    // ---- sniff：图片 -----------------------------------------------------

    #[test]
    fn a_png_header_yields_the_ihdr_size() {
        let bytes = png_header(640, 480);
        assert_eq!(image_of(&bytes), (ImageFormat::Png, 640, 480));
    }

    #[test]
    fn a_gif_header_yields_the_logical_screen_size() {
        let bytes = gif_header(320, 200);
        assert_eq!(image_of(&bytes), (ImageFormat::Gif, 320, 200));
    }

    #[test]
    fn a_bmp_header_yields_the_info_header_size() {
        let bytes = bmp_header(1024, 768);
        assert_eq!(image_of(&bytes), (ImageFormat::Bmp, 1024, 768));
    }

    #[test]
    fn a_bmp_with_negative_height_uses_the_absolute_value() {
        // 自顶向下位图的高度是负数，UI 要的是像素数量。
        assert_eq!(image_of(&bmp_header(16, -16)), (ImageFormat::Bmp, 16, 16));
    }

    #[test]
    fn a_webp_vp8x_header_yields_the_canvas_size() {
        let bytes = webp_vp8x(400, 300);
        assert_eq!(image_of(&bytes), (ImageFormat::WebP, 400, 300));
    }

    #[test]
    fn a_webp_riff_header_without_vp8x_is_still_webp() {
        // 有损 VP8 / 无损 VP8L 的宽高埋在码流里，这层不读 → 宽高给 0，
        // 但格式必须认出来。
        let mut bytes = b"RIFF".to_vec();
        bytes.extend_from_slice(&0u32.to_le_bytes());
        bytes.extend_from_slice(b"WEBP");
        bytes.extend_from_slice(b"VP8 ");
        bytes.extend_from_slice(&[0u8; 16]);
        assert_eq!(image_of(&bytes), (ImageFormat::WebP, 0, 0));
    }

    #[test]
    fn a_jpeg_header_is_scanned_for_the_sof_segment() {
        let bytes = jpeg_header(800, 600);
        assert_eq!(image_of(&bytes), (ImageFormat::Jpeg, 800, 600));
    }

    #[test]
    fn a_jpeg_without_a_sof_segment_degrades_to_binary() {
        // 只有 SOI + APP0，段链走完也没见到 SOF：量不出尺寸就只能说二进制。
        let mut bytes = vec![0xFF, 0xD8, 0xFF, 0xE0, 0x00, 0x04, 0x00, 0x00];
        bytes.push(0x00);
        assert_eq!(sniff(&bytes), ContentKind::Binary);
    }

    #[test]
    fn a_truncated_png_header_does_not_panic_and_reports_zero_size() {
        // 只有 8 字节签名，IHDR 一个字都没有。签名已经足够确定「这是 PNG」，
        // 宽高给 0（不猜），而不是丢回 Binary。
        let mut bytes = png_header(1, 1);
        bytes.truncate(8);
        assert_eq!(image_of(&bytes), (ImageFormat::Png, 0, 0));
    }

    #[test]
    fn a_png_signature_with_a_broken_ihdr_tag_reports_zero_size() {
        let mut bytes = png_header(64, 64);
        // 把 "IHDR" 改坏：不该被当成 PNG 尺寸来源。
        bytes[12..16].copy_from_slice(b"XXXX");
        assert_eq!(image_of(&bytes), (ImageFormat::Png, 0, 0));
    }

    // ---- read_head -------------------------------------------------------

    #[test]
    fn reading_a_missing_path_is_not_found() {
        let dir = TempDir::new("missing");
        let path = dir.path.join("nope.txt");
        assert_eq!(read_head(&path, 0), Err(HeadError::NotFound));
    }

    #[test]
    fn reading_a_directory_is_is_dir() {
        let dir = TempDir::new("isdir");
        assert_eq!(read_head(&dir.path, 0), Err(HeadError::IsDir));
    }

    #[test]
    fn the_limit_caps_the_bytes_and_sets_truncated() {
        let dir = TempDir::new("limit");
        let body: Vec<u8> = vec![b'x'; 100];
        let path = dir.file("long.txt", &body);
        let head = read_head(&path, 10).unwrap();
        assert_eq!(head.bytes.len(), 10, "必须只读 limit 个字节");
        assert!(head.truncated, "文件还有 90 字节没读，要标记截断");
    }

    #[test]
    fn a_short_file_is_read_whole_and_not_truncated() {
        let dir = TempDir::new("whole");
        let path = dir.file("short.txt", b"just a line\n");
        let head = read_head(&path, 1024).unwrap();
        assert_eq!(head.bytes, b"just a line\n");
        assert!(!head.truncated);
    }

    #[test]
    fn a_file_exactly_at_the_limit_is_not_truncated() {
        let dir = TempDir::new("exact");
        let body: Vec<u8> = vec![b'y'; 50];
        let path = dir.file("exact.txt", &body);
        let head = read_head(&path, 50).unwrap();
        assert_eq!(head.bytes.len(), 50);
        assert!(!head.truncated, "正好读完不算截断");
    }

    #[test]
    fn a_zero_limit_falls_back_to_the_default_head_size() {
        let dir = TempDir::new("default");
        let path = dir.file("small.txt", b"abc");
        let head = read_head(&path, 0).unwrap();
        assert_eq!(head.bytes, b"abc");
        assert!(DEFAULT_HEAD_BYTES >= head.bytes.len());
    }

    #[test]
    fn an_empty_file_reads_as_empty_and_has_no_text() {
        let dir = TempDir::new("empty");
        let path = dir.file("empty.txt", b"");
        let head = read_head(&path, 0).unwrap();
        assert!(head.bytes.is_empty());
        assert!(!head.truncated);
        assert_eq!(sniff(&head.bytes), ContentKind::Empty);
        assert!(head_text(&head).is_none(), "空文件不是文本，由 UI 单独显示");
    }

    // ---- head_text -------------------------------------------------------

    #[test]
    fn text_cut_inside_a_multibyte_char_loses_bytes_not_adds_replacement() {
        // 「中」= E4 B8 AD；这里只给 E4 B8，正好切在字符中间。
        let head = Head {
            bytes: b"ab".iter().copied().chain([0xE4, 0xB8]).collect(),
            truncated: true,
        };
        let (text, more) = head_text(&head).unwrap();
        assert_eq!(text, "ab", "半个字符要整段丢掉");
        assert!(!text.contains('\u{FFFD}'), "不能产生替换字符");
        assert!(more, "字节被截断，第二项必须为真");
    }

    #[test]
    fn a_long_text_is_trimmed_to_the_preview_line_cap() {
        let total = PREVIEW_MAX_LINES + 50;
        let mut bytes = Vec::new();
        for i in 0..total {
            bytes.extend_from_slice(format!("line {i}\n").as_bytes());
        }
        let head = Head {
            bytes,
            truncated: false,
        };
        let (text, more) = head_text(&head).unwrap();
        assert_eq!(text.lines().count(), PREVIEW_MAX_LINES);
        assert!(more, "裁掉了 50 行，必须告诉用户还有更多");
    }

    #[test]
    fn text_exactly_at_the_line_cap_is_not_flagged() {
        let bytes: Vec<u8> = (0..PREVIEW_MAX_LINES)
            .flat_map(|i| format!("l{i}\n").into_bytes())
            .collect();
        let head = Head {
            bytes,
            truncated: false,
        };
        let (text, more) = head_text(&head).unwrap();
        assert_eq!(text.lines().count(), PREVIEW_MAX_LINES);
        assert!(!more);
    }

    #[test]
    fn a_truncated_head_flags_more_even_when_the_lines_fit() {
        let head = Head {
            bytes: b"only one line".to_vec(),
            truncated: true,
        };
        let (_, more) = head_text(&head).unwrap();
        assert!(more);
    }

    #[test]
    fn images_and_binaries_have_no_text_form() {
        let png = Head {
            bytes: png_header(2, 2),
            truncated: false,
        };
        assert!(head_text(&png).is_none(), "图片不该走文本通道");
        let binary = Head {
            bytes: vec![0x00, 0xFF, 0x00],
            truncated: false,
        };
        assert!(head_text(&binary).is_none(), "二进制不该走文本通道");
    }

    #[test]
    fn crlf_lines_do_not_keep_their_carriage_returns() {
        let head = Head {
            bytes: b"a\r\nb\r\n".to_vec(),
            truncated: false,
        };
        let (text, _) = head_text(&head).unwrap();
        assert_eq!(text, "a\nb");
        assert!(!text.contains('\r'));
    }

    // ---- HeadError::message ---------------------------------------------

    #[test]
    fn every_head_error_has_a_non_empty_message() {
        let cases = [
            HeadError::NotFound,
            HeadError::IsDir,
            HeadError::Denied,
            HeadError::Io(ErrorKind::InvalidData),
        ];
        for err in cases {
            assert!(!err.message().is_empty(), "{err:?} 的说明不能为空");
        }
    }
}

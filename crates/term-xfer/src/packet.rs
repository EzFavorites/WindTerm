//! 线格式：控制字符、CRC、帧编解码、YMODEM 头块解析。
//!
//! 这一层不做状态判断，只做「字节 ↔ 结构」。状态机（[`crate::Receiver`] /
//! [`crate::Sender`]）负责决定下一步发什么。
//!
//! # 帧格式
//!
//! ```text
//! SOH(0x01) | blk | ~blk | data[128]  | crc_hi crc_lo   ← 128 字节帧
//! STX(0x02) | blk | ~blk | data[1024] | crc_hi crc_lo   ← 1024 字节帧
//!                       ↑ 校验和模式只有 1 字节
//! ```
//!
//! 块号从 1 开始、8 位回绕（255 之后是 0，不是 256），`~blk` 是 `blk` 的按位取反。
//! 帧长由**帧头字节**唯一决定，所以接收方能从任意切片里重新对齐——这是「任意切分
//! 都能解析」的前提。

/// 128 字节块帧头（Small block）。
pub const SOH: u8 = 0x01;
/// 1024 字节块帧头（Large block / "1K"）。
pub const STX: u8 = 0x02;
/// 传输结束（End Of Transmission）。
pub const EOT: u8 = 0x04;
/// 肯定应答。
pub const ACK: u8 = 0x06;
/// 否定应答，也用于「请用 8 位校验和模式」的握手。
pub const NAK: u8 = 0x15;
/// 中止，连发两个即中止传输。
pub const CAN: u8 = 0x18;
/// 块尾填充（Ctrl-Z）。
pub const SUB: u8 = 0x1A;
/// 「请用 CRC-16 模式」的握手字符。
pub const CRC_REQUEST: u8 = b'C';

/// 中止序列：本端要放弃时一次性甩出的字节串（连发 8 个 CAN 是老习惯，比 2 个更难
/// 被对端的噪声过滤掉）。
pub const ABORT_SEQUENCE: [u8; 8] = [CAN; 8];

/// 连收多少个 CAN 才算对方中止。
pub const ABORT_CAN_COUNT: u8 = 2;

/// 128 字节块长。
pub const BLOCK_128: usize = 128;
/// 1024 字节块长。
pub const BLOCK_1024: usize = 1024;
/// YMODEM 头块永远是 128 字节（即使数据块用 1K）。
pub const HEADER_BLOCK_SIZE: usize = BLOCK_128;
/// 帧固定开销：帧头 + 块号 + 块号补码。
const FRAME_OVERHEAD: usize = 3;

/// CRC-16/XMODEM：多项式 `0x1021`，初值 0，输入输出都不反转，无终值异或。
///
/// 自测向量：`crc16(b"123456789") == 0x31C3`。
pub fn crc16(data: &[u8]) -> u16 {
    let mut crc: u16 = 0;
    for &byte in data {
        crc ^= u16::from(byte) << 8;
        for _ in 0..8 {
            crc = if crc & 0x8000 != 0 {
                (crc << 1) ^ 0x1021
            } else {
                crc << 1
            };
        }
    }
    crc
}

/// 8 位累加和（按字节回绕）。
pub fn sum8(data: &[u8]) -> u8 {
    data.iter().fold(0u8, |acc, &b| acc.wrapping_add(b))
}

/// 帧长由帧头字节决定；非帧头返回 `None`。
pub fn frame_len(header: u8, checksum: crate::config::Checksum) -> Option<usize> {
    let payload = match header {
        SOH => BLOCK_128,
        STX => BLOCK_1024,
        _ => return None,
    };
    Some(FRAME_OVERHEAD + payload + checksum.trailer_len())
}

/// 把一块载荷编码成完整帧。
///
/// `payload` 长度必须正好是 128 或 1024（调用方负责填充），帧头字节由它推出。
pub(crate) fn encode_frame(
    block: u8,
    payload: &[u8],
    checksum: crate::config::Checksum,
) -> Vec<u8> {
    let header = match payload.len() {
        BLOCK_128 => SOH,
        BLOCK_1024 => STX,
        other => panic!("block payload must be 128 or 1024 bytes, got {other}"),
    };
    let mut out = Vec::with_capacity(FRAME_OVERHEAD + payload.len() + checksum.trailer_len());
    out.push(header);
    out.push(block);
    out.push(!block);
    out.extend_from_slice(payload);
    match checksum {
        crate::config::Checksum::Crc16 => {
            let crc = crc16(payload);
            out.push((crc >> 8) as u8);
            out.push((crc & 0x00FF) as u8);
        }
        crate::config::Checksum::Sum8 => out.push(sum8(payload)),
    }
    out
}

/// 一次解析尝试的结果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Parsed {
    /// 字节还不够凑一帧，等下一次 `feed`。
    Incomplete,
    /// 凑够一帧的形状了，但校验不过（块号补码或帧尾）。`consumed` 是应当丢弃的
    /// 字节数——即便校验失败帧长也是确定的，整帧跳过即可重新对齐。
    Invalid { consumed: usize },
    /// 一帧有效数据。
    Valid {
        /// 块号（1..=255，8 位回绕；0 留给 YMODEM 头块）。
        block: u8,
        /// 载荷（128 或 1024 字节，未去填充）。
        payload: Vec<u8>,
        /// 本帧占用的字节数。
        consumed: usize,
    },
}

/// 从缓冲区**头部**解析一帧。
///
/// 调用方保证 `buf[0]` 是 [`SOH`] 或 [`STX`]；其它字节（EOT / CAN / 噪声）由状态机
/// 先行分流。返回 `Incomplete` 时缓冲区原样保留。
pub(crate) fn parse_frame(buf: &[u8], checksum: crate::config::Checksum) -> Parsed {
    let Some(&header) = buf.first() else {
        return Parsed::Incomplete;
    };
    let Some(total) = frame_len(header, checksum) else {
        return Parsed::Incomplete;
    };
    if buf.len() < total {
        return Parsed::Incomplete;
    }
    let payload_len = total - FRAME_OVERHEAD - checksum.trailer_len();
    let block = buf[1];
    let block_check = buf[2];
    let payload = &buf[FRAME_OVERHEAD..FRAME_OVERHEAD + payload_len];
    let trailer_ok = match checksum {
        crate::config::Checksum::Crc16 => {
            let crc_at = FRAME_OVERHEAD + payload_len;
            let want = u16::from_be_bytes([buf[crc_at], buf[crc_at + 1]]);
            crc16(payload) == want
        }
        crate::config::Checksum::Sum8 => sum8(payload) == buf[FRAME_OVERHEAD + payload_len],
    };
    if block != !block_check || !trailer_ok {
        return Parsed::Invalid { consumed: total };
    }
    Parsed::Valid {
        block,
        payload: payload.to_vec(),
        consumed: total,
    }
}

/// YMODEM 头块（块 0）解析成功后的内容。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct FileHeader {
    /// 文件名（不含路径信息；线上是什么就是什么）。
    pub(crate) name: String,
    /// 文件字节数。长度字段为空串时为 `None`（少数发送方不填）。
    pub(crate) size: Option<u64>,
}

/// 解析 YMODEM 头块载荷。
///
/// 线上形态：`name \0 size-ascii \0 [mtime \0 mode \0 serial \0]` 然后 0 填充。
/// 返回 `Ok(None)` 表示**空头块**（载荷首字节就是 `\0`），那是「批次结束」信号，
/// 不是错误。
pub(crate) fn parse_header(payload: &[u8]) -> Result<Option<FileHeader>, String> {
    if payload.is_empty() || payload[0] == 0 {
        return Ok(None);
    }
    let mut parts = payload.split(|&b| b == 0);
    let name_bytes = parts.next().unwrap_or(&[]);
    let name = String::from_utf8_lossy(name_bytes).into_owned();
    if name.is_empty() {
        return Err("文件名为空".to_string());
    }
    let size_field = parts.next().unwrap_or(&[]);
    let size_field = String::from_utf8_lossy(size_field).into_owned();
    let size = if size_field.trim().is_empty() {
        None
    } else {
        match size_field.trim().parse::<u64>() {
            Ok(v) => Some(v),
            Err(_) => return Err(format!("长度字段不是十进制数字：{size_field:?}")),
        }
    };
    Ok(Some(FileHeader { name, size }))
}

/// 构造 YMODEM 头块载荷（固定 128 字节，尾部 0 填充）。
pub(crate) fn build_header_payload(name: &str, size: u64) -> Vec<u8> {
    let mut out = Vec::with_capacity(HEADER_BLOCK_SIZE);
    out.extend_from_slice(name.as_bytes());
    out.push(0);
    out.extend_from_slice(size.to_string().as_bytes());
    out.push(0);
    out.resize(HEADER_BLOCK_SIZE, 0);
    out
}

/// 批次结束用的空头块载荷（全 0）。
pub(crate) fn empty_header_payload() -> Vec<u8> {
    vec![0u8; HEADER_BLOCK_SIZE]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Checksum;

    #[test]
    fn crc16_matches_known_vector() {
        assert_eq!(crc16(b"123456789"), 0x31C3);
        assert_eq!(crc16(b""), 0x0000);
    }

    #[test]
    fn sum8_wraps() {
        assert_eq!(sum8(&[0xFF, 0x02]), 0x01);
        assert_eq!(sum8(&[]), 0x00);
    }

    #[test]
    fn frame_roundtrip_crc() {
        let payload = vec![0x41u8; BLOCK_128];
        let frame = encode_frame(7, &payload, Checksum::Crc16);
        assert_eq!(frame.len(), frame_len(SOH, Checksum::Crc16).unwrap());
        assert_eq!(frame[0], SOH);
        assert_eq!(frame[1], 7);
        assert_eq!(frame[2], !7);
        match parse_frame(&frame, Checksum::Crc16) {
            Parsed::Valid {
                block,
                payload: got,
                consumed,
            } => {
                assert_eq!(block, 7);
                assert_eq!(got, payload);
                assert_eq!(consumed, frame.len());
            }
            other => panic!("expected valid frame, got {other:?}"),
        }
    }

    #[test]
    fn frame_roundtrip_sum8_and_1024() {
        let payload = vec![0x5Au8; BLOCK_1024];
        let frame = encode_frame(255, &payload, Checksum::Sum8);
        assert_eq!(frame[0], STX);
        assert_eq!(frame.len(), frame_len(STX, Checksum::Sum8).unwrap());
        match parse_frame(&frame, Checksum::Sum8) {
            Parsed::Valid { block, payload: got, .. } => {
                assert_eq!(block, 255);
                assert_eq!(got.len(), BLOCK_1024);
            }
            other => panic!("expected valid frame, got {other:?}"),
        }
    }

    #[test]
    fn short_buffer_is_incomplete() {
        let payload = vec![0u8; BLOCK_128];
        let frame = encode_frame(1, &payload, Checksum::Crc16);
        for cut in 0..frame.len() {
            assert_eq!(parse_frame(&frame[..cut], Checksum::Crc16), Parsed::Incomplete);
        }
    }

    #[test]
    fn corrupted_payload_is_invalid() {
        let payload = vec![0x11u8; BLOCK_128];
        let mut frame = encode_frame(1, &payload, Checksum::Crc16);
        frame[10] ^= 0xFF;
        assert_eq!(
            parse_frame(&frame, Checksum::Crc16),
            Parsed::Invalid {
                consumed: frame.len()
            }
        );
    }

    #[test]
    fn corrupted_block_complement_is_invalid() {
        let payload = vec![0x11u8; BLOCK_128];
        let mut frame = encode_frame(3, &payload, Checksum::Crc16);
        frame[2] ^= 0x01;
        assert!(matches!(
            parse_frame(&frame, Checksum::Crc16),
            Parsed::Invalid { .. }
        ));
    }

    #[test]
    fn header_payload_roundtrip() {
        let payload = build_header_payload("firmware.bin", 12345);
        assert_eq!(payload.len(), HEADER_BLOCK_SIZE);
        let parsed = parse_header(&payload).unwrap().unwrap();
        assert_eq!(parsed.name, "firmware.bin");
        assert_eq!(parsed.size, Some(12345));
    }

    #[test]
    fn header_without_size_field() {
        let mut payload = Vec::new();
        payload.extend_from_slice(b"a.txt\0\0");
        payload.resize(HEADER_BLOCK_SIZE, 0);
        let parsed = parse_header(&payload).unwrap().unwrap();
        assert_eq!(parsed.name, "a.txt");
        assert_eq!(parsed.size, None);
    }

    #[test]
    fn empty_header_means_end_of_batch() {
        assert_eq!(parse_header(&empty_header_payload()), Ok(None));
    }

    #[test]
    fn header_with_bad_size_is_error() {
        let mut payload = Vec::new();
        payload.extend_from_slice(b"a.txt\0not-a-number\0");
        payload.resize(HEADER_BLOCK_SIZE, 0);
        assert!(parse_header(&payload).is_err());
    }
}

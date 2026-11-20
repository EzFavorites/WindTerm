//! 协议参数。

use crate::packet;

/// 帧尾巴用哪种校验。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Checksum {
    /// CRC-16/XMODEM（多项式 `0x1021`，初值 0，不反转），2 字节，大端在线上。
    /// 握手字符是 `'C'`（0x43）。
    Crc16,
    /// 8 位累加和，1 字节。握手字符是 `NAK`（0x15）。
    ///
    /// 只在对接上古设备时用：它能检出错误的概率远低于 CRC，`sz`/`rx` 现代版本
    /// 默认都不用它。
    Sum8,
}

impl Checksum {
    /// 帧尾字节数。
    pub fn trailer_len(self) -> usize {
        match self {
            Checksum::Crc16 => 2,
            Checksum::Sum8 => 1,
        }
    }

    /// 接收方用于「请按这种校验发」的握手字符。
    pub fn request_byte(self) -> u8 {
        match self {
            Checksum::Crc16 => packet::CRC_REQUEST,
            Checksum::Sum8 => packet::NAK,
        }
    }
}

/// 传输协议变体。
///
/// 三者的关系是包含式的：YMODEM = XMODEM-1K + 批处理扩展（文件头块 / 多文件 /
/// 结束空头块）；XMODEM-1K = XMODEM + 1024 字节块。所以本枚举既是「用哪套线格式」
/// 也是「功能上多给多少」。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Protocol {
    /// 经典 XMODEM：128 字节块，单文件，**线上不带文件名和长度**。
    Xmodem,
    /// XMODEM-1K：主体用 1024 字节块，尾块自动降为 128 字节（`sz` 的行为）。
    /// 仍然单文件、不带头块。
    Xmodem1k,
    /// YMODEM（即 YMODEM-1K）：块 0 携带「文件名 + 长度」，可连续传多个文件，
    /// 最后一个文件之后用一个空头块宣布批次结束。
    Ymodem,
}

impl Protocol {
    /// 是否为批处理协议（带文件头块 / 多文件）。
    pub fn is_batch(self) -> bool {
        matches!(self, Protocol::Ymodem)
    }

    /// 单个数据块的最大字节数。
    pub fn max_block_size(self) -> usize {
        match self {
            Protocol::Xmodem => packet::BLOCK_128,
            Protocol::Xmodem1k | Protocol::Ymodem => packet::BLOCK_1024,
        }
    }

    /// 给「还剩 `remaining` 字节」挑一个块长。
    ///
    /// 1K 模式下剩余不超过 128 字节时降级为 128 字节帧（SOH）：这既省带宽，也让
    /// 线上的帧长和载荷贴合。**接收方不依赖这条策略**——它一律同时接受 SOH 与
    /// STX，所以对端用别的策略也能收。
    pub(crate) fn block_size_for(self, remaining: usize) -> usize {
        match self {
            Protocol::Xmodem => packet::BLOCK_128,
            Protocol::Xmodem1k | Protocol::Ymodem => {
                if remaining <= packet::BLOCK_128 {
                    packet::BLOCK_128
                } else {
                    packet::BLOCK_1024
                }
            }
        }
    }
}

/// 传输参数。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Config {
    /// 协议变体。
    pub protocol: Protocol,
    /// 校验方式（接收方据此决定握手字符；发送方据此编码帧尾，且会被对端的握手
    /// 字符实时覆盖——见 [`crate::Sender`]）。
    pub checksum: Checksum,
    /// 同一期望上的连续失败上限，超过即放弃（`0` 表示「一次都不许失败」）。
    ///
    /// XMODEM 的经典默认值是 10。
    pub max_retries: u32,
    /// 块尾填充字节，默认 [`crate::packet::SUB`]（0x1A）。少数老系统用 0x00。
    pub padding: u8,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            protocol: Protocol::Xmodem,
            checksum: Checksum::Crc16,
            max_retries: 10,
            padding: packet::SUB,
        }
    }
}

impl Config {
    /// 按协议取一份默认参数（CRC-16、重试上限 10、`SUB` 填充）。
    pub fn for_protocol(protocol: Protocol) -> Self {
        Self {
            protocol,
            ..Self::default()
        }
    }

    /// 覆盖校验方式（链式写法）。
    pub fn with_checksum(mut self, checksum: Checksum) -> Self {
        self.checksum = checksum;
        self
    }

    /// 覆盖重试上限（链式写法）。
    pub fn with_max_retries(mut self, max_retries: u32) -> Self {
        self.max_retries = max_retries;
        self
    }
}

//! 线格式：控制字符、CRC-32、三种帧头、数据子包、ZDLE 转义、ZFILE 文件信息。
//!
//! 这一层不做状态判断，只做「字节 ↔ 结构」。状态机（[`crate::Receiver`] /
//! [`crate::Sender`]）负责决定下一步发什么。
//!
//! # 一帧的两层
//!
//! ZMODEM 的帧由**帧头**和（可选的）**数据子包**拼成：
//!
//! 1. **帧头**（[`encode_header`] / [`parse_header`]）：1 字节类型 + 4 字节头数据
//!    + CRC。三种线格式，**解析时三种全收**，与 [`crate::Config::header`] 无关：
//!
//!    ```text
//!    ZHEX    *  *  ZDLE 'B'  <14 个十六进制字符>       0D 8A [XON]  20/21 字节
//!    ZBIN    *  ZDLE 'A'     type f0 f1 f2 f3 crc16(2)             10 字节 + 转义
//!    ZBIN32  *  ZDLE 'C'     type f0 f1 f2 f3 crc32(4)             12 字节 + 转义
//!    ```
//!
//!    `ZHEX` 的终止符是 `0D 8A`（不是 `0D 0A`），且除 `ZACK` / `ZFIN` 之外还要
//!    再跟一个裸的 `XON`——见 [`encode_hex_header`] 与模块文档末尾「线上事实」。
//!
//!    注意三种帧头的**类型字段都是 1 字节**——ZBIN32 与 ZBIN 只差指示符与 CRC
//!    宽度，不差类型字段宽度（Forsberg 协议描述 §7.3.1/7.3.2，`lrzsz` 的
//!    `zgethdr` 里两种形状都是 5 字节「类型 + 4 字节头」）。
//!
//!    二进制帧头的类型字节、4 字节头数据、CRC 都按 ZDLE 规则转义；所以像
//!    `ZCRC`（类型 13 = 0x0D）、`ZCAN`（16 = 0x10）这类类型字节在线上一定带转义。
//! 2. **数据子包**（[`encode_subpacket`] / [`parse_subpacket`]）：变长、逐字节
//!    转义、CRC、四种帧尾。只有 `ZDATA` / `ZFILE` / `ZSINIT` 这类帧头后面才跟
//!    子包，其余帧头后面直接是下一个帧头。
//!
//!    **子包的 CRC 宽度跟帧头走**：`ZHEX`/`ZBIN` 后面是 CRC-16，`ZBIN32` 后面是
//!    CRC-32（规范原文：「If a data packet follows a HEX header, it is protected
//!    with CRC-16」）。真实 `rz`/`sz` 按收到的帧头类型决定子包校验宽度，不跟着
//!    走会直接判帧错。
//!
//! # 转义
//!
//! 子包（以及二进制帧头）里的数据字节若落在 {`CR`, `DLE`, `XON`, `XOFF`, `ZDLE`}
//! **以及它们的高位变体** {`CR|0x80`, `DLE|0x80`, `XON|0x80`, `XOFF|0x80`} 里，
//! 写成 `ZDLE` + (`原字节 ^ 0x40`)。规则见 [`needs_escape`]。
//!
//! 高位变体**必须**转义，虽然它们看起来「不危险」：`lrzsz` 的 `zdlread2()`
//! （`zm.c:190-200`）把裸的 `XON|0200`（0x91）与 `XOFF|0200`（0x93）当成流控字符
//! **直接吃掉**，于是解出来的字节流整体错位，对端只会报 `Bad CRC`。裸的 `0x8D`
//! （`CR|0x80`）更糟：它落到 `zdlread2()` 的 `default` 分支后既不满足
//! `(c & 0140) == 0100` 也不是流控，直接判 `Bad escape sequence`。
//!
//! **多转义是合法的**（解码方只需无脑异或），所以本模块一律转义这几个字节，
//! 不做「能省则省」的优化。
//!
//! 四种帧尾（`ZCRCE` `ZCRCG` `ZCRCQ` `ZCRCW`）都跟在 `ZDLE` 后面，且**不**异或。
//! 它们的取值是 `h`/`i`/`j`/`k`（0x68..0x6B），与被转义的五个字节的编码
//! （0x4D/0x50/0x51/0x53/0x58）不重叠，所以「`ZDLE` 后面是什么」可以唯一判定。
//!
//! # CRC
//!
//! - 帧头：`ZHEX`/`ZBIN` 用 CRC-16/XMODEM，直接复用 [`term_xfer::packet::crc16`]；
//!   `ZBIN32` 用 CRC-32。
//! - 数据子包：按帧头类型选 CRC-16 或 CRC-32。
//! - CRC-32 是多项式 `0x04C1_1DB7` 的**反射**形式 `0xEDB8_8320`，初值 `0xFFFF_FFFF`，
//!   末尾异或 `0xFFFF_FFFF`——与 zlib / PNG 完全相同。已知向量：
//!   `crc32(b"123456789") == 0xCBF4_3926`。
//!
//! # 从真联调学到的线上事实（`lrzsz` 0.13.1）
//!
//! 下面每一条都是被真机 `sz` / `rz` 打回来之后，回到 `lrzsz` 源码与真机字节上复核
//! 出来的。它们不是「文档可选项」，写错任何一条都会得到 `Bad CRC` / 对端无限重发。
//!
//! | 线上事实 | 源码位置 |
//! | --- | --- |
//! | `ZHEX` 终止符是 `0D 8A`；除 `ZACK`/`ZFIN` 外再跟一个裸 `XON` | `zshhdr()` `zm.c:409-418` |
//! | 帧头 4 字节**位置字段是小端**（`ZP0=0` 是最低字节） | `zmodem.h:70-74`、`stohdr()` `zm.c:959`、`rclhdr()` `zm.c:970` |
//! | `ZRINIT` 能力位在头数据的**第 4 字节**（`ZF0 == 3`），缓冲大小在前两字节（小端） | `#define ZF0 3` `zmodem.h`、`lrz.c:1648-1664`、`lsz.c:1621` |
//! | `ZFILE` 头数据的第 4 字节是转换选项 `ZF0`（`ZCBIN == 1`） | `zmodem.h` 的 `ZF0`/`ZCBIN`、`lsz.c:1745-1753` |
//! | 帧头 CRC-16 覆盖「类型 + 4 字节头」，**不加**零字节（表法等价于补 16 个 0 位） | `zsbhdr()` `zm.c:344-353` |
//! | 子包 CRC 覆盖「载荷 + 帧尾字节」，`ZCRCW` 收尾后**立刻补一个 XON** | `zsdata()` `zm.c:428-447` |
//! | 裸的 `0x91` / `0x93` 被当流控**静默吃掉**；裸的 `0x8D` 判 `Bad escape sequence` | `zdlread2()` `zm.c:180-247` |
//! | 接收端读 14 个 hex 后只读 1 字节，是 `0D`/`0x8D` 就再丢 1 字节 | `zrhhdr()` `zm.c:869-878` |
//!
//! 反过来的结论也很重要：**子包 CRC 的字节序我们本来就对**（CRC-16 高字节在前、
//! CRC-32 低字节在前），帧头 CRC 也本来就对——用真机 `sz` 的三处帧头/子包反算是
//! `0x9927` / `0x46ae` / `0x0b9d`，与我们的实现逐字节一致。所以那两条 CRITICAL 不是
//! 校验算法的问题。
//!
//! # 外部输入的两道硬上限
//!
//! 状态机的输入缓冲是外部字节驱动的分配，必须有上限（否则一个不结束的帧就能把
//! 内存吃干）：
//!
//! - [`MAX_ZPAD_RUN`]：帧头前连续 `ZPAD` 的上限。合法发送方最多打两三个。
//! - [`MAX_BUFFER`]：一帧解析缓冲的上限。取 `2 × (MAX_BLOCK_SIZE + 32)`，正好
//!   大于「最坏合法帧」——8 KiB 载荷全部要转义时 16 KiB，加帧头 20 字节、
//!   帧尾 2 字节、转义后的 CRC 8 字节，共 16414 字节。

use crate::config::HeaderKind;

pub use term_xfer::packet::crc16;

/// 帧头填充符 `'*'`。帧头前可以出现任意多个（本模块用 [`MAX_ZPAD_RUN`] 兜住）。
pub const ZPAD: u8 = 0x2A;
/// 转义符（同时也是 `CAN`）：转义序列与中止信号都从它开始。
pub const ZDLE: u8 = 0x18;
/// 十六进制帧头指示符 `'B'`。
pub const ZHEX: u8 = b'B';
/// 16 位 CRC 二进制帧头指示符 `'A'`。
pub const ZBIN: u8 = b'A';
/// 32 位 CRC 二进制帧头指示符 `'C'`。
pub const ZBIN32: u8 = b'C';
/// 回车。
pub const CR: u8 = 0x0D;
/// 换行。
pub const LF: u8 = 0x0A;
/// `CR` 的高位变体（`CR | 0x80`）。**必须转义**，理由见模块文档「转义」。
pub const CR_HIGH: u8 = 0x8D;
/// `LF` 的高位变体。`lrzsz` 的 `zshhdr()`（`zm.c:409-418`）用它做 `ZHEX` 帧尾的
/// 第二个字节：线上是 `0D 8A`，**不是** `0D 0A`。
pub const LF_HIGH: u8 = 0x8A;
/// 数据链路层设备控制 1（`Ctrl-P`），必须转义。
pub const DLE: u8 = 0x10;
/// `DLE` 的高位变体。`lrzsz` 的 `zsendline_init()`（`zm.c:933-940`）恒转义它。
pub const DLE_HIGH: u8 = 0x90;
/// 继续发送（`Ctrl-Q`），必须转义。
pub const XON: u8 = 0x11;
/// `XON` 的高位变体。`lrzsz` 的 `zdlread2()` 会**静默吃掉**裸的它。
pub const XON_HIGH: u8 = 0x91;
/// 暂停发送（`Ctrl-S`），必须转义。
pub const XOFF: u8 = 0x13;
/// `XOFF` 的高位变体。`lrzsz` 的 `zdlread2()` 会**静默吃掉**裸的它。
pub const XOFF_HIGH: u8 = 0x93;
/// 中止信号。与 [`ZDLE`] 同值，但语义不同：连发 [`ABORT_CAN_COUNT`] 个表示放弃。
pub const CAN: u8 = 0x18;

/// 子包帧尾：本帧结束，CRC 跟在后面，不需要应答。
pub const ZCRCE: u8 = b'h';
/// 子包帧尾：本帧还有后续子包，不需要应答。
pub const ZCRCG: u8 = b'i';
/// 子包帧尾：本帧还有后续子包，但请回一个 `ZACK`。
pub const ZCRCQ: u8 = b'j';
/// 子包帧尾：本帧结束，等待应答（`ZACK` / `ZRPOS`）。停等模式的发送方用它。
pub const ZCRCW: u8 = b'k';

/// 连收多少个 `CAN` 才算对端中止。`lrzsz` 用 5 个。
pub const ABORT_CAN_COUNT: usize = 5;

/// 本端放弃时一次性甩出的中止序列。`lrzsz` 发 8 个，比 5 个更难被噪声过滤掉。
pub const ABORT_SEQUENCE: [u8; 8] = [CAN; 8];

/// 帧头前连续 `ZPAD` 的上限。合法发送方最多打两三个，4096 已经极度宽裕。
pub const MAX_ZPAD_RUN: usize = 4096;

/// 单个帧（含子包）的解析缓冲上限。
///
/// `2 × (8192 + 32) = 16448`，大于最坏合法帧 16414 字节（见模块文档）。
pub const MAX_BUFFER: usize = 2 * (crate::config::MAX_BLOCK_SIZE + 32);

/// 接收方在 `ZRINIT` 里宣告的能力位：`CANFC32`（支持 CRC-32）。
pub const RINIT_CANFC32: u8 = 0x20;
/// 接收方在 `ZRINIT` 里宣告的能力位：`ESCCTL`（能处理控制字符转义）。
pub const RINIT_ESCCTL: u8 = 0x40;

/// 本实现的接收方宣告的能力位。
///
/// **故意不带 `CANFDX`（0x01，全双工）**：发送方看到它不是全双工就会用 `ZCRCW`
/// 停等（每块等一个 `ZACK`），这是默认路径；带上了会诱导真实对端改用 `ZCRCG`
/// 连发。连发本身我们已经能解析（[`crate::Config::streaming`]），但默认不收——
/// 它的错误恢复粒度是整段回退，值不值得由调用方决定。
pub const RINIT_FLAGS: u8 = RINIT_CANFC32 | RINIT_ESCCTL;

/// `ZRINIT` 里宣告的接收缓冲大小。0 表示「不限制」，给个具体值更利于对端流控。
pub const RINIT_BUFFER: u16 = 8192;

/// 数据子包用哪种 CRC（由它前面那个帧头的类型决定，见 [`HeaderKind::subpacket_crc`]）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum CrcKind {
    /// CRC-16/XMODEM，2 字节，**高字节在前**。
    Crc16,
    /// CRC-32（反射，初值/终值异或 `0xFFFF_FFFF`），4 字节，**低字节在前**。
    ///
    /// 字节序是照 `lrzsz` 的 `zsbh32()` / `zsda32()` 抄的：它们都是 `crc >>= 8`
    /// 逐字节发，也就是小端。写成大端会被真实 `rz`/`sz` 判「Bad CRC」。
    Crc32,
}

impl CrcKind {
    /// CRC 字段的字节数（也即校验强度：16 位 2 字节、32 位 4 字节）。
    pub const fn width(self) -> usize {
        match self {
            CrcKind::Crc16 => 2,
            CrcKind::Crc32 => 4,
        }
    }
}

/// **数据子包**的 CRC-16：载荷 + 帧尾字节一起算（`lrzsz` 的 `zsdata()` 里
/// `crc = updcrc(frameend, crc)`）。
///
/// 帧尾参与校验不是可选项：真实 `sz` 就是这么发的，漏掉它会让每个 `ZFILE` /
/// `ZDATA` 都被判校验错（第一次真联调就是这么卡住的）。
fn subpacket_crc16(data: &[u8], end: FrameEnd) -> u16 {
    crc16_update(crc16_update(0, data), &[end.as_byte()])
}

/// **数据子包**的 CRC-32：载荷 + 帧尾字节，初值 `0xFFFF_FFFF`，结果取反。
fn subpacket_crc32(data: &[u8], end: FrameEnd) -> u32 {
    crc32_update(crc32_update(0xFFFF_FFFF, data), &[end.as_byte()]) ^ 0xFFFF_FFFF
}

/// ZMODEM 帧类型。
///
/// 编号照 `lrzsz` 的 `zmodem.h`（与 Forsberg 的协议描述一致）。十六进制帧头里以
/// 两个十六进制字符编码，二进制帧头里是 1 个（可能被转义的）字节。
#[allow(non_camel_case_types, clippy::upper_case_acronyms)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum FrameType {
    /// 发送方请求接收方发送 `ZRINIT`（会话的第一帧）。
    ZRQINIT = 0,
    /// 接收方就绪：宣告能力位与缓冲大小。
    ZRINIT = 1,
    /// 发送方→接收方：注意序列（`ZSINIT`）。本轮只做「收到就跳过」。
    ZSINIT = 2,
    /// 确认：头数据里的 4 字节是「已收到的偏移」。
    ZACK = 3,
    /// 文件信息：头数据 4 字节全 0，文件名/长度/mtime 在随后的数据子包里。
    ZFILE = 4,
    /// 接收方拒绝接收该文件。
    ZSKIP = 5,
    /// 否定应答。
    ZNAK = 6,
    /// 对端中止。
    ZABORT = 7,
    /// 收尾握手。
    ZFIN = 8,
    /// 请从指定偏移重发。
    ZRPOS = 9,
    /// 数据块：头数据 4 字节是块起始偏移，载荷在随后（带转义）的子包里。
    ZDATA = 10,
    /// 文件结束：头数据 4 字节是文件总长度。
    ZEOF = 11,
    /// 对端报告文件错误（打不开、写不进）。
    ZFERR = 12,
    /// 对端请求某段数据的 CRC。
    ZCRC = 13,
    /// 挑战应答（很少用）。
    ZCHALLENGE = 14,
    /// 全部文件传完。
    ZCOMPL = 15,
    /// 对端取消。
    ZCAN = 16,
    /// 剩余空间查询。
    ZFREECNT = 17,
    /// 远端命令。
    ZCOMMAND = 18,
    /// 远端标准错误输出。
    ZSTDERR = 19,
}

impl FrameType {
    /// 线上的帧类型字节。
    pub const fn as_byte(self) -> u8 {
        self as u8
    }

    /// 从帧类型字节还原。未知值返回 `None`。
    pub const fn from_byte(byte: u8) -> Option<Self> {
        Some(match byte {
            0 => FrameType::ZRQINIT,
            1 => FrameType::ZRINIT,
            2 => FrameType::ZSINIT,
            3 => FrameType::ZACK,
            4 => FrameType::ZFILE,
            5 => FrameType::ZSKIP,
            6 => FrameType::ZNAK,
            7 => FrameType::ZABORT,
            8 => FrameType::ZFIN,
            9 => FrameType::ZRPOS,
            10 => FrameType::ZDATA,
            11 => FrameType::ZEOF,
            12 => FrameType::ZFERR,
            13 => FrameType::ZCRC,
            14 => FrameType::ZCHALLENGE,
            15 => FrameType::ZCOMPL,
            16 => FrameType::ZCAN,
            17 => FrameType::ZFREECNT,
            18 => FrameType::ZCOMMAND,
            19 => FrameType::ZSTDERR,
            _ => return None,
        })
    }

    /// 这类帧能否用二进制帧头（`ZBIN` / `ZBIN32`）。
    ///
    /// 只有**数据段**的帧可以：`ZFILE`（后面跟文件信息子包）、`ZDATA`（载荷子包）、
    /// `ZEOF`、`ZSINIT`（注意序列子包）。其余都是握手/控制帧，必须用可打印的
    /// `ZHEX`——握手发生在双方还不确定链路是否 8-bit clean 的时候，这也是真实
    /// `sz`/`rz` 的行为（`ZRQINIT`、`ZRINIT`、`ZACK`、`ZRPOS`、`ZFIN` 全是 ZHEX）。
    pub const fn is_data_frame(self) -> bool {
        matches!(
            self,
            FrameType::ZFILE | FrameType::ZDATA | FrameType::ZEOF | FrameType::ZSINIT
        )
    }

    /// 按「帧类别」决定帧头：数据段用 `data`（协商结果），控制帧永远 `ZHEX`。
    pub const fn header_kind(self, data: HeaderKind) -> HeaderKind {
        if self.is_data_frame() {
            data
        } else {
            HeaderKind::Hex
        }
    }
}

/// 数据子包的四种帧尾。
#[allow(non_camel_case_types, clippy::upper_case_acronyms)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum FrameEnd {
    /// `ZCRCE`：本帧结束，CRC 跟在后面，不需要应答。
    ZCRCE,
    /// `ZCRCG`：本帧还有后续子包，不需要应答。
    ZCRCG,
    /// `ZCRCQ`：本帧还有后续子包，请回一个 `ZACK`。
    ZCRCQ,
    /// `ZCRCW`：本帧结束，等待应答。停等模式的发送方用它；连发时只用在最后一块。
    ZCRCW,
}

impl FrameEnd {
    /// 帧尾字节（跟在 `ZDLE` 后面，不异或）。
    pub const fn as_byte(self) -> u8 {
        match self {
            FrameEnd::ZCRCE => ZCRCE,
            FrameEnd::ZCRCG => ZCRCG,
            FrameEnd::ZCRCQ => ZCRCQ,
            FrameEnd::ZCRCW => ZCRCW,
        }
    }

    /// 从 `ZDLE` 后面的那个字节还原帧尾。不是四种之一返回 `None`（那是一个转义字节）。
    pub const fn from_byte(byte: u8) -> Option<Self> {
        Some(match byte {
            ZCRCE => FrameEnd::ZCRCE,
            ZCRCG => FrameEnd::ZCRCG,
            ZCRCQ => FrameEnd::ZCRCQ,
            ZCRCW => FrameEnd::ZCRCW,
            _ => return None,
        })
    }

    /// 是否要求对端回 `ZACK`。
    pub const fn wants_ack(self) -> bool {
        matches!(self, FrameEnd::ZCRCW | FrameEnd::ZCRCQ)
    }
}

/// CRC-32 查表（反射多项式 `0xEDB8_8320`，编译期生成）。
const CRC32_TABLE: [u32; 256] = build_crc32_table();

const fn build_crc32_table() -> [u32; 256] {
    let mut table = [0u32; 256];
    let mut i = 0;
    while i < 256 {
        let mut crc = i as u32;
        let mut bit = 0;
        while bit < 8 {
            crc = if crc & 1 != 0 {
                (crc >> 1) ^ 0xEDB8_8320
            } else {
                crc >> 1
            };
            bit += 1;
        }
        table[i] = crc;
        i += 1;
    }
    table
}

/// 推进 CRC-16/XMODEM 寄存器（初值 `0`，与 [`crc16`] 同一算法）。
///
/// 拆出这个函数是因为帧尾字节也要参与数据子包的校验，但 `crc16` 只接受完整切片。
fn crc16_update(mut crc: u16, bytes: &[u8]) -> u16 {
    for &byte in bytes {
        crc ^= u16::from(byte) << 8;
        let mut bit = 0;
        while bit < 8 {
            crc = if crc & 0x8000 != 0 {
                (crc << 1) ^ 0x1021
            } else {
                crc << 1
            };
            bit += 1;
        }
    }
    crc
}

/// 推进 CRC-32 寄存器（返回**未取反**的中间值，初值应为 `0xFFFF_FFFF`）。
fn crc32_update(mut crc: u32, bytes: &[u8]) -> u32 {
    for &byte in bytes {
        let index = ((crc ^ u32::from(byte)) & 0xFF) as usize;
        crc = (crc >> 8) ^ CRC32_TABLE[index];
    }
    crc
}

/// CRC-32（反射，多项式 `0xEDB8_8320`，初值/终值异或均为 `0xFFFF_FFFF`）。
///
/// 与 zlib / PNG / Ethernet 的 CRC-32 相同。ZMODEM 的 `ZBIN32` 帧头与
/// `ZBIN32` 后面的数据子包用的就是它，但**线上字节序是小端**（见 [`CrcKind`]）。
///
/// 自测向量：
///
/// ```
/// assert_eq!(term_zmodem::frame::crc32(b"123456789"), 0xCBF4_3926);
/// assert_eq!(term_zmodem::frame::crc32(b""), 0);
/// ```
pub fn crc32(data: &[u8]) -> u32 {
    crc32_update(0xFFFF_FFFF, data) ^ 0xFFFF_FFFF
}

/// 数据流里可以被**静默丢弃**的流控字节：`XON` / `XOFF` 及其高位变体。
///
/// `lrzsz` 的 `zdlread2()`（`zm.c:200-207`）在子包里遇到这四个值一律 `goto again`
/// 跳过——它们只可能是链路上的流控噪声，不可能是载荷（合法发送方会用 `ZDLE`
/// 转义载荷里的这四个字节）。
///
/// 本实现照做有两个理由：一是与真实实现对偶（对端不转义时也能活），二是
/// [`encode_hex_header`] 给非 `ZACK`/`ZFIN` 帧加的 `XON` 尾巴在**逐字节喂入**时
/// 会比帧头晚一批到达，只能在子包入口处吸收。
pub const fn is_flow_control(byte: u8) -> bool {
    matches!(byte, XON | XON_HIGH | XOFF | XOFF_HIGH)
}

/// 子包里的这个字节是否需要转义。
///
/// 集合照 `lrzsz` 的 `zsendline_init()`（`zm.c:914-954`）来：`ZDLE` / `XON` /
/// `XOFF` / `DLE` 及其 `|0x80` 变体恒转义；`CR` 与 `CR|0x80` 在 `Zctlesc` 打开时
/// 恒转义、关闭时「仅当上一字节是 `'@'`」才转义——那是 Telenet 网络转义的遗留，
/// 本实现对它们**一律转义**（多转义对解码方无害，少转义会直接毁掉整帧）。
///
/// 高位变体不能省：裸的 `0x91` / `0x93` 会被 `zdlread2()` 当流控吃掉，裸的
/// `0x8D` 会被它判 `Bad escape sequence`。
pub fn needs_escape(byte: u8) -> bool {
    matches!(
        byte,
        CR | CR_HIGH | DLE | DLE_HIGH | XON | XON_HIGH | XOFF | XOFF_HIGH | ZDLE
    )
}

/// 往 `out` 追加一个（必要时转义的）子包数据字节。
pub fn push_escaped(out: &mut Vec<u8>, byte: u8) {
    if needs_escape(byte) {
        out.push(ZDLE);
        out.push(byte ^ 0x40);
    } else {
        out.push(byte);
    }
}

fn push_hex(out: &mut Vec<u8>, byte: u8) {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    out.push(DIGITS[(byte >> 4) as usize]);
    out.push(DIGITS[(byte & 0x0F) as usize]);
}

fn hex_nibble(byte: u8) -> Option<u8> {
    Some(match byte {
        b'0'..=b'9' => byte - b'0',
        b'a'..=b'f' => byte - b'a' + 10,
        b'A'..=b'F' => byte - b'A' + 10,
        _ => return None,
    })
}

fn decode_hex_field(field: &[u8], out: &mut [u8]) -> bool {
    if field.len() != out.len() * 2 {
        return false;
    }
    for (slot, pair) in out.iter_mut().zip(field.chunks_exact(2)) {
        let Some(high) = hex_nibble(pair[0]) else {
            return false;
        };
        let Some(low) = hex_nibble(pair[1]) else {
            return false;
        };
        *slot = (high << 4) | low;
    }
    true
}

/// 编码一个帧头，线格式由 `kind` 决定。
///
/// - `Hex`：20 字节（`ZACK`/`ZFIN`）或 21 字节（多一个 `XON` 尾巴）；
/// - `Bin`：10 字节（不含转义膨胀）；
/// - `Bin32`：12 字节（不含转义膨胀）。
pub fn encode_header(kind: HeaderKind, frame: FrameType, data: [u8; 4]) -> Vec<u8> {
    match kind {
        HeaderKind::Hex => encode_hex_header(frame, data),
        HeaderKind::Bin => encode_binary_header(false, frame, data),
        HeaderKind::Bin32 => encode_binary_header(true, frame, data),
    }
}

/// 编码一个十六进制帧头（[`HeaderKind::Hex`] 的特例，保留给需要固定形状的调用方）。
///
/// 输出 `ZPAD ZPAD ZDLE 'B'` + 14 个十六进制字符 + `CR 0x8A`，并且**除
/// `ZACK` / `ZFIN` 之外**再跟一个裸的 `XON`，共 21 字节（`ZACK` / `ZFIN` 20 字节）。
///
/// 三处细节都是照 `lrzsz` 的 `zshhdr()`（`zm.c:382-420`）抄的：
///
/// 1. 终止符是 `015 0212`——第二个字节是 `LF|0x80`（0x8A）。写成 `0D 0A` 也能被
///    `zrhhdr()` 吃下（它只要求「第一个字节是 `CR` 或 `0x8D` 就再丢 1 字节」），
///    但没理由跟真实实现对不上。
/// 2. `XON` 尾巴的作用是「把被假 `XOFF` 掐住的远端重新放行」。`ZACK` / `ZFIN` 是
///    回应帧，远端本来就在等它们，所以不加。
/// 3. 帧头里的 4 字节数据在**被覆盖的语义**上各有讲究（位置是小端、`ZRINIT` 的
///    能力位在最后一字节），见 [`encode_position`] / [`rinit_header_data`]。
pub fn encode_hex_header(frame: FrameType, data: [u8; 4]) -> Vec<u8> {
    let mut out = Vec::with_capacity(21);
    out.extend_from_slice(&[ZPAD, ZPAD, ZDLE, ZHEX]);

    let mut crc_input = [0u8; 5];
    crc_input[0] = frame.as_byte();
    crc_input[1..].copy_from_slice(&data);
    let crc = crc16(&crc_input);

    push_hex(&mut out, frame.as_byte());
    for byte in data {
        push_hex(&mut out, byte);
    }
    push_hex(&mut out, (crc >> 8) as u8);
    push_hex(&mut out, (crc & 0x00FF) as u8);
    out.extend_from_slice(&[CR, LF_HIGH]);
    if !matches!(frame, FrameType::ZFIN | FrameType::ZACK) {
        out.push(XON);
    }
    out
}

/// 编码一个二进制帧头：`ZPAD ZDLE 'A'|'C'` + 类型 + 4 字节头 + CRC，逐字节转义。
fn encode_binary_header(bin32: bool, frame: FrameType, data: [u8; 4]) -> Vec<u8> {
    let mut out = Vec::with_capacity(12);
    out.push(ZPAD);
    out.push(ZDLE);
    out.push(if bin32 { ZBIN32 } else { ZBIN });

    let mut covered = [0u8; 5];
    covered[0] = frame.as_byte();
    covered[1..].copy_from_slice(&data);
    for &byte in &covered {
        push_escaped(&mut out, byte);
    }
    if bin32 {
        // lrzsz 的 zsbh32() 是 `crc >>= 8` 逐字节发：**低字节在前**。
        let crc = crc32(&covered);
        for shift in [0, 8, 16, 24] {
            push_escaped(&mut out, (crc >> shift) as u8);
        }
    } else {
        let crc = crc16(&covered);
        push_escaped(&mut out, (crc >> 8) as u8);
        push_escaped(&mut out, (crc & 0x00FF) as u8);
    }
    out
}

/// 解出来的一个帧头。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Header {
    /// 线格式（`Hex` / `Bin` / `Bin32`）。它决定后面数据子包用哪种 CRC。
    pub kind: HeaderKind,
    /// 帧类型。
    pub frame: FrameType,
    /// 帧头携带的 4 字节数据。
    pub data: [u8; 4],
    /// 本帧头在输入里占用的字节数（含前导 `ZPAD`、尾部 `0D 8A` 与可选 `XON`、转义字节）。
    pub consumed: usize,
}

/// [`parse_header`] 的结果。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ParsedHeader {
    /// 字节还不够凑一个帧头，等下一次 `feed`。
    Incomplete,
    /// 这段字节不是（或不是一个完整的）合法帧头。`consumed` 是建议丢弃的字节数，
    /// 恒 `>= 1`，调用方照它丢即可重新对齐。
    Invalid {
        /// 建议丢弃的字节数。
        consumed: usize,
    },
    /// 一个校验通过的帧头。
    Valid(Header),
}

/// 从缓冲区**头部**解析一个帧头（三种线格式全收）。
///
/// - 帧头前允许有任意多个 `ZPAD`（0x2A），全部吃掉；超过 [`MAX_ZPAD_RUN`] 时
///   调用方（`crate::scan::Scanner`，crate 内部）会判洪泛，不在这里处理；
/// - `ZDLE` 后面按指示符 `'B'` / `'A'` / `'C'` 分别走十六进制 / 二进制 16 位 /
///   二进制 32 位；
/// - 不是 `ZPAD` / `ZDLE` 开头的字节按噪声丢弃（`Invalid { consumed: 1 }`）。
///
/// # 十六进制帧头的一个字节前瞻
///
/// `ZHEX` 的终止符 `0D 8A` 紧跟在 14 个十六进制字符后面。解析器要求「14 个字符
/// 之后还能看到一个字节」，只看到 `CR`（或 `0x8D`）时还要再等一个字节确认终止符的
/// 第二个字节。否则逐字节喂入时帧头一到第 18 字节就被判完整，随后到达的 `0D 8A`
/// 会被子包解析器当成载荷的前两个字节。二进制帧头没有终止符，只需要凑齐固定字节数。
///
/// 终止符后面的 `XON` 是**可选**的，所以这里不因为它缺失而返回 `Incomplete`——
/// 对端若不带这个尾巴就永远不会再发字节，等下去就是死锁。逐字节喂入时那个 `XON`
/// 会比帧头晚一批到达，由 [`crate::Receiver`] 在子包入口用
/// [`is_flow_control`] 吸收（`lrzsz` 的 `zdlread2()` 也是这个行为）。
///
/// 返回 `Incomplete` 时调用方应保留缓冲区原样，等下一批字节。
pub fn parse_header(buf: &[u8]) -> ParsedHeader {
    if buf.is_empty() {
        return ParsedHeader::Incomplete;
    }
    let mut zpad = 0;
    while zpad < buf.len() && buf[zpad] == ZPAD {
        zpad += 1;
    }
    if zpad == buf.len() {
        // 全是 ZPAD：也许下一批字节里才有 ZDLE。
        return ParsedHeader::Incomplete;
    }
    if buf[zpad] != ZDLE {
        return ParsedHeader::Invalid {
            consumed: zpad + 1,
        };
    }
    let Some(&indicator) = buf.get(zpad + 1) else {
        return ParsedHeader::Incomplete;
    };
    match indicator {
        ZHEX => parse_hex_body(buf, zpad),
        ZBIN => parse_binary_body(buf, zpad, false),
        ZBIN32 => parse_binary_body(buf, zpad, true),
        _ => ParsedHeader::Invalid {
            consumed: zpad + 1,
        },
    }
}

fn parse_hex_body(buf: &[u8], zpad: usize) -> ParsedHeader {
    let body = zpad + 2;
    const FIELD_LEN: usize = 14; // type(2) + data(8) + crc(4)
    let end = body + FIELD_LEN;
    if buf.len() <= end {
        // `<=` 不是 `<`：见 [`parse_header`] 的「一个字节前瞻」。
        return ParsedHeader::Incomplete;
    }
    let mut fields = [0u8; 7];
    if !decode_hex_field(&buf[body..end], &mut fields) {
        return ParsedHeader::Invalid { consumed: end };
    }
    let stated = u16::from_be_bytes([fields[5], fields[6]]);
    let mut crc_input = [0u8; 5];
    crc_input.copy_from_slice(&fields[..5]);
    if crc16(&crc_input) != stated {
        return ParsedHeader::Invalid { consumed: end };
    }
    let Some(frame) = FrameType::from_byte(fields[0]) else {
        return ParsedHeader::Invalid { consumed: end };
    };

    let mut consumed = end;
    // 终止符：`lrzsz` 的 `zrhhdr()`（`zm.c:869-878`）在 14 个 hex 字符之后**读 1
    // 字节**，若它是 `0215`（0x8D）或 `015`（0x0D）就再丢 1 字节。所以 `0D 8A`、
    // `0D 0A`、`8D 0A` 都要吃，而且「只读到 CR」时得再等一个字节，否则终止符的第
    // 二个字节会漏进后面的子包。
    if matches!(buf[consumed], CR | CR_HIGH) {
        if buf.len() == end + 1 {
            // 只到了 `CR` / `0x8D`，终止符的第二个字节也许在下一批里。
            return ParsedHeader::Incomplete;
        }
        consumed += 1;
        if matches!(buf[consumed], LF | LF_HIGH) {
            consumed += 1;
        }
    }
    // 帧头后面可能跟一个流控 `XON`（`zshhdr()` 对非 `ZACK`/`ZFIN` 帧恒发）。
    // 注意不能顺手吃掉 `NUL`：`ZFILE` 的子包载荷完全可能是以 `NUL` 开头的
    // （文件名为空时的第一个字节），吃了就丢数据。
    if consumed < buf.len() && buf[consumed] == XON {
        consumed += 1;
    }
    ParsedHeader::Valid(Header {
        kind: HeaderKind::Hex,
        frame,
        data: [fields[1], fields[2], fields[3], fields[4]],
        consumed,
    })
}

/// 读一个「可能被转义」的字节（二进制帧头用；这里没有帧尾概念）。
fn read_header_byte(buf: &[u8], at: usize) -> Option<(u8, usize)> {
    let &byte = buf.get(at)?;
    if byte != ZDLE {
        return Some((byte, 1));
    }
    let &next = buf.get(at + 1)?;
    Some((next ^ 0x40, 2))
}

fn parse_binary_body(buf: &[u8], zpad: usize, bin32: bool) -> ParsedHeader {
    let mut cursor = zpad + 2;
    let mut covered = [0u8; 5];
    for slot in covered.iter_mut() {
        let Some((byte, used)) = read_header_byte(buf, cursor) else {
            return ParsedHeader::Incomplete;
        };
        *slot = byte;
        cursor += used;
    }
    let crc_kind = if bin32 { CrcKind::Crc32 } else { CrcKind::Crc16 };
    let mut crc_bytes = [0u8; 4];
    for slot in crc_bytes.iter_mut().take(crc_kind.width()) {
        let Some((byte, used)) = read_header_byte(buf, cursor) else {
            return ParsedHeader::Incomplete;
        };
        *slot = byte;
        cursor += used;
    }
    let crc_ok = match crc_kind {
        CrcKind::Crc16 => {
            let stated = u16::from_be_bytes([crc_bytes[0], crc_bytes[1]]);
            crc16(&covered) == stated
        }
        CrcKind::Crc32 => crc32(&covered) == u32::from_le_bytes(crc_bytes),
    };
    if !crc_ok {
        return ParsedHeader::Invalid { consumed: cursor };
    }
    let Some(frame) = FrameType::from_byte(covered[0]) else {
        return ParsedHeader::Invalid { consumed: cursor };
    };
    ParsedHeader::Valid(Header {
        kind: if bin32 {
            HeaderKind::Bin32
        } else {
            HeaderKind::Bin
        },
        frame,
        data: [covered[1], covered[2], covered[3], covered[4]],
        consumed: cursor,
    })
}

/// 把偏移写进帧头的 4 字节位置字段（`ZP0..ZP3`）。
///
/// **小端**。`lrzsz` 的 `zmodem.h` 里 `ZP0 = 0`、`ZP3 = 3`，`stohdr()`（`zm.c:959`）
/// 逐字节写 `pos`、`pos>>8`、`pos>>16`、`pos>>24`，所以线上第一个字节是最低字节；
/// `rclhdr()`（`zm.c:970`）反向拼回。
///
/// 位置字段的字节序**只在偏移为 0 时才看不出来**——`ZFILE` 之后每一帧的偏移都非 0，
/// 写成大端会让对端读到 `0x0004_0000` 这种天文数字，表现是「对端无视我们的
/// `ZACK`，一直重发」。
pub const fn encode_position(pos: u32) -> [u8; 4] {
    pos.to_le_bytes()
}

/// 从帧头的 4 字节位置字段（`ZP0..ZP3`）读回偏移。与 [`encode_position`] 对称。
pub const fn decode_position(data: [u8; 4]) -> u32 {
    u32::from_le_bytes(data)
}

/// 组装 `ZRINIT` 的 4 字节头数据：`[缓冲大小低字节, 缓冲大小高字节, 0, 能力位]`。
///
/// `lrzsz` 的 `tryz()`（`lrz.c:1648-1664`）先 `stohdr(SEGMENTS*MAX_BLOCK)` 把缓冲
/// 大小按**小端**铺进 `ZP0..ZP3`，随后 `Txhdr[ZF0] = CANFC32|CANFDX|CANOVIO` 覆盖
/// **第 4 个字节**——`zmodem.h` 里 `#define ZF0 3`，即 `ZF0` 是头数据的**最后一
/// 字节**，`ZF1=2`、`ZF2=1`、`ZF3=0`。
///
/// 所以能力位在**末字节**，缓冲大小在**前两字节（小端）**。把能力位放在第一个字节
/// 的写法能跑通（对端只是读不到我们的能力位、退化成 `ZBIN`），但对端算出来的缓冲
/// 大小会是个错值（`lsz.c:1621` 就是 `Rxhdr[ZP0] + (Rxhdr[ZP1]<<8)`）。
pub const fn rinit_header_data(flags: u8, buffer: u16) -> [u8; 4] {
    let size = buffer.to_le_bytes();
    [size[0], size[1], 0, flags]
}

/// 从 `ZRINIT` 的 4 字节头数据里读能力位（`ZF0`，即**最后一个字节**）。
pub const fn rinit_flags(data: [u8; 4]) -> u8 {
    data[3]
}

/// `ZFILE` 帧头头数据里的「文件转换选项」（`ZF0`，即**第 4 个字节**）。
///
/// `zmodem.h`：`ZCBIN = 1`（二进制传输，禁止换行转换）。发送方不声明它时，多数
/// 接收方按 `ZCBIN` 处理（`lrz.c:1684-1688`：`if (!zconv) zconv = ZCBIN;`），
/// 但显式声明更安全——它的反作用是让「对端把 `CR LF` 转换成本地行尾」，那会直接
/// 改坏二进制文件。
pub const ZCBIN: u8 = 1;

/// 本端 `ZFILE` 帧头固定携带的头数据：`ZF3..ZF1` 全 0，`ZF0 = ZCBIN`。
///
/// 注意 `ZFILE` 的 4 字节头数据**不是位置字段**，不能用
/// [`encode_position`]——真实 `sz` 发的是 `00 00 00 01`（`ZF0=1`），
/// 而那按位置解释是 `0x0100_0000`。
pub const FILE_HEADER_DATA: [u8; 4] = [0, 0, 0, ZCBIN];

/// 编码一个数据子包：转义后的载荷 + `ZDLE` + 帧尾 + CRC（逐字节转义）。
///
/// CRC 覆盖**载荷 + 帧尾字节**；CRC-16 高字节在前、CRC-32 低字节在前，
/// 与 `lrzsz` 的 `zsdata()` / `zsda32()` 一致。
pub fn encode_subpacket(data: &[u8], end: FrameEnd, crc: CrcKind) -> Vec<u8> {
    let mut out = Vec::with_capacity(data.len() + data.len() / 2 + 12);
    for &byte in data {
        push_escaped(&mut out, byte);
    }
    out.push(ZDLE);
    out.push(end.as_byte());
    match crc {
        CrcKind::Crc16 => {
            let value = subpacket_crc16(data, end);
            push_escaped(&mut out, (value >> 8) as u8);
            push_escaped(&mut out, value as u8);
        }
        CrcKind::Crc32 => {
            let value = subpacket_crc32(data, end);
            for shift in [0, 8, 16, 24] {
                push_escaped(&mut out, (value >> shift) as u8);
            }
        }
    }
    out
}

/// 编码一个完整的 `ZDATA` 帧（帧头 + 一个数据子包）。
///
/// `offset` 按 [`encode_position`] 写成**小端**；子包 CRC 由 `header` 决定
/// （[`HeaderKind::subpacket_crc`]），必须与帧头匹配。
pub fn encode_data_frame(
    offset: u32,
    payload: &[u8],
    end: FrameEnd,
    header: HeaderKind,
) -> Vec<u8> {
    let mut out = encode_header(header, FrameType::ZDATA, encode_position(offset));
    out.extend_from_slice(&encode_subpacket(payload, end, header.subpacket_crc()));
    out
}

/// 解出来的一个数据子包。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Subpacket {
    /// 转义还原后的载荷。
    pub data: Vec<u8>,
    /// 帧尾。
    pub end: FrameEnd,
    /// 本子包在输入里占用的字节数。
    pub consumed: usize,
}

/// [`parse_subpacket`] 的结果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ParsedSubpacket {
    /// 字节还不够，等下一次 `feed`。
    Incomplete,
    /// 帧尾或 CRC 不对。`consumed` 是建议丢弃的字节数（含 CRC 字段），恒 `>= 1`。
    Invalid {
        /// 建议丢弃的字节数。
        consumed: usize,
    },
    /// 一个校验通过的子包。
    Valid(Subpacket),
}

enum EscapedByte {
    Byte(u8, usize),
    Incomplete,
}

/// 读一个「可能被转义」的子包字节（`ZFILE` / `ZDATA` 的载荷与 CRC 都用它）。
fn read_subpacket_byte(buf: &[u8], at: usize) -> EscapedByte {
    let Some(&byte) = buf.get(at) else {
        return EscapedByte::Incomplete;
    };
    if byte != ZDLE {
        return EscapedByte::Byte(byte, 1);
    }
    let Some(&next) = buf.get(at + 1) else {
        return EscapedByte::Incomplete;
    };
    EscapedByte::Byte(next ^ 0x40, 2)
}

/// 从缓冲区**头部**解析一个数据子包（调用方保证当前位置紧跟在一个
/// [`FrameType::ZDATA`] / [`FrameType::ZFILE`] / [`FrameType::ZSINIT`] 帧头之后）。
///
/// `crc` 必须取自刚解析出的 [`Header::kind`] 的
/// [`HeaderKind::subpacket_crc`]——校验宽度错了会一直判帧错。
///
/// 数据里连续出现多个 `ZDLE` 也能正确解：遇到帧尾字节（`h`/`i`/`j`/`k`）才算
/// 结束，其余 `ZDLE + x` 一律按 `x ^ 0x40` 还原。
pub fn parse_subpacket(buf: &[u8], crc: CrcKind) -> ParsedSubpacket {
    let mut data = Vec::new();
    let mut at = 0;
    loop {
        let Some(&byte) = buf.get(at) else {
            return ParsedSubpacket::Incomplete;
        };
        if byte != ZDLE {
            data.push(byte);
            at += 1;
            continue;
        }
        let Some(&next) = buf.get(at + 1) else {
            return ParsedSubpacket::Incomplete;
        };
        if let Some(end) = FrameEnd::from_byte(next) {
            let mut crc_bytes = [0u8; 4];
            let mut cursor = at + 2;
            for slot in crc_bytes.iter_mut().take(crc.width()) {
                match read_subpacket_byte(buf, cursor) {
                    EscapedByte::Byte(byte, used) => {
                        *slot = byte;
                        cursor += used;
                    }
                    EscapedByte::Incomplete => return ParsedSubpacket::Incomplete,
                }
            }
            let crc_ok = match crc {
                CrcKind::Crc16 => {
                    let stated = u16::from_be_bytes([crc_bytes[0], crc_bytes[1]]);
                    subpacket_crc16(&data, end) == stated
                }
                CrcKind::Crc32 => subpacket_crc32(&data, end) == u32::from_le_bytes(crc_bytes),
            };
            if !crc_ok {
                return ParsedSubpacket::Invalid { consumed: cursor };
            }
            return ParsedSubpacket::Valid(Subpacket {
                data,
                end,
                consumed: cursor,
            });
        }
        // 普通转义字节。
        data.push(next ^ 0x40);
        at += 2;
    }
}

/// `ZFILE` 子包解出来的文件信息。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileInfo {
    /// 文件名（不含路径处理：线上是什么就是什么，调用方负责清洗）。
    pub name: String,
    /// 文件字节数。对端没填时为 `None`（少数发送方不填，只能靠 `ZEOF` 判断结束）。
    pub size: Option<u64>,
    /// 修改时间（Unix 秒）。对端没填或字段不是八进制时为 `None`。
    ///
    /// **不编造默认值**：`None` 就是「对端没给」，不能当成 1970 年。
    pub mtime: Option<u32>,
    /// 文件模式（Unix 权限位）。对端没填或字段不是八进制时为 `None`。
    ///
    /// 线上是八进制字符串（`sz` 发 `100644`），这里只保留低 12 位
    /// （`0o7777`）：`"100644"` → `0o644`。`None` 不能当成权限 `000`。
    pub mode: Option<u32>,
}

/// 编码 `ZFILE` 的数据子包。
///
/// 线上形态：`文件名 \0 长度(十进制) 空格 mtime(八进制) 空格 mode(八进制) 空格
/// 序号(八进制) \0`。序号本实现恒发 `0`，mode 恒发 `100644`（ZMODEM 不传内容，
/// 权限位只影响对端落盘时的默认值）。
pub fn encode_file_info(name: &str, size: u64, mtime: u32) -> Vec<u8> {
    let mut out = Vec::with_capacity(name.len() + 32);
    out.extend_from_slice(name.as_bytes());
    out.push(0);
    out.extend_from_slice(format!("{size} {mtime:o} 100644 0").as_bytes());
    out.push(0);
    out
}

/// 解析 `ZFILE` 的数据子包。
///
/// 只强制要求「文件名非空」与「长度字段是十进制数字」；mtime / mode / 序号缺失或
/// 不合法时按 `None` 处理（老实现未必填全，而 `0` 会被调用方当成 1970 年 / 权限
/// `000`，所以**不能编造默认值**）。
///
/// mode 按八进制解析后只保留低 12 位：线上常见的 `100644` 得到 `0o644`。
///
/// # 错误
///
/// 返回人读的诊断字符串（由状态机包成 [`term_xfer::Error::BadHeader`]）。
pub fn parse_file_info(data: &[u8]) -> Result<FileInfo, String> {
    let nul = data.iter().position(|&byte| byte == 0).unwrap_or(data.len());
    let name = String::from_utf8_lossy(&data[..nul]).into_owned();
    if name.is_empty() {
        return Err("文件名为空".to_string());
    }
    let rest = data.get(nul + 1..).unwrap_or(&[]);
    let rest = std::str::from_utf8(rest)
        .map_err(|_| "文件信息字段不是合法的 UTF-8/ASCII".to_string())?;
    let mut fields = rest
        .split(|c: char| c.is_ascii_whitespace())
        .filter(|field| !field.is_empty());
    let size = match fields.next() {
        Some(field) => Some(
            field
                .parse::<u64>()
                .map_err(|_| format!("长度字段不是十进制数字：{field:?}"))?,
        ),
        None => None,
    };
    let mtime = fields
        .next()
        .and_then(|field| u32::from_str_radix(field, 8).ok());
    let mode = fields
        .next()
        .and_then(|field| u32::from_str_radix(field, 8).ok())
        .map(|mode| mode & 0o7777);
    Ok(FileInfo {
        name,
        size,
        mtime,
        mode,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn crc32_matches_known_vectors() {
        assert_eq!(crc32(b"123456789"), 0xCBF4_3926);
        assert_eq!(
            crc32(b"The quick brown fox jumps over the lazy dog"),
            0x414F_A339
        );
    }

    #[test]
    fn crc32_of_empty_is_zero() {
        assert_eq!(crc32(b""), 0);
    }

    #[test]
    fn crc16_is_the_xfer_one() {
        // 复用 term-xfer 的实现：这条断言是「没有偷偷换算法」的钉子。
        assert_eq!(crc16(b"123456789"), 0x31C3);
    }

    #[test]
    fn zrqinit_hex_header_ends_with_cr_8a_and_xon() {
        // 真机 `sz` 的 ZRQINIT：`* * ZDLE B` + "00"+"00000000"+"0000" + `0d 8a 11`。
        let frame = encode_hex_header(FrameType::ZRQINIT, [0; 4]);
        let mut want = vec![ZPAD, ZPAD, ZDLE, ZHEX];
        want.extend_from_slice(b"00");
        want.extend_from_slice(b"00000000");
        want.extend_from_slice(b"0000");
        want.extend_from_slice(&[CR, LF_HIGH, XON]);
        assert_eq!(frame, want);
        assert_eq!(frame.len(), 21, "ZRQINIT 的 ZHEX 帧头 21 字节");
    }

    #[test]
    fn zack_and_zfin_hex_headers_have_no_xon_tail() {
        // `zshhdr()`：`if (type != ZFIN && type != ZACK) s[len++]=021;`
        for frame in [FrameType::ZACK, FrameType::ZFIN] {
            let bytes = encode_hex_header(frame, [0; 4]);
            assert_eq!(bytes.len(), 20, "{frame:?} 的 ZHEX 帧头 20 字节");
            assert_eq!(bytes[18..], [CR, LF_HIGH], "{frame:?} 不该带 XON 尾巴");
        }
        for frame in [
            FrameType::ZRINIT,
            FrameType::ZRPOS,
            FrameType::ZNAK,
            FrameType::ZSKIP,
        ] {
            let bytes = encode_hex_header(frame, [0; 4]);
            assert_eq!(bytes[18..], [CR, LF_HIGH, XON], "{frame:?} 该带 XON 尾巴");
        }
    }

    #[test]
    fn hex_header_accepts_cr_lf_cr_8a_and_high_cr() {
        // `zrhhdr()` 只要求「第一个字节是 CR 或 0x8D」，第二个字节不论是不是 LF 都吃掉。
        let base = encode_hex_header(FrameType::ZRINIT, [1, 2, 3, 4]);
        let body = &base[..18];
        for tail in [
            vec![0x0D, 0x8A, XON],
            vec![0x0D, 0x0A, XON],
            vec![0x8D, 0x0A, XON],
        ] {
            let mut buf = body.to_vec();
            buf.extend_from_slice(&tail);
            match parse_header(&buf) {
                ParsedHeader::Valid(header) => {
                    assert_eq!(header.frame, FrameType::ZRINIT, "tail = {tail:02x?}");
                    assert_eq!(header.data, [1, 2, 3, 4]);
                    assert_eq!(header.consumed, buf.len(), "tail = {tail:02x?}");
                }
                other => panic!("tail = {tail:02x?} 应当合法，得到 {other:?}"),
            }
        }

        // 只到 `CR` 时必须再等一个字节（终止符的第二个字节可能还在路上）。
        let mut half = body.to_vec();
        half.push(0x0D);
        assert_eq!(parse_header(&half), ParsedHeader::Incomplete);

        // `XON` 尾巴可省：没有它也是合法帧头（有些实现不发）。
        let mut bare = body.to_vec();
        bare.extend_from_slice(&[0x0D, 0x8A]);
        match parse_header(&bare) {
            ParsedHeader::Valid(header) => assert_eq!(header.consumed, 20),
            other => panic!("无 XON 尾巴的帧头也该合法，得到 {other:?}"),
        }
    }

    #[test]
    fn position_field_is_little_endian() {
        // 真机 `sz`：`stohdr()` 逐字节发 `pos`、`pos>>8`、`pos>>16`、`pos>>24`。
        assert_eq!(encode_position(0), [0, 0, 0, 0]);
        assert_eq!(encode_position(1024), [0x00, 0x04, 0x00, 0x00]);
        assert_eq!(encode_position(0xDEAD_BEEF), [0xEF, 0xBE, 0xAD, 0xDE]);
        assert_eq!(decode_position(encode_position(0x00AB_CDEF)), 0x00AB_CDEF);
        // 写反了的钉子：大端会把 1024 读成 262144。
        assert_eq!(decode_position(1024u32.to_be_bytes()), 0x0004_0000);
        // 线上形态：帧头里 4 个 hex 字符对 `1024` 是 "00040000"，不是 "00000400"。
        let frame = encode_hex_header(FrameType::ZACK, encode_position(1024));
        assert_eq!(&frame[6..14], b"00040000");
    }

    #[test]
    fn rinit_flags_live_in_the_last_header_byte() {
        // `zmodem.h` 的 `#define ZF0 3`：能力位在第 4 个字节，缓冲大小在前两字节（小端）。
        let data = rinit_header_data(RINIT_FLAGS, RINIT_BUFFER);
        assert_eq!(data, [0x00, 0x20, 0x00, RINIT_FLAGS]);
        assert_eq!(
            data[0] as u16 | (data[1] as u16) << 8,
            8192,
            "lsz.c:1621 的算法"
        );
        assert_eq!(rinit_flags(data), RINIT_FLAGS);
        assert_eq!(
            rinit_flags([0x60, 0x20, 0x00, 0x00]),
            0x00,
            "能力位不在首字节"
        );
    }

    #[test]
    fn zfile_header_declares_binary_transfer() {
        // 真实 `sz` 的 ZFILE 头数据是 `00 00 00 01`（`ZF0 = ZCBIN`）。
        assert_eq!(FILE_HEADER_DATA[3], ZCBIN);
        assert_eq!(ZCBIN, 1);
        assert_eq!(FILE_HEADER_DATA, [0, 0, 0, 1]);
    }

    #[test]
    fn hex_header_roundtrip() {
        let frame = encode_header(HeaderKind::Hex, FrameType::ZRPOS, [0xDE, 0xAD, 0xBE, 0xEF]);
        match parse_header(&frame) {
            ParsedHeader::Valid(header) => {
                assert_eq!(header.kind, HeaderKind::Hex);
                assert_eq!(header.frame, FrameType::ZRPOS);
                assert_eq!(header.data, [0xDE, 0xAD, 0xBE, 0xEF]);
                assert_eq!(header.consumed, frame.len());
            }
            other => panic!("expected valid header, got {other:?}"),
        }
    }

    #[test]
    fn header_control_bytes_are_hex_encoded_not_escaped() {
        // 帧头携带的 4 字节可以是任意值：十六进制帧头把它们写成 ASCII，
        // 不会出现裸的 ZDLE（0x18），所以不需要转义。
        let data = [0x18, 0x10, 0x13, 0x11];
        let frame = encode_header(HeaderKind::Hex, FrameType::ZACK, data);
        assert!(!frame[4..].contains(&ZDLE));
        match parse_header(&frame) {
            ParsedHeader::Valid(header) => assert_eq!(header.data, data),
            other => panic!("expected valid header, got {other:?}"),
        }
    }

    #[test]
    fn header_skips_any_number_of_zpads() {
        for extra in 1..=5 {
            let mut frame = vec![ZPAD; extra];
            frame.extend_from_slice(&encode_header(HeaderKind::Bin32, FrameType::ZFIN, [0; 4]));
            match parse_header(&frame) {
                ParsedHeader::Valid(header) => {
                    assert_eq!(header.kind, HeaderKind::Bin32);
                    assert_eq!(header.frame, FrameType::ZFIN);
                    assert_eq!(header.consumed, frame.len());
                }
                other => panic!("{extra} 个额外 ZPAD 时解析失败：{other:?}"),
            }
        }
    }

    #[test]
    fn hex_header_incomplete_at_every_prefix() {
        let frame = encode_hex_header(FrameType::ZRINIT, [1, 2, 3, 4]);
        assert_eq!(frame.len(), 21, "ZRINIT 带 XON 尾巴");
        // 前 20 字节（`* * ZDLE B` + 14 hex + `0D 8A`）是必需部分，少一个字节都不完整。
        for cut in 0..20 {
            assert_eq!(
                parse_header(&frame[..cut]),
                ParsedHeader::Incomplete,
                "cut = {cut}"
            );
        }
        // 第 20 字节是**可选**的 `XON` 尾巴：到 20 就已经能判定了，到 21 就全吃掉。
        for cut in [20, 21] {
            match parse_header(&frame[..cut]) {
                ParsedHeader::Valid(header) => assert_eq!(header.consumed, cut),
                other => panic!("cut = {cut} 应当合法，得到 {other:?}"),
            }
        }
    }

    #[test]
    fn header_followed_immediately_by_data_is_valid() {
        // 没有终止符、紧跟一个字节：够判定了，不该无限等。
        let mut buf = encode_hex_header(FrameType::ZACK, [0, 0, 0, 5]);
        buf.truncate(18); // 去掉终止符 `0D 8A`
        buf.push(b'h');
        match parse_header(&buf) {
            ParsedHeader::Valid(header) => {
                assert_eq!(header.frame, FrameType::ZACK);
                assert_eq!(header.consumed, 18);
            }
            other => panic!("应当合法，得到 {other:?}"),
        }
    }

    #[test]
    fn hex_header_with_bad_crc_is_invalid() {
        let mut frame = encode_hex_header(FrameType::ZACK, [0, 0, 4, 0]);
        // 最后两个 hex 字符是 CRC；改掉其中一个（第 17 字节，位于 `transmitted` 区）。
        let at = 17;
        assert!(frame[at].is_ascii_hexdigit(), "第 {at} 字节应当是 hex 字符");
        frame[at] = if frame[at] == b'0' { b'1' } else { b'0' };
        match parse_header(&frame) {
            ParsedHeader::Invalid { consumed } => assert_eq!(consumed, 18),
            other => panic!("expected invalid, got {other:?}"),
        }
    }

    #[test]
    fn garbage_prefix_is_dropped_one_byte_at_a_time() {
        let frame = encode_header(HeaderKind::Bin, FrameType::ZFIN, [0; 4]);
        let mut buf = b"login banner\r\n".to_vec();
        buf.extend_from_slice(&frame);
        let mut skipped = 0;
        loop {
            match parse_header(&buf) {
                ParsedHeader::Valid(header) => {
                    assert_eq!(header.frame, FrameType::ZFIN);
                    break;
                }
                ParsedHeader::Invalid { consumed } => {
                    assert!(consumed >= 1);
                    buf.drain(..consumed);
                    skipped += consumed;
                }
                ParsedHeader::Incomplete => panic!("不该不完整"),
            }
        }
        assert_eq!(skipped, b"login banner\r\n".len());
    }

    #[test]
    fn control_frames_are_hex_data_frames_follow_negotiation() {
        // 这张表就是「握手必须可打印」的钉子：控制帧无视协商结果。
        for frame in [
            FrameType::ZRQINIT,
            FrameType::ZRINIT,
            FrameType::ZFIN,
            FrameType::ZACK,
            FrameType::ZNAK,
            FrameType::ZSKIP,
            FrameType::ZFERR,
            FrameType::ZABORT,
            FrameType::ZRPOS,
        ] {
            assert!(!frame.is_data_frame(), "{frame:?} 不是数据帧");
            assert_eq!(
                frame.header_kind(HeaderKind::Bin32),
                HeaderKind::Hex,
                "{frame:?} 必须用 ZHEX"
            );
            assert_eq!(frame.header_kind(HeaderKind::Hex), HeaderKind::Hex);
        }
        for frame in [
            FrameType::ZFILE,
            FrameType::ZDATA,
            FrameType::ZEOF,
            FrameType::ZSINIT,
        ] {
            assert!(frame.is_data_frame(), "{frame:?} 是数据帧");
            assert_eq!(frame.header_kind(HeaderKind::Bin32), HeaderKind::Bin32);
            assert_eq!(frame.header_kind(HeaderKind::Bin), HeaderKind::Bin);
            assert_eq!(frame.header_kind(HeaderKind::Hex), HeaderKind::Hex);
        }
    }

    #[test]
    fn zbin_header_roundtrip() {
        let frame = encode_header(HeaderKind::Bin, FrameType::ZRPOS, [0, 0, 4, 0]);
        assert_eq!(frame[0], ZPAD);
        assert_eq!(frame[1], ZDLE);
        assert_eq!(frame[2], ZBIN);
        assert_eq!(frame.len(), 10, "无转义时 ZBIN 帧头 10 字节");
        match parse_header(&frame) {
            ParsedHeader::Valid(header) => {
                assert_eq!(header.kind, HeaderKind::Bin);
                assert_eq!(header.frame, FrameType::ZRPOS);
                assert_eq!(header.data, [0, 0, 4, 0]);
                assert_eq!(header.consumed, frame.len());
            }
            other => panic!("expected valid header, got {other:?}"),
        }
    }

    #[test]
    fn zbin32_header_roundtrip() {
        let frame = encode_header(HeaderKind::Bin32, FrameType::ZDATA, [0, 0, 8, 0]);
        assert_eq!(frame[0], ZPAD);
        assert_eq!(frame[1], ZDLE);
        assert_eq!(frame[2], ZBIN32);
        assert_eq!(frame.len(), 12, "无转义时 ZBIN32 帧头 12 字节");
        match parse_header(&frame) {
            ParsedHeader::Valid(header) => {
                assert_eq!(header.kind, HeaderKind::Bin32);
                assert_eq!(header.frame, FrameType::ZDATA);
                assert_eq!(header.data, [0, 0, 8, 0]);
                assert_eq!(header.consumed, frame.len());
            }
            other => panic!("expected valid header, got {other:?}"),
        }
    }

    #[test]
    fn binary_header_escapes_type_and_data_bytes() {
        // ZCRC = 13 = 0x0D（必须转义），数据里塞满其余四个控制字节。
        let data = [0x18, 0x10, 0x13, 0x11];
        for kind in [HeaderKind::Bin, HeaderKind::Bin32] {
            let frame = encode_header(kind, FrameType::ZCRC, data);
            for (escaped, original) in [(0x4D, 0x0D), (0x58, 0x18), (0x50, 0x10)] {
                assert!(
                    frame.windows(2).any(|w| w == [ZDLE, escaped]),
                    "{kind:?} 里 {original:#04x} 应当被转义"
                );
            }
            assert!(
                frame.windows(2).any(|w| w == [ZDLE, 0x53]),
                "{kind:?} 里 0x13 应当被转义"
            );
            assert!(
                frame.windows(2).any(|w| w == [ZDLE, 0x51]),
                "{kind:?} 里 0x11 应当被转义"
            );
            match parse_header(&frame) {
                ParsedHeader::Valid(header) => {
                    assert_eq!(header.kind, kind);
                    assert_eq!(header.frame, FrameType::ZCRC);
                    assert_eq!(header.data, data);
                    assert_eq!(header.consumed, frame.len());
                }
                other => panic!("{kind:?} 转义后解析失败：{other:?}"),
            }
        }
    }

    #[test]
    fn binary_headers_incomplete_at_every_prefix() {
        for kind in [HeaderKind::Bin, HeaderKind::Bin32] {
            let frame = encode_header(kind, FrameType::ZFILE, [0x18, 0x10, 0x13, 0x11]);
            for cut in 0..frame.len() {
                assert_eq!(
                    parse_header(&frame[..cut]),
                    ParsedHeader::Incomplete,
                    "{kind:?} 在 cut = {cut} 时应当不完整"
                );
            }
            assert!(matches!(parse_header(&frame), ParsedHeader::Valid(_)));
        }
    }

    #[test]
    fn binary_header_with_bad_crc_is_invalid() {
        for kind in [HeaderKind::Bin, HeaderKind::Bin32] {
            let mut frame = encode_header(kind, FrameType::ZACK, [0, 0, 4, 0]);
            let last = frame.len() - 1;
            frame[last] ^= 0x01;
            assert!(
                matches!(parse_header(&frame), ParsedHeader::Invalid { .. }),
                "{kind:?} 改坏 CRC 后应当无效"
            );
        }
    }

    #[test]
    fn mixed_header_stream_parses_all_three() {
        let mut stream = encode_header(HeaderKind::Hex, FrameType::ZRQINIT, [0; 4]);
        stream.extend(encode_header(HeaderKind::Bin32, FrameType::ZFILE, [0; 4]));
        stream.extend(encode_header(HeaderKind::Bin, FrameType::ZDATA, [0, 0, 1, 0]));
        stream.extend(encode_header(HeaderKind::Hex, FrameType::ZACK, [0, 0, 1, 0]));

        let mut seen = Vec::new();
        while !stream.is_empty() {
            match parse_header(&stream) {
                ParsedHeader::Valid(header) => {
                    seen.push((header.kind, header.frame));
                    stream.drain(..header.consumed);
                }
                other => panic!("混流解析失败：{other:?}"),
            }
        }
        assert_eq!(
            seen,
            vec![
                (HeaderKind::Hex, FrameType::ZRQINIT),
                (HeaderKind::Bin32, FrameType::ZFILE),
                (HeaderKind::Bin, FrameType::ZDATA),
                (HeaderKind::Hex, FrameType::ZACK),
            ]
        );
    }

    #[test]
    fn high_bit_flow_control_bytes_must_be_escaped() {
        // 第一次真联调就是死在这里：`0x91` / `0x93` 被 `zdlread2()` 当流控吃掉，
        // 整帧错位后对端只报 `Bad CRC`；裸的 `0x8D` 会被判 `Bad escape sequence`。
        for byte in [
            CR, CR_HIGH, DLE, DLE_HIGH, XON, XON_HIGH, XOFF, XOFF_HIGH, ZDLE,
        ] {
            assert!(needs_escape(byte), "{byte:#04x} 必须转义");
        }
        for byte in [0x00, 0x06, b'A', 0x7F, 0xA0, 0xFF] {
            assert!(!needs_escape(byte), "{byte:#04x} 不必转义");
        }

        // 子包里也要真的写出来，而且要能解回去。
        let payload = [XON_HIGH, XOFF_HIGH, DLE_HIGH, CR_HIGH];
        let encoded = encode_subpacket(&payload, FrameEnd::ZCRCW, CrcKind::Crc16);
        for original in payload {
            let escaped = original ^ 0x40;
            assert!(
                encoded.windows(2).any(|w| w == [ZDLE, escaped]),
                "{original:#04x} 应当被转义成 {ZDLE:#04x} {escaped:#04x}"
            );
        }
        match parse_subpacket(&encoded, CrcKind::Crc16) {
            ParsedSubpacket::Valid(packet) => assert_eq!(packet.data, payload),
            other => panic!("expected valid subpacket, got {other:?}"),
        }
    }

    #[test]
    fn subpacket_roundtrip_escapes_control_bytes() {
        let payload = [0x0D, 0x10, 0x11, 0x13, 0x18, b'A'];
        let encoded = encode_subpacket(&payload, FrameEnd::ZCRCW, CrcKind::Crc32);
        assert!(encoded.windows(2).any(|w| w == [ZDLE, 0x4D]));
        assert!(encoded.windows(2).any(|w| w == [ZDLE, 0x50]));
        assert!(encoded.windows(2).any(|w| w == [ZDLE, 0x51]));
        assert!(encoded.windows(2).any(|w| w == [ZDLE, 0x53]));
        assert!(encoded.windows(2).any(|w| w == [ZDLE, 0x58]));
        match parse_subpacket(&encoded, CrcKind::Crc32) {
            ParsedSubpacket::Valid(packet) => {
                assert_eq!(packet.data, payload);
                assert_eq!(packet.end, FrameEnd::ZCRCW);
                assert_eq!(packet.consumed, encoded.len());
            }
            other => panic!("expected valid subpacket, got {other:?}"),
        }
    }

    #[test]
    fn subpacket_crc16_roundtrip() {
        let payload = b"hello";
        let encoded = encode_subpacket(payload, FrameEnd::ZCRCW, CrcKind::Crc16);
        match parse_subpacket(&encoded, CrcKind::Crc16) {
            ParsedSubpacket::Valid(packet) => assert_eq!(packet.data, payload),
            other => panic!("expected valid subpacket, got {other:?}"),
        }
    }

    #[test]
    fn subpacket_crc_width_must_match_header() {
        // 同一份载荷，用错 CRC 宽度必须判错——这正是「子包 CRC 跟帧头走」的钉子。
        let payload = b"hello";
        let as16 = encode_subpacket(payload, FrameEnd::ZCRCW, CrcKind::Crc16);
        let as32 = encode_subpacket(payload, FrameEnd::ZCRCW, CrcKind::Crc32);

        // 16 位 CRC 只给得出 2 字节，按 32 位读是凑不齐的：先判「不完整」，
        // 绝不能因为缺字节就当成合法包。
        assert!(matches!(
            parse_subpacket(&as16, CrcKind::Crc32),
            ParsedSubpacket::Incomplete
        ));
        let mut padded = as16.clone();
        padded.extend_from_slice(&[0x00, 0x00]);
        assert!(matches!(
            parse_subpacket(&padded, CrcKind::Crc32),
            ParsedSubpacket::Invalid { .. }
        ));

        // 反过来：32 位 CRC 按 16 位读，CRC 立刻对不上。
        assert!(matches!(
            parse_subpacket(&as32, CrcKind::Crc16),
            ParsedSubpacket::Invalid { .. }
        ));
    }

    #[test]
    fn subpacket_handles_consecutive_zdle() {
        let payload = [0x18, 0x18, 0x18, 0x18];
        let encoded = encode_subpacket(&payload, FrameEnd::ZCRCE, CrcKind::Crc32);
        match parse_subpacket(&encoded, CrcKind::Crc32) {
            ParsedSubpacket::Valid(packet) => {
                assert_eq!(packet.data, payload);
                assert_eq!(packet.end, FrameEnd::ZCRCE);
            }
            other => panic!("expected valid subpacket, got {other:?}"),
        }
    }

    #[test]
    fn subpacket_keeps_raw_zpad() {
        // ZPAD 在子包里是普通数据，不转义也不特殊。
        let payload = [ZPAD, ZPAD, ZPAD];
        let encoded = encode_subpacket(&payload, FrameEnd::ZCRCW, CrcKind::Crc32);
        match parse_subpacket(&encoded, CrcKind::Crc32) {
            ParsedSubpacket::Valid(packet) => assert_eq!(packet.data, payload),
            other => panic!("expected valid subpacket, got {other:?}"),
        }
    }

    #[test]
    fn subpacket_incomplete_at_every_prefix() {
        for crc in [CrcKind::Crc16, CrcKind::Crc32] {
            let encoded = encode_subpacket(b"hello, zmodem", FrameEnd::ZCRCW, crc);
            for cut in 0..encoded.len() {
                assert_eq!(
                    parse_subpacket(&encoded[..cut], crc),
                    ParsedSubpacket::Incomplete,
                    "{crc:?} 在 cut = {cut} 时应当不完整"
                );
            }
        }
    }

    #[test]
    fn subpacket_with_bad_crc_is_invalid() {
        let mut encoded = encode_subpacket(b"hello", FrameEnd::ZCRCW, CrcKind::Crc32);
        let last = encoded.len() - 1;
        encoded[last] ^= 0x01;
        assert!(matches!(
            parse_subpacket(&encoded, CrcKind::Crc32),
            ParsedSubpacket::Invalid { .. }
        ));
    }

    #[test]
    fn subpacket_with_bad_payload_is_invalid() {
        let mut encoded = encode_subpacket(b"hello", FrameEnd::ZCRCW, CrcKind::Crc32);
        encoded[0] = b'H';
        assert!(matches!(
            parse_subpacket(&encoded, CrcKind::Crc32),
            ParsedSubpacket::Invalid { .. }
        ));
    }

    #[test]
    fn large_subpacket_roundtrip() {
        let payload: Vec<u8> = (0..crate::config::MAX_BLOCK_SIZE)
            .map(|i| (i % 256) as u8)
            .collect();
        let encoded = encode_subpacket(&payload, FrameEnd::ZCRCW, CrcKind::Crc32);
        assert!(encoded.len() <= MAX_BUFFER, "合法帧必须落在缓冲上限内");
        match parse_subpacket(&encoded, CrcKind::Crc32) {
            ParsedSubpacket::Valid(packet) => assert_eq!(packet.data, payload),
            other => panic!("expected valid subpacket, got {other:?}"),
        }
    }

    #[test]
    fn data_frame_uses_header_kind_crc() {
        for kind in [HeaderKind::Hex, HeaderKind::Bin, HeaderKind::Bin32] {
            let frame = encode_data_frame(1024, b"payload", FrameEnd::ZCRCW, kind);
            match parse_header(&frame) {
                ParsedHeader::Valid(header) => {
                    assert_eq!(header.kind, kind);
                    assert_eq!(header.frame, FrameType::ZDATA);
                    let rest = &frame[header.consumed..];
                    match parse_subpacket(rest, kind.subpacket_crc()) {
                        ParsedSubpacket::Valid(packet) => assert_eq!(packet.data, b"payload"),
                        other => panic!("{kind:?} 子包解析失败：{other:?}"),
                    }
                }
                other => panic!("{kind:?} 帧头解析失败：{other:?}"),
            }
        }
    }

    #[test]
    fn file_info_roundtrip() {
        let encoded = encode_file_info("固件 v1.bin", 123_456, 1_700_000_000);
        let info = parse_file_info(&encoded).unwrap();
        assert_eq!(info.name, "固件 v1.bin");
        assert_eq!(info.size, Some(123_456));
        assert_eq!(info.mtime, Some(1_700_000_000));
    }

    #[test]
    fn file_info_without_trailing_fields() {
        let mut data = Vec::new();
        data.extend_from_slice(b"a.txt\0");
        data.extend_from_slice(b"42");
        let info = parse_file_info(&data).unwrap();
        assert_eq!(info.name, "a.txt");
        assert_eq!(info.size, Some(42));
        assert_eq!(info.mtime, None);
    }

    #[test]
    fn file_info_rejects_empty_name() {
        assert!(parse_file_info(b"\0 1 0 100644 0\0").is_err());
    }

    #[test]
    fn file_info_rejects_bad_size() {
        assert!(parse_file_info(b"a.txt\0not-a-number\0").is_err());
    }
}

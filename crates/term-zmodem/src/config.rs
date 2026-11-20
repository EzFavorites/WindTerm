//! 协议参数。
//!
//! 与 `term-xfer::Config` 的差别：X/YMODEM 的「校验方式 / 块尾填充 / 协议变体」在
//! ZMODEM 里都不存在，所以这里只有三项：**帧头的线格式**、**数据子包的最大载荷**、
//! **重试上限**。

use crate::frame::CrcKind;

/// 单个数据子包的默认载荷上限（字节）。
///
/// `sz` 的默认值是 1024；本实现的停等（每块等一个 ZACK）策略下 1024 已经足够，
/// 再大会让「一个块坏掉就要重传整块」的代价变高。
pub const DEFAULT_BLOCK_SIZE: usize = 1024;

/// 单个数据子包的载荷上限（字节）。
///
/// ZMODEM 协议本身没有硬上限（受接收方缓冲约束），本实现取 8192——与 `lrzsz`
/// 的默认接收缓冲一致。超过此值的 [`Config::block_size`] 会被钳到它。
pub const MAX_BLOCK_SIZE: usize = 8192;

/// 发送方**数据帧**用哪种帧头（**握手/控制帧永远 `ZHEX`，与它无关**；
/// **接收侧三种全收，也与它无关**）。
///
/// 三种帧头的线上形态（依据 Forsberg 的协议描述与 `lrzsz` 的实现）：
///
/// ```text
/// ZHEX   *  *  ZDLE 'B'  <14 个十六进制字符>       0D 8A [XON]   20/21 字节
/// ZBIN   *  ZDLE 'A'     type f0 f1 f2 f3 crc16(2)                 10 字节 + 转义
/// ZBIN32 *  ZDLE 'C'     type f0 f1 f2 f3 crc32(4)                 12 字节 + 转义
/// ```
///
/// 三者的类型字段都是 **1 字节**；ZBIN32 与 ZBIN 的差别只有指示符（`'C'`/`'A'`）
/// 与 CRC 宽度（32/16 位）。帧头里的每一个字节（类型、4 字节头数据、CRC）都按
/// ZDLE 规则转义。
///
/// `ZHEX` 的终止符是 `0D 8A`（`LF|0x80`），并且除 `ZACK`/`ZFIN` 之外再跟一个裸
/// `XON`——这是 `lrzsz` 的 `zshhdr()` 的线上形态。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum HeaderKind {
    /// `ZPAD ZDLE 'C'` + 类型 + 4 字节头 + CRC-32。**默认值**：现代链路都是
    /// 8-bit clean，二进制帧头最短、校验最强。对端 `ZRINIT` 没宣告 `CANFC32`
    /// 时自动降到 [`HeaderKind::Bin`]。
    #[default]
    Bin32,
    /// `ZPAD ZDLE 'A'` + 类型 + 4 字节头 + CRC-16。钉死 16 位 CRC：即使对端支持
    /// 32 位也不升级。
    Bin,
    /// `ZPAD ZPAD ZDLE 'B'` + 14 个十六进制字符 + `0D 8A`（+ 可选 `XON`）。选它表示
    /// **连数据帧也用可打印帧头**，留给只认 `ZHEX` 的对端。
    Hex,
}

impl HeaderKind {
    /// 跟在**这种帧头**后面的数据子包该用哪种 CRC。
    ///
    /// 规范原文：`ZHEX` 后面跟的数据包用 CRC-16；`ZBIN` 用 CRC-16；`ZBIN32` 用
    /// CRC-32。这一条不是可选的：真实 `rz`/`sz` 按收到的帧头类型决定子包的校验
    /// 宽度，不跟着走会直接判帧错。
    pub const fn subpacket_crc(self) -> CrcKind {
        match self {
            HeaderKind::Bin32 => CrcKind::Crc32,
            HeaderKind::Bin | HeaderKind::Hex => CrcKind::Crc16,
        }
    }
}

/// 传输参数。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Config {
    /// 发送方**数据帧**（`ZFILE` / `ZDATA` / `ZEOF` / `ZSINIT`）用哪种帧头。
    ///
    /// 握手与控制帧（`ZRQINIT` / `ZRINIT` / `ZACK` / `ZRPOS` / `ZFIN` …）**永远
    /// 用 `ZHEX`**，不受它影响——开场时双方还不知道链路是否 8-bit clean。
    ///
    /// 实际生效值还要看对端 `ZRINIT` 的 `CANFC32` 位：选 `Bin32` 但对端不支持
    /// CRC-32 时自动降到 [`HeaderKind::Bin`]；选 `Hex` 则数据帧也用可打印帧头。
    ///
    /// 接收不受它限制：帧头三种都认，回应答永远是 `ZHEX`。
    pub header: HeaderKind,

    /// 单个 `ZDATA` 子包携带的最大载荷字节数。
    ///
    /// 构造 [`crate::Sender`] / [`crate::Receiver`] 时会被钳到
    /// `1..=`[`MAX_BLOCK_SIZE`]，所以 [`crate::Sender::config`] 返回的是**实际生效值**。
    pub block_size: usize,

    /// 同一阶段上的连续失败上限，超过即放弃（`0` 表示「一次都不许失败」）。
    ///
    /// 失败包括：帧头/子包校验错、对端要求的偏移倒退、调用方定时器判定的超时。
    /// 每次有效推进都会把计数清零。
    pub max_retries: u32,

    /// 多文件批处理：`ZEOF` 之后是否继续接受下一个 `ZFILE`。
    ///
    /// 默认 `false`（单文件）：`ZEOF` 之后再来 `ZFILE` 一律回 `ZSKIP(0)`，让对端
    /// 直接走到 `ZFIN`。置 `true` 后接收方接受下一个文件，每个文件结束发一次
    /// [`crate::Action::FileDone`] 作为「上一文件结束」的边界；发送方则允许
    /// [`crate::Sender::add_file`] 排队多个文件。
    ///
    /// `false` 时线上字节与行为逐字节不变（opt-in）。
    pub batch: bool,

    /// 发送方是否用 `ZCRCG` 连发（不等逐块 `ZACK`），只在结尾用 `ZCRCW` + `ZEOF`。
    ///
    /// 默认 `false`：停等（`ZCRCW` 逐块确认），每块等一个 `ZACK`。置 `true` 后
    /// 一次推进就会把剩余数据全部打成 `ZCRCG` 子包发出；接收方侧的恢复粒度随之
    /// 变成「从已收偏移整段回退」（`ZRPOS(已收字节)`）。
    ///
    /// 接收方两种帧尾都能解析，与本字段无关。`false` 时线上字节逐字节不变（opt-in）。
    pub streaming: bool,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            header: HeaderKind::default(),
            block_size: DEFAULT_BLOCK_SIZE,
            max_retries: 10,
            batch: false,
            streaming: false,
        }
    }
}

impl Config {
    /// 覆盖帧头格式（链式写法）。
    pub fn with_header(mut self, header: HeaderKind) -> Self {
        self.header = header;
        self
    }

    /// 覆盖数据子包大小（链式写法）。
    pub fn with_block_size(mut self, block_size: usize) -> Self {
        self.block_size = block_size;
        self
    }

    /// 覆盖重试上限（链式写法）。
    pub fn with_max_retries(mut self, max_retries: u32) -> Self {
        self.max_retries = max_retries;
        self
    }

    /// 打开 / 关闭多文件批处理（链式写法，默认关闭）。
    pub fn with_batch(mut self, batch: bool) -> Self {
        self.batch = batch;
        self
    }

    /// 打开 / 关闭 `ZCRCG` 连发（链式写法，默认关闭 = 停等 `ZCRCW`）。
    pub fn with_streaming(mut self, streaming: bool) -> Self {
        self.streaming = streaming;
        self
    }

    /// 钳到合法范围后的参数（状态机内部保存这一份）。
    pub(crate) fn clamped(mut self) -> Self {
        self.block_size = self.block_size.clamp(1, MAX_BLOCK_SIZE);
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clamps_block_size() {
        assert_eq!(Config::default().with_block_size(0).clamped().block_size, 1);
        assert_eq!(
            Config::default()
                .with_block_size(usize::MAX)
                .clamped()
                .block_size,
            MAX_BLOCK_SIZE
        );
        assert_eq!(
            Config::default().with_block_size(333).clamped().block_size,
            333
        );
    }

    #[test]
    fn default_header_is_bin32() {
        assert_eq!(Config::default().header, HeaderKind::Bin32);
    }

    #[test]
    fn subpacket_crc_follows_header_kind() {
        assert_eq!(HeaderKind::Bin32.subpacket_crc(), CrcKind::Crc32);
        assert_eq!(HeaderKind::Bin.subpacket_crc(), CrcKind::Crc16);
        assert_eq!(HeaderKind::Hex.subpacket_crc(), CrcKind::Crc16);
    }
}

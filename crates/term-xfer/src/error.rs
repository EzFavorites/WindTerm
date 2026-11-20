//! 传输层错误。
//!
//! 只有**放弃传输**级别的失败才进 [`Error`]：单帧校验错、块号跳变、握手未到都是
//! 可重试路径，走 NAK / 重发，不在这里冒泡。重试耗尽时用
//! [`Error::TooManyRetries`] 一次性上报，并用 [`Fault`] 记住最后一次失败的原因。

use std::fmt;

/// 可重试路径上的最后一次失败原因（仅在重试耗尽时作为诊断信息上报）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Fault {
    /// 帧级校验失败：块号补码不匹配（`!blk != ~blk`），或 CRC / 校验和不对。
    Checksum,
    /// 帧本身有效，但块号不是期望值，也不是上一块的重复帧。
    BlockNumber {
        /// 当时期望的块号。
        expected: u8,
        /// 实际收到的块号。
        got: u8,
    },
    /// 什么都没来（调用方定时器判超时，或握手阶段一直等不到首帧）。
    Timeout,
}

impl fmt::Display for Fault {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Fault::Checksum => write!(f, "帧校验失败"),
            Fault::BlockNumber { expected, got } => {
                write!(f, "块号不符（期望 {expected}，收到 {got}）")
            }
            Fault::Timeout => write!(f, "等待对端超时"),
        }
    }
}

/// 导致传输放弃的错误。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Error {
    /// 连续失败次数超过上限，放弃传输。
    TooManyRetries {
        /// 连续失败次数。
        failures: u32,
        /// 最后一次失败的原因。
        last: Fault,
    },

    /// 对端连发 CAN 字节，主动中止（`CAN CAN` 是 XMODEM/YMODEM 的中止信号）。
    Cancelled,

    /// YMODEM 文件头块解析失败（块 0 没有文件名，或长度字段不是十进制数字）。
    BadHeader {
        /// 人读的诊断说明。
        detail: String,
    },

    /// 待发送文件的描述本身不合法（空文件名、含 NUL、头块放不下）。
    BadFileSpec {
        /// 人读的诊断说明。
        detail: String,
    },

    /// 发送方被要求开始，但队列里一个文件都没有。
    NoFileToSend,

    /// 传输已经开始，不允许再改队列。
    AlreadyStarted,
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::TooManyRetries { failures, last } => {
                write!(f, "连续失败 {failures} 次后放弃：{last}")
            }
            Error::Cancelled => write!(f, "对端发出中止信号（CAN CAN）"),
            Error::BadHeader { detail } => write!(f, "文件头块无法解析：{detail}"),
            Error::BadFileSpec { detail } => write!(f, "文件不合法：{detail}"),
            Error::NoFileToSend => write!(f, "没有待发送的文件"),
            Error::AlreadyStarted => write!(f, "传输已开始，不能再修改队列"),
        }
    }
}

impl std::error::Error for Error {}

//! 帧头与 `CAN` 中止序列的判别。
//!
//! 二进制帧头以 `ZDLE`（0x18）开头，而中止序列也是一串 0x18——同一批字节既可能
//! 是「帧头的第一个字节」，也可能是「第 N 个 CAN」。判别逻辑两边状态机都要用，
//! 所以单独放一处，配自己的测试，避免两边各写一份、各错一处。
//!
//! # 判据
//!
//! - `ZDLE` 后面跟着 `'B'` / `'A'` / `'C'` → 这是帧头（哪怕只到了一半，也是
//!   `Wait`，不能当 CAN 数）；
//! - `ZDLE` 后面是别的字节 → 这是 CAN（`lrzsz` 连发 5 个判中止）；
//! - 缓冲区里只剩一个孤零零的 `ZDLE` → 两种都可能。先按 CAN 记一次（这样「5 个
//!   CAN 恰好分片在末尾」也能及时中止），但**不消费**它；下一批字节到了若是
//!   指示符，就按帧头走（成功解析时计数清零），否则再按 CAN 消费。
//! - 帧头前连续 `ZPAD` 超过 [`frame::MAX_ZPAD_RUN`] → `Flood`，由状态机报错。
//!
//! 「已经按 CAN 记过一次」用 `tail_counted` 记住，避免同一个字节被重复计数。

use crate::frame::{self, ParsedHeader};

/// 一次扫描的结论。
pub(crate) enum Scan {
    /// 一个合法帧头（`consumed` 在 `Header` 里）。
    Header(frame::Header),
    /// 连续 `CAN` 达到中止阈值。
    Abort,
    /// 丢弃 `consumed` 个字节（噪声、坏帧、单个 CAN）。
    Garbage(usize),
    /// 字节还不够，等下一批。
    Wait,
    /// 帧头前的 `ZPAD` 串超过上限。
    Flood,
}

/// 帧头/中止判别器。每个状态机持有一个。
#[derive(Debug)]
pub(crate) struct Scanner {
    cans: usize,
    /// 缓冲区末尾那个孤立的 `ZDLE` 已经按 CAN 记过一次了。
    tail_counted: bool,
}

impl Scanner {
    pub(crate) fn new() -> Self {
        Self {
            cans: 0,
            tail_counted: false,
        }
    }

    pub(crate) fn scan(&mut self, buf: &[u8]) -> Scan {
        let Some(&first) = buf.first() else {
            return Scan::Wait;
        };

        if first == frame::ZPAD {
            let run = buf.iter().take_while(|&&byte| byte == frame::ZPAD).count();
            if run > frame::MAX_ZPAD_RUN {
                return Scan::Flood;
            }
        }

        match frame::parse_header(buf) {
            ParsedHeader::Valid(header) => {
                self.cans = 0;
                self.tail_counted = false;
                Scan::Header(header)
            }
            ParsedHeader::Invalid { consumed } => {
                let binary_head =
                    matches!(buf.get(1), Some(&byte) if byte == frame::ZBIN || byte == frame::ZBIN32);
                if first == frame::CAN && !binary_head {
                    if self.tail_counted {
                        // 上一批里已经数过这个字节，这次只把它消费掉。
                        self.tail_counted = false;
                    } else {
                        self.cans += 1;
                        if self.cans >= frame::ABORT_CAN_COUNT {
                            return Scan::Abort;
                        }
                    }
                    Scan::Garbage(1)
                } else {
                    self.cans = 0;
                    self.tail_counted = false;
                    Scan::Garbage(consumed.max(1))
                }
            }
            ParsedHeader::Incomplete => {
                if first == frame::CAN && buf.len() == 1 {
                    if !self.tail_counted {
                        self.cans += 1;
                        self.tail_counted = true;
                    }
                    if self.cans >= frame::ABORT_CAN_COUNT {
                        return Scan::Abort;
                    }
                }
                Scan::Wait
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 模拟状态机的 drain 循环：能丢就丢，遇 `Wait` 停下。`true` 表示中止。
    fn drive(scanner: &mut Scanner, buf: &mut Vec<u8>) -> bool {
        loop {
            match scanner.scan(buf) {
                Scan::Garbage(n) => {
                    let n = n.min(buf.len());
                    buf.drain(..n);
                }
                Scan::Abort => return true,
                Scan::Header(_) | Scan::Wait | Scan::Flood => return false,
            }
        }
    }

    #[test]
    fn five_cans_at_once_abort() {
        let mut scanner = Scanner::new();
        let mut buf = vec![frame::CAN; 5];
        assert!(drive(&mut scanner, &mut buf));
    }

    #[test]
    fn four_cans_do_not_abort() {
        let mut scanner = Scanner::new();
        let mut buf = vec![frame::CAN; 4];
        assert!(!drive(&mut scanner, &mut buf));
    }

    #[test]
    fn five_cans_byte_by_byte_abort_on_last() {
        let mut scanner = Scanner::new();
        let mut buf = Vec::new();
        for i in 0..5 {
            buf.push(frame::CAN);
            let aborted = drive(&mut scanner, &mut buf);
            if i < 4 {
                assert!(!aborted, "第 {} 个 CAN 不该中止", i + 1);
            } else {
                assert!(aborted, "第 5 个 CAN 应当中止");
            }
        }
    }

    #[test]
    fn binary_header_starting_with_zdle_is_not_a_can() {
        let header = frame::encode_header(
            crate::config::HeaderKind::Bin32,
            frame::FrameType::ZACK,
            [0, 0, 1, 0],
        );
        // 去掉前导 ZPAD，强制让帧头以 ZDLE 开头——这是最容易被误判成 CAN 的形状。
        let bare = &header[1..];
        let mut scanner = Scanner::new();
        for cut in 0..bare.len() {
            let mut buf = bare[..cut].to_vec();
            assert!(!drive(&mut scanner, &mut buf), "cut = {cut} 不该中止");
        }
        assert!(matches!(
            scanner.scan(bare),
            Scan::Header(header) if header.frame == frame::FrameType::ZACK
        ));
    }

    #[test]
    fn zpad_flood_is_reported() {
        let mut scanner = Scanner::new();
        let flood = vec![frame::ZPAD; frame::MAX_ZPAD_RUN + 1];
        assert!(matches!(scanner.scan(&flood), Scan::Flood));
    }

    #[test]
    fn few_zpads_just_wait() {
        let mut scanner = Scanner::new();
        assert!(matches!(
            scanner.scan(&[frame::ZPAD, frame::ZPAD]),
            Scan::Wait
        ));
    }

    #[test]
    fn garbage_is_dropped() {
        let mut scanner = Scanner::new();
        assert!(matches!(scanner.scan(b"hello"), Scan::Garbage(1)));
    }
}

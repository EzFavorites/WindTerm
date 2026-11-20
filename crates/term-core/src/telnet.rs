//! Telnet 选项协商的**纯语义层**。
//!
//! 对齐公开 RFC：854（基础命令与 IAC 转义）、857（ECHO）、858（SGA）、
//! 1073（NAWS）。这里只做「原始字节流 → 纯数据 + 应答字节」的翻译，
//! 不碰 socket、不设定时器、不猜对端能力——真正把 `reply` 写回通道的是
//! `term-runtime`。因此本模块满足零 IO、零线程、零墙钟。
//!
//! # 协商策略
//!
//! 本端是终端客户端，只承认四个选项：
//! - 对端 `WILL ECHO` / `WILL SGA` → 接受（回 `DO`）；
//! - 对端 `DO SGA` / `DO NAWS` → 接受（回 `WILL`）；
//! - 其余一律拒绝：对 `WILL` 回 `DONT`，对 `DO` 回 `WONT`。
//!   特别是 `DO ECHO` 必须拒绝——回显由远端 shell 负责，客户端不做本地回显。
//!
//! 已生效的选项再次收到同一协商不回声，避免 `WILL`/`DO` 乒乓；这是
//! RFC 1143 状态机的最小可用子集（够用即可，不引入完整的队列语义）。
//!
//! # 子协商
//!
//! `IAC SB ... IAC SE` 整段从数据流里剥离，内容不进入 `data`。NAWS 由本端
//! 主动发送：`set_window_size` 返回需要写回的字节，编码时把每个 `0xFF`
//! 转义成 `IAC IAC`（RFC 854 §3、RFC 1073 §2）。
//!
//! # 已知边界
//!
//! - 只解析、不处理对端发来的 `SB NAWS`（方向相反，本端不是数据源）。
//! - 拒绝的选项不记忆，对端若反复发 `DO`/`WILL` 会反复收到 `WONT`/`DONT`
//!   ——这符合 RFC，且不会造成状态漂移。

/// 命令字节：所有 Telnet 命令都以它开头（RFC 854）。
pub const IAC: u8 = 255;
/// 表示「不要做 / 拒绝做」的否定应答（RFC 854）。
pub const DONT: u8 = 254;
/// 请求对端启用某选项（RFC 854）。
pub const DO: u8 = 253;
/// 表示「不会做」的本地否定应答（RFC 854）。
pub const WONT: u8 = 252;
/// 告知对端「本端将启用某选项」（RFC 854）。
pub const WILL: u8 = 251;
/// 子协商开始（RFC 854）。
pub const SB: u8 = 250;
/// 子协商结束（RFC 854）。
pub const SE: u8 = 240;
/// 远端回显选项（RFC 857）。
pub const ECHO: u8 = 1;
/// 抑制 Go Ahead（RFC 858）。
pub const SGA: u8 = 3;
/// 窗口尺寸选项（RFC 1073）。
pub const NAWS: u8 = 31;

/// 一次 `feed` 的产物：可以喂给 VT 的纯数据，以及需要写回远端的应答。
///
/// 两者分开是因为调用方要送去的方向不同：`data` 去 `vte`，`reply` 回 socket。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TelnetOutput {
    /// 剥掉全部 Telnet 命令后的数据字节（`IAC IAC` 已还原成单个 `0xFF`）。
    pub data: Vec<u8>,
    /// 需要原样写回远端的应答；没有协商时为空。
    pub reply: Vec<u8>,
}

/// 解析器状态。跨 `feed` 保留，命令被拆包也能正确接上。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum State {
    /// 普通数据。
    Data,
    /// 刚读到 `IAC`，等下一个命令字节。
    Iac,
    /// 已读到 `WILL` / `WONT` / `DO` / `DONT`，等选项字节。
    Negotiate(u8),
    /// `IAC SB` 之后，收集子协商内容直到 `IAC SE`。
    Sub,
    /// 子协商里刚读到 `IAC`。
    SubIac,
}

/// Telnet 协商状态机。每连接一个，`feed` 的返回按顺序写出即可。
#[derive(Clone, Debug)]
pub struct TelnetNegotiator {
    state: State,
    /// 当前子协商已收集的字节（仅用于跳过，不解释）。
    sub: Vec<u8>,
    /// 本端已同意的选项（`WILL` 方向）。
    local: [bool; 256],
    /// 对端已声明的选项（`DO` 方向）。
    remote: [bool; 256],
    /// 最近一次设置的窗口尺寸，供接受 `DO NAWS` 时补发。
    window: Option<(u16, u16)>,
}

impl Default for TelnetNegotiator {
    fn default() -> Self {
        Self::new()
    }
}

impl TelnetNegotiator {
    /// 全空的协商状态：没有任何选项生效。
    pub fn new() -> Self {
        Self {
            state: State::Data,
            sub: Vec::new(),
            local: [false; 256],
            remote: [false; 256],
            window: None,
        }
    }

    /// 喂入一段原始字节，返回可交给 VT 的数据与本轮应答。
    ///
    /// 不完整的前缀（如只有 `IAC`）留在状态里，下一次 `feed` 继续。
    pub fn feed(&mut self, input: &[u8]) -> TelnetOutput {
        let mut out = TelnetOutput::default();
        for &b in input {
            self.step(b, &mut out);
        }
        out
    }

    /// 记住窗口尺寸，并返回需要写回远端的 NAWS 子协商字节。
    ///
    /// 只有对端发过 `DO NAWS` 且本端已 `WILL` 时对端才会采纳这段字节；
    /// 是否现在发送由调用方按连接状态决定。
    pub fn set_window_size(&mut self, cols: u16, rows: u16) -> Vec<u8> {
        self.window = Some((cols, rows));
        naws_subnegotiation(cols, rows)
    }

    /// 最近一次设置的窗口尺寸；还没设置过就是 `None`。
    pub fn window_size(&self) -> Option<(u16, u16)> {
        self.window
    }

    /// 本端是否已同意某选项（`WILL` 生效）。
    pub fn local_enabled(&self, option: u8) -> bool {
        self.local[option as usize]
    }

    /// 对端是否已启用某选项（`DO` 生效）。
    pub fn remote_enabled(&self, option: u8) -> bool {
        self.remote[option as usize]
    }

    fn step(&mut self, b: u8, out: &mut TelnetOutput) {
        match self.state {
            State::Data => {
                if b == IAC {
                    self.state = State::Iac;
                } else {
                    out.data.push(b);
                }
            }
            State::Iac => {
                self.state = State::Data;
                match b {
                    // RFC 854 §3：数据里的 0xFF 写成两个 IAC。
                    IAC => out.data.push(IAC),
                    WILL | WONT | DO | DONT => self.state = State::Negotiate(b),
                    SB => {
                        self.sub.clear();
                        self.state = State::Sub;
                    }
                    // NOP / GA / DM / BRK 等单字节命令：无副作用，直接丢弃。
                    _ => {}
                }
            }
            State::Negotiate(verb) => {
                self.state = State::Data;
                self.negotiate(verb, b, out);
            }
            State::Sub => {
                if b == IAC {
                    self.state = State::SubIac;
                } else {
                    self.sub.push(b);
                }
            }
            State::SubIac => {
                self.state = State::Sub;
                match b {
                    SE => {
                        // 内容一律丢弃：本端不消费对端的子协商。
                        self.sub.clear();
                        self.state = State::Data;
                    }
                    // 子协商内部的 0xFF 同样双写转义。
                    IAC => self.sub.push(IAC),
                    // 畸形序列：保留字节、吞掉 IAC，避免把内容漏进数据流。
                    other => self.sub.push(other),
                }
            }
        }
    }

    fn negotiate(&mut self, verb: u8, option: u8, out: &mut TelnetOutput) {
        let i = option as usize;
        match verb {
            DO => {
                // 已生效时不回声，避免无限协商。
                if self.local[i] {
                    return;
                }
                if accepts_local(option) {
                    self.local[i] = true;
                    push_command(out, WILL, option);
                    // 对端刚获准读取 NAWS，补发当前尺寸。
                    if option == NAWS {
                        if let Some((cols, rows)) = self.window {
                            out.reply.extend(naws_subnegotiation(cols, rows));
                        }
                    }
                } else {
                    push_command(out, WONT, option);
                }
            }
            DONT => {
                if self.local[i] {
                    self.local[i] = false;
                    push_command(out, WONT, option);
                }
            }
            WILL => {
                if self.remote[i] {
                    return;
                }
                if accepts_remote(option) {
                    self.remote[i] = true;
                    push_command(out, DO, option);
                } else {
                    push_command(out, DONT, option);
                }
            }
            WONT => {
                if self.remote[i] {
                    self.remote[i] = false;
                    push_command(out, DONT, option);
                }
            }
            _ => {}
        }
    }
}

/// 本端愿意执行的选项：只负责 SGA 与上报窗口尺寸；不做本地回显。
fn accepts_local(option: u8) -> bool {
    matches!(option, SGA | NAWS)
}

/// 本端希望对端执行的选项：回显与抑制 Go Ahead。
fn accepts_remote(option: u8) -> bool {
    matches!(option, ECHO | SGA)
}

fn push_command(out: &mut TelnetOutput, verb: u8, option: u8) {
    out.reply.extend_from_slice(&[IAC, verb, option]);
}

/// 编码一条 NAWS 子协商：`IAC SB NAWS <cols> <rows> IAC SE`。
///
/// 尺寸按 u16 大端发送，字节值 `0xFF` 必须写成 `IAC IAC`（RFC 1073 §2），
/// 否则对端会把数据误当命令。这是纯函数，便于测试与复用。
pub fn naws_subnegotiation(cols: u16, rows: u16) -> Vec<u8> {
    let mut v = vec![IAC, SB, NAWS];
    push_u16(&mut v, cols);
    push_u16(&mut v, rows);
    v.extend_from_slice(&[IAC, SE]);
    v
}

fn push_u16(v: &mut Vec<u8>, value: u16) {
    for b in value.to_be_bytes() {
        v.push(b);
        if b == IAC {
            v.push(IAC);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn escaped_iac_becomes_a_literal_byte() {
        let mut n = TelnetNegotiator::new();
        let out = n.feed(&[b'a', IAC, IAC, b'b']);
        assert_eq!(out.data, vec![b'a', 0xFF, b'b']);
        assert!(out.reply.is_empty(), "转义不是协商，不该有应答");
    }

    #[test]
    fn do_echo_is_refused_with_wont() {
        let mut n = TelnetNegotiator::new();
        let out = n.feed(&[IAC, DO, ECHO]);
        assert!(out.data.is_empty());
        assert_eq!(out.reply, vec![IAC, WONT, ECHO], "客户端不做本地回显");
        assert!(!n.local_enabled(ECHO));
    }

    #[test]
    fn will_sga_is_accepted_with_do() {
        let mut n = TelnetNegotiator::new();
        let out = n.feed(&[IAC, WILL, SGA]);
        assert!(out.data.is_empty());
        assert_eq!(out.reply, vec![IAC, DO, SGA]);
        assert!(n.remote_enabled(SGA));
    }

    #[test]
    fn subnegotiation_is_stripped_from_data() {
        let mut n = TelnetNegotiator::new();
        let mut wire = b"x".to_vec();
        wire.extend_from_slice(&[IAC, SB, NAWS, 0, 80, 0, 24, IAC, SE]);
        wire.push(b'y');
        let out = n.feed(&wire);
        assert_eq!(out.data, b"xy".to_vec(), "SB/SE 内容不得漏进数据");
        assert!(out.reply.is_empty());
    }

    #[test]
    fn naws_encoding_escapes_0xff_bytes() {
        assert_eq!(
            naws_subnegotiation(80, 24),
            vec![IAC, SB, NAWS, 0, 80, 0, 24, IAC, SE]
        );
        // cols=0x00FF 的低字节 0xFF、rows=0xFFFF 的两个字节都要双写。
        assert_eq!(
            naws_subnegotiation(0x00FF, 0xFFFF),
            vec![IAC, SB, NAWS, 0x00, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, IAC, SE]
        );
    }

    #[test]
    fn mixed_data_and_commands_survive_a_round_trip() {
        let mut n = TelnetNegotiator::new();
        let mut wire = b"$ ".to_vec();
        wire.extend_from_slice(&[IAC, WILL, ECHO]);
        wire.extend_from_slice(b"ls\r\n");
        wire.extend_from_slice(&[IAC, IAC]);
        wire.extend_from_slice(&[IAC, WILL, SGA]);
        let out = n.feed(&wire);
        let mut expected = b"$ ls\r\n".to_vec();
        expected.push(0xFF);
        assert_eq!(out.data, expected);
        assert_eq!(out.reply, vec![IAC, DO, ECHO, IAC, DO, SGA]);
    }

    #[test]
    fn unknown_options_are_refused_without_panic() {
        let mut n = TelnetNegotiator::new();
        let out = n.feed(&[IAC, DO, 99, IAC, WILL, 99, IAC, 42, IAC, 249]);
        assert_eq!(out.reply, vec![IAC, WONT, 99, IAC, DONT, 99]);
        assert!(out.data.is_empty(), "NOP(249) 等单字节命令应被丢弃");
    }

    #[test]
    fn every_option_byte_is_handled() {
        let mut n = TelnetNegotiator::new();
        for option in 0..=u8::MAX {
            let out = n.feed(&[IAC, WILL, option]);
            assert!(out.data.is_empty(), "选项 {option} 的内容不该进数据流");
        }
    }

    #[test]
    fn commands_split_across_feeds_are_reassembled() {
        let mut n = TelnetNegotiator::new();
        assert!(n.feed(&[IAC]).reply.is_empty());
        assert!(n.feed(&[DO]).reply.is_empty());
        assert_eq!(n.feed(&[SGA]).reply, vec![IAC, WILL, SGA]);
        assert!(n.local_enabled(SGA));
    }

    #[test]
    fn unterminated_subnegotiation_does_not_leak_data() {
        let mut n = TelnetNegotiator::new();
        let out = n.feed(&[b'a', IAC, SB, NAWS, 0, 80]);
        assert_eq!(out.data, b"a".to_vec(), "SB 未结束前不得恢复数据");
        let out = n.feed(&[0, 24, IAC, SE, b'b']);
        assert_eq!(out.data, b"b".to_vec(), "SE 之后才恢复");
    }

    #[test]
    fn accepted_naws_also_sends_the_current_size() {
        let mut n = TelnetNegotiator::new();
        let seeded = n.set_window_size(120, 40);
        assert_eq!(n.window_size(), Some((120, 40)));
        let out = n.feed(&[IAC, DO, NAWS]);
        let mut expected = vec![IAC, WILL, NAWS];
        expected.extend(seeded);
        assert_eq!(out.reply, expected);
        assert!(n.local_enabled(NAWS));
    }

    #[test]
    fn repeated_negotiation_is_not_echoed_and_dont_turns_it_off() {
        let mut n = TelnetNegotiator::new();
        assert_eq!(n.feed(&[IAC, DO, SGA]).reply, vec![IAC, WILL, SGA]);
        assert!(
            n.feed(&[IAC, DO, SGA]).reply.is_empty(),
            "已生效的选项不回声"
        );
        assert_eq!(n.feed(&[IAC, DONT, SGA]).reply, vec![IAC, WONT, SGA]);
        assert!(!n.local_enabled(SGA));
    }
}

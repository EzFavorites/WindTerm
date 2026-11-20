//! 串口通道：把 [`serial::Serial`] 包成一条 `TermChannel`。
//!
//! 三条语义与 PTY 通道**不一样**，接线时最容易按老直觉写错：
//!
//! 1. **没有子进程**。`poll()` 不报"进程退出码"，只在**拔线时**报一次
//!    `Some("serial disconnected: <设备>")`，之后恒 `None`；`finished()` 与它
//!    同源。判活的依据是 `Serial::is_connected()`（termios 探活 + 读到的 EOF），
//!    **不是** `read` 返回 0——空闲一帧就判死是这类通道最经典的 bug。
//! 2. **写不一定一次写完**。硬件流控（RTS/CTS）或 XON/XOFF 卡住时，
//!    `Serial::write_all` 只写得出去一部分；剩下的必须留在 [`SerialChannel`]
//!    自己的缓冲里，下一帧先冲缓冲。缓冲上限 256 KiB，超了**丢最新的**并
//!    记进 `dropped`——丢新的不会把已排队的字节折断。
//! 3. **`resize` 什么都不发**。串口的行列由设备侧决定，这里只记下来
//!    （trait 要求有这个方法），一个字节都不会写到线上。
//!
//! RX/TX 计数直接取 [`serial::Serial::counters`]，状态栏的补充说明由
//! [`TermChannel::status_note`] 给。

use std::io::ErrorKind;

use term_core::session::SessionConfig;

use crate::channel::TermChannel;
use crate::serial::{self, Flow, Parity, Serial, SerialConfig};

/// `drain()` 一帧最多搬这么多字节：多条命令灌进来时不能让一次 drain 无限增长。
const FRAME_LIMIT: usize = 64 * 1024;

/// 写缓冲上限。超了就丢**最新**的字节（已排队的不能折断）。
const WRITE_LIMIT: usize = 256 * 1024;

/// 一次 `read` 的临时缓冲：串口单次就绪的数据通常远小于这个数。
const READ_CHUNK: usize = 8 * 1024;

/// 把 [`Serial`] 包成一个 `TermChannel`。
pub struct SerialChannel {
    serial: Serial,
    /// 通道结束的**一次性**说明（`poll()` 取走即清空）。
    notice: Option<String>,
    /// 生命周期是否已结束：`poll` 报过断开 / `close` 过 / 端口读写报过错。
    over: bool,
    /// 因流控没写出去、等着下一帧补写的字节。
    pending: Vec<u8>,
    /// 缓冲溢出或端口坏掉而丢掉的字节数（只增不减）。
    dropped: u64,
    /// 串口没有窗口尺寸概念，但 `TermChannel` 要求记住它。
    size: (usize, usize),
    description: String,
}

impl SerialChannel {
    /// 打开一个串口并包装成通道。参数不合法时在碰设备之前就失败。
    pub fn open(cfg: &SerialConfig) -> Result<Self, serial::SerialError> {
        Ok(Self {
            serial: serial::open(cfg)?,
            notice: None,
            over: false,
            pending: Vec::new(),
            dropped: 0,
            size: (0, 0),
            description: describe(cfg),
        })
    }

    /// 记录在案的窗口尺寸（串口会话用不到，只给测试和诊断看）。
    pub fn size(&self) -> (usize, usize) {
        self.size
    }

    /// 因缓冲溢出 / 端口故障丢掉的字节数。
    pub fn dropped(&self) -> u64 {
        self.dropped
    }

    /// 结束这次会话，并留下**一次**给用户看的原因。
    fn end(&mut self, why: String) {
        if self.notice.is_none() {
            self.notice = Some(why);
        }
        self.over = true;
        self.pending.clear();
    }

    fn device(&self) -> &str {
        &self.serial.config().device
    }

    /// 先把积压的写出去。流控堵着就原样留着，端口坏了就收摊。
    fn flush(&mut self) {
        if self.pending.is_empty() {
            return;
        }
        match self.serial.write_all(&self.pending) {
            Ok(0) => {}
            Ok(n) => {
                self.pending.drain(..n);
            }
            Err(serial::SerialError::Io(ErrorKind::WouldBlock)) => {}
            Err(err) => {
                let why = format!("serial write failed: {}: {err}", self.device());
                self.end(why);
            }
        }
    }

    /// 直接写，写不完的尾巴进缓冲。
    fn push_out(&mut self, bytes: &[u8]) {
        match self.serial.write_all(bytes) {
            Ok(n) if n == bytes.len() => {}
            Ok(n) => self.queue(&bytes[n..]),
            Err(serial::SerialError::Io(ErrorKind::WouldBlock)) => self.queue(bytes),
            Err(err) => {
                let why = format!("serial write failed: {}: {err}", self.device());
                self.end(why);
            }
        }
    }

    /// 排队，最多排到 [`WRITE_LIMIT`]；放不下的算丢弃。
    fn queue(&mut self, bytes: &[u8]) {
        let (take, drop) = admit(self.pending.len(), bytes.len(), WRITE_LIMIT);
        self.pending.extend_from_slice(&bytes[..take]);
        self.dropped += drop as u64;
    }
}

impl TermChannel for SerialChannel {
    fn drain(&mut self) -> Vec<u8> {
        let mut out = Vec::new();
        if self.over {
            return out;
        }
        let mut buf = [0u8; READ_CHUNK];
        while out.len() < FRAME_LIMIT {
            let room = FRAME_LIMIT - out.len();
            let chunk = &mut buf[..room.min(READ_CHUNK)];
            match self.serial.read(chunk) {
                // 内核说"没有更多"：本轮到此为止。是不是拔线由 `is_connected` 判。
                Ok(0) => break,
                Ok(n) => out.extend_from_slice(&chunk[..n]),
                // 硬错误（EIO 之类）：本轮收工，`poll` 会把断开报出去。
                Err(_) => break,
            }
        }
        out
    }

    fn write(&mut self, bytes: &[u8]) {
        if self.over {
            return;
        }
        self.flush();
        if self.pending.is_empty() {
            self.push_out(bytes);
        } else {
            // 上一帧还堵着：新字节排到队尾，不插队。
            self.queue(bytes);
        }
    }

    fn resize(&mut self, cols: usize, rows: usize) {
        // 串口没有窗口尺寸：只记着，不往线上发任何字节。
        self.size = (cols, rows);
    }

    fn poll(&mut self) -> Option<String> {
        if !self.over && !self.serial.is_connected() {
            let why = format!("serial disconnected: {}", self.device());
            self.end(why);
        }
        self.notice.take()
    }

    fn finished(&self) -> bool {
        self.over
    }

    fn close(&mut self) {
        self.over = true;
        self.notice = None;
        self.pending.clear();
        self.serial.close();
    }

    fn description(&self) -> &str {
        &self.description
    }

    fn counters(&self) -> Option<(u64, u64)> {
        Some(self.serial.counters())
    }

    fn status_note(&self) -> Option<String> {
        let (rx, tx) = self.serial.counters();
        Some(format!(
            "rx {} / tx {}, dropped {}",
            human_bytes(rx),
            human_bytes(tx),
            self.dropped
        ))
    }
}

/// 会话配置 → 串口参数。设备路径就是 `SessionConfig::host`（表单上那一格）。
pub fn config_of(config: &SessionConfig) -> SerialConfig {
    SerialConfig {
        device: config.host.trim().to_string(),
        baud: config.baud_rate,
        data_bits: config.data_bits,
        stop_bits: config.stop_bits,
        parity: parity_of(&config.parity),
        flow: flow_of(&config.flow_control),
        ..SerialConfig::default()
    }
}

/// 会话里存的是界面上的显示字符串（"None" / "Odd" / "Even"），认不出的按
/// "没设"处理——串口面板的默认就是 None，不能因为多了一个空格就开不了口。
fn parity_of(text: &str) -> Parity {
    let tag = text.trim().to_ascii_lowercase();
    if tag.starts_with('o') {
        Parity::Odd
    } else if tag.starts_with('e') {
        Parity::Even
    } else {
        Parity::None
    }
}

/// `"RTS/CTS"` / `"Hardware"` → 硬件流控，`"XON/XOFF"` / `"Software"` → 软件流控。
fn flow_of(text: &str) -> Flow {
    let tag = text.trim().to_ascii_lowercase();
    if tag.contains("rts") || tag.contains("hard") {
        Flow::Hardware
    } else if tag.contains("xon") || tag.contains("soft") {
        Flow::Software
    } else {
        Flow::None
    }
}

/// `serial /dev/cu.usbserial-1420 @115200 8N1 [RTS/CTS]`：一眼看出连的哪个口、
/// 什么参数。行格式写成 `8E1` 这种串口行业写法，流控非默认时才追加。
fn describe(cfg: &SerialConfig) -> String {
    let parity = match cfg.parity {
        Parity::None => 'N',
        Parity::Odd => 'O',
        Parity::Even => 'E',
    };
    let mut text = format!(
        "serial {} @{} {}{}{}",
        cfg.device, cfg.baud, cfg.data_bits, parity, cfg.stop_bits
    );
    match cfg.flow {
        Flow::None => {}
        Flow::Software => text.push_str(" XON/XOFF"),
        Flow::Hardware => text.push_str(" RTS/CTS"),
    }
    text
}

/// `12.3 KB` / `4 B`：状态栏那一行里放得下的写法。
fn human_bytes(n: u64) -> String {
    const KIB: f64 = 1024.0;
    let value = n as f64;
    if n < 1024 {
        format!("{n} B")
    } else if value < KIB * KIB {
        format!("{:.1} KB", value / KIB)
    } else {
        format!("{:.1} MB", value / (KIB * KIB))
    }
}

/// 缓冲上限之内还能收多少字节、因此丢多少（丢的是**最新**的那一截）。
fn admit(pending: usize, incoming: usize, limit: usize) -> (usize, usize) {
    let room = limit.saturating_sub(pending);
    let take = incoming.min(room);
    (take, incoming - take)
}

#[cfg(test)]
mod tests {
    use super::*;
    use term_core::session::Protocol;

    #[test]
    fn description_names_device_baud_and_frame() {
        let cfg = SerialConfig {
            device: "/dev/cu.usbserial-1420".to_string(),
            baud: 115_200,
            ..SerialConfig::default()
        };
        assert_eq!(describe(&cfg), "serial /dev/cu.usbserial-1420 @115200 8N1");
        let even = SerialConfig {
            parity: Parity::Even,
            flow: Flow::Hardware,
            ..cfg.clone()
        };
        assert_eq!(describe(&even), "serial /dev/cu.usbserial-1420 @115200 8E1 RTS/CTS");
        let xon = SerialConfig {
            flow: Flow::Software,
            ..cfg
        };
        assert!(describe(&xon).ends_with(" 8N1 XON/XOFF"), "{}", describe(&xon));
    }

    #[test]
    fn session_strings_become_enum_params() {
        let mut config = SessionConfig::new(Protocol::Serial);
        config.host = "  /dev/ttyUSB0 ".to_string();
        config.baud_rate = 57_600;
        config.data_bits = 7;
        config.stop_bits = 2;
        config.parity = "Even".to_string();
        config.flow_control = "RTS/CTS".to_string();

        let cfg = config_of(&config);
        assert_eq!(cfg.device, "/dev/ttyUSB0", "设备路径要去掉两端空白");
        assert_eq!((cfg.baud, cfg.data_bits, cfg.stop_bits), (57_600, 7, 2));
        assert_eq!((cfg.parity, cfg.flow), (Parity::Even, Flow::Hardware));

        let mut plain = SessionConfig::new(Protocol::Serial);
        assert_eq!(config_of(&plain).flow, Flow::None);
        plain.parity = "odd".to_string();
        plain.flow_control = "XON/XOFF".to_string();
        assert_eq!((config_of(&plain).parity, config_of(&plain).flow), (Parity::Odd, Flow::Software));
    }

    #[test]
    fn status_note_reads_like_a_status_bar() {
        assert_eq!(human_bytes(0), "0 B");
        assert_eq!(human_bytes(4), "4 B");
        assert_eq!(human_bytes(1023), "1023 B");
        assert_eq!(human_bytes(12_600), "12.3 KB");
        assert_eq!(human_bytes(1024 * 1024 * 3 / 2), "1.5 MB");
    }

    #[test]
    fn write_buffer_drops_the_newest_bytes() {
        assert_eq!(admit(0, 10, WRITE_LIMIT), (10, 0));
        assert_eq!(admit(WRITE_LIMIT, 10, WRITE_LIMIT), (0, 10), "满了丢最新的");
        assert_eq!(admit(WRITE_LIMIT - 4, 10, WRITE_LIMIT), (4, 6));
        assert_eq!(admit(0, WRITE_LIMIT + 1, WRITE_LIMIT), (WRITE_LIMIT, 1));
    }

    // ---- 用 PTY 当假串口（不需要真硬件；也绝不打开本机任何 /dev/cu.*）----

    #[cfg(unix)]
    struct FakePort {
        master: Option<std::os::unix::io::RawFd>,
        slave: std::os::unix::io::RawFd,
        path: String,
    }

    #[cfg(unix)]
    impl FakePort {
        fn open() -> Option<Self> {
            let pair = nix::pty::openpty(
                None::<&nix::pty::Winsize>,
                None::<&nix::sys::termios::Termios>,
            )
            .ok()?;
            let path = nix::unistd::ttyname(pair.slave)
                .ok()?
                .to_string_lossy()
                .into_owned();
            // master 读要么读到数据要么 EAGAIN，不能把测试挂死。
            let flags = nix::fcntl::fcntl(pair.master, nix::fcntl::FcntlArg::F_GETFL).ok()?;
            let mut oflag = nix::fcntl::OFlag::from_bits_truncate(flags);
            oflag.insert(nix::fcntl::OFlag::O_NONBLOCK);
            nix::fcntl::fcntl(pair.master, nix::fcntl::FcntlArg::F_SETFL(oflag)).ok()?;
            Some(Self {
                master: Some(pair.master),
                slave: pair.slave,
                path,
            })
        }

        fn write(&self, bytes: &[u8]) {
            if let Some(fd) = self.master {
                let _ = nix::unistd::write(fd, bytes);
            }
        }

        /// 在 `budget` 内读一段（读不到就是空）。
        fn read(&self, budget: std::time::Duration) -> Vec<u8> {
            let deadline = std::time::Instant::now() + budget;
            let mut out = Vec::new();
            let mut buf = [0u8; 512];
            while out.is_empty() {
                let Some(fd) = self.master else { break };
                match nix::unistd::read(fd, &mut buf) {
                    Ok(n) => out.extend_from_slice(&buf[..n]),
                    Err(nix::errno::Errno::EAGAIN) | Err(nix::errno::Errno::EINTR) => {
                        if std::time::Instant::now() >= deadline {
                            break;
                        }
                        std::thread::sleep(std::time::Duration::from_millis(2));
                    }
                    Err(_) => break,
                }
            }
            out
        }

        /// 模拟拔线：关掉 master，slave 侧的 `read` 从此返回 0（或 EIO）。
        fn unplug(&mut self) {
            if let Some(fd) = self.master.take() {
                let _ = nix::unistd::close(fd);
            }
        }
    }

    #[cfg(unix)]
    impl Drop for FakePort {
        fn drop(&mut self) {
            self.unplug();
            let _ = nix::unistd::close(self.slave);
        }
    }

    #[cfg(unix)]
    fn open_on(port: &FakePort) -> Option<SerialChannel> {
        let cfg = SerialConfig {
            device: port.path.clone(),
            ..SerialConfig::default()
        };
        match SerialChannel::open(&cfg) {
            Ok(ch) => Some(ch),
            Err(err) => {
                // 平台限制：有的系统不允许把 pty 当串口配 termios。
                eprintln!("跳过：这台机器上的 pty 不能当假串口（{err}）");
                None
            }
        }
    }

    #[cfg(unix)]
    #[test]
    fn pty_backed_channel_moves_bytes_and_counts_them() {
        let Some(port) = FakePort::open() else {
            eprintln!("跳过：拿不到 pty 从设备路径");
            return;
        };
        let Some(mut ch) = open_on(&port) else { return };

        port.write(b"hello\n");
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
        let mut got = Vec::new();
        while !got.ends_with(b"hello\n") {
            got.extend(ch.drain());
            if !got.ends_with(b"hello\n") {
                assert!(std::time::Instant::now() < deadline, "drain 没拿到 master 写的字节");
                std::thread::sleep(std::time::Duration::from_millis(2));
            }
        }

        ch.write(b"world");
        assert_eq!(port.read(std::time::Duration::from_secs(2)), b"world");
        assert_eq!(ch.counters(), Some((6, 5)), "计数要等于实际进出的字节数");
        let note = ch.status_note().unwrap_or_default();
        assert!(note.contains("rx 6 B") && note.contains("tx 5 B"), "{note}");
        assert!(note.contains("dropped 0"), "{note}");
    }

    #[cfg(unix)]
    #[test]
    fn pty_hangup_is_reported_once_by_poll() {
        let Some(mut port) = FakePort::open() else {
            eprintln!("跳过：拿不到 pty 从设备路径");
            return;
        };
        let Some(mut ch) = open_on(&port) else { return };
        assert!(ch.poll().is_none(), "还连着的时候不该报断开");
        assert!(!ch.finished());

        port.unplug();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
        let mut note = None;
        while note.is_none() {
            // 与 supervisor 同序：先 drain 撞到 EOF/EIO，再 poll 判定。
            let _ = ch.drain();
            note = ch.poll();
            if note.is_none() {
                assert!(std::time::Instant::now() < deadline, "拔线后 poll 没有报出断开");
                std::thread::sleep(std::time::Duration::from_millis(2));
            }
        }
        let text = note.unwrap_or_default();
        assert!(text.contains("serial disconnected"), "{text}");
        assert!(text.contains(&port.path), "文案要带上设备路径：{text}");
        assert!(ch.finished());
        assert_eq!(ch.poll(), None, "断开只报一次");
    }

    #[cfg(unix)]
    #[test]
    fn resize_is_recorded_but_never_reaches_the_wire() {
        let Some(port) = FakePort::open() else {
            eprintln!("跳过：拿不到 pty 从设备路径");
            return;
        };
        let Some(mut ch) = open_on(&port) else { return };
        ch.resize(120, 40);
        assert_eq!(ch.size(), (120, 40));
        assert!(
            port.read(std::time::Duration::from_millis(80)).is_empty(),
            "串口没有窗口尺寸，resize 一个字节都不该发"
        );
    }

    #[cfg(unix)]
    #[test]
    fn close_finishes_the_channel_without_a_second_notice() {
        let Some(port) = FakePort::open() else {
            eprintln!("跳过：拿不到 pty 从设备路径");
            return;
        };
        let Some(mut ch) = open_on(&port) else { return };
        ch.close();
        assert!(ch.finished());
        assert_eq!(ch.poll(), None);
        assert!(ch.counters().is_some(), "关掉之后计数仍然可读");
    }
}

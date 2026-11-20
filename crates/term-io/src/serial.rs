//! 原生串口后端（Windows / macOS / Linux）。
//!
//! 后端是 `serialport`（serialport-rs）：Windows 上它走 `CreateFileW` + `DCB` +
//! `COMMTIMEOUTS`，unix 上走 termios + `ppoll`。本模块只负责**语义**——把轮询模型
//! 需要的那几条保证翻译成统一行为，与平台无关：
//!
//! - **非阻塞读**：`timeout = 0`，没数据时后端报 `TimedOut`，这里翻译成 `Ok(0)`。
//!   轮询模型下"这轮没数据"是常态，不是错误。
//! - **挂断**：unix 侧后端报 `BrokenPipe`（`poll` 的 `POLLHUP`），Windows 侧设备拔掉
//!   后 `ReadFile` 直接失败。两者都记进 `hangup`，[`Serial::is_connected`] 立刻为假。
//! - **计数**：`rx` / `tx` 单调递增，[`Serial::reconnect`] **不清零**——否则状态栏
//!   会在重连那一刻跳回 0。
//! - **热插拔**：Windows 上没有 unix 的 fd 概念，句柄留在 `Option<Box<dyn SerialPort>>`
//!   里；`close()` 把它置 `None`，之后读写返回 `NotConnected`，与旧实现 fd=-1 同义。
//!
//! 为什么不自己写：这段曾经是手写 termios + `nix`，2026-11-16 补 Windows 时发现
//! 要另写一套 `CreateFile`/`DCB`/`COMMTIMEOUTS`/`ClearCommError`，而 `serialport`
//! 已经把三平台都做了（Apache-2.0/MIT），且 unix 侧与手写版等价（`O_NOCTTY |
//! O_NONBLOCK | O_CLOEXEC` + `cfmakeraw` + 设置回读校验）。手写版留在 git 历史里。

use std::fmt;
use std::io::{Read, Write};
use std::time::Duration;

use serialport::{
    ClearBuffer, DataBits, FlowControl, Parity as SpParity, SerialPort, StopBits as SpStopBits,
};

/// 支持的波特率。
///
/// 平台无关的一张表：`serialport` 在 Windows 上其实接受任意 `BaudRate`，但两边
/// 用同一张表，错误文案与下拉框才能一致；表外一律报 [`SerialError::UnsupportedBaud`]，
/// **不退回 9600**——那样用户会以为连上了，实际每一帧都是错的。
pub const SUPPORTED_BAUDS: &[u32] = &[9600, 19200, 38400, 57600, 115200, 230400];

/// 校验位：串口面板上的 None / Odd / Even。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Parity {
    #[default]
    None,
    Odd,
    Even,
}

/// 流控：`Software` 是 XON/XOFF，`Hardware` 是 RTS/CTS。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Flow {
    #[default]
    None,
    Software,
    Hardware,
}

/// 打开一个串口需要的全部参数。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SerialConfig {
    /// `COM3`、`/dev/ttyUSB0`、`/dev/cu.usbserial-1420`、`/dev/serial/by-id/...`。
    pub device: String,
    pub baud: u32,
    /// 5..=8。
    pub data_bits: u8,
    /// 1 或 2。
    pub stop_bits: u8,
    pub parity: Parity,
    pub flow: Flow,
    /// 打开时是否拉高 DTR（有些板子把它当复位以外的握手线用）。
    pub dtr: bool,
    pub rts: bool,
}

impl Default for SerialConfig {
    fn default() -> Self {
        // 115200 8N1、无流控是绝大多数 USB 转串口模块的出厂状态。
        Self {
            device: String::new(),
            baud: 115_200,
            data_bits: 8,
            stop_bits: 1,
            parity: Parity::None,
            flow: Flow::None,
            dtr: true,
            rts: true,
        }
    }
}

impl SerialConfig {
    /// 纯校验：不碰文件系统、不打开设备，所以能在没有串口的机器上逐条测。
    ///
    /// 参数类错误统一是 [`SerialError::Io`] + `InvalidInput`：调用方对它的反应
    /// 都是"让用户改表单"，不必再细分（人话提醒在 `Display` 里）。
    pub fn validate(&self) -> Result<(), SerialError> {
        if self.device.trim().is_empty() {
            return Err(SerialError::NotFound(self.device.clone()));
        }
        if !SUPPORTED_BAUDS.contains(&self.baud) {
            return Err(SerialError::UnsupportedBaud(self.baud));
        }
        if !(5..=8).contains(&self.data_bits) || !matches!(self.stop_bits, 1 | 2) {
            return Err(SerialError::Io(std::io::ErrorKind::InvalidInput));
        }
        // RTS/CTS 按字节边界握手，5~7 位时一个字节凑不满 8 位，流控位的时序对不上。
        // 这是硬件事实，宁可拒绝也不发出去静默丢字节。
        if self.flow == Flow::Hardware && self.data_bits != 8 {
            return Err(SerialError::Io(std::io::ErrorKind::InvalidInput));
        }
        Ok(())
    }
}

/// 串口错误的**类别**。调用方按类别决定反应：`NotFound` / `Busy` 可以等热插拔
/// 或提示用户，`PermissionDenied` 给一条能照着敲的命令，`UnsupportedBaud`
/// 退回选择列表。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SerialError {
    /// 设备不存在（还没插 / 名字错 / 插上又拔了）。
    NotFound(String),
    /// 没权限（Linux 上常见：当前用户不在 `dialout` 组）。
    PermissionDenied(String),
    /// 已被别的进程占用。
    Busy(String),
    /// 其余系统错误，只留 `ErrorKind`。
    Io(std::io::ErrorKind),
    UnsupportedBaud(u32),
}

impl fmt::Display for SerialError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotFound(device) => write!(
                f,
                "串口设备不存在：{device}。确认设备插好、名字正确（Windows 形如 COM3，\
                 macOS 形如 /dev/cu.usbserial-1420，Linux 形如 /dev/ttyUSB0）"
            ),
            Self::PermissionDenied(device) => write!(
                f,
                "没有权限打开串口 {device}。Linux 上把自己加进 dialout 组：\
                 sudo usermod -aG dialout $USER（重新登录后生效）；\
                 Windows 上确认没有别的程序独占该端口"
            ),
            Self::Busy(device) => write!(
                f,
                "串口 {device} 已被其它程序占用。关掉 minicom / screen / cu / 串口助手等\
                 占用者后再试"
            ),
            Self::Io(std::io::ErrorKind::InvalidInput) => f.write_str(
                "串口参数不合法：数据位须 5..8、停止位 1/2；硬件流控（RTS/CTS）要求 8 数据位",
            ),
            Self::Io(std::io::ErrorKind::NotConnected) => {
                f.write_str("串口未打开：上一次重连没有成功，请重新连接设备")
            }
            Self::Io(kind) => write!(f, "串口 I/O 错误：{kind}"),
            Self::UnsupportedBaud(baud) => {
                write!(f, "不支持的波特率 {baud}；本平台可用：{}", baud_table())
            }
        }
    }
}

impl std::error::Error for SerialError {}

/// `9600 / 19200 / ...`：错误文案里要让人看见可选项。
fn baud_table() -> String {
    SUPPORTED_BAUDS.iter().map(u32::to_string).collect::<Vec<_>>().join(" / ")
}

/// 打开一个串口。参数不合法时在碰设备**之前**就失败。
pub fn open(cfg: &SerialConfig) -> Result<Serial, SerialError> {
    cfg.validate()?;
    let port = open_port(cfg)?;
    Ok(Serial {
        cfg: cfg.clone(),
        port: Some(port),
        rx: 0,
        tx: 0,
        hangup: false,
    })
}

/// 按 `SerialConfig` 打开并配置。失败时没有任何句柄泄漏（`Box` 就地丢弃）。
fn open_port(cfg: &SerialConfig) -> Result<Box<dyn SerialPort>, SerialError> {
    let mut port = serialport::new(cfg.device.trim(), cfg.baud)
        .data_bits(data_bits(cfg.data_bits))
        .parity(sp_parity(cfg.parity))
        .stop_bits(stop_bits(cfg.stop_bits))
        .flow_control(flow_control(cfg.flow))
        // 0 = 不等待：读/写立刻返回，没数据由上层轮询。见文件头的「非阻塞读」。
        .timeout(Duration::ZERO)
        .open()
        .map_err(|err| classify(&cfg.device, err))?;
    // 同名设备（尤其是拔插后复用 COM 号的）缓冲区里可能残留上一次会话的字节。
    // 串口会话的第一帧不该是别人的尾巴。
    port.clear(ClearBuffer::All).map_err(|err| classify(&cfg.device, err))?;
    // DTR/RTS 拨不动**不算**打开失败：虚拟串口没有 modem 控制线，pty 也是。
    // 旧实现按 errno 放行 ENOTTY/EINVAL，而 `serialport` 把 errno 折成 io::ErrorKind
    // 后已分不出这两者，所以这里一律容忍——拨不动不影响收发。
    let _ = port.write_data_terminal_ready(cfg.dtr);
    let _ = port.write_request_to_send(cfg.rts);
    Ok(port)
}

/// 数据位 → 后端的枚举。`validate()` 挡住了 5..=8 以外的值，`_` 只能落到 8。
fn data_bits(bits: u8) -> DataBits {
    match bits {
        5 => DataBits::Five,
        6 => DataBits::Six,
        7 => DataBits::Seven,
        _ => DataBits::Eight,
    }
}

fn sp_parity(parity: Parity) -> SpParity {
    match parity {
        Parity::None => SpParity::None,
        Parity::Odd => SpParity::Odd,
        Parity::Even => SpParity::Even,
    }
}

fn stop_bits(bits: u8) -> SpStopBits {
    if bits == 2 {
        SpStopBits::Two
    } else {
        SpStopBits::One
    }
}

fn flow_control(flow: Flow) -> FlowControl {
    match flow {
        Flow::None => FlowControl::None,
        Flow::Software => FlowControl::Software,
        Flow::Hardware => FlowControl::Hardware,
    }
}

/// 列出当前系统上像串口的设备路径（字典序、去重）。**不打开设备**——它会在
/// "选择串口"下拉框打开时被调用，不能去抢端口。
///
/// Windows 用后端的 `available_ports()`（内部走 SetupAPI，与 `libudev` 无关）；
/// unix 保持自己扫 `/dev`：`serialport` 在 Linux 上要 `libudev` 才枚举，我们已经
/// 关掉了那个特性（见 `Cargo.toml`），而扫目录本来就是这个模块在做的事。
pub fn list_devices() -> Vec<String> {
    #[cfg(windows)]
    let mut out = windows_candidates();
    #[cfg(target_os = "linux")]
    let mut out = linux_candidates();
    #[cfg(target_os = "macos")]
    let mut out = mac_candidates();
    #[cfg(not(any(windows, target_os = "linux", target_os = "macos")))]
    let mut out: Vec<String> = Vec::new();
    out.sort();
    out.dedup();
    out
}

/// Windows：`COM3`、`COM12`……枚举失败就是空列表（下拉框允许手输）。
#[cfg(windows)]
fn windows_candidates() -> Vec<String> {
    serialport::available_ports()
        .map(|ports| ports.into_iter().map(|info| info.port_name).collect())
        .unwrap_or_default()
}

/// 读目录里的文件名；读不到就是空（`/dev/serial/by-id` 没设备时根本不存在）。
#[cfg(any(target_os = "linux", target_os = "macos"))]
fn read_names(dir: &str) -> Vec<String> {
    let Ok(entries) = std::fs::read_dir(dir) else { return Vec::new() };
    let names = entries.flatten().map(|e| e.file_name().to_string_lossy().into_owned());
    names.collect()
}

/// Linux：`/dev/serial/by-id` 下的条目带芯片序列号，换 USB 口也不变，优先列出；
/// 再补上内核按驱动命名的 `ttyUSB*` / `ttyACM*`（平台 UART `ttyAMA*` 同理）。
#[cfg(target_os = "linux")]
fn linux_candidates() -> Vec<String> {
    let mut out: Vec<String> = read_names("/dev/serial/by-id")
        .iter()
        .map(|name| format!("/dev/serial/by-id/{name}"))
        .collect();
    let tty = read_names("/dev").into_iter().filter(|name| {
        name.starts_with("ttyUSB") || name.starts_with("ttyACM") || name.starts_with("ttyAMA")
    });
    out.extend(tty.map(|name| format!("/dev/{name}")));
    out
}

/// macOS：只认 `cu.` 前缀。`tty.` 那组会**挡住别人**用同一设备（它要求独占，
/// 且会等 DCD），而 `cu.` 正是"我主动拨出去"的语义。
#[cfg(target_os = "macos")]
fn mac_candidates() -> Vec<String> {
    read_names("/dev")
        .into_iter()
        .filter(|name| name.starts_with("cu.") && !is_builtin_console(name))
        .map(|name| format!("/dev/{name}"))
        .collect()
}

/// `cu.debug-console` 与 `cu.Bluetooth-Incoming-Port` 是系统自带的控制台，不是
/// 用户插的设备。列进下拉框只会让人误选（选 debug-console 会把自己赶出去）。
#[cfg(target_os = "macos")]
fn is_builtin_console(name: &str) -> bool {
    name.starts_with("cu.debug-console") || name.starts_with("cu.Bluetooth")
}

/// 一个打开着的串口。
pub struct Serial {
    cfg: SerialConfig,
    /// `None` = 已关闭（`reconnect` 失败后），读写都返回 `NotConnected`。
    port: Option<Box<dyn SerialPort>>,
    /// 累计收发字节，单调递增——`reconnect` 不清零，否则重连那一刻状态栏会跳。
    rx: u64,
    tx: u64,
    /// 后端已经说过"对端没了"或读写报过错。见 [`Serial::is_connected`]。
    hangup: bool,
}

/// 非阻塞写：连续这么多次都写不动就停手，把已写数量交回去。
const WRITE_STALL_LIMIT: u32 = 64;

impl Serial {
    /// 非阻塞读。没有数据（后端报 `TimedOut`）返回 `Ok(0)` 而不是错误。
    ///
    /// `BrokenPipe`（`POLLHUP`）与 `Ok(0)` 都表示对端没了：记进 `hangup`，之后
    /// [`is_connected`](Self::is_connected) 变假——pty 与部分虚拟串口只有这一条
    /// 挂断信号（`bytes_to_read` 照样成功），所以不能只靠探活。
    pub fn read(&mut self, buf: &mut [u8]) -> Result<usize, SerialError> {
        let Some(port) = self.port.as_mut() else {
            return Err(SerialError::Io(std::io::ErrorKind::NotConnected));
        };
        match port.read(buf) {
            Ok(0) => {
                if !buf.is_empty() {
                    self.hangup = true;
                }
                Ok(0)
            }
            Ok(n) => {
                self.rx += n as u64;
                Ok(n)
            }
            Err(err) if err.kind() == std::io::ErrorKind::TimedOut => Ok(0),
            Err(err) if err.kind() == std::io::ErrorKind::Interrupted => Ok(0),
            Err(err) if err.kind() == std::io::ErrorKind::BrokenPipe => {
                self.hangup = true;
                Ok(0)
            }
            Err(err) => {
                self.hangup = true;
                Err(classify_kind(&self.cfg.device, err.kind()))
            }
        }
    }

    /// 全量写，返回**实际写出**的字节数。
    ///
    /// 非阻塞写可能只出去一部分：写不动时让出 CPU 重试，连续
    /// [`WRITE_STALL_LIMIT`] 次都写不动就停手，把已写数量交回去——
    /// 下帧继续还是判定端口挂了，由调用方决定。
    pub fn write_all(&mut self, bytes: &[u8]) -> Result<usize, SerialError> {
        let Some(port) = self.port.as_mut() else {
            return Err(SerialError::Io(std::io::ErrorKind::NotConnected));
        };
        let (mut done, mut stalls) = (0usize, 0u32);
        while done < bytes.len() {
            match port.write(&bytes[done..]) {
                Ok(0) => break,
                Ok(n) => {
                    done += n;
                    stalls = 0;
                }
                Err(err) if err.kind() == std::io::ErrorKind::Interrupted => continue,
                Err(err) if err.kind() == std::io::ErrorKind::TimedOut => {
                    stalls += 1;
                    if stalls >= WRITE_STALL_LIMIT {
                        break;
                    }
                    std::thread::yield_now();
                }
                Err(err) => {
                    self.tx += done as u64;
                    self.hangup = true;
                    return Err(classify_kind(&self.cfg.device, err.kind()));
                }
            }
        }
        self.tx += done as u64;
        if done == 0 && !bytes.is_empty() {
            return Err(SerialError::Io(std::io::ErrorKind::WouldBlock));
        }
        Ok(done)
    }

    /// 端口还活着吗。
    ///
    /// 两条判据：本进程没有记过挂断，且后端还能问到"有多少字节待读"（unix 上是
    /// `FIONREAD` ioctl，Windows 上是 `ClearCommError`）——设备拔掉后它们会失败。
    /// 真串口拔线的第一信号通常是读写报错，所以 `hangup` 才是主判据。
    pub fn is_connected(&self) -> bool {
        match self.port.as_ref() {
            Some(port) => !self.hangup && port.bytes_to_read().is_ok(),
            None => false,
        }
    }

    /// 累计字节数 `(rx, tx)`。
    pub fn counters(&self) -> (u64, u64) {
        (self.rx, self.tx)
    }

    /// 热插拔：丢掉旧句柄，按同一份配置重开。计数不清零；失败时端口保持关闭，
    /// 之后再调用可以再试——这就是"等设备插回来"的语义。
    pub fn reconnect(&mut self) -> Result<(), SerialError> {
        self.close();
        self.port = Some(open_port(&self.cfg)?);
        self.hangup = false;
        Ok(())
    }

    pub fn config(&self) -> &SerialConfig {
        &self.cfg
    }

    /// 主动关掉端口。关掉之后 `is_connected()` 为假，再读写会返回 `NotConnected`。
    pub fn close(&mut self) {
        // `serialport` 的 trait 没有 close：句柄随 `Box` 析构关闭。
        self.port = None;
    }
}

/// 打开阶段的错误（`serialport::Error`）→ 调用方能按类别反应的错误。
fn classify(device: &str, err: serialport::Error) -> SerialError {
    use serialport::ErrorKind as K;
    match err.kind() {
        // 后端把"拔了 / 名字错 / 被占用"都归到 NoDevice，`classify_kind` 里再按
        // 具体 io::ErrorKind 是否够不着——这里只能给最常见的那个答案。
        K::NoDevice => SerialError::NotFound(device.to_string()),
        K::InvalidInput => SerialError::Io(std::io::ErrorKind::InvalidInput),
        K::Io(kind) => classify_kind(device, kind),
        K::Unknown => SerialError::Io(std::io::ErrorKind::Other),
    }
}

/// 读写阶段的 `io::ErrorKind` → 类别。
///
/// `BrokenPipe` 是 unix 侧 `POLLHUP` 的样子；`NotFound` / `NotConnected` 是设备
/// 消失。三者对调用方都是"这条会话完了，去等热插拔"。
fn classify_kind(device: &str, kind: std::io::ErrorKind) -> SerialError {
    use std::io::ErrorKind as K;
    match kind {
        K::NotFound | K::NotConnected | K::BrokenPipe => SerialError::NotFound(device.to_string()),
        K::PermissionDenied => SerialError::PermissionDenied(device.to_string()),
        other => SerialError::Io(other),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::ErrorKind;

    /// 填好设备名的基线配置，其余是默认的 8N1 @ 115200。
    fn cfg() -> SerialConfig {
        SerialConfig { device: "/dev/cu.test-only".to_string(), ..SerialConfig::default() }
    }

    fn invalid_input(config: &SerialConfig) -> bool {
        config.validate() == Err(SerialError::Io(ErrorKind::InvalidInput))
    }

    #[test]
    fn default_is_8n1_at_115200() {
        let config = SerialConfig::default();
        assert_eq!(config.baud, 115_200);
        assert_eq!((config.data_bits, config.stop_bits), (8, 1));
        assert_eq!((config.parity, config.flow), (Parity::None, Flow::None));
        assert!(config.dtr && config.rts);
        assert!(config.validate().is_err(), "设备名为空必须被拒绝");
        assert_eq!(cfg().validate(), Ok(()));
    }

    #[test]
    fn empty_device_is_not_found() {
        let config = SerialConfig { device: "  ".to_string(), ..SerialConfig::default() };
        assert_eq!(config.validate(), Err(SerialError::NotFound("  ".into())));
    }

    #[test]
    fn unsupported_baud_names_the_value_and_the_table() {
        assert_eq!(
            SerialConfig { baud: 12_345, ..cfg() }.validate(),
            Err(SerialError::UnsupportedBaud(12_345))
        );
        let text = SerialError::UnsupportedBaud(12_345).to_string();
        assert!(text.contains("12345"), "文案要带上那个数字：{text}");
        assert!(text.contains("115200"), "文案要列出可用值：{text}");
    }

    #[test]
    fn every_listed_baud_validates() {
        for baud in SUPPORTED_BAUDS {
            let config = SerialConfig { baud: *baud, ..cfg() };
            assert_eq!(config.validate(), Ok(()), "{baud} 列出来了却校验不过");
        }
    }

    #[test]
    fn data_bits_stop_bits_and_parity_are_checked() {
        for bits in [0u8, 4, 9, 16] {
            assert!(invalid_input(&SerialConfig { data_bits: bits, ..cfg() }));
        }
        for bits in 5u8..=8 {
            assert_eq!(SerialConfig { data_bits: bits, ..cfg() }.validate(), Ok(()));
        }
        for stops in [0u8, 3, 5] {
            assert!(invalid_input(&SerialConfig { stop_bits: stops, ..cfg() }));
        }
        assert_eq!(SerialConfig { stop_bits: 2, ..cfg() }.validate(), Ok(()));
        for parity in [Parity::None, Parity::Odd, Parity::Even] {
            assert_eq!(SerialConfig { parity, ..cfg() }.validate(), Ok(()), "{parity:?}");
        }
    }

    #[test]
    fn hardware_flow_needs_eight_data_bits() {
        // RTS/CTS 按字节握手：7 位数据凑不满一个字节，拒绝。
        let config = SerialConfig { data_bits: 7, flow: Flow::Hardware, ..cfg() };
        assert!(invalid_input(&config));
        assert_eq!(SerialConfig { data_bits: 8, ..config.clone() }.validate(), Ok(()));
        // 软件流控（XON/XOFF）走数据通道里的控制字符，不挑位数。
        assert_eq!(SerialConfig { flow: Flow::Software, ..config }.validate(), Ok(()));
    }

    #[test]
    fn permission_denied_says_how_to_fix_it() {
        let text = SerialError::PermissionDenied("/dev/ttyUSB0".to_string()).to_string();
        assert!(text.contains("dialout") && text.contains("usermod"), "{text}");
    }

    /// 每个配置项都得真的落到后端枚举上：`validate()` 放行了、映射却写漏一个分支，
    /// 表现就是"选 Even 校验位、实际按 None 发"——这种错只能靠逐项断言挡住。
    #[test]
    fn every_config_value_maps_onto_a_backend_enum() {
        assert_eq!(sp_parity(Parity::None), SpParity::None);
        assert_eq!(sp_parity(Parity::Odd), SpParity::Odd);
        assert_eq!(sp_parity(Parity::Even), SpParity::Even);

        assert_eq!(flow_control(Flow::None), FlowControl::None);
        assert_eq!(flow_control(Flow::Software), FlowControl::Software);
        assert_eq!(flow_control(Flow::Hardware), FlowControl::Hardware);

        assert_eq!(stop_bits(1), SpStopBits::One);
        assert_eq!(stop_bits(2), SpStopBits::Two);
        // 5..=8 全得落到不同的枚举上（`_` 兜底只能给 8）。
        assert_eq!(data_bits(5), DataBits::Five);
        assert_eq!(data_bits(6), DataBits::Six);
        assert_eq!(data_bits(7), DataBits::Seven);
        assert_eq!(data_bits(8), DataBits::Eight);
    }

    #[cfg(unix)]
    #[test]
    fn open_reports_missing_device_before_anything_else() {
        // 只碰一个肯定不存在的路径，不打开任何真实串口。
        let missing = SerialConfig {
            device: "/dev/definitely-not-a-serial-port".to_string(),
            ..SerialConfig::default()
        };
        assert_eq!(open(&missing).err(), Some(SerialError::NotFound(missing.device.clone())));
        // 参数非法时连 open 都不该发生。
        let bad = SerialConfig { data_bits: 9, ..missing };
        assert_eq!(open(&bad).err(), Some(SerialError::Io(ErrorKind::InvalidInput)));
    }

    #[test]
    fn device_list_is_sorted_unique_and_never_panics() {
        let mut expected = list_devices();
        let devices = expected.clone();
        expected.sort();
        expected.dedup();
        assert_eq!(devices, expected);
        // 只断言"是个列表"：CI 上没有串口，空列表是合法结果。
        assert!(devices.len() < 10_000);
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn builtin_consoles_are_not_offered_as_ports() {
        assert!(is_builtin_console("cu.debug-console"));
        assert!(is_builtin_console("cu.Bluetooth-Incoming-Port"));
        assert!(!is_builtin_console("cu.usbserial-1420"));
        assert!(!is_builtin_console("cu.SLAB_USBtoUART"));
    }
}

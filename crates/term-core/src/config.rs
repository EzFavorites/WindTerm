//! 会话与窗口的**默认值常量**（纯数据，零依赖）。
//!
//! 这些值取自会话/窗口默认配置（设计令牌源于参考实现与本项目实测），是
//! 「对话框里那一堆默认值」的唯一来源：新建会话表单、设置页、状态栏都从这里取。
//!
//! 为什么单独一个模块而不是塞在 `theme`：它们与颜色、调色板无关。R12 之前
//! 两组常量混在 `theme.rs` 里，于是「新增一个会话默认值」看起来像在改主题。
//! 分开之后依赖方向也更干净——`session` / `config` 里的数值不需要主题模块参与。

/// Metrics sourced from reference terminal config files.
pub const DEFAULT_COLS: usize = 80; // terminal/configs/session.config -> window.columns
pub const DEFAULT_ROWS: usize = 24; // terminal/configs/session.config -> window.rows
pub const DEFAULT_FONT_SIZE: f32 = 13.0; // global/wind.config -> text.fontSize
pub const DEFAULT_SCROLLBACK: usize = 999999; // window.scrollbackLimit default
pub const MAX_COLS: usize = 9999; // SessionDialog::createWindowPage
pub const MAX_ROWS: usize = 999; // SessionDialog::createWindowPage
pub const MAX_SCROLLBACK: usize = 999999999; // SessionDialog::createWindowPage
pub const SERIAL_BAUD: u16 = 9600; // terminal/configs/session.config -> serial.baudRate
pub const SSH_PORT: u16 = 22;
pub const TELNET_PORT: u16 = 23;
pub const DEFAULT_TERM: &str = "xterm-256color"; // session.term

/// `session.connectionTimeout` — the shipped `terminal/configs/session.config`
/// ships 20 seconds, not the 30 the New Session dialog's spin box shows when a
/// field is untouched.
pub const CONNECTION_TIMEOUT: u32 = 20;
/// `session.keepAlive` (seconds, 0 = off).
pub const KEEP_ALIVE: u32 = 0;
/// `session.tcpKeepAlive` ships *off*; `session.tcpLowDelay` ships *on*.
pub const TCP_KEEP_ALIVE: bool = false;
pub const TCP_LOW_DELAY: bool = true;
/// `window.mouseWheelScrollLines` (min 1, max 9 — createWindowPage).
pub const MOUSE_WHEEL_SCROLL_LINES: u32 = 1;
/// `window.caretStyle` — the config ships the lower-case name "block"; the combo
/// (createCaretStyleCombo) lists 8 styles in the order below.
pub const CARET_STYLES: [&str; 8] = [
    "Block",
    "Line",
    "Rectangle",
    "Underline",
    "Blinking Block",
    "Blinking Line",
    "Blinking Rectangle",
    "Blinking Underline",
];

/// W4/P2：把会话偏好的 `caret_style` 索引（§2.5 八选一，见 `CARET_STYLES`）
/// 映射到渲染层真正认识的「形状 + 是否闪烁」二元组。
///
/// 索引顺序与 `CARET_STYLES` 严格对齐：0-3 为非闪烁（Block/Line/Rectangle/
/// Underline），4-7 为对应的闪烁变体。渲染层 `grid::CursorShape` 只认四种形状，
/// 闪烁是独立的布尔量——这正是「8 = 4 形状 × 2 闪烁」的分解。
///
/// 越界索引（用户手改 config 或默认值漂移）按 `Block` 兜底，绝不 panic。
pub fn caret_style_to_shape_blink(idx: usize) -> (crate::grid::CursorShape, bool) {
    use crate::grid::CursorShape;
    // 越界索引（用户手改 config 或默认值漂移）按 Block 无闪烁兜底，绝不 panic。
    if idx >= 8 {
        return (CursorShape::Block, false);
    }
    let blink = idx >= 4;
    let shape = match idx % 4 {
        0 => CursorShape::Block,
        1 => CursorShape::Beam,
        2 => CursorShape::HollowBlock,
        3 => CursorShape::Underline,
        _ => CursorShape::Block,
    };
    (shape, blink)
}

/// 光标闪烁周期的出厂值（秒）。`Period` 指「亮多久 = 灭多久」，
/// 也就是完整方波周期为 `2 × period`。视觉基线 `13-caret.png` 显示 `0.5 seconds`。
///
/// 与 `caret_style` 同属**会话属性**（`SessionConfig`）：每条会话可以有自己的
/// 光标外观，改动只影响新会话。
pub const DEFAULT_CARET_PERIOD: f32 = 0.5;

/// 光标宽度的出厂值（像素，`Underline` / `Beam` 用）。视觉基线显示 `1.5 pixels`。
///
/// `Block` / `HollowBlock` 是整格填充，不使用这个值。渲染侧会夹进
/// `[0.5, 格宽/格高]`，避免手改 prefs 写出一个盖满整格的横杠。
pub const DEFAULT_CARET_WIDTH: f32 = 1.5;

/// 光标周期的可调区间（秒）。`0.1` 快得几乎看不清、`2.0` 慢得像呼吸灯，
/// 再往外就没有「光标」的意义了。
pub const CARET_PERIOD_RANGE: std::ops::RangeInclusive<f32> = 0.1..=2.0;

/// 光标宽度的可调区间（像素）。
pub const CARET_WIDTH_RANGE: std::ops::RangeInclusive<f32> = 0.5..=6.0;

/// 把任意（含手改 prefs 的越界值）夹进有效区间；`NaN` 回落出厂值。
///
/// 纯函数、无副作用——渲染层与设置页共用这一份夹取，两处不会各判一套。
pub fn clamp_caret_period(period: f32) -> f32 {
    if period.is_finite() {
        period.clamp(*CARET_PERIOD_RANGE.start(), *CARET_PERIOD_RANGE.end())
    } else {
        DEFAULT_CARET_PERIOD
    }
}

/// 同 [`clamp_caret_period`]，给宽度用。
pub fn clamp_caret_width(width: f32) -> f32 {
    if width.is_finite() {
        width.clamp(*CARET_WIDTH_RANGE.start(), *CARET_WIDTH_RANGE.end())
    } else {
        DEFAULT_CARET_WIDTH
    }
}

/// 本帧光标是否处于「亮」的相位。
///
/// `period` 是「亮多久 = 灭多久」，所以方波周期为 `2 × period`：
/// `(t / (2p))` 的小数部分落在 `[0, 0.5)` 即亮。`period = 0.5` 时就是
/// 「亮 0.5s、灭 0.5s」，与 2026-11-19 之前硬编码的 1 Hz 方波完全一致——
/// 默认行为零变化，只有用户真的改了 Period 才看得出差别。
pub fn caret_lit_at(now_secs: f64, period: f32) -> bool {
    let p = clamp_caret_period(period) as f64;
    (now_secs / (2.0 * p)).fract() < 0.5
}


/// `terminal.answerBack` — the answer sent to ENQ (`^E`), verbatim from
/// session.config. `terminal.answerId` is the same key's companion.
pub const ANSWER_BACK: &str = "Wind";
/// `session.charset`.
pub const CHARSET: &str = "UTF-8";
/// `serial.dataBits` / `stopBits` / `parity` / `flowControl`.
pub const SERIAL_DATA_BITS: u8 = 8;
pub const SERIAL_STOP_BITS: u8 = 1;
pub const SERIAL_PARITY: &str = "None";
pub const SERIAL_FLOW_CONTROL: &str = "None";
/// SFTP / Modem 页的默认权限（取自传输页默认值）。
///
/// **这两个是 mode 位（八进制字面量），不是十进制数字**。以前写成十进制
/// `644` / `755` 只为了对话框里能直接 `format!("{}")` 显示那三串数字，代价是
/// 它没有一个是能直接用的权限位——`644` 十进制等于八进制 `1204`，当 mode 传下去
/// 会得到一份乱七八糟的权限。现在按本义写成 `0o644` / `0o755`，显示侧用
/// `{:o}` 取三位数（输出仍是 `644` / `755`，与视觉基线一致）。
pub const DEFAULT_FILE_MODE: u32 = 0o644;
pub const DEFAULT_DIR_MODE: u32 = 0o755;
/// SFTP 的 `permissions` 字段按 POSIX mode 理解，**包含文件类型位**。
/// 建普通文件用 `DEFAULT_FILE_MODE | S_IFREG`，建目录用 `DEFAULT_DIR_MODE | S_IFDIR`。
///
/// 不带上类型位在 OpenSSH 的 `sftp-server` 上也能通过（它对 `perm` 做 `& 07777`），
/// 但协议原文写的是「same as POSIX mode」，别的实现可能严格——按标准带。
pub const S_IFREG: u32 = 0o100000;
pub const S_IFDIR: u32 = 0o40000;
/// 状态栏时钟的刷新周期（2000 ms）。
pub const CLOCK_REFRESH_MS: u64 = 2000;
/// 标签着色 12 色的**名称**，与 `theme::DESIGN_TOKENS.tab_colors` 同序，
/// 即 `design-tokens.json › tabColorSwatches`（picture-100.jpg 像素聚类实测）。
/// 顺序严格对应，改这里必须同步 `DESIGN_TOKENS.tab_colors`。
pub const TAB_COLOR_NAMES: [&str; 12] = [
    "Dark Cyan",
    "Deep Pink",
    "Forest Green",
    "Goldenrod",
    "Indian Red",
    "Medium Orchid",
    "Medium Slate Blue",
    "Olive",
    "Red",
    "Royal Blue",
    "Steel Blue",
    "Teal",
];

/// 标签右键 `Tab Color` 子菜单：第 0 项为「不着色」(Default)，其后 12 项取
/// `theme::DESIGN_TOKENS.tab_colors` 的实测色值。
///
/// 色值**唯一真相源**是 `DESIGN_TOKENS.tab_colors`：这里只按比例拆出 RGB 三元组，
/// 不另写一份十六进制——否则两套 12 色迟早漂移（B3 单一转换点）。
pub const TAB_COLORS: [(&str, u8, u8, u8); 13] = {
    let tk = crate::theme::DESIGN_TOKENS.tab_colors;
    let mut arr: [(&str, u8, u8, u8); 13] = [("Default", 0, 0, 0); 13];
    arr[0] = ("Default", 0, 0, 0);
    let mut i = 0;
    while i < 12 {
        let v = tk[i];
        arr[i + 1] = (
            TAB_COLOR_NAMES[i],
            (v >> 16) as u8,
            (v >> 8) as u8,
            v as u8,
        );
        i += 1;
    }
    arr
};

/// `session.group` default group name shown in the tree.
pub const DEFAULT_GROUP: &str = "Servers";
/// The serial baud rates createBaudRateCombo fills from the
/// static list.
pub const BAUD_RATES: [u32; 12] = [
    110, 300, 600, 1200, 2400, 4800, 9600, 14400, 19200, 38400, 57600, 115200,
];
/// SessionDialog::createTermCombo — the terminal types the
/// dropdown offers, in the order a conventional terminal lists them.
pub const TERM_TYPES: [&str; 12] = [
    "xterm",
    "xterm-256color",
    "screen",
    "screen-256color",
    "linux",
    "vt100",
    "vt102",
    "vt220",
    "vt320",
    "ansi",
    "tty",
    "dumb",
];

#[cfg(test)]
mod tests {
    use super::*;
    use crate::grid::CursorShape;

    /// W4/P2：八种 `CARET_STYLES` 必须一一映射到「4 形状 × 2 闪烁」，
    /// 且前四非闪烁、后四闪烁，形状按 Block/Line/Rectangle/Underline 循环。
    #[test]
    fn caret_style_maps_to_eight_shapes_and_blink() {
        let want: [(CursorShape, bool); 8] = [
            (CursorShape::Block, false),
            (CursorShape::Beam, false),        // Line
            (CursorShape::HollowBlock, false), // Rectangle
            (CursorShape::Underline, false),
            (CursorShape::Block, true), // Blinking Block
            (CursorShape::Beam, true),  // Blinking Line
            (CursorShape::HollowBlock, true), // Blinking Rectangle
            (CursorShape::Underline, true), // Blinking Underline
        ];
        for (i, (shape, blink)) in want.iter().enumerate() {
            assert_eq!(
                caret_style_to_shape_blink(i),
                (*shape, *blink),
                "caret_style 索引 {i} 映射错误"
            );
        }
    }

    /// 越界索引绝不 panic，按 Block 兜底。
    #[test]
    fn caret_style_out_of_range_clamps_to_block() {
        assert_eq!(caret_style_to_shape_blink(99), (CursorShape::Block, false));
    }

    /// 2026-11-19：周期 / 宽度的夹取。越界与 NaN 都要收回有效区间 / 出厂值，
    /// 否则手改 prefs 写个 `100.0` 或 `NaN` 会让光标永不闪或永不显示。
    #[test]
    fn caret_period_and_width_clamp_bad_inputs() {
        assert_eq!(clamp_caret_period(0.0), *CARET_PERIOD_RANGE.start());
        assert_eq!(clamp_caret_period(99.0), *CARET_PERIOD_RANGE.end());
        assert_eq!(clamp_caret_period(f32::NAN), DEFAULT_CARET_PERIOD);
        assert_eq!(clamp_caret_width(0.0), *CARET_WIDTH_RANGE.start());
        assert_eq!(clamp_caret_width(99.0), *CARET_WIDTH_RANGE.end());
        assert_eq!(clamp_caret_width(f32::NAN), DEFAULT_CARET_WIDTH);
        // 区间内的正常值原样返回。
        assert_eq!(clamp_caret_period(0.5), 0.5);
        assert_eq!(clamp_caret_width(1.5), 1.5);
    }

    /// `caret_lit_at` 的相位：`period=0.5` 时必须与旧硬编码
    /// `(t*2).fract() < 1.0`（等价于 `t.fract()*2 < 1`）逐点一致，
    /// 这是「默认零行为变化」的判据。
    #[test]
    fn caret_lit_matches_the_old_hardcoded_phase_at_default_period() {
        for &t in &[0.0_f64, 0.1, 0.24, 0.25, 0.49, 0.5, 0.75, 0.99, 1.0, 1.3, 7.8] {
            let old = (t.fract() * 2.0) < 1.0;
            assert_eq!(
                caret_lit_at(t, DEFAULT_CARET_PERIOD),
                old,
                "t={t} 时新相位与旧 1 Hz 方波不一致"
            );
        }
    }

    /// 周期变长后同一时刻的亮灭会延后：`period=1.0` 时 0.6 秒仍亮
    /// （`0.6/(2*1)=0.3 < 0.5`），而 `period=0.5` 时同样 0.6 秒已灭。
    #[test]
    fn a_longer_period_stays_lit_longer() {
        assert!(caret_lit_at(0.6, 0.5) == false);
        assert!(caret_lit_at(0.6, 1.0));
    }
}


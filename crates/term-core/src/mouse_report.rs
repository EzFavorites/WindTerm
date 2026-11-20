//! `xterm` 鼠标报告（Mouse Tracking）的纯编码层（ADR-0014 / R2）。
//!
//! 状态由 `vt::Terminal` 持有：DECSET 1000/1002/1003 决定**报不报、报哪些**，
//! 1005/1006/1015 决定**怎么编码**。快照把这两项带到投影层，投影层只把一帧
//! 指针事实交给这里的纯函数，拿到字节后写回会话。
//!
//! 本模块零 IO、零线程、零墙钟、零 UI 框架依赖：`encode` / `cell_at` /
//! `encode_frame` 全部是纯函数，可以在无头环境里逐字节验证。

/// DECSET 1000 / 1002 / 1003 三档，优先级 1003 > 1002 > 1000。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum MouseMode {
    /// 没开鼠标报告。
    #[default]
    Off,
    /// 1000：只报按下 / 释放，拖动一律不报。
    Clicks,
    /// 1002：按下 / 释放 + **跨格**的拖动（判重在投影层传 `prev_cell`）。
    CellMotion,
    /// 1003：按下 / 释放 + 每一帧的拖动。
    AllMotion,
}

/// 坐标/按钮的编码，优先级 SGR(1006) > Urxvt(1015) > Utf8(1005) > X10(默认)。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum MouseEncoding {
    /// `CSI M` + 三个单字节（默认编码）。
    #[default]
    X10,
    /// 1005：`CSI M` + 按钮字节 + 两个 UTF-8 字符。
    Utf8,
    /// 1006：`CSI < Cb ; col ; row M/m`，不受 223 列限制。
    Sgr,
    /// 1015：`CSI Cb ; col ; row M`（没有 `<`）。
    Urxvt,
}

/// 投影层从快照里读到的完整配置。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct MouseConfig {
    pub mode: MouseMode,
    pub encoding: MouseEncoding,
}

/// 报告里随按钮字节一起送出的修饰键位。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Modifiers {
    pub shift: bool,
    pub alt: bool,
    pub ctrl: bool,
}

/// 一次指针事件。`col` / `row` 是 **1-based** 字符格坐标。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MouseEvent {
    /// `button`: 0=左、1=中、2=右。
    Press {
        button: u8,
        col: u16,
        row: u16,
        modifiers: Modifiers,
    },
    Release {
        col: u16,
        row: u16,
        modifiers: Modifiers,
    },
    /// 仅 `CellMotion` / `AllMotion` 下产生。
    Drag {
        button: u8,
        col: u16,
        row: u16,
        modifiers: Modifiers,
    },
    WheelUp {
        col: u16,
        row: u16,
        modifiers: Modifiers,
    },
    WheelDown {
        col: u16,
        row: u16,
        modifiers: Modifiers,
    },
}

/// 一个窗格的网格几何（纯值）：像素原点、单元格尺寸、本屏行列数。
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct GridMetrics {
    pub start_x: f32,
    pub start_y: f32,
    pub cell_w: f32,
    pub cell_h: f32,
    pub cols: usize,
    pub rows: usize,
}

/// 投影层在一帧里采集到的指针事实。三个 `[bool; 3]` 依次是 [左, 中, 右]。
///
/// 收成纯值的理由：投影框架的类型不能进本 crate，而坐标换算与判重又必须
/// 能无头测试。`scroll` 沿用投影层既有的符号约定（>0 = 向上滚）。
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct PointerFrame {
    pub x: f32,
    pub y: f32,
    pub has_pos: bool,
    pub pressed: [bool; 3],
    pub released: [bool; 3],
    pub down: [bool; 3],
    pub dragged: bool,
    pub hovered: bool,
    pub scroll: f32,
    pub modifiers: Modifiers,
}

/// 把一次指针事件编码成写回会话的字节；`MouseMode::Off` 时返回空。
pub fn encode(cfg: &MouseConfig, ev: &MouseEvent) -> Vec<u8> {
    if cfg.mode == MouseMode::Off {
        return Vec::new();
    }
    // 1000 只报按下 / 释放：拖动既不在协议里，也不该被当成一次点击漏出去。
    if cfg.mode == MouseMode::Clicks && matches!(ev, MouseEvent::Drag { .. }) {
        return Vec::new();
    }
    let (col, row) = coords_of(ev);
    // xterm 的坐标是 1-based；0 说明调用方没换算好，宁可不发也不发错格。
    if col == 0 || row == 0 {
        return Vec::new();
    }
    let (cb, sgr_final) = cb_and_final(ev);
    match cfg.encoding {
        MouseEncoding::X10 => encode_x10(cb, col, row),
        MouseEncoding::Utf8 => encode_utf8(cb, col, row),
        MouseEncoding::Sgr => encode_sgr(cb, col, row, sgr_final),
        MouseEncoding::Urxvt => encode_urxvt(cb, col, row),
    }
}

/// 像素坐标 → 1-based 字符格坐标。越界（含负向 / 超出本屏）返回 `None`。
///
/// 换算顺序固定为「先减原点，再除格子」；有效值再 clamp 到 `[1, cols]` /
/// `[1, rows]`。全程用 `floor` 与 saturating 转换，没有 panic 路径。
pub fn cell_at(geom: &GridMetrics, x: f32, y: f32) -> Option<(u16, u16)> {
    if !x.is_finite() || !y.is_finite() || geom.cell_w <= 0.0 || geom.cell_h <= 0.0 {
        return None;
    }
    let cx = ((x - geom.start_x) / geom.cell_w).floor();
    let cy = ((y - geom.start_y) / geom.cell_h).floor();
    if cx < 0.0 || cy < 0.0 || cx >= geom.cols as f32 || cy >= geom.rows as f32 {
        return None;
    }
    // 浮点转整数在 Rust 里是饱和的；这里再夹一次上界，保证是有效格。
    let col = (cx as usize).saturating_add(1).min(geom.cols.max(1));
    let row = (cy as usize).saturating_add(1).min(geom.rows.max(1));
    match (u16::try_from(col), u16::try_from(row)) {
        (Ok(c), Ok(r)) if c > 0 && r > 0 => Some((c, r)),
        _ => None,
    }
}

/// 一帧指针事实 → 报告字节 + 下一帧要记住的拖动格。
///
/// `prev_cell` 由投影层在帧间保存（放在窗口部件的临时数据里，不动 `Tab`）：
/// `CellMotion` 只在格子变化时发 `Drag`，`AllMotion` 每帧都发；按下时记下
/// 起点、释放时清空。返回的第二个值就是这份状态，调用方原样存回即可。
pub fn encode_frame(
    cfg: &MouseConfig,
    frame: &PointerFrame,
    geom: &GridMetrics,
    prev_cell: Option<(u16, u16)>,
) -> (Vec<u8>, Option<(u16, u16)>) {
    if cfg.mode == MouseMode::Off || !frame.has_pos {
        return (Vec::new(), None);
    }
    let Some((col, row)) = cell_at(geom, frame.x, frame.y) else {
        return (Vec::new(), None);
    };

    let mods = frame.modifiers;
    let mut events: Vec<MouseEvent> = Vec::new();
    let mut next_cell = prev_cell;

    for (i, is_pressed) in frame.pressed.iter().enumerate() {
        if *is_pressed {
            events.push(MouseEvent::Press {
                button: i as u8,
                col,
                row,
                modifiers: mods,
            });
            next_cell = Some((col, row));
        }
    }

    // 拖动要带上「哪个键还按着」；没有键按着就不该凭空造一条 Drag。
    let held = if frame.down[0] {
        Some(0u8)
    } else if frame.down[1] {
        Some(1)
    } else if frame.down[2] {
        Some(2)
    } else {
        None
    };
    if frame.dragged {
        if let Some(button) = held {
            let moved = prev_cell != Some((col, row));
            let report = match cfg.mode {
                MouseMode::AllMotion => true,
                MouseMode::CellMotion => moved,
                MouseMode::Clicks | MouseMode::Off => false,
            };
            if report {
                events.push(MouseEvent::Drag {
                    button,
                    col,
                    row,
                    modifiers: mods,
                });
                next_cell = Some((col, row));
            }
        }
    }

    for is_released in frame.released.iter() {
        if *is_released {
            events.push(MouseEvent::Release {
                col,
                row,
                modifiers: mods,
            });
            next_cell = None;
        }
    }

    // 滚轮只在指针悬停在本窗格时上报；一帧一个事件（每个 tick 一条报告）。
    if frame.hovered && frame.scroll.abs() > f32::EPSILON {
        events.push(if frame.scroll > 0.0 {
            MouseEvent::WheelUp {
                col,
                row,
                modifiers: mods,
            }
        } else {
            MouseEvent::WheelDown {
                col,
                row,
                modifiers: mods,
            }
        });
    }

    let mut out = Vec::new();
    for ev in &events {
        out.extend_from_slice(&encode(cfg, ev));
    }
    (out, next_cell)
}

fn modifier_bits(m: Modifiers) -> u8 {
    let mut bits = 0u8;
    if m.shift {
        bits |= 4;
    }
    if m.alt {
        bits |= 8;
    }
    if m.ctrl {
        bits |= 16;
    }
    bits
}

/// 返回 `(Cb, SGR 终止符)`。低 2 位 0=左 / 1=中 / 2=右 / 3=释放；
/// Shift=4、Alt=8、Ctrl=16、拖动=32、滚轮=64/65。
/// 只有 1006 用 `M` / `m` 区分按下与释放，其余编码一律大写 `M`。
fn cb_and_final(ev: &MouseEvent) -> (u8, u8) {
    match *ev {
        MouseEvent::Press {
            button, modifiers, ..
        } => (button.min(2) | modifier_bits(modifiers), b'M'),
        MouseEvent::Drag {
            button, modifiers, ..
        } => (button.min(2) | modifier_bits(modifiers) | 32, b'M'),
        MouseEvent::Release { modifiers, .. } => (3 | modifier_bits(modifiers), b'm'),
        MouseEvent::WheelUp { modifiers, .. } => (64 | modifier_bits(modifiers), b'M'),
        MouseEvent::WheelDown { modifiers, .. } => (65 | modifier_bits(modifiers), b'M'),
    }
}

fn coords_of(ev: &MouseEvent) -> (u16, u16) {
    match *ev {
        MouseEvent::Press { col, row, .. }
        | MouseEvent::Release { col, row, .. }
        | MouseEvent::Drag { col, row, .. }
        | MouseEvent::WheelUp { col, row, .. }
        | MouseEvent::WheelDown { col, row, .. } => (col, row),
    }
}

fn encode_x10(cb: u8, col: u16, row: u16) -> Vec<u8> {
    // 单字节坐标最大是 255 - 32 = 223。超出就整条丢弃：截断成 u8 会
    // 产出「指向另一个格子」的错误报告，比不发更糟。
    if col > 223 || row > 223 {
        return Vec::new();
    }
    let mut out = Vec::with_capacity(6);
    out.extend_from_slice(b"\x1b[M");
    // Cb 上界 65 + 4 + 8 + 16 + 32 = 125，+32 后 157，不会溢出 u8。
    out.push(cb.saturating_add(32));
    out.push((col + 32) as u8);
    out.push((row + 32) as u8);
    out
}

fn encode_utf8(cb: u8, col: u16, row: u16) -> Vec<u8> {
    let Some(col_ch) = char::from_u32(32 + col as u32) else {
        return Vec::new();
    };
    let Some(row_ch) = char::from_u32(32 + row as u32) else {
        return Vec::new();
    };
    let mut out = Vec::with_capacity(8);
    out.extend_from_slice(b"\x1b[M");
    out.push(cb.saturating_add(32));
    let mut buf = [0u8; 4];
    out.extend_from_slice(col_ch.encode_utf8(&mut buf).as_bytes());
    out.extend_from_slice(row_ch.encode_utf8(&mut buf).as_bytes());
    out
}

fn encode_sgr(cb: u8, col: u16, row: u16, final_byte: u8) -> Vec<u8> {
    let mut out = format!("\x1b[<{cb};{col};{row}").into_bytes();
    out.push(final_byte);
    out
}

fn encode_urxvt(cb: u8, col: u16, row: u16) -> Vec<u8> {
    format!("\x1b[{cb};{col};{row}M").into_bytes()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg(mode: MouseMode, encoding: MouseEncoding) -> MouseConfig {
        MouseConfig { mode, encoding }
    }

    fn press(col: u16, row: u16) -> MouseEvent {
        MouseEvent::Press {
            button: 0,
            col,
            row,
            modifiers: Modifiers::default(),
        }
    }

    fn release(col: u16, row: u16) -> MouseEvent {
        MouseEvent::Release {
            col,
            row,
            modifiers: Modifiers::default(),
        }
    }

    fn drag(col: u16, row: u16) -> MouseEvent {
        MouseEvent::Drag {
            button: 0,
            col,
            row,
            modifiers: Modifiers::default(),
        }
    }

    fn geom() -> GridMetrics {
        GridMetrics {
            start_x: 10.0,
            start_y: 20.0,
            cell_w: 8.0,
            cell_h: 16.0,
            cols: 80,
            rows: 24,
        }
    }

    fn frame(x: f32, y: f32) -> PointerFrame {
        PointerFrame {
            x,
            y,
            has_pos: true,
            ..PointerFrame::default()
        }
    }

    #[test]
    fn x10_press_three_single_byte_coordinates() {
        // Cb=0 → 32(空格)，col/row=1 → 33('!')。
        assert_eq!(
            encode(&cfg(MouseMode::Clicks, MouseEncoding::X10), &press(1, 1)),
            b"\x1b[M !!".to_vec()
        );
        // 右键 + 中键按钮号也进低 2 位。
        let right = MouseEvent::Press {
            button: 2,
            col: 3,
            row: 7,
            modifiers: Modifiers::default(),
        };
        assert_eq!(
            encode(&cfg(MouseMode::Clicks, MouseEncoding::X10), &right),
            vec![0x1b, b'[', b'M', 34, 35, 39]
        );
    }

    #[test]
    fn utf8_encodes_each_coordinate_as_a_character() {
        // 32+200=U+00E8 → C3 A8；32+100=U+0084 → C2 84。
        assert_eq!(
            encode(
                &cfg(MouseMode::Clicks, MouseEncoding::Utf8),
                &press(200, 100)
            ),
            vec![0x1b, b'[', b'M', 32, 0xC3, 0xA8, 0xC2, 0x84]
        );
        // ASCII 范围内与 X10 逐字节一致。
        assert_eq!(
            encode(&cfg(MouseMode::Clicks, MouseEncoding::Utf8), &press(1, 1)),
            b"\x1b[M !!".to_vec()
        );
    }

    #[test]
    fn sgr_press_uses_uppercase_and_release_uses_lowercase() {
        let c = cfg(MouseMode::Clicks, MouseEncoding::Sgr);
        assert_eq!(encode(&c, &press(1, 1)), b"\x1b[<0;1;1M".to_vec());
        assert_eq!(encode(&c, &release(1, 1)), b"\x1b[<3;1;1m".to_vec());
        // 拖动只在 1002/1003 下报；用 CellMotion 验它的 32 位。
        let motion = cfg(MouseMode::CellMotion, MouseEncoding::Sgr);
        assert_eq!(
            encode(&motion, &drag(5, 7)),
            b"\x1b[<32;5;7M".to_vec(),
            "拖动位 32"
        );
    }

    #[test]
    fn urxvt_has_no_less_than_sign_and_no_final_letter_distinction() {
        let c = cfg(MouseMode::Clicks, MouseEncoding::Urxvt);
        assert_eq!(encode(&c, &press(3, 7)), b"\x1b[0;3;7M".to_vec());
        assert_eq!(encode(&c, &release(3, 7)), b"\x1b[3;3;7M".to_vec());
    }

    #[test]
    fn modifier_bits_are_shift_alt_ctrl() {
        let m = Modifiers {
            shift: true,
            alt: true,
            ctrl: true,
        };
        let ev = MouseEvent::Press {
            button: 0,
            col: 1,
            row: 1,
            modifiers: m,
        };
        // 4 + 8 + 16 = 28。
        assert_eq!(
            encode(&cfg(MouseMode::Clicks, MouseEncoding::Sgr), &ev),
            b"\x1b[<28;1;1M".to_vec()
        );
        // X10 里 28+32=60 是 '<'。
        assert_eq!(
            encode(&cfg(MouseMode::Clicks, MouseEncoding::X10), &ev),
            b"\x1b[M<!!".to_vec()
        );
        let alt_only = MouseEvent::Press {
            button: 1,
            col: 1,
            row: 1,
            modifiers: Modifiers {
                alt: true,
                ..Modifiers::default()
            },
        };
        assert_eq!(
            encode(&cfg(MouseMode::Clicks, MouseEncoding::Sgr), &alt_only),
            b"\x1b[<9;1;1M".to_vec()
        );
    }

    #[test]
    fn wheel_uses_buttons_64_and_65() {
        let c = cfg(MouseMode::Clicks, MouseEncoding::Sgr);
        assert_eq!(
            encode(
                &c,
                &MouseEvent::WheelUp {
                    col: 2,
                    row: 3,
                    modifiers: Modifiers::default()
                }
            ),
            b"\x1b[<64;2;3M".to_vec()
        );
        assert_eq!(
            encode(
                &c,
                &MouseEvent::WheelDown {
                    col: 2,
                    row: 3,
                    modifiers: Modifiers::default()
                }
            ),
            b"\x1b[<65;2;3M".to_vec()
        );
        // X10 下 64+32=96('`')，坐标 2/3 → 34/35。
        assert_eq!(
            encode(
                &cfg(MouseMode::Clicks, MouseEncoding::X10),
                &MouseEvent::WheelUp {
                    col: 2,
                    row: 3,
                    modifiers: Modifiers::default()
                }
            ),
            vec![0x1b, b'[', b'M', 96, 34, 35]
        );
    }

    #[test]
    fn x10_drops_coordinates_over_223_but_sgr_does_not() {
        let x10 = cfg(MouseMode::Clicks, MouseEncoding::X10);
        assert!(encode(&x10, &press(224, 1)).is_empty());
        assert!(encode(&x10, &press(1, 224)).is_empty());
        assert_eq!(
            encode(&x10, &press(223, 1)),
            vec![0x1b, b'[', b'M', 32, 255, 33]
        );
        assert_eq!(
            encode(
                &cfg(MouseMode::Clicks, MouseEncoding::Sgr),
                &press(300, 250)
            ),
            b"\x1b[<0;300;250M".to_vec()
        );
    }

    #[test]
    fn utf8_rejects_surrogate_codepoints() {
        // 32 + 55264 = 0xD800，落在代理区，不是合法的 Unicode 标量值。
        assert!(encode(
            &cfg(MouseMode::Clicks, MouseEncoding::Utf8),
            &press(55264, 1)
        )
        .is_empty());
    }

    #[test]
    fn off_and_clicks_drag_are_silent() {
        assert!(encode(&cfg(MouseMode::Off, MouseEncoding::Sgr), &press(1, 1)).is_empty());
        assert!(encode(&cfg(MouseMode::Clicks, MouseEncoding::Sgr), &drag(2, 2)).is_empty());
        assert!(!encode(
            &cfg(MouseMode::CellMotion, MouseEncoding::Sgr),
            &drag(2, 2)
        )
        .is_empty());
        assert!(!encode(
            &cfg(MouseMode::AllMotion, MouseEncoding::Sgr),
            &drag(2, 2)
        )
        .is_empty());
        // 0 坐标（调用方没换算好）也不发。
        assert!(encode(&cfg(MouseMode::Clicks, MouseEncoding::Sgr), &press(0, 1)).is_empty());
    }

    #[test]
    fn cell_at_subtracts_origin_divides_cell_and_discards_out_of_range() {
        let g = geom();
        assert_eq!(cell_at(&g, 10.0, 20.0), Some((1, 1)));
        assert_eq!(cell_at(&g, 17.9, 35.9), Some((1, 1)));
        assert_eq!(cell_at(&g, 18.0, 36.0), Some((2, 2)));
        assert_eq!(cell_at(&g, 9.9, 20.0), None);
        assert_eq!(cell_at(&g, 10.0, 19.9), None);
        assert_eq!(cell_at(&g, 10.0 + 80.0 * 8.0, 20.0), None);
        assert_eq!(cell_at(&g, 10.0, 20.0 + 24.0 * 16.0), None);
        assert_eq!(cell_at(&g, f32::NAN, 20.0), None);
        assert_eq!(cell_at(&g, 10.0, f32::INFINITY), None);
        let bad = GridMetrics {
            cell_w: 0.0,
            ..g
        };
        assert_eq!(cell_at(&bad, 10.0, 20.0), None);
    }

    #[test]
    fn cell_motion_reports_drag_only_when_the_cell_changes() {
        let c = cfg(MouseMode::CellMotion, MouseEncoding::Sgr);
        let mut f = frame(10.0, 20.0);
        f.dragged = true;
        f.down = [true, false, false];
        // 同一格：不发；状态仍是这一格。
        let (bytes, next) = encode_frame(&c, &f, &geom(), Some((1, 1)));
        assert!(bytes.is_empty());
        assert_eq!(next, Some((1, 1)));
        // 换了一格：发一条 Drag，状态更新。
        let (bytes, next) = encode_frame(&c, &f, &geom(), Some((1, 2)));
        assert_eq!(bytes, b"\x1b[<32;1;1M".to_vec());
        assert_eq!(next, Some((1, 1)));
        // 没有上一格（刚按下那一帧）：发。
        let (bytes, next) = encode_frame(&c, &f, &geom(), None);
        assert_eq!(bytes, b"\x1b[<32;1;1M".to_vec());
        assert_eq!(next, Some((1, 1)));
    }

    #[test]
    fn all_motion_reports_drag_every_frame_and_press_release_track_state() {
        let c = cfg(MouseMode::AllMotion, MouseEncoding::Sgr);
        let mut f = frame(10.0, 20.0);
        f.dragged = true;
        f.down = [true, false, false];
        let (bytes, next) = encode_frame(&c, &f, &geom(), Some((1, 1)));
        assert_eq!(bytes, b"\x1b[<32;1;1M".to_vec());
        assert_eq!(next, Some((1, 1)));

        let mut p = frame(10.0, 20.0);
        p.pressed = [true, false, false];
        let (bytes, next) = encode_frame(&c, &p, &geom(), None);
        assert_eq!(bytes, b"\x1b[<0;1;1M".to_vec());
        assert_eq!(next, Some((1, 1)));

        let mut r = frame(18.0, 36.0);
        r.released = [true, false, false];
        let (bytes, next) = encode_frame(&c, &r, &geom(), Some((1, 1)));
        assert_eq!(bytes, b"\x1b[<3;2;2m".to_vec());
        assert_eq!(next, None);
    }

    #[test]
    fn frame_wheel_is_hover_gated_and_direction_aware() {
        let c = cfg(MouseMode::Clicks, MouseEncoding::Sgr);
        let mut up = frame(10.0, 20.0);
        up.hovered = true;
        up.scroll = 3.0;
        assert_eq!(
            encode_frame(&c, &up, &geom(), None).0,
            b"\x1b[<64;1;1M".to_vec()
        );
        let mut down = up;
        down.scroll = -3.0;
        assert_eq!(
            encode_frame(&c, &down, &geom(), None).0,
            b"\x1b[<65;1;1M".to_vec()
        );
        let mut away = up;
        away.hovered = false;
        assert!(encode_frame(&c, &away, &geom(), None).0.is_empty());
        // 没开报告时整帧静默。
        assert!(encode_frame(&cfg(MouseMode::Off, MouseEncoding::Sgr), &up, &geom(), None)
            .0
            .is_empty());
    }
}

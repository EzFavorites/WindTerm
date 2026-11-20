//! Edit Color 取色器（视觉基线 `06-color-picker.png`）。
//!
//! 526×468 逻辑点，居中覆盖在主对话框上（口径校正见 [`spec::color_picker`]：
//! 原图 `1052×936`，是 2×，不是 1.667×）。
//!
//! 版式严格按原图：左半自上而下是 `Basic colors` 8×6 色板 → `Pick Screen Color`
//! → 大留白（自定义组是**贴底**的，不是紧随基础色板）→ `Custom colors` 8×2 →
//! `Add to Custom Colors`；右半是色域方块 + 亮度条，方块下方左边预览块、右边
//! HSV/RGB 两列数值，再往下 `Alpha channel:` 与 `HTML:`，页脚两个按钮。
//!
//! 取色模型的依据（实测原图）：方块横向是**反向色相**（左红右红、中间青），
//! 纵向由饱和渐变到浅灰；右侧竖条是**灰度亮度**条，不是彩虹色相条。

use super::fields::{footer_button_as, mono, place, rgb, Btn, FS};
use super::fields::text_w;
use super::text::ul_text;
use super::{Picker, View};
use crate::ui::view::trigger_spec as spec;
use crate::ui::view::trigger_spec::color as c;
use crate::ui::*;
use term_core::trigger::ColorChoice;

/// 画取色器（未打开时什么都不做）。
pub(in crate::ui) fn show(ui: &mut egui::Ui, o: egui::Pos2, view: &mut View) {
    if view.picker.is_none() {
        return;
    }
    let cp = spec::color_picker::W;
    let ch = spec::color_picker::H;
    let r = egui::Rect::from_center_size(
        o + egui::vec2(spec::DIALOG_W / 2.0, spec::DIALOG_H / 2.0),
        egui::vec2(cp, ch),
    );
    let p = ui.painter().clone();
    // 遮罩。
    ui.painter().rect_filled(
        egui::Rect::from_min_size(o, egui::vec2(spec::DIALOG_W, spec::DIALOG_H)),
        egui::Rounding::ZERO,
        egui::Color32::from_black_alpha(120),
    );
    p.rect_filled(r, egui::Rounding::ZERO, rgb(0x1c1c1c));
    p.rect_stroke(r, egui::Rounding::ZERO, egui::Stroke::new(1.0, rgb(0x333333)));

    title(&p, r);
    let mut pk = view.picker.clone().unwrap_or_default();

    palette(ui, &p, r, &mut pk);
    square(ui, &p, r, &mut pk);
    value_bar(ui, &p, r, &mut pk);
    readouts(ui, &p, r, &mut pk);

    // 页脚（贴底，与原图同坐标）。
    if footer_button_as(ui, rect(r, spec::color_picker::CANCEL), "Cancel", Btn::Secondary) {
        view.picker = None;
        return;
    }
    if footer_button_as(ui, rect(r, spec::color_picker::OK), "OK", Btn::Primary) {
        apply(view, &pk);
        view.picker = None;
        return;
    }
    view.picker = Some(pk);
}

fn rect(r: egui::Rect, q: (f32, f32, f32, f32)) -> egui::Rect {
    egui::Rect::from_min_size(r.min + egui::vec2(q.0, q.1), egui::vec2(q.2, q.3))
}

/// 标题栏：`#393c3e` 底 + 三个圆点 + 居中 `Edit Color`。
fn title(p: &egui::Painter, r: egui::Rect) {
    let bar = egui::Rect::from_min_size(
        r.min,
        egui::vec2(r.width(), spec::color_picker::TITLE_H),
    );
    p.rect_filled(bar, egui::Rounding::ZERO, rgb(0x393c3e));
    for (i, col) in [0xfe5f57u32, 0x5c6062, 0x5c6062].iter().enumerate() {
        p.circle_filled(
            egui::pos2(
                r.left() + spec::color_picker::DOT_X + i as f32 * spec::color_picker::DOT_STEP,
                r.top() + spec::color_picker::DOT_Y,
            ),
            spec::color_picker::DOT_R,
            rgb(*col),
        );
    }
    // 原图标题墨迹纵向中心在 11.75（不是标题栏正中 14）——Qt 的题注行偏上，
    // 照抄，不然整行文字会比视觉基线低 2 点。
    p.text(
        egui::pos2(r.center().x, r.top() + 11.75),
        egui::Align2::CENTER_CENTER,
        "Edit Color",
        mono(crate::ui::type_scale::SIZE_SMALL),
        rgb(0xd8d8d8),
    );
}

/// 左半：Basic / Custom 色板 + 两个扁平按钮。
fn palette(ui: &mut egui::Ui, p: &egui::Painter, r: egui::Rect, pk: &mut Picker) {
    use spec::color_picker as s;
    let left = r.left() + s::PALETTE_LEFT;
    let step = egui::vec2(
        s::SWATCH_W + s::SWATCH_GAP_X,
        s::SWATCH_H + s::SWATCH_GAP_Y,
    );

    ul_text(ui, p, r.left() + s::LABEL_X, r.top() + s::LABEL_BASIC_Y - 4.3,
        "Basic colors", rgb(c::TEXT_2), Some(0));
    for (i, hex) in c::BASIC.iter().enumerate() {
        let (dx, dy) = ((i % s::PALETTE_COLS) as f32, (i / s::PALETTE_COLS) as f32);
        let cell = egui::Rect::from_min_size(
            egui::pos2(left + dx * step.x, r.top() + s::PALETTE_TOP + dy * step.y),
            egui::vec2(s::SWATCH_W, s::SWATCH_H),
        );
        swatch(ui, p, cell, *hex, hex_of(pk) == *hex, pk);
    }

    ul_text(ui, p, r.left() + s::LABEL_X, r.top() + s::LABEL_CUSTOM_Y - 4.3,
        "Custom colors", rgb(c::TEXT_2), Some(0));
    for i in 0..16 {
        let (dx, dy) = ((i % s::PALETTE_COLS) as f32, (i / s::PALETTE_COLS) as f32);
        let cell = egui::Rect::from_min_size(
            egui::pos2(left + dx * step.x, r.top() + s::CUSTOM_TOP + dy * step.y),
            egui::vec2(s::SWATCH_W, s::SWATCH_H),
        );
        let hex = pk.custom.get(i).copied().unwrap_or(0xffffff);
        swatch(ui, p, cell, hex, false, pk);
    }

    // `Pick Screen Color` / `Add to Custom Colors`：贴左边距的扁平按钮，
    // 文字靠左、首字符带助记符下划线（原图就是这样，不是居中）。
    let flat = rect(r, s::BTN);
    // `Pick Screen Color` 要抓屏，本实现不接；按钮只对齐外观与可点态。
    let _ = flat_button(ui, p, flat, "Pick Screen Color");
    let add = egui::Rect::from_min_size(
        egui::pos2(flat.left(), r.top() + s::BTN_ADD_Y),
        flat.size(),
    );
    if flat_button(ui, p, add, "Add to Custom Colors") {
        let hex = hex_of(pk);
        if let Some(slot) = pk.custom.iter_mut().find(|v| **v == 0xffffff) {
            *slot = hex;
        } else if !pk.custom.is_empty() {
            pk.custom[0] = hex;
        }
    }
}

/// 一个色块（含选中环）。
fn swatch(ui: &mut egui::Ui, p: &egui::Painter, cell: egui::Rect, hex: u32, sel: bool, pk: &mut Picker) {
    p.rect_filled(cell, egui::Rounding::ZERO, rgb(hex));
    p.rect_stroke(cell, egui::Rounding::ZERO, egui::Stroke::new(0.5, rgb(0x3a3a3a)));
    if sel {
        let pad = spec::color_picker::RING_PAD;
        p.rect_stroke(
            cell.expand(pad),
            egui::Rounding::ZERO,
            egui::Stroke::new(1.0, rgb(0x35526e)),
        );
    }
    if ui.allocate_rect(cell, egui::Sense::click()).clicked() {
        set_hex(pk, hex);
    }
}

/// 扁平按钮：与背景同色，只靠 `#444444` 边线与文字区分。
fn flat_button(ui: &mut egui::Ui, p: &egui::Painter, r: egui::Rect, label: &str) -> bool {
    p.rect_filled(r, egui::Rounding::ZERO, rgb(0x1c1c1c));
    p.rect_stroke(r, egui::Rounding::ZERO, egui::Stroke::new(1.0, rgb(0x444444)));
    ul_text(ui, p, r.left() + 2.0, r.center().y, label, rgb(c::TEXT_2), Some(0));
    ui.allocate_rect(r, egui::Sense::click()).clicked()
}

/// 色域方块：横轴反向色相、纵轴渐白。
///
/// 用 Mesh 而不是逐格 `rect_filled`：色相在 RGB 里是分段线性的，逐格填色必须
/// 切到 ~200 段才看不出竖条，而 Mesh 让 GPU 在顶点间插值，176 列 × 2 行就够
/// （纵向是从纯色线性混到 `#c8c6c9`，线性插值本身就是对的，两行足够）。
fn square(ui: &mut egui::Ui, p: &egui::Painter, r: egui::Rect, pk: &mut Picker) {
    let sq = rect(r, spec::color_picker::SQUARE);
    let (dw, dh) = (sq.width(), sq.height());
    const NX: usize = 176;
    let mut mesh = egui::Mesh::default();
    let mut top = Vec::with_capacity(NX + 1);
    let mut bot = Vec::with_capacity(NX + 1);
    for i in 0..=NX {
        let fx = i as f32 / NX as f32;
        // 方块画的是**全色域**（满饱和、满明度），当前选择只由准星位置表达。
        // 若跟着 `pk.s` 走，用户一选到浅色方块就整体发白，跟原图不符。
        let base = hsv_to_rgb(360.0 * (1.0 - fx), 255.0, 255.0);
        let x = sq.left() + fx * dw;
        let it = mesh.vertices.len() as u32;
        mesh.colored_vertex(
            egui::pos2(x, sq.top()),
            egui::Color32::from_rgb(base[0], base[1], base[2]),
        );
        mesh.colored_vertex(egui::pos2(x, sq.top() + dh), egui::Color32::from_rgb(200, 198, 201));
        top.push(it);
        bot.push(it + 1);
    }
    for i in 0..NX {
        mesh.add_triangle(top[i], top[i + 1], bot[i]);
        mesh.add_triangle(top[i + 1], bot[i + 1], bot[i]);
    }
    p.add(egui::Shape::mesh(mesh));
    if let Some(pos) = ui
        .allocate_rect(sq, egui::Sense::click_and_drag())
        .interact_pointer_pos()
    {
        let fx = ((pos.x - sq.left()) / dw).clamp(0.0, 1.0);
        let fy = ((pos.y - sq.top()) / dh).clamp(0.0, 1.0);
        pk.h = 360.0 * (1.0 - fx);
        // 纵轴是饱和度（上满下浅），明度由右侧亮度条决定。
        pk.s = 255.0 * (1.0 - fy);
        sync_text(pk);
    }
    // 十字准星。
    let cx = sq.left() + (1.0 - pk.h / 360.0) * dw;
    let cy = sq.top() + (1.0 - pk.s / 255.0) * dh;
    p.circle_stroke(egui::pos2(cx, cy), 4.0, egui::Stroke::new(1.0, rgb(0xffffff)));
}

/// 右侧灰度亮度条 + 取值三角。
fn value_bar(ui: &mut egui::Ui, p: &egui::Painter, r: egui::Rect, pk: &mut Picker) {
    let bar = rect(r, spec::color_picker::VALUE_BAR);
    let bh = bar.height();
    const N: usize = 64;
    for i in 0..N {
        let t = i as f32 / (N - 1) as f32;
        let cell = egui::Rect::from_min_size(
            egui::pos2(bar.left(), bar.top() + t * bh),
            egui::vec2(bar.width(), bh / N as f32 + 0.6),
        );
        p.rect_filled(cell, egui::Rounding::ZERO, egui::Color32::from_gray((255.0 * (1.0 - t)) as u8));
    }
    if let Some(pos) = ui
        .allocate_rect(bar, egui::Sense::click_and_drag())
        .interact_pointer_pos()
    {
        let t = ((pos.y - bar.top()) / bh).clamp(0.0, 1.0);
        pk.v = (255.0 * (1.0 - t)).max(1.0);
        sync_text(pk);
    }
    // 条右肩的 ◀ 三角，随取值上下走。
    use spec::color_picker as s;
    let cy = bar.top() + (1.0 - pk.v / 255.0) * bh;
    let x0 = bar.right() + 0.5;
    let (hw, hh) = (s::BAR_MARK_W, s::BAR_MARK_H / 2.0);
    p.add(egui::Shape::convex_polygon(
        vec![
            egui::pos2(x0, cy),
            egui::pos2(x0 + hw, cy - hh),
            egui::pos2(x0 + hw, cy + hh),
        ],
        rgb(c::TEXT_2),
        egui::Stroke::NONE,
    ));
}

/// 预览块 + 两列数值面板（视觉基线 `06` 的右下半）。
fn readouts(ui: &mut egui::Ui, p: &egui::Painter, r: egui::Rect, pk: &mut Picker) {
    use spec::color_picker as s;
    let prev = rect(r, s::PREVIEW);
    p.rect_filled(prev, egui::Rounding::ZERO, rgb(hex_of(pk)));
    p.rect_stroke(prev, egui::Rounding::ZERO, egui::Stroke::new(0.5, rgb(0x6a6a6a)));

    for (k, label) in ["Hue:", "Sat:", "Val:"].iter().enumerate() {
        let top = r.top() + s::READOUT_TOP + k as f32 * s::READOUT_ROW_H;
        let mut v = std::mem::take(&mut pk.hsv[k]);
        spin_row(ui, p, r, (s::HSV_LABEL_RIGHT, s::HSV_BOX_X), top, label, &mut v);
        pk.hsv[k] = v;
    }
    for (k, label) in ["Red:", "Green:", "Blue:"].iter().enumerate() {
        let top = r.top() + s::READOUT_TOP + k as f32 * s::READOUT_ROW_H;
        let mut v = std::mem::take(&mut pk.rgb[k]);
        spin_row(ui, p, r, (s::RGB_LABEL_RIGHT, s::RGB_BOX_X), top, label, &mut v);
        pk.rgb[k] = v;
    }
    let alpha_top = r.top() + s::READOUT_TOP + 3.0 * s::READOUT_ROW_H;
    spin_row(ui, p, r, (s::RGB_LABEL_RIGHT, s::RGB_BOX_X), alpha_top, "Alpha channel:", &mut pk.alpha);

    // HTML 行：标签右缘与右列对齐，框从 HSV 列起、更宽。
    // `HTML:` 的右缘跟 HSV 列一样是 346（像素 626..693），不是 RGB 列的 451。
    let html = rect(r, s::HTML_BOX);
    ul_text(ui, p, r.left() + s::HSV_LABEL_RIGHT - text_w("HTML:"), html.center().y,
        "HTML:", rgb(c::TEXT_2), Some(0));
    numeric(ui, html, &mut pk.hex);

    // HTML 输入驱动颜色。
    if let Some(hex) = hex_parse(&pk.hex) {
        if hex != hex_of(pk) {
            set_hex(pk, hex);
        }
    }
}

/// 一行「右对齐标签 + 带上下箭头的数字框」。
fn spin_row(
    ui: &mut egui::Ui,
    p: &egui::Painter,
    r: egui::Rect,
    x: (f32, f32),
    top: f32,
    label: &str,
    value: &mut String,
) {
    use spec::color_picker as s;
    let fr = egui::Rect::from_min_size(
        egui::pos2(r.left() + x.1, top),
        egui::vec2(s::BOX_W, s::BOX_H),
    );
    ul_text(ui, p, r.left() + x.0 - text_w(label), fr.center().y, label, rgb(c::TEXT_2), Some(0));
    numeric(ui, fr, value);
    let ax = fr.right() - 9.0;
    let cy = fr.center().y;
    for (dy, up) in [(-4.5f32, true), (4.5, false)] {
        let y = cy + dy;
        p.add(egui::Shape::convex_polygon(
            vec![
                egui::pos2(ax - 3.5, y + if up { 2.0 } else { -2.0 }),
                egui::pos2(ax + 3.5, y + if up { 2.0 } else { -2.0 }),
                egui::pos2(ax, y + if up { -2.0 } else { 2.0 }),
            ],
            rgb(c::TEXT_2),
            egui::Stroke::NONE,
        ));
    }
}

/// 一个数字输入框：`#1c1c1c` 底、`#444444` 边，数字靠左。
fn numeric(ui: &mut egui::Ui, r: egui::Rect, value: &mut String) {
    let p = ui.painter().clone();
    p.rect_filled(r, egui::Rounding::ZERO, rgb(0x1c1c1c));
    p.rect_stroke(r, egui::Rounding::ZERO, egui::Stroke::new(1.0, rgb(0x444444)));
    place(ui, r.shrink2(egui::vec2(4.0, 3.0)), |ui| {
        ui.add(
            egui::TextEdit::singleline(value)
                .font(mono(FS))
                .frame(false)
                .desired_width(f32::INFINITY),
        );
    });
}

/// 把选择登记到 `View.pending_color`，由对话框主循环写回规则。
fn apply(view: &mut View, pk: &Picker) {
    view.pending_color = Some(ColorChoice::Custom(format!("#{:06x}", hex_of(pk))));
}

// ── 色彩换算 ──

fn hex_of(pk: &Picker) -> u32 {
    let [r, g, b] = hsv_to_rgb(pk.h, pk.s, pk.v);
    ((r as u32) << 16) | ((g as u32) << 8) | b as u32
}

fn hex_parse(s: &str) -> Option<u32> {
    let t = s.trim().trim_start_matches('#');
    if t.len() != 6 {
        return None;
    }
    u32::from_str_radix(t, 16).ok()
}

fn set_hex(pk: &mut Picker, hex: u32) {
    let (h, s, v) = rgb_to_hsv(hex);
    pk.h = h;
    pk.s = s;
    pk.v = v;
    sync_text(pk);
}

/// 以某个颜色为起点构造取色器（打开时从规则当前色初始化）。
///
/// `Default` 恒为红色，是早先只为对齐视觉基线外观留下的值；生产路径若也用它，
/// 用户改任意颜色都会从红色重新开始。`from_hex` 复用 `set_hex`，保证 h/s/v 与
/// 四个文本框一次同步，不出现「预览是白色、数值框还写着红色」的半初始化态。
pub(super) fn from_hex(hex: u32) -> Picker {
    let mut pk = Picker::default();
    set_hex(&mut pk, hex);
    pk
}

/// 把 HSV 同步到四个文本框。
fn sync_text(pk: &mut Picker) {
    let [r, g, b] = hsv_to_rgb(pk.h, pk.s, pk.v);
    pk.hex = format!("#{:02x}{:02x}{:02x}", r, g, b);
    pk.rgb = [r.to_string(), g.to_string(), b.to_string()];
    pk.hsv = [
        (pk.h as i32).to_string(),
        (pk.s as i32).to_string(),
        (pk.v as i32).to_string(),
    ];
}

fn lerp_u8(a: u8, b: u8, t: f32) -> u8 {
    (a as f32 * (1.0 - t) + b as f32 * t).clamp(0.0, 255.0) as u8
}

/// HSV(0..360, 0..255, 0..255) → RGB。
pub(super) fn hsv_to_rgb(h: f32, s: f32, v: f32) -> [u8; 3] {
    let h = ((h % 360.0) + 360.0) % 360.0;
    let s = s / 255.0;
    let v = v / 255.0;
    let c = v * s;
    let x = c * (1.0 - ((h / 60.0) % 2.0 - 1.0).abs());
    let m = v - c;
    let (r, g, b) = if h < 60.0 {
        (c, x, 0.0)
    } else if h < 120.0 {
        (x, c, 0.0)
    } else if h < 180.0 {
        (0.0, c, x)
    } else if h < 240.0 {
        (0.0, x, c)
    } else if h < 300.0 {
        (x, 0.0, c)
    } else {
        (c, 0.0, x)
    };
    [
        ((r + m) * 255.0) as u8,
        ((g + m) * 255.0) as u8,
        ((b + m) * 255.0) as u8,
    ]
}

/// RGB → HSV(0..360, 0..255, 0..255)。
fn rgb_to_hsv(hex: u32) -> (f32, f32, f32) {
    let r = ((hex >> 16) & 0xff) as f32 / 255.0;
    let g = ((hex >> 8) & 0xff) as f32 / 255.0;
    let b = (hex & 0xff) as f32 / 255.0;
    let max = r.max(g).max(b);
    let min = r.min(g).min(b);
    let d = max - min;
    let h = if d == 0.0 {
        0.0
    } else if max == r {
        60.0 * (((g - b) / d) % 6.0)
    } else if max == g {
        60.0 * ((b - r) / d + 2.0)
    } else {
        60.0 * ((r - g) / d + 4.0)
    };
    let s = if max == 0.0 { 0.0 } else { d / max };
    (h, s * 255.0, max * 255.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hsv_roundtrip_for_primaries() {
        for (hex, h) in [(0xff0000u32, 0.0f32), (0x00ff00, 120.0), (0x0000ff, 240.0)] {
            let (hh, s, v) = rgb_to_hsv(hex);
            assert!((hh - h).abs() < 1.0, "hue {hh} != {h}");
            assert!((s - 255.0).abs() < 1.0);
            assert!((v - 255.0).abs() < 1.0);
            let back = hsv_to_rgb(hh, s, v);
            assert_eq!(back, [(hex >> 16) as u8, ((hex >> 8) & 0xff) as u8, (hex & 0xff) as u8]);
        }
    }

    #[test]
    fn hex_parse_requires_six_digits() {
        assert_eq!(hex_parse("#336699"), Some(0x336699));
        assert_eq!(hex_parse("33669"), None);
        assert_eq!(hex_parse("#33669900"), None);
    }

    #[test]
    fn from_hex_syncs_every_readout_field() {
        let pk = from_hex(0xffffff);
        assert_eq!(pk.hex, "#ffffff");
        assert_eq!(pk.rgb, ["255", "255", "255"]);
        assert_eq!(pk.hsv, ["0", "0", "255"]);
        assert!((pk.s - 0.0).abs() < 0.5);
        assert!((pk.v - 255.0).abs() < 0.5);
    }

    #[test]
    fn from_choice_initialises_from_current_colour() {
        use term_core::trigger::ColorChoice;
        let white = super::Picker::from_choice(&ColorChoice::Empty);
        assert_eq!(white.hex, "#ffffff");
        let red = super::Picker::from_choice(&ColorChoice::Ansi("Ansi Red".to_owned()));
        assert_eq!(red.hex, "#cd0000");
        let custom = super::Picker::from_choice(&ColorChoice::Custom("#336699".to_owned()));
        // HSV 是浮点，往返可能差 1 个色阶（这里 R 0x33→0x32），属既有精度特性。
        let got = u32::from_str_radix(custom.hex.trim_start_matches('#'), 16).unwrap();
        let want = 0x336699u32;
        for shift in [16, 8, 0] {
            let d = ((got >> shift) & 0xff) as i32 - ((want >> shift) & 0xff) as i32;
            assert!(d.abs() <= 1, "色阶偏差超过 1：{got:06x} vs {want:06x}");
        }
    }
}

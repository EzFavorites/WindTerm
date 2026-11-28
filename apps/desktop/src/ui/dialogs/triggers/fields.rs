//! Trigger 对话框的通用绘制件：绝对定位的 field / combo / 标签 / 开关 / 按钮。
//!
//! 与 `dialog_chrome.rs` 的同名件刻意分开：那边尺寸是为 745 宽旧对话框钉死的，
//! 这里全部按 `ui::view::trigger_spec` 的 768×562 新坐标系。颜色来自同一份色板 token。

use crate::ui::*;
use crate::ui::view::trigger_spec::color as c;

/// u32 → Color32（本模块私有，避免和 dialog_chrome 的 rgb 争夺 pub 名）。
pub(in crate::ui::dialogs) fn rgb(hex: u32) -> egui::Color32 {
    crate::ui::kit::spec::col(hex)
}

/// 在指定矩形内绘制，坐标绝对、不跟随流式光标。
pub(in crate::ui::dialogs) fn place<R>(
    ui: &mut egui::Ui,
    rect: egui::Rect,
    add: impl FnOnce(&mut egui::Ui) -> R,
) -> R {
    ui.scope_builder(egui::UiBuilder::new().max_rect(rect), |ui| {
        ui.set_clip_rect(rect);
        add(ui)
    })
    .inner
}

/// 等宽字体 ID：全 UI 字体族统一由 `kit/type_scale` 提供，这里只转发。
pub(in crate::ui::dialogs) fn mono(size: f32) -> egui::FontId {
    crate::ui::kit::type_scale::font(size)
}

/// 正文 / 标签的字号（逻辑点）。
///
/// 视觉基线两套口径下 `Description:` 的墨迹都是 167 px、字距都是 14.7 px @2×，
/// 折算 **7.35 逻辑点/字符**。本机构建的等宽族是 Menlo（字距 0.602 em），
/// 因此 12.2 才对得上；此前一律按 11.0 画（字距 6.63），整体窄了 10%。
/// 这个值统一定义在 `kit/type_scale::SIZE_FIELD`，此处只是别名。
pub(in crate::ui::dialogs) const FS: f32 = crate::ui::kit::type_scale::SIZE_FIELD;

/// 等宽文本宽度与助记符下划线（实现在 [`super::text`]，这里只做转发，
/// 让页面的 `use super::fields::{...}` 不必再记一个新模块）。
pub(in crate::ui::dialogs) use super::text::{text_w, ul_text};

/// 两色线性插值（`t` 为 0..1）。
fn mix(a: u32, b: u32, t: f32) -> egui::Color32 {
    let ch = |sh: u32| {
        let x = ((a >> sh) & 0xFF) as f32 * (1.0 - t) + ((b >> sh) & 0xFF) as f32 * t;
        x.round().clamp(0.0, 255.0) as u8
    };
    egui::Color32::from_rgb(ch(16), ch(8), ch(0))
}

/// 视觉基线的「立体」控件底：按钮与下拉共用同一套（`01` 与 `11` 实测一致）。
///
/// 结构自外向内：1 点 `#141414` 阴影 → 上边 1 点高光 `hi` → 纵向渐变
/// `from..to` → 下边 1 点暗高光 `lo`。
pub(in crate::ui::dialogs) fn bevel(
    p: &egui::Painter,
    r: egui::Rect,
    hi: u32,
    lo: u32,
    from: u32,
    to: u32,
) {
    p.rect_filled(r.expand(1.0), egui::Rounding::ZERO, rgb(0x141414));
    let n = r.height().round().max(1.0) as i32;
    for i in 0..n {
        let t = (i as f32 + 0.5) / n as f32;
        let y = r.top() + i as f32 + 0.5;
        p.line_segment(
            [egui::pos2(r.left(), y), egui::pos2(r.right(), y)],
            egui::Stroke::new(1.0, mix(from, to, t)),
        );
    }
    p.line_segment([r.left_top(), r.right_top()], egui::Stroke::new(1.0, rgb(hi)));
    let yb = r.bottom() - 0.5;
    p.line_segment(
        [egui::pos2(r.left(), yb), egui::pos2(r.right(), yb)],
        egui::Stroke::new(1.0, rgb(lo)),
    );
}

/// 左对齐的字段标签：`LEFT_CENTER` 锚在 `(x, y)`，首个 `ul` 字符带下划线。
///
/// 视觉基线里 `Pattern:` / `Description:` / 四行 Style 标签都是这种「首字母下划线」
/// 的助记符写法，而下划线离基线下沉约 2 点。
pub(in crate::ui::dialogs) fn label_ul(ui: &egui::Ui, x: f32, y: f32, text: &str, ul: char) {
    let at = text.char_indices().position(|(_, ch)| ch == ul);
    super::text::ul_text(ui, &ui.painter().clone(), x, y, text, rgb(c::TEXT_2), at);
}

/// 正文标签：`text` 用 [`FS`] 等宽，颜色 `TEXT_2`。
pub(in crate::ui::dialogs) fn label(ui: &egui::Ui, pos: egui::Pos2, text: &str) {
    ui.painter()
        .text(pos, egui::Align2::LEFT_TOP, text, mono(FS), rgb(c::TEXT_2));
}

/// 在矩形内居中画一个带助记符下划线的标题（标签条用）。
pub(in crate::ui::dialogs) fn mnemonic_at(
    ui: &egui::Ui,
    p: &egui::Painter,
    rect: egui::Rect,
    text: &str,
    underline: char,
) {
    let font = mono(crate::ui::type_scale::SIZE_SMALL);
    let width = ui
        .fonts(|f| f.layout_no_wrap(text.to_owned(), font.clone(), rgb(c::TEXT)));
    let x = rect.center().x - width.rect.width() / 2.0;
    let y = rect.center().y - width.rect.height() / 2.0;
    p.text(
        egui::pos2(x, y),
        egui::Align2::LEFT_TOP,
        text,
        font.clone(),
        rgb(c::TEXT),
    );
    // 只在第一个匹配位置画一条 1px 下划线。
    let Some(idx) = text.char_indices().find(|(_, ch)| *ch == underline).map(|(i, _)| i) else {
        return;
    };
    let before = &text[..idx];
    let w_before = ui
        .fonts(|f| f.layout_no_wrap(before.to_owned(), font.clone(), rgb(c::TEXT)))
        .rect
        .width();
    let w_ch = ui
        .fonts(|f| f.layout_no_wrap(underline.to_string(), font.clone(), rgb(c::TEXT)))
        .rect
        .width();
    let uy = y + width.rect.height() - 1.0;
    p.line_segment(
        [egui::pos2(x + w_before, uy), egui::pos2(x + w_before + w_ch, uy)],
        egui::Stroke::new(1.0, rgb(c::TEXT)),
    );
}

/// 输入框：深底 `#1c1c1c` + `#404040` 边框，12px 等宽文字。返回 TextEdit 是否 changed。
pub(in crate::ui::dialogs) fn field(
    ui: &mut egui::Ui,
    rect: egui::Rect,
    value: &mut String,
    hint: &str,
    focus: bool,
) -> bool {
    let p = ui.painter().clone();
    let bg = if focus { c::BTN_BG } else { c::BG };
    p.rect_filled(rect, egui::Rounding::ZERO, rgb(bg));
    p.rect_stroke(
        rect,
        egui::Rounding::ZERO,
        egui::Stroke::new(
            if focus { 1.0 } else { 0.75 },
            rgb(if focus { c::BLUE_FOCUS } else { c::FIELD_LINE }),
        ),
    );
    let inner = rect.shrink2(egui::vec2(4.0, 2.0));
    let changed = place(ui, inner, |ui| {
        ui.style_mut().text_styles.insert(
            egui::TextStyle::Body,
            mono(FS),
        );
        let te = egui::TextEdit::singleline(value)
            .font(mono(FS))
            .text_color(rgb(c::TEXT_2))
            .hint_text(egui::RichText::new(hint).color(rgb(c::TEXT_4)))
            .desired_width(f32::INFINITY)
            .frame(false);
        ui.add(te).changed()
    });
    changed
}

/// 多行文本框（Match / Send / Pattern）。
///
/// `focused` 决定描边：视觉基线 `15`/`18` 的 Send 框是获得焦点的蓝边 `#007cd2`，
/// 其余框是灰边。焦点态由调用方显式给出——这是 1:1 对齐的静态状态，不跟随
/// 真实鼠标焦点，否则截图夹具每次出图的框色会随指针位置漂移。
pub(in crate::ui::dialogs) fn multiline(
    ui: &mut egui::Ui,
    rect: egui::Rect,
    value: &mut String,
    hint: &str,
    focused: bool,
) -> bool {
    let p = ui.painter().clone();
    p.rect_filled(rect, egui::Rounding::ZERO, rgb(c::BG));
    let line = if focused { c::BLUE_TRACK } else { c::FIELD_LINE };
    p.rect_stroke(
        rect,
        egui::Rounding::ZERO,
        egui::Stroke::new(0.75, rgb(line)),
    );
    place(ui, rect.shrink2(egui::vec2(4.0, 3.0)), |ui| {
        ui.add(
            egui::TextEdit::multiline(value)
                .font(mono(FS))
                .text_color(rgb(c::TEXT_2))
                .hint_text(egui::RichText::new(hint).color(rgb(c::TEXT_4)))
                .desired_width(f32::INFINITY)
                .frame(false),
        )
        .changed()
    })
}

/// 下拉框右端的实心下三角。
///
/// 视觉基线实测：7×3.5 逻辑点、右缘距框右 4.5 点、颜色 `#606060`（不是描边线）。
fn combo_arrow(p: &egui::Painter, rect: egui::Rect, col: u32) {
    let ax = rect.right() - 4.5;
    let cy = rect.center().y;
    p.add(egui::Shape::convex_polygon(
        vec![
            egui::pos2(ax - 7.0, cy - 1.75),
            egui::pos2(ax, cy - 1.75),
            egui::pos2(ax - 3.5, cy + 1.75),
        ],
        rgb(col),
        egui::Stroke::NONE,
    ));
}

/// 下拉框外观（不展开，仅显示值与箭头）。返回点击 Response。
pub(in crate::ui::dialogs) fn combo(
    ui: &mut egui::Ui,
    rect: egui::Rect,
    text: &str,
    focus: bool,
) -> egui::Response {
    let p = ui.painter().clone();
    if focus {
        bevel(&p, rect, c::BLUE_FOCUS, c::BLUE_FOCUS, 0x2a3038, 0x24282f);
    } else {
        bevel(&p, rect, 0x424242, 0x3c3c3c, 0x292929, 0x232323);
    }
    p.text(
        egui::pos2(rect.left() + 5.0, rect.center().y),
        egui::Align2::LEFT_CENTER,
        text,
        mono(FS),
        rgb(c::TEXT_2),
    );
    combo_arrow(&p, rect, 0x606060);
    ui.allocate_rect(rect, egui::Sense::click())
}

/// 带色块的下拉框（Highlight 弹窗的四行用）。
///
/// `swatch` 外层 `Some` 表示这行要画色块：内层 `Some(hex)` 是实色块，
/// `None` 画视觉基线里的「无颜色」方块（8×8、内填 `#262626`、一道 `/` 斜杠）；
/// 整行无色块（如 `Line Style`）传外层 `None`。
///
/// `enabled=false` 是视觉基线里 `Line Style = None` 时 `Line Color` 那一行的样子
/// （文字 `#555555`、色块与箭头一并压暗），但控件仍然可点。
pub(in crate::ui::dialogs) fn color_combo(
    ui: &mut egui::Ui,
    rect: egui::Rect,
    text: &str,
    swatch: Option<Option<u32>>,
    focus: bool,
    enabled: bool,
) -> egui::Response {
    let p = ui.painter().clone();
    if focus {
        bevel(&p, rect, c::BLUE_FOCUS, c::BLUE_FOCUS, 0x2a3038, 0x24282f);
    } else {
        bevel(&p, rect, 0x424242, 0x3c3c3c, 0x292929, 0x232323);
    }
    let mut text_x = rect.left() + 5.0;
    if let Some(fill) = swatch {
        let sw = 8.0;
        let sr = egui::Rect::from_min_size(
            egui::pos2(rect.left() + 5.0, rect.center().y - sw / 2.0),
            egui::vec2(sw, sw),
        );
        let (hi, lo) = if enabled { (0x737373, 0x525252) } else { (0x4a4a4a, 0x3a3a3a) };
        p.rect_filled(sr, egui::Rounding::ZERO, rgb(0x262626));
        p.line_segment([sr.left_top(), sr.right_top()], egui::Stroke::new(1.0, rgb(hi)));
        p.line_segment(
            [sr.left_bottom(), sr.right_bottom()],
            egui::Stroke::new(1.0, rgb(lo)),
        );
        p.line_segment([sr.left_top(), sr.left_bottom()], egui::Stroke::new(1.0, rgb(lo)));
        p.line_segment([sr.right_top(), sr.right_bottom()], egui::Stroke::new(1.0, rgb(lo)));
        match fill {
            Some(hex) => {
                p.rect_filled(sr.shrink(1.0), egui::Rounding::ZERO, rgb(hex));
            }
            None => {
                // 「无颜色」标记：左下到右上的一道斜杠。
                p.line_segment(
                    [sr.left_bottom() + egui::vec2(2.0, -2.0), sr.right_top() + egui::vec2(-2.0, 2.0)],
                    egui::Stroke::new(1.0, rgb(if enabled { 0x6d6d6d } else { 0x464646 })),
                );
            }
        }
        text_x = sr.right() + 7.0;
    }
    p.text(
        egui::pos2(text_x, rect.center().y),
        egui::Align2::LEFT_CENTER,
        text,
        mono(FS),
        rgb(if enabled { c::TEXT_2 } else { 0x555555 }),
    );
    combo_arrow(&p, rect, if enabled { 0x606060 } else { 0x3f3f3f });
    ui.allocate_rect(rect, egui::Sense::click())
}

/// 圆角开关（深蓝底白滑块），尺寸按视觉基线。返回是否 changed。
pub(in crate::ui::dialogs) fn toggle(
    ui: &mut egui::Ui,
    rect: egui::Rect,
    on: &mut bool,
) -> bool {
    let resp = ui.allocate_rect(rect, egui::Sense::click());
    if resp.clicked() {
        *on = !*on;
    }
    let p = ui.ctx().layer_painter(ui.layer_id()).with_clip_rect(rect);
    let r = rect.height() / 2.0;
    p.rect_filled(
        rect,
        egui::Rounding::same(r),
        rgb(if *on { c::BLUE_TRACK } else { c::LINE_SOFT }),
    );
    let knob = rect.height() - 2.0;
    let cx = if *on {
        rect.right() - 1.0 - knob / 2.0
    } else {
        rect.left() + 1.0 + knob / 2.0
    };
    p.circle_filled(egui::pos2(cx, rect.center().y), knob / 2.0, rgb(c::TEXT));
    resp.clicked()
}

/// Match 框底部的 flag bar：`.*  Aa  W` 三个可点切换。
pub(in crate::ui::dialogs) fn flag_bar(
    ui: &mut egui::Ui,
    rect: egui::Rect,
    flags: &mut crate::ui::dialogs::triggers::model::MatchFlags,
    literal: &mut bool,
    case_sensitive: &mut bool,
) {
    let p = ui.painter().clone();
    p.line_segment(
        [rect.left_top(), rect.right_top()],
        egui::Stroke::new(1.0, rgb(0x444444)),
    );
    let items = [
        ("Aa", !*case_sensitive),
        ("W", flags.whole_word),
        (".*", !*literal),
    ];
    // 视觉基线顺序：.* Aa W —— 手工排三个命中区。
    let labels = [".*", "Aa", "W"];
    let mut x = rect.left() + 1.5;
    for lab in labels.iter() {
        let w = mono(FS).size * 0.602 * lab.len() as f32;
        let hit = egui::Rect::from_min_size(
            egui::pos2(x, rect.top() + 1.0),
            egui::vec2(w + 4.0, rect.height() - 2.0),
        );
        let active = items.iter().find(|(s, _)| *s == *lab).map(|(_, v)| *v).unwrap_or(false);
        let resp = ui.allocate_rect(hit, egui::Sense::click());
        // 视觉基线里三个标记一律 `#a1a1a1`（未激活态），激活才提亮。
        let col = if resp.clicked() {
            c::BLUE
        } else if active {
            c::TEXT
        } else {
            0xa1a1a1
        };
        let font = mono(FS);
        let galley = ui.fonts(|f| f.layout_no_wrap((*lab).to_owned(), font.clone(), rgb(col)));
        let top = rect.center().y - galley.rect.height() / 2.0;
        p.galley(egui::pos2(x, top), galley, rgb(col));
        // `W` 在视觉基线里带助记符下划线。
        if *lab == "W" {
            let uy = top + mono(FS).size + 1.5;
            p.line_segment(
                [egui::pos2(x, uy), egui::pos2(x + w, uy)],
                egui::Stroke::new(1.0, rgb(col)),
            );
        }
        if resp.clicked() {
            match *lab {
                ".*" => *literal = !*literal,
                "Aa" => *case_sensitive = !*case_sensitive,
                "W" => flags.whole_word = !flags.whole_word,
                _ => {}
            }
        }
        x += w + 5.5;
    }
}

/// 页脚按钮的外观态。
#[derive(Clone, Copy, PartialEq, Eq)]
pub(in crate::ui::dialogs) enum Btn {
    /// 灰面（Cancel 一类）。
    Secondary,
    /// 蓝边蓝面（主操作 / 焦点态）。
    Primary,
    /// 灰面 + 暗字：视觉基线里「未改动」时 OK 就长这样。仍然可点——本实现还没有
    /// 改动跟踪（dirty）状态，外观先对齐视觉基线。
    Muted,
}

/// 页脚按钮：Cancel（灰面）/ Ok·Next（蓝面高亮）。返回是否点击。
pub(in crate::ui::dialogs) fn footer_button(
    ui: &mut egui::Ui,
    rect: egui::Rect,
    text: &str,
    primary: bool,
) -> bool {
    let kind = if primary { Btn::Primary } else { Btn::Secondary };
    footer_button_as(ui, rect, text, kind)
}

/// 页脚按钮的圆角半径（视觉基线 `21` / `26` / `28` 实测约 2 点）。
const BTN_RADIUS: f32 = 2.0;

/// 按钮面：圆角 + **四边统一的 1px 描边** + 竖向渐变填充。
///
/// 为什么不用 [`bevel`]：`bevel` 是「直角 + 只有上边亮、下边暗」的立体边，
/// 而视觉基线里 `Cancel` / `Next` / `Ok` 四边都是同一条 1px 线（次按钮
/// `#404040`、主按钮 `#3ea0ec`），且带 2 点圆角。早期按钮用 `bevel`，出的图
/// 是方块、主按钮只有上边一条蓝线——与视觉基线完全不是同一种控件。
fn button_face(p: &egui::Painter, r: egui::Rect, top: u32, bottom: u32, border: u32) {
    // 收 0.5 点：让 1px 描边整条落在框内，而不是骑在边界上溢出半格。
    let r = r.shrink(0.5);
    let n = r.height().round().max(1.0) as i32;
    for i in 0..n {
        let t = (i as f32 + 0.5) / n as f32;
        let y = r.top() + i as f32 + 0.5;
        // 圆角段按圆弧内缩，否则逐行铺色会画成直角、把圆角盖掉。
        let dy = if y < r.top() + BTN_RADIUS {
            r.top() + BTN_RADIUS - y
        } else if y > r.bottom() - BTN_RADIUS {
            y - (r.bottom() - BTN_RADIUS)
        } else {
            0.0
        };
        let inset = if dy > 0.0 {
            BTN_RADIUS - (BTN_RADIUS * BTN_RADIUS - dy * dy).max(0.0).sqrt()
        } else {
            0.0
        };
        p.line_segment(
            [
                egui::pos2(r.left() + inset, y),
                egui::pos2(r.right() - inset, y),
            ],
            egui::Stroke::new(1.0, mix(top, bottom, t)),
        );
    }
    p.rect_stroke(
        r,
        egui::Rounding::same(BTN_RADIUS),
        egui::Stroke::new(1.0, rgb(border)),
    );
}

/// 按钮里的居中文字；`ul` 给出时在首处画助记符下划线。
fn button_text(
    ui: &egui::Ui,
    p: &egui::Painter,
    rect: egui::Rect,
    text: &str,
    ul: Option<char>,
) {
    let font = mono(FS);
    let col = rgb(c::TEXT_2);
    let g = ui.fonts(|f| f.layout_no_wrap(text.to_owned(), font.clone(), col));
    let x = rect.center().x - g.rect.width() / 2.0;
    let y = rect.center().y - g.rect.height() / 2.0;
    p.text(egui::pos2(x, y), egui::Align2::LEFT_TOP, text, font.clone(), col);
    let Some(idx) = ul
        .and_then(|ch| text.char_indices().find(|(_, c)| *c == ch).map(|(i, _)| i))
    else {
        return;
    };
    let w_before = ui
        .fonts(|f| f.layout_no_wrap(text[..idx].to_owned(), font.clone(), col))
        .rect
        .width();
    let w_ch = ui
        .fonts(|f| f.layout_no_wrap(text[idx..].chars().next().unwrap_or(' ').to_string(), font.clone(), col))
        .rect
        .width();
    let uy = y + g.rect.height() - 1.0;
    p.line_segment(
        [egui::pos2(x + w_before, uy), egui::pos2(x + w_before + w_ch, uy)],
        egui::Stroke::new(1.0, col),
    );
}

/// 页脚按钮（`Cancel` / 主按钮），三种外观态。
pub(in crate::ui::dialogs) fn footer_button_as(
    ui: &mut egui::Ui,
    rect: egui::Rect,
    text: &str,
    kind: Btn,
) -> bool {
    let resp = ui.allocate_rect(rect, egui::Sense::click());
    let p = ui.painter().clone();
    // 视觉基线实测：次按钮填充 `#282828 → #232323`、描边 `#404040`、文字 `#bbbbbb`；
    // 主按钮填充 `#333a42 → #2c3239`、描边 `#3ea0ec`（四边同色）、文字 `#bbbbbb`
    // 且**首字母带助记符下划线**（`N̲ext` / `O̲k`）。次按钮没有下划线。
    let (top, bottom, border, ul) = match kind {
        Btn::Primary => (0x333a42, 0x2c3239, 0x3ea0ec, true),
        Btn::Secondary | Btn::Muted => (0x282828, 0x232323, 0x404040, false),
    };
    button_face(&p, rect, top, bottom, border);
    if kind == Btn::Muted {
        p.text(
            rect.center(),
            egui::Align2::CENTER_CENTER,
            text,
            mono(FS),
            rgb(0x555555),
        );
    } else {
        button_text(ui, &p, rect, text, ul.then(|| text.chars().next().unwrap_or(' ')));
    }
    resp.clicked()
}

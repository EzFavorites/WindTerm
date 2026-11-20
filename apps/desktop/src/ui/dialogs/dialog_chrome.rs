//! 对话框的绘制小件：绝对定位、色值、按钮、两列表格、标签+输入框。
//!
//! 2026-11-16 从 `settings_live_onekeys.rs` 抽出来——用户要求这两个对话框
//! **1:1 还原视觉基线**，几何全部按实测坐标绝对定位，因此需要一套「先算矩形、
//! 再往矩形里画」的公共件；TwoKeys 与 Triggers 各写一份就会漂。
//!
//! 尺寸常量（表头高、行高、输入框高）是实测值，见各常量注释。

use crate::ui::*;

/// 十六进制字面量 → 颜色（项目里 `col` 是各文件私有小工具，公共处只留这一份）。
pub(in crate::ui) fn rgb(hex: u32) -> egui::Color32 {
    egui::Color32::from_rgb((hex >> 16) as u8, ((hex >> 8) & 0xFF) as u8, (hex & 0xFF) as u8)
}

/// 铺输入框底色，且**不受父级 `add_enabled_ui(false)` 的褪色影响**。
///
/// 视觉基线里没有选中方案时字段底仍是 `#333333`；`ui.painter()` 在禁用作用域
/// 里会把颜色朝面板底混过去（实测 51→39），所以这里走不带 fade 的 layer painter。
pub(in crate::ui) fn fill_field(ui: &egui::Ui, rect: egui::Rect, hex: u32) {
    ui.ctx()
        .layer_painter(ui.layer_id())
        .with_clip_rect(rect)
        .rect_filled(rect, egui::Rounding::ZERO, rgb(hex));
}

/// 把 `add` 画进**指定的矩形**里，而不是跟着光标流式排。
///
/// 视觉基线的每个元素都有确定坐标；flow 布局的 `add_space` 会随字体度量漂移，
/// 逐像素对齐时就差出几个点。
///
/// 返回 `add` 的返回值（2026-11-17 泛型化）：调用方大多只要副作用（`R = ()`），
/// 但「在指定矩形里放一个按钮并拿它的 `Response`」是常态——原来的 `()` 签名逼着
/// 调用方用 `ctx.data` 中转，脏且慢一帧。
pub(in crate::ui) fn place<R>(
    ui: &mut egui::Ui,
    rect: egui::Rect,
    add: impl FnOnce(&mut egui::Ui) -> R,
) -> R {
    // clip 外扩 2px：描边(stroke)画在 rect 边界上，若把 clip 卡死在 rect，
    // 外侧那一半描边会被裁掉（实测 Settings 页脚 Close 的蓝框只剩两个角）。
    // 外扩 2px 容得下最常见的 1.5px 描边 + 抗锯齿，又不会让元素画到邻近控件。
    ui.scope_builder(egui::UiBuilder::new().max_rect(rect), |ui| {
        ui.set_clip_rect(rect.expand(2.0));
        add(ui)
    })
    .inner
}

/// 小方按钮（`+` / `-`）：视觉基线约 19×16，带 1px 边框。
pub(in crate::ui) fn tiny_button(ui: &mut egui::Ui, label: &str) -> egui::Response {
    let sense = if ui.is_enabled() {
        egui::Sense::click()
    } else {
        egui::Sense::hover()
    };
    let (rect, resp) = ui.allocate_exact_size(egui::vec2(19.0, 16.0), sense);
    let p = ui.painter_at(rect);
    let bg = if resp.hovered() && ui.is_enabled() {
        rgb(0x3A3A3A)
    } else {
        rgb(0x2A2A2A)
    };
    p.rect_filled(rect, egui::Rounding::ZERO, bg);
    p.rect_stroke(rect, egui::Rounding::ZERO, egui::Stroke::new(1.0, rgb(0x4A4A4A)));
    p.text(
        rect.center(),
        egui::Align2::CENTER_CENTER,
        label,
        egui::TextStyle::Button.resolve(ui.style()),
        rgb(0xC8C8C8),
    );
    resp
}

/// 纯文字按钮（`Duplicate` / `Rename`）：视觉基线无边框。
pub(in crate::ui) fn plain_button(ui: &mut egui::Ui, label: &str, enabled: bool) -> egui::Response {
    ui.add_enabled(
        enabled,
        egui::Button::new(egui::RichText::new(label).color(rgb(0xC8C8C8)))
            .frame(false)
            .min_size(egui::vec2(0.0, 16.0)),
    )
}

/// 带边框按钮（设置页页脚 `Cancel` / `Ok`）。`primary` 用参考实现的蓝调描边。
///
/// 尺寸与配色从 [`crate::ui::kit::spec::FooterButton`] 查；这里只保留
/// 「按内容流式排一行、返回 `Response`」这一层布局语义。
pub(in crate::ui) fn framed_button(
    ui: &mut egui::Ui,
    label: &str,
    enabled: bool,
    primary: bool,
) -> egui::Response {
    use crate::ui::kit::spec::FooterButton;
    let spec = if primary {
        FooterButton::SettingsOk.spec()
    } else {
        FooterButton::SettingsCancel.spec()
    };
    let (rect, resp) = ui.allocate_exact_size(spec.size, egui::Sense::click());
    crate::ui::kit::footer::draw(ui, rect, rect, label, spec, resp.hovered(), enabled);
    resp
}

/// 表头行高：视觉基线物理 322..358 → 18 逻辑点。
pub(in crate::ui) const THEAD_H: f32 = 18.0;
/// 表格数据行高：物理行距 39.5 → 19.5 逻辑点。
pub(in crate::ui) const TROW_H: f32 = 19.5;

/// 一张两列表格：表头 + 若干行，行内容由 `row` 画（行号从 0 起）。
pub(in crate::ui) fn table_frame(
    ui: &mut egui::Ui,
    rect: egui::Rect,
    rows: usize,
    head_l: &str,
    head_r: &str,
    mut row: impl FnMut(&mut egui::Ui, egui::Rect, usize),
) {
    let p = ui.painter().clone();
    p.rect_stroke(rect, egui::Rounding::ZERO, egui::Stroke::new(1.0, rgb(0x444444)));
    let head = egui::Rect::from_min_size(rect.min, egui::vec2(rect.width(), THEAD_H));
    p.rect_filled(head, egui::Rounding::ZERO, rgb(0x222222));
    p.line_segment(
        [head.left_bottom(), head.right_bottom()],
        egui::Stroke::new(1.0, rgb(0x3A3A3A)),
    );
    p.line_segment(
        [
            egui::pos2(rect.center().x, head.top()),
            egui::pos2(rect.center().x, rect.bottom()),
        ],
        egui::Stroke::new(1.0, rgb(0x3A3A3A)),
    );
    let font = egui::TextStyle::Button.resolve(ui.style());
    p.text(
        head.min + egui::vec2(4.0, 3.0),
        egui::Align2::LEFT_TOP,
        head_l,
        font.clone(),
        rgb(0xC8C8C8),
    );
    p.text(
        egui::pos2(rect.center().x + 4.0, head.top() + 3.0),
        egui::Align2::LEFT_TOP,
        head_r,
        font,
        rgb(0xC8C8C8),
    );
    for r in 0..rows {
        let top = head.bottom() + r as f32 * TROW_H;
        if top + TROW_H > rect.bottom() {
            break;
        }
        let rr = egui::Rect::from_min_size(
            egui::pos2(rect.left() + 1.0, top),
            egui::vec2(rect.width() - 2.0, TROW_H),
        );
        if r > 0 {
            p.line_segment(
                [rr.left_top(), rr.right_top()],
                egui::Stroke::new(1.0, rgb(0x242424)),
            );
        }
        row(ui, rr, r);
    }
}

/// 输入框高：视觉基线物理 110..158 → 24 逻辑点。
pub(in crate::ui) const FIELD_HH: f32 = 24.0;

/// 一行「标签 + 输入框」，两个 y 都是相对容器顶的实测值。
pub(in crate::ui) fn labeled_field(
    ui: &mut egui::Ui,
    width: f32,
    label: &str,
    label_y: f32,
    field_y: f32,
    value: &mut String,
) {
    let o = ui.cursor().min;
    place(
        ui,
        egui::Rect::from_min_size(o + egui::vec2(0.0, label_y), egui::vec2(width, 12.0)),
        |ui| {
            ui.label(egui::RichText::new(label).color(rgb(0xC8C8C8)));
        },
    );
    let rect = egui::Rect::from_min_size(
        o + egui::vec2(0.0, field_y),
        egui::vec2(width, FIELD_HH),
    );
    fill_field(ui, rect, 0x333333);
    place(ui, rect.shrink2(egui::vec2(3.0, 2.0)), |ui| {
        ui.add(
            egui::TextEdit::singleline(value)
                .desired_width(f32::INFINITY)
                .frame(false),
        );
    });
}

/// 深底输入框：`#333333` 铺底、无边框、`FIELD_HH` 高。
///
/// 与 [`labeled_field`] 画出来的那个框**同一套观感**——那个把「标签 + 框」一起
/// 画，这个只画框，给「一个框不独占一行」的地方用（OneKeys 的 Public Key 表、
/// Keyboard Interactive 表、Account 的 Expect/Send 行）。
///
/// 2026-11-17 之前这些地方各写一份 `TextEdit::frame(false)` 且**不铺底**：同一页
/// 里 Username 是灰底框、Identity File 是裸文字，看起来像两个软件。
pub(in crate::ui) fn dark_field(
    ui: &mut egui::Ui,
    rect: egui::Rect,
    value: &mut String,
) -> egui::Response {
    fill_field(ui, rect, 0x333333);
    // 内缩 3×2：输入框本身不带底色，靠这层铺底当边框；裁切仍用整格 `rect`，
    // 光标贴边时不会被削。
    place(ui, rect.shrink2(egui::vec2(3.0, 2.0)), |ui| {
        ui.add(
            egui::TextEdit::singleline(value)
                .desired_width(f32::INFINITY)
                .frame(false),
        )
    })
}

/// 参考实现的圆角开关（Triggers `Rules:` 行右端那颗）。
///
/// 实测（物理 1450..1513 × 82..113 → 逻辑 **31.5×15.5**）：深蓝底 `#003E69`，
/// 两端是**完整半圆**（圆角 = 高的一半 = 7.75）；白色圆滑块直径 11.5，
/// **四周各留 2 点**（上下 2、左右 2），开态贴右、关态贴左。
///
/// 注意：早先版本把高度测成 11——那是取样窗口在 y 方向截断导致的，真实高度
/// 15.5。高度一错，圆角就只有 5.5（不再是半圆），宽高比也偏扁，看起来「太宽」。
///
/// 为什么不直接用 `Checkbox`：egui 的 checkbox 是方框 + 对勾，与视觉基线的胶囊
/// 滑块完全不是一个形状。也不用 `add_enabled_ui` 包：那会把绘制乘灰，而视觉基线
/// 里没有选中项时开关仍是蓝的（只是不可点）——所以 `enabled` 只关交互。
pub(in crate::ui) fn toggle(
    ui: &mut egui::Ui,
    size: egui::Vec2,
    on: &mut bool,
    enabled: bool,
) -> egui::Response {
    let sense = if enabled {
        egui::Sense::click()
    } else {
        egui::Sense::hover()
    };
    let (rect, mut resp) = ui.allocate_exact_size(size, sense);
    if resp.clicked() {
        *on = !*on;
        resp.mark_changed();
    }
    // 不能用 `ui.painter_at`：父级若是 `add_enabled_ui(false)`，Painter 会带
    // `fade_to_color`，白色滑块被冲淡成中灰（实测 OneKeys 里就是这个问题）。
    // `layer_painter` 不带 fade，颜色按写死的值出。
    let p = ui.ctx()
        .layer_painter(ui.layer_id())
        .with_clip_rect(rect);
    let r = size.y / 2.0;
    p.rect_filled(
        rect,
        egui::Rounding::same(r),
        if *on { rgb(0x003E69) } else { rgb(0x3A3A3A) },
    );
    // 参考实现没有额外的亮描边（放大 7 倍看只有抗锯齿过渡），画上去反而显得厚。
    // 关态留一圈浅边是为了在深底上能分出边界。
    if !*on {
        p.rect_stroke(rect, egui::Rounding::same(r), egui::Stroke::new(1.0, rgb(0x4A4A4A)));
    }
    // 滑块四周各留 2 点（实测：胶囊 15.5 高 / 滑块 11.5 直径）。
    let knob = size.y - 4.0;
    let cx = if *on {
        rect.right() - 2.0 - knob / 2.0
    } else {
        rect.left() + 2.0 + knob / 2.0
    };
    p.circle_filled(egui::pos2(cx, rect.center().y), knob / 2.0, rgb(0xFFFFFF));
    resp
}

/// 一个密码输入框（`frame(false)` + 可选明码）。
pub(in crate::ui) fn password_field(
    ui: &mut egui::Ui,
    width: f32,
    y: f32,
    value: &mut String,
    visible: bool,
) -> bool {
    let o = ui.cursor().min;
    let rect = egui::Rect::from_min_size(o + egui::vec2(0.0, y), egui::vec2(width, FIELD_HH));
    fill_field(ui, rect, 0x333333);
    let mut changed = false;
    place(ui, rect.shrink2(egui::vec2(3.0, 2.0)), |ui| {
        changed = ui
            .add(
                egui::TextEdit::singleline(value)
                    .password(!visible)
                    .desired_width(f32::INFINITY)
                    .frame(false),
            )
            .changed();
    });
    changed
}

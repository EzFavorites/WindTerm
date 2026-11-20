//! 页脚按钮的**唯一绘制原语**（L3）。
//!
//! 四套页脚按钮的外观差异是有意的（见 [`super::spec::FooterButton`]），
//! 所以原语不规定「按钮长什么样」，只规定「规格怎么翻译成 egui 形状」：
//! 底 / 渐变 / 描边 / 文字，全部从规格表读。调用方只负责算矩形与拿
//! `Response`，这样交互 id 与点击归属完全留在原来的位置，零像素回归。

use eframe::egui;

use super::spec::{ButtonGlyph, FooterButtonSpec};

/// `framed_button(false)`  historically 的禁用态：不使用主题色，避免父级
/// `add_enabled_ui` 把按钮整块冲淡（与 `dialog_chrome::toggle` 同一条教训）。
const DISABLED_FILL: u32 = 0x242424;
const DISABLED_BORDER: u32 = 0x4A4A4A;
const DISABLED_TEXT: u32 = 0x6A6A6A;

/// 按规格画一颗按钮。`hovered` 由调用方从自己的 `Response` 传进来：
/// 菜单弹层里的指针状态可能是夹具钉出来的，不一定是当前真鼠标。
///
/// `clip` 必须传调用方原来的画笔裁剪矩形：描边有一半落在按钮矩形外侧，
/// 把画笔裁到 `rect` 会削掉那一圈抗锯齿——确认弹窗的蓝边会少一像素。
/// 各调用点原来裁的是「整个窗口」还是「按钮自己」并不一致，这里照原样传。
pub(in crate::ui) fn draw(
    ui: &egui::Ui,
    clip: egui::Rect,
    rect: egui::Rect,
    label: &str,
    spec: FooterButtonSpec,
    hovered: bool,
    enabled: bool,
) {
    let p = ui.painter_at(clip);
    if let Some((top, bottom)) = spec.gradient {
        gradient(&p, rect, top, bottom);
    } else {
        let fill = if !enabled {
            DISABLED_FILL
        } else if hovered {
            spec.hover_fill
        } else {
            spec.fill
        };
        p.rect_filled(rect, spec.rounding, crate::theme::chrome(fill));
    }
    let border = if enabled { spec.border } else { DISABLED_BORDER };
    p.rect_stroke(
        rect.shrink(spec.stroke_inset),
        spec.rounding,
        egui::Stroke::new(spec.border_width, crate::theme::chrome(border)),
    );
    let color = if enabled { spec.text } else { DISABLED_TEXT };
    match spec.glyph {
        ButtonGlyph::Widget => {
            p.text(
                rect.center(),
                egui::Align2::CENTER_CENTER,
                label,
                egui::TextStyle::Button.resolve(ui.style()),
                crate::theme::chrome(color),
            );
        }
        ButtonGlyph::Monospace { size, underline_first } => {
            let font = crate::ui::type_scale::font(size);
            if underline_first {
                let (first, rest) = split_first_char(label);
                let mut job = egui::text::LayoutJob::default();
                let plain = egui::TextFormat::simple(font.clone(), crate::theme::chrome(color));
                job.append(
                    first,
                    0.0,
                    egui::TextFormat {
                        underline: egui::Stroke::new(1.0, crate::theme::chrome(color)),
                        ..plain.clone()
                    },
                );
                job.append(rest, 0.0, plain);
                let galley = p.layout_job(job);
                p.galley(
                    rect.center() - egui::vec2(galley.size().x / 2.0, galley.size().y / 2.0),
                    galley,
                    crate::theme::chrome(color),
                );
            } else {
                p.text(
                    rect.center(),
                    egui::Align2::CENTER_CENTER,
                    label,
                    font,
                    crate::theme::chrome(color),
                );
            }
        }
    }
    // 交互 id 留在调用方：原语不知道按钮属于哪个弹窗，也不该替它造 id。
}

/// 首字符与其余部分。**不能 `split_at(1)`**：中文按钮字（「否」/「是」）
/// 是多字节 UTF-8，切在字节中间直接 panic（zh 出图实测过）。
fn split_first_char(s: &str) -> (&str, &str) {
    match s.char_indices().nth(1) {
        Some((i, _)) => s.split_at(i),
        None => (s, ""),
    }
}

/// 自上而下的逐行渐变。扫描线数按高度向上取整，步长与迁移前逐字一致。
fn gradient(p: &egui::Painter, rect: egui::Rect, top: u32, bottom: u32) {
    let top = crate::theme::chrome(top);
    let bottom = crate::theme::chrome(bottom);
    let steps = rect.height().ceil() as usize;
    for i in 0..steps {
        let t = i as f32 / (steps.max(2) - 1) as f32;
        let c = lerp(top, bottom, t);
        let y = rect.top() + i as f32;
        p.line_segment(
            [egui::pos2(rect.left(), y), egui::pos2(rect.right(), y)],
            egui::Stroke::new(1.0, c),
        );
    }
}

fn lerp(a: egui::Color32, b: egui::Color32, t: f32) -> egui::Color32 {
    let f = |x: u8, y: u8| (x as f32 + (y as f32 - x as f32) * t).round() as u8;
    egui::Color32::from_rgb(f(a.r(), b.r()), f(a.g(), b.g()), f(a.b(), b.b()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_chinese_first_char_is_not_split_in_the_middle_of_a_utf8_sequence() {
        assert_eq!(split_first_char("否"), ("否", ""));
        assert_eq!(split_first_char("取消"), ("取", "消"));
        assert_eq!(split_first_char("No"), ("N", "o"));
    }
}

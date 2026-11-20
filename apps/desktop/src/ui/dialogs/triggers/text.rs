//! 等宽文本的排字与助记符下划线（R9：从 `fields.rs` 拆出）。
//!
//! 两件事必须只有一份实现：
//! - [`text_w`]：按字宽摆位。以前各处用 `mono(11).size * 0.62` 估，偏小 8%，
//!   相邻两段文字会直接叠在一起（Sessions 页的 `Check All`/`Uncheck All`）。
//! - [`ul_text`]：`Ansi Bright Black` 的下划线要落在 `Black` 的 `B` 上而不是
//!   `Bright` 的，所以位置必须由调用方给**字符下标**，不能靠「首个匹配字符」。

use super::fields::{mono, FS};

/// 等宽文本的排字宽度（逻辑点）。
pub(in crate::ui::dialogs) fn text_w(s: &str) -> f32 {
    s.chars().count() as f32 * FS * 0.602
}

/// 一段墨迹的位置与样式（给 [`underline`] 打包参数，免得参数过 7 个）。
struct Ink<'a> {
    text: &'a str,
    x: f32,
    /// 墨迹顶（`galley` 顶边，不是基线）。
    y0: f32,
    h: f32,
    col: egui::Color32,
}

/// `LEFT_CENTER` 锚定的一段文字，第 `ul_at` 个字符（**字符**下标）带下划线。
pub(in crate::ui::dialogs) fn ul_text(
    ui: &egui::Ui,
    p: &egui::Painter,
    left: f32,
    center_y: f32,
    text: &str,
    col: egui::Color32,
    ul_at: Option<usize>,
) {
    let galley = ui.fonts(|f| f.layout_no_wrap(text.to_owned(), mono(FS), col));
    let h = galley.rect.height();
    let y0 = center_y - h / 2.0;
    p.galley(egui::pos2(left, y0), galley, col);
    if let Some(i) = ul_at {
        if let Some((byte, _)) = text.char_indices().nth(i) {
            underline(
                ui,
                p,
                Ink { text, x: left, y0, h, col },
                byte,
            );
        }
    }
}

/// 给 `ink.text` 里从 `byte` 起的那个字符画下划线。
fn underline(ui: &egui::Ui, p: &egui::Painter, ink: Ink<'_>, byte: usize) {
    let font = mono(FS);
    let w_pre = ui
        .fonts(|f| f.layout_no_wrap(ink.text[..byte].to_owned(), font.clone(), ink.col))
        .rect
        .width();
    let ch = ink.text[byte..].chars().next().unwrap_or(' ');
    let w_ch = ui
        .fonts(|f| f.layout_no_wrap(ch.to_string(), font, ink.col))
        .rect
        .width();
    let uy = ink.y0 + ink.h - 1.0;
    p.line_segment(
        [
            egui::pos2(ink.x + w_pre, uy),
            egui::pos2(ink.x + w_pre + w_ch, uy),
        ],
        egui::Stroke::new(1.0, ink.col),
    );
}

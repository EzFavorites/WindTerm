//! Ui 级组合字段：**标签 + 控件**这一层，函数式原语。
//!
//! `kit::form` 管的是「字段框 + 行」的排布；这一差一层：调用方只需说
//! 「这是一行带访问键的勾选项」「这是一个通栏输入框，右端内嵌浏览按钮」，
//! 不必再在弹窗里手写 `ui.horizontal` + 裸 egui 控件。输出只由入参决定，
//! 组件夹具可以逐字节定格（ADR-0021：不做 retained 组件）。

use eframe::egui;

use super::form::{field_box, field_stroke};

/// 访问键下划线的两套字体口径。两处原先各抄一份，现在收成变体：
/// `Form` 是设置页的灰标签（Body + PLACEHOLDER），`Dialog` 是会话弹窗的
/// 正文标签（Button + 正文色）。两者都把下划线颜色烤进 galley——透明色
/// 在 layout 阶段就丢了，画出来是看不见的。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::ui) enum UnderlineStyle {
    Form,
    Dialog,
    /// 会话弹窗的正文勾选/单选：金标准里这些长句标签**没有**访问键下划线。
    /// 用这个变体替换裸 `ui.checkbox` 才能做到逐像素不变。
    None,
}

/// 生成「访问键首字母带下划线」的 `LayoutJob`。
///
/// `acc_word` 是英文原文里要划的词；显示文本中找到它的首字符就划那个字，
/// 中文退化为划首字。`style` 决定字体与文字色，见 [`UnderlineStyle`]。
pub(in crate::ui) fn underlined_job(
    ui: &egui::Ui,
    shown: &str,
    acc_word: &str,
    style: UnderlineStyle,
) -> egui::text::LayoutJob {
    let (font, color) = match style {
        UnderlineStyle::Form => (
            egui::TextStyle::Body.resolve(ui.style()),
            egui::Color32::PLACEHOLDER,
        ),
        UnderlineStyle::Dialog | UnderlineStyle::None => (
            egui::TextStyle::Button.resolve(ui.style()),
            ui.visuals().text_color(),
        ),
    };
    let plain = egui::TextFormat::simple(font, color);
    let mut marked = plain.clone();
    if style != UnderlineStyle::None {
        marked.underline = egui::Stroke::new(1.0, ui.visuals().text_color());
    }

    let key = acc_word.chars().next().unwrap_or('A');
    let mut job = egui::text::LayoutJob::default();
    match shown.find(key) {
        Some(i) if shown.is_char_boundary(i) => {
            job.append(&shown[..i], 0.0, plain.clone());
            job.append(&key.to_string(), 0.0, marked);
            job.append(&shown[i + key.len_utf8()..], 0.0, plain);
        }
        _ => {
            // 访问键词没在显示文本里出现（典型：中文界面 + 英文 acc_word），
            // 退化为划首字——与设置页原注释的承诺一致，不留下「没有下划线的标签」。
            let first = shown.chars().next().map(|c| c.len_utf8()).unwrap_or(0);
            if first > 0 {
                job.append(&shown[..first], 0.0, marked);
                job.append(&shown[first..], 0.0, plain);
            } else {
                job.append(shown, 0.0, plain);
            }
        }
    }
    job
}

/// 只读选项的绘制口径：egui 的 `add_enabled(false)` 会把控件与文字一起
/// 冲淡，看起来像「坏掉」；兼容终端的只读行仍是正常亮度，只是点不动。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::ui) enum ChoiceKind {
    Checkbox,
    Radio,
}

fn paint_choice(
    ui: &mut egui::Ui,
    kind: ChoiceKind,
    label: &str,
    acc_word: &str,
    value: &mut bool,
    interactive: bool,
    underline: UnderlineStyle,
) -> egui::Response {
    let job = underlined_job(ui, label, acc_word, underline);
    let galley = ui.fonts(|f| f.layout_job(job));
    let h = ui
        .fonts(|f| f.row_height(&egui::TextStyle::Button.resolve(ui.style())))
        .max(14.0);
    // 所有勾选/单选共用同一套几何：方框 12×12、文字左缘固定 18pt。
    // 此前「选中」走 egui 原生 Checkbox、「未选中」走这份手绘，两者
    // 大小与描边都不同，一眼就能看出不是一组控件。
    let box_side = 12.0;
    let width = box_side + 6.0 + galley.size().x;
    let sense = if interactive {
        egui::Sense::click()
    } else {
        egui::Sense::hover()
    };
    let (rect, mut resp) = ui.allocate_exact_size(egui::vec2(width, h), sense);
    if interactive && resp.clicked() {
        *value = !*value;
        resp.mark_changed();
    }
    // 不能用 `painter_at(rect)`：它的裁剪区正好是 rect，而 1pt 描边有一半
    // 落在 rect 左缘之外，画出来就是「radio / checkbox 左边缺一截」。
    // 这里只画自己刚分配的矩形，用父级 painter 即可。
    let p = ui.painter();
    // 鼠标落在整行上都算命中，但只在方框/圆圈上给视觉反馈。
    let ink = ui.visuals().text_color();
    let hovered = interactive && resp.hovered();
    let mark = egui::Rect::from_min_size(
        egui::pos2(rect.left(), rect.center().y - box_side / 2.0),
        egui::vec2(box_side, box_side),
    );
    let stroke = egui::Stroke::new(1.0, ink);
    match kind {
        ChoiceKind::Checkbox => {
            p.rect_filled(
                mark,
                egui::Rounding::ZERO,
                if hovered {
                    egui::Color32::from_white_alpha(18)
                } else {
                    egui::Color32::TRANSPARENT
                },
            );
            p.rect_stroke(mark, egui::Rounding::ZERO, stroke);
            if *value {
                // 勾的两段都落在 12×12 方框内部（旧坐标右端跑到框外）。
                p.line_segment(
                    [
                        mark.left_top() + egui::vec2(2.5, 6.5),
                        mark.left_top() + egui::vec2(5.5, 9.0),
                    ],
                    stroke,
                );
                p.line_segment(
                    [
                        mark.left_top() + egui::vec2(5.5, 9.0),
                        mark.left_top() + egui::vec2(9.5, 3.0),
                    ],
                    stroke,
                );
            }
        }
        ChoiceKind::Radio => {
            if hovered {
                p.circle_filled(mark.center(), box_side / 2.0, egui::Color32::from_white_alpha(18));
            }
            p.circle_stroke(mark.center(), box_side / 2.0, stroke);
            if *value {
                p.circle_filled(mark.center(), box_side / 4.0, ink);
            }
        }
    }
    p.galley(
        egui::pos2(rect.left() + box_side + 6.0, rect.center().y - galley.size().y / 2.0),
        galley,
        ink,
    );
    resp
}

/// 一行勾选/单选。`interactive = false` 时画成**正常亮度的只读行**，
/// 而不是 `add_enabled(false)` 那种灰掉的失效控件。
///
/// 返回 `true` 表示本帧被点击（只读时恒 `false`）。
pub(in crate::ui) fn choice_row(
    ui: &mut egui::Ui,
    kind: ChoiceKind,
    label: &str,
    acc_word: &str,
    value: &mut bool,
    interactive: bool,
) -> bool {
    choice_row_styled(ui, kind, label, acc_word, value, interactive, UnderlineStyle::Dialog)
}

/// 同 [`choice_row`]，但指定访问键下划线的样式。会话弹窗的正文勾选传
/// [`UnderlineStyle::None`]，替换裸 `ui.checkbox` 时像素不变。
pub(in crate::ui) fn choice_row_styled(
    ui: &mut egui::Ui,
    kind: ChoiceKind,
    label: &str,
    acc_word: &str,
    value: &mut bool,
    interactive: bool,
    underline: UnderlineStyle,
) -> bool {
    choice_row_resp(ui, kind, label, acc_word, value, interactive, underline).clicked()
}

/// 同 [`choice_row_styled`]，但返回 `Response`，供需要挂 hover 说明的调用点用。
pub(in crate::ui) fn choice_row_resp(
    ui: &mut egui::Ui,
    kind: ChoiceKind,
    label: &str,
    acc_word: &str,
    value: &mut bool,
    interactive: bool,
    underline: UnderlineStyle,
) -> egui::Response {
    paint_choice(ui, kind, label, acc_word, value, interactive, underline)
}

/// 一行「标题 + 等宽横向单选」（会话弹窗的 Keyboard / Mode / Security /
/// Logging 都用这一套）。金标准里同组选项是对齐的三列，而不是竖着排成一串；
/// 标题单独占一行、不塞进窄标签列，长标题因此不会被截成 `Backspace se…`。
///
/// 返回被点的下标。`slot_w` 保留兼容旧调用点；选项按文字自然宽度排布，
/// 固定槽宽会把长文案顶出内容区被裁掉。
pub(in crate::ui) fn radio_choices(
    ui: &mut egui::Ui,
    label: &str,
    options: &[&str],
    selected: usize,
    interactive: bool,
    slot_w: f32,
) -> Option<usize> {
    ui.label(label);
    let mut clicked = None;
    let _ = slot_w; // 兼容旧调用点；选项按文字自然宽度排，避免长文案被裁。
    ui.horizontal(|ui| {
        for (i, name) in options.iter().enumerate() {
            if i > 0 {
                ui.add_space(12.0);
            }
            let mut on = selected == i;
            if choice_row_styled(
                ui,
                ChoiceKind::Radio,
                name,
                "",
                &mut on,
                interactive,
                UnderlineStyle::None,
            ) {
                clicked = Some(i);
            }
        }
    });
    clicked
}

/// 通栏输入框，右端**内嵌**浏览槽（`| ...`）——目录选择框的公共画法。
///
/// 旧写法是 `TextEdit(desired_width(300))` + 框外一个独立按钮：总宽不足、
/// 浏览按钮飘在框外、控件高度还差 4pt。这里把三者放进同一个
/// `field_box`，框线只有一圈，浏览槽用一条竖线与输入区分开。
///
/// 返回 `(输入框 Response, 浏览按钮 Response)`。调用方在浏览按钮上挂
/// `on_hover_text` 并处理点击；本原语不认识 `PathTarget` 之类的业务。
pub(in crate::ui) fn browse_field(
    ui: &mut egui::Ui,
    text: &mut String,
    hint: &str,
    browse_label: &str,
    height: f32,
    browse_width: f32,
) -> (egui::Response, egui::Response) {
    let mut responses = None;
    field_box(ui, |ui| {
        responses = Some(ui.horizontal(|ui| {
            ui.set_height(height);
            // `set_width` 只抬下限；这里手动把余宽钉死，避免两个控件
            // 各自按内容算宽后右侧留一大段空白。
            let edit_w = (ui.available_width() - browse_width - 8.0).max(20.0);
            let edit = ui.add_sized(
                egui::vec2(edit_w, height),
                egui::TextEdit::singleline(text)
                    .hint_text(hint)
                    .frame(false),
            );
            ui.add_space(6.0);
            let rect = ui.allocate_exact_size(
                egui::vec2(browse_width, height),
                egui::Sense::click(),
            );
            let browse = rect.1;
            let p = ui.painter_at(rect.0);
            if browse.hovered() {
                p.rect_filled(rect.0, egui::Rounding::ZERO, egui::Color32::from_white_alpha(12));
            }
            // 竖线（视觉基线 `| ...`）：距按钮左缘 1pt，上下各留 8pt。
            let d = rect.0.left() + 1.0;
            p.line_segment(
                [
                    egui::pos2(d, rect.0.top() + 8.0),
                    egui::pos2(d, rect.0.bottom() - 8.0),
                ],
                field_stroke(),
            );
            p.text(
                rect.0.center() + egui::vec2(2.0, 0.0),
                egui::Align2::CENTER_CENTER,
                browse_label,
                egui::TextStyle::Button.resolve(ui.style()),
                ui.visuals().text_color(),
            );
            (edit, browse)
        })
        .inner);
    });
    responses.expect("field_box always runs its closure")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn underlined_job_marks_access_key_not_every_matching_char() {
        egui::__run_test_ui(|ui| {
            let job = underlined_job(ui, "Language", "Language", UnderlineStyle::Form);
            // 三段：前缀（空）、访问键、余串。egui 0.29 的 section 只存
            // `byte_range`，文字本体在 `job.text`。
            assert_eq!(job.sections.len(), 3);
            let slice = |i: usize| &job.text[job.sections[i].byte_range.clone()];
            assert_eq!(slice(1), "L");
            assert!(job.sections[1].format.underline.width > 0.0);
            assert_eq!(slice(2), "anguage");
        });
    }

    #[test]
    fn chinese_falls_back_to_the_first_character() {
        egui::__run_test_ui(|ui| {
            let job = underlined_job(ui, "外观", "Appearance", UnderlineStyle::Dialog);
            // 回退时首字被标成访问键，因此它是第 0 段（不是英文命中时的第 1 段）。
            assert_eq!(&job.text[job.sections[0].byte_range.clone()], "外");
            assert!(job.sections[0].format.underline.width > 0.0);
        });
    }
}

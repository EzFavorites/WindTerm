//! `settings_pages` 的**画法**：分组框、两列栅格、灰显控件、只读快捷键表。
//!
//! 与数据分开是为了 R9（`ui/` 下非 `mod.rs` 文件 ≤ 450 行）：十六页的规格表
//! 自己就三百多行，画法再挤进去就超了。两边按「数据 / 画法」切，
//! 加一页只动数据文件，改外观只动这个文件。

use super::settings_pages::{effective_chords, page_note, page_spec, Block, Row};
use super::settings::SettingsPage;
use crate::ui::*;

pub(in crate::ui) fn draw(ui: &mut egui::Ui, page: SettingsPage, overrides: &[(usize, String)]) {
    let Some(spec) = page_spec(page) else { return };
    // 说明文字跟着页走：有的页缺的是「哪个字段」，有的页缺的是「哪一段渲染」，
    // 两者混用一句就会变成套话。
    let note = page_note(page);
    // 视觉基线 `17-widgets.png`：点组头 Widgets 右侧是真的一片空白，连标题都没有。
    if spec.blocks.is_empty() {
        return;
    }
    ui.add_space(6.0);
    ui.label(egui::RichText::new(spec.title).color(egui::Color32::from_rgb(0xBB, 0xBB, 0xBB)));
    ui.add_space(8.0);
    ui.spacing_mut().item_spacing.y = 6.0;
    for block in spec.blocks {
        match block {
            Block::Group { title, rows } => {
                group_box(ui, |ui| {
                    ui.label(egui::RichText::new(*title).strong());
                    ui.add_space(11.0);
                    for row in *rows {
                        draw_row(ui, row, note);
                    }
                });
                ui.add_space(6.0);
            }
            Block::Rows(rows) => {
                for row in *rows {
                    draw_row(ui, row, note);
                }
                ui.add_space(6.0);
            }
            Block::Shortcuts => shortcuts_table(ui, overrides),
        }
    }
    // 说明文字不画在页底：视觉基线的 19 张设置截图里没有任何页脚小字，
    // 灰显理由一律挂在控件的 hover 上（同一份 `note`），既守住 I5 又保持 1:1。
    let _ = note;
}

/// 圆角描边分组框：视觉基线每一页都是这个外观。
pub(in crate::ui) fn group_box(ui: &mut egui::Ui, add: impl FnOnce(&mut egui::Ui)) {
    // 视觉基线的组标题压在框上边线上方：框线在标题下方、首行控件上方。做法是不动
    // 30 个调用点，画完 Frame 后抹掉原上边线，再按视觉基线在 `top + 30` 补一条。
    // 逐像素比对时线在 top+24 太贴标题、top+27 又比视觉基线高 6 物理像素；配合
    // Application 页标题后 16pt 的留白，+30 才对齐「标题→线→首行」三段。
    // 描边色取得比参考略亮：灰显页里 egui 会把 stroke 朝面板底色混一档，
    // 提亮补偿后可见度才和参考的清晰矩形框一致。
    let frame_stroke = egui::Stroke::new(1.0, egui::Color32::from_rgb(0x5A, 0x5A, 0x5A));
    let response = egui::Frame::none()
        .stroke(frame_stroke)
        .rounding(6.0)
        .inner_margin(egui::Margin { left: 6.0, right: 6.0, top: 6.0, bottom: 6.0 })
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            add(ui);
        })
        .response;
    let r = response.rect;
    let p = ui.painter_at(r.expand(2.0));
    let bg = crate::ui::harness::canvas_bg();
    p.rect_filled(
        egui::Rect::from_min_max(
            egui::pos2(r.left() - 2.0, r.top() - 2.0),
            egui::pos2(r.right() + 2.0, r.top() + 4.0),
        ),
        egui::Rounding::ZERO,
        bg,
    );
    p.line_segment(
        [
            egui::pos2(r.left() + 3.0, r.top() + 30.0),
            egui::pos2(r.right() - 3.0, r.top() + 30.0),
        ],
        frame_stroke,
    );
}

/// 对话框顶部的方标签（参考实现 OneKeys / Triggers 顶上那排）。
///
/// 与 `log_settings::protocol_tab` 同形，区别只有「高度由调用方给」：那两个
/// 对话框的标签条实测高 23 逻辑点（`1408×1314` @2× 的 y 2..48）。
///
/// 两档底色按参考实现：**选中的比未选的更暗**（`#1C1C1C` vs `#333333`），看起来
/// 像选中的那块与下方内容区连成一片。文字色反过来——选中更亮。
///
/// 访问键下划线画在**第一个字符**下：WindTerm 的对话框标签都用首字母作访问键
/// （`SSH` 的 S、`Account` 的 A、`Highlight Text` 的 H），视觉基线每块下方都有。
pub(in crate::ui) fn dialog_tab(
    ui: &mut egui::Ui,
    width: f32,
    height: f32,
    label: &str,
    active: bool,
) -> egui::Response {
    let (rect, resp) = ui.allocate_exact_size(egui::vec2(width, height), egui::Sense::click());
    let p = ui.painter_at(rect);
    let (bg, fg) = if active {
        (0x1C1C1C, 0xE6E6E6)
    } else {
        (0x333333, 0xBFBFBF)
    };
    p.rect_filled(rect, egui::Rounding::ZERO, egui::Color32::from_rgb(
        (bg >> 16) as u8,
        ((bg >> 8) & 0xFF) as u8,
        (bg & 0xFF) as u8,
    ));
    let color = egui::Color32::from_rgb(
        (fg >> 16) as u8,
        ((fg >> 8) & 0xFF) as u8,
        (fg & 0xFF) as u8,
    );
    let font = egui::TextStyle::Button.resolve(ui.style());
    let galley = p.layout_no_wrap(label.to_owned(), font.clone(), color);
    let w = galley.size().x;
    p.galley(rect.center() - galley.size() / 2.0, galley, color);
    // 首字符下划线：等宽字体下 cw = 总宽 / 字符数。
    let cw = w / label.chars().count().max(1) as f32;
    let x0 = rect.center().x - w / 2.0;
    let y = rect.center().y + ui.style().text_styles[&egui::TextStyle::Button].size * 0.62;
    p.line_segment([egui::pos2(x0, y), egui::pos2(x0 + cw, y)], egui::Stroke::new(1.0, color));
    resp
}

/// 画一行。`settings_live_text` 也用它画「没有消费者」的那几行——同一页里
/// 真控件与灰行必须一眼能分开，而「灰行长什么样」只有这一份实现。
pub(in crate::ui) fn draw_row(ui: &mut egui::Ui, row: &Row, note: &str) {
    match row {
        Row::Label(text) => {
            ui.label(*text);
        }
        Row::Check(text, on) => {
            let mut checked = *on;
            ui.add_enabled(false, egui::Checkbox::new(&mut checked, *text))
                .on_disabled_hover_text(note);
        }
        Row::DisabledCheck(text) => {
            // W2：形状照旧（灰复选框），但绝不打勾——勾代表「已生效」，
            // 而这一项既没落盘字段也没有读者。悬停给出与页底同一句理由。
            let mut checked = false;
            ui.add_enabled(false, egui::Checkbox::new(&mut checked, *text))
                .on_disabled_hover_text(note);
        }
        Row::Radios { label, items, selected } => {
            if !label.is_empty() {
                ui.label(*label);
            }
            for (i, name) in items.iter().enumerate() {
                ui.add_enabled(false, egui::RadioButton::new(i == *selected, *name));
            }
        }
        Row::Combo { label, value } => {
            ui.horizontal(|ui| {
                if !label.is_empty() {
                    right_label(ui, label);
                }
                ui.add_enabled_ui(false, |ui| {
                    // 230 是视觉基线的组合框宽度，但**不能闭眼用**：右侧栏只有约 330，
                    // 140 标签 + 230 组合＝370，一上来就超出 30——egui 会把窗口顶宽，
                    // 而窗口尺寸是钉死的，最后裁掉的是页脚与 `Close`。
                    let width = ui.available_width();
                    // W2：只读 Combo 不能再让人以为「点一下就能改」。
                    // 形状不变，但悬停明确给出「尚未落盘、故意灰显」的理由，
                    // 与页底那行说明同一份文案，避免两处说法打架。
                    egui::ComboBox::from_id_salt(*label)
                        .selected_text(*value)
                        .width(width)
                        // `width` 只是下限：默认 TextWrapMode::Extend 会按选中文字
                        // 的真实长度再撑宽，长文案把钉死的窗口顶宽后页脚 `Close`
                        // 就被裁掉（Tabs 页的「Activate the previously selected tab」）。
                        // Truncate 让选中的长文字在框内省略，宽度真正封顶。
                        .wrap_mode(egui::TextWrapMode::Truncate)
                        .show_ui(ui, |_ui| {})
                        .response
                        .on_disabled_hover_text(note);
                });
            });
        }
        Row::ComboAbove { label, value } => {
            // 视觉基线：标签独占一行（左对齐、首字符带访问下划线），控件在下一行
            // 占满整个右栏宽。这跟 Row::Combo 的「右对齐标签列 + 右侧框」是两套画法，
            // 不能共用 right_label——那会把长标签塞进 110 宽的格子截断成 "Start compl…"。
            underlined_label(ui, label);
            ui.add_enabled_ui(false, |ui| {
                egui::ComboBox::from_id_salt(*label)
                    .selected_text(*value)
                    .width(ui.available_width())
                    .wrap_mode(egui::TextWrapMode::Truncate)
                    .show_ui(ui, |_ui| {})
                    .response
                    .on_disabled_hover_text(note);
            });
        }
        Row::SpinAbove { label, value } => {
            underlined_label(ui, label);
            let mut shown = (*value).to_string();
            let button_w = 44.0;
            let field_w = (ui.available_width() - button_w - ui.spacing().item_spacing.x).max(1.0);
            ui.add_enabled(
                false,
                egui::TextEdit::singleline(&mut shown).desired_width(field_w),
            );
        }
        Row::Spin { label, value } => {
            ui.horizontal(|ui| {
                right_label(ui, label);
                let mut shown = value.to_string();
                // 两个控件同在一行：TextEdit 不能吃掉全部余量，否则后面的
                // ▲▼ 按钮再占一份，整行比右栏宽，把钉死的对话框顶大。
                let button_w = 44.0;
                let field_w = (ui.available_width() - button_w - ui.spacing().item_spacing.x).max(1.0);
                ui.add_enabled(
                    false,
                    egui::TextEdit::singleline(&mut shown).desired_width(field_w),
                );
                ui.add_enabled(false, egui::Button::new("▲▼"));
            });
        }
        Row::Text { label, hint } => {
            ui.horizontal(|ui| {
                right_label(ui, label);
                let mut empty = String::new();
                ui.add_enabled(
                    false,
                    egui::TextEdit::singleline(&mut empty)
                        .hint_text(*hint)
                        .desired_width(ui.available_width()),
                );
            });
        }
    }
}

/// 标签列同样右对齐到一条竖线（视觉基线的两列栅格）。
pub(in crate::ui) fn right_label(ui: &mut egui::Ui, text: &str) {
    // 固定 140 宽的标签列。**必须 truncate**：`ui.label` 的 galley 不受分配宽度约束，
    // 长文案会把 `allocate_ui_with_layout` 撑大，进而顶宽整个右侧栏——窗口是钉死尺寸的
    // （`StableWindow`），于是最后被裁掉的是页脚与 `Close` 按钮（2026-11-09 实测）。
    ui.allocate_ui_with_layout(
        egui::vec2(LABEL_W, 20.0),
        egui::Layout::right_to_left(egui::Align::Center),
        |ui| {
            // 跟随设置页全局字号（12 号窄体），不在这里硬编码字体——
            // 之前写死 proportional(14) 会盖掉全局设置，长标签因此被截断。
            ui.add(egui::Label::new(text).truncate());
        },
    );
}

/// 左对齐标签，首字符带访问下划线（视觉基线 "Lock screen timeout:" 的 L 等）。
/// 灰显行的标签用普通文字色下划线即可——访问键是视觉线索，不假装有焦点。
fn underlined_label(ui: &mut egui::Ui, text: &str) {
    let mut job = egui::text::LayoutJob::default();
    let font = egui::TextStyle::Body.resolve(ui.style());
    let color = ui.visuals().text_color();
    let acc = text.chars().next().map(|c| c.len_utf8()).unwrap_or(0);
    let mut underlined = egui::TextFormat::simple(font.clone(), color);
    underlined.underline = egui::Stroke::new(1.0, color);
    job.append(&text[..acc], 0.0, underlined);
    job.append(&text[acc..], 0.0, egui::TextFormat::simple(font, color));
    ui.label(job);
}

/// 标签列宽（视觉基线 ≈140）。
const LABEL_W: f32 = 175.0;

/// `Shortcut Keys` 页的表：`DEFAULT_SHORTCUTS` 的每一对（命令, 生效和弦），只读。
///
/// 这里曾经还有一张 `Search` 页的两列表（6 个搜索引擎 + `+ − ▲ ▼ / Modify`
/// 工具条）。Search 改成手绘的 live 页后那张表零使用者，一并删掉：留着它就是
/// 第二套「引擎长什么样」的说法，改一边不会提醒另一边。
/// 列宽收窄到 150 + 150：这页只有「命令 / 和弦」两列。
fn shortcuts_table(ui: &mut egui::Ui, overrides: &[(usize, String)]) {
    let rows = effective_chords(overrides);
    group_box(ui, |ui| {
        ui.spacing_mut().item_spacing.y = 2.0;
        ui.horizontal(|ui| {
            ui.add_sized(egui::vec2(150.0, 18.0), egui::Label::new("Command"));
            ui.add_sized(egui::vec2(150.0, 18.0), egui::Label::new("Shortcut"));
        });
        for (label, chord) in &rows {
            ui.horizontal(|ui| {
                // 10 行都是出厂绑定名，150 够长；`truncate` 是防手改过的超长
                // 覆盖把右栏顶宽——窗口尺寸钉死，最后被裁掉的是页脚与 `Close`。
                ui.add_sized(egui::vec2(150.0, 18.0), egui::Label::new(*label).truncate());
                ui.add_sized(
                    egui::vec2(150.0, 18.0),
                    egui::Label::new(chord.as_str()).truncate(),
                );
            });
        }
    });
    ui.add_space(4.0);
}


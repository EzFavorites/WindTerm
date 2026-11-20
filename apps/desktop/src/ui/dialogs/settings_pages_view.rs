//! `settings_pages` 的**画法**：分组框、两列栅格、灰显控件、只读快捷键表。
//!
//! 与数据分开是为了 `ui/` 下非 `mod.rs` 文件 ≤ 450 行：十六页的规格表
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
    // 视觉基线 `视觉基线`：点组头 Widgets 右侧是真的一片空白，连标题都没有。
    if spec.blocks.is_empty() {
        return;
    }
    // 标签列宽按**本页**最长的标签量出来，全页共用一列（用户 的
    // 口径：「文字左对齐，同一个页面保证输入框宽度一致」）。参考布局是 网格布局，
    // 所以 Wrap 页的列窄、Command History 页的列宽；写死一个常数会让窄标签的
    // 页在标签与控件之间多出一大段空白。
    let col = page_label_col(ui, spec.blocks);
    ui.add_space(6.0);
    // `spec.title` 只在**没有分组框**的页上画（Proxy：标题 + 一个通栏组合框，
    // 见 `视觉基线`）。有分组框的页，第一行字就是那个框的标题——
    // 再画一行 `spec.title` 会变成「Mouse Tracking / Tracking Events」两级标题，
    // 而视觉基线 `视觉基线` 只有后者。
    if matches!(spec.blocks.first(), Some(Block::Rows(_))) {
        ui.label(egui::RichText::new(spec.title).color(egui::Color32::from_rgb(0xBB, 0xBB, 0xBB)));
        ui.add_space(8.0);
    }
    ui.spacing_mut().item_spacing.y = 6.0;
    for block in spec.blocks {
        match block {
            Block::Group { title, rows } => {
                group_box(ui, Some(title), |ui| {
                    for row in *rows {
                        draw_row(ui, row, note, col);
                    }
                });
                ui.add_space(6.0);
            }
            Block::Rows(rows) => {
                for row in *rows {
                    draw_row(ui, row, note, col);
                }
                ui.add_space(6.0);
            }
            Block::Shortcuts => shortcuts_table(ui, overrides),
        }
    }
    // 说明文字不画在页底：视觉基线的 19 张设置截图里没有任何页脚小字，
    // 灰显理由一律挂在控件的 hover 上（同一份 `note`），既守住规则又保持 1:1。
    let _ = note;
}

/// 分组框：标题画在框**外**上方，框是闭合的四边、2pt 小圆角（参考布局 分组框 的样子）。
///
/// 老实现画完 Frame 后把上边线抹掉、再在 `top + 30` 补一条，想做出「标题压在上边
/// 线上」的效果——但抹的范围是整条 `top-2 ..= top+4`，**连左右两个上圆角一起抹掉了**，
/// 于是整片页面看起来「只有两条竖线、没有顶框」（用户 反馈）。现在标题
/// 由调用方传进来、在框外画，框本身就是一个完整的矩形，不需要任何涂抹。
///
/// 描边色 #2E2E2E 是视觉基线实测值（`视觉基线` 框线物理像素 46，画布 28）；
/// 圆角 2pt 也是实测——原来的 6pt 明显比视觉基线圆（用户说「圆角偏大」）。
pub(in crate::ui) fn group_box(
    ui: &mut egui::Ui,
    title: Option<&str>,
    add: impl FnOnce(&mut egui::Ui),
) {
    if let Some(t) = title {
        // 组标题是 #BBBBBB、常规字重：视觉基线 19 张设置页里所有组标题的像素亮度
        // 都是 187（`视觉基线` 的 `Wrap`、`09-…` 的 `Capacity`、
        // `视觉基线` 的 `Application` 全一致），既不更亮也不加粗。
        // 之前这里用 `.strong()`（更亮 + 加粗），与同一页页级灰标题是两套样式。
        ui.label(egui::RichText::new(t).color(egui::Color32::from_rgb(0xBB, 0xBB, 0xBB)));
        // 标题行底 → 框上边线：视觉基线实测 7 物理像素（3.5 逻辑）。
        ui.add_space(TITLE_GAP);
    }
    // 右栏在一个 `ScrollArea` 里，它的**裁剪区**比可用宽度窄约 3 逻辑点（滚动条
    // 那条带子）。框若占满可用宽度，右边框就落在裁剪区外——整条消失。这里先量出
    // 这段「看不见的差」，把框宽度收回来，四边内边距因此左右对称（都是 10）。
    let slack = (ui.cursor().left() + ui.available_width() - ui.clip_rect().right()).max(0.0) + 1.0;
    let inner_w = (ui.available_width() - slack - 2.0 * GROUP_PAD).max(40.0);
    let frame = egui::Frame::none()
        .inner_margin(egui::Margin {
            left: GROUP_PAD,
            right: GROUP_PAD,
            top: GROUP_PAD,
            bottom: GROUP_PAD,
        })
        .show(ui, |ui| {
            ui.set_width(inner_w);
            add(ui);
        });
    // 描边**画在矩形内侧**，而且右缘要收进裁剪区，不能交给 `Frame::stroke`。
    //
    // 两层原因，都会让右边框整条消失（用户 两次反馈「右侧框线又没有了」）：
    //   1. `Frame` 的 stroke 以边线为中心、各外扩半个宽度，落在裁剪边上的那半条
    //      直接被裁掉；
    //   2. 右栏在 `ScrollArea` 里，滚动区的可用宽度比它的**裁剪区**宽约 4 逻辑点
    //      （滚动条用的那条带子），而右栏内容占满了可用宽度 ⇒ 框右缘落在裁剪区外。
    // 所以取「矩形的右缘」与「裁剪区右缘」的较小值，再往里收半个描边，四条边都画
    // 在可见区内。几何尺寸一点不动：字段本身还比框右缘内缩 10 点，不受影响。
    let r = frame.response.rect;
    // 右缘最多到裁剪区里 1 点处，否则描边有一半落在裁剪区外（见上面第 2 条）。
    let right = r.right().min(ui.clip_rect().right()) - 1.0;
    let painted = egui::Rect::from_min_max(
        egui::pos2(r.left() + 0.5, r.top() + 0.5),
        egui::pos2(right, r.bottom() - 0.5),
    );
    // 用父 `painter`（裁剪区就是右栏的裁剪区），不要 `painter_at(painted)`：
    // 后者把裁剪区收成描边矩形本身，边线正好压在裁剪边界上会被整条剔掉
    // （实测：四条边一起消失）。
    ui.painter().rect_stroke(
        painted,
        egui::Rounding::same(GROUP_ROUNDING),
        egui::Stroke::new(1.0, GROUP_STROKE),
    );
}

/// 分组框描边色（视觉基线实测 #2E2E2E）。
const GROUP_STROKE: egui::Color32 = egui::Color32::from_rgb(0x2E, 0x2E, 0x2E);
/// 分组框圆角（视觉基线实测 ≈2）。
const GROUP_ROUNDING: f32 = 2.0;
/// 分组框内边距：视觉基线里标签文字左缘距框线 10（`454+2 → 476` 物理）。
const GROUP_PAD: f32 = 10.0;
/// 组标题与框上边线的间隙。
const TITLE_GAP: f32 = 2.0;

/// 对话框顶部的方标签（参考布局 OneKeys / Triggers 顶上那排）。
///
/// 与 `log_settings::protocol_tab` 同形，区别只有「高度由调用方给」：那两个
/// 对话框的标签条实测高 23 逻辑点（`1408×1314` @2× 的 y 2..48）。
///
/// 两档底色按参考布局：**选中的比未选的更暗**（`#1C1C1C` vs `#333333`），看起来
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
pub(in crate::ui) fn draw_row(ui: &mut egui::Ui, row: &Row, note: &str, col: f32) {
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
            // 形状照旧（灰复选框），但绝不打勾——勾代表「已生效」，
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
                    field_label(ui, label, col);
                }
                ui.add_enabled_ui(false, |ui| {
                    // 控件宽度＝标签列之后的全部余量（同一页所有行右缘齐在框右边）。
                    // **不能闭眼用视觉基线的 230**：egui 会把窗口顶宽，而窗口尺寸是
                    // 钉死的，最后裁掉的是页脚与 `Close`。
                    let width = ui.available_width();
                    // 只读 Combo 不能再让人以为「点一下就能改」。
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
            // 占满整个右栏宽。这跟 `Row::Combo` 的「标签列 + 右侧框」是两套画法，
            // 不能共用 `field_label`——那会把长标签塞进短格子里截断成 "Start compl…"。
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
                field_label(ui, label, col);
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
                field_label(ui, label, col);
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

/// 标签列：**左对齐**、列宽由本页最长标签决定，控件紧接其后占满余下宽度。
///
/// 老实现是「175 宽右对齐标签列」：标签挤到列的右端、冒号对齐，但左边空出一大块，
/// 而控件又只有半个右栏宽——这就是用户说的「输入框和文字需要占满一行，不允许留
/// 大段空白」。参考布局是 网格布局：标签贴框左内边距起排，控件从「本页最长标签
/// + 间距」处开始，右缘一路齐到框右边。
///
/// **必须 truncate**：`ui.label` 的 galley 不受分配宽度约束，长文案会把
/// `allocate_ui_with_layout` 撑大，进而顶宽整个右侧栏——窗口是钉死尺寸的
/// （`StableWindow`），于是最后被裁掉的是页脚与 `Close` 按钮（实测）。
///
/// **必须 `set_min_width`**：`allocate_ui_with_layout` 分配的宽度是子 ui 的
/// `min_rect`（内容尺寸），不是传进去的 `col`。标签比 `col` 窄时子 ui 就只占
/// 文字那么宽，控件左缘跟着提前——同页几行于是各不相同（实测：
/// `Days to keep history:` 那行字段左缘 718.5、`History size:` 那行 662.5，
/// 相差 56 点）。把这个格子撑满 `col`，控件左缘才会落在同一条竖线上。
pub(in crate::ui) fn field_label(ui: &mut egui::Ui, text: &str, col: f32) {
    ui.allocate_ui_with_layout(
        egui::vec2(col, 20.0),
        egui::Layout::left_to_right(egui::Align::Center),
        |ui| {
            ui.set_min_width(col);
            // 跟随设置页全局字号（12 号窄体），不在这里硬编码字体——
            // 之前写死 proportional(14) 会盖掉全局设置，长标签因此被截断。
            ui.add(egui::Label::new(text).truncate());
        },
    );
}

/// 一行里标签之后的**字段框**：套一层描边、占满剩余宽度。
///
/// 视觉基线每个字段（含只有一两个数字的 Spin）都是「标签列之后整段宽」的方框，
/// 数字靠左、▲▼ 贴右缘。`ui.add(DragValue)` 裸放只有几十像素宽，右边于是空出
/// 一大段——用户报的「留大段空白」主要就是这一类。
pub(in crate::ui) fn field_box(ui: &mut egui::Ui, add: impl FnOnce(&mut egui::Ui)) {
    let visuals = ui.visuals().clone();
    egui::Frame::none()
        .fill(visuals.extreme_bg_color)
        .stroke(visuals.widgets.inactive.bg_stroke)
        .rounding(visuals.widgets.inactive.rounding)
        .inner_margin(egui::Margin::symmetric(6.0, 3.0))
        .show(ui, |ui| {
            // 框里的控件不能再画一层自己的框：`DragValue` 会给自己那个小矩形描边，
            // 通栏字段里就出现「框中框」。把常态底色与描边清空，只留外面这一层。
            let v = ui.visuals_mut();
            v.widgets.inactive.weak_bg_fill = egui::Color32::TRANSPARENT;
            v.widgets.inactive.bg_stroke = egui::Stroke::NONE;
            ui.set_width(ui.available_width());
            add(ui);
        });
}

/// 量出本页标签列宽＝**本页最长标签**的实测宽度 + [`LABEL_GAP`]。
///
/// 传空列表时给一个兜底值（页面自己没声明标签集时用它，不会把控件挤成一条缝）。
pub(in crate::ui) fn label_col(ui: &egui::Ui, labels: &[&str]) -> f32 {
    let font = egui::TextStyle::Body.resolve(ui.style());
    let widest = labels
        .iter()
        .map(|t| {
            ui.fonts(|f| {
                f.layout_no_wrap((*t).to_owned(), font.clone(), egui::Color32::WHITE)
                    .size()
                    .x
            })
        })
        .fold(0.0_f32, f32::max);
    if widest <= 0.0 {
        LABEL_FALLBACK
    } else {
        widest + LABEL_GAP
    }
}

/// 标签文字右缘 → 控件左缘（视觉基线实测 9 逻辑点：`773 → 790` 物理）。
const LABEL_GAP: f32 = 9.0;
/// 页面没声明标签集时的标签列宽（够放 `Font family 1:` 这一档）。
const LABEL_FALLBACK: f32 = 100.0;

/// 一页所有分组框里出现过的标签（`Row::Label` 之类整宽行不算）。
fn page_label_col(ui: &egui::Ui, blocks: &[Block]) -> f32 {
    let mut labels: Vec<&str> = Vec::new();
    let mut push = |rows: &'static [Row]| {
        for row in rows {
            match row {
                Row::Combo { label, .. } | Row::Spin { label, .. } | Row::Text { label, .. }
                    if !label.is_empty() =>
                {
                    labels.push(label);
                }
                _ => {}
            }
        }
    };
    for block in blocks {
        match block {
            Block::Group { rows, .. } => push(rows),
            Block::Rows(rows) => push(rows),
            Block::Shortcuts => {}
        }
    }
    label_col(ui, &labels)
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

/// `Shortcut Keys` 页的表：`DEFAULT_SHORTCUTS` 的每一对（命令, 生效和弦），只读。
///
/// 这里曾经还有一张 `Search` 页的两列表（6 个搜索引擎 + `+ − ▲ ▼ / Modify`
/// 工具条）。Search 改成手绘的 live 页后那张表零使用者，一并删掉：留着它就是
/// 第二套「引擎长什么样」的说法，改一边不会提醒另一边。
/// 列宽收窄到 150 + 150：这页只有「命令 / 和弦」两列。
fn shortcuts_table(ui: &mut egui::Ui, overrides: &[(usize, String)]) {
    let rows = effective_chords(overrides);
    group_box(ui, None, |ui| {
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


//! `settings_pages` 的**画法**：分组框、两列栅格、灰显控件、只读快捷键表。
//!
//! 与数据分开是为了 （`ui/` 下非 `mod.rs` 文件 ≤ 450 行）：十六页的规格表
//! 自己就三百多行，画法再挤进去就超了。两边按「数据 / 画法」切，
//! 加一页只动数据文件，改外观只动这个文件。

use super::settings_pages::{effective_chords, page_note, page_spec, Block, Row};
use super::settings::SettingsPage;
use crate::ui::*;
use crate::ui::kit::field::{choice_row_resp, ChoiceKind, UnderlineStyle};

// 组框的节奏常量现在只有一份出处（`kit::spec`，金标准 @2x 实测）。
// 这里保留名字给同文件内的调用点，值与注释同步，不再各写各的。
pub(in crate::ui) use crate::ui::kit::spec::{BLOCK_GAP, GROUP_PAD, TITLE_GAP};

// 表单控件本体在 `ui/kit/form.rs`——设置页与两个会话弹窗共用同一份。这里只做
// **路径转发**，不重新定义任何东西：`settings_pages_view::field_row` 与
// `crate::ui::kit::form::field_row` 是同一个函数、同一份间距常量。
pub(in crate::ui) use crate::ui::kit::form::{
    dead_value, field_height, field_label, field_row, label_col, read_only, set_row_rhythm,
    spin_field_delta, Deco, Slot, FIELD_TO_ROW,
};

pub(in crate::ui) fn draw(ui: &mut egui::Ui, page: SettingsPage, overrides: &[(usize, String)]) {
    let Some(spec) = page_spec(page) else { return };
    // 说明文字跟着页走：有的页缺的是「哪个字段」，有的页缺的是「哪一段渲染」，
    // 两者混用一句就会变成套话。
    let note = page_note(page);
    // 视觉基线 `17-widgets.png`：点组头 Widgets 右侧是真的一片空白，连标题都没有。
    if spec.blocks.is_empty() {
        return;
    }
    // 标签列宽按**本页**最长的标签量出来，全页共用一列（用户 的
    // 口径：「文字左对齐，同一个页面保证输入框宽度一致」）。兼容终端是 QGridLayout，
    // 所以 Wrap 页的列窄、Command History 页的列宽；写死一个常数会让窄标签的
    // 页在标签与控件之间多出一大段空白。
    let col = page_label_col(ui, spec.blocks);
    ui.add_space(6.0);
    // `spec.title` 只在**没有分组框**的页上画（Proxy：标题 + 一个通栏组合框，
    // 见 `03-proxy.png`）。有分组框的页，第一行字就是那个框的标题——
    // 再画一行 `spec.title` 会变成「Mouse Tracking / Tracking Events」两级标题，
    // 而视觉基线 `10-terminal-mouse-tracking.png` 只有后者。
    if matches!(spec.blocks.first(), Some(Block::Rows(_))) {
        ui.label(egui::RichText::new(spec.title).color(egui::Color32::from_rgb(0xBB, 0xBB, 0xBB)));
        ui.add_space(8.0);
    }
    ui.spacing_mut().item_spacing.y = FIELD_TO_ROW;
    for block in spec.blocks {
        match block {
            Block::Group { title, rows } => {
                group_box(ui, Some(title), |ui| {
                    for row in *rows {
                        draw_row(ui, row, note, col);
                    }
                });
                // 组框之间：item_spacing(FIELD_TO_ROW) + BLOCK_GAP = 11.5pt，
                // 与金标准 07-terminal.png 的 23 物理像素一致。
                ui.add_space(BLOCK_GAP);
            }
            Block::Rows(rows) => {
                for row in *rows {
                    draw_row(ui, row, note, col);
                }
                ui.add_space(BLOCK_GAP);
            }
            Block::Shortcuts => shortcuts_table(ui, overrides),
        }
    }
    // 说明文字不画在页底：视觉基线的 19 张设置截图里没有任何页脚小字，
    // 灰显理由一律挂在控件的 hover 上（同一份 `note`），既守住  又保持 1:1。
    let _ = note;
}

/// 分组框：标题画在框**外**上方，框是闭合的四边、2pt 小圆角（兼容终端 QGroupBox 的样子）。
///
/// 老实现画完 Frame 后把上边线抹掉、再在 `top + 30` 补一条，想做出「标题压在上边
/// 线上」的效果——但抹的范围是整条 `top-2 ..= top+4`，**连左右两个上圆角一起抹掉了**，
/// 于是整片页面看起来「只有两条竖线、没有顶框」（用户 反馈）。现在标题
/// 由调用方传进来、在框外画，框本身就是一个完整的矩形，不需要任何涂抹。
///
/// 描边色 #2E2E2E 是视觉基线实测值（`16-text-wrap.png` 框线物理像素 46，画布 28）；
/// 圆角 2pt 也是实测——原来的 6pt 明显比视觉基线圆（用户说「圆角偏大」）。
pub(in crate::ui) fn group_box(
    ui: &mut egui::Ui,
    title: Option<&str>,
    add: impl FnOnce(&mut egui::Ui),
) {
    if let Some(t) = title {
        // 组标题是 #BBBBBB、常规字重：视觉基线 19 张设置页里所有组标题的像素亮度
        // 都是 187（`16-text-wrap.png` 的 `Wrap`、`09-…` 的 `Capacity`、
        // `02-appearance.png` 的 `Application` 全一致），既不更亮也不加粗。
        // 之前这里用 `.strong `（更亮 + 加粗），与同一页页级灰标题是两套样式。
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
            // `set_width` 只抬**下限**，`available_width ` 仍可能拿到父给的更宽值，
            // 于是字段框按那个更宽的值铺满、右缘越过内边距，左右留白变成 2 : 12
            // （实测）。上下限都钉到 `inner_w`，左右才真正对称。
            ui.set_max_width(inner_w);
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

/// 对话框顶部的方标签（OneKeys / Triggers / Modem 共用）。
///
/// 画法已收进 [`crate::ui::kit::tabs::tab`]：两档底色（选中 #1C1C1C，
/// 与下方内容区连成一片 / 未选 #333333）、首字符访问键下划线都只有一处实现。
/// 这里保留旧名字给现有调用点，不再是一份独立绘制代码。
pub(in crate::ui) use crate::ui::kit::tabs::tab as dialog_tab;


/// 画一行。`settings_live_text` 也用它画「没有消费者」的那几行——同一页里
/// 真控件与灰行必须一眼能分开，而「灰行长什么样」只有这一份实现。
pub(in crate::ui) fn draw_row(ui: &mut egui::Ui, row: &Row, note: &str, col: f32) {
    match row {
        Row::Label(text) => {
            ui.label(*text);
        }
        Row::Check(text, on) => {
            let mut checked = *on;
            choice_row_resp(ui, ChoiceKind::Checkbox, text, "", &mut checked, false, UnderlineStyle::None)
                .on_hover_text(note);
        }
        Row::DisabledCheck(text) => {
            // 形状照旧（灰复选框），但绝不打勾——勾代表「已生效」，
            // 而这一项既没落盘字段也没有读者。悬停给出与页底同一句理由。
            let mut checked = false;
            choice_row_resp(ui, ChoiceKind::Checkbox, text, "", &mut checked, false, UnderlineStyle::None)
                .on_hover_text(note);
        }
        Row::Radios { label, items, selected } => {
            if !label.is_empty() {
                ui.label(*label);
            }
            for (i, name) in items.iter().enumerate() {
                let mut on = i == *selected;
                choice_row_resp(ui, ChoiceKind::Radio, name, "", &mut on, false, UnderlineStyle::None);
            }
        }
        Row::Combo { label, value } => {
            field_row(ui, label, Slot::Inline, col, note, |ui| {
                dead_value(ui, value, Deco::Down);
            });
        }
        // 视觉基线：标签独占一行（左对齐、首字符带访问下划线），控件在下一行
        // 占满整个右栏宽。这跟 `Row::Combo` 的「标签列 + 右侧框」是两套排布，
        // 不能共用 `field_label`——那会把长标签塞进短格子里截断成 "Start compl…"。
        Row::ComboAbove { label, value } => {
            field_row(ui, label, Slot::Above, col, note, |ui| {
                dead_value(ui, value, Deco::Down);
            });
        }
        Row::SpinAbove { label, value } => {
            field_row(ui, label, Slot::Above, col, note, |ui| {
                dead_value(ui, value, Deco::Spin);
            });
        }
        Row::Spin { label, value } => {
            field_row(ui, label, Slot::Inline, col, note, |ui| {
                dead_value(ui, value, Deco::Spin);
            });
        }
        Row::Text { label, hint } => {
            field_row(ui, label, Slot::Inline, col, note, |ui| {
                read_only(
                    ui,
                    hint,
                    crate::theme::chrome(term_core::theme::CHROME_SCALE.hint),
                    Deco::None,
                );
            });
        }
    }
}


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
        // 两列都按**可用宽度**分：命令名一列吃掉「除和弦列之外的全部」，
        // 和弦列固定 150。原先两列各写死 150，表只占中间的 300，右边空出
        // 一大片（用户 「左右留白」）。
        let gap = ui.spacing().item_spacing.x;
        let chord_w = 150.0;
        let label_w = (ui.available_width() - chord_w - gap).max(120.0);
        // 用 `field_label` 而不是 `add_sized(Label)`：后者把内容在格子**居中**，
        // 于是整张表变成两列居中的字，与仓里其余左对齐的文字对不上。
        ui.horizontal(|ui| {
            field_label(ui, "Command", label_w);
            field_label(ui, "Shortcut", chord_w);
        });
        for (label, chord) in &rows {
            ui.horizontal(|ui| {
                // 10 行都是出厂绑定名；`field_label` 自带 `truncate`，防手改过的
                // 超长覆盖把右栏顶宽——窗口尺寸钉死，最后被裁掉的是页脚 `Close`。
                field_label(ui, label, label_w);
                field_label(ui, chord, chord_w);
            });
        }
    });
    ui.add_space(4.0);
}


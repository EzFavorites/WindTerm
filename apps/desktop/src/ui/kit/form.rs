//! 表单控件的**唯一实现**：设置页、会话设置页（Edit / Default Session）、
//! 新增会话页（New Session）共用这一份。
//!
//! # 为什么单独一个模块
//!
//! 三个弹窗族原先各写一套：设置页有 `field_box` / `field_row`，会话弹窗有
//! `spin_box` / `form_row` / `heading` / `combo_str`。同一件事（「标签 + 字段框」）
//! 在两边用了不同的盒子、不同的行距、不同的标签对齐，用户的原话是
//! 「各不统一」。把二者合并到这里：
//!
//! - **盒子**：[`field_box`] 是全仓唯一的字段面（底、描边、圆角、内边距）；
//!   `TextEdit` / `DragValue` / `ComboBox` 三种控件放进去之后是同一个盒子。
//! - **行**：[`field_row`] 是全仓唯一的「标签 + 盒子」排布，
//!   [`Slot::Above`] / [`Slot::Inline`] 是兼容终端的两种排布，不是两套风格。
//! - **参数**：[`LABEL_TO_FIELD`] 与 [`FIELD_TO_ROW`] 是唯一的两个间距，
//!   [`set_row_rhythm`] 把它们装进 `item_spacing`。
//!
//! 会话弹窗那边的列宽（`FIELD_W` / `FORM_W` / `LABEL_W`）仍留在
//! `session_form_body`：那是那个对话框的**布局**宽度，不是控件风格。
//!
//! # 本层不认识业务
//!
//! 与 `ui/kit` 的其它原语一样，这里只认识 `egui::Ui` 与主题色号：没有
//! `TerminalWindow`、没有 `Prefs`。所以生产路径与组件夹具可以调用同一份。

use crate::ui::*;

/// 一行字段的**排布**。兼容终端两种都在用：长标签独占一行（控件通栏），
/// 短标签与控件同一行。两种排布共用同一组间距，所以同一页里不会出现
/// 「堆叠行挤成 0、行内行却空 4」这种两种节奏并存的样子。
#[derive(Clone, Copy, PartialEq)]
pub(in crate::ui) enum Slot {
    /// 标签独占一行，字段框在下一行占满整行宽。
    Above,
    /// 标签在左（按 `col` 对齐），字段框接在标签之后占满余宽。
    Inline,
}

/// 行距与标签间隙的**唯一出处**是 [`super::spec`]：
/// `LABEL_TO_FIELD = 7`（标签底 → 框顶）、`FIELD_TO_ROW = 9`（框底 → 下一行）。
/// 金标准实测比例是 14px : 18px（@2x），两者必须保持「行距 > 标签间隙」，
/// 否则「标签属于哪一行」在视觉上无法区分，整页糊成等距横条。
pub(in crate::ui) use super::spec::{FIELD_TO_ROW, LABEL_TO_FIELD};

/// 一页的行距从这一处取——`draw` 与所有 live 页共用，改一次全页跟着变。
pub(in crate::ui) fn set_row_rhythm(ui: &mut egui::Ui) {
    ui.spacing_mut().item_spacing.y = FIELD_TO_ROW;
}

/// 画一行「标签 + 字段框」。**全仓唯一的字段行实现**。
///
/// # 为什么必须只有一处
///
/// Tabs 页原先自己抄了 `live_combo` / `dead_combo` / `dead_inline` 三个函数，
/// 与本文件的 `draw_row` 并行存在。两套在三个维度上都不同：
///
/// - **控件**：真行用 `ComboBox`（常态底读 `weak_bg_fill` = 按钮底 #262626），
///   灰行用 `TextEdit`（读 `extreme_bg_color` = 输入框底 #1C1C1C）；
/// - **间距**：堆叠行把 `item_spacing.y` 压成 0，行内行又手补 4；
/// - **标签**：堆叠行用裸 `ui.label`，行内行用带访问键下划线的 `field_label`。
///
/// 于是同一页里一眼看得出「三行买来的、两行自己搭的」——用户的原话是
/// 「保持输入框风格一致」。这里把标签、字段框、间距收成一份，真控件与灰显行
/// 只差「能不能点」。
///
/// # 上下间距为什么不交给 `item_spacing`
///
/// 一行的「标签」与「字段框」是两个 widget，外层 `item_spacing.y` 是**行间距**
/// （[`FIELD_TO_ROW`]）。两者的区别必须分开表达，否则不是行距被压成 0，
/// 就是标签与框之间被撑成一行。所以这里临时把内层间距归零、显式补
/// [`LABEL_TO_FIELD`]，画完还原。
pub(in crate::ui) fn field_row(
    ui: &mut egui::Ui,
    label: &str,
    slot: Slot,
    col: f32,
    note: &str,
    add_field: impl FnOnce(&mut egui::Ui),
) {
    let field = |ui: &mut egui::Ui| {
        let resp = field_box(ui, add_field);
        // 真控件没有「为什么灰着」要解释，别挂一个空盒子。
        if !note.is_empty() {
            resp.on_hover_text(note);
        }
    };
    // **归零只发生在「标签 → 字段框」之间**，不能提前到函数开头：egui 在
    // 分配下一个 widget 时才取当时的 `item_spacing`，提前归零会把「上一个框 →
    // 本行标签」那一段一起吃掉，整页于是退化成等距的窄条（实测：11 点被
    // 吃掉后，框与框之间只剩 15.5 点，标签看起来属于上面那个框）。
    match slot {
        Slot::Above => {
            underlined_label(ui, label);
            // 只补**差额**：egui 在「每个 widget 被分配」的那一刻就按当时的
            // `item_spacing` 推进了光标，所以标签之后已经多出了 `FIELD_TO_ROW`
            // 那一段，这里减去它、只留 `LABEL_TO_FIELD`。
            //
            // 曾经写的是「标签后把 item_spacing 归零、画完还原」——那是错的，
            // 而且错在相反的方向：归零发生在标签**之后**，对标签那一格已经来
            // 不及，反而让下一个数字框少拿了 13 点。插桩实测：
            // 框底到标签 0 点、标签到本行框 19 点（= 6 + 13），每个标签都像
            // 属于上面那个框。
            ui.add_space(LABEL_TO_FIELD - FIELD_TO_ROW);
            field(ui);
        }
        Slot::Inline => {
            // 行内行是一个 `ui.horizontal`，它本身就是单个 widget，
            // 与外层的行距无关，一个字都不用改。
            ui.horizontal(|ui| {
                field_label(ui, label, col);
                field(ui);
            });
        }
    }
}

/// 只读字段右端的装饰。
#[derive(Clone, Copy, PartialEq)]
pub(in crate::ui) enum Deco {
    /// 纯文本字段（无装饰）。
    None,
    /// 下拉框的 `▾`。
    Down,
    /// 数字框的 `▲▼`。
    Spin,
}

/// 数值框：字段框 + 右端 `▲▼`。**全仓唯一的数值框外观**。
///
/// `DragValue` 自己不画上下箭头——真箭头是 egui 在编辑态才有的。视觉基线
/// （Caret / Command History / Binary 各页）每一个数值框右端都有 `▲▼`，
/// 而会话弹窗那一族是手绘的（`session_form_body::spin_box`）。设置页原先两边
/// 都没有，于是「数值框」与「下拉框」只差有没有箭头、与只读行又差得更多。
/// 这里把箭头补在字段框上，真控件与只读值（[`read_only`] 的 `Deco::Spin`）
/// 就共用同一个三角形画法。
pub(in crate::ui) fn spin_field(
    ui: &mut egui::Ui,
    add: impl FnOnce(&mut egui::Ui),
) -> egui::Response {
    spin_field_delta(ui, add).0
}

/// 带**可点击箭头**的数字框：返回 `(字段 Response, 点中的方向)`，
/// 方向是 `+1`（点 `▲`）或 `-1`（点 `▼`），由调用方按自己的数值类型改值。
/// 箭头顺序与命中区只在这里定义（此前顺序画反且只有图形没有交互）。
pub(in crate::ui) fn spin_field_delta(
    ui: &mut egui::Ui,
    add: impl FnOnce(&mut egui::Ui),
) -> (egui::Response, Option<i32>) {
    let resp = field_box(ui, add);
    let cx = resp.rect.right() - SPIN_INSET;
    let cy = resp.rect.center().y;
    let color = crate::theme::chrome(term_core::theme::CHROME_SCALE.icon);
    paint_triangle(ui, cx, cy - 4.0, true, color);
    paint_triangle(ui, cx, cy + 4.0, false, color);

    // 上下各注册一个命中区；用 `ui.interact` 不推进光标，不影响字段布局。
    let half = egui::vec2(7.0, 4.5);
    let up_hit = egui::Rect::from_center_size(egui::pos2(cx, cy - 4.5), half);
    let down_hit = egui::Rect::from_center_size(egui::pos2(cx, cy + 4.5), half);
    let up = ui
        .interact(up_hit, ui.id().with("spin_up"), egui::Sense::click())
        .clicked();
    let down = ui
        .interact(down_hit, ui.id().with("spin_down"), egui::Sense::click())
        .clicked();
    (resp, if up { Some(1) } else if down { Some(-1) } else { None })
}

/// `▲▼` 距字段框右缘的距离（与下拉框 `▾` 的图标位同一档）。
const SPIN_INSET: f32 = 12.0;

/// 单个三角：`up = true` 画朝上的 `▲`，否则画 `▼`。
fn paint_triangle(ui: &egui::Ui, cx: f32, cy: f32, up: bool, color: egui::Color32) {
    let d = 3.0;
    let dir = if up { -1.0 } else { 1.0 };
    ui.painter().add(egui::Shape::convex_polygon(
        vec![
            egui::pos2(cx - d, cy - dir * d / 2.0),
            egui::pos2(cx + d, cy - dir * d / 2.0),
            egui::pos2(cx, cy + dir * d / 2.0),
        ],
        color,
        egui::Stroke::NONE,
    ));
}

/// 只读字段的**值**：文字靠左、装饰贴右缘，不注册任何交互。
///
/// # 为什么不用「灰显的真控件」
///
/// 原先这里写的是 `ui.add_enabled(false, ComboBox/TextEdit)`，而 `add_enabled`
/// 内部会调 `Ui::disable `，它把 painter 的 fade 色调到近黑；文字 `#BBB` 乘
/// 上那档透明度之后落到 `#2F` 上下，在 `#1C1C1C` 的框里等于**一片空白**
/// （实测：整页灰显行只剩几个空盒子）。
///
/// 只读字段本来就不需要交互，也不需要焦点：直接画字最可控——颜色是我们给的，
/// 不会随 egui 的灰显策略变化。这也顺带守住了「灰行永远是空框」那条铁律：
/// 这里不注册任何 widget，就不可能有勾、有焦点、有弹层。
pub(in crate::ui) fn dead_value(ui: &mut egui::Ui, value: &str, deco: Deco) {
    let color = crate::theme::chrome(term_core::theme::CHROME_SCALE.icon);
    read_only(ui, value, color, deco);
}

/// [`dead_value`] 的通用形态：同一套画法，颜色由调用方给（占位文字用更暗的
/// `hint`，与真控件未输入时的样子一致）。
/// 字段框的**高度**：与真控件同一个公式，全仓唯一的出处。
///
/// `ComboBox` 的封闭态高度是 `max(文字行高 + 2×button_padding, interact_size.y)`，
/// 这里照抄。原先这个公式只在 [`read_only`] 里写了一份，其它地方（Wrap 页的
/// `Columns` 文本行、Storage 页的 `Clear` 按钮）各写死 `20.0`——于是同一页里
/// 真字段 22 点、旁边那个 20 点，并排就是两条不一样高的横条。
pub(in crate::ui) fn field_height(ui: &egui::Ui) -> f32 {
    let font = egui::TextStyle::Button.resolve(ui.style());
    let line_h = ui.fonts(|f| f.row_height(&font));
    (line_h + 2.0 * ui.spacing().button_padding.y)
        .max(ui.spacing().interact_size.y)
        .max(FIELD_H)
}

pub(in crate::ui) fn read_only(ui: &mut egui::Ui, value: &str, color: egui::Color32, deco: Deco) {
    let font = egui::TextStyle::Button.resolve(ui.style());
    let height = field_height(ui);
    let (rect, _) = ui.allocate_exact_size(
        egui::vec2(ui.available_width(), height),
        egui::Sense::hover(),
    );
    let p = ui.painter();
    p.text(
        rect.left_center(),
        egui::Align2::LEFT_CENTER,
        value,
        font,
        color,
    );
    // 装饰只占右端一小块：几何与真控件的图标位一致（`icon_width` / `icon_spacing`），
    // 这样真行与灰行的文字起点、箭头位置完全重合，切换时不会跳。
    let icon_w = ui.spacing().icon_width;
    let half = icon_w / 2.0;
    let cx = rect.right() - half;
    let cy = rect.center().y;
    match deco {
        Deco::None => {}
        Deco::Down => {
            let d = half * 0.8;
            p.add(egui::Shape::convex_polygon(
                vec![
                    egui::pos2(cx - d, cy - d / 2.0),
                    egui::pos2(cx + d, cy - d / 2.0),
                    egui::pos2(cx, cy + d / 2.0),
                ],
                color,
                egui::Stroke::NONE,
            ));
        }
        Deco::Spin => {
            paint_triangle(ui, cx, cy - 4.0, true, color);
            paint_triangle(ui, cx, cy + 4.0, false, color);
        }
    }
}

/// 标签列：**左对齐**、列宽由本页最长标签决定，控件紧接其后占满余下宽度。
///
/// 老实现是「175 宽右对齐标签列」：标签挤到列的右端、冒号对齐，但左边空出一大块，
/// 而控件又只有半个右栏宽——这就是用户说的「输入框和文字需要占满一行，不允许留
/// 大段空白」。兼容终端是 QGridLayout：标签贴框左内边距起排，控件从「本页最长标签
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

/// 字段面的三件事：**底色**取控件视觉的输入框底，**描边**与**圆角**在这里定死。
///
/// 这是全仓唯一的字段面定义。它同时被 [`field_box`]（设置页那种「标签 + 通栏
/// 盒子」）与 `kit::window::unify_control_surface`（每个弹窗里由 egui 自己画
/// 框的 `TextEdit` / `ComboBox`）读取——于是「设置页的框」与「会话弹窗的框」
/// 是同一圈线、同一个圆角，而不是两个各自调过的数。
///
/// 描边 #2E2E2E 是视觉基线实测值（`16-text-wrap.png` 框线物理像素 46、画布 28）；
/// 圆角 2pt 同样实测（原先 6pt 明显比视觉基线圆）。
pub(in crate::ui) const FIELD_BORDER: u32 = 0x2E_2E_2E;
pub(in crate::ui) const FIELD_ROUNDING: f32 = 2.0;
/// 字段框左右 / 上下内边距：文字距框线的最小留白。
pub(in crate::ui) const FIELD_PAD_X: f32 = 6.0;
pub(in crate::ui) const FIELD_PAD_Y: f32 = 0.0;

/// 字段的**内容高度**：设置页由 [`field_height`] 按控件公式算出，会话弹窗那边
/// 直接 `add_sized` 用它。两个弹窗族的输入框因此是同一个高度，不再一个 27 一个 23。
pub(in crate::ui) const FIELD_H: f32 = 22.0;

/// [`FIELD_BORDER`] 的那一圈 1pt 描边。
pub(in crate::ui) fn field_stroke() -> egui::Stroke {
    egui::Stroke::new(
        1.0,
        egui::Color32::from_rgb(
            (FIELD_BORDER >> 16) as u8,
            ((FIELD_BORDER >> 8) & 0xFF) as u8,
            (FIELD_BORDER & 0xFF) as u8,
        ),
    )
}

/// 一行里标签之后的**字段框**：套一层描边、占满剩余宽度。
///
/// 视觉基线每个字段（含只有一两个数字的 Spin）都是「标签列之后整段宽」的方框，
/// 数字靠左、▲▼ 贴右缘。`ui.add(DragValue)` 裸放只有几十像素宽，右边于是空出
/// 一大段——用户报的「留大段空白」主要就是这一类。
///
/// **这也是全仓唯一的输入框外观**：`TextEdit`、`Spin`、`ComboBox` 三种控件的
/// 常态底分别读 `extreme_bg_color` 与 `weak_bg_fill`（两个不同的色号），套进
/// 这个框之后它们各自的底色与描边都被清空，留下的只有这一层——于是同一页
/// 里三种控件是同一个盒子，不会再出现「下拉框比文本框亮一档」。
pub(in crate::ui) fn field_box(ui: &mut egui::Ui, add: impl FnOnce(&mut egui::Ui)) -> egui::Response {
    let frame = egui::Frame::none()
        .fill(ui.visuals().extreme_bg_color)
        .stroke(field_stroke())
        .rounding(egui::Rounding::same(FIELD_ROUNDING))
        .inner_margin(egui::Margin::symmetric(FIELD_PAD_X, FIELD_PAD_Y))
        .show(ui, |ui| {
            // 框里的控件不能再画一层自己的框：`DragValue` / `ComboBox` 都会给
            // 自己那个矩形填底描边，通栏字段里就出现「框中框」，而且**填的是
            // 另一个色号**（按钮底 #262626）。四态一起清，悬停时也不会冒出一
            // 块比字段亮的底。
            let v = ui.visuals_mut();
            for w in [
                &mut v.widgets.noninteractive,
                &mut v.widgets.inactive,
                &mut v.widgets.hovered,
                &mut v.widgets.active,
            ] {
                w.weak_bg_fill = egui::Color32::TRANSPARENT;
                w.bg_stroke = egui::Stroke::NONE;
            }
            v.widgets.open.weak_bg_fill = egui::Color32::TRANSPARENT;
            v.widgets.open.bg_stroke = egui::Stroke::NONE;
            ui.set_width(ui.available_width());
            add(ui);
        });
    frame.response
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

/// 左对齐标签，首字符带访问下划线（视觉基线 "Lock screen timeout:" 的 L 等）。
/// 灰显行的标签用普通文字色下划线即可——访问键是视觉线索，不假装有焦点。
pub(in crate::ui) fn underlined_label(ui: &mut egui::Ui, text: &str) {
    let font = egui::TextStyle::Body.resolve(ui.style());
    let color = ui.visuals().text_color();
    let acc = text.chars().next().map(|c| c.len_utf8()).unwrap_or(0);
    let mut underlined = egui::TextFormat::simple(font.clone(), color);
    underlined.underline = egui::Stroke::new(1.0, color);
    let mut job = egui::text::LayoutJob::default();
    job.append(&text[..acc], 0.0, underlined);
    job.append(&text[acc..], 0.0, egui::TextFormat::simple(font, color));
    // 高度**自己定**（`label_h`），不用 `ui.label` 的自动高度。
    let (rect, _) = ui.allocate_exact_size(
        egui::vec2(ui.available_width(), label_h(ui)),
        egui::Sense::hover(),
    );
    job.wrap.max_width = rect.width();
    let galley = ui.fonts(|f| f.layout_job(job));
    let pos = egui::pos2(rect.left(), rect.center().y - galley.size().y / 2.0);
    ui.painter().galley(pos, galley, color);
}

/// 右对齐标签格：文字贴格子右缘，长标签截断成 `…`。
///
/// 会话弹窗（`session_form_body`）与 `Appearance` 页的标签是右对齐的（视觉基线
/// `01-application.png` 的 `* Language:` / `Theme:` 冒号在同一条竖线上），
/// 而设置页正文页是左对齐。两种对齐都是兼容终端，但**格子**必须只有一份实现：
/// 宽度、行高、截断规则同源，才不会一边 20 点高一边 22 点。
pub(in crate::ui) fn label_right(ui: &mut egui::Ui, text: &str, col: f32) {
    let height = ui.fonts(|f| f.row_height(&egui::TextStyle::Body.resolve(ui.style())));
    ui.allocate_ui_with_layout(
        egui::vec2(col, height),
        egui::Layout::right_to_left(egui::Align::Center),
        |ui| {
            ui.set_min_width(col);
            // **必须 truncate**：裸 `ui.label` 的 galley 不受分配宽度约束，
            // 长标签会把 `allocate_ui_with_layout` 撑大，进而顶宽整个右栏
            // （对话框是钉死尺寸的，最后被裁掉的是页脚按钮）。
            ui.add(egui::Label::new(text).truncate());
        },
    );
}

/// 标签行的**实测高度**：只取字体的行高，不含任何额外的行距。
///
/// [`underlined_label`] 早期是裸 `ui.label(job)`，由 egui 自己定高。实测那样
/// 画出来的标签矩形比字形高出十几点，于是「上一个框 → 标签」被吃掉、
/// 「标签 → 本行框」被撑开：整页看起来每个标签都属于**上面**那个框
/// （实测：框底到字形 1 点、字形到本行框 19 点，正好反了）。
/// 自己分配高度之后，两头分别是 [`FIELD_TO_ROW`] 与 [`LABEL_TO_FIELD`]。
fn label_h(ui: &egui::Ui) -> f32 {
    ui.fonts(|f| f.row_height(&egui::TextStyle::Body.resolve(ui.style())))
}


#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn row_rhythm_is_looser_than_the_label_gap() {
        // 反了的话「标签属于下面这个框」与「框属于下面这个标签」在视觉上
        // 无法区分，整页糊成等距横条——这是 修过的那一类 bug。
        assert!(
            FIELD_TO_ROW > LABEL_TO_FIELD,
            "行距必须大于标签到字段的间隙：{} vs {}",
            FIELD_TO_ROW,
            LABEL_TO_FIELD
        );
    }

    #[test]
    fn set_row_rhythm_writes_the_single_row_pitch() {
        egui::__run_test_ui(|ui| {
            ui.spacing_mut().item_spacing.y = 0.0;
            set_row_rhythm(ui);
            assert_eq!(ui.spacing().item_spacing.y, FIELD_TO_ROW);
        });
    }

    #[test]
    fn field_height_matches_the_widget_formula() {
        // 真控件（ComboBox 封闭态 / TextEdit）的高度是
        // `max(行高 + 2×button_padding, interact_size)`；只读值必须同高，
        // 否则真行与灰行并排是两条不等高的横条（实测差 2 点）。
        egui::__run_test_ui(|ui| {
            let line_h = ui.fonts(|f| f.row_height(&egui::TextStyle::Button.resolve(ui.style())));
            let want = (line_h + 2.0 * ui.spacing().button_padding.y)
                .max(ui.spacing().interact_size.y)
                .max(FIELD_H);
            assert_eq!(field_height(ui), want);
            // 下限就是两个弹窗族共用的那个高度：设置页按公式算、会话弹窗直接
            // `add_sized(FIELD_H)`，两条路必须落在同一个数上。
            assert!(field_height(ui) >= FIELD_H);
        });
    }
}

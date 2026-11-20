//! hex 视图的**绘制**：表头徽标 + 逐行 dump + 底部药丸。
//!
//! # 分工（为什么单独一个文件）
//!
//! 本文件**只画**：只吃 `&RawLog`、几何、调色板与字体，手上没有
//! `&mut TerminalWindow`，也读不到 `ViewState`。模式是什么、滚到哪、字号多大，
//! 全部由 `pane.rs` 算好后传进来。于是这里不可能偷偷改窗口状态，也不可能
//! 成为第二个写入者；排版与状态在 `bin_spec.rs`（零 `egui`，可单测）。
//!
//! # 为什么这里没有「切换」
//!
//! 切到 Bin 不在这里发生，也不清任何东西：字节流从标签创建那一刻起就在
//! `tab.raw` 里攒着（见 `bin_spec` 模块头）。这里只回答一个问题——把已经收到
//! 的那段字节，按当前参数画成一张 dump 表。

use crate::ui::*;
use super::bin_spec::{self, RawLog};
use super::hex_params::{byte_char_at, HexParams};
use term_app::i18n::Lang;

/// 斑马线那层底色的透明度（前景色的 alpha 系数）。
///
/// 定得这么淡是有意的：它的作用是让人**按 4 字节数位置**，不是装饰。
/// 再深一点就会和正文抢注意力，长时间盯着看反而更累；调到 0.04 时在深色底上
/// 已经能看出列块，又不至于让人以为那几个字节被选中了。
///
/// 与 Text 页的「隔行底色」(`view.rs`) 是两套东西：那个按**行**、这个按**组**，
/// 两者可以同时开着。
const ZEBRA_ALPHA: f32 = 0.06;

/// 斑马线那层底色本身：前景色按 [`ZEBRA_ALPHA`] 调暗。
///
/// 抽出来是为了让**唯一一份**定义同时被绘制和单测用到：`zebra_scope_tests`
/// 要数「这一帧画了几块底色」，判据只能是「填充色等于这个值」。测试自己再算
/// 一遍 `gamma_multiply(0.06)` 的话，常量一改测试就变成在验证自己。
pub(super) fn zebra_tint(fg: egui::Color32) -> egui::Color32 {
    fg.gamma_multiply(ZEBRA_ALPHA)
}

/// 表头高度：一行字 + 一点留白。表头之下才是字节行。
///
/// 表头不叠在字节行上：dump 的第一行几乎总是用户最想看的那一行（最新数据），
/// 在它上面压一个半透明徽标等于把它遮掉。
fn header_h(cell_h: f32) -> f32 {
    cell_h + 6.0
}

/// 表尾高度：底部那颗药丸占的地方。
///
/// 与表头同一条理由——药丸若浮在最后一行字节上，遮住的正是**最新**那几个字节，
/// 而 hex 视图最常见的用法就是盯尾巴。所以宁可少画一行字节，也要给它一条
/// 自己的带子。
fn footer_h(cell_h: f32) -> f32 {
    // `+10` 而不是 `+4`：药丸本身是「一行字 + 上下各 4pt 内边距 + 1pt 描边」，
    // 留得比它矮一点，最后一行字节就会被它压住一格——那正是这条带子要避免的事。
    cell_h + 10.0
}

/// 一屏能画几行 hex（**不含**表头与表尾）。
///
/// 绘制与滚轮都要这个数，所以只能有一份算法：两处各算一次的话，「滚一行」
/// 与「画一行」会以不同的行高理解同一批字节，滚到底时最后一行会跳。
pub(in crate::ui) fn view_rows(avail_h: f32, cell_h: f32) -> usize {
    let cell_h = cell_h.max(1.0);
    let body = (avail_h - header_h(cell_h) - footer_h(cell_h)).max(cell_h);
    ((body / cell_h).floor() as usize).max(1)
}

/// 复制用的文本：**看得见的那一屏**，与画出来的行同源（都走 `visible`）。
///
/// `Cmd+C` 在 Bin 下复制的就是这个：用户看不见的字节不进剪贴板，
/// 与 Text 下「只复制选区内看得见的行」是同一条规矩。
pub(in crate::ui) fn copy_text(
    log: &RawLog,
    avail_h: f32,
    cell_h: f32,
    params: HexParams,
) -> String {
    bin_spec::visible_hex_text(log, view_rows(avail_h, cell_h), params)
}

/// 画一屏 hex。
///
/// 参数全是只读输入：`rect` 是可用区，`cell` 是等宽字的一个格子，
/// `params` 是**活动标签**的排版（`View ▸ Hex *` 改它、不落盘），
/// `wanted_columns` 是夹取前的列数，`zebra` 是「相邻加宽组交替底色」
/// （设置 ▸ Binary 页那一格），`palette` / `font_id` 是主题与字体。
/// 调用方（`pane.rs`）已经把「该不该画」判完了。
#[allow(clippy::too_many_arguments)]
pub(in crate::ui) fn paint(
    painter: &egui::Painter,
    log: &RawLog,
    rect: Rect,
    cell: Vec2,
    params: HexParams,
    wanted_columns: usize,
    zebra: bool,
    palette: &Palette,
    font_id: &egui::FontId,
    lang: Lang,
) {
    let fg = to_color32(palette.default_fg);
    // 表头与药丸用弱化色：它们是说明，不是内容；与字节列同色会让人以为
    // 「BIN · 16 bytes/row」也是数据的一部分。
    let dim = fg.gamma_multiply(0.62);
    let left = rect.min.x + GRID_PAD;
    let top = rect.min.y + GRID_PAD;
    let avail_h = (rect.height() - GRID_PAD * 2.0).max(cell.y * 2.0);
    let rows = view_rows(avail_h, cell.y);

    // 表头左侧：当前排版。用户改过 Base / Columns / Group 之后，这张表
    // 按什么排必须写在明面上——菜单已经不画勾了，这里是**唯一**的读数。
    let mut head = format!(
        "{} · {} {} · {} {} · {} {}",
        t(lang, "BIN"),
        params.columns,
        t(lang, "bytes/row"),
        t(lang, "base"),
        params.base,
        t(lang, "group"),
        params.group
    );
    // 列数被窗宽夹过时点名：不说的话用户会以为 `32 Columns` 没生效。
    // 括号里写 prefs 里的原值（`wanted_columns`），不是夹完的值。
    if params.columns != wanted_columns {
        head = format!(
            "{head} ({wanted_columns} {})",
            t(lang, "needs a wider pane")
        );
    }
    painter.text(
        egui::pos2(left, top),
        egui::Align2::LEFT_TOP,
        head,
        font_id.clone(),
        dim,
    );

    // 表头右侧：只在**真丢过**字节时出现。环上限是 1 MiB，正常会话一辈子
    // 见不到它；见到了就说明有洪水，而用户看到的偏移已经不从 0 开始了——
    // 这一条是「屏幕上的偏移为什么从一个六位数开头」的唯一解释。
    // （这里不写具体的十六进制地址：R12 禁止仓库里出现八位以上的地址字面，
    //   那是 Mach-O / 调试符号里才会有的东西，写进注释会被合规检查判红。）
    let dropped = log.dropped();
    if dropped > 0 {
        let gone = format!("{} {}", dropped, t(lang, "bytes dropped"));
        painter.text(
            egui::pos2(rect.max.x - GRID_PAD, top),
            egui::Align2::RIGHT_TOP,
            gone,
            font_id.clone(),
            dim,
        );
    }

    // 字节行：一行一个字符串，整行一次 `text()`。hex 表本来就是等宽的，
    // 拆成「偏移列 + 字节列」分别画只会多出两处对齐计算。
    //
    // 底色（斑马线）必须先画：它是背景，画在字之后会盖住字节。位置由
    // `hex_params::byte_char_at` 给出——与 `row_text` 共用同一个 `sep_after`，
    // 所以底色块与字节永远对得上（各写一遍的话，改分隔符规则时底色会错一格）。
    let body_top = top + header_h(cell.y);
    let stripe = if zebra { Some(zebra_tint(fg)) } else { None };
    for (i, (offset, bytes)) in log.visible(rows, params).iter().enumerate() {
        let y = body_top + i as f32 * cell.y;
        if let Some(tint) = stripe {
            // 组号 = 第几个 `division` 字节段；奇数段铺底色，于是相邻两段一深一浅。
            //
            // 一段画**一整块**（含段内的一格分隔符），不是逐字节各画一格：逐字节
            // 画的话每两格就留一条缝，一条 16 字节的行会变成 16 条竖条，看不出
            // 「每 4 个字节一段」这件事——而那正是这层底色的全部用途。
            // 段与段之间那条加宽横缝不铺，两段才分得开。
            //
            // **为什么按 `division` 分段、不按 `group`**（用户裁定的定论，别再改）：
            // 这个开关的名字就是 `Alternate column background color`，而「column」
            // 在这套参数里唯一的指称是 `devision columns`。让它跟着 `group` 走就会
            // 出现「只改了分组、没改列宽，底色却变了」的错位——两件事串了线。
            // `group = 8 / division = 4` 时一段 8 个字节连写、中间没有分隔符，于是
            // 色块边界落在连写的数字中间：可读，而且那正是「第几段」的锚点。
            let div = params.division.max(1);
            let mut start = 0usize;
            let mut band = 0usize;
            while start < bytes.len() {
                let end = (start + div).min(bytes.len());
                if !band.is_multiple_of(2) {
                    let x0 = byte_char_at(start, params);
                    // 末字节的起点 + 它自己的位宽 = 这一段的右边界（段内分隔符
                    // 已经算在 `byte_char_at` 的累加里了）。
                    let x1 = byte_char_at(end - 1, params) + params.digits();
                    painter.rect_filled(
                        Rect::from_min_size(
                            egui::pos2(left + x0 as f32 * cell.x, y),
                            Vec2::new((x1 - x0) as f32 * cell.x, cell.y),
                        ),
                        0.0,
                        tint,
                    );
                }
                start = end;
                band += 1;
            }
        }
        let line = bin_spec::row_text(bytes, *offset, params);
        painter.text(
            egui::pos2(left, y),
            egui::Align2::LEFT_TOP,
            line,
            font_id.clone(),
            fg,
        );
    }

    // 底部药丸：**总是**有一条，两种状态各说一件用户此刻最需要知道的事。
    //
    // - 回看中：这是历史、怎么回到跟随（否则以为输出停了，与 Text 下那个
    //   回看药丸同一个理由）；
    // - 贴底：选区与查找在这一屏不可用（不写出来，用户会反复点、反复没反应，
    //   而「点了没反应」正是 R5 要消灭的那种假开关）。
    let pill = if log.anchor.is_some() {
        format!(
            "{}: {} {} · {}",
            t(lang, "Bin history"),
            log.ring.len(),
            t(lang, "bytes"),
            t(lang, "scroll to bottom to follow")
        )
    } else {
        t(
            lang,
            "Selection and Find are unavailable in Bin mode (Mode ▸ Hex Edit to switch back)",
        )
        .to_string()
    };
    let pill_fg = fg.gamma_multiply(0.75);
    pill_text(
        painter,
        rect,
        &pill,
        pill_fg,
        to_color32(palette.default_bg),
        font_id,
    );
}

/// 右下角那颗带边框的药丸。与 `pane.rs` 的回看提示同款画法：加底色是为了
/// 压在最后一行字节上时仍然读得清（第一版直接画字，实测会重叠）。
fn pill_text(
    painter: &egui::Painter,
    rect: Rect,
    text: &str,
    fg: egui::Color32,
    bg: egui::Color32,
    font_id: &egui::FontId,
) {
    let galley = painter.layout_no_wrap(text.to_string(), font_id.clone(), fg);
    let size = galley.size();
    let pad = Vec2::new(8.0, 4.0);
    let max = rect.max - Vec2::new(GRID_PAD, GRID_PAD) - size - pad * 2.0;
    let pill = Rect::from_min_size(max, size + pad * 2.0);
    painter.rect_filled(pill, 4.0, bg);
    painter.rect_stroke(pill, 4.0, egui::Stroke::new(1.0, fg.gamma_multiply(0.35)));
    painter.galley(pill.min + pad, galley, fg);
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 表头吃掉一行字的高度，表尾再留一条给药丸，剩下的才排字节行；
    /// 矮到放不下一行字时至少留一行。
    #[test]
    fn the_header_and_footer_never_eat_the_last_row() {
        // 384pt 高、16pt 一格：表头 22 + 表尾 26，剩 336 → 21 行（向下取整）。
        assert_eq!(view_rows(384.0, 16.0), 21);
        // 矮到比表头还短也要留一行：画不了 22 行不是「一行都不画」的理由。
        assert_eq!(view_rows(16.0, 16.0), 1);
        // 单调：可用高度涨了，行数不会反而变少。
        assert!(view_rows(400.0, 16.0) >= view_rows(320.0, 16.0));
        // 行高变了，同一个高度能排的行数必须跟着变（否则字号改了会画出屏）。
        assert!(view_rows(384.0, 12.0) > view_rows(384.0, 24.0));
    }
}
